//! Response codes (`TPM_RC`) and response framing.
//!
//! Values: Table 20 of TPM 2.0 Library Part 2 v184 (see `codes.rs` for the source check).

use crate::codes::st;

/// A `TPM_RC` value. Format-one codes (bit 7 set) may carry a parameter, handle or session index.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rc(pub u32);

const RC_VER1: u32 = 0x100;
const RC_FMT1: u32 = 0x080;
const RC_WARN: u32 = 0x900;

impl Rc {
    pub const SUCCESS: Rc = Rc(0x000);

    // Format-zero (RC_VER1 + n).
    pub const INITIALIZE: Rc = Rc(RC_VER1);
    pub const FAILURE: Rc = Rc(RC_VER1 + 0x001);
    pub const AUTH_MISSING: Rc = Rc(RC_VER1 + 0x025);
    pub const COMMAND_SIZE: Rc = Rc(RC_VER1 + 0x042);
    pub const COMMAND_CODE: Rc = Rc(RC_VER1 + 0x043);
    pub const AUTHSIZE: Rc = Rc(RC_VER1 + 0x044);
    pub const NEEDS_TEST: Rc = Rc(RC_VER1 + 0x053);

    // Format-one (RC_FMT1 + n); index modifiers are added with the `in_*` methods.
    pub const ATTRIBUTES: Rc = Rc(RC_FMT1 + 0x002);
    pub const HASH: Rc = Rc(RC_FMT1 + 0x003);
    pub const VALUE: Rc = Rc(RC_FMT1 + 0x004);
    pub const MODE: Rc = Rc(RC_FMT1 + 0x009);
    pub const HANDLE: Rc = Rc(RC_FMT1 + 0x00B);
    pub const SIZE: Rc = Rc(RC_FMT1 + 0x015);
    pub const TAG: Rc = Rc(RC_FMT1 + 0x017);
    pub const INSUFFICIENT: Rc = Rc(RC_FMT1 + 0x01A);
    pub const BAD_AUTH: Rc = Rc(RC_FMT1 + 0x022);

    // Warnings (RC_WARN + n).
    pub const OBJECT_MEMORY: Rc = Rc(RC_WARN + 0x002);
    pub const LOCALITY: Rc = Rc(RC_WARN + 0x007);
    pub const REFERENCE_H0: u32 = RC_WARN + 0x010;
    pub const REFERENCE_S0: u32 = RC_WARN + 0x018;

    /// `TPM_RC_BAD_TAG` (0x01E); the TPM returns it only to a TPM 1.2 compatible caller (Part 2,
    /// clause 6.6.1), phase A answers an unrecognised tag with `TAG` instead.
    pub const BAD_TAG: Rc = Rc(0x01E);

    /// Adds the one-based parameter number `n` (`TPM_RC_P + TPM_RC_n`) to a format-one code.
    pub fn in_param(self, n: u8) -> Rc {
        if self.0 & RC_FMT1 != 0 && (1..=15).contains(&n) {
            Rc(self.0 | 0x040 | (u32::from(n) << 8))
        } else {
            self
        }
    }

    /// Adds the one-based handle number `n` (1..=7) to a format-one code.
    pub fn in_handle(self, n: u8) -> Rc {
        if self.0 & RC_FMT1 != 0 && (1..=7).contains(&n) {
            Rc(self.0 | (u32::from(n) << 8))
        } else {
            self
        }
    }

    /// Adds the one-based session number `n` (1..=7) to a format-one code (`TPM_RC_S + TPM_RC_n`).
    pub fn in_session(self, n: u8) -> Rc {
        if self.0 & RC_FMT1 != 0 && (1..=7).contains(&n) {
            Rc(self.0 | 0x800 | (u32::from(n) << 8))
        } else {
            self
        }
    }

    /// `TPM_RC_REFERENCE_Hn` for the zero-based handle index `i` (warning, no index modifier).
    pub fn reference_handle(i: usize) -> Rc {
        Rc(Self::REFERENCE_H0 + (i.min(6) as u32))
    }

