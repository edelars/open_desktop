//! NV indices, hierarchy policies, credentials and duplication with hash agility (slice 1c). The
//! expected Names, digests, HMACs, wrappers and extend values come from Python `hashlib` and
//! `hmac` and from the `openssl` command line (the scripts are in the task scratchpad), not from
//! the code under test. The TPM entropy is fixed, so the `nonceTPM` values are inputs pinned here;
//! the SHA-256 flow is pinned to the bytes the code produced before this slice (commit 78c3ba5).

use super::tests_b::{
    NV0, define_raw, hc, nv_public, nv_read_public, nv_write, run_hmac, split_response, start_cmd,
    start_hmac, start_params,
};
use super::tests_c2::parse_attestation;
use super::tests_obj::{
    ECDSA, base, create_child, create_primary, ecc, ecc_storage, load, parse_primary, rsa_storage,
    unmarshal_public,
};
use super::*;
use crate::codes::{object_attr as oa, se, session_attr};
use crate::dispatch::Invocation;
use crate::marshal::{SymDef, Unique, Writer};
use crate::nv::attr as nv_attr;
use sha2::{Digest, Sha256};

const NULL: u32 = handle::RH_NULL;
const OWNER: u32 = handle::RH_OWNER;
const CONTINUE: u8 = session_attr::CONTINUE_SESSION;

fn hex_of(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `len` octets `first, first + 1, ...`: the authValues and the data of the pinned vectors.
fn seq(len: usize, first: u8) -> Vec<u8> {
    (0..len).map(|i| first.wrapping_add(i as u8)).collect()
}

/// The scripted SHA-256 flow whose raw responses (and the digest of the snapshot) are pinned in
/// `GOLDEN`.
fn sha256_nv_flow() -> Vec<(&'static str, String)> {
    let mut t = started();
    let mut log: Vec<(&'static str, String)> = Vec::new();
    let mut note = |label: &'static str, resp: &[u8]| {
        assert_eq!(rc(resp), 0, "{label}");
        log.push((label, hex_of(resp)));
    };
    let rw = nv_attr::OWNERWRITE | nv_attr::OWNERREAD | nv_attr::AUTHWRITE | nv_attr::AUTHREAD;
    let (ord, ctr, ext, bits, pol) = (NV0 + 1, NV0 + 2, NV0 + 3, NV0 + 4, NV0 + 5);
    let define = |t: &mut Tpm, index: u32, attrs: u32, policy: &[u8], size: u16| {
        define_raw(
            t,
            OWNER,
            b"",
            b"index-auth",
            &nv_public(index, alg::SHA256, attrs, policy, size),
        )
    };
    note("define_ordinary", &define(&mut t, ord, rw, &[], 40));
    note("define_counter", &define(&mut t, ctr, rw | 0x10, &[], 8));
    note("define_extend", &define(&mut t, ext, rw | 0x40, &[], 32));
    note("define_bits", &define(&mut t, bits, rw | 0x20, &[], 8));
    note(
        "define_policy",
        &define(
            &mut t,
            pol,
            nv_attr::POLICYWRITE | nv_attr::OWNERREAD,
            &[0x42; 32],
            4,
        ),
    );
    note(
        "write",
        &nv_write(&mut t, ord, ord, b"index-auth", &[0x5c; 40], 0),
    );
    note("read", &pw_cmd_read(&mut t, ord, b"index-auth", 16, 4));
    note(
        "increment",
        &pw(&mut t, cc::NV_INCREMENT, &[OWNER, ctr], b"", &[]),
    );
    note(
        "increment_again",
        &pw(&mut t, cc::NV_INCREMENT, &[ctr, ctr], b"index-auth", &[]),
    );
    note(
        "extend",
        &pw(
            &mut t,
            cc::NV_EXTEND,
            &[ext, ext],
            b"index-auth",
            &tpm2b(b"abc"),
        ),
    );
    note(
        "set_bits",
        &pw(
            &mut t,
            cc::NV_SET_BITS,
            &[OWNER, bits],
            b"",
            &u64b(0x0102_0304_0506_0708),
        ),
    );
    for (label, index) in [
        ("public_ordinary", ord),
        ("public_counter", ctr),
        ("public_extend", ext),
        ("public_bits", bits),
        ("public_policy", pol),
    ] {
        note(
            label,
            &run(&mut t, plain(cc::NV_READ_PUBLIC, &[index], &[])),
        );
    }
    note(
        "read_extend",
        &pw_cmd_read(&mut t, ext, b"index-auth", 32, 0),
    );

    // NV_Certify with an ECDSA key: a range and the digest of the whole index.
    let key = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"pw",
        &ecc(base() | oa::SIGN, SymDef::Null, ECDSA),
    ));
    let certify = |t: &mut Tpm, size: u16, offset: u16| {
        run(
            t,
            with_pw(
                cc::NV_CERTIFY,
                &[key.handle, OWNER, ord],
                &[b"pw", b""],
                &cat(&[
                    &tpm2b(b"fresh"),
                    &u16b(alg::NULL),
                    &u16b(size),
                    &u16b(offset),
                ]),
            ),
        )
    };
    note("certify_range", &certify(&mut t, 8, 4));
    note("certify_whole", &certify(&mut t, 0, 0));

    // A trial policy session over the NV assertions.
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
    note(
        "policy_nv",
        &pw(
            &mut t,
            cc::POLICY_NV,
            &[ord, ord, tr],
            b"index-auth",
            &cat(&[&tpm2b(&[0x5c; 4]), &u16b(0), &u16b(0)]),
        ),
    );
    note(
        "policy_nv_digest",
        &run(&mut t, plain(cc::POLICY_GET_DIGEST, &[tr], &[])),
    );
    note(
        "policy_nv_written",
        &run(&mut t, plain(cc::POLICY_NV_WRITTEN, &[tr], &[1])),
    );
    note(
        "policy_nv_written_digest",
        &run(&mut t, plain(cc::POLICY_GET_DIGEST, &[tr], &[])),
    );
    note(
        "policy_authorize_nv",
        &pw(
            &mut t,
            cc::POLICY_AUTHORIZE_NV,
            &[ord, ord, tr],
            b"index-auth",
            &[],
        ),
    );
    note(
        "policy_authorize_nv_digest",
        &run(&mut t, plain(cc::POLICY_GET_DIGEST, &[tr], &[])),
    );

    // An HMAC session authorizes NV_Write with the authValue of the index.
    let mut s = start_hmac(&mut t, NULL, b"", &[0x66; 16]);
    let name = nv_read_public(&mut t, ord).1;
    let mut c = hc(
        cc::NV_WRITE,
        &[ord, ord],
        cat(&[&tpm2b(b"hmac-written"), &u16b(0)]),
        (ord, b"index-auth"),
    );
    c.names = vec![name.clone(), name];
    note("hmac_write", &run_hmac(&mut t, &mut s, &c));

    // A primary policy and the snapshot of the whole state.
    note(
        "set_primary_policy",
        &pw(
            &mut t,
            cc::SET_PRIMARY_POLICY,
            &[OWNER],
            b"",
            &cat(&[&tpm2b(&[0x24; 32]), &u16b(alg::SHA256)]),
        ),
    );
    let snapshot = t.persistent_snapshot();
    log.push(("snapshot_len", snapshot.len().to_string()));
    log.push(("snapshot_sha256", hex_of(&Sha256::digest(&snapshot))));
    log
}

fn pw(t: &mut Tpm, code: u32, handles: &[u32], password: &[u8], params: &[u8]) -> Vec<u8> {
    run(t, with_pw(code, handles, &[password], params))
}

fn pw_cmd_read(t: &mut Tpm, index: u32, password: &[u8], size: u16, offset: u16) -> Vec<u8> {
    pw(
        t,
        cc::NV_READ,
        &[index, index],
        password,
        &cat(&[&u16b(size), &u16b(offset)]),
    )
}

fn u64b(v: u64) -> Vec<u8> {
    v.to_be_bytes().to_vec()
}

const GOLDEN: [(&str, &str); 30] = [
    ("define_ordinary", "80020000001300000000000000000000010000"),
    ("define_counter", "80020000001300000000000000000000010000"),
    ("define_extend", "80020000001300000000000000000000010000"),
    ("define_bits", "80020000001300000000000000000000010000"),
    ("define_policy", "80020000001300000000000000000000010000"),
    ("write", "80020000001300000000000000000000010000"),
    (
        "read",
        "800200000025000000000000001200105c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c0000010000",
    ),
    ("increment", "80020000001300000000000000000000010000"),
    ("increment_again", "80020000001300000000000000000000010000"),
    ("extend", "80020000001300000000000000000000010000"),
    ("set_bits", "80020000001300000000000000000000010000"),
    (
        "public_ordinary",
        "80010000003e00000000000e01000001000b20060006000000280022000b2801a156c56f4dca4261077a5e255febe95c8869b94ee481f544a27a25e066d6",
    ),
    (
        "public_counter",
        "80010000003e00000000000e01000002000b20060016000000080022000bcf8d2f3134483db055119de28498936a8e3fcff15cdf3a53aec957fb0c8d7cc4",
    ),
    (
        "public_extend",
        "80010000003e00000000000e01000003000b20060046000000200022000bfed9c953406296fe436cd80dd345b2c0e6dbcd086b5e9f91e3d7bd9241c23be4",
    ),
    (
        "public_bits",
        "80010000003e00000000000e01000004000b20060026000000080022000b5b464045a02782178fa565cf1c4df6487991c7cb44b146863a0d66346e2da166",
    ),
    (
        "public_policy",
        "80010000005e00000000002e01000005000b000200080020424242424242424242424242424242424242424242424242424242424242424200040022000b4c997d26fd373287d697462c31e3b0fb6b5767488ec8ab28f9fc28a62ca1da33",
    ),
    (
        "read_extend",
        "80020000003500000000000000220020365aa7d8f7f9402c4b9434502b4cc89ddb09fe50d7cd95b493b834c62d5a53700000010000",
    ),
    (
        "certify_range",
        "8002000000dc00000000000000c4007aff54434780140022000b515275a22f50b515660777f295b213eea103fc8fe261aaa111d00c8d23ae007a000566726573680000000000000000568926c98ab5481e01fff05150643cefce0022000b2801a156c56f4dca4261077a5e255febe95c8869b94ee481f544a27a25e066d6000400085c5c5c5c5c5c5c5c0018000b0020411b994955bbb2781e582da698b93a1fd9c171abf65a4b76af52375f4bd7343800204288b0f5b02d64124ff004f60998dbd1bdb5172c4c2df170cdae7d61b1fb7b6a00000100000000010000",
    ),
    (
        "certify_whole",
        "8002000000f200000000000000da0090ff544347801c0022000b515275a22f50b515660777f295b213eea103fc8fe261aaa111d00c8d23ae007a000566726573680000000000000000568926c98ab5481e01fff05150643cefce0022000b2801a156c56f4dca4261077a5e255febe95c8869b94ee481f544a27a25e066d600205f8ed2fbfe9253050207fadb4f7a06434ea6b58000620bfc12f6ab169858f2230018000b0020e4b235b90fd113c0ca837f0956f03b81918b03576b2c37a7973d36a8a942a24800209552541c7790ee9b86da47d371561a613b761f34c797c6f97ca864271b0550f800000100000000010000",
    ),
    (
        "trial_start",
        "800100000030000000000300000000203f64e53032fbb062370276a9b143893ba0b75566d5d0942f03f9402b234c48bf",
    ),
    ("policy_nv", "80020000001300000000000000000000010000"),
    (
        "policy_nv_digest",
        "80010000002c00000000002026c96734b1a1bf785935787150ab85c0a04ae933bcc964922fad7791533b65f4",
    ),
    ("policy_nv_written", "80010000000a00000000"),
    (
        "policy_nv_written_digest",
        "80010000002c000000000020eaa29ebb67676f9457c780a6a5c2f685ba8eb59a6fac74b8a206b5bfcfc01bb3",
    ),
    (
        "policy_authorize_nv",
        "80020000001300000000000000000000010000",
    ),
    (
        "policy_authorize_nv_digest",
        "80010000002c000000000020e0516d3e6f3e71f5ade5136eac43d3c306e000849714816763bcaceb848470a1",
    ),
    (
        "hmac_write",
        "800200000043000000000000000000107b3c1bf8fd90bc408f7cd7822ce5562d010020ef114a44f08f2f2a46223456439bc07724ea22185c9059a551be6a1e6162e0d5",
    ),
    (
        "set_primary_policy",
        "80020000001300000000000000000000010000",
    ),
    ("snapshot_len", "581"),
    (
        "snapshot_sha256",
        "a995d706e05d0811ece3b7992b22ef485c7f5321c519e6e8d0f2a57360e5df9a",
    ),
];

