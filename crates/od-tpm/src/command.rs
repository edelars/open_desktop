//! Command framing: header, authorization area and password sessions.
//!
//! Command layout (Part 2 v184, clause 6; Part 3 command tables): `tag` (UINT16), `commandSize`
//! (UINT32), `commandCode` (UINT32), the handle area, then — only for `TPM_ST_SESSIONS` —
//! `authorizationSize` (UINT32) and the authorization area, then the parameters.
//!
//! This module checks the form of a password session (`TPM_RS_PW`, Part 1 v184, clause 17.4):
//! empty `nonceCaller`, only `continueSession` may be set, `hmac` carries the password. HMAC
//! sessions (slice A of phase B) are classified and verified by `dispatch.rs` and `session.rs`;
//! a policy-session handle is answered with the warning `TPM_RC_REFERENCE_Sn` (no policy session
//! is ever loaded).
//!
//! NOT VERIFIED against TPM 2.0 Library v185 (the open v184 text does not give the codes):
//! a password session with a non-empty nonce is answered `TPM_RC_BAD_AUTH` and one with other
//! attribute bits `TPM_RC_ATTRIBUTES` (both with the session number); a malformed authorization
//! area is `TPM_RC_AUTHSIZE`.

use crate::codes::{handle, st};
use crate::marshal::{Reader, TpmsAuthCommand, limits};
use crate::response::Rc;

/// The ten octets every command starts with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Header {
    pub tag: u16,
    pub size: u32,
    pub code: u32,
}

/// Size of the command and response header.
pub const HEADER_SIZE: usize = 10;

/// Validates the header against the whole command buffer.
///
/// - fewer than 10 octets, or `commandSize` different from the buffer length, or a buffer above
///   the maximum assembled command size: `TPM_RC_COMMAND_SIZE`;
/// - a `tag` other than `TPM_ST_NO_SESSIONS`/`TPM_ST_SESSIONS`: `TPM_RC_TAG` (Part 2, clause 6.6.1,
///   for a TPM without TPM 1.2 compatibility).
pub fn parse_header(cmd: &[u8]) -> Result<Header, Rc> {
    if cmd.len() < HEADER_SIZE || cmd.len() > limits::MAX_COMMAND_SIZE {
        return Err(Rc::COMMAND_SIZE);
    }
    let mut r = Reader::new(cmd);
    let tag = r.u16()?;
    if tag != st::NO_SESSIONS && tag != st::SESSIONS {
        return Err(Rc::TAG);
    }
    let size = r.u32()?;
    if size as usize != cmd.len() {
        return Err(Rc::COMMAND_SIZE);
    }
    let code = r.u32()?;
    Ok(Header { tag, size, code })
}

/// Parses the authorization area that follows the handle area of a `TPM_ST_SESSIONS` command. A
/// session takes a nonce and an `hmac` of up to one SHA-512 digest here; the dispatcher narrows the
/// limit to the hash of each loaded session (`TPM_RC_AUTHSIZE`) and keeps 32 octets for a password
/// (the password of `TPM_RS_PW` is read here as an `hmac` of up to 64 octets, without narrowing).
/// Per Part 3 v185 §5.5, the size must cover at least one session header (nine
/// octets), and the area must contain at most [`limits::MAX_AUTH_SESSIONS`] complete sessions.
pub fn parse_auth_area(r: &mut Reader<'_>) -> Result<Vec<TpmsAuthCommand>, Rc> {
    let size = r.u32().map_err(|_| Rc::AUTHSIZE)? as usize;
    if !(9..=r.remaining()).contains(&size) {
        return Err(Rc::AUTHSIZE);
    }
    let mut area = Reader::new(r.bytes(size)?);
    let mut sessions = Vec::new();
    while area.remaining() > 0 {
        if sessions.len() == limits::MAX_AUTH_SESSIONS {
            return Err(Rc::AUTHSIZE);
        }
        sessions.push(
            TpmsAuthCommand::unmarshal_up_to(&mut area, limits::MAX_HASH_SIZE)
                .map_err(|_| Rc::AUTHSIZE)?,
        );
    }
    Ok(sessions)
}

