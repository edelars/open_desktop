//! ML-DSA-65 (FIPS 204; S7-23, slice 7): TPMT_PUBLIC of type ML_DSA, SignDigest and
//! VerifyDigestSignature over an external mu, the sign and verify sequences, attestation and
//! PolicySigned with message signing, the limit of two loaded ML-DSA objects, contexts and
//! snapshots, and a CRB frame with a 3309-octet signature.
//!
//! `sha256_golden_unchanged` was recorded at HEAD 8ac583b, before this slice changed any code: the
//! TPM is deterministic (fixed entropy), so the SHA-256 of one transcript of the existing RSA and
//! ECC signing, policy, context and object-memory paths pins their bytes.

use sha2::{Digest, Sha256};

use super::tests_b::{define, nv_write, shutdown, start_session, startup};
use super::tests_c2::{certify, parse_attestation, quote};
use super::tests_obj::{
    NO_PCR, OWNER, Primary, base, contains, create_child, create_primary, create_primary_raw,
    creation_params, ecc_sign, ecc_storage, evict, flush, load, load_external, name_hash,
    parse_child, parse_loaded, parse_primary, read_public, rsa_sign, secret_of, sign,
    unmarshal_public, verify,
};
use super::*;
use crate::codes::{object_attr as oa, se};
use crate::hash::MAX_SEQUENCES;
use crate::marshal::{PublicParams, TpmtPublic, Unique, limits};
use crate::nv::attr as nv_attr;

const NULL: u32 = handle::RH_NULL;

/// The replies of one scenario over the paths that exist at HEAD 8ac583b.
fn golden_transcript() -> Vec<u8> {
    let mut t = started();
    let mut log: Vec<u8> = Vec::new();
    let mut put = |reply: &[u8]| {
        log.extend_from_slice(&(reply.len() as u32).to_be_bytes());
        log.extend_from_slice(reply);
    };
    let ecdsa = cat(&[&u16b(alg::ECDSA), &u16b(alg::SHA256)]);
    let rsassa = cat(&[&u16b(alg::RSASSA), &u16b(alg::SHA256)]);
    for (template, scheme) in [(ecc_sign(), ecdsa.clone()), (rsa_sign(), rsassa.clone())] {
        let created = create_primary(&mut t, OWNER, b"pw", &template);
        put(&created);
        let key = parse_primary(&created);
        put(&read_public(&mut t, key.handle));
        // PolicySigned: a real session, a trial session and a bad signature.
        let nonce = [0u8; 0];
        let a_hash = Sha256::digest(cat(&[&nonce, &0u32.to_be_bytes(), b"", b"ref"]));
        let signed = sign(&mut t, key.handle, b"pw", &a_hash, &scheme);
        put(&signed);
        put(&verify(&mut t, key.handle, &a_hash, params(&signed, 0)));
        let public = unmarshal_public(&key.public);
        let external = load_external(&mut t, &public, NULL);
        put(&external);
        let authority = u32::from_be_bytes(external[10..14].try_into().unwrap());
        for (index, (kind, signature)) in [
            (se::POLICY, params(&signed, 0).to_vec()),
            (se::TRIAL, params(&signed, 0).to_vec()),
            (se::POLICY, {
                let mut bad = params(&signed, 0).to_vec();
                *bad.last_mut().unwrap() ^= 1;
                bad
            }),
        ]
        .into_iter()
        .enumerate()
        {
            let session = start_session(&mut t, kind, NULL, b"", &[0x44; 16]);
            put(&session.nonce_tpm);
            let request = cat(&[
                &tpm2b(&nonce),
                &tpm2b(b""),
                &tpm2b(b"ref"),
                &0u32.to_be_bytes(),
                &signature,
            ]);
            let reply = run(
                &mut t,
                plain(cc::POLICY_SIGNED, &[authority, session.handle], &request),
            );
            assert_eq!(rc(&reply) == 0, index < 2, "PolicySigned {index}");
            put(&reply);
            put(&run(
                &mut t,
                plain(cc::POLICY_GET_DIGEST, &[session.handle], &[]),
            ));
            put(&flush(&mut t, session.handle));
        }
        put(&flush(&mut t, authority));
        // A context round trip and a signature after it.
        let context = run(&mut t, plain(cc::CONTEXT_SAVE, &[key.handle], &[]));
        put(&context);
        put(&flush(&mut t, key.handle));
        let loaded = run(&mut t, plain(cc::CONTEXT_LOAD, &[], params(&context, 0)));
        put(&loaded);
        let handle = u32::from_be_bytes(loaded[10..14].try_into().unwrap());
        put(&sign(&mut t, handle, b"pw", &a_hash, &scheme));
        put(&flush(&mut t, handle));
    }
    // Every transient slot used, then one more.
    for round in 0..9u8 {
        let template = if round % 2 == 0 {
            ecc_sign()
        } else {
            rsa_sign()
        };
        put(&create_primary(&mut t, OWNER, &[round], &template));
    }
    put(&run(
        &mut t,
        plain(
            cc::GET_CAPABILITY,
            &[],
            &cat(&[&u32b(1), &u32b(0x8000_0000), &u32b(20)]),
        ),
    ));
    put(&t.persistent_snapshot());
    log
}

#[test]
fn sha256_golden_unchanged() {
    let digest = Sha256::digest(golden_transcript());
    let hexed: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(
        hexed, "e5e4f8dfb73a6d14181f3ff2d538fb742d5072415f93871d42aee417c5c51ab5",
        "golden transcript digest"
    );
}

// ------------------------------------------------------------------------------- fixtures
//
// Where a value comes from outside this implementation the comment says so. Tools: OpenSSL 4.1.0
// ("openssl genpkey -algorithm ML-DSA-65 -pkeyopt hexseed:...", "pkeyutl -sign/-verify -rawin"
// with "-pkeyopt deterministic:1", "context-string:..." and "mu:1") and Python 3 "hashlib"
// (SHA-256, SHAKE256). The TPM signs with the deterministic variant of FIPS 204, so a signature
// of the key below is reproduced byte for byte by "openssl pkeyutl -sign -pkeyopt deterministic:1".
// - The key is the primary of "mldsa_template(false, true, SHA-256)" under the owner hierarchy with the
//   entropy of "started()". Its seed (the private value, Part 2 Table 211) was read from the
//   TPM; "openssl genpkey -pkeyopt hexseed:<seed>" derives the same 1952-octet public key, and
//   "pkeyutl -verify" accepts every signature below.
// - The mu values are SHAKE256(SHAKE256(pk, 64) || 0 || len(ctx) || ctx || M, 64) (FIPS 204,
//   Algorithm 7, line 6; Algorithm 2, line 10) computed with Python; "pkeyutl -pkeyopt mu:1"
//   signs them to the same bytes as the message signature, and the hedged signature of
//   "mldsa65-openssl-sig-mu.bin" over a mu also verifies as a message signature with its context.
// - "kat/mldsa65-openssl-pub.bin" is the public key of the OpenSSL key with seed 00 01 .. 1f, the
//   three "mldsa65-openssl-sig-*.bin" are signatures that OpenSSL made with it (the default hedged
//   mode, random rnd): over the message "ML-DSA-65 external message" with the context "ctx-1",
//   over the mu of that pair, and over the PolicySigned message 00000000 || "ref".

