//! Objects: the in-TPM model of RSA, ECC and sealed-data objects, the stores for transient and
//! persistent objects, template rules, key derivation and the protection of the private area
//! (slice B of phase B, S7-23). The commands are in `object_cmd.rs`.
//!
//! **Kinds.** A *primary* object is created by `TPM2_CreatePrimary` from the seed of a hierarchy
//! and the template; a *child* object by `TPM2_Create` under a storage parent from the TPM DRBG
//! and is returned as a public area and a private blob that `TPM2_Load` takes back; an *external*
//! object (`TPM2_LoadExternal`) has a public area only and no authorization value.
//!
//! **Types.** RSA-2048 (exponent 65537) with RSASSA-PKCS1-v1_5 and ECC NIST P-256 with ECDSA, both
//! with SHA-256 as `nameAlg` and signature hash; a keyed-hash object holds sealed data
//! (`TPM2_Unseal`). Storage keys (`restricted` + `decrypt`, no `sign`) can be parents.
//!
//! **Primary derivation** (Part 1 v184, clauses 25.6.3 and 25.7: "choice of the entropy generation
//! for Primary Objects is a vendor option"). Our choice: `material = KDFa(SHA-256, seed, "ODTPM-
//! PRIMARY", H(template), data, 384 bits)` seeds an HMAC_DRBG (`random.rs`); the seed value and
//! then the key are drawn from it. The same seed, template and `inSensitive.data` give the same
//! object; a new seed gives another. No proof value enters the derivation: an endorsement primary
//! (the EK) is a function of the endorsement seed (EPS) only (Part 1, 12.4.2: the EK changes only
//! with `TPM2_ChangeEPS`). `TPM2_Clear` replaces the storage seed (and so `shProof`, `ehProof`)
//! but not the EPS, so the EK survives it; changing the endorsement auth does not change it
//! either. `TPM2_ChangeEPS` is NOT implemented. Ordinary (non-primary) objects of the
//! endorsement hierarchy are invalidated by `TPM2_Clear` through the flush of the hierarchy.
//! The RSA prime search is that of `rsa`/`crypto-primes` at the pinned versions: a version change
//! may change the primary key for the same inputs (relevant to persistent state, slice C).
//!
//! **Private area.** `outPrivate` follows Part 1 v184, clauses 20.3-20.5: `integrity (TPM2B) ||
//! symIv (TPM2B) || encSensitive`, with `symKey = KDFa(nameAlg, seedValue(parent), "STORAGE",
//! Name, NULL, 128)`, `HMACkey = KDFa(nameAlg, seedValue(parent), "INTEGRITY", NULL, NULL, 256)`
//! and `outerHMAC = HMAC(HMACkey, symIv || encSensitive || Name)`, with the integrity checked
//! before decryption. `encSensitive` is the marshaled `TPM2B_SENSITIVE` encrypted with AES-128 in
//! CFB mode (full-block feedback; crates `aes` and `cfb-mode`), key `symKey` (16 octets), IV the
//! 16-octet `symIv` drawn from the TPM DRBG at `TPM2_Create`. Checked in slice C1 against the text of
//! Part 1 v184, clauses 20.2-20.5 (the labels, the KDFa inputs, the CFB mode of the parent's
//! symmetric cipher with a random `symIv`, the outer HMAC over the marshaled `TPM2B_IV`, the
//! encrypted `size` field of `TPM2B_SENSITIVE`, the integrity value before the IV): the
//! construction matches; blobs of other TPMs were not tried. An RSA key's sensitive field holds both
//! primes `p || q` (256 octets), not one prime (a deviation). The authValue is padded with zero
//! octets to one digest (clause 25.7.3).
//!
//! Secrets (keys, seed values, authValues, sealed data) are wiped on drop and never printed.
//!
//! NOT VERIFIED against TPM 2.0 Library v185/v184 (Part 4, which gives the per-command response
//! codes, was not available; the open Part 3 text names only some codes): the code for a parent
//! that is not a storage key (`TPM_RC_TYPE`, handle 1); `TPM_RC_ATTRIBUTES` for keyed-hash
//! objects with `sign`, `decrypt` or `restricted` (HMAC/XOR keys are not implemented in slice B),
//! `TPM_RC_SCHEME` for scheme/attribute combinations of a template, `TPM_RC_RANGE` for an RSA
//! exponent other than 65537, `TPM_RC_HASH` for a `nameAlg` of NULL in a created object,
//! `TPM_RC_SIZE` for a malformed `inPrivate`, `TPM_RC_SENSITIVE` for a private area that does not
//! unmarshal, `TPM_RC_KEY_SIZE`/`TPM_RC_BINDING` for a sensitive area that does not match the
//! public area, `TPM_RC_POLICY_FAIL` for a password or HMAC session on an object whose `userWithAuth`
//! is CLEAR (policy sessions are required, Part 3 5.6), `TPM_RC_AUTH_UNAVAILABLE` for an object
//! that has no private part, `TPM_RC_HIERARCHY` for
//! objects of a disabled hierarchy, the Qualified Name input (`parentQN || Name`, both with their
//! `nameAlg` prefix), the `creationHash` input (marshaled `TPMS_CREATION_DATA` without a size), the
//! `name` input of tickets (the Name buffer without a size), the creation PCR digest (SHA-256
//! over the selected PCR values in index order; empty when no PCR is selected) and the capacity
//! limits [`MAX_OBJECTS`] and [`MAX_PERSISTENT`] (choices of this implementation).

