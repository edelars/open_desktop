//! Command-level tests of slice B of phase B (S7-23): objects and keys. Every command goes
//! through `Tpm::execute` with raw bytes, like `tests.rs` and `tests_b.rs`.
//!
//! Independent evidence:
//! - the bytes of one `TPMT_PUBLIC` are written by hand from Part 2 v184, Tables 216 and 219;
//! - the RSA and ECDSA verification vectors were produced by OpenSSL 4.1.0 (`openssl genpkey`,
//!   `openssl dgst -sha256 -sign`) on 2026-11-07 and verified with plain Python integer
//!   arithmetic (`pow` for RSASSA-PKCS1-v1_5, an affine-coordinate P-256 ECDSA verification),
//!   not with the crates under test;
//! - the golden ECC primary below was created by this TPM and its signature verified by the same
//!   Python code, then pinned;
//! - hashes (names, creation hash, PCR digest) are recomputed here with `sha2`.
//!
//! Response codes: those in `docs/specs/VTPM.md` marked NOT VERIFIED are the ones the open text
//! (Part 2/3 v184) does not name; the tests pin the behaviour of this implementation.

use sha2::{Digest, Sha256};

use super::tests_b::{hc, pw_cmd, run_hmac, start_hmac};
use super::*;
use crate::codes::{curve, object_attr as oa};
use crate::marshal::{PublicParams, Scheme, SymDef, TpmtPublic, Unique, Writer};
use crate::object::{MAX_OBJECTS, MAX_PERSISTENT};

pub(super) const OWNER: u32 = handle::RH_OWNER;
pub(super) const PLATFORM: u32 = handle::RH_PLATFORM;
pub(super) const ENDORSEMENT: u32 = handle::RH_ENDORSEMENT;
pub(super) const NULL: u32 = handle::RH_NULL;
pub(super) const FIRST_OBJECT: u32 = 0x8000_0003;

// ----------------------------------------------------------------------- vectors (OpenSSL)

const MSG_DIGEST: &str = "e9f7e9b5966f9b269d9f7d388136f3b4ffbfc5ac97b41d36827d574b6b137984";
const RSA_N: &str = "906572f08cff451003fc330dbe0480ae265f33e56ccfa16e6f1626b75101bb0e305782969b20233cda84408d7cc004b74cb32ee483340e1ec2387de72bc55cfc53f8106679333c659637ea158299afa8236027b1e5558b46afa574c8d79fb50b068ebcd557d140fd741a4d0f0157f606299ff96f7bb0f2f1bcd319771b678968f3c616f6888778dd111c4a3e02be35f04d61a787769e2c3d4c3d1a18e56890a2afebefec28e518bfecd99bd63cb9fb88bcc774bcf0b4d62e624ecc4a6a31dcf7f7adcd0dc68b5c407aec9e06b62a5f48f8c7e367611444fe44511f85a5907f0484aa007ee65552c22055b07678bb1eef74fa8329fc114ea916e569a5cc66dc45";
const RSA_SIG: &str = "889851290577be6d58502807bcfff39389af7f6d6b5ea4cbb2c3071b4ecc9c1ec9f90b89ed5b43e8b74a4a6c3daa8f2baf9076a0a3f04071ff83dd53bf3cb490825e653cfdecf9b70a6512d1c99362b996b0425eeb4165e6799fb724cebea2cf1f4d022307b24136d5ecbbe6e42a5ab722ad0bc1226057fbd95796de7c357ecde7ab846bcf845a1169b8a0aa59158b4e826ee3044be008deeb54091345bbbc65db5f0489e1134855f342f56ac636208cbc831248efba5fbc031defbd29290fe220fd265066f198ba14006bb4ac857a04ff65171f3d3d353d0d183f76a8e6172dd2e19f99c73cfe135fb51c8b16e0b339d2003d879fabef8a49bd99bb06ab2be5";
const EC_X: &str = "133971fe14fe45b2ba1a005dd0278f0f3b44ad5c66a73d46a715967b153751b2";
const EC_Y: &str = "d861fbf4e2b49d2d3e8f9d6131b533d70b763a88ff2efb6568cf2dd7c93ce447";
const EC_R: &str = "4fcbe1c9a9cb653bab58f32e409ce33f7cb565b46098f2178f461ab77b25b9ca";
const EC_S: &str = "5041abdc28c66c4d8a10b2efefa9e053e536bbdb8ee227b9aa79a4b894a8cc87";

// ------------------------------------------------------------------------------ templates

pub(super) fn base() -> u32 {
    oa::FIXED_TPM | oa::FIXED_PARENT | oa::SENSITIVE_DATA_ORIGIN | oa::USER_WITH_AUTH
}

pub(super) fn ecc(attributes: u32, symmetric: SymDef, scheme: Scheme) -> TpmtPublic {
    TpmtPublic {
        name_alg: alg::SHA256,
        attributes,
        auth_policy: vec![],
        params: PublicParams::Ecc {
            symmetric,
            scheme,
            curve: curve::NIST_P256,
        },
        unique: Unique::Ecc {
            x: vec![],
            y: vec![],
        },
    }
}

pub(super) fn rsa(attributes: u32, symmetric: SymDef, scheme: Scheme) -> TpmtPublic {
    TpmtPublic {
        name_alg: alg::SHA256,
        attributes,
        auth_policy: vec![],
        params: PublicParams::Rsa {
            symmetric,
            scheme,
            key_bits: 2048,
            exponent: 0,
        },
        unique: Unique::Rsa(vec![]),
    }
}

pub(super) const ECDSA: Scheme = Scheme {
    alg: alg::ECDSA,
    hash: alg::SHA256,
};
pub(super) const RSASSA: Scheme = Scheme {
    alg: alg::RSASSA,
    hash: alg::SHA256,
};

pub(super) fn ecc_sign() -> TpmtPublic {
    ecc(base() | oa::SIGN, SymDef::Null, ECDSA)
}

pub(super) fn rsa_sign() -> TpmtPublic {
    rsa(base() | oa::SIGN, SymDef::Null, RSASSA)
}

pub(super) fn ecc_storage() -> TpmtPublic {
    ecc(
        base() | oa::RESTRICTED | oa::DECRYPT,
        SymDef::Aes128Cfb,
        Scheme::NULL,
    )
}

pub(super) fn rsa_storage() -> TpmtPublic {
    rsa(
        base() | oa::RESTRICTED | oa::DECRYPT,
        SymDef::Aes128Cfb,
        Scheme::NULL,
    )
}

pub(super) fn sealed() -> TpmtPublic {
    TpmtPublic {
        name_alg: alg::SHA256,
        attributes: oa::FIXED_TPM | oa::FIXED_PARENT | oa::USER_WITH_AUTH,
        auth_policy: vec![],
        params: PublicParams::KeyedHash {
            scheme: Scheme::NULL,
        },
        unique: Unique::Digest(vec![]),
    }
}

// ------------------------------------------------------------------------------- commands

pub(super) const NO_PCR: [u8; 4] = [0, 0, 0, 0];
const NULL_VALIDATION: [u8; 8] = [0x80, 0x24, 0x40, 0, 0, 7, 0, 0];

pub(super) fn creation_params(
    auth: &[u8],
    data: &[u8],
    public: &TpmtPublic,
    outside: &[u8],
    pcr: &[u8],
) -> Vec<u8> {
    let sens = cat(&[&tpm2b(auth), &tpm2b(data)]);
    cat(&[
        &tpm2b(&sens),
        &tpm2b(&public.to_bytes()),
        &tpm2b(outside),
        pcr,
    ])
}

pub(super) fn create_primary_raw(t: &mut Tpm, hier: u32, pw: &[u8], params: &[u8]) -> Vec<u8> {
    run(t, with_pw(cc::CREATE_PRIMARY, &[hier], &[pw], params))
}

pub(super) fn create_primary(t: &mut Tpm, hier: u32, auth: &[u8], public: &TpmtPublic) -> Vec<u8> {
    let params = creation_params(auth, b"", public, b"", &NO_PCR);
    create_primary_raw(t, hier, b"", &params)
}

pub(super) fn create_child(
    t: &mut Tpm,
    parent: u32,
    pw: &[u8],
    auth: &[u8],
    data: &[u8],
    public: &TpmtPublic,
) -> Vec<u8> {
    let params = creation_params(auth, data, public, b"", &NO_PCR);
    run(t, with_pw(cc::CREATE, &[parent], &[pw], &params))
}

pub(super) fn load(t: &mut Tpm, parent: u32, pw: &[u8], private: &[u8], public: &[u8]) -> Vec<u8> {
    let params = cat(&[&tpm2b(private), &tpm2b(public)]);
    run(t, with_pw(cc::LOAD, &[parent], &[pw], &params))
}

pub(super) fn sign_params(digest: &[u8], scheme: &[u8]) -> Vec<u8> {
    cat(&[&tpm2b(digest), scheme, &NULL_VALIDATION])
}

pub(super) fn sign(t: &mut Tpm, key: u32, pw: &[u8], digest: &[u8], scheme: &[u8]) -> Vec<u8> {
    run(
        t,
        with_pw(cc::SIGN, &[key], &[pw], &sign_params(digest, scheme)),
    )
}

pub(super) fn verify(t: &mut Tpm, key: u32, digest: &[u8], signature: &[u8]) -> Vec<u8> {
    run(
        t,
        plain(
            cc::VERIFY_SIGNATURE,
            &[key],
            &cat(&[&tpm2b(digest), signature]),
        ),
    )
}

pub(super) fn read_public(t: &mut Tpm, h: u32) -> Vec<u8> {
    run(t, plain(cc::READ_PUBLIC, &[h], &[]))
}

pub(super) fn load_external(t: &mut Tpm, public: &TpmtPublic, hierarchy: u32) -> Vec<u8> {
    run(
        t,
        plain(
            cc::LOAD_EXTERNAL,
            &[],
            &cat(&[&u16b(0), &tpm2b(&public.to_bytes()), &u32b(hierarchy)]),
        ),
    )
}

pub(super) fn evict(t: &mut Tpm, auth: u32, pw: &[u8], object: u32, persistent: u32) -> Vec<u8> {
    run(
        t,
        with_pw(cc::EVICT_CONTROL, &[auth, object], &[pw], &u32b(persistent)),
    )
}

pub(super) fn flush(t: &mut Tpm, h: u32) -> Vec<u8> {
    run(t, plain(cc::FLUSH_CONTEXT, &[], &u32b(h)))
}

// ------------------------------------------------------------------------------- parsing

#[derive(Debug)]
pub(super) struct Primary {
    pub(super) handle: u32,
    pub(super) public: Vec<u8>,
    pub(super) creation: Vec<u8>,
    pub(super) hash: Vec<u8>,
    pub(super) ticket: (u16, u32, Vec<u8>),
    pub(super) name: Vec<u8>,
}

fn read_ticket(r: &mut Reader<'_>) -> (u16, u32, Vec<u8>) {
    let tag = r.u16().unwrap();
    let h = r.u32().unwrap();
    (tag, h, r.tpm2b(64).unwrap().to_vec())
}

pub(super) fn parse_primary(resp: &[u8]) -> Primary {
    let handle = u32::from_be_bytes([resp[10], resp[11], resp[12], resp[13]]);
    let p = params(resp, 1);
    let mut r = Reader::new(p);
    let public = r.tpm2b(limits::MAX_RESPONSE_SIZE).unwrap().to_vec();
    let creation = r.tpm2b(1024).unwrap().to_vec();
    let hash = r.tpm2b(64).unwrap().to_vec();
    let ticket = read_ticket(&mut r);
    let name = r.tpm2b(64).unwrap().to_vec();
    r.finish().unwrap();
    Primary {
        handle,
        public,
        creation,
        hash,
        ticket,
        name,
    }
}

pub(super) struct Child {
    pub(super) private: Vec<u8>,
    pub(super) public: Vec<u8>,
    pub(super) creation: Vec<u8>,
    pub(super) hash: Vec<u8>,
    pub(super) ticket: (u16, u32, Vec<u8>),
}

pub(super) fn parse_child(resp: &[u8]) -> Child {
    let p = params(resp, 0);
    let mut r = Reader::new(p);
    let private = r.tpm2b(1024).unwrap().to_vec();
    let public = r.tpm2b(limits::MAX_RESPONSE_SIZE).unwrap().to_vec();
    let creation = r.tpm2b(1024).unwrap().to_vec();
    let hash = r.tpm2b(64).unwrap().to_vec();
    let ticket = read_ticket(&mut r);
    r.finish().unwrap();
    Child {
        private,
        public,
        creation,
        hash,
        ticket,
    }
}

/// `(handle, name)` of a successful `Load`/`LoadExternal`.
pub(super) fn parse_loaded(resp: &[u8]) -> (u32, Vec<u8>) {
    let h = u32::from_be_bytes([resp[10], resp[11], resp[12], resp[13]]);
    let mut r = Reader::new(params(resp, 1));
    let name = r.tpm2b(64).unwrap().to_vec();
    r.finish().unwrap();
    (h, name)
}

pub(super) fn name_hash(public: &[u8]) -> Vec<u8> {
    [&alg::SHA256.to_be_bytes()[..], &Sha256::digest(public)].concat()
}

pub(super) fn unmarshal_public(bytes: &[u8]) -> TpmtPublic {
    let mut r = Reader::new(bytes);
    let t = TpmtPublic::unmarshal(&mut r).unwrap();
    r.finish().unwrap();
    t
}

/// The private value of a loaded object, for "never leaves the TPM" checks.
pub(super) fn secret_of(t: &Tpm, h: u32) -> Vec<u8> {
    use crate::object::Secret;
    match &t.object(h).unwrap().sensitive.as_ref().unwrap().secret {
        Secret::Rsa(k) => crate::crypto::rsa_primes(k).unwrap().to_vec(),
        Secret::Ecc(k) => crate::crypto::ecc_scalar(k).to_vec(),
        Secret::MlKem(k) => crate::kem::private_seed(k).unwrap().to_vec(),
        Secret::Data(d) => d.to_vec(),
    }
}