const TPM_SEED: &str = "9422a64d0ce736a1a1fedacc058781b8d19d90b682c34f136132e30023f64896";
const TPM_PK_SHA256: &str = "ae5653b53bb857237d2799c3a8cc47ea3b650b8af90090817b8bd5d92e9e33f3";
const TPM_NAME: &str = "000b1f4eed7c8cc1ec151605aff71c4930db11de2ce48a6c26a699a3681bf0d55fac";
const MESSAGE: &[u8] = b"ML-DSA-65 TPM message";
const MU: &str = "be5c584910f9ddaae2b7e6c21e46640c8258c0c0349979545b572548d89c1281962edc36857354036b92b52caea9cc0b370617d6c5435ac0b48f7a3ecfb1b382";
const SIG_SHA256: &str = "7bdbbd1ce8dddd55e4811c5834971a1d7cecf3402eb81e31bef60fc35921eb26";
const SIG_EMPTY_SHA256: &str = "7773a09a03ab77fd8330d305000acfa5d19c584308c6826d454f26c9fef92265";
const SIG_CTX_SHA256: &str = "c59b96971980a58e4fc79d8ba830f3a9532b27e9c6fedcb9f35c2dd4d3e3d53a";
const SIG_LONG_SHA256: &str = "916882d00ae8481a47ed1260875282a5bf0db030a383dcfcf26473b1da94f054";
const SIG_POLICY_SHA256: &str = "5ad424d8bf57bc0c31256e2c43170fa80a6a48c24a1881634ef58a6a379ba670";
const LONG_SHA256: &str = "0f1a2812a0c0154ac66d3a7535245d24b36eb2c24b01ea1f1f05e4874a86e3c7";
// The external key, loaded with attributes {sign}, allowExternalMu YES, nameAlg SHA-256.
const EXT_NAME: &str = "000bdd83e0cb4633179ed0da23e7297cfa1b98b12b21e031d3ac6e22734dfec1d67f";
const EXT_MESSAGE: &[u8] = b"ML-DSA-65 external message";
const EXT_MU: &str = "c0557cfd19b5543e7104d122b81eeb9655ea63721b6ee8a3fcd13864387767763cc1dcdab907fc33406692013bbbb510596e5b52666a224051ca28ce63785198";
const EXT_PUBLIC: &[u8] = include_bytes!("kat/mldsa65-openssl-pub.bin");
const EXT_SIG_MSG: &[u8] = include_bytes!("kat/mldsa65-openssl-sig-msg.bin");
const EXT_SIG_MU: &[u8] = include_bytes!("kat/mldsa65-openssl-sig-mu.bin");
const EXT_SIG_POLICY: &[u8] = include_bytes!("kat/mldsa65-openssl-sig-policy.bin");
// The attestation of test "mldsa65_quote_or_certify_verifies" and its TPMT_SIGNATURE; the signature
// verifies with "openssl pkeyutl -verify -rawin" under the key derived from the seed of the signer,
// and "-pkeyopt deterministic:1" signs the attestation to the same octets.
const ATTEST_SHA256: &str = "8aecfa7487ae2fb0ddb36400f25316a50f9c9423575180764b14cef2503d85cb";
const ATTEST_SIGNATURE_SHA256: &str =
    "7ae0b891befac7321777a66456754ccf33c003f5a1b321d9512115fb20c0944e";
const NULL_VALIDATION: [u8; 8] = [0x80, 0x24, 0x40, 0, 0, 7, 0, 0];
const OK_REPLY: &str = "80010000000a00000000";

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_of(&Sha256::digest(bytes))
}

fn mldsa_template(restricted: bool, external_mu: bool, name_alg: u16) -> TpmtPublic {
    TpmtPublic {
        name_alg,
        attributes: base() | oa::SIGN | if restricted { oa::RESTRICTED } else { 0 },
        auth_policy: vec![],
        params: PublicParams::MlDsa {
            parameter_set: 2,
            allow_external_mu: external_mu,
        },
        unique: Unique::MlDsa(vec![]),
    }
}

/// The public part of the OpenSSL key: only `sign`, so that it is a verification key.
fn external_public(external_mu: bool) -> TpmtPublic {
    TpmtPublic {
        name_alg: alg::SHA256,
        attributes: oa::SIGN,
        auth_policy: vec![],
        params: PublicParams::MlDsa {
            parameter_set: 2,
            allow_external_mu: external_mu,
        },
        unique: Unique::MlDsa(EXT_PUBLIC.to_vec()),
    }
}

/// A TPM with the key of the fixtures above.
fn tpm_key() -> (Tpm, Primary) {
    let mut t = started();
    let created = create_primary(
        &mut t,
        OWNER,
        b"pw",
        &mldsa_template(false, true, alg::SHA256),
    );
    ok(&created);
    let key = parse_primary(&created);
    (t, key)
}

fn handle_of(resp: &[u8]) -> u32 {
    ok(resp);
    u32::from_be_bytes(resp[10..14].try_into().unwrap())
}

/// The 3309 octets of a successful `TPMT_SIGNATURE` of an ML-DSA key: `sigAlg` and the size are
/// checked and cut off.
fn sig_octets(resp: &[u8]) -> Vec<u8> {
    ok(resp);
    let p = params(resp, 0);
    assert_eq!(p[..4], [0x00, 0xA1, 0x0C, 0xED], "TPM_ALG_MLDSA, 3309");
    assert_eq!(p.len(), 4 + 3309);
    p[4..].to_vec()
}

fn signature_bytes(octets: &[u8]) -> Vec<u8> {
    cat(&[&u16b(alg::MLDSA), &tpm2b(octets)])
}

fn sign_message(t: &mut Tpm, key: u32, pw: &[u8], context: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    let start = run(
        t,
        plain(
            cc::SIGN_SEQUENCE_START,
            &[key],
            &cat(&[&tpm2b(b"seq"), &tpm2b(context)]),
        ),
    );
    ok(&start);
    let sequence = seq_handle(&start);
    let (last, head) = parts.split_last().unwrap();
    for part in head {
        ok(&seq_update(t, sequence, b"seq", part));
    }
    let reply = run(
        t,
        with_pw(
            cc::SIGN_SEQUENCE_COMPLETE,
            &[sequence, key],
            &[b"seq", pw],
            &tpm2b(last),
        ),
    );
    if rc(&reply) != 0 {
        // A failed completion keeps the sequence; free its slot.
        ok(&flush(t, sequence));
    }
    reply
}

fn verify_message(
    t: &mut Tpm,
    key: u32,
    context: &[u8],
    signature: &[u8],
    parts: &[&[u8]],
) -> Vec<u8> {
    let start = run(
        t,
        plain(
            cc::VERIFY_SEQUENCE_START,
            &[key],
            &cat(&[&tpm2b(b"vseq"), &tpm2b(b""), &tpm2b(context)]),
        ),
    );
    ok(&start);
    let sequence = seq_handle(&start);
    for part in parts {
        ok(&seq_update(t, sequence, b"vseq", part));
    }
    let reply = run(
        t,
        with_pw(
            cc::VERIFY_SEQUENCE_COMPLETE,
            &[sequence, key],
            &[b"vseq"],
            signature,
        ),
    );
    if rc(&reply) != 0 {
        ok(&flush(t, sequence));
    }
    reply
}