use cfb_mode::cipher::KeyIvInit;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::codes::{alg, handle, object_attr};
use crate::crypto::{
    EccPrivate, RsaPrivate, ecc_from_scalar, ecc_generate, ecc_point_valid, ecc_public, ecc_scalar,
    ecc_verify, hmac_sha256, hmac_sha256_verify, kdfa_sha256, rsa_from_one_prime, rsa_from_primes,
    rsa_generate, rsa_modulus, rsa_primes, rsa_verify, supported_rsa_exponent,
};
use crate::hash::{AuthValue, MAX_SEQUENCES};
use crate::marshal::{
    MAX_ECC_BYTES, MAX_RSA_BYTES, MAX_SYM_DATA, PublicParams, Reader, SymDef, TpmsSensitiveCreate,
    TpmtPublic, TpmtSignature, Unique, Writer, limits,
};
use crate::pcr::Pcrs;
use crate::random::Drbg;
use crate::response::Rc;

/// Transient objects held at once (other than sequence objects, which have their own slots).
pub const MAX_OBJECTS: usize = 8;
/// Persistent objects held at once.
pub const MAX_PERSISTENT: usize = 16;
/// Largest `TPM2B_PRIVATE` accepted by `TPM2_Load` (ours is below 400 octets).
pub const MAX_PRIVATE: usize = 512;

/// Handle of the transient object in `slot`: after the sequence slots `0x80000000..`.
pub(crate) fn object_handle(slot: usize) -> u32 {
    (u32::from(handle::HT_TRANSIENT) << 24) | (MAX_SEQUENCES + slot) as u32
}

/// Slot of a transient object handle, if the handle lies in the object range.
fn object_slot(h: u32) -> Option<usize> {
    if handle::handle_type(h) != handle::HT_TRANSIENT {
        return None;
    }
    let n = (h & 0x00FF_FFFF) as usize;
    n.checked_sub(MAX_SEQUENCES).filter(|s| *s < MAX_OBJECTS)
}

/// The type-specific private value.
#[derive(Clone)]
pub(crate) enum Secret {
    Rsa(Box<RsaPrivate>),
    Ecc(EccPrivate),
    /// Sealed data.
    Data(Zeroizing<Vec<u8>>),
}

/// `TPMT_SENSITIVE`: authValue, seedValue and the private value.
#[derive(Clone)]
pub(crate) struct Sensitive {
    pub auth: AuthValue,
    /// Protection seed of a storage key, obfuscation value of sealed data, empty otherwise.
    pub seed: Zeroizing<Vec<u8>>,
    pub secret: Secret,
}

/// A loaded object. `sensitive` is `None` for an external (public-only) object.
#[derive(Clone)]
pub(crate) struct Object {
    pub public: TpmtPublic,
    pub name: Vec<u8>,
    pub qualified_name: Vec<u8>,
    /// Hierarchy of the object: owner (storage), platform, endorsement or null.
    pub hierarchy: u32,
    pub sensitive: Option<Sensitive>,
}

/// `nameAlg || H(TPMT_PUBLIC)` (Part 1, clause 14).
pub(crate) fn name_of(public: &TpmtPublic) -> Vec<u8> {
    let mut n = alg::SHA256.to_be_bytes().to_vec();
    n.extend_from_slice(&Sha256::digest(public.to_bytes()));
    n
}

/// `nameAlg || H(parentQN || Name)` (Part 1, clause 24.5).
pub(crate) fn qualified_name_of(parent_qn: &[u8], name: &[u8]) -> Vec<u8> {
    let mut qn = alg::SHA256.to_be_bytes().to_vec();
    let mut h = Sha256::new();
    h.update(parent_qn);
    h.update(name);
    qn.extend_from_slice(&h.finalize());
    qn
}

impl Object {
    /// Verifies a digest with the public key. Each command checks its own usage and scheme rules.
    pub(crate) fn verify_signature(&self, digest: &[u8], signature: &TpmtSignature) -> bool {
        match (&self.public.unique, signature) {
            (Unique::Rsa(n), TpmtSignature::Rsassa { sig, .. }) => rsa_verify(n, digest, sig),
            (Unique::Ecc { x, y }, TpmtSignature::Ecdsa { r, s, .. }) => {
                ecc_verify(x, y, digest, r, s)
            }
            _ => false,
        }
    }
    /// Builds an object; `parent_qn` is the Qualified Name of the parent, or the hierarchy
    /// handle for a primary (Part 1, clause 24.5).
    pub(crate) fn new(
        public: TpmtPublic,
        sensitive: Option<Sensitive>,
        hierarchy: u32,
        parent_qn: &[u8],
    ) -> Object {
        let name = name_of(&public);
        let qualified_name = if sensitive.is_none() {
            // An external object's Qualified Name is its Name (Part 3, clause 12.3.1).
            name.clone()
        } else {
            qualified_name_of(parent_qn, &name)
        };
        Object {
            public,
            name,
            qualified_name,
            hierarchy,
            sensitive,
        }
    }

    pub(crate) fn has(&self, bit: u32) -> bool {
        self.public.attributes & bit != 0
    }

    /// A storage key: an asymmetric `restricted` `decrypt` key that is not a signing key (Part 2,
    /// clause 8.3.3.13); only such a key can be a parent.
    pub(crate) fn is_storage(&self) -> bool {
        is_storage(&self.public)
    }
}

fn is_storage(public: &TpmtPublic) -> bool {
    let a = public.attributes;
    matches!(
        public.params,
        PublicParams::Rsa { .. } | PublicParams::Ecc { .. }
    ) && a & object_attr::RESTRICTED != 0
        && a & object_attr::DECRYPT != 0
        && a & object_attr::SIGN == 0
}

/// Transient object slots.
#[derive(Default)]
pub(crate) struct Objects {
    slots: [Option<Object>; MAX_OBJECTS],
}

impl Objects {
    pub(crate) fn clear(&mut self) {
        self.slots = Default::default();
    }

    pub(crate) fn free_slots(&self) -> usize {
        self.slots.iter().filter(|s| s.is_none()).count()
    }

