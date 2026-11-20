//! Salted sessions and sessions bound to an object or an NV index (S7-23, slice 4):
//! `TPM2_StartAuthSession` with a `tpmKey` (RSA-OAEP with the label "SECRET" or ECDH with
//! KDFe "SECRET") and with a `bind` that is neither NULL nor a hierarchy.
//!
//! The `encryptedSalt` values are made outside the TPM: RSA with `openssl pkeyutl -encrypt
//! -pkeyopt rsa_padding_mode:oaep -pkeyopt rsa_oaep_md:sha256 -pkeyopt rsa_mgf1_md:sha256
//! -pkeyopt rsa_oaep_label:...` for the modulus of the TPM key, ECC with pure Python P-256
//! arithmetic and `hashlib` (ephemeral scalar 0x1234567890abcdef repeated four times, KDFe =
//! SHA-256(counter 1 || Z.x || "SECRET\0" || E.x || key.x)). The session keys, the command HMAC and
//! the response HMAC of the two pinned exchanges are Python `hmac`/`hashlib` results too
//! (2026-11-20, Part 1 clauses 17.6.5 and 17.6.10); the TPM is deterministic (fixed entropy), so
//! `nonceTPM` is pinned as well. The other tests use the client of `tests_b.rs`.

use sha2::{Digest, Sha256};

use super::tests_b::{
    Hc, Hs, define_raw, handles_of, hc, hmac_command, kdfa_256, nv_public, nv_read_public, pw_cmd,
    run_hmac, set_auth, split_response, start_cmd, start_hmac, start_params, trimmed,
};
use super::tests_obj::{
    base, create_child, create_primary, ecc_sign, ecc_storage, flush, load, load_external,
    parse_child, parse_loaded, parse_primary, rsa, rsa_sign, rsa_storage, sealed, sign_params,
    unmarshal_public,
};
use super::*;
use crate::codes::{object_attr as oa, se};
use crate::marshal::{MAX_RSA_BYTES, Scheme, SymDef, TpmtPublic, Unique};
use crate::nv::attr;

const OWNER: u32 = handle::RH_OWNER;
const LOCKOUT: u32 = handle::RH_LOCKOUT;
const NULL: u32 = handle::RH_NULL;

// ----------------------------------------------------------------------------- vectors

/// `nonceCaller` of the pinned exchanges.
const NONCE_CALLER: [u8; 16] = [0x33; 16];

/// RSA-2048 storage primary (restricted decrypt, AES-128-CFB), owner hierarchy, empty template data.
const RSA_SALT: &str = "404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f";
const RSA_ENC_SECRET: &str = "1c662949a239781228c291793ae1d9716411ac727a1ce6d85a4a789bc628002f664d5d470bddf62e60ab9d982c40a89dde91835ffb93e7b6710ea31e48ceb9764e9629551bdd263f63372391025b266b87af52dff4360d3b57efea993dbdb50f26fa4943e7c744858c84d2838d7ac13032e4eed4a59a8d3b1a75b51609e3ad531e93fc8fcf71b4aa5a85bd3984fb8ce23a8b491a834443f394b2e37994f2a7b10aaaccda46708bcd08c67784a06808cd2551aa1544d4d5ef6f2b326efba19e917065ea1375b0d818a533543850e458c4ce39111e69d7f55efd9e97d0c1af2856520c3701dafeef4b0a21971f6087c71f24a0860aeb468f6aaadbb5bef8e14e06";
const RSA_ENC_IDENTITY: &str = "251e173719fa4ef1ccbc48ce586b13286e4a03cddaee3c711cd3ebf323bf57240e9e1c6c3a1dfcd26ed617df598131370a526e60ebd0f35a6c1097278ef1058ca6979ae34d5cc5ac6de409019843637c6400b4091dcda19c06444c5858039255341cb484a55ec73b0df53e1199112621843d420cb4b236fb881dcf23a5c39b6af080a45e798384fccd2cdec5d227f5cce60307da24045136ae534f33f705694cbe743d46982fbaa6a641abe6424a993d90412d91f6e205ab5e0a02eab0d69f03d38d90b9bb9c9d815eb770c70af103fc66d1c4745b50859473f2e68222631617aa582dbcbeaf02b2556f7ed6cb41d7a52c3fa7eeb93350745c40b68223ef12a6";
const RSA_ENC_NO_NUL: &str = "a42c81d612a76507f28b884d6cbef9394fb36f570e48b205337cce2de903cc60aca991d40374f0c25128a530eba61ec17aaae05df4c2c10ae9391936b4c6d6cb903b6520ae1e503bd291fd8a183f35432c8e61c7bd4f2fe46ec7bdf47437f33039e935bad45e3880c8cbe47e6f7032309f8870e3b9697db2f1e210e52f60385d61e6c658d7d7d79325ca1d9a7379644673473ad0e8d598a5e5de41da5c7599913a956ec47e86b575ef4cfc278e07b28923adc21f15c633b3afe2594560c0e3e2e0888e6b1eed2c852b5e41b444dd62a4d718934e7489d89e2e6f248d0db619d1f33121ce1497fad21fc33a59f9a85bc56aab3f7523e84397d76a16ec6d561e84";
const RSA_ENC_LONG_SALT: &str = "acc88678ed7792b14345a632b59e0a28ed99f15a77db291d294d3dd52ddb7a7dfc2b3d3f33473fdded3cac1bb50a0a068665a685e8351a133f692b022bd16522c845434de3e9e716fbbf93b0244ad13fbfe04cf08ccd49c9d5af57d56bcdc84fc34fd1176a15713d7a3c521189900976fdc12313cdb65250a4044cb0fea997d761cdc9051369a274723efeb013a32d1025b14ae280a1854a8583b6312c416e08f1fff9e3209c2c57471ad412a1e649013ee176b25052777651efbc5aaf94e94f6985f122efcf41953d95aaac4f677f3eb8ed3a67338348f47629595ed65fb1b88ee89f99b28b5242d53daef423b32b467debff7894cd584b10955e90ba36c525";
const RSA_NONCE_TPM: &str = "b20eb6e67cbd2a46071f965dd20a4995";
const RSA_SESSION_KEY: &str = "8e3d2b8009766ff09696da04a0d0d8d4a557b0d33a8c38350a6b5489adcb3b4c";
const RSA_CMD_HMAC: &str = "c190b0b0fa24f9d97b4d0733a9770649eca6420997d43fe51936f5e3b9fe6303";
const RSA_NONCE_NEW: &str = "3efcc9330ae5e2ed327a6f00e7bf1419";
const RSA_RSP_HMAC: &str = "b323fb63a1801780c9ac78833ec9da9b829b89e788efa63de3f84382f6102d20";