fn sign_digest(t: &mut Tpm, key: u32, pw: &[u8], digest: &[u8]) -> Vec<u8> {
    let params = cat(&[&tpm2b(b""), &tpm2b(digest), &NULL_VALIDATION]);
    run(t, with_pw(cc::SIGN_DIGEST, &[key], &[pw], &params))
}

fn verify_digest(t: &mut Tpm, key: u32, digest: &[u8], signature: &[u8]) -> Vec<u8> {
    let params = cat(&[&tpm2b(b""), &tpm2b(digest), signature]);
    run(t, plain(cc::VERIFY_DIGEST_SIGNATURE, &[key], &params))
}

fn load_external_key(t: &mut Tpm, external_mu: bool) -> (u32, Vec<u8>) {
    parse_loaded(&load_external(t, &external_public(external_mu), NULL))
}

// ------------------------------------------------------------------------------------- tests

#[test]
fn mldsa65_create_primary_public_size_and_name() {
    let (mut t, key) = tpm_key();
    // TPMT_PUBLIC by hand (Part 2 v185, Tables 230 and 234): type 00A1, nameAlg SHA-256,
    // attributes fixedTPM|fixedParent|sensitiveDataOrigin|userWithAuth|sign, an empty authPolicy,
    // parameterSet 2 (ML-DSA-65), allowExternalMu YES, then TPM2B_PUBLIC_KEY_MLDSA of 1952 octets.
    assert_eq!(key.public.len(), 15 + 1952);
    assert_eq!(key.public[..15], hex("00a1000b00040072000000020107a0"));
    // The public key is the one OpenSSL derives from the seed, and the Name is nameAlg || H(TPMT_PUBLIC).
    assert_eq!(sha256_hex(&key.public[15..]), TPM_PK_SHA256);
    assert_eq!(hex_of(&secret_of(&t, key.handle)), TPM_SEED);
    assert_eq!(hex_of(&key.name), TPM_NAME);
    assert_eq!(hex_of(&name_hash(&key.public)), TPM_NAME);
    // ReadPublic returns the same public area and Name.
    let read = read_public(&mut t, key.handle);
    ok(&read);
    let mut r = Reader::new(params(&read, 0));
    assert_eq!(r.tpm2b(limits::MAX_RESPONSE_SIZE).unwrap(), key.public);
    assert_eq!(r.tpm2b(66).unwrap(), key.name);
    // The Qualified Name of a primary is nameAlg || H(hierarchy handle || Name) (Part 1, clause 24.5).
    let qualified = r.tpm2b(66).unwrap().to_vec();
    assert_eq!(
        qualified,
        [
            &alg::SHA256.to_be_bytes()[..],
            &Sha256::digest([&OWNER.to_be_bytes()[..], &key.name].concat())
        ]
        .concat()
    );
    r.finish().unwrap();
    assert_eq!(
        unmarshal_public(&key.public),
        mldsa_template_with_unique(&key.public)
    );
    // nameAlg SHA-384 and SHA-512: the Name is that digest of the TPMT_PUBLIC, computed with the
    // sha2 crate (a SHA-512 Name has 2 + 64 octets).
    ok(&flush(&mut t, key.handle));
    for name_alg in [alg::SHA384, alg::SHA512] {
        let created = create_primary(&mut t, OWNER, b"pw", &mldsa_template(false, true, name_alg));
        let handle = handle_of(&created);
        let mut r = Reader::new(params(&created, 1));
        let public = r.tpm2b(limits::MAX_RESPONSE_SIZE).unwrap().to_vec();
        let digest: Vec<u8> = if name_alg == alg::SHA384 {
            sha2::Sha384::digest(&public).to_vec()
        } else {
            sha2::Sha512::digest(&public).to_vec()
        };
        let name = [&name_alg.to_be_bytes()[..], &digest].concat();
        assert_eq!(name.len(), if name_alg == alg::SHA384 { 50 } else { 66 });
        assert!(contains(
            &created,
            &[&(name.len() as u16).to_be_bytes()[..], &name].concat()
        ));
        assert_eq!(public.len(), 15 + 1952);
        ok(&flush(&mut t, handle));
    }
    // The external key: a Name and a loaded public area of 1952 octets.
    let (external, name) = load_external_key(&mut t, true);
    assert_eq!(hex_of(&name), EXT_NAME);
    assert_eq!(sha256_hex(EXT_PUBLIC).len(), 64);
    ok(&flush(&mut t, external));
    // TestParms: ML-DSA-65 with and without allowExternalMu; 44 and 87 are TPM_RC_PARMS, a
    // YES/NO octet of 2 is TPM_RC_VALUE.
    for encoded in ["00a1000200", "00a1000201"] {
        assert_eq!(
            hex_of(&run(&mut t, plain(0x18A, &[], &hex(encoded)))),
            OK_REPLY,
            "{encoded}"
        );
    }
    for (encoded, error) in [
        ("00a1000100", Rc::PARMS),
        ("00a1000300", Rc::PARMS),
        ("00a1000202", Rc::VALUE),
    ] {
        err(
            &run(&mut t, plain(0x18A, &[], &hex(encoded))),
            error.in_param(1),
        );
    }
}

fn mldsa_template_with_unique(public: &[u8]) -> TpmtPublic {
    let mut template = mldsa_template(false, true, alg::SHA256);
    template.unique = Unique::MlDsa(public[15..].to_vec());
    template
}

#[test]
fn mldsa65_sign_digest_verifies_with_openssl() {
    let (mut t, key) = tpm_key();
    let mu = hex(MU);
    assert_eq!(mu.len(), 64);
    let signed = sign_digest(&mut t, key.handle, b"pw", &mu);
    let signature = sig_octets(&signed);
    assert_eq!(signature.len(), 3309);
    // OpenSSL (pkeyutl -sign -pkeyopt mu:1 -pkeyopt deterministic:1) makes the same 3309 octets,
    // and "pkeyutl -verify" accepts them.
    assert_eq!(sha256_hex(&signature), SIG_SHA256);
    // The signature over mu is the signature over the message: a sequence gives the same octets.
    let by_message = sign_message(&mut t, key.handle, b"pw", b"", &[MESSAGE]);
    assert_eq!(sig_octets(&by_message), signature);
    // VerifyDigestSignature accepts it and returns a TPM_ST_DIGEST_VERIFIED ticket.
    let verified = verify_digest(&mut t, key.handle, &mu, params(&signed, 0));
    ok(&verified);
    assert_eq!(params(&verified, 0)[..2], [0x80, 0x27]);
    // A changed mu, a changed signature and a mu of another size are refused.
    let mut other = mu.clone();
    other[0] ^= 1;
    err(
        &verify_digest(&mut t, key.handle, &other, params(&signed, 0)),
        Rc::SIGNATURE,
    );
    let mut forged = signature.clone();
    forged[1000] ^= 1;
    err(
        &verify_digest(&mut t, key.handle, &mu, &signature_bytes(&forged)),
        Rc::SIGNATURE,
    );
    err(
        &sign_digest(&mut t, key.handle, b"pw", &mu[..63]),
        Rc::SIZE.in_param(2),
    );
    err(
        &verify_digest(&mut t, key.handle, &mu[..63], params(&signed, 0)),
        Rc::SIZE.in_param(2),
    );
    // A wrong password is refused before anything is signed.
    assert_ne!(rc(&sign_digest(&mut t, key.handle, b"bad", &mu)), 0);
}