#[test]
fn sha256_nv_golden_unchanged() {
    let flow = sha256_nv_flow();
    assert_eq!(flow.len(), GOLDEN.len());
    for ((label, resp), (golden_label, golden)) in flow.iter().zip(GOLDEN) {
        assert_eq!(*label, golden_label);
        assert_eq!(resp, golden, "{label}");
    }
}

// ------------------------------------------------------------------------------------ helpers

const RW: u32 = nv_attr::OWNERWRITE
    | nv_attr::OWNERREAD
    | nv_attr::AUTHWRITE
    | nv_attr::AUTHREAD
    | nv_attr::NO_DA;
const COUNTER: u32 = 0x10;
const EXTEND: u32 = 0x40;

fn define(
    t: &mut Tpm,
    index: u32,
    name_alg: u16,
    attributes: u32,
    policy: &[u8],
    size: u16,
    auth: &[u8],
) -> Vec<u8> {
    define_raw(
        t,
        OWNER,
        b"",
        auth,
        &nv_public(index, name_alg, attributes, policy, size),
    )
}

/// `(public area, Name)` of `TPM2_NV_ReadPublic`; a SHA-512 Name has 66 octets, two more than
/// the shared helper reads.
fn read_public(t: &mut Tpm, index: u32) -> (Vec<u8>, Vec<u8>) {
    let r = run(t, plain(cc::NV_READ_PUBLIC, &[index], &[]));
    ok(&r);
    let mut rd = Reader::new(params(&r, 0));
    let public = rd.tpm2b(1024).unwrap().to_vec();
    let name = rd.tpm2b(66).unwrap().to_vec();
    rd.finish().unwrap();
    (public, name)
}

fn read_index(t: &mut Tpm, index: u32, password: &[u8], size: u16, offset: u16) -> Vec<u8> {
    pw(
        t,
        cc::NV_READ,
        &[index, index],
        password,
        &cat(&[&u16b(size), &u16b(offset)]),
    )
}

fn read_data(resp: &[u8]) -> Vec<u8> {
    ok(resp);
    let mut r = Reader::new(params(resp, 0));
    let data = r.tpm2b(512).unwrap().to_vec();
    r.finish().unwrap();
    data
}

// ------------------------------------------------------------------------------------ vectors

// Python `hashlib`: Name = nameAlg || SHA-384(TPMS_NV_PUBLIC) of the four indices below, before and
// after the first write (`TPMA_NV_WRITTEN` is part of the public area).
const NAME_A: &str = "000c03271969ef7283f10df8c0dfeee7e5e76a702acde6bce2897129e786bf7f5b3dee01388bdb0905e5cff0087278246b59";
const NAME_A_WRITTEN: &str = "000c59830e709b6a4fcd3edb9b9878ab30f6da6879c56732d562df9ed1457a45b423b456f3dced01cac45e7a78c6360af184";
const NAME_B: &str = "000c38511c6fffacedc2f67f402deb93b5ce5ba1651b7b12c7105add724bef4c0ba4ef804bbc887a83dad1b1d42bd8ed93ca";
const NAME_B_WRITTEN: &str = "000cfcd6d4a71197b112401ff0a04e5bbfab7c30fade8080153697b27a6ed22444c6ba6b3041115df2fa1ea14a0ddd78322b";
const NAME_C: &str = "000c984948b4b2e139a6b158141408c0a8da2ff8e1e8bfdb38c6c1cda57604c01ec8b0951b13b8d006bc8f0261b9867441ad";
const NAME_C_WRITTEN: &str = "000c3a60a393649ded9baeba7e21f8b41eea02478a881d6730b5f319122881cc56f967f0572519168d7d9f2aa220188c8876";
const NAME_D: &str = "000c35697c1f78b4defc0031fb502ff8108427cb3650551383e3986f6e0cb920e188408ceb8ebc862efb8857e22889f80493";
const PUBLIC_A: &str = "01000001000c0206000600000028";
/// SHA-384(48 zero octets || "abc") and SHA-384(that || "def"): `TPM2_NV_Extend` with nameAlg SHA-384.
const EXTEND_1: &str = "b1c16eb7634112b7c9d5ebd27e62a2d4528bbfcfd68b62d3afd9ecf98e0f413a84314acce78317fb69fd895155343e09";
const EXTEND_2: &str = "ba598f53461d55592887b15124753fb581a0e3b4961eaefea1714a8f289ad5eac2e9193704097bdc71a4dbe8e671a5f5";

const SESSION_NONCE_TPM: &str = "3f64e53032fbb062370276a9b143893ba0b75566d5d0942f03f9402b234c48bffc7d74cb580374ade99168f98c8582f8";
const HMAC_COMMAND: &str = "e402a9a7730dfd02cfbe5f986d935f5f3591fb45a1c9dc671bb8b3a8d39381d56d5980d460b898da110bc760f3f551d4";
const ACK_NONCE_TPM: &str = "fff9690bde8aad0397efc5f7e644b5aa762c04536c611dcbf136fcb45495031e577f54f47412504d076d947356899290";
const ACK_HMAC: &str = "7eef3bc401fc41eaf1713faa3f3aadb3368d9365070369eb93298e804a427e8d4c9b99b1480090ab3cf9ae280db2d627";

