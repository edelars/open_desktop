//! SYMCIPHER objects and restricted ML-KEM-768 Storage parents with the labeled KEM (S7-23,
//! slice 5): Part 1 v185 clauses 8.4.5.2 (Table 5), 24.5.3.3, 24.7.5.1 and 47.4, Part 2 v185
//! Tables 165, 232 and 233, Part 3 v185 clause 12.1.
//!
//! `sha256_golden_unchanged` was recorded at HEAD a5fb76c, before this slice changed any code: the
//! TPM is deterministic (fixed entropy), so the SHA-256 of one transcript of the existing RSA, ECC,
//! unrestricted ML-KEM, keyed-hash, credential, duplication and snapshot paths pins their bytes.

use sha2::{Digest, Sha256, Sha384, Sha512};

use super::tests_b::{kdfa_256, start_cmd, start_params};
use super::tests_obj::{
    Child, DIGEST, NO_PCR, Primary, base, contains, create_child, create_primary,
    create_primary_raw, ecc, ecc_sign, ecc_storage, evict, flush, load, load_external, name_hash,
    parse_child, read_public, rsa_storage, sealed, secret_of, sign, unmarshal_public, verify,
};
use super::*;
use crate::codes::{object_attr as oa, se};
use crate::dispatch::Invocation;
use crate::marshal::{PublicParams, Scheme, SymDef, TpmtPublic, Unique, Writer, limits};
use crate::object::{Secret, Sensitive, sensitive_from_bytes, sensitive_to_bytes};
use zeroize::Zeroizing;

const OWNER: u32 = handle::RH_OWNER;
const NULL: u32 = handle::RH_NULL;

fn mlkem_unrestricted() -> TpmtPublic {
    TpmtPublic {
        name_alg: alg::SHA256,
        attributes: base() | oa::DECRYPT,
        auth_policy: vec![],
        params: PublicParams::MlKem {
            symmetric: SymDef::Null,
            parameter_set: 2,
        },
        unique: Unique::MlKem(vec![]),
    }
}

fn handle_of(resp: &[u8]) -> u32 {
    ok(resp);
    u32::from_be_bytes(resp[10..14].try_into().unwrap())
}

fn loaded(resp: &[u8]) -> (u32, Vec<u8>) {
    ok(resp);
    let mut r = Reader::new(params(resp, 1));
    let name = r.tpm2b(66).unwrap().to_vec();
    r.finish().unwrap();
    (u32::from_be_bytes(resp[10..14].try_into().unwrap()), name)
}

fn movable(mut public: TpmtPublic) -> TpmtPublic {
    public.attributes &= !(oa::FIXED_TPM | oa::FIXED_PARENT);
    public
}

/// `TPM2_MakeCredential` (no authorization): the whole reply.
fn make_credential_raw(t: &mut Tpm, key: u32, credential: &[u8], name: &[u8]) -> Vec<u8> {
    run(
        t,
        plain(
            cc::MAKE_CREDENTIAL,
            &[key],
            &cat(&[&tpm2b(credential), &tpm2b(name)]),
        ),
    )
}

/// The replies of one scenario over the paths that exist at HEAD a5fb76c.
fn golden_transcript() -> Vec<u8> {
    let mut t = started();
    let mut log: Vec<u8> = Vec::new();
    let mut put = |reply: &[u8]| {
        log.extend_from_slice(&(reply.len() as u32).to_be_bytes());
        log.extend_from_slice(reply);
    };
    let rsa_parent = create_primary(&mut t, OWNER, b"rsa-pw", &rsa_storage());
    put(&rsa_parent);
    let rsa_handle = handle_of(&rsa_parent);
    let ecc_parent = create_primary(&mut t, OWNER, b"ecc-pw", &ecc_storage());
    put(&ecc_parent);
    let ecc_handle = handle_of(&ecc_parent);
    let kem = create_primary(&mut t, OWNER, b"kem-pw", &mlkem_unrestricted());
    put(&kem);
    put(&flush(&mut t, handle_of(&kem)));
    // Children of both storage keys: an ECDSA key, a sealed blob, an ML-KEM key.
    let mut movable_children = Vec::new();
    for (parent, pw) in [(rsa_handle, &b"rsa-pw"[..]), (ecc_handle, &b"ecc-pw"[..])] {
        for (template, data) in [
            (movable(ecc_sign()), &b""[..]),
            (movable(sealed()), &b"sealed data"[..]),
            (movable(mlkem_unrestricted()), &b""[..]),
        ] {
            let created = create_child(&mut t, parent, pw, b"child-pw", data, &template);
            put(&created);
            let child = parse_child(&created);
            let reply = load(&mut t, parent, pw, &child.private, &child.public);
            put(&reply);
            movable_children.push((parent, loaded(&reply), child.public));
        }
    }
    // Credentials for both parent types.
    for parent in [rsa_handle, ecc_handle] {
        let made = make_credential_raw(&mut t, parent, b"credential", &[0, 0xB, 7, 7, 7, 7]);
        put(&made);
        let mut r = Reader::new(params(&made, 0));
        let blob = r.tpm2b(200).unwrap().to_vec();
        let secret = r.tpm2b(1024).unwrap().to_vec();
        let (activated, _) = movable_children[0].1.clone();
        let reply = t.cmd_activate_credential(
            &Invocation {
                handles: [activated, parent, 0],
            },
            &mut Reader::new(&cat(&[&tpm2b(&blob), &tpm2b(&secret)])),
        );
        put(&reply
            .map(|o| o.params)
            .unwrap_or_else(|rc| rc.0.to_be_bytes().to_vec()));
    }
    // Duplication of every movable child to the other storage key, then Import.
    for (parent, (object, _), public) in movable_children.clone() {
        let new_parent = if parent == rsa_handle {
            ecc_handle
        } else {
            rsa_handle
        };
        for inner in [SymDef::Null, SymDef::Aes128Cfb, SymDef::Aes256Cfb] {
            let mut w = Writer::new();
            w.tpm2b(&[]);
            inner.marshal(&mut w);
            let Ok(out) = t.cmd_duplicate(
                &Invocation {
                    handles: [object, new_parent, 0],
                },
                &mut Reader::new(&w.finish()),
            ) else {
                put(b"duplicate refused");
                continue;
            };
            put(&out.params);
            let mut r = Reader::new(&out.params);
            let key = r.tpm2b(32).unwrap().to_vec();
            let blob = r.tpm2b(1024).unwrap().to_vec();
            let secret = r.tpm2b(1024).unwrap().to_vec();
            let pw: &[u8] = if new_parent == rsa_handle {
                b"rsa-pw"
            } else {
                b"ecc-pw"
            };
            let mut inner_bytes = Writer::new();
            inner.marshal(&mut inner_bytes);
            put(&run(
                &mut t,
                with_pw(
                    cc::IMPORT,
                    &[new_parent],
                    &[pw],
                    &cat(&[
                        &tpm2b(&key),
                        &tpm2b(&public),
                        &tpm2b(&blob),
                        &tpm2b(&secret),
                        &inner_bytes.finish(),
                    ]),
                ),
            ));
        }
    }
    // Sessions: unsalted, and salted with a bad secret for both key types.
    let session = |key: u32, salt: &[u8]| {
        plain(
            cc::START_AUTH_SESSION,
            &[key, handle::RH_NULL],
            &cat(&[
                &tpm2b(&[0x44; 16]),
                &tpm2b(salt),
                &[se::HMAC],
                &u16b(alg::NULL),
                &u16b(alg::SHA256),
            ]),
        )
    };
    put(&run(&mut t, session(handle::RH_NULL, &[])));
    put(&run(&mut t, session(rsa_handle, &[7; 256])));
    put(&run(&mut t, session(ecc_handle, &[7; 4])));
    // The snapshot carries every seed.
    put(&t.persistent_snapshot());
    log
}

#[test]
fn sha256_golden_unchanged() {
    let digest = Sha256::digest(golden_transcript());
    let hexed: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(
        hexed, "7852ab0a624f648fc1cf91bc5182c7610ffc5440b2d4cb8e4213109f88e78d3d",
        "golden transcript digest"
    );
}

// ------------------------------------------------------------------------------- fixtures

const KEM_AUTH: &[u8] = b"kem-parent";

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The digest of `name_alg` over `parts`, computed here with the `sha2` crate and not by the TPM.
fn digest_of(name_alg: u16, parts: &[&[u8]]) -> Vec<u8> {
    fn over<D: Digest>(parts: &[&[u8]]) -> Vec<u8> {
        let mut d = D::new();
        for part in parts {
            d.update(part);
        }
        d.finalize().to_vec()
    }
    match name_alg {
        alg::SHA256 => over::<Sha256>(parts),
        alg::SHA384 => over::<Sha384>(parts),
        alg::SHA512 => over::<Sha512>(parts),
        _ => unreachable!("a hash"),
    }
}

fn digest_size(name_alg: u16) -> usize {
    digest_of(name_alg, &[]).len()
}

fn mlkem_template(attributes: u32, symmetric: SymDef, name_alg: u16) -> TpmtPublic {
    TpmtPublic {
        name_alg,
        attributes,
        auth_policy: vec![],
        params: PublicParams::MlKem {
            symmetric,
            parameter_set: 2,
        },
        unique: Unique::MlKem(vec![]),
    }
}

/// A restricted decryption ML-KEM-768 Storage parent.
fn mlkem_storage(symmetric: SymDef, name_alg: u16) -> TpmtPublic {
    mlkem_template(base() | oa::RESTRICTED | oa::DECRYPT, symmetric, name_alg)
}

fn symcipher(attributes: u32, symmetric: SymDef, name_alg: u16) -> TpmtPublic {
    TpmtPublic {
        name_alg,
        attributes,
        auth_policy: vec![],
        params: PublicParams::SymCipher { symmetric },
        unique: Unique::Digest(vec![]),
    }
}

/// A data key: unrestricted, `sign` (encrypt) and `decrypt`.
fn sym_leaf(symmetric: SymDef, name_alg: u16) -> TpmtPublic {
    symcipher(base() | oa::SIGN | oa::DECRYPT, symmetric, name_alg)
}

/// A Storage parent: restricted decryption.
fn sym_storage(symmetric: SymDef) -> TpmtPublic {
    symcipher(
        base() | oa::RESTRICTED | oa::DECRYPT,
        symmetric,
        alg::SHA256,
    )
}

/// A started TPM with the SHA-256 / AES-128 ML-KEM Storage primary of the owner hierarchy.
fn kem_fixture() -> (Tpm, Primary) {
    let mut t = started();
    let primary = parse_primary(&create_primary(
        &mut t,
        OWNER,
        KEM_AUTH,
        &mlkem_storage(SymDef::Aes128Cfb, alg::SHA256),
    ));
    (t, primary)
}

