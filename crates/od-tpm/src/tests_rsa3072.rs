//! RSA-3072, RSASSA-PSS and RSA-OAEP with SHA-384/SHA-512 (slice 2a of hash and key agility).
//!
//! Where a value comes from outside this implementation the test says so. `openssl` 4.1.0 made the
//! signatures and ciphertexts below that are labelled "openssl" (`dgst -sign`, `pkeyutl -encrypt`,
//! `genpkey` for the fixed 3072-bit key of the self-test), and it checked every value labelled
//! "pinned" (a TPM output taken once and verified or decrypted by `openssl` outside this crate with
//! the public key, or with the private key of the deterministic primary, rebuilt as a PEM by a Python
//! script from the primes the TPM held): `dgst -verify` with `rsa_padding_mode:pss` and
//! `rsa_pss_saltlen:digest` for the signatures, `pkeyutl -decrypt` with `rsa_oaep_md`, `rsa_mgf1_md`
//! and `rsa_oaep_label` for the ciphertexts. The TPM is deterministic (fixed entropy, DRBG), so a
//! pinned value is reproduced byte for byte; a changed value prints `FIXTURE <name> <hex>` on stderr
//! before the assertion fails, to be checked with `openssl` again and then re-pinned. The SHA-256,
//! RSA-2048 flow is pinned to the replies the code produced before this slice (commit d9d832c).

use super::tests_b::{shutdown, startup};
use super::tests_c2::{parse_attestation, quote, sel04};
use super::tests_obj::{
    NULL, OWNER, base, create_child, create_primary, create_primary_raw, evict, flush, load,
    load_external, parse_child, parse_loaded, parse_primary, read_public, rsa, rsa_sig_scheme,
    rsa_sign, rsa_storage, secret_of, sign, unmarshal_public, verify,
};
use super::*;
use crate::codes::{object_attr as oa, su};
use crate::crypto::{HashAlg, hash};
use crate::marshal::{PublicParams, Scheme, SymDef, TpmtPublic, Unique, Writer};
use crate::object::MAX_PRIVATE;
use crate::self_test::{
    EXT_RSA_N, RSA3072_DIGEST_256, RSA3072_DIGEST_384, RSA3072_MESSAGE, RSA3072_N, RSA3072_P,
    RSA3072_PKCS1_SHA256, RSA3072_PSS_SHA256, RSA3072_PSS_SHA384, RSA3072_Q, hex as hex_array,
};

