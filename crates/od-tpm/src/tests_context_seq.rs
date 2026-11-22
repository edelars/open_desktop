//! Context of sequence objects (S7-23, slice 8): ContextSave and ContextLoad of hash, sign and
//! verify sequences (RSA, ECC and ML-DSA keys), the context gap of session contexts
//! (`TPM_RC_CONTEXT_GAP`, `TPM_PT_CONTEXT_GAP_MAX`) and the unchanged context bytes of sessions and
//! objects.
//!
//! `sha256_golden_unchanged` was recorded at HEAD c0860cc, before this slice changed any code: the
//! TPM is deterministic (fixed entropy), so the SHA-256 of one transcript of the session and object
//! context paths pins their bytes. Every expected value of a hash or a signature comes from Python
//! `hashlib` or `openssl`, not from this implementation (the comments name the script).

use sha2::{Digest, Sha256};

use super::tests_b::{shutdown, start_session, startup};
use super::tests_obj::{OWNER, create_primary, ecc_sign, flush, parse_primary, rsa_sign, sign};
use super::*;
use crate::codes::{se, su};
use crate::hash::Sequence;

fn hex_of(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

const NULL: u32 = handle::RH_NULL;

fn save(t: &mut Tpm, h: u32) -> Vec<u8> {
    run(t, plain(cc::CONTEXT_SAVE, &[h], &[]))
}

fn load_ctx(t: &mut Tpm, ctx: &[u8]) -> Vec<u8> {
    run(t, plain(cc::CONTEXT_LOAD, &[], ctx))
}

/// The replies of one scenario over the session and object context paths of HEAD c0860cc.
fn golden_transcript() -> Vec<u8> {
    let mut t = started();
    let mut log: Vec<u8> = Vec::new();
    let mut put = |reply: &[u8]| {
        log.extend_from_slice(&(reply.len() as u32).to_be_bytes());
        log.extend_from_slice(reply);
    };
    let ecdsa = cat(&[&u16b(alg::ECDSA), &u16b(alg::SHA256)]);
    let digest = Sha256::digest(b"context golden");
    // Objects: save, flush, load, sign, a second save, and the refusals of a corrupt context.
    for template in [ecc_sign(), rsa_sign()] {
        let created = create_primary(&mut t, OWNER, b"pw", &template);
        put(&created);
        let key = parse_primary(&created);
        let context = save(&mut t, key.handle);
        put(&context);
        put(&save(&mut t, key.handle));
        put(&flush(&mut t, key.handle));
        let ctx = params(&context, 0).to_vec();
        let loaded = load_ctx(&mut t, &ctx);
        put(&loaded);
        let handle = u32::from_be_bytes(loaded[10..14].try_into().unwrap());
        if template.type_alg() == alg::ECC {
            put(&sign(&mut t, handle, b"pw", &digest, &ecdsa));
        }
        let mut bad = ctx.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        put(&load_ctx(&mut t, &bad));
        let mut bad = ctx.clone();
        bad[7] ^= 1;
        put(&load_ctx(&mut t, &bad));
        let mut bad = ctx.clone();
        bad[8..12].copy_from_slice(&0x8000_0002u32.to_be_bytes());
        put(&load_ctx(&mut t, &bad));
        let mut bad = ctx.clone();
        bad[12..16].copy_from_slice(&handle::RH_OWNER.to_be_bytes());
        put(&load_ctx(&mut t, &bad));
        put(&flush(&mut t, handle));
    }
    // Sessions: HMAC, policy and trial; replay, supersede and flush.
    for kind in [se::HMAC, se::POLICY, se::TRIAL] {
        let s = start_session(&mut t, kind, NULL, b"", &[0x44; 16]);
        put(&s.nonce_tpm);
        let first = save(&mut t, s.handle);
        put(&first);
        let first = params(&first, 0).to_vec();
        put(&load_ctx(&mut t, &first));
        put(&load_ctx(&mut t, &first));
        let second = save(&mut t, s.handle);
        put(&second);
        put(&load_ctx(&mut t, params(&second, 0)));
        let third = save(&mut t, s.handle);
        put(&third);
        put(&load_ctx(&mut t, params(&second, 0)));
        put(&load_ctx(&mut t, params(&third, 0)));
        let fourth = save(&mut t, s.handle);
        put(&fourth);
        put(&flush(&mut t, s.handle));
        put(&load_ctx(&mut t, params(&fourth, 0)));
    }
    // An event sequence cannot be saved; an unknown handle neither.
    let event = run(
        &mut t,
        plain(
            cc::HASH_SEQUENCE_START,
            &[],
            &cat(&[&tpm2b(b""), &u16b(alg::NULL)]),
        ),
    );
    put(&event);
    put(&save(&mut t, 0x8000_0000));
    put(&save(&mut t, 0x8100_0000));
    // Sessions saved across Shutdown(STATE) and Startup(STATE), then Startup(CLEAR).
    let s = start_session(&mut t, se::HMAC, NULL, b"", &[0x55; 16]);
    let saved = save(&mut t, s.handle);
    put(&saved);
    shutdown(&mut t, su::STATE);
    t.init();
    startup(&mut t, su::STATE);
    put(&load_ctx(&mut t, params(&saved, 0)));
    put(&save(&mut t, s.handle));
    shutdown(&mut t, su::STATE);
    t.init();
    startup(&mut t, su::CLEAR);
    put(&load_ctx(&mut t, params(&saved, 0)));
    put(&t.persistent_snapshot());
    log
}

#[test]
fn sha256_golden_unchanged() {
    let digest = Sha256::digest(golden_transcript());
    let hexed: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(
        hexed, "969ea6a10dd7d46fdba595dc7beca502b2a2d02a9e9991bcf1affbe049a2773b",
        "golden transcript digest"
    );
}

// ------------------------------------------------------------------------------- fixtures
//
// Where a value comes from outside this implementation the comment says so. Tools: Python 3
// "hashlib" and "hmac" (scratchpad script ctxseq_vectors.py) and the signatures pinned by the
// earlier slices, which OpenSSL verified ("openssl dgst -sha384 -verify" and "pkeyutl -verify" for the
// RSA and ECC keys of tests_sha2_sign.rs; "openssl pkeyutl -sign -pkeyopt deterministic:1" and the
// OpenSSL-made signature of the external ML-DSA key for tests_mldsa.rs).

/// The message of the hash sequence tests: 300 octets, octet i is (7 i + 3) mod 256.
fn message() -> Vec<u8> {
    (0..300).map(|i| ((7 * i + 3) % 256) as u8).collect()
}

// Python hashlib over `message()`.
const MESSAGE_SHA256: &str = "04773f8726c81cafcfa1a09a82664b98b00d2021031a1715bca1154f2dad3472";
const MESSAGE_SHA384: &str = "060eb8e1c01cc6c6c8c03fcb7898155f2b836fb97c9d331a7a296a1a209aac9d261b2b268c4ba3c7299a3dd12b5bf81f";
const MESSAGE_SHA512: &str = "46e56ad30db9ef50f8b6762ba55839737f3fba34ab47863c9daff7b3f58f97fe3465a52dd364560db47f802909ced49093322621ea0aebf8e0696b85ca8f81f0";

/// The digest and the hierarchy of the ticket of a SequenceComplete reply (the digest of SHA-384 and
/// SHA-512 is longer than the PCR digest limit of the shared helper).
fn digest_and_hierarchy(resp: &[u8]) -> (Vec<u8>, u32) {
    let mut rd = Reader::new(params(resp, 0));
    let digest = rd.tpm2b(limits::MAX_HASH_SIZE).unwrap().to_vec();
    let ticket = TpmtTkHashcheck::unmarshal(&mut rd).unwrap();
    rd.finish().unwrap();
    (digest, ticket.hierarchy)
}

fn handle_of(resp: &[u8]) -> u32 {
    ok(resp);
    u32::from_be_bytes(resp[10..14].try_into().unwrap())
}

/// `TPMS_CONTEXT` fields of a `TPM2_ContextSave` response: `(sequence, savedHandle, hierarchy)`.
fn fields(context: &[u8]) -> (u64, u32, u32) {
    (
        u64::from_be_bytes(context[..8].try_into().unwrap()),
        u32::from_be_bytes(context[8..12].try_into().unwrap()),
        u32::from_be_bytes(context[12..16].try_into().unwrap()),
    )
}

/// Saves the context of `h` (the response must be a success) and returns the `TPMS_CONTEXT`.
fn context_of(t: &mut Tpm, h: u32) -> Vec<u8> {
    let saved = save(t, h);
    ok(&saved);
    params(&saved, 0).to_vec()
}

fn start_sign(t: &mut Tpm, key: u32, auth: &[u8], context: &[u8]) -> u32 {
    handle_of(&run(
        t,
        plain(
            cc::SIGN_SEQUENCE_START,
            &[key],
            &cat(&[&tpm2b(auth), &tpm2b(context)]),
        ),
    ))
}

fn start_verify(t: &mut Tpm, key: u32, auth: &[u8], context: &[u8]) -> u32 {
    handle_of(&run(
        t,
        plain(
            cc::VERIFY_SEQUENCE_START,
            &[key],
            &cat(&[&tpm2b(auth), &tpm2b(&[]), &tpm2b(context)]),
        ),
    ))
}

fn complete_sign(t: &mut Tpm, sequence: u32, key: u32, auth: &[u8], last: &[u8]) -> Vec<u8> {
    run(
        t,
        with_pw(
            cc::SIGN_SEQUENCE_COMPLETE,
            &[sequence, key],
            &[auth, b"pw"],
            &tpm2b(last),
        ),
    )
}

fn complete_verify(t: &mut Tpm, sequence: u32, key: u32, auth: &[u8], signature: &[u8]) -> Vec<u8> {
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

// ------------------------------------------------------------------------------------- tests

#[test]
fn hash_sequence_context_save_load_continues_identically() {
    let msg = message();
    for (hash, expected) in [
        (alg::SHA256, MESSAGE_SHA256),
        (alg::SHA384, MESSAGE_SHA384),
        (alg::SHA512, MESSAGE_SHA512),
    ] {
        let mut t = started();
        let h = seq_handle(&start_seq(&mut t, b"seq-pw", hash));
        // 130 octets: two blocks of SHA-256 and a partial block of every hash are buffered.
        ok(&seq_update(&mut t, h, b"seq-pw", &msg[..100]));
        ok(&seq_update(&mut t, h, b"seq-pw", &msg[100..130]));
        let context = context_of(&mut t, h);
        // sequence 1 of the object counter, savedHandle 0x80000001, no hierarchy.
        assert_eq!(fields(&context), (1, 0x8000_0001, NULL));
        // The sequence stays loaded after the save; the message and the authValue are not in the
        // clear in the context.
        ok(&seq_update(&mut t, h, b"seq-pw", b""));
        assert!(!super::tests_obj::contains(&context, &msg[..40]));
        assert!(!super::tests_obj::contains(&context, b"seq-pw"));
        // Flush, load: the same slot and handle, the same state.
        ok(&flush(&mut t, h));
        err(
            &seq_update(&mut t, h, b"seq-pw", b"x"),
            Rc::reference_handle(0),
        );
        let loaded = handle_of(&load_ctx(&mut t, &context));
        assert_eq!(loaded, h);
        // The authValue of the sequence came back with it.
        err(
            &seq_update(&mut t, loaded, b"wrong", b"x"),
            Rc::BAD_AUTH.in_session(1),
        );
        ok(&seq_update(&mut t, loaded, b"seq-pw", &msg[130..230]));
        let done = seq_complete(&mut t, loaded, b"seq-pw", &msg[230..], NULL);
        let (digest, ticket) = digest_and_hierarchy(&done);
        assert_eq!(
            hex_of(&digest),
            expected,
            "SHA-{hash:#06x} after save and load"
        );
        assert_eq!(ticket, NULL);
        // The context loads again (as an object context does) and gives the state of the save: the
        // rest of the message from octet 130.
        let again = handle_of(&load_ctx(&mut t, &context));
        let done = seq_complete(&mut t, again, b"seq-pw", &msg[130..], NULL);
        assert_eq!(hex_of(&digest_and_hierarchy(&done).0), expected);
        // Two copies at once (the original is still loaded), each takes a slot of its own.
        let first = seq_handle(&start_seq(&mut t, b"o", hash));
        ok(&seq_update(&mut t, first, b"o", &msg[..77]));
        let ctx = context_of(&mut t, first);
        let copy = handle_of(&load_ctx(&mut t, &ctx));
        assert_ne!(copy, first);
        for h in [first, copy] {
            let done = seq_complete(&mut t, h, b"o", &msg[77..], NULL);
            assert_eq!(hex_of(&digest_and_hierarchy(&done).0), expected);
        }
    }
}

#[test]
fn hash_sequence_context_keeps_the_hashcheck_ticket_rule() {
    // The first buffer decides whether a ticket may be issued (Part 3 v185 17.8.1): a buffer that
    // starts with TPM_GENERATED_VALUE gives no ticket, also across a save and a load.
    let mut t = started();
    for (first, ticket_expected) in [
        (&b"abcd"[..], true),
        (&[0xff, b'T', b'C', b'G', 1][..], false),
    ] {
        let h = seq_handle(&start_seq(&mut t, b"p", alg::SHA256));
        ok(&seq_update(&mut t, h, b"p", first));
        let ctx = context_of(&mut t, h);
        ok(&flush(&mut t, h));
        let h = handle_of(&load_ctx(&mut t, &ctx));
        let done = seq_complete(&mut t, h, b"p", b"tail", handle::RH_OWNER);
        let (_, ticket) = digest_and_hierarchy(&done);
        assert_eq!(ticket == handle::RH_OWNER, ticket_expected);
    }
}

#[test]
fn sign_sequence_context_save_load_verifies_with_openssl() {
    // The signatures are the ones OpenSSL verified over MESSAGE ("openssl dgst -sha384|-sha512 -verify",
    // tests_sha2_sign.rs): RSASSA-2048 and ECDSA P-256 are deterministic, so a sequence that is saved
    // and loaded in the middle of the message must give the pinned bytes.
    for (rsa_key, hash) in [
        (true, alg::SHA384),
        (false, alg::SHA384),
        (true, alg::SHA512),
        (false, alg::SHA512),
    ] {
        let mut t = started();
        let key = super::tests_sha2_sign::pinned_key(&mut t, rsa_key, hash);
        let expected = super::tests_sha2_sign::pinned_signature(rsa_key, hash);
        let message = super::tests_sha2_sign::MESSAGE;
        let sequence = start_sign(&mut t, key.handle, b"seq", b"");
        ok(&seq_update(&mut t, sequence, b"seq", &message[..7]));
        let context = context_of(&mut t, sequence);
        assert_eq!(fields(&context), (1, 0x8000_0001, NULL));
        ok(&seq_update(&mut t, sequence, b"seq", &message[7..12]));
        // The original keeps going and signs the whole message.
        let whole = complete_sign(&mut t, sequence, key.handle, b"seq", &message[12..]);
        ok(&whole);
        assert_eq!(hex_of(params(&whole, 0)), expected);
        // The context (octets 0..7) continues in a fresh slot to the same signature.
        let loaded = handle_of(&load_ctx(&mut t, &context));
        ok(&seq_update(&mut t, loaded, b"seq", &message[7..20]));
        let signed = complete_sign(&mut t, loaded, key.handle, b"seq", &message[20..]);
        ok(&signed);
        assert_eq!(hex_of(params(&signed, 0)), expected, "{rsa_key} {hash:#x}");
        // The signing key is bound by Name: another key is TPM_RC_SIGN_CONTEXT_KEY after a load.
        let other = parse_primary(&create_primary(&mut t, OWNER, b"pw", &ecc_sign()));
        let loaded = handle_of(&load_ctx(&mut t, &context));
        err(
            &complete_sign(&mut t, loaded, other.handle, b"seq", b"x"),
            Rc::SIGN_CONTEXT_KEY.in_handle(2),
        );
        // The authValue of the sequence is the one of the start command.
        err(
            &complete_sign(&mut t, loaded, key.handle, b"other", b"x"),
            Rc::BAD_AUTH.in_session(1),
        );
        // Flush and load: handle reuse, the digest still the pinned one.
        ok(&flush(&mut t, loaded));
        let loaded = handle_of(&load_ctx(&mut t, &context));
        ok(&seq_update(&mut t, loaded, b"seq", &message[7..]));
        let signed = complete_sign(&mut t, loaded, key.handle, b"seq", b"");
        assert_eq!(hex_of(params(&signed, 0)), expected);
    }
}

#[test]
fn sign_sequence_context_keeps_the_generated_prefix_check() {
    // A restricted key refuses a message that starts with TPM_GENERATED_VALUE (TPM_RC_VALUE): the
    // four octets arrive before the save, the rest after the load.
    let mut t = started();
    let mut template = rsa_sign();
    template.attributes |= crate::codes::object_attr::RESTRICTED;
    let key = parse_primary(&create_primary(&mut t, OWNER, b"pw", &template));
    let sequence = start_sign(&mut t, key.handle, b"seq", b"");
    ok(&seq_update(&mut t, sequence, b"seq", &[0xff, b'T']));
    let context = context_of(&mut t, sequence);
    ok(&flush(&mut t, sequence));
    let loaded = handle_of(&load_ctx(&mut t, &context));
    ok(&seq_update(&mut t, loaded, b"seq", b"C"));
    err(
        &complete_sign(&mut t, loaded, key.handle, b"seq", b"G"),
        Rc::VALUE.in_param(1),
    );
}

#[test]
fn verify_sequence_context_save_load() {
    // The signatures are OpenSSL-verified (see above). A verify sequence of a key under the owner
    // hierarchy also carries the ticket accumulators: the ticket that comes after a save and a load is
    // byte for byte the one of a sequence that was never saved.
    for (rsa_key, hash) in [
        (true, alg::SHA384),
        (false, alg::SHA384),
        (false, alg::SHA512),
    ] {
        let mut t = started();
        let key = super::tests_sha2_sign::pinned_key(&mut t, rsa_key, hash);
        let signature = hex(super::tests_sha2_sign::pinned_signature(rsa_key, hash));
        let message = super::tests_sha2_sign::MESSAGE;
        // Without a save.
        let plain_run = start_verify(&mut t, key.handle, b"vseq", b"");
        ok(&seq_update(&mut t, plain_run, b"vseq", &message[..9]));
        ok(&seq_update(&mut t, plain_run, b"vseq", &message[9..]));
        let reference = complete_verify(&mut t, plain_run, key.handle, b"vseq", &signature);
        ok(&reference);
        assert_eq!(
            params(&reference, 0)[..2],
            [0x80, 0x26],
            "TPM_ST_MESSAGE_VERIFIED"
        );
        assert_eq!(params(&reference, 0).len(), 2 + 4 + 2 + 32);
        // With a save and a load in the middle.
        let sequence = start_verify(&mut t, key.handle, b"vseq", b"");
        ok(&seq_update(&mut t, sequence, b"vseq", &message[..9]));
        let context = context_of(&mut t, sequence);
        assert_eq!(fields(&context).1, 0x8000_0001);
        ok(&flush(&mut t, sequence));
        let loaded = handle_of(&load_ctx(&mut t, &context));
        // A wrong message after the load is a bad signature, and the sequence stays.
        err(
            &complete_verify(&mut t, loaded, key.handle, b"vseq", &signature),
            Rc::SIGNATURE,
        );
        ok(&seq_update(&mut t, loaded, b"vseq", &message[9..]));
        let verified = complete_verify(&mut t, loaded, key.handle, b"vseq", &signature);
        ok(&verified);
        assert_eq!(verified, reference, "ticket after save and load");
        // A flipped signature and another key are refused.
        let loaded = handle_of(&load_ctx(&mut t, &context));
        ok(&seq_update(&mut t, loaded, b"vseq", &message[9..]));
        let mut bad = signature.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        err(
            &complete_verify(&mut t, loaded, key.handle, b"vseq", &bad),
            Rc::SIGNATURE,
        );
        let other = parse_primary(&create_primary(&mut t, OWNER, b"pw", &ecc_sign()));
        err(
            &complete_verify(&mut t, loaded, other.handle, b"vseq", &signature),
            Rc::SIGN_CONTEXT_KEY.in_handle(2),
        );
        ok(&flush(&mut t, loaded));
    }
}

#[test]
fn verify_sequence_context_never_holds_the_hierarchy_proof() {
    let mut t = started();
    let key = super::tests_sha2_sign::pinned_key(&mut t, false, alg::SHA384);
    let message = super::tests_sha2_sign::MESSAGE;
    let sequence = start_verify(&mut t, key.handle, b"vseq", b"");
    ok(&seq_update(&mut t, sequence, b"vseq", &message[..9]));
    // The plaintext body, before it is encrypted: it carries the proof identifiers (the digest of the
    // proof) and the inner hashes, and neither a proof nor a key block made of it.
    let slot = t.seq_slot(sequence, 0).unwrap();
    let body = t
        .sequences
        .get(slot)
        .and_then(Sequence::context_body)
        .unwrap();
    let contains = |needle: &[u8]| body.windows(needle.len()).any(|w| w == needle);
    for hierarchy in [
        handle::RH_PLATFORM,
        handle::RH_OWNER,
        handle::RH_ENDORSEMENT,
    ] {
        let proof = t.proof(hierarchy).unwrap();
        assert!(!contains(&proof[..]), "proof of {hierarchy:#x} in the body");
        assert!(!contains(&proof[..16]), "a part of the proof in the body");
        let id: [u8; 32] = Sha256::digest(&proof[..]).into();
        assert!(contains(&id), "proof identifier of {hierarchy:#x}");
    }
    // The saved context loads while the proof is the same, as many times as wanted.
    let context = context_of(&mut t, sequence);
    ok(&flush(&mut t, sequence));
    let loaded = handle_of(&load_ctx(&mut t, &context));
    ok(&flush(&mut t, loaded));
    // After the proof of the owner hierarchy changes the prefix cannot be authenticated any more:
    // the load is refused as TPM_RC_INTEGRITY (parameter 1) and the TPM stays sound.
    t.persistent.owner_seed[0] ^= 1;
    err(&load_ctx(&mut t, &context), Rc::INTEGRITY.in_param(1));
    assert!(!t.failed);
}

#[test]
fn mldsa_sign_sequence_context_save_load() {
    use super::tests_mldsa::{
        EXT_MESSAGE, EXT_NAME, EXT_SIG_MSG, MESSAGE as MLDSA_MESSAGE, SIG_CTX_SHA256,
        load_external_key, sha256_hex, sig_octets, signature_bytes, tpm_key,
    };
    // Sign: the message and the context "ctx-1" are buffered, saved, loaded and finished; the
    // signature is the pinned one (SIG_CTX_SHA256: "openssl pkeyutl -sign -pkeyopt deterministic:1
    // -pkeyopt context-string:ctx-1", reproduced byte for byte, see tests_mldsa.rs).
    let (mut t, key) = tpm_key();
    let sequence = start_sign(&mut t, key.handle, b"seq", b"ctx-1");
    ok(&seq_update(&mut t, sequence, b"seq", &MLDSA_MESSAGE[..10]));
    let context = context_of(&mut t, sequence);
    assert_eq!(fields(&context), (1, 0x8000_0001, NULL));
    // The buffered message and the context string are not in the clear.
    assert!(!super::tests_obj::contains(&context, &MLDSA_MESSAGE[..10]));
    assert!(!super::tests_obj::contains(&context, b"ctx-1"));
    ok(&flush(&mut t, sequence));
    let loaded = handle_of(&load_ctx(&mut t, &context));
    ok(&seq_update(&mut t, loaded, b"seq", &MLDSA_MESSAGE[10..12]));
    let signed = complete_sign(&mut t, loaded, key.handle, b"seq", &MLDSA_MESSAGE[12..]);
    let octets = sig_octets(&signed);
    assert_eq!(sha256_hex(&octets), SIG_CTX_SHA256);
    // The same context finishes a different message to a different signature (the buffer is the
    // saved one, not a fixed one).
    let loaded = handle_of(&load_ctx(&mut t, &context));
    let other = complete_sign(&mut t, loaded, key.handle, b"seq", b"another tail");
    assert_ne!(sig_octets(&other), octets);
    // Verify with the OpenSSL-made signature of the external key (message and context "ctx-1").
    let mut t = started();
    let (external, name) = load_external_key(&mut t, true);
    assert_eq!(hex_of(&name), EXT_NAME);
    let signature = signature_bytes(EXT_SIG_MSG);
    let sequence = start_verify(&mut t, external, b"vseq", b"ctx-1");
    ok(&seq_update(&mut t, sequence, b"vseq", &EXT_MESSAGE[..5]));
    let context = context_of(&mut t, sequence);
    ok(&flush(&mut t, sequence));
    let loaded = handle_of(&load_ctx(&mut t, &context));
    ok(&seq_update(&mut t, loaded, b"vseq", &EXT_MESSAGE[5..]));
    let verified = complete_verify(&mut t, loaded, external, b"vseq", &signature);
    ok(&verified);
    assert_eq!(
        params(&verified, 0)[..2],
        [0x80, 0x26],
        "TPM_ST_MESSAGE_VERIFIED"
    );
    // A saved sequence keeps its context string: a message under another one does not verify.
    let sequence = start_verify(&mut t, external, b"vseq", b"ctx-2");
    ok(&seq_update(&mut t, sequence, b"vseq", EXT_MESSAGE));
    let context = context_of(&mut t, sequence);
    ok(&flush(&mut t, sequence));
    let loaded = handle_of(&load_ctx(&mut t, &context));
    err(
        &complete_verify(&mut t, loaded, external, b"vseq", &signature),
        Rc::SIGNATURE,
    );
}

#[test]
fn mldsa_sequence_context_has_the_size_of_the_largest_blob() {
    // The saved body of a sign sequence of an ML-DSA key with an empty context and the authValue
    // "seq" is 59 octets and the message: the blob (integrity TPM2B 34, size 2, inner sequence 8) must
    // fit MAX_CONTEXT_BLOB = 4096, so 3993 octets of message save and load, 3994 are TPM_RC_SIZE for
    // the handle, and the refusal changes nothing (the counter does not move).
    use super::tests_mldsa::{sig_octets, tpm_key};
    let (mut t, key) = tpm_key();
    let fill = |t: &mut Tpm, sequence: u32, n: usize| {
        let mut left = n;
        while left > 0 {
            let chunk = left.min(1000);
            ok(&seq_update(t, sequence, b"seq", &vec![0x5a; chunk]));
            left -= chunk;
        }
    };
    let fits = start_sign(&mut t, key.handle, b"seq", b"");
    fill(&mut t, fits, 3993);
    let context = context_of(&mut t, fits);
    assert_eq!(fields(&context).0, 1);
    // blobSize is the last field before the blob: 2 octets of size at 16..18.
    assert_eq!(u16::from_be_bytes([context[16], context[17]]), 4096);
    let one_shot = sig_octets(&complete_sign(&mut t, fits, key.handle, b"seq", b""));
    let loaded = handle_of(&load_ctx(&mut t, &context));
    let after = sig_octets(&complete_sign(&mut t, loaded, key.handle, b"seq", b""));
    assert_eq!(after, one_shot);
    let too_big = start_sign(&mut t, key.handle, b"seq", b"");
    fill(&mut t, too_big, 3994);
    err(&save(&mut t, too_big), Rc::SIZE.in_handle(1));
    // The counter did not move: the next save is sequence 2.
    ok(&seq_update(&mut t, too_big, b"seq", b""));
    ok(&flush(&mut t, too_big));
    let next = start_sign(&mut t, key.handle, b"seq", b"");
    assert_eq!(fields(&context_of(&mut t, next)).0, 2);
}

#[test]
fn sequence_context_refusals() {
    let msg = message();
    let mut t = started();
    let h = seq_handle(&start_seq(&mut t, b"p", alg::SHA256));
    ok(&seq_update(&mut t, h, b"p", &msg[..50]));
    let context = context_of(&mut t, h);
    // A corrupt context: the integrity value fails, whatever octet changed.
    for at in [context.len() - 1, 20, 40] {
        let mut bad = context.clone();
        bad[at] ^= 1;
        err(&load_ctx(&mut t, &bad), Rc::INTEGRITY.in_param(1));
    }
    // The savedHandle and the sequence are part of the integrity value: an object or an stClear
    // object cannot be made of it, and a sequence number beyond the counter is TPM_RC_VALUE.
    for saved in [0x8000_0000u32, 0x8000_0002] {
        let mut bad = context.clone();
        bad[8..12].copy_from_slice(&saved.to_be_bytes());
        err(&load_ctx(&mut t, &bad), Rc::INTEGRITY.in_param(1));
    }
    let mut bad = context.clone();
    bad[7] = 2;
    err(&load_ctx(&mut t, &bad), Rc::VALUE.in_param(1));
    // An unknown saved handle and a hierarchy other than TPM_RH_NULL are TPM_RC_VALUE.
    for saved in [0x8000_0003u32, 0x8000_00ff, 0x8100_0000, 0x0000_0001] {
        let mut bad = context.clone();
        bad[8..12].copy_from_slice(&saved.to_be_bytes());
        err(&load_ctx(&mut t, &bad), Rc::VALUE.in_param(1));
    }
    let mut bad = context.clone();
    bad[12..16].copy_from_slice(&handle::RH_OWNER.to_be_bytes());
    err(&load_ctx(&mut t, &bad), Rc::VALUE.in_param(1));
    // A context that is not a whole TPMS_CONTEXT.
    err(
        &load_ctx(&mut t, &context[..context.len() - 1]),
        Rc::INSUFFICIENT.in_param(1),
    );
    // No free sequence slot: TPM_RC_OBJECT_MEMORY, a warning that changes nothing.
    let mut held = vec![h];
    while t.sequences.free_slots() > 0 {
        held.push(seq_handle(&start_seq(&mut t, b"p", alg::SHA256)));
    }
    err(&load_ctx(&mut t, &context), Rc::OBJECT_MEMORY);
    ok(&flush(&mut t, held.pop().unwrap()));
    ok(&load_ctx(&mut t, &context));
    // The HMAC and event sequences are not saved (TPM_RC_VALUE, handle 1); nor is a handle that is
    // not loaded (TPM_RC_REFERENCE_H0).
    let (mut t, parent) = super::tests_obj::with_ecc_parent();
    let event = seq_handle(&start_seq(&mut t, b"", alg::NULL));
    err(&save(&mut t, event), Rc::VALUE.in_handle(1));
    let secret: Vec<u8> = (1..=32).collect();
    let (key, _, _) = super::tests_c2::make_child(
        &mut t,
        parent,
        &super::tests_c2::hmac_key(super::tests_c2::HMAC_SHA256, false),
        b"key",
        &secret,
    );
    let hmac_sequence = super::tests_c2::hmac_start(&mut t, key, b"key", b"seq", alg::NULL);
    err(
        &save(&mut t, handle_of(&hmac_sequence)),
        Rc::VALUE.in_handle(1),
    );
    err(&save(&mut t, 0x8000_0002), Rc::reference_handle(0));
    // A TPM Reset (and Restart: no) ends a sequence context: the sequence number is beyond the
    // counter of the new epoch (TPM_RC_VALUE), and once the counter has caught up the integrity
    // value of the other epoch does not match.
    let mut t = started();
    let h = seq_handle(&start_seq(&mut t, b"p", alg::SHA256));
    let context = context_of(&mut t, h);
    shutdown(&mut t, su::CLEAR);
    t.init();
    startup(&mut t, su::CLEAR);
    err(&load_ctx(&mut t, &context), Rc::VALUE.in_param(1));
    let h = seq_handle(&start_seq(&mut t, b"p", alg::SHA256));
    let fresh = context_of(&mut t, h);
    assert_eq!(fields(&fresh).0, 1);
    err(&load_ctx(&mut t, &context), Rc::INTEGRITY.in_param(1));
    ok(&load_ctx(&mut t, &fresh));
}

#[test]
fn hmac_sha256_stream_matches_rfc_4231_and_restores() {
    use crate::crypto::HmacSha256Stream;
    use zeroize::Zeroizing;
    // RFC 4231, test cases 2 and 6 (key longer than the block), also computed with Python "hmac".
    let cases: [(&[u8], &[u8], &str); 2] = [
        (
            b"Jefe",
            b"what do ya want for nothing?",
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
        ),
        (
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First",
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54",
        ),
    ];
    for (key, data, expected) in cases {
        let mut stream = HmacSha256Stream::new(key);
        stream.update(&data[..5]);
        // The saved parts (key block, inner hash) give a stream that finishes the same tag.
        let mut restored = HmacSha256Stream::from_parts(
            Zeroizing::new(*stream.key_block()),
            stream.inner().clone(),
            false,
        );
        stream.update(&data[5..]);
        restored.update(&data[5..]);
        assert_eq!(hex_of(&stream.finalize()), expected);
        assert_eq!(hex_of(&restored.finalize()), expected);
    }
}

// ------------------------------------------------------------------------ the context gap

fn gap_max(t: &mut Tpm) -> u32 {
    let reply = get_cap(t, cap::TPM_PROPERTIES, pt::CONTEXT_GAP_MAX, 1);
    let (_, _, body, count) = cap_list(&reply);
    assert_eq!(count, 1);
    assert_eq!(body[..4], pt::CONTEXT_GAP_MAX.to_be_bytes());
    u32::from_be_bytes(body[4..8].try_into().unwrap())
}

fn hmac_session(t: &mut Tpm, nonce: u8) -> u32 {
    start_session(t, se::HMAC, NULL, b"", &[nonce; 16]).handle
}

#[test]
fn context_gap_max_matches_window() {
    // The property is the window of the implementation, not 0, and the behaviour follows it to the
    // octet: with the oldest saved session context at contextID 1 a new one may be at most
    // 1 + CONTEXT_GAP_MAX, and the next save is TPM_RC_CONTEXT_GAP (at load a stale context is
    // TPM_RC_HANDLE, see the next test). 0xFFFF is the window of 16-bit
    // context slots of the reference implementation (NOT VERIFIED against Part 4, not available).
    let mut t = started();
    let window = u64::from(gap_max(&mut t));
    assert_eq!(window, 0xFFFF);
    let oldest = hmac_session(&mut t, 1);
    assert_eq!(fields(&context_of(&mut t, oldest)).0, 1);
    t.context.set_session_counter(window - 1);
    // Distances window - 1 and window: allowed; window + 1: refused.
    let ids: Vec<Option<u64>> = (2..5u8)
        .map(|nonce| {
            let s = hmac_session(&mut t, nonce);
            let saved = save(&mut t, s);
            (rc(&saved) == 0).then(|| fields(params(&saved, 0)).0)
        })
        .collect();
    assert_eq!(ids, vec![Some(window), Some(window + 1), None]);
}

#[test]
fn context_gap_refuses_gap_larger_than_window() {
    let mut t = started();
    let window = u64::from(gap_max(&mut t));
    let a = hmac_session(&mut t, 1);
    let context_a = context_of(&mut t, a);
    assert_eq!(fields(&context_a), (1, a, NULL));
    // The newest context may be exactly CONTEXT_GAP_MAX past the oldest: contextID 1 + window.
    let b = hmac_session(&mut t, 2);
    t.context.set_session_counter(window);
    let context_b = context_of(&mut t, b);
    assert_eq!(fields(&context_b).0, 1 + window);
    // One more is a gap larger than the window: TPM_RC_CONTEXT_GAP (a warning: no handle or
    // parameter number, the response is the 10-octet header), and nothing changes: the session stays
    // loaded and the counter does not move.
    let c = hmac_session(&mut t, 3);
    let refused = save(&mut t, c);
    err(&refused, Rc::CONTEXT_GAP);
    assert_eq!(rc(&refused), 0x0000_0901);
    err(&save(&mut t, c), Rc::CONTEXT_GAP);
    // Objects and sequences are not subject to the gap of session contexts.
    let primary = parse_primary(&create_primary(&mut t, OWNER, b"pw", &ecc_sign()));
    ok(&save(&mut t, primary.handle));
    let sequence = seq_handle(&start_seq(&mut t, b"p", alg::SHA256));
    ok(&save(&mut t, sequence));
    ok(&flush(&mut t, sequence));
    // The oldest context still loads (it is among the saved ones); then the gap is smaller and the
    // session can be saved: the counter was 1 + window, the load made it 2 + window and the save
    // takes the next contextID.
    assert_eq!(handle_of(&load_ctx(&mut t, &context_a)), a);
    assert_eq!(fields(&context_of(&mut t, c)).0, 3 + window);
    // The first context is loaded and gone from the record: its sequence is now older than the
    // counter by more than the window and not among the saved ones. TPM_RC_CONTEXT_GAP is returned
    // only at save, so the load is TPM_RC_HANDLE (parameter 1), as for a recent one.
    err(&load_ctx(&mut t, &context_a), Rc::HANDLE.in_param(1));
    // The context of b is among the saved ones: it loads at any distance, and its replay, a few
    // contexts behind the counter, is TPM_RC_HANDLE as before.
    assert_eq!(handle_of(&load_ctx(&mut t, &context_b)), b);
    err(&load_ctx(&mut t, &context_b), Rc::HANDLE.in_param(1));
    // A forged context of a far past is TPM_RC_HANDLE too, before any other check of the blob.
    let mut forged = context_a.clone();
    forged[..8].copy_from_slice(&0u64.to_be_bytes());
    err(&load_ctx(&mut t, &forged), Rc::HANDLE.in_param(1));
}

#[test]
fn context_save_load_within_window_unchanged() {
    // Contexts of sessions and objects as before slice 8, with a record of saved sessions that is
    // many contexts old but within the window.
    let mut t = started();
    let a = hmac_session(&mut t, 1);
    let context_a = context_of(&mut t, a);
    let b = hmac_session(&mut t, 2);
    let mut last = Vec::new();
    for round in 0..200u64 {
        let saved = context_of(&mut t, b);
        // Saves and loads count: save 2, load 3, save 4, ...
        assert_eq!(fields(&saved).0, 2 + 2 * round);
        assert_eq!(handle_of(&load_ctx(&mut t, &saved)), b);
        last = saved;
    }
    // A replay of the loaded context inside the window: TPM_RC_HANDLE, as before.
    err(&load_ctx(&mut t, &last), Rc::HANDLE.in_param(1));
    // The context of a, 400 contexts old, is among the saved ones and loads.
    assert_eq!(handle_of(&load_ctx(&mut t, &context_a)), a);
    err(&load_ctx(&mut t, &context_a), Rc::HANDLE.in_param(1));
    // FlushContext of a saved session forgets it; the context answers TPM_RC_HANDLE.
    let context_b = context_of(&mut t, b);
    ok(&flush(&mut t, b));
    err(&load_ctx(&mut t, &context_b), Rc::HANDLE.in_param(1));
    // Objects: any number of saves and loads, in any order, at any counter value.
    let key = parse_primary(&create_primary(&mut t, OWNER, b"pw", &ecc_sign()));
    let one = context_of(&mut t, key.handle);
    let two = context_of(&mut t, key.handle);
    assert_eq!((fields(&one).0, fields(&two).0), (1, 2));
    assert_eq!(fields(&one).1, 0x8000_0000);
    t.context.set_session_counter(u64::MAX / 2);
    ok(&load_ctx(&mut t, &two));
    ok(&load_ctx(&mut t, &one));
    ok(&load_ctx(&mut t, &two));
    // Sessions after a huge jump of the counter, with nothing saved: no gap to measure.
    let c = hmac_session(&mut t, 3);
    assert_eq!(fields(&context_of(&mut t, c)).0, u64::MAX / 2 + 1);
}