/// Creates and loads a child; `(handle, Name, creation reply)`.
/// `parse_loaded` of the sibling tests with room for a SHA-512 Name (66 octets).
fn parse_loaded(resp: &[u8]) -> (u32, Vec<u8>) {
    ok(resp);
    let handle = u32::from_be_bytes([resp[10], resp[11], resp[12], resp[13]]);
    let mut r = Reader::new(params(resp, 1));
    let name = r.tpm2b(66).unwrap().to_vec();
    r.finish().unwrap();
    (handle, name)
}

/// `parse_primary` of the sibling tests with room for a SHA-512 Name (66 octets).
fn parse_primary(resp: &[u8]) -> Primary {
    ok(resp);
    let handle = u32::from_be_bytes([resp[10], resp[11], resp[12], resp[13]]);
    let mut r = Reader::new(params(resp, 1));
    let public = r.tpm2b(limits::MAX_RESPONSE_SIZE).unwrap().to_vec();
    let creation = r.tpm2b(1024).unwrap().to_vec();
    let hash = r.tpm2b(66).unwrap().to_vec();
    let ticket = (
        r.u16().unwrap(),
        r.u32().unwrap(),
        r.tpm2b(66).unwrap().to_vec(),
    );
    let name = r.tpm2b(66).unwrap().to_vec();
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

fn make_child(
    t: &mut Tpm,
    parent: u32,
    pw: &[u8],
    template: &TpmtPublic,
    auth: &[u8],
    data: &[u8],
) -> (u32, Vec<u8>, Child) {
    let created = create_child(t, parent, pw, auth, data, template);
    ok(&created);
    let child = parse_child(&created);
    let loaded = load(t, parent, pw, &child.private, &child.public);
    ok(&loaded);
    let (handle, name) = parse_loaded(&loaded);
    (handle, name, child)
}

fn sign_and_verify(t: &mut Tpm, key: u32, pw: &[u8]) {
    let reply = sign(t, key, pw, &DIGEST, &u16b(alg::NULL));
    ok(&reply);
    let signature = params(&reply, 0).to_vec();
    ok(&verify(t, key, &DIGEST, &signature));
}

fn unseal(t: &mut Tpm, key: u32, pw: &[u8]) -> Vec<u8> {
    let reply = run(t, with_pw(cc::UNSEAL, &[key], &[pw], &[]));
    ok(&reply);
    let mut r = Reader::new(params(&reply, 0));
    let data = r.tpm2b(128).unwrap().to_vec();
    r.finish().unwrap();
    data
}

fn seed_value(t: &Tpm, handle: u32) -> Vec<u8> {
    t.object(handle)
        .unwrap()
        .sensitive
        .as_ref()
        .unwrap()
        .seed
        .to_vec()
}

fn raw_creation(public: &[u8], auth: &[u8], data: &[u8]) -> Vec<u8> {
    let sensitive = cat(&[&tpm2b(auth), &tpm2b(data)]);
    cat(&[&tpm2b(&sensitive), &tpm2b(public), &tpm2b(&[]), &NO_PCR])
}

fn movable_sealed() -> TpmtPublic {
    movable(sealed())
}

// ----------------------------------------------------------------- scenarios behind the pins

/// AES-256 SYMCIPHER Storage primary and a sealed child under it: the parent's seedValue and the
/// child's `TPM2B_PUBLIC` contents and `TPM2B_PRIVATE` contents.
fn sym_parent_vector() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut t = started();
    let parent = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"sym-parent",
        &sym_storage(SymDef::Aes256Cfb),
    ));
    let created = create_child(
        &mut t,
        parent.handle,
        b"sym-parent",
        b"seal-pw",
        b"python vector",
        &sealed(),
    );
    ok(&created);
    let child = parse_child(&created);
    (seed_value(&t, parent.handle), child.public, child.private)
}

/// A sealed child under the AES-128 ML-KEM Storage primary (the protection of a child does not
/// involve the KEM): the parent's seedValue, the child's public and private contents.
fn kem_parent_vector() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (mut t, parent) = kem_fixture();
    let created = create_child(
        &mut t,
        parent.handle,
        KEM_AUTH,
        b"seal-pw",
        b"python vector",
        &sealed(),
    );
    ok(&created);
    let child = parse_child(&created);
    (seed_value(&t, parent.handle), child.public, child.private)
}

/// `TPM2_MakeCredential` for the ML-KEM primary and the Name of an ECDSA primary:
/// `(credentialBlob, secret, Name)`.
fn made_credential() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (mut t, kem) = kem_fixture();
    let activated = parse_primary(&create_primary(&mut t, OWNER, b"act-pw", &ecc_sign()));
    let reply = make_credential(&mut t, kem.handle, CREDENTIAL, &activated.name);
    ok(&reply);
    let mut r = Reader::new(params(&reply, 0));
    let blob = r.tpm2b(200).unwrap().to_vec();
    let secret = r.tpm2b(1088).unwrap().to_vec();
    r.finish().unwrap();
    (blob, secret, activated.name)
}

const CREDENTIAL: &[u8] = b"tpm made credential";

fn make_credential(t: &mut Tpm, key: u32, credential: &[u8], name: &[u8]) -> Vec<u8> {
    run(
        t,
        plain(
            cc::MAKE_CREDENTIAL,
            &[key],
            &cat(&[&tpm2b(credential), &tpm2b(name)]),
        ),
    )
}

/// `TPM2_ActivateCredential` without the authorizations: the credential or the error.
fn activate_credential(
    t: &mut Tpm,
    activated: u32,
    key: u32,
    blob: &[u8],
    secret: &[u8],
) -> Result<Vec<u8>, Rc> {
    let out = t.cmd_activate_credential(
        &Invocation {
            handles: [activated, key, 0],
        },
        &mut Reader::new(&cat(&[&tpm2b(blob), &tpm2b(secret)])),
    )?;
    let mut r = Reader::new(&out.params);
    let credential = r.tpm2b(64).unwrap().to_vec();
    r.finish().unwrap();
    Ok(credential)
}

/// `(encryptionKeyOut, duplicate, outSymSeed)` of `TPM2_Duplicate`.
type Duplicated = (Vec<u8>, Vec<u8>, Vec<u8>);

/// `TPM2_Duplicate` without its policy authorization: `(encryptionKeyOut, duplicate, outSymSeed)`.
fn duplicate(
    t: &mut Tpm,
    object: u32,
    new_parent: u32,
    key_in: &[u8],
    symmetric: SymDef,
) -> Result<Duplicated, Rc> {
    let mut w = Writer::new();
    w.tpm2b(key_in);
    symmetric.marshal(&mut w);
    let out = t.cmd_duplicate(
        &Invocation {
            handles: [object, new_parent, 0],
        },
        &mut Reader::new(&w.finish()),
    )?;
    let mut r = Reader::new(&out.params);
    let key = r.tpm2b(32).unwrap().to_vec();
    let blob = r.tpm2b(1024).unwrap().to_vec();
    let secret = r.tpm2b(1088).unwrap().to_vec();
    r.finish().unwrap();
    Ok((key, blob, secret))
}

fn import(
    t: &mut Tpm,
    parent: u32,
    pw: &[u8],
    (key, public, blob, secret): (&[u8], &[u8], &[u8], &[u8]),
    symmetric: SymDef,
) -> Vec<u8> {
    let mut sym = Writer::new();
    symmetric.marshal(&mut sym);
    run(
        t,
        with_pw(
            cc::IMPORT,
            &[parent],
            &[pw],
            &cat(&[
                &tpm2b(key),
                &tpm2b(public),
                &tpm2b(blob),
                &tpm2b(secret),
                &sym.finish(),
            ]),
        ),
    )
}

/// A source ECC Storage primary, a movable sealed child, and the duplicate of it for the ML-KEM
/// primary: `(child public, duplicate, outSymSeed)`.
fn duplicated_for_kem() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (mut t, kem) = kem_fixture();
    let source = parse_primary(&create_primary(&mut t, OWNER, b"src-pw", &ecc_storage()));
    let (object, _, child) = make_child(
        &mut t,
        source.handle,
        b"src-pw",
        &movable_sealed(),
        b"obj-pw",
        b"duplicated secret",
    );
    let (key, blob, secret) = duplicate(&mut t, object, kem.handle, &[], SymDef::Null).unwrap();
    assert!(key.is_empty());
    (child.public, blob, secret)
}

const NONCE_CALLER: [u8; 16] = [0x33; 16];

fn start_salted(t: &mut Tpm, key: u32, salt: &[u8]) -> Vec<u8> {
    run(
        t,
        start_cmd(
            key,
            NULL,
            &start_params(&NONCE_CALLER, salt, se::HMAC, &u16b(alg::NULL), alg::SHA256),
        ),
    )
}

/// `(session handle, nonceTPM)` of a successful `TPM2_StartAuthSession`.
fn session_of(resp: &[u8]) -> (u32, Vec<u8>) {
    ok(resp);
    let mut r = Reader::new(&resp[14..]);
    let nonce = r.tpm2b(64).unwrap().to_vec();
    r.finish().unwrap();
    (
        u32::from_be_bytes([resp[10], resp[11], resp[12], resp[13]]),
        nonce,
    )
}

fn session_key(t: &Tpm, handle: u32) -> Vec<u8> {
    let slot = t.sessions.slot_of(handle).unwrap();
    t.sessions.get(slot).unwrap().key.as_slice().to_vec()
}

// ------------------------------------------------------------------------------------ SYMCIPHER