#[test]
fn nv_index_sha384_name_define_write_read_increment() {
    let mut t = started();
    let (a, b, c, d) = (NV0 + 1, NV0 + 2, NV0 + 3, NV0 + 4);
    let (auth_a, auth_b, auth_c) = (seq(48, 0x30), seq(48, 0x50), seq(48, 0x70));
    ok(&define(&mut t, a, alg::SHA384, RW, &[], 40, &auth_a));
    ok(&define(
        &mut t,
        b,
        alg::SHA384,
        RW | COUNTER,
        &[],
        8,
        &auth_b,
    ));
    ok(&define(
        &mut t,
        c,
        alg::SHA384,
        RW | EXTEND,
        &[],
        48,
        &auth_c,
    ));
    let policy = [0x42u8; 48];
    let policy_attrs = nv_attr::POLICYWRITE | nv_attr::OWNERREAD | nv_attr::NO_DA;
    ok(&define(
        &mut t,
        d,
        alg::SHA384,
        policy_attrs,
        &policy,
        4,
        b"",
    ));
    // The Name is nameAlg || SHA-384(public area): 50 octets.
    let (public, name) = read_public(&mut t, a);
    assert_eq!(hex_of(&public), PUBLIC_A);
    assert_eq!(hex_of(&name), NAME_A);
    assert_eq!(name.len(), 50);
    for (index, expected) in [(b, NAME_B), (c, NAME_C), (d, NAME_D)] {
        assert_eq!(hex_of(&read_public(&mut t, index).1), expected);
    }

    // Authorization limits: one digest of nameAlg for authValue and authPolicy (Part 3, 31.3.1).
    let public = |index, policy: &[u8]| nv_public(index, alg::SHA384, RW, policy, 4);
    err(
        &define_raw(&mut t, OWNER, b"", &[1; 49], &public(NV0 + 9, &[])),
        Rc::SIZE.in_param(1),
    );
    err(
        &define_raw(&mut t, OWNER, b"", b"", &public(NV0 + 9, &[9; 32])),
        Rc::SIZE.in_param(2),
    );
    err(
        &define_raw(&mut t, OWNER, b"", b"", &public(NV0 + 9, &[9; 49])),
        Rc::SIZE.in_param(2),
    );
    // An extend index is one digest of its nameAlg: 48 octets for SHA-384 (32 for SHA-256).
    err(
        &define(&mut t, NV0 + 9, alg::SHA384, RW | EXTEND, &[], 32, b""),
        Rc::SIZE.in_param(2),
    );
    err(
        &define(&mut t, NV0 + 9, alg::SHA256, RW | EXTEND, &[], 48, b""),
        Rc::SIZE.in_param(2),
    );
    ok(&define(
        &mut t,
        NV0 + 9,
        alg::SHA256,
        RW | EXTEND,
        &[],
        32,
        b"",
    ));
    // A nameAlg that is no hash of this TPM is TPM_RC_HASH.
    err(
        &define(&mut t, NV0 + 10, 0x0004, RW, &[], 4, b""),
        Rc::HASH.in_param(2),
    );

    // NV_Write and NV_Read authorized by the 48-octet authValue as a password.
    let data = seq(40, 0x80);
    ok(&nv_write(&mut t, a, a, &auth_a, &data, 0));
    assert_eq!(hex_of(&read_public(&mut t, a).1), NAME_A_WRITTEN);
    let read = read_index(&mut t, a, &auth_a, 40, 0);
    assert_eq!(read_data(&read), data);
    // The password of an index is at most one digest of its nameAlg; a password of 33..48 octets is
    // checked (it was an authorization size error while only SHA-256 existed), 49 is not.
    let mut wrong = auth_a.clone();
    wrong[47] ^= 1;
    err(
        &read_index(&mut t, a, &wrong, 40, 0),
        Rc::BAD_AUTH.in_session(1),
    );
    err(
        &read_index(&mut t, a, &auth_a[..47], 40, 0),
        Rc::BAD_AUTH.in_session(1),
    );
    err(
        &read_index(&mut t, a, &[auth_a.as_slice(), &[7]].concat(), 40, 0),
        Rc::AUTHSIZE,
    );
    // The owner authorizes too (the index allows it); an empty password does not authorize the index.
    assert_eq!(
        read_data(&pw(
            &mut t,
            cc::NV_READ,
            &[OWNER, a],
            b"",
            &cat(&[&u16b(4), &u16b(36)]),
        )),
        data[36..].to_vec()
    );
    err(
        &read_index(&mut t, a, b"", 40, 0),
        Rc::BAD_AUTH.in_session(1),
    );

    // NV_Increment: the counter is 8 octets whatever the nameAlg.
    ok(&pw(&mut t, cc::NV_INCREMENT, &[b, b], &auth_b, &[]));
    ok(&pw(&mut t, cc::NV_INCREMENT, &[OWNER, b], b"", &[]));
    assert_eq!(
        read_data(&read_index(&mut t, b, &auth_b, 8, 0)),
        2u64.to_be_bytes()
    );
    assert_eq!(hex_of(&read_public(&mut t, b).1), NAME_B_WRITTEN);

    // NV_Extend: data_new = SHA-384(data_old || data), the old value 48 zero octets at first.
    let extend = |t: &mut Tpm, data: &[u8]| pw(t, cc::NV_EXTEND, &[c, c], &auth_c, &tpm2b(data));
    ok(&extend(&mut t, b"abc"));
    assert_eq!(
        hex_of(&read_data(&read_index(&mut t, c, &auth_c, 48, 0))),
        EXTEND_1
    );
    assert_eq!(hex_of(&read_public(&mut t, c).1), NAME_C_WRITTEN);
    ok(&extend(&mut t, b"def"));
    assert_eq!(
        hex_of(&read_data(&read_index(&mut t, c, &auth_c, 48, 0))),
        EXTEND_2
    );

    // An HMAC session of authHash SHA-384 authorizes NV_Write: the HMAC key is the 48-octet authValue
    // of the index (the session is unbound), the cpHash holds the 50-octet Name of the index, and the
    // nonces and HMACs are 48 octets long. Python: HMAC-SHA-384 over SHA-384(cpHash inputs) || nonces.
    let nonce_caller = seq(48, 0x11);
    let start = run(
        &mut t,
        start_cmd(
            NULL,
            NULL,
            &start_params(&nonce_caller, &[], se::HMAC, &u16b(alg::NULL), alg::SHA384),
        ),
    );
    ok(&start);
    let session = u32::from_be_bytes(start[10..14].try_into().unwrap());
    let mut nonce = Reader::new(&start[14..]);
    assert_eq!(hex_of(nonce.tpm2b(64).unwrap()), SESSION_NONCE_TPM);
    let hmac_command = |t: &mut Tpm, tag: &str, nonce_caller: &[u8]| {
        let entry = cat(&[
            &u32b(session),
            &tpm2b(nonce_caller),
            &[CONTINUE],
            &tpm2b(&hex(tag)),
        ]);
        let area = cat(&[&u32b(entry.len() as u32), &entry]);
        let write = cat(&[&tpm2b(b"hmac-data"), &u16b(0)]);
        run(t, frame(st::SESSIONS, cc::NV_WRITE, &[a, a], &area, &write))
    };
    // A tag cut to the length of a SHA-256 HMAC does not authorize.
    err(
        &hmac_command(&mut t, &HMAC_COMMAND[..64], &seq(48, 0x60)),
        Rc::BAD_AUTH.in_session(1),
    );
    let resp = hmac_command(&mut t, HMAC_COMMAND, &seq(48, 0x60));
    let (rparams, acks) = split_response(&resp, 0);
    assert!(rparams.is_empty());
    assert_eq!(acks.len(), 1);
    assert_eq!(
        hex_of(&acks[0].0),
        ACK_NONCE_TPM,
        "nonceTPM of the acknowledgement"
    );
    assert_eq!(acks[0].1, CONTINUE);
    assert_eq!(hex_of(&acks[0].2), ACK_HMAC, "response HMAC");
    let mut written = data.clone();
    written[..9].copy_from_slice(b"hmac-data");
    assert_eq!(read_data(&read_index(&mut t, a, &auth_a, 40, 0)), written);
}

// ------------------------------------------------------------------------------ policy sessions

/// Starts a policy or trial session of `auth_hash` with a nonce of 16 octets; its handle.
fn start_policy(t: &mut Tpm, kind: u8, auth_hash: u16) -> u32 {
    let resp = run(
        t,
        start_cmd(
            NULL,
            NULL,
            &start_params(&[0x44; 16], &[], kind, &u16b(alg::NULL), auth_hash),
        ),
    );
    ok(&resp);
    u32::from_be_bytes(resp[10..14].try_into().unwrap())
}

fn session_digest(t: &mut Tpm, session: u32) -> Vec<u8> {
    let r = run(t, plain(cc::POLICY_GET_DIGEST, &[session], &[]));
    ok(&r);
    let mut rd = Reader::new(params(&r, 0));
    let d = rd.tpm2b(64).unwrap().to_vec();
    rd.finish().unwrap();
    d
}

/// Python `hashlib`: the policyDigest after `TPM2_PolicyNV` (operandB = octets 4..8 of the index,
/// offset 4, TPM_EO_EQ) and after `TPM2_PolicyAuthorizeNV`, from the Zero Digest of the session:
/// `H_session(digest || TPM_CC_PolicyNV || H_session(operandB || offset || operation) || Name)`
/// and `H_session(digest || TPM_CC_PolicyAuthorizeNV || Name)` with the Name of the index (nameAlg
/// of the index, 34, 50 or 66 octets). The hash of the digest is the hash of the session, not the
/// `nameAlg` of the index; the index only chooses the Name.
/// (hash of the session, nameAlg of the index, Name of the written index, PolicyNV, PolicyAuthorizeNV).
const POLICY_NV_CASES: [(u16, u16, &str, &str, &str); 5] = [
    (
        alg::SHA384,
        alg::SHA384,
        "000c0d23121c25dda0c31040dfcec54eaafe136a7846a3c05f8814ad53c6dd8bfeab9eb4a0b5e24ec2ab6434f527850806cf",
        "6bdf17a16981787bcc18d9a84d5706a28e4a104b61e046c74d790569f14d28f400b80c8d304e6d3e15dac966a7766aec",
        "f6350cd4e0a36bf998f239c37e5a78d9eceb63756463add9911be4b4e6744ef9dfc1d643f3386c8fd80b221bedcf031c",
    ),
    (
        alg::SHA256,
        alg::SHA384,
        "000c6d5e29fb467cb59015614d304419278bb3e27e9cef4091fde9ac3b1eba44a451feb9d1ddfd46558e4d75a4633a48734f",
        "b20109e1069943f345df240c4ad9e1b8977e924cf85d787efb0b8d6e0f5cc72d",
        "99fa740086fec1bd82c14efd209d5975e8f469971b21eaabe1c258ef840fb5db",
    ),
    (
        alg::SHA512,
        alg::SHA384,
        "000cf80c81a9c58d526688c54e4ea976072042bf1be5eeb9c232a03900bd518b59b784686d5bf5e00507b40ba4f31f70c385",
        "c77ed63cb6a1ef2a22c622b03d843bfb9a083d146cd41e0a97bff472c3d828a50d56adcf78d8ad6e506b88c1b4b0b2ee736253823db1d1f126738815f560fd1f",
        "d36bb83b045c165fc9b1f4e20043c35ffa7451a92805a2766d5f1f249ad2f8ccae5e0c2658f2be4ee74067e5b0ec43ba41a30530a78c976135dc8933fbf8cb7b",
    ),
    (
        alg::SHA384,
        alg::SHA512,
        "000d5f72c54555860bb77b0b7d5d3cef5330ea899450c98c8a2f7adf4fc8ca2eab650d93cf4deae1e98bd2393bab15a2205dcbe9a4e8da5d779e77cf837990cca437",
        "6f13159ff11f5bcbcd7812cbdd33821c38dada81d831d277f9d8411d3919640b6a9d934282c482d096a40daeab277e26",
        "7e93c6ec1397b9b31f4a2ddb62b7d11d268cb43e09c4b0c4c1b797945440213e62c80dae24f9128a12fb77c3b15c1ed3",
    ),
    (
        alg::SHA256,
        alg::SHA256,
        "000b4cb1d10b6b1cb049d68fa816e0c4f6dd5ac16c802716922bdc0e288563ef69ff",
        "9526310441ad4850741273c779903e45acb1c59c96fa7400a99824535f9fecc5",
        "c22281ef40c526edf8cb35ab8e7a7074eaf1ab5ee132e14035c615f8533d0ce3",
    ),
];

#[test]
fn policy_nv_with_sha384_index() {
    let mut t = started();
    let data = seq(20, 0xa0);
    for (number, (session_hash, name_alg, name, policy_nv, authorize_nv)) in
        POLICY_NV_CASES.iter().enumerate()
    {
        let index = NV0 + 0x20 + number as u32;
        let auth = b"idx-auth";
        ok(&define(&mut t, index, *name_alg, RW, &[], 20, auth));
        ok(&nv_write(&mut t, index, index, auth, &data, 0));
        assert_eq!(hex_of(&read_public(&mut t, index).1), *name);
        let handles = |session: u32| [index, index, session];
        let args = |operand: &[u8], offset: u16| cat(&[&tpm2b(operand), &u16b(offset), &u16b(0)]);

        // A real session compares the data at the offset: a wrong operand is TPM_RC_POLICY.
        let s = start_policy(&mut t, se::POLICY, *session_hash);
        err(
            &pw(
                &mut t,
                cc::POLICY_NV,
                &handles(s),
                auth,
                &args(&data[5..9], 4),
            ),
            Rc::POLICY,
        );
        assert_eq!(
            session_digest(&mut t, s),
            vec![0u8; hash_size(*session_hash)]
        );
        ok(&pw(
            &mut t,
            cc::POLICY_NV,
            &handles(s),
            auth,
            &args(&data[4..8], 4),
        ));
        assert_eq!(
            hex_of(&session_digest(&mut t, s)),
            *policy_nv,
            "PolicyNV case {number}"
        );

        // A trial session needs no data check (the operand here is not in the index).
        let trial = start_policy(&mut t, se::TRIAL, *session_hash);
        ok(&pw(
            &mut t,
            cc::POLICY_NV,
            &handles(trial),
            auth,
            &args(&data[4..8], 4),
        ));
        assert_eq!(hex_of(&session_digest(&mut t, trial)), *policy_nv);
        ok(&run(&mut t, plain(cc::POLICY_RESTART, &[trial], &[])));
        ok(&pw(
            &mut t,
            cc::POLICY_AUTHORIZE_NV,
            &handles(trial),
            auth,
            &[],
        ));
        assert_eq!(
            hex_of(&session_digest(&mut t, trial)),
            *authorize_nv,
            "PolicyAuthorizeNV case {number}"
        );
    }
}

