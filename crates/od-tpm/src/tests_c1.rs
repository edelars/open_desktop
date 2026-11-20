//! Command-level tests of slice C1 of phase B (S7-23): `RSA_Encrypt`, `RSA_Decrypt`, `ECDH_ZGen`,
//! dictionary-attack protection and the persistent-state snapshot v2. Every command goes through
//! `Tpm::execute` with raw bytes, like the other test modules.
//!
//! Independent evidence (generated 2026-11-07 by a Python script that is not kept in the repository,
//! like the earlier slices' vectors: pure Python `hashlib`/`pow`/integer arithmetic, cross-checked
//! with OpenSSL 4.1.0):
//! - RSAES-PKCS1-v1_5 and RSAES-OAEP (SHA-256, with and without label) padding written in Python
//!   and verified against `openssl pkeyutl` in both directions with an OpenSSL-generated key; the
//!   ciphertexts for the TPM key were made by that Python code;
//! - the raw RSA (NULL scheme) ciphertexts are plain `pow(m, 65537, n)`;
//! - ECDH: the shared point `Z = [d_B] Q_A` is computed by an affine-coordinate P-256 implementation
//!   in Python with the private scalar of an OpenSSL key `B` and the public point of the TPM key
//!   `A`; `openssl pkeyutl -derive` gives the same x. The TPM computes `[d_A] Q_B` with its own key.
//!   By the Diffie-Hellman identity the two agree only if the TPM multiplies correctly;
//! - TPM-made ciphertexts for the external key were decrypted with `openssl pkeyutl -decrypt` once
//!   (OAEP, OAEP with label, RSAES); the tests decrypt them with the private primes of the same
//!   key through the `rsa` crate.
//!
//! The TPM keys are deterministic primaries (`ENTROPY` of `tests.rs`, owner hierarchy); their public
//! values are pinned, which also pins the derivation of the primary keys.

use super::tests_b::*;
use super::tests_obj::*;
use super::*;
use crate::StateError;
use crate::codes::{curve, object_attr as oa, pt};
use crate::marshal::{PublicParams, Scheme, SymDef, TpmtPublic, Unique, Writer};
use sha2::{Digest, Sha256};

fn rsa_decrypt_key(scheme: Scheme) -> TpmtPublic {
    rsa(base() | oa::DECRYPT, SymDef::Null, scheme)
}

fn ecc_decrypt_key(scheme: Scheme) -> TpmtPublic {
    ecc(base() | oa::DECRYPT, SymDef::Null, scheme)
}

const OAEP_SCHEME: Scheme = Scheme {
    alg: alg::OAEP,
    hash: alg::SHA256,
};
const RSAES_SCHEME: Scheme = Scheme {
    alg: alg::RSAES,
    hash: alg::NULL,
};
const ECDH_SCHEME: Scheme = Scheme {
    alg: alg::ECDH,
    hash: alg::SHA256,
};

fn rsaes() -> Vec<u8> {
    u16b(alg::RSAES)
}

fn oaep() -> Vec<u8> {
    cat(&[&u16b(alg::OAEP), &u16b(alg::SHA256)])
}

fn rsa_encrypt(t: &mut Tpm, key: u32, msg: &[u8], scheme: &[u8], label: &[u8]) -> Vec<u8> {
    let p = cat(&[&tpm2b(msg), scheme, &tpm2b(label)]);
    run(t, plain(cc::RSA_ENCRYPT, &[key], &p))
}

fn rsa_decrypt(
    t: &mut Tpm,
    key: u32,
    pw: &[u8],
    ct: &[u8],
    scheme: &[u8],
    label: &[u8],
) -> Vec<u8> {
    let p = cat(&[&tpm2b(ct), scheme, &tpm2b(label)]);
    run(t, with_pw(cc::RSA_DECRYPT, &[key], &[pw], &p))
}

/// The `TPM2B_PUBLIC_KEY_RSA` of a successful `RSA_Encrypt`/`RSA_Decrypt`.
fn out_buffer(resp: &[u8]) -> Vec<u8> {
    let mut r = Reader::new(params(resp, 0));
    let v = r.tpm2b(256).unwrap().to_vec();
    r.finish().unwrap();
    v
}

fn ecc_point(x: &[u8], y: &[u8]) -> Vec<u8> {
    tpm2b(&cat(&[&tpm2b(x), &tpm2b(y)]))
}

fn ecdh_zgen(t: &mut Tpm, key: u32, pw: &[u8], point: &[u8]) -> Vec<u8> {
    run(t, with_pw(cc::ECDH_Z_GEN, &[key], &[pw], point))
}

/// `(x, y)` of the `TPM2B_ECC_POINT` of a successful `ECDH_ZGen`.
fn out_point(resp: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut r = Reader::new(params(resp, 0));
    let point = read_point(&mut r);
    r.finish().unwrap();
    point
}

fn read_point(r: &mut Reader<'_>) -> (Vec<u8>, Vec<u8>) {
    let n = usize::from(r.u16().unwrap());
    let mut p = r.sub(n).unwrap();
    let x = p.tpm2b(32).unwrap().to_vec();
    let y = p.tpm2b(32).unwrap().to_vec();
    p.finish().unwrap();
    (x, y)
}

fn public_rsa_n(public: &[u8]) -> Vec<u8> {
    match unmarshal_public(public).unique {
        Unique::Rsa(n) => n,
        _ => panic!("not an RSA key"),
    }
}

fn public_ecc_point(public: &[u8]) -> (Vec<u8>, Vec<u8>) {
    match unmarshal_public(public).unique {
        Unique::Ecc { x, y } => (x, y),
        _ => panic!("not an ECC key"),
    }
}

fn ext_rsa_encryption_key() -> TpmtPublic {
    let mut p = rsa(oa::DECRYPT | oa::USER_WITH_AUTH, SymDef::Null, Scheme::NULL);
    p.unique = Unique::Rsa(hex(EXT_RSA_N));
    p
}

/// Decrypts `ct` with the private primes of the external key (not the TPM).
fn ext_decrypt(padding: crate::crypto::RsaPadding, label: &[u8], ct: &[u8]) -> Vec<u8> {
    let pq = [hex(EXT_RSA_P), hex(EXT_RSA_Q)].concat();
    let key = crate::crypto::rsa_from_primes(&pq, &hex(EXT_RSA_N)).unwrap();
    let mut rng = crate::random::Drbg::new(&[3u8; 48]).unwrap();
    crate::crypto::rsa_decrypt_private(&key, padding, label, ct, &mut rng)
        .unwrap()
        .to_vec()
}

// ============================================================================ RSA

/// MGF1 with SHA-256 (RFC 8017, B.2.1), written out here so that the check below does not rely on
/// the padding code of the `rsa` crate.
fn mgf1_sha256(seed: &[u8], len: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut counter = 0u32;
    while out.len() < len {
        let mut h = Sha256::new();
        h.update(seed);
        h.update(counter.to_be_bytes());
        out.extend_from_slice(&h.finalize());
        counter += 1;
    }
    out.truncate(len);
    out
}

/// RSAES-OAEP-DECRYPT padding check (RFC 8017, 7.1.2, steps 3.b to 3.g) of the 256-octet encoded
/// message `em` (the output of the raw private operation), SHA-256, MGF1-SHA-256.
fn manual_oaep_unpad(label: &[u8], em: &[u8]) -> Option<Vec<u8>> {
    if em.len() != 256 || em[0] != 0 {
        return None;
    }
    let (masked_seed, masked_db) = (&em[1..33], &em[33..]);
    let seed: Vec<u8> = masked_seed
        .iter()
        .zip(mgf1_sha256(masked_db, 32))
        .map(|(a, b)| a ^ b)
        .collect();
    let db: Vec<u8> = masked_db
        .iter()
        .zip(mgf1_sha256(&seed, 223))
        .map(|(a, b)| a ^ b)
        .collect();
    if db[..32] != Sha256::digest(label)[..] {
        return None;
    }
    let sep = db[32..].iter().position(|&b| b != 0)? + 32;
    (db[sep] == 1).then(|| db[sep + 1..].to_vec())
}

/// RSAES-PKCS1-v1_5 padding check (RFC 8017, 7.2.2): `00 02 PS 00 M` with at least 8 octets of PS.
fn manual_rsaes_unpad(em: &[u8]) -> Option<Vec<u8>> {
    if em.len() != 256 || em[0] != 0 || em[1] != 2 {
        return None;
    }
    let sep = em[2..].iter().position(|&b| b == 0)? + 2;
    (sep >= 10).then(|| em[sep + 1..].to_vec())
}

#[test]
fn rsa_decrypt_opens_independently_made_ciphertexts() {
    let mut t = started();
    let p = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"kpw",
        &rsa_decrypt_key(Scheme::NULL),
    ));
    // The deterministic primary is the pinned key the vectors were made for.
    assert_eq!(public_rsa_n(&p.public), hex(TPM_RSA_N));
    assert!(matches!(
        unmarshal_public(&p.public).params,
        PublicParams::Rsa { .. }
    ));
    let key = p.handle;
    let msg = hex(MSG);
    // OAEP (SHA-256), without and with a label (the zero octet belongs to the label).
    let r = rsa_decrypt(&mut t, key, b"kpw", &hex(TPM_CT_OAEP), &oaep(), b"");
    assert_eq!(out_buffer(&r), msg);
    let r = rsa_decrypt(
        &mut t,
        key,
        b"kpw",
        &hex(TPM_CT_OAEP_LABEL),
        &oaep(),
        b"lbl\0",
    );
    assert_eq!(out_buffer(&r), msg);
    // RSAES-PKCS1-v1_5.
    let r = rsa_decrypt(&mut t, key, b"kpw", &hex(TPM_CT_RSAES), &rsaes(), b"");
    assert_eq!(out_buffer(&r), msg);
    // No padding (the key and inScheme are NULL): the modular exponentiation, 256 octets with
    // the leading zero octet.
    let r = rsa_decrypt(&mut t, key, b"kpw", &hex(TPM_CT_RAW), &u16b(alg::NULL), b"");
    assert_eq!(out_buffer(&r), hex(RAW_MSG));
    // A wrong label or the wrong padding is TPM_RC_VALUE for the cipher text.
    err(
        &rsa_decrypt(
            &mut t,
            key,
            b"kpw",
            &hex(TPM_CT_OAEP_LABEL),
            &oaep(),
            b"xyz\0",
        ),
        Rc::VALUE.in_param(1),
    );
    err(
        &rsa_decrypt(&mut t, key, b"kpw", &hex(TPM_CT_OAEP_LABEL), &oaep(), b""),
        Rc::VALUE.in_param(1),
    );
    err(
        &rsa_decrypt(&mut t, key, b"kpw", &hex(TPM_CT_OAEP), &oaep(), b"lbl\0"),
        Rc::VALUE.in_param(1),
    );
    err(
        &rsa_decrypt(&mut t, key, b"kpw", &hex(TPM_CT_RSAES), &oaep(), b""),
        Rc::VALUE.in_param(1),
    );
}

