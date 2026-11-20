//! Session-based parameter encryption (slice C3 of phase B, S7-23): Part 1 v184, clause 19, and the
//! `symmetric` parameter of `TPM2_StartAuthSession` (Part 2, Table 159).
//!
//! A session started with `symmetric` = AES-128-CFB (`TPM_ALG_AES`, 128 bits, `TPM_ALG_CFB`) or
//! XOR (`TPM_ALG_XOR`, hash SHA-256) can carry the `decrypt` and `encrypt` attributes of
//! `TPMA_SESSION`:
//! - `decrypt` SET: the first parameter of the command, a sized buffer, arrives encrypted; the
//!   TPM decrypts its data portion (not its size field) after the authorization check (the
//!   cpHash is computed over the encrypted parameter, clause 19.1).
//! - `encrypt` SET: the TPM encrypts the data portion of the first parameter of the response, a
//!   sized buffer, before it computes the rpHash.
//!
//! `sessionValue` is `sessionKey || authValue` of the entity the session authorizes (the binding
//! is ignored, clause 19.1; the sessions of a command are exactly its authorization sessions).
//! CFB: `KDFa(SHA-256, sessionValue, "CFB", nonceNewer, nonceOlder, 256)`, the first 16 octets are
//! the AES key, the next 16 the IV (clause 19.3); XOR: `mask = KDFa(SHA-256, sessionValue, "XOR",
//! nonceNewer, nonceOlder, 8 * size)` (clauses 19.2 and 9.4.7.3). For a command `nonceNewer` is
//! the `nonceCaller` of the session and `nonceOlder` the `nonceTPM` the session had before the
//! command; for a response `nonceNewer` is the new `nonceTPM` and `nonceOlder` the `nonceCaller`.
//!
//! Rules: a session with a NULL symmetric algorithm that asks for encryption is
//! `TPM_RC_SYMMETRIC` (clause 19.1); at most one session of a command has `decrypt` and at most
//! one has `encrypt`; the attribute on a command whose first parameter (or response parameter) is
//! not a sized buffer is `TPM_RC_ATTRIBUTES` (see the lists below).
//!
//! NOT VERIFIED against TPM 2.0 Library v185/v184: `TPM_RC_ATTRIBUTES` (session number) for
//! `decrypt`/`encrypt` on a command without a sized first parameter/response parameter, and for a
//! second session with the same attribute (the text states the rules, not the code); the lists
//! [`first_parameter_is_sized`] and [`first_response_parameter_is_sized`] (derived from the
//! command tables of Part 3: the first parameter being a `TPM2B_*`; `TPM2_StartAuthSession` is
//! excluded, no test or text confirms an encrypted `nonceCaller`); the codes `TPM_RC_KEY_SIZE`
//! (AES key size other than 128) and `TPM_RC_HASH` (XOR hash other than SHA-256) of
//! `StartAuthSession` (`TPM_RC_MODE` for a mode other than CFB is named in Part 3, 11.1.1); that
//! the `authValue` in `sessionValue` has no trailing zeros (as in every authorization
//! computation); that the attributes of the response acknowledgement keep `decrypt` and
//! `encrypt` as sent. A session used only for encryption (without an authorized handle) is not
//! implemented: sessions beyond the authorized handles are `TPM_RC_AUTHSIZE` (`dispatch.rs`).

use crate::codes::{alg, cc};
use crate::crypto::kdfa_sha256;
use crate::marshal::Reader;
use crate::object::aes128_cfb;
use crate::response::Rc;

/// The symmetric algorithm of a session (`TPMT_SYM_DEF`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SessionSym {
    Null,
    Xor,
    AesCfb128,
}

impl SessionSym {
    /// Parses a `TPMT_SYM_DEF` (parameter 4 of `TPM2_StartAuthSession`): `TPM_ALG_NULL`; XOR with
    /// its hash (SHA-256); AES with 128 key bits and mode CFB. Anything else is refused with the
    /// parameter number 4.
    pub(crate) fn unmarshal(r: &mut Reader<'_>) -> Result<SessionSym, Rc> {
        match r.u16()? {
            alg::NULL => Ok(SessionSym::Null),
            alg::XOR => match r.u16()? {
                alg::SHA256 => Ok(SessionSym::Xor),
                _ => Err(Rc::HASH.in_param(4)),
            },
            alg::AES => {
                if r.u16()? != 128 {
                    return Err(Rc::KEY_SIZE.in_param(4));
                }
                if r.u16()? != alg::CFB {
                    return Err(Rc::MODE.in_param(4));
                }
                Ok(SessionSym::AesCfb128)
            }
            _ => Err(Rc::SYMMETRIC.in_param(4)),
        }
    }

    /// One octet for the saved context of a session.
    pub(crate) fn to_octet(self) -> u8 {
        match self {
            SessionSym::Null => 0,
            SessionSym::Xor => 1,
            SessionSym::AesCfb128 => 2,
        }
    }