#[test]
fn mldsa65_verify_signature_accepts_openssl_signature() {
    let mut t = started();
    let (external, name) = load_external_key(&mut t, true);
    assert_eq!(hex_of(&name), EXT_NAME);
    // The message signature made by OpenSSL, with its context.
    let signature = signature_bytes(EXT_SIG_MSG);
    let verified = verify_message(&mut t, external, b"ctx-1", &signature, &[EXT_MESSAGE]);
    ok(&verified);
    assert_eq!(
        params(&verified, 0)[..2],
        [0x80, 0x26],
        "TPM_ST_MESSAGE_VERIFIED"
    );
    // The message may arrive in pieces.
    ok(&verify_message(
        &mut t,
        external,
        b"ctx-1",
        &signature,
        &[&EXT_MESSAGE[..5], &EXT_MESSAGE[5..]],
    ));
    // Another context, another message and the signature over something else are refused.
    err(
        &verify_message(&mut t, external, b"ctx-2", &signature, &[EXT_MESSAGE]),
        Rc::SIGNATURE,
    );
    err(
        &verify_message(&mut t, external, b"", &signature, &[EXT_MESSAGE]),
        Rc::SIGNATURE,
    );
    err(
        &verify_message(
            &mut t,
            external,
            b"ctx-1",
            &signature,
            &[b"another message"],
        ),
        Rc::SIGNATURE,
    );
    err(
        &verify_message(
            &mut t,
            external,
            b"",
            &signature_bytes(EXT_SIG_POLICY),
            &[EXT_MESSAGE],
        ),
        Rc::SIGNATURE,
    );
    // The mu of (message, context) with the signatures that OpenSSL made over the message and
    // over the mu itself: both verify as signatures over that mu.
    let mu = hex(EXT_MU);
    for octets in [EXT_SIG_MU, EXT_SIG_MSG] {
        let verified = verify_digest(&mut t, external, &mu, &signature_bytes(octets));
        ok(&verified);
        assert_eq!(params(&verified, 0)[..2], [0x80, 0x27]);
    }
    err(
        &verify_digest(&mut t, external, &mu, &signature_bytes(EXT_SIG_POLICY)),
        Rc::SIGNATURE,
    );
    // TPM2_VerifySignature is the digest command of the deprecated schemes: not for ML-DSA.
    err(
        &verify(&mut t, external, &mu, &signature_bytes(EXT_SIG_MU)),
        Rc::SCHEME.in_param(2),
    );
}

#[test]
fn mldsa65_sign_sequence_matches_one_shot() {
    let (mut t, key) = tpm_key();
    // One call, and the same message in two and in three parts.
    let one_shot = sig_octets(&sign_message(&mut t, key.handle, b"pw", b"", &[MESSAGE]));
    assert_eq!(sha256_hex(&one_shot), SIG_SHA256);
    for parts in [
        vec![&MESSAGE[..10], &MESSAGE[10..]],
        vec![&MESSAGE[..3], &MESSAGE[3..4], &MESSAGE[4..]],
        vec![&b""[..], MESSAGE],
    ] {
        let signed = sign_message(&mut t, key.handle, b"pw", b"", &parts);
        assert_eq!(sig_octets(&signed), one_shot);
    }
    // The signature verifies with the verify sequence and with the digest command (the mu of the pair).
    let signature = signature_bytes(&one_shot);
    ok(&verify_message(
        &mut t,
        key.handle,
        b"",
        &signature,
        &[MESSAGE],
    ));
    ok(&verify_digest(&mut t, key.handle, &hex(MU), &signature));
    // The empty message and a context.
    let empty = sign_message(&mut t, key.handle, b"pw", b"", &[b""]);
    assert_eq!(sha256_hex(&sig_octets(&empty)), SIG_EMPTY_SHA256);
    let with_context = sign_message(&mut t, key.handle, b"pw", b"ctx-1", &[MESSAGE]);
    assert_eq!(sha256_hex(&sig_octets(&with_context)), SIG_CTX_SHA256);
    ok(&verify_message(
        &mut t,
        key.handle,
        b"ctx-1",
        &signature_bytes(&sig_octets(&with_context)),
        &[MESSAGE],
    ));
    // The longest context is 255 octets; a longer one does not unmarshal.
    let longest = vec![0x61; 255];
    ok(&sign_message(
        &mut t,
        key.handle,
        b"pw",
        &longest,
        &[MESSAGE],
    ));
    err(
        &run(
            &mut t,
            plain(
                cc::SIGN_SEQUENCE_START,
                &[key.handle],
                &cat(&[&tpm2b(b"seq"), &tpm2b(&[0x61; 256])]),
            ),
        ),
        Rc::SIZE.in_param(2),
    );
    // A message of 2548 octets in updates of 1024: the signature OpenSSL makes over it.
    let long: Vec<u8> = (0..2548).map(|i| ((i * 7 + 3) % 251) as u8).collect();
    assert_eq!(sha256_hex(&long), LONG_SHA256);
    let signed = sign_message(
        &mut t,
        key.handle,
        b"pw",
        b"",
        &[&long[..1024], &long[1024..2048], &long[2048..]],
    );
    assert_eq!(sha256_hex(&sig_octets(&signed)), SIG_LONG_SHA256);
    // A message past the buffer of the sequence is TPM_RC_SIZE and leaves the sequence usable.
    let start = run(
        &mut t,
        plain(
            cc::SIGN_SEQUENCE_START,
            &[key.handle],
            &cat(&[&tpm2b(b"seq"), &tpm2b(b"")]),
        ),
    );
    let sequence = seq_handle(&start);
    for _ in 0..8 {
        ok(&seq_update(&mut t, sequence, b"seq", &[1; 1024]));
    }
    err(
        &seq_update(&mut t, sequence, b"seq", &[1; 1]),
        Rc::SIZE.in_param(1),
    );
    err(
        &run(
            &mut t,
            with_pw(
                cc::SIGN_SEQUENCE_COMPLETE,
                &[sequence, key.handle],
                &[b"seq", b"pw"],
                &tpm2b(&[1; 1]),
            ),
        ),
        Rc::SIZE.in_param(1),
    );
    ok(&run(
        &mut t,
        with_pw(
            cc::SIGN_SEQUENCE_COMPLETE,
            &[sequence, key.handle],
            &[b"seq", b"pw"],
            &tpm2b(b""),
        ),
    ));
    // A sequence of another key is refused.
    let other = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"pw",
        &mldsa_template(false, false, alg::SHA256),
    ));
    let start = run(
        &mut t,
        plain(
            cc::SIGN_SEQUENCE_START,
            &[key.handle],
            &cat(&[&tpm2b(b"seq"), &tpm2b(b"")]),
        ),
    );
    err(
        &run(
            &mut t,
            with_pw(
                cc::SIGN_SEQUENCE_COMPLETE,
                &[seq_handle(&start), other.handle],
                &[b"seq", b"pw"],
                &tpm2b(MESSAGE),
            ),
        ),
        Rc::SIGN_CONTEXT_KEY.in_handle(2),
    );
}