#[test]
fn symcipher_aes128_and_256_create_load_and_name() {
    for (symmetric, bits, name_alg) in [
        (SymDef::Aes128Cfb, 128u16, alg::SHA256),
        (SymDef::Aes256Cfb, 256, alg::SHA256),
        (SymDef::Aes256Cfb, 256, alg::SHA384),
        (SymDef::Aes128Cfb, 128, alg::SHA512),
    ] {
        let label = format!("AES-{bits}-CFB, nameAlg {name_alg:#06x}");
        let key_bytes = usize::from(bits / 8);
        let digest_len = digest_size(name_alg);
        let template = sym_leaf(symmetric, name_alg);
        // TPMT_PUBLIC by hand (Part 2 Tables 165, 233 and 236): type, nameAlg, attributes, empty
        // authPolicy, TPMT_SYM_DEF_OBJECT (AES, keyBits, CFB), then unique as a TPM2B_DIGEST.
        let header = cat(&[
            &u16b(0x0025),
            &u16b(name_alg),
            &u32b(template.attributes),
            &u16b(0),
            &u16b(alg::AES),
            &u16b(bits),
            &u16b(alg::CFB),
            &u16b(digest_len as u16),
        ]);
        assert_eq!(
            template.to_bytes()[..header.len() - 2],
            header[..header.len() - 2]
        );

        let mut t = started();
        let reply = create_primary(&mut t, OWNER, b"sym-pw", &template);
        let primary = parse_primary(&reply);
        assert_eq!(primary.public[..header.len()], header[..], "{label}");
        assert_eq!(primary.public.len(), header.len() + digest_len, "{label}");
        let key = secret_of(&t, primary.handle);
        let seed = seed_value(&t, primary.handle);
        assert_eq!((key.len(), seed.len()), (key_bytes, digest_len), "{label}");
        // unique = H(seedValue || key) (Part 1, Equation 48 and clause 24.5.3.3).
        assert_eq!(
            primary.public[header.len()..],
            digest_of(name_alg, &[&seed, &key])[..],
            "{label}"
        );
        // Name = nameAlg || H(TPMT_PUBLIC), with the hash of the test and not of the TPM.
        let name = cat(&[&u16b(name_alg), &digest_of(name_alg, &[&primary.public])]);
        assert_eq!(primary.name, name, "{label}");
        assert_eq!(t.object(primary.handle).unwrap().name, name, "{label}");
        // Neither the key nor its obfuscation value leaves the TPM.
        assert!(
            !contains(&reply, &key) && !contains(&reply, &seed),
            "{label}"
        );
        let read = read_public(&mut t, primary.handle);
        ok(&read);
        let mut r = Reader::new(params(&read, 0));
        assert_eq!(r.tpm2b(1024).unwrap(), &primary.public[..], "{label}");
        assert_eq!(r.tpm2b(66).unwrap(), &name[..], "{label}");
        // A primary's Qualified Name hashes its hierarchy handle and its Name.
        let qualified = cat(&[
            &u16b(name_alg),
            &digest_of(name_alg, &[&u32b(OWNER), &name]),
        ]);
        assert_eq!(r.tpm2b(66).unwrap(), &qualified[..], "{label}");
        r.finish().unwrap();

        // Create and Load under an ECC Storage parent.
        let parent = parse_primary(&create_primary(&mut t, OWNER, b"parent-pw", &ecc_storage()));
        let (handle, loaded_name, child) = make_child(
            &mut t,
            parent.handle,
            b"parent-pw",
            &template,
            b"child-auth",
            b"",
        );
        let (key, seed) = (secret_of(&t, handle), seed_value(&t, handle));
        assert_eq!((key.len(), seed.len()), (key_bytes, digest_len), "{label}");
        let unique = digest_of(name_alg, &[&seed, &key]);
        assert_eq!(child.public[child.public.len() - digest_len..], unique[..]);
        assert_eq!(
            loaded_name,
            cat(&[&u16b(name_alg), &digest_of(name_alg, &[&child.public])]),
            "{label}"
        );
        assert!(!contains(&child.private, &key), "{label}");
        let reloaded = unmarshal_public(&child.public);
        assert_eq!(reloaded.params, PublicParams::SymCipher { symmetric });
        assert_eq!(reloaded.unique, Unique::Digest(unique));

        // A key supplied by the caller (sensitiveDataOrigin CLEAR): padded with zero octets at the
        // high-order end up to the key size (Part 1, clause 24.7.5.1).
        let mut supplied = template.clone();
        supplied.attributes &= !oa::SENSITIVE_DATA_ORIGIN;
        for data in [vec![0xA5; 5], vec![0x11; key_bytes]] {
            let (h, _, _) = make_child(&mut t, parent.handle, b"parent-pw", &supplied, b"", &data);
            let mut expected = vec![0u8; key_bytes - data.len()];
            expected.extend_from_slice(&data);
            assert_eq!(secret_of(&t, h), expected, "{label}");
            ok(&flush(&mut t, h));
        }
        // More than the key size is TPM_RC_SIZE (Part 3, clause 12.1.1, step 2); data with
        // sensitiveDataOrigin SET, or none with it CLEAR, TPM_RC_ATTRIBUTES.
        err(
            &create_child(
                &mut t,
                parent.handle,
                b"parent-pw",
                b"",
                &vec![1u8; key_bytes + 1],
                &supplied,
            ),
            Rc::SIZE.in_param(1),
        );
        err(
            &create_child(&mut t, parent.handle, b"parent-pw", b"", b"x", &template),
            Rc::ATTRIBUTES.in_param(2),
        );
        err(
            &create_child(&mut t, parent.handle, b"parent-pw", b"", b"", &supplied),
            Rc::ATTRIBUTES.in_param(2),
        );
    }
}

#[test]
fn symcipher_parent_protects_child_and_snapshot_round_trip() {
    for symmetric in [SymDef::Aes128Cfb, SymDef::Aes256Cfb] {
        let mut t = started();
        let reply = create_primary(&mut t, OWNER, b"sym-parent", &sym_storage(symmetric));
        let parent = parse_primary(&reply);
        let parent_public = unmarshal_public(&parent.public);
        assert_eq!(parent_public.params, PublicParams::SymCipher { symmetric });
        assert_eq!(
            parent_public.attributes & (oa::RESTRICTED | oa::DECRYPT | oa::SIGN),
            oa::RESTRICTED | oa::DECRYPT
        );
        let (signing, _, _) = make_child(
            &mut t,
            parent.handle,
            b"sym-parent",
            &ecc_sign(),
            b"sign-pw",
            b"",
        );
        sign_and_verify(&mut t, signing, b"sign-pw");
        let (item, _, _) = make_child(
            &mut t,
            parent.handle,
            b"sym-parent",
            &sealed(),
            b"seal-pw",
            b"sealed secret",
        );
        assert_eq!(unseal(&mut t, item, b"seal-pw"), b"sealed secret");
        // A Storage child of a SYMCIPHER parent has a grandchild of its own, and a SYMCIPHER
        // Storage key may itself be the child.
        let (storage, _, _) = make_child(
            &mut t,
            parent.handle,
            b"sym-parent",
            &ecc(
                base() | oa::RESTRICTED | oa::DECRYPT,
                symmetric,
                Scheme::NULL,
            ),
            b"st-pw",
            b"",
        );
        let (grandchild, _, _) =
            make_child(&mut t, storage, b"st-pw", &sealed(), b"g-pw", b"grandchild");
        assert_eq!(unseal(&mut t, grandchild, b"g-pw"), b"grandchild");
        let (nested, _, _) = make_child(
            &mut t,
            parent.handle,
            b"sym-parent",
            &sym_storage(symmetric),
            b"nest-pw",
            b"",
        );
        let (leaf, _, _) = make_child(
            &mut t,
            nested,
            b"nest-pw",
            &sym_leaf(SymDef::Aes256Cfb, alg::SHA256),
            b"leaf-pw",
            b"",
        );
        assert_eq!(secret_of(&t, leaf).len(), 32);
        for handle in [grandchild, storage, leaf, nested, signing, item] {
            ok(&flush(&mut t, handle));
        }
        // Sealed data and the like never leave the TPM in the private part.
        let created = create_child(
            &mut t,
            parent.handle,
            b"sym-parent",
            b"seal-pw",
            b"sealed secret",
            &sealed(),
        );
        ok(&created);
        let child = parse_child(&created);
        assert!(!contains(&child.private, b"sealed secret"));

        // The private part is bound to its parent and its integrity is checked before decryption.
        let other = parse_primary(&create_primary(
            &mut t,
            OWNER,
            b"other",
            &sym_storage(if symmetric == SymDef::Aes128Cfb {
                SymDef::Aes256Cfb
            } else {
                SymDef::Aes128Cfb
            }),
        ));
        let foreign = load(
            &mut t,
            other.handle,
            b"other",
            &child.private,
            &child.public,
        );
        err(&foreign, Rc::INTEGRITY);
        for position in [3, 20, child.private.len() - 1] {
            let mut tampered = child.private.clone();
            tampered[position] ^= 1;
            let reply = load(
                &mut t,
                parent.handle,
                b"sym-parent",
                &tampered,
                &child.public,
            );
            err(&reply, Rc::INTEGRITY);
        }

        // ContextSave and ContextLoad keep the SYMCIPHER parent usable.
        let saved = run(&mut t, plain(cc::CONTEXT_SAVE, &[parent.handle], &[]));
        ok(&saved);
        ok(&flush(&mut t, parent.handle));
        let restored = run(&mut t, plain(cc::CONTEXT_LOAD, &[], params(&saved, 0)));
        ok(&restored);
        let handle = u32::from_be_bytes(restored[10..14].try_into().unwrap());
        let loaded = load(&mut t, handle, b"sym-parent", &child.private, &child.public);
        ok(&loaded);
    }

    // The persistent state: the parent's seed and key survive a snapshot, and a private part made
    // before it loads under the persistent copy after it.
    let mut t = started();
    let parent = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"sym-parent",
        &sym_storage(SymDef::Aes256Cfb),
    ));
    let created = create_child(
        &mut t,
        parent.handle,
        b"sym-parent",
        b"seal-pw",
        b"sealed secret",
        &sealed(),
    );
    let child = parse_child(&created);
    ok(&evict(&mut t, OWNER, b"", parent.handle, 0x8100_0001));
    let snapshot = t.persistent_snapshot();
    let mut restored = Tpm::restore(&snapshot, &[8u8; 48]).unwrap();
    assert_eq!(restored.persistent_snapshot(), snapshot);
    ok(&run(&mut restored, plain(cc::STARTUP, &[], &[0, 0])));
    let read = read_public(&mut restored, 0x8100_0001);
    ok(&read);
    assert_eq!(
        Reader::new(params(&read, 0)).tpm2b(1024).unwrap(),
        &parent.public[..]
    );
    let (item, _) = parse_loaded(&{
        let reply = load(
            &mut restored,
            0x8100_0001,
            b"sym-parent",
            &child.private,
            &child.public,
        );
        ok(&reply);
        reply
    });
    assert_eq!(unseal(&mut restored, item, b"seal-pw"), b"sealed secret");

    // Pinned vector, verified outside the TPM (2026-11-20, Python hmac/hashlib and "openssl enc
    // -aes-256-cfb"): the HMAC over symIv || encrypted || Name under KDFa(SHA-256, seedValue,
    // "INTEGRITY"), the CFB decryption under KDFa(SHA-256, seedValue, "STORAGE", Name, 256), the
    // sensitive area (type, authValue, seedValue, "python vector") and
    // unique = SHA-256(seedValue || data). The TPM is deterministic (fixed entropy).
    let (seed, public, private) = sym_parent_vector();
    assert_eq!(hex_of(&seed), SYM_SEED);
    assert_eq!(hex_of(&public), SYM_PUBLIC);
    assert_eq!(hex_of(&private), SYM_PRIVATE);
}

