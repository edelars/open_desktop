//! ECC P-384 (slice 6 of hash and key agility, S7-23): Create/CreatePrimary/Load, ECDSA Sign and
//! VerifySignature with SHA-256, SHA-384 and SHA-512 digests, ECDH_ZGen and ECDH_KeyGen,
//! ECC_Parameters, KDFe of a P-384 storage parent, capabilities, contexts and snapshots.
//!
//! Where a value comes from outside this implementation the test says so. Scripts: Python 3
//! (`hashlib`, `hmac`, an affine-coordinate P-384 implementation with integer arithmetic) and
//! `openssl` 4.1.0 (`genpkey`, `dgst -sign`, `dgst -verify`, `pkeyutl -derive`). The TPM is deterministic
//! (fixed entropy, DRBG, RFC 6979 nonces), so a value that the TPM produced and that was then checked
//! outside this crate is "pinned" and reproduced byte for byte; a changed value prints
//! `FIXTURE <name> <hex>` on stderr before the assertion fails, to be checked again outside and
//! re-pinned. With the environment variable `P384_COLLECT` set the pins are only printed.
//! - `create_primary`: the `TPMT_PUBLIC` is marshaled by hand from Part 2 and its Name computed with
//!   `hashlib`; the point was checked to be on the curve by Python.
//! - `sign`: every signature was verified with `openssl dgst -shaN -verify` against the public key
//!   rebuilt from the pinned point (the digest is cut to 48 octets for SHA-512 by OpenSSL as well).
//! - `verify`: the key and the signatures were made by `openssl genpkey` and `dgst -sign`.
//! - `ecdh`: `Z = [d_peer] Q_tpm` was computed by Python and its x-coordinate by `openssl pkeyutl
//!   -derive`.
//! - `kdfe`: the seed of `MakeCredential` was recomputed with `hashlib` from the private scalar of the
//!   parent and the ephemeral point, and the integrity value of the blob with `hmac`.
//!
//! The ECC P-256 flows keep the replies of commit abf5923 (`sha256_p256_golden_unchanged`).

use super::tests_b::{shutdown, startup};
use super::tests_obj::{
    OWNER, base, create_child, create_primary, ecc, ecc_sign, ecc_storage, evict, flush, load,
    load_external, parse_child, parse_loaded, parse_primary, read_public, secret_of, sign,
    unmarshal_public, verify,
};
use super::*;
use crate::codes::{curve, object_attr as oa, su};
use crate::crypto::{HashAlg, hash, hmac, kdfa};
use crate::marshal::{PublicParams, Scheme, SymDef, TpmtPublic, Unique};