    pub(crate) fn from_octet(b: u8) -> Option<SessionSym> {
        match b {
            0 => Some(SessionSym::Null),
            1 => Some(SessionSym::Xor),
            2 => Some(SessionSym::AesCfb128),
            _ => None,
        }
    }
}

/// `true` if the first parameter of the command (after the handle area) is a sized buffer, so that
/// `decrypt` applies (Part 3 v184 command tables: a `TPM2B_*` first parameter). The implemented
/// commands only.
pub(crate) fn first_parameter_is_sized(code: u32) -> bool {
    matches!(
        code,
        cc::STIR_RANDOM
            | cc::HASH
            | cc::HASH_SEQUENCE_START
            | cc::SEQUENCE_UPDATE
            | cc::SEQUENCE_COMPLETE
            | cc::SIGN_SEQUENCE_START
            | cc::VERIFY_SEQUENCE_START
            | cc::SIGN_SEQUENCE_COMPLETE
            | cc::EVENT_SEQUENCE_COMPLETE
            | cc::PCR_EVENT
            | cc::NV_DEFINE_SPACE
            | cc::NV_WRITE
            | cc::NV_EXTEND
            | cc::HIERARCHY_CHANGE_AUTH
            | cc::SET_PRIMARY_POLICY
            | cc::START_AUTH_SESSION
            | cc::CREATE_PRIMARY
            | cc::CREATE
            | cc::LOAD
            | cc::LOAD_EXTERNAL
            | cc::MAKE_CREDENTIAL
            | cc::ACTIVATE_CREDENTIAL
            | cc::OBJECT_CHANGE_AUTH
            | cc::IMPORT
            | cc::DUPLICATE
            | cc::SIGN
            | cc::SIGN_DIGEST
            | cc::DECAPSULATE
            | cc::VERIFY_SIGNATURE
            | cc::VERIFY_DIGEST_SIGNATURE
            | cc::RSA_ENCRYPT
            | cc::RSA_DECRYPT
            | cc::ECDH_Z_GEN
            | cc::HMAC
            | cc::HMAC_START
            | cc::QUOTE
            | cc::CERTIFY
            | cc::CERTIFY_CREATION
            | cc::NV_CERTIFY
            | cc::NV_CHANGE_AUTH
            | cc::GET_TIME
            | cc::GET_SESSION_AUDIT_DIGEST
            | cc::POLICY_PCR
            | cc::POLICY_SECRET
            | cc::POLICY_SIGNED
            | cc::POLICY_TICKET
            | cc::POLICY_COUNTER_TIMER
            | cc::POLICY_NV
            | cc::POLICY_CP_HASH
            | cc::POLICY_NAME_HASH
            | cc::POLICY_DUPLICATION_SELECT
            | cc::POLICY_AUTHORIZE
            | cc::POLICY_TEMPLATE
            | cc::POLICY_PARAMETERS
    )
}

/// `true` if the first parameter of the response of the command is a sized buffer, so that
/// `encrypt` applies (the response parameter area, after the handle area).
pub(crate) fn first_response_parameter_is_sized(code: u32) -> bool {
    matches!(
        code,
        cc::GET_RANDOM
            | cc::ENCAPSULATE
            | cc::DECAPSULATE
            | cc::GET_TEST_RESULT
            | cc::START_AUTH_SESSION
            | cc::HASH
            | cc::SEQUENCE_COMPLETE
            | cc::UNSEAL
            | cc::RSA_ENCRYPT
            | cc::RSA_DECRYPT
            | cc::ECDH_KEY_GEN
            | cc::ECDH_Z_GEN
            | cc::HMAC
            | cc::NV_READ
            | cc::NV_READ_PUBLIC
            | cc::READ_PUBLIC
            | cc::CREATE
            | cc::CREATE_PRIMARY
            | cc::LOAD
            | cc::LOAD_EXTERNAL
            | cc::MAKE_CREDENTIAL
            | cc::ACTIVATE_CREDENTIAL
            | cc::OBJECT_CHANGE_AUTH
            | cc::IMPORT
            | cc::DUPLICATE
            | cc::QUOTE
            | cc::CERTIFY
            | cc::CERTIFY_CREATION
            | cc::NV_CERTIFY
            | cc::GET_TIME
            | cc::GET_SESSION_AUDIT_DIGEST
            | cc::POLICY_GET_DIGEST
            | cc::POLICY_SECRET
            | cc::POLICY_SIGNED
    )
}

