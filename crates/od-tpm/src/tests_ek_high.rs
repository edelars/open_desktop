//! TCG EK Credential Profile 2.7, High Range storage templates H-6 (RSA-3072), H-3 (ECC NIST
//! P-384) and H-27 (ML-KEM-768), §5.4 (S7-23). Host side only: the module has no issuer, and no
//! certificate is provisioned; a DER certificate is stored by the caller at the standard index.
//!
//! Values that come from outside this implementation (Python 3 hashlib/hmac and OpenSSL 4.1.0,
//! 2026-11-20, script `ek_high_gen.py`): the Names (SHA-384 of the TPMT_PUBLIC the TPM created),
//! PolicyB recomputed from PolicyA and PolicyC (and equal to Table 8), the P-384 point (on the
//! curve, and the key rebuilt from the TPM scalar has the same public point), the ML-KEM key
//! (OpenSSL builds the same encapsulation key from the 64-octet seed of the TPM), the RSA key
//! (n = p * q), credentials made outside for the EKs (OAEP SHA-384 with the label IDENTITY and a
//! zero octet, ECDH and KDFe with SHA-384, OpenSSL ML-KEM-768 encapsulation and KDFa with SHA-384,
//! then KDFa, AES-256-CFB and HMAC-SHA-384) and credentials made by the TPM, opened outside by
//! decryption with a key rebuilt from the private values of the TPM. The TPM is deterministic.
//! `sha256_golden_unchanged` was recorded at HEAD 7a2cb9b, before the High Range code existed.

use sha2::{Digest, Sha256, Sha384};

use super::tests_b::{
    define, nv_read, nv_read_data, nv_write, pw_cmd, set_auth, shutdown, start_cmd, start_params,
    startup,
};
use super::tests_obj::{
    NO_PCR, Primary, create_primary, create_primary_raw, creation_params, ecc_sign, flush,
    parse_primary, read_public, secret_of,
};
use super::*;
use crate::codes::se;
use crate::credential::{DUPLICATE_LABEL, make_seed, protect_seed_blob};
use crate::crypto::HashAlg;
use crate::ek::{HighRangeEk, LowRangeEk, POLICY_B_SHA384};
use crate::marshal::{TpmtPublic, Unique};
use crate::nv::{NV_BUFFER_MAX, attr};
use crate::random::Drbg;

const EH: u32 = handle::RH_ENDORSEMENT;
const PROFILES: [HighRangeEk; 3] = [
    HighRangeEk::Rsa3072,
    HighRangeEk::EccP384,
    HighRangeEk::MlKem768,
];

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Everything the Low Range EKs of a fixed TPM produce: the input templates, the indices, the
/// CreatePrimary and ReadPublic replies and the private values.
fn low_range_transcript() -> Vec<u8> {
    let mut log = Vec::new();
    let mut put = |bytes: &[u8]| {
        log.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        log.extend_from_slice(bytes);
    };
    let mut t = started();
    for profile in [LowRangeEk::Rsa2048, LowRangeEk::EccP256] {
        let template = profile.template();
        put(&template.to_bytes());
        put(&profile.certificate_index().to_be_bytes());
        let response = create_primary(&mut t, EH, b"", &template);
        ok(&response);
        put(&response);
        let primary = parse_primary(&response);
        put(&read_public(&mut t, primary.handle));
        put(&secret_of(&t, primary.handle));
    }
    log
}

#[test]
fn sha256_golden_unchanged() {
    let digest = Sha256::digest(low_range_transcript());
    assert_eq!(
        hex_of(&digest),
        "dc8b94c77fa3dc477765837d65244b4764917988e715cdef72347bf1add36d6b",
        "Low Range golden transcript digest"
    );
}

fn make_ek(t: &mut Tpm, profile: HighRangeEk) -> Primary {
    let response = create_primary(t, EH, b"", &profile.template());
    ok(&response);
    parse_primary(&response)
}

/// PolicyB of Table 8 (SHA-384), as printed in the specification.
const TABLE_8_POLICY_B_SHA384: &str = "b26e7d28d11a50bc53d882bcf5fd3a1a074148bb35d3b4e4cb1c0ad9bde419ca\
     cb47ba09699646150f9fc000f3f80e12";