fn hex_of(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn digest_of(hash_alg: HashAlg, data: &[u8]) -> Vec<u8> {
    hash(hash_alg, &[data]).to_vec()
}

fn ecc_point(x: &[u8], y: &[u8]) -> Vec<u8> {
    tpm2b(&cat(&[&tpm2b(x), &tpm2b(y)]))
}

/// Pinned values, see the module comment.
const PINNED: &[(&str, &str)] = &[
    (
        "create.sha256.x",
        "a1b21a552dce85fd1a71d43c6b29868bbfe56a1ac9003f639c8c3d8280c4b8ddcdd6e714c80bce39b9e91aceda277d40",
    ),
    (
        "create.sha256.y",
        "9ddfa25bd85742014ab621cc9106f059514bbc6b2d9f632cd83396c7633272340084552e831ee12ed8fa14058fb3f0cf",
    ),
    (
        "create.sha256.name",
        "000b68dbef7e90a8259e3a469e693574c97d63a9d5f72e825c0bf0eda8d3c0b24c00",
    ),
    (
        "create.sha384.x",
        "66b08abfdb460dddea6d095b5ca069c51adce3d50d57b11fbf17f15e0ff16cb23746943555977b158079f8ebbe482e2f",
    ),
    (
        "create.sha384.y",
        "c80bfe701a8008dad6b129a8048da296bfc17ff02d64a13ee2f67d7ca3152073bfb8e84d84c8adda862c2b4b1e1fd720",
    ),
    (
        "create.sha384.name",
        "000c7c94d1822d060dbd77ee8feea833b7e60d033b0f903b881c3667e98569b86e9b6ae6ca618efd835f8eac8204c6889bff",
    ),
    (
        "create.sha512.x",
        "042a3e18ef72885ef2ed08717d9a97a1bd37d64c447448dd65350b382067902959e90dc4bd392a618859dfd19c6913c6",
    ),
    (
        "create.sha512.y",
        "378f25a7513254798f1ceb5f69376374dbdd711bd002e6d47540ea4411c531920e4a7246502c3414b221952ee5395f7d",
    ),
    (
        "create.sha512.name",
        "000d41d46953eb2a025d0fa94a2514666cb3d187ba7649c917f7e69c54d0f63601b132d9c74f603ad28124c5a30b86acb7ea0c1217f968fac4e5fa05cc4cda7ab87c",
    ),
    (
        "zgen.tpm.x",
        "67cda4f49aad67ea3fd2bd0cd49025183a994611d6455246b648e58ca6f1eee56b63001b881b6cc3eaaf342f01c5f007",
    ),
    (
        "zgen.tpm.y",
        "42d07bc866751bff97c4b9b877d2836558bbd76c6660a646ddfc43591573c3e1247cfe0f4dd84865652814b937ec69c5",
    ),
    (
        "zgen.z.x",
        "91591e87709cd50b660a57914dc8324bbdfd18e18fc0cd7fb4a02a1063a7830e7afdeb1b3ccdc883e955bef598f6cb75",
    ),
    (
        "zgen.z.y",
        "1f2fbaf29dac253abc226bfde8d2d22a5be6e1bcf149b7f2c5f7cdb126fcc04caf6cb27e1f25c72ad7f443427a5cbd45",
    ),
    (
        "sign.sha256.x",
        "a1b21a552dce85fd1a71d43c6b29868bbfe56a1ac9003f639c8c3d8280c4b8ddcdd6e714c80bce39b9e91aceda277d40",
    ),
    (
        "sign.sha256.y",
        "9ddfa25bd85742014ab621cc9106f059514bbc6b2d9f632cd83396c7633272340084552e831ee12ed8fa14058fb3f0cf",
    ),
    (
        "sign.sha256.r",
        "76d60de88a1aa6623c77d9511945394452f28defd425d69c7a558ded73fe51bf475c52d4c0db53019a652632215cf6e3",
    ),
    (
        "sign.sha256.s",
        "8126e4c8be5a9a231b732636f1a88102cba506771af64a6b9162de2dc53a9e92b82df1405184b81df0a7c098695f9f8d",
    ),
    (
        "sign.sha384.x",
        "cfcae25b2c56f485ffa8e115539510452a6d8e544cf6c46638826233a833ec216c6b2914c9a23d14c79c0b5cbca76453",
    ),
    (
        "sign.sha384.y",
        "dd6e3ef6a2976388e2443dae86656029ee570f745bc0b99354fd9b3172d11424bae611a1c22dda6a4801685a8035ce26",
    ),
    (
        "sign.sha384.r",
        "5ae4e0839f6dd6fc5ab3ac42c34547e74f400ae317db80fd07d2023ac0aa74f62bf55457e61aa43f2197989b91f03109",
    ),
    (
        "sign.sha384.s",
        "452e2b2addeabbe0a6b4b4f448a45a31480b760512baefc4e9ac7ed314a19a42cbe71fd82f063c465171549c89bc604c",
    ),
    (
        "sign.sha512.x",
        "3fcd907ac1ca41207910bb26881c38053b55de7c873dbf9c0a8ca2dc4c7422b16e2ac179dc7247c2bfd4c6f89aa7fe99",
    ),
    (
        "sign.sha512.y",
        "2fc31aab37a78fcacbb8f820eaaa56f59cd8cdc365b08f4619c18b52641a0ba3c4d0aa21548becdee18eb4adc3d90871",
    ),
    (
        "sign.sha512.r",
        "c97488d3e7ae1fd2c43c7b4fba8d4f5b85d2154a00f7d0e4dfc19fc38de79956705fd82ea4920a39bd0f6817ef5b15ba",
    ),
    (
        "sign.sha512.s",
        "e6bc784da5ce55926effb3c08f27e470d3b11cb394752cf66e9ccd18130a896b5421aea9a0f1fb72c2ae39ead4a5efea",
    ),
    (
        "kdfe.sha384.px",
        "97fc294e6a4a0796d553ab271b57a5bae22258830986798031a074a60211410a000df4d256bff7005cf8b0fae388b338",
    ),
    (
        "kdfe.sha384.py",
        "85a3742ecbc65cfa6b113eea529fda92c84f342c1e4d876c493764d75f93f60966a40c0ae78b10ca786ff5ec33d2e9c7",
    ),
    (
        "kdfe.sha384.d",
        "879894bb952aef26083f89f68c53ad1c4fdb1ed722c87ddf5faf76153e6e498cb7da660a249870e78220fdf4e2743bf9",
    ),
    (
        "kdfe.sha384.ex",
        "dbed976a1f24674d963369f7b3b1a8798af64d867697ad2a9b5842dabee49bdb11f988312c5aad20bfba005dd8bb73f4",
    ),
    (
        "kdfe.sha384.ey",
        "f55e1aeff104ddca382adb2c8b882cb7dac96717b114932d5321ed40547ee90142c4551ce0eb7e6d8de044bf6fb22068",
    ),
    (
        "kdfe.sha384.blob",
        "0030ce63aba669cc55228051160e4f4508f5ada01f53a0a0bb563d5ae3365976f594f77cdedb797256bb01069329a4b3dd26f3246cdeab05b9c09ddf21cdfc3c3f715091356bd38d47fe207c787740705b70819f",
    ),
    (
        "kdfe.sha256.px",
        "9bab2a4298d1f49f45fef765f08558d30437bb6119c845f6b767a1dc8503e42aae27e4b070a8e7708d265452b2862471",
    ),
    (
        "kdfe.sha256.py",
        "5791f7e01cdc5cf0cea442f6862d649a98c8a2c8743c6eb00e677f803cc15f40010b24f5f330f018ae0c5c66973dcb38",
    ),
    (
        "kdfe.sha256.d",
        "17005884fa150ce5872b8d20eaa8d19132e004991105b517c866ce242c36419a3dac5b7de41602aabeb4286c2c34f961",
    ),
    (
        "kdfe.sha256.ex",
        "3f4b84336a4e276da956f85721479297003b598cb7964b83480d1b4768c0d8a7115aee5923c490ca53b3afbb26727fd6",
    ),
    (
        "kdfe.sha256.ey",
        "a94fcb09715753c638cefc6739d8801226ecdf5de2ea1613d853ea7b0413dea295424418c879db5d60a00d5383dc677b",
    ),
    (
        "kdfe.sha256.blob",
        "0020d0b5e83dd6ac8c548ec6a6aa6a3284beff6ba59d3b433e28fcd21bae59610b737502be8d820d4f416a7d8da17ffdb3490aec2a4793eee93067bae171f29d0af71298",
    ),
    (
        "kdfe.sha384.seed",
        "613790e3d17da2108bc29688cd502daccef2181e3f7b2cdd497a2b5849e5385fdde378f93db7886dbe3f9bed7324c3b0",
    ),
    (
        "kdfe.sha256.seed",
        "24523089a4142692f27df3b37b739587b74a0a065f9c6e0aaf7d7088225b3e7b",
    ),
];

/// Compares a TPM output with its pinned value; a mismatch prints the new value first.
fn pinned(name: &str, actual: &[u8]) {
    let actual = hex_of(actual);
    if std::env::var_os("P384_COLLECT").is_some() {
        eprintln!("FIXTURE {name} {actual}");
        return;
    }
    let expected = PINNED
        .iter()
        .find(|(key, _)| *key == name)
        .map_or("", |(_, value)| *value);
    if actual != expected {
        eprintln!("FIXTURE {name} {actual}");
    }
    assert_eq!(actual, expected, "{name}");
}

const MESSAGE: &[u8] = b"Open Desktop ECC P-384";
const HASHES: [(HashAlg, &str); 3] = [
    (HashAlg::Sha256, "sha256"),
    (HashAlg::Sha384, "sha384"),
    (HashAlg::Sha512, "sha512"),
];
const NULL_VALIDATION: [u8; 8] = [0x80, 0x24, 0x40, 0, 0, 7, 0, 0];

fn template(name_alg: u16, attributes: u32, symmetric: SymDef, scheme: Scheme) -> TpmtPublic {
    let mut public = ecc(attributes, symmetric, scheme);
    public.name_alg = name_alg;
    if let PublicParams::Ecc { curve: id, .. } = &mut public.params {
        *id = curve::NIST_P384;
    }
    public
}

fn ecdsa(hash_alg: HashAlg) -> Scheme {
    Scheme {
        alg: alg::ECDSA,
        hash: hash_alg.alg(),
    }
}

fn signing(name_alg: u16, hash_alg: HashAlg) -> TpmtPublic {
    template(name_alg, base() | oa::SIGN, SymDef::Null, ecdsa(hash_alg))
}

fn ecdh_key(name_alg: u16) -> TpmtPublic {
    template(name_alg, base() | oa::DECRYPT, SymDef::Null, Scheme::NULL)
}

fn storage(name_alg: u16) -> TpmtPublic {
    template(
        name_alg,
        base() | oa::RESTRICTED | oa::DECRYPT,
        SymDef::Aes128Cfb,
        Scheme::NULL,
    )
}

/// The point `(x, y)` of a `TPMT_PUBLIC` of an ECC key.
fn point_of(public: &[u8]) -> (Vec<u8>, Vec<u8>) {
    match unmarshal_public(public).unique {
        Unique::Ecc { x, y } => (x, y),
        _ => panic!("not an ECC key"),
    }
}

/// `(handle, public, name)` of a successful `CreatePrimary`; the shared parser stops at 64-octet Names.
fn created(response: &[u8]) -> (u32, Vec<u8>, Vec<u8>) {
    ok(response);
    let handle = u32::from_be_bytes([response[10], response[11], response[12], response[13]]);
    let mut reader = Reader::new(params(response, 1));
    let public = reader.tpm2b(1024).unwrap().to_vec();
    let _creation_data = reader.tpm2b(1024).unwrap();
    let _creation_hash = reader.tpm2b(64).unwrap();
    let (_tag, _hierarchy) = (reader.u16().unwrap(), reader.u32().unwrap());
    let _digest = reader.tpm2b(64).unwrap();
    let name = reader.tpm2b(66).unwrap().to_vec();
    reader.finish().unwrap();
    (handle, public, name)
}

fn handle_of(response: &[u8]) -> u32 {
    ok(response);
    u32::from_be_bytes([response[10], response[11], response[12], response[13]])
}

fn scheme_bytes(hash_alg: HashAlg) -> Vec<u8> {
    cat(&[&u16b(alg::ECDSA), &u16b(hash_alg.alg())])
}

/// `(hash, r, s)` of a successful ECDSA `TPMT_SIGNATURE`.
fn parse_ecdsa(signature: &[u8]) -> (u16, Vec<u8>, Vec<u8>) {
    let mut reader = Reader::new(signature);
    assert_eq!(reader.u16().unwrap(), alg::ECDSA);
    let hash_alg = reader.u16().unwrap();
    let r = reader.tpm2b(48).unwrap().to_vec();
    let s = reader.tpm2b(48).unwrap().to_vec();
    reader.finish().unwrap();
    (hash_alg, r, s)
}

fn ecdsa_bytes(hash_alg: HashAlg, r: &[u8], s: &[u8]) -> Vec<u8> {
    cat(&[
        &u16b(alg::ECDSA),
        &u16b(hash_alg.alg()),
        &tpm2b(r),
        &tpm2b(s),
    ])
}

fn sign_digest(t: &mut Tpm, key: u32, pw: &[u8], digest: &[u8]) -> Vec<u8> {
    let params = cat(&[&tpm2b(b""), &tpm2b(digest), &NULL_VALIDATION]);
    run(t, with_pw(cc::SIGN_DIGEST, &[key], &[pw], &params))
}

fn verify_digest(t: &mut Tpm, key: u32, digest: &[u8], signature: &[u8]) -> Vec<u8> {
    let params = cat(&[&tpm2b(b""), &tpm2b(digest), signature]);
    run(t, plain(cc::VERIFY_DIGEST_SIGNATURE, &[key], &params))
}

fn zgen(t: &mut Tpm, key: u32, pw: &[u8], x: &[u8], y: &[u8]) -> Vec<u8> {
    run(t, with_pw(cc::ECDH_Z_GEN, &[key], &[pw], &ecc_point(x, y)))
}

fn read_point(reader: &mut Reader<'_>) -> (Vec<u8>, Vec<u8>) {
    let n = usize::from(reader.u16().unwrap());
    let mut point = reader.sub(n).unwrap();
    let x = point.tpm2b(48).unwrap().to_vec();
    let y = point.tpm2b(48).unwrap().to_vec();
    point.finish().unwrap();
    (x, y)
}

/// `(x, y)` of the `TPM2B_ECC_POINT` of a successful `ECDH_ZGen`.
fn out_point(response: &[u8]) -> (Vec<u8>, Vec<u8>) {
    ok(response);
    let mut reader = Reader::new(params(response, 0));
    let point = read_point(&mut reader);
    reader.finish().unwrap();
    point
}

/// The value pinned under `name`; `None` while the pins are being collected.
fn pin(name: &str) -> Option<Vec<u8>> {
    PINNED
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, value)| hex(value))
}