#[test]
fn mldsa65_restricted_key_refuses_a_generated_prefix_and_signs_attestations() {
    let mut t = started();
    let restricted = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"pw",
        &mldsa_template(true, false, alg::SHA256),
    ));
    // A message that begins with TPM_GENERATED_VALUE is not signed by a restricted key.
    err(
        &sign_message(
            &mut t,
            restricted.handle,
            b"pw",
            b"",
            &[&hex("ff54434780170000")],
        ),
        Rc::VALUE.in_param(1),
    );
    ok(&sign_message(
        &mut t,
        restricted.handle,
        b"pw",
        b"",
        &[b"plain message"],
    ));
    // It cannot sign a digest: there is no ticket that proves a mu (Part 3, clause 20.7).
    err(
        &sign_digest(&mut t, restricted.handle, b"pw", &[7; 64]),
        Rc::SCHEME.in_handle(1),
    );
}

#[test]
fn mldsa65_quote_or_certify_verifies() {
    let mut t = started();
    let signer = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"pw",
        &mldsa_template(true, false, alg::SHA256),
    ));
    let certified = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"cpw",
        &mldsa_template(false, true, alg::SHA256),
    ));
    let null = u16b(alg::NULL);
    let reply = certify(
        &mut t,
        certified.handle,
        signer.handle,
        [b"cpw", b"pw"],
        b"fresh",
        &null,
    );
    ok(&reply);
    let (attest, signature) = parse_attestation(&reply);
    // TPMS_ATTEST: magic, TPM_ST_ATTEST_CERTIFY, qualifiedSigner, extraData, clockInfo, firmware,
    // then the Name and the Qualified Name of the certified object.
    assert_eq!(attest[..6], hex("ff5443478017"));
    assert!(contains(&attest, b"fresh"));
    assert!(contains(&attest, &certified.name));
    // TPMT_SIGNATURE: TPM_ALG_MLDSA, a TPM2B of 3309 octets, no hash.
    assert_eq!(signature[..4], [0x00, 0xA1, 0x0C, 0xED]);
    assert_eq!(signature.len(), 4 + 3309);
    // The signature is over the TPMS_ATTEST itself (not its digest) with an empty context.
    // An ML-DSA key has no scheme, so inScheme can only be NULL; a signature scheme of another
    // family is TPM_RC_SCHEME, and so is Quote, whose PCR digest needs the hash of the scheme.
    let ecdsa = cat(&[&u16b(alg::ECDSA), &u16b(alg::SHA256)]);
    err(
        &certify(
            &mut t,
            certified.handle,
            signer.handle,
            [b"cpw", b"pw"],
            b"fresh",
            &ecdsa,
        ),
        Rc::SCHEME.in_param(2),
    );
    for scheme in [&null, &ecdsa] {
        err(
            &quote(&mut t, signer.handle, b"pw", b"fresh", scheme, &NO_PCR),
            Rc::SCHEME.in_param(2),
        );
    }
    // Only two ML-DSA objects stay loaded: the certified one makes room for the verification key.
    ok(&flush(&mut t, certified.handle));
    let signer_public = &signer.public[signer.public.len() - 1952..];
    let (external, _) = parse_loaded(&load_external(
        &mut t,
        &TpmtPublic {
            unique: Unique::MlDsa(signer_public.to_vec()),
            attributes: oa::SIGN | oa::RESTRICTED,
            ..mldsa_template(true, false, alg::SHA256)
        },
        NULL,
    ));
    ok(&verify_message(
        &mut t,
        external,
        b"",
        &signature,
        &[&attest],
    ));
    let mut altered = attest.clone();
    *altered.last_mut().unwrap() ^= 1;
    err(
        &verify_message(&mut t, external, b"", &signature, &[&altered]),
        Rc::SIGNATURE,
    );
    // Pinned: the attestation of this TPM is deterministic; both values were checked outside
    // (openssl pkeyutl -verify of the signature over the attestation).
    assert_eq!(sha256_hex(&attest), ATTEST_SHA256);
    assert_eq!(sha256_hex(&signature), ATTEST_SIGNATURE_SHA256);
}

#[test]
fn mldsa65_policy_signed_accepts_valid_signature() {
    let mut t = started();
    let (external, name) = load_external_key(&mut t, true);
    // PolicyUpdate(TPM_CC_PolicySigned, authObject->Name, policyRef) (Part 3 v185, clause 23.2.3):
    // H(H(zeros || 0x160 || Name) || policyRef), with the Name pinned above.
    let first = Sha256::digest([&[0u8; 32][..], &0x160u32.to_be_bytes(), &name].concat());
    let expected = Sha256::digest([&first[..], b"ref"].concat()).to_vec();
    // toBeSigned = nonceTPM || expiration || cpHashA || policyRef = 00000000 || "ref" (Part 3 v185,
    // Equation 3), signed by OpenSSL without a hash.
    let request = |signature: &[u8], reference: &[u8]| {
        cat(&[
            &tpm2b(b""),
            &tpm2b(b""),
            &tpm2b(reference),
            &u32b(0),
            &signature_bytes(signature),
        ])
    };
    let session = start_session(&mut t, se::POLICY, NULL, b"", &[0x44; 16]);
    ok(&run(
        &mut t,
        plain(
            cc::POLICY_SIGNED,
            &[external, session.handle],
            &request(EXT_SIG_POLICY, b"ref"),
        ),
    ));
    let digest = run(&mut t, plain(cc::POLICY_GET_DIGEST, &[session.handle], &[]));
    ok(&digest);
    assert_eq!(params(&digest, 0), tpm2b(&expected));
    // A signature over another message, another reference and the signature of another key are refused.
    for (signature, reference) in [(EXT_SIG_MSG, &b"ref"[..]), (EXT_SIG_POLICY, b"other")] {
        let fresh = start_session(&mut t, se::POLICY, NULL, b"", &[0x44; 16]);
        err(
            &run(
                &mut t,
                plain(
                    cc::POLICY_SIGNED,
                    &[external, fresh.handle],
                    &request(signature, reference),
                ),
            ),
            Rc::POLICY_FAIL,
        );
        ok(&flush(&mut t, fresh.handle));
    }
    let mut bad = EXT_SIG_POLICY.to_vec();
    bad[3000] ^= 0x10;
    let fresh = start_session(&mut t, se::POLICY, NULL, b"", &[0x44; 16]);
    err(
        &run(
            &mut t,
            plain(
                cc::POLICY_SIGNED,
                &[external, fresh.handle],
                &request(&bad, b"ref"),
            ),
        ),
        Rc::POLICY_FAIL,
    );
    // A signature labelled with a hash does not fit an ML-DSA key.
    let labelled = cat(&[
        &tpm2b(b""),
        &tpm2b(b""),
        &tpm2b(b"ref"),
        &u32b(0),
        &cat(&[
            &u16b(alg::ECDSA),
            &u16b(alg::SHA256),
            &tpm2b(&[1; 32]),
            &tpm2b(&[2; 32]),
        ]),
    ]);
    err(
        &run(
            &mut t,
            plain(cc::POLICY_SIGNED, &[external, fresh.handle], &labelled),
        ),
        Rc::POLICY_FAIL,
    );
    // A trial session does not check the signature (Part 3, clause 23.3.1).
    let trial = start_session(&mut t, se::TRIAL, NULL, b"", &[0x44; 16]);
    ok(&run(
        &mut t,
        plain(
            cc::POLICY_SIGNED,
            &[external, trial.handle],
            &request(&bad, b"ref"),
        ),
    ));
    let trial_digest = run(&mut t, plain(cc::POLICY_GET_DIGEST, &[trial.handle], &[]));
    assert_eq!(params(&trial_digest, 0), tpm2b(&expected));
    // A signature made by the TPM key over the same toBeSigned message is accepted as well.
    let (mut t, key) = tpm_key();
    let (authority, authority_name) =
        parse_loaded(&load_external(&mut t, &unmarshal_public(&key.public), NULL));
    let signed = sig_octets(&sign_message(
        &mut t,
        key.handle,
        b"pw",
        b"",
        &[&hex("00000000726566")],
    ));
    assert_eq!(sha256_hex(&signed), SIG_POLICY_SHA256);
    let session = start_session(&mut t, se::POLICY, NULL, b"", &[0x44; 16]);
    ok(&run(
        &mut t,
        plain(
            cc::POLICY_SIGNED,
            &[authority, session.handle],
            &request(&signed, b"ref"),
        ),
    ));
    assert_eq!(authority_name, key.name);
}

