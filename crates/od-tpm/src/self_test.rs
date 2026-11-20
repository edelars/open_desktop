//! Synchronous known-answer tests for the advertised TPM algorithms.
//! The test material is public. Randomized operations use separate fixture DRBGs.

use sha2::{Digest, Sha384};

use crate::credential;
use crate::crypto::{
    RsaPadding, ecc_ecdh, ecc_from_scalar, ecc_public, ecc_sign, ecc_verify, kdfa_sha256,
    rsa_decrypt_private, rsa_encrypt_public, rsa_from_primes, rsa_sign, rsa_verify,
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
        && random::self_test()
        && drbg_known_answers().is_some()
        && transport_known_answers()
        && credential::self_test()
        && ecc_known_answers().is_some()
        && rsa_known_answers().is_some()
        && mlkem_known_answers().is_some()
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

fn rsa_known_answers() -> Option<()> {
    const N: [u8; 256] = hex(EXT_RSA_N);
    const P: [u8; 128] = hex(EXT_RSA_P);
    const Q: [u8; 128] = hex(EXT_RSA_Q);
    let mut primes = [0; 256];
    primes[..128].copy_from_slice(&P);
    primes[128..].copy_from_slice(&Q);
    let key = rsa_from_primes(&primes, &N)?;
    let mut rng = Drbg::new(&[5; 48])?;
    let signature = rsa_sign(&key, &RSA_MESSAGE_HASH, &mut rng)?;
    if signature.as_slice() != RSA_SIGNATURE || !rsa_verify(&N, &RSA_MESSAGE_HASH, &RSA_SIGNATURE) {
        return None;
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

const fn hex<const N: usize>(text: &str) -> [u8; N] {
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