    pub(crate) fn loaded_handles(&self) -> Vec<u32> {
        (0..MAX_OBJECTS)
            .filter(|i| self.slots[*i].is_some())
            .map(object_handle)
            .collect()
    }

    pub(crate) fn get(&self, h: u32) -> Option<&Object> {
        object_slot(h).and_then(|s| self.slots[s].as_ref())
    }

    /// Stores `o` in the lowest free slot and returns its handle; `None` when all slots are used.
    pub(crate) fn insert(&mut self, o: Object) -> Option<u32> {
        let slot = self.slots.iter().position(Option::is_none)?;
        self.slots[slot] = Some(o);
        Some(object_handle(slot))
    }

    /// Flushes the object with handle `h`; `false` if none is loaded there.
    pub(crate) fn release(&mut self, h: u32) -> bool {
        match object_slot(h) {
            Some(s) => self.slots[s].take().is_some(),
            None => false,
        }
    }

    /// Flushes every object of `hierarchy`.
    pub(crate) fn flush_hierarchy(&mut self, hierarchy: u32) {
        for s in &mut self.slots {
            if s.as_ref().is_some_and(|o| o.hierarchy == hierarchy) {
                *s = None;
            }
        }
    }
}

/// Persistent objects (`TPM2_EvictControl`). They survive `_TPM_Init` and `Startup` and are in the
/// snapshot with their private parts (`snapshot.rs`).
#[derive(Default)]
pub(crate) struct PersistentObjects {
    items: Vec<(u32, Object)>,
}

impl PersistentObjects {
    pub(crate) fn get(&self, h: u32) -> Option<&Object> {
        self.items.iter().find(|(k, _)| *k == h).map(|(_, o)| o)
    }

    pub(crate) fn handles(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.items.iter().map(|(h, _)| *h).collect();
        v.sort_unstable();
        v
    }

    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    /// The objects in handle order, for the snapshot.
    pub(crate) fn sorted(&self) -> Vec<(u32, &Object)> {
        let mut v: Vec<(u32, &Object)> = self.items.iter().map(|(h, o)| (*h, o)).collect();
        v.sort_unstable_by_key(|(h, _)| *h);
        v
    }

    /// Rebuilds the store from restored items; `None` for a duplicate handle or too many items.
    pub(crate) fn from_items(items: Vec<(u32, Object)>) -> Option<PersistentObjects> {
        if items.len() > MAX_PERSISTENT {
            return None;
        }
        for (i, (h, _)) in items.iter().enumerate() {
            if items[..i].iter().any(|(k, _)| k == h) {
                return None;
            }
        }
        Some(PersistentObjects { items })
    }

    pub(crate) fn is_full(&self) -> bool {
        self.items.len() >= MAX_PERSISTENT
    }

    pub(crate) fn insert(&mut self, h: u32, o: Object) {
        self.items.push((h, o));
    }

    pub(crate) fn remove(&mut self, h: u32) {
        self.items.retain(|(k, _)| *k != h);
    }

    /// Evicts every object of `hierarchy` (`TPM2_Clear`).
    pub(crate) fn flush_hierarchy(&mut self, hierarchy: u32) {
        self.items.retain(|(_, o)| o.hierarchy != hierarchy);
    }
}

/// The object a handle names: a transient or a persistent object.
pub(crate) fn find_object<'a>(
    objects: &'a Objects,
    persistent: &'a PersistentObjects,
    h: u32,
) -> Option<&'a Object> {
    match handle::handle_type(h) {
        handle::HT_TRANSIENT => objects.get(h),
        handle::HT_PERSISTENT => persistent.get(h),
        _ => None,
    }
}

// ------------------------------------------------------------------------------ template rules