fn hash_size(hash: u16) -> usize {
    crate::crypto::HashAlg::from_alg(hash).unwrap().size()
}

// ------------------------------------------------------------------------------------ hierarchies

/// Python `hashlib`: the policyDigest of `TPM2_PolicyCommandCode(TPM_CC_HierarchyChangeAuth)` from
/// the Zero Digest of a SHA-256, SHA-384 and SHA-512 session.
const HIER_POLICY: [(u16, &str); 3] = [
    (
        alg::SHA256,
        "be698d8bd14927cd7ed7bdd93be41f27ff2626bd2d8a1540e2f9445f2ee5ab2e",
    ),
    (
        alg::SHA384,
        "db6dd12af5a3f465556c2bfc15ddf609261d71013f26f06c2f77f2a878f5b416bdf47d79eee73e6d6c6caf3c5032ac2a",
    ),
    (
        alg::SHA512,
        "ee8fcc5cdca248b805ace6dcf3ce1f285296a538a9979941bf23bbbc5811254ba8e8d4f37ddbd0dd971496d1076911e5ff3e9a7d4f6c27320bbb0fd795aec67c",
    ),
];

fn set_primary_policy(t: &mut Tpm, hierarchy: u32, digest: &[u8], hash: u16) -> Vec<u8> {
    pw(
        t,
        cc::SET_PRIMARY_POLICY,
        &[hierarchy],
        b"",
        &cat(&[&tpm2b(digest), &u16b(hash)]),
    )
}

/// `TPM2_HierarchyChangeAuth` of the owner under a policy session whose HMAC is empty (the policy
/// does not ask for the authValue).
fn change_owner_auth_under(t: &mut Tpm, session: u32) -> Vec<u8> {
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
            cc::HIERARCHY_CHANGE_AUTH,
            &[OWNER],
            &area,
            &tpm2b(b"new-owner"),
        ),
    )
}

#[test]
fn hierarchy_policy_48_octets_follows_session_hash() {
    let mut t = started();
    let digest = |hash: u16| hex(HIER_POLICY.iter().find(|(h, _)| *h == hash).unwrap().1);
    let (d384, d512) = (digest(alg::SHA384), digest(alg::SHA512));
    assert_eq!((d384.len(), d512.len()), (48, 64));

    // The digest must be one digest of hashAlg (TPM_RC_SIZE); a hash the TPM lacks is TPM_RC_HASH.
    err(
        &set_primary_policy(&mut t, OWNER, &d384[..32], alg::SHA384),
        Rc::SIZE.in_param(1),
    );
    err(
        &set_primary_policy(&mut t, OWNER, &d384, alg::SHA256),
        Rc::SIZE.in_param(1),
    );
    err(
        &set_primary_policy(&mut t, OWNER, &d384, alg::SHA512),
        Rc::SIZE.in_param(1),
    );
    err(
        &set_primary_policy(&mut t, OWNER, &d384, alg::NULL),
        Rc::SIZE.in_param(1),
    );
    err(
        &set_primary_policy(&mut t, OWNER, &[1; 20], 0x0004),
        Rc::HASH.in_param(2),
    );
    err(
        &set_primary_policy(&mut t, OWNER, &[1; 65], alg::SHA512),
        Rc::SIZE.in_param(1),
    );
    assert_eq!(t.permanent_policy(OWNER), None);

    ok(&set_primary_policy(&mut t, OWNER, &d384, alg::SHA384));
    ok(&set_primary_policy(
        &mut t,
        handle::RH_ENDORSEMENT,
        &d512,
        alg::SHA512,
    ));
    let policy = t.permanent_policy(OWNER).unwrap();
    assert_eq!(
        (policy.alg, policy.digest()),
        (crate::crypto::HashAlg::Sha384, &d384[..])
    );
    // TPM_CAP_AUTH_POLICIES lists each policy with its own hash.
    let listed = cap_list(&get_cap(&mut t, cap::AUTH_POLICIES, 0x4000_0000, 8));
    assert!(!listed.0);
    assert_eq!(
        listed.2,
        cat(&[
            &u32b(OWNER),
            &u16b(alg::SHA384),
            &d384,
            &u32b(handle::RH_LOCKOUT),
            &u16b(alg::NULL),
            &u32b(handle::RH_ENDORSEMENT),
            &u16b(alg::SHA512),
            &d512,
            &u32b(handle::RH_PLATFORM),
            &u16b(alg::NULL),
        ])
    );

    // A session whose hash is another one has another policyDigest for the same assertion, and the
    // 48-octet policy matches no digest of 32 or 64 octets: only the SHA-384 session authorizes.
    for hash in [alg::SHA256, alg::SHA512, alg::SHA384] {
        let s = start_policy(&mut t, se::POLICY, hash);
        ok(&run(
            &mut t,
            plain(
                cc::POLICY_COMMAND_CODE,
                &[s],
                &u32b(cc::HIERARCHY_CHANGE_AUTH),
            ),
        ));
        assert_eq!(
            session_digest(&mut t, s),
            digest(hash),
            "digest of hash {hash:#x}"
        );
        let resp = change_owner_auth_under(&mut t, s);
        if hash == alg::SHA384 {
            ok(&resp);
        } else {
            err(&resp, Rc::POLICY_FAIL.in_session(1));
        }
    }
    // The command changed the owner authValue: the new value authorizes, the empty one does not.
    err(&set_auth_pw(&mut t, b"", b"x"), Rc::BAD_AUTH.in_session(1));
    ok(&set_auth_pw(&mut t, b"new-owner", b""));

    // The policy is cleared with NULL and an empty digest (a SHA-384 policy is not a SHA-256 one).
    ok(&pw(
        &mut t,
        cc::SET_PRIMARY_POLICY,
        &[OWNER],
        b"",
        &cat(&[&tpm2b(&[]), &u16b(alg::NULL)]),
    ));
    assert_eq!(t.permanent_policy(OWNER), None);
}

fn set_auth_pw(t: &mut Tpm, password: &[u8], new: &[u8]) -> Vec<u8> {
    pw(
        t,
        cc::HIERARCHY_CHANGE_AUTH,
        &[OWNER],
        password,
        &tpm2b(new),
    )
}

// ---------------------------------------------------------------------------------- snapshot

#[test]
fn nv_sha384_survives_snapshot_restore() {
    use super::tests_b::startup;
    use crate::codes::su;
    let mut t = started();
    let (a, b, c, d, e) = (NV0 + 1, NV0 + 2, NV0 + 3, NV0 + 4, NV0 + 5);
    let (auth_a, auth_b, auth_c, auth_e) =
        (seq(48, 0x30), seq(48, 0x50), seq(48, 0x70), seq(64, 0x90));
    ok(&define(&mut t, a, alg::SHA384, RW, &[], 40, &auth_a));
    ok(&define(
        &mut t,
        b,
        alg::SHA384,
        RW | COUNTER,
        &[],
        8,
        &auth_b,
    ));
    ok(&define(
        &mut t,
        c,
        alg::SHA384,
        RW | EXTEND,
        &[],
        48,
        &auth_c,
    ));
    let policy_attrs = nv_attr::POLICYWRITE | nv_attr::OWNERREAD | nv_attr::NO_DA;
    ok(&define(
        &mut t,
        d,
        alg::SHA384,
        policy_attrs,
        &[0x42; 48],
        4,
        b"",
    ));
    ok(&define(
        &mut t,
        e,
        alg::SHA512,
        RW | EXTEND,
        &[],
        64,
        &auth_e,
    ));
    let data = seq(40, 0x80);
    ok(&nv_write(&mut t, a, a, &auth_a, &data, 0));
    ok(&pw(&mut t, cc::NV_INCREMENT, &[b, b], &auth_b, &[]));
    ok(&pw(&mut t, cc::NV_EXTEND, &[c, c], &auth_c, &tpm2b(b"abc")));
    ok(&pw(&mut t, cc::NV_EXTEND, &[e, e], &auth_e, &tpm2b(b"abc")));
    let d384 = hex(
        "db6dd12af5a3f465556c2bfc15ddf609261d71013f26f06c2f77f2a878f5b416bdf47d79eee73e6d6c6caf3c5032ac2a",
    );
    let d512 = hex(
        "ee8fcc5cdca248b805ace6dcf3ce1f285296a538a9979941bf23bbbc5811254ba8e8d4f37ddbd0dd971496d1076911e5ff3e9a7d4f6c27320bbb0fd795aec67c",
    );
    ok(&set_primary_policy(&mut t, OWNER, &d384, alg::SHA384));
    ok(&set_primary_policy(
        &mut t,
        handle::RH_ENDORSEMENT,
        &d512,
        alg::SHA512,
    ));
    let publics: Vec<_> = [a, b, c, d, e]
        .iter()
        .map(|i| read_public(&mut t, *i))
        .collect();
    assert_eq!(hex_of(&publics[0].1), NAME_A_WRITTEN);
    assert_eq!(hex_of(&publics[1].1), NAME_B_WRITTEN);
    assert_eq!(hex_of(&publics[2].1), NAME_C_WRITTEN);
    assert_eq!(hex_of(&publics[3].1), NAME_D);
    assert_eq!(publics[4].1.len(), 66);
    assert_eq!(&publics[4].1[..2], &u16b(alg::SHA512)[..]);
    assert_eq!(&publics[4].1[2..], &sha2::Sha512::digest(&publics[4].0)[..]);

    // The record of an index already holds nameAlg, authPolicy and authValue: the version of the
    // snapshot stays 8, and a SHA-256-only snapshot is byte-identical (the golden flow above).
    let snapshot = t.persistent_snapshot();
    assert_eq!(&snapshot[8..10], &8u16.to_be_bytes());
    let mut restored = Tpm::restore(&snapshot, &ENTROPY).unwrap();
    assert_eq!(restored.persistent_snapshot(), snapshot);
    startup(&mut restored, su::CLEAR);
    for (index, (public, name)) in [a, b, c, d, e].iter().zip(&publics) {
        assert_eq!(
            &read_public(&mut restored, *index),
            &(public.clone(), name.clone())
        );
    }
    // The data and the authValues of 48 and 64 octets are restored, and the commands go on.
    assert_eq!(
        read_data(&read_index(&mut restored, a, &auth_a, 40, 0)),
        data
    );
    assert_eq!(
        read_data(&read_index(&mut restored, b, &auth_b, 8, 0)),
        1u64.to_be_bytes()
    );
    ok(&pw(&mut restored, cc::NV_INCREMENT, &[b, b], &auth_b, &[]));
    assert_eq!(
        read_data(&read_index(&mut restored, b, &auth_b, 8, 0)),
        2u64.to_be_bytes()
    );
    assert_eq!(
        hex_of(&read_data(&read_index(&mut restored, c, &auth_c, 48, 0))),
        EXTEND_1
    );
    ok(&pw(
        &mut restored,
        cc::NV_EXTEND,
        &[c, c],
        &auth_c,
        &tpm2b(b"def"),
    ));
    assert_eq!(
        hex_of(&read_data(&read_index(&mut restored, c, &auth_c, 48, 0))),
        EXTEND_2
    );
    let extend_e = sha2::Sha512::digest(
        [
            &sha2::Sha512::digest([&[0u8; 64][..], b"abc"].concat())[..],
            b"x",
        ]
        .concat(),
    );
    ok(&pw(
        &mut restored,
        cc::NV_EXTEND,
        &[e, e],
        &auth_e,
        &tpm2b(b"x"),
    ));
    assert_eq!(
        read_data(&read_index(&mut restored, e, &auth_e, 64, 0)),
        extend_e.to_vec()
    );
    // A wrong password (one octet changed in the last position of 48) still fails.
    let mut wrong = auth_a.clone();
    wrong[47] ^= 1;
    err(
        &read_index(&mut restored, a, &wrong, 40, 0),
        Rc::BAD_AUTH.in_session(1),
    );
    // The primary policies keep their hash and digest.
    let policy = restored.permanent_policy(OWNER).unwrap();
    assert_eq!(
        (policy.alg, policy.digest()),
        (crate::crypto::HashAlg::Sha384, &d384[..])
    );
    let policy = restored.permanent_policy(handle::RH_ENDORSEMENT).unwrap();
    assert_eq!(
        (policy.alg, policy.digest()),
        (crate::crypto::HashAlg::Sha512, &d512[..])
    );
}