#[test]
fn rsa_encrypt_to_an_external_key_is_decrypted_by_its_owner() {
    use crate::crypto::RsaPadding;
    let mut t = started();
    let (key, _) = parse_loaded(&load_external(&mut t, &ext_rsa_encryption_key(), NULL));
    let msg = hex(MSG);
    // Raw RSA is deterministic and equals `pow(m, e, n)` computed in Python.
    let r = rsa_encrypt(&mut t, key, &hex(RAW_MSG), &u16b(alg::NULL), b"");
    assert_eq!(out_buffer(&r), hex(EXT_CT_RAW));
    // OAEP and RSAES are randomized by the TPM DRBG; the owner of the key opens them (the same
    // ciphertexts were opened by `openssl pkeyutl -decrypt` when the test was made).
    let c1 = out_buffer(&rsa_encrypt(&mut t, key, &msg, &oaep(), b""));
    let c2 = out_buffer(&rsa_encrypt(&mut t, key, &msg, &oaep(), b""));
    assert_eq!(c1.len(), 256);
    assert_ne!(c1, c2, "OAEP is randomized");
    assert_eq!(ext_decrypt(RsaPadding::Oaep, b"", &c1), msg);
    assert_eq!(ext_decrypt(RsaPadding::Oaep, b"", &c2), msg);
    let c = out_buffer(&rsa_encrypt(&mut t, key, &msg, &oaep(), b"lbl\0"));
    assert_eq!(ext_decrypt(RsaPadding::Oaep, b"lbl\0", &c), msg);
    let c = out_buffer(&rsa_encrypt(&mut t, key, &msg, &rsaes(), b""));
    assert_eq!(ext_decrypt(RsaPadding::Rsaes, b"", &c), msg);
    // The same ciphertexts through a padding check written out in the test (RFC 8017) over the raw
    // private operation: the encryption direction does not depend on the padding code of the `rsa`
    // crate (computed independently: manual MGF1/OAEP and PKCS1 v1.5 unpadding, SHA-256).
    let raw = |c: &[u8]| ext_decrypt(RsaPadding::None, b"", c);
    let c = out_buffer(&rsa_encrypt(&mut t, key, &msg, &oaep(), b""));
    assert_eq!(manual_oaep_unpad(b"", &raw(&c)).unwrap(), msg);
    let c = out_buffer(&rsa_encrypt(&mut t, key, &msg, &oaep(), b"lbl\0"));
    assert_eq!(manual_oaep_unpad(b"lbl\0", &raw(&c)).unwrap(), msg);
    assert!(manual_oaep_unpad(b"other\0", &raw(&c)).is_none());
    let c = out_buffer(&rsa_encrypt(&mut t, key, &msg, &rsaes(), b""));
    assert_eq!(manual_rsaes_unpad(&raw(&c)).unwrap(), msg);
    // The label is bound to the ciphertext: the private side with another label refuses it.
    let pq = [hex(EXT_RSA_P), hex(EXT_RSA_Q)].concat();
    let ext = crate::crypto::rsa_from_primes(&pq, &hex(EXT_RSA_N)).unwrap();
    let mut rng = crate::random::Drbg::new(&[3u8; 48]).unwrap();
    let c = out_buffer(&rsa_encrypt(&mut t, key, &msg, &oaep(), b"lbl\0"));
    assert!(
        crate::crypto::rsa_decrypt_private(&ext, RsaPadding::Oaep, b"other\0", &c, &mut rng)
            .is_none()
    );
}

#[test]
fn rsa_encrypt_then_decrypt_in_the_tpm_for_every_scheme_combination() {
    let mut t = started();
    let msg = b"round trip".to_vec();
    // (scheme of the key, schemes of inScheme that work, the padding they select)
    for (key_scheme, ok_schemes) in [
        (Scheme::NULL, vec![rsaes(), oaep()]),
        (RSAES_SCHEME, vec![u16b(alg::NULL), rsaes()]),
        (OAEP_SCHEME, vec![u16b(alg::NULL), oaep()]),
    ] {
        let key = parse_primary(&create_primary(
            &mut t,
            OWNER,
            b"kpw",
            &rsa_decrypt_key(key_scheme),
        ))
        .handle;
        for s in &ok_schemes {
            let c = out_buffer(&rsa_encrypt(&mut t, key, &msg, s, b""));
            assert_eq!(c.len(), 256);
            let r = rsa_decrypt(&mut t, key, b"kpw", &c, s, b"");
            assert_eq!(out_buffer(&r), msg, "scheme {key_scheme:?} in {s:?}");
        }
        // The other padding is TPM_RC_SCHEME for parameter 2 (Part 3, Table 42).
        let wrong = match key_scheme.alg {
            alg::RSAES => Some(oaep()),
            alg::OAEP => Some(rsaes()),
            _ => None,
        };
        if let Some(w) = wrong {
            err(
                &rsa_encrypt(&mut t, key, &msg, &w, b""),
                Rc::SCHEME.in_param(2),
            );
            err(
                &rsa_decrypt(&mut t, key, b"kpw", &[0u8; 256], &w, b""),
                Rc::SCHEME.in_param(2),
            );
        }
        // Both NULL on a NULL-scheme key is raw RSA: a message below n comes back unchanged
        // (as 256 octets with leading zeros).
        if key_scheme.alg == alg::NULL {
            let c = out_buffer(&rsa_encrypt(&mut t, key, &msg, &u16b(alg::NULL), b""));
            let r = rsa_decrypt(&mut t, key, b"kpw", &c, &u16b(alg::NULL), b"");
            let plain = out_buffer(&r);
            assert_eq!(plain.len(), 256);
            assert_eq!(&plain[256 - msg.len()..], &msg[..]);
            assert!(plain[..256 - msg.len()].iter().all(|b| *b == 0));
        }
        assert!(flush(&mut t, key) != vec![]);
    }
    // The message size limits: k - 2 hLen - 2 = 190 for OAEP, k - 11 = 245 for RSAES.
    let key = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"kpw",
        &rsa_decrypt_key(Scheme::NULL),
    ))
    .handle;
    ok(&rsa_encrypt(&mut t, key, &[7u8; 190], &oaep(), b""));
    err(
        &rsa_encrypt(&mut t, key, &[7u8; 191], &oaep(), b""),
        Rc::VALUE.in_param(1),
    );
    ok(&rsa_encrypt(&mut t, key, &[7u8; 245], &rsaes(), b""));
    err(
        &rsa_encrypt(&mut t, key, &[7u8; 246], &rsaes(), b""),
        Rc::VALUE.in_param(1),
    );
    let c = out_buffer(&rsa_encrypt(&mut t, key, &[7u8; 190], &oaep(), b""));
    assert_eq!(
        out_buffer(&rsa_decrypt(&mut t, key, b"kpw", &c, &oaep(), b"")),
        vec![7u8; 190]
    );
    // Raw: an integer not below n is TPM_RC_VALUE, in both directions.
    err(
        &rsa_encrypt(&mut t, key, &[0xFF; 256], &u16b(alg::NULL), b""),
        Rc::VALUE.in_param(1),
    );
    err(
        &rsa_decrypt(&mut t, key, b"kpw", &[0xFF; 256], &u16b(alg::NULL), b""),
        Rc::VALUE.in_param(1),
    );
}

#[test]
fn rsa_encryption_command_errors() {
    let mut t = started();
    let rsa_key = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"kpw",
        &rsa_decrypt_key(Scheme::NULL),
    ))
    .handle;
    let sign_key = parse_primary(&create_primary(&mut t, OWNER, b"kpw", &rsa_sign())).handle;
    let storage = parse_primary(&create_primary(&mut t, OWNER, b"kpw", &rsa_storage())).handle;
    let ecc_key = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"kpw",
        &ecc_decrypt_key(Scheme::NULL),
    ))
    .handle;
    let (ext, _) = parse_loaded(&load_external(&mut t, &ext_rsa_encryption_key(), NULL));
    let msg = b"m";
    let ct = vec![1u8; 256];
    // Part 3, 14.3.1: an RSA key (TPM_RC_KEY) with restricted CLEAR and decrypt SET
    // (TPM_RC_ATTRIBUTES); 14.2.1 checks only that the key is an RSA key.
    err(
        &rsa_decrypt(&mut t, ecc_key, b"kpw", &ct, &oaep(), b""),
        Rc::KEY.in_handle(1),
    );
    err(
        &rsa_decrypt(&mut t, sign_key, b"kpw", &ct, &oaep(), b""),
        Rc::ATTRIBUTES.in_handle(1),
    );
    err(
        &rsa_decrypt(&mut t, storage, b"kpw", &ct, &oaep(), b""),
        Rc::ATTRIBUTES.in_handle(1),
    );
    err(
        &rsa_encrypt(&mut t, ecc_key, msg, &oaep(), b""),
        Rc::KEY.in_handle(1),
    );
    // Any RSA public key encrypts, whatever its attributes (the text checks none), as long as its
    // scheme is NULL: a signing scheme selects no padding (Part 3, Table 42).
    ok(&rsa_encrypt(&mut t, storage, msg, &rsaes(), b""));
    ok(&rsa_encrypt(&mut t, storage, msg, &oaep(), b""));
    err(
        &rsa_encrypt(&mut t, sign_key, msg, &oaep(), b""),
        Rc::SCHEME.in_param(2),
    );
    // A key that is only public cannot decrypt: TPM_RC_AUTH_UNAVAILABLE.
    err(
        &rsa_decrypt(&mut t, ext, b"", &ct, &oaep(), b""),
        Rc::AUTH_UNAVAILABLE,
    );
    // The label must end with a zero octet (parameter 3), for both commands.
    err(
        &rsa_encrypt(&mut t, rsa_key, msg, &oaep(), b"label"),
        Rc::VALUE.in_param(3),
    );
    err(
        &rsa_decrypt(&mut t, rsa_key, b"kpw", &ct, &oaep(), b"label"),
        Rc::VALUE.in_param(3),
    );
    // inScheme: a selector other than NULL/RSAES/OAEP, OAEP with another hash or without one.
    err(
        &rsa_encrypt(&mut t, rsa_key, msg, &u16b(alg::RSASSA), b""),
        Rc::VALUE.in_param(2),
    );
    err(
        &rsa_encrypt(
            &mut t,
            rsa_key,
            msg,
            &cat(&[&u16b(alg::OAEP), &u16b(0x0004)]),
            b"",
        ),
        Rc::HASH.in_param(2),
    );
    err(
        &rsa_encrypt(
            &mut t,
            rsa_key,
            msg,
            &cat(&[&u16b(alg::OAEP), &u16b(alg::NULL)]),
            b"",
        ),
        Rc::HASH.in_param(2),
    );
    // A signing key's RSASSA scheme selects no padding.
    let mut rsassa_pub = rsa_decrypt_key(Scheme::NULL);
    rsassa_pub.attributes |= oa::SIGN;
    rsassa_pub.params = PublicParams::Rsa {
        symmetric: SymDef::Null,
        scheme: RSASSA,
        key_bits: 2048,
        exponent: 0,
    };
    rsassa_pub.unique = Unique::Rsa(hex(EXT_RSA_N));
    let (rs, _) = parse_loaded(&load_external(&mut t, &rsassa_pub, NULL));
    err(
        &rsa_encrypt(&mut t, rs, msg, &oaep(), b""),
        Rc::SCHEME.in_param(2),
    );
    // Sizes: a message or cipher text above 256 octets, trailing octets, a cipher text of the
    // wrong length for the padding, a tampered OAEP cipher text.
    let long = cat(&[&u16b(257), &[0u8; 257], &oaep(), &u16b(0)]);
    err(
        &run(&mut t, plain(cc::RSA_ENCRYPT, &[rsa_key], &long)),
        Rc::SIZE.in_param(1),
    );
    let mut trailing = cat(&[&tpm2b(msg), &oaep(), &tpm2b(b"")]);
    trailing.push(0);
    err(
        &run(&mut t, plain(cc::RSA_ENCRYPT, &[rsa_key], &trailing)),
        Rc::SIZE,
    );
    err(
        &rsa_decrypt(&mut t, rsa_key, b"kpw", &ct[..255], &oaep(), b""),
        Rc::VALUE.in_param(1),
    );
    let mut c = out_buffer(&rsa_encrypt(&mut t, rsa_key, msg, &oaep(), b""));
    c[100] ^= 1;
    err(
        &rsa_decrypt(&mut t, rsa_key, b"kpw", &c, &oaep(), b""),
        Rc::VALUE.in_param(1),
    );
    // Handles and sessions: RSA_Encrypt takes no authorization, RSA_Decrypt needs one.
    err(
        &run(
            &mut t,
            with_pw(
                cc::RSA_ENCRYPT,
                &[rsa_key],
                &[b""],
                &cat(&[&tpm2b(msg), &oaep(), &tpm2b(b"")]),
            ),
        ),
        Rc::ATTRIBUTES.in_session(1),
    );
    err(
        &run(
            &mut t,
            plain(
                cc::RSA_DECRYPT,
                &[rsa_key],
                &cat(&[&tpm2b(&ct), &oaep(), &tpm2b(b"")]),
            ),
        ),
        Rc::AUTH_MISSING,
    );
    err(
        &rsa_decrypt(&mut t, rsa_key, b"wrong", &ct, &oaep(), b""),
        Rc::AUTH_FAIL.in_session(1),
    );
    err(
        &rsa_encrypt(&mut t, 0x8000_0009, msg, &oaep(), b""),
        Rc::reference_handle(0),
    );
    err(
        &rsa_encrypt(&mut t, OWNER, msg, &oaep(), b""),
        Rc::VALUE.in_handle(1),
    );
}