// A key made by `openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-384`, its signatures of
// MESSAGE_EXT made by `openssl dgst -shaN -sign`, and a second key (the ECDH peer).
const MESSAGE_EXT: &[u8] = b"Open Desktop ECC P-384 verify";
const EXT_X: &str = "83bf818f1e838586bc17b3d7723fe1592dd09941f2eb5998b6a02e08404ef19b0fdc0b98071925e60a4f471fd89096f1";
const EXT_Y: &str = "449c4bdf16e69097fc0f40c9bf009d0fb4e680388ef2e414f9dcc2aaaef526263874043eea0932e4b01a8a6539fec46f";
const EXT_SIGNATURES: [(&str, &str, &str); 3] = [
    (
        "62c149b5873f08f787c4a8708502572a449bde1ceb189881ada12012c015fa42",
        "64f64ff361ee7a6d168fcb396032a89d3291c1d42e360dbada42708e75132a3bd3f3fb85e90f25840125b3142e578322",
        "332524494be0e33883454b9328fe9217f7a02700b942ce2ca9344539c6b50e52a0d0798aadc3f40efa7d4306ce5bcc2c",
    ),
    (
        "4914dfd9604eb4f292ccea6c9f5fa90d197d4e55337877e9593c8ce331c5924ce362295b1e58f7ed8f30a4481bb027c5",
        "215c41992c17c0ffd9dd848c0e4b0fc2e39054577d7986ed67751d79172eb7c6de99b969738ca5e66d785b1f5bdf9d87",
        "b5b22039af6391adda2f8c7f6efece18578e97afce436851ab1c7e1fdc36597d74761ef6067e1a20a109972246e00140",
    ),
    (
        "dbcf32447d3f6ef8ef421a652dc9822ad19e66cb38b7af8a8194617574127ea9631e93963f4bb50aff36bec78d90ed31843be2bde94c13d1e94266a5f07e91c1",
        "481cada866f47762e66f02bdf27219b65a6abf5afe49ab03535d7117d59341d6da2fd448f0e3f0d5e31951a633870875",
        "233512cbf45cb4fac1f2de76231964f1582635fe86547b0ab443e1f037ee9b92730e1cd2d7b9a709b4cd6fbd36c920d4",
    ),
];
// `openssl dgst -sha384 -sign` of the message "short-r 53": r has a leading zero octet.
const SHORT_R_MESSAGE: &[u8] = b"short-r 53";
const SHORT_R: (&str, &str) = (
    "00e4662d0fac25e58ab8dcb0511fd1313419ffeaec40ff107be199030b3bb227f189f0f52f89454a2cfeb71f29cd6685",
    "3bc93d58de6d937b4e8bdbc8f18d9a0c59b562a321a9092e2e7c0ff050e7d74d90cebc8b2c1e84ab6a139460a57d2811",
);
const PEER_X: &str = "8503cee3d35a3af0dc02217b8a76d7bfbd98ed2e5218148e608be567c4bf3751ab9fb67a0942eab57535b075bc4d967f";
const PEER_Y: &str = "c3455b3e80e721266bd2ccea15abce95a53a790a94886035136d5cec35500356c484e0ad87f06c2b81ee5b908cb04d10";

// ------------------------------------------------------------------------------ CreatePrimary

#[test]
fn p384_create_primary_public_and_name_matches_python() {
    let mut t = started();
    for (hash_alg, label) in HASHES {
        let name_alg = hash_alg.alg();
        let (handle, public, name) = created(&create_primary(
            &mut t,
            OWNER,
            b"pw",
            &signing(name_alg, hash_alg),
        ));
        let (x, y) = point_of(&public);
        assert_eq!((x.len(), y.len()), (48, 48));
        // Python checked y^2 = x^3 - 3x + b (mod p) with the parameters of FIPS 186-4 D.1.2.4.
        pinned(&format!("create.{label}.x"), &x);
        pinned(&format!("create.{label}.y"), &y);
        // The `TPMT_PUBLIC` by hand (Part 2 v184, Tables 216 and 219): type ECC (0x0023), nameAlg,
        // objectAttributes fixedTPM | fixedParent | sensitiveDataOrigin | userWithAuth | sign
        // (0x00040072), empty authPolicy, symmetric NULL (0x0010), scheme ECDSA (0x0018) with the
        // hash, curve NIST_P384 (0x0004), kdf NULL (0x0010), then x and y.
        let by_hand = cat(&[
            &[0x00, 0x23],
            &name_alg.to_be_bytes(),
            &[0x00, 0x04, 0x00, 0x72],
            &[0x00, 0x00],
            &[0x00, 0x10],
            &[0x00, 0x18],
            &name_alg.to_be_bytes(),
            &[0x00, 0x04],
            &[0x00, 0x10],
            &tpm2b(&x),
            &tpm2b(&y),
        ]);
        assert_eq!(public, by_hand);
        // The Name is nameAlg || H(TPMT_PUBLIC), computed with Python hashlib.
        assert_eq!(&name[..2], &name_alg.to_be_bytes());
        assert_eq!(name.len(), 2 + hash_alg.size());
        pinned(&format!("create.{label}.name"), &name);
        // ReadPublic gives the same area and Name.
        let response = read_public(&mut t, handle);
        let mut reader = Reader::new(params(&response, 0));
        assert_eq!(reader.tpm2b(1024).unwrap(), &by_hand[..]);
        assert_eq!(reader.tpm2b(66).unwrap(), &name[..]);
        // The same template gives the same primary key (Part 1, clause 24).
        ok(&flush(&mut t, handle));
        let (again, again_public, again_name) = created(&create_primary(
            &mut t,
            OWNER,
            b"pw",
            &signing(name_alg, hash_alg),
        ));
        assert_eq!(again_public, public);
        assert_eq!(again_name, name);
        ok(&flush(&mut t, again));
    }
}