/// Unrestricted RSA-2048 decrypt primary (no symmetric algorithm, no scheme), the same salt.
const RSA2_ENC_SECRET: &str = "832d453686dbc5cae71f021b1e4e4120f40846de379c08934bc0e6c83cc2f4370a170be3f26f38256bec6bc0dd9b0bdf11a3af806577cee9694cae31b6796207c0aaf707bab31a65bfd595fefec18d6674e51bc1fb5d69630d2baa3ee876092749e1b51ff2a79613614fa4fcab4017f49710e4b4d648ec9affb5d4d727bc31d92ac48d7db92d2d88414dec07698820beea8bfd4e615c105d18cfc89bb68b71905d9612307f00d0be9015d5aac7465fb66dd84f58467c530ff8d49183111c2a836e206f6d336ea3d93598cea2c332f65a9f3b92672807fe4ab7c17c1fd998947d1b3d47e4e6bc12538732402263b6b655e2179b04ac0d23efbb8b36c4aef4dc6e";

/// ECC P-256 storage primary; `ENC` is the marshalled point E (two TPM2B).
const ECC_SALT: &str = "acb9bff3d3d7d242daa429d382aac63d5f65a45cd902aa785460861e5a89f7d7";
const ECC_SALT_IDENTITY: &str = "44e3e3ab3f4f39593542d473a15cbb9b80d19ac5176eb79dc56b93c3db467285";
const ECC_ENC: &str = "0020471c3e758c4904285bba7e53118ed0f524adeb0757d25bd2f8e7b0d76dfa714c0020dd520f7aca8a8b917acc37f51de8f0c9bbe3ad858382e702dc25a12d09f7a858";
const ECC_ENC_OFF_CURVE: &str = "0020471c3e758c4904285bba7e53118ed0f524adeb0757d25bd2f8e7b0d76dfa714c0020dd520f7aca8a8b917acc37f51de8f0c9bbe3ad858382e702dc25a12d09f7a859";
const ECC_NONCE_TPM: &str = "3f64e53032fbb062370276a9b143893b";
const ECC_SESSION_KEY: &str = "8319d9f98dc5319b9441d8b13bb10a1e1a70b6240a9b400f274bdbad53dc0ee3";
const ECC_CMD_HMAC: &str = "5feebe205340d13c9e8cd23e4c51606787a5319ec6a1898aedd714511ed75c0e";
const ECC_NONCE_NEW: &str = "40dfabbb2ce0a1e4be5ea5e06355c692";
const ECC_RSP_HMAC: &str = "c33563d47e63a28511d5ed469fa814134f7c11b8f7a2c785c340e08e76832714";

// ------------------------------------------------------------------------------ helpers