pub(super) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// A started TPM with an ECC storage primary under the owner hierarchy.
pub(super) fn with_ecc_parent() -> (Tpm, u32) {
    let mut t = started();
    let p = parse_primary(&create_primary(&mut t, OWNER, b"parent-pw", &ecc_storage()));
    (t, p.handle)
}

pub(super) fn ecc_sig_scheme() -> Vec<u8> {
    cat(&[&u16b(alg::ECDSA), &u16b(alg::SHA256)])
}

pub(super) fn rsa_sig_scheme() -> Vec<u8> {
    cat(&[&u16b(alg::RSASSA), &u16b(alg::SHA256)])
}

#[test]
fn import_accepts_a_single_rsa_prime_and_checks_the_binding() {
    use crate::object::MAX_PRIVATE;
    use crate::self_test::{EXT_RSA_N, EXT_RSA_P, EXT_RSA_Q};

    let (mut t, parent) = with_ecc_parent();
    let mut public = rsa(oa::USER_WITH_AUTH | oa::SIGN, SymDef::Null, RSASSA);
    public.unique = Unique::Rsa(hex(EXT_RSA_N));
    let import = |prime: &[u8]| {
        let sensitive = cat(&[
            &u16b(alg::RSA),
            &tpm2b(b"child"),
            &tpm2b(b""),
            &tpm2b(prime),
        ]);
        let parameters = cat(&[
            &tpm2b(b""),
            &tpm2b(&public.to_bytes()),
            &tpm2b(&tpm2b(&sensitive)),
            &tpm2b(b""),
            &u16b(alg::NULL),
        ]);
        with_pw(cc::IMPORT, &[parent], &[b"parent-pw"], &parameters)
    };
    let prime = hex(EXT_RSA_P);
    let other = hex(EXT_RSA_Q);
    let slots = t.objects.free_slots();
    let reply = run(&mut t, import(&prime));
    ok(&reply);
    assert_eq!(t.objects.free_slots(), slots);
    let mut reader = Reader::new(params(&reply, 0));
    let private = reader.tpm2b(MAX_PRIVATE).unwrap();
    reader.finish().unwrap();
    let reply = load(&mut t, parent, b"parent-pw", private, &public.to_bytes());
    ok(&reply);
    let handle = u32::from_be_bytes(reply[10..14].try_into().unwrap());
    ok(&sign(&mut t, handle, b"child", &DIGEST, &u16b(alg::NULL)));
    ok(&run(&mut t, import(&other)));
    err(
        &run(&mut t, import(&cat(&[&prime, &other]))),
        Rc::KEY_SIZE.in_param(3),
    );
    let mut wrong = prime;
    wrong[127] ^= 2;
    err(&run(&mut t, import(&wrong)), Rc::BINDING.in_param(3));
}

fn null_scheme() -> Vec<u8> {
    u16b(alg::NULL)
}

pub(super) const DIGEST: [u8; 32] = [0x5A; 32];

// ===================================================================== structures