// ------------------------------------------------------------------------------- Sign

#[test]
fn p384_ecdsa_sign_digest_verifies_with_openssl() {
    let mut t = started();
    for ((hash_alg, label), digest_size) in HASHES.into_iter().zip([32, 48, 64]) {
        let key = parse_primary(&create_primary(
            &mut t,
            OWNER,
            b"pw",
            &signing(alg::SHA256, hash_alg),
        ));
        let (x, y) = point_of(&key.public);
        pinned(&format!("sign.{label}.x"), &x);
        pinned(&format!("sign.{label}.y"), &y);
        let digest = digest_of(hash_alg, MESSAGE);
        assert_eq!(digest.len(), digest_size);
        let signed = sign(&mut t, key.handle, b"pw", &digest, &scheme_bytes(hash_alg));
        ok(&signed);
        let (sig_hash, r, s) = parse_ecdsa(params(&signed, 0));
        assert_eq!(sig_hash, hash_alg.alg());
        assert_eq!((r.len(), s.len()), (48, 48));
        // Pinned: `openssl dgst -shaN -verify` accepted (r, s) as the DER signature of MESSAGE with
        // the public key rebuilt from the pinned point; for SHA-512 the 64-octet digest is cut to its
        // leftmost 48 octets (FIPS 186-4, clause 6.4), as the `ecdsa` crate and OpenSSL both do.
        pinned(&format!("sign.{label}.r"), &r);
        pinned(&format!("sign.{label}.s"), &s);
        // The three verification commands accept it, a changed bit is TPM_RC_SIGNATURE.
        let wire = params(&signed, 0).to_vec();
        ok(&verify(&mut t, key.handle, &digest, &wire));
        ok(&verify_digest(&mut t, key.handle, &digest, &wire));
        let mut bad = s.clone();
        bad[20] ^= 1;
        let bad = ecdsa_bytes(hash_alg, &r, &bad);
        err(&verify(&mut t, key.handle, &digest, &bad), Rc::SIGNATURE);
        err(
            &verify_digest(&mut t, key.handle, &digest, &bad),
            Rc::SIGNATURE,
        );
        let mut other = digest.clone();
        other[0] ^= 1;
        err(&verify(&mut t, key.handle, &other, &wire), Rc::SIGNATURE);
        // SignDigest gives the same bytes (the nonce follows RFC 6979, so it is deterministic).
        let again = sign_digest(&mut t, key.handle, b"pw", &digest);
        assert_eq!(params(&again, 0), &wire[..]);
        // A digest of another hash than the scheme names is TPM_RC_SIZE.
        let other_size = digest_of(
            if hash_alg == HashAlg::Sha256 {
                HashAlg::Sha384
            } else {
                HashAlg::Sha256
            },
            MESSAGE,
        );
        err(
            &sign(
                &mut t,
                key.handle,
                b"pw",
                &other_size,
                &scheme_bytes(hash_alg),
            ),
            Rc::SIZE.in_param(1),
        );
        ok(&flush(&mut t, key.handle));
    }
}

#[test]
fn p384_verify_signature_accepts_openssl_signature() {
    let mut t = started();
    let (x, y) = (hex(EXT_X), hex(EXT_Y));
    let load_key = |t: &mut Tpm, hash_alg: HashAlg| {
        let mut public = signing(alg::SHA256, hash_alg);
        public.attributes = oa::SIGN;
        public.unique = Unique::Ecc {
            x: x.clone(),
            y: y.clone(),
        };
        parse_loaded(&load_external(t, &public, super::tests_obj::NULL)).0
    };
    for ((hash_alg, _), (digest, r, s)) in HASHES.into_iter().zip(EXT_SIGNATURES) {
        let key = load_key(&mut t, hash_alg);
        let digest = hex(digest);
        assert_eq!(digest, digest_of(hash_alg, MESSAGE_EXT));
        let (r, s) = (hex(r), hex(s));
        let wire = ecdsa_bytes(hash_alg, &r, &s);
        ok(&verify(&mut t, key, &digest, &wire));
        ok(&verify_digest(&mut t, key, &digest, &wire));
        // A signature of another message, of another key or with a changed value is refused.
        let mut other = digest.clone();
        other[47 % digest.len()] ^= 0x80;
        err(&verify(&mut t, key, &other, &wire), Rc::SIGNATURE);
        let mut bad = r.clone();
        bad[0] ^= 1;
        err(
            &verify(&mut t, key, &digest, &ecdsa_bytes(hash_alg, &bad, &s)),
            Rc::SIGNATURE,
        );
        err(
            &verify(&mut t, key, &digest, &ecdsa_bytes(hash_alg, &s, &r)),
            Rc::SIGNATURE,
        );
        err(
            &verify(&mut t, key, &digest, &ecdsa_bytes(hash_alg, &[0], &s)),
            Rc::SIGNATURE,
        );
        // The digest has the width of the hash the signature is labelled with (TPM_RC_SIZE), and a
        // signature of another hash does not verify the digest of that hash.
        let wrong = if hash_alg == HashAlg::Sha256 {
            HashAlg::Sha384
        } else {
            HashAlg::Sha256
        };
        err(
            &verify(&mut t, key, &digest, &ecdsa_bytes(wrong, &r, &s)),
            Rc::SIZE.in_param(1),
        );
        err(
            &verify(
                &mut t,
                key,
                &digest_of(wrong, MESSAGE_EXT),
                &ecdsa_bytes(wrong, &r, &s),
            ),
            Rc::SIGNATURE,
        );
        // A value longer than 48 octets does not fit the curve.
        let mut long = vec![0u8];
        long.extend_from_slice(&r);
        err(
            &verify(&mut t, key, &digest, &ecdsa_bytes(hash_alg, &long, &s)),
            Rc::SIZE.in_param(2),
        );
        ok(&flush(&mut t, key));
    }
    // OpenSSL wrote r without its leading zero octet (DER integers): the TPM2B may be shorter.
    let key = load_key(&mut t, HashAlg::Sha384);
    let digest = digest_of(HashAlg::Sha384, SHORT_R_MESSAGE);
    let (r, s) = (hex(SHORT_R.0), hex(SHORT_R.1));
    assert_eq!(r[0], 0);
    ok(&verify(
        &mut t,
        key,
        &digest,
        &ecdsa_bytes(HashAlg::Sha384, &r, &s),
    ));
    ok(&verify(
        &mut t,
        key,
        &digest,
        &ecdsa_bytes(HashAlg::Sha384, &r[1..], &s),
    ));
}

// -------------------------------------------------------------------------------- ECDH, KDFe