/// Encrypts or decrypts the data portion of the sized buffer at the start of `params` in place
/// (`[size u16][data]`; the size field stays). A buffer whose size field does not fit is left
/// alone: the command reports its own `TPM_RC_SIZE`. `nonce_newer` and `nonce_older` as in the
/// module comment.
pub(crate) fn apply(
    sym: SessionSym,
    session_value: &[u8],
    nonce_newer: &[u8],
    nonce_older: &[u8],
    params: &mut [u8],
    encrypt: bool,
) {
    if params.len() < 2 || sym == SessionSym::Null {
        return;
    }
    let size = usize::from(u16::from_be_bytes([params[0], params[1]]));
    if size == 0 || 2 + size > params.len() {
        return;
    }
    let data = &mut params[2..2 + size];
    match sym {
        SessionSym::Null => {}
        SessionSym::Xor => {
            let mask = kdfa_sha256(
                session_value,
                b"XOR",
                nonce_newer,
                nonce_older,
                (size * 8) as u32,
            );
            for (d, m) in data.iter_mut().zip(mask.iter()) {
                *d ^= m;
            }
        }
        SessionSym::AesCfb128 => {
            let kv = kdfa_sha256(session_value, b"CFB", nonce_newer, nonce_older, 256);
            let mut iv = [0u8; 16];
            iv.copy_from_slice(&kv[16..32]);
            let out = aes128_cfb(&kv[..16], &iv, data, encrypt);
            data.copy_from_slice(&out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Independent evidence (2026-11-07): `sessionValue` = "sv-0123456789abcdef", nonceNewer =
    // 16 x 0x11, nonceOlder = 16 x 0x22, plaintext "hello, parameter encryption!" (28 octets).
    // KDFa(SHA-256, ...) and the XOR mask were computed with Python `hmac`/`hashlib`; the AES-128-CFB
    // ciphertext with `openssl enc -aes-128-cfb` (key and IV = the two halves of the KDFa output).
    const SV: &[u8] = b"sv-0123456789abcdef";
    const PLAIN: &[u8] = b"hello, parameter encryption!";
    const XOR_CT: &str = "a2a3bc81404fdde08b7944ee59126e530380fe46a86b584d57a5455d";
    const CFB_CT: &str = "ee55abb9c4280144b16b0e87a7c8598e95714446b30a543da0a74e2a";

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    }

    fn sized(data: &[u8]) -> Vec<u8> {
        let mut v = (data.len() as u16).to_be_bytes().to_vec();
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn xor_and_cfb_match_independent_vectors() {
        let (n, o) = ([0x11u8; 16], [0x22u8; 16]);
        for (sym, ct) in [(SessionSym::Xor, XOR_CT), (SessionSym::AesCfb128, CFB_CT)] {
            let mut p = sized(PLAIN);
            apply(sym, SV, &n, &o, &mut p, true);
            assert_eq!(&p[..2], &[0, 28], "the size field is not encrypted");
            assert_eq!(&p[2..], &hex(ct)[..], "{sym:?}");
            apply(sym, SV, &n, &o, &mut p, false);
            assert_eq!(&p[2..], PLAIN);
        }
    }

    #[test]
    fn data_after_the_first_parameter_and_bad_sizes_are_left_alone() {
        let (n, o) = ([1u8; 16], [2u8; 16]);
        // A second parameter after the sized buffer is not touched.
        let mut p = sized(b"abcd");
        p.extend_from_slice(&[9, 9, 9]);
        apply(SessionSym::AesCfb128, SV, &n, &o, &mut p, true);
        assert_eq!(&p[6..], &[9, 9, 9]);
        assert_ne!(&p[2..6], b"abcd");
        // A size field larger than the buffer, an empty buffer and a NULL algorithm: unchanged.
        let mut bad = vec![0, 9, 1, 2];
        apply(SessionSym::Xor, SV, &n, &o, &mut bad, false);
        assert_eq!(bad, [0, 9, 1, 2]);
        let mut empty = vec![0, 0];
        apply(SessionSym::Xor, SV, &n, &o, &mut empty, true);
        assert_eq!(empty, [0, 0]);
        let mut p = sized(b"abcd");
        apply(SessionSym::Null, SV, &n, &o, &mut p, true);
        assert_eq!(&p[2..], b"abcd");
    }

    #[test]
    fn sym_def_parsing() {
        let parse = |b: &[u8]| SessionSym::unmarshal(&mut Reader::new(b));
        assert_eq!(parse(&[0, 0x10]), Ok(SessionSym::Null));
        assert_eq!(parse(&[0, 0x0A, 0, 0x0B]), Ok(SessionSym::Xor));
        assert_eq!(
            parse(&[0, 0x06, 0, 128, 0, 0x43]),
            Ok(SessionSym::AesCfb128)
        );
        assert_eq!(parse(&[0, 0x0A, 0, 0x04]), Err(Rc::HASH.in_param(4)));
        assert_eq!(
            parse(&[0, 0x06, 1, 0, 0, 0x43]),
            Err(Rc::KEY_SIZE.in_param(4))
        );
        assert_eq!(
            parse(&[0, 0x06, 0, 128, 0, 0x44]),
            Err(Rc::MODE.in_param(4))
        );
        assert_eq!(parse(&[0, 0x13]), Err(Rc::SYMMETRIC.in_param(4)));
        for s in [SessionSym::Null, SessionSym::Xor, SessionSym::AesCfb128] {
            assert_eq!(SessionSym::from_octet(s.to_octet()), Some(s));
        }
        assert_eq!(SessionSym::from_octet(3), None);
    }
}
