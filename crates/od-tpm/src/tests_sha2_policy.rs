//! Policy sessions of SHA-384 and SHA-512 (hash agility, step E). The expected digests, the
//! tickets and the signatures come from Python `hashlib`, `hmac` and OpenSSL (scripts in the task
//! scratchpad), not from the code under test. The TPM entropy is fixed, so the `nonceTPM` values
//! and the Names of the keys are inputs pinned here.

use super::tests_b::{
    NV0, define_raw, nv_public, nv_read_public, nv_write, pw_cmd, split_response, start_cmd,
    start_params,
};
use super::tests_obj::{
    create_child, create_primary, ecc, ecc_sig_scheme, ecc_sign, load, load_external, parse_child,
    parse_loaded, parse_primary, sign, sign_params, verify, with_ecc_parent,
};
use super::*;
use crate::codes::{object_attr as oa, se, session_attr};
use crate::crypto::{HashAlg, TpmDigest};
use crate::marshal::{Scheme, SymDef, TpmtPublic, Unique};
use crate::nv::attr as nv_attr;
use crate::policy::PolicyHashRestriction;

const NULL: u32 = handle::RH_NULL;
const OWNER: u32 = handle::RH_OWNER;

fn hex_of(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Starts a policy or trial session of `auth_hash` with a nonce of `nonce_len` octets; the handle
/// and the first `nonceTPM`.
fn start_policy(t: &mut Tpm, kind: u8, auth_hash: u16, nonce_len: usize) -> (u32, Vec<u8>) {
    let resp = run(
        t,
        start_cmd(
            NULL,
            NULL,
            &start_params(
                &vec![0x44; nonce_len],
                &[],
                kind,
                &u16b(alg::NULL),
                auth_hash,
            ),
        ),
    );
    ok(&resp);
    let mut r = Reader::new(&resp[14..]);
    let nonce = r.tpm2b(64).unwrap().to_vec();
    r.finish().unwrap();
    (u32::from_be_bytes(resp[10..14].try_into().unwrap()), nonce)
}

fn pol(t: &mut Tpm, code: u32, session: u32, p: &[u8]) -> Vec<u8> {
    run(t, plain(code, &[session], p))
}

fn session_digest(t: &mut Tpm, session: u32) -> Vec<u8> {
    let r = pol(t, cc::POLICY_GET_DIGEST, session, &[]);
    ok(&r);
    let mut rd = Reader::new(params(&r, 0));
    let d = rd.tpm2b(64).unwrap().to_vec();
    rd.finish().unwrap();
    d
}

fn or_params(list: &[&[u8]]) -> Vec<u8> {
    let mut p = u32b(list.len() as u32);
    for d in list {
        p.extend_from_slice(&tpm2b(d));
    }
    p
}

/// Selection of PCR 0 and PCR 4 in the SHA-256 bank.
const SEL04: &str = "00000001000b03110000";

/// The scripted SHA-256 flow whose raw responses are pinned in `GOLDEN`.
fn sha256_flow() -> Vec<(&'static str, String)> {
    let mut t = started();
    let mut log: Vec<(&'static str, String)> = Vec::new();
    let mut note = |label: &'static str, resp: &[u8]| {
        ok(resp);
        log.push((label, hex_of(resp)));
    };
    let start = run(
        &mut t,
        start_cmd(
            NULL,
            NULL,
            &start_params(&[0x44; 16], &[], se::POLICY, &u16b(alg::NULL), alg::SHA256),
        ),
    );
    note("start", &start);
    let s = u32::from_be_bytes(start[10..14].try_into().unwrap());
    let sel = hex(SEL04);
    let steps: Vec<(&'static str, u32, Vec<u8>)> = vec![
        ("auth_value", cc::POLICY_AUTH_VALUE, vec![]),
        ("command_code", cc::POLICY_COMMAND_CODE, u32b(cc::SIGN)),
        ("pcr", cc::POLICY_PCR, cat(&[&tpm2b(&[]), &sel])),
        ("locality", cc::POLICY_LOCALITY, vec![1]),
        ("cp_hash", cc::POLICY_CP_HASH, tpm2b(&[0x83; 32])),
    ];
    for (label, code, p) in &steps {
        note(label, &pol(&mut t, *code, s, p));
        note("digest", &pol(&mut t, cc::POLICY_GET_DIGEST, s, &[]));
    }
    let current = session_digest(&mut t, s);
    note(
        "or",
        &pol(
            &mut t,
            cc::POLICY_OR,
            s,
            &or_params(&[&current, &[0x21; 32]]),
        ),
    );
    note("digest", &pol(&mut t, cc::POLICY_GET_DIGEST, s, &[]));
    note("save", &run(&mut t, plain(cc::CONTEXT_SAVE, &[s], &[])));

    // A trial session reaches the assertions whose inputs a real session cannot fabricate.
    let trial = run(
        &mut t,
        start_cmd(
            NULL,
            NULL,
            &start_params(&[0x55; 32], &[], se::TRIAL, &u16b(alg::NULL), alg::SHA256),
        ),
    );
    note("trial_start", &trial);
    let tr = u32::from_be_bytes(trial[10..14].try_into().unwrap());
    let mut key_name = u16b(alg::SHA256);
    key_name.extend_from_slice(&[0x66; 32]);
    let object_name = cat(&[&u16b(alg::SHA256), &[0x77; 32]]);
    let parent_name = cat(&[&u16b(alg::SHA256), &[0x78; 32]]);
    let duplicate_names = cat(&[&tpm2b(&object_name), &tpm2b(&parent_name), &[1]]);
    let trial_steps: Vec<(&'static str, u32, Vec<u8>)> =
        vec![("trial_name_hash", cc::POLICY_NAME_HASH, tpm2b(&[0x91; 32]))];
    for (label, code, p) in &trial_steps {
        note(label, &pol(&mut t, *code, tr, p));
        note("digest", &pol(&mut t, cc::POLICY_GET_DIGEST, tr, &[]));
    }
    note("trial_restart", &pol(&mut t, cc::POLICY_RESTART, tr, &[]));
    note(
        "trial_dup",
        &pol(&mut t, cc::POLICY_DUPLICATION_SELECT, tr, &duplicate_names),
    );
    note("digest", &pol(&mut t, cc::POLICY_GET_DIGEST, tr, &[]));
    let authorize = cat(&[
        &tpm2b(&[0x31; 32]),
        &tpm2b(b"ref"),
        &tpm2b(&key_name),
        &u16b(0x8022),
        &u32b(NULL),
        &tpm2b(&[]),
    ]);
    note(
        "trial_authorize",
        &pol(&mut t, cc::POLICY_AUTHORIZE, tr, &authorize),
    );
    note("digest", &pol(&mut t, cc::POLICY_GET_DIGEST, tr, &[]));
    let counter = cat(&[&tpm2b(&[0, 0, 0, 0, 0, 0, 0, 9]), &u16b(0), &u16b(5)]);
    note(
        "trial_counter_timer",
        &pol(&mut t, cc::POLICY_COUNTER_TIMER, tr, &counter),
    );
    note("digest", &pol(&mut t, cc::POLICY_GET_DIGEST, tr, &[]));
    note(
        "trial_save",
        &run(&mut t, plain(cc::CONTEXT_SAVE, &[tr], &[])),
    );
    log
}

/// The raw responses of `sha256_flow` taken from the code at commit aee7045 (before SHA-384 and
/// SHA-512 policy digests); the policy digests inside were also recomputed with Python `hashlib`
/// (`H(policyDigest || commandCode || data)`). The context blobs are deterministic: the entropy is
/// fixed.
const GOLDEN: [(&str, &str); 25] = [
    (
        "start",
        "800100000020000000000300000000103f64e53032fbb062370276a9b143893b",
    ),
    ("auth_value", "80010000000a00000000"),
    (
        "digest",
        "80010000002c0000000000208fcd2169ab92694e0c633f1ab772842b8241bbc20288981fc7ac1eddc1fddb0e",
    ),
    ("command_code", "80010000000a00000000"),
    (
        "digest",
        "80010000002c000000000020d9979a6b278c1d135ce124837caf9de446d714718eee9e3620b58c80a043a953",
    ),
    ("pcr", "80010000000a00000000"),
    (
        "digest",
        "80010000002c000000000020dcf06902ab6fa1d1a1e98e52ab1fc3b462e2f27462efc4beaed7b37696ff2dc4",
    ),
    ("locality", "80010000000a00000000"),
    (
        "digest",
        "80010000002c0000000000201c07d1356df82fe30344d49ed9e48b9436bcb295debd58decb8ff24679bb111c",
    ),
    ("cp_hash", "80010000000a00000000"),
    (
        "digest",
        "80010000002c00000000002092ac919153a319e2c9cd37d43ea21feb79785733cb1c5d7a3d59b0e36165737a",
    ),
    ("or", "80010000000a00000000"),
    (
        "digest",
        "80010000002c000000000020e7d478d1c3f97fb27c16cb17828fcc6ced6dad0987f49ded4859dd06817e3152",
    ),
    (
        "save",
        "8001000000c6000000000000000000000001030000004000000700aa0020727f775f7a81293b9848d1920ed3704280a1d6996851764e465c695f6a0aeb5535f3cc92fe7f6f56f34fcb13019643990f4efbaa35453f55b2f91fa97477cd6489e42d539e63a70ba2490a0dc334ad9f3b0122027dc69d35bdf9d25f14bf582fa1332ef936c89f96f70fa22254cdd882c422507996ceeaaf50c755f4d3badbada3893e99c2b06096660e9a70b6b643f2ea80931386b293663567b161910fc1b38cd85a9c36bb0785",
    ),
    (
        "trial_start",
        "8001000000300000000003000001002040dfabbb2ce0a1e4be5ea5e06355c69280d8a3c1e294401e5fe8d6ceac34440a",
    ),
    ("trial_name_hash", "80010000000a00000000"),
    (
        "digest",
        "80010000002c000000000020bfd93bf39b2b89d7ce5dc692c8b2fe26e72c0be4e4609cad205a5ad27fd80e34",
    ),
    ("trial_restart", "80010000000a00000000"),
    ("trial_dup", "80010000000a00000000"),
    (
        "digest",
        "80010000002c000000000020a75b85a4be8f552ba69e37bc3a07d890d305d98f0e1aa738529399d5148e49b5",
    ),
    ("trial_authorize", "80010000000a00000000"),
    (
        "digest",
        "80010000002c00000000002008607d311e51e0d9821bdecb58c1742e38a7478f2107d30c5ecb9bd66f06c399",
    ),
    ("trial_counter_timer", "80010000000a00000000"),
    (
        "digest",
        "80010000002c000000000020a0269ca4cbb1e7c0cd31437b23532d03f14d279223311a18f5a47207b6ed6382",
    ),
    (
        "trial_save",
        "80010000009300000000000000000000000203000001400000070077002053ecaf9777d2c035cd8630c76e8336c5cb7eb62ea8ffccd9ed5e54800bbd22c44c58b709f287881d6ce8b99eb148d8307e9cf74a4fe32fa9fa3ed8e9b6b0298863f424ec89e7ab6bb5dc75118ad3cac2038991b9a72487c0664ccece3e400938c1cb3f80da5b0a7b23111b869f3f7ace877925769b",
    ),
];

#[test]
fn sha256_policy_golden_unchanged() {
    let flow = sha256_flow();
    assert_eq!(flow.len(), GOLDEN.len());
    for ((label, resp), (golden_label, golden)) in flow.iter().zip(GOLDEN) {
        assert_eq!(*label, golden_label);
        assert_eq!(resp, golden, "{label}");
    }
}

// ------------------------------------------------------------------------------- vectors

/// Python: `sha256("abc")`, the digest extended into PCR 4 of the SHA-256 bank.
const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
/// Selection of PCR 0 and PCR 4 in the SHA-256 bank and in the SHA-384 bank.
const SEL_SHA256_04: &str = "00000001000b03110000";
const SEL_SHA384_04: &str = "00000001000c03110000";
/// Python: `SHA-384(PCR0 || PCR4)` of the SHA-256 bank (PCR 4 extended with `sha256("abc")`).
const P1: &str = "a352cd0bee5c00d2890597add212d9e8f46657efabc36fe8b61420c15781d02c7e7c93e42ef1eb94085a4fedda02ca2d";
/// Python: `SHA-384(48 x 00 || 48 x 00)`, PCR 0 and PCR 4 of the SHA-384 bank.
const P2: &str = "f57bb7ed82c6ae4a29e6c9879338c592c7d42a39135583e8ccbe3940f2344b0eb6eb8503db0ffd6a39ddd00cd07d8317";
/// Python: `H384(0^48 || 0000017F || SEL_SHA256_04 || P1)`.
const D1: &str = "5331d090b388c40f3ffc708aecddb7fe236c6c1c36633a35d8ff2405da4fe17e88d35dcb709a969dd492ae71421405c0";
/// Python: `H384(D1 || 0000017F || SEL_SHA384_04 || P2)`.
const D2: &str = "2041282435e54838872a8a502a1db63ef3377870cff7135d8f1d0450aaf88080dc6361634e33f881a99cb8455282da8a";
/// Python: `H384(D2 || 0000016C || 0000015D)` (PolicyCommandCode(TPM2_Sign)).
const D3: &str = "de2630fe8a2424c7c6fc8d2f03bd0c744dfceaaafed6026b43cee898ae88c931e632ceb1ef1fc5f27d2a932e365ad5b3";
/// Python: `H384(0^48 || 0000016C || 0000015D)`.
const CC_SIGN_384: &str = "5093ebce18eacac2fef5a1f4dcb9c8167ee1f45479b189ea362e20f9f86cc93deb6ca4270098915db6dec7d56c9f1979";
/// Python: `H384(CC_SIGN_384 || 0000016E || 48 x 83)` (PolicyCpHash).
const CC_SIGN_CP_384: &str = "be1ee73cf41a0811830c125bf67f3c3b2288e2ea774fcc4450d042b4549edb646f91526e60d5f723bdb0f58904e4710c";
/// Python: `H512(0^64 || 0000016C || 0000015D)`.
const E1: &str = "202de4e365f17cbcdfb5c522ad3119915d2f81be945135627f5a76a7183d8b23c1b768dfb13381a4ccb17593bc0957605e24c9eaea04b97d50897f03f984cd9d";
/// Python: `H512(E1 || 0000016B)` (PolicyAuthValue).
const E2: &str = "910cbff87a1f237aaedb1eb9e3aaeb85378dbf67bd1f10913deae9b50f84b98f91fd13f2d910a088db511fb3bf118e379cf816558089f585f22bfb18e4295546";
/// Python: `H512(0^64 || 00000171 || E2 || 64 x 21)` (PolicyOR).
const E3: &str = "cce5e51304d57f2fcc0c8b04bba41f13a4b7680927b76c702d0a711559fe9edd81855d6eea26219f21aae04143c62acedd4b13faca33db68127a7a17e61e622d";
/// Python: `H384(0^48 || 0000016F || 01)` (PolicyLocality(1)), the policy the authority approves.
const APPROVED: &str = "3c6a526ff03d42da670f1cba535f9f1ea8a27e3c810fdb56f4470432b1da5f9ded3e5e330fc094026082a7018a1005ae";
/// Python: `SHA-256(APPROVED || "ref-384")`, the digest the authority signs.
const SIGNED_SHA256: &str = "456ccce3b1851ed67ee2975121a2e8e3ad2e2e80443718a7d5366a7dd036df34";
/// Python: `H384(0^48 || 0000017F || 00000000 || H384(""))`, `H512` likewise for 64 octets.
const PCR_EMPTY_384: &str = "656932859ff2614df96224c5d9e8e6827fcdf46e7e74e367b43c43f38ba78c6125ec7eb3b975be89f582deb8e580849e";
const PCR_EMPTY_512: &str = "598589262e1e6613e7e0b9389bc01b025aed52f670bc15e9cfef6769fe2e191b1962d3f6fca12788c7011b4f6797178823f38efdd29c6d3bfa41b1dbe4df4b0f";
/// Python: `H384(0^48 || 0000017F || SEL_SHA384_04 || P2)` and the SHA-512 counterpart.
const Q384: &str = "09f7ba4da0b3d02d1aad8001e53203758baa0cd004721a5aea91eb5d118c1605ec862e5ea98ff50ba7808af15d3c083f";
const Q512: &str = "74a0f12c9e7933d642629890ede6016dbdb075e784d87893e342a7a625fface5db6b4fd3efeacc1ad97584bb545e2486b4ed8146d4329d742257c4fe2d2d9377";
/// Python: `H384(0^48 || 0000017F || SEL_SHA256_04 || 48 x 45)`.
const TRIAL384: &str = "5fefa249ae0a72e2846c0de7c63a44a1cb93811c4edb4d6a9bf81a5a3e9b06d395b478aafc0025612b5d5b28e3069fb6";
/// Python: `H(H(0^n || 00000151 || 40000001) || "ref-384")` for SHA-384 and SHA-512 (PolicySecret of the
/// owner, replayed by PolicyTicket).
const TICKET_384: &str = "173bd34abcd36c37b3566092376ae9f797e6b5af000a9d5d2b8003c8b39644f33e9384a76984517196f53e7d0cadeebc";
const TICKET_512: &str = "90c8346fd808a06fe05197bfc36761d0778395f103899764e475149ce00f33ea2b15d532753bab710d15e9f49f7e901a3af17e3522fade829472d4b9e60a1e4c";
/// The Name of the NV index and of the authority primary come from the TPM (inputs); the digests
/// below were computed from them with Python: `H384(0^48 || 0000016A || name)` then `H384(. || "ref-384")`,
/// and `H384(0^48 || 00000192 || nvName)`.
const AUTHORITY_NAME: &str = "000cacb6d4a3739925ead7b637dd41bc9ce1b6e7c44882103eaa848fe5e6f1e44092cc196505a9b9e66f04ed0dcd817239bd";
const AUTHORIZED_384: &str = "78d802a6f224f6f74282c8c037460157870fa8a1791699131ed9b9f2bf21feebf15c570d4ef106e71033b8da8a9e4b44";
const NV_NAME: &str = "000bb0e3c42ed910b5037913964b1894889038ae504011d334f18cf33807c028becf";
const AUTHORIZED_NV_384: &str = "b349d3f3e63cbf5136478e6b818200c25e8dd8415a9dd95389238358ad1e97077d6957233715a39a153ddaa2dd8fd0c4";

/// A signing key of the PolicySigned authority, made with OpenSSL (`openssl ecparam`, P-256); the
/// signatures are `openssl dgst -shaNNN -sign` over the authorization message
/// `nonceTPM || expiration || cpHashA || policyRef` (empty nonce, expiration 0), checked with
/// `openssl dgst -verify`. The Name is `nameAlg || SHA-256(TPMT_PUBLIC)`, assembled by hand in Python.
const KEY_X: &str = "63a3949c48a01c8b710dbbee124986ac72cb6ff8016829416a065a2adc786a93";
const KEY_Y: &str = "b91cd1ee4ee100b8af5e65e4978bc6abb13b3aac0bdfcf9518bc7832bb39cb4c";

struct SignedVector {
    hash: u16,
    session: u16,
    r: &'static str,
    s: &'static str,
    name: &'static str,
    cp: &'static str,
    reference: &'static [u8],
    digest: &'static str,
}

/// Python: `aHash = H(msg)` was signed; `digest = H(H(0 || 00000160 || name) || ref)`.
const SIGNED: [SignedVector; 2] = [
    SignedVector {
        hash: alg::SHA384,
        session: alg::SHA384,
        r: "243b9e7d38137840ec991924ac216446ea9895e16b03a1eea955e95acdfe8287",
        s: "92cc2f7e2b77794744b4df0f6775d9482544b1817f769b9a06ddfd017cb24b25",
        name: "000bd5f563ae0252306088d979df664bcbd69c827648eb1dc743c57d2184426c93dd",
        cp: "838383838383838383838383838383838383838383838383838383838383838383838383838383838383838383838383",
        reference: b"ref-384",
        digest: "bf1700b0083e7e793d01cf497ebcb8ca91952b427840f538ff562a298bc4c387187620569c619a8635113ee8bd070a8d",
    },
    SignedVector {
        hash: alg::SHA512,
        session: alg::SHA512,
        r: "1bf10e1f1eb583aaa2cc8d4b3baf267b7ab10ccc87dccc165e23fe1079166d11",
        s: "1a274ff94141f655e8351fd17e59606f782f48a55078122f4f0ba1541358b72b",
        name: "000b6c31af0944ee66c69f998dfee10d16555c40dd9596c9e44045ab96970ac497fa",
        cp: "83838383838383838383838383838383838383838383838383838383838383838383838383838383838383838383838383838383838383838383838383838383",
        reference: b"ref-512",
        digest: "de67f3e3e9982c6e59708215809aa3509e9071c97e88cd1a5579dcb81b98cb0549d655c85299a4a3dc7f1f26a68b1e366c1292fcba1e7db08dd82ede550d2bb9",
    },
];

// --------------------------------------------------------------------------------- helpers

const CONTINUE: u8 = session_attr::CONTINUE_SESSION;

fn digest_size(auth_hash: u16) -> usize {
    HashAlg::from_alg(auth_hash).unwrap().size()
}

fn save(t: &mut Tpm, h: u32) -> Vec<u8> {
    let resp = run(t, plain(cc::CONTEXT_SAVE, &[h], &[]));
    ok(&resp);
    params(&resp, 0).to_vec()
}

fn load_ctx(t: &mut Tpm, ctx: &[u8]) -> u32 {
    let resp = run(t, plain(cc::CONTEXT_LOAD, &[], ctx));
    ok(&resp);
    u32::from_be_bytes(resp[10..14].try_into().unwrap())
}

fn loaded_session_hash(t: &Tpm, session: u32) -> (HashAlg, HashAlg) {
    let slot = t.sessions.slot_of(session).unwrap();
    let session = t.sessions.get(slot).unwrap();
    (session.auth_hash, session.policy.hash)
}

/// `TPM2_Sign` of a fixed SHA-256 digest, authorized by a policy session with an empty HMAC (an
/// unbound session whose policy does not ask for the authValue has nothing to prove).
fn sign_under_policy(t: &mut Tpm, key: u32, session: u32) -> Vec<u8> {
    let entry = cat(&[
        &u32b(session),
        &tpm2b(&[0x99; 16]),
        &[CONTINUE],
        &tpm2b(&[]),
    ]);
    let area = cat(&[&u32b(entry.len() as u32), &entry]);
    run(
        t,
        frame(
            st::SESSIONS,
            cc::SIGN,
            &[key],
            &area,
            &sign_params(&[7u8; 32], &ecc_sig_scheme()),
        ),
    )
}

/// A signing child of the parent whose `authPolicy` is `policy` and whose `nameAlg` is the hash
/// with that digest size.
fn policy_child(t: &mut Tpm, parent: u32, policy: &[u8]) -> u32 {
    // The shared helpers read a Name of at most 64 octets; a SHA-512 Name has 66.
    let mut public = ecc_sign();
    public.attributes &= !oa::USER_WITH_AUTH;
    public.name_alg = match policy.len() {
        48 => alg::SHA384,
        64 => alg::SHA512,
        _ => alg::SHA256,
    };
    public.auth_policy = policy.to_vec();
    let created = create_child(t, parent, b"parent-pw", b"kpw", b"", &public);
    ok(&created);
    let child = parse_child(&created);
    let loaded = load(t, parent, b"parent-pw", &child.private, &child.public);
    ok(&loaded);
    u32::from_be_bytes(loaded[10..14].try_into().unwrap())
}

fn assert_signs(t: &mut Tpm, key: u32, session: u32) {
    let resp = sign_under_policy(t, key, session);
    let (signature, _) = split_response(&resp, 0);
    ok(&verify(t, key, &[7u8; 32], &signature));
}

fn pcr_params(pcr_digest: &[u8], selection: &str) -> Vec<u8> {
    cat(&[&tpm2b(pcr_digest), &hex(selection)])
}

// ------------------------------------------------------------------------------------ tests

#[test]
fn sha384_policy_session_satisfies_48_octet_auth_policy() {
    let (mut t, parent) = with_ecc_parent();
    ok(&extend(&mut t, 4, &hex(ABC_SHA256)));
    let key = policy_child(&mut t, parent, &hex(D3));
    let (s, nonce) = start_policy(&mut t, se::POLICY, alg::SHA384, 48);
    assert_eq!(nonce.len(), 48);
    assert_eq!(session_digest(&mut t, s), vec![0u8; 48]);

    // The PCR digest of the SHA-256 bank is hashed with SHA-384 (the hash of the session)...
    ok(&pol(
        &mut t,
        cc::POLICY_PCR,
        s,
        &pcr_params(&[], SEL_SHA256_04),
    ));
    assert_eq!(session_digest(&mut t, s), hex(D1));
    // ...and the PCR values of the SHA-384 bank are 48 octets each.
    ok(&pol(
        &mut t,
        cc::POLICY_PCR,
        s,
        &pcr_params(&hex(P2), SEL_SHA384_04),
    ));
    assert_eq!(session_digest(&mut t, s), hex(D2));
    ok(&pol(&mut t, cc::POLICY_COMMAND_CODE, s, &u32b(cc::SIGN)));
    assert_eq!(session_digest(&mut t, s), hex(D3));
    assert_eq!(session_digest(&mut t, s).len(), 48);

    // The 48-octet authPolicy is met; the session policy starts again afterwards.
    assert_signs(&mut t, key, s);
    assert_eq!(session_digest(&mut t, s), vec![0u8; 48]);
    err(
        &sign_under_policy(&mut t, key, s),
        Rc::POLICY_FAIL.in_session(1),
    );

    // A SHA-256 session cannot meet a 48-octet authPolicy, even with the same assertions.
    let (narrow, _) = start_policy(&mut t, se::POLICY, alg::SHA256, 16);
    ok(&pol(
        &mut t,
        cc::POLICY_COMMAND_CODE,
        narrow,
        &u32b(cc::SIGN),
    ));
    assert_eq!(session_digest(&mut t, narrow).len(), 32);
    err(
        &sign_under_policy(&mut t, key, narrow),
        Rc::POLICY_FAIL.in_session(1),
    );
    // Another assertion chain of a SHA-384 session does not either.
    let (other, _) = start_policy(&mut t, se::POLICY, alg::SHA384, 16);
    ok(&pol(
        &mut t,
        cc::POLICY_COMMAND_CODE,
        other,
        &u32b(cc::SIGN),
    ));
    assert_eq!(session_digest(&mut t, other), hex(CC_SIGN_384));
    err(
        &sign_under_policy(&mut t, key, other),
        Rc::POLICY_FAIL.in_session(1),
    );
    // A trial session of SHA-384 calculates the same digest and never authorizes.
    let (trial, _) = start_policy(&mut t, se::TRIAL, alg::SHA384, 16);
    ok(&pol(
        &mut t,
        cc::POLICY_PCR,
        trial,
        &pcr_params(&[], SEL_SHA256_04),
    ));
    ok(&pol(
        &mut t,
        cc::POLICY_PCR,
        trial,
        &pcr_params(&hex(P2), SEL_SHA384_04),
    ));
    ok(&pol(
        &mut t,
        cc::POLICY_COMMAND_CODE,
        trial,
        &u32b(cc::SIGN),
    ));
    assert_eq!(session_digest(&mut t, trial), hex(D3));
    err(
        &sign_under_policy(&mut t, key, trial),
        Rc::ATTRIBUTES.in_session(1),
    );
}

#[test]
fn sha512_policy_session_digest_is_64_octets() {
    let (mut t, parent) = with_ecc_parent();
    let key = policy_child(&mut t, parent, &hex(E1));
    let (s, nonce) = start_policy(&mut t, se::POLICY, alg::SHA512, 64);
    assert_eq!(nonce.len(), 64);
    let raw = pol(&mut t, cc::POLICY_GET_DIGEST, s, &[]);
    ok(&raw);
    assert_eq!(raw.len(), 10 + 2 + 64);
    assert_eq!(session_digest(&mut t, s), vec![0u8; 64]);

    ok(&pol(&mut t, cc::POLICY_COMMAND_CODE, s, &u32b(cc::SIGN)));
    assert_eq!(session_digest(&mut t, s), hex(E1));
    // PolicyRestart returns to a Zero Digest of 64 octets.
    ok(&pol(&mut t, cc::POLICY_RESTART, s, &[]));
    assert_eq!(session_digest(&mut t, s), vec![0u8; 64]);
    ok(&pol(&mut t, cc::POLICY_COMMAND_CODE, s, &u32b(cc::SIGN)));
    // The 64-octet authPolicy of a SHA-512 object is met.
    assert_signs(&mut t, key, s);

    // PolicyAuthValue and PolicyOR: the list holds digests of 64 octets and the new digest starts
    // from a Zero Digest of 64 octets.
    ok(&pol(&mut t, cc::POLICY_COMMAND_CODE, s, &u32b(cc::SIGN)));
    ok(&pol(&mut t, cc::POLICY_AUTH_VALUE, s, &[]));
    assert_eq!(session_digest(&mut t, s), hex(E2));
    let other = [0x21u8; 64];
    err(
        &pol(&mut t, cc::POLICY_OR, s, &or_params(&[&other, &other])),
        Rc::VALUE.in_param(1),
    );
    ok(&pol(
        &mut t,
        cc::POLICY_OR,
        s,
        &or_params(&[&hex(E2), &other]),
    ));
    assert_eq!(session_digest(&mut t, s), hex(E3));

    // A trial session of SHA-512: an empty PCR selection hashes the empty string with SHA-512.
    let (trial, _) = start_policy(&mut t, se::TRIAL, alg::SHA512, 32);
    ok(&pol(
        &mut t,
        cc::POLICY_PCR,
        trial,
        &pcr_params(&[], "00000000"),
    ));
    assert_eq!(session_digest(&mut t, trial), hex(PCR_EMPTY_512));
    // And SHA-384 likewise.
    let (trial, _) = start_policy(&mut t, se::TRIAL, alg::SHA384, 32);
    ok(&pol(
        &mut t,
        cc::POLICY_PCR,
        trial,
        &pcr_params(&[], "00000000"),
    ));
    assert_eq!(session_digest(&mut t, trial), hex(PCR_EMPTY_384));
}

#[test]
fn sha384_policy_session_context_round_trip() {
    let (mut t, parent) = with_ecc_parent();
    let key = policy_child(&mut t, parent, &hex(CC_SIGN_384));

    // A policy session of SHA-384 keeps its hash, its digest and its use across ContextSave and
    // ContextLoad; the same for a trial session and for SHA-512.
    let (s, _) = start_policy(&mut t, se::POLICY, alg::SHA384, 48);
    ok(&pol(&mut t, cc::POLICY_COMMAND_CODE, s, &u32b(cc::SIGN)));
    let context = save(&mut t, s);
    err(
        &pol(&mut t, cc::POLICY_GET_DIGEST, s, &[]),
        Rc::reference_handle(0),
    );
    assert_eq!(load_ctx(&mut t, &context), s);
    assert_eq!(
        loaded_session_hash(&t, s),
        (HashAlg::Sha384, HashAlg::Sha384)
    );
    assert_eq!(session_digest(&mut t, s), hex(CC_SIGN_384));
    assert_signs(&mut t, key, s);

    // A hash restriction (cpHash) of 48 octets travels with the session.
    let (r, _) = start_policy(&mut t, se::POLICY, alg::SHA384, 16);
    ok(&pol(&mut t, cc::POLICY_COMMAND_CODE, r, &u32b(cc::SIGN)));
    ok(&pol(&mut t, cc::POLICY_CP_HASH, r, &tpm2b(&[0x83; 48])));
    assert_eq!(session_digest(&mut t, r), hex(CC_SIGN_CP_384));
    let context = save(&mut t, r);
    assert_eq!(load_ctx(&mut t, &context), r);
    assert_eq!(session_digest(&mut t, r), hex(CC_SIGN_CP_384));
    let slot = t.sessions.slot_of(r).unwrap();
    let pinned = TpmDigest::from_output(&[0x83; 48]);
    assert_eq!(
        t.policy_use_check(
            slot,
            (cc::SIGN, NULL),
            |rule| rule == PolicyHashRestriction::CpHash(pinned),
            &hex(CC_SIGN_CP_384),
            false,
            1
        ),
        Ok((false, false))
    );
    assert_eq!(
        t.policy_use_check(
            slot,
            (cc::SIGN, NULL),
            |_| false,
            &hex(CC_SIGN_CP_384),
            false,
            1
        ),
        Err(Rc::POLICY_FAIL.in_session(1))
    );

    for (kind, hash) in [
        (se::TRIAL, alg::SHA384),
        (se::POLICY, alg::SHA512),
        (se::TRIAL, alg::SHA512),
    ] {
        let (x, _) = start_policy(&mut t, kind, hash, 16);
        ok(&pol(&mut t, cc::POLICY_COMMAND_CODE, x, &u32b(cc::SIGN)));
        let before = session_digest(&mut t, x);
        assert_eq!(before.len(), digest_size(hash));
        let context = save(&mut t, x);
        assert_eq!(load_ctx(&mut t, &context), x);
        assert_eq!(session_digest(&mut t, x), before);
        let hash = HashAlg::from_alg(hash).unwrap();
        assert_eq!(loaded_session_hash(&t, x), (hash, hash));
        // A used-up context cannot be loaded again.
        err(
            &run(&mut t, plain(cc::CONTEXT_LOAD, &[], &context)),
            Rc::HANDLE.in_param(1),
        );
    }
}

/// Parameters of `TPM2_PolicyAuthorize`.
fn authorize_args(approved: &[u8], reference: &[u8], name: &[u8], ticket: &[u8]) -> Vec<u8> {
    cat(&[&tpm2b(approved), &tpm2b(reference), &tpm2b(name), ticket])
}

#[test]
fn policy_authorize_with_sha384_key_name_and_verified_ticket() {
    let mut t = started();
    let mut template = ecc_sign();
    template.name_alg = alg::SHA384;
    let authority = parse_primary(&create_primary(&mut t, OWNER, b"authority", &template));
    assert_eq!(
        authority.name.len(),
        50,
        "a SHA-384 Name is nameAlg and 48 octets"
    );
    assert_eq!(hex_of(&authority.name), AUTHORITY_NAME);
    let reference = b"ref-384";
    let null_ticket = cat(&[&u16b(st::VERIFIED), &u32b(NULL), &tpm2b(b"")]);

    // The approved policy is a 48-octet SHA-384 digest of a real session.
    let (real, _) = start_policy(&mut t, se::POLICY, alg::SHA384, 16);
    ok(&pol(&mut t, cc::POLICY_LOCALITY, real, &[1]));
    let approved = session_digest(&mut t, real);
    assert_eq!(approved, hex(APPROVED));
    // The authority signs SHA-256(approved || policyRef): the verified ticket (an HMAC-SHA-256 under
    // the hierarchy proof, as before hash agility) is made by VerifyDigestSignature.
    let signed = hex(SIGNED_SHA256);
    let signature = params(
        &sign(
            &mut t,
            authority.handle,
            b"authority",
            &signed,
            &ecc_sig_scheme(),
        ),
        0,
    )
    .to_vec();
    let verified = run(
        &mut t,
        plain(
            cc::VERIFY_DIGEST_SIGNATURE,
            &[authority.handle],
            &cat(&[&tpm2b(&[]), &tpm2b(&signed), &signature]),
        ),
    );
    ok(&verified);
    let ticket = params(&verified, 0).to_vec();
    // tag, hierarchy, hash and the 32-octet HMAC-SHA-256.
    assert_eq!(ticket.len(), 2 + 4 + 2 + 2 + 32);
    assert_eq!(&ticket[8..10], &[0, 32]);

    for (parameters, expected) in [
        // An approved policy of 32 octets is not the digest of the session.
        (
            authorize_args(&approved[..32], reference, &authority.name, &ticket),
            Rc::VALUE.in_param(1),
        ),
        (
            authorize_args(&approved, b"altered", &authority.name, &ticket),
            Rc::POLICY,
        ),
        (
            authorize_args(&approved, reference, &authority.name, &null_ticket),
            Rc::POLICY,
        ),
        (
            authorize_args(&approved, reference, &[0, 0x10], &ticket),
            Rc::HASH.in_param(3),
        ),
        (
            authorize_args(&approved, reference, &authority.name[..49], &ticket),
            Rc::SIZE.in_param(3),
        ),
    ] {
        err(
            &pol(&mut t, cc::POLICY_AUTHORIZE, real, &parameters),
            expected,
        );
        assert_eq!(session_digest(&mut t, real), approved);
    }
    // A legacy ticket for a SHA-384 key covers a SHA-384 digest of the approved policy, which this
    // TPM cannot issue (VerifySignature takes SHA-256 digests only): not valid.
    let legacy = run(
        &mut t,
        plain(
            cc::VERIFY_SIGNATURE,
            &[authority.handle],
            &cat(&[&tpm2b(&signed), &signature]),
        ),
    );
    ok(&legacy);
    err(
        &pol(
            &mut t,
            cc::POLICY_AUTHORIZE,
            real,
            &authorize_args(&approved, reference, &authority.name, params(&legacy, 0)),
        ),
        Rc::POLICY,
    );
    ok(&pol(
        &mut t,
        cc::POLICY_AUTHORIZE,
        real,
        &authorize_args(&approved, reference, &authority.name, &ticket),
    ));
    assert_eq!(session_digest(&mut t, real), hex(AUTHORIZED_384));

    // A trial session reaches the same digest without a ticket.
    let (trial, _) = start_policy(&mut t, se::TRIAL, alg::SHA384, 16);
    ok(&pol(
        &mut t,
        cc::POLICY_AUTHORIZE,
        trial,
        &authorize_args(&[], reference, &authority.name, &null_ticket),
    ));
    let (trial_full, _) = start_policy(&mut t, se::TRIAL, alg::SHA384, 16);
    ok(&pol(
        &mut t,
        cc::POLICY_AUTHORIZE,
        trial_full,
        &authorize_args(&approved, reference, &authority.name, &null_ticket),
    ));
    assert_eq!(session_digest(&mut t, trial_full), hex(AUTHORIZED_384));
}

#[test]
fn policy_authorize_nv_refuses_mismatched_hash_alg() {
    let mut t = started();
    let (s384, _) = start_policy(&mut t, se::POLICY, alg::SHA384, 16);
    let (s512, _) = start_policy(&mut t, se::POLICY, alg::SHA512, 16);
    let (s256, _) = start_policy(&mut t, se::POLICY, alg::SHA256, 16);
    let (trial, _) = start_policy(&mut t, se::TRIAL, alg::SHA384, 16);
    ok(&pol(&mut t, cc::POLICY_LOCALITY, s384, &[1]));
    let approved = session_digest(&mut t, s384);
    assert_eq!(approved, hex(APPROVED));
    let attrs = nv_attr::AUTHREAD | nv_attr::OWNERWRITE | nv_attr::READ_STCLEAR;
    let auth = b"read-secret";
    let tagged = |hash: u16, digest: &[u8], tail: &[u8]| cat(&[&u16b(hash), digest, tail]);
    let wrong_digest = [0x5a; 48];
    // (data written to the index, the error of the SHA-384 session)
    let cases: [(Vec<u8>, Rc); 4] = [
        // A TPMT_HA of SHA-256 in an index read by a SHA-384 session.
        (tagged(alg::SHA256, &[0x5a; 32], &[]), Rc::HASH.in_handle(2)),
        // A SHA-384 TPMT_HA of another digest.
        (
            tagged(alg::SHA384, &wrong_digest, &[]),
            Rc::VALUE.in_handle(2),
        ),
        // One octet short of a SHA-384 TPMT_HA.
        (
            tagged(alg::SHA384, &approved[..47], &[]),
            Rc::INSUFFICIENT.in_handle(2),
        ),
        // The approved digest, with additional octets after it (allowed).
        (tagged(alg::SHA384, &approved, &[1, 2, 3]), Rc(0)),
    ];
    for (number, (data, expected)) in cases.iter().enumerate() {
        let index = NV0 + number as u32;
        ok(&define_raw(
            &mut t,
            OWNER,
            b"",
            auth,
            &nv_public(index, alg::SHA256, attrs, &[], data.len() as u16),
        ));
        ok(&nv_write(&mut t, OWNER, index, b"", data, 0));
        let handles = |session: u32| [index, index, session];
        let reply = pw_cmd(&mut t, cc::POLICY_AUTHORIZE_NV, &handles(s384), auth, &[]);
        if expected.0 == 0 {
            ok(&reply);
            let name = nv_read_public(&mut t, index).1;
            assert_eq!(hex_of(&name), NV_NAME);
            assert_eq!(session_digest(&mut t, s384), hex(AUTHORIZED_NV_384));
        } else {
            err(&reply, *expected);
            assert_eq!(session_digest(&mut t, s384), approved);
        }
        // A SHA-512 session reads the same index with its own hash: SHA-256 and SHA-384 digests are
        // not its hash, and a 64-octet digest is not there.
        let wide = pw_cmd(&mut t, cc::POLICY_AUTHORIZE_NV, &handles(s512), auth, &[]);
        err(&wide, Rc::HASH.in_handle(2));
        assert_eq!(session_digest(&mut t, s512), vec![0u8; 64]);
        // A SHA-256 session accepts only a SHA-256 TPMT_HA.
        let narrow = pw_cmd(&mut t, cc::POLICY_AUTHORIZE_NV, &handles(s256), auth, &[]);
        if number == 0 {
            err(&narrow, Rc::VALUE.in_handle(2));
        } else {
            err(&narrow, Rc::HASH.in_handle(2));
        }
        // A trial session needs no data check and starts from a Zero Digest of its own size.
        ok(&pw_cmd(
            &mut t,
            cc::POLICY_AUTHORIZE_NV,
            &handles(trial),
            auth,
            &[],
        ));
        assert_eq!(session_digest(&mut t, trial).len(), 48);
        ok(&pol(&mut t, cc::POLICY_RESTART, trial, &[]));
    }
}

fn external_key(scheme_hash: u16) -> TpmtPublic {
    let mut public = ecc(
        oa::SIGN | oa::USER_WITH_AUTH,
        SymDef::Null,
        Scheme {
            alg: alg::ECDSA,
            hash: scheme_hash,
        },
    );
    public.unique = Unique::Ecc {
        x: hex(KEY_X),
        y: hex(KEY_Y),
    };
    public
}

fn ecdsa_signature(hash: u16, r: &str, s: &str) -> Vec<u8> {
    cat(&[
        &u16b(alg::ECDSA),
        &u16b(hash),
        &tpm2b(&hex(r)),
        &tpm2b(&hex(s)),
    ])
}

fn signed_params(cp: &[u8], reference: &[u8], signature: &[u8]) -> Vec<u8> {
    cat(&[
        &tpm2b(&[]),
        &tpm2b(cp),
        &tpm2b(reference),
        &u32b(0),
        signature,
    ])
}

#[test]
fn policy_signed_ahash_follows_sig_hash_alg() {
    for v in &SIGNED {
        let mut t = started();
        let (key, name) = parse_loaded(&load_external(&mut t, &external_key(v.hash), NULL));
        assert_eq!(hex_of(&name), v.name, "Name of the authority (Python)");
        let (s, _) = start_policy(&mut t, se::POLICY, v.session, 16);
        let handles = [key, s];
        let cp = hex(v.cp);
        let good = ecdsa_signature(v.hash, v.r, v.s);
        let policy_signed = |t: &mut Tpm, cp: &[u8], reference: &[u8], signature: &[u8]| {
            run(
                t,
                plain(
                    cc::POLICY_SIGNED,
                    &handles,
                    &signed_params(cp, reference, signature),
                ),
            )
        };
        let zero = vec![0u8; digest_size(v.session)];
        // The message was hashed with the hash of the signature: other inputs do not verify...
        err(
            &policy_signed(&mut t, &cp, b"altered", &good),
            Rc::POLICY_FAIL,
        );
        // ...and the same r and s labelled with SHA-256 are refused: the key's fixed scheme names the
        // wide hash, so the supplied scheme differs (the scheme check, not the aHash comparison).
        let narrow = ecdsa_signature(alg::SHA256, v.r, v.s);
        err(
            &policy_signed(&mut t, &cp, v.reference, &narrow),
            Rc::POLICY_FAIL,
        );
        // A wide hash of another size than the session is refused: SHA-384 for a SHA-512 session
        // and the reverse.
        let other = if v.hash == alg::SHA384 {
            alg::SHA512
        } else {
            alg::SHA384
        };
        err(
            &policy_signed(&mut t, &cp, v.reference, &ecdsa_signature(other, v.r, v.s)),
            Rc::HASH.in_param(5),
        );
        // A cpHashA of one digest of another size is a size error.
        err(
            &policy_signed(&mut t, &[0x83; 32], v.reference, &good),
            Rc::SIZE.in_param(2),
        );
        assert_eq!(session_digest(&mut t, s), zero);
        // The signature over H(nonce || expiration || cpHashA || policyRef) verifies (OpenSSL).
        ok(&policy_signed(&mut t, &cp, v.reference, &good));
        assert_eq!(session_digest(&mut t, s), hex(v.digest));
    }
    // A SHA-256 session still takes a SHA-256 signature and refuses a SHA-384 one.
    let mut t = started();
    let (key, _) = parse_loaded(&load_external(&mut t, &external_key(alg::SHA384), NULL));
    let (s, _) = start_policy(&mut t, se::POLICY, alg::SHA256, 16);
    err(
        &run(
            &mut t,
            plain(
                cc::POLICY_SIGNED,
                &[key, s],
                &signed_params(
                    &[],
                    b"ref-384",
                    &ecdsa_signature(alg::SHA384, SIGNED[0].r, SIGNED[0].s),
                ),
            ),
        ),
        Rc::HASH.in_param(5),
    );
}

/// An external authority key with a NULL signature scheme (any scheme is accepted from it), the
/// same OpenSSL key as `external_key`.
fn external_key_null_scheme() -> TpmtPublic {
    let mut public = ecc(oa::SIGN | oa::USER_WITH_AUTH, SymDef::Null, Scheme::NULL);
    public.unique = Unique::Ecc {
        x: hex(KEY_X),
        y: hex(KEY_Y),
    };
    public
}

/// Python (`gen_null_scheme.py`): `aHash = SHA-256(0 || 00000000 || 48 x 83 || ref)` of the
/// authority above, signed with `openssl dgst -sha256 -sign` (verified with `-verify`); the Name is
/// `000b || SHA-256(TPMT_PUBLIC)` with a NULL scheme and the session digest is
/// `H384(H384(0^48 || 00000160 || name) || ref)`.
const NULL_SCHEME_R: &str = "0585977a34a6746fe1dcda3aaa2cc1ae182b0b2e498bd5e80e84a77ccb038bf0";
const NULL_SCHEME_S: &str = "73c247a10f8272150196eb9d84eb89e0e80d4f79a7bcfbf3f95f65f186fbca92";
const NULL_SCHEME_NAME: &str =
    "000bc553eab4945e94d01cf11bd1a5532a4938409b318b82bbe6a61446d14234711e";
const NULL_SCHEME_DIGEST: &str = "3acb4b309b3e4036a66c8eceb0a84d085f39d8d9801a727413e29725738a4942e09a3202a025e884692cdcd584d1c254";

#[test]
fn policy_signed_sha256_signature_is_accepted_in_sha384_session() {
    let mut t = started();
    let (key, name) = parse_loaded(&load_external(&mut t, &external_key_null_scheme(), NULL));
    assert_eq!(
        hex_of(&name),
        NULL_SCHEME_NAME,
        "Name of the authority (Python)"
    );
    let (s, _) = start_policy(&mut t, se::POLICY, alg::SHA384, 16);
    let cp = [0x83u8; 48];
    let reference = b"ref-sha256-in-384";
    let signature = ecdsa_signature(alg::SHA256, NULL_SCHEME_R, NULL_SCHEME_S);
    let policy_signed = |t: &mut Tpm, reference: &[u8], signature: &[u8]| {
        run(
            t,
            plain(
                cc::POLICY_SIGNED,
                &[key, s],
                &signed_params(&cp, reference, signature),
            ),
        )
    };
    // The message is hashed with the sigHashAlg of the signature (SHA-256), not with the
    // SHA-384 of the session: a signature over another policyRef does not verify.
    err(
        &policy_signed(&mut t, b"altered", &signature),
        Rc::POLICY_FAIL,
    );
    assert_eq!(session_digest(&mut t, s), vec![0u8; 48]);
    ok(&policy_signed(&mut t, reference, &signature));
    assert_eq!(session_digest(&mut t, s), hex(NULL_SCHEME_DIGEST));
}

#[test]
fn policy_pcr_size_and_value_errors_for_sha384() {
    let mut t = started();
    ok(&extend(&mut t, 4, &hex(ABC_SHA256)));
    let (s, _) = start_policy(&mut t, se::POLICY, alg::SHA384, 16);
    let zero = vec![0u8; 48];
    // Above one digest of the session hash: TPM_RC_SIZE; a non-empty digest of another size, or
    // the wrong digest: TPM_RC_VALUE; the policy digest is unchanged.
    for (given, expected) in [
        (vec![1u8; 49], Rc::SIZE.in_param(1)),
        (vec![1u8; 64], Rc::SIZE.in_param(1)),
        (vec![1u8; 32], Rc::VALUE.in_param(1)),
        (vec![1u8; 5], Rc::VALUE.in_param(1)),
        (vec![1u8; 48], Rc::VALUE.in_param(1)),
    ] {
        err(
            &pol(
                &mut t,
                cc::POLICY_PCR,
                s,
                &pcr_params(&given, SEL_SHA384_04),
            ),
            expected,
        );
        assert_eq!(session_digest(&mut t, s), zero);
    }
    // The digest of the SHA-256 bank at one octet short of the session hash is a value error.
    err(
        &pol(
            &mut t,
            cc::POLICY_PCR,
            s,
            &pcr_params(&hex(P1)[..47], SEL_SHA256_04),
        ),
        Rc::VALUE.in_param(1),
    );
    ok(&pol(
        &mut t,
        cc::POLICY_PCR,
        s,
        &pcr_params(&hex(P2), SEL_SHA384_04),
    ));
    assert_eq!(session_digest(&mut t, s), hex(Q384));
    ok(&pol(&mut t, cc::POLICY_RESTART, s, &[]));
    ok(&pol(
        &mut t,
        cc::POLICY_PCR,
        s,
        &pcr_params(&hex(P1), SEL_SHA256_04),
    ));
    assert_eq!(session_digest(&mut t, s), hex(D1));

    // A SHA-512 session: 64 octets, the values come from the SHA-384 bank.
    let (w, _) = start_policy(&mut t, se::POLICY, alg::SHA512, 16);
    err(
        &pol(
            &mut t,
            cc::POLICY_PCR,
            w,
            &pcr_params(&[1u8; 65], SEL_SHA384_04),
        ),
        Rc::SIZE.in_param(1),
    );
    err(
        &pol(
            &mut t,
            cc::POLICY_PCR,
            w,
            &pcr_params(&hex(P2), SEL_SHA384_04),
        ),
        Rc::VALUE.in_param(1),
    );
    ok(&pol(
        &mut t,
        cc::POLICY_PCR,
        w,
        &pcr_params(&[], SEL_SHA384_04),
    ));
    assert_eq!(session_digest(&mut t, w), hex(Q512));

    // A trial session takes a given digest of exactly the session size as it is; any other size
    // is a value error.
    let (trial, _) = start_policy(&mut t, se::TRIAL, alg::SHA384, 16);
    err(
        &pol(
            &mut t,
            cc::POLICY_PCR,
            trial,
            &pcr_params(&[0x45; 32], SEL_SHA256_04),
        ),
        Rc::VALUE.in_param(1),
    );
    err(
        &pol(
            &mut t,
            cc::POLICY_PCR,
            trial,
            &pcr_params(&[0x45; 49], SEL_SHA256_04),
        ),
        Rc::SIZE.in_param(1),
    );
    ok(&pol(
        &mut t,
        cc::POLICY_PCR,
        trial,
        &pcr_params(&[0x45; 48], SEL_SHA256_04),
    ));
    assert_eq!(session_digest(&mut t, trial), hex(TRIAL384));
}

#[test]
fn policy_session_name_and_operand_limits() {
    let mut t = started();
    let secret = |t: &mut Tpm, s: u32, nonce: &[u8], cp: &[u8], reference: &[u8]| {
        run(
            t,
            with_pw(
                cc::POLICY_SECRET,
                &[OWNER, s],
                &[b""],
                &cat(&[&tpm2b(nonce), &tpm2b(cp), &tpm2b(reference), &u32b(0)]),
            ),
        )
    };
    for hash in [alg::SHA256, alg::SHA384, alg::SHA512] {
        let n = digest_size(hash);
        let big = vec![0x11u8; n + 1];
        let full = vec![0x11u8; n];
        let (trial, _) = start_policy(&mut t, se::TRIAL, hash, 16);
        let (real, _) = start_policy(&mut t, se::POLICY, hash, 16);
        // cpHash and nameHash are exactly one digest.
        for code in [cc::POLICY_CP_HASH, cc::POLICY_NAME_HASH] {
            for session in [trial, real] {
                err(
                    &pol(&mut t, code, session, &tpm2b(&big)),
                    Rc::SIZE.in_param(1),
                );
                err(
                    &pol(&mut t, code, session, &tpm2b(&full[..n - 1])),
                    Rc::SIZE.in_param(1),
                );
            }
            ok(&pol(&mut t, code, trial, &tpm2b(&full)));
            ok(&pol(&mut t, cc::POLICY_RESTART, trial, &[]));
        }
        // The digests of PolicyOR are one digest each, and PolicyCounterTimer takes an operand of
        // up to one digest.
        err(
            &pol(&mut t, cc::POLICY_OR, trial, &or_params(&[&full, &big])),
            Rc::SIZE.in_param(1),
        );
        ok(&pol(
            &mut t,
            cc::POLICY_OR,
            trial,
            &or_params(&[&full, &full]),
        ));
        let counter = |operand: &[u8]| cat(&[&tpm2b(operand), &u16b(0), &u16b(5)]);
        err(
            &pol(&mut t, cc::POLICY_COUNTER_TIMER, trial, &counter(&big)),
            Rc::SIZE.in_param(1),
        );
        ok(&pol(
            &mut t,
            cc::POLICY_COUNTER_TIMER,
            trial,
            &counter(&full),
        ));
        // The nonce, the cpHash and the policyRef of the authorization assertions.
        err(&secret(&mut t, trial, &big, &[], &[]), Rc::SIZE.in_param(1));
        err(&secret(&mut t, trial, &[], &big, &[]), Rc::SIZE.in_param(2));
        err(&secret(&mut t, trial, &[], &[], &big), Rc::SIZE.in_param(3));
        ok(&secret(&mut t, trial, &full, &full, &full));
        // PolicyAuthorize takes at least a SHA-384 digest and one digest of a longer hash; a
        // Name of up to 66 octets (nameAlg and a SHA-512 digest).
        let null_ticket = cat(&[&u16b(st::VERIFIED), &u32b(NULL), &tpm2b(b"")]);
        let name = |len: usize| {
            let mut name = u16b(alg::SHA512);
            name.resize(len, 0x66);
            name
        };
        let field = n.max(48);
        let wide_field = vec![0x22u8; field + 1];
        let fits = vec![0x22u8; field];
        err(
            &pol(
                &mut t,
                cc::POLICY_AUTHORIZE,
                trial,
                &authorize_args(&wide_field, &[], &name(66), &null_ticket),
            ),
            Rc::SIZE.in_param(1),
        );
        err(
            &pol(
                &mut t,
                cc::POLICY_AUTHORIZE,
                trial,
                &authorize_args(&[], &wide_field, &name(66), &null_ticket),
            ),
            Rc::SIZE.in_param(2),
        );
        err(
            &pol(
                &mut t,
                cc::POLICY_AUTHORIZE,
                trial,
                &authorize_args(&[], &[], &name(67), &null_ticket),
            ),
            Rc::SIZE.in_param(3),
        );
        ok(&pol(
            &mut t,
            cc::POLICY_AUTHORIZE,
            trial,
            &authorize_args(&fits, &fits, &name(66), &null_ticket),
        ));
        // PolicyDuplicationSelect: Names of up to 66 octets.
        let duplicate = |object: &[u8], parent: &[u8]| cat(&[&tpm2b(object), &tpm2b(parent), &[1]]);
        err(
            &pol(
                &mut t,
                cc::POLICY_DUPLICATION_SELECT,
                trial,
                &duplicate(&name(67), &name(66)),
            ),
            Rc::SIZE.in_param(1),
        );
        err(
            &pol(
                &mut t,
                cc::POLICY_DUPLICATION_SELECT,
                trial,
                &duplicate(&name(66), &name(67)),
            ),
            Rc::SIZE.in_param(2),
        );
        ok(&pol(
            &mut t,
            cc::POLICY_DUPLICATION_SELECT,
            trial,
            &duplicate(&name(66), &name(66)),
        ));
        // PolicyTicket: cpHashA and policyRef of one digest, a Name of up to 66 octets.
        let ticket = |cp: &[u8], reference: &[u8], name: &[u8]| {
            cat(&[
                &tpm2b(&[0; 8]),
                &tpm2b(cp),
                &tpm2b(reference),
                &tpm2b(name),
                &u16b(st::AUTH_SECRET),
                &u32b(OWNER),
                &tpm2b(&[0; 32]),
            ])
        };
        err(
            &pol(&mut t, cc::POLICY_TICKET, trial, &ticket(&big, &[], &[])),
            Rc::SIZE.in_param(2),
        );
        err(
            &pol(&mut t, cc::POLICY_TICKET, trial, &ticket(&[], &big, &[])),
            Rc::SIZE.in_param(3),
        );
        err(
            &pol(
                &mut t,
                cc::POLICY_TICKET,
                trial,
                &ticket(&[], &[], &name(67)),
            ),
            Rc::SIZE.in_param(4),
        );
        ok(&pol(
            &mut t,
            cc::POLICY_TICKET,
            trial,
            &ticket(&full, &full, &name(66)),
        ));
    }
}

#[test]
fn policy_secret_ticket_stays_hmac_sha256_for_any_session_hash() {
    let mut t = started();
    let secret = |t: &mut Tpm, s: u32| {
        let resp = run(
            t,
            with_pw(
                cc::POLICY_SECRET,
                &[OWNER, s],
                &[b""],
                &cat(&[
                    &tpm2b(&[]),
                    &tpm2b(&[]),
                    &tpm2b(b"ref-384"),
                    &u32b((-60i32) as u32),
                ]),
            ),
        );
        ok(&resp);
        params(&resp, 0).to_vec()
    };
    let (narrow, _) = start_policy(&mut t, se::POLICY, alg::SHA256, 16);
    let baseline = secret(&mut t, narrow);
    // timeout (8 octets) and a ticket of tag, hierarchy and a 32-octet HMAC-SHA-256.
    assert_eq!(&baseline[10..18], &hex("8023400000010020"));
    assert_eq!(baseline.len(), 10 + 8 + 32);
    for (hash, expected) in [(alg::SHA384, TICKET_384), (alg::SHA512, TICKET_512)] {
        let (issuer, _) = start_policy(&mut t, se::POLICY, hash, 16);
        let output = secret(&mut t, issuer);
        // The ticket does not depend on the hash of the session: the same bytes as for SHA-256.
        assert_eq!(output, baseline);
        assert_eq!(session_digest(&mut t, issuer), hex(expected));
        let (replay, _) = start_policy(&mut t, se::POLICY, hash, 16);
        let request = cat(&[
            &output[..10],
            &tpm2b(&[]),
            &tpm2b(b"ref-384"),
            &tpm2b(&u32b(OWNER)),
            &output[10..],
        ]);
        ok(&pol(&mut t, cc::POLICY_TICKET, replay, &request));
        assert_eq!(session_digest(&mut t, replay), hex(expected));
        let mut forged = request.clone();
        let last = forged.len() - 1;
        forged[last] ^= 1;
        let (other, _) = start_policy(&mut t, se::POLICY, hash, 16);
        err(
            &pol(&mut t, cc::POLICY_TICKET, other, &forged),
            Rc::TICKET.in_param(5),
        );
    }
}