#[test]
fn p384_ecdh_zgen_matches_openssl_peer() {
    let mut t = started();
    let key = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"pw",
        &ecdh_key(alg::SHA384),
    ));
    let (qx, qy) = point_of(&key.public);
    pinned("zgen.tpm.x", &qx);
    pinned("zgen.tpm.y", &qy);
    let (peer_x, peer_y) = (hex(PEER_X), hex(PEER_Y));
    // Z = [d_tpm] Q_peer. Python computed [d_peer] Q_tpm with the private key of the OpenSSL peer
    // and the pinned point of the TPM key; `openssl pkeyutl -derive` gave the same x-coordinate.
    let (zx, zy) = out_point(&zgen(&mut t, key.handle, b"pw", &peer_x, &peer_y));
    assert_eq!((zx.len(), zy.len()), (48, 48));
    pinned("zgen.z.x", &zx);
    pinned("zgen.z.y", &zy);
    // Coordinates without their leading zero octets are padded.
    // A point that is not on the curve, one of P-256 size and one that is too long are refused.
    let mut bad_y = peer_y.clone();
    bad_y[47] ^= 1;
    err(
        &zgen(&mut t, key.handle, b"pw", &peer_x, &bad_y),
        Rc::ECC_POINT.in_param(1),
    );
    err(
        &zgen(&mut t, key.handle, b"pw", &[1u8; 32], &[2u8; 32]),
        Rc::ECC_POINT.in_param(1),
    );
    err(
        &zgen(&mut t, key.handle, b"pw", &[0u8; 49], &peer_y),
        Rc::SIZE.in_param(1),
    );
    err(
        &zgen(&mut t, key.handle, b"pw", &[0u8; 48], &[0u8; 48]),
        Rc::ECC_POINT.in_param(1),
    );
    err(
        &zgen(&mut t, key.handle, b"wrong", &peer_x, &peer_y),
        Rc::AUTH_FAIL.in_session(1),
    );
}

/// `TPM2_MakeCredential` under a P-384 storage parent: the seed is `KDFe(nameAlg of the parent, Z,
/// "IDENTITY", ephemeral x, parent x)` (Part 1, clause 24.5), the integrity value an HMAC over the
/// encrypted credential and the Name with the key `KDFa(nameAlg, seed, "INTEGRITY")`.
#[test]
fn p384_kdfe_uses_the_name_algorithm_of_the_parent() {
    let mut t = started();
    let activated = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"object",
        &signing(alg::SHA256, HashAlg::Sha256),
    ));
    for (parent_hash, label) in [(HashAlg::Sha384, "sha384"), (HashAlg::Sha256, "sha256")] {
        let parent = parse_primary(&create_primary(
            &mut t,
            OWNER,
            b"key",
            &storage(parent_hash.alg()),
        ));
        let (px, py) = point_of(&parent.public);
        pinned(&format!("kdfe.{label}.px"), &px);
        pinned(&format!("kdfe.{label}.py"), &py);
        // The private scalar of the primary key, for the Python recomputation of Z.
        pinned(&format!("kdfe.{label}.d"), &secret_of(&t, parent.handle));
        let credential = [0x3c; 32];
        let response = run(
            &mut t,
            plain(
                cc::MAKE_CREDENTIAL,
                &[parent.handle],
                &cat(&[&tpm2b(&credential), &tpm2b(&activated.name)]),
            ),
        );
        ok(&response);
        let mut outer = Reader::new(params(&response, 0));
        let blob = outer.tpm2b(4 + 2 * 48).unwrap().to_vec();
        let secret = outer.tpm2b(2 + 2 * (2 + 48)).unwrap().to_vec();
        outer.finish().unwrap();
        // The encrypted secret is the ephemeral point (TPMS_ECC_POINT) of the curve of the parent.
        let mut point = Reader::new(&secret);
        let ex = point.tpm2b(48).unwrap().to_vec();
        let ey = point.tpm2b(48).unwrap().to_vec();
        point.finish().unwrap();
        assert_eq!((ex.len(), ey.len()), (48, 48));
        pinned(&format!("kdfe.{label}.ex"), &ex);
        pinned(&format!("kdfe.{label}.ey"), &ey);
        // The blob is the integrity value of the size of the digest, then the encrypted credential.
        let size = parent_hash.size();
        assert_eq!(&blob[..2], &(size as u16).to_be_bytes());
        assert_eq!(blob.len(), 2 + size + 2 + credential.len());
        pinned(&format!("kdfe.{label}.blob"), &blob);
        // Python computed seed = H(00000001 || Z.x || "IDENTITY\0" || ex || px) with H the nameAlg of
        // the parent (one block: the seed is one digest) from Z = [d] E.
        let seed = pin(&format!("kdfe.{label}.seed"));
        if let Some(seed) = seed {
            assert_eq!(seed.len(), size);
            let integrity_key = kdfa(
                parent_hash,
                &seed,
                b"INTEGRITY",
                &[],
                &[],
                (size * 8) as u32,
            );
            let encrypted = &blob[2 + size..];
            let expected = hmac(parent_hash, &integrity_key, &[encrypted, &activated.name]);
            assert_eq!(&blob[2..2 + size], expected.as_slice());
        }
        // ActivateCredential recovers the credential.
        let activate = run(
            &mut t,
            with_pw(
                cc::ACTIVATE_CREDENTIAL,
                &[activated.handle, parent.handle],
                &[b"object", b"key"],
                params(&response, 0),
            ),
        );
        ok(&activate);
        assert_eq!(params(&activate, 0), &tpm2b(&credential)[..]);
        ok(&flush(&mut t, parent.handle));
    }
}

#[test]
fn p384_ecdh_key_gen_returns_ephemeral_points_of_the_curve() {
    let mut t = started();
    let key = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"pw",
        &ecdh_key(alg::SHA256),
    ));
    // A public-only copy of the key: ECDH_KeyGen needs no private part.
    let mut public = unmarshal_public(&key.public);
    public.attributes = oa::SIGN;
    public.params = match public.params {
        PublicParams::Ecc {
            symmetric,
            curve: c,
            ..
        } => PublicParams::Ecc {
            symmetric,
            scheme: ecdsa(HashAlg::Sha256),
            curve: c,
        },
        other => other,
    };
    let (external, _) = parse_loaded(&load_external(&mut t, &public, super::tests_obj::NULL));
    let before = t.persistent_snapshot();
    let mut previous = None;
    for _ in 0..2 {
        let response = run(&mut t, plain(cc::ECDH_KEY_GEN, &[external], &[]));
        ok(&response);
        let mut reader = Reader::new(params(&response, 0));
        let shared = read_point(&mut reader);
        let (qx, qy) = read_point(&mut reader);
        reader.finish().unwrap();
        assert_eq!((qx.len(), qy.len()), (48, 48));
        // The ephemeral point is on the curve and the private key recovers the same Z.
        assert!(crate::crypto::ecc_point_valid(curve::NIST_P384, &qx, &qy));
        let recovered = out_point(&zgen(&mut t, key.handle, b"pw", &qx, &qy));
        assert_eq!(shared, recovered);
        assert_ne!(previous, Some((qx.clone(), qy.clone())));
        previous = Some((qx, qy));
    }
    assert_eq!(t.persistent_snapshot(), before);
}