#[test]
fn tpmt_public_layout_matches_hand_written_bytes() {
    // Part 2 v184, Table 219 (TPMT_PUBLIC) with Table 216 (TPMS_ECC_PARMS): type ECC 0x0023,
    // nameAlg SHA-256, attributes fixedTPM|fixedParent|sensitiveDataOrigin|userWithAuth|sign
    // (0x00040072), empty authPolicy, symmetric NULL, scheme ECDSA/SHA-256, curve NIST P-256,
    // kdf NULL, unique = two empty TPM2B_ECC_PARAMETER.
    let bytes: Vec<u8> = vec![
        0x00, 0x23, 0x00, 0x0B, 0x00, 0x04, 0x00, 0x72, 0x00, 0x00, 0x00, 0x10, 0x00, 0x18, 0x00,
        0x0B, 0x00, 0x03, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00,
    ];
    let t = unmarshal_public(&bytes);
    assert_eq!(t, ecc_sign());
    assert_eq!(t.to_bytes(), bytes);
    // RSA (Table 215): symmetric NULL, scheme RSASSA/SHA-256, keyBits 2048, exponent 0, unique =
    // an empty TPM2B_PUBLIC_KEY_RSA.
    let rsa_bytes: Vec<u8> = vec![
        0x00, 0x01, 0x00, 0x0B, 0x00, 0x04, 0x00, 0x72, 0x00, 0x00, 0x00, 0x10, 0x00, 0x14, 0x00,
        0x0B, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    assert_eq!(unmarshal_public(&rsa_bytes), rsa_sign());
    assert_eq!(rsa_sign().to_bytes(), rsa_bytes);
    // A storage key states AES-128-CFB: 0x0006, 128, 0x0043.
    let storage = ecc_storage().to_bytes();
    assert_eq!(&storage[10..16], &[0x00, 0x06, 0x00, 0x80, 0x00, 0x43]);
    assert_eq!(unmarshal_public(&storage), ecc_storage());
}

#[test]
fn tpmt_public_unmarshal_codes() {
    let good = ecc_sign().to_bytes();
    let parse = |b: &[u8]| TpmtPublic::unmarshal(&mut Reader::new(b)).unwrap_err();
    // Object type SYMCIPHER (0x0025) is not implemented: TPM_RC_TYPE (Table 211).
    let mut b = good.clone();
    b[1] = 0x25;
    assert_eq!(parse(&b), Rc::TYPE);
    // nameAlg SHA-1: TPM_RC_HASH (Table 78).
    let mut b = good.clone();
    b[3] = 0x04;
    assert_eq!(parse(&b), Rc::HASH);
    // Reserved attribute bit 0: TPM_RC_RESERVED_BITS.
    let mut b = good.clone();
    b[7] |= 1;
    assert_eq!(parse(&b), Rc::RESERVED_BITS);
    // Curve NIST P-384: TPM_RC_CURVE; KDF other than NULL: TPM_RC_KDF.
    let mut b = good.clone();
    b[17] = 0x04;
    assert_eq!(parse(&b), Rc::CURVE);
    let mut b = good.clone();
    b[19] = 0x20;
    assert_eq!(parse(&b), Rc::KDF);
    // Unsupported symmetric algorithm, AES key size, and AES mode have distinct errors.
    let mut b = good.clone();
    b[11] = 0x13;
    assert_eq!(parse(&b), Rc::SYMMETRIC);
    let storage = ecc_storage().to_bytes();
    let mut b = storage.clone();
    b[15] = 0x44;
    assert_eq!(parse(&b), Rc::MODE);
    let mut b = storage;
    b[13] = 0xC0;
    assert_eq!(parse(&b), Rc::VALUE);
    b[12..14].copy_from_slice(&256u16.to_be_bytes());
    let aes256 = TpmtPublic::unmarshal(&mut Reader::new(&b)).unwrap();
    assert_eq!(aes256.to_bytes(), b);
    // Cut off.
    assert_eq!(parse(&good[..good.len() - 1]), Rc::INSUFFICIENT);
    // TPM2B_PUBLIC whose size is not that of the structure: TPM_RC_SIZE.
    let mut sized = tpm2b(&good);
    sized[1] += 1;
    sized.push(0);
    assert_eq!(
        TpmtPublic::unmarshal_sized(&mut Reader::new(&sized)).unwrap_err(),
        Rc::SIZE
    );
}

// ================================================================== CreatePrimary

#[test]
fn create_primary_ecc_returns_a_loaded_object_with_consistent_artifacts() {
    let mut t = started();
    let resp = create_primary(&mut t, OWNER, b"pw", &ecc_sign());
    let pr = parse_primary(&resp);
    assert_eq!(pr.handle, FIRST_OBJECT);
    // The public area is the template with a public point on the curve.
    let public = unmarshal_public(&pr.public);
    let Unique::Ecc { x, y } = &public.unique else {
        panic!("not an ECC unique")
    };
    assert_eq!((x.len(), y.len()), (32, 32));
    assert!(crate::crypto::ecc_point_valid(x, y));
    assert_eq!(public.attributes, ecc_sign().attributes);
    // Name = nameAlg || SHA-256(TPMT_PUBLIC); Qualified Name of a primary = H(hierarchy handle
    // || Name) (Part 1, clause 24.5).
    assert_eq!(pr.name, name_hash(&pr.public));
    let rp = read_public(&mut t, pr.handle);
    let mut r = Reader::new(params(&rp, 0));
    assert_eq!(r.tpm2b(1024).unwrap(), &pr.public[..]);
    assert_eq!(r.tpm2b(64).unwrap(), &pr.name[..]);
    let qn = r.tpm2b(64).unwrap().to_vec();
    r.finish().unwrap();
    let expect_qn = name_hash(&[&OWNER.to_be_bytes()[..], &pr.name].concat());
    assert_eq!(qn, expect_qn);
    // Creation data (Table 246): empty PCR selection and digest, locality 0, parent is the
    // hierarchy (nameAlg NULL, Name and QN the handle), the creation hash is SHA-256 of it.
    let expected_creation = cat(&[
        &u32b(0),
        &tpm2b(&[]),
        &[1],
        &u16b(alg::NULL),
        &tpm2b(&OWNER.to_be_bytes()),
        &tpm2b(&OWNER.to_be_bytes()),
        &tpm2b(&[]),
    ]);
    assert_eq!(pr.creation, expected_creation);
    assert_eq!(pr.hash, Sha256::digest(&pr.creation).to_vec());
    // Creation ticket: TPM_ST_CREATION, owner hierarchy, a 32-octet HMAC.
    assert_eq!((pr.ticket.0, pr.ticket.1), (0x8021, OWNER));
    assert_eq!(pr.ticket.2.len(), 32);
    // The private scalar appears in no response.
    let secret = secret_of(&t, pr.handle);
    assert_eq!(secret.len(), 32);
    assert!(!contains(&resp, &secret));
    assert!(!contains(&rp, &secret));
}

#[test]
fn create_primary_rsa_returns_a_2048_bit_modulus() {
    let mut t = started();
    let resp = create_primary(&mut t, OWNER, b"", &rsa_sign());
    let pr = parse_primary(&resp);
    let public = unmarshal_public(&pr.public);
    let Unique::Rsa(n) = &public.unique else {
        panic!("not an RSA unique")
    };
    assert_eq!(n.len(), 256);
    assert_eq!(n[0] & 0x80, 0x80, "a 2048-bit modulus");
    assert_eq!(n[255] & 1, 1);
    assert_eq!(pr.name, name_hash(&pr.public));
    let secret = secret_of(&t, pr.handle);
    assert_eq!(secret.len(), 256);
    assert!(!contains(&resp, &secret[..128]));
    assert!(!contains(&resp, &secret[128..]));
}

#[test]
fn create_primary_is_deterministic_in_seed_and_template() {
    let public_of = |t: &mut Tpm, hier: u32, auth: &[u8], tpl: &TpmtPublic| {
        let r = create_primary(t, hier, auth, tpl);
        let p = parse_primary(&r);
        flush(t, p.handle);
        p.public
    };
    let mut t = started();
    let a = public_of(&mut t, OWNER, b"", &ecc_sign());
    // The same TPM again, and a second TPM with the same owner entropy: the same key.
    assert_eq!(public_of(&mut t, OWNER, b"", &ecc_sign()), a);
    let mut t2 = started();
    assert_eq!(public_of(&mut t2, OWNER, b"", &ecc_sign()), a);
    // The authValue, the outside info and the creation PCR do not enter the derivation.
    assert_eq!(public_of(&mut t, OWNER, b"another-auth", &ecc_sign()), a);
    let params = creation_params(
        b"",
        b"",
        &ecc_sign(),
        b"outside",
        &[0, 0, 0, 1, 0, 0x0B, 3, 1, 0, 0],
    );
    let r = create_primary_raw(&mut t, OWNER, b"", &params);
    let p = parse_primary(&r);
    assert_eq!(p.public, a);
    flush(&mut t, p.handle);
    // Another template (here another attribute), another hierarchy, another seed: another key.
    let mut other = ecc_sign();
    other.attributes |= oa::NO_DA;
    let b = public_of(&mut t, OWNER, b"", &other);
    assert_ne!(b, a);
    assert_ne!(public_of(&mut t, PLATFORM, b"", &ecc_sign()), a);
    assert_ne!(public_of(&mut t, ENDORSEMENT, b"", &ecc_sign()), a);
    assert_ne!(public_of(&mut t, NULL, b"", &ecc_sign()), a);
    let mut t3 = Tpm::new(&[6u8; 48]).unwrap();
    ok(&run(&mut t3, plain(cc::STARTUP, &[], &[0, 0])));
    assert_ne!(public_of(&mut t3, OWNER, b"", &ecc_sign()), a);
    // A snapshot carries the seeds: a restored TPM derives the same primary.
    let snap = t.persistent_snapshot();
    let mut restored = Tpm::restore(&snap, &[8u8; 48]).unwrap();
    ok(&run(&mut restored, plain(cc::STARTUP, &[], &[0, 0])));
    assert_eq!(public_of(&mut restored, OWNER, b"", &ecc_sign()), a);
}

#[test]
fn create_primary_rsa_is_deterministic() {
    let mut t = started();
    let a = parse_primary(&create_primary(&mut t, OWNER, b"", &rsa_sign()));
    flush(&mut t, a.handle);
    let b = parse_primary(&create_primary(&mut t, OWNER, b"", &rsa_sign()));
    assert_eq!(a.public, b.public);
    assert_eq!(a.name, b.name);
}

#[test]
fn endorsement_key_survives_clear() {
    // Part 1, 12.4.2: the EK is a function of the EPS; TPM2_Clear changes the SPS (and shProof,
    // ehProof) but not the EPS, so the same template gives the same key. The owner primary,
    // which follows the storage seed, does change.
    let mut t = started();
    let public_of = |t: &mut Tpm, who: u32| {
        let p = parse_primary(&create_primary(t, who, b"", &ecc_sign()));
        flush(t, p.handle);
        p.public
    };
    let ek = public_of(&mut t, ENDORSEMENT);
    let owner = public_of(&mut t, OWNER);
    assert_eq!(public_of(&mut t, ENDORSEMENT), ek);
    ok(&run(
        &mut t,
        with_pw(cc::CLEAR, &[handle::RH_PLATFORM], &[b""], &[]),
    ));
    assert_eq!(public_of(&mut t, ENDORSEMENT), ek);
    assert_ne!(public_of(&mut t, OWNER), owner);
}

#[test]
fn endorsement_auth_does_not_change_the_ek() {
    let mut t = started();
    let a = parse_primary(&create_primary(&mut t, ENDORSEMENT, b"", &ecc_sign()));
    flush(&mut t, a.handle);
    ok(&run(
        &mut t,
        with_pw(
            cc::HIERARCHY_CHANGE_AUTH,
            &[ENDORSEMENT],
            &[b""],
            &tpm2b(b"new-endorsement-auth"),
        ),
    ));
    // The endorsement hierarchy now needs its new authorization value.
    let params = creation_params(b"", b"", &ecc_sign(), b"", &NO_PCR);
    err(
        &create_primary_raw(&mut t, ENDORSEMENT, b"", &params),
        Rc::BAD_AUTH.in_session(1),
    );
    let b = parse_primary(&create_primary_raw(
        &mut t,
        ENDORSEMENT,
        b"new-endorsement-auth",
        &params,
    ));
    assert_eq!(a.public, b.public);
}

#[test]
fn create_primary_in_the_null_hierarchy_issues_a_creation_ticket() {
    let mut t = started();
    let pr = parse_primary(&create_primary(&mut t, NULL, b"", &ecc_sign()));
    assert_eq!(
        (pr.ticket.0, pr.ticket.1, pr.ticket.2.len()),
        (0x8021, NULL, 32)
    );
    // Parent of a temporary object: nameAlg NULL, Name and QN the handle of TPM_RH_NULL.
    assert!(contains(&pr.creation, &tpm2b(&NULL.to_be_bytes())));
}

#[test]
fn create_primary_creation_data_carries_pcr_digest_and_outside_info() {
    let mut t = started();
    ok(&extend(&mut t, 0, &[0x11; 32]));
    let pcr0 = read_pcr(&mut t, 0);
    // creationPCR: one SHA-256 selection of PCR 0.
    let sel = [0, 0, 0, 1, 0, 0x0B, 3, 1, 0, 0];
    let params = creation_params(b"", b"", &ecc_sign(), b"owner-data", &sel);
    let pr = parse_primary(&create_primary_raw(&mut t, OWNER, b"", &params));
    let expected = cat(&[
        &sel,
        &tpm2b(&Sha256::digest(&pcr0)),
        &[1],
        &u16b(alg::NULL),
        &tpm2b(&OWNER.to_be_bytes()),
        &tpm2b(&OWNER.to_be_bytes()),
        &tpm2b(b"owner-data"),
    ]);
    assert_eq!(pr.creation, expected);
}

#[test]
fn creation_ticket_is_an_hmac_under_the_hierarchy_proof() {
    use crate::crypto::{hmac_sha256, kdfa_sha256};
    let mut t = started();
    let pr = parse_primary(&create_primary(&mut t, OWNER, b"", &ecc_sign()));
    // Part 2, Equation (4): HMAC(proof, TPM_ST_CREATION || name || H(creationData)); the proof is
    // derived from the storage seed (`object_cmd.rs`).
    let proof = kdfa_sha256(
        &t.persistent.owner_seed[..32],
        b"ODTPM-SHPROOF",
        &[],
        &[],
        256,
    );
    let expect = hmac_sha256(&proof, &[&u16b(0x8021), &pr.name, &pr.hash]);
    assert_eq!(pr.ticket.2, expect.to_vec());
    // Another hierarchy has another proof.
    let pl = parse_primary(&create_primary(&mut t, PLATFORM, b"", &ecc_sign()));
    assert_eq!(pl.ticket.1, PLATFORM);
    let pproof = kdfa_sha256(
        &t.persistent.platform_seed[..32],
        b"ODTPM-PHPROOF",
        &[],
        &[],
        256,
    );
    assert_eq!(
        pl.ticket.2,
        hmac_sha256(&pproof, &[&u16b(0x8021), &pl.name, &pl.hash]).to_vec()
    );
}

#[test]
fn create_primary_authorization_and_handle_errors() {
    let mut t = started();
    let params = creation_params(b"", b"", &ecc_sign(), b"", &NO_PCR);
    // No authorization session, a wrong password, a handle that is no hierarchy.
    err(
        &run(&mut t, plain(cc::CREATE_PRIMARY, &[OWNER], &params)),
        Rc::AUTH_MISSING,
    );
    err(
        &create_primary_raw(&mut t, OWNER, b"wrong", &params),
        Rc::BAD_AUTH.in_session(1),
    );
    err(
        &create_primary_raw(&mut t, handle::RH_LOCKOUT, b"", &params),
        Rc::VALUE.in_handle(1),
    );
    err(
        &create_primary_raw(&mut t, 0x8000_0000, b"", &params),
        Rc::VALUE.in_handle(1),
    );
    // A changed owner authorization is required.
    ok(&run(
        &mut t,
        with_pw(cc::HIERARCHY_CHANGE_AUTH, &[OWNER], &[b""], &tpm2b(b"new")),
    ));
    err(
        &create_primary_raw(&mut t, OWNER, b"", &params),
        Rc::BAD_AUTH.in_session(1),
    );
    ok(&create_primary_raw(&mut t, OWNER, b"new", &params));
    // A disabled hierarchy cannot authorize.
    ok(&run(
        &mut t,
        with_pw(
            cc::HIERARCHY_CONTROL,
            &[PLATFORM],
            &[b""],
            &cat(&[&u32b(OWNER), &[0]]),
        ),
    ));
    err(
        &create_primary_raw(&mut t, OWNER, b"new", &params),
        Rc::HIERARCHY.in_handle(1),
    );
}

#[test]
fn create_primary_template_errors() {
    let mut t = started();
    let go = |t: &mut Tpm, tpl: &TpmtPublic, auth: &[u8], data: &[u8]| {
        create_primary_raw(
            t,
            OWNER,
            b"",
            &creation_params(auth, data, tpl, b"", &NO_PCR),
        )
    };
    let mut bad = ecc_sign();
    bad.name_alg = 0x0004;
    err(&go(&mut t, &bad, b"", b""), Rc::HASH.in_param(2));
    // Reserved attribute bit (bit 3).
    let mut bad = ecc_sign();
    bad.attributes |= 8;
    err(&go(&mut t, &bad, b"", b""), Rc::RESERVED_BITS.in_param(2));
    // restricted + sign + decrypt; neither sign nor decrypt.
    let mut bad = ecc_sign();
    bad.attributes |= oa::RESTRICTED | oa::DECRYPT;
    err(&go(&mut t, &bad, b"", b""), Rc::ATTRIBUTES.in_param(2));
    let mut bad = ecc_sign();
    bad.attributes &= !oa::SIGN;
    err(&go(&mut t, &bad, b"", b""), Rc::ATTRIBUTES.in_param(2));
    // x509sign without sign, firmwareLimited.
    let mut bad = ecc_storage();
    bad.attributes |= oa::X509_SIGN;
    err(&go(&mut t, &bad, b"", b""), Rc::ATTRIBUTES.in_param(2));
    let mut bad = ecc_sign();
    bad.attributes |= oa::FIRMWARE_LIMITED;
    err(&go(&mut t, &bad, b"", b""), Rc::ATTRIBUTES.in_param(2));
    // sensitive data for an asymmetric key; sensitiveDataOrigin CLEAR.
    err(&go(&mut t, &ecc_sign(), b"", b"x"), Rc::VALUE.in_param(1));
    let mut bad = ecc_sign();
    bad.attributes &= !oa::SENSITIVE_DATA_ORIGIN;
    err(&go(&mut t, &bad, b"", b""), Rc::ATTRIBUTES.in_param(2));
    // fixedTPM and fixedParent differ under a fixedTPM parent; encryptedDuplication with fixedTPM.
    let mut bad = ecc_sign();
    bad.attributes &= !oa::FIXED_PARENT;
    err(&go(&mut t, &bad, b"", b""), Rc::ATTRIBUTES.in_param(2));
    let mut bad = ecc_sign();
    bad.attributes |= oa::ENCRYPTED_DUPLICATION;
    err(&go(&mut t, &bad, b"", b""), Rc::ATTRIBUTES.in_param(2));
    // A storage key without a symmetric algorithm; a signing key with one.
    let bad = ecc(
        base() | oa::RESTRICTED | oa::DECRYPT,
        SymDef::Null,
        Scheme::NULL,
    );
    err(&go(&mut t, &bad, b"", b""), Rc::SYMMETRIC.in_param(2));
    let bad = ecc(base() | oa::SIGN, SymDef::Aes128Cfb, ECDSA);
    err(&go(&mut t, &bad, b"", b""), Rc::SYMMETRIC.in_param(2));
    // Schemes: ECC signing key with a NULL scheme; RSA decryption key with RSASSA.
    let bad = ecc(base() | oa::SIGN, SymDef::Null, Scheme::NULL);
    err(&go(&mut t, &bad, b"", b""), Rc::SCHEME.in_param(2));
    let bad = rsa(base() | oa::DECRYPT, SymDef::Null, RSASSA);
    err(&go(&mut t, &bad, b"", b""), Rc::SCHEME.in_param(2));
    // RSA exponent other than 65537; authPolicy of the wrong size.
    let mut bad = rsa_sign();
    if let PublicParams::Rsa { exponent, .. } = &mut bad.params {
        *exponent = 3;
    }
    err(&go(&mut t, &bad, b"", b""), Rc::RANGE.in_param(2));
    let mut bad = ecc_sign();
    bad.auth_policy = vec![1; 5];
    err(&go(&mut t, &bad, b"", b""), Rc::SIZE.in_param(2));
    // authValue above one digest: TPM_RC_SIZE for the sensitive area (parameter 1).
    err(
        &go(&mut t, &ecc_sign(), &[1u8; 33], b""),
        Rc::SIZE.in_param(1),
    );
    // Keyed hash: XOR (`decrypt`) and restricted keys are not implemented (an HMAC key with `sign`
    // is slice C2); sealed without data.
    for extra in [oa::DECRYPT, oa::RESTRICTED | oa::SIGN, oa::X509_SIGN] {
        let mut bad = sealed();
        bad.attributes |= extra;
        err(&go(&mut t, &bad, b"", b"x"), Rc::ATTRIBUTES.in_param(2));
    }
    err(&go(&mut t, &sealed(), b"", b""), Rc::ATTRIBUTES.in_param(2));
    let mut bad = sealed();
    bad.attributes |= oa::SENSITIVE_DATA_ORIGIN;
    err(&go(&mut t, &bad, b"", b"x"), Rc::ATTRIBUTES.in_param(2));
    // Nothing above created an object.
    assert_eq!(t.objects.free_slots(), MAX_OBJECTS);
}

#[test]
fn create_primary_framing_errors() {
    let mut t = started();
    let good = creation_params(b"", b"", &ecc_sign(), b"", &NO_PCR);
    // Trailing octets.
    let mut long = good.clone();
    long.push(0);
    err(&create_primary_raw(&mut t, OWNER, b"", &long), Rc::SIZE);
    // Missing creationPCR.
    err(
        &create_primary_raw(&mut t, OWNER, b"", &good[..good.len() - 4]),
        Rc::INSUFFICIENT.in_param(4),
    );
    // outsideInfo above sizeof(TPMT_HA).
    err(
        &create_primary_raw(
            &mut t,
            OWNER,
            b"",
            &creation_params(b"", b"", &ecc_sign(), &[0u8; 35], &NO_PCR),
        ),
        Rc::SIZE.in_param(3),
    );
    // creationPCR with more selections than the implemented banks.
    let too_many = creation_params(b"", b"", &ecc_sign(), b"", &[0, 0, 0, 3]);
    err(
        &create_primary_raw(&mut t, OWNER, b"", &too_many),
        Rc::SIZE.in_param(4),
    );
    // TPM2B_SENSITIVE_CREATE whose size does not match its content.
    let sens = cat(&[&tpm2b(b""), &tpm2b(b""), &[0]]);
    let bad = cat(&[
        &tpm2b(&sens),
        &tpm2b(&ecc_sign().to_bytes()),
        &tpm2b(b""),
        &NO_PCR,
    ]);
    err(
        &create_primary_raw(&mut t, OWNER, b"", &bad),
        Rc::SIZE.in_param(1),
    );
    assert_eq!(t.objects.free_slots(), MAX_OBJECTS);
}

#[test]
fn object_slots_are_limited() {
    let mut t = started();
    for i in 0..MAX_OBJECTS {
        let p = parse_primary(&create_primary(&mut t, OWNER, b"", &ecc_sign()));
        assert_eq!(p.handle, FIRST_OBJECT + i as u32);
    }
    let params = creation_params(b"", b"", &ecc_sign(), b"", &NO_PCR);
    err(
        &create_primary_raw(&mut t, OWNER, b"", &params),
        Rc::OBJECT_MEMORY,
    );
    assert_eq!(t.objects.free_slots(), 0);
    // Flushing one frees the slot, which is reused.
    ok(&flush(&mut t, FIRST_OBJECT + 2));
    let p = parse_primary(&create_primary(&mut t, OWNER, b"", &ecc_sign()));
    assert_eq!(p.handle, FIRST_OBJECT + 2);
}

// ============================================================= Create / Load / Sign / Verify

#[test]
fn create_load_sign_verify_ecc() {
    let (mut t, parent) = with_ecc_parent();
    let resp = create_child(&mut t, parent, b"parent-pw", b"child-pw", b"", &ecc_sign());
    let child = parse_child(&resp);
    // The template with a point on the curve; nothing of the private value in the blob.
    let public = unmarshal_public(&child.public);
    let Unique::Ecc { x, y } = &public.unique else {
        panic!()
    };
    assert!(crate::crypto::ecc_point_valid(x, y));
    assert_eq!(child.hash, Sha256::digest(&child.creation).to_vec());
    assert_eq!((child.ticket.0, child.ticket.1), (0x8021, OWNER));
    // Not loaded yet: the new object occupies no slot.
    assert_eq!(t.objects.free_slots(), MAX_OBJECTS - 1);
    let lr = load(&mut t, parent, b"parent-pw", &child.private, &child.public);
    let (key, name) = parse_loaded(&lr);
    assert_eq!(name, name_hash(&child.public));
    let secret = secret_of(&t, key);
    assert!(
        !contains(&resp, &secret),
        "the private scalar never leaves the TPM"
    );
    assert!(
        !contains(&child.private, &secret),
        "outPrivate is encrypted"
    );
    // Sign with the object's password, verify with the same key and as a loaded public key.
    let sr = sign(&mut t, key, b"child-pw", &DIGEST, &ecc_sig_scheme());
    let sig = params(&sr, 0).to_vec();
    assert_eq!(&sig[..4], &[0x00, 0x18, 0x00, 0x0B]);
    let vr = verify(&mut t, key, &DIGEST, &sig);
    let ticket = read_ticket(&mut Reader::new(params(&vr, 0)));
    assert_eq!((ticket.0, ticket.1), (0x8022, OWNER));
    assert_eq!(ticket.2.len(), 32);
    // Verified ticket (Part 2, clause 10.7.4): HMAC(proof, TPM_ST_VERIFIED || digest || keyName).
    let proof = crate::crypto::kdfa_sha256(
        &t.persistent.owner_seed[..32],
        b"ODTPM-SHPROOF",
        &[],
        &[],
        256,
    );
    assert_eq!(
        ticket.2,
        crate::crypto::hmac_sha256(&proof, &[&u16b(0x8022), &DIGEST, &name]).to_vec()
    );
    // Deterministic ECDSA (RFC 6979): the same digest signs identically.
    assert_eq!(
        params(&sign(&mut t, key, b"child-pw", &DIGEST, &null_scheme()), 0),
        &sig[..]
    );
    // A corrupted signature or digest fails with TPM_RC_SIGNATURE.
    let mut bad = sig.clone();
    let last = bad.len() - 1;
    bad[last] ^= 1;
    err(&verify(&mut t, key, &DIGEST, &bad), Rc::SIGNATURE);
    let mut other = DIGEST;
    other[0] ^= 1;
    err(&verify(&mut t, key, &other, &sig), Rc::SIGNATURE);
    // Wrong authorization, no authorization.
    err(
        &sign(&mut t, key, b"parent-pw", &DIGEST, &ecc_sig_scheme()),
        Rc::AUTH_FAIL.in_session(1),
    );
    err(
        &run(
            &mut t,
            plain(cc::SIGN, &[key], &sign_params(&DIGEST, &ecc_sig_scheme())),
        ),
        Rc::AUTH_MISSING,
    );
}

#[test]
fn create_load_sign_verify_rsa() {
    let mut t = started();
    let parent = parse_primary(&create_primary(&mut t, OWNER, b"", &rsa_storage())).handle;
    let child = parse_child(&create_child(
        &mut t,
        parent,
        b"",
        b"rsa-pw",
        b"",
        &rsa_sign(),
    ));
    let (key, _) = parse_loaded(&load(&mut t, parent, b"", &child.private, &child.public));
    let sr = sign(&mut t, key, b"rsa-pw", &DIGEST, &rsa_sig_scheme());
    let sig = params(&sr, 0).to_vec();
    // TPMT_SIGNATURE: RSASSA, SHA-256, a 256-octet signature.
    assert_eq!(&sig[..6], &[0x00, 0x14, 0x00, 0x0B, 0x01, 0x00]);
    assert_eq!(sig.len(), 2 + 2 + 2 + 256);
    ok(&verify(&mut t, key, &DIGEST, &sig));
    let mut bad = sig.clone();
    bad[10] ^= 1;
    err(&verify(&mut t, key, &DIGEST, &bad), Rc::SIGNATURE);
    // The key's scheme is RSASSA: ECDSA in inScheme is refused, NULL is accepted.
    err(
        &sign(&mut t, key, b"rsa-pw", &DIGEST, &ecc_sig_scheme()),
        Rc::SCHEME.in_param(2),
    );
    ok(&sign(&mut t, key, b"rsa-pw", &DIGEST, &null_scheme()));
    // The public modulus of the signing key has 256 octets.
    let public = unmarshal_public(&child.public);
    let Unique::Rsa(n) = public.unique else {
        panic!()
    };
    assert_eq!(n.len(), 256);
}

#[test]
fn sign_with_an_hmac_session() {
    let (mut t, parent) = with_ecc_parent();
    let child = parse_child(&create_child(
        &mut t,
        parent,
        b"parent-pw",
        b"hs-pw",
        b"",
        &ecc_sign(),
    ));
    let (key, name) = parse_loaded(&load(
        &mut t,
        parent,
        b"parent-pw",
        &child.private,
        &child.public,
    ));
    let mut hs = start_hmac(&mut t, NULL, &[], &[0x11; 16]);
    let mut c = hc(
        cc::SIGN,
        &[key],
        sign_params(&DIGEST, &ecc_sig_scheme()),
        (key, b"hs-pw"),
    );
    // The cpHash uses the Name of the object (Part 1, Table 6).
    c.names = vec![name.clone()];
    let resp = run_hmac(&mut t, &mut hs, &c);
    ok(&resp);
    // A wrong authValue inside the HMAC: TPM_RC_AUTH_FAIL (the object is DA protected) for session 1.
    let mut c = hc(
        cc::SIGN,
        &[key],
        sign_params(&DIGEST, &ecc_sig_scheme()),
        (key, b"not-it"),
    );
    c.names = vec![name];
    err(&run_hmac(&mut t, &mut hs, &c), Rc::AUTH_FAIL.in_session(1));
}

#[test]
fn create_errors() {
    let (mut t, parent) = with_ecc_parent();
    // Wrong parent password, no session.
    err(
        &create_child(&mut t, parent, b"nope", b"", b"", &ecc_sign()),
        Rc::AUTH_FAIL.in_session(1),
    );
    let params = creation_params(b"", b"", &ecc_sign(), b"", &NO_PCR);
    err(
        &run(&mut t, plain(cc::CREATE, &[parent], &params)),
        Rc::AUTH_MISSING,
    );
    // An unloaded handle, a PCR handle, a sequence object.
    err(
        &create_child(&mut t, 0x8000_0009, b"", b"", b"", &ecc_sign()),
        Rc::reference_handle(0),
    );
    err(
        &run(&mut t, with_pw(cc::CREATE, &[3], &[b""], &params)),
        Rc::VALUE.in_handle(1),
    );
    ok(&run(
        &mut t,
        plain(
            cc::HASH_SEQUENCE_START,
            &[],
            &cat(&[&tpm2b(b""), &u16b(alg::SHA256)]),
        ),
    ));
    err(
        &run(&mut t, with_pw(cc::CREATE, &[0x8000_0000], &[b""], &params)),
        Rc::SEQUENCE,
    );
    // A signing key is no parent (TPM_RC_TYPE, handle 1); neither is a public-only key.
    let sk = parse_child(&create_child(
        &mut t,
        parent,
        b"parent-pw",
        b"k",
        b"",
        &ecc_sign(),
    ));
    let (key, _) = parse_loaded(&load(&mut t, parent, b"parent-pw", &sk.private, &sk.public));
    err(
        &create_child(&mut t, key, b"k", b"", b"", &ecc_sign()),
        Rc::TYPE.in_handle(1),
    );
    // Template errors are reported with parameter 2 as for CreatePrimary.
    let mut bad = ecc_sign();
    bad.attributes &= !oa::SIGN;
    err(
        &create_child(&mut t, parent, b"parent-pw", b"", b"", &bad),
        Rc::ATTRIBUTES.in_param(2),
    );
    // Under a fixedTPM parent a child with fixedTPM CLEAR and fixedParent SET is refused; with
    // both CLEAR (a duplicable object) it is created.
    let mut dup = ecc_sign();
    dup.attributes &= !oa::FIXED_TPM;
    err(
        &create_child(&mut t, parent, b"parent-pw", b"", b"", &dup),
        Rc::ATTRIBUTES.in_param(2),
    );
    dup.attributes &= !oa::FIXED_PARENT;
    ok(&create_child(&mut t, parent, b"parent-pw", b"", b"", &dup));
    // A nested storage key.
    ok(&create_child(
        &mut t,
        parent,
        b"parent-pw",
        b"",
        b"",
        &ecc_storage(),
    ));
}

#[test]
fn create_requires_fixed_storage_children_to_match_the_parent_cipher() {
    let storage = |symmetric| {
        ecc(
            base() | oa::RESTRICTED | oa::DECRYPT,
            symmetric,
            Scheme::NULL,
        )
    };
    for (parent_cipher, other_cipher) in [
        (SymDef::Aes128Cfb, SymDef::Aes256Cfb),
        (SymDef::Aes256Cfb, SymDef::Aes128Cfb),
    ] {
        let mut t = started();
        let template = storage(parent_cipher);
        let parent = parse_primary(&create_primary(&mut t, OWNER, b"parent", &template)).handle;
        ok(&create_child(
            &mut t, parent, b"parent", b"", b"", &template,
        ));

        let mut mismatched = storage(other_cipher);
        err(
            &create_child(&mut t, parent, b"parent", b"", b"", &mismatched),
            Rc::SYMMETRIC.in_param(2),
        );
        mismatched.attributes &= !(oa::FIXED_TPM | oa::FIXED_PARENT);
        ok(&create_child(
            &mut t,
            parent,
            b"parent",
            b"",
            b"",
            &mismatched,
        ));
    }
}

#[test]
fn load_errors() {
    let (mut t, parent) = with_ecc_parent();
    let child = parse_child(&create_child(
        &mut t,
        parent,
        b"parent-pw",
        b"c",
        b"",
        &ecc_sign(),
    ));
    // Wrong password; empty inPrivate; trailing octets.
    err(
        &load(&mut t, parent, b"nope", &child.private, &child.public),
        Rc::AUTH_FAIL.in_session(1),
    );
    err(
        &load(&mut t, parent, b"parent-pw", &[], &child.public),
        Rc::SIZE.in_param(1),
    );
    let extra = cat(&[&tpm2b(&child.private), &tpm2b(&child.public), &[0]]);
    err(
        &run(
            &mut t,
            with_pw(cc::LOAD, &[parent], &[b"parent-pw"], &extra),
        ),
        Rc::SIZE,
    );
    // Integrity: any changed octet of the private area, another public area, another parent.
    for i in [0usize, 3, 40, child.private.len() - 1] {
        let mut bad = child.private.clone();
        bad[i] ^= 1;
        let r = load(&mut t, parent, b"parent-pw", &bad, &child.public);
        // A change in the size fields makes the container malformed (TPM_RC_SIZE), anything
        // else fails the integrity check.
        assert!(
            matches!(rc(&r), x if x == Rc::INTEGRITY.0 || x == Rc::SIZE.0),
            "octet {i}: {:#x}",
            rc(&r)
        );
    }
    let mut other_public = child.public.clone();
    let n = other_public.len();
    other_public[n - 1] ^= 1;
    err(
        &load(&mut t, parent, b"parent-pw", &child.private, &other_public),
        Rc::INTEGRITY,
    );
    let other_parent = parse_primary(&create_primary(&mut t, PLATFORM, b"", &ecc_storage())).handle;
    err(
        &load(&mut t, other_parent, b"", &child.private, &child.public),
        Rc::INTEGRITY,
    );
    // The parent must be a loaded storage key.
    err(
        &load(&mut t, 0x8000_0009, b"", &child.private, &child.public),
        Rc::reference_handle(0),
    );
    // Nothing was loaded by the failures.
    assert_eq!(t.objects.free_slots(), MAX_OBJECTS - 2);
    // Re-deriving the same primary gives a parent that loads the child again.
    ok(&flush(&mut t, parent));
    let again = parse_primary(&create_primary(&mut t, OWNER, b"parent-pw", &ecc_storage())).handle;
    ok(&load(
        &mut t,
        again,
        b"parent-pw",
        &child.private,
        &child.public,
    ));
    // After TPM2_Clear (a new storage seed) the child cannot be loaded under the new primary.
    ok(&run(&mut t, with_pw(cc::CLEAR, &[PLATFORM], &[b""], &[])));
    let moved = parse_primary(&create_primary(&mut t, OWNER, b"", &ecc_storage())).handle;
    err(
        &load(&mut t, moved, b"", &child.private, &child.public),
        Rc::INTEGRITY,
    );
    // No free slot.
    for _ in 0..MAX_OBJECTS {
        let _ = create_primary(&mut t, NULL, b"", &ecc_sign());
    }
    err(
        &load(&mut t, moved, b"", &child.private, &child.public),
        Rc::OBJECT_MEMORY,
    );
}

#[test]
fn sign_and_verify_errors() {
    let (mut t, parent) = with_ecc_parent();
    let child = parse_child(&create_child(
        &mut t,
        parent,
        b"parent-pw",
        b"c",
        b"",
        &ecc_sign(),
    ));
    let (key, _) = parse_loaded(&load(
        &mut t,
        parent,
        b"parent-pw",
        &child.private,
        &child.public,
    ));
    let good = sign(&mut t, key, b"c", &DIGEST, &ecc_sig_scheme());
    let sig = params(&good, 0).to_vec();
    // Digest of the wrong size; a scheme with another hash (slice 1b: SHA-384 parses, and differs
    // from the key's SHA-256 scheme, so it is TPM_RC_SCHEME for inScheme); RSASSA for an ECC key.
    err(
        &sign(&mut t, key, b"c", &DIGEST[..31], &ecc_sig_scheme()),
        Rc::SIZE.in_param(1),
    );
    err(
        &sign(
            &mut t,
            key,
            b"c",
            &DIGEST,
            &cat(&[&u16b(alg::ECDSA), &u16b(0x000C)]),
        ),
        Rc::SCHEME.in_param(2),
    );
    // A hash that is no signing hash (SHA3-256) stays TPM_RC_HASH.
    err(
        &sign(
            &mut t,
            key,
            b"c",
            &DIGEST,
            &cat(&[&u16b(alg::ECDSA), &u16b(0x0027)]),
        ),
        Rc::HASH.in_param(2),
    );
    err(
        &sign(&mut t, key, b"c", &DIGEST, &rsa_sig_scheme()),
        Rc::SCHEME.in_param(2),
    );
    // inScheme with an unknown algorithm.
    err(
        &sign(
            &mut t,
            key,
            b"c",
            &DIGEST,
            &cat(&[&u16b(0x0019), &u16b(alg::SHA256)]),
        ),
        Rc::SCHEME.in_param(2),
    );
    // validation: a ticket tag other than TPM_ST_HASHCHECK; a non-NULL ticket; a restricted key.
    let bad_tag = cat(&[
        &tpm2b(&DIGEST),
        &ecc_sig_scheme(),
        &[0x80, 0x22, 0x40, 0, 0, 7, 0, 0],
    ]);
    err(
        &run(&mut t, with_pw(cc::SIGN, &[key], &[b"c"], &bad_tag)),
        Rc::TAG.in_param(3),
    );
    let ticket = cat(&[
        &tpm2b(&DIGEST),
        &ecc_sig_scheme(),
        &[0x80, 0x24, 0x40, 0, 0, 1, 0, 2, 9, 9],
    ]);
    err(
        &run(&mut t, with_pw(cc::SIGN, &[key], &[b"c"], &ticket)),
        Rc::TICKET.in_param(3),
    );
    let restricted = ecc(base() | oa::SIGN | oa::RESTRICTED, SymDef::Null, ECDSA);
    let rk = parse_child(&create_child(
        &mut t,
        parent,
        b"parent-pw",
        b"r",
        b"",
        &restricted,
    ));
    let (rkey, _) = parse_loaded(&load(&mut t, parent, b"parent-pw", &rk.private, &rk.public));
    err(
        &sign(&mut t, rkey, b"r", &DIGEST, &ecc_sig_scheme()),
        Rc::TICKET.in_param(3),
    );
    // A storage key does not sign (TPM_RC_KEY); a decrypt-only key neither.
    err(
        &sign(&mut t, parent, b"parent-pw", &DIGEST, &ecc_sig_scheme()),
        Rc::KEY.in_handle(1),
    );
    // x509sign keys do not sign with TPM2_Sign.
    let mut x = ecc_sign();
    x.attributes |= oa::X509_SIGN;
    let xk = parse_child(&create_child(&mut t, parent, b"parent-pw", b"x", b"", &x));
    let (xkey, _) = parse_loaded(&load(&mut t, parent, b"parent-pw", &xk.private, &xk.public));
    err(
        &sign(&mut t, xkey, b"x", &DIGEST, &ecc_sig_scheme()),
        Rc::ATTRIBUTES.in_handle(1),
    );
    // userWithAuth CLEAR: no password or HMAC can authorize, only a policy session (Part 3, clause
    // 5.6, step 7: TPM_RC_POLICY_FAIL with the session number).
    let mut pol = ecc_sign();
    pol.attributes &= !oa::USER_WITH_AUTH;
    let pk = parse_child(&create_child(&mut t, parent, b"parent-pw", b"p", b"", &pol));
    let (pkey, _) = parse_loaded(&load(&mut t, parent, b"parent-pw", &pk.private, &pk.public));
    err(
        &sign(&mut t, pkey, b"p", &DIGEST, &ecc_sig_scheme()),
        Rc::POLICY_FAIL.in_session(1),
    );
    // An unloaded key; trailing octets.
    err(
        &sign(&mut t, 0x8000_0000 + 20, b"", &DIGEST, &ecc_sig_scheme()),
        Rc::reference_handle(0),
    );
    let mut long = sign_params(&DIGEST, &ecc_sig_scheme());
    long.push(0);
    err(
        &run(&mut t, with_pw(cc::SIGN, &[key], &[b"c"], &long)),
        Rc::SIZE,
    );
    // VerifySignature: a signature of another type, an unknown sigAlg (also NULL), a digest of
    // the wrong size, a key with sign CLEAR.
    err(
        &verify(
            &mut t,
            key,
            &DIGEST,
            &cat(&[&u16b(alg::RSASSA), &u16b(alg::SHA256), &tpm2b(&[0; 256])]),
        ),
        Rc::SCHEME.in_param(2),
    );
    err(
        &verify(&mut t, key, &DIGEST, &u16b(alg::NULL)),
        Rc::SCHEME.in_param(2),
    );
    err(
        &verify(&mut t, key, &DIGEST[..30], &sig),
        Rc::SIZE.in_param(1),
    );
    err(&verify(&mut t, parent, &DIGEST, &sig), Rc::KEY.in_handle(1));
    // Zero r and s are no valid signature.
    let zero = cat(&[
        &u16b(alg::ECDSA),
        &u16b(alg::SHA256),
        &tpm2b(&[0; 32]),
        &tpm2b(&[0; 32]),
    ]);
    err(&verify(&mut t, key, &DIGEST, &zero), Rc::SIGNATURE);
    // The authorization-free commands refuse a session (no authorized handle).
    err(
        &run(
            &mut t,
            with_pw(
                cc::VERIFY_SIGNATURE,
                &[key],
                &[b""],
                &cat(&[&tpm2b(&DIGEST), &sig]),
            ),
        ),
        Rc::ATTRIBUTES.in_session(1),
    );
}

// ======================================================================== LoadExternal

fn hexb(s: &str) -> Vec<u8> {
    hex(s)
}

fn ext_ecc_public() -> TpmtPublic {
    let mut p = ecc(oa::SIGN | oa::USER_WITH_AUTH, SymDef::Null, ECDSA);
    p.unique = Unique::Ecc {
        x: hexb(EC_X),
        y: hexb(EC_Y),
    };
    p
}

fn ext_rsa_public() -> TpmtPublic {
    let mut p = rsa(oa::SIGN | oa::USER_WITH_AUTH, SymDef::Null, RSASSA);
    p.unique = Unique::Rsa(hexb(RSA_N));
    p
}

fn ecdsa_signature(r: &[u8], s: &[u8]) -> Vec<u8> {
    cat(&[&u16b(alg::ECDSA), &u16b(alg::SHA256), &tpm2b(r), &tpm2b(s)])
}

fn rsassa_signature(sig: &[u8]) -> Vec<u8> {
    cat(&[&u16b(alg::RSASSA), &u16b(alg::SHA256), &tpm2b(sig)])
}

#[test]
fn mlkem_encapsulates_with_a_public_only_key_and_decapsulates_with_the_private_key() {
    let mut t = started();
    let template = TpmtPublic {
        name_alg: alg::SHA256,
        attributes: base() | oa::DECRYPT,
        auth_policy: vec![],
        params: PublicParams::MlKem {
            symmetric: SymDef::Null,
            parameter_set: 2,
        },
        unique: Unique::MlKem(vec![]),
    };
    let key = parse_primary(&create_primary(&mut t, OWNER, b"kem key", &template));
    let mut public = TpmtPublic::unmarshal(&mut Reader::new(&key.public)).unwrap();
    public.attributes = 0; // Encapsulate needs the public key and no authenticated attributes.
    let (external, _) = parse_loaded(&load_external(&mut t, &public, NULL));
    let encapsulated = run(&mut t, plain(cc::ENCAPSULATE, &[external], &[]));
    let mut r = Reader::new(params(&encapsulated, 0));
    let shared = r.tpm2b(32).unwrap();
    let ciphertext = r.tpm2b(1088).unwrap();
    assert_eq!((shared.len(), ciphertext.len()), (32, 1088));
    r.finish().unwrap();

    let decapsulate = |t: &mut Tpm, ciphertext: &[u8], password: &[u8]| {
        run(
            t,
            with_pw(
                cc::DECAPSULATE,
                &[key.handle],
                &[password],
                &tpm2b(ciphertext),
            ),
        )
    };
    let reply = decapsulate(&mut t, ciphertext, b"kem key");
    assert_eq!(params(&reply, 0), tpm2b(shared));
    err(
        &decapsulate(&mut t, ciphertext, b"wrong"),
        Rc::AUTH_FAIL.in_session(1),
    );
    err(
        &decapsulate(&mut t, &ciphertext[..1087], b"kem key"),
        Rc::SIZE.in_param(1),
    );
}

#[test]
fn verify_sequence_tickets_authenticate_the_raw_message_after_a_failed_completion() {
    let mut t = started();
    let key = parse_primary(&create_primary(&mut t, OWNER, b"owner key", &ecc_sign()));
    let input = b"verify through\0two buffers";
    let digest = Sha256::digest(input);
    let signature_reply = sign(&mut t, key.handle, b"owner key", &digest, &null_scheme());
    let signature = params(&signature_reply, 0);
    let auth = b"verify sequence";
    let sequence = seq_handle(&run(
        &mut t,
        plain(
            cc::VERIFY_SEQUENCE_START,
            &[key.handle],
            &cat(&[&tpm2b(auth), &tpm2b(&[]), &tpm2b(&[])]),
        ),
    ));
    ok(&seq_update(&mut t, sequence, auth, &input[..14]));
    let complete = |t: &mut Tpm, signature: &[u8]| {
        run(
            t,
            with_pw(
                cc::VERIFY_SEQUENCE_COMPLETE,
                &[sequence, key.handle],
                &[auth],
                signature,
            ),
        )
    };
    err(&complete(&mut t, signature), Rc::SIGNATURE);
    ok(&seq_update(&mut t, sequence, auth, &input[14..]));
    let reply = complete(&mut t, signature);
    let proof = crate::crypto::kdfa_sha256(
        &t.persistent.owner_seed[..32],
        b"ODTPM-SHPROOF",
        &[],
        &[],
        256,
    );
    let tag = u16b(st::MESSAGE_VERIFIED);
    let expected = crate::crypto::hmac_sha256(&proof, &[&tag, input, &key.name]);
    assert_eq!(
        params(&reply, 0),
        cat(&[&tag, &u32b(OWNER), &tpm2b(&expected)])
    );
    err(&complete(&mut t, signature), Rc::reference_handle(0));
}

#[test]
fn restricted_sign_sequence_rejects_a_generated_prefix_without_consuming_the_final_buffer() {
    let mut t = started();
    let restricted = ecc(base() | oa::RESTRICTED | oa::SIGN, SymDef::Null, ECDSA);
    let key = parse_primary(&create_primary(&mut t, PLATFORM, b"key", &restricted)).handle;
    let started = run(
        &mut t,
        plain(
            cc::SIGN_SEQUENCE_START,
            &[key],
            &cat(&[&tpm2b(b"sequence"), &tpm2b(&[])]),
        ),
    );
    let sequence = seq_handle(&started);
    ok(&seq_update(&mut t, sequence, b"sequence", b"\xff"));
    ok(&seq_update(&mut t, sequence, b"sequence", b"TC"));
    let complete = |t: &mut Tpm, data| {
        run(
            t,
            with_pw(
                cc::SIGN_SEQUENCE_COMPLETE,
                &[sequence, key],
                &[b"sequence", b"key"],
                &tpm2b(data),
            ),
        )
    };
    err(&complete(&mut t, b"G"), Rc::VALUE.in_param(1));
    let reply = complete(&mut t, b"X");
    let digest = Sha256::digest(b"\xffTCX");
    ok(&verify(&mut t, key, &digest, params(&reply, 0)));
    err(&complete(&mut t, b"X"), Rc::reference_handle(0));
}

#[test]
fn restricted_sign_digest_requires_a_hashcheck_bound_to_the_first_buffer() {
    let mut t = started();
    let message = b"digest signing test";
    let hashed = run(&mut t, hash_cmd(message, alg::SHA256, OWNER));
    let (digest, expected_ticket) = digest_and_ticket(&hashed);
    assert_eq!(digest.as_slice(), &Sha256::digest(message)[..]);
    assert_eq!(expected_ticket.hierarchy, OWNER);
    assert_eq!(expected_ticket.digest.len(), 32);

    let restricted = ecc(base() | oa::RESTRICTED | oa::SIGN, SymDef::Null, ECDSA);
    let key = parse_primary(&create_primary(&mut t, PLATFORM, b"pw", &restricted)).handle;
    for first_len in [0, 3, 4] {
        let sequence = seq_handle(&start_seq(&mut t, b"", alg::SHA256));
        ok(&seq_update(&mut t, sequence, b"", &message[..first_len]));
        let completed = seq_complete(&mut t, sequence, b"", &message[first_len..], OWNER);
        let (computed, ticket) = digest_and_ticket(&completed);
        assert_eq!(computed, digest);
        let mut w = Writer::new();
        w.tpm2b(&[]);
        w.tpm2b(&digest);
        ticket.marshal(&mut w);
        let reply = run(
            &mut t,
            with_pw(cc::SIGN_DIGEST, &[key], &[b"pw"], &w.finish()),
        );
        if first_len == 4 {
            assert_eq!(ticket, expected_ticket);
            ok(&verify(&mut t, key, &digest, params(&reply, 0)));
        } else {
            assert_eq!(ticket.hierarchy, NULL);
            assert!(ticket.digest.is_empty());
            err(&reply, Rc::TICKET.in_param(3));
        }
    }
}

#[test]
fn verify_digest_signature_uses_the_fixed_scheme_and_encodes_hash_metadata() {
    let mut t = started();
    let digest = hexb(MSG_DIGEST);
    let rsa_sig = rsassa_signature(&hexb(RSA_SIG));
    let ecc_sig = ecdsa_signature(&hexb(EC_R), &hexb(EC_S));
    let verify = |t: &mut Tpm, key, context: &[u8], digest: &[u8], signature: &[u8]| {
        run(
            t,
            plain(
                cc::VERIFY_DIGEST_SIGNATURE,
                &[key],
                &cat(&[&tpm2b(context), &tpm2b(digest), signature]),
            ),
        )
    };
    let (rsa, _) = parse_loaded(&load_external(&mut t, &ext_rsa_public(), NULL));
    let reply = verify(&mut t, rsa, &[], &digest, &rsa_sig);
    assert_eq!(params(&reply, 0), hexb("802740000007000b0000"));

    let (ecc, name) = parse_loaded(&load_external(&mut t, &ext_ecc_public(), OWNER));
    let reply = verify(&mut t, ecc, &[], &digest, &ecc_sig);
    let proof = crate::crypto::kdfa_sha256(
        &t.persistent.owner_seed[..32],
        b"ODTPM-SHPROOF",
        &[],
        &[],
        256,
    );
    let metadata = u16b(alg::SHA256);
    let hmac = crate::crypto::hmac_sha256(&proof, &[&u16b(0x8027), &digest, &name, &metadata]);
    assert_eq!(
        params(&reply, 0),
        cat(&[&hexb("802740000001000b"), &tpm2b(&hmac)])
    );
    err(
        &verify(&mut t, ecc, &[], &digest, &rsa_sig),
        Rc::SCHEME.in_param(3),
    );
    err(
        &verify(&mut t, ecc, &[0], &digest, &ecc_sig),
        Rc::SIZE.in_param(1),
    );
    err(
        &verify(&mut t, ecc, &[], &digest[..31], &ecc_sig),
        Rc::SIZE.in_param(2),
    );
    err(
        &verify(&mut t, ecc, &[], &Sha256::digest(&digest), &ecc_sig),
        Rc::SIGNATURE,
    );

    let mut open = ext_ecc_public();
    let PublicParams::Ecc { scheme, .. } = &mut open.params else {
        unreachable!()
    };
    *scheme = Scheme::NULL;
    let (open, _) = parse_loaded(&load_external(&mut t, &open, NULL));
    err(
        &verify(&mut t, open, &[], &digest, &ecc_sig),
        Rc::SCHEME.in_handle(1),
    );
}

#[test]
fn load_external_ecc_public_key_verifies_an_openssl_signature() {
    let mut t = started();
    let (h, name) = parse_loaded(&load_external(&mut t, &ext_ecc_public(), NULL));
    assert_eq!(h, FIRST_OBJECT);
    assert_eq!(name, name_hash(&ext_ecc_public().to_bytes()));
    let digest = hexb(MSG_DIGEST);
    let sig = ecdsa_signature(&hexb(EC_R), &hexb(EC_S));
    let vr = verify(&mut t, h, &digest, &sig);
    // A key in the Null hierarchy: the NULL verified ticket.
    assert_eq!(
        read_ticket(&mut Reader::new(params(&vr, 0))),
        (0x8022, NULL, vec![])
    );
    // r and s with the leading zeros stripped are accepted as TPM2B values.
    let mut bad = hexb(EC_S);
    bad[31] ^= 1;
    err(
        &verify(&mut t, h, &digest, &ecdsa_signature(&hexb(EC_R), &bad)),
        Rc::SIGNATURE,
    );
    let mut wrong = digest.clone();
    wrong[5] ^= 0x40;
    err(&verify(&mut t, h, &wrong, &sig), Rc::SIGNATURE);
    // The same key in the owner hierarchy returns a real ticket.
    let (h2, _) = parse_loaded(&load_external(&mut t, &ext_ecc_public(), OWNER));
    let vr = verify(&mut t, h2, &digest, &sig);
    let ticket = read_ticket(&mut Reader::new(params(&vr, 0)));
    assert_eq!((ticket.0, ticket.1, ticket.2.len()), (0x8022, OWNER, 32));
    // ReadPublic of an external object returns its public area; its Qualified Name is its Name.
    let rp = read_public(&mut t, h);
    let mut r = Reader::new(params(&rp, 0));
    assert_eq!(r.tpm2b(1024).unwrap(), &ext_ecc_public().to_bytes()[..]);
    let n = r.tpm2b(64).unwrap().to_vec();
    assert_eq!(r.tpm2b(64).unwrap(), &n[..]);
}

#[test]
fn load_external_rsa_public_key_verifies_an_openssl_signature() {
    let mut t = started();
    let (h, name) = parse_loaded(&load_external(&mut t, &ext_rsa_public(), NULL));
    assert_eq!(name, name_hash(&ext_rsa_public().to_bytes()));
    let digest = hexb(MSG_DIGEST);
    let sig = rsassa_signature(&hexb(RSA_SIG));
    ok(&verify(&mut t, h, &digest, &sig));
    let mut bad = hexb(RSA_SIG);
    bad[100] ^= 1;
    err(
        &verify(&mut t, h, &digest, &rsassa_signature(&bad)),
        Rc::SIGNATURE,
    );
    let mut wrong = digest;
    wrong[0] ^= 1;
    err(&verify(&mut t, h, &wrong, &sig), Rc::SIGNATURE);
    // A signature of the wrong length.
    err(
        &verify(
            &mut t,
            h,
            &hexb(MSG_DIGEST),
            &rsassa_signature(&hexb(RSA_SIG)[..255]),
        ),
        Rc::SIGNATURE,
    );
}

#[test]
fn load_external_has_no_authorization_and_cannot_sign_or_persist() {
    let mut t = started();
    let (h, _) = parse_loaded(&load_external(&mut t, &ext_ecc_public(), OWNER));
    // No private part: no authValue to check (TPM_RC_AUTH_UNAVAILABLE), whatever the password.
    err(
        &sign(&mut t, h, b"", &DIGEST, &ecc_sig_scheme()),
        Rc::AUTH_UNAVAILABLE,
    );
    err(
        &run(
            &mut t,
            with_pw(
                cc::CREATE,
                &[h],
                &[b""],
                &creation_params(b"", b"", &ecc_sign(), b"", &NO_PCR),
            ),
        ),
        Rc::AUTH_UNAVAILABLE,
    );
    // Not persistent: public only.
    err(
        &evict(&mut t, OWNER, b"", h, 0x8100_0001),
        Rc::ATTRIBUTES.in_handle(2),
    );
}

#[test]
fn load_external_errors() {
    let mut t = started();
    // A sensitive area (the "crypto accelerator" use) is not implemented.
    let with_private = cat(&[
        &u16b(4),
        &[0, 0, 0, 0],
        &tpm2b(&ext_ecc_public().to_bytes()),
        &u32b(NULL),
    ]);
    err(
        &run(&mut t, plain(cc::LOAD_EXTERNAL, &[], &with_private)),
        Rc::VALUE.in_param(1),
    );
    // A hierarchy that is none of owner, platform, endorsement, null.
    err(
        &load_external(&mut t, &ext_ecc_public(), handle::RH_LOCKOUT),
        Rc::VALUE.in_param(3),
    );
    // A point that is not on the curve.
    let mut off = ext_ecc_public();
    if let Unique::Ecc { y, .. } = &mut off.unique {
        y[31] ^= 1;
    }
    err(
        &load_external(&mut t, &off, NULL),
        Rc::ECC_POINT.in_param(2),
    );
    // A coordinate of the wrong size.
    if let Unique::Ecc { x, .. } = &mut off.unique {
        x.pop();
    }
    err(
        &load_external(&mut t, &off, NULL),
        Rc::ECC_POINT.in_param(2),
    );
    // nameAlg NULL (no Name) is not supported; authPolicy of the wrong size.
    let mut bad = ext_ecc_public();
    bad.name_alg = alg::NULL;
    err(&load_external(&mut t, &bad, NULL), Rc::HASH.in_param(2));
    let mut bad = ext_ecc_public();
    bad.auth_policy = vec![0; 7];
    err(&load_external(&mut t, &bad, NULL), Rc::SIZE.in_param(2));
    // RSA: an even modulus, a short modulus, an exponent other than 65537.
    let mut bad = ext_rsa_public();
    if let Unique::Rsa(n) = &mut bad.unique {
        n[255] &= !1;
    }
    err(&load_external(&mut t, &bad, NULL), Rc::KEY.in_param(2));
    let mut bad = ext_rsa_public();
    if let Unique::Rsa(n) = &mut bad.unique {
        n.pop();
    }
    err(&load_external(&mut t, &bad, NULL), Rc::KEY.in_param(2));
    let mut bad = ext_rsa_public();
    if let PublicParams::Rsa { exponent, .. } = &mut bad.params {
        *exponent = 17;
    }
    err(&load_external(&mut t, &bad, NULL), Rc::RANGE.in_param(2));
    // A sealed-data object without private part is of no use.
    err(
        &load_external(&mut t, &sealed(), NULL),
        Rc::TYPE.in_param(2),
    );
    // A disabled hierarchy.
    ok(&run(
        &mut t,
        with_pw(
            cc::HIERARCHY_CONTROL,
            &[PLATFORM],
            &[b""],
            &cat(&[&u32b(ENDORSEMENT), &[0]]),
        ),
    ));
    err(
        &load_external(&mut t, &ext_ecc_public(), ENDORSEMENT),
        Rc::HIERARCHY.in_param(3),
    );
    // Trailing octets; a session on a command without authorized handles.
    let mut long = cat(&[&u16b(0), &tpm2b(&ext_ecc_public().to_bytes()), &u32b(NULL)]);
    long.push(0);
    err(&run(&mut t, plain(cc::LOAD_EXTERNAL, &[], &long)), Rc::SIZE);
    err(
        &run(
            &mut t,
            with_pw(cc::LOAD_EXTERNAL, &[], &[b""], &long[..long.len() - 1]),
        ),
        Rc::ATTRIBUTES.in_session(1),
    );
    assert_eq!(t.objects.free_slots(), MAX_OBJECTS);
}

// ============================================================================= ReadPublic

#[test]
fn read_public_errors_and_never_returns_private_parts() {
    let (mut t, parent) = with_ecc_parent();
    // Unloaded handle, a sequence object, a PCR handle, a missing persistent handle.
    err(
        &read_public(&mut t, 0x8000_0000 + 30),
        Rc::reference_handle(0),
    );
    ok(&run(
        &mut t,
        plain(
            cc::HASH_SEQUENCE_START,
            &[],
            &cat(&[&tpm2b(b""), &u16b(alg::SHA256)]),
        ),
    ));
    err(&read_public(&mut t, 0x8000_0000), Rc::SEQUENCE);
    err(&read_public(&mut t, 5), Rc::VALUE.in_handle(1));
    err(&read_public(&mut t, 0x8100_0001), Rc::HANDLE.in_handle(1));
    err(
        &read_public(&mut t, handle::RH_OWNER),
        Rc::VALUE.in_handle(1),
    );
    // The response holds the public area, the Name and the Qualified Name and nothing else, for
    // a storage key (its seed value is secret) and for sealed data.
    let rp = read_public(&mut t, parent);
    let secret = secret_of(&t, parent);
    let seed = t
        .object(parent)
        .unwrap()
        .sensitive
        .as_ref()
        .unwrap()
        .seed
        .to_vec();
    assert_eq!(seed.len(), 32);
    assert!(!contains(&rp, &seed));
    assert!(!contains(&rp, &secret));
    let mut r = Reader::new(params(&rp, 0));
    r.tpm2b(1024).unwrap();
    r.tpm2b(64).unwrap();
    r.tpm2b(64).unwrap();
    r.finish().unwrap();
    // Trailing octets in the parameter area.
    err(
        &run(&mut t, plain(cc::READ_PUBLIC, &[parent], &[0])),
        Rc::SIZE,
    );
}

// =============================================================================== Unseal

#[test]
fn seal_and_unseal() {
    let (mut t, parent) = with_ecc_parent();
    let secret = b"correct horse battery staple";
    let child = parse_child(&create_child(
        &mut t,
        parent,
        b"parent-pw",
        b"seal-pw",
        secret,
        &sealed(),
    ));
    // The plaintext is in neither the response nor the protected blob.
    assert!(!contains(&child.private, secret));
    let public = unmarshal_public(&child.public);
    assert!(matches!(public.unique, Unique::Digest(ref d) if d.len() == 32));
    let (item, _) = parse_loaded(&load(
        &mut t,
        parent,
        b"parent-pw",
        &child.private,
        &child.public,
    ));
    let r = run(&mut t, with_pw(cc::UNSEAL, &[item], &[b"seal-pw"], &[]));
    let mut rd = Reader::new(params(&r, 0));
    assert_eq!(rd.tpm2b(128).unwrap(), secret);
    rd.finish().unwrap();
    // Wrong password, no session, trailing octets.
    err(
        &run(&mut t, with_pw(cc::UNSEAL, &[item], &[b"x"], &[])),
        Rc::AUTH_FAIL.in_session(1),
    );
    err(
        &run(&mut t, plain(cc::UNSEAL, &[item], &[])),
        Rc::AUTH_MISSING,
    );
    err(
        &run(&mut t, with_pw(cc::UNSEAL, &[item], &[b"seal-pw"], &[0])),
        Rc::SIZE,
    );
    // A signing key, a storage key and an unloaded handle.
    let sk = parse_child(&create_child(
        &mut t,
        parent,
        b"parent-pw",
        b"s",
        b"",
        &ecc_sign(),
    ));
    let (key, _) = parse_loaded(&load(&mut t, parent, b"parent-pw", &sk.private, &sk.public));
    err(
        &run(&mut t, with_pw(cc::UNSEAL, &[key], &[b"s"], &[])),
        Rc::ATTRIBUTES.in_handle(1),
    );
    err(
        &run(&mut t, with_pw(cc::UNSEAL, &[parent], &[b"parent-pw"], &[])),
        Rc::ATTRIBUTES.in_handle(1),
    );
    err(
        &run(&mut t, with_pw(cc::UNSEAL, &[0x8000_0020], &[b""], &[])),
        Rc::reference_handle(0),
    );
    // Data above 128 octets is refused while unmarshaling the sensitive area.
    err(
        &create_child(&mut t, parent, b"parent-pw", b"", &[1u8; 129], &sealed()),
        Rc::SIZE.in_param(1),
    );
    // A data object without userWithAuth cannot be authorized by a password.
    let mut pol = sealed();
    pol.attributes &= !oa::USER_WITH_AUTH;
    let pk = parse_child(&create_child(
        &mut t,
        parent,
        b"parent-pw",
        b"",
        b"data",
        &pol,
    ));
    let (pkey, _) = parse_loaded(&load(&mut t, parent, b"parent-pw", &pk.private, &pk.public));
    err(
        &run(&mut t, with_pw(cc::UNSEAL, &[pkey], &[b""], &[])),
        Rc::POLICY_FAIL.in_session(1),
    );
}

#[test]
fn a_sealed_primary_unseals_after_it_is_created_again() {
    // The data is part of the derivation: the same data gives the same object, other data another.
    let mut t = started();
    let mk = |t: &mut Tpm, data: &[u8]| {
        let params = creation_params(b"pw", data, &sealed(), b"", &NO_PCR);
        parse_primary(&create_primary_raw(t, OWNER, b"", &params))
    };
    let a = mk(&mut t, b"alpha");
    ok(&flush(&mut t, a.handle));
    let b = mk(&mut t, b"alpha");
    assert_eq!(a.public, b.public);
    ok(&flush(&mut t, b.handle));
    let c = mk(&mut t, b"bravo");
    assert_ne!(a.public, c.public);
    let r = run(&mut t, with_pw(cc::UNSEAL, &[c.handle], &[b"pw"], &[]));
    assert_eq!(Reader::new(params(&r, 0)).tpm2b(128).unwrap(), b"bravo");
}

// ============================================================================ EvictControl

#[test]
fn object_change_auth_rewraps_a_persistent_child_without_changing_it() {
    let mut t = started();
    let parent = parse_primary(&create_primary(&mut t, OWNER, b"parent", &ecc_storage()));
    let parent_alias = 0x8100_0001;
    ok(&evict(&mut t, OWNER, b"", parent.handle, parent_alias));
    let mut template = sealed();
    template.attributes |= oa::NO_DA;
    let child = parse_child(&create_child(
        &mut t,
        parent.handle,
        b"parent",
        b"old",
        b"secret",
        &template,
    ));
    let (loaded, name) = parse_loaded(&load(
        &mut t,
        parent.handle,
        b"parent",
        &child.private,
        &child.public,
    ));
    let target = 0x8100_0002;
    ok(&evict(&mut t, OWNER, b"", loaded, target));
    let before = t.persistent_snapshot();
    let public = read_public(&mut t, target);
    let change = |t: &mut Tpm, auth: &[u8]| {
        pw_cmd(
            t,
            cc::OBJECT_CHANGE_AUTH,
            &[target, parent_alias],
            b"old",
            &tpm2b(auth),
        )
    };
    let result = change(&mut t, b"new\0\0");
    let mut response = Reader::new(params(&result, 0));
    let private = response.tpm2b(1024).unwrap();
    response.finish().unwrap();
    assert_ne!(private, child.private);
    assert_ne!(params(&result, 0), params(&change(&mut t, b"new\0\0"), 0));
    err(&change(&mut t, &[1; 33]), Rc::SIZE.in_param(1));
    err(
        &run(
            &mut t,
            plain(cc::OBJECT_CHANGE_AUTH, &[target, parent_alias], &[0; 2]),
        ),
        Rc::AUTH_MISSING,
    );
    assert_eq!(t.persistent_snapshot(), before);

    ok(&flush(&mut t, loaded));
    let (replacement, new_name) = parse_loaded(&load(
        &mut t,
        parent_alias,
        b"parent",
        private,
        &child.public,
    ));
    assert_eq!(name, new_name);
    assert_eq!(public, read_public(&mut t, replacement));
    let unseal = |t: &mut Tpm, handle, auth: &[u8]| pw_cmd(t, cc::UNSEAL, &[handle], auth, &[]);
    assert_eq!(params(&unseal(&mut t, target, b"old"), 0), tpm2b(b"secret"));
    assert_eq!(
        params(&unseal(&mut t, replacement, b"new"), 0),
        tpm2b(b"secret")
    );
    err(&unseal(&mut t, target, b"new"), Rc::BAD_AUTH.in_session(1));
    err(
        &unseal(&mut t, replacement, b"old"),
        Rc::BAD_AUTH.in_session(1),
    );
    let mut damaged = private.to_vec();
    damaged[3] ^= 1;
    err(
        &load(&mut t, parent_alias, b"parent", &damaged, &child.public),
        Rc::INTEGRITY,
    );
}

#[test]
fn object_change_auth_requires_the_original_storage_parent() {
    let mut t = started();
    let parent = parse_primary(&create_primary(&mut t, OWNER, b"parent", &ecc_storage()));
    let wrong_parent = parse_primary(&create_primary(&mut t, ENDORSEMENT, b"", &ecc_storage()));
    let child = parse_child(&create_child(
        &mut t,
        parent.handle,
        b"parent",
        b"old",
        b"",
        &ecc_sign(),
    ));
    let (child, _) = parse_loaded(&load(
        &mut t,
        parent.handle,
        b"parent",
        &child.private,
        &child.public,
    ));
    let change = |t: &mut Tpm, child, parent, auth: &[u8]| {
        pw_cmd(
            t,
            cc::OBJECT_CHANGE_AUTH,
            &[child, parent],
            auth,
            &tpm2b(b""),
        )
    };
    err(
        &change(&mut t, child, wrong_parent.handle, b"old"),
        Rc::PARENT,
    );
    err(
        &change(&mut t, wrong_parent.handle, parent.handle, b""),
        Rc::PARENT,
    );
    err(&change(&mut t, child, child, b"old"), Rc::TYPE.in_handle(2));
    err(&change(&mut t, child, NULL, b"old"), Rc::VALUE.in_handle(2));
    ok(&flush(&mut t, wrong_parent.handle));
    let mut public = unmarshal_public(&parent.public);
    public.attributes = oa::RESTRICTED | oa::DECRYPT;
    let (external, _) = parse_loaded(&load_external(&mut t, &public, NULL));
    err(
        &change(&mut t, child, external, b"old"),
        Rc::TYPE.in_handle(2),
    );
    err(
        &change(&mut t, external, parent.handle, b""),
        Rc::AUTH_UNAVAILABLE,
    );
}

#[test]
fn object_change_auth_preserves_the_storage_seed_for_existing_descendants() {
    let mut t = started();
    let root = parse_primary(&create_primary(&mut t, OWNER, b"", &ecc_storage()));
    let storage = parse_child(&create_child(
        &mut t,
        root.handle,
        b"",
        b"old",
        b"",
        &ecc_storage(),
    ));
    let (parent, _) = parse_loaded(&load(
        &mut t,
        root.handle,
        b"",
        &storage.private,
        &storage.public,
    ));
    let sealed_child = parse_child(&create_child(
        &mut t,
        parent,
        b"old",
        b"",
        b"leaf",
        &sealed(),
    ));
    let changed = pw_cmd(
        &mut t,
        cc::OBJECT_CHANGE_AUTH,
        &[parent, root.handle],
        b"old",
        &tpm2b(&[7; 32]),
    );
    let private = Reader::new(params(&changed, 0)).tpm2b(1024).unwrap();
    ok(&flush(&mut t, parent));
    let (parent, _) = parse_loaded(&load(&mut t, root.handle, b"", private, &storage.public));
    let (leaf, _) = parse_loaded(&load(
        &mut t,
        parent,
        &[7; 32],
        &sealed_child.private,
        &sealed_child.public,
    ));
    assert_eq!(
        params(&pw_cmd(&mut t, cc::UNSEAL, &[leaf], b"", &[]), 0),
        tpm2b(b"leaf")
    );
}

#[test]
fn evict_control_makes_objects_persistent_and_evicts_them() {
    let mut t = started();
    let pr = parse_primary(&create_primary(&mut t, OWNER, b"pwd", &ecc_sign()));
    let ph = 0x8100_0001;
    ok(&evict(&mut t, OWNER, b"", pr.handle, ph));
    // Both handles name the object: ReadPublic and Sign through the persistent handle.
    let rp = read_public(&mut t, ph);
    assert_eq!(
        Reader::new(params(&rp, 0)).tpm2b(1024).unwrap(),
        &pr.public[..]
    );
    let sig = params(&sign(&mut t, ph, b"pwd", &DIGEST, &ecc_sig_scheme()), 0).to_vec();
    ok(&verify(&mut t, pr.handle, &DIGEST, &sig));
    let (_, _, body, count) = cap_list(&get_cap(&mut t, cap::HANDLES, 0x8100_0000, 10));
    assert_eq!((count, body), (1, u32b(ph)));
    let (_, _, props, _) = cap_list(&get_cap(&mut t, cap::TPM_PROPERTIES, pt::HR_PERSISTENT, 2));
    assert_eq!(&props[4..8], &u32b(1)[..]);
    assert_eq!(&props[12..16], &u32b(MAX_PERSISTENT as u32 - 1)[..]);
    // Flush of the transient copy, Startup and a power cycle leave the persistent object.
    ok(&flush(&mut t, pr.handle));
    ok(&run(&mut t, plain(cc::SHUTDOWN, &[], &[0, 0])));
    t.init();
    ok(&run(&mut t, plain(cc::STARTUP, &[], &[0, 0])));
    ok(&read_public(&mut t, ph));
    ok(&sign(&mut t, ph, b"pwd", &DIGEST, &ecc_sig_scheme()));
    // A persistent handle cannot be flushed.
    err(&flush(&mut t, ph), Rc::VALUE.in_param(1));
    // The handle is taken: TPM_RC_NV_DEFINED.
    let again = parse_primary(&create_primary(&mut t, OWNER, b"", &ecc_sign()));
    err(&evict(&mut t, OWNER, b"", again.handle, ph), Rc::NV_DEFINED);
    // Evict: the two handles must be equal, the object must exist.
    err(
        &evict(&mut t, OWNER, b"", ph, 0x8100_0002),
        Rc::HANDLE.in_param(1),
    );
    ok(&evict(&mut t, OWNER, b"", ph, ph));
    err(&read_public(&mut t, ph), Rc::HANDLE.in_handle(1));
    err(&evict(&mut t, OWNER, b"", ph, ph), Rc::HANDLE.in_handle(2));
    // The transient copy of a persistent object is independent: evicting did not flush `again`.
    ok(&read_public(&mut t, again.handle));
}

#[test]
fn evict_control_ranges_and_hierarchies() {
    let mut t = started();
    let owner_obj = parse_primary(&create_primary(&mut t, OWNER, b"", &ecc_sign())).handle;
    let endorse_obj = parse_primary(&create_primary(&mut t, ENDORSEMENT, b"", &ecc_sign())).handle;
    let platform_obj = parse_primary(&create_primary(&mut t, PLATFORM, b"", &ecc_sign())).handle;
    let null_obj = parse_primary(&create_primary(&mut t, NULL, b"", &ecc_sign())).handle;
    // Owner range 0x81000000..=0x817FFFFF (Part 3, 28.5.1): the first and last value, then the
    // first platform value is out of range for the owner.
    ok(&evict(&mut t, OWNER, b"", owner_obj, 0x8100_0000));
    ok(&evict(&mut t, OWNER, b"", endorse_obj, 0x817F_FFFF));
    err(
        &evict(&mut t, OWNER, b"", owner_obj, 0x8180_0000),
        Rc::RANGE.in_param(1),
    );
    // Platform range 0x81800000..=0x81FFFFFF.
    ok(&evict(&mut t, PLATFORM, b"", platform_obj, 0x8180_0000));
    err(
        &evict(&mut t, PLATFORM, b"", platform_obj, 0x817F_FFFF),
        Rc::RANGE.in_param(1),
    );
    ok(&evict(&mut t, PLATFORM, b"", platform_obj, 0x81FF_FFFF));
    // A value that is no persistent handle at all: TPMI_DH_PERSISTENT, TPM_RC_VALUE.
    for bad in [0x8000_0000u32, 0x8200_0000, 0x0100_0000, 0] {
        err(
            &evict(&mut t, OWNER, b"", owner_obj, bad),
            Rc::VALUE.in_param(1),
        );
    }
    // The wrong hierarchy for the authorization: the platform authorization takes only
    // platform objects, the owner authorization storage and endorsement objects.
    err(
        &evict(&mut t, PLATFORM, b"", owner_obj, 0x8180_0001),
        Rc::HIERARCHY.in_handle(2),
    );
    err(
        &evict(&mut t, OWNER, b"", platform_obj, 0x8100_0001),
        Rc::HIERARCHY.in_handle(2),
    );
    // An object of the Null hierarchy cannot be made persistent.
    err(
        &evict(&mut t, OWNER, b"", null_obj, 0x8100_0001),
        Rc::ATTRIBUTES.in_handle(2),
    );
    // An owner cannot evict an object of the platform range; the platform can evict any.
    err(
        &evict(&mut t, OWNER, b"", 0x8180_0000, 0x8180_0000),
        Rc::RANGE.in_handle(2),
    );
    ok(&evict(&mut t, PLATFORM, b"", 0x8180_0000, 0x8180_0000));
    ok(&evict(&mut t, PLATFORM, b"", 0x81FF_FFFF, 0x81FF_FFFF));
    // stClear objects cannot be made persistent.
    let mut st = ecc_sign();
    st.attributes |= oa::ST_CLEAR;
    let st_obj = parse_primary(&create_primary(&mut t, OWNER, b"", &st)).handle;
    err(
        &evict(&mut t, OWNER, b"", st_obj, 0x8100_0003),
        Rc::ATTRIBUTES.in_handle(2),
    );
}

#[test]
fn evict_control_errors() {
    let mut t = started();
    let obj = parse_primary(&create_primary(&mut t, OWNER, b"", &ecc_sign())).handle;
    // Authorization: wrong password, none, a handle that is no provision handle.
    err(
        &evict(&mut t, OWNER, b"wrong", obj, 0x8100_0001),
        Rc::BAD_AUTH.in_session(1),
    );
    err(
        &run(
            &mut t,
            plain(cc::EVICT_CONTROL, &[OWNER, obj], &u32b(0x8100_0001)),
        ),
        Rc::AUTH_MISSING,
    );
    err(
        &evict(&mut t, ENDORSEMENT, b"", obj, 0x8100_0001),
        Rc::VALUE.in_handle(1),
    );
    // objectHandle: unloaded, a sequence object, a PCR.
    err(
        &evict(&mut t, OWNER, b"", 0x8000_0020, 0x8100_0001),
        Rc::reference_handle(1),
    );
    err(
        &evict(&mut t, OWNER, b"", 3, 0x8100_0001),
        Rc::VALUE.in_handle(2),
    );
    // Parameter framing.
    err(
        &run(
            &mut t,
            with_pw(cc::EVICT_CONTROL, &[OWNER, obj], &[b""], &[0, 0]),
        ),
        Rc::INSUFFICIENT.in_param(1),
    );
    err(
        &run(
            &mut t,
            with_pw(
                cc::EVICT_CONTROL,
                &[OWNER, obj],
                &[b""],
                &[0x81, 0, 0, 1, 0],
            ),
        ),
        Rc::SIZE,
    );
    // Nothing became persistent.
    assert_eq!(t.persistent.objects.len(), 0);
    // The persistent store has a capacity.
    for i in 0..MAX_PERSISTENT as u32 {
        ok(&evict(&mut t, OWNER, b"", obj, 0x8100_0010 + i));
    }
    err(&evict(&mut t, OWNER, b"", obj, 0x8100_0100), Rc::NV_SPACE);
}

#[test]
fn hierarchy_changes_flush_or_disable_objects() {
    let mut t = started();
    let owner_obj = parse_primary(&create_primary(&mut t, OWNER, b"", &ecc_sign())).handle;
    let endorse_obj = parse_primary(&create_primary(&mut t, ENDORSEMENT, b"", &ecc_sign())).handle;
    let platform_obj = parse_primary(&create_primary(&mut t, PLATFORM, b"", &ecc_sign())).handle;
    let null_obj = parse_primary(&create_primary(&mut t, NULL, b"", &ecc_sign())).handle;
    ok(&evict(&mut t, OWNER, b"", owner_obj, 0x8100_0001));
    // Disabling the storage hierarchy flushes its transient objects; the persistent object stays
    // but cannot be used.
    let control = |t: &mut Tpm, which: u32, state: u8| {
        run(
            t,
            with_pw(
                cc::HIERARCHY_CONTROL,
                &[PLATFORM],
                &[b""],
                &cat(&[&u32b(which), &[state]]),
            ),
        )
    };
    ok(&control(&mut t, OWNER, 0));
    err(&read_public(&mut t, owner_obj), Rc::reference_handle(0));
    err(
        &read_public(&mut t, 0x8100_0001),
        Rc::HIERARCHY.in_handle(1),
    );
    ok(&read_public(&mut t, endorse_obj));
    ok(&read_public(&mut t, platform_obj));
    ok(&read_public(&mut t, null_obj));
    // Enabling it again brings the persistent object back.
    ok(&control(&mut t, OWNER, 1));
    ok(&read_public(&mut t, 0x8100_0001));
    // TPM2_Clear flushes the objects (transient and persistent) of the storage and endorsement
    // hierarchies and keeps the platform and null objects.
    let obj2 = parse_primary(&create_primary(&mut t, OWNER, b"", &ecc_sign())).handle;
    ok(&run(&mut t, with_pw(cc::CLEAR, &[PLATFORM], &[b""], &[])));
    err(&read_public(&mut t, obj2), Rc::reference_handle(0));
    err(&read_public(&mut t, endorse_obj), Rc::reference_handle(0));
    err(&read_public(&mut t, 0x8100_0001), Rc::HANDLE.in_handle(1));
    ok(&read_public(&mut t, platform_obj));
    ok(&read_public(&mut t, null_obj));
}

#[test]
fn startup_and_power_cycle_flush_transient_objects() {
    let mut t = started();
    let a = parse_primary(&create_primary(&mut t, OWNER, b"", &ecc_sign())).handle;
    ok(&run(&mut t, plain(cc::SHUTDOWN, &[], &[0, 1])));
    t.init();
    ok(&run(&mut t, plain(cc::STARTUP, &[], &[0, 1])));
    err(&read_public(&mut t, a), Rc::reference_handle(0));
    let a = parse_primary(&create_primary(&mut t, OWNER, b"", &ecc_sign())).handle;
    ok(&flush(&mut t, a));
    err(&flush(&mut t, a), Rc::HANDLE.in_param(1));
    let b = parse_primary(&create_primary(&mut t, OWNER, b"", &ecc_sign())).handle;
    ok(&run(&mut t, plain(cc::SHUTDOWN, &[], &[0, 0])));
    t.init();
    ok(&run(&mut t, plain(cc::STARTUP, &[], &[0, 0])));
    err(&read_public(&mut t, b), Rc::reference_handle(0));
}

// ========================================================================= GetCapability

#[test]
fn capability_lists_algorithms_curves_and_object_handles() {
    let mut t = started();
    // TPM_CAP_ECC_CURVES: NIST P-256 only (TPML_ECC_CURVE).
    let (more, c, body, count) = cap_list(&get_cap(&mut t, cap::ECC_CURVES, 0, 10));
    assert!(!more && c == cap::ECC_CURVES);
    assert_eq!((count, body), (1, u16b(curve::NIST_P256)));
    assert_eq!(cap_list(&get_cap(&mut t, cap::ECC_CURVES, 4, 10)).3, 0);
    // Transient handles: loaded sequence and key objects, ascending.
    ok(&run(
        &mut t,
        plain(
            cc::HASH_SEQUENCE_START,
            &[],
            &cat(&[&tpm2b(b""), &u16b(alg::SHA256)]),
        ),
    ));
    let k = parse_primary(&create_primary(&mut t, OWNER, b"", &ecc_sign())).handle;
    let (_, _, body, count) = cap_list(&get_cap(&mut t, cap::HANDLES, 0x8000_0000, 20));
    assert_eq!(count, 2);
    assert_eq!(body, [&u32b(0x8000_0000)[..], &u32b(k)].concat());
    // The handles of the loaded objects are in the command table's response of GetCapability and
    // free transient slots shrink (3 sequence + 8 object slots).
    let (_, _, v, _) = cap_list(&get_cap(
        &mut t,
        cap::TPM_PROPERTIES,
        pt::HR_TRANSIENT_AVAIL,
        1,
    ));
    assert_eq!(&v[4..8], &u32b(11 - 2)[..]);
}

// ========================================================================= robustness

#[test]
fn mutated_object_commands_never_panic() {
    let (mut t, parent) = with_ecc_parent();
    let child = parse_child(&create_child(
        &mut t,
        parent,
        b"parent-pw",
        b"c",
        b"",
        &ecc_sign(),
    ));
    let (key, _) = parse_loaded(&load(
        &mut t,
        parent,
        b"parent-pw",
        &child.private,
        &child.public,
    ));
    let sig = params(&sign(&mut t, key, b"c", &DIGEST, &ecc_sig_scheme()), 0).to_vec();
    let seeds = vec![
        with_pw(
            cc::CREATE_PRIMARY,
            &[OWNER],
            &[b""],
            &creation_params(b"", b"", &ecc_sign(), b"", &NO_PCR),
        ),
        with_pw(
            cc::CREATE,
            &[parent],
            &[b"parent-pw"],
            &creation_params(b"a", b"", &ecc_sign(), b"o", &NO_PCR),
        ),
        with_pw(
            cc::LOAD,
            &[parent],
            &[b"parent-pw"],
            &cat(&[&tpm2b(&child.private), &tpm2b(&child.public)]),
        ),
        plain(
            cc::LOAD_EXTERNAL,
            &[],
            &cat(&[&u16b(0), &tpm2b(&ext_ecc_public().to_bytes()), &u32b(NULL)]),
        ),
        plain(cc::READ_PUBLIC, &[key], &[]),
        with_pw(cc::UNSEAL, &[key], &[b"c"], &[]),
        with_pw(cc::EVICT_CONTROL, &[OWNER, key], &[b""], &u32b(0x8100_0001)),
        with_pw(
            cc::SIGN,
            &[key],
            &[b"c"],
            &sign_params(&DIGEST, &ecc_sig_scheme()),
        ),
        plain(cc::VERIFY_SIGNATURE, &[key], &cat(&[&tpm2b(&DIGEST), &sig])),
    ];
    let mut state = 0x9E37_79B9u32;
    let mut next = move || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 16) as usize
    };
    for seed in &seeds {
        for len in 0..=seed.len() {
            let _ = run(&mut t, seed[..len].to_vec());
        }
        for _ in 0..150 {
            let mut c = seed.clone();
            for _ in 0..1 + next() % 3 {
                let i = next() % c.len();
                c[i] = next() as u8;
            }
            if next() % 2 == 0 {
                let n = c.len() as u32;
                c[2..6].copy_from_slice(&n.to_be_bytes());
            }
            // Keep the cheap path: a mutation into an RSA template would generate a key.
            let _ = run(&mut t, c);
            // The mutations break passwords: keep DA from locking the key out (`da.rs`).
            t.persistent.da.failed_tries = 0;
        }
        // The TPM is still consistent: the parent signs.
        ok(&sign(&mut t, key, b"c", &DIGEST, &ecc_sig_scheme()));
        // Free what the mutations created so that the next round is not starved of slots.
        for slot in 0..MAX_OBJECTS as u32 {
            let h = FIRST_OBJECT + slot;
            if h != parent && h != key {
                let _ = flush(&mut t, h);
            }
        }
    }
}