fn hex_of(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn hs_from(resp: &[u8], bind: Option<(u32, &[u8])>, nonce_caller: &[u8]) -> Hs {
    ok(resp);
    let handle = u32::from_be_bytes([resp[10], resp[11], resp[12], resp[13]]);
    let mut r = Reader::new(&resp[14..]);
    let nonce_tpm = r.tpm2b(64).unwrap().to_vec();
    r.finish().unwrap();
    let (key, bound) = match bind {
        None => (Vec::new(), None),
        Some((h, auth)) => {
            let auth = trimmed(auth);
            (
                kdfa_256(&auth, b"ATH", &nonce_tpm, nonce_caller),
                Some((h, auth)),
            )
        }
    };
    Hs {
        handle,
        key,
        nonce_tpm,
        bound,
    }
}

fn start_raw(t: &mut Tpm, bind: u32, kind: u8, hash: u16, nonce: &[u8]) -> Vec<u8> {
    run(
        t,
        start_cmd(
            NULL,
            bind,
            &start_params(nonce, &[], kind, &u16b(alg::NULL), hash),
        ),
    )
}

/// Every response of a fixed exchange with unbound and hierarchy-bound sessions (SHA-256 and
/// SHA-384, HMAC, policy and trial) hashed together; the transcript is deterministic (fixed
/// entropy).
fn golden_transcript() -> String {
    let mut t = started();
    let mut log: Vec<u8> = Vec::new();
    log.extend(set_auth(&mut t, OWNER, b"", b"ownerpw"));
    let r = start_raw(&mut t, NULL, se::HMAC, alg::SHA256, &[1; 16]);
    log.extend(&r);
    let mut unbound = hs_from(&r, None, &[1; 16]);
    let r = start_raw(&mut t, OWNER, se::HMAC, alg::SHA256, &[2; 20]);
    log.extend(&r);
    let mut owner = hs_from(&r, Some((OWNER, b"ownerpw")), &[2; 20]);
    let r = start_raw(&mut t, LOCKOUT, se::HMAC, alg::SHA256, &[3; 16]);
    log.extend(&r);
    let mut lockout = hs_from(&r, Some((LOCKOUT, b"")), &[3; 16]);
    // A bound session authorizes the entity it is bound to with the session key alone; the
    // owner authValue changes in the same command.
    let mut c = hc(
        cc::HIERARCHY_CHANGE_AUTH,
        &[OWNER],
        tpm2b(b"ownerpw2"),
        (OWNER, b"ownerpw"),
    );
    c.entity_after = b"ownerpw2".to_vec();
    log.extend(run_hmac(&mut t, &mut owner, &c));
    let c = hc(
        cc::DICTIONARY_ATTACK_LOCK_RESET,
        &[LOCKOUT],
        vec![],
        (LOCKOUT, b""),
    );
    log.extend(run_hmac(&mut t, &mut lockout, &c));
    log.extend(run_hmac(&mut t, &mut unbound, &c));
    // Context round trip of the bound session.
    let saved = run(&mut t, plain(cc::CONTEXT_SAVE, &[lockout.handle], &[]));
    log.extend(&saved);
    let loaded = run(&mut t, plain(cc::CONTEXT_LOAD, &[], params(&saved, 0)));
    log.extend(&loaded);
    log.extend(run_hmac(&mut t, &mut lockout, &c));
    for (bind, kind, hash, nonce) in [
        (OWNER, se::POLICY, alg::SHA256, vec![4u8; 16]),
        (NULL, se::TRIAL, alg::SHA256, vec![5; 16]),
        (OWNER, se::HMAC, alg::SHA384, vec![6; 48]),
        (OWNER, se::POLICY, alg::SHA512, vec![7; 64]),
    ] {
        log.extend(start_raw(&mut t, bind, kind, hash, &nonce));
    }
    hex_of(&Sha256::digest(&log))
}

/// Pinned on the commit before salted sessions (8107776): the sessions without `tpmKey` and
/// with a hierarchy `bind` are byte-for-byte what they were.
const GOLDEN: &str = "d12a5557d40a029269e9f3083e9b26f3637f91a8902891dc47554149d7312488";

/// A TPM with an RSA-2048 storage primary: (TPM, handle, modulus).
fn rsa_fixture() -> (Tpm, u32, Vec<u8>) {
    let mut t = started();
    let p = parse_primary(&create_primary(&mut t, OWNER, b"", &rsa_storage()));
    let Unique::Rsa(n) = unmarshal_public(&p.public).unique else {
        panic!("RSA key")
    };
    (t, p.handle, n)
}

/// A TPM with an ECC P-256 storage primary: (TPM, handle, x, y).
fn ecc_fixture() -> (Tpm, u32, Vec<u8>, Vec<u8>) {
    let mut t = started();
    let p = parse_primary(&create_primary(&mut t, OWNER, b"", &ecc_storage()));
    let Unique::Ecc { x, y } = unmarshal_public(&p.public).unique else {
        panic!("ECC key")
    };
    (t, p.handle, x, y)
}

fn rsa_decrypt_template() -> TpmtPublic {
    rsa(base() | oa::DECRYPT, SymDef::Null, Scheme::NULL)
}

/// Starts an unbound salted HMAC session (SHA-256, no symmetric algorithm).
fn start_salted(t: &mut Tpm, key: u32, enc: &[u8], nonce_caller: &[u8]) -> Vec<u8> {
    start_salted_bound(t, key, NULL, enc, nonce_caller)
}

fn start_salted_bound(
    t: &mut Tpm,
    key: u32,
    bind: u32,
    enc: &[u8],
    nonce_caller: &[u8],
) -> Vec<u8> {
    run(
        t,
        start_cmd(
            key,
            bind,
            &start_params(nonce_caller, enc, se::HMAC, &u16b(alg::NULL), alg::SHA256),
        ),
    )
}

/// `TPM2_DictionaryAttackLockReset` (no parameters) authorized by session `handle` with the given
/// HMAC; the lockout authValue is empty.
fn lockout_reset(handle: u32, nonce_caller: &[u8], attrs: u8, hmac: &[u8]) -> Vec<u8> {
    let mut area = handle.to_be_bytes().to_vec();
    area.extend_from_slice(&tpm2b(nonce_caller));
    area.push(attrs);
    area.extend_from_slice(&tpm2b(hmac));
    let mut auth = (area.len() as u32).to_be_bytes().to_vec();
    auth.extend_from_slice(&area);
    frame(
        st::SESSIONS,
        cc::DICTIONARY_ATTACK_LOCK_RESET,
        &[LOCKOUT],
        &auth,
        &[],
    )
}

fn session_handle_of(resp: &[u8]) -> u32 {
    ok(resp);
    u32::from_be_bytes([resp[10], resp[11], resp[12], resp[13]])
}

fn nonce_tpm_of(resp: &[u8]) -> Vec<u8> {
    ok(resp);
    let mut r = Reader::new(&resp[14..]);
    let n = r.tpm2b(64).unwrap().to_vec();
    r.finish().unwrap();
    n
}

/// The session key the TPM holds for the session with this handle.
fn session_key(t: &Tpm, handle: u32) -> Vec<u8> {
    let slot = t.sessions.slot_of(handle).unwrap();
    t.sessions.get(slot).unwrap().key.as_slice().to_vec()
}

fn lockout_counter(t: &mut Tpm) -> u32 {
    let (_, _, body, count) = cap_list(&get_cap(t, cap::TPM_PROPERTIES, pt::LOCKOUT_COUNTER, 1));
    assert_eq!(count, 1);
    assert_eq!(&body[..4], &u32b(pt::LOCKOUT_COUNTER)[..]);
    u32::from_be_bytes([body[4], body[5], body[6], body[7]])
}

/// Runs the pinned exchange on a salted session: the session key is the independent one, the
/// pinned command HMAC authorizes and the response HMAC is the independent one.
fn pinned_exchange(t: &mut Tpm, session: u32, cmd_hmac: &str, nonce_new: &str, rsp_hmac: &str) {
    let resp = run(t, lockout_reset(session, &NONCE_CALLER, 1, &hex(cmd_hmac)));
    let (params, acks) = split_response(&resp, 0);
    assert!(params.is_empty());
    assert_eq!(acks.len(), 1);
    assert_eq!(hex_of(&acks[0].0), nonce_new);
    assert_eq!(acks[0].1, 1);
    assert_eq!(hex_of(&acks[0].2), rsp_hmac);
}

// --------------------------------------------------------------------------------- tests

#[test]
fn sha256_golden_unchanged() {
    assert_eq!(golden_transcript(), GOLDEN);
}

#[test]
fn salted_rsa_session_key_matches_independent_python() {
    let (mut t, key, _) = rsa_fixture();
    let resp = start_salted(&mut t, key, &hex(RSA_ENC_SECRET), &NONCE_CALLER);
    let session = session_handle_of(&resp);
    assert_eq!(hex_of(&nonce_tpm_of(&resp)), RSA_NONCE_TPM);
    // sessionKey = KDFa(SHA-256, salt, "ATH", nonceTPM, nonceCaller, 256) of Python.
    assert_eq!(hex_of(&session_key(&t, session)), RSA_SESSION_KEY);
    // The salt itself is not the key, and the key is not the unsalted (empty) one.
    assert_ne!(hex_of(&session_key(&t, session)), RSA_SALT);
    assert_eq!(
        hex_of(&kdfa_256(
            &hex(RSA_SALT),
            b"ATH",
            &hex(RSA_NONCE_TPM),
            &NONCE_CALLER
        )),
        RSA_SESSION_KEY
    );
    pinned_exchange(&mut t, session, RSA_CMD_HMAC, RSA_NONCE_NEW, RSA_RSP_HMAC);
}

#[test]
fn salted_ecc_session_key_matches_independent_python() {
    let (mut t, key, _, _) = ecc_fixture();
    let resp = start_salted(&mut t, key, &hex(ECC_ENC), &NONCE_CALLER);
    let session = session_handle_of(&resp);
    assert_eq!(hex_of(&nonce_tpm_of(&resp)), ECC_NONCE_TPM);
    assert_eq!(hex_of(&session_key(&t, session)), ECC_SESSION_KEY);
    // The KDFe of Python with the other label ("IDENTITY") would give another key.
    assert_ne!(
        hex_of(&kdfa_256(
            &hex(ECC_SALT_IDENTITY),
            b"ATH",
            &hex(ECC_NONCE_TPM),
            &NONCE_CALLER
        )),
        ECC_SESSION_KEY
    );
    assert_eq!(
        hex_of(&kdfa_256(
            &hex(ECC_SALT),
            b"ATH",
            &hex(ECC_NONCE_TPM),
            &NONCE_CALLER
        )),
        ECC_SESSION_KEY
    );
    pinned_exchange(&mut t, session, ECC_CMD_HMAC, ECC_NONCE_NEW, ECC_RSP_HMAC);
}

#[test]
fn salted_session_refuses_wrong_label_or_bad_size() {
    let (mut t, key, _) = rsa_fixture();
    let mut flipped = hex(RSA_ENC_SECRET);
    flipped[100] ^= 0x40;
    let wrong_size = |n: usize| vec![0x5a; n];
    for (what, enc, expect) in [
        // OAEP labels "IDENTITY\0" and "SECRET" (no NUL), a salt of 33 octets (more than a digest).
        (
            "label IDENTITY",
            hex(RSA_ENC_IDENTITY),
            Rc::VALUE.in_param(2),
        ),
        (
            "label without NUL",
            hex(RSA_ENC_NO_NUL),
            Rc::VALUE.in_param(2),
        ),
        (
            "salt over a digest",
            hex(RSA_ENC_LONG_SALT),
            Rc::VALUE.in_param(2),
        ),
        ("flipped bit", flipped, Rc::VALUE.in_param(2)),
        // The size of the ciphertext is the size of the modulus.
        ("empty", Vec::new(), Rc::VALUE.in_param(2)),
        (
            "255 octets",
            hex(RSA_ENC_SECRET)[..255].to_vec(),
            Rc::VALUE.in_param(2),
        ),
        ("257 octets", wrong_size(257), Rc::VALUE.in_param(2)),
        ("zeros", vec![0; 256], Rc::VALUE.in_param(2)),
        // More than any supported key can return is a size error of the parameter.
        (
            "385 octets",
            wrong_size(MAX_RSA_BYTES + 1),
            Rc::SIZE.in_param(2),
        ),
    ] {
        let resp = start_salted(&mut t, key, &enc, &NONCE_CALLER);
        assert_eq!(rc(&resp), expect.0, "{what}: {:#x}", rc(&resp));
    }
    let (mut t, key, _, _) = ecc_fixture();
    let point = hex(ECC_ENC);
    let mut short_x = point.clone();
    short_x[1] = 31;
    short_x.remove(2);
    let mut trailing = point.clone();
    trailing.push(0);
    for (what, enc) in [
        ("off the curve", hex(ECC_ENC_OFF_CURVE)),
        ("31-octet x", short_x),
        ("trailing octet", trailing),
        ("RSA ciphertext", hex(RSA_ENC_SECRET)),
        ("x only", point[..34].to_vec()),
        ("empty", Vec::new()),
    ] {
        let resp = start_salted(&mut t, key, &enc, &NONCE_CALLER);
        assert_eq!(
            rc(&resp),
            Rc::VALUE.in_param(2).0,
            "{what}: {:#x}",
            rc(&resp)
        );
    }
    // A nonce that is too short is still the first error, before the salt is looked at.
    err(
        &start_salted(&mut t, key, &hex(ECC_ENC), &[0x33; 15]),
        Rc::SIZE.in_param(1),
    );
    // No salt with a NULL key; no session was created by any failure.
    err(
        &start_salted(&mut t, NULL, &hex(ECC_ENC), &NONCE_CALLER),
        Rc::VALUE.in_param(2),
    );
    assert!(handles_of(&mut t, 0x0200_0000).is_empty());
}

#[test]
fn salted_session_with_wrong_tpmkey_auth_fails_or_not_decrypt() {
    let mut t = started();
    let rsa_key = parse_primary(&create_primary(&mut t, OWNER, b"", &rsa_storage()));
    let rsa_dec = parse_primary(&create_primary(&mut t, OWNER, b"", &rsa_decrypt_template()));
    let ecc_key = parse_primary(&create_primary(&mut t, OWNER, b"", &ecc_storage()));
    let rsa_sig = parse_primary(&create_primary(&mut t, OWNER, b"", &rsa_sign()));
    let ecc_sig = parse_primary(&create_primary(&mut t, OWNER, b"", &ecc_sign()));
    // A sealed data object (child of the ECC storage key).
    let data = {
        let child = parse_child(&create_child(
            &mut t,
            ecc_key.handle,
            b"",
            b"",
            b"secret",
            &sealed(),
        ));
        let (handle, _) = parse_loaded(&load(
            &mut t,
            ecc_key.handle,
            b"",
            &child.private,
            &child.public,
        ));
        handle
    };
    // A key that cannot decrypt, whatever the salt: TPM_RC_KEY for the handle.
    for k in [rsa_sig.handle, ecc_sig.handle, data] {
        for enc in [hex(RSA_ENC_SECRET), hex(ECC_ENC), Vec::new()] {
            err(
                &start_salted(&mut t, k, &enc, &NONCE_CALLER),
                Rc::KEY.in_handle(1),
            );
        }
    }
    // A public-only copy of a decrypt key (LoadExternal) has no private part.
    let public = unmarshal_public(&rsa_key.public);
    let (external, _) = parse_loaded(&load_external(&mut t, &public, NULL));
    err(
        &start_salted(&mut t, external, &hex(RSA_ENC_SECRET), &NONCE_CALLER),
        Rc::KEY.in_handle(1),
    );
    // The right salt for another key is no valid ciphertext for this one.
    err(
        &start_salted(&mut t, rsa_dec.handle, &hex(RSA_ENC_SECRET), &NONCE_CALLER),
        Rc::VALUE.in_param(2),
    );
    err(
        &start_salted(&mut t, ecc_key.handle, &hex(RSA_ENC_SECRET), &NONCE_CALLER),
        Rc::VALUE.in_param(2),
    );
    err(
        &start_salted(&mut t, rsa_key.handle, &hex(ECC_ENC), &NONCE_CALLER),
        Rc::VALUE.in_param(2),
    );
    // An ECC key opens any point: another key gives another salt, so the pinned HMAC of the
    // session of the key the point was made for does not fit and the authorization fails.
    let (mut u, _, _, _) = ecc_fixture();
    let other = parse_primary(&create_primary(&mut u, OWNER, b"", &{
        let mut template = ecc_storage();
        template.attributes |= oa::NO_DA;
        template
    }));
    let resp = start_salted(&mut u, other.handle, &hex(ECC_ENC), &NONCE_CALLER);
    let session = session_handle_of(&resp);
    assert_ne!(hex_of(&session_key(&u, session)), ECC_SESSION_KEY);
    err(
        &run(
            &mut u,
            lockout_reset(session, &NONCE_CALLER, 1, &hex(ECC_CMD_HMAC)),
        ),
        Rc::AUTH_FAIL.in_session(1),
    );
    // The unrestricted decrypt key (no symmetric algorithm) is a good `tpmKey`: the salt made
    // for its modulus opens, and the session key is KDFa of the salt.
    let resp = start_salted(&mut t, rsa_dec.handle, &hex(RSA2_ENC_SECRET), &NONCE_CALLER);
    let nonce_tpm = nonce_tpm_of(&resp);
    assert_eq!(
        session_key(&t, session_handle_of(&resp)),
        kdfa_256(&hex(RSA_SALT), b"ATH", &nonce_tpm, &NONCE_CALLER)
    );
}

#[test]
fn salted_session_context_round_trip() {
    // The salt is used only when the session starts: the session key is in the context, so a
    // saved and loaded salted session still carries the independent key and authorizes.
    let (mut t, key, _, _) = ecc_fixture();
    let session = session_handle_of(&start_salted(&mut t, key, &hex(ECC_ENC), &NONCE_CALLER));
    let saved = run(&mut t, plain(cc::CONTEXT_SAVE, &[session], &[]));
    ok(&saved);
    assert!(
        t.sessions.slot_of(session).is_none(),
        "saved sessions leave the TPM"
    );
    let loaded = run(&mut t, plain(cc::CONTEXT_LOAD, &[], params(&saved, 0)));
    assert_eq!(session_handle_of(&loaded), session);
    assert_eq!(hex_of(&session_key(&t, session)), ECC_SESSION_KEY);
    pinned_exchange(&mut t, session, ECC_CMD_HMAC, ECC_NONCE_NEW, ECC_RSP_HMAC);
}

#[test]
fn salted_and_bound_session_key_holds_both_secrets() {
    let (mut t, key, _, _) = ecc_fixture();
    ok(&set_auth(&mut t, OWNER, b"", b"ownerpw"));
    let resp = start_salted_bound(&mut t, key, OWNER, &hex(ECC_ENC), &NONCE_CALLER);
    let nonce_tpm = nonce_tpm_of(&resp);
    let material = [&b"ownerpw"[..], &hex(ECC_SALT)].concat();
    let expected = kdfa_256(&material, b"ATH", &nonce_tpm, &NONCE_CALLER);
    assert_eq!(session_key(&t, session_handle_of(&resp)), expected);
    // Bound to the owner: authorizing the owner uses the session key alone.
    let mut s = Hs {
        handle: session_handle_of(&resp),
        key: expected,
        nonce_tpm,
        bound: Some((OWNER, b"ownerpw".to_vec())),
    };
    let mut c = hc(
        cc::HIERARCHY_CHANGE_AUTH,
        &[OWNER],
        tpm2b(b"ownerpw2"),
        (OWNER, b"ownerpw"),
    );
    c.nonce_caller = NONCE_CALLER.to_vec();
    c.entity_after = b"ownerpw2".to_vec();
    ok(&run_hmac(&mut t, &mut s, &c));
}

/// A signing child (ECDSA, authValue `pw`) of an ECC storage primary, loaded.
struct Child {
    parent: u32,
    parent_name: Vec<u8>,
    public: Vec<u8>,
    private: Vec<u8>,
    key: u32,
    name: Vec<u8>,
}

fn loaded_child(t: &mut Tpm, pw: &[u8]) -> Child {
    let parent = parse_primary(&create_primary(t, OWNER, b"", &ecc_storage()));
    let child = parse_child(&create_child(t, parent.handle, b"", pw, b"", &ecc_sign()));
    let (key, name) = parse_loaded(&load(t, parent.handle, b"", &child.private, &child.public));
    Child {
        parent: parent.handle,
        parent_name: parent.name,
        public: child.public,
        private: child.private,
        key,
        name,
    }
}

fn sign_hc(key: u32, name: &[u8], entity_auth: &[u8]) -> Hc {
    let digest = [7u8; 32];
    let scheme = cat(&[&u16b(alg::ECDSA), &u16b(alg::SHA256)]);
    let mut c = hc(
        cc::SIGN,
        &[key],
        sign_params(&digest, &scheme),
        (key, entity_auth),
    );
    c.names = vec![name.to_vec()];
    c
}

#[test]
fn bound_object_session_authvalue_change_breaks_session() {
    let mut t = started();
    let c = loaded_child(&mut t, b"kpw");
    // Bound to the object: its authValue is the key material, and authorizing the object itself
    // uses the session key alone (Equation 22).
    let mut s = start_hmac(&mut t, c.key, b"kpw", &[9; 16]);
    assert_eq!(s.bound, Some((c.key, b"kpw".to_vec())));
    assert_eq!(session_key(&t, s.handle), s.key);
    assert_eq!(lockout_counter(&mut t), 0);
    ok(&run_hmac(&mut t, &mut s, &sign_hc(c.key, &c.name, b"kpw")));
    // A client that takes the session for unbound (key || authValue) fails, and the failure counts
    // for the DA protection of the object like any failed authorization.
    let unbound_view = Hs {
        handle: s.handle,
        key: s.key.clone(),
        nonce_tpm: s.nonce_tpm.clone(),
        bound: None,
    };
    err(
        &run(
            &mut t,
            hmac_command(
                &unbound_view,
                &sign_hc(c.key, &c.name, b"kpw"),
                false,
                false,
            ),
        ),
        Rc::AUTH_FAIL.in_session(1),
    );
    assert_eq!(lockout_counter(&mut t), 1);
    // The bound session changes the authValue of the object (ObjectChangeAuth is made as a new
    // private blob; the response is protected with the session key alone).
    let mut change = hc(
        cc::OBJECT_CHANGE_AUTH,
        &[c.key, c.parent],
        tpm2b(b"new-pw"),
        (c.key, b"kpw"),
    );
    change.names = vec![c.name.clone(), c.parent_name.clone()];
    let resp = run_hmac(&mut t, &mut s, &change);
    let (out, _) = split_response(&resp, 0);
    let new_private = Reader::new(&out).tpm2b(1024).unwrap().to_vec();
    assert_ne!(new_private, c.private);
    ok(&flush(&mut t, c.key));
    let (key2, name2) = parse_loaded(&load(&mut t, c.parent, b"", &new_private, &c.public));
    assert_eq!(name2, c.name, "the Name does not depend on the authValue");
    ok(&super::tests_obj::sign(
        &mut t,
        key2,
        b"new-pw",
        &[7; 32],
        &cat(&[&u16b(alg::ECDSA), &u16b(alg::SHA256)]),
    ));
    // The authValue of the entity changed: the session is no longer bound to it, so the HMAC
    // key is sessionKey alone no more (here the stale client still uses it) and is refused.
    let mut stale = Hs {
        handle: s.handle,
        key: s.key.clone(),
        nonce_tpm: s.nonce_tpm.clone(),
        bound: Some((key2, b"new-pw".to_vec())),
    };
    err(
        &run(
            &mut t,
            hmac_command(&stale, &sign_hc(key2, &name2, b"new-pw"), false, false),
        ),
        Rc::AUTH_FAIL.in_session(1),
    );
    assert_eq!(lockout_counter(&mut t), 2);
    // With sessionKey || new authValue the same session works.
    stale.bound = None;
    ok(&run_hmac(
        &mut t,
        &mut stale,
        &sign_hc(key2, &name2, b"new-pw"),
    ));
    // A session bound to the changed object works again with the key alone.
    let mut fresh = start_hmac(&mut t, key2, b"new-pw", &[8; 16]);
    ok(&run_hmac(
        &mut t,
        &mut fresh,
        &sign_hc(key2, &name2, b"new-pw"),
    ));
    assert_eq!(lockout_counter(&mut t), 2);
}

#[test]
fn bound_nv_session_uses_the_index_authvalue_and_follows_its_change() {
    let mut t = started();
    let idx = super::tests_b::NV0 + 5;
    ok(&define_raw(
        &mut t,
        OWNER,
        b"",
        b"nvpw",
        &nv_public(idx, alg::SHA256, attr::AUTHWRITE | attr::AUTHREAD, &[], 16),
    ));
    let nv_hc = |t: &mut Tpm, code: u32, params: Vec<u8>, auth: &[u8]| {
        let name = nv_read_public(t, idx).1;
        let mut c = hc(code, &[idx, idx], params, (idx, auth));
        c.names = vec![name.clone(), name];
        c
    };
    let mut s = start_hmac(&mut t, idx, b"nvpw", &[6; 16]);
    assert_eq!(session_key(&t, s.handle), s.key);
    // A bound session survives a context round trip (the bound handle and authValue are saved).
    let saved = run(&mut t, plain(cc::CONTEXT_SAVE, &[s.handle], &[]));
    let loaded = run(&mut t, plain(cc::CONTEXT_LOAD, &[], params(&saved, 0)));
    assert_eq!(session_handle_of(&loaded), s.handle);
    let write = nv_hc(
        &mut t,
        cc::NV_WRITE,
        cat(&[&tpm2b(&[1, 2, 3, 4]), &u16b(0)]),
        b"nvpw",
    );
    ok(&run_hmac(&mut t, &mut s, &write));
    // The client's model of a bound session whose entity changed its Name: the key alone is
    // wrong now (IsSessionBindEntity compares the Name, which carries TPMA_NV_WRITTEN), the
    // key with the authValue is right. The session was saved and loaded before the write, so
    // the Name survived the context body (tag 2).
    let bound_view = Hs {
        handle: s.handle,
        key: s.key.clone(),
        nonce_tpm: s.nonce_tpm.clone(),
        bound: Some((idx, b"nvpw".to_vec())),
    };
    let read = nv_hc(&mut t, cc::NV_READ, cat(&[&u16b(4), &u16b(0)]), b"nvpw");
    err(
        &run(&mut t, hmac_command(&bound_view, &read, false, false)),
        Rc::AUTH_FAIL.in_session(1),
    );
    s.bound = None;
    let resp = run_hmac(&mut t, &mut s, &read);
    let (out, _) = split_response(&resp, 0);
    assert_eq!(Reader::new(&out).tpm2b(16).unwrap(), &[1, 2, 3, 4]);
    // The index is undefined and defined again, unwritten, with the same public area and another
    // authValue: the Name is the one of the start, the authValue is not. The key alone is
    // refused, the key with the new authValue works.
    ok(&pw_cmd(
        &mut t,
        cc::NV_UNDEFINE_SPACE,
        &[OWNER, idx],
        b"",
        &[],
    ));
    ok(&define_raw(
        &mut t,
        OWNER,
        b"",
        b"nvpw2",
        &nv_public(idx, alg::SHA256, attr::AUTHWRITE | attr::AUTHREAD, &[], 16),
    ));
    let write_params = cat(&[&tpm2b(&[1, 2, 3, 4]), &u16b(0)]);
    let write2 = nv_hc(&mut t, cc::NV_WRITE, write_params.clone(), b"nvpw2");
    let mut stale = Hs {
        handle: s.handle,
        key: s.key.clone(),
        nonce_tpm: s.nonce_tpm.clone(),
        bound: Some((idx, b"nvpw2".to_vec())),
    };
    err(
        &run(&mut t, hmac_command(&stale, &write2, false, false)),
        Rc::AUTH_FAIL.in_session(1),
    );
    stale.bound = None;
    ok(&run_hmac(&mut t, &mut stale, &write2));
}

/// A session bound to an NV index is not bound to another index that has the same handle and the
/// very same authValue but another Name (here another size): the key alone is refused and the
/// authValue joins the key.
#[test]
fn bound_nv_session_is_not_bound_to_another_index_with_the_same_handle_and_authvalue() {
    let mut t = started();
    let idx = super::tests_b::NV0 + 5;
    let public = |size| {
        nv_public(
            idx,
            alg::SHA256,
            attr::AUTHWRITE | attr::AUTHREAD,
            &[],
            size,
        )
    };
    ok(&define_raw(&mut t, OWNER, b"", b"nvpw", &public(16)));
    let mut s = start_hmac(&mut t, idx, b"nvpw", &[6; 16]);
    ok(&pw_cmd(
        &mut t,
        cc::NV_UNDEFINE_SPACE,
        &[OWNER, idx],
        b"",
        &[],
    ));
    ok(&define_raw(&mut t, OWNER, b"", b"nvpw", &public(8)));
    let name = nv_read_public(&mut t, idx).1;
    let mut write = hc(
        cc::NV_WRITE,
        &[idx, idx],
        cat(&[&tpm2b(&[1, 2, 3, 4]), &u16b(0)]),
        (idx, b"nvpw"),
    );
    write.names = vec![name.clone(), name];
    // The client that still believes the session is bound uses the key alone: refused.
    let bound_view = Hs {
        handle: s.handle,
        key: s.key.clone(),
        nonce_tpm: s.nonce_tpm.clone(),
        bound: Some((idx, b"nvpw".to_vec())),
    };
    err(
        &run(&mut t, hmac_command(&bound_view, &write, false, false)),
        Rc::AUTH_FAIL.in_session(1),
    );
    s.bound = None;
    ok(&run_hmac(&mut t, &mut s, &write));
}

/// A salt that decrypts to the Empty Buffer with a NULL `bind` leaves the session key empty (the
/// reference's condition is `bind != TPM_RH_NULL || seed.size != 0`); with a bound entity the key
/// is still derived, from the authValue alone.
#[test]
fn empty_decrypted_salt_with_null_bind_leaves_the_key_empty() {
    use crate::crypto::{HashAlg, RsaPadding, rsa_encrypt_public};
    let (mut t, key, n) = rsa_fixture();
    let enc = rsa_encrypt_public(
        &n,
        RsaPadding::OaepWith(HashAlg::Sha256),
        b"SECRET\0",
        &[],
        &mut t.drbg,
    )
    .unwrap();
    let resp = start_salted(&mut t, key, &enc, &NONCE_CALLER);
    assert!(session_key(&t, session_handle_of(&resp)).is_empty());
    ok(&set_auth(&mut t, OWNER, b"", b"ownerpw"));
    let resp = start_salted_bound(&mut t, key, OWNER, &enc, &NONCE_CALLER);
    let expected = kdfa_256(b"ownerpw", b"ATH", &nonce_tpm_of(&resp), &NONCE_CALLER);
    assert_eq!(session_key(&t, session_handle_of(&resp)), expected);
}

#[test]
fn bind_handles_of_start_auth_session() {
    let mut t = started();
    let c = loaded_child(&mut t, b"kpw");
    let good = start_params(&[1; 16], &[], se::HMAC, &u16b(alg::NULL), alg::SHA256);
    // A loaded object and a defined NV index are bind entities now (a PCR, an undefined index and
    // a password handle still are not).
    ok(&run(&mut t, start_cmd(NULL, c.key, &good)));
    let idx = super::tests_b::NV0 + 1;
    err(
        &run(&mut t, start_cmd(NULL, idx, &good)),
        Rc::VALUE.in_handle(2),
    );
    ok(&define_raw(
        &mut t,
        OWNER,
        b"",
        b"",
        &nv_public(idx, alg::SHA256, attr::AUTHWRITE | attr::AUTHREAD, &[], 8),
    ));
    ok(&run(&mut t, start_cmd(NULL, idx, &good)));
    for bind in [7, handle::RS_PW, 0x4000_0002] {
        err(
            &run(&mut t, start_cmd(NULL, bind, &good)),
            Rc::VALUE.in_handle(2),
        );
    }
    // A loaded object is a tpmKey too: a signing key has no decrypt attribute (TPM_RC_KEY, no
    // longer TPM_RC_VALUE for the handle), and a key with the empty salt is refused as well.
    err(
        &run(&mut t, start_cmd(c.key, NULL, &good)),
        Rc::KEY.in_handle(1),
    );
}