    /// `TPM_RC_REFERENCE_Sn` for the zero-based session index `i`.
    pub fn reference_session(i: usize) -> Rc {
        Rc(Self::REFERENCE_S0 + (i.min(6) as u32))
    }
}

/// Command result before framing: handles for the response handle area and response parameters.
#[derive(Default)]
pub(crate) struct Output {
    pub handles: Vec<u8>,
    pub params: Vec<u8>,
}

/// Error response: tag `TPM_ST_NO_SESSIONS`, `responseSize` 10, the code (Part 2, clause 6.6.1).
pub fn error_response(rc: Rc) -> Vec<u8> {
    let mut v = Vec::with_capacity(10);
    v.extend_from_slice(&st::NO_SESSIONS.to_be_bytes());
    v.extend_from_slice(&10u32.to_be_bytes());
    v.extend_from_slice(&rc.0.to_be_bytes());
    v
}

/// Success response. With sessions it adds `parameterSize` and one acknowledgement per
/// command session (password acknowledgement: empty nonce, `continueSession` SET, empty HMAC;
/// Part 1, clause 17.4, Table 18). `session_attrs` are the attribute octets of the command sessions.
pub(crate) fn success_response(out: &Output, session_attrs: &[u8], with_sessions: bool) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&out.handles);
    if with_sessions {
        body.extend_from_slice(&(out.params.len() as u32).to_be_bytes());
    }
    body.extend_from_slice(&out.params);
    if with_sessions {
        for attrs in session_attrs {
            body.extend_from_slice(&0u16.to_be_bytes());
            body.push(attrs | 0x01);
            body.extend_from_slice(&0u16.to_be_bytes());
        }
    }
    let tag = if with_sessions {
        st::SESSIONS
    } else {
        st::NO_SESSIONS
    };
    let mut v = Vec::with_capacity(10 + body.len());
    v.extend_from_slice(&tag.to_be_bytes());
    v.extend_from_slice(&((10 + body.len()) as u32).to_be_bytes());
    v.extend_from_slice(&Rc::SUCCESS.0.to_be_bytes());
    v.extend_from_slice(&body);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc_values_match_part_2_table_20() {
        assert_eq!(Rc::INITIALIZE.0, 0x100);
        assert_eq!(Rc::COMMAND_CODE.0, 0x143);
        assert_eq!(Rc::COMMAND_SIZE.0, 0x142);
        assert_eq!(Rc::SIZE.0, 0x095);
        assert_eq!(Rc::INSUFFICIENT.0, 0x09A);
        assert_eq!(Rc::VALUE.0, 0x084);
        assert_eq!(Rc::HASH.0, 0x083);
        assert_eq!(Rc::BAD_AUTH.0, 0x0A2);
        assert_eq!(Rc::LOCALITY.0, 0x907);
        assert_eq!(Rc::NEEDS_TEST.0, 0x153);
    }

    #[test]
    fn index_modifiers_follow_clause_6_6_2() {
        // Part 2 clause 6.6.2 example: TPM_RC_VALUE for the first parameter of Startup is 0x1C4.
        assert_eq!(Rc::VALUE.in_param(1).0, 0x1C4);
        assert_eq!(Rc::VALUE.in_handle(1).0, 0x184);
        assert_eq!(Rc::BAD_AUTH.in_session(1).0, 0x9A2);
        // Format-zero and warning codes carry no index.
        assert_eq!(Rc::INITIALIZE.in_param(1), Rc::INITIALIZE);
        assert_eq!(Rc::LOCALITY.in_session(1), Rc::LOCALITY);
    }

    #[test]
    fn reference_warnings() {
        assert_eq!(Rc::reference_handle(0).0, 0x910);
        assert_eq!(Rc::reference_session(1).0, 0x919);
    }

    #[test]
    fn error_response_is_ten_bytes() {
        assert_eq!(
            error_response(Rc::COMMAND_CODE),
            [0x80, 0x01, 0, 0, 0, 10, 0, 0, 0x01, 0x43]
        );
    }
}