#[test]
fn rsaes_decryption_rejects_invalid_padding_implicitly() {
    // Deviation recorded in docs/specs/VTPM.md: the `rsa` crate answers an invalid RSAES padding
    // with a deterministic synthetic message (a defence against padding oracles), where Part 3,
    // clause 14.3.1 asks for TPM_RC_VALUE.
    let mut t = started();
    let key = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"kpw",
        &rsa_decrypt_key(Scheme::NULL),
    ))
    .handle;
    let junk = vec![0x42u8; 256];
    let a = out_buffer(&rsa_decrypt(&mut t, key, b"kpw", &junk, &rsaes(), b""));
    let b = out_buffer(&rsa_decrypt(&mut t, key, b"kpw", &junk, &rsaes(), b""));
    assert_eq!(a, b);
    assert_ne!(a, hex(MSG));
}

#[test]
fn rsa_private_values_never_leave_the_tpm_through_decrypt() {
    let mut t = started();
    let key = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"kpw",
        &rsa_decrypt_key(Scheme::NULL),
    ))
    .handle;
    let secret = secret_of(&t, key);
    let r = rsa_decrypt(&mut t, key, b"kpw", &hex(TPM_CT_OAEP), &oaep(), b"");
    assert!(!contains(&r, &secret[..16]) && !contains(&r, &secret[128..144]));
    let r = rsa_decrypt(&mut t, key, b"kpw", &hex(TPM_CT_RAW), &u16b(alg::NULL), b"");
    assert!(!contains(&r, &secret[..16]));
}

// ========================================================================== ECDH

#[test]
fn ecc_parameters_reports_the_p256_registry_values_and_rejects_unsupported_curves() {
    let mut t = started();
    let response = run(
        &mut t,
        plain(cc::ECC_PARAMETERS, &[], &u16b(curve::NIST_P256)),
    );
    let mut reader = Reader::new(params(&response, 0));
    for value in [0x0003, 256, 0x0020, 0x000b, 0x0010] {
        assert_eq!(reader.u16().unwrap(), value);
    }
    for expected in [
        "ffffffff00000001000000000000000000000000ffffffffffffffffffffffff",
        "ffffffff00000001000000000000000000000000fffffffffffffffffffffffc",
        "5ac635d8aa3a93e7b3ebbd55769886bc651d06b0cc53b0f63bce3c3e27d2604b",
        "6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296",
        "4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5",
        "ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551",
        "01",
    ] {
        assert_eq!(reader.tpm2b(32).unwrap(), hex(expected));
    }
    reader.finish().unwrap();
    for id in [0, 4, 0xffff] {
        err(
            &run(&mut t, plain(cc::ECC_PARAMETERS, &[], &u16b(id))),
            Rc::CURVE.in_param(1),
        );
    }
    err(
        &run(&mut t, plain(cc::ECC_PARAMETERS, &[], &[0])),
        Rc::INSUFFICIENT.in_param(1),
    );
    err(
        &run(&mut t, plain(cc::ECC_PARAMETERS, &[], &[0, 3, 0])),
        Rc::SIZE,
    );
    for (code, attributes) in [(0x163, 1 << 25), (0x178, 0)] {
        let (_, _, body, count) = cap_list(&get_cap(&mut t, cap::COMMANDS, code, 1));
        assert_eq!(count, 1);
        assert_eq!(
            u32::from_be_bytes(body.try_into().unwrap()),
            code | attributes
        );
    }
}

#[test]
fn ecdh_key_gen_uses_a_public_only_ecc_key_and_returns_fresh_ephemeral_points() {
    let mut t = started();
    let key = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"pw",
        &ecc_decrypt_key(Scheme::NULL),
    ));
    let mut public = unmarshal_public(&key.public);
    public.attributes = oa::SIGN;
    let PublicParams::Ecc { scheme, .. } = &mut public.params else {
        unreachable!()
    };
    *scheme = Scheme {
        alg: alg::ECDSA,
        hash: alg::SHA256,
    };
    let (external, _) = parse_loaded(&load_external(&mut t, &public, NULL));
    let before = t.persistent_snapshot();
    let request = plain(cc::ECDH_KEY_GEN, &[external], &[]);
    let mut previous = None;
    for _ in 0..2 {
        let response = run(&mut t, request.clone());
        let mut reader = Reader::new(params(&response, 0));
        let shared = read_point(&mut reader);
        let (qx, qy) = read_point(&mut reader);
        reader.finish().unwrap();
        let recovered = ecdh_zgen(&mut t, key.handle, b"pw", &ecc_point(&qx, &qy));
        assert_eq!(shared, out_point(&recovered));
        let public = (qx, qy);
        assert_ne!(previous, Some(public.clone()));
        previous = Some(public);
    }
    assert_eq!(t.persistent_snapshot(), before);
    err(
        &run(&mut t, plain(cc::ECDH_KEY_GEN, &[external], &[0])),
        Rc::SIZE,
    );
    err(
        &run(&mut t, plain(cc::ECDH_KEY_GEN, &[0x80000040], &[])),
        Rc::reference_handle(0),
    );
    let (rsa, _) = parse_loaded(&load_external(&mut t, &ext_rsa_encryption_key(), NULL));
    err(
        &run(&mut t, plain(cc::ECDH_KEY_GEN, &[rsa], &[])),
        Rc::KEY.in_handle(1),
    );
}

#[test]
fn ecdh_zgen_matches_an_independently_computed_shared_point() {
    let mut t = started();
    let p = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"kpw",
        &ecc_decrypt_key(Scheme::NULL),
    ));
    assert_eq!(
        public_ecc_point(&p.public),
        (hex(TPM_EC_X), hex(TPM_EC_Y)),
        "the pinned key the vector was made for"
    );
    // Z = [d_A] Q_B; Python and OpenSSL computed [d_B] Q_A with the private key of B.
    let r = ecdh_zgen(
        &mut t,
        p.handle,
        b"kpw",
        &ecc_point(&hex(EC_B_X), &hex(EC_B_Y)),
    );
    assert_eq!(out_point(&r), (hex(EC_Z_X), hex(EC_Z_Y)));
    // The same key with the scheme ECDH is another key (the template differs) and works too.
    let q = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"kpw",
        &ecc_decrypt_key(ECDH_SCHEME),
    ));
    let (qx, qy) = public_ecc_point(&q.public);
    let a = out_point(&ecdh_zgen(
        &mut t,
        q.handle,
        b"kpw",
        &ecc_point(&hex(EC_B_X), &hex(EC_B_Y)),
    ));
    assert_ne!(a.0, hex(EC_Z_X));
    // Diffie-Hellman between two TPM keys: [d_A] Q_Q == [d_Q] Q_A.
    let ab = out_point(&ecdh_zgen(&mut t, p.handle, b"kpw", &ecc_point(&qx, &qy)));
    let ba = out_point(&ecdh_zgen(
        &mut t,
        q.handle,
        b"kpw",
        &ecc_point(&hex(TPM_EC_X), &hex(TPM_EC_Y)),
    ));
    assert_eq!(ab, ba);
}

#[test]
fn ecdh_zgen_accepts_coordinates_without_their_leading_zeros() {
    let mut t = started();
    let key = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"kpw",
        &ecc_decrypt_key(Scheme::NULL),
    ))
    .handle;
    // A point of the curve whose x has a leading zero octet (one key in 256).
    let mut rng = crate::random::Drbg::new(&[5u8; 48]).unwrap();
    let (x, y) = loop {
        let k = crate::crypto::ecc_generate(&mut rng).unwrap();
        let (x, y) = crate::crypto::ecc_public(&k).unwrap();
        if x[0] == 0 {
            break (x, y);
        }
    };
    let full = out_point(&ecdh_zgen(&mut t, key, b"kpw", &ecc_point(&x, &y)));
    // The same point with the zero octet stripped (a TPM2B without leading zeros) is accepted
    // and gives the same Z.
    let stripped = out_point(&ecdh_zgen(&mut t, key, b"kpw", &ecc_point(&x[1..], &y)));
    assert_eq!(stripped, full);
    // Still refused: a point that is not on the curve once padded (TPM_RC_ECC_POINT), and a
    // coordinate of more than 32 octets (TPM_RC_SIZE).
    err(
        &ecdh_zgen(&mut t, key, b"kpw", &ecc_point(&x[2..], &y)),
        Rc::ECC_POINT.in_param(1),
    );
    let long = [&[0u8][..], &x[..]].concat();
    err(
        &ecdh_zgen(&mut t, key, b"kpw", &ecc_point(&long, &y)),
        Rc::SIZE.in_param(1),
    );
}

