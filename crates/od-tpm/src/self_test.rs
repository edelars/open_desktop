//! Synchronous known-answer tests for the advertised TPM algorithms.
//! The test material is public. Randomized operations use separate fixture DRBGs.

use sha2::{Digest, Sha384};

use crate::credential;
use crate::crypto::{
    HashAlg, RsaPadding, ecc_ecdh, ecc_from_scalar, ecc_public, ecc_sign, ecc_verify, hash, hmac,
    kdfa, kdfa_sha256, rsa_decrypt_private, rsa_encrypt_public, rsa_from_primes, rsa_sign,
    rsa_sign_pss, rsa_verify, rsa_verify_hash, rsa_verify_pss,
};
use crate::paramenc::{self, SessionSym};
use crate::pcr::sha256;
use crate::random::{self, Drbg};

pub(crate) fn known_answers() -> bool {
    const SHA384_ABC: [u8; 48] = hex(
        "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7",
    );
    sha256(b"abc") == hex("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        && Sha384::digest(b"abc")[..] == SHA384_ABC
        && sha2_known_answers()
        && random::self_test()
        && drbg_known_answers().is_some()
        && transport_known_answers()
        && credential::self_test()
        && ecc_known_answers().is_some()
        && rsa_known_answers().is_some()
        && rsa3072_known_answers().is_some()
        && mlkem_known_answers().is_some()
}

// SHA-512, KDFa and HMAC over SHA-384/SHA-512 and the ECDSA verification of SHA-384/SHA-512
// digests. All values are computed outside this crate with Python hashlib/hmac (the KDFa label,
// contexts and key are those of `transport_known_answers`), except the published ECDSA
// signatures of RFC 6979 Appendix A.2.5 (P-256, message "sample"), which a Python verifier
// checked against the published public key.
fn sha2_known_answers() -> bool {
    const SHA512_ABC: [u8; 64] = hex(
        "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
    );
    const KDFA_384_600: [u8; 75] = hex(
        "f02fc2fb91bd9da254b6703aca4d714443769d927e9b05d7d3d6cad38839a45775aff5c7827de8d6b6e64a9b535d29a0f5054b58b5fc1fd305f7abb7a27122cedc40f29c3c7e1947d56042",
    );
    const KDFA_512_600: [u8; 75] = hex(
        "e67bb9db2b69a2714b3aab6bf7fe077b66aac683099af2c74c8f6db8c95ee2184bb7bdaeb52d472a7ea96f5a813bb12a60667fe13d9d5699ca3193212e3b4e7105d0399b2b0dc2ff3e6127",
    );
    // RFC 4231 test case 2.
    const HMAC_384: [u8; 48] = hex(
        "af45d2e376484031617f78d2b58a6b1b9c7ef464f5a01b47e42ec3736322445e8e2240ca5e69e2c78b3239ecfab21649",
    );
    const HMAC_512: [u8; 64] = hex(
        "164b7a7bfcf819e2e395fbe73b56e0a387bd64222e831fd610270cd7ea2505549758bf75c05a994a6d034f65f8f0e6fdcaeab1a34d4a6b4b636e070a38bce737",
    );
    const X: [u8; 32] = hex("60fed4ba255a9d31c961eb74c6356d68c049b8923b61fa6ce669622e60f29fb6");
    const Y: [u8; 32] = hex("7903fe1008b8bc99a41ae9e95628bc64f2f1b20c2d7e9f5177a3c294d4462299");
    const R_384: [u8; 32] = hex("0eafea039b20e9b42309fb1d89e213057cbf973dc0cfc8f129edddc800ef7719");
    const S_384: [u8; 32] = hex("4861f0491e6998b9455193e34e7b0d284ddd7149a74b95b9261f13abde940954");
    const R_512: [u8; 32] = hex("8496a60b5e9b47c825488827e0495b0e3fa109ec4568fd3f8d1097678eb97f00");
    const S_512: [u8; 32] = hex("2362ab1adbe2b8adf9cb9edab740ea6049c028114f2460f96554f61fae3302fe");
    // The algorithm table agrees with itself.
    for h in [HashAlg::Sha256, HashAlg::Sha384, HashAlg::Sha512] {
        if HashAlg::from_alg(h.alg()) != Some(h) || hash(h, &[b"abc"]).as_slice().len() != h.size()
        {
            return false;
        }
    }
    let (key, context_u, context_v) = ([1u8; 32], [2u8; 16], [3u8; 16]);
    let (data_a, data_b) = (&b"what do ya want "[..], &b"for nothing?"[..]);
    hash(HashAlg::Sha512, &[b"abc"]).as_slice() == SHA512_ABC
        && kdfa(HashAlg::Sha384, &key, b"ATH", &context_u, &context_v, 600).as_slice()
            == KDFA_384_600
        && kdfa(HashAlg::Sha512, &key, b"ATH", &context_u, &context_v, 600).as_slice()
            == KDFA_512_600
        && hmac(HashAlg::Sha384, b"Jefe", &[data_a, data_b]).as_slice() == HMAC_384
        && hmac(HashAlg::Sha512, b"Jefe", &[data_a, data_b]).as_slice() == HMAC_512
        && ecc_verify(
            &X,
            &Y,
            hash(HashAlg::Sha384, &[b"sample"]).as_slice(),
            &R_384,
            &S_384,
        )
        && ecc_verify(
            &X,
            &Y,
            hash(HashAlg::Sha512, &[b"sample"]).as_slice(),
            &R_512,
            &S_512,
        )
}

// NIST ACVP ML-KEM-encapDecap-FIPS203-tr1, revision 975de31eb83d87039ec88934fdc47d8c312b892d.
// ML-KEM-768 groups 2/6, cases 26/96. Keys and answers are public test material.
fn mlkem_known_answers() -> Option<()> {
    use ml_kem::ml_kem_768::EncapsulationKey;
    use ml_kem::{Decapsulate, TryKeyInit};

    const SEED: [u8; 64] = hex(
        "2f9325677156319738d9512406f4135275ddceeb7025c273c230c5aa3598f06e95ae7b26b8b6985de3e83441745d6a8424b273dd58b16aabb9dd1d0ab4157f6d",
    );
    const PUBLIC_SHA256: [u8; 32] =
        hex("0b43cfd80fa7cb6eb5896444e3dd171322e0355400388eaff81b9c0146fab8d6");
    const DECAPSULATED: [u8; 32] =
        hex("0039695f071cbe88d820bf4483d060c2d217ed1b7b6f48616ea21368fa2f76cb");
    const M: [u8; 32] = hex("de00df8311d05e830c71b568da4cf0f0c7f458d94139de92d3c5f21c3ca48ad8");
    const CIPHERTEXT_SHA256: [u8; 32] =
        hex("9c55953bc189c554e9307e83243a960b479161acd06c183f238d3bba3666459a");
    const ENCAPSULATED: [u8; 32] =
        hex("2ef8de64bfbfeb974ec48e8fb839fa16ed5b848d573429ad906493818481f075");

    let private = crate::kem::private_from_seed(&SEED).ok()?;
    let decapsulated = private
        .decapsulate_slice(include_bytes!("kat/mlkem768-acvp96-ct.bin"))
        .ok()?;
    let public =
        EncapsulationKey::new_from_slice(include_bytes!("kat/mlkem768-acvp26-ek.bin")).ok()?;
    let (ciphertext, encapsulated) = public.encapsulate_deterministic(&M.into());
    (sha256(&crate::kem::public_bytes(&private)) == PUBLIC_SHA256
        && decapsulated.as_slice() == DECAPSULATED
        && sha256(ciphertext.as_slice()) == CIPHERTEXT_SHA256
        && encapsulated.as_slice() == ENCAPSULATED)
        .then_some(())
}

// Fixture DRBG stream independently computed from SP 800-90A using Python hashlib/HMAC.
fn drbg_known_answers() -> Option<()> {
    const FIRST: [u8; 32] = hex("5990bc4a3ace179f764ce400fb04ea068702231318252f994164efcb56ed50fe");
    const SECOND: [u8; 40] =
        hex("600751a52d539ac1458309cd147cfed5585468bffd48b981458c9fa61c01dea1ab9a16b913c8cb12");
    let entropy: [u8; 48] = core::array::from_fn(|i| i as u8);
    let mut rng = Drbg::new(&entropy)?;
    let mut first = [0; 32];
    let mut second = [0; 40];
    rng.generate(&mut first)?;
    rng.generate(&mut second)?;
    (first == FIRST && second == SECOND).then_some(())
}

fn transport_known_answers() -> bool {
    const KDFA_256: [u8; 32] =
        hex("32316f0b89c58eaa1e22f5fda644e5b5aca6fbb7605b7bc37d8ffe830730fc07");
    const XOR: [u8; 28] = hex("a2a3bc81404fdde08b7944ee59126e530380fe46a86b584d57a5455d");
    const CFB128: [u8; 28] = hex("ee55abb9c4280144b16b0e87a7c8598e95714446b30a543da0a74e2a");
    const CFB256: [u8; 28] = hex("5025ed706adbfe1c36e214472cec2265bde977b9c78b3072055c1d96");
    if kdfa_sha256(&[1; 32], b"ATH", &[2; 16], &[3; 16], 256).as_slice() != KDFA_256 {
        return false;
    }
    let key = b"sv-0123456789abcdef";
    let plain = b"\x00\x1chello, parameter encryption!";
    for (sym, expected) in [
        (SessionSym::Xor, XOR),
        (SessionSym::AesCfb128, CFB128),
        (SessionSym::AesCfb256, CFB256),
    ] {
        let mut param = *plain;
        paramenc::apply(sym, key, &[0x11; 16], &[0x22; 16], &mut param, true);
        if param[..2] != [0, 28] || param[2..] != expected {
            return false;
        }
        paramenc::apply(sym, key, &[0x11; 16], &[0x22; 16], &mut param, false);
        if param != *plain {
            return false;
        }
    }
    true
}

// RFC 6979 Appendix A.2.5, SHA-256("sample") with the published P-256 test key.
// The ECDH coordinates are independently calculated for scalar 2 and that non-generator peer.
fn ecc_known_answers() -> Option<()> {
    const D: [u8; 32] = hex("c9afa9d845ba75166b5c215767b1d6934e50c3db36e89b127b8a622b120f6721");
    const X: [u8; 32] = hex("60fed4ba255a9d31c961eb74c6356d68c049b8923b61fa6ce669622e60f29fb6");
    const Y: [u8; 32] = hex("7903fe1008b8bc99a41ae9e95628bc64f2f1b20c2d7e9f5177a3c294d4462299");
    const R: [u8; 32] = hex("efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716");
    const S: [u8; 32] = hex("f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8");
    const ZX: [u8; 32] = hex("ed3687f8bd593c3d260ead3cbf2d4ac102e1e845e1f58da14343c20e6b1a3d4b");
    const ZY: [u8; 32] = hex("37856c506e12c97117bcc59642d099b6a9cd1dee43186d30a1645effcab20df4");
    let key = ecc_from_scalar(&D)?;
    let (x, y) = ecc_public(&key)?;
    let digest = sha256(b"sample");
    let (r, s) = ecc_sign(&key, &digest)?;
    if x.as_slice() != X
        || y.as_slice() != Y
        || r.as_slice() != R
        || s.as_slice() != S
        || !ecc_verify(&X, &Y, &digest, &R, &S)
    {
        return None;
    }
    let mut scalar = [0; 32];
    scalar[31] = 2;
    let peer = ecc_from_scalar(&scalar)?;
    let (zx, zy) = ecc_ecdh(&peer, &X, &Y)?;
    (zx.as_slice() == ZX && zy.as_slice() == ZY).then_some(())
}

// Synthetic RSA-2048 key already used by the externally checked command vectors.
// Byte strings are shared with the integration tests. They are not TPM hierarchy keys.
pub(crate) const EXT_RSA_N: &str = concat!(
    "b6b69daba0f0e0d6246da2202df16a574a318e3cfae808680103fc1691dccd0c",
    "5c857c3e9bfecf97f7a21adefe26b2252f193ef0ffb69770dc4709e62c591e5f",
    "f4fea1a4a74e6df9aaadb9b9e4852d0b249893b1785fdb1f48330c7441fff4dd",
    "6a1f24ef3e3f4c10057bbca6ab5cdfe3a01d816cc353dd0b50fa51b25e1a3529",
    "d529fb299d3d38b4dadf33b0a6851c6cc7a29c4a63560868f4804ddc5baf1dc7",
    "09fee622a953c09af5833924dd3efbc8d90a83f69693bb9b5f6ba560df78e895",
    "223181c9ac36edca4aec5eeb18d7c7e4d0d8032b13947f0d696f7bc6acaa8c68",
    "b58b096b5b8991b103f7afd07bb24592c83516972ff629c7d8a794f98fc98a61",
);
pub(crate) const EXT_RSA_P: &str = concat!(
    "f0637c28545965c0d1476b756f530aaad92012ac1101640a07e51d183228d705",
    "dd5e6393874c8c7c48653834fc19f425fa80a254a6eecd4d9e38d711dc6c737a",
    "f5a444cfeca628ecf33589d3e2bba3d2e29bbea69b4eaae595c0364282954fa6",
    "76d881c8bb06d1ade13a09cf58aea6263c8998396c10a813b9e666b3816ba29f",
);
pub(crate) const EXT_RSA_Q: &str = concat!(
    "c294443a6cd262858996540306d90e8606a9b646cc8e497cc09c53a92d269357",
    "91087cccb17d762e97871eb114d11a5cd38c39036953cf4ca646c458085c52b3",
    "dade50c76ce4b4900f73238cd44a78cf3124d640d0d35558d9a9fda758f6c259",
    "84a0147689ae1d6f9b32303e5c3f3208d2980a3c6dd9919603ce33392180b2ff",
);

// PKCS #1 encoded messages and modular exponentiation computed independently in Python,
// with a fresh synthetic deterministic HMAC_DRBG for each padding mode.
const RSA_MESSAGE_HASH: [u8; 32] =
    hex("776c3d76c58ce4085112637e3edb646856dfe1de2df5892e74a704e4412de7a0");
const RSA_SIGNATURE: [u8; 256] = hex(concat!(
    "2dba5743bcb1444c4bfebd9593cf27685177383478e97e1a245a3882edb1c94e",
    "41c210331b2408bc4e105752a79c5b329828cf57a31002412094f2eeb86764b9",
    "645ab1777f44e0c1a819fdbf46a1e830c62a231da37f54a138d640cffa2c9a56",
    "90fc2d6aae0129d6861ba7d8f11dd958c9a85cd93ae10903cf91b9573d356f69",
    "f6ad694abcabfb0b14c688ff3568af470911fb51153e7ed141a680508d83e1b7",
    "d2e15309b633827f44eaeb4c44a88ffa5eba78c57a88c209a9684284ce5a5371",
    "57d7952631676d4b5c1181c7215fad232041a0b72f91fbc1510c49f357cba751",
    "137f04b3d808d2cfa0b7667a01e64412a6129c00beeaa6a62368537fe5717ebe",
));
const RSA_SHA2_MESSAGE: &[u8] = b"Open Desktop SHA-2 agility";
const RSA_SIGNATURE_384: [u8; 256] = hex(concat!(
    "3f87909d7fa6ad591c25e711ca7914318f82c45ddac87e06ca5966c0eb5603a3",
    "8168cf8a37e764b778ef351dd43ed6a154be958ddabfc173fe2bdfb26e772baa",
    "eb173093d0c02266d3019e9bb3f95ac4037cb50e47bc8bf459d55d599db7a0c8",
    "57e02301fbc28d2d9e375f5ffba6571770c81d524fac5f29bd4eebd582f36c11",
    "a69fc7900c2d9ef0aac5d468d88ba6d06ff9276da065c3c09498f1fbc59f1b0d",
    "03e6bf0f9595a24228842fe72ca8aadb6b92fdc5c6c598aecc2e54389240276e",
    "4c7cdfb802dd8ecd54df538f574ecf761231297c7158b9878ce7960377e64559",
    "e0efe893fc52c683c84b249e155826d246d4bf4b6d747b6087153467f9bf5620",
));
const RSA_SIGNATURE_512: [u8; 256] = hex(concat!(
    "79762a7568771c314eb7223d8a0c38c77af2b52dd7d08abf9596499fc5176c97",
    "f9cf07c785bbca09f5ee903aa4c75ab68a9f2be6aa36c1486a71ede89005aa19",
    "743bab518bdc14192a56be5fad71dd1d5e89c2788627d9fe5101ce0552c8adb2",
    "64d5ec9fa691af12eb7de97ad1c557514c573141f69bffd4e87a44948f9c8658",
    "579669306ad6c2d38f3145ec568ece8380e7e10a6d6496c65e1b416c2eae1a75",
    "651de73afcd37e920305939258feaaf3c0bf4d5deae3601642a3160f14c97baa",
    "3bf91bf8346b0ce9546d3cc732a133691195c80067a9816bbee028065223579c",
    "03554b7df820a7529604b74d9c0e4925a5d241048e233e9f794d66cc66ee9c53",
));
const RSAES_CIPHER: [u8; 256] = hex(concat!(
    "a56933e19a6d11d0100449a58a2bc04ac19d5d486d2f69a5e04443f94f23b4db",
    "3078dbe2068f8108853178903dc0c7f8a2178fc454f93ee6101a837347257bc9",
    "37ebe783e7e9234078c10c421eb039c4c201879da0d02f49d3dfa2d92b3985b3",
    "e3b97ae36cefa571b92dc16e08f4af8cb3df9237ba6c1e1db99548069ce9a3f1",
    "b83494888b8f65db9846c411c45076be5a324f3418357763ae432733650dbb76",
    "1a1a7da6d70eedb287b46491ac3858647c21b32f437658d4e873416876f95880",
    "8afad8ef54cd7c48659d0a24b6e9fba9215763d4b362d95ed477e323c63267d4",
    "703f85295b6a6356fc7eb5ef71bbac1305cddb6c1b1cc9ac1ca129527b0730a1",
));
const OAEP_CIPHER: [u8; 256] = hex(concat!(
    "59c0993fc3abd7f9f378e7d00b1bd2f87ae3d1b2d22e129848777c76d47ef270",
    "401777159aae21233bb7a498e48cc7974004ab3cc2c30486d8021ac272b5772e",
    "e1c311c2f9dd06e1f61ba01a9f010db3619b28e33c8c15a7743c4aa9b8a09cb7",
    "90ff18a9f023639b3949c5c6831f78be9e37e3080a2fb379080a51107a525c45",
    "116126ff40ee0bbda6f26941b9e18becf6aaedbb63bec01305644f3fa1857795",
    "4571a68bc108f37f490b881cb0186566a97b448a80d39ee5ab5257c5d2fa012d",
    "024f1b1f1817e489481ad72a508d0cd812e45cba78d6a53cc97d4c9263d4f78d",
    "d40aa4fe5f4d2d1ced3107a187c957384c50ccbbe8890b17d99c079d3b414d76",
));

// Synthetic RSA-3072 key made by `openssl genpkey` (public test material, not a TPM key). The message is
// "Open Desktop RSA-3072 self-test"; the signatures come from `openssl dgst -sign` (PKCS #1 v1.5 is
// deterministic; PSS with `rsa_pss_saltlen:digest` is salted, so it is verified, not compared).
pub(crate) const RSA3072_N: [u8; 384] = hex(concat!(
    "b7c539bdd13d9f075d237ce39c3e0a5b68fad0cd7241df9741bfe9179d30df25",
    "b8994f1e0347c01e02bcd788aa24de28cbad67a5e3f546491f2fd199e39c1d3e",
    "d1e9e844779e8d622f39ba802b3e0d26ce2a6d106563add74ee04d4e51e47e77",
    "fd8ca56800338b9dd91bf51b6c13564653297f2144e79051d26c80e0c2cca78f",
    "168c5e60974457720f033e509c8fb83c5497883babbc1aff36f8b12f6171012f",
    "a55c72284e31198690f905e2a5a873cace07f8359decc3e56497b8d1e7cf35bd",
    "06a961d221a001b34945e2c34b31f67988bdcf26bf2cac81ce4a692868a5aa3d",
    "a2fe5be66b75f7d8d3bc29a98e28ca8c88127c5f0a2c56d0bebc0fbddeb3bd39",
    "dede4a457b26b6882a3ee503a63ff6de16640121ad43676c450f08ec5c516d9e",
    "7758eaee7118493bebd6d22df0d81ebde83f976bbfa1e69e7dd9be903c49ea30",
    "f1417c0982d9662f2dfdc87a19b85c3f8e5cdf3f1eee4c3a0455c36fe83afa06",
    "db6bdf2762b78e2749fa40f02c778d0aa59fe833326d3ea2ed1d26217e32a2fd",
));
pub(crate) const RSA3072_P: [u8; 192] = hex(concat!(
    "f1233dc22cb42f141256c06a8eb42aefc6b3281e7c9baf84e41459cada2dd762",
    "a50e3bbc61cc47d5ab8c5eb6532baa7d73445cc13209bddfb1ee4dba4b2ed602",
    "e1d6ec38b92121ef20d50ca1e2e3ef8d19ccc17e6f7f316b14f166fda054324b",
    "93a9f7e924fc06b972d5d717a0739936542a97c7d47baa09ad59ff0550b15201",
    "c6293f88d2cd85c996ef1c0a1af398eff5cba5b0a5ee9d539b8348c01fb522f8",
    "37a2b3512cac4d746fa88276efcf09a854b3b566d4b743272522581062b5e083",
));
pub(crate) const RSA3072_Q: [u8; 192] = hex(concat!(
    "c318d29f786d5e89d603e7fe3b10ae1eb7ff9a6637b7c76cca36685e41037d67",
    "f7f60f6f28bb8457bc05cefe713dee006033c4ef031882ded6617185de611b5f",
    "0ea3d799f0041df7b45d283ec8a127c889b908c02a1fd4647b6f668e55d9bc12",
    "2853aeaf68f6ce87b052099ffb584edb6c56c3af112ed1186ccb24a59575b0aa",
    "b097f77ccfa17321eb42253cceaa87d8b18b5bb3b25ca554e40702e1d07a3695",
    "29be7be2a595c1d281e5ca50acc525b08f5ecdec7ce545c38830af49bf0f167f",
));
#[cfg(test)]
pub(crate) const RSA3072_MESSAGE: &[u8] = b"Open Desktop RSA-3072 self-test";
pub(crate) const RSA3072_DIGEST_256: [u8; 32] =
    hex("56404a62e8463ce3d97999e47ac135d16aa52ca4cb5e69fc897febe996dfba57");
pub(crate) const RSA3072_DIGEST_384: [u8; 48] = hex(concat!(
    "37ac4b6e75fca9b341feb8de100b411e1e702485a52b40c9ace5430c1f36c2b6",
    "151898dafb0ee54f9ebf206cb48b3101",
));
pub(crate) const RSA3072_PKCS1_SHA256: [u8; 384] = hex(concat!(
    "b634f99ecedc612a5cab73c3d2cc1cce69d63c858753f13a3c3bd2c37c394a3d",
    "5b73d549ee4d9e175f07a6c8056ef356caf35a73cb16fa6a28ab300cfcaffb43",
    "63b519c957d76d791723ca790c602d644339db61c941857ce851057059beac0a",
    "695dc738394766b74c2bcf6af389875ab32f162fdf7017c52fdb03bfdc1229a5",
    "e1ab674d1e3bc196ffe907da3223c2c1e88e27372f0756060dd407304554b67c",
    "288b1bf4c4491788d3aa2ea64605428cd1d36a6e1bd3bfbafef7fedf5e1223fb",
    "2e658592ac3f65d31faa90829676d822a151f6dab1484b9511675b0925bf8a50",
    "225077395aad761e3bc75713f67eff896ce3bcacc05055dca4897960aad23008",
    "ea97b336aece7c087f660e44a34b22778888170758e1f6b6fe4eb34e3f2c704c",
    "4a73d3e3fc7a1e9ab54646fad806fc6e9f511603ee4c77be7d5449fe677353ce",
    "12e0282b41002bcfa783b9e01e7414c40d776362e131aa60809813ec19eca9c2",
    "653a9004e3dac41c414a6df864207ccf9a07017443db201b65f3ff1495a1f6a7",
));
pub(crate) const RSA3072_PSS_SHA256: [u8; 384] = hex(concat!(
    "096f310bf110d13c0a4b51c9d91f5b71dc8d6a3953aaf2f8bd1d3bec57dc03fe",
    "0310f46daf18812fbb00136835ef417e9f208d5d69cb57e4055ab8520ca59940",
    "d9e1558a4fa70776a20f15010ddcf52c3e04b934bfc798e4f04f6e3479055400",
    "cd4fa870ea4cda98d04a9a80a9b12188e96b7757d96598b8c99cfe9dcf6a1ccc",
    "82019540c3820f09a53d9866db0ef3d32d7adacfda560291b097d7259bdd3011",
    "40a72e572b8ef96fee236c9a0db8d2541177214b1afa1cb8e5e3eb857073348e",
    "10c04fe91a84ebdf01b0905005f2add3957f3cb46089089337cf443b17596d05",
    "b9a268d2da193317c8ed629dbb3d249ca029d58270c72b0c16427bad5a860664",
    "dcdc42eec3b4b57c58b7c5963fec53be8bb905fe2f6813312608df512f944337",
    "2fb7ffebf74374ea098abb81735559a0679c1a9a01a56abfc52b15db8ea09db3",
    "6c614719b3fee4dd560df03f276102fa5961c6434cbc1b36e49dfd04a8704982",
    "cb209064dbb85c55a360aac1db5a39ad4169aaca5317178499f31502af06bdd0",
));
pub(crate) const RSA3072_PSS_SHA384: [u8; 384] = hex(concat!(
    "8ed6783624acb87e7ff566898dc402b8542c87be6587b6e95bae69422db3489a",
    "73861126675b8e21d20a074b49b1654ca95b58f0b61224f32028fb37f8087087",
    "6bfebc7268657385b352a67130522b31da9781e936ac6977a01e53a3787a08fc",
    "a54634a1c6e093feefc0696e734fdb367d17d2aebb59e79f76575c58c16e5a5c",
    "fc0841e7018401525c8d0c17b2f607d535f7e74c9e65d07ba363348192e98163",
    "4ce44d7977676dfa824b580bd813147c4b427682b32cb35c08065b1529eb5b35",
    "abbe50d7e30cf9fef6618dd9681fd7bb5101300eb5be8649a39a584b4c025a35",
    "9d5520b75a8497964c782fb7c177c96ec7f390ce9eb2d0b4c8c200bbba0752f7",
    "00ece5c3c37f418b950d59f5d0cf6411a9e091ac9998105f96e6b0ccc5de5679",
    "2bfa6f34d6ae0837ef4311e996b4f506bff921164068676549c58dff13ac120e",
    "8d55c954a879b4f430438ea9af8c7e608ee089de47204f86ab7bd9b481cb8706",
    "38e784e9360c49bb9eb163dc4bd3e8e7fabcba83ede020046ed8198abfa9392a",
));

fn rsa3072_known_answers() -> Option<()> {
    let mut primes = [0; 384];
    primes[..192].copy_from_slice(&RSA3072_P);
    primes[192..].copy_from_slice(&RSA3072_Q);
    let key = rsa_from_primes(&primes, &RSA3072_N)?;
    let mut rng = Drbg::new(&[5; 48])?;
    // RSASSA-PKCS1-v1_5 SHA-256: sign (deterministic) and verify.
    let signature = rsa_sign(&key, HashAlg::Sha256, &RSA3072_DIGEST_256, &mut rng)?;
    if signature.as_slice() != RSA3072_PKCS1_SHA256
        || !rsa_verify_hash(
            &RSA3072_N,
            HashAlg::Sha256,
            &RSA3072_DIGEST_256,
            &RSA3072_PKCS1_SHA256,
        )
    {
        return None;
    }
    // RSASSA-PSS made outside this crate (SHA-256 and SHA-384, salt as long as the digest) verifies;
    // neither padding mode accepts the other's signature.
    if !rsa_verify_pss(
        &RSA3072_N,
        HashAlg::Sha256,
        &RSA3072_DIGEST_256,
        &RSA3072_PSS_SHA256,
    ) || !rsa_verify_pss(
        &RSA3072_N,
        HashAlg::Sha384,
        &RSA3072_DIGEST_384,
        &RSA3072_PSS_SHA384,
    ) || rsa_verify_pss(
        &RSA3072_N,
        HashAlg::Sha256,
        &RSA3072_DIGEST_256,
        &RSA3072_PKCS1_SHA256,
    ) || rsa_verify_hash(
        &RSA3072_N,
        HashAlg::Sha256,
        &RSA3072_DIGEST_256,
        &RSA3072_PSS_SHA256,
    ) {
        return None;
    }
    // PSS signing: the salt is random, so the signature is checked by the verifier above.
    let signature = rsa_sign_pss(&key, HashAlg::Sha384, &RSA3072_DIGEST_384, &mut rng)?;
    rsa_verify_pss(&RSA3072_N, HashAlg::Sha384, &RSA3072_DIGEST_384, &signature).then_some(())
}

fn rsa_known_answers() -> Option<()> {
    const N: [u8; 256] = hex(EXT_RSA_N);
    const P: [u8; 128] = hex(EXT_RSA_P);
    const Q: [u8; 128] = hex(EXT_RSA_Q);
    let mut primes = [0; 256];
    primes[..128].copy_from_slice(&P);
    primes[128..].copy_from_slice(&Q);
    let key = rsa_from_primes(&primes, &N)?;
    let mut rng = Drbg::new(&[5; 48])?;
    let signature = rsa_sign(&key, HashAlg::Sha256, &RSA_MESSAGE_HASH, &mut rng)?;
    if signature.as_slice() != RSA_SIGNATURE || !rsa_verify(&N, &RSA_MESSAGE_HASH, &RSA_SIGNATURE) {
        return None;
    }
    // RSASSA-PKCS1-v1_5 with SHA-384 and SHA-512 (the EM and pow(EM, d, n) computed in Python).
    for (hash_alg, expected) in [
        (HashAlg::Sha384, &RSA_SIGNATURE_384),
        (HashAlg::Sha512, &RSA_SIGNATURE_512),
    ] {
        let digest = hash(hash_alg, &[RSA_SHA2_MESSAGE]);
        let signature = rsa_sign(&key, hash_alg, digest.as_slice(), &mut rng)?;
        if signature.as_slice() != expected
            || !rsa_verify_hash(&N, hash_alg, digest.as_slice(), expected)
            || rsa_verify_hash(&N, HashAlg::Sha256, digest.as_slice(), expected)
        {
            return None;
        }
    }
    let message = b"Open Desktop TPM self-test";
    for (padding, label, ciphertext) in [
        (RsaPadding::Rsaes, &[][..], &RSAES_CIPHER),
        (RsaPadding::Oaep, &b"self-test\0"[..], &OAEP_CIPHER),
    ] {
        let mut encrypt_rng = Drbg::new(&[5; 48])?;
        let mut decrypt_rng = Drbg::new(&[5; 48])?;
        if rsa_encrypt_public(&N, padding, label, message, &mut encrypt_rng)?.as_slice()
            != ciphertext
            || rsa_decrypt_private(&key, padding, label, ciphertext, &mut decrypt_rng)?.as_slice()
                != message
        {
            return None;
        }
    }
    Some(())
}

pub(crate) const fn hex<const N: usize>(text: &str) -> [u8; N] {
    const fn nibble(byte: u8) -> u8 {
        match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            _ => panic!("invalid self-test vector"),
        }
    }
    let bytes = text.as_bytes();
    assert!(bytes.len() == N * 2);
    let mut out = [0; N];
    let mut i = 0;
    while i < N {
        out[i] = nibble(bytes[i * 2]) << 4 | nibble(bytes[i * 2 + 1]);
        i += 1;
    }
    out
}