#[test]
fn p384_ecc_parameters_and_test_parms_report_the_curve() {
    let mut t = started();
    let response = run(
        &mut t,
        plain(cc::ECC_PARAMETERS, &[], &u16b(curve::NIST_P384)),
    );
    ok(&response);
    let mut reader = Reader::new(params(&response, 0));
    // curveID, keySize, kdf (KDF1_SP800_56A, SHA-384), sign (NULL).
    for value in [0x0004, 384, 0x0020, 0x000c, 0x0010] {
        assert_eq!(reader.u16().unwrap(), value);
    }
    // p, a, b, gX, gY, n from NIST SP 800-186 (`openssl ecparam -name secp384r1 -param_enc explicit
    // -text`), then the cofactor.
    for expected in [
        "fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffeffffffff0000000000000000ffffffff",
        "fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffeffffffff0000000000000000fffffffc",
        "b3312fa7e23ee7e4988e056be3f82d19181d9c6efe8141120314088f5013875ac656398d8a2ed19d2a85c8edd3ec2aef",
        "aa87ca22be8b05378eb1c71ef320ad746e1d3b628ba79b9859f741e082542a385502f25dbf55296c3a545e3872760ab7",
        "3617de4a96262c6f5d9e98bf9292dc29f8f41dbd289a147ce9da3113b5f0b8c00a60b1ce1d7e819d7a431d7c90ea0e5f",
        "ffffffffffffffffffffffffffffffffffffffffffffffffc7634d81f4372ddf581a0db248b0a77aecec196accc52973",
        "01",
    ] {
        assert_eq!(reader.tpm2b(48).unwrap(), hex(expected));
    }
    reader.finish().unwrap();
    // The generator is the public point of the scalar 1 (the crate multiplies, it does not read the table).
    let one = crate::crypto::ecc_from_scalar(curve::NIST_P384, &{
        let mut d = [0u8; 48];
        d[47] = 1;
        d
    })
    .unwrap();
    let (gx, gy) = crate::crypto::ecc_public(&one).unwrap();
    assert_eq!(
        hex_of(&gx),
        "aa87ca22be8b05378eb1c71ef320ad746e1d3b628ba79b9859f741e082542a385502f25dbf55296c3a545e3872760ab7"
    );
    assert_eq!(
        hex_of(&gy),
        "3617de4a96262c6f5d9e98bf9292dc29f8f41dbd289a147ce9da3113b5f0b8c00a60b1ce1d7e819d7a431d7c90ea0e5f"
    );
    // TestParms: P-384 with the three ECDSA hashes, with ECDH, with a NULL scheme.
    for encoded in [
        "002300100018000b00040010",
        "002300100018000c00040010",
        "002300100018000d00040010",
        "002300100019000c00040010",
        "00230010001000040010",
        "0023000600800043001000040010",
    ] {
        ok(&run(&mut t, plain(cc::TEST_PARMS, &[], &hex(encoded))));
    }
    // ECDH names SHA-256 on P-256, as before; on P-384 any of the three hashes.
    err(
        &run(
            &mut t,
            plain(cc::TEST_PARMS, &[], &hex("002300100019000c00030010")),
        ),
        Rc::HASH.in_param(1),
    );
    ok(&run(
        &mut t,
        plain(cc::TEST_PARMS, &[], &hex("002300100019000b00030010")),
    ));
}

// ----------------------------------------------------------- storage parents, contexts, snapshots

#[test]
fn p384_storage_parent_protects_child_and_snapshot_round_trip() {
    let mut t = started();
    let parent = handle_of(&create_primary(
        &mut t,
        OWNER,
        b"ppw",
        &storage(alg::SHA384),
    ));
    // A P-384 signing child: Create, Load, Sign, Verify.
    let child = parse_child(&create_child(
        &mut t,
        parent,
        b"ppw",
        b"kpw",
        b"",
        &signing(alg::SHA256, HashAlg::Sha384),
    ));
    assert!(child.private.len() <= crate::object::MAX_PRIVATE);
    let (x, y) = point_of(&child.public);
    assert_eq!((x.len(), y.len()), (48, 48));
    let key = handle_of(&load(&mut t, parent, b"ppw", &child.private, &child.public));
    // The scalar of the child is in the private area only under the protection of the parent.
    let scalar = secret_of(&t, key);
    assert_eq!(scalar.len(), 48);
    assert!(!super::tests_obj::contains(&child.private, &scalar));
    assert!(!super::tests_obj::contains(&child.public, &scalar));
    let digest = digest_of(HashAlg::Sha384, MESSAGE);
    let signed = sign(&mut t, key, b"kpw", &digest, &scheme_bytes(HashAlg::Sha384));
    ok(&signed);
    let wire = params(&signed, 0).to_vec();
    ok(&verify(&mut t, key, &digest, &wire));
    err(
        &sign(
            &mut t,
            key,
            b"wrong",
            &digest,
            &scheme_bytes(HashAlg::Sha384),
        ),
        Rc::AUTH_FAIL.in_session(1),
    );
    // Integrity: a changed octet of the private area, another parent, another public area.
    for i in [0usize, 3, 40, child.private.len() - 1] {
        let mut bad = child.private.clone();
        bad[i] ^= 1;
        let r = load(&mut t, parent, b"ppw", &bad, &child.public);
        assert!(
            matches!(rc(&r), x if x == Rc::INTEGRITY.0 || x == Rc::SIZE.0),
            "octet {i}: {:#x}",
            rc(&r)
        );
    }
    let other_parent = handle_of(&create_primary(
        &mut t,
        OWNER,
        b"opw",
        &storage(alg::SHA256),
    ));
    // (a SHA-256 parent expects an integrity value of 32 octets, the area has one of 48).
    let r = load(&mut t, other_parent, b"opw", &child.private, &child.public);
    assert!(matches!(rc(&r), x if x == Rc::INTEGRITY.0 || x == Rc::SIZE.0));
    // Parents and children of the two curves mix: a P-256 child under the P-384 parent and a
    // P-384 child under a P-256 parent.
    let p256_child = parse_child(&create_child(
        &mut t,
        parent,
        b"ppw",
        b"cpw",
        b"",
        &ecc_sign(),
    ));
    let p256_key = handle_of(&load(
        &mut t,
        parent,
        b"ppw",
        &p256_child.private,
        &p256_child.public,
    ));
    let p256_digest = digest_of(HashAlg::Sha256, MESSAGE);
    let p256_signed = sign(
        &mut t,
        p256_key,
        b"cpw",
        &p256_digest,
        &scheme_bytes(HashAlg::Sha256),
    );
    ok(&p256_signed);
    assert_eq!(parse_ecdsa(params(&p256_signed, 0)).1.len(), 32);
    ok(&verify(
        &mut t,
        p256_key,
        &p256_digest,
        params(&p256_signed, 0),
    ));
    let p256_parent = handle_of(&create_primary(&mut t, OWNER, b"qpw", &ecc_storage()));
    let mixed = parse_child(&create_child(
        &mut t,
        p256_parent,
        b"qpw",
        b"mpw",
        b"",
        &signing(alg::SHA256, HashAlg::Sha256),
    ));
    let mixed_key = handle_of(&load(
        &mut t,
        p256_parent,
        b"qpw",
        &mixed.private,
        &mixed.public,
    ));
    let mixed_signed = sign(
        &mut t,
        mixed_key,
        b"mpw",
        &p256_digest,
        &scheme_bytes(HashAlg::Sha256),
    );
    assert_eq!(parse_ecdsa(params(&mixed_signed, 0)).1.len(), 48);
    ok(&verify(
        &mut t,
        mixed_key,
        &p256_digest,
        params(&mixed_signed, 0),
    ));
    ok(&flush(&mut t, p256_key));
    ok(&flush(&mut t, mixed_key));
    // A context holds the P-384 key: save, flush, load, the same signature.
    let ctx = params(&run(&mut t, plain(cc::CONTEXT_SAVE, &[key], &[])), 0).to_vec();
    ok(&flush(&mut t, key));
    let loaded = run(&mut t, plain(cc::CONTEXT_LOAD, &[], &ctx));
    ok(&loaded);
    let key = u32::from_be_bytes([loaded[10], loaded[11], loaded[12], loaded[13]]);
    let again = sign(&mut t, key, b"kpw", &digest, &scheme_bytes(HashAlg::Sha384));
    assert_eq!(params(&again, 0), &wire[..]);
    assert_eq!(secret_of(&t, key), scalar);
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
        &scheme_bytes(HashAlg::Sha384),
    );
    assert_eq!(params(&persistent, 0), &wire[..]);
    let response = read_public(&mut restored, 0x8100_0001);
    let mut reader = Reader::new(params(&response, 0));
    assert_eq!(point_of(reader.tpm2b(1024).unwrap()), (x, y));
}