#[test]
fn ecdh_zgen_errors() {
    let mut t = started();
    let key = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"kpw",
        &ecc_decrypt_key(Scheme::NULL),
    ))
    .handle;
    let storage = parse_primary(&create_primary(&mut t, OWNER, b"kpw", &ecc_storage())).handle;
    let signer = parse_primary(&create_primary(&mut t, OWNER, b"kpw", &ecc_sign())).handle;
    let rsa_key = parse_primary(&create_primary(&mut t, OWNER, b"kpw", &rsa_sign())).handle;
    let (bx, by) = (hex(EC_B_X), hex(EC_B_Y));
    let good = ecc_point(&bx, &by);
    ok(&ecdh_zgen(&mut t, key, b"kpw", &good));
    // Part 3, 14.5.1: an ECC key (TPM_RC_KEY) with restricted CLEAR and decrypt SET
    // (TPM_RC_ATTRIBUTES).
    err(
        &ecdh_zgen(&mut t, rsa_key, b"kpw", &good),
        Rc::KEY.in_handle(1),
    );
    err(
        &ecdh_zgen(&mut t, storage, b"kpw", &good),
        Rc::ATTRIBUTES.in_handle(1),
    );
    err(
        &ecdh_zgen(&mut t, signer, b"kpw", &good),
        Rc::ATTRIBUTES.in_handle(1),
    );
    // inPoint must be on the curve (TPM_RC_ECC_POINT): a changed y, a point of the wrong size,
    // the point at (0, 0).
    let mut bad_y = by.clone();
    bad_y[31] ^= 1;
    err(
        &ecdh_zgen(&mut t, key, b"kpw", &ecc_point(&bx, &bad_y)),
        Rc::ECC_POINT.in_param(1),
    );
    err(
        &ecdh_zgen(&mut t, key, b"kpw", &ecc_point(&bx[1..], &by)),
        Rc::ECC_POINT.in_param(1),
    );
    err(
        &ecdh_zgen(&mut t, key, b"kpw", &ecc_point(&[0u8; 32], &[0u8; 32])),
        Rc::ECC_POINT.in_param(1),
    );
    err(
        &ecdh_zgen(&mut t, key, b"kpw", &ecc_point(&[], &[])),
        Rc::ECC_POINT.in_param(1),
    );
    // Framing: a coordinate above 32 octets, an inner size that does not match, trailing octets.
    err(
        &ecdh_zgen(&mut t, key, b"kpw", &ecc_point(&[1u8; 33], &by)),
        Rc::SIZE.in_param(1),
    );
    let mut short = good.clone();
    short[1] -= 2;
    err(
        &ecdh_zgen(&mut t, key, b"kpw", &short),
        Rc::INSUFFICIENT.in_param(1),
    );
    let mut long = good.clone();
    long[1] += 1;
    long.push(0);
    err(&ecdh_zgen(&mut t, key, b"kpw", &long), Rc::SIZE.in_param(1));
    let mut trailing = good.clone();
    trailing.push(0);
    err(&ecdh_zgen(&mut t, key, b"kpw", &trailing), Rc::SIZE);
    // Authorization.
    err(
        &ecdh_zgen(&mut t, key, b"nope", &good),
        Rc::AUTH_FAIL.in_session(1),
    );
    err(
        &run(&mut t, plain(cc::ECDH_Z_GEN, &[key], &good)),
        Rc::AUTH_MISSING,
    );
    let (ext, _) = parse_loaded(&load_external(&mut t, &ext_ecc_encryption_key(), NULL));
    err(&ecdh_zgen(&mut t, ext, b"", &good), Rc::AUTH_UNAVAILABLE);
    err(
        &ecdh_zgen(&mut t, 0x8000_0009, b"", &good),
        Rc::reference_handle(0),
    );
}

fn ext_ecc_encryption_key() -> TpmtPublic {
    let mut p = ecc_decrypt_key(Scheme::NULL);
    p.unique = Unique::Ecc {
        x: hex(EC_B_X),
        y: hex(EC_B_Y),
    };
    p
}

// ========================================================================= DA

fn property(t: &mut Tpm, tag: u32) -> u32 {
    let (_, _, body, count) = cap_list(&get_cap(t, cap::TPM_PROPERTIES, tag, 1));
    assert_eq!(count, 1);
    assert_eq!(&body[..4], &u32b(tag)[..]);
    u32::from_be_bytes([body[4], body[5], body[6], body[7]])
}

/// `(failedTries, maxTries, recoveryTime, lockoutRecovery)` as `GetCapability` reports them.
fn da_state(t: &mut Tpm) -> (u32, u32, u32, u32) {
    (
        property(t, pt::LOCKOUT_COUNTER),
        property(t, pt::MAX_AUTH_FAIL),
        property(t, pt::LOCKOUT_INTERVAL),
        property(t, pt::LOCKOUT_RECOVERY),
    )
}

fn in_lockout(t: &mut Tpm) -> bool {
    permanent_attributes(t) & (1 << 9) != 0
}

fn da_parameters(
    t: &mut Tpm,
    pw: &[u8],
    max: u32,
    recovery: u32,
    lockout_recovery: u32,
) -> Vec<u8> {
    let p = cat(&[&u32b(max), &u32b(recovery), &u32b(lockout_recovery)]);
    pw_cmd(t, cc::DICTIONARY_ATTACK_PARAMETERS, &[LOCKOUT], pw, &p)
}

fn da_lock_reset(t: &mut Tpm, pw: &[u8]) -> Vec<u8> {
    pw_cmd(t, cc::DICTIONARY_ATTACK_LOCK_RESET, &[LOCKOUT], pw, &[])
}

/// A started TPM with DA parameters (max 3, recovery 600 s, lockout recovery 86400 s) and a signing
/// key with the password `kpw`; returns the key handle.
fn da_setup() -> (Tpm, u32) {
    let mut t = started();
    ok(&da_parameters(&mut t, b"", 3, 600, 86_400));
    let key = parse_primary(&create_primary(&mut t, OWNER, b"kpw", &ecc_sign())).handle;
    (t, key)
}

fn sign_with(t: &mut Tpm, key: u32, pw: &[u8]) -> Vec<u8> {
    sign(t, key, pw, &DIGEST, &ecc_sig_scheme())
}

#[test]
fn da_defaults_are_the_recorded_choice() {
    let mut t = started();
    assert_eq!(da_state(&mut t), (0, 32, 600, 86_400));
    assert!(!in_lockout(&mut t));
}

#[test]
fn wrong_object_passwords_lock_the_tpm_out_at_max_tries() {
    let (mut t, key) = da_setup();
    assert_eq!(da_state(&mut t), (0, 3, 600, 86_400));
    for tries in 1..=3 {
        err(&sign_with(&mut t, key, b"bad"), Rc::AUTH_FAIL.in_session(1));
        assert_eq!(da_state(&mut t).0, tries);
    }
    // Lockout: failedTries equals maxTries; TPM_RC_LOCKOUT even for the right password, before the
    // password is looked at, and the counter stays.
    assert!(in_lockout(&mut t));
    err(&sign_with(&mut t, key, b"kpw"), Rc::LOCKOUT);
    err(&sign_with(&mut t, key, b"bad"), Rc::LOCKOUT);
    assert_eq!(da_state(&mut t).0, 3);
    // What is not DA protected still works: the hierarchy authorizations (owner, endorsement,
    // platform), PCR, objects without authorization needs.
    ok(&set_auth(&mut t, OWNER, b"", b"own"));
    ok(&set_auth(&mut t, OWNER, b"own", b""));
    ok(&create_primary(&mut t, OWNER, b"another", &ecc_sign()));
}

#[test]
fn da_protects_only_objects_nv_indices_and_lockout_auth() {
    use crate::nv::attr as nv_attrs;
    let (mut t, key) = da_setup();
    // Not DA protected: hierarchy authorizations (except lockout), a PCR, a noDA object, an object
    // of the platform hierarchy, a PLATFORMCREATE index. Wrong values are TPM_RC_BAD_AUTH and
    // change nothing.
    let no_da = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"ndpw",
        &ecc(base() | oa::SIGN | oa::NO_DA, SymDef::Null, ECDSA),
    ))
    .handle;
    let platform_key = parse_primary(&create_primary(&mut t, PLATFORM, b"ppw", &ecc_sign())).handle;
    ok(&define(
        &mut t,
        PLATFORM,
        NV0 + 7,
        nv_attrs::AUTHWRITE | nv_attrs::AUTHREAD | nv_attrs::PLATFORMCREATE,
        4,
    ));
    for _ in 0..6 {
        for who in [OWNER, ENDORSEMENT, PLATFORM] {
            err(
                &pw_cmd(
                    &mut t,
                    cc::HIERARCHY_CHANGE_AUTH,
                    &[who],
                    b"x",
                    &tpm2b(b"y"),
                ),
                Rc::BAD_AUTH.in_session(1),
            );
        }
        err(
            &run(
                &mut t,
                with_pw(cc::PCR_EXTEND, &[16], &[b"x"], &sha256_ha(&[1; 32])),
            ),
            Rc::BAD_AUTH.in_session(1),
        );
        err(
            &sign_with(&mut t, no_da, b"bad"),
            Rc::BAD_AUTH.in_session(1),
        );
        err(
            &sign_with(&mut t, platform_key, b"bad"),
            Rc::BAD_AUTH.in_session(1),
        );
        err(
            &nv_write(&mut t, NV0 + 7, NV0 + 7, b"bad", &[1], 0),
            Rc::BAD_AUTH.in_session(1),
        );
    }
    assert_eq!(da_state(&mut t).0, 0);
    // A DA-protected NV index and object count.
    ok(&define_raw(
        &mut t,
        OWNER,
        b"",
        b"nvpw",
        &nv_public(
            NV0 + 1,
            alg::SHA256,
            nv_attrs::AUTHWRITE | nv_attrs::AUTHREAD,
            &[],
            4,
        ),
    ));
    err(
        &nv_write(&mut t, NV0 + 1, NV0 + 1, b"bad", &[1], 0),
        Rc::AUTH_FAIL.in_session(1),
    );
    assert_eq!(da_state(&mut t).0, 1);
    // Lock the TPM out: the DA-protected NV index and object are refused, the others work.
    err(&sign_with(&mut t, key, b"bad"), Rc::AUTH_FAIL.in_session(1));
    err(&sign_with(&mut t, key, b"bad"), Rc::AUTH_FAIL.in_session(1));
    assert!(in_lockout(&mut t));
    err(&sign_with(&mut t, key, b"kpw"), Rc::LOCKOUT);
    err(
        &nv_write(&mut t, NV0 + 1, NV0 + 1, b"nvpw", &[1], 0),
        Rc::LOCKOUT,
    );
    ok(&sign_with(&mut t, no_da, b"ndpw"));
    ok(&sign_with(&mut t, platform_key, b"ppw"));
    ok(&nv_write(&mut t, NV0 + 7, NV0 + 7, b"", &[1, 2, 3, 4], 0));
    // The owner authorization is not DA protected and still works.
    ok(&set_auth(&mut t, OWNER, b"", b"own"));
}

#[test]
fn da_lockout_ends_by_self_healing_and_each_failure_restarts_the_timer() {
    let (mut t, key) = da_setup();
    err(&sign_with(&mut t, key, b"bad"), Rc::AUTH_FAIL.in_session(1));
    err(&sign_with(&mut t, key, b"bad"), Rc::AUTH_FAIL.in_session(1));
    // recoveryTime is 600 s: one failure is forgotten per interval without a failure.
    pass_time(&mut t, 599);
    assert_eq!(da_state(&mut t).0, 2);
    pass_time(&mut t, 1);
    assert_eq!(da_state(&mut t).0, 1);
    pass_time(&mut t, 599);
    assert_eq!(da_state(&mut t).0, 1);
    pass_time(&mut t, 1);
    assert_eq!(da_state(&mut t).0, 0);
    // Not below zero, however long the TPM runs.
    pass_time(&mut t, 100_000);
    assert_eq!(da_state(&mut t).0, 0);
    // A new failure restarts the timer.
    err(&sign_with(&mut t, key, b"bad"), Rc::AUTH_FAIL.in_session(1));
    pass_time(&mut t, 400);
    err(&sign_with(&mut t, key, b"bad"), Rc::AUTH_FAIL.in_session(1));
    pass_time(&mut t, 599);
    assert_eq!(da_state(&mut t).0, 2);
    pass_time(&mut t, 1);
    assert_eq!(da_state(&mut t).0, 1);
    // Lockout ends when failedTries drops below maxTries.
    err(&sign_with(&mut t, key, b"bad"), Rc::AUTH_FAIL.in_session(1));
    err(&sign_with(&mut t, key, b"bad"), Rc::AUTH_FAIL.in_session(1));
    assert!(in_lockout(&mut t));
    err(&sign_with(&mut t, key, b"kpw"), Rc::LOCKOUT);
    pass_time(&mut t, 600);
    assert!(!in_lockout(&mut t));
    ok(&sign_with(&mut t, key, b"kpw"));
    // A correct authorization does not clear the counter.
    assert_eq!(da_state(&mut t).0, 2);
}