fn check_profile(
    profile: HighRangeEk,
    type_alg: &str,
    parameters: &str,
    unique_len: &[usize],
    name: &str,
    index: u32,
) {
    // The input template: type, nameAlg SHA-384, userWithAuth and adminWithPolicy SET (Table 7:
    // 0x000300f2), PolicyB, the parameters, an empty unique field.
    let mut expected = hex(type_alg);
    expected.extend(hex("000c000300f20030"));
    expected.extend(hex(TABLE_8_POLICY_B_SHA384));
    expected.extend(hex(parameters));
    expected.extend(vec![0; 2 * unique_len.len()]);
    let template = profile.template();
    assert_eq!(template.to_bytes(), expected);
    assert_eq!(template.name_alg, alg::SHA384);
    assert_eq!(template.attributes, 0x0003_00f2);
    assert_eq!(template.auth_policy, hex(TABLE_8_POLICY_B_SHA384));
    assert_eq!(profile.certificate_index(), index);

    let mut t = started();
    let ek = make_ek(&mut t, profile);
    let mut reader = Reader::new(&ek.public);
    let mut generated = TpmtPublic::unmarshal(&mut reader).unwrap();
    reader.finish().unwrap();
    match (&generated.unique, unique_len) {
        (Unique::Rsa(n), [384]) => assert!(n[0] & 0x80 != 0 && n[383] & 1 == 1),
        (Unique::Ecc { x, y }, [48, 48]) => {
            assert_eq!((x.len(), y.len()), (48, 48));
            assert!(x.iter().any(|b| *b != 0) && y.iter().any(|b| *b != 0));
        }
        (Unique::MlKem(key), [1184]) => assert_eq!(key.len(), 1184),
        (unexpected, _) => panic!("unexpected EK unique field: {unexpected:?}"),
    }
    if let Unique::Rsa(n) = &generated.unique {
        assert_eq!(n.len(), 384);
    }
    // Only the unique field differs from the input template.
    generated.unique = template.unique.clone();
    assert_eq!(generated, template);
    // The Name is nameAlg || SHA-384(TPMT_PUBLIC): computed here, and pinned from Python.
    let mut computed = hex("000c");
    computed.extend(Sha384::digest(&ek.public));
    assert_eq!(ek.name, computed);
    assert_eq!(hex_of(&ek.name), name);
    let response = read_public(&mut t, ek.handle);
    ok(&response);
    let mut reader = Reader::new(params(&response, 0));
    assert_eq!(reader.tpm2b(2048).unwrap(), ek.public);
    assert_eq!(reader.tpm2b(64).unwrap(), computed);
}

#[test]
fn ek_high_range_rsa3072_template_matches_profile() {
    // H-6 (Table 9): RSA, AES-256-CFB, scheme NULL, keyBits 3072, exponent 0 (65537), unique empty.
    check_profile(
        HighRangeEk::Rsa3072,
        "0001",
        "00060100004300100c0000000000",
        &[384],
        "000c306e09aec2bc49753df0834cf4810cdc448ebd8c71987bf37f893465823672e90c6e05e3faaa04a5dae6f51ed3ab381f",
        0x01c0_001c,
    );
}

#[test]
fn ek_high_range_p384_template_matches_profile() {
    // H-3 (Table 11): ECC, AES-256-CFB, scheme NULL, TPM_ECC_NIST_P384 (0x0004), KDF NULL, empty point.
    check_profile(
        HighRangeEk::EccP384,
        "0023",
        "000601000043001000040010",
        &[48, 48],
        "000c9a524eb89df9e34095b44011d744ad63a7d408224aeb8d5ea283b1ec350b8e05ddcf7bbcc4bed68d4ba2b5f3a9f7cd51",
        0x01c0_0016,
    );
}

#[test]
fn ek_high_range_mlkem768_template_matches_profile() {
    // H-27 (Table 13): ML-KEM, AES-256-CFB, TPM_MLKEM_768 (2), unique empty.
    check_profile(
        HighRangeEk::MlKem768,
        "00a0",
        "0006010000430002",
        &[1184],
        "000c7ceecce841f02ad9439aea294010adaafaaffaca5a7219da25774e5e00ebb36783b0aa8c0ebe37855ed9ae9f385fb906",
        0x01c0_0062,
    );
}

fn start_trial_sha384(t: &mut Tpm) -> u32 {
    let response = run(
        t,
        start_cmd(
            handle::RH_NULL,
            handle::RH_NULL,
            &start_params(&[0x44; 16], &[], se::TRIAL, &u16b(alg::NULL), alg::SHA384),
        ),
    );
    ok(&response);
    u32::from_be_bytes(response[10..14].try_into().unwrap())
}

fn h384(parts: &[&[u8]]) -> Vec<u8> {
    let mut hasher = Sha384::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().to_vec()
}