#[test]
fn mldsa65_two_loaded_objects_limit_and_third_refused() {
    let mut t = started();
    // The limit is on loaded transient ML-DSA objects (MAX_LOADED_MLDSA = 2).
    let first = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"pw",
        &mldsa_template(false, true, alg::SHA256),
    ));
    let second = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"pw",
        &mldsa_template(true, false, alg::SHA256),
    ));
    let third_template = mldsa_template(false, false, alg::SHA256);
    err(
        &create_primary(&mut t, OWNER, b"pw", &third_template),
        Rc::OBJECT_MEMORY,
    );
    // LoadExternal of a third ML-DSA key is refused too, and ContextLoad of a saved one.
    err(
        &load_external(&mut t, &external_public(true), NULL),
        Rc::OBJECT_MEMORY,
    );
    let saved = run(&mut t, plain(cc::CONTEXT_SAVE, &[first.handle], &[]));
    ok(&saved);
    // Objects of the other types still load: the ML-DSA limit is not the slot limit.
    let ecc_key = parse_primary(&create_primary(&mut t, OWNER, b"pw", &ecc_sign()));
    let storage = parse_primary(&create_primary(&mut t, OWNER, b"pw", &ecc_storage()));
    // The capability lists the loaded objects and the free slots as before.
    let handles = |t: &mut Tpm| {
        let (_, _, body, count) = cap_list(&get_cap(
            t,
            cap::HANDLES,
            0x8000_0000 + MAX_SEQUENCES as u32,
            100,
        ));
        (count, body)
    };
    assert_eq!(handles(&mut t).0, 4);
    // Flushing one ML-DSA object makes room for exactly one: the saved one is the third again.
    ok(&flush(&mut t, first.handle));
    let third = parse_primary(&create_primary(&mut t, OWNER, b"pw", &third_template));
    err(
        &run(&mut t, plain(cc::CONTEXT_LOAD, &[], params(&saved, 0))),
        Rc::OBJECT_MEMORY,
    );
    ok(&flush(&mut t, third.handle));
    let reloaded = handle_of(&run(
        &mut t,
        plain(cc::CONTEXT_LOAD, &[], params(&saved, 0)),
    ));
    let signed = sign_message(&mut t, reloaded, b"pw", b"", &[MESSAGE]);
    assert_eq!(sha256_hex(&sig_octets(&signed)), SIG_SHA256);
    assert_eq!(handles(&mut t).0, 4);
    // A persistent ML-DSA object takes no transient slot: evict the restricted key, flush it, and
    // two other ML-DSA objects load beside it.
    ok(&evict(&mut t, OWNER, b"", second.handle, 0x8100_0001));
    ok(&flush(&mut t, second.handle));
    ok(&flush(&mut t, reloaded));
    let a = parse_primary(&create_primary(&mut t, OWNER, b"pw", &third_template));
    let b = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"pw",
        &mldsa_template(false, true, alg::SHA384),
    ));
    err(
        &create_primary(
            &mut t,
            OWNER,
            b"pw",
            &mldsa_template(false, true, alg::SHA512),
        ),
        Rc::OBJECT_MEMORY,
    );
    ok(&sign_message(
        &mut t,
        0x8100_0001,
        b"pw",
        b"",
        &[b"persistent"],
    ));
    ok(&sign_message(&mut t, a.handle, b"pw", b"", &[b"transient"]));
    ok(&flush(&mut t, a.handle));
    ok(&flush(&mut t, b.handle));
    // Other keys were never limited.
    ok(&flush(&mut t, ecc_key.handle));
    ok(&flush(&mut t, storage.handle));
}

#[test]
fn mldsa65_create_and_load_child_under_a_storage_parent() {
    let mut t = started();
    let parent = parse_primary(&create_primary(&mut t, OWNER, b"pw", &ecc_storage()));
    let child = parse_child(&create_child(
        &mut t,
        parent.handle,
        b"pw",
        b"cpw",
        b"",
        &mldsa_template(false, true, alg::SHA256),
    ));
    // outPublic: the public area with the 1952-octet key; outPrivate: the sensitive area with the
    // 32-octet seed, protected by the parent (a seed, an integrity value and an AES-CFB IV).
    assert_eq!(child.public.len(), 15 + 1952);
    assert!(child.private.len() < 200, "{}", child.private.len());
    let (loaded, name) = parse_loaded(&load(
        &mut t,
        parent.handle,
        b"pw",
        &child.private,
        &child.public,
    ));
    assert_eq!(name, name_hash(&child.public));
    let signed = sign_message(&mut t, loaded, b"cpw", b"", &[MESSAGE]);
    let signature = signature_bytes(&sig_octets(&signed));
    ok(&verify_message(&mut t, loaded, b"", &signature, &[MESSAGE]));
    ok(&verify_digest(
        &mut t,
        loaded,
        &mu_of(&child.public, MESSAGE),
        &signature,
    ));
    // The private area does not carry the signature key in clear and does not load with another public key.
    let seed = secret_of(&t, loaded);
    assert_eq!(seed.len(), 32);
    assert!(!contains(&child.private, &seed));
    let other = parse_child(&create_child(
        &mut t,
        parent.handle,
        b"pw",
        b"cpw",
        b"",
        &mldsa_template(false, true, alg::SHA256),
    ));
    assert_ne!(other.public, child.public);
    // (the private area is bound to its public area through its integrity value: TPM_RC_INTEGRITY, as for an RSA or ECC child)
    err(
        &load(&mut t, parent.handle, b"pw", &child.private, &other.public),
        Rc::INTEGRITY,
    );
}