#[test]
fn da_parameters_zero_disable_or_force_the_lockout() {
    let (mut t, key) = da_setup();
    // recoveryTime 0 disables DA: failures are TPM_RC_BAD_AUTH and nothing counts.
    ok(&da_parameters(&mut t, b"", 3, 0, 86_400));
    for _ in 0..10 {
        err(&sign_with(&mut t, key, b"bad"), Rc::BAD_AUTH.in_session(1));
    }
    assert_eq!(da_state(&mut t).0, 0);
    assert!(!in_lockout(&mut t));
    ok(&sign_with(&mut t, key, b"kpw"));
    // maxTries 0 with DA on: the TPM is in lockout from the start (Part 3, clause 25.3.1).
    ok(&da_parameters(&mut t, b"", 0, 600, 86_400));
    assert!(in_lockout(&mut t));
    err(&sign_with(&mut t, key, b"kpw"), Rc::LOCKOUT);
    ok(&da_parameters(&mut t, b"", 3, 600, 86_400));
    assert!(!in_lockout(&mut t));
    ok(&sign_with(&mut t, key, b"kpw"));
}

#[test]
fn da_lock_reset_clears_the_lockout_and_both_commands_need_lockout_auth() {
    let (mut t, key) = da_setup();
    ok(&set_auth(&mut t, LOCKOUT, b"", b"lk"));
    for _ in 0..3 {
        err(&sign_with(&mut t, key, b"bad"), Rc::AUTH_FAIL.in_session(1));
    }
    err(&sign_with(&mut t, key, b"kpw"), Rc::LOCKOUT);
    // The other authorizations and handles are no substitute for lockoutAuth.
    let params = cat(&[&u32b(9), &u32b(9), &u32b(9)]);
    for who in [OWNER, PLATFORM, ENDORSEMENT, NULL] {
        err(
            &pw_cmd(
                &mut t,
                cc::DICTIONARY_ATTACK_PARAMETERS,
                &[who],
                b"",
                &params,
            ),
            Rc::VALUE.in_handle(1),
        );
        err(
            &pw_cmd(&mut t, cc::DICTIONARY_ATTACK_LOCK_RESET, &[who], b"", &[]),
            Rc::VALUE.in_handle(1),
        );
    }
    // A wrong lockoutAuth is TPM_RC_AUTH_FAIL and puts lockoutAuth itself in a lockout, whatever
    // failedTries is (Part 1, clause 17.8.5); failedTries does not change.
    err(&da_lock_reset(&mut t, b"bad"), Rc::AUTH_FAIL.in_session(1));
    assert_eq!(da_state(&mut t).0, 3);
    err(&da_lock_reset(&mut t, b"lk"), Rc::LOCKOUT);
    err(&da_parameters(&mut t, b"lk", 9, 9, 9), Rc::LOCKOUT);
    pass_time(&mut t, 86_399);
    err(&da_lock_reset(&mut t, b"lk"), Rc::LOCKOUT);
    pass_time(&mut t, 1);
    // lockoutRecovery has passed: the reset works even though the TPM is in lockout.
    ok(&da_lock_reset(&mut t, b"lk"));
    assert_eq!(da_state(&mut t).0, 0);
    assert!(!in_lockout(&mut t));
    ok(&sign_with(&mut t, key, b"kpw"));
    // The parameters change only with lockoutAuth; the values are reported by GetCapability.
    err(
        &da_parameters(&mut t, b"", 9, 9, 9),
        Rc::AUTH_FAIL.in_session(1),
    );
    assert_eq!(da_state(&mut t), (0, 3, 600, 86_400));
    pass_time(&mut t, 86_400);
    ok(&da_parameters(&mut t, b"lk", 10, 20, 30));
    assert_eq!(da_state(&mut t), (0, 10, 20, 30));
    // Framing: three UINT32 parameters, no more, no fewer.
    let mut long = params.clone();
    long.push(0);
    err(
        &pw_cmd(
            &mut t,
            cc::DICTIONARY_ATTACK_PARAMETERS,
            &[LOCKOUT],
            b"lk",
            &long,
        ),
        Rc::SIZE,
    );
    err(
        &pw_cmd(
            &mut t,
            cc::DICTIONARY_ATTACK_PARAMETERS,
            &[LOCKOUT],
            b"lk",
            &params[..8],
        ),
        Rc::INSUFFICIENT.in_param(3),
    );
    err(
        &pw_cmd(
            &mut t,
            cc::DICTIONARY_ATTACK_LOCK_RESET,
            &[LOCKOUT],
            b"lk",
            &[0],
        ),
        Rc::SIZE,
    );
    err(
        &run(
            &mut t,
            plain(cc::DICTIONARY_ATTACK_LOCK_RESET, &[LOCKOUT], &[]),
        ),
        Rc::AUTH_MISSING,
    );
}

#[test]
fn lockout_recovery_zero_needs_a_startup_and_a_power_cycle_restarts_the_timer() {
    let (mut t, _key) = da_setup();
    ok(&set_auth(&mut t, LOCKOUT, b"", b"lk"));
    ok(&da_parameters(&mut t, b"lk", 3, 600, 0));
    err(&da_lock_reset(&mut t, b"bad"), Rc::AUTH_FAIL.in_session(1));
    // lockoutRecovery 0: no amount of time re-enables lockoutAuth, `_TPM_Init` and `Startup` do.
    pass_time(&mut t, 1_000_000_000);
    err(&da_lock_reset(&mut t, b"lk"), Rc::LOCKOUT);
    t.init();
    startup(&mut t, 0);
    ok(&da_lock_reset(&mut t, b"lk"));
    // With a lockoutRecovery above zero the wait restarts in the new power cycle.
    ok(&da_parameters(&mut t, b"lk", 3, 600, 50));
    err(&da_lock_reset(&mut t, b"bad"), Rc::AUTH_FAIL.in_session(1));
    t.init();
    startup(&mut t, 0);
    err(&da_lock_reset(&mut t, b"lk"), Rc::LOCKOUT);
    pass_time(&mut t, 50);
    ok(&da_lock_reset(&mut t, b"lk"));
}

#[test]
fn da_counts_failures_of_hmac_sessions_too_and_survives_a_power_cycle() {
    let (mut t, _) = da_setup();
    let p = parse_primary(&create_primary(&mut t, OWNER, b"hpw", &ecc_sign()));
    let mut hs = start_hmac(&mut t, NULL, &[], &[0x11; 16]);
    let mut c = hc(
        cc::SIGN,
        &[p.handle],
        sign_params(&DIGEST, &ecc_sig_scheme()),
        (p.handle, b"wrong"),
    );
    c.names = vec![p.name.clone()];
    err(&run_hmac(&mut t, &mut hs, &c), Rc::AUTH_FAIL.in_session(1));
    assert_eq!(da_state(&mut t).0, 1);
    // failedTries is persistent: it survives `_TPM_Init`, an orderly `Startup` and a snapshot.
    shutdown(&mut t, 0);
    t.init();
    startup(&mut t, 0);
    assert_eq!(da_state(&mut t), (1, 3, 600, 86_400));
    let r = Tpm::restore(&t.persistent_snapshot(), &[9; 48]).unwrap();
    assert_eq!(r.persistent.da, t.persistent.da);
}

#[test]
fn disorderly_startup_adds_a_failed_try_up_to_max_tries() {
    // NOT VERIFIED against TPM 2.0 Library v184 17.8.6; docs/specs/VTPM.md, DA.
    let (mut t, key) = da_setup();
    err(&sign_with(&mut t, key, b"bad"), Rc::AUTH_FAIL.in_session(1));
    assert_eq!(da_state(&mut t), (1, 3, 600, 86_400));
    // An orderly shutdown (`Shutdown`, then the snapshot): no extra failed try.
    shutdown(&mut t, 0);
    let mut r = Tpm::restore(&t.persistent_snapshot(), &[9; 48]).unwrap();
    startup(&mut r, 0);
    assert_eq!(da_state(&mut r), (1, 3, 600, 86_400));
    // A snapshot taken while the TPM runs (no `Shutdown`) is disorderly: one more failed try per
    // power cycle, and never above `maxTries`.
    for expected in [2, 3, 3] {
        let snap = r.persistent_snapshot();
        r = Tpm::restore(&snap, &[9; 48]).unwrap();
        startup(&mut r, 0);
        assert_eq!(da_state(&mut r).0, expected);
    }
    assert!(in_lockout(&mut r));
}

#[test]
fn clear_resets_the_da_state_to_the_defaults() {
    let (mut t, key) = da_setup();
    err(&sign_with(&mut t, key, b"bad"), Rc::AUTH_FAIL.in_session(1));
    ok(&set_auth(&mut t, LOCKOUT, b"", b"lk"));
    err(&da_lock_reset(&mut t, b"bad"), Rc::AUTH_FAIL.in_session(1));
    ok(&pw_cmd(&mut t, cc::CLEAR, &[PLATFORM], b"", &[]));
    assert_eq!(da_state(&mut t), (0, 32, 600, 86_400));
    // lockoutAuth is empty and enabled again.
    ok(&da_lock_reset(&mut t, b""));
}

