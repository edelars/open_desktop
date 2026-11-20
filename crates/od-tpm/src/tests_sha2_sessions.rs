//! Sessions with an authHash of SHA-384 or SHA-512 (hash agility, step D). The expected HMACs, the
//! session keys, the CFB ciphertexts and the audit digests come from Python `hmac`, `hashlib` and
//! `openssl enc` (the scripts are in the task scratchpad), not from the code under test: every
//! transcript below was computed from its inputs by Python and agrees with the TPM. The TPM
//! entropy is fixed, so the `nonceTPM` values (and the Names of the sealed primaries) are inputs
//! pinned here; if the use of the DRBG changes they drift and the vectors must be regenerated.
//! Policy and trial sessions follow the same hashes since step E (`tests_sha2_policy.rs`).

use super::tests_b::{
    Hs, hc, hmac_auth_entry, set_auth, split_response, start_cmd, start_hmac, start_params,
    start_session,
};
use super::tests_c3::{AES_SYM, XOR_SYM, start_sym};
use super::tests_obj::{NO_PCR, create_primary_raw, creation_params, sealed};
use super::*;
use crate::codes::{se, session_attr};
use crate::crypto::{HashAlg, hash};

const NULL: u32 = handle::RH_NULL;
const OWNER: u32 = handle::RH_OWNER;
const ENDORSEMENT: u32 = handle::RH_ENDORSEMENT;
const AUDIT: u8 = session_attr::AUDIT | session_attr::CONTINUE_SESSION;
const AUDIT_EXCLUSIVE: u8 = AUDIT | session_attr::AUDIT_EXCLUSIVE;
const CONTINUE: u8 = session_attr::CONTINUE_SESSION;
const TEST_PARMS: &str = "002300100018000b00030010";