/// mu of the key in `public` (a TPMT_PUBLIC) and `message` with an empty context (FIPS 204,
/// Algorithm 7, line 6), from the `ml-dsa` crate; the pinned mu values of the other tests were
/// computed with Python.
fn mu_of(public: &[u8], message: &[u8]) -> Vec<u8> {
    use ml_dsa::signature::digest::Update;
    let encoded =
        ml_dsa::EncodedVerifyingKey::<ml_dsa::MlDsa65>::try_from(&public[public.len() - 1952..])
            .unwrap();
    let key = ml_dsa::VerifyingKey::<ml_dsa::MlDsa65>::decode(&encoded);
    let mu = key
        .compute_mu(
            |shake| {
                shake.update(message);
                Ok(())
            },
            &[],
        )
        .unwrap();
    mu.as_slice().to_vec()
}

#[test]
fn mldsa65_snapshot_restore_signs_identically() {
    let (mut t, key) = tpm_key();
    let before = sig_octets(&sign_message(&mut t, key.handle, b"pw", b"", &[MESSAGE]));
    assert_eq!(sha256_hex(&before), SIG_SHA256);
    // ContextSave and ContextLoad: the loaded object signs the same octets.
    let saved = run(&mut t, plain(cc::CONTEXT_SAVE, &[key.handle], &[]));
    ok(&saved);
    ok(&flush(&mut t, key.handle));
    let loaded = handle_of(&run(
        &mut t,
        plain(cc::CONTEXT_LOAD, &[], params(&saved, 0)),
    ));
    assert_eq!(
        sig_octets(&sign_message(&mut t, loaded, b"pw", b"", &[MESSAGE])),
        before
    );
    assert_eq!(secret_of(&t, loaded), hex(TPM_SEED));
    // A persistent object survives Shutdown, the snapshot and Startup with its seed.
    ok(&evict(&mut t, OWNER, b"", loaded, 0x8100_0001));
    shutdown(&mut t, 1);
    let snapshot = t.persistent_snapshot();
    let mut restored = Tpm::restore(&snapshot, &[9; 48]).unwrap();
    startup(&mut restored, 0);
    let read = read_public(&mut restored, 0x8100_0001);
    ok(&read);
    let mut r = Reader::new(params(&read, 0));
    assert_eq!(r.tpm2b(limits::MAX_RESPONSE_SIZE).unwrap(), key.public);
    assert_eq!(r.tpm2b(66).unwrap(), key.name);
    let after = sig_octets(&sign_message(
        &mut restored,
        0x8100_0001,
        b"pw",
        b"",
        &[MESSAGE],
    ));
    assert_eq!(after, before);
    assert_eq!(secret_of_persistent(&restored, 0x8100_0001), hex(TPM_SEED));
    // The restored object verifies the signature made before the restart, and OpenSSL's.
    ok(&verify_message(
        &mut restored,
        0x8100_0001,
        b"",
        &signature_bytes(&before),
        &[MESSAGE],
    ));
    // A second generation: the snapshot of the restored TPM restores the same key again.
    shutdown(&mut restored, 1);
    let mut again = Tpm::restore(&restored.persistent_snapshot(), &[7; 48]).unwrap();
    startup(&mut again, 0);
    let after = sig_octets(&sign_message(
        &mut again,
        0x8100_0001,
        b"pw",
        b"",
        &[MESSAGE],
    ));
    assert_eq!(after, before);
}

fn secret_of_persistent(t: &Tpm, handle: u32) -> Vec<u8> {
    use crate::object::Secret;
    let object = t.persistent.objects.get(handle).unwrap();
    match &object.sensitive.as_ref().unwrap().secret {
        Secret::MlDsa(key) => crate::mldsa::private_seed(key).to_vec(),
        _ => panic!("not an ML-DSA object"),
    }
}

#[test]
fn mldsa65_refuses_wrong_parameter_set_and_bad_length() {
    let mut t = started();
    let raw_creation = |public: &[u8], data: &[u8]| {
        cat(&[
            &tpm2b(&cat(&[&tpm2b(b""), &tpm2b(data)])),
            &tpm2b(public),
            &tpm2b(b""),
            &NO_PCR,
        ])
    };
    let good = mldsa_template(false, true, alg::SHA256).to_bytes();
    assert_eq!(good, hex("00a1000b0004007200000002010000"));
    // ML-DSA-44 (1) and ML-DSA-87 (3), and parameter set 0 and 4: TPM_RC_PARMS; a TPMI_YES_NO of 2 is TPM_RC_VALUE.
    for (offset, value, error) in [
        (11, 1u8, Rc::PARMS),
        (11, 3, Rc::PARMS),
        (11, 0, Rc::PARMS),
        (11, 4, Rc::PARMS),
        (12, 2, Rc::VALUE),
    ] {
        let mut public = good.clone();
        public[offset] = value;
        err(
            &create_primary_raw(&mut t, OWNER, b"", &raw_creation(&public, b"")),
            error.in_param(2),
        );
    }
    // Attributes: sign only (not decrypt, not neither), not x509sign, allowExternalMu not with restricted.
    for attributes in [
        base() | oa::SIGN | oa::DECRYPT,
        base(),
        base() | oa::SIGN | oa::X509_SIGN,
        base() | oa::SIGN | oa::RESTRICTED,
    ] {
        let mut template = mldsa_template(false, true, alg::SHA256);
        template.attributes = attributes;
        err(
            &create_primary(&mut t, OWNER, b"pw", &template),
            Rc::ATTRIBUTES.in_param(2),
        );
    }
    // A caller's key material is refused: the TPM generates the seed (TPM_RC_VALUE, parameter 1).
    err(
        &create_primary_raw(
            &mut t,
            OWNER,
            b"",
            &creation_params(
                b"",
                b"seed of the caller",
                &mldsa_template(false, true, alg::SHA256),
                b"",
                &NO_PCR,
            ),
        ),
        Rc::VALUE.in_param(1),
    );
    // A public key of the wrong length: TPM_RC_KEY for LoadExternal.
    let mut short = external_public(true);
    short.unique = Unique::MlDsa(EXT_PUBLIC[..1951].to_vec());
    err(&load_external(&mut t, &short, NULL), Rc::KEY.in_param(2));
    let (external, _) = load_external_key(&mut t, true);
    // Signatures of the wrong length: 3308 octets verify as TPM_RC_SIGNATURE, 3310 do not unmarshal.
    let shorter = signature_bytes(&EXT_SIG_MSG[..3308]);
    let longer = signature_bytes(&[EXT_SIG_MSG, &[0][..]].concat());
    err(
        &verify_message(&mut t, external, b"ctx-1", &shorter, &[EXT_MESSAGE]),
        Rc::SIGNATURE,
    );
    err(
        &verify_message(&mut t, external, b"ctx-1", &longer, &[EXT_MESSAGE]),
        Rc::SIZE.in_param(1),
    );
    err(
        &verify_digest(&mut t, external, &hex(EXT_MU), &shorter),
        Rc::SIGNATURE,
    );
    err(
        &verify_digest(&mut t, external, &hex(EXT_MU), &longer),
        Rc::SIZE.in_param(3),
    );
    err(
        &verify_digest(&mut t, external, &hex(EXT_MU), &signature_bytes(&[])),
        Rc::SIGNATURE,
    );
    // A signature labelled with a hash (or of another family) does not fit the key: TPM_RC_SCHEME.
    let ecdsa = cat(&[
        &u16b(alg::ECDSA),
        &u16b(alg::SHA256),
        &tpm2b(&[1; 32]),
        &tpm2b(&[2; 32]),
    ]);
    let rsassa = cat(&[&u16b(alg::RSASSA), &u16b(alg::SHA256), &tpm2b(&[1; 256])]);
    let hash_mldsa = cat(&[&u16b(0x00A2), &u16b(alg::SHA256), &tpm2b(&[1; 32])]);
    for foreign in [&ecdsa, &rsassa, &hash_mldsa] {
        err(
            &verify_digest(&mut t, external, &hex(EXT_MU), foreign),
            Rc::SCHEME.in_param(3),
        );
        err(
            &verify_message(&mut t, external, b"ctx-1", foreign, &[EXT_MESSAGE]),
            Rc::SCHEME.in_param(1),
        );
    }
    // The other way round: an ML-DSA signature for an ECC key (TPM2_VerifySignature, digest commands).
    let ecc_key = parse_primary(&create_primary(&mut t, OWNER, b"pw", &ecc_sign()));
    err(
        &verify(
            &mut t,
            ecc_key.handle,
            &[0; 32],
            &signature_bytes(EXT_SIG_MSG),
        ),
        Rc::SCHEME.in_param(2),
    );
    err(
        &verify_digest(
            &mut t,
            ecc_key.handle,
            &[0; 32],
            &signature_bytes(EXT_SIG_MSG),
        ),
        Rc::SCHEME.in_param(3),
    );
    // Key usage: TPM2_Sign (deprecated, digest schemes) does not take ML-DSA keys, SignDigest needs
    // allowExternalMu, a context is only for sequences, and an external key has no private part.
    let (mut t, key) = tpm_key();
    let digest = [0u8; 32];
    for scheme in [
        u16b(alg::NULL),
        cat(&[&u16b(alg::ECDSA), &u16b(alg::SHA256)]),
    ] {
        err(
            &sign(&mut t, key.handle, b"pw", &digest, &scheme),
            Rc::SCHEME.in_handle(1),
        );
    }
    let no_mu = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"pw",
        &mldsa_template(false, false, alg::SHA256),
    ));
    err(
        &sign_digest(&mut t, no_mu.handle, b"pw", &[7; 64]),
        Rc::SCHEME.in_handle(1),
    );
    err(
        &verify_digest(
            &mut t,
            no_mu.handle,
            &[7; 64],
            &signature_bytes(EXT_SIG_MSG),
        ),
        Rc::SCHEME.in_handle(1),
    );
    // With a context: TPM_RC_SIZE (the mu includes it).
    err(
        &run(
            &mut t,
            with_pw(
                cc::SIGN_DIGEST,
                &[key.handle],
                &[b"pw"],
                &cat(&[&tpm2b(b"ctx"), &tpm2b(&hex(MU)), &NULL_VALIDATION]),
            ),
        ),
        Rc::SIZE.in_param(1),
    );
    ok(&flush(&mut t, key.handle));
    ok(&flush(&mut t, no_mu.handle));
    let (external, _) = load_external_key(&mut t, true);
    err(
        &sign_digest(&mut t, external, b"", &hex(EXT_MU)),
        Rc::AUTH_UNAVAILABLE.in_handle(1),
    );
}