// ---------------------------------------------------------------------------- unsupported curves

#[test]
fn p384_refuses_unsupported_curve() {
    let mut t = started();
    // NIST P-521 (5), NIST P-192 (1), NIST P-224 (2), BN_P256 (0x10) and BN_P638 (0x11).
    for id in [1u16, 2, 5, 0x10, 0x11, 0x20] {
        let mut public = signing(alg::SHA256, HashAlg::Sha256);
        public.params = PublicParams::Ecc {
            symmetric: SymDef::Null,
            scheme: ecdsa(HashAlg::Sha256),
            curve: id,
        };
        let creation =
            super::tests_obj::creation_params(b"pw", b"", &public, b"", &super::tests_obj::NO_PCR);
        let created = super::tests_obj::create_primary_raw(&mut t, OWNER, b"", &creation);
        err(&created, Rc::CURVE.in_param(2));
        let load = load_external(&mut t, &public, super::tests_obj::NULL);
        err(&load, Rc::CURVE.in_param(2));
        err(
            &run(&mut t, plain(cc::ECC_PARAMETERS, &[], &u16b(id))),
            Rc::CURVE.in_param(1),
        );
        let encoded = cat(&[
            &u16b(alg::ECC),
            &u16b(alg::NULL),
            &u16b(alg::NULL),
            &u16b(id),
            &u16b(alg::NULL),
        ]);
        err(
            &run(&mut t, plain(cc::TEST_PARMS, &[], &encoded)),
            Rc::CURVE.in_param(1),
        );
    }
    // A P-384 key whose point has P-256 coordinates, and a P-256 key with P-384 coordinates.
    let mut external = signing(alg::SHA256, HashAlg::Sha256);
    external.attributes = oa::SIGN;
    external.unique = Unique::Ecc {
        x: vec![1; 32],
        y: vec![2; 32],
    };
    err(
        &load_external(&mut t, &external, super::tests_obj::NULL),
        Rc::ECC_POINT.in_param(2),
    );
    let mut p256 = ecc_sign();
    p256.attributes = oa::SIGN;
    p256.unique = Unique::Ecc {
        x: vec![1; 48],
        y: vec![2; 48],
    };
    err(
        &load_external(&mut t, &p256, super::tests_obj::NULL),
        Rc::SIZE.in_param(2),
    );
    // A P-256 key refuses a P-384 point (the coordinate is longer than the curve): TPM_RC_SIZE as before.
    let key = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"pw",
        &ecc(base() | oa::DECRYPT, SymDef::Null, Scheme::NULL),
    ));
    err(
        &zgen(&mut t, key.handle, b"pw", &[1u8; 33], &[2u8; 33]),
        Rc::SIZE.in_param(1),
    );
}

// --------------------------------------------------------------------------------- capabilities

#[test]
fn capability_ecc_curves_and_loaded_curves_include_p384() {
    let mut t = started();
    let (more, c, body, count) = cap_list(&get_cap(&mut t, cap::ECC_CURVES, 0, 10));
    assert!(!more && c == cap::ECC_CURVES);
    assert_eq!(
        (count, body),
        (2, [u16b(curve::NIST_P256), u16b(curve::NIST_P384)].concat())
    );
    // The listing starts at the first curve not below the property and paginates.
    let (_, _, body, count) = cap_list(&get_cap(&mut t, cap::ECC_CURVES, 4, 10));
    assert_eq!((count, body), (1, u16b(curve::NIST_P384)));
    assert_eq!(cap_list(&get_cap(&mut t, cap::ECC_CURVES, 5, 10)).3, 0);
    let (more, _, body, count) = cap_list(&get_cap(&mut t, cap::ECC_CURVES, 0, 1));
    assert!(more);
    assert_eq!((count, body), (1, u16b(curve::NIST_P256)));
    // TPM_PT_LOADED_CURVES counts the curves of the list.
    let (_, _, body, count) = cap_list(&get_cap(&mut t, cap::TPM_PROPERTIES, pt::LOADED_CURVES, 1));
    assert_eq!(count, 1);
    assert_eq!(body, [u32b(pt::LOADED_CURVES), u32b(2)].concat());
}

#[test]
fn p384_self_test_known_answers_pass() {
    // RFC 6979 A.2.6 (SHA-384 "sample"), the ECDH point of the scalar 2 and the public point of the
    // RFC 6979 key (OpenSSL and Python), the signatures of RFC 6979 A.2.6 for SHA-256 and SHA-512 (verified), and sign-verify round trips.
    assert!(crate::self_test::known_answers());
}

#[test]
fn p256_signature_value_limits_are_signature_below_49_octets_and_size_above() {
    // The limit of a signature value is the one of the largest curve (48 octets) for every curve, so
    // a P-256 value of 33 to 48 octets is a signature that does not verify (TPM_RC_SIGNATURE) and one
    // of 49 octets or more is TPM_RC_SIZE. This pins the current limit; the reference may differ.
    let mut t = started();
    let key = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"pw",
        &ecc(base() | oa::SIGN, SymDef::Null, ecdsa(HashAlg::Sha256)),
    ));
    assert_eq!(point_of(&key.public).0.len(), 32);
    let digest = digest_of(HashAlg::Sha256, MESSAGE);
    let wire = |r: usize, s: usize| ecdsa_bytes(HashAlg::Sha256, &vec![1; r], &vec![1; s]);
    for size in [33, 40, 48] {
        err(
            &verify(&mut t, key.handle, &digest, &wire(size, 32)),
            Rc::SIGNATURE,
        );
        err(
            &verify(&mut t, key.handle, &digest, &wire(32, size)),
            Rc::SIGNATURE,
        );
    }
    for size in [49, 64] {
        err(
            &verify(&mut t, key.handle, &digest, &wire(size, 32)),
            Rc::SIZE.in_param(2),
        );
        err(
            &verify(&mut t, key.handle, &digest, &wire(32, size)),
            Rc::SIZE.in_param(2),
        );
    }
}

// ------------------------------------------------------------------------------------ golden