#[test]
fn symcipher_refuses_bad_keybits_and_mode() {
    let good = sym_leaf(SymDef::Aes128Cfb, alg::SHA256).to_bytes();
    // Offsets: type 0, nameAlg 2, attributes 4, authPolicy 8, symmetric.algorithm 10, keyBits 12,
    // mode 14, unique 16.
    let with = |at: usize, value: u16| {
        let mut bytes = good.clone();
        bytes[at..at + 2].copy_from_slice(&value.to_be_bytes());
        bytes
    };
    let parse = |b: &[u8]| TpmtPublic::unmarshal(&mut Reader::new(b)).unwrap_err();
    // keyBits: TPM_RC_VALUE for anything but 128 and 256 (Part 2, TPMI_AES_KEY_BITS).
    for bits in [0u16, 64, 127, 129, 192, 255, 257, 512, 0xFFFF] {
        assert_eq!(parse(&with(12, bits)), Rc::VALUE, "keyBits {bits}");
    }
    // mode: TPM_RC_MODE for CTR, OFB, CBC, ECB and NULL; CFB is the only one.
    for mode in [alg::NULL, 0x0040, 0x0041, 0x0042, 0x0044] {
        assert_eq!(parse(&with(14, mode)), Rc::MODE, "mode {mode:#06x}");
    }
    // The cipher: TPM_RC_SYMMETRIC for NULL ("TPMT_SYM_DEF_OBJECT" without the +), SM4, XOR, ...
    for algorithm in [alg::NULL, 0x0013, 0x000A, 0x0001] {
        assert_eq!(
            parse(&with(10, algorithm)),
            Rc::SYMMETRIC,
            "cipher {algorithm:#06x}"
        );
    }
    // A unique longer than the digest of nameAlg, a reserved attribute bit, a short buffer.
    assert_eq!(parse(&with(16, 33)), Rc::SIZE);
    let mut reserved = good.clone();
    reserved[4] |= 0x80;
    assert_eq!(parse(&reserved), Rc::RESERVED_BITS);
    assert_eq!(parse(&good[..15]), Rc::INSUFFICIENT);
    // The same bytes through TPM2_CreatePrimary report parameter 2.
    let mut t = started();
    let before = t.persistent_snapshot();
    for (bytes, expected) in [
        (with(12, 192), Rc::VALUE),
        (with(14, 0x0042), Rc::MODE),
        (with(10, alg::NULL), Rc::SYMMETRIC),
    ] {
        err(
            &create_primary_raw(&mut t, OWNER, b"", &raw_creation(&bytes, b"", b"")),
            expected.in_param(2),
        );
    }
    assert_eq!(t.persistent_snapshot(), before);
    // TPM2_TestParms knows the object type now, and the same refusals.
    for (encoded, expected) in [
        ("0025000600800043", None),
        ("0025000601000043", None),
        ("0025000600c00043", Some(Rc::VALUE)),
        ("0025000600800042", Some(Rc::MODE)),
        ("00250010", Some(Rc::SYMMETRIC)),
        ("0025001300800043", Some(Rc::SYMMETRIC)),
        ("00a00006008000430002", None),
        ("00a00006010000430002", None),
        ("00a000100003", Some(Rc::PARMS)),
    ] {
        let reply = run(&mut t, plain(0x18A, &[], &hex(encoded)));
        match expected {
            None => ok(&reply),
            Some(rc) => err(&reply, rc.in_param(1)),
        }
    }

    // The attribute rules of the template (Part 3, clause 12.1.1; Part 2, clauses 8.3.3.12-15).
    let attributes = |bits: u32| symcipher(base() | bits, SymDef::Aes128Cfb, alg::SHA256);
    for (template, expected) in [
        (attributes(0), Rc::ATTRIBUTES),
        (attributes(oa::RESTRICTED), Rc::ATTRIBUTES),
        (
            attributes(oa::RESTRICTED | oa::SIGN | oa::DECRYPT),
            Rc::ATTRIBUTES,
        ),
        (attributes(oa::SIGN | oa::X509_SIGN), Rc::ATTRIBUTES),
        (sym_leaf(SymDef::Aes128Cfb, alg::NULL), Rc::HASH),
    ] {
        err(
            &create_primary(&mut t, OWNER, b"", &template),
            expected.in_param(2),
        );
    }
    for accepted in [
        attributes(oa::SIGN),
        attributes(oa::DECRYPT),
        attributes(oa::SIGN | oa::DECRYPT),
        attributes(oa::RESTRICTED | oa::SIGN),
        attributes(oa::RESTRICTED | oa::DECRYPT),
    ] {
        let primary = parse_primary(&create_primary(&mut t, OWNER, b"", &accepted));
        ok(&flush(&mut t, primary.handle));
    }

    // The sensitive area of a loaded private part: the key has the size of the template's
    // cipher (TPM_RC_KEY_SIZE), is bound to the public unique (TPM_RC_BINDING) and has the
    // sensitiveType of a SYMCIPHER (TPM_RC_TYPE).
    let primary = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"auth",
        &sym_leaf(SymDef::Aes128Cfb, alg::SHA256),
    ));
    let public = unmarshal_public(&primary.public);
    let sensitive = t.object(primary.handle).unwrap().sensitive.clone().unwrap();
    let encode = |s: &Sensitive| sensitive_to_bytes(s, alg::SYMCIPHER, alg::SHA256).unwrap();
    assert!(sensitive_from_bytes(&encode(&sensitive), &public).is_ok());
    let with_secret = |secret: &[u8]| Sensitive {
        secret: Secret::Data(Zeroizing::new(secret.to_vec())),
        ..sensitive.clone()
    };
    let key = secret_of(&t, primary.handle);
    for short in [&key[..15], &key[..0], &[key.clone(), vec![0]].concat()[..]] {
        assert_eq!(
            sensitive_from_bytes(&encode(&with_secret(short)), &public).err(),
            Some(Rc::KEY_SIZE)
        );
    }
    let mut changed = key.clone();
    changed[0] ^= 1;
    assert_eq!(
        sensitive_from_bytes(&encode(&with_secret(&changed)), &public).err(),
        Some(Rc::BINDING)
    );
    let mut other_seed = sensitive.clone();
    other_seed.seed[0] ^= 1;
    assert_eq!(
        sensitive_from_bytes(&encode(&other_seed), &public).err(),
        Some(Rc::BINDING)
    );
    assert_eq!(
        sensitive_from_bytes(
            &sensitive_to_bytes(&sensitive, alg::KEYEDHASH, alg::SHA256).unwrap(),
            &public
        )
        .err(),
        Some(Rc::TYPE)
    );

    // A SYMCIPHER key signs no digest, shares no secret and has no public-only form.
    let signing = primary.handle;
    err(
        &sign(&mut t, signing, b"auth", &DIGEST, &u16b(alg::NULL)),
        Rc::KEY.in_handle(1),
    );
    let storage = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"sym-storage",
        &sym_storage(SymDef::Aes128Cfb),
    ));
    err(
        &make_credential(&mut t, storage.handle, b"credential", &[0, 0xB, 1]),
        Rc::TYPE.in_handle(1),
    );
    err(
        &run(
            &mut t,
            plain(
                cc::MAKE_CREDENTIAL,
                &[primary.handle],
                &cat(&[&tpm2b(b"c"), &tpm2b(&[0, 0xB, 1])]),
            ),
        ),
        Rc::TYPE.in_handle(1),
    );
    let (item, _, _) = {
        let ecc = parse_primary(&create_primary(&mut t, OWNER, b"ecc", &ecc_storage()));
        make_child(&mut t, ecc.handle, b"ecc", &movable_sealed(), b"s", b"data")
    };
    assert_eq!(
        activate_credential(&mut t, item, storage.handle, &[0; 34], &[0; 4]).err(),
        Some(Rc::TYPE.in_handle(2))
    );
    assert_eq!(
        duplicate(&mut t, item, storage.handle, &[], SymDef::Null).err(),
        Some(Rc::TYPE.in_handle(2))
    );
    err(
        &load_external(
            &mut t,
            &{
                let mut p = public.clone();
                p.attributes = oa::DECRYPT;
                p
            },
            NULL,
        ),
        Rc::KEY.in_param(2),
    );
}

// ---------------------------------------------------------------------------------------- ML-KEM

fn mlkem_public(t: &Tpm, handle: u32) -> Vec<u8> {
    match &t.object(handle).unwrap().public.unique {
        Unique::MlKem(key) => key.clone(),
        _ => unreachable!("an ML-KEM key"),
    }
}