#[test]
fn ek_high_range_policy_b_is_or_of_policy_a_and_policy_c() {
    // A.2.2 to A.2.5 of the profile, with SHA-384 and a zero starting digest.
    let zero = [0u8; 48];
    let policy_a = h384(&[&h384(&[&zero, &u32b(cc::POLICY_SECRET), &u32b(EH)])]);
    assert_eq!(
        hex_of(&policy_a),
        "8bbf2266537c171cb56e403c4dc1d4b64f432611dc386e6f532050c3278c930e143e8bb1133824ccb431053871c6db53"
    );
    // Policy Index I-2: 0x01C07F02, SHA-384, attributes 0x220F1008, authPolicy PolicyA, 0x32 octets.
    let mut public = u32b(0x01c0_7f02);
    public.extend(hex("000c220f10080030"));
    public.extend(&policy_a);
    public.extend(hex("0032"));
    let mut index_name = hex("000c");
    index_name.extend(h384(&[&public]));
    assert_eq!(
        hex_of(&index_name),
        "000cdb62fca346612c976732ff4e8621fb4e858be82586486504f7d02e621f8d7d61ae32cfc60c4d120609ed6768afcf090c"
    );
    let policy_c = h384(&[&zero, &u32b(cc::POLICY_AUTHORIZE_NV), &index_name]);
    assert_eq!(
        hex_of(&policy_c),
        "d6032ce61f2fb3c240eb3cf6a33237ef2b6a16f4293c22b455e261cffd217ad5b4947c2d73e63005eed2dc2b3593d165"
    );
    let policy_b = h384(&[&zero, &u32b(cc::POLICY_OR), &policy_a, &policy_c]);
    assert_eq!(policy_b, POLICY_B_SHA384);
    assert_eq!(hex_of(&policy_b), TABLE_8_POLICY_B_SHA384);

    // The engine reaches the same digest: PolicySecret(TPM_RH_ENDORSEMENT) and PolicyOR in a trial
    // session of SHA-384.
    let mut t = started();
    let session = start_trial_sha384(&mut t);
    ok(&run(
        &mut t,
        with_pw(
            cc::POLICY_SECRET,
            &[EH, session],
            &[b""],
            &cat(&[&tpm2b(&[]), &tpm2b(&[]), &tpm2b(&[]), &u32b(0)]),
        ),
    ));
    let digest = |t: &mut Tpm| {
        let response = run(t, plain(cc::POLICY_GET_DIGEST, &[session], &[]));
        ok(&response);
        Reader::new(params(&response, 0))
            .tpm2b(64)
            .unwrap()
            .to_vec()
    };
    assert_eq!(digest(&mut t), policy_a);
    ok(&run(
        &mut t,
        plain(
            cc::POLICY_OR,
            &[session],
            &cat(&[&u32b(2), &tpm2b(&policy_a), &tpm2b(&policy_c)]),
        ),
    ));
    assert_eq!(digest(&mut t), policy_b);
}

fn recreate_ek(t: &mut Tpm, password: &[u8], profile: HighRangeEk) -> Vec<u8> {
    let inputs = creation_params(b"", b"", &profile.template(), b"", &NO_PCR);
    let response = create_primary_raw(t, EH, password, &inputs);
    ok(&response);
    let primary = parse_primary(&response);
    ok(&flush(t, primary.handle));
    primary.public
}

#[test]
fn ek_high_range_keys_are_deterministic_from_seed() {
    for profile in PROFILES {
        let mut t = Tpm::new(&[6; 48]).unwrap();
        startup(&mut t, 0);
        let first = recreate_ek(&mut t, b"", profile);
        assert_eq!(recreate_ek(&mut t, b"", profile), first, "{profile:?}");

        // A new Endorsement authorization and TPM2_Clear keep the Endorsement Primary Seed.
        ok(&set_auth(&mut t, EH, b"", b"new"));
        assert_eq!(recreate_ek(&mut t, b"new", profile), first);
        ok(&pw_cmd(&mut t, cc::CLEAR, &[handle::RH_LOCKOUT], b"", &[]));
        assert_eq!(recreate_ek(&mut t, b"", profile), first);

        // The same seeds after a restart from the snapshot.
        shutdown(&mut t, 0);
        let mut restored = Tpm::restore(&t.persistent_snapshot(), &[9; 48]).unwrap();
        startup(&mut restored, 0);
        assert_eq!(recreate_ek(&mut restored, b"", profile), first);

        // Another TPM (another seed) gives another key.
        let mut other = Tpm::new(&[7; 48]).unwrap();
        startup(&mut other, 0);
        assert_ne!(recreate_ek(&mut other, b"", profile), first);
    }
    let mut t = started();
    let publics = PROFILES.map(|profile| recreate_ek(&mut t, b"", profile));
    assert!(publics[0] != publics[1] && publics[1] != publics[2] && publics[0] != publics[2]);
}