#[test]
fn capability_reports_the_da_parameters_and_the_new_commands() {
    let mut t = started();
    // The four variable properties follow `TPM_PT_HR_PERSISTENT_AVAIL` in the list.
    let (_, _, body, count) = cap_list(&get_cap(
        &mut t,
        cap::TPM_PROPERTIES,
        pt::LOCKOUT_COUNTER,
        8,
    ));
    assert_eq!(count, 4);
    let values: Vec<(u32, u32)> = body
        .chunks(8)
        .map(|b| {
            (
                u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
                u32::from_be_bytes([b[4], b[5], b[6], b[7]]),
            )
        })
        .collect();
    assert_eq!(
        values,
        vec![
            (pt::LOCKOUT_COUNTER, 0),
            (pt::MAX_AUTH_FAIL, 32),
            (pt::LOCKOUT_INTERVAL, 600),
            (pt::LOCKOUT_RECOVERY, 86_400)
        ]
    );
    assert_eq!(property(&mut t, pt::LOCKOUT_COUNTER), 0);
    // Command attributes: DictionaryAttack* are {NV} with one handle, RSA_Encrypt has one handle
    // and no NV, ECDH_ZGen and RSA_Decrypt one handle.
    let (_, _, body, count) = cap_list(&get_cap(&mut t, cap::COMMANDS, 0x139, 2));
    assert_eq!(count, 2);
    let attrs: Vec<u32> = body
        .chunks(4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    assert_eq!(attrs[0], 0x139 | 1 << 22 | 1 << 25);
    assert_eq!(attrs[1], 0x13A | 1 << 22 | 1 << 25);
    for code in [0x154u32, 0x159, 0x174] {
        let (_, _, body, count) = cap_list(&get_cap(&mut t, cap::COMMANDS, code, 1));
        assert_eq!(count, 1);
        assert_eq!(
            u32::from_be_bytes([body[0], body[1], body[2], body[3]]),
            code | 1 << 25
        );
    }
}

// =================================================================== snapshot v2

const OWN: &[u8] = b"ownerpw";
const END: &[u8] = b"endorsementpw";
const LK: &[u8] = b"lockoutpw";
const NVPW: &[u8] = b"nvindexpw";

struct Rich {
    t: Tpm,
    ecc_public: Vec<u8>,
    rsa_public: Vec<u8>,
    ecc_sig: Vec<u8>,
    rsa_sig: Vec<u8>,
}

fn sealed_data_index_public(index: u32, attributes: u32, size: u16) -> Vec<u8> {
    nv_public(index, alg::SHA256, attributes, &[], size)
}

/// A TPM with every kind of persistent state: the four hierarchy authorization values, DA
/// parameters and a failed try, three NV indices (one write-locked), persistent objects of the
/// owner (ECC and RSA), the endorsement and the platform hierarchy, then `Shutdown(CLEAR)`.
fn rich_tpm() -> Rich {
    use crate::nv::attr as na;
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", OWN));
    ok(&set_auth(&mut t, ENDORSEMENT, b"", END));
    ok(&set_auth(&mut t, LOCKOUT, b"", LK));
    ok(&set_auth(&mut t, PLATFORM, b"", b"platformpw"));
    ok(&da_parameters(&mut t, LK, 5, 111, 222));
    // NV: owner index with data, an index with its own authValue, a write-locked index.
    let owner_rw = na::OWNERWRITE | na::OWNERREAD;
    ok(&define_raw(
        &mut t,
        OWNER,
        OWN,
        b"",
        &sealed_data_index_public(NV0 + 1, owner_rw, 8),
    ));
    ok(&nv_write(
        &mut t,
        OWNER,
        NV0 + 1,
        OWN,
        &[1, 2, 3, 4, 5, 6, 7, 8],
        0,
    ));
    ok(&define_raw(
        &mut t,
        OWNER,
        OWN,
        NVPW,
        &sealed_data_index_public(NV0 + 2, na::AUTHWRITE | na::AUTHREAD, 4),
    ));
    ok(&nv_write(&mut t, NV0 + 2, NV0 + 2, NVPW, b"auth", 0));
    ok(&define_raw(
        &mut t,
        OWNER,
        OWN,
        b"",
        &sealed_data_index_public(NV0 + 3, owner_rw | na::WRITEDEFINE, 4),
    ));
    ok(&nv_write(&mut t, OWNER, NV0 + 3, OWN, b"lock", 0));
    ok(&pw_cmd(
        &mut t,
        cc::NV_WRITE_LOCK,
        &[OWNER, NV0 + 3],
        OWN,
        &[],
    ));
    // Persistent objects.
    let ecc_p = parse_primary(&create_primary_raw(
        &mut t,
        OWNER,
        OWN,
        &creation_params(b"eccpw", b"", &ecc_sign(), b"", &NO_PCR),
    ));
    ok(&evict(&mut t, OWNER, OWN, ecc_p.handle, 0x8100_0001));
    let rsa_p = parse_primary(&create_primary_raw(
        &mut t,
        OWNER,
        OWN,
        &creation_params(b"rsapw", b"", &rsa_sign(), b"", &NO_PCR),
    ));
    ok(&evict(&mut t, OWNER, OWN, rsa_p.handle, 0x8100_0002));
    let end_p = parse_primary(&create_primary_raw(
        &mut t,
        ENDORSEMENT,
        END,
        &creation_params(b"endpw", b"", &ecc_storage(), b"", &NO_PCR),
    ));
    ok(&evict(&mut t, OWNER, OWN, end_p.handle, 0x8101_0001));
    let plat_p = parse_primary(&create_primary_raw(
        &mut t,
        PLATFORM,
        b"platformpw",
        &creation_params(b"platpw", b"", &ecc_sign(), b"", &NO_PCR),
    ));
    ok(&evict(
        &mut t,
        PLATFORM,
        b"platformpw",
        plat_p.handle,
        0x8180_0001,
    ));
    // One failed try, one signature of each signing key.
    err(
        &sign_with(&mut t, 0x8100_0001, b"bad"),
        Rc::AUTH_FAIL.in_session(1),
    );
    let ecc_sig = params(&sign_with(&mut t, 0x8100_0001, b"eccpw"), 0).to_vec();
    let rsa_sig = params(
        &sign(&mut t, 0x8100_0002, b"rsapw", &DIGEST, &rsa_sig_scheme()),
        0,
    )
    .to_vec();
    shutdown(&mut t, 0);
    let ecc_public = params(&read_public(&mut t, 0x8100_0001), 0).to_vec();
    let rsa_public = params(&read_public(&mut t, 0x8100_0002), 0).to_vec();
    Rich {
        t,
        ecc_public,
        rsa_public,
        ecc_sig,
        rsa_sig,
    }
}

fn refix(snapshot: &mut [u8]) {
    let n = snapshot.len() - 16 - 32;
    snapshot[12..16].copy_from_slice(&(n as u32).to_be_bytes());
    let digest = Sha256::digest(&snapshot[16..16 + n]);
    snapshot[16 + n..].copy_from_slice(&digest);
}

fn offset_of(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("pattern in the snapshot")
}

#[test]
fn snapshot_round_trip_keeps_auth_nv_da_and_persistent_objects() {
    use crate::nv::attr as na;
    let rich = rich_tpm();
    let t = rich.t;
    let snap = t.persistent_snapshot();
    let mut r = Tpm::restore(&snap, &[9; 48]).unwrap();
    // The restored TPM is the same persistent state: the snapshot is identical byte for byte.
    assert_eq!(r.persistent_snapshot(), snap);
    startup(&mut r, 0);
    // DA: parameters and the failed try (checked first: self-healing needs time to pass).
    assert_eq!(da_state(&mut r), (1, 5, 111, 222));
    // Hierarchy authorization values: the old ones work, the empty ones do not; platformAuth is
    // volatile and empty after `Startup(CLEAR)`.
    for (who, pw) in [(OWNER, OWN), (ENDORSEMENT, END), (LOCKOUT, LK)] {
        err(
            &set_auth(&mut r, who, b"", b"x"),
            if who == LOCKOUT {
                Rc::AUTH_FAIL
            } else {
                Rc::BAD_AUTH
            }
            .in_session(1),
        );
        pass_time(&mut r, 86_400);
        ok(&set_auth(&mut r, who, pw, pw));
    }
    ok(&set_auth(&mut r, PLATFORM, b"", b""));
    assert_eq!(
        permanent_attributes(&mut r) & 7,
        7,
        "the AuthSet flags are persistent"
    );
    // NV: public areas, data, authValues and the lock.
    assert_eq!(handles_of(&mut r, NV0), vec![NV0 + 1, NV0 + 2, NV0 + 3]);
    let mut orig = t;
    for i in [NV0 + 1, NV0 + 2, NV0 + 3] {
        assert_eq!(nv_read_public(&mut r, i), nv_read_public(&mut orig, i));
    }
    assert_eq!(
        nv_read_data(&nv_read(&mut r, OWNER, NV0 + 1, OWN, 8, 0)),
        [1, 2, 3, 4, 5, 6, 7, 8]
    );
    assert_eq!(
        nv_read_data(&nv_read(&mut r, NV0 + 2, NV0 + 2, NVPW, 4, 0)),
        b"auth"
    );
    err(
        &nv_read(&mut r, NV0 + 2, NV0 + 2, b"", 4, 0),
        Rc::AUTH_FAIL.in_session(1),
    );
    err(
        &nv_write(&mut r, OWNER, NV0 + 3, OWN, b"more", 0),
        Rc::NV_LOCKED,
    );
    assert_ne!(nv_attributes(&mut r, NV0 + 3) & na::WRITELOCKED, 0);
    // Persistent objects: same public areas and names, same keys (deterministic signatures).
    assert_eq!(
        handles_of(&mut r, 0x8100_0000),
        vec![0x8100_0001, 0x8100_0002, 0x8101_0001, 0x8180_0001]
    );
    assert_eq!(
        params(&read_public(&mut r, 0x8100_0001), 0),
        &rich.ecc_public[..]
    );
    assert_eq!(
        params(&read_public(&mut r, 0x8100_0002), 0),
        &rich.rsa_public[..]
    );
    assert_eq!(
        params(&sign_with(&mut r, 0x8100_0001, b"eccpw"), 0),
        &rich.ecc_sig[..]
    );
    assert_eq!(
        params(
            &sign(&mut r, 0x8100_0002, b"rsapw", &DIGEST, &rsa_sig_scheme()),
            0
        ),
        &rich.rsa_sig[..]
    );
    ok(&sign_with(&mut r, 0x8180_0001, b"platpw"));
    // The primary seeds are restored: the same template under the same hierarchy is the same key.
    let again = parse_primary(&create_primary_raw(
        &mut r,
        OWNER,
        OWN,
        &creation_params(b"eccpw", b"", &ecc_sign(), b"", &NO_PCR),
    ));
    assert_eq!(again.public, orig_public_of(&mut orig, 0x8100_0001));
    // An evicted persistent object stays evicted in the next snapshot.
    ok(&evict(&mut r, OWNER, OWN, 0x8100_0001, 0x8100_0001));
    let r2 = Tpm::restore(&r.persistent_snapshot(), &[7; 48]).unwrap();
    assert_eq!(r2.persistent.objects.len(), 3);
}

#[test]
fn snapshot_with_the_maximum_of_nv_and_persistent_objects_round_trips() {
    use crate::nv::{MAX_NV_INDICES, NV_POOL_SIZE, attr as na};
    use crate::object::MAX_PERSISTENT;
    let mut t = started();
    // The whole NV pool in the most indices: 128 indices of 128 octets, all with data.
    let per = NV_POOL_SIZE / MAX_NV_INDICES;
    for i in 1..=MAX_NV_INDICES as u32 {
        let attrs = na::OWNERWRITE | na::OWNERREAD;
        ok(&define_raw(
            &mut t,
            OWNER,
            b"",
            b"",
            &sealed_data_index_public(NV0 + i, attrs, per as u16),
        ));
        ok(&nv_write(
            &mut t,
            OWNER,
            NV0 + i,
            b"",
            &vec![i as u8; per],
            0,
        ));
    }
    // The most persistent objects, each with its own authValue.
    for i in 0..MAX_PERSISTENT as u32 {
        let auth = format!("persistent-{i}");
        let p = parse_primary(&create_primary(&mut t, OWNER, auth.as_bytes(), &ecc_sign()));
        ok(&evict(&mut t, OWNER, b"", p.handle, 0x8100_0001 + i));
        ok(&flush(&mut t, p.handle));
    }
    let snap = t.persistent_snapshot();
    assert!(
        snap.len() > 4096 + NV_POOL_SIZE - 1,
        "the payload is far above any initial buffer size: {}",
        snap.len()
    );
    let r = Tpm::restore(&snap, &[9; 48]).unwrap();
    assert_eq!(r.persistent_snapshot(), snap);
    assert_eq!(r.persistent.nv.count(), MAX_NV_INDICES);
    assert_eq!(r.persistent.objects.len(), MAX_PERSISTENT);
}

fn orig_public_of(t: &mut Tpm, persistent: u32) -> Vec<u8> {
    let r = read_public(t, persistent);
    let mut rd = Reader::new(params(&r, 0));
    rd.tpm2b(1024).unwrap().to_vec()
}

#[test]
fn snapshot_holds_secrets_in_clear_text_and_says_so() {
    // docs/specs/VTPM.md: the snapshot is sensitive data (S9-27 protects it).
    let rich = rich_tpm();
    let snap = rich.t.persistent_snapshot();
    for secret in [OWN, END, LK, NVPW, &b"lock"[..]] {
        assert!(
            contains(&snap, secret),
            "{}",
            String::from_utf8_lossy(secret)
        );
    }
    assert!(contains(&snap, &rich.t.persistent.owner_seed[..]));
    // `platformAuth` is volatile and does not go into the snapshot.
    assert!(!contains(&snap, b"platformpw"));
    // The Debug output of the TPM and of the errors reveals nothing of it.
    let dbg = format!("{:?}", rich.t);
    assert!(!dbg.contains("ownerpw") && !dbg.contains("seed"));
    let e = format!("{:?} {}", StateError::BadDigest, StateError::BadField);
    assert!(!e.contains("pw"));
}

#[test]
fn damaged_snapshots_are_refused() {
    let rich = rich_tpm();
    let snap = rich.t.persistent_snapshot();
    let entropy = [9u8; 48];
    assert!(Tpm::restore(&snap, &entropy).is_ok());
    // Every flipped bit of every octet (header, payload, digest) is an error.
    for i in 0..snap.len() {
        for bit in 0u8..8 {
            let mut b = snap.clone();
            b[i] ^= 1 << bit;
            assert!(
                Tpm::restore(&b, &entropy).is_err(),
                "bit {bit} of octet {i}"
            );
        }
    }
    // Every truncation and every extension.
    for n in 0..snap.len() {
        assert!(Tpm::restore(&snap[..n], &entropy).is_err(), "cut at {n}");
    }
    let mut longer = snap.clone();
    longer.push(0);
    assert_eq!(
        Tpm::restore(&longer, &entropy).unwrap_err(),
        StateError::BadLength
    );
    // A payload length above what the snapshot holds or above the limit.
    let mut b = snap.clone();
    b[12..16].copy_from_slice(&(snap.len() as u32).to_be_bytes());
    assert_eq!(
        Tpm::restore(&b, &entropy).unwrap_err(),
        StateError::BadLength
    );
    let mut b = snap.clone();
    b[12..16].copy_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(
        Tpm::restore(&b, &entropy).unwrap_err(),
        StateError::BadLength
    );
    // Wrong magic, flags, entropy.
    let mut b = snap.clone();
    b[0] = b'X';
    assert_eq!(
        Tpm::restore(&b, &entropy).unwrap_err(),
        StateError::BadMagic
    );
    let mut b = snap.clone();
    b[11] = 1;
    assert_eq!(
        Tpm::restore(&b, &entropy).unwrap_err(),
        StateError::BadField
    );
    assert_eq!(
        Tpm::restore(&snap, &[0u8; 47]).unwrap_err(),
        StateError::InsufficientEntropy
    );
}

#[test]
fn snapshots_of_another_version_are_refused() {
    let snap = tpm_new().persistent_snapshot();
    assert_eq!(
        u16::from_be_bytes([snap[8], snap[9]]),
        crate::SNAPSHOT_VERSION
    );
    for v in [0u16, 1, 2, 4, 0x0100, u16::MAX] {
        let mut b = snap.clone();
        b[8..10].copy_from_slice(&v.to_be_bytes());
        assert_eq!(
            Tpm::restore(&b, &[9; 48]).unwrap_err(),
            StateError::UnsupportedVersion(v)
        );
    }
    // The only snapshot v1 ever made (seeds and counters, 161 octets) is refused as version 1,
    // not restored into a TPM with empty authorization values and NV.
    let mut v1 = Vec::new();
    v1.extend_from_slice(b"ODTPMSTA");
    v1.extend_from_slice(&1u16.to_be_bytes());
    v1.extend_from_slice(&0u16.to_be_bytes());
    v1.extend_from_slice(&113u32.to_be_bytes());
    let payload = vec![7u8; 113];
    v1.extend_from_slice(&payload);
    {
        v1.extend_from_slice(&Sha256::digest(&payload));
    }
    assert_eq!(v1.len(), 161);
    assert_eq!(
        Tpm::restore(&v1, &[9; 48]).unwrap_err(),
        StateError::UnsupportedVersion(1)
    );
}

#[test]
fn persistent_rsa_snapshot_validates_the_encoded_exponent() {
    const HANDLE: u32 = 0x8100_0001;
    let mut t = started();
    let mut template = rsa_sign();
    let PublicParams::Rsa { exponent, .. } = &mut template.params else {
        unreachable!("RSA signing template");
    };
    *exponent = 65_537;
    let primary = parse_primary(&create_primary_raw(
        &mut t,
        OWNER,
        b"",
        &creation_params(b"rsapw", b"", &template, b"", &NO_PCR),
    ));
    ok(&evict(&mut t, OWNER, b"", primary.handle, HANDLE));
    let snapshot = t.persistent_snapshot();
    let public_response = read_public(&mut t, HANDLE);
    let signature = sign(&mut t, HANDLE, b"rsapw", &DIGEST, &rsa_sig_scheme());
    let mut restored = Tpm::restore(&snapshot, &[9; 48]).unwrap();
    assert_eq!(restored.persistent_snapshot(), snapshot);
    startup(&mut restored, 0);
    assert_eq!(
        params(&read_public(&mut restored, HANDLE), 0),
        params(&public_response, 0)
    );
    assert_eq!(
        params(
            &sign(&mut restored, HANDLE, b"rsapw", &DIGEST, &rsa_sig_scheme()),
            0
        ),
        params(&signature, 0)
    );

    let object = t.persistent.objects.get(HANDLE).unwrap();
    let encoded_public = |public: &TpmtPublic| {
        let mut writer = Writer::new();
        public.marshal(&mut writer);
        writer.finish()
    };
    let original = encoded_public(&object.public);
    let mut incompatible = object.public.clone();
    let PublicParams::Rsa { exponent, .. } = &mut incompatible.params else {
        unreachable!("persistent RSA key");
    };
    *exponent = 3;
    let sensitive =
        crate::object::sensitive_to_bytes(object.sensitive.as_ref().unwrap(), alg::RSA).unwrap();
    assert!(matches!(
        crate::object::sensitive_from_bytes(&sensitive, &incompatible),
        Err(Rc::BINDING)
    ));
    let mut invalid_snapshot = snapshot;
    let offset = offset_of(&invalid_snapshot, &original);
    invalid_snapshot[offset..offset + original.len()]
        .copy_from_slice(&encoded_public(&incompatible));
    refix(&mut invalid_snapshot);
    assert_eq!(
        Tpm::restore(&invalid_snapshot, &[9; 48]).unwrap_err(),
        StateError::BadField
    );
}

#[test]
fn inconsistent_fields_with_a_valid_digest_are_refused() {
    let rich = rich_tpm();
    let snap = rich.t.persistent_snapshot();
    let entropy = [9u8; 48];
    let refuse = |b: &mut Vec<u8>, expect: StateError, what: &str| {
        refix(b);
        assert_eq!(Tpm::restore(b, &entropy).unwrap_err(), expect, "{what}");
    };
    // Trailing octets.
    let mut b = snap.clone();
    let digest_at = b.len() - 32;
    b.insert(digest_at, 0);
    refuse(&mut b, StateError::BadField, "trailing octet");
    // Duplicate NV handle: the second index takes the handle of the first.
    let mut b = snap.clone();
    let second = offset_of(&b, &u32b(NV0 + 2));
    b[second..second + 4].copy_from_slice(&u32b(NV0 + 1));
    refuse(&mut b, StateError::BadField, "duplicate NV handle");
    // An NV handle that is no NV index.
    let mut b = snap.clone();
    let first = offset_of(&b, &u32b(NV0 + 1));
    b[first] = 0x40;
    refuse(&mut b, StateError::BadField, "NV handle of another type");
    // NV `nameAlg` other than SHA-256.
    let mut b = snap.clone();
    let first = offset_of(&b, &u32b(NV0 + 1));
    b[first + 5] = 0x04;
    refuse(&mut b, StateError::BadField, "NV nameAlg");
    // A persistent handle outside the range of the hierarchy of its object.
    let mut b = snap.clone();
    let at = offset_of(&b, &u32b(0x8100_0001));
    b[at..at + 4].copy_from_slice(&u32b(0x8180_0009));
    refuse(&mut b, StateError::BadField, "persistent handle range");
    // A private part that does not belong to the public area: the ECC scalar changed.
    let scalar = {
        let o = rich.t.persistent.objects.get(0x8100_0001).unwrap();
        match &o.sensitive.as_ref().unwrap().secret {
            crate::object::Secret::Ecc(k) => crate::crypto::ecc_scalar(k).to_vec(),
            _ => unreachable!(),
        }
    };
    let mut b = snap.clone();
    let at = offset_of(&b, &scalar);
    b[at + 5] ^= 1;
    refuse(&mut b, StateError::BadField, "private part of another key");
    // Reserved hierarchy flag bits, a DA flag that is no boolean, an orderly flag that is none.
    let mut b = snap.clone();
    // Payload offsets (the payload starts at octet 16): the flags follow the seeds, the counters
    // and the three `lv8` authorization values.
    let flags_at = 16 + 113 + (1 + OWN.len()) + (1 + END.len()) + (1 + LK.len());
    b[flags_at] |= 0x10;
    refuse(&mut b, StateError::BadField, "hierarchy flags");
    let mut b = snap.clone();
    b[16 + 112] = 2;
    refuse(&mut b, StateError::BadField, "orderly flag");
    let mut b = snap.clone();
    b[flags_at + 1 + 16] = 2;
    refuse(&mut b, StateError::BadField, "lockoutAuth enabled flag");
    // An authorization value above one digest.
    let mut b = snap.clone();
    b[16 + 113] = 33;
    refuse(&mut b, StateError::BadField, "auth value length");
    // The same payload cut off inside the NV block is a short payload.
    let mut b = snap.clone();
    let n = b.len();
    b.drain(n - 32 - 200..n - 32);
    refuse(&mut b, StateError::BadLength, "cut payload");
}

// ================================================================== robustness

/// A small deterministic generator for the mutation tests.
fn lcg(seed: u32) -> impl FnMut() -> usize {
    let mut state = seed;
    move || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 16) as usize
    }
}