#[test]
fn mlkem_restricted_storage_parent_create_load() {
    let (mut t, parent) = kem_fixture();
    // TPMT_PUBLIC by hand (Part 2 Tables 232 and 233): type ML-KEM, nameAlg, attributes, empty
    // authPolicy, TPMS_MLKEM_PARMS (symmetric AES-128-CFB, parameterSet ML-KEM-768 = 2), then the
    // 1184-octet encapsulation key as unique.
    let template = mlkem_storage(SymDef::Aes128Cfb, alg::SHA256);
    let header = cat(&[
        &u16b(alg::MLKEM),
        &u16b(alg::SHA256),
        &u32b(template.attributes),
        &u16b(0),
        &u16b(alg::AES),
        &u16b(128),
        &u16b(alg::CFB),
        &u16b(2),
        &u16b(1184),
    ]);
    assert_eq!(parent.public[..header.len()], header[..]);
    assert_eq!(parent.public.len(), header.len() + 1184);
    assert_eq!(parent.name, name_hash(&parent.public));
    let seed = secret_of(&t, parent.handle);
    let public_key = mlkem_public(&t, parent.handle);
    assert_eq!(parent.public[header.len()..], public_key[..]);
    // The key pair of the TPM is the one OpenSSL (4.1, ML-KEM-768 from the 64-octet seed d || z)
    // derives: its encapsulation key is the public unique (2026-11-20).
    assert_eq!(hex_of(&seed), KEM_SEED);
    assert_eq!(hex_of(&Sha256::digest(&public_key)), KEM_PK_SHA256);
    assert_eq!(seed_value(&t, parent.handle).len(), 32);

    // Children of every kind, protected with the seedValue of the parent.
    let (signing, _, _) = make_child(
        &mut t,
        parent.handle,
        KEM_AUTH,
        &ecc_sign(),
        b"sign-pw",
        b"",
    );
    sign_and_verify(&mut t, signing, b"sign-pw");
    let (item, _, _) = make_child(
        &mut t,
        parent.handle,
        KEM_AUTH,
        &sealed(),
        b"seal-pw",
        b"sealed secret",
    );
    assert_eq!(unseal(&mut t, item, b"seal-pw"), b"sealed secret");
    let (storage, _, _) = make_child(
        &mut t,
        parent.handle,
        KEM_AUTH,
        &ecc_storage(),
        b"st-pw",
        b"",
    );
    let (grandchild, _, _) =
        make_child(&mut t, storage, b"st-pw", &sealed(), b"g-pw", b"grandchild");
    assert_eq!(unseal(&mut t, grandchild, b"g-pw"), b"grandchild");
    let (data_key, _, _) = make_child(
        &mut t,
        parent.handle,
        KEM_AUTH,
        &sym_leaf(SymDef::Aes256Cfb, alg::SHA256),
        b"data-pw",
        b"",
    );
    assert_eq!(secret_of(&t, data_key).len(), 32);
    // An unrestricted ML-KEM child works with Encapsulate and Decapsulate.
    let (kem_child, _, _) = make_child(
        &mut t,
        parent.handle,
        KEM_AUTH,
        &mlkem_template(base() | oa::DECRYPT, SymDef::Null, alg::SHA256),
        b"kem-pw",
        b"",
    );
    let encapsulated = run(&mut t, plain(cc::ENCAPSULATE, &[kem_child], &[]));
    ok(&encapsulated);
    let mut r = Reader::new(params(&encapsulated, 0));
    let (shared, ciphertext) = (
        r.tpm2b(32).unwrap().to_vec(),
        r.tpm2b(1088).unwrap().to_vec(),
    );
    let decapsulated = run(
        &mut t,
        with_pw(
            cc::DECAPSULATE,
            &[kem_child],
            &[b"kem-pw"],
            &tpm2b(&ciphertext),
        ),
    );
    assert_eq!(params(&decapsulated, 0), tpm2b(&shared));
    // The restricted key encapsulates (a public operation) but never decapsulates (Part 3,
    // clause 14.11: restricted CLEAR and decrypt SET).
    let encapsulated = run(&mut t, plain(cc::ENCAPSULATE, &[parent.handle], &[]));
    ok(&encapsulated);
    err(
        &run(
            &mut t,
            with_pw(
                cc::DECAPSULATE,
                &[parent.handle],
                &[KEM_AUTH],
                &tpm2b(&ciphertext),
            ),
        ),
        Rc::ATTRIBUTES.in_handle(1),
    );
    // A private part bound to the parent and its integrity.
    let created = create_child(
        &mut t,
        parent.handle,
        KEM_AUTH,
        b"seal-pw",
        b"sealed secret",
        &sealed(),
    );
    let child = parse_child(&created);
    assert!(!contains(&child.private, b"sealed secret"));
    let mut tampered = child.private.clone();
    tampered[40] ^= 1;
    err(
        &load(&mut t, parent.handle, KEM_AUTH, &tampered, &child.public),
        Rc::INTEGRITY,
    );
    // The same child of the AES-128 parent, verified outside the TPM (Python hmac/hashlib and
    // "openssl enc -aes-128-cfb", 2026-11-20): integrity, CFB, sensitive area and unique.
    let (parent_seed, public, private) = kem_parent_vector();
    assert_eq!(hex_of(&parent_seed), KEM_SEEDVAL);
    assert_eq!(hex_of(&public), KEM_CHILD_PUBLIC);
    assert_eq!(hex_of(&private), KEM_CHILD_PRIVATE);

    for handle in [signing, item, grandchild, storage, data_key, kem_child] {
        ok(&flush(&mut t, handle));
    }

    // AES-256 and nameAlg SHA-384 / SHA-512 parents protect with their own key size and hash.
    for (symmetric, name_alg) in [
        (SymDef::Aes256Cfb, alg::SHA256),
        (SymDef::Aes128Cfb, alg::SHA384),
        (SymDef::Aes256Cfb, alg::SHA512),
    ] {
        let primary = parse_primary(&create_primary(
            &mut t,
            OWNER,
            b"p2",
            &mlkem_storage(symmetric, name_alg),
        ));
        assert_eq!(
            primary.name,
            cat(&[&u16b(name_alg), &digest_of(name_alg, &[&primary.public])])
        );
        assert_eq!(seed_value(&t, primary.handle).len(), digest_size(name_alg));
        let (item, _, _) = make_child(
            &mut t,
            primary.handle,
            b"p2",
            &named(sealed(), name_alg),
            b"seal-pw",
            b"data",
        );
        assert_eq!(unseal(&mut t, item, b"seal-pw"), b"data");
        ok(&flush(&mut t, item));
        ok(&flush(&mut t, primary.handle));
    }

    // Templates. A restricted ML-KEM key is a decryption Storage key with a cipher; unrestricted
    // keys have NULL (Part 2, Table 232).
    let kem = |attributes: u32, symmetric: SymDef| {
        mlkem_template(base() | attributes, symmetric, alg::SHA256)
    };
    for (template, expected) in [
        (
            kem(oa::RESTRICTED | oa::DECRYPT, SymDef::Null),
            Rc::SYMMETRIC,
        ),
        (kem(oa::DECRYPT, SymDef::Aes128Cfb), Rc::SYMMETRIC),
        (kem(oa::RESTRICTED, SymDef::Aes128Cfb), Rc::ATTRIBUTES),
        (kem(0, SymDef::Null), Rc::ATTRIBUTES),
        (kem(oa::SIGN, SymDef::Null), Rc::ATTRIBUTES),
        (
            kem(oa::RESTRICTED | oa::SIGN | oa::DECRYPT, SymDef::Aes128Cfb),
            Rc::ATTRIBUTES,
        ),
        (
            kem(oa::DECRYPT | oa::X509_SIGN, SymDef::Null),
            Rc::ATTRIBUTES,
        ),
    ] {
        err(
            &create_primary(&mut t, OWNER, b"", &template),
            expected.in_param(2),
        );
    }
}

fn named(mut public: TpmtPublic, name_alg: u16) -> TpmtPublic {
    public.name_alg = name_alg;
    public
}

#[test]
fn mlkem_labeled_kem_duplicate_import_round_trip() {
    // (a) The TPM duplicates objects for an ML-KEM Storage key, and imports them back.
    let (mut t, kem) = kem_fixture();
    let source = parse_primary(&create_primary(&mut t, OWNER, b"src-pw", &ecc_storage()));
    let mut signing = movable(ecc_sign());
    signing.attributes &= !oa::FIXED_PARENT;
    for (item_template, data, auth) in [
        (movable_sealed(), &b"duplicated secret"[..], &b"obj-pw"[..]),
        (signing, &b""[..], &b"sig-pw"[..]),
    ] {
        let (object, name, child) =
            make_child(&mut t, source.handle, b"src-pw", &item_template, auth, data);
        for inner in [SymDef::Null, SymDef::Aes128Cfb, SymDef::Aes256Cfb] {
            let (key, blob, secret) = duplicate(&mut t, object, kem.handle, &[], inner).unwrap();
            assert_eq!(key.len(), usize::from(inner.key_bits() / 8));
            // outSymSeed is the ML-KEM-768 ciphertext, the outer wrapper starts with an HMAC of
            // the nameAlg of the new parent.
            assert_eq!(secret.len(), 1088);
            assert_eq!(blob[..2], [0, 32]);
            let imported = import(
                &mut t,
                kem.handle,
                KEM_AUTH,
                (&key, &child.public, &blob, &secret),
                inner,
            );
            ok(&imported);
            let mut r = Reader::new(params(&imported, 0));
            let private = r.tpm2b(1024).unwrap().to_vec();
            r.finish().unwrap();
            let loaded = load(&mut t, kem.handle, KEM_AUTH, &private, &child.public);
            ok(&loaded);
            let (copy, copy_name) = parse_loaded(&loaded);
            assert_eq!(copy_name, name);
            if data.is_empty() {
                sign_and_verify(&mut t, copy, auth);
            } else {
                assert_eq!(unseal(&mut t, copy, auth), data);
            }
            ok(&flush(&mut t, copy));
        }
        ok(&flush(&mut t, object));
    }
    // Two duplications of the same object have two ciphertexts and two seeds.
    let (object, _, _) = make_child(
        &mut t,
        source.handle,
        b"src-pw",
        &movable_sealed(),
        b"o",
        b"d",
    );
    let first = duplicate(&mut t, object, kem.handle, &[], SymDef::Null).unwrap();
    let second = duplicate(&mut t, object, kem.handle, &[], SymDef::Null).unwrap();
    assert_ne!(first.2, second.2);
    assert_ne!(first.1, second.1);

    // (b) A public-only copy of the ML-KEM key is enough for Duplicate.
    let mut public = unmarshal_public(&kem.public);
    public.attributes = oa::RESTRICTED | oa::DECRYPT;
    let (external, _) = parse_loaded(&load_external(&mut t, &public, NULL));
    let (_, blob, secret) = duplicate(&mut t, object, external, &[], SymDef::Aes128Cfb).unwrap();
    assert_eq!((secret.len(), blob[..2].to_vec()), (1088, vec![0, 32]));

    // (c) With nameAlg SHA-384 and SHA-512 the seed, the HMAC and the KDFa are of that hash.
    for name_alg in [alg::SHA384, alg::SHA512] {
        let size = digest_size(name_alg);
        let new_parent = parse_primary(&create_primary(
            &mut t,
            OWNER,
            b"p384",
            &mlkem_storage(SymDef::Aes256Cfb, name_alg),
        ));
        let (key, blob, secret) =
            duplicate(&mut t, object, new_parent.handle, &[], SymDef::Aes128Cfb).unwrap();
        assert_eq!(blob[..2], (size as u16).to_be_bytes());
        assert_eq!(secret.len(), 1088);
        // Import wants the public area of the duplicated object.
        let reply = read_public(&mut t, object);
        let public = Reader::new(params(&reply, 0)).tpm2b(1024).unwrap().to_vec();
        let imported = import(
            &mut t,
            new_parent.handle,
            b"p384",
            (&key, &public, &blob, &secret),
            SymDef::Aes128Cfb,
        );
        ok(&imported);
        ok(&flush(&mut t, new_parent.handle));
    }

    // (d) A duplicate made outside the TPM for the ML-KEM key: OpenSSL (4.1) encapsulates to
    // the encapsulation key, K goes through KDFa(SHA-256, K, "DUPLICATE", ciphertext, publicKey,// 256) to the seed, and Python builds the wrapper (KDFa "STORAGE" and "INTEGRITY", AES-128-CFB
    // with a zero IV, HMAC-SHA-256 over the ciphertextand the Name) around a sealed object.
    let (mut t, kem) = kem_fixture();
    let public = hex(DUP_PUBLIC);
    let imported = import(
        &mut t,
        kem.handle,
        KEM_AUTH,
        (&[], &public, &hex(DUP_BLOB), &hex(DUP_CT)),
        SymDef::Null,
    );
    ok(&imported);
    let mut r = Reader::new(params(&imported, 0));
    let private = r.tpm2b(1024).unwrap().to_vec();
    let loaded = load(&mut t, kem.handle, KEM_AUTH, &private, &public);
    ok(&loaded);
    let (item, name) = parse_loaded(&loaded);
    assert_eq!(hex_of(&name), DUP_NAME);
    assert_eq!(unseal(&mut t, item, b""), hex(DUP_DATA));
    // The same ciphertext with the seed of another protocol label does not open (the HMAC fails),
    // a ciphertext of another length is a value error, an oversized one a size error.
    err(
        &import(
            &mut t,
            kem.handle,
            KEM_AUTH,
            (&[], &public, &hex(DUP_WRONG_LABEL_BLOB), &hex(DUP_CT)),
            SymDef::Null,
        ),
        Rc::INTEGRITY.in_param(3),
    );
    let mut changed = hex(DUP_CT);
    changed[100] ^= 1;
    err(
        &import(
            &mut t,
            kem.handle,
            KEM_AUTH,
            (&[], &public, &hex(DUP_BLOB), &changed),
            SymDef::Null,
        ),
        Rc::INTEGRITY.in_param(3),
    );
    for (length, expected) in [(1087, Rc::VALUE), (1089, Rc::SIZE), (1, Rc::VALUE)] {
        let secret = vec![7u8; length];
        err(
            &import(
                &mut t,
                kem.handle,
                KEM_AUTH,
                (&[], &public, &hex(DUP_BLOB), &secret),
                SymDef::Null,
            ),
            expected.in_param(4),
        );
    }
    // The pinned duplicate the TPM makes for the same key, opened outside: HMAC, CFB and sensitive
    // area of the sealed object under KDFa(SHA-256, K, "DUPLICATE", ...) (Python and OpenSSL).
    let (public, blob, secret) = duplicated_for_kem();
    assert_eq!(hex_of(&public), DUPD_PUBLIC);
    assert_eq!(hex_of(&blob), DUPD_BLOB);
    assert_eq!(hex_of(&Sha256::digest(&secret)), DUPD_SECRET_SHA256);
}