/// An opaque DER-shaped certificate: SEQUENCE with a two-octet length, then `length - 4` bytes.
fn der_like(length: usize, fill: u8) -> Vec<u8> {
    let mut data = vec![0x30, 0x82];
    data.extend(((length - 4) as u16).to_be_bytes());
    data.extend((0..length - 4).map(|i| fill ^ (i % 251) as u8));
    data
}

#[test]
fn ek_high_range_certificate_nv_write_read() {
    const PLATFORM: u32 = handle::RH_PLATFORM;
    const OWNER: u32 = handle::RH_OWNER;
    const ATTRIBUTES: u32 = attr::PLATFORMCREATE
        | attr::PPWRITE
        | attr::OWNERREAD
        | attr::AUTHREAD
        | attr::NO_DA
        | attr::WRITEDEFINE;
    // §2.2.2.5: H-6 RSA 3072, H-3 ECC NIST P384, H-27 ML-KEM-768 (the index list of the profile).
    let cases = [
        (HighRangeEk::Rsa3072, 0x01c0_001c, der_like(1500, 0x52)),
        (HighRangeEk::EccP384, 0x01c0_0016, der_like(900, 0x45)),
        (HighRangeEk::MlKem768, 0x01c0_0062, der_like(4400, 0x4d)),
    ];
    let mut t = started();
    for (profile, index, data) in &cases {
        assert_eq!(profile.certificate_index(), *index);
        // High Range: from 0x01c00012, an even index (no nonce, no template index).
        assert!((0x01c0_0012..=0x01c0_7fff).contains(index) && index % 2 == 0);
        // A new vTPM has no certificate and the engine does not issue one.
        err(
            &run(&mut t, plain(cc::NV_READ_PUBLIC, &[*index], &[])),
            Rc::HANDLE.in_handle(1),
        );
        err(
            &define(&mut t, OWNER, *index, ATTRIBUTES, data.len() as u16),
            Rc::ATTRIBUTES.in_param(2),
        );
        ok(&define(
            &mut t,
            PLATFORM,
            *index,
            ATTRIBUTES,
            data.len() as u16,
        ));
        for (i, chunk) in data.chunks(NV_BUFFER_MAX).enumerate() {
            ok(&nv_write(
                &mut t,
                PLATFORM,
                *index,
                b"",
                chunk,
                (i * NV_BUFFER_MAX) as u16,
            ));
        }
        ok(&pw_cmd(
            &mut t,
            cc::NV_WRITE_LOCK,
            &[PLATFORM, *index],
            b"",
            &[],
        ));
    }
    // The three certificate indices are distinct, and distinct from the Low Range ones.
    let mut indices: Vec<u32> = cases.iter().map(|case| case.1).collect();
    indices.extend([
        LowRangeEk::Rsa2048.certificate_index(),
        LowRangeEk::EccP256.certificate_index(),
    ]);
    indices.sort_unstable();
    indices.dedup();
    assert_eq!(indices.len(), 5);

    ok(&pw_cmd(&mut t, cc::CLEAR, &[handle::RH_LOCKOUT], b"", &[]));
    shutdown(&mut t, 0);
    let mut t = Tpm::restore(&t.persistent_snapshot(), &[6; 48]).unwrap();
    startup(&mut t, 0);
    for (_, index, data) in &cases {
        for auth in [OWNER, *index] {
            let mut read = Vec::new();
            for (i, chunk) in data.chunks(NV_BUFFER_MAX).enumerate() {
                let response = nv_read(
                    &mut t,
                    auth,
                    *index,
                    b"",
                    chunk.len() as u16,
                    (i * NV_BUFFER_MAX) as u16,
                );
                ok(&response);
                read.extend(nv_read_data(&response));
            }
            assert_eq!(&read, data);
        }
        err(
            &nv_write(&mut t, PLATFORM, *index, b"", b"X", 0),
            Rc::NV_LOCKED,
        );
    }
}

const ACTIVATED_NAME: &str = "000b136a43f615f9fde6a02e4523a0415dd9b2cd72724e0737621027abd252056bb1";
const CREDENTIAL: &[u8] = b"external credential";

fn activate_with(
    t: &mut Tpm,
    activated: &Primary,
    ek: &Primary,
    ek_password: &[u8],
    blob: &[u8],
    secret: &[u8],
) -> Vec<u8> {
    // The activated object is authorized by its password; the EK is a USER-role key with
    // userWithAuth SET and an empty authValue.
    run(
        t,
        with_pw(
            cc::ACTIVATE_CREDENTIAL,
            &[activated.handle, ek.handle],
            &[b"act-pw", ek_password],
            &cat(&[&tpm2b(blob), &tpm2b(secret)]),
        ),
    )
}