// ------------------------------------------------------------------------------------ NV_Certify

/// Python `hashlib`: the digests of the 40 octets `80 81 ...` of the index below.
const DATA_SHA256: &str = "410e1925d90a2fa4d7b4d57effe97c579e035009c89ed4ca41e992d296d73135";
const DATA_SHA384: &str = "39022e4c4648ba0f717731c5dc9b43f207779086cdd6d50023073518c9632335466efb55e970dec5399fd0e2a77fcf6c";
const DATA_SHA512: &str = "81c964ef22c03f1901f930ecca428a5967f44c05f5993d2de28249445cc2d7ea710a13e7c0e5a31b3a357b62497c743647b00e48c10cd63e17b67a80a45368af";

/// An owner primary ECDSA key (P-256) whose scheme names `hash`.
fn signing_key(t: &mut Tpm, hash: u16) -> super::tests_obj::Primary {
    let scheme = crate::marshal::Scheme {
        alg: alg::ECDSA,
        hash,
    };
    parse_primary(&create_primary(
        t,
        OWNER,
        b"pw",
        &ecc(base() | oa::SIGN, SymDef::Null, scheme),
    ))
}

/// `TPM2_NV_Certify` of `index` with the signing key `signer` (`TPM_RH_NULL`: no signature).
fn nv_certify(
    t: &mut Tpm,
    signer: u32,
    index: u32,
    scheme: &[u8],
    size: u16,
    offset: u16,
) -> Vec<u8> {
    let key_pw: &[u8] = if signer == NULL { b"" } else { b"pw" };
    run(
        t,
        with_pw(
            cc::NV_CERTIFY,
            &[signer, OWNER, index],
            &[key_pw, b""],
            &cat(&[&tpm2b(b"fresh"), scheme, &u16b(size), &u16b(offset)]),
        ),
    )
}

/// The attestation and the signature of each certification below, verified outside this implementation
/// by `openssl dgst -sha512 -verify` (`-sha384` for the last) against the public key (x || y of the
/// P-256 key, a SubjectPublicKeyInfo built in Python; the same check with the last octet of the
/// attestation changed fails). The Python script rebuilt the tail of each TPMS_ATTEST (the Name of
/// the index, 66 octets, and the digest or the range) from `hashlib` and found it equal; the
/// header fields (clock, firmware) are not independently verified.
const CERTIFY_VECTORS: [(&str, &str, &str, &str); 3] = [
    (
        "sha512_whole",
        "ad5fa46a51590c1528ea3bf97612035f579c00fb39bb4057f67fab428bf8d68f6820f819e38af55003d0e78952b0ec0a6348eedf3032888c7cfa33147389db67",
        "ff544347801c0022000b61e98dd7ba53f761a8a6ef0f027b31a04eb036e93f7c8ec8edf30b4107d3fde6000566726573680000000000000000058cf7c3daa6829801ea772641134661780042000d83254e55940aeff552ed0a038fce834d40c0c8daef1dd6d99cea6d851f544cfc25726ae049364a7687cb974205da442b7add8e25450ac44e45e0752a33ef643f004081c964ef22c03f1901f930ecca428a5967f44c05f5993d2de28249445cc2d7ea710a13e7c0e5a31b3a357b62497c743647b00e48c10cd63e17b67a80a45368af",
        "0018000d00201d28e1f22633541a2e0122b9e53360efff8ae11468b6687dddaa316f3021a02200204ea4bec66226f52ee8eb1b5f4749ac2c0fefea5e95e19a08dac93b752e88e30d",
    ),
    (
        "sha512_range",
        "ad5fa46a51590c1528ea3bf97612035f579c00fb39bb4057f67fab428bf8d68f6820f819e38af55003d0e78952b0ec0a6348eedf3032888c7cfa33147389db67",
        "ff54434780140022000b61e98dd7ba53f761a8a6ef0f027b31a04eb036e93f7c8ec8edf30b4107d3fde6000566726573680000000000000000058cf7c3daa6829801ea772641134661780042000d83254e55940aeff552ed0a038fce834d40c0c8daef1dd6d99cea6d851f544cfc25726ae049364a7687cb974205da442b7add8e25450ac44e45e0752a33ef643f000400088485868788898a8b",
        "0018000d0020bacd81e59301e9acb34db36bd5f908a36e64998e11e74dcf2725c0138c33a23e0020d90e54d1f1ef73fec36036737ad5c422e7771cbc75e17bbf9772d29905b6d752",
    ),
    (
        "sha384_whole",
        "6b3e7a69cc0c59971737655ecfa37d7a996ede5bd3e88c5c41e558ceb02355f02d67f0aad841a6458509cb38b840089fe257729a6ffa1e00142d27161d8ad795",
        "ff544347801c0022000b720e4607b7a96a8355070a580fb3b84e708e46c2b5bd8e7d6264d12d9fda34260005667265736800000000000000004748f2c8ed76035e01108febf8f71f3a840042000d83254e55940aeff552ed0a038fce834d40c0c8daef1dd6d99cea6d851f544cfc25726ae049364a7687cb974205da442b7add8e25450ac44e45e0752a33ef643f003039022e4c4648ba0f717731c5dc9b43f207779086cdd6d50023073518c9632335466efb55e970dec5399fd0e2a77fcf6c",
        "0018000c00203a1f7b44a37894810bcb49d47fede048b5b94ef27b050f308a929f6a0c7fc1cf0020b09cf6f09d6bf05855536b73c20264d2566aa4c12daffc36c6b763d298b81a4c",
    ),
];

#[test]
fn nv_index_sha512_certify_digest_follows_alg() {
    let mut t = started();
    let index = NV0 + 1;
    let data = seq(40, 0x80);
    ok(&define(&mut t, index, alg::SHA512, RW, &[], 40, b""));
    ok(&nv_write(&mut t, OWNER, index, b"", &data, 0));
    let name = read_public(&mut t, index).1;
    assert_eq!((name.len(), &name[..2]), (66, &u16b(alg::SHA512)[..]));
    let key512 = signing_key(&mut t, alg::SHA512);
    let key384 = signing_key(&mut t, alg::SHA384);
    let attested_name = tpm2b(&name);
    let ecdsa = |hash: u16| cat(&[&u16b(alg::ECDSA), &u16b(hash)]);

    for (label, key, whole, size, offset) in [
        ("sha512_whole", &key512, true, 0, 0),
        ("sha512_range", &key512, false, 8, 4),
        ("sha384_whole", &key384, true, 0, 0),
    ] {
        let reply = nv_certify(&mut t, key.handle, index, &u16b(alg::NULL), size, offset);
        let (attest, signature) = parse_attestation(&reply);
        let public = unmarshal_public(&key.public).unique;
        let Unique::Ecc { x, y } = public else {
            panic!("not an ECC key")
        };
        let (pinned_label, pinned_public, pinned_attest, pinned_signature) =
            CERTIFY_VECTORS.iter().find(|v| v.0 == label).unwrap();
        assert_eq!(
            hex_of(&[x.as_slice(), y.as_slice()].concat()),
            *pinned_public,
            "{label}"
        );
        assert_eq!(hex_of(&attest), *pinned_attest, "{pinned_label}");
        assert_eq!(hex_of(&signature), *pinned_signature, "{pinned_label}");
        // The Name of the index is attested whatever the hash of the signature, 66 octets here.
        let digest = if key.handle == key512.handle {
            &data_digest(DATA_SHA512)
        } else {
            &data_digest(DATA_SHA384)
        };
        let tail = if whole {
            assert_eq!(attest[4..6], [0x80, 0x1c]);
            cat(&[&attested_name, &tpm2b(digest)])
        } else {
            assert_eq!(attest[4..6], [0x80, 0x14]);
            cat(&[&attested_name, &u16b(offset), &tpm2b(&data[4..12])])
        };
        assert!(attest.ends_with(&tail), "{label}");
        // The signature names the hash of the scheme of the key.
        let hash = if key.handle == key512.handle {
            alg::SHA512
        } else {
            alg::SHA384
        };
        assert_eq!(signature[..4], cat(&[&u16b(alg::ECDSA), &u16b(hash)])[..]);
    }
    // The digest of the index is hashed with the hash of the scheme: a key whose scheme is SHA-384
    // certifies a SHA-512 index with a 48-octet digest, and inScheme may repeat the hash.
    let named = nv_certify(&mut t, key384.handle, index, &ecdsa(alg::SHA384), 0, 0);
    let (attest, _) = parse_attestation(&named);
    assert!(attest.ends_with(&tpm2b(&data_digest(DATA_SHA384))));
    err(
        &nv_certify(&mut t, key384.handle, index, &ecdsa(alg::SHA512), 0, 0),
        Rc::SCHEME.in_param(2),
    );
    // Without a signing key the hash of inScheme digests the index, whatever its nameAlg.
    for (hash, expected) in [
        (alg::SHA256, DATA_SHA256),
        (alg::SHA384, DATA_SHA384),
        (alg::SHA512, DATA_SHA512),
    ] {
        let unsigned = nv_certify(&mut t, NULL, index, &ecdsa(hash), 0, 0);
        let (attest, signature) = parse_attestation(&unsigned);
        assert!(attest.ends_with(&cat(&[&attested_name, &tpm2b(&data_digest(expected))])));
        assert_eq!(signature, [0, 0x10]);
    }

    // qualifyingData is a TPM2B_DATA, up to sizeof(TPMT_HA) = 66 octets (35 were refused while SHA-256
    // was the only hash); 67 is too long.
    let qualified = |t: &mut Tpm, len: usize| {
        run(
            t,
            with_pw(
                cc::NV_CERTIFY,
                &[key512.handle, OWNER, index],
                &[b"pw", b""],
                &cat(&[&tpm2b(&vec![1; len]), &u16b(alg::NULL), &u16b(0), &u16b(0)]),
            ),
        )
    };
    for len in [35, 66] {
        let (attest, _) = parse_attestation(&qualified(&mut t, len));
        // extraData follows the Name of the signer in the TPMS_ATTEST.
        assert!(
            attest
                .windows(len + 2)
                .any(|w| w == cat(&[&u16b(len as u16), &vec![1; len]]))
        );
    }
    err(&qualified(&mut t, 67), Rc::SIZE.in_param(1));
}