#[test]
fn mlkem_makecredential_activate_round_trip() {
    // (a) TPM round trips for every nameAlg of the Storage key and both AES sizes; a public-only
    // copy of the key makes credentials as well.
    for (name_alg, symmetric) in [
        (alg::SHA256, SymDef::Aes128Cfb),
        (alg::SHA256, SymDef::Aes256Cfb),
        (alg::SHA384, SymDef::Aes128Cfb),
        (alg::SHA512, SymDef::Aes256Cfb),
    ] {
        let label = format!("{name_alg:#06x} {symmetric:?}");
        let mut t = started();
        let kem = parse_primary(&create_primary(
            &mut t,
            OWNER,
            KEM_AUTH,
            &mlkem_storage(symmetric, name_alg),
        ));
        let activated = parse_primary(&create_primary(&mut t, OWNER, b"act-pw", &ecc_sign()));
        let mut public = unmarshal_public(&kem.public);
        public.attributes = oa::RESTRICTED | oa::DECRYPT;
        let (external, _) = parse_loaded(&load_external(&mut t, &public, NULL));
        let size = digest_size(name_alg);
        for key in [kem.handle, external] {
            let reply = make_credential(&mut t, key, CREDENTIAL, &activated.name);
            ok(&reply);
            let mut r = Reader::new(params(&reply, 0));
            let blob = r.tpm2b(200).unwrap().to_vec();
            let secret = r.tpm2b(1088).unwrap().to_vec();
            r.finish().unwrap();
            assert_eq!(blob[..2], (size as u16).to_be_bytes(), "{label}");
            assert_eq!(blob.len(), 2 + size + 2 + CREDENTIAL.len(), "{label}");
            assert_eq!(secret.len(), 1088, "{label}");
            assert_eq!(
                activate_credential(&mut t, activated.handle, kem.handle, &blob, &secret),
                Ok(CREDENTIAL.to_vec()),
                "{label}"
            );
            // Another Name does not activate; a changed ciphertext decapsulates to another
            // secret (implicit rejection) and fails the integrity check, as does a changed blob.
            let other = parse_primary(&create_primary(&mut t, OWNER, b"other", &ecc_storage()));
            assert_eq!(
                activate_credential(&mut t, other.handle, kem.handle, &blob, &secret),
                Err(Rc::INTEGRITY.in_param(1)),
                "{label}"
            );
            ok(&flush(&mut t, other.handle));
            let mut changed = secret.clone();
            changed[500] ^= 1;
            assert_eq!(
                activate_credential(&mut t, activated.handle, kem.handle, &blob, &changed),
                Err(Rc::INTEGRITY.in_param(1)),
                "{label}"
            );
            let mut changed = blob.clone();
            *changed.last_mut().unwrap() ^= 1;
            assert_eq!(
                activate_credential(&mut t, activated.handle, kem.handle, &changed, &secret),
                Err(Rc::INTEGRITY.in_param(1)),
                "{label}"
            );
            // A ciphertext of the wrong size: a value error for sizes the key does not open, a
            // size error beyond the largest ciphertext.
            for (length, expected) in [(1087, Rc::VALUE), (1, Rc::VALUE), (1089, Rc::SIZE)] {
                assert_eq!(
                    activate_credential(
                        &mut t,
                        activated.handle,
                        kem.handle,
                        &blob,
                        &vec![3u8; length]
                    ),
                    Err(expected.in_param(2)),
                    "{label} {length}"
                );
            }
        }
    }

    // (b) A credential made outside the TPM: OpenSSL encapsulates to the key, the seed is
    // KDFa(SHA-256, K, "IDENTITY", ciphertext, publicKey, 256) and Python wraps the credential.
    let (mut t, kem) = kem_fixture();
    let activated = parse_primary(&create_primary(&mut t, OWNER, b"act-pw", &ecc_sign()));
    assert_eq!(hex_of(&activated.name), ACT_NAME);
    assert_eq!(
        activate_credential(
            &mut t,
            activated.handle,
            kem.handle,
            &hex(EXT_CRED_BLOB),
            &hex(CRED_CT)
        ),
        Ok(b"external credential".to_vec())
    );
    // The label is part of the derivation: the seed of the "DUPLICATE" label fails the HMAC.
    assert_eq!(
        activate_credential(
            &mut t,
            activated.handle,
            kem.handle,
            &hex(EXT_CRED_WRONG_LABEL_BLOB),
            &hex(CRED_CT)
        ),
        Err(Rc::INTEGRITY.in_param(1))
    );

    // (c) The same with a SHA-384 / AES-256 key (KDFa and the HMAC of SHA-384, 48-octet seed).
    let mut t = started();
    let kem = parse_primary(&create_primary(
        &mut t,
        OWNER,
        KEM_AUTH,
        &mlkem_storage(SymDef::Aes256Cfb, alg::SHA384),
    ));
    assert_eq!(hex_of(&secret_of(&t, kem.handle)), K384_SEED);
    assert_eq!(
        hex_of(&Sha256::digest(mlkem_public(&t, kem.handle))),
        K384_PK_SHA256
    );
    let activated = parse_primary(&create_primary(&mut t, OWNER, b"act-pw", &ecc_sign()));
    assert_eq!(hex_of(&activated.name), K384_ACT_NAME);
    assert_eq!(
        activate_credential(
            &mut t,
            activated.handle,
            kem.handle,
            &hex(K384_BLOB),
            &hex(K384_CT)
        ),
        Ok(b"external credential".to_vec())
    );

    // (d) The credential the TPM makes, opened outside (OpenSSL decapsulation with the seed of
    // the key, then Python: the seed, the wrapper, byte for byte).
    let (blob, secret, name) = made_credential();
    assert_eq!(hex_of(&blob), MADE_BLOB);
    assert_eq!(hex_of(&Sha256::digest(&secret)), MADE_SECRET_SHA256);
    assert_eq!(hex_of(&name), ACT_NAME);
}

#[test]
fn mlkem_salted_session_key_matches_independent() {
    // The encryptedSalt is the ML-KEM-768 ciphertext OpenSSL made for the key of the TPM; the
    // salt is KDFa(SHA-256, K, "SECRET", ciphertext, publicKey, 256) and the session key
    // KDFa(SHA-256, salt, "ATH", nonceTPM, nonceCaller, 256) (Python hmac, 2026-11-20).
    let (mut t, kem) = kem_fixture();
    let (session, nonce_tpm) = session_of(&start_salted(&mut t, kem.handle, &hex(SALT_CT)));
    assert_eq!(hex_of(&nonce_tpm), NONCE_TPM);
    assert_eq!(hex_of(&session_key(&t, session)), SALT_SESSION_KEY);
    assert_eq!(
        hex_of(&kdfa_256(&hex(SALT), b"ATH", &nonce_tpm, &NONCE_CALLER)),
        SALT_SESSION_KEY
    );
    // The label is bound in: the salt of "IDENTITY" would give another key.
    assert_ne!(SALT_WRONG_LABEL_SESSION_KEY, SALT_SESSION_KEY);
    assert_eq!(
        hex_of(&kdfa_256(
            &hex(SALT_WRONG_LABEL),
            b"ATH",
            &nonce_tpm,
            &NONCE_CALLER
        )),
        SALT_WRONG_LABEL_SESSION_KEY
    );
    // Another ciphertext gives another salt, not an error (FIPS 203 implicit rejection).
    let mut changed = hex(SALT_CT);
    changed[10] ^= 1;
    let (other, nonce) = session_of(&start_salted(&mut t, kem.handle, &changed));
    assert_ne!(hex_of(&session_key(&t, other)), SALT_SESSION_KEY);
    assert_ne!(nonce, nonce_tpm);
    // Sizes: empty and shorter are value errors, longer than a ciphertext a size error.
    for (length, expected) in [
        (0, Rc::VALUE),
        (1, Rc::VALUE),
        (1087, Rc::VALUE),
        (1089, Rc::SIZE),
    ] {
        err(
            &start_salted(&mut t, kem.handle, &vec![1u8; length]),
            expected.in_param(2),
        );
    }
    // The key must hold its private part.
    let mut public = unmarshal_public(&kem.public);
    public.attributes = oa::RESTRICTED | oa::DECRYPT;
    let (external, _) = parse_loaded(&load_external(&mut t, &public, NULL));
    err(
        &start_salted(&mut t, external, &hex(SALT_CT)),
        Rc::KEY.in_handle(1),
    );
    // An unrestricted ML-KEM decryption key works too: the TPM's own Encapsulate gives K and the
    // ciphertext, the test derives the salt with its own KDFa.
    let unrestricted = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"u",
        &mlkem_template(base() | oa::DECRYPT, SymDef::Null, alg::SHA256),
    ));
    let encapsulated = run(&mut t, plain(cc::ENCAPSULATE, &[unrestricted.handle], &[]));
    ok(&encapsulated);
    let mut r = Reader::new(params(&encapsulated, 0));
    let (shared, ciphertext) = (
        r.tpm2b(32).unwrap().to_vec(),
        r.tpm2b(1088).unwrap().to_vec(),
    );
    let public_key = mlkem_public(&t, unrestricted.handle);
    let salt = {
        // KDFa with contextU = ciphertext and contextV = publicKey is longer than kdfa_256 takes.
        let mut data = 1u32.to_be_bytes().to_vec();
        data.extend_from_slice(b"SECRET\0");
        data.extend_from_slice(&ciphertext);
        data.extend_from_slice(&public_key);
        data.extend_from_slice(&256u32.to_be_bytes());
        crate::crypto::hmac_sha256(&shared, &[&data]).to_vec()
    };
    let (session, nonce) = session_of(&start_salted(&mut t, unrestricted.handle, &ciphertext));
    assert_eq!(
        session_key(&t, session),
        kdfa_256(&salt, b"ATH", &nonce, &NONCE_CALLER)
    );
}