/// Checks that session number `n` (one-based) is a well-formed password session.
pub fn check_password_session(s: &TpmsAuthCommand, n: usize) -> Result<(), Rc> {
    let num = n as u8;
    if s.session_handle == handle::RS_PW {
        if !s.nonce.is_empty() {
            return Err(Rc::BAD_AUTH.in_session(num));
        }
        // Only `continueSession` (bit 0) may be set for a password authorization.
        if s.attributes & 0xFE != 0 {
            return Err(Rc::ATTRIBUTES.in_session(num));
        }
        return Ok(());
    }
    match handle::handle_type(s.session_handle) {
        // HMAC (0x02) sessions are handled by the dispatcher before this function is called; a
        // handle of that type that reaches it names no loaded session. Policy (0x03) sessions
        // are not implemented and never loaded.
        0x02 | 0x03 => Err(Rc::reference_session(n - 1)),
        _ => Err(Rc::VALUE.in_session(num)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::marshal::Writer;

    fn cmd(tag: u16, size: u32, code: u32, tail: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&tag.to_be_bytes());
        v.extend_from_slice(&size.to_be_bytes());
        v.extend_from_slice(&code.to_be_bytes());
        v.extend_from_slice(tail);
        v
    }

    #[test]
    fn header_ok() {
        let c = cmd(st::NO_SESSIONS, 12, 0x144, &[0, 0]);
        assert_eq!(
            parse_header(&c),
            Ok(Header {
                tag: st::NO_SESSIONS,
                size: 12,
                code: 0x144
            })
        );
    }

    #[test]
    fn header_errors() {
        assert_eq!(parse_header(&[]), Err(Rc::COMMAND_SIZE));
        assert_eq!(
            parse_header(&[0x80, 1, 0, 0, 0, 10, 0, 0, 1]),
            Err(Rc::COMMAND_SIZE)
        );
        // Unknown tag.
        assert_eq!(parse_header(&cmd(0x8003, 10, 0x144, &[])), Err(Rc::TAG));
        // commandSize not equal to the buffer length (both directions).
        assert_eq!(
            parse_header(&cmd(st::NO_SESSIONS, 11, 0x144, &[])),
            Err(Rc::COMMAND_SIZE)
        );
        assert_eq!(
            parse_header(&cmd(st::NO_SESSIONS, 10, 0x144, &[0])),
            Err(Rc::COMMAND_SIZE)
        );
        // Above the CRB limit.
        let big = vec![0u8; limits::MAX_COMMAND_SIZE + 1];
        assert_eq!(parse_header(&big), Err(Rc::COMMAND_SIZE));
    }

    fn pw(password: &[u8]) -> Vec<u8> {
        let mut w = Writer::new();
        TpmsAuthCommand {
            session_handle: handle::RS_PW,
            nonce: vec![],
            attributes: 1,
            hmac: password.to_vec(),
        }
        .marshal(&mut w);
        w.finish()
    }

    #[test]
    fn auth_area_parsing() {
        let one = pw(b"x");
        let mut area = (one.len() as u32).to_be_bytes().to_vec();
        area.extend_from_slice(&one);
        let mut r = Reader::new(&area);
        let s = parse_auth_area(&mut r).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].hmac, b"x");
        r.finish().unwrap();
    }

    #[test]
    fn auth_area_errors() {
        assert_eq!(
            parse_auth_area(&mut Reader::new(&[0, 0, 0, 0])),
            Err(Rc::AUTHSIZE)
        );
        // authorizationSize beyond the buffer.
        assert_eq!(
            parse_auth_area(&mut Reader::new(&[0, 0, 0, 9, 1])),
            Err(Rc::AUTHSIZE)
        );
        // Cut-off session inside the area.
        assert_eq!(
            parse_auth_area(&mut Reader::new(&[0, 0, 0, 2, 0x40, 0])),
            Err(Rc::AUTHSIZE)
        );
        // Missing size field.
        assert_eq!(
            parse_auth_area(&mut Reader::new(&[0, 0])),
            Err(Rc::AUTHSIZE)
        );
        // Four sessions.
        let one = pw(b"");
        let mut area = ((one.len() * 4) as u32).to_be_bytes().to_vec();
        for _ in 0..4 {
            area.extend_from_slice(&one);
        }
        assert_eq!(parse_auth_area(&mut Reader::new(&area)), Err(Rc::AUTHSIZE));
    }

    #[test]
    fn password_session_checks() {
        let mk = |h: u32, nonce: &[u8], attrs: u8| TpmsAuthCommand {
            session_handle: h,
            nonce: nonce.to_vec(),
            attributes: attrs,
            hmac: vec![],
        };
        assert_eq!(
            check_password_session(&mk(handle::RS_PW, &[], 0), 1),
            Ok(())
        );
        assert_eq!(
            check_password_session(&mk(handle::RS_PW, &[], 1), 1),
            Ok(())
        );
        assert_eq!(
            check_password_session(&mk(handle::RS_PW, &[1], 1), 1),
            Err(Rc::BAD_AUTH.in_session(1))
        );
        assert_eq!(
            check_password_session(&mk(handle::RS_PW, &[], 0x20), 2),
            Err(Rc::ATTRIBUTES.in_session(2))
        );
        assert_eq!(
            check_password_session(&mk(0x0200_0000, &[], 1), 1),
            Err(Rc::reference_session(0))
        );
        assert_eq!(
            check_password_session(&mk(0x0300_0001, &[], 1), 2),
            Err(Rc::reference_session(1))
        );
        assert_eq!(
            check_password_session(&mk(0x0000_0001, &[], 1), 1),
            Err(Rc::VALUE.in_session(1))
        );
        assert_eq!(
            check_password_session(&mk(handle::RH_OWNER, &[], 1), 1),
            Err(Rc::VALUE.in_session(1))
        );
    }
}