/// Every reply of an ECC P-256, SHA-256 flow (primaries, ECDSA signing and verification, ECDH_ZGen,
/// ECDH_KeyGen, ECC_Parameters, a child key, a context and `TestParms`) as the SHA-256 of its bytes.
/// The DRBG is shared by all commands, so a change in the octets any of them draws changes every
/// later reply.
fn ecc_p256_sha256_flow() -> Vec<String> {
    let mut t = started();
    let mut out = Vec::new();
    let mut note = |reply: Vec<u8>| {
        ok(&reply);
        out.push(hex_of(&digest_of(HashAlg::Sha256, &reply)));
        reply
    };
    let ecdh_key = ecc(base() | oa::DECRYPT, SymDef::Null, Scheme::NULL);
    let signer = parse_primary(&note(create_primary(&mut t, OWNER, b"pw", &ecc_sign())));
    let ecdh = parse_primary(&note(create_primary(&mut t, OWNER, b"pw", &ecdh_key)));
    let storage = parse_primary(&note(create_primary(&mut t, OWNER, b"pw", &ecc_storage())));
    let digest = digest_of(HashAlg::Sha256, b"golden p256");
    let scheme = cat(&[&u16b(alg::ECDSA), &u16b(alg::SHA256)]);
    let signed = note(sign(&mut t, signer.handle, b"pw", &digest, &scheme));
    note(verify(&mut t, signer.handle, &digest, params(&signed, 0)));
    let gx = hex("6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296");
    let gy = hex("4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5");
    note(run(
        &mut t,
        with_pw(
            cc::ECDH_Z_GEN,
            &[ecdh.handle],
            &[b"pw"],
            &ecc_point(&gx, &gy),
        ),
    ));
    note(run(&mut t, plain(cc::ECDH_KEY_GEN, &[signer.handle], &[])));
    note(run(
        &mut t,
        plain(cc::ECC_PARAMETERS, &[], &u16b(curve::NIST_P256)),
    ));
    let child = parse_child(&note(create_child(
        &mut t,
        storage.handle,
        b"pw",
        b"cpw",
        b"",
        &ecc_sign(),
    )));
    let (handle, _) = parse_loaded(&note(load(
        &mut t,
        storage.handle,
        b"pw",
        &child.private,
        &child.public,
    )));
    let signed = note(sign(&mut t, handle, b"cpw", &digest, &scheme));
    note(verify(&mut t, handle, &digest, params(&signed, 0)));
    let ctx = note(run(&mut t, plain(cc::CONTEXT_SAVE, &[handle], &[])));
    note(run(&mut t, plain(cc::FLUSH_CONTEXT, &[], &u32b(handle))));
    note(run(&mut t, plain(cc::CONTEXT_LOAD, &[], params(&ctx, 0))));
    for encoded in [
        "002300100018000b00030010",
        "002300100018000d00030010",
        "002300100010001000030010",
        "002300100018000b00050010",
    ] {
        let reply = run(&mut t, plain(0x18A, &[], &hex(encoded)));
        out.push(hex_of(&digest_of(HashAlg::Sha256, &reply)));
    }
    out
}

#[test]
fn sha256_p256_golden_unchanged() {
    // Taken on commit abf5923, before the ECC P-384 changes. Rows 6 and 8-12 were re-pinned when
    // `TPM2_Create` and `TPM2_ECDH_KeyGen` began to generate ECC keys with extra random bits (Part 1
    // v185 §44.6): they carry a key from `Create` or the DRBG output after it; the other rows are as
    // they were.
    const GOLDEN: [&str; 19] = [
        "29e2e3f0909910c6d2d376156a7ade399dfe34a3dbfc71b30191466b2db7d182",
        "0d4448ee099bd563985c7dfae0aac5e3e10d4f5231b63b1427372d23aeab6542",
        "67f6ec38e1cf1efaa0b6b2b8d465cfb3392f4882c896d49c56901944b6390303",
        "933a7c7f177c3f2e227c17a07a5f354262eef8b5e5a1ec443aeff14b0b873d10",
        "7ca1ac48fa25bec1a8f724932f642c2a31875aae0c5a17e2eb2b1e2331f7bf16",
        "3cf5594ed38c0ca38f5a914a7cfacd66adbaf0f4e3cf11a6d9a03a06ed11d6cd",
        "5cb3564f12b029e9811d598c71f7d0f7e34e52fee7881b1c30d2219942a88bf8",
        "38ecf2837bf0c7ffdfabafbeb765898230031c6bf3f7fe2f4c632fcdc993ad51",
        "f3d1658e22650a283451d938fec0a765d23f12940b6018aa59697bbfb4654182",
        "84df619238acee9b2574bb7eca3602b6c886a2608719319801173dcb5761d7eb",
        "a85e21e836b3ed3bb2792fcc33affb406fc7066f663d3ed5bb0e36a81856c8a1",
        "d26ca13c365bc29c37e9f822ee9786e7c3d864f8cfa5734c06f58d413face51a",
        "69f4d4cdda4c397388cd0a3f414998220cbed771eb2af6ac2e7f045e0ccf15d3",
        "5b2896e614c8b5077c7bea543c34c66431aa403bd731a18d35b9d5e07cd08613",
        "be5c3c48c5672e23bd299e7369f4c7b6682014e70daf63af7b1617784dcf1256",
        "5b2896e614c8b5077c7bea543c34c66431aa403bd731a18d35b9d5e07cd08613",
        "5b2896e614c8b5077c7bea543c34c66431aa403bd731a18d35b9d5e07cd08613",
        "7faac055939fac1ccc6764b79e7788d93f4ba6d53cb5b3eee6f036f00b4d9470",
        "7faac055939fac1ccc6764b79e7788d93f4ba6d53cb5b3eee6f036f00b4d9470",
    ];
    assert_eq!(ecc_p256_sha256_flow(), GOLDEN);
}

/// Part 1 v185 §44.6: an ECC key is generated with extra random bits (FIPS 186-5 A.2.1). `TPM2_Create`
/// and `TPM2_ECDH_KeyGen` draw the key that `ecc_generate_extra_random_bits` makes from the TPM
/// DRBG (the same state in a second TPM shows it); a Primary keeps the rejection sampling (its
/// entropy is a vendor option, Part 1 §25.6.3, and the EK and SRK must not change: the golden tests
/// above pin the Primary of the P-256 flows).
#[test]
fn create_and_ecdh_key_gen_use_extra_random_bits() {
    use crate::crypto::{ecc_generate, ecc_generate_extra_random_bits, ecc_public, ecc_scalar};

    for (curve_id, parent_template, child_template) in [
        (curve::NIST_P256, ecc_storage(), ecc_sign()),
        (
            curve::NIST_P384,
            storage(alg::SHA256),
            signing(alg::SHA256, HashAlg::Sha256),
        ),
    ] {
        let setup = || {
            let mut t = started();
            let parent = created(&create_primary(&mut t, OWNER, b"pw", &parent_template)).0;
            (t, parent)
        };
        // Create: the scalar of the child is the extra-random-bits key of the DRBG state before it.
        let (mut t, parent) = setup();
        let child = parse_child(&create_child(
            &mut t,
            parent,
            b"pw",
            b"cpw",
            b"",
            &child_template,
        ));
        let (handle, _) = parse_loaded(&load(&mut t, parent, b"pw", &child.private, &child.public));
        let scalar = secret_of(&t, handle);
        let (mut same_state, _) = setup();
        let expected = ecc_generate_extra_random_bits(&mut same_state.drbg, curve_id).unwrap();
        assert_eq!(
            scalar,
            ecc_scalar(&expected).to_vec(),
            "curve {curve_id:#x}"
        );
        assert_eq!(point_of(&child.public), ecc_public(&expected).unwrap());
        // It is not the key that rejection sampling would give from that state.
        let (mut other_state, _) = setup();
        let rejection = ecc_generate(&mut other_state.drbg, curve_id).unwrap();
        assert_ne!(scalar, ecc_scalar(&rejection).to_vec());
        // ECDH_KeyGen: the ephemeral point is the public point of the extra-random-bits key.
        let (mut t, parent) = setup();
        let reply = run(&mut t, plain(cc::ECDH_KEY_GEN, &[parent], &[]));
        ok(&reply);
        let (mut same_state, _) = setup();
        let expected = ecc_generate_extra_random_bits(&mut same_state.drbg, curve_id).unwrap();
        let (x, y) = ecc_public(&expected).unwrap();
        let point = ecc_point(&x, &y);
        assert!(
            reply.windows(point.len() - 2).any(|w| w == &point[2..]),
            "curve {curve_id:#x}"
        );
    }
}