// ------------------------------------------------------------------------------------ vectors
//
// Made and checked outside the TPM on 2026-11-20 with OpenSSL 4.1.0 (ML-KEM-768 encapsulation and
// decapsulation, "openssl enc -aes-*-cfb") and Python hmac/hashlib. The TPM is deterministic (fixed
// entropy), so what it makes is pinned as well.

const SYM_SEED: &str = "419da6c3437cac02bc61544143d0883652a773aca5006c04ee64988d38f79075";
const SYM_PUBLIC: &str =
    "0008000b0000005200000010002059d0886c0ea85aa3b023a739e0a35194252bf0964bb47b8bccf4174fc78c0008";
const SYM_PRIVATE: &str = "00207452172314a61315488f99c924bab8afd412ad3cd61fa366f0a6f54672bfe66d001040dfabbb2ce0a1e4be5ea5e06355c692e0c25cf1ee8916373f2416f2aec390527cc0c40569902e7751547e6f37a7b86ea99c5d0eaee1ef52b4db7b11cbc654bbe4aff97b3a4ce352636bff33357f94489a48a1b7fd1a4731ed39a57548e8084f5e9c17e033aa7b";
const KEM_SEEDVAL: &str = "50c177491b4d07a7d1d36143c374df47c2c451a014485a69c9666378b2fe407a";
const KEM_CHILD_PUBLIC: &str =
    "0008000b0000005200000010002059d0886c0ea85aa3b023a739e0a35194252bf0964bb47b8bccf4174fc78c0008";
const KEM_CHILD_PRIVATE: &str = "0020afc48bef8a2b6f9b718f27ce677e705cee0b2f7d7f6e66f01cb86f513e649c61001040dfabbb2ce0a1e4be5ea5e06355c69229780c1e1b8acb79d0875ccc485c79a97e3da5700e577e978057016e3fc95370640400fd0b0598ba1b010818eb5e800e1868b7c4825278900de8fbb0987859434b44c225ee1566cc91f358c3f6db9d4f5674aa9c66800e";
const KEM_SEED: &str = "06959f7bc18a5dddb19997803c52ab56bebb5f979bbbdb682c5aecf4b0079ccfed71d6919dd9c5adcd26c897e5e3799d6a133382263516a74ec497f2c74e160b";
const KEM_PK_SHA256: &str = "99b561731e34097462f28ef2948ea75bf969d44ec20ec5b34389c193e4fe0338";
const MADE_BLOB: &str = "00201ca0018ac3cc9ae9bf2493a8e7684baa7f2bd24a36be13e5edb897e73ea75bf9138c306028fc1505cb1416fd4a5afae243de0bf4de";
const MADE_SECRET_SHA256: &str = "69195ffba5db78d50a0e3ac5f4010dc874dafcc759c577d5f917c0603434084a";
const ACT_NAME: &str = "000b136a43f615f9fde6a02e4523a0415dd9b2cd72724e0737621027abd252056bb1";
const DUPD_PUBLIC: &str =
    "0008000b000000400000001000203e45a0450a8cfd55bc0c1681ef9ce563c493c1dd63f70b6dcbc1d3acf61bd8d3";
const DUPD_BLOB: &str = "0020d85994d1abfce95dba87cf73be2adc7c1740363f4d588fad4d758cc7b07dd6834cfa94e930477b67817eb26eb7e828e688e9d04bdc6aa965ec4d71bf090eb3aa4f7917d2e553ed71d9976c51b0ca8f5329e4cdd47eeb7b3c412a777780cf591f39228151fe8bbea9b741b3b3152733f9f1287e89c26996f0c3c549";
const DUPD_SECRET_SHA256: &str = "49fddd4050a04c3da564b9604a325051ed724f10ad44a28fcee78c5b3a45983a";
const NONCE_TPM: &str = "3f64e53032fbb062370276a9b143893b";
const SALT_CT: &str = "66dae6d09f2e4b3fb25bb59c40f04c5a2a1b0a9ae0359dedc8a2e689af7d4995a856ba8770bc714eb89a3d74d408e1c7006d6d8ef7daa49b370a92eb15442e50d3625ed651901074f3898f30ec39f14daf8097e7a9f9c207dde74406e5e00aebcd7d39544abd778c4160be72e9755c9b4b69d6874a038afcc0f2f94a9cc7c3541d2ebc6387d1c7250ab84a2ccc84858a73e7b5bdbe36cb24b5531774ee361e03e1d63ab01419e5c0bb0060479e5d7f1b3111d18d01a362ea966af2bdb008c362c91dc13b4c1e7a7f96fcbf798ab064b451448db44ba513332b02d01ea92d6471a63c84fa0a162a7d82e308283be9b4c9759bb5f3614e86c2f3c1d997975368a845646eeb7ffdd95c515bef313b8562f0a52db4d2d0ae9a356d1c52ae6cfa5e7b8b34fc7ca1fc0f82ff42f33bcee6bbd969d80e5a158a661824f51f23b7c5b88608ba1d329f8c444a6d58b2a5a2fcd7f0a1605253eb6cf9722036f65103a61b3bfb4cecd02fea1bf8895054a2babe85783686aaec9bde965a526b4244c3f315557ccbd61cfcc86ad01de8b1bc754efd8d548162b44c8bea8289e78953a2d78310ef452b1a02e81f1b10bdfcabee3e083a60fca0482da3d2d6ca4cebb6d6148865a63abe45ebadd3592a900853c19a8a63dd9956334967f00400f753b16eb9b6fe7fafff50506c37d1cf0058f1603c00724362951c092ef40cce0f9a1319811f5544be2c6ec44d2b820b4144114af0402fbac761b7978f293f27e986692d121bbec699f1b2ed7f544e6de7105c1752270cfb69b273d5f2bb24c7ab06ee6899fb1c76e238a85f5c2347d907896a437ca69a9f65be1ecf5e3a7a563e571981a77b427c62e69335f3571b3646581d5e32663c08e54739333e3e48a8d142498a416fc70e3542a3f8996f03ceb9f8bb6cffce1617b85e28fc50d7ce2f29498f234c5461fe837ed1b1c6261293ba0e5e55e2ff078210fb910c5c47aa4d5d2f41d949edc50da9d3b31c329cb959c31c09871193f999a2842a91ce2dab016de32046e61b4fd08ff7db8964143c7997b280d911090cc5ed1bdbf8867b97c362ebd6458f0e7559f5fcd566ef0fc2b4dea4d7f38fee5c37ffcd7a5e26adb9021f4a05d6321aa6c740bfed533bcc90d0d1956c4bdc7736f9016d2f348d280771c61e4a88410291eb8c235d022b3a9c247d2d4bc2bbf2e7183ed194cb3fa8857299cbdb7f03d20076b70ebf93b89b8fbd13525495480f1dfbb2fd6a6ccfbf1b298e9ab5fbedc9a37ed57359dee8a91a8c7a782f538f6b35d25b7009d231648ed0d63c7d5e6e0290839178e756a7acdd715a69b48320331264c697d25b364adeec2a2bd51951a891e073d006f4a75f700c6c22244c5fe0094754fabe5ab84a21c873664ac7b38b6693cedd02cca12445f7cfae7c4366a9e58504c81496af3bbde7ae1406d976f8d70093bc0ebbd02a06f4fc099f19c7c674fa099c69e3e59f610b7b6612e27732cabf36a406e76b7f6a84a3e3ae309129d0409d915aceab17d5e63d149768e3b741";
const SALT: &str = "77e326e179adf6ba316e9918422ee54f3b742c59b10fcba6823ff0e0910d99f3";
const SALT_WRONG_LABEL: &str = "cf69816481a6f990bb539792648efa6d732465d07fc09f34c869eef5e6736dda";
const SALT_SESSION_KEY: &str = "05e53c170c6ee822efec99291a8c0f05c1e40798677303c7adf60c91ee2655c1";
const SALT_WRONG_LABEL_SESSION_KEY: &str =
    "ea4d6dfe21c041ca760e81205eafe1beb6c0cf14dbc64f807c883f97d07e466e";