fn hex_of(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn digest_of(hash_alg: HashAlg, data: &[u8]) -> Vec<u8> {
    hash(hash_alg, &[data]).to_vec()
}

/// Compares a TPM output with its pinned value; a mismatch prints the new value first.
fn pinned(name: &str, actual: &[u8], expected: &str) {
    let actual = hex_of(actual);
    if actual != expected {
        eprintln!("FIXTURE {name} {actual}");
    }
    assert_eq!(actual, expected, "{name}");
}

const MESSAGE: &[u8] = b"Open Desktop RSA slice 2a";
const RSASSA_256: Scheme = Scheme {
    alg: alg::RSASSA,
    hash: alg::SHA256,
};

fn scheme(scheme_alg: u16, hash_alg: HashAlg) -> Scheme {
    Scheme {
        alg: scheme_alg,
        hash: hash_alg.alg(),
    }
}

fn scheme_bytes(s: Scheme) -> Vec<u8> {
    cat(&[&u16b(s.alg), &u16b(s.hash)])
}

/// An RSA template with `bits` key bits.
fn template(bits: u16, attributes: u32, symmetric: SymDef, scheme: Scheme) -> TpmtPublic {
    let mut public = rsa(attributes, symmetric, scheme);
    if let PublicParams::Rsa { key_bits, .. } = &mut public.params {
        *key_bits = bits;
    }
    public
}

fn signing(bits: u16, scheme: Scheme) -> TpmtPublic {
    template(bits, base() | oa::SIGN, SymDef::Null, scheme)
}

fn decrypting(bits: u16, scheme: Scheme) -> TpmtPublic {
    template(bits, base() | oa::DECRYPT, SymDef::Null, scheme)
}

fn modulus_of(public: &[u8]) -> Vec<u8> {
    match unmarshal_public(public).unique {
        Unique::Rsa(n) => n,
        _ => panic!("not an RSA key"),
    }
}

/// A primary key of the owner hierarchy with the authorization value `pw`: its handle and modulus.
fn primary(t: &mut Tpm, public: &TpmtPublic) -> (u32, Vec<u8>) {
    let p = parse_primary(&create_primary(t, OWNER, b"pw", public));
    (p.handle, modulus_of(&p.public))
}

/// `(sigAlg, hash, signature)` of a `TPMT_SIGNATURE`.
fn parse_signature(signature: &[u8]) -> (u16, u16, Vec<u8>) {
    let mut r = Reader::new(signature);
    let (a, h) = (r.u16().unwrap(), r.u16().unwrap());
    let s = r.tpm2b(384).unwrap().to_vec();
    r.finish().unwrap();
    (a, h, s)
}

fn signature_bytes(sig_alg: u16, hash_alg: HashAlg, sig: &[u8]) -> Vec<u8> {
    cat(&[&u16b(sig_alg), &u16b(hash_alg.alg()), &tpm2b(sig)])
}

const NULL_VALIDATION: [u8; 8] = [0x80, 0x24, 0x40, 0, 0, 7, 0, 0];

fn sign_digest(t: &mut Tpm, key: u32, pw: &[u8], digest: &[u8]) -> Vec<u8> {
    let params = cat(&[&tpm2b(b""), &tpm2b(digest), &NULL_VALIDATION]);
    run(t, with_pw(cc::SIGN_DIGEST, &[key], &[pw], &params))
}

fn verify_digest(t: &mut Tpm, key: u32, digest: &[u8], signature: &[u8]) -> Vec<u8> {
    let params = cat(&[&tpm2b(b""), &tpm2b(digest), signature]);
    run(t, plain(cc::VERIFY_DIGEST_SIGNATURE, &[key], &params))
}

fn sequence_sign(t: &mut Tpm, key: u32, key_pw: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    let auth = b"seq";
    let start = run(
        t,
        plain(
            cc::SIGN_SEQUENCE_START,
            &[key],
            &cat(&[&tpm2b(auth), &tpm2b(&[])]),
        ),
    );
    let sequence = seq_handle(&start);
    let (last, head) = parts.split_last().unwrap();
    for part in head {
        ok(&seq_update(t, sequence, auth, part));
    }
    run(
        t,
        with_pw(
            cc::SIGN_SEQUENCE_COMPLETE,
            &[sequence, key],
            &[auth, key_pw],
            &tpm2b(last),
        ),
    )
}

fn sequence_verify(t: &mut Tpm, key: u32, signature: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    let auth = b"vseq";
    let start = run(
        t,
        plain(
            cc::VERIFY_SEQUENCE_START,
            &[key],
            &cat(&[&tpm2b(auth), &tpm2b(&[]), &tpm2b(&[])]),
        ),
    );
    let sequence = seq_handle(&start);
    for part in parts {
        ok(&seq_update(t, sequence, auth, part));
    }
    run(
        t,
        with_pw(
            cc::VERIFY_SEQUENCE_COMPLETE,
            &[sequence, key],
            &[auth],
            signature,
        ),
    )
}

/// The `TPMT_PUBLIC` of `TPM2_ReadPublic`.
fn read_public_area(t: &mut Tpm, handle: u32) -> TpmtPublic {
    let response = read_public(t, handle);
    let mut r = Reader::new(params(&response, 0));
    unmarshal_public(r.tpm2b(limits::MAX_RESPONSE_SIZE).unwrap())
}

/// The handle of a successful `CreatePrimary`/`Load` response (the shared parsers stop at 64-octet Names).
fn handle_of(response: &[u8]) -> u32 {
    ok(response);
    u32::from_be_bytes([response[10], response[11], response[12], response[13]])
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
    let v = r.tpm2b(384).unwrap().to_vec();
    r.finish().unwrap();
    v
}

/// Every reply of an RSA-2048, SHA-256 flow (primaries, RSASSA signing and verification, OAEP,
/// RSAES and raw RSA, a child key and `TestParms`) as the SHA-256 of its bytes. The DRBG is
/// shared by all commands, so a change in the octets any of them draws changes every later reply.
fn rsa2048_sha256_flow() -> Vec<String> {
    let mut t = started();
    let mut out = Vec::new();
    let mut note = |reply: Vec<u8>| {
        ok(&reply);
        out.push(hex_of(&digest_of(HashAlg::Sha256, &reply)));
        reply
    };
    let oaep_key = rsa(
        base() | oa::DECRYPT,
        SymDef::Null,
        Scheme {
            alg: alg::OAEP,
            hash: alg::SHA256,
        },
    );
    let any_key = rsa(base() | oa::DECRYPT, SymDef::Null, Scheme::NULL);
    let signer = parse_primary(&note(create_primary(&mut t, OWNER, b"pw", &rsa_sign())));
    let oaep = parse_primary(&note(create_primary(&mut t, OWNER, b"pw", &oaep_key)));
    let any = parse_primary(&note(create_primary(&mut t, OWNER, b"pw", &any_key)));
    let storage = parse_primary(&note(create_primary(&mut t, OWNER, b"pw", &rsa_storage())));
    let digest = digest_of(HashAlg::Sha256, b"golden rsa2048");
    let signed = note(sign(
        &mut t,
        signer.handle,
        b"pw",
        &digest,
        &rsa_sig_scheme(),
    ));
    note(verify(&mut t, signer.handle, &digest, params(&signed, 0)));
    let message = b"rsa2048 golden";
    let oaep_scheme = cat(&[&u16b(alg::OAEP), &u16b(alg::SHA256)]);
    for (key, scheme, label) in [
        (oaep.handle, u16b(alg::NULL), &b"label\0"[..]),
        (oaep.handle, oaep_scheme.clone(), &b""[..]),
        (any.handle, oaep_scheme.clone(), &b"label\0"[..]),
        (any.handle, u16b(alg::RSAES), &b""[..]),
        (any.handle, u16b(alg::NULL), &b""[..]),
    ] {
        let encrypted = note(rsa_encrypt(&mut t, key, message, &scheme, label));
        let cipher = out_buffer(&encrypted);
        note(rsa_decrypt(&mut t, key, b"pw", &cipher, &scheme, label));
    }
    let child = parse_child(&note(create_child(
        &mut t,
        storage.handle,
        b"pw",
        b"cpw",
        b"",
        &rsa_sign(),
    )));
    let (handle, _) = parse_loaded(&note(load(
        &mut t,
        storage.handle,
        b"pw",
        &child.private,
        &child.public,
    )));
    let signed = note(sign(&mut t, handle, b"cpw", &digest, &rsa_sig_scheme()));
    note(verify(&mut t, handle, &digest, params(&signed, 0)));
    for encoded in [
        "000100100014000b080000010001",
        "000100100017000b080000000000",
        "00010006008000430010080000000000",
    ] {
        note(run(&mut t, plain(0x18A, &[], &hex(encoded))));
    }
    out
}

// ------------------------------------------------------------------------------ RSA-3072 keys

const A_N: &str = "b644b16b456039807f1995f16458902d7c94aa232ce2f4b905d5fc33ef26c4ccd086e1dfd796da6cf816e822215e018ab81c15401131db401ea76272a5398f9b49ad1783169dd847cd26e518c72e11f14c9497c649fc3a2ceff15207d49aba8fad64049a1affbb4b78b72c3129e3fda746834728bbcb7cb009e54b039eeb383618f468606f7659dd55eda768b15ccc9f73c0e708021ae0075fc82011636c756a4254706506ba087e71e98a06c06d809524ef4083be8d5ee74c45d656e380b5fee817dc5764a264e1b0111f9abe602dec462518de35374dec77d9db3e06319314704cd6390eeaef05122f153742cab4c56057f8bcf19aa72e45008d9084f89c3f613de91c572a33f0b581c5c842d37c4ad0dc6d92ba91098edcd7d73164d837a9183fc1e89c5f6ab6fb0add51ef0345aec9cc26fc194c0c161cef9ecabae5c7b87b2a0e8cfc0ceaf38b901764ad692148a745041b297554a5b5d0c281581ceeafcce88ae0f6181b2ddbfcc4dea7a4f3276d1b76d050587ae0ff7df9911ac71013";
const A_SIGNATURE: &str = "1e3b9828190402a68b50f0e3d6160a2a57d8c37257c6beb78cc8e3d8a2a1103686bd92b72abe3cf06b03e32c5575b7d652fd75214d997b4c73d53494c4b2089ec28d3aae66625c18fbff98d1bc8c9b48a2d90ea122f990ad9f14029ade8e8f7b63a9bc94e3b98f0c85474191383044dbf30cbe0adc15c56437f884ae7af945ae4c628279a0b14339e1ec6cfcfbdbdff5e13457d51e81d6dcd04d3a29c2c327127ea8953be9591beb5b0ccfaa924180e4eb0d1842c0e125ed99e66332ff2cfcf3a2b4926fe2bf193f35febca54c496412a65e077b5cb89e6e532c7256c55355af2d9f9d4d552427132349f0df57c8fa60fbdbae03547394c03239f6a10c4e3552f734f05d78cd1848eb3ae7ec13e387fd871dc1d17f469c7c1409d092282c3fad1414859b4ae4b370da3d4f462c6b778cdcd5717341a2905ac74480b0d8e065faae88a97807baa25c9e485560a12f0b65a97a31d1dab2540fc822fc9f361883938a11916e978509ed6f9711d85936da772e1eda73f8f4dd8de35cd983edf6bac9";

#[test]
fn rsa3072_sign_digest_rsassa_sha256_verifies_with_openssl() {
    let mut t = started();
    let (key, n) = primary(&mut t, &signing(3072, RSASSA_256));
    assert_eq!(n.len(), 384);
    assert_eq!(n[0] & 0x80, 0x80, "the modulus has all 3072 bits");
    pinned("A.n", &n, A_N);
    let digest = digest_of(HashAlg::Sha256, MESSAGE);
    let signed = sign_digest(&mut t, key, b"pw", &digest);
    let (sig_alg, sig_hash, signature) = parse_signature(params(&signed, 0));
    assert_eq!((sig_alg, sig_hash), (alg::RSASSA, alg::SHA256));
    assert_eq!(signature.len(), 384);
    // Pinned: `openssl dgst -sha256 -verify` accepted this signature with the public key built from
    // the modulus above; PKCS #1 v1.5 is deterministic, so it is also what `openssl dgst -sha256
    // -sign` gives with the private key of this primary.
    pinned("A.signature", &signature, A_SIGNATURE);
    // TPM2_Sign with the key's scheme gives the same signature.
    let signed_again = sign(&mut t, key, b"pw", &digest, &rsa_sig_scheme());
    assert_eq!(params(&signed_again, 0), params(&signed, 0));
    // The three verification commands accept it, a flipped bit is TPM_RC_SIGNATURE.
    let wire = params(&signed, 0).to_vec();
    ok(&verify(&mut t, key, &digest, &wire));
    ok(&verify_digest(&mut t, key, &digest, &wire));
    ok(&sequence_verify(&mut t, key, &wire, &[MESSAGE]));
    let mut bad = signature.clone();
    bad[200] ^= 1;
    let bad = signature_bytes(alg::RSASSA, HashAlg::Sha256, &bad);
    err(&verify(&mut t, key, &digest, &bad), Rc::SIGNATURE);
    err(&verify_digest(&mut t, key, &digest, &bad), Rc::SIGNATURE);
    // The sequence signs the same message with the same bytes.
    let sequence = sequence_sign(&mut t, key, b"pw", &[&MESSAGE[..10], &MESSAGE[10..]]);
    assert_eq!(parse_signature(params(&sequence, 0)).2, signature);
    // A signature of another size is not a 3072-bit signature.
    let short = signature_bytes(alg::RSASSA, HashAlg::Sha256, &signature[..383]);
    err(&verify(&mut t, key, &digest, &short), Rc::SIGNATURE);
    // The public area read back is the one that was created.
    let public = read_public_area(&mut t, key);
    assert!(matches!(
        public.params,
        PublicParams::Rsa { key_bits: 3072, .. }
    ));
}

#[test]
fn rsa3072_create_load_sign_snapshot_restore() {
    let mut t = started();
    // The parent protects the private areas with SHA-512: its integrity value is 64 octets, so the
    // private area of a 3072-bit storage key (seed value of 64 octets too) is the largest there is.
    let mut parent_template = rsa_storage();
    parent_template.name_alg = alg::SHA512;
    let parent = handle_of(&create_primary(&mut t, OWNER, b"ppw", &parent_template));
    // A 3072-bit signing child: Create, Load, Sign, Verify.
    let child = parse_child(&create_child(
        &mut t,
        parent,
        b"ppw",
        b"kpw",
        b"",
        &signing(3072, RSASSA_256),
    ));
    let n = modulus_of(&child.public);
    assert_eq!(n.len(), 384);
    assert!(child.private.len() <= MAX_PRIVATE);
    let key = handle_of(&load(&mut t, parent, b"ppw", &child.private, &child.public));
    let digest = digest_of(HashAlg::Sha256, MESSAGE);
    let signed = sign(&mut t, key, b"kpw", &digest, &rsa_sig_scheme());
    let wire = params(&signed, 0).to_vec();
    let (_, _, signature) = parse_signature(&wire);
    ok(&verify(&mut t, key, &digest, &wire));
    assert!(crate::crypto::rsa_verify_hash(
        &n,
        HashAlg::Sha256,
        &digest,
        &signature
    ));
    err(
        &sign(&mut t, key, b"wrong", &digest, &rsa_sig_scheme()),
        Rc::AUTH_FAIL.in_session(1),
    );
    // A context holds the 3072-bit key: save, flush, load, the same signature.
    let ctx = params(&run(&mut t, plain(cc::CONTEXT_SAVE, &[key], &[])), 0).to_vec();
    ok(&flush(&mut t, key));
    let loaded = run(&mut t, plain(cc::CONTEXT_LOAD, &[], &ctx));
    ok(&loaded);
    let key = u32::from_be_bytes([loaded[10], loaded[11], loaded[12], loaded[13]]);
    let again = sign(&mut t, key, b"kpw", &digest, &rsa_sig_scheme());
    assert_eq!(params(&again, 0), &wire[..]);
    // The largest private area: a 3072-bit storage child with a SHA-512 nameAlg under that parent
    // is 2 + 64 (integrity) + 2 + 16 (IV) + 522 (the encrypted TPM2B_SENSITIVE) = 606 octets,
    // above the 512 octets TPM2_Load accepted for RSA-2048 only.
    let mut storage = template(
        3072,
        base() | oa::RESTRICTED | oa::DECRYPT,
        SymDef::Aes128Cfb,
        Scheme::NULL,
    );
    storage.name_alg = alg::SHA512;
    let big = parse_child(&create_child(&mut t, parent, b"ppw", b"spw", b"", &storage));
    assert_eq!(big.private.len(), 606);
    assert!(big.private.len() > 512 && big.private.len() <= MAX_PRIVATE);
    let storage_key = handle_of(&load(&mut t, parent, b"ppw", &big.private, &big.public));
    // ... and it is a working parent: a 2048-bit child under it loads and signs.
    let grandchild = parse_child(&create_child(
        &mut t,
        storage_key,
        b"spw",
        b"gpw",
        b"",
        &rsa_sign(),
    ));
    let g = handle_of(&load(
        &mut t,
        storage_key,
        b"spw",
        &grandchild.private,
        &grandchild.public,
    ));
    ok(&sign(&mut t, g, b"gpw", &digest, &rsa_sig_scheme()));
    // Persistent: the key survives Shutdown and a snapshot restore and signs the same bytes.
    ok(&evict(&mut t, OWNER, b"", key, 0x8100_0001));
    shutdown(&mut t, su::CLEAR);
    let mut restored = Tpm::restore(&t.persistent_snapshot(), &[7; 48]).unwrap();
    startup(&mut restored, su::CLEAR);
    let persistent = sign(
        &mut restored,
        0x8100_0001,
        b"kpw",
        &digest,
        &rsa_sig_scheme(),
    );
    assert_eq!(params(&persistent, 0), &wire[..]);
    let public = read_public_area(&mut restored, 0x8100_0001);
    assert_eq!(public.unique, Unique::Rsa(n));
}

#[test]
fn rsa3072_test_parms_accepts_keybits_3072_and_rejects_other() {
    let mut t = started();
    let before = t.persistent_snapshot();
    // TPMT_PUBLIC_PARMS: type RSA (0001), symmetric NULL (0010), scheme (NULL, RSAES, RSASSA, RSAPSS
    // and OAEP with each hash), keyBits, exponent 0 (65537).
    for bits in ["0800", "0c00"] {
        for scheme in [
            "0010", "0015", "0014000b", "0014000c", "0014000d", "0016000b", "0016000c", "0016000d",
            "0017000b", "0017000c", "0017000d",
        ] {
            let encoded = format!("00010010{scheme}{bits}00000000");
            let response = run(&mut t, plain(0x18A, &[], &hex(&encoded)));
            assert_eq!(response, hex("80010000000a00000000"), "{encoded}");
        }
    }
    // Any other key size is TPM_RC_VALUE for parameter 1 (Part 2, Table 189); a hash outside the
    // three is TPM_RC_HASH.
    for bits in ["0400", "0bff", "0c01", "1000", "0000", "ffff"] {
        let encoded = format!("000100100014000b{bits}00000000");
        err(
            &run(&mut t, plain(0x18A, &[], &hex(&encoded))),
            Rc::VALUE.in_param(1),
        );
    }
    for scheme in ["00140004", "00160004", "00170004", "00160010"] {
        let encoded = format!("00010010{scheme}0c0000000000");
        err(
            &run(&mut t, plain(0x18A, &[], &hex(&encoded))),
            Rc::HASH.in_param(1),
        );
    }
    assert_eq!(t.persistent_snapshot(), before);
    // CreatePrimary and Create take the same key sizes from the template.
    for bits in [4096u16, 1024, 3073] {
        let mut public = signing(2048, RSASSA_256);
        if let PublicParams::Rsa { key_bits, .. } = &mut public.params {
            *key_bits = bits;
        }
        let r = create_primary(&mut t, OWNER, b"pw", &public);
        err(&r, Rc::VALUE.in_param(2));
    }
}

// ----------------------------------------------------------------------------------- RSASSA-PSS

const C_N: &str = "b1d486f29b6991819ffadc8ba7d7bf46346dce3e8e4c3fc9ea0a30089b3e42923c3e1d889e7f434ab78782c22f97c1b622ab940e14c62aeddb97c7c71bd3c4108ce4d3f87c8416f82b954d72c44b58b083b124fdad2b5f3ba5d105373e1c8231d86ec5a4e81e2e452408f3965b9bea7b5dc38a2818d4940b53295118cc0b82c065991c422fda99514a94cd8a008d3884ed0ff82891d33dad502d3280a7eb0ce64aee769ff2ff7ab90264fd761aa278ab94a5a88a1493bf549966250a55ebb6cc28560b02f3579fa7fad80cc0adaed157a119e5ebd6eb287b80402dbe8ab9cbdf4888f2e435a5d167dfad8b82b4aa14a608847549ee624d8f0a5e411fee7c3c9c5b3ebc1eea2497096a34d3a5f8d487b9115177edba07b2a03352b4b05563884b6cac3fecab026e90c0b99209a320a873a473e34ad441d701e26c5ca42fc3c0fa80321cac04e257558d026dc9c34688bf785dec081edda51efab4be8782250e09e130a0473ce2d5d7f31b1afc8b8c11cac07c8a1d26d4d2b7d6e52c22fe2a3505";
const C_PSS_SHA256: &str = "071cb8047ab45915ef6e0eacdbf6e4a0947c6fbbd741d95c68ae0ac37869ffc9e8442ac7833fec6b50afe258ae6c80e24b0eb392ed28fe495b64e15f637888b1fcc43f9204faab14189615b8621b211789ac425ca10ca7bc2cde5873c9e4394001458112c672424976f397a1ca1be03030ca1e771d4cec13f88068f3ec783f403e9589d64f7764222ff911c150a6c70336b438eb7e8fc8d150be3164fbbacbe3a74d65578695d53cecc43e521f83c4566119a2e2390101a8c72425d5bc37b41c0d29fa05e328ed9f3b527579473ccd261c073dabeb6b3c4b8fb56e8aea58dc2dfd346a385099542c8f4e9a9ed0fae6fff6d42b5605e7012e86fbd77ca6a38aebbad304e8de400f71ee22bdfef40415fd69e0717a42896be8fe92e6a3dabda51b9ea30efafed4cb2b45b53cae2499088ffed560132b8ae9ec702353cdcdd148a52f1ea5aec4b000cc619ab30f1f6442883e8220ec09653de7cb293ac0f6d524b7169dfc8648b400853751636613b5a7ea3abb15c9ebb7e9008fda0be401ad9e65";
const C_PSS_SHA384: &str = "7a629ffe9ae6def17c90226cd63993ecc75deb6536c935eec9ce513cc5a431c55c1678e74392ddb1638d5871771b50680cb254333f27b7cee0156344c18f90e335417dcd4ffff18a06a5ea1fa3ef976cbd80dd6604b05ce60b6b1ae833077836d89cf18e9e82b25f50ae7ea78cfd2d5a9f61c985828d2379f7f7ed69b02a73604bea03a060b9307ece98e49f1318efb6223506f5f749aeaac78b51c584bb3c20499437c32526d02f7f1a8b36d6ea60cb4a384a3044e755c260446d9c41ebf140a3a422a3b8dbeaf50108ca380d6f154a2b93fb13da95585e13c3839b7a863cd4133c6599310e8a86c5b3a16eb90251a998691bbdf68a23e83fdf77bbf6e928debed50e79d2875cce70e7194e5932ac038abd46904220770ec3077dbeec5c0a1a4b357b8d8ac269baf5c8d38b28f2538ae67c12edd70035c4ae0f4b2b9f87aecfa5edd6c05539295f790d7eafdfcf36b43acc800178503bfee4dfe40690b82ece8f753a490c59e95159683c6a540e528e42836534c0ce3f54121dcc9bddfbff16";
const D_N: &str = "c942602694119c04c948b2be23a47534f086f715a8bcda86eb0ba03d37f4be9085a62451042d2696b520e2ea0ff62c3b9c7600f02aa3e21115bd61a0325cadf911a061e3dcfd6442a4499f0527a42d47ccf8482d064b94cbd189d8871fe993986e44e2637a0aecac446d6ef4f0ef047a2fa3f0a63b26059ad93afe9197b3242652b24a86dac7347c8211571627f7309b43c4fe272592c735c36b9646a2becefa7f91cba368202721bbd1bed5bead8aed89b68692796c99c6f5e7695a38a0e867e7d5a2d34c47f4c72865eba1cf052be7db3c5759ecd6cd8647cf00893dcb41ca5657e12c027e4228920cf86aeb045ad31a6908d1ed0a114ad26d8fcbda9fe887";
const D_PSS_SHA384: &str = "770e05fdb4ea375287573a62314c17efdfa3aef9e0c6ef53f97ece5c53b52c80846a2a24cf9d8616338897510da51449e38ea2f6c626f1d8758f1688a008b5091bffa85fa86f71e6c83d7ad42946ea5d3ca395270ab59b52fb98c34bedf587a340b1af5c7d2830517606bbbfd0eb0714e70ed69945f9d04efa8c919883b3d267dbecbdb188d4ee4b3b9c0d392fb1b0670e4d610687f4f3722b0f7d73fa70e8c00ceb5e8048944dcfa5245953292ec6da9a9dc3b46412215e8ebc2476dddb828aab7d8c5d29093860646ce8b46790209c7af69d17061614e91f97e1996dd74df86967f07a3d8a8a077096f4b8547dfd21f04093b111acaad2b00baf7703c2258c";

#[test]
fn rsapss_sign_verify_sha256_and_sha384_with_openssl() {
    let mut t = started();
    // A 3072-bit signing key without a scheme: `inScheme` selects RSAPSS (or RSASSA) per command.
    let (c, n) = primary(&mut t, &signing(3072, Scheme::NULL));
    pinned("C.n", &n, C_N);
    for (hash_alg, expected) in [
        (HashAlg::Sha256, C_PSS_SHA256),
        (HashAlg::Sha384, C_PSS_SHA384),
    ] {
        let digest = digest_of(hash_alg, MESSAGE);
        let pss = scheme_bytes(scheme(alg::RSAPSS, hash_alg));
        let signed = sign(&mut t, c, b"pw", &digest, &pss);
        let wire = params(&signed, 0).to_vec();
        let (sig_alg, sig_hash, signature) = parse_signature(&wire);
        assert_eq!((sig_alg, sig_hash), (alg::RSAPSS, hash_alg.alg()));
        assert_eq!(signature.len(), 384);
        // Pinned: `openssl dgst -sha256|-sha384 -sigopt rsa_padding_mode:pss -sigopt
        // rsa_pss_saltlen:digest -verify` accepted this signature with the public key built from
        // the modulus above, over the message that hashes to `digest`.
        pinned(&format!("C.pss.{hash_alg:?}"), &signature, expected);
        ok(&verify(&mut t, c, &digest, &wire));
        // The salt comes from the DRBG: a second signature differs and verifies too.
        let second = params(&sign(&mut t, c, b"pw", &digest, &pss), 0).to_vec();
        assert_ne!(second, wire);
        ok(&verify(&mut t, c, &digest, &second));
        // The same bytes under the RSASSA label are no PKCS #1 v1.5 signature, and the RSASSA
        // signature of the key is no PSS signature.
        err(
            &verify(
                &mut t,
                c,
                &digest,
                &signature_bytes(alg::RSASSA, hash_alg, &signature),
            ),
            Rc::SIGNATURE,
        );
        let rsassa = scheme_bytes(scheme(alg::RSASSA, hash_alg));
        let pkcs1 = params(&sign(&mut t, c, b"pw", &digest, &rsassa), 0).to_vec();
        ok(&verify(&mut t, c, &digest, &pkcs1));
        let (_, _, pkcs1) = parse_signature(&pkcs1);
        err(
            &verify(
                &mut t,
                c,
                &digest,
                &signature_bytes(alg::RSAPSS, hash_alg, &pkcs1),
            ),
            Rc::SIGNATURE,
        );
    }

    // A 2048-bit key whose scheme is RSAPSS with SHA-384: SignDigest, the sequences and the
    // attestation commands sign with PSS; inScheme must repeat the scheme or be NULL.
    let pss384 = scheme(alg::RSAPSS, HashAlg::Sha384);
    let (d, n) = primary(&mut t, &signing(2048, pss384));
    assert_eq!(n.len(), 256);
    pinned("D.n", &n, D_N);
    let digest = digest_of(HashAlg::Sha384, MESSAGE);
    let signed = sign_digest(&mut t, d, b"pw", &digest);
    let wire = params(&signed, 0).to_vec();
    let (sig_alg, sig_hash, signature) = parse_signature(&wire);
    assert_eq!((sig_alg, sig_hash), (alg::RSAPSS, alg::SHA384));
    assert_eq!(signature.len(), 256);
    // Pinned: verified with `openssl dgst -sha384 ... rsa_pss_saltlen:digest -verify`.
    pinned("D.pss.Sha384", &signature, D_PSS_SHA384);
    ok(&verify(&mut t, d, &digest, &wire));
    ok(&verify_digest(&mut t, d, &digest, &wire));
    ok(&sequence_verify(&mut t, d, &wire, &[MESSAGE]));
    let rsassa384 = scheme_bytes(scheme(alg::RSASSA, HashAlg::Sha384));
    err(
        &sign(&mut t, d, b"pw", &digest, &rsassa384),
        Rc::SCHEME.in_param(2),
    );
    err(
        &sign(
            &mut t,
            d,
            b"pw",
            &digest_of(HashAlg::Sha256, MESSAGE),
            &scheme_bytes(scheme(alg::RSAPSS, HashAlg::Sha256)),
        ),
        Rc::SCHEME.in_param(2),
    );
    ok(&sign(&mut t, d, b"pw", &digest, &u16b(alg::NULL)));
    ok(&sign(&mut t, d, b"pw", &digest, &scheme_bytes(pss384)));
    let sequence = sequence_sign(&mut t, d, b"pw", &[&MESSAGE[..7], &MESSAGE[7..]]);
    let (sig_alg, sig_hash, _) = parse_signature(params(&sequence, 0));
    assert_eq!((sig_alg, sig_hash), (alg::RSAPSS, alg::SHA384));
    ok(&sequence_verify(
        &mut t,
        d,
        params(&sequence, 0),
        &[MESSAGE],
    ));
    err(
        &sequence_verify(&mut t, d, params(&sequence, 0), &[b"another message"]),
        Rc::SIGNATURE,
    );
    // An attestation is hashed with SHA-384 and signed with RSAPSS.
    let quoted = quote(&mut t, d, b"pw", b"qualifying", &u16b(alg::NULL), &sel04());
    let (attest, attest_signature) = parse_attestation(&quoted);
    assert_eq!(&attest_signature[..4], &[0x00, 0x16, 0x00, 0x0c]);
    ok(&verify(
        &mut t,
        d,
        &digest_of(HashAlg::Sha384, &attest),
        &attest_signature,
    ));
    // A key whose scheme is RSASSA verifies RSASSA signatures only.
    let (e, _) = primary(&mut t, &rsa_sign());
    err(
        &verify(
            &mut t,
            e,
            &digest_of(HashAlg::Sha256, MESSAGE),
            &signature_bytes(alg::RSAPSS, HashAlg::Sha256, &[1; 256]),
        ),
        Rc::SCHEME.in_param(2),
    );
}

fn external_3072(scheme: Scheme) -> TpmtPublic {
    let mut public = template(3072, oa::SIGN | oa::USER_WITH_AUTH, SymDef::Null, scheme);
    public.unique = Unique::Rsa(RSA3072_N.to_vec());
    public
}

// Signatures of the fixed 3072-bit key of the self-test made by `openssl dgst -sha256 -sigopt
// rsa_padding_mode:pss -sign` over the self-test message with `rsa_pss_saltlen` 0, 20 and max (350
// octets); OTHER_MESSAGE has the salt of the digest length, over the message "another message".

#[test]
fn rsapss_verify_accepts_any_salt_length_but_refuses_corrupted_sig() {
    let mut t = started();
    let (key, _) = parse_loaded(&load_external(&mut t, &external_3072(Scheme::NULL), NULL));
    let (fixed, _) = parse_loaded(&load_external(
        &mut t,
        &external_3072(scheme(alg::RSAPSS, HashAlg::Sha256)),
        NULL,
    ));
    let digest = RSA3072_DIGEST_256;
    let pss = |sig: &[u8]| signature_bytes(alg::RSAPSS, HashAlg::Sha256, sig);
    // Control: the signature of `openssl` with a salt as long as the digest verifies, with every
    // command that takes a signature.
    ok(&verify(&mut t, key, &digest, &pss(&RSA3072_PSS_SHA256)));
    ok(&verify(&mut t, fixed, &digest, &pss(&RSA3072_PSS_SHA256)));
    ok(&verify_digest(
        &mut t,
        fixed,
        &digest,
        &pss(&RSA3072_PSS_SHA256),
    ));
    ok(&sequence_verify(
        &mut t,
        fixed,
        &pss(&RSA3072_PSS_SHA256),
        &[RSA3072_MESSAGE],
    ));
    // Valid PSS signatures with a salt of 0, 20 and 350 octets (`openssl` accepts each under its own
    // `rsa_pss_saltlen`; 350 is the maximum, 384 - 32 - 2) are accepted: the verification detects the
    // salt length like the reference TPM (CryptRsa.c PssDecode); only signing fixes it to the digest
    // length. The same signatures over another digest are refused.
    let other_digest = digest_of(HashAlg::Sha256, b"another message");
    for (name, sig) in [
        ("0", &RSA3072_PSS_SALT0),
        ("20", &RSA3072_PSS_SALT20),
        ("max", &RSA3072_PSS_SALTMAX),
    ] {
        let wire = pss(sig);
        ok(&verify(&mut t, key, &digest, &wire));
        ok(&verify(&mut t, fixed, &digest, &wire));
        ok(&verify_digest(&mut t, fixed, &digest, &wire));
        ok(&sequence_verify(&mut t, fixed, &wire, &[RSA3072_MESSAGE]));
        err(&verify(&mut t, key, &other_digest, &wire), Rc::SIGNATURE);
        assert_eq!(sig.len(), 384, "salt {name}");
        // A flipped bit of the same signature is refused whatever the salt length.
        let mut bad = sig.to_vec();
        bad[200] ^= 0x01;
        err(&verify(&mut t, key, &digest, &pss(&bad)), Rc::SIGNATURE);
    }
    // A valid signature of another message, a corrupted one and one of another size.
    err(
        &verify(&mut t, key, &digest, &pss(&RSA3072_PSS_OTHER_MESSAGE)),
        Rc::SIGNATURE,
    );
    for at in [0usize, 1, 100, 191, 383] {
        for bit in [0x01u8, 0x80] {
            let mut bad = RSA3072_PSS_SHA256;
            bad[at] ^= bit;
            err(&verify(&mut t, key, &digest, &pss(&bad)), Rc::SIGNATURE);
            err(
                &verify_digest(&mut t, fixed, &digest, &pss(&bad)),
                Rc::SIGNATURE,
            );
        }
    }
    err(
        &verify(&mut t, key, &digest, &pss(&RSA3072_PSS_SHA256[..383])),
        Rc::SIGNATURE,
    );
    err(&verify(&mut t, key, &digest, &pss(&[])), Rc::SIGNATURE);
    err(
        &verify(&mut t, key, &digest, &pss(&[0; 385])),
        Rc::SIZE.in_param(2),
    );
    // Another digest, another hash label: the signature is for SHA-256 and this message only.
    let other = digest_of(HashAlg::Sha256, b"another message");
    err(
        &verify(&mut t, key, &other, &pss(&RSA3072_PSS_SHA256)),
        Rc::SIGNATURE,
    );
    err(
        &verify(
            &mut t,
            key,
            &RSA3072_DIGEST_384,
            &signature_bytes(alg::RSAPSS, HashAlg::Sha384, &RSA3072_PSS_SHA256),
        ),
        Rc::SIGNATURE,
    );
    // The PKCS #1 v1.5 signature under the PSS label and the PSS signature under the RSASSA label.
    err(
        &verify(&mut t, key, &digest, &pss(&RSA3072_PKCS1_SHA256)),
        Rc::SIGNATURE,
    );
    err(
        &verify(
            &mut t,
            key,
            &digest,
            &signature_bytes(alg::RSASSA, HashAlg::Sha256, &RSA3072_PSS_SHA256),
        ),
        Rc::SIGNATURE,
    );
}

#[test]
fn rsa3072_external_key_verifies_openssl_signatures_and_checks_the_modulus_size() {
    let mut t = started();
    let (key, name) = parse_loaded(&load_external(&mut t, &external_3072(Scheme::NULL), NULL));
    assert_eq!(name.len(), 34);
    // Signatures made by `openssl dgst -sign` with the private key of the fixed 3072-bit key.
    let sha256 = RSA3072_DIGEST_256;
    let sha384 = RSA3072_DIGEST_384;
    ok(&verify(
        &mut t,
        key,
        &sha256,
        &signature_bytes(alg::RSASSA, HashAlg::Sha256, &RSA3072_PKCS1_SHA256),
    ));
    ok(&verify(
        &mut t,
        key,
        &sha256,
        &signature_bytes(alg::RSAPSS, HashAlg::Sha256, &RSA3072_PSS_SHA256),
    ));
    ok(&verify(
        &mut t,
        key,
        &sha384,
        &signature_bytes(alg::RSAPSS, HashAlg::Sha384, &RSA3072_PSS_SHA384),
    ));
    // A key with the scheme RSASSA does not take a PSS signature, nor the reverse.
    let (rsassa_key, _) = parse_loaded(&load_external(&mut t, &external_3072(RSASSA_256), NULL));
    err(
        &verify(
            &mut t,
            rsassa_key,
            &sha256,
            &signature_bytes(alg::RSAPSS, HashAlg::Sha256, &RSA3072_PSS_SHA256),
        ),
        Rc::SCHEME.in_param(2),
    );
    ok(&verify(
        &mut t,
        rsassa_key,
        &sha256,
        &signature_bytes(alg::RSASSA, HashAlg::Sha256, &RSA3072_PKCS1_SHA256),
    ));
    // The modulus must have keyBits bits and be odd (TPM_RC_KEY for parameter 2).
    let mut wrong_size = external_3072(Scheme::NULL);
    wrong_size.unique = Unique::Rsa(hex(EXT_RSA_N));
    err(
        &load_external(&mut t, &wrong_size, NULL),
        Rc::KEY.in_param(2),
    );
    let mut as_2048 = external_3072(Scheme::NULL);
    as_2048.params = PublicParams::Rsa {
        symmetric: SymDef::Null,
        scheme: Scheme::NULL,
        key_bits: 2048,
        exponent: 0,
    };
    err(&load_external(&mut t, &as_2048, NULL), Rc::KEY.in_param(2));
    let mut even = external_3072(Scheme::NULL);
    even.unique = Unique::Rsa([&RSA3072_N[..383], &[RSA3072_N[383] & 0xfe]].concat());
    err(&load_external(&mut t, &even, NULL), Rc::KEY.in_param(2));
    let mut short_top = external_3072(Scheme::NULL);
    short_top.unique = Unique::Rsa([&[RSA3072_N[0] & 0x7f][..], &RSA3072_N[1..]].concat());
    err(
        &load_external(&mut t, &short_top, NULL),
        Rc::KEY.in_param(2),
    );
}

#[test]
fn rsa3072_primes_survive_the_duplicate_and_private_area_codecs() {
    use crate::crypto::{rsa_from_one_prime, rsa_from_primes, rsa_primes, rsa_sign};
    use crate::hash::AuthValue;
    use crate::object::{
        Secret, Sensitive, sensitive_from_bytes, sensitive_from_duplicate, sensitive_to_bytes,
        sensitive_to_duplicate,
    };
    use zeroize::Zeroizing;

    let pq = [&RSA3072_P[..], &RSA3072_Q[..]].concat();
    let key = rsa_from_primes(&pq, &RSA3072_N).unwrap();
    assert_eq!(&rsa_primes(&key).unwrap()[..], &pq[..]);
    // The fixed key was made by `openssl genpkey`; the signature below is `openssl dgst -sign`'s.
    let mut rng = crate::random::Drbg::new(&[5; 48]).unwrap();
    let signature = rsa_sign(&key, HashAlg::Sha256, &RSA3072_DIGEST_256, &mut rng).unwrap();
    assert_eq!(signature.as_slice(), RSA3072_PKCS1_SHA256);

    let mut public = external_3072(RSASSA_256);
    public.attributes = base() | oa::SIGN;
    let sensitive = Sensitive {
        auth: AuthValue::from_slice_trimmed_up_to(b"auth", 32).unwrap(),
        seed: Zeroizing::new(Vec::new()),
        secret: Secret::Rsa(Box::new(key)),
    };
    // The local private-area codec carries both primes (384 octets of private value).
    let bytes = sensitive_to_bytes(&sensitive, alg::RSA, alg::SHA256).unwrap();
    assert_eq!(bytes.len(), 2 + 2 + (2 + 32) + 2 + (2 + 384));
    let back = sensitive_from_bytes(&bytes, &public).unwrap();
    let Secret::Rsa(back) = &back.secret else {
        panic!("not an RSA key");
    };
    assert_eq!(&rsa_primes(back).unwrap()[..], &pq[..]);
    // The duplicate codec carries one prime (192 octets); the other is derived from the modulus.
    let duplicate = sensitive_to_duplicate(&sensitive, alg::RSA, alg::SHA256).unwrap();
    assert_eq!(duplicate.len(), 2 + 2 + (2 + 32) + 2 + (2 + 192));
    let back = sensitive_from_duplicate(&duplicate, &public).unwrap();
    let Secret::Rsa(back) = &back.secret else {
        panic!("not an RSA key");
    };
    assert_eq!(&rsa_primes(back).unwrap()[..], &pq[..]);
    // Either prime gives the key back; a wrong prime, or one of the 2048-bit size, does not.
    assert!(rsa_from_one_prime(&RSA3072_P, &RSA3072_N).is_some());
    assert!(rsa_from_one_prime(&RSA3072_Q, &RSA3072_N).is_some());
    let mut wrong = RSA3072_P;
    wrong[191] ^= 2;
    assert!(rsa_from_one_prime(&wrong, &RSA3072_N).is_none());
    assert!(rsa_from_one_prime(&RSA3072_P[..128], &RSA3072_N).is_none());
    // A private area of the other size does not belong to the public area (TPM_RC_KEY_SIZE).
    let mut small = public.clone();
    small.params = PublicParams::Rsa {
        symmetric: SymDef::Null,
        scheme: RSASSA_256,
        key_bits: 2048,
        exponent: 0,
    };
    assert_eq!(
        sensitive_from_bytes(&bytes, &small).err(),
        Some(Rc::KEY_SIZE)
    );
}

// ----------------------------------------------------------------------------------- RSA-OAEP

const B_N: &str = "be7e6f0bd3eeedf48ac5259a549ac9270948a6171bf9d4eb30b22863a7558f01f9b62fa38f3a931a960f69ccf878ac54b3301e2c8f7e306c8b4f438b1f31713a4e9e87f3d49bd81d2b37688e0e8a2a237425fda1d24879b4f40b701dbe227305a819d4fed9ac8564d3b772fc28d66fdfc1aacdd8a0aac0bb9c7dcf73e317e4fd8da2d5b7033a82ae9da9840d0204d8bf6ecfaa3be678ad1819318b2da2ab9fa2b887dc1652e1ec7bba79ec78d12ea4c8d0f0f46e756a0361ce041fb48c2a576910e8384bb367477cf92a8edd4ce00166af38744bbf6c57a5d8f11ce24bfd0ad9837f8d84f79750b5549be52fd3eec6fc1f6f33012c1b502c037f1e4fb5913cf695323d0a2127da12a0925330dba42cc87d951963368320698dac721d62d42030c0cd16e5d9959256b262e9225854b30e91aa19ed49589b1e7d616c6763c5603a6711e088d692b50311dd5fff78c323e0dc995e62a2e1f1b37bfda68e28dcd948f81c9c9ecec437ccb8b431611879afb3a3ec2e479d13d437fea964060e2e9bab";
const B_TPM_OAEP: [&str; 4] = [
    "91c04b35e1af1453984663143296e77e0e087b9d154019779b7bf0f0d3fd0767c163aab25604b2c7595f351545da2f31683a12d49c577b9a0e9e62622217192f54f08f61a2fa3cc7bbf5a358ce9732299e868e743e7bd993dfa8b6fd7a79efa1b82e915f7fc39f419fa9b3f06f746f45f564d6290e165b2794686c79d06617500daf19096ccfe2c2457b2ae57e6474ef7e5b962ee16d54464405566f9eb7e72c6bffffab3a7c3800a1a552f098af7a628328a36b73e109880f92711bbcffdb256b5fb8d9986bf2181eb339725503a52528e7aa748b7db397a0141e9fad7bf2ec5cc3736d8709fabb3ed9b46b503fd6240815bae1514c7d41f646faed9d421db028e32a07cf2a5ef936ad24a9b5fdb21ec7a8936e1b51ac08f5c94e60778435e9cd1c4a00f45a8f64a0beeab8ece7cfff9ae262db54d9b2ad0639f3a3d299a9e9fd9f38d06591dcf02a7e89e8c5e49974b49236076cd19f3b13104f8a4e7ff4ea8d2642288daaff360ad562a161a6de83ee5719d5d9626a8369ecd83cf2cb03ef",
    "953298c08de293a9976563ed7afaf70ef8ff1904858e0506c08f3a8889e3bab5928faebcd0dace305f880f948d72f90189258330e53b90b4eac73cc29da7f7f67cdc1aef82a67cc0b04a2c4b85380466b9a9303f130f7a2821e9369c6096ef55fc2ab77912499a19749f024a03173fc1893304ce613852dbff830647846b2e12a83b0b50721cb3035fe5d3273c27cc48c02383055302f721a99a7c24ebf78e32f46ebe336771bd840e4893746e50f71505cbed603fc311566d71b646ebacee7d6ec67ca2df58e83f4b641be87611ae3dfc29e222bc51c570fd22a34223d71078f7927d11333a6a2383833025a592125f1fabed1c7bda788ede0e52f4ae6d5744127d6f1443fc9b0be3777da242d3ef1e975b7ead352b305f57c7a15a6e7bed98ae3122a69334ff4217b5460ad6f1b3c233dcda6c3c0d38df6df5dbd3597d10ada66a91359cf1974d4c47a3ce7aa045c11b762a915c1524448f458594857cdc17ed466107bfff5f558f2c41cfdf8d3edbbdb4dc24aa4ed1daaad8d1121bc1f6f6",
    "219dbb55b4f665cdacf5b896764613ac8a1e46c39608fbfebdfe321d67bb0e7da2c08192e37ea53d807fc75bad2826664d9bca7463807e00cec9346c10601f3cf34ca91c421b26dc38cc0da9ce66b49071da931905a94cea52bb7ba3aa74cb0769df69b433c41d46673a2f8593b0c067ad812de19ed1bdefd6cc989ab04b160cbba94299c1d544d7c875cff6d6a7ef83f483efeed6a37201055d201e1a7fa517146e3fae51afec8b33c5e59a62be5e58b30026ea17e328b6624752b19945398316d70e11dd3c8445788c6b3b4a876249cab179eb2ad66bd9b10f6361511da3474236e08076291b626c44e1ee23273995f69f47f93b9c4eceedbc2791de45fa095c3fe3025887200f670bd852431961ca7132b9121ef016dc0104c2eef877ce3f152e81ab3e26a5c28802b1132faacbc715ba7af4e6fd8ba3b87cf0152a18479b3991a3105af8ae88aae3a900c575d9d7f00a9d29279f5e0bdee50103accbc35d12a359fd15b99ba371d8336276723d6907571e863fc5149a0569005daaa41f8e",
    "9c933b53059f181aee69ce140743d0207a72ccc97ea2c5e980b330d352e3f0e10fc3a1bd829a5dc928328470fd156777b16c56ff401315dfba1ec03d5799642e19b239c4e914f3fb9574af3d5559f3ae48349dc7e7b7dd3e79099c19c56777b2df545f129705db8a4385f86ccb853555eeb1456c3b5f83f6a4384ee4057b43417302b1c97b9c1223723da3545f07a39e620ef1dbb01abb78839c266022d32cce9de243591735c042934c7920c9601715ba7efa33fe9f3c5e30f9073e9f2316c988c9dcf74322670bd0a01c6c7081a580c7c4ec9e8b4e9a8eb6e90bbf65cbb38788c3602e4affc9938a823a03d373befdfb7edd5929009135e7ba0972fb997090a012b7aab684c8abe1aa3e4bc5e1486c5b64dc4983db0353786e260f562b848cd82f214f4e72d08c96a00494690445062352f9098e6a4c8f4b7ca5526e58c2ec575606dc2656d445700a89c543b12881f5d2f304a0ed7f90ab0da880be983b903d496b3ddd95b7125742ac3210e2301f926a2aadbbef1b0329f5ebd8722fd3ab",
];
const B_OPENSSL_OAEP: [&str; 4] = [
    "0e9360bae028132ecf57b0f9966545645bd3aab0d9d38c60fb1afa4806b2ac0877f575a14620514df129ad8eaab7e91c3f671a5f0810865920bc3a3701dbb4c24af21c45c68186057125a30f2d622757201521e6d3a9b6753cc8538750822a316af5858cc971fff64b4930bdcb3abee019f9f6f06afd103d848bdd0d3f29467c72c3a31e60444e09530789f38b48e2d7ce25f849fd199cd6fee61ade79a0236a8ccfa0015fdd8f8b74144e2b30e72e000c3b6ea82f08dcc0c9d8001194d236983238696f476945f159b6ef766618a52a036d250306a1e3fbbf029c5928aa968682d68dc3e5c6dd355911dc2eb13822968f2cd4774c17c17e3d03f150607a00eb08010fdff67797c0f72608c7b04d822b131f5252c699d67c28b29ce9e22ef07b8fe8c6b27b06b6994e7597e7cac522512f960200aa5ab345d22db17398af9dd748b3d9eb819ea646dcf037c4beab337cd319d5d8b42d6b56eed690fd8a06945e3cf4c27d998e0d035b5be025f98dad111d51aaac7f63355b4f6f7127362b4c7a",
    "1682a68a799650374cc17d7fc9c275208535ba2bd0274c2197148553b1c94793d2c77ddf86f18a3a48952ed3f8d871bece9a77d12123602b11b39075354203514e3c685cb05340f8db8f0252cda85e1b36efe6db4876c00d7de12393c44cd71ecf7bc3cc4353d06111ef30ee5774986fbcc1d0d10fe66c5bb7e96ccf6e6f15e600c2f68220767eeef378812c47a8f19c26924b0331a8683a9dada9ec2ac5f23ca68c9035a4e47ed8cd8da01c18f62ca1fc6c071faec4add2dbf56015b2a0f6e1c05b0e973e1af54caa75c1d8caf728022315bc66d671337c4f87064c434a4c78efd40df0028b72f22eb65bc09e0427e58287c133e03d4deabc87876101bc9d0388d5776eae6f619aa4dd1e5e4ac98b4edebebaf9482bf2ec81106278bde954f4dfecae0f93c86b1138e5112e10f4ca9bf6ff16f6023609068869b8964197b979e174671ec66d86f7999389864f0f06832c562cc3d25989bfd14fcda47abba324cf48f654fc111ef1eb029133939e303cb9f6b0a408cbe61630e5cf3a0290975d",
    "bc9e7c7d7b0ad7b2e2190f8d25b35ae720d7502229d75ddb08a28f448967b5d1cb289899e30fc88aad4df37480064e5cd2eade7b5a393207c07924acc356265bef4110ad03d926a266e305ee823176330ad290eee06c47127f63285b0a320189fa47ae37f55e7ad62ff88e2ccfc859cf10f8782dd7efbdf169e9c0d9e08cd5d45aa7cada672e89d61bb49a9a21efbdeb4f710930360ae32c7281a6eec7625df6e24a5a9659af01a59b4aa660f78ca888860d4135bdd1ba806b53f3b92b7262018d24b531e4702bc64c7780a95c07c02b37655f9051f865dd3038a4afd6911496aa88a51fb975321da24f5e8fcf543e2b051af392a2fe4c156be0fd01628221463cc935b76cd45441221de3a026c1fbdd3e04944487e2f7f513cb1911b9e0183d5c84f0fa89b0782d19e59f074d9e0e7536b4dafad4054963bdca74b599799160f73b384ad3b05f07913f03eb8099af65468cfc62480599f677f8a0194c712bd81b1b98c863e66f3f9ff6e36a39ca809e386e7013ada7f10d01b5d905eb53e37f",
    "a23e13f50f0d072ec297501afaaf9db398d5425b8770dd87e713d1675332c1d2bd078f3415f9909e91475121e77f0c7355e0de66c4547b5da5ecef485789c384a19d4dbbbd172d1e9e1e285c403b6189f22dcaf4ae0387bdee3d1d7e7c9a1fc48a3f6e8fafa3282bc9b98b7b4bb46fc1f43b74d95f39ee76e46c6d9aa967907c3b92425cd3acfc36f599769abd81f905fe8487d16931a73fd6b1664df0b6153895e43b635378cb6aba5562b2bfaf4ee053f0c805c36869cfd0985cc9cc479df73785ac8d89903ef8e22df466e40aa3a3e1d92cbf00ece0889817e7e38873c12c40708da3db03e8ce9e682185838f4be3ec47e852de3463d10e568d0a53d055c5a29caa01233fd9f830271f927ad658d618e6444adb3fc253c11ff40711a6f5ce69e5ac3f1dafa279c36ec1da181f9848f7a3c1ee221734e9dc8adc851235f59c19d74b5932edf07af688edc236a65520205d07dc4724277181f5b8d18b822e43b6e4750cc96b8e784480421680ad787e2c6126075a34161c16bc34d229c9f894",
];

const OAEP_MESSAGE: &[u8] = b"rsa oaep message for openssl";
/// The hash and the label (with its terminating zero octet) of each OAEP case.
const OAEP_CASES: [(HashAlg, &[u8]); 4] = [
    (HashAlg::Sha256, b""),
    (HashAlg::Sha384, b"lbl\0"),
    (HashAlg::Sha512, b"lbl\0"),
    (HashAlg::Sha384, b""),
];

#[test]
fn rsa_encrypt_oaep_sha384_sha512_roundtrip_and_openssl_decrypt() {
    let mut t = started();
    let (b, n) = primary(&mut t, &decrypting(3072, Scheme::NULL));
    pinned("B.n", &n, B_N);
    assert_eq!(secret_of(&t, b).len(), 384, "p || q of the 3072-bit key");
    let oaep = |hash_alg: HashAlg| scheme_bytes(scheme(alg::OAEP, hash_alg));
    // The TPM encrypts. Pinned: `openssl pkeyutl -decrypt -pkeyopt rsa_padding_mode:oaep -pkeyopt
    // rsa_oaep_md:<hash> -pkeyopt rsa_mgf1_md:<hash> [-pkeyopt rsa_oaep_label:<hex>]` with the
    // private key rebuilt from the primes of this key (Python) gave OAEP_MESSAGE for each.
    let mut ciphertexts = Vec::new();
    for (i, (hash_alg, label)) in OAEP_CASES.iter().enumerate() {
        let encrypted = rsa_encrypt(&mut t, b, OAEP_MESSAGE, &oaep(*hash_alg), label);
        let ciphertext = out_buffer(&encrypted);
        assert_eq!(ciphertext.len(), 384);
        pinned(&format!("B.oaep.{i}"), &ciphertext, B_TPM_OAEP[i]);
        ciphertexts.push(ciphertext);
    }
    for (i, (hash_alg, label)) in OAEP_CASES.iter().enumerate() {
        let scheme = oaep(*hash_alg);
        // The TPM decrypts its own ciphertext and the one `openssl pkeyutl -encrypt` made with the
        // public key of this key (the same hash for the label and for MGF1, the same label).
        for ciphertext in [ciphertexts[i].clone(), hex(B_OPENSSL_OAEP[i])] {
            let decrypted = rsa_decrypt(&mut t, b, b"pw", &ciphertext, &scheme, label);
            assert_eq!(out_buffer(&decrypted), OAEP_MESSAGE, "case {i}");
        }
        // Another hash or another label does not open it.
        for other in [HashAlg::Sha256, HashAlg::Sha384, HashAlg::Sha512] {
            if other != *hash_alg {
                err(
                    &rsa_decrypt(&mut t, b, b"pw", &ciphertexts[i], &oaep(other), label),
                    Rc::VALUE.in_param(1),
                );
            }
        }
        err(
            &rsa_decrypt(&mut t, b, b"pw", &ciphertexts[i], &scheme, b"other\0"),
            Rc::VALUE.in_param(1),
        );
    }
    // The largest message is k - 2 * hLen - 2 octets: 318, 286 and 254 for k = 384.
    for (hash_alg, largest) in [
        (HashAlg::Sha256, 318usize),
        (HashAlg::Sha384, 286),
        (HashAlg::Sha512, 254),
    ] {
        let message = vec![0x5a; largest];
        let encrypted = rsa_encrypt(&mut t, b, &message, &oaep(hash_alg), b"");
        let ciphertext = out_buffer(&encrypted);
        let decrypted = rsa_decrypt(&mut t, b, b"pw", &ciphertext, &oaep(hash_alg), b"");
        assert_eq!(out_buffer(&decrypted), message);
        err(
            &rsa_encrypt(&mut t, b, &vec![0x5a; largest + 1], &oaep(hash_alg), b""),
            Rc::VALUE.in_param(1),
        );
    }
    // A 3072-bit key does not take a message or ciphertext longer than its modulus.
    err(
        &rsa_encrypt(&mut t, b, &[0; 385], &oaep(HashAlg::Sha256), b""),
        Rc::SIZE.in_param(1),
    );
}

#[test]
fn rsa_encrypt_oaep_hash_mismatch_refused() {
    let mut t = started();
    let (e, _) = primary(
        &mut t,
        &decrypting(2048, scheme(alg::OAEP, HashAlg::Sha384)),
    );
    let (h, _) = primary(
        &mut t,
        &decrypting(2048, scheme(alg::OAEP, HashAlg::Sha512)),
    );
    let (any, _) = primary(&mut t, &decrypting(2048, Scheme::NULL));
    let message = b"hash mismatch";
    let oaep = |hash_alg: HashAlg| scheme_bytes(scheme(alg::OAEP, hash_alg));
    let null = u16b(alg::NULL);
    // The scheme of the key: NULL and the same OAEP scheme both select OAEP with its hash.
    let by_null = out_buffer(&rsa_encrypt(&mut t, e, message, &null, b""));
    let by_scheme = out_buffer(&rsa_encrypt(
        &mut t,
        e,
        message,
        &oaep(HashAlg::Sha384),
        b"",
    ));
    for ciphertext in [&by_null, &by_scheme] {
        for s in [&null, &oaep(HashAlg::Sha384)] {
            let decrypted = rsa_decrypt(&mut t, e, b"pw", ciphertext, s, b"");
            assert_eq!(out_buffer(&decrypted), message);
        }
    }
    // Another OAEP hash than the key's is TPM_RC_SCHEME for parameter 2, in both commands.
    for other in [HashAlg::Sha256, HashAlg::Sha512] {
        err(
            &rsa_encrypt(&mut t, e, message, &oaep(other), b""),
            Rc::SCHEME.in_param(2),
        );
        err(
            &rsa_decrypt(&mut t, e, b"pw", &by_null, &oaep(other), b""),
            Rc::SCHEME.in_param(2),
        );
    }
    err(
        &rsa_encrypt(&mut t, h, message, &oaep(HashAlg::Sha384), b""),
        Rc::SCHEME.in_param(2),
    );
    ok(&rsa_encrypt(
        &mut t,
        h,
        message,
        &oaep(HashAlg::Sha512),
        b"",
    ));
    // RSAES does not fit an OAEP key, and the other way round.
    err(
        &rsa_encrypt(&mut t, e, message, &u16b(alg::RSAES), b""),
        Rc::SCHEME.in_param(2),
    );
    // A hash outside SHA-256, SHA-384 and SHA-512 is TPM_RC_HASH for the scheme (parameter 2), a
    // selector that is no encryption scheme TPM_RC_VALUE, whatever the key.
    for key in [e, any] {
        for bad_hash in [0x0004u16, alg::NULL, 0x0027] {
            let scheme = cat(&[&u16b(alg::OAEP), &u16b(bad_hash)]);
            err(
                &rsa_encrypt(&mut t, key, message, &scheme, b""),
                Rc::HASH.in_param(2),
            );
            err(
                &rsa_decrypt(&mut t, key, b"pw", &by_null, &scheme, b""),
                Rc::HASH.in_param(2),
            );
        }
        for selector in [alg::RSAPSS, alg::RSASSA] {
            err(
                &rsa_encrypt(&mut t, key, message, &u16b(selector), b""),
                Rc::VALUE.in_param(2),
            );
        }
    }
    // A key without a scheme takes the hash of inScheme; a ciphertext of one hash is not one of
    // another (a failed padding check is TPM_RC_VALUE for the ciphertext).
    let sha384 = out_buffer(&rsa_encrypt(
        &mut t,
        any,
        message,
        &oaep(HashAlg::Sha384),
        b"",
    ));
    let opened = rsa_decrypt(&mut t, any, b"pw", &sha384, &oaep(HashAlg::Sha384), b"");
    assert_eq!(out_buffer(&opened), message);
    err(
        &rsa_decrypt(&mut t, any, b"pw", &sha384, &oaep(HashAlg::Sha512), b""),
        Rc::VALUE.in_param(1),
    );
    err(
        &rsa_decrypt(&mut t, any, b"pw", &sha384, &oaep(HashAlg::Sha256), b""),
        Rc::VALUE.in_param(1),
    );
    // A signing key's scheme (RSASSA, RSAPSS) selects no padding.
    for sign_scheme in [RSASSA_256, scheme(alg::RSAPSS, HashAlg::Sha256)] {
        let (signer, _) = primary(&mut t, &signing(2048, sign_scheme));
        err(
            &rsa_encrypt(&mut t, signer, message, &oaep(HashAlg::Sha256), b""),
            Rc::SCHEME.in_param(2),
        );
    }
}

#[test]
fn rsa_key_templates_follow_the_rsapss_and_oaep_scheme_rules() {
    let mut t = started();
    let pss = scheme(alg::RSAPSS, HashAlg::Sha384);
    let oaep = scheme(alg::OAEP, HashAlg::Sha512);
    // Signing keys, restricted or not, take RSAPSS with each hash; decryption keys do not.
    for hash_alg in [HashAlg::Sha256, HashAlg::Sha384, HashAlg::Sha512] {
        let s = scheme(alg::RSAPSS, hash_alg);
        for attributes in [base() | oa::SIGN, base() | oa::SIGN | oa::RESTRICTED] {
            let public = template(2048, attributes, SymDef::Null, s);
            let (_, n) = primary(&mut t, &public);
            assert_eq!(n.len(), 256);
        }
    }
    for (attributes, symmetric) in [
        (base() | oa::DECRYPT, SymDef::Null),
        (base() | oa::DECRYPT | oa::RESTRICTED, SymDef::Aes128Cfb),
        (base() | oa::SIGN | oa::DECRYPT, SymDef::Null),
    ] {
        let public = template(2048, attributes, symmetric, pss);
        err(
            &create_primary(&mut t, OWNER, b"pw", &public),
            Rc::SCHEME.in_param(2),
        );
    }
    // OAEP with SHA-512 on an unrestricted decryption key; not on a signing or a restricted key.
    let (_, n) = primary(&mut t, &decrypting(2048, oaep));
    assert_eq!(n.len(), 256);
    for (attributes, symmetric) in [
        (base() | oa::SIGN, SymDef::Null),
        (base() | oa::DECRYPT | oa::RESTRICTED, SymDef::Aes128Cfb),
        (base() | oa::SIGN | oa::DECRYPT, SymDef::Null),
    ] {
        let public = template(2048, attributes, symmetric, oaep);
        err(
            &create_primary(&mut t, OWNER, b"pw", &public),
            Rc::SCHEME.in_param(2),
        );
    }
    // The hash of the scheme is one of the three.
    for scheme_alg in [alg::RSASSA, alg::RSAPSS, alg::OAEP] {
        let mut w = Writer::new();
        template(2048, base() | oa::SIGN, SymDef::Null, RSASSA_256).marshal(&mut w);
        let mut bytes = w.finish();
        // type, nameAlg, attributes, authPolicy (2), symmetric (2), then the scheme and its hash
        let at = 2 + 2 + 4 + 2 + 2;
        bytes[at..at + 2].copy_from_slice(&scheme_alg.to_be_bytes());
        bytes[at + 2..at + 4].copy_from_slice(&0x0004u16.to_be_bytes());
        let params = cat(&[
            &tpm2b(&cat(&[&tpm2b(b"pw"), &tpm2b(b"")])),
            &tpm2b(&bytes),
            &tpm2b(b""),
            &[0, 0, 0, 0],
        ]);
        err(
            &create_primary_raw(&mut t, OWNER, b"", &params),
            Rc::HASH.in_param(2),
        );
    }
}

#[test]
fn sha256_rsa2048_golden_unchanged() {
    // Taken on commit d9d832c, before the RSA-3072 / RSAPSS / OAEP-hash changes.
    const GOLDEN: [&str; 23] = [
        "062e7dd7907668166538fb61f565d3062ffccfdadcdf2348808966c9ed3038d7",
        "8c52b0bfac31cff686788d468d87dd8e3bc5c790f8e53170ce79bb4d3ae1cfec",
        "34d1087a6fb78725ee5e520aa9d264fca3cdc534533d36c26ce4412c2c3d74d2",
        "58ebe2543a86d11d94b809aac98bd6d6b20762120f2da73c53dbe54f08a5f1d9",
        "c7a7d76cdb2caa30c4fb69d0f63709fd3fd72a5df3eadc9f7d25395691824795",
        "32c8f37361e9bfe4d407320ad36c7fbe2e57b269a95a617dc765cf4e842fbbe7",
        "a8ba8e0db48984af12daf173a676e27ecd45445876b5b504043adb52ce842741",
        "ead42c14675c4b5ed10823f4d0592bc6c0090a5bca191eb92a4db09df170d308",
        "3ed63ff603a699c97df4b5fc8147b36f00fdc15e6935b9ec80e09dc31a2b9074",
        "ead42c14675c4b5ed10823f4d0592bc6c0090a5bca191eb92a4db09df170d308",
        "9e5a6ff93237d5dd1f7f8a831e7c7b7fe0ece6bda146f44c5bd251b2df30401e",
        "ead42c14675c4b5ed10823f4d0592bc6c0090a5bca191eb92a4db09df170d308",
        "f16556aa1eb0b2ede3b7397e63915a5abae5499000cceb98bb3e849a6fd2fdcf",
        "ead42c14675c4b5ed10823f4d0592bc6c0090a5bca191eb92a4db09df170d308",
        "aeed89a49d39758cc034d17f8c7e3c8bc654e38e73b7b0f78456467dfd299249",
        "495c383a0a8524184c89e00d5cf0a62b41d755f4b835bcbff4356c93b6d2b215",
        "b0535aed65ce0d5fe273b3dbe2dcc2a338bb02252f71fa91cee03d001007d238",
        "ee5421dbc96a6130d341dba6009de92c6c86b7b959745d93d268efacdcaac863",
        "376826816ce4da910aa59f797f5fe363dbd3dacb4ea7ccf7033d8041326b0819",
        "75b97d120073ae758a20c03aff83d6618e045cf0dcca611777c92097b5df1439",
        "5b2896e614c8b5077c7bea543c34c66431aa403bd731a18d35b9d5e07cd08613",
        "5b2896e614c8b5077c7bea543c34c66431aa403bd731a18d35b9d5e07cd08613",
        "5b2896e614c8b5077c7bea543c34c66431aa403bd731a18d35b9d5e07cd08613",
    ];
    assert_eq!(rsa2048_sha256_flow(), GOLDEN);
}
const RSA3072_PSS_SALT0: [u8; 384] = hex_array(concat!(
    "1a1494a2d8187808799e8f8c67b094705421fe7265b834ef1ca1ec824eb6e5a1",
    "448d3aa7524ee93d2b9a12b07908165861b1010f20f16156110654c4c81234ed",
    "9504a443405b70f386895bfc5a5e9a835d82a7e1732003c843ba9abb0a483b5b",
    "9afecdbb6accaa76e363158906d78244976efdfd21f8a1b5c05b57223bd73f63",
    "2940942edadfae1e2cd85cd2cca31e1da64339316fb82531c20da337b87ed21a",
    "42d119f728f387c1b25c4180928c8ad46a7e647e6f152835cdb30cb6ddf142d2",
    "574b0270b387dd7448b5de63176f2ad7dcd78317fb0826aeb01c15918a6edf26",
    "fb69bee25dacbdc77ed7498abf5fa5fed66b75375faaeb20469336022ab88e68",
    "e88f1a93d393607cc82dffc207f029988f9747bae17a643e0a8c289672041d15",
    "ded9882ce53c3b1709979af1286d6a626161f639322f48f5793ac458c7a5b420",
    "06adf55a68ed15c23c3ea5eb59e3b3f6e1892b38b287fb593e5d8b616e5f85a1",
    "6df10076dbef08338899f69ed0cf2d45117464f24998046191fb6dfc80693bf6",
));
const RSA3072_PSS_SALT20: [u8; 384] = hex_array(concat!(
    "ad9c0030d0568dccf8e845f1c747a7a2550a1bb8406cb7988d1c0e7c4a305a2b",
    "d4e4151ff3436fa8b1255c5f49d026ac76ef1af66a7cc2b2f32a4a3e9cd2e4f7",
    "8b2808c35469759f903943df1e7ecc9decf21e5af5de94000b83df4fb01c9b37",
    "a08bd9a32d5485c5099fac302518de792796bf89523140e13601abffb5aa2131",
    "a00e3b4c753bacb1acbf39b4f46e961e3b69a8bc7614be4f3cd6655fd55e7150",
    "f12b8337f2fcb7f0e7468e042414aa62a1b5bad9ccbc91ef6bd54c18fe4472ba",
    "c7060def1e8e4a1772a95336b15bfc13cb2377f6cf24fdf2722d3a28494cf76f",
    "9ef57ad1f27c75c78acaa015d2b5db5f550a37f714ab38da316be0fe47214df6",
    "3735d99ba8b0262a3d7e1aa955ca4c256a718f2f95a3a253d978ef84a39bdcd8",
    "53c2e61c72e5261f8a5031e3742bce757f0ccbfccea0e358adf37d4a766ec091",
    "c9d740fae57a1314a0dee22ab1560cf64527c558c35ade4cb197edb71619e6b6",
    "34d6512468bbd17072fc9addfa22b6c29bb5e07f32cdedfc5e5a9c923d30510e",
));
const RSA3072_PSS_SALTMAX: [u8; 384] = hex_array(concat!(
    "3e30d7a4a6bfcaba1c772deaae1f237fdcfce660412277fffac6ed385f8bbbad",
    "1f86e8a16b23bf7c1a9c08ed1be11dc04953d6485ef4cd1ac3026a80334e0449",
    "ad93ce86e7ebd4e2666f20e51e82045a14a0e07bf645c8698d6a71d6d4753ff5",
    "09ec5783b17c279ae77d6f1b67f81d5cb560921c82a78e76ed15c483e5e5de9c",
    "92da80d1a72ac04254e80bb8c488aa75971e9e77708f942c0473345bed316703",
    "838760fac5728abe694149978c8d2315b07ff982874bf61c3497db9f754640e1",
    "ecb400157ab5038c1363eef26f8f051853383a4f06f4b954d6dbb79a4fcb0e6e",
    "5cfbc00474266a31c5ae580ef77c0145d17394343cd844dfe9d7a406a83283da",
    "669719ee0f1b6eac0205b607192f65f7ff9315ea6ed2bdfc1eca03a52cf93ff8",
    "ea1b0b1acc1be00a29bf9043a74c30c49ccf8d52cbfb7cfa798c39e52be9392d",
    "ce214c329cdff5fe7249c6981fd9b9931d6e2902acdb73857abb7b67be734f7d",
    "86d2ddb5e0bf9d7188382b9772ce8bcfa315b9ab20b5bf1b3733e1717a78c7ef",
));
const RSA3072_PSS_OTHER_MESSAGE: [u8; 384] = hex_array(concat!(
    "3cb85552054a0f12a7c177fde3ca56b540ea64ad40f677bb2f86c6b5e3408fb2",
    "5b1acf02eae28266514bdc2f5646789051704ea4421141d96fd1d3f7b637f560",
    "196849f4d81dc030995d0e95fc9b176094eda2d03dc4d4a60a091a1f687fa1c1",
    "b00edd6edd0fdb11a139ded0a58afe7eacc0db6ec6a3aea522b5395e5febfce4",
    "d3e210203df764771b57237ca93ae12df985fd2932417ab1b8ccc64a11d6a715",
    "1d667809439ecea631f7271da8cfda3d4bbc3f1db1aa20fae06dc2b612defeda",
    "dd4c3852ce010e1e83c471b25d4390c335c7159db3f2c1871ae8c042d366c4a0",
    "77d088578d63be0b88efee7b88ecb85def36485c23f27db7279b71b3cbe9a5bd",
    "bb9a39ed423541add8bdfdbba5882fbe8e3cc81c46edb3aca33cb99ee52bf81d",
    "93ad3eb81a57dd2a0fd779c4955b8c468e72791036700b2576f711e34190b89a",
    "f9465223c53f85b7ed1305bdd360ffe03c71011a04c4481d28feda41d8346ab4",
    "a7507937ba7288b1b47b7000875454312d8946945fa385c93d7064950938703a",
));