#[test]
fn mutated_c1_commands_never_panic_and_answer_well_formed() {
    let mut t = started();
    let rsa_key = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"kpw",
        &rsa_decrypt_key(Scheme::NULL),
    ))
    .handle;
    let ecc_key = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"kpw",
        &ecc_decrypt_key(Scheme::NULL),
    ))
    .handle;
    ok(&da_parameters(&mut t, b"", 1_000_000, 600, 86_400));
    let seeds = vec![
        plain(
            cc::RSA_ENCRYPT,
            &[rsa_key],
            &cat(&[&tpm2b(b"message"), &oaep(), &tpm2b(b"lbl\0")]),
        ),
        with_pw(
            cc::RSA_DECRYPT,
            &[rsa_key],
            &[b"kpw"],
            &cat(&[&tpm2b(&hex(TPM_CT_OAEP)), &oaep(), &tpm2b(b"")]),
        ),
        with_pw(
            cc::ECDH_Z_GEN,
            &[ecc_key],
            &[b"kpw"],
            &ecc_point(&hex(EC_B_X), &hex(EC_B_Y)),
        ),
        with_pw(
            cc::DICTIONARY_ATTACK_PARAMETERS,
            &[LOCKOUT],
            &[b""],
            &cat(&[&u32b(1_000_000), &u32b(600), &u32b(86_400)]),
        ),
        with_pw(cc::DICTIONARY_ATTACK_LOCK_RESET, &[LOCKOUT], &[b""], &[]),
    ];
    let mut next = lcg(0x1234_5678);
    for seed in &seeds {
        for len in 0..=seed.len() {
            let _ = run(&mut t, seed[..len].to_vec());
        }
        for _ in 0..200 {
            let mut c = seed.clone();
            for _ in 0..1 + next() % 3 {
                let i = next() % c.len();
                c[i] = next() as u8;
            }
            if next().is_multiple_of(2) {
                let n = c.len() as u32;
                c[2..6].copy_from_slice(&n.to_be_bytes());
            }
            let _ = run(&mut t, c);
            // Mutated passwords must not lock the keys out of the rest of the test.
            t.persistent.da.failed_tries = 0;
            t.persistent.da.lockout_auth_enabled = true;
            t.persistent.da.max_tries = 1_000_000;
        }
    }
    // The TPM still works.
    let r = rsa_decrypt(&mut t, rsa_key, b"kpw", &hex(TPM_CT_OAEP), &oaep(), b"");
    assert_eq!(out_buffer(&r), hex(MSG));
}