const CRED_CT: &str = "f6379e5b8e4d6451f00e7d9e298697daab508ce1f27165717e2480f9d300ea6f5eb60e32b7e3ba5288fdfc9b406bff8a367907ceac6681f2a6ffa14d46db81413b8c61259436dab10a70092a9e32997635395e541c9a819aa5c7f395001a351a54bb6ca2a4a673254f15f5b53da94dac6b46ab8803dc256ca6a541aad8d146e901d1a56df41bc8468136ebe38cf43205ff14f31db193736ebc82eaaf2e5f6ae8300671bc2fdd686cf96b43abdebaa8fe11bf27e0dd24645e6396a88185062c8938df23274e43c0c1c0b4d02a37ac6b5a38d775ca8b18bf658585f5c907f5db9e5617c20bac975bef1c6b49a179c9f2975aee6c92003c25178b000e747353fae8a214f9e101a32c387914201b12f3cd93a598e8340bc275c114a6215d31b8934b3ca1e2dc643624fd7b2a64ffd1d37c6a7aaa6cba3a32f91d79848f11164097b5004498bb0400b52408330e90e550c17cb094cb3db3fcd9dc704ef2966b6a534c21fd3471cd45d435d677ee1c303132475f295697fa48464894e6c816bec22342265ddae2592efffccf02b5c904129e7f145a9eac7124fa04a224faca81c7c1082c3215c5a6d9d7455f900db345a86cdf2b7ea44b7985953792f1d938169cb722bd96018d56c3b264dc5c8ce9f3362386e0eb78cbe8a8f8df3e7549fdebf7aad949e76ab2a63a1a91aff3f03fa8501bc86782d27c996eda368d8aac6444d8b9139e9b911f9fc0e360a43a1a881e9a82e29fe25dec14e9d63dd9e3f38035630ff4d2da2a1cbd836c789f8ad845b9c930e43245672ddc8f2437a96dfc5662711d9b5652bd30ab4f379f07c20d413f14217a537b7442c8b5f3b448f6766804da7619e221fe708048812d75c0be01cd336799bdb59874bf18babe59acf1ce5e7eb506e2cbed9b7a41f7737f007ecb47ec3a92d004b3be995ee8ee55e55fb2f3727bd1db567738a7190d78ef324c89ff573291a11a28f37ebe901f2c5cebdcdb5a7d8b20315d2dd0f769297d7f6f607cbd50a5fe9b4169c34d386c71167f671161ad87408226f7291287e3a7f6a067abbd343a7e34afd79f81fab6f167baf08389c2c304a708ba9a5f442ca4318ab68c64327c8af4cc0b66d7a4409187983e5477c6f2d4cb8f419f745b1e170f2b7752d9526581476190708e46e4891b769ef4375d2fbf65b1a62f8b61fc760bd002c1ec99c98b84dd8e2f80b5ac76425ab2f4c8d2dcd9eb41a2f05b27acc4363b0bf140e4d6ab364255e8c839dcb9811a0f0e591edb53c9f4782dee214e7996e830b9003f0cad4b94a1b439e0d788852c6b7e0b1ae92d18b5605d28a474f47bdb4d4c6836bf1efb639d86b6ec7dbd33c79e8d8ef6775acbe45eb301e92a2e8bb02427d1ff2d7a509fc7a05a9be080e5ac336bba278d2680c7bf6a8cde9d763945d936dbcc29586973a611b9c52ace441702437a10d7735edf5db59357f85e13ce504a89ecd76ccbf8adce1dd064f60a016fb6f2f6c7fa494df21ef4a0e96ab495c7aaf3b86f6629187443004e3e5f682ece744857bb";
const EXT_CRED_BLOB: &str = "00202e1a3395ee3550b71a4e55c54cefb0a130f4fa5d119c4459e6463c8a7f6ded01ca47a3efc03808a96b1a023eec86659e9fb29d22e5";
const EXT_CRED_WRONG_LABEL_BLOB: &str = "00208cb3439e8fa1d4b56ef54ae58fd38fdcc88de187814cc6182f7b9d5bfee4c34380bf449882e377a57b8a8978507e74991b2a60f541";
const DUP_CT: &str = "8d73d027e24dda37a809e2430556391dd2206d1dfebe1cc715f1da1cc6b98a2bfe9daccf887b4d203013763d19a643d888a1b1c9dda7bfc5461d57223d097ac0aa694150b6e1072e500e891743525deb5d69a15c5729dc5e4cf88603f1c570452babca2e482268067fcf9591abeff491b9a1a1624a8673e43069830b7e3d2ffa2c55755f5314bedccde4a210ceb130dde54c586a671b890ef2cc9ba3c202f124aa13919e080f1399dc85f710f2ced06353ab5dd22f78278ad4e68af530206cfcf6a09e3ea85a5f9171b29535cbdba8d1de18fd35318e70764f3cf7a499b1549462c7fa104c8e20a0a53975104876fe515b1c9b24c046e86107815fdd0e578585b531814fba6b7fcf7d35793849586ac891b406718097e0eba39a61022cc4295a69d59efd307e31c514a0128ef6252d1c0da26721b022d33c693e985d0926b3aa024a4215d46889336c47a84eb72a3a7fce000c98aed9b9e8e484b9415694c807d08adf3b55b330b144228fb3940b26937f162a5b09def6e0897a93f28b8e6d6e2949197cdab04f6d94a931500873fa2ad30e9c4e6dca3d2a0e2c7eb6c16188fdae621919c55dcff2150e38cc10874f84b6046b62876e07b6e5327048a8aa82de49845a73ab027d96f2ec9961c52107b9d86d0a0f1b796b717f9ee0833efafb96742173495b4ad226c061cc25f9996fdb6ab4ffa1ecf80ee77e7a026221856fbfdf2bbd7c81c2cf01a27862b7e709a2779a7c3359b6f8ef56af999093699dcbf4b624b10e631e8639b13f5cfefd889079368888dbc81bbbab36879948f25b8451ff5c4cf7552d9ce12baf5a1c94c8836c55736c2ea98eea0da9de46eed7c0361e073aba3b10313cc7c15c147e2835091ff7476ef0808d5cc40225c3691c0bba0dc7047c159822b3deefa328d3f7f559b3ae397cff114d2ab17f988de1e38dc0dc33119594b4bdb525210b187217535ea53a8222ab9fc2ef3eb23c74fddb602f19ef5527b6ad4b3f04a48cf6b22569927e654b895fbac27f2dd034a5d2e65dfaada5c2684bb8f1d088e7355dabc799b40626a6123a5a0d1f92df6a1d01f34dfa8d2df1f25b5ed50cdce5baff286f269ccd7b2ea6d6900c0e6cc1dbd8ee3252ec43fe4229a6bf17fce3ea0a1449ff76edc7d1a7782b9aabc5c42a7f5e691e8b4448db0ac76b6f09ced9f00da131484b77f233d0ca185db5637999c43706923077a7c68eda93b6e6fd8bcb5e31bd36f39cf3c839330edeb7ff08137f52b8de418999277e207c7b5b675f1fe6087f1038a9c682e73d5356602a0442d322780375b4e327eeddf78bd4e9ba6a6479d09013993637d18579dc672f9eee54e2375ba085a4c0a488f02b441c98c312788e5c0d0592f57bd56e96928bb30fb2b26b95eeb662ba3d0451c68dde8915fbc3182cddda25f7a89a4f596aec05c8c65268f173e041b5003f55f379e566017f36317aa60dc4cfe066c8fbb0542ddc630ac6453d790af4e6e947a5c76e2cea6e20a672572087a19f98926c20c441162ef523e41209b8";
const DUP_PUBLIC: &str =
    "0008000b000000400000001000203f0ef213e0a72e646cd4ac6b8e643ab9a2d93e026345d4fa25d302fa7e4c52ea";
const DUP_NAME: &str = "000b394eaf739b8a7383cc089b8470d6a4cf16835505f4a53dbf193c795a88aa3d7c";
const DUP_BLOB: &str = "0020da64f77cbf98e3069228bf371f3316fcac5f1195d15ac164868ba1438a2655c191c01a5c7a08e004a11094ea5b525f27055cf2a87816852bc1c9cd717ec2270f0e84cbbf81d789cb5b67c0b309ee0ed65167f79df4c1f5cbeb86de5f6315a5c39df7f833c1cbc5a36b30adf949dbba12a1aa5324515bccfc5fc2c742a186f6bb";
const DUP_WRONG_LABEL_BLOB: &str = "00201fe614e3f414732132ea17ceac41d5e76c44538ff0438ac48afd3c87e400c0d80f574a95fe1597a808d0772d14b78031293f5d90e6f7218f18167171e5d761b67fbe624b5d50578923a59afad5807d73eac30a110a3869a42df8eb942c10306c95d7638e2b2baed0ddebdc738443921baf1b78c3efbb07c1b82ce107727c1947";
const DUP_DATA: &str = "65787465726e616c207365616c656420736563726574";
const K384_CT: &str = "795960be064351299096dfe9df3b12e55fc2ddb3937a63f353f32304ef852e909031f9f48c50b2c8e84d772365fe383b041f70617f4332e2ffeab1a01fa3f8504cae5142a8fbe81eb47a784f1307a4a53e27ff61fbe60ab437c9d5c6b6eeec999428bda7987d6ee77bb9bab6d8215411278efed930bcce8272bf523b0f07572e1d9080a9eca441bd3d1d558c721c2db1bb13dbc74a4da40214e4a5b6c576d050eb9d7b30233837e8477786e36747693e4b7b7e20291d211c43dd55a580dd6b3589e10b9a10f8202918e54649c6fd9cadd47dd4117921bc9d855e0dd638d2a0c5d81a87de797261c55fe3df831abb9f7ad6ab96d6fde641a87846d5acc30baa66099c049c0e86306ed499aa59777d622e49047c8911efadc85655e2822de0b206c7a0744d6a0d882ce089ee8cc71eb3d35ead65540c0a5c2bdbda9bf926f5f6180c9e4b2479c429669d4bfe082936d4273303398bc8de9004c29a938beba05b5665370f1e7ab6b7cb0d8b4e4ef2a260e7ae3b749598de5d8af7ea367610bc3be3363d39dc9f70eb0788af9da1d22ede989297e3fcc4c60cc3f6189ae03402231a6c8b675494e0acbc01415bb6fc4113a71cdaf999ce8d47add6ad296b9ea7ba634d4e484368c9df64b58bdfc6728dcf55fc0be7040c9c51070788a84ff1836af5070764994c2ae53433590b5451fe8dd69339ea6637877bd26bf55d6b9b89487a3f51d15cff82e3d02b2878f6545911e981e54d352d8e7e8e08bd295210a4071ec19520eb8a0efed2eb8aa60629d07f2563ffe7959905d2594e6148e72b2e7c567e3f5a7dd21b573d89ce613c1f13ceff637fe790e146e07a610c98a429a6ab9100e4373624be26411babdd1ce84285537b2d9df9caf3b8ed253f264c2231754c2ca39f1d0d2b0fe253df724950f07307e5aa41e6a9a79e1606c2fde0117cbfd5ba9a8f1a0b478cb5ae478b62ff3306a45c5e9e3f0f4b4a422bd649fcdd1f2f80e289acd8ce6050899ef8c461d7537bcc5aef32b836177d6b254ae755f07b58887adee9d25567ae88c605920ea7abf203513889a2cc9c3974c649f7169d0b2f3c5e59c25d75d5a50f3dbaa4fad816e2b034ed5f4739f3468c6a743d4827216b3c0c00e0a85b1e16294605bc436061d965f2eb884f0d11bc5a71fd46b3910c264e4d1588d35b5c9a5cf90d2789aab1ac039735f2ed538665819ddb14ab72870cee310703dae1ac920ad7a6bfd66bc114678cba20e01d9d1a2ee52c879e6b6882576a5d067886cc416a777d1ab2b494eab61071d56439b56f56a135694f41559d7e1ad2d161f8e37efdcec5f3cb3cc05b80cce0d6b80468272b3fd24c31e909887e7ad2337bdc1dc2f6467b030d61ed1b853eca8c5d29b688e0497783bf42cba4dde4a70223250f74f84b9b4cb0bf5d68874dc2b1f14b140c66db37717a0b8a716cc9ced98dbb39f19f14ea5e98df1952f8601ca5b3f1e0839d401851820c64ba1eb1ea3fba03b77b12d4d8a42abcda3afb0f69ba7505d343f1aac1d83c90f9fe9e";
const K384_BLOB: &str = "00309c09f242d3e6556d6c0721d1f33b29a75823fa5caaea94530933c46f40ed1f65ec336461d0570b44283dd34d757b1e13258eba0d07c2876b49d50a7da74497588e1d0d8889";
const K384_ACT_NAME: &str = "000b136a43f615f9fde6a02e4523a0415dd9b2cd72724e0737621027abd252056bb1";
const K384_PK_SHA256: &str = "391a10bf2e2a65d367cd5aad9e0f48f40086c684f08d28913e6bbb32e51eec10";
const K384_SEED: &str = "878591ecce5d6e3425a73a76fa023307ec509a3640a9db5752b5f666c05b6c39ab52c447acbb933c7d1e93ed353885afe0df806b228e9958789c3b08d0a3ddcd";