fn data_digest(hex_digest: &str) -> Vec<u8> {
    hex(hex_digest)
}

// ------------------------------------------------------------------ credentials and duplication

fn primary_handle(resp: &[u8]) -> u32 {
    ok(resp);
    u32::from_be_bytes(resp[10..14].try_into().unwrap())
}

/// `(handle, Name)` of a Load reply; a SHA-512 Name is 66 octets.
fn loaded(resp: &[u8]) -> (u32, Vec<u8>) {
    ok(resp);
    let mut r = Reader::new(params(resp, 1));
    let name = r.tpm2b(66).unwrap().to_vec();
    r.finish().unwrap();
    (u32::from_be_bytes(resp[10..14].try_into().unwrap()), name)
}

fn named(mut public: crate::marshal::TpmtPublic, name_alg: u16) -> crate::marshal::TpmtPublic {
    public.name_alg = name_alg;
    public
}

/// An owner primary storage key of `name_alg`; `rsa` picks RSA-2048 over P-256.
fn storage_primary(t: &mut Tpm, rsa: bool, name_alg: u16, auth: &[u8]) -> u32 {
    let template = if rsa { rsa_storage() } else { ecc_storage() };
    primary_handle(&create_primary(t, OWNER, auth, &named(template, name_alg)))
}

/// Creates and loads a child of `parent` (password `parent_pw`); `(handle, Name, public, private)`.
fn child(
    t: &mut Tpm,
    parent: u32,
    parent_pw: &[u8],
    public: &crate::marshal::TpmtPublic,
    auth: &[u8],
    data: &[u8],
) -> (u32, Vec<u8>, Vec<u8>, Vec<u8>) {
    let created = create_child(t, parent, parent_pw, auth, data, public);
    ok(&created);
    let c = super::tests_obj::parse_child(&created);
    let (handle, name) = loaded(&load(t, parent, parent_pw, &c.private, &c.public));
    (handle, name, c.public, c.private)
}

/// `(encryptionKeyOut, duplicate, outSymSeed)` of `TPM2_Duplicate`.
type Duplicated = (Vec<u8>, Vec<u8>, Vec<u8>);

/// `TPM2_Duplicate` called without its authorization (it needs a policy session for the DUP role;
/// the wrappers are what is tested): `(encryptionKeyOut, duplicate, outSymSeed)`.
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
    let secret = r.tpm2b(1024).unwrap().to_vec();
    r.finish().unwrap();
    Ok((key, blob, secret))
}

fn sym_bytes(symmetric: SymDef) -> Vec<u8> {
    let mut w = Writer::new();
    symmetric.marshal(&mut w);
    w.finish()
}

#[allow(clippy::too_many_arguments)]
fn import(
    t: &mut Tpm,
    parent: u32,
    parent_pw: &[u8],
    key: &[u8],
    public: &[u8],
    blob: &[u8],
    secret: &[u8],
    symmetric: SymDef,
) -> Vec<u8> {
    run(
        t,
        with_pw(
            cc::IMPORT,
            &[parent],
            &[parent_pw],
            &cat(&[
                &tpm2b(key),
                &tpm2b(public),
                &tpm2b(blob),
                &tpm2b(secret),
                &sym_bytes(symmetric),
            ]),
        ),
    )
}

/// `TPM2_MakeCredential` (no authorization): `(credentialBlob, secret)`.
fn make_credential(
    t: &mut Tpm,
    key: u32,
    credential: &[u8],
    object_name: &[u8],
) -> Result<(Vec<u8>, Vec<u8>), Rc> {
    let resp = run(
        t,
        plain(
            cc::MAKE_CREDENTIAL,
            &[key],
            &cat(&[&tpm2b(credential), &tpm2b(object_name)]),
        ),
    );
    if rc(&resp) != 0 {
        return Err(Rc(rc(&resp)));
    }
    let mut r = Reader::new(params(&resp, 0));
    let blob = r.tpm2b(1024).unwrap().to_vec();
    let secret = r.tpm2b(1024).unwrap().to_vec();
    r.finish().unwrap();
    Ok((blob, secret))
}

/// `TPM2_ActivateCredential` called without the ADMIN-role authorization of the activated object.
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

#[test]
fn make_credential_objectname_66_octets_for_sha512() {
    let mut t = started();
    let sha256 = storage_primary(&mut t, false, alg::SHA256, b"");
    let sha384 = storage_primary(&mut t, false, alg::SHA384, b"");
    let sha512 = storage_primary(&mut t, false, alg::SHA512, b"");
    // TPM2B_NAME is sizeof(TPMU_NAME): 2 + 64 octets, the Name of a SHA-512 object; 67 is too long,
    // whatever the hash of the key. (35 octets were too long while SHA-256 was the only hash.)
    for key in [sha256, sha384, sha512] {
        for len in [0, 34, 35, 50, 66] {
            assert!(
                make_credential(&mut t, key, &[7; 32], &vec![8; len]).is_ok(),
                "{len}"
            );
        }
        assert_eq!(
            make_credential(&mut t, key, &[7; 32], &[8; 67]),
            Err(Rc::SIZE.in_param(2))
        );
    }
    // The credential is at most one digest of the nameAlg of the key (TPM_RC_SIZE).
    for (key, digest) in [(sha256, 32), (sha384, 48), (sha512, 64)] {
        let name = [9u8; 66];
        let (blob, secret) = make_credential(&mut t, key, &vec![7; digest], &name).unwrap();
        // credentialBlob = integrity (one digest, size-prefixed) || the encrypted size-prefixed credential.
        assert_eq!(blob.len(), 2 + digest + 2 + digest);
        assert_eq!(blob[..2], (digest as u16).to_be_bytes());
        assert!(!secret.is_empty());
        assert!(make_credential(&mut t, key, &vec![7; digest.min(32)], &name).is_ok());
        assert_eq!(
            make_credential(&mut t, key, &vec![7; digest + 1], &name),
            Err(Rc::SIZE.in_param(1))
        );
    }
}