/// Checks a template of `TPM2_Create`/`TPM2_CreatePrimary` or the public area of `TPM2_Load`
/// against the attribute rules of Part 2 v184, clause 8.3.3 and the creation text of Part 3,
/// clause 12.1. `parent_fixed_tpm`/`parent_enc_dup` describe the parent (a primary's parent is
/// fixedTPM). The public area is parameter 2 of every caller.
pub(crate) fn check_public(
    public: &TpmtPublic,
    parent_fixed_tpm: bool,
    parent_enc_dup: bool,
) -> Result<(), Rc> {
    let bad = |rc: Rc| rc.in_param(2);
    if public.name_alg != alg::SHA256 {
        return Err(bad(Rc::HASH));
    }
    let a = public.attributes;
    let set = |bit: u32| a & bit != 0;
    if set(object_attr::FIRMWARE_LIMITED | object_attr::SVN_LIMITED) {
        return Err(bad(Rc::ATTRIBUTES));
    }
    if !public.auth_policy.is_empty() && public.auth_policy.len() != limits::MAX_DIGEST_SIZE {
        return Err(bad(Rc::SIZE));
    }
    // fixedTPM / fixedParent (clause 8.3.3.2) and encryptedDuplication (8.3.3.11).
    if parent_fixed_tpm {
        if set(object_attr::FIXED_TPM) != set(object_attr::FIXED_PARENT) {
            return Err(bad(Rc::ATTRIBUTES));
        }
    } else if set(object_attr::FIXED_TPM) {
        return Err(bad(Rc::ATTRIBUTES));
    }
    if set(object_attr::FIXED_TPM) {
        if set(object_attr::ENCRYPTED_DUPLICATION) {
            return Err(bad(Rc::ATTRIBUTES));
        }
    } else if !parent_fixed_tpm && set(object_attr::ENCRYPTED_DUPLICATION) != parent_enc_dup {
        return Err(bad(Rc::ATTRIBUTES));
    }
    let (sign, decrypt, restricted) = (
        set(object_attr::SIGN),
        set(object_attr::DECRYPT),
        set(object_attr::RESTRICTED),
    );
    match &public.params {
        PublicParams::KeyedHash { scheme } => {
            // Sealed data (neither `sign` nor `decrypt`, scheme NULL) or an unrestricted HMAC
            // signing key (`sign`, scheme NULL or HMAC with SHA-256; slice C2). XOR keys
            // (`decrypt`), restricted keyed hashes (they need tickets) and `x509sign` are refused.
            if decrypt || restricted || set(object_attr::X509_SIGN) {
                return Err(bad(Rc::ATTRIBUTES));
            }
            let ok = if sign {
                matches!(scheme.alg, alg::NULL | alg::HMAC)
            } else {
                scheme.alg == alg::NULL
            };
            if !ok {
                return Err(bad(Rc::SCHEME));
            }
        }
        PublicParams::Rsa {
            symmetric,
            scheme,
            exponent,
            ..
        } => {
            check_asymmetric_attributes(public, sign, decrypt, restricted)?;
            if !supported_rsa_exponent(*exponent) {
                return Err(bad(Rc::RANGE));
            }
            check_symmetric(public, *symmetric)?;
            let ok = match (sign, decrypt, restricted) {
                // sign + decrypt: scheme NULL; unrestricted signing: RSASSA or NULL; restricted
                // signing: RSASSA; restricted decrypt: NULL; unrestricted decrypt: RSAES, OAEP or
                // NULL (Part 2, Table 215; slice C1).
                (true, true, _) | (false, true, true) => scheme.alg == alg::NULL,
                (false, true, false) => {
                    matches!(scheme.alg, alg::NULL | alg::RSAES | alg::OAEP)
                }
                (true, false, false) => scheme.alg == alg::RSASSA || scheme.alg == alg::NULL,
                (true, false, true) => scheme.alg == alg::RSASSA,
                (false, false, _) => false,
            };
            if !ok {
                return Err(bad(Rc::SCHEME));
            }
        }
        PublicParams::Ecc {
            symmetric, scheme, ..
        } => {
            check_asymmetric_attributes(public, sign, decrypt, restricted)?;
            check_symmetric(public, *symmetric)?;
            // A signing key needs a signing scheme (Part 2, Table 216); an unrestricted decryption
            // key (`TPM2_ECDH_ZGen`) ECDH or NULL; all others NULL.
            let ok = if sign && !decrypt {
                scheme.alg == alg::ECDSA
            } else if decrypt && !sign && !restricted {
                matches!(scheme.alg, alg::NULL | alg::ECDH)
            } else {
                scheme.alg == alg::NULL
            };
            if !ok {
                return Err(bad(Rc::SCHEME));
            }
        }
    }
    Ok(())
}

fn check_asymmetric_attributes(
    public: &TpmtPublic,
    sign: bool,
    decrypt: bool,
    restricted: bool,
) -> Result<(), Rc> {
    // Neither sign nor decrypt (Part 3, clause 12.1.1); restricted needs one of them and cannot
    // be both (Part 2, clauses 8.3.3.12 and 8.3.3.14); x509sign needs sign (8.3.3.15).
    let bad_attrs = (!sign && !decrypt)
        || (restricted && sign && decrypt)
        || (public.attributes & object_attr::X509_SIGN != 0 && !sign);
    if bad_attrs {
        return Err(Rc::ATTRIBUTES.in_param(2));
    }
    Ok(())
}

fn check_symmetric(public: &TpmtPublic, symmetric: SymDef) -> Result<(), Rc> {
    // A restricted decryption (storage) key states AES-128-CFB, every other key TPM_ALG_NULL
    // (Part 2, Tables 215 and 216).
    let want = if is_storage(public) {
        SymDef::Aes128Cfb
    } else {
        SymDef::Null
    };
    if symmetric != want {
        return Err(Rc::SYMMETRIC.in_param(2));
    }
    Ok(())
}