fn activate(t: &mut Tpm, activated: &Primary, ek: &Primary, blob: &[u8], secret: &[u8]) -> Vec<u8> {
    activate_with(t, activated, ek, b"", blob, secret)
}

#[test]
fn ek_high_range_activate_credential_round_trip() {
    // A restricted decryption EK opens the secret of a credential (the seed) with its private
    // key. RSA: OAEP SHA-384; ECC: ECDH and KDFe; ML-KEM: decapsulation and KDFa (all with
    // SHA-384). It does not decrypt arbitrary data (`ek_high_range_restricted_ek_refuses_...`).
    let vectors = [
        (
            HighRangeEk::Rsa3072,
            "0030317ae5285ecfd860dc8d094eb1c652117afcf1e2c59b0727aa08cf83cbd13aeebd2806074ed9320f577200c6ef0ca081a645b860e6e367560a91d0c50e5a8c7013637fbf65",
            "47ec1db914c77fa00033f884c209d65829e1124e6c43e20db813315def0dd30c4a7266e4ae0cf26835f510db7b7681027c189717d34ebc431dc8652313f4d4adc4cc22156d17111456281b49c46cd05bd10ce29759fc6fb5bf917fdea63476810e88f69f0d154ab7ccac156e98c05bc31fc03271a628c0a3e3ff9e940a4c62699b5f3ac553bd9f07916ef5dfb658b421fa653c760dc0114916a9bdee97029e074207bcf26640725b4e81094869226d47d5282c61d28c61cd5506a98bdcb87b42744d0732fdc006b8eb09db4d09a1670ee8de326f03f36ff9027edefcb0e6a333c7dca2539d141e837bad09e145f53f6d9b81df9ab201b10a5ee79540429b5e41d867bf5b6887f9d7ad72f9dbc4961d77b2b883a2c6887fb72aee3052c52aa67a46146614272e11cc0ed148f00d588cee75cb2ed7a645b9ef6e6454b8a6a2785f7739821903c1b4307fbee60bc78c6d02c9346f970dc67bcc44c6305e33dc20b4bc48954cfd5a89d801f8d17619fc110fe816d756bc5b1811f1ca5b296fd14f88",
            "00307a97899520217d61d3eb42e3492ea1e4b3f9659fcccbb30c68b8c6e2703720e825c684d1d0bde244cad5a65dee5c25d4b09be0b399776a501e0f813c09b139fc48599d119e",
            "d57c192fe7ed85e13705f6dcf3355a88c7098496f3759452221316671f4e9428",
        ),
        (
            HighRangeEk::EccP384,
            "003035db0f6133095f837da926902666c7b52b3d1e6192fbee63c6fd0e7e61845d9a9e9e47eed40be53d4106a25f78396c958914e3247d63dc6504647711465b695ebab00a0011",
            "0030bad34cac177719ee9f4c3afc647e3c26c59227ce9edd37faddc7eaa9caca0bc5221693af4356f45840949a94fb6d4ba70030201ed2cf2eb9a4bf4251756396530b10bfaf2dbdc57040c41f068f3bc5944c976f4ea4532afd4c929057b098774f624c",
            "0030a9cf2bc0642d8c1ff672a2d96fb302e15aa1fcefd8ca265a2ddc157a6523f4facb2476706671df23a95b586660ad3f6908aded57d8313d78376c010fac664829f73768919e",
            "29357a9f57d17161d64289bc343174b0a1796294a9e6e176bd7454e8d32e2950",
        ),
        (
            HighRangeEk::MlKem768,
            "0030937e2fb9b7c8820370be6cdd07a894ac23469bc0d9d556d5d5c8b99a91a10fc1f8c95773957cb09cfd5f32bced9138a0e8f2338cdc7229f6171a8945e97d2798e5816ad670",
            "c47389b707b34357b49289b91bf2e1f116de178dcf4692516238ad33ef34677186a67b4c72bf4331b95572ce455f8391cd400da9032ac99dbd7f74e93ba229d7cc5feeea98179dc64924d7b56e8d61640b6254768f288b7c2c2d24f9bfddc8134765b8a672e2b70433dc88902509c8da3119062344a139b9ebc6aa6961bd6f818c7f97b837aade696d2236bcfac8a671f7f1475cbe7e04ce8f6a260812c35b0347c296ce7d3e568f0e97b892c02426d8bcc73a18595d6bfe41d04634232c2b84bad0ec524f2d90e963346fcdb95d311bdcbd3638d396e6addad0601f6388fb7a0643c7b0cdc793236af5e07ba9c9815363debb7255f10858a0411eae258566f4bb0ef54bb2dc12a86ec1f515c0e87575e1289b46ecd271b0227e3de792d27831b3df7b0728dad8ce73de3e53d7b0b91ae164ceabdf2d2425825f496ccb4e80769f16093d8303d1f1ef482b8b4fb721706948258d57ef891fff4f2dfaf952428f501061703da538cda7769c363daf637596647f419706918dbaa6fb86c10caa40458d9eabf9449cb4b3a9f2bbc6a5f313c11006d3ba25bbc878e925e70df4a43a1af481245309bdf9ed818d7ee157418cb0c2e3866146dd1f3fc93d818b3ae4f2054be5988b9895b4f0fd2175c5d08518a11d7b4707dd153b7f6cf938aa66c39b6ba9a188375628c2005df6f8796bbc0085312d583f2a9855f7f89dc6bef0788963cebf12b4e4316c3f5e837f651bf785ffa13c758bd26f77fcb1ee6a2c69406bf439238e15527022464a8c1a2bd17a5a8665c6ad5498092364eccfa72eee88a1b01967f747ba5325d1ff0f830725864dd3d4d65a763ad4e851aa632847593c4eac40877e22f3614784efc4f795cf88d8e9932a8375ab59c5a42ce9e6077cf2c0344fbeb4f76e9fc591ac06fbdd4ff800c641c873f855797732cc5c2070f91ecaf9d08e1c1e3dedf18b74831bb4192bae444aa50f3052234561513c00675d213e4f016a5ab3fe0725f496d083ee0ef9a2675506481004b2e017c3327c181d9f3168b13b8d04169b0ec233b442ce28c94306427d7b4cd7b3ca152fbfc6d7a0ff101091ef14740732f74d621e43c85eba049b75daae206e06d07ff16ca9bc21e91a57480c99fb61350f89cee2e2534e51f51fd2ef06b4e5c2dc6f64ebee4ec06cfe33bf41e95bd54b87932b855c884250f8b75af98d86762b2d93ff653586779376d9b10bd5ea1da90857b8a53808344f3b2efd3a54c752513ff903d2b5a15d4f33e7b19f4f96d687e653e68404229b825e5fc334987bca426345d0a23f076af6ff25d73358700d735bcfec3cd30e23ba83d6d8c6b8a9ffe318b05b5dd42c5d3ade559803faea3586f474295513137f5a32cb5ab9a4b26e92d51234721de46891844c4225da7ae5fe1a54e73d70147b50c375c5e4a3d63bd1dcc15946204c05c3f05385bd21ef1242fa86b2ecf9fd24a78da2fa466323febc609182ed59b102bb15fc74a249e80852d58707a677fdfe7cff126e2c28e83f989ecf1054bf1e3535a4",
            "0030910978520c11ac40dbedde5dd5c619baa4ecc33f9abd0c8ae4dc98937a0ce7da588ba5d910bb89eeab08e9acaf52d4ab84179e62032a5d8dd1c95451f2867fdfb68f312b36",
            "1bca7e0024ecbeab6af658b138ef90ab5dce869ec6c143a8de175b29ad27f1ea",
        ),
    ];
    for (profile, ext_blob, ext_secret, tpm_blob, tpm_secret_sha256) in vectors {
        let mut t = started();
        let ek = make_ek(&mut t, profile);
        let activated = parse_primary(&create_primary(
            &mut t,
            handle::RH_OWNER,
            b"act-pw",
            &ecc_sign(),
        ));
        assert_eq!(hex_of(&activated.name), ACTIVATED_NAME);

        // (a) The credential the TPM makes (deterministic: nothing else has drawn from the DRBG yet),
        // opened outside with the private key rebuilt from the values of the TPM, and then by the
        // TPM itself.
        let made = run(
            &mut t,
            plain(
                cc::MAKE_CREDENTIAL,
                &[ek.handle],
                &cat(&[&tpm2b(b"tpm made credential"), &tpm2b(&activated.name)]),
            ),
        );
        ok(&made);
        let mut reader = Reader::new(params(&made, 0));
        let blob = reader.tpm2b(300).unwrap().to_vec();
        let secret = reader.tpm2b(1100).unwrap().to_vec();
        reader.finish().unwrap();
        assert_eq!(hex_of(&blob), tpm_blob);
        assert_eq!(hex_of(&Sha256::digest(&secret)), tpm_secret_sha256);
        let response = activate(&mut t, &activated, &ek, &blob, &secret);
        ok(&response);
        assert_eq!(params(&response, 0), tpm2b(b"tpm made credential"));

        // (b) A credential made outside the TPM for the public key of the EK.
        let response = activate(&mut t, &activated, &ek, &hex(ext_blob), &hex(ext_secret));
        ok(&response);
        assert_eq!(params(&response, 0), tpm2b(CREDENTIAL), "{profile:?}");
        let mut tampered = hex(ext_blob);
        tampered[4] ^= 1;
        err(
            &activate(&mut t, &activated, &ek, &tampered, &hex(ext_secret)),
            Rc::INTEGRITY.in_param(1),
        );
        // The EK authValue is empty: another password does not authorize it.
        err(
            &activate_with(
                &mut t,
                &activated,
                &ek,
                b"x",
                &hex(ext_blob),
                &hex(ext_secret),
            ),
            Rc::AUTH_FAIL.in_session(2),
        );
    }

    // The label is part of the ML-KEM seed derivation: the seed of "DUPLICATE" fails the HMAC.
    let mut t = started();
    let ek = make_ek(&mut t, HighRangeEk::MlKem768);
    let activated = parse_primary(&create_primary(
        &mut t,
        handle::RH_OWNER,
        b"act-pw",
        &ecc_sign(),
    ));
    err(
        &activate(
            &mut t,
            &activated,
            &ek,
            &hex(
                "003016035c6124a55acf4403db77219daa8a90759fc39da154026058ca66f75225cedac00939ccd36d6141be8db77810b3aac96fb5cd8eab4d2339fccf7e42d6addd9e9e92a7c6",
            ),
            &hex(
                "c47389b707b34357b49289b91bf2e1f116de178dcf4692516238ad33ef34677186a67b4c72bf4331b95572ce455f8391cd400da9032ac99dbd7f74e93ba229d7cc5feeea98179dc64924d7b56e8d61640b6254768f288b7c2c2d24f9bfddc8134765b8a672e2b70433dc88902509c8da3119062344a139b9ebc6aa6961bd6f818c7f97b837aade696d2236bcfac8a671f7f1475cbe7e04ce8f6a260812c35b0347c296ce7d3e568f0e97b892c02426d8bcc73a18595d6bfe41d04634232c2b84bad0ec524f2d90e963346fcdb95d311bdcbd3638d396e6addad0601f6388fb7a0643c7b0cdc793236af5e07ba9c9815363debb7255f10858a0411eae258566f4bb0ef54bb2dc12a86ec1f515c0e87575e1289b46ecd271b0227e3de792d27831b3df7b0728dad8ce73de3e53d7b0b91ae164ceabdf2d2425825f496ccb4e80769f16093d8303d1f1ef482b8b4fb721706948258d57ef891fff4f2dfaf952428f501061703da538cda7769c363daf637596647f419706918dbaa6fb86c10caa40458d9eabf9449cb4b3a9f2bbc6a5f313c11006d3ba25bbc878e925e70df4a43a1af481245309bdf9ed818d7ee157418cb0c2e3866146dd1f3fc93d818b3ae4f2054be5988b9895b4f0fd2175c5d08518a11d7b4707dd153b7f6cf938aa66c39b6ba9a188375628c2005df6f8796bbc0085312d583f2a9855f7f89dc6bef0788963cebf12b4e4316c3f5e837f651bf785ffa13c758bd26f77fcb1ee6a2c69406bf439238e15527022464a8c1a2bd17a5a8665c6ad5498092364eccfa72eee88a1b01967f747ba5325d1ff0f830725864dd3d4d65a763ad4e851aa632847593c4eac40877e22f3614784efc4f795cf88d8e9932a8375ab59c5a42ce9e6077cf2c0344fbeb4f76e9fc591ac06fbdd4ff800c641c873f855797732cc5c2070f91ecaf9d08e1c1e3dedf18b74831bb4192bae444aa50f3052234561513c00675d213e4f016a5ab3fe0725f496d083ee0ef9a2675506481004b2e017c3327c181d9f3168b13b8d04169b0ec233b442ce28c94306427d7b4cd7b3ca152fbfc6d7a0ff101091ef14740732f74d621e43c85eba049b75daae206e06d07ff16ca9bc21e91a57480c99fb61350f89cee2e2534e51f51fd2ef06b4e5c2dc6f64ebee4ec06cfe33bf41e95bd54b87932b855c884250f8b75af98d86762b2d93ff653586779376d9b10bd5ea1da90857b8a53808344f3b2efd3a54c752513ff903d2b5a15d4f33e7b19f4f96d687e653e68404229b825e5fc334987bca426345d0a23f076af6ff25d73358700d735bcfec3cd30e23ba83d6d8c6b8a9ffe318b05b5dd42c5d3ade559803faea3586f474295513137f5a32cb5ab9a4b26e92d51234721de46891844c4225da7ae5fe1a54e73d70147b50c375c5e4a3d63bd1dcc15946204c05c3f05385bd21ef1242fa86b2ecf9fd24a78da2fa466323febc609182ed59b102bb15fc74a249e80852d58707a677fdfe7cff126e2c28e83f989ecf1054bf1e3535a4",
            ),
        ),
        Rc::INTEGRITY.in_param(1),
    );
}