// Golden primaries: created by this TPM from the entropy `[5; 48]` (owner hierarchy, template
// `ecc_sign`/`rsa_sign`, authValue "gold") and signed over MSG_DIGEST on 2026-11-07; each
// signature was then verified with plain Python integer arithmetic (an affine P-256 ECDSA
// verification, `pow(S, 65537, N)` against the EMSA-PKCS1-v1_5 block), not with the crates under
// test. The test pins the derivation of primary keys (module comment of `object.rs`): a change of
// the derivation, of the DRBG or of the prime search of `rsa`/`crypto-primes` changes the keys.
const GOLD_ECC_X: &str = "ab6972f04d961e2fa7c41d75ad795f2b65e163f7869b62ccb7d0c08cadbc1c67";
const GOLD_ECC_Y: &str = "c10a88c646b0bbcc39f09a48650f3c4f079e41cb411d0a905abd33d2c7631934";
const GOLD_ECC_SIG: &str = "0018000b00205a9aac6421d23dcb7b76eaf0027178d7ac6782f96829baf57ddd0da39701474800209819017cca6857577e1085f348ccf55b105f0f4138dbf4c597cf53b71439c45a";
const GOLD_RSA_N: &str = "f4de71590602729f83acce9fce9c8c0d6dae509e07345644fc0da10faf5577825d55f87a69666b3130f128c23b8475f3898b81d641a982f9fe9cb0d9df823dfb808693b9d3596a5a341fb7c7c51877035cb58f9ded234df7c49d1bc86085012350dca74e369baec646e34cade7bf51976e2668f6e2bff3f745b1002a0d47ca698eae55a77b1dd192baf179cfcb8d523d06314194c233fa16713a6d1791e0d68c6f2d60887e88dad76ce791b8106b01cb72b598cb4b7637557a9140164b660923082cc405bba54e87d970603f40630df2e76f8b050326c8b4c459cd0e79204616ca65ac560d825e97d12fef64684625d4b34765185158bd0425a40f06b9081d55";
const GOLD_RSA_SIG: &str = "0014000b010098000ee9c5ba9b66cf34c75613a5b2c0d1673205907a7568bf3a618cbac9702f13171fd7124de51f38a97ae787b65630151637545eca09bcb000742380644a7a62dad9c9439a6c7077bf8f6d2e47f338955aca0887bc57328c50b10baf756c349bf4f4e6c0ede7986d0743a4cc478fd84a52ccab01366a2808b37dfc31ee7d98a2885d63e6de4b46719c7204e12dca85e42f244b00575dd22136fb5ba6e275a1815135c4c8d382ff6e7fd750f32b189501ed6bc9500651ab086716f30ddf29dce4e9d1292a0f38b51f9b736acde2fc92b87bad0f6e9fa32ece425fd437713f96b08b5fc1759c8b8b121257032a59ad078e1ad9df5772a94a914b5691572193f8";

#[test]
fn golden_primaries_are_pinned_and_their_signatures_verify_externally() {
    let mut t = started();
    let digest = hexb(MSG_DIGEST);
    let pr = parse_primary(&create_primary(&mut t, OWNER, b"gold", &ecc_sign()));
    let Unique::Ecc { x, y } = unmarshal_public(&pr.public).unique else {
        panic!()
    };
    assert_eq!((x, y), (hexb(GOLD_ECC_X), hexb(GOLD_ECC_Y)));
    let sig = sign(&mut t, pr.handle, b"gold", &digest, &ecc_sig_scheme());
    assert_eq!(params(&sig, 0), &hexb(GOLD_ECC_SIG)[..]);
    let pr = parse_primary(&create_primary(&mut t, OWNER, b"gold", &rsa_sign()));
    let Unique::Rsa(n) = unmarshal_public(&pr.public).unique else {
        panic!()
    };
    assert_eq!(n, hexb(GOLD_RSA_N));
    let sig = sign(&mut t, pr.handle, b"gold", &digest, &rsa_sig_scheme());
    assert_eq!(params(&sig, 0), &hexb(GOLD_RSA_SIG)[..]);
}