#[test]
fn crb_frame_carries_mldsa_signature_of_3309_octets() {
    // The CRB window holds 3968 octets (VTPM.md); frames up to MAX_COMMAND_SIZE and
    // MAX_RESPONSE_SIZE (8192) cross it in chunks. NV_Certify of 512 octets signed by an ML-DSA-65 key
    // answers with a TPM2B_ATTEST and a TPMT_SIGNATURE of 3313 octets: more than one window.
    let (mut t, key) = tpm_key();
    let index = 0x0100_0001;
    ok(&define(
        &mut t,
        OWNER,
        index,
        nv_attr::OWNERWRITE | nv_attr::OWNERREAD,
        512,
    ));
    let data: Vec<u8> = (0..512).map(|i| (i % 251) as u8).collect();
    ok(&nv_write(&mut t, OWNER, index, b"", &data, 0));
    let command = with_pw(
        cc::NV_CERTIFY,
        &[key.handle, OWNER, index],
        &[b"pw", b""],
        &cat(&[&tpm2b(&[0x5a; 64]), &u16b(alg::NULL), &u16b(512), &u16b(0)]),
    );
    // The digest of the whole index (size and offset 0) needs the hash of the signing scheme, which
    // an ML-DSA key does not have: TPM_RC_HASH.
    err(
        &run(
            &mut t,
            with_pw(
                cc::NV_CERTIFY,
                &[key.handle, OWNER, index],
                &[b"pw", b""],
                &cat(&[&tpm2b(b"n"), &u16b(alg::NULL), &u16b(0), &u16b(0)]),
            ),
        ),
        Rc::HASH.in_param(2),
    );
    let reply = t.execute(0, &command, 0);
    ok(&reply);
    assert!(reply.len() > 3968, "{}", reply.len());
    assert!(reply.len() <= limits::MAX_RESPONSE_SIZE, "{}", reply.len());
    // The response size field covers the whole frame.
    assert_eq!(
        u32::from_be_bytes(reply[2..6].try_into().unwrap()) as usize,
        reply.len()
    );
    let mut r = Reader::new(params(&reply, 0));
    let attest = r.tpm2b(limits::MAX_RESPONSE_SIZE).unwrap().to_vec();
    assert!(contains(&attest, &data));
    let signature = r.bytes(r.remaining()).unwrap().to_vec();
    assert_eq!(signature.len(), 2 + 2 + 3309);
    assert_eq!(signature[..4], [0x00, 0xA1, 0x0C, 0xED]);
    // The signature in a command frame: VerifySequenceComplete with the 3313 octets.
    let (external, _) = load_external_key(&mut t, true);
    let start = cat(&[&tpm2b(b"vseq"), &tpm2b(b""), &tpm2b(b"ctx-1")]);
    let started_sequence = run(
        &mut t,
        plain(cc::VERIFY_SEQUENCE_START, &[external], &start),
    );
    let sequence = seq_handle(&started_sequence);
    ok(&seq_update(&mut t, sequence, b"vseq", EXT_MESSAGE));
    let verify_command = with_pw(
        cc::VERIFY_SEQUENCE_COMPLETE,
        &[sequence, external],
        &[b"vseq"],
        &signature_bytes(EXT_SIG_MSG),
    );
    assert!(verify_command.len() > 3313 && verify_command.len() <= limits::MAX_COMMAND_SIZE);
    ok(&t.execute(0, &verify_command, 0));
    // The signature of the attestation verifies with the key that made it.
    ok(&verify_message(
        &mut t,
        key.handle,
        b"",
        &signature,
        &[&attest],
    ));
    // The advertised limits cover these frames.
    let (_, _, body, _) = cap_list(&get_cap(
        &mut t,
        cap::TPM_PROPERTIES,
        pt::MAX_RESPONSE_SIZE,
        1,
    ));
    assert_eq!(body, [u32b(pt::MAX_RESPONSE_SIZE), u32b(8192)].concat());
}