#[test]
fn ek_high_range_restricted_ek_refuses_plain_decryption() {
    // The EK is restricted with decrypt SET: RSA_Decrypt, ECDH_ZGen and Decapsulate are refused
    // with TPM_RC_ATTRIBUTES for the key handle (Part 3, clauses 14.2, 14.5 and 14.11), valid
    // parameters notwithstanding. ActivateCredential is the only way to use the private key.
    let mut t = started();

    let rsa = make_ek(&mut t, HighRangeEk::Rsa3072);
    let oaep = cat(&[&u16b(alg::OAEP), &u16b(alg::SHA384)]);
    err(
        &run(
            &mut t,
            with_pw(
                cc::RSA_DECRYPT,
                &[rsa.handle],
                &[b""],
                &cat(&[&tpm2b(&[1; 384]), &oaep, &tpm2b(b"")]),
            ),
        ),
        Rc::ATTRIBUTES.in_handle(1),
    );

    let ecc = make_ek(&mut t, HighRangeEk::EccP384);
    let mut reader = Reader::new(&ecc.public);
    let Unique::Ecc { x, y } = TpmtPublic::unmarshal(&mut reader).unwrap().unique else {
        panic!("the P-384 EK has an ECC point");
    };
    err(
        &run(
            &mut t,
            with_pw(
                cc::ECDH_Z_GEN,
                &[ecc.handle],
                &[b""],
                &tpm2b(&cat(&[&tpm2b(&x), &tpm2b(&y)])),
            ),
        ),
        Rc::ATTRIBUTES.in_handle(1),
    );

    let kem = make_ek(&mut t, HighRangeEk::MlKem768);
    // Encapsulation is a public operation and works; decapsulation of its ciphertext does not.
    let encapsulated = run(&mut t, plain(cc::ENCAPSULATE, &[kem.handle], &[]));
    ok(&encapsulated);
    let mut reader = Reader::new(params(&encapsulated, 0));
    reader.tpm2b(32).unwrap();
    let ciphertext = reader.tpm2b(1088).unwrap().to_vec();
    err(
        &run(
            &mut t,
            with_pw(cc::DECAPSULATE, &[kem.handle], &[b""], &tpm2b(&ciphertext)),
        ),
        Rc::ATTRIBUTES.in_handle(1),
    );
}