/// The rules that tie the template to the creation data: `sensitiveDataOrigin` is SET exactly
/// when `inSensitive.data` is empty (Part 3, clause 12.1.1), and an asymmetric key takes no data
/// (`TPM_RC_VALUE`, parameter 1).
pub(crate) fn check_creation(public: &TpmtPublic, sens: &TpmsSensitiveCreate) -> Result<(), Rc> {
    let origin = public.attributes & object_attr::SENSITIVE_DATA_ORIGIN != 0;
    match public.params {
        PublicParams::KeyedHash { .. } => {
            // A sealed-data object with no data is invalid (sign and decrypt are CLEAR), and its
            // data come from the caller. An HMAC key (`sign`) takes the caller's key and no
            // `sensitiveDataOrigin`, or the TPM generates one (data empty, origin SET).
            let hmac_key = public.attributes & object_attr::SIGN != 0;
            let valid = if hmac_key {
                sens.data.is_empty() == origin
            } else {
                !sens.data.is_empty() && !origin
            };
            if !valid {
                return Err(Rc::ATTRIBUTES.in_param(2));
            }
        }
        _ => {
            if !sens.data.is_empty() {
                return Err(Rc::VALUE.in_param(1));
            }
            if !origin {
                return Err(Rc::ATTRIBUTES.in_param(2));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------- key derivation

/// A public area with its `unique` filled in and the matching sensitive area.
pub(crate) struct Derived {
    pub public: TpmtPublic,
    pub sensitive: Sensitive,
}

fn new_seed(rng: &mut Drbg) -> Result<Zeroizing<Vec<u8>>, Rc> {
    let a = rng.generate_array().ok_or(Rc::FAILURE)?;
    Ok(Zeroizing::new(a.to_vec()))
}

/// Creates the private and public values of a template from `rng` (the TPM DRBG for an ordinary
/// object, a derivation DRBG for a primary). The seed value comes first, then the key.
pub(crate) fn derive_object(
    template: &TpmtPublic,
    user_auth: &[u8],
    data: &[u8],
    rng: &mut Drbg,
) -> Result<Derived, Rc> {
    let mut public = template.clone();
    let auth = AuthValue::from_slice_trimmed(user_auth).ok_or(Rc::SIZE.in_param(1))?;
    let storage = is_storage(template);
    let (secret, unique, seed) = match &template.params {
        PublicParams::Rsa { .. } => {
            let seed = if storage {
                new_seed(rng)?
            } else {
                Zeroizing::new(Vec::new())
            };
            let key = rsa_generate(rng).ok_or(Rc::FAILURE)?;
            let n = rsa_modulus(&key);
            (Secret::Rsa(Box::new(key)), Unique::Rsa(n), seed)
        }
        PublicParams::Ecc { .. } => {
            let seed = if storage {
                new_seed(rng)?
            } else {
                Zeroizing::new(Vec::new())
            };
            let key = ecc_generate(rng).ok_or(Rc::FAILURE)?;
            let (x, y) = ecc_public(&key).ok_or(Rc::FAILURE)?;
            (Secret::Ecc(key), Unique::Ecc { x, y }, seed)
        }
        PublicParams::KeyedHash { .. } => {
            let seed = new_seed(rng)?;
            // An HMAC key without caller data is generated: one digest of random octets.
            let data: Zeroizing<Vec<u8>> = if data.is_empty() {
                new_seed(rng)?
            } else {
                Zeroizing::new(data.to_vec())
            };
            // unique = H(obfuscate || key) (Part 1, Equation 48).
            let mut h = Sha256::new();
            h.update(&seed[..]);
            h.update(&data[..]);
            let unique = Unique::Digest(h.finalize().to_vec());
            (Secret::Data(data), unique, seed)
        }
    };
    public.unique = unique;
    Ok(Derived {
        public,
        sensitive: Sensitive { auth, seed, secret },
    })
}

/// The DRBG of a primary object: seeded from the hierarchy seed, the hash of the template and
/// `inSensitive.data` (see the module comment). Nothing else enters the derivation: in particular
/// no proof value, so the endorsement primary is a function of the endorsement seed alone.
pub(crate) fn primary_drbg(seed: &[u8], template: &TpmtPublic, data: &[u8]) -> Result<Drbg, Rc> {
    let template_hash = Sha256::digest(template.to_bytes());
    let material = kdfa_sha256(seed, b"ODTPM-PRIMARY", &template_hash, data, 384);
    Drbg::new(&material).ok_or(Rc::FAILURE)
}

// ------------------------------------------------------------------------------ sensitive area

/// Pads the authValue with zero octets to one digest (Part 1, clause 25.7.3).
fn padded_auth(a: &AuthValue) -> [u8; limits::MAX_DIGEST_SIZE] {
    let mut out = [0u8; limits::MAX_DIGEST_SIZE];
    out[..a.as_slice().len()].copy_from_slice(a.as_slice());
    out
}

/// `TPM2B_SENSITIVE` of the object (Table 225): `size || sensitiveType || authValue || seedValue
/// || sensitive`.
pub(crate) fn sensitive_to_bytes(s: &Sensitive, type_alg: u16) -> Result<Zeroizing<Vec<u8>>, Rc> {
    sensitive_with_auth(s, type_alg, &s.auth)
}

fn sensitive_with_auth(
    s: &Sensitive,
    type_alg: u16,
    auth: &AuthValue,
) -> Result<Zeroizing<Vec<u8>>, Rc> {
    let secret: Zeroizing<Vec<u8>> = match &s.secret {
        Secret::Rsa(k) => rsa_primes(k).ok_or(Rc::FAILURE)?,
        Secret::Ecc(k) => ecc_scalar(k),
        Secret::Data(d) => d.clone(),
    };
    // Reserved up front so that the secret is never copied by a reallocation.
    let mut w = Writer {
        buf: Vec::with_capacity(2 + 2 + 34 + 34 + 2 + MAX_RSA_BYTES + 16),
    };
    let mut body = Writer {
        buf: Vec::with_capacity(2 + 34 + 34 + 2 + MAX_RSA_BYTES + 16),
    };
    body.u16(type_alg);
    let auth = Zeroizing::new(padded_auth(auth));
    body.tpm2b(&auth[..]);
    body.tpm2b(&s.seed);
    body.tpm2b(&secret);
    w.tpm2b(&body.buf);
    zeroize::Zeroize::zeroize(&mut body.buf);
    Ok(Zeroizing::new(w.buf))
}

/// Unmarshals a `TPM2B_SENSITIVE` and checks it against `public` (Part 3, clause 12.2.1):
/// `TPM_RC_SENSITIVE` if it does not unmarshal, `TPM_RC_TYPE` for another `sensitiveType`,
/// `TPM_RC_KEY_SIZE` for a private value of the wrong size, `TPM_RC_BINDING` if the private value
/// does not belong to the public area.
pub(crate) fn sensitive_from_bytes(bytes: &[u8], public: &TpmtPublic) -> Result<Sensitive, Rc> {
    read_sensitive(bytes, public, false)
}

/// A duplication blob carries canonical TPM sensitive fields. The local
/// private object envelope retains its existing two-prime RSA layout.
pub(crate) fn sensitive_from_duplicate(bytes: &[u8], public: &TpmtPublic) -> Result<Sensitive, Rc> {
    read_sensitive(bytes, public, true)
}

fn read_sensitive(bytes: &[u8], public: &TpmtPublic, duplicate: bool) -> Result<Sensitive, Rc> {
    type Raw = (
        u16,
        Zeroizing<Vec<u8>>,
        Zeroizing<Vec<u8>>,
        Zeroizing<Vec<u8>>,
    );
    let parse = || -> Result<Raw, Rc> {
        let mut r = Reader::new(bytes);
        let n = usize::from(r.u16()?);
        let mut b = r.sub(n)?;
        if r.remaining() != 0 {
            return Err(Rc::SIZE);
        }
        let t = b.u16()?;
        let auth = Zeroizing::new(b.tpm2b(limits::MAX_DIGEST_SIZE)?.to_vec());
        let seed = Zeroizing::new(b.tpm2b(limits::MAX_DIGEST_SIZE)?.to_vec());
        let secret = Zeroizing::new(b.tpm2b(MAX_RSA_BYTES)?.to_vec());
        b.finish()?;
        Ok((t, auth, seed, secret))
    };
    let (t, auth, seed, secret) = parse().map_err(|_| Rc::SENSITIVE)?;
    if t != public.type_alg() {
        return Err(Rc::TYPE);
    }
    let auth = AuthValue::from_slice_trimmed(&auth).ok_or(Rc::SENSITIVE)?;
    if duplicate {
        let storage = object_attr::RESTRICTED | object_attr::DECRYPT;
        let required_seed = if matches!(public.params, PublicParams::KeyedHash { .. }) {
            limits::MAX_DIGEST_SIZE
        } else if public.attributes & storage == storage {
            limits::MAX_DIGEST_SIZE / 2
        } else {
            0
        };
        if seed.len() < required_seed {
            return Err(Rc::KEY_SIZE);
        }
    }
    let secret = match &public.params {
        PublicParams::Rsa { exponent, .. } => {
            let Unique::Rsa(n) = &public.unique else {
                return Err(Rc::BINDING);
            };
            let size = if duplicate {
                MAX_RSA_BYTES / 2
            } else {
                MAX_RSA_BYTES
            };
            if secret.len() != size {
                return Err(Rc::KEY_SIZE);
            }
            if !supported_rsa_exponent(*exponent) {
                return Err(Rc::BINDING);
            }
            let key = if duplicate {
                rsa_from_one_prime(&secret, n)
            } else {
                rsa_from_primes(&secret, n)
            };
            Secret::Rsa(Box::new(key.ok_or(Rc::BINDING)?))
        }
        PublicParams::Ecc { .. } => {
            let Unique::Ecc { x, y } = &public.unique else {
                return Err(Rc::BINDING);
            };
            if secret.is_empty()
                || secret.len() > MAX_ECC_BYTES
                || (!duplicate && secret.len() != MAX_ECC_BYTES)
            {
                return Err(Rc::KEY_SIZE);
            }
            let mut padded = Zeroizing::new([0u8; MAX_ECC_BYTES]);
            padded[MAX_ECC_BYTES - secret.len()..].copy_from_slice(&secret);
            let key = ecc_from_scalar(&padded[..]).ok_or(Rc::BINDING)?;
            // The public point must be the scalar's public point (clause 12.2.1).
            if ecc_public(&key).as_ref() != Some(&(x.clone(), y.clone())) {
                return Err(Rc::BINDING);
            }
            Secret::Ecc(key)
        }
        PublicParams::KeyedHash { .. } => {
            if secret.len() > MAX_SYM_DATA {
                return Err(Rc::KEY_SIZE);
            }
            let Unique::Digest(u) = &public.unique else {
                return Err(Rc::BINDING);
            };
            let mut h = Sha256::new();
            h.update(&seed[..]);
            h.update(&secret[..]);
            if h.finalize().as_slice() != u.as_slice() {
                return Err(Rc::BINDING);
            }
            Secret::Data(secret)
        }
    };
    Ok(Sensitive { auth, seed, secret })
}

// ---------------------------------------------------------------------- private area protection

/// AES-128-CFB (full-block feedback, as `TPMT_SYM_DEF_OBJECT` AES/128/CFB states) of `data` under
/// the 16-octet `sym_key` and `iv`; `encrypt` selects the direction. The result has the length
/// of `data`.
pub(crate) fn aes128_cfb(
    sym_key: &[u8],
    iv: &[u8; 16],
    data: &[u8],
    encrypt: bool,
) -> Zeroizing<Vec<u8>> {
    let mut key = Zeroizing::new([0u8; 16]);
    let n = key.len().min(sym_key.len());
    key[..n].copy_from_slice(&sym_key[..n]);
    let mut buf = Zeroizing::new(data.to_vec());
    if encrypt {
        cfb_mode::Encryptor::<aes::Aes128>::new(&(*key).into(), &(*iv).into()).encrypt(&mut buf);
    } else {
        cfb_mode::Decryptor::<aes::Aes128>::new(&(*key).into(), &(*iv).into()).decrypt(&mut buf);
    }
    buf
}

/// The contents of a `TPM2B_PRIVATE` for `sensitive` (a `TPM2B_SENSITIVE`) of the object with
/// `name`, protected under the parent's `seed`: `integrity || symIv || encSensitive` (module
/// comment).
pub(crate) fn protect(seed: &[u8], name: &[u8], sensitive: &[u8], iv: &[u8; 16]) -> Vec<u8> {
    let sym_key = kdfa_sha256(seed, b"STORAGE", name, &[], 128);
    let enc = aes128_cfb(&sym_key, iv, sensitive, true);
    let hmac_key = kdfa_sha256(seed, b"INTEGRITY", &[], &[], 256);
    let mut iv_field = Writer::new();
    iv_field.tpm2b(iv);
    let tag = hmac_sha256(&hmac_key, &[&iv_field.buf, &enc, name]);
    let mut out = Writer::new();
    out.tpm2b(&tag);
    out.bytes(&iv_field.buf);
    out.bytes(&enc);
    out.finish()
}

/// Verifies the integrity of a private area and decrypts it: `TPM_RC_INTEGRITY` before any
/// decryption (Part 3, clause 12.2.1), `TPM_RC_SIZE` for a malformed container.
fn unprotect(seed: &[u8], name: &[u8], blob: &[u8]) -> Result<Zeroizing<Vec<u8>>, Rc> {
    let mut r = Reader::new(blob);
    let tag = r.tpm2b(limits::MAX_DIGEST_SIZE).map_err(|_| Rc::SIZE)?;
    let iv_bytes = r.tpm2b(16).map_err(|_| Rc::SIZE)?;
    let iv: [u8; 16] = iv_bytes.try_into().map_err(|_| Rc::SIZE)?;
    if r.remaining() == 0 {
        return Err(Rc::SIZE);
    }
    let enc = r.bytes(r.remaining()).map_err(|_| Rc::SIZE)?;
    let hmac_key = kdfa_sha256(seed, b"INTEGRITY", &[], &[], 256);
    let mut iv_field = Writer::new();
    iv_field.tpm2b(&iv);
    if !hmac_sha256_verify(&hmac_key, &[&iv_field.buf, enc, name], tag) {
        return Err(Rc::INTEGRITY);
    }
    let sym_key = kdfa_sha256(seed, b"STORAGE", name, &[], 128);
    Ok(aes128_cfb(&sym_key, &iv, enc, false))
}

/// Protects the sensitive area of `obj` for a parent seed (`TPM2_Create`).
pub(crate) fn private_blob(
    obj_public: &TpmtPublic,
    name: &[u8],
    sensitive: &Sensitive,
    auth: &AuthValue,
    parent_seed: &[u8],
    iv: &[u8; 16],
) -> Result<Vec<u8>, Rc> {
    let bytes = sensitive_with_auth(sensitive, obj_public.type_alg(), auth)?;
    Ok(protect(parent_seed, name, &bytes, iv))
}

/// Opens a private blob for `public` under a parent seed (`TPM2_Load`).
pub(crate) fn open_private(
    public: &TpmtPublic,
    parent_seed: &[u8],
    blob: &[u8],
) -> Result<Sensitive, Rc> {
    let name = name_of(public);
    let plain = unprotect(parent_seed, &name, blob)?;
    sensitive_from_bytes(&plain, public)
}

// --------------------------------------------------------------------------- public-only objects

/// The checks of `TPM2_LoadExternal` for a public area without a private part: the point of an
/// ECC key is on the curve (`TPM_RC_ECC_POINT`, Part 3, clause 12.3.1), an RSA modulus is 2048
/// bits and odd (`TPM_RC_KEY`), the exponent is 65537 (`TPM_RC_RANGE`), `nameAlg` is SHA-256
/// (`TPM_RC_HASH`), `authPolicy` is empty or one digest (`TPM_RC_SIZE`).
pub(crate) fn check_external_public(public: &TpmtPublic) -> Result<(), Rc> {
    let bad = |rc: Rc| rc.in_param(2);
    if public.name_alg != alg::SHA256 {
        return Err(bad(Rc::HASH));
    }
    if !public.auth_policy.is_empty() && public.auth_policy.len() != limits::MAX_DIGEST_SIZE {
        return Err(bad(Rc::SIZE));
    }
    let a = public.attributes;
    if a & object_attr::X509_SIGN != 0
        && (a & object_attr::SIGN == 0 || matches!(public.params, PublicParams::KeyedHash { .. }))
    {
        return Err(bad(Rc::ATTRIBUTES));
    }
    if a & (object_attr::FIRMWARE_LIMITED | object_attr::SVN_LIMITED) != 0 {
        return Err(bad(Rc::ATTRIBUTES));
    }
    match (&public.params, &public.unique) {
        (PublicParams::Rsa { exponent, .. }, Unique::Rsa(n)) => {
            if !supported_rsa_exponent(*exponent) {
                return Err(bad(Rc::RANGE));
            }
            if n.len() != MAX_RSA_BYTES || n[0] & 0x80 == 0 || n[MAX_RSA_BYTES - 1] & 1 == 0 {
                return Err(bad(Rc::KEY));
            }
        }
        (PublicParams::Ecc { .. }, Unique::Ecc { x, y }) => {
            if !ecc_point_valid(x, y) {
                return Err(bad(Rc::ECC_POINT));
            }
        }
        // Sealed data without its private part is of no use; keep the object model simple.
        (PublicParams::KeyedHash { .. }, _) => return Err(bad(Rc::TYPE)),
        _ => return Err(bad(Rc::KEY)),
    }
    Ok(())
}

// ---------------------------------------------------------------------------------- creation data

/// SHA-256 over the selected PCR values (selection order, ascending index); empty when nothing is
/// selected (Part 2, Table 246: `pcrDigest.size` is zero for an empty selection).
pub(crate) fn creation_pcr_digest(
    pcrs: &Pcrs,
    selection: &crate::marshal::TpmlPcrSelection,
) -> Vec<u8> {
    pcrs.selection_digest(selection)
        .map_or_else(Vec::new, |digest| digest.to_vec())
}

/// Handle types a hierarchy value may take for objects (`TPMI_RH_HIERARCHY+`).
pub(crate) fn is_object_hierarchy(h: u32) -> bool {
    matches!(
        h,
        handle::RH_OWNER | handle::RH_PLATFORM | handle::RH_ENDORSEMENT | handle::RH_NULL
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::marshal::Scheme;

    fn ecc_sign_template() -> TpmtPublic {
        TpmtPublic {
            name_alg: alg::SHA256,
            attributes: object_attr::FIXED_TPM
                | object_attr::FIXED_PARENT
                | object_attr::SENSITIVE_DATA_ORIGIN
                | object_attr::USER_WITH_AUTH
                | object_attr::SIGN,
            auth_policy: vec![],
            params: PublicParams::Ecc {
                symmetric: SymDef::Null,
                scheme: Scheme {
                    alg: alg::ECDSA,
                    hash: alg::SHA256,
                },
                curve: crate::codes::curve::NIST_P256,
            },
            unique: Unique::Ecc {
                x: vec![],
                y: vec![],
            },
        }
    }

    #[test]
    fn handle_layout_follows_the_sequence_slots() {
        assert_eq!(object_handle(0), 0x8000_0000 + MAX_SEQUENCES as u32);
        assert_eq!(
            object_slot(object_handle(MAX_OBJECTS - 1)),
            Some(MAX_OBJECTS - 1)
        );
        assert_eq!(object_slot(0x8000_0000), None);
        assert_eq!(object_slot(object_handle(MAX_OBJECTS)), None);
        assert_eq!(object_slot(0x8100_0000), None);
    }

    #[test]
    fn template_rules() {
        let ok = ecc_sign_template();
        assert_eq!(check_public(&ok, true, false), Ok(()));
        // fixedTPM must equal fixedParent under a fixedTPM parent.
        let mut t = ok.clone();
        t.attributes &= !object_attr::FIXED_PARENT;
        assert_eq!(
            check_public(&t, true, false),
            Err(Rc::ATTRIBUTES.in_param(2))
        );
        // Neither sign nor decrypt.
        let mut t = ok.clone();
        t.attributes &= !object_attr::SIGN;
        assert_eq!(
            check_public(&t, true, false),
            Err(Rc::ATTRIBUTES.in_param(2))
        );
        // An ECC signing key needs ECDSA.
        let mut t = ok.clone();
        if let PublicParams::Ecc { scheme, .. } = &mut t.params {
            *scheme = Scheme::NULL;
        }
        assert_eq!(check_public(&t, true, false), Err(Rc::SCHEME.in_param(2)));
        // Restricted signing and decrypting together.
        let mut t = ok.clone();
        t.attributes |= object_attr::RESTRICTED | object_attr::DECRYPT;
        assert_eq!(
            check_public(&t, true, false),
            Err(Rc::ATTRIBUTES.in_param(2))
        );
        // nameAlg NULL.
        let mut t = ok;
        t.name_alg = alg::NULL;
        assert_eq!(check_public(&t, true, false), Err(Rc::HASH.in_param(2)));
    }

    #[test]
    fn protect_round_trip_and_tamper() {
        let seed = [7u8; 32];
        let name = [9u8; 34];
        let sens = vec![1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let blob = protect(&seed, &name, &sens, &[3u8; 16]);
        assert_eq!(&unprotect(&seed, &name, &blob).unwrap()[..], &sens[..]);
        // Another name, another seed, a flipped octet: integrity failure.
        assert_eq!(unprotect(&seed, &[8u8; 34], &blob), Err(Rc::INTEGRITY));
        assert_eq!(unprotect(&[6u8; 32], &name, &blob), Err(Rc::INTEGRITY));
        let mut bad = blob.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert_eq!(unprotect(&seed, &name, &bad), Err(Rc::INTEGRITY));
        // The ciphertext does not contain the plaintext.
        assert!(!blob.windows(sens.len()).any(|w| w == &sens[..]));
        assert_eq!(unprotect(&seed, &name, &[]), Err(Rc::SIZE));
    }

    #[test]
    fn aes128_cfb_matches_the_nist_vectors() {
        // NIST SP 800-38A, F.3.13 (CFB128-AES128.Encrypt), the first two blocks.
        let key = [
            0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf,
            0x4f, 0x3c,
        ];
        let iv: [u8; 16] = core::array::from_fn(|i| i as u8);
        let plain = [
            0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96, 0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93,
            0x17, 0x2a, 0xae, 0x2d, 0x8a, 0x57, 0x1e, 0x03, 0xac, 0x9c, 0x9e, 0xb7, 0x6f, 0xac,
            0x45, 0xaf, 0x8e, 0x51,
        ];
        let cipher = [
            0x3b, 0x3f, 0xd9, 0x2e, 0xb7, 0x2d, 0xad, 0x20, 0x33, 0x34, 0x49, 0xf8, 0xe8, 0x3c,
            0xfb, 0x4a, 0xc8, 0xa6, 0x45, 0x37, 0xa0, 0xb3, 0xa9, 0x3f, 0xcd, 0xe3, 0xcd, 0xad,
            0x9f, 0x1c, 0xe5, 0x8b,
        ];
        assert_eq!(&aes128_cfb(&key, &iv, &plain, true)[..], &cipher[..]);
        assert_eq!(&aes128_cfb(&key, &iv, &cipher, false)[..], &plain[..]);
    }

    #[test]
    fn protect_handles_empty_partial_and_maximum_sizes() {
        let seed = [7u8; 32];
        let name = [9u8; 34];
        let iv = [3u8; 16];
        // Empty sensitive: the container is rejected (`TPM_RC_SIZE`), nothing to decrypt.
        let blob = protect(&seed, &name, &[], &iv);
        assert_eq!(unprotect(&seed, &name, &blob), Err(Rc::SIZE));
        // One octet, partial blocks, whole blocks and the largest blob `TPM2B_PRIVATE` holds
        // (integrity 2 + 32, symIv 2 + 16, the rest encSensitive).
        let largest = MAX_PRIVATE - (2 + 32 + 2 + 16);
        for len in [1usize, 15, 16, 17, 100, 256, largest] {
            let sens: Vec<u8> = (0..len).map(|i| (i * 7 + 1) as u8).collect();
            let blob = protect(&seed, &name, &sens, &iv);
            assert_eq!(blob.len(), 52 + len);
            assert_eq!(&unprotect(&seed, &name, &blob).unwrap()[..], &sens[..]);
            // Any single flipped octet is caught by the integrity check before decryption.
            for at in [2usize, 40, blob.len() - 1] {
                let mut bad = blob.clone();
                bad[at] ^= 0x80;
                assert_eq!(unprotect(&seed, &name, &bad), Err(Rc::INTEGRITY));
            }
        }
        assert_eq!(
            protect(&seed, &name, &vec![0u8; largest], &iv).len(),
            MAX_PRIVATE
        );
    }
}