#[test]
fn mutated_snapshot_payloads_never_panic() {
    let rich = rich_tpm();
    let snap = rich.t.persistent_snapshot();
    let mut next = lcg(0x9E37_79B9);
    let mut accepted = 0;
    for _ in 0..400 {
        let mut b = snap.clone();
        for _ in 0..1 + next() % 4 {
            let i = 16 + next() % (b.len() - 16 - 32);
            b[i] = next() as u8;
        }
        // A valid digest gets the mutation past the corruption check into the field checks.
        refix(&mut b);
        if Tpm::restore(&b, &[9; 48]).is_ok() {
            accepted += 1;
        }
    }
    // Most mutations hit a checked field or a secret that must match its public area; the ones
    // that land on free data (auth values, NV data, counters) are legitimate states.
    assert!(accepted < 400);
}

// ------------------------------------------------------------------------------- vectors

const MSG: &str = "4f70656e204465736b746f70207654504d205253412074657374206d657373616765";
const TPM_RSA_N: &str = "df060fa4a851d3d3fcfae3d249171aac8ae5ac804b674ab9c0bcb43cd493efd028e1801d5c600af7d467ee5cea3e6ac91fa428e6b36f08cd714972b7372e3d97831afabacb6fc7a5bbcfc61f60f7c97f2ba37bc413d70618ede1cc08f7c789104722261b7ca559920af4a925cea2022aca7ac80887560416103999c5daee4d16d37a59bf1128bdb71f44530bf69fb77db643a7ef7d429271afeccee75d7de24532bdbce1c820fc5c25d990440c56a74c63b309d509fc3a7abca352183d4858f172af056b3eb159cca1c95be5347aef500985490383e4d84602b7d9e969c890353837e7a9067b9946573cead935173686431e680b36f045d0b6a69abe1f3654d3";
const TPM_CT_OAEP: &str = "57282ed48d49f3afdbd35fea97eb33934eebde020b5c5e578606f865ee9373543c1d57e514f10125aed49db5a5aff64c50da7f773ee4766a018c53223b4dc3ea043aa603ac32b1596ed5deaf4012d96aeb82b9d261d30ec8e04cdf465052a03bc8e899b61d463d89e11f564af9130daf130e2178d91d0d0a116c1f7f833f87eece02543700a0d104abf034a8c3e56a774144e6a85779fb45f399c6fd74ee854b238b5915d9578b17120d63588c6aff28579f1fba0a3ef0402651957b00ce0bdf24ea8cd2ffa19da1e791c5e0a0e0ad3854afeddd147f1f34408c0c3669e48a966ac29e73f2debaccc95e03fd0597ce8456305ff13702dd0be24f6b16ea5b2824";
const TPM_CT_OAEP_LABEL: &str = "7e83fc32ca5b40957d4cc35fb3dd5ce01222e261c50a57e25e051e13f1e4bfc9fb91259f42e96f7439e207e3aedcfec403e5a1b3919a2e8769eede42f48f760303f48ba27a040ca6e3449e38d14fbbcdd308178d3577da81a9acb86b4a23f7e0f9dbbd7003291e7c59bcca9921ab515a629b6173cca5422613652a143011a562eb0d55d1ed2fb01c58cfbcb69287da12f9df07e1ce2a8afea77b926e26ef51188c9b30e5dde95da01353b7179f54aa764137d70061f2735999849efb50f9ad0b34d3127a7fd4519861922dcbcec725b278abeefae20f68ba4954dc789ddfa3c4285dc75c94b875234ce4da649cac2755badcdb2fa145eb28eccd27e10c07a590";
const TPM_CT_RSAES: &str = "526c1adb97f21632075b526bfb6cbeae6a90316e1e3cfce2587605fed4303ac6efd082547492710ae11f2c5f98eaf1cf3096034cc44153de0e91cec82e65d31f1b6032f10e8d650212e29336461627a2aa649dac9f84dbf16ae2ae8ad4c601addf49d80936049706feb378fdc13976ed9be66786fafb2205029ffea9591af9b6c6fc4c8b01c85856fe52c9a35e1b1168dbadb10aa2bedf15c90280e5e65200acbf2a197bb86abc040ab1c617db6fba8568e8ae1ae5339c8c8f7c4e4c1aa113b4417f43c4c4c4465db7c94533c95889ff75bbf4c7be20c7bac782cd7518d80d86e22b4520835d1cda94aad6cc4f78232baa39fef8dcdbb5d03b8cd145ff18c250";
const RAW_MSG: &str = "000a11181f262d343b424950575e656c737a81888f969da4abb2b9c0c7ced5dce3eaf1f8ff060d141b222930373e454c535a61686f767d848b9299a0a7aeb5bcc3cad1d8dfe6edf4fb020910171e252c333a41484f565d646b727980878e959ca3aab1b8bfc6cdd4dbe2e9f0f7fe050c131a21282f363d444b525960676e757c838a91989fa6adb4bbc2c9d0d7dee5ecf3fa01080f161d242b323940474e555c636a71787f868d949ba2a9b0b7bec5ccd3dae1e8eff6fd040b121920272e353c434a51585f666d747b828990979ea5acb3bac1c8cfd6dde4ebf2f900070e151c232a31383f464d545b626970777e858c939aa1a8afb6bdc4cbd2d9e0e7eef5fc";
const TPM_CT_RAW: &str = "147c6b32b87704bfe2f9236aec410a60a1d1dc39a734ddfd8b8301d2378dec3c896278a8e7ead1afce0ebd1dbf055d7dbf40b20e0362e4983e24f0ae81cb5d0e13da81ef69c7e51652cc7da3f5c057ec66670446badecc70d3d57a20c967d70371f3e6822efde5753813a41ecb958122eabe1cf46322713300f9c826c88ab1f78b9b4a35ff8f9740c0bd6c1af5d666fa50ad2c9fa40764b8de0f80dadd17c50b9ad554018dc112536966c772d1792ee12fc517a7f2f2000abf5d168748e6339ae5516905b5df4ba9d1aa9fa3fe47575f880ea3bada8c1cce259251c45e8c535d458515692bf19e9cf584b61a0c32b8bba63986da840f7f17039d64943bfbeb31";
const EXT_RSA_N: &str = "b6b69daba0f0e0d6246da2202df16a574a318e3cfae808680103fc1691dccd0c5c857c3e9bfecf97f7a21adefe26b2252f193ef0ffb69770dc4709e62c591e5ff4fea1a4a74e6df9aaadb9b9e4852d0b249893b1785fdb1f48330c7441fff4dd6a1f24ef3e3f4c10057bbca6ab5cdfe3a01d816cc353dd0b50fa51b25e1a3529d529fb299d3d38b4dadf33b0a6851c6cc7a29c4a63560868f4804ddc5baf1dc709fee622a953c09af5833924dd3efbc8d90a83f69693bb9b5f6ba560df78e895223181c9ac36edca4aec5eeb18d7c7e4d0d8032b13947f0d696f7bc6acaa8c68b58b096b5b8991b103f7afd07bb24592c83516972ff629c7d8a794f98fc98a61";
const EXT_RSA_P: &str = "f0637c28545965c0d1476b756f530aaad92012ac1101640a07e51d183228d705dd5e6393874c8c7c48653834fc19f425fa80a254a6eecd4d9e38d711dc6c737af5a444cfeca628ecf33589d3e2bba3d2e29bbea69b4eaae595c0364282954fa676d881c8bb06d1ade13a09cf58aea6263c8998396c10a813b9e666b3816ba29f";
const EXT_RSA_Q: &str = "c294443a6cd262858996540306d90e8606a9b646cc8e497cc09c53a92d26935791087cccb17d762e97871eb114d11a5cd38c39036953cf4ca646c458085c52b3dade50c76ce4b4900f73238cd44a78cf3124d640d0d35558d9a9fda758f6c25984a0147689ae1d6f9b32303e5c3f3208d2980a3c6dd9919603ce33392180b2ff";
const EXT_CT_RAW: &str = "ac189ad9647c1d2a84ee085617b053ee81e388f0d36491718c840d5a1853dddb12e87281af87d2db7b55d975d08af56794ea4a7ba73190cb11d4d0a681cba6514228b8a55709750a3afcd80e3bc309119d6179f7180692bb4b56be7487f418e24fb8bcdb4cb9576097ba07f425961d90af75d425ee9fabeec703325a21bec0f6df775bcc1f6f2df3f1a4757c93154ca0f036fa09c8ec3da07a59858c3f39b26d11124b19edaeceb37a3dc000bc6106f9dc4fa23ae9b8bbdb34182a2c1094ed6579b9c4219c8406bdbea0552ce3ed20b6549971d8431e7deae6743c71d17a158479e46c785f1ce5673a1b371df55e304c6392abf6341c628e69f5924a29ec0417";
const TPM_EC_X: &str = "868aae5f959c4bb61a75a462b05c50a4e7db6a604b9573a67f6e1e9344ed64ef";
const TPM_EC_Y: &str = "f8541ac4bfb4bb4b512331072a4005b5787e4d8778c87eeab8640ff21f53c999";
const EC_B_X: &str = "e71a5ad7cb2fd316199f6fcd8aa8f21091e1032abb0ed80f7c9d241a62cf5802";
const EC_B_Y: &str = "ba894ee7a6840211c7f0a12a12ca1780a749676b44c88b8f86367ba8f43a677c";
const EC_Z_X: &str = "635d65312dc52e4251b44b1f00532c526afdc2bf5a5ad41bd9c2d94c3753fc4b";
const EC_Z_Y: &str = "9e24242a824b52efdd614b68530dab4d66d36984c81f652c6229aae5f9511143";