#[test]
fn ek_high_range_credential_with_another_label_is_refused() {
    // The label is part of the seed derivation: OAEP label (RSA), KDFe label (ECC) and KDFa label
    // (ML-KEM). The credential is made here for the public key of the EK, once with IDENTITY (it
    // opens) and once with DUPLICATE (it does not), with everything else the same.
    for profile in PROFILES {
        let mut t = started();
        let ek = make_ek(&mut t, profile);
        let activated = parse_primary(&create_primary(
            &mut t,
            handle::RH_OWNER,
            b"act-pw",
            &ecc_sign(),
        ));
        let mut reader = Reader::new(&ek.public);
        let public = TpmtPublic::unmarshal(&mut reader).unwrap();
        let clear = tpm2b(b"labeled credential");
        let mut rng = Drbg::new(&[5; 48]).unwrap();
        let mut credential_for = |label: &[u8]| {
            let (seed, secret) = make_seed(&public, label, &mut rng, HashAlg::Sha384).unwrap();
            let (symmetric, _) = public.storage_parameters().unwrap();
            let blob =
                protect_seed_blob(&seed, symmetric, HashAlg::Sha384, &activated.name, &clear);
            (blob, secret)
        };

        let (blob, secret) = credential_for(b"IDENTITY\0");
        let response = activate(&mut t, &activated, &ek, &blob, &secret);
        ok(&response);
        assert_eq!(
            params(&response, 0),
            tpm2b(b"labeled credential"),
            "{profile:?}"
        );

        let (blob, secret) = credential_for(DUPLICATE_LABEL);
        let expected = match profile {
            // OAEP does not open under another label.
            HighRangeEk::Rsa3072 => Rc::VALUE.in_param(2),
            // The seed differs, so the integrity value of the wrapper does not match.
            HighRangeEk::EccP384 | HighRangeEk::MlKem768 => Rc::INTEGRITY.in_param(1),
        };
        err(&activate(&mut t, &activated, &ek, &blob, &secret), expected);
    }
}