fn hex_of(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn sha256_hex(b: &[u8]) -> String {
    hex_of(hash(HashAlg::Sha256, &[b]).as_slice())
}

/// `len` octets `first, first + 1, ...`: the nonces and secrets of the pinned transcripts.
fn seq(len: usize, first: u8) -> Vec<u8> {
    (0..len).map(|i| first.wrapping_add(i as u8)).collect()
}

/// One pinned exchange: the HMAC the client sends, then the new `nonceTPM` and the HMAC of the
/// response acknowledgement (an empty `rsp` skips the response HMAC).
struct Pin {
    cmd: &'static str,
    nonce: &'static str,
    rsp: &'static str,
}

/// One session entry of a command: its handle, `nonceCaller`, attributes, the attributes expected
/// in the acknowledgement and the pinned exchange.
struct Entry {
    handle: u32,
    nonce_caller: Vec<u8>,
    attrs: u8,
    response_attrs: u8,
    pin: Pin,
}

fn entry(handle: u32, nonce_caller: Vec<u8>, attrs: u8, response_attrs: u8, pin: Pin) -> Entry {
    Entry {
        handle,
        nonce_caller,
        attrs,
        response_attrs,
        pin,
    }
}

/// Starts an HMAC session of `auth_hash` and checks the first `nonceTPM` against the pin.
fn start_pinned(
    t: &mut Tpm,
    auth_hash: u16,
    bind: u32,
    nonce_caller: &[u8],
    sym: &[u8],
    nonce_tpm: &str,
) -> u32 {
    let resp = run(
        t,
        start_cmd(
            NULL,
            bind,
            &start_params(nonce_caller, &[], se::HMAC, sym, auth_hash),
        ),
    );
    ok(&resp);
    let mut r = Reader::new(&resp[14..]);
    let nonce = r.tpm2b(64).unwrap().to_vec();
    r.finish().unwrap();
    assert_eq!(nonce.len(), nonce_caller.len());
    assert_eq!(hex_of(&nonce), nonce_tpm, "first nonceTPM (fixed entropy)");
    u32::from_be_bytes(resp[10..14].try_into().unwrap())
}

/// Sends a command whose HMACs are the pinned ones and checks every acknowledgement; returns the
/// response parameters.
fn step(
    t: &mut Tpm,
    passwords: &[&[u8]],
    code: u32,
    handles: &[u32],
    params: &[u8],
    entries: &[Entry],
) -> Vec<u8> {
    let mut area = auth_area(passwords).split_off(4);
    for e in entries {
        area.extend_from_slice(&e.handle.to_be_bytes());
        area.extend_from_slice(&tpm2b(&e.nonce_caller));
        area.push(e.attrs);
        area.extend_from_slice(&tpm2b(&hex(e.pin.cmd)));
    }
    let area = cat(&[&(area.len() as u32).to_be_bytes(), &area]);
    let resp = run(t, frame(st::SESSIONS, code, handles, &area, params));
    let (rparams, acks) = split_response(&resp, 0);
    assert_eq!(acks.len(), passwords.len() + entries.len());
    for (e, ack) in entries.iter().zip(&acks[passwords.len()..]) {
        assert_eq!(
            hex_of(&ack.0),
            e.pin.nonce,
            "nonceTPM of the acknowledgement"
        );
        assert_eq!(ack.1, e.response_attrs, "attributes of the acknowledgement");
        if !e.pin.rsp.is_empty() {
            assert_eq!(hex_of(&ack.2), e.pin.rsp, "response HMAC");
        }
    }
    rparams
}

fn loaded_audit_digest(t: &Tpm, session: u32) -> Vec<u8> {
    let slot = t.sessions.slot_of(session).unwrap();
    t.sessions.get(slot).unwrap().audit_digest.unwrap().to_vec()
}

#[test]
fn sha384_hmac_session_exchange_matches_independent_python_values() {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", b"owner-auth-384"));
    let nonce_caller = seq(48, 0x11);
    let s = start_pinned(
        &mut t,
        alg::SHA384,
        OWNER,
        &nonce_caller,
        &u16b(alg::NULL),
        "3f64e53032fbb062370276a9b143893ba0b75566d5d0942f03f9402b234c48bffc7d74cb580374ade99168f98c8582f8",
    );
    let slot = t.sessions.slot_of(s).unwrap();
    let session = t.sessions.get(slot).unwrap();
    assert_eq!(session.auth_hash, HashAlg::Sha384);
    assert_eq!(session.key.as_slice().len(), 48);
    // sessionKey = KDFa(SHA-384, "owner-auth-384", "ATH", nonceTPM, nonceCaller, 384) (Python).
    assert_eq!(
        hex_of(session.key.as_slice()),
        "305533234a75419556cd0367865ee11e3cf5c6fe2fac370feef752eb27f9196153512b17554500633b1c931aafa78d5a",
        "sessionKey"
    );

    // The session is bound to the owner: its key alone is the HMAC key, in the command and in the
    // response (Part 1, Equation 22), and the nonces and HMACs are 48 octets long.
    let owner_change = |new: &[u8]| tpm2b(new);
    step(
        &mut t,
        &[],
        cc::HIERARCHY_CHANGE_AUTH,
        &[OWNER],
        &owner_change(b"rotated-auth-1"),
        &[entry(
            s,
            seq(48, 0x60),
            CONTINUE,
            CONTINUE,
            Pin {
                cmd: "812874718a1bdc8ce405661401530c94ccac080090d8aaf145c78eca91c110361ea073786f73ee79bda20d938af3f182",
                nonce: "fff9690bde8aad0397efc5f7e644b5aa762c04536c611dcbf136fcb45495031e577f54f47412504d076d947356899290",
                rsp: "4b376c396da82926163b340b1bb84a9caa29cd517731797f63300fda2a1d67f975699d56a25be3d5e287c4513dc9df46",
            },
        )],
    );
    // With the new authValue the binding no longer matches: sessionKey || authValue is the key.
    step(
        &mut t,
        &[],
        cc::HIERARCHY_CHANGE_AUTH,
        &[OWNER],
        &owner_change(b"rotated-auth-2"),
        &[entry(
            s,
            seq(48, 0x70),
            CONTINUE,
            CONTINUE,
            Pin {
                cmd: "19c2142b8b39ea5b968a064c6e919e2e38939df98c8013ccc52fc51c002e3f00d4a175d1231c0ed16a0c741f7ceecd72",
                nonce: "fccd26fce733b3c303a9b6f0d2075ab981a60e93e058340b063063baae90815254f3df92888bcceacf13040b35a30dee",
                rsp: "fb46ef85015a7c49f3cefed40c916e6e374ad85e50d5441d3f0f60cca33a1103661e65b435a47c623b1f9fc93a5ecebd",
            },
        )],
    );
    // An HMAC cut to the length of a SHA-256 HMAC does not authorize and leaves the session alone.
    let short = "19c2142b8b39ea5b968a064c6e919e2e38939df98c8013ccc52fc51c002e3f00";
    err(
        &run(
            &mut t,
            frame(
                st::SESSIONS,
                cc::HIERARCHY_CHANGE_AUTH,
                &[OWNER],
                &cat(&[
                    &(4 + 2 + 48 + 1 + 2 + 32u32).to_be_bytes(),
                    &s.to_be_bytes(),
                    &tpm2b(&seq(48, 0x80)),
                    &[CONTINUE],
                    &tpm2b(&hex(short)),
                ]),
                &tpm2b(b"rotated-auth-3"),
            ),
        ),
        Rc::BAD_AUTH.in_session(1),
    );
}

/// `CreatePrimary` of a sealed data object with the `nameAlg` given and a 48 or 64 octet
/// `userAuth`; returns the handle and the Name (66 octets for SHA-512).
fn sealed_primary(t: &mut Tpm, name_alg: u16, auth: &[u8], data: &[u8]) -> (u32, Vec<u8>) {
    let mut public = sealed();
    public.name_alg = name_alg;
    let params = creation_params(auth, data, &public, b"", &NO_PCR);
    let resp = create_primary_raw(t, OWNER, b"", &params);
    ok(&resp);
    let mut r = Reader::new(params_of(&resp));
    r.tpm2b(1024).unwrap();
    r.tpm2b(1024).unwrap();
    r.tpm2b(64).unwrap();
    r.u16().unwrap();
    r.u32().unwrap();
    r.tpm2b(64).unwrap();
    let name = r.tpm2b(66).unwrap().to_vec();
    r.finish().unwrap();
    (u32::from_be_bytes(resp[10..14].try_into().unwrap()), name)
}

fn params_of(resp: &[u8]) -> &[u8] {
    params(resp, 1)
}

const NAME_SEALED_SHA512: &str = "000d36559981736554436841c08aa671497404d24544eddeb4478044ad6f4b0e502d260e6de4bb5d24af6b7732d77107b62a640db17b01e52e21c58c223e24464a0f";
const NAME_SEALED_SHA384: &str = "000c9ca9d78efcbcd7890b95e7a1fcaf1670af7c65cb31d486efa608d889ab014e31dda43a16601d99e203b465b474097521";

#[test]
fn sha512_hmac_session_authorizes_object_with_48_and_64_octet_auth() {
    let mut t = started();
    let (auth512, auth384) = (seq(64, 0x41), seq(48, 0x51));
    let (p512, name512) = sealed_primary(&mut t, alg::SHA512, &auth512, b"sealed-512");
    let (p384, name384) = sealed_primary(&mut t, alg::SHA384, &auth384, b"sealed-384");
    assert_eq!(hex_of(&name512), NAME_SEALED_SHA512);
    assert_eq!(hex_of(&name384), NAME_SEALED_SHA384);
    assert_eq!(name512.len(), 66);
    assert_eq!(name384.len(), 50);
    let s = start_pinned(
        &mut t,
        alg::SHA512,
        NULL,
        &seq(64, 0x31),
        &u16b(alg::NULL),
        "3f64e53032fbb062370276a9b143893ba0b75566d5d0942f03f9402b234c48bffc7d74cb580374ade99168f98c8582f8834416b2daaf350e5e01f87456a65d35",
    );
    // An unbound session: sessionKey is empty, the key is the authValue of the object, 64 octets for
    // the SHA-512 object (longer than the 32-octet block of SHA-256 and 48 of SHA-384).
    let r = step(
        &mut t,
        &[],
        cc::UNSEAL,
        &[p512],
        &[],
        &[entry(
            s,
            seq(64, 0x80),
            CONTINUE,
            CONTINUE,
            Pin {
                cmd: "7b2e18933d8dbc11e86d2ed99f612d14308af88d61ea72f0faa5057840eed27a005b4c7dd4be20b0869afcbe1fe687734072b10048753c1fbb05f7707d745d37",
                nonce: "fff9690bde8aad0397efc5f7e644b5aa762c04536c611dcbf136fcb45495031e577f54f47412504d076d94735689929082fedcde13d710232629e0176aba2b32",
                rsp: "13b92307dde075d40b2b90437685e33ad6a23f1b5945a56dcbc1ff47721ead020168037e0b5ac3f6a3dc35ef8251513024ae30cf29b34d1656c1ddc5c8551b7d",
            },
        )],
    );
    assert_eq!(Reader::new(&r).tpm2b(128).unwrap(), b"sealed-512");
    // A SHA-384 object with a 48-octet authValue under the same SHA-512 session.
    let r = step(
        &mut t,
        &[],
        cc::UNSEAL,
        &[p384],
        &[],
        &[entry(
            s,
            seq(64, 0x90),
            CONTINUE,
            CONTINUE,
            Pin {
                cmd: "66abb91ec2c63473c1ab04fb13b5115cc44c8e1c1801e3d41d6a2dad14bc1547d28850d71c507e8bbd6b372c5cca545aaef5da6ab2571f7248a1d168decf3825",
                nonce: "fccd26fce733b3c303a9b6f0d2075ab981a60e93e058340b063063baae90815254f3df92888bcceacf13040b35a30deedf5fa25de461ecee82effc3a385451b7",
                rsp: "38c7de60f4bbf971fd50104b49b9abb256a649de51ec4f970e119f4c618c1419ec69ced027a6ee74f9fb2a4f241d8084b51dc52b0b7fe25c8e7e67254da132eb",
            },
        )],
    );
    assert_eq!(Reader::new(&r).tpm2b(128).unwrap(), b"sealed-384");
    // A SHA-512 HMAC under a wrong authValue is refused.
    const WRONG_HMAC: &str = "66abb91ec2c63473c1ab04fb13b5115cc44c8e1c1801e3d41d6a2dad14bc1547d28850d71c507e8bbd6b372c5cca545aaef5da6ab2571f7248a1d168decf3825";
    err(
        &run(
            &mut t,
            frame(
                st::SESSIONS,
                cc::UNSEAL,
                &[p512],
                &cat(&[
                    &(4 + 2 + 64 + 1 + 2 + 64u32).to_be_bytes(),
                    &s.to_be_bytes(),
                    &tpm2b(&seq(64, 0xA0)),
                    &[CONTINUE],
                    &tpm2b(&hex(WRONG_HMAC)),
                ]),
                &[],
            ),
        ),
        Rc::AUTH_FAIL.in_session(1),
    );
}

#[test]
fn sha384_session_parameter_encryption_cfb_matches_python() {
    // TPM2_Hash with decrypt and encrypt on a session that authorizes nothing: sessionValue is the
    // sessionKey alone; the request carries the CFB ciphertext of the data (key and IV from
    // KDFa-SHA-384, nonceNewer = nonceCaller, nonceOlder = nonceTPM), the response the CFB
    // ciphertext of the SHA-256 digest (nonceNewer = new nonceTPM, nonceOlder = nonceCaller).
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", b"owner-auth-cfb"));
    let s = start_pinned(
        &mut t,
        alg::SHA384,
        OWNER,
        &seq(48, 0x21),
        &AES_SYM,
        "3f64e53032fbb062370276a9b143893ba0b75566d5d0942f03f9402b234c48bffc7d74cb580374ade99168f98c8582f8",
    );
    let plain = b"parameter encryption with SHA-384 KDFa";
    let cipher = "201f060bf739e0f7f2657c60d575d6ab9f1e00f592189f2022a2674138abeb7cc2b22621c413";
    assert_ne!(hex_of(plain), cipher);
    let attrs = CONTINUE | session_attr::DECRYPT | session_attr::ENCRYPT;
    let r = step(
        &mut t,
        &[],
        cc::HASH,
        &[],
        &cat(&[&tpm2b(&hex(cipher)), &u16b(alg::SHA256), &u32b(NULL)]),
        &[entry(
            s,
            seq(48, 0xA0),
            attrs,
            attrs,
            Pin {
                cmd: "2f812549f975dc8ffc2ba5408fff6ec178a5387787a7705d77f7cc9be5f7923486c230e233166676524a1961830a104f",
                nonce: "fff9690bde8aad0397efc5f7e644b5aa762c04536c611dcbf136fcb45495031e577f54f47412504d076d947356899290",
                rsp: "91ec09012a78b276122812cc2cd0501b2a381fd6d49d43cf00bf6d6c340969f2bb0b0eeffe473d8264262e3bbd3a83bb",
            },
        )],
    );
    // CFB(SHA-256(plain)) and the empty NULL-hierarchy ticket; the digest of the decrypted data is
    // the digest of the plaintext, so the request was decrypted with the same key.
    assert_eq!(
        hex_of(&r),
        "0020948bca1c23211a5d426a2f998e292c211b7d14d06cbc38c7d7a9c6e166a36b1d8024400000070000"
    );

    // TPM2_Unseal with encrypt on an authorizing session: sessionValue = sessionKey (empty) ||
    // authValue (48 octets).
    let mut t = started();
    let auth384 = seq(48, 0x51);
    let (p384, name384) = sealed_primary(&mut t, alg::SHA384, &auth384, b"sealed-384");
    assert_eq!(hex_of(&name384), NAME_SEALED_SHA384);
    let s = start_pinned(
        &mut t,
        alg::SHA384,
        NULL,
        &seq(48, 0x25),
        &AES_SYM,
        "3f64e53032fbb062370276a9b143893ba0b75566d5d0942f03f9402b234c48bffc7d74cb580374ade99168f98c8582f8",
    );
    let attrs = CONTINUE | session_attr::ENCRYPT;
    let r = step(
        &mut t,
        &[],
        cc::UNSEAL,
        &[p384],
        &[],
        &[entry(
            s,
            seq(48, 0xB0),
            attrs,
            attrs,
            Pin {
                cmd: "956a80d9fe8abf6dda13bf47eb7317c577039bede68cbe5f60805cfb48988bf34977c264802be9ec2e13999bc8ab80f1",
                nonce: "fff9690bde8aad0397efc5f7e644b5aa762c04536c611dcbf136fcb45495031e577f54f47412504d076d947356899290",
                rsp: "96cfd4752e027250fac6a8376c61f00ff9131efdadb098e9fbb249359f58d2448e645e374865cd9466cd85c7422788da",
            },
        )],
    );
    assert_eq!(hex_of(&r), "000afa0fc9adfff8a9743cef");
    assert_ne!(Reader::new(&r).tpm2b(128).unwrap(), b"sealed-384");
}

#[test]
fn xor_parameter_mask_uses_session_auth_hash() {
    // TPM2_Hash with decrypt and encrypt on a SHA-384 session whose symmetric definition is XOR
    // (SHA-256, the only hash StartAuthSession accepts there). The mask is
    // KDFa(authHash = SHA-384, sessionKey, "XOR", nonceNewer, nonceOlder, 8 * size) as in
    // CryptXORObfuscation(session->authHashAlg, ...) of ms-tpm-20-ref, not a SHA-256 KDFa. Python
    // `hmac`/`hashlib` (xor_sha384.py in the task scratchpad) computed the sessionKey, the masks, the
    // ciphertexts and the HMACs; the same script reproduces the pinned CFB transcript above.
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", b"owner-auth-xor"));
    let s = start_pinned(
        &mut t,
        alg::SHA384,
        OWNER,
        &seq(48, 0x21),
        &XOR_SYM,
        "3f64e53032fbb062370276a9b143893ba0b75566d5d0942f03f9402b234c48bffc7d74cb580374ade99168f98c8582f8",
    );
    let slot = t.sessions.slot_of(s).unwrap();
    assert_eq!(
        hex_of(t.sessions.get(slot).unwrap().key.as_slice()),
        "089d75b74b43755cff8bbe84e6928b37d9e9a4b76d1a52b533d7df801dc9476c93581d983cd747ca305425d9506f4686",
        "sessionKey"
    );
    let plain = b"parameter encryption with SHA-384 KDFa";
    // plain XOR KDFa-SHA-384(sessionKey, "XOR", nonceCaller, nonceTPM, 304 bits).
    let cipher = "603f9138af0424fb3544e7f14570aa24df5420519d222bb500a9a459b66e4ce10b039a57a163";
    assert_eq!(cipher.len(), plain.len() * 2);
    let attrs = CONTINUE | session_attr::DECRYPT | session_attr::ENCRYPT;
    let r = step(
        &mut t,
        &[],
        cc::HASH,
        &[],
        &cat(&[&tpm2b(&hex(cipher)), &u16b(alg::SHA256), &u32b(NULL)]),
        &[entry(
            s,
            seq(48, 0xA0),
            attrs,
            attrs,
            Pin {
                cmd: "0de30f8df72de446bd212d5e5e219e25333890556dbc0811fd759de38f66c85283a61d7584fc25a95cda2b86c9b8d89c",
                nonce: "fff9690bde8aad0397efc5f7e644b5aa762c04536c611dcbf136fcb45495031e577f54f47412504d076d947356899290",
                rsp: "3c93350866490acc2a5bb5231a7bec4e97bc5957a0edfc0bde900132f4a3f6c0457359af2a3a17e5bee9832941e6b637",
            },
        )],
    );
    // XOR(SHA-256(plain), KDFa-SHA-384(sessionKey, "XOR", nonceTPM, nonceCaller, 256 bits)) and the
    // empty NULL-hierarchy ticket. The digest is that of the plaintext only if the request mask is
    // the SHA-384 one, and the response is the Python ciphertext only if the response mask is too.
    assert_eq!(
        hex_of(&r),
        "0020b10da8356a19e084e6c3959efdd23477cf71b6adcf69af65ebc51672ced5f1648024400000070000"
    );
}

/// A fresh TPM with an owner-bound SHA-384 session; sends `TPM2_HierarchyChangeAuth` with a
/// `new_len` octet newAuth (`seq(new_len, 0x40)`) under the pinned HMAC and returns the response.
fn hierarchy_change_auth_sha384(new_len: usize, hmac_hex: &str) -> Vec<u8> {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", b"owner-auth-384"));
    let s = start_pinned(
        &mut t,
        alg::SHA384,
        OWNER,
        &seq(48, 0x11),
        &u16b(alg::NULL),
        "3f64e53032fbb062370276a9b143893ba0b75566d5d0942f03f9402b234c48bffc7d74cb580374ade99168f98c8582f8",
    );
    run(
        &mut t,
        frame(
            st::SESSIONS,
            cc::HIERARCHY_CHANGE_AUTH,
            &[OWNER],
            &cat(&[
                &(4 + 2 + 48 + 1 + 2 + 48u32).to_be_bytes(),
                &s.to_be_bytes(),
                &tpm2b(&seq(48, 0x90)),
                &[CONTINUE],
                &tpm2b(&hex(hmac_hex)),
            ]),
            &tpm2b(&seq(new_len, 0x40)),
        ),
    )
}

#[test]
fn sha384_session_hierarchy_change_auth_keeps_the_32_octet_limit() {
    // The limit of newAuth is one SHA-256 digest (CONTEXT_INTEGRITY_HASH_SIZE), not the 48 octets of
    // the SHA-384 session that authorizes the command. The HMACs are Python `hmac` values
    // (hca_sha384.py in the task scratchpad), so the command is authorized and fails on the size.
    ok(&hierarchy_change_auth_sha384(
        32,
        "f3d78138c539a04f599a19da2913c2b98faf039cf210055b2193bfe329b288c2bc4c537eb6409218cddc2dc2cb5f1bba",
    ));
    err(
        &hierarchy_change_auth_sha384(
            33,
            "db659e23db5ffcbf1519d684ebd53f0e649dda44551bef26feb3579bae8568c8055d547ef843bff8045c8182c92da914",
        ),
        Rc::SIZE.in_param(1),
    );
    err(
        &hierarchy_change_auth_sha384(
            48,
            "f4afb90c2c131915df7d955de98141d9931b1bf9d2889071abb0b1f1a0d02b2c954217497f1bbdb5a2e5fa416586139e",
        ),
        Rc::SIZE.in_param(1),
    );
}

fn audit_pin_entry(s: u32, first: u8, pin: Pin) -> Entry {
    entry(s, seq(48, first), AUDIT, AUDIT_EXCLUSIVE, pin)
}

#[test]
fn sha384_session_audit_digest_uses_auth_hash() {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", b"owner-auth-audit"));
    let s = start_pinned(
        &mut t,
        alg::SHA384,
        OWNER,
        &seq(48, 0x41),
        &u16b(alg::NULL),
        "3f64e53032fbb062370276a9b143893ba0b75566d5d0942f03f9402b234c48bffc7d74cb580374ade99168f98c8582f8",
    );
    // auditDigest = SHA-384(zeros(48) || cpHash || rpHash) of TPM2_TestParms, then extended by the
    // second one (Python hashlib). cpHash = SHA-384(0000018A || parameters), rpHash =
    // SHA-384(00000000 || 0000018A).
    step(
        &mut t,
        &[],
        cc::TEST_PARMS,
        &[],
        &hex(TEST_PARMS),
        &[audit_pin_entry(
            s,
            0xC0,
            Pin {
                cmd: "b527f017db2620c4b45b06471465170d1278b855ed08f91ef2219bbd0543b5ef9270b5020279ddf207d6f57998b7624f",
                nonce: "fff9690bde8aad0397efc5f7e644b5aa762c04536c611dcbf136fcb45495031e577f54f47412504d076d947356899290",
                rsp: "95a2a3297b2e80df1e968d869c44d207b86e6e0591993cf8e3775dd3829b305c34b3eec5070d5aff5e32781df49644ce",
            },
        )],
    );
    assert_eq!(
        hex_of(&loaded_audit_digest(&t, s)),
        "4e50b0ea133d8e37f3202b048bb269cbb29e39787086e946db9420f88adbc73034d8680a6f47a8825d181034b28c8a6b"
    );
    assert_eq!(loaded_audit_digest(&t, s).len(), 48);
    step(
        &mut t,
        &[],
        cc::TEST_PARMS,
        &[],
        &hex(TEST_PARMS),
        &[audit_pin_entry(
            s,
            0xD0,
            Pin {
                cmd: "5549725fcbd11cf2aa31225eb74c9dc9f8c1aa533867a4b2750264d42cfef27925ad5ec2fe833f5604934f906058c4f7",
                nonce: "fccd26fce733b3c303a9b6f0d2075ab981a60e93e058340b063063baae90815254f3df92888bcceacf13040b35a30dee",
                rsp: "a905150be8f79abe6a0a7a75a80a09972edbfc093ab052be028b1f3483a57f6e1b982c1661acd1cc026e3697df2bfdd6",
            },
        )],
    );
    let digest = "aebd16edf156629cc9fd07dfcbfb7336c987c0aca775f85504bc1f3bfb4b61f42125887e4adf7e3abc05f4ff5e64d107";
    assert_eq!(hex_of(&loaded_audit_digest(&t, s)), digest);

    // TPM2_GetSessionAuditDigest reports the 48-octet digest (the attestation is not signed).
    let r = step(
        &mut t,
        &[b"", b""],
        cc::GET_SESSION_AUDIT_DIGEST,
        &[ENDORSEMENT, NULL, s],
        &hex("00000010"),
        &[entry(
            s,
            seq(48, 0xE0),
            AUDIT_EXCLUSIVE,
            AUDIT_EXCLUSIVE,
            Pin {
                cmd: "9773f7221f22ec41ea242cf5d065c6ce9296dfe6a06d648acbcf280e1f7f69c4cbf9452312be9bfc729a57c78a7e7343",
                nonce: "1022e553f3b01173d4c690288f28b26b84d6d9d9debec63eb52958bb73209a947ce28f1ab536439dc43a408e746fd3d5",
                rsp: "3e8bd3024b29d550609f9931f25f3816259f735ef4098d596981b84ca9f96bd8ac06339947bac9152b18e9c3582bb869",
            },
        )],
    );
    let mut result = Reader::new(&r);
    let attest = result.tpm2b(1024).unwrap();
    assert_eq!(result.u16().unwrap(), 0x0010);
    result.finish().unwrap();
    let mut a = Reader::new(attest);
    assert_eq!(a.u32().unwrap(), 0xff54_4347);
    assert_eq!(a.u16().unwrap(), 0x8016);
    a.tpm2b(68).unwrap();
    a.tpm2b(64).unwrap();
    a.bytes(17 + 8).unwrap();
    assert_eq!(a.u8().unwrap(), 1);
    assert_eq!(hex_of(a.tpm2b(64).unwrap()), digest);
    a.finish().unwrap();
}

#[test]
fn sha384_session_context_save_load_round_trip() {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", b"owner-auth-ctx"));
    let s = start_pinned(
        &mut t,
        alg::SHA384,
        OWNER,
        &seq(48, 0x61),
        &AES_SYM,
        "3f64e53032fbb062370276a9b143893ba0b75566d5d0942f03f9402b234c48bffc7d74cb580374ade99168f98c8582f8",
    );
    let slot = t.sessions.slot_of(s).unwrap();
    let key_before = t.sessions.get(slot).unwrap().key.as_slice().to_vec();
    let nonce_before = t.sessions.get(slot).unwrap().nonce_tpm.clone();
    let round_trip = |t: &mut Tpm| {
        let saved = run(t, plain(cc::CONTEXT_SAVE, &[s], &[]));
        ok(&saved);
        assert!(
            !t.sessions.is_loaded(slot),
            "ContextSave removes the session"
        );
        let loaded = run(t, plain(cc::CONTEXT_LOAD, &[], &saved[10..]));
        ok(&loaded);
        assert_eq!(loaded[10..], s.to_be_bytes());
        let session = t.sessions.get(slot).unwrap();
        assert_eq!(session.auth_hash, HashAlg::Sha384);
        assert_eq!(session.key.as_slice().len(), 48);
        assert_eq!(session.nonce_tpm.len(), 48);
    };
    // Bound, not audited: the key, the bound entity and the nonce come back.
    round_trip(&mut t);
    let session = t.sessions.get(slot).unwrap();
    assert_eq!(session.key.as_slice(), &key_before[..]);
    assert_eq!(session.nonce_tpm, nonce_before);
    assert!(session.bound.is_some());
    // The first audited command drops the binding and makes a 48-octet digest, which the next
    // save and load keep; the following commands continue the HMAC chain.
    step(
        &mut t,
        &[],
        cc::TEST_PARMS,
        &[],
        &hex(TEST_PARMS),
        &[audit_pin_entry(
            s,
            0x10,
            Pin {
                cmd: "52f68dbc901af68da312351b81f942bb394e8eea0e1aa09c03c618d5278a9d613b014cb0860a500a599a8c972ba43b35",
                nonce: "fff9690bde8aad0397efc5f7e644b5aa762c04536c611dcbf136fcb45495031e577f54f47412504d076d947356899290",
                rsp: "b6304e8df033ecae9b7156d2cdd3fc44f73f3b51e3097091ba2e985881c9b3b679f2d9d98cdf034ca6274735c07f4b45",
            },
        )],
    );
    round_trip(&mut t);
    let session = t.sessions.get(slot).unwrap();
    assert!(session.bound.is_none());
    assert_eq!(
        hex_of(session.audit_digest.unwrap().as_slice()),
        "4e50b0ea133d8e37f3202b048bb269cbb29e39787086e946db9420f88adbc73034d8680a6f47a8825d181034b28c8a6b"
    );
    step(
        &mut t,
        &[],
        cc::TEST_PARMS,
        &[],
        &hex(TEST_PARMS),
        &[audit_pin_entry(
            s,
            0x20,
            Pin {
                cmd: "e52693e3d345a4747d47a993ae8f6fed0b893b93747e89f49e55a701e27debced5c63bdf9ea152c85ac6f580d4a43e35",
                nonce: "fccd26fce733b3c303a9b6f0d2075ab981a60e93e058340b063063baae90815254f3df92888bcceacf13040b35a30dee",
                rsp: "4dd8d6fe95bc3820eefdee99046a56efb5597c10dff3fd223909bb30c95d12d5bc9e5594ae28255ff373625b6174d623",
            },
        )],
    );
    round_trip(&mut t);
    assert_eq!(
        hex_of(&loaded_audit_digest(&t, s)),
        "aebd16edf156629cc9fd07dfcbfb7336c987c0aca775f85504bc1f3bfb4b61f42125887e4adf7e3abc05f4ff5e64d107"
    );
}

/// ContextSave replies of every kind of SHA-256 session (the `authHash` nibble of the context
/// header is zero, so they are byte-identical to those of the commit before step D, cc1b4d6).
fn sha256_session_context_flow() -> Vec<String> {
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", b"owner pw"));
    let mut rows = Vec::new();
    let keep = |t: &mut Tpm, h: u32, rows: &mut Vec<String>| {
        let saved = run(t, plain(cc::CONTEXT_SAVE, &[h], &[]));
        ok(&saved);
        rows.push(sha256_hex(&saved));
        let loaded = run(t, plain(cc::CONTEXT_LOAD, &[], &saved[10..]));
        ok(&loaded);
        assert_eq!(loaded[10..], h.to_be_bytes());
    };
    let unbound = start_hmac(&mut t, NULL, b"", &[0x21; 16]);
    keep(&mut t, unbound.handle, &mut rows);
    let bound_cfb = start_sym(&mut t, OWNER, b"owner pw", &AES_SYM);
    keep(&mut t, bound_cfb.handle, &mut rows);
    let bound_xor = start_sym(&mut t, OWNER, b"owner pw", &[0x00, 0x0A, 0x00, 0x0B]);
    keep(&mut t, bound_xor.handle, &mut rows);
    let audited = start_hmac(&mut t, OWNER, b"owner pw", &[0x22; 20]);
    audit_once(&mut t, &audited);
    keep(&mut t, audited.handle, &mut rows);
    let trial = start_session(&mut t, se::TRIAL, NULL, b"", &[0x23; 16]);
    keep(&mut t, trial.handle, &mut rows);
    let policy = start_session(&mut t, se::POLICY, OWNER, b"owner pw", &[0x24; 16]);
    ok(&run(
        &mut t,
        plain(cc::POLICY_COMMAND_CODE, &[policy.handle], &u32b(cc::UNSEAL)),
    ));
    ok(&run(
        &mut t,
        plain(cc::POLICY_LOCALITY, &[policy.handle], &[1]),
    ));
    keep(&mut t, policy.handle, &mut rows);
    let auth_value = start_session(&mut t, se::POLICY, NULL, b"", &[0x25; 16]);
    ok(&run(
        &mut t,
        plain(cc::POLICY_AUTH_VALUE, &[auth_value.handle], &[]),
    ));
    keep(&mut t, auth_value.handle, &mut rows);
    rows
}

/// One audited `TPM2_TestParms` through a SHA-256 HMAC session (the first audited use drops a binding).
fn audit_once(t: &mut Tpm, s: &Hs) {
    let mut c = hc(cc::TEST_PARMS, &[], hex(TEST_PARMS), (NULL, &[]));
    c.attrs = AUDIT;
    let entry = hmac_auth_entry(s, &c, &[], false, false);
    let area = cat(&[&(entry.len() as u32).to_be_bytes(), &entry]);
    ok(&run(
        t,
        frame(st::SESSIONS, c.code, &c.handles, &area, &c.params),
    ));
}

// SHA-256 of every ContextSave reply of `sha256_session_context_flow`, taken on the commit before step D
// (HEAD cc1b4d6; the fixed entropy makes the replies reproducible).
const SHA256_CONTEXT_GOLDEN: [&str; 7] = [
    "cf1ef1ee1b8af01e9a4219ff7590544fdaa29a900ffd1dc6436bbf2ab70530fc",
    "c34b0a7f9f22ac4794ed5aae7055e9c34629d39e32cf1f57070ed7253e5152c0",
    "43847a1455700fac4060634ba7eaf86ff344be10503267f7f68d46109423570d",
    "b1d5afa67ff09949a62932f9b96448e9671ec03df28e49ea8520192e22af23af",
    "3db721c0cf8c5cfe5ce23ccc179f3c7d906926a81a4f2a8f36b7acefbefcb1d7",
    "9473eeae8befc1464f2dc6a43c01f78b9ab6f74fdc0d22da4254aa366c487961",
    "5f30899a3084238310e2a37a5387caaaa58cf593b9146b37b383f3042e094800",
];

#[test]
fn sha256_session_context_is_byte_identical_golden() {
    let flow = sha256_session_context_flow();
    assert_eq!(flow.len(), SHA256_CONTEXT_GOLDEN.len());
    for (i, (got, want)) in flow.iter().zip(SHA256_CONTEXT_GOLDEN).enumerate() {
        assert_eq!(got, want, "ContextSave reply {i}");
    }
}

#[test]
fn cp_hash_cache_per_algorithm() {
    // One command with two sessions of different authHash: cpHash and rpHash are computed per
    // algorithm (SHA-384 for the authorizing session, SHA-512 for the audit session) and both
    // HMACs are right; then the same with the two roles swapped.
    let mut t = started();
    ok(&set_auth(&mut t, OWNER, b"", b"owner-auth-7"));
    let s384 = start_pinned(
        &mut t,
        alg::SHA384,
        NULL,
        &seq(48, 0x11),
        &u16b(alg::NULL),
        "3f64e53032fbb062370276a9b143893ba0b75566d5d0942f03f9402b234c48bffc7d74cb580374ade99168f98c8582f8",
    );
    let s512 = start_pinned(
        &mut t,
        alg::SHA512,
        NULL,
        &seq(64, 0x12),
        &u16b(alg::NULL),
        "fff9690bde8aad0397efc5f7e644b5aa762c04536c611dcbf136fcb45495031e577f54f47412504d076d94735689929082fedcde13d710232629e0176aba2b32",
    );
    step(
        &mut t,
        &[],
        cc::HIERARCHY_CHANGE_AUTH,
        &[OWNER],
        &tpm2b(b"owner-auth-7b"),
        &[
            entry(
                s384,
                seq(48, 0x33),
                CONTINUE,
                CONTINUE,
                Pin {
                    cmd: "ad97dbd865d013c0a6d7f3a16307d695b282820eb3a606cdf540dfb1173e2c01af3afb7bdacca5ee8002a2bb2399a601",
                    nonce: "fccd26fce733b3c303a9b6f0d2075ab981a60e93e058340b063063baae90815254f3df92888bcceacf13040b35a30dee",
                    rsp: "fff051ac24b9e90a29d29350b780670a019eae51e8a1f7a3d9a1e05534ddd615df13329a46a9da1153107ab8e52f89c3",
                },
            ),
            entry(
                s512,
                seq(64, 0x44),
                AUDIT,
                AUDIT_EXCLUSIVE,
                Pin {
                    cmd: "75a4be507eef634a34c99e363c9422e2361fcaf4a52565d9073b1fa827084217cef76d29cf60a6f065ebc5021b68ab9ae36d7c8847a380f9c0dcb5bdedd48d74",
                    nonce: "1022e553f3b01173d4c690288f28b26b84d6d9d9debec63eb52958bb73209a947ce28f1ab536439dc43a408e746fd3d564ff87d42e8f5570cb983a99d753cc0c",
                    rsp: "a97e14ba3d76528e6d57c09677ee25fdb02e0de72bff5906da4c32165f0194352f37ff47c3db2cae02cbb9881804284a1f9ba11243e0a0e06833bd343a19d7ae",
                },
            ),
        ],
    );
    // SHA-512(zeros(64) || cpHash || rpHash) of the audited command (Python hashlib).
    assert_eq!(
        hex_of(&loaded_audit_digest(&t, s512)),
        "4819086cd6d68c433f9424daa2cca61a09e1eb608a0265fa894f62c6ca2bfd462fa6c85cec96b4eb0634b78dbe3a00815193cfcdf921e368d0f17a9b5e87ee82"
    );
    step(
        &mut t,
        &[],
        cc::HIERARCHY_CHANGE_AUTH,
        &[OWNER],
        &tpm2b(b"owner-auth-7c"),
        &[
            entry(
                s512,
                seq(64, 0x55),
                CONTINUE,
                CONTINUE,
                Pin {
                    cmd: "db1051ca905b5a65a63a9600d2dcb6c831665579f0d982e0ab885a1f9357ffd273495bad688a2873d66a5e22312e67c6aff6649e5a4091554a43099e4ff6836a",
                    nonce: "8026dbddae0a21ad3e94c22b2f61a2ce194fa8079c8ce5bf0fda4a2604212c77c5f2453ea30a576b382ef920946ccc1687a30c19852629f03ef336c5d0929026",
                    rsp: "277b550c2447c4233d8bfbc588da7249ffe812a5dbfdcb70d2f7e88b5354d09288add2d3f41810a2433ef014c60e7bb6d0b3e86c1d7f2feb3f5736de55f25cb3",
                },
            ),
            entry(
                s384,
                seq(48, 0x66),
                AUDIT,
                AUDIT_EXCLUSIVE,
                Pin {
                    cmd: "5c1402aa982e41db310456c908d7d9a10fd3838b8f769df4a2ad08b9be113c1e3beb1e2e754e96a033285aac859b6c52",
                    nonce: "8e27283311d62b4679cf808fba4c8c47945cacfb6eafd91201254bac5318b57a2bf92fae8a26fcb74301ed6302d0d61f",
                    rsp: "91276422cc597df9721dd9bfcce20f1bc5999e2089cd4aab9976fabc42daf02f1d867b2cc36303e09eb3a4804c7c67a5",
                },
            ),
        ],
    );
    assert_eq!(
        hex_of(&loaded_audit_digest(&t, s384)),
        "6ffee4f525793ab73e2b83494bae05c64f4baa89c1ed8d385eee1b6e554ed43545684fe1fe8f7f5f41d1db871ef22921"
    );
}

fn start_attempt(t: &mut Tpm, kind: u8, nonce_len: usize, auth_hash: u16) -> Vec<u8> {
    run(
        t,
        start_cmd(
            NULL,
            NULL,
            &start_params(&seq(nonce_len, 1), &[], kind, &u16b(alg::NULL), auth_hash),
        ),
    )
}

#[test]
fn auth_hash_unsupported_refused() {
    let mut t = started();
    // SHA3-256 (0x0027), SHA-1, SHA-256's neighbours and TPM_ALG_NULL: TPM_RC_HASH, parameter 5.
    for unsupported in [0x0027u16, 0x0004, 0x0012, 0x0010, 0x0000, 0xFFFF] {
        err(
            &start_attempt(&mut t, se::HMAC, 32, unsupported),
            Rc::HASH.in_param(5),
        );
    }
    // A policy or trial session takes the same hashes since step E, and no others.
    for kind in [se::POLICY, se::TRIAL] {
        for wide in [alg::SHA256, alg::SHA384, alg::SHA512] {
            ok(&start_attempt(&mut t, kind, 32, wide));
        }
        err(
            &start_attempt(&mut t, kind, 32, 0x0027),
            Rc::HASH.in_param(5),
        );
    }
    // The nonce is at least 16 and at most one digest of the authHash.
    for (hash, size) in [(alg::SHA256, 32), (alg::SHA384, 48), (alg::SHA512, 64)] {
        ok(&start_attempt(&mut t, se::HMAC, size, hash));
        ok(&start_attempt(&mut t, se::HMAC, 16, hash));
        err(
            &start_attempt(&mut t, se::HMAC, size + 1, hash),
            Rc::SIZE.in_param(1),
        );
        err(
            &start_attempt(&mut t, se::HMAC, 15, hash),
            Rc::SIZE.in_param(1),
        );
    }
    // Nothing but the accepted sessions was loaded.
    assert_eq!(t.sessions.loaded(), 6 + 6);
}

#[test]
fn session_nonce_and_hmac_sizes_follow_the_auth_hash() {
    let mut t = started();
    let mut session = |hash: u16, nonce_len: usize| {
        let resp = start_attempt(&mut t, se::HMAC, nonce_len, hash);
        ok(&resp);
        u32::from_be_bytes(resp[10..14].try_into().unwrap())
    };
    let (s256, s384, s512) = (
        session(alg::SHA256, 32),
        session(alg::SHA384, 48),
        session(alg::SHA512, 64),
    );
    // An audit session with a wrong HMAC: the size checks come first (TPM_RC_AUTHSIZE for a nonce or
    // an HMAC above one digest of the authHash of the session), then the HMAC (TPM_RC_BAD_AUTH).
    let attempt = |t: &mut Tpm, session: u32, nonce_len: usize, hmac_len: usize| {
        let entry = cat(&[
            &session.to_be_bytes(),
            &tpm2b(&seq(nonce_len, 0x70)),
            &[AUDIT],
            &tpm2b(&seq(hmac_len, 0x90)),
        ]);
        let area = cat(&[&(entry.len() as u32).to_be_bytes(), &entry]);
        run(
            t,
            frame(st::SESSIONS, cc::TEST_PARMS, &[], &area, &hex(TEST_PARMS)),
        )
    };
    for (s, digest) in [(s256, 32), (s384, 48), (s512, 64)] {
        err(&attempt(&mut t, s, 16, digest), Rc::BAD_AUTH.in_session(1));
        err(
            &attempt(&mut t, s, digest, digest),
            Rc::BAD_AUTH.in_session(1),
        );
        err(&attempt(&mut t, s, 16, digest + 1), Rc::AUTHSIZE);
        err(&attempt(&mut t, s, digest + 1, digest), Rc::AUTHSIZE);
    }
    // A password stays at 32 octets whatever the hash of the sessions.
    let password = |t: &mut Tpm, len: usize| {
        run(
            t,
            with_pw(
                cc::HIERARCHY_CHANGE_AUTH,
                &[OWNER],
                &[&seq(len, 1)],
                &tpm2b(b"x"),
            ),
        )
    };
    err(&password(&mut t, 32), Rc::BAD_AUTH.in_session(1));
    err(&password(&mut t, 33), Rc::AUTHSIZE);
    err(&password(&mut t, 48), Rc::AUTHSIZE);
}