#[test]
fn duplicate_import_activate_sha384_objects() {
    use super::tests_obj::{sealed, sign, verify};
    use crate::marshal::Scheme;
    let mut t = started();
    let source = storage_primary(&mut t, false, alg::SHA384, b"source-pw");
    // (new parent: RSA?, nameAlg, inner symmetric wrapper): the outer wrapper (KDFa, HMAC, and OAEP
    // or KDFe) uses the nameAlg of the new parent, the inner integrity the nameAlg of the object.
    let routes = [
        (false, alg::SHA384, SymDef::Aes128Cfb),
        (false, alg::SHA512, SymDef::Aes256Cfb),
        (true, alg::SHA384, SymDef::Null),
        (true, alg::SHA512, SymDef::Aes128Cfb),
    ];
    for (route, (rsa, parent_alg, inner)) in routes.into_iter().enumerate() {
        let parent_hash = if parent_alg == alg::SHA384 { 48 } else { 64 };
        let new_parent = storage_primary(&mut t, rsa, parent_alg, b"new-pw");
        for object_alg in [alg::SHA384, alg::SHA512] {
            let digest_size = if object_alg == alg::SHA384 { 48 } else { 64 };
            // A sealed data object and an ECDSA signing key, both movable (no fixedTPM, no fixedParent).
            let mut sealed = named(sealed(), object_alg);
            sealed.attributes &= !(oa::FIXED_TPM | oa::FIXED_PARENT);
            let mut signing = named(
                ecc(
                    oa::SIGN | oa::USER_WITH_AUTH | oa::SENSITIVE_DATA_ORIGIN,
                    SymDef::Null,
                    Scheme {
                        alg: alg::ECDSA,
                        hash: object_alg,
                    },
                ),
                object_alg,
            );
            signing.attributes &= !(oa::FIXED_TPM | oa::FIXED_PARENT);
            for (is_sealed, template) in [(true, sealed), (false, signing)] {
                let label = format!("route {route}, nameAlg {object_alg:#x}, sealed {is_sealed}");
                let (object, name, public, _) = child(
                    &mut t,
                    source,
                    b"source-pw",
                    &template,
                    b"object-pw",
                    if is_sealed { b"secret data" } else { b"" },
                );
                assert_eq!(name.len(), 2 + digest_size, "{label}");

                // Duplicate to the new parent: the wrapper is the one the new parent's hash makes.
                let (key, blob, secret) =
                    duplicate(&mut t, object, new_parent, &[], inner).unwrap();
                assert_eq!(key.len(), usize::from(inner.key_bits() / 8), "{label}");
                assert_eq!(blob[..2], (parent_hash as u16).to_be_bytes(), "{label}");
                let imported = import(
                    &mut t, new_parent, b"new-pw", &key, &public, &blob, &secret, inner,
                );
                ok(&imported);
                let private = {
                    let mut r = Reader::new(params(&imported, 0));
                    let p = r.tpm2b(1024).unwrap().to_vec();
                    r.finish().unwrap();
                    p
                };
                // The imported object loads under the new parent with the same Name and works.
                let (copy, copy_name) =
                    loaded(&load(&mut t, new_parent, b"new-pw", &private, &public));
                assert_eq!(copy_name, name, "{label}");
                if is_sealed {
                    let unsealed = run(&mut t, with_pw(cc::UNSEAL, &[copy], &[b"object-pw"], &[]));
                    ok(&unsealed);
                    assert_eq!(&params(&unsealed, 0)[2..], b"secret data", "{label}");
                } else {
                    let scheme = cat(&[&u16b(alg::ECDSA), &u16b(object_alg)]);
                    let digest = vec![0x5a; digest_size];
                    let signed = sign(&mut t, copy, b"object-pw", &digest, &scheme);
                    ok(&signed);
                    // The original key verifies the signature of its copy.
                    ok(&verify(&mut t, object, &digest, params(&signed, 0)));
                }

                // A changed octet of the duplicate, of the seed or of the inner key is an integrity
                // (or value) failure and imports nothing.
                let mut bad = blob.clone();
                *bad.last_mut().unwrap() ^= 1;
                err(
                    &import(
                        &mut t, new_parent, b"new-pw", &key, &public, &bad, &secret, inner,
                    ),
                    Rc::INTEGRITY.in_param(3),
                );
                if !key.is_empty() {
                    let mut bad_key = key.clone();
                    bad_key[0] ^= 1;
                    err(
                        &import(
                            &mut t, new_parent, b"new-pw", &bad_key, &public, &blob, &secret, inner,
                        ),
                        Rc::INTEGRITY.in_param(3),
                    );
                }

                // Without a new parent only the inner wrapper stays, and it imports under any parent.
                if inner != SymDef::Null {
                    let (key, blob, secret) = duplicate(&mut t, object, NULL, &[], inner).unwrap();
                    assert!(secret.is_empty());
                    ok(&import(
                        &mut t,
                        new_parent,
                        b"new-pw",
                        &key,
                        &public,
                        &blob,
                        &[],
                        inner,
                    ));
                }

                // MakeCredential to the storage key of the new parent and ActivateCredential of the
                // object with it: the credential is one digest of the hash of the key.
                let credential = seq(parent_hash, 0x21);
                let (cred_blob, cred_secret) =
                    make_credential(&mut t, new_parent, &credential, &name).unwrap();
                assert_eq!(
                    cred_blob.len(),
                    2 + parent_hash + 2 + parent_hash,
                    "{label}"
                );
                assert_eq!(
                    activate_credential(&mut t, copy, new_parent, &cred_blob, &cred_secret),
                    Ok(credential.clone()),
                    "{label}"
                );
                // Another Name does not activate it.
                let (other, ..) = child(
                    &mut t,
                    source,
                    b"source-pw",
                    &named(super::tests_obj::sealed(), alg::SHA384),
                    b"x",
                    b"y",
                );
                assert_eq!(
                    activate_credential(&mut t, other, new_parent, &cred_blob, &cred_secret),
                    Err(Rc::INTEGRITY.in_param(1)),
                    "{label}"
                );
                for h in [other, copy, object] {
                    ok(&run(&mut t, plain(cc::FLUSH_CONTEXT, &[], &u32b(h))));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// ActivateCredential and MakeCredential with the wrappers of a SHA-384 and a SHA-512 storage key
// (RSA with OAEP, ECC with KDFe). Everything below was made outside the TPM code: the keys by
// `openssl genpkey`, the OAEP ciphertexts and the ECDH by `openssl pkeyutl` (OAEP and MGF1 hash
// of the storage key, label `IDENTITY\0`), KDFa, KDFe and the HMAC by Python `hashlib`/`hmac`,
// the CFB wrapper by `openssl enc -aes-NNN-cfb` with a zero IV (script: `cred_gen.py` of the task
// scratchpad). With SHA-256 in place of the hash of the key, the OAEP padding or the KDFe differ and
// none of these opens.

const OUTSIDE_RSA_N: &str = "b2a0e1df8acefe608fb939f9075557115599ba251d8f506fbba8d12797795db80a2d1a94b066be8b40af1eec6cd49e620a745a0b434052311c39ef4c436c1cc6d286bd9c4c8310f58caf817f5e204a14beee935a6e459141091853ab79940b7be9363bca256f4a8efcfd87dcc8b31b19e2fecc0bff81ce99685a7e3d6780f7d2439a1a13e6625ff008180d6c8e39845aeb87f45a86c3e113ab00290edfa1ab22bc948fb3ff71c982a486f7c625b51309a548c79a81c7a810d1022105b0fa4c697b4a02b331dbea6c391882b78f2ee8e6bbf6ea372f2c3f62d0291886a272d5673e6c39ae9041e32372b6e42672bd62952a1fda0a1602e43e6497f7f29270fe39";
const OUTSIDE_RSA_PQ: &str = "f21a0f6493bb2d7b59e009d66540cf942d4cf3480b6aeb724f56fb68a68976828e905584808cac01dc58516386c7320017286fc846a209e6e5e4e6f1cd3718b7bd4311d27f69ee6b53ae14071d7740bab5980f6ca477e9fa28600bf3c186539db11957d2649bc9a1f30ec79b481956a8fa28aa7ad233005aa4cb11bcfcc78c41bce203c473acd416a613484e5a70e0ecbf3aa33f63134ba198feb4348b5ae705dfd01fcf2fb307cec0847d8f24a124413bab65231a7e3bcf1a90db31e63a2a13fcae76174c2087069a1c5a7ef5d279eb2e55e7e1c203e86cac600e6193ed44f74a0a6e5b2fea002643c31108a0cae2e4b36ea8503135c9a734df45084bc0d3f9";
const OUTSIDE_ECC_D: &str = "07641ec68a49e6e8dc602670095408d1d747c84b131c983c94398fdfc5a6adf5";
const OUTSIDE_ECC_X: &str = "fd7f8ad2a475a7c95f4a59ff97cd328ef8d496c8f5e5148b3eafaafde0ddb617";
const OUTSIDE_ECC_Y: &str = "121700c9b51007502b2e20205788ddddb4a58871ee1edadcc844abc6fe05221b";

type OutsideCredential = (
    bool,
    u16,
    SymDef,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);

/// (RSA key, nameAlg of the storage key, its cipher, Name of the activated object, credential,
/// credentialBlob, secret).
const OUTSIDE_CREDENTIALS: [OutsideCredential; 4] = [
    (
        true,
        alg::SHA384,
        SymDef::Aes128Cfb,
        "000c404346494c4f5255585b5e6164676a6d707376797c7f8285888b8e9194979a9da0a3a6a9acafb2b5b8bbbec1c4c7cacd",
        "909192939495969798999a9b9c9d9e9fa0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebf",
        "00305fae412290880244d38ca62858931570262d141cebca8d0b0d45661d7016dc9c4de0415a81afcd0d71f25c51b2cdc74d44368f09487a1254ae2b835c5552f389268d2399848ac378c79828fdefc41dd5419804d3c4202c7b467e14ae60a9fd3d7527",
        "625c26374956394f7af24e43ce08101999272fa7b537f721d8b29f2cafd740ac791554e123826e1f91626c45e0293e9ce65df392d500e96175d078ffe5bf6acb6e07170c449df21b9a8de5cf7ef4f0287d4139aa2786dceef89a9dbca000d40ce33320752eca723469a99c7c6014a3b350df900e61b2313d425775acf15ae31e9e56aeb8db49e63a9191a99ca69c8da3eccef6e17e22186c93e0a1394724dd1ca4e8feca6510f66a8ec444b14af8a2eb8bc4c5e70c3336fa7b01c177643af0f9275f6eb0b0b98676c49a2d6bf6668e53337abc0777c51d9eb0f0879968d44643c86915a3af07f86cd2cd7f8e0aa88fa4a261279aa70be05f0c9492396b2d084a",
    ),
    (
        true,
        alg::SHA512,
        SymDef::Aes256Cfb,
        "000d4144474a4d505356595c5f6265686b6e7174777a7d808386898c8f9295989b9ea1a4a7aaadb0b3b6b9bcbfc2c5c8cbced1d4d7dadde0e3e6e9eceff2f5f8fbfe",
        "9798999a9b9c9d9e9fa0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebfc0c1c2c3c4c5c6c7c8c9cacbcccdcecfd0d1d2d3d4d5d6",
        "0040f72807874a76d9f7e5ca760d409d89b293fc0cf2df86ef0280b46e335878e25cfc2eb6efff2959444c643524f34bb9715fceb1592189570769923bb6b8d18571fc297254ba825a91d76128cc2a953afd2ac1e62295836aa0e7d8f8050cf258ccb0d8f14ca62247941a44067792a04e10efab8bf9e3ced3756454fef030265124eb92",
        "5e19390cb8f64a28b5bcbbe9d2886dca17b00cd05c1304fd9b5a4152126ab31a4bc1025ef4ef5609b8b74485f71b59d4a4ca8117b43db2cf3073a86299e5e6d9d923b741b0ca92e77ea1b947a69543c72b6ebe58a2f1dce9a34f91fd87f96d88a5c16d3c79cd55bbad4c7585a8e1d1df6397c8ed4fb83d4127d847d424ce471a2233d79ca93e47b144512bd3f4d1754b8873f496970e7ae82f6c3a0e2f872ea5886a3144b927c2740887431eda2a48d7c4485bfb9d3f6a639c5e67efb2370143468d4eaf900910a247116229efe5a1aa694df2bf79030e0b0725019f1b60ec491f13de9b0c53d0b1e729290351f0050dec5cb90bf428f65e2d826ddf3a534c4e",
    ),
    (
        false,
        alg::SHA384,
        SymDef::Aes256Cfb,
        "000c4245484b4e5154575a5d606366696c6f7275787b7e8184878a8d909396999c9fa2a5a8abaeb1b4b7babdc0c3c6c9cccf",
        "9e9fa0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebfc0c1c2c3c4c5c6c7c8c9cacbcccd",
        "00302ac09760a890a44a367f1b27b9e9693eb3fcedfaa516f88310d603ba040ddbc5c6cbe718320c9e431b2752c9d60090378ce62d1621962a6a7a3a68ed91ce37505fddedcd24d1120dcae3ec1909f174bdce2cf191e8f11bd6efd1895b0edb795846b6",
        "002005fdcc2b59513f2e7a6ec68c11f9e5fa89122accd278bd6407931b29cfb595d70020e94820cdf5cedee5497ab6343965b3114ff50c43a1a64a65561b9cf38a2495db",
    ),
    (
        false,
        alg::SHA512,
        SymDef::Aes128Cfb,
        "000d4346494c4f5255585b5e6164676a6d707376797c7f8285888b8e9194979a9da0a3a6a9acafb2b5b8bbbec1c4c7cacdd0d3d6d9dcdfe2e5e8ebeef1f4f7fafd00",
        "a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebfc0c1c2c3c4c5c6c7c8c9cacbcccdcecfd0d1d2d3d4d5d6d7d8d9dadbdcdddedfe0e1e2e3e4",
        "0040c153cf2ab8ed32ec87bb9d08c3a10b1e89f04a2b2a533f59510f4fb77629650875a08c8090065ecb153d2b74421982c4948c637b19f5c9c643146f7d05f13e7cf80f3eafda7d8868292866d0d040a2719478cc9a29f0d86b5e076a8a5b93cf2139c9b156ff7beb08d4a12d82bb6eb04a18fecef7ec95640e95253f838538ef4d1b9b",
        "00202dd8d4404e3b937270ec840ba9181438454d60f060db58e2e0205dd0945cf4a80020be6c1e79ac94cba0204da8c9adc44cfbe3452e129187c72f16551953d32302ac",
    ),
];

/// `TPM2_MakeCredential` of the same inputs on a TPM of the fixed entropy: (credentialBlob, secret),
/// opened outside the TPM code by the script with the private keys (the credential came out).
const MADE_CREDENTIALS: [(&str, &str); 4] = [
    (
        "0030826a4950dd2bc11a7dffb3487ca7bc9d26dd3fa500e88e86c14d2beaee627e79accc024ba39040d2e611a2983d96f6e2f1996aec073fa16369a15fbbf95fa37c4b17da24f75562a4bf4dc0efd6ad3ab70158e8cd3a2dae1712b715eb1ba6fbc1b9b5",
        "28a452bb613f082e9f8616005570742ee89870509fdc481d987e14a6400dd63a870c47e3f87d8e0baa63a54608363c6fd994e1ce1e83499efba747aae5f963b30cf3e1b04c2215db4eaf3b85c83c97014fc96fe0029d1e9c578a2c87be6019c6bef3aed9fe283b4d2f91d8ce861358f4b0b92a069f668ca367a0755248b02c0d9de1a9f9c2253bb947e528687c8e5ba1b34ea4edf9192e147ac942b54e90325adcf0f3b83deae3b1d7ad0f28bd531de4ef9601e6294ec6e2afddb3013de034193a6945c0f4e9888b900c55e68cf170652763bc0ff9cdec5e810d13eb730a4e663ee73d568fd790d33f46337911e033d202dcbd9ebc4cc0041def118545802871",
    ),
    (
        "004091590089b5c6fdd0a304f66b61409ca9d83ae4f4ccedf738f0198ea9ce32552f92259cef1aa8728a91dfec7b0a39e9467e3bb17e0b1c1adfea245c670ac8088b01f244a1587782d973f2f9a1fea535a6bd58ed7aeff57e7e0ec2f02e83b060be56481d9a447dc6a871559200f3db373916bdcd3474d5e84300e69c57a4f300e85204",
        "66a8291312a66c360b7912644d15974ab392a312292cabfc67aa4f9cce29f936f4f4a1d0dbcdfc163f8ecd73f4ba9395b55951b30f0316b871bf0b2ebb04d0dad5082ebcf5fa2553383b10a8a368c66082953c54e7df7870e9d50968ff10102ec43bd88a078cd419f26e9afcee52dea227d458b83eb2944cae00e4f222f36cc184781509f210052bc163597590ce7c88e8780840f444080007c125ce703a17a7ff311391baf2e9c31dfc6672a212c6314e4909af89a05ebf4b268367e0d750f37c79b6b3052566044f352d68e1e2f6385c8784fc23370d2ed3c8bf02bb021539e483db84a42be9c1119531edaa3999f2ab3514b4f6f2a8273ff668a8c8ed3d37",
    ),
    (
        "0030587a83eb976a4f57b734f7f4a300dcb21ef401a34041c7185686f1c60d9f8cc9fd33815388c579b30384d9b280db31d2cd8faa336e4d513a92693145970bb58a3204a7f83cdbdde781eb9c3d7f43e43e83a976366b46415e5696e17348e2e555fad1",
        "0020e6787f30809181a6edd0ed48cb1e81563f309aefaca2986656c053f2370c0e210020113ea4272808eb094aba39a68f35078051e65622a2c801fdaa9a5630bbe707b2",
    ),
    (
        "0040afc2c02cf9bc5783216e2a311f0be27204cd6cfa05fc8e11f8c89ff689a9f87af7966fc4090850bf1217d5139d8079d17c9da6e3521959d136c717cfdca031a579862a40f41e947ca31706a5b8c042dce8aeae0d05b12a4930606870e661d6126bf54df2841e1d505d092dda4fdeff0fd9746d10bcb813927c783a4fabbc3407e43d",
        "0020e6787f30809181a6edd0ed48cb1e81563f309aefaca2986656c053f2370c0e210020113ea4272808eb094aba39a68f35078051e65622a2c801fdaa9a5630bbe707b2",
    ),
];

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

/// A loaded storage key (with its private part) of the key material above.
fn outside_storage_key(t: &mut Tpm, rsa: bool, name_alg: u16, symmetric: SymDef) -> u32 {
    use crate::crypto::{ecc_from_scalar, rsa_from_primes};
    use crate::hash::AuthValue;
    use crate::marshal::PublicParams;
    use crate::object::{Object, Secret, Sensitive, name_of};
    let mut public = named(if rsa { rsa_storage() } else { ecc_storage() }, name_alg);
    let secret = match &mut public.params {
        PublicParams::Rsa {
            symmetric: cipher, ..
        } => {
            *cipher = symmetric;
            public.unique = Unique::Rsa(unhex(OUTSIDE_RSA_N));
            let key = rsa_from_primes(&unhex(OUTSIDE_RSA_PQ), &unhex(OUTSIDE_RSA_N)).unwrap();
            Secret::Rsa(Box::new(key))
        }
        PublicParams::Ecc {
            symmetric: cipher, ..
        } => {
            *cipher = symmetric;
            public.unique = Unique::Ecc {
                x: unhex(OUTSIDE_ECC_X),
                y: unhex(OUTSIDE_ECC_Y),
            };
            Secret::Ecc(
                ecc_from_scalar(crate::codes::curve::NIST_P256, &unhex(OUTSIDE_ECC_D)).unwrap(),
            )
        }
        _ => unreachable!(),
    };
    let name = name_of(&public);
    let sensitive = Sensitive {
        auth: AuthValue::from_slice_trimmed_up_to(&[], 32).unwrap(),
        seed: zeroize::Zeroizing::new(Vec::new()),
        secret,
    };
    let object = Object {
        public,
        qualified_name: name.clone(),
        name,
        hierarchy: NULL,
        sensitive: Some(sensitive),
    };
    t.objects.insert(object).unwrap()
}

/// A loaded private object whose Name is `name` (the Name is what the wrapper binds).
fn outside_named_object(t: &mut Tpm, name: &[u8]) -> u32 {
    use crate::hash::AuthValue;
    use crate::object::{Object, Secret, Sensitive};
    let name_alg = u16::from_be_bytes([name[0], name[1]]);
    let object = Object {
        public: named(super::tests_obj::sealed(), name_alg),
        name: name.to_vec(),
        qualified_name: name.to_vec(),
        hierarchy: NULL,
        sensitive: Some(Sensitive {
            auth: AuthValue::from_slice_trimmed_up_to(&[], 32).unwrap(),
            seed: zeroize::Zeroizing::new(Vec::new()),
            secret: Secret::Data(zeroize::Zeroizing::new(vec![1])),
        }),
    };
    t.objects.insert(object).unwrap()
}

#[test]
fn activate_credential_opens_wrappers_made_outside_the_tpm_for_sha384_and_sha512_keys() {
    for (case, (rsa, name_alg, symmetric, name, credential, blob, secret)) in
        OUTSIDE_CREDENTIALS.into_iter().enumerate()
    {
        let (name, credential) = (unhex(name), unhex(credential));
        let (blob, secret) = (unhex(blob), unhex(secret));
        let mut t = started();
        let key = outside_storage_key(&mut t, rsa, name_alg, symmetric);
        let object = outside_named_object(&mut t, &name);
        assert_eq!(
            activate_credential(&mut t, object, key, &blob, &secret),
            Ok(credential.clone()),
            "case {case}"
        );
        // Another Name of the activated object is an integrity failure.
        let mut other_name = name.clone();
        *other_name.last_mut().unwrap() ^= 1;
        let other = outside_named_object(&mut t, &other_name);
        assert_eq!(
            activate_credential(&mut t, other, key, &blob, &secret),
            Err(Rc::INTEGRITY.in_param(1)),
            "case {case}"
        );
        // The same key material with another hash as nameAlg opens neither the OAEP ciphertext nor
        // the wrapper (OAEP, KDFe, KDFa and HMAC all follow the nameAlg of the key).
        for other_alg in [alg::SHA256, alg::SHA384, alg::SHA512] {
            if other_alg != name_alg {
                let wrong = outside_storage_key(&mut t, rsa, other_alg, symmetric);
                assert!(
                    activate_credential(&mut t, object, wrong, &blob, &secret).is_err(),
                    "case {case}, nameAlg {other_alg:#x}"
                );
            }
        }
    }
}

#[test]
fn make_credential_for_sha384_and_sha512_keys_opens_outside_the_tpm() {
    for (case, ((rsa, name_alg, symmetric, name, credential, ..), (blob, secret))) in
        OUTSIDE_CREDENTIALS
            .into_iter()
            .zip(MADE_CREDENTIALS)
            .enumerate()
    {
        let (name, credential) = (unhex(name), unhex(credential));
        // A fresh TPM of the fixed entropy: the OAEP seed and the ephemeral key are pinned.
        let mut t = started();
        let key = outside_storage_key(&mut t, rsa, name_alg, symmetric);
        let (made_blob, made_secret) = make_credential(&mut t, key, &credential, &name).unwrap();
        assert_eq!(hex_of(&made_blob), blob, "case {case}");
        assert_eq!(hex_of(&made_secret), secret, "case {case}");
        let object = outside_named_object(&mut t, &name);
        assert_eq!(
            activate_credential(&mut t, object, key, &made_blob, &made_secret),
            Ok(credential),
            "case {case}"
        );
    }
}
