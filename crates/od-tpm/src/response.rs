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
    pub const SEQUENCE: Rc = Rc(RC_VER1 + 0x003);
    /// Slice C2 (Part 2, Table 20): policy sessions and `TPM2_PolicyPCR`.
    pub const POLICY: Rc = Rc(RC_VER1 + 0x026);
    pub const PCR: Rc = Rc(RC_VER1 + 0x027);
    pub const PCR_CHANGED: Rc = Rc(RC_VER1 + 0x028);
    pub const CPHASH: Rc = Rc(RC_VER1 + 0x051);
    pub const NO_RESULT: Rc = Rc(RC_VER1 + 0x054);
    pub const DISABLED: Rc = Rc(RC_VER1 + 0x020);
    pub const EXCLUSIVE: Rc = Rc(RC_VER1 + 0x021);
    pub const AUTH_TYPE: Rc = Rc(RC_VER1 + 0x024);
    pub const AUTH_MISSING: Rc = Rc(RC_VER1 + 0x025);
    pub const AUTH_UNAVAILABLE: Rc = Rc(RC_VER1 + 0x02F);
    pub const COMMAND_SIZE: Rc = Rc(RC_VER1 + 0x042);
    pub const COMMAND_CODE: Rc = Rc(RC_VER1 + 0x043);
    pub const AUTHSIZE: Rc = Rc(RC_VER1 + 0x044);
    pub const AUTH_CONTEXT: Rc = Rc(RC_VER1 + 0x045);
    pub const NV_RANGE: Rc = Rc(RC_VER1 + 0x046);
    pub const NV_SIZE: Rc = Rc(RC_VER1 + 0x047);
    pub const NV_LOCKED: Rc = Rc(RC_VER1 + 0x048);
    pub const NV_AUTHORIZATION: Rc = Rc(RC_VER1 + 0x049);
    pub const NV_UNINITIALIZED: Rc = Rc(RC_VER1 + 0x04A);
    pub const NV_SPACE: Rc = Rc(RC_VER1 + 0x04B);
    pub const NV_DEFINED: Rc = Rc(RC_VER1 + 0x04C);
    pub const PARENT: Rc = Rc(RC_VER1 + 0x052);
    pub const SENSITIVE: Rc = Rc(RC_VER1 + 0x055);
    pub const NEEDS_TEST: Rc = Rc(RC_VER1 + 0x053);

    // Format-one (RC_FMT1 + n); index modifiers are added with the `in_*` methods.
    pub const ATTRIBUTES: Rc = Rc(RC_FMT1 + 0x002);
    pub const HASH: Rc = Rc(RC_FMT1 + 0x003);
    pub const VALUE: Rc = Rc(RC_FMT1 + 0x004);
    pub const HIERARCHY: Rc = Rc(RC_FMT1 + 0x005);
    pub const KEY_SIZE: Rc = Rc(RC_FMT1 + 0x007);
    pub const MODE: Rc = Rc(RC_FMT1 + 0x009);
    pub const TYPE: Rc = Rc(RC_FMT1 + 0x00A);
    pub const HANDLE: Rc = Rc(RC_FMT1 + 0x00B);
    pub const KDF: Rc = Rc(RC_FMT1 + 0x00C);
    pub const RANGE: Rc = Rc(RC_FMT1 + 0x00D);
    pub const AUTH_FAIL: Rc = Rc(RC_FMT1 + 0x00E);
    pub const NONCE: Rc = Rc(RC_FMT1 + 0x00F);
    pub const SCHEME: Rc = Rc(RC_FMT1 + 0x012);
    pub const SIZE: Rc = Rc(RC_FMT1 + 0x015);
    pub const SYMMETRIC: Rc = Rc(RC_FMT1 + 0x016);
    pub const TAG: Rc = Rc(RC_FMT1 + 0x017);
    pub const INSUFFICIENT: Rc = Rc(RC_FMT1 + 0x01A);
    pub const SIGNATURE: Rc = Rc(RC_FMT1 + 0x01B);
    pub const KEY: Rc = Rc(RC_FMT1 + 0x01C);
    pub const INTEGRITY: Rc = Rc(RC_FMT1 + 0x01F);
    pub const TICKET: Rc = Rc(RC_FMT1 + 0x020);
    pub const RESERVED_BITS: Rc = Rc(RC_FMT1 + 0x021);
    pub const BAD_AUTH: Rc = Rc(RC_FMT1 + 0x022);
    pub const POLICY_FAIL: Rc = Rc(RC_FMT1 + 0x01D);
    pub const EXPIRED: Rc = Rc(RC_FMT1 + 0x023);
    pub const POLICY_CC: Rc = Rc(RC_FMT1 + 0x024);
    pub const BINDING: Rc = Rc(RC_FMT1 + 0x025);
    pub const PARMS: Rc = Rc(RC_FMT1 + 0x02A);
    pub const SIGN_CONTEXT_KEY: Rc = Rc(RC_FMT1 + 0x02D);
    pub const CURVE: Rc = Rc(RC_FMT1 + 0x026);
    pub const ECC_POINT: Rc = Rc(RC_FMT1 + 0x027);

    // Warnings (RC_WARN + n).
    pub const OBJECT_MEMORY: Rc = Rc(RC_WARN + 0x002);
    pub const SESSION_HANDLES: Rc = Rc(RC_WARN + 0x005);
    pub const OBJECT_HANDLES: Rc = Rc(RC_WARN + 0x006);
    pub const LOCALITY: Rc = Rc(RC_WARN + 0x007);
    /// `TPM_RC_LOCKOUT` (Part 2, Table 20): DA lockout, or `lockoutAuth` disabled after a failure.
    pub const LOCKOUT: Rc = Rc(RC_WARN + 0x021);
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

/// One acknowledgement in a response with sessions: `TPMS_AUTH_RESPONSE` (Part 2, Table 154).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionAck {
    /// `nonceTPM`: empty for a password authorization.
    pub nonce: Vec<u8>,
    pub attributes: u8,
    /// Response HMAC: empty for a password authorization (and for an HMAC session whose command
    /// carried an empty HMAC, Part 1 v184, clause 17.6.15).
    pub hmac: Vec<u8>,
}

impl SessionAck {
    /// Password acknowledgement: empty nonce, `continueSession` SET, empty HMAC (Part 1,
    /// clause 17.4, Table 18).
    pub(crate) fn password(command_attributes: u8) -> SessionAck {
        SessionAck {
            nonce: Vec::new(),
            attributes: command_attributes | 0x01,
            hmac: Vec::new(),
        }
    }
}

/// Success response. With sessions it adds `parameterSize` and one acknowledgement per command
/// session.
pub(crate) fn success_response(out: &Output, acks: &[SessionAck], with_sessions: bool) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&out.handles);
    if with_sessions {
        body.extend_from_slice(&(out.params.len() as u32).to_be_bytes());
    }
    body.extend_from_slice(&out.params);
    if with_sessions {
        for ack in acks {
            body.extend_from_slice(&(ack.nonce.len() as u16).to_be_bytes());
            body.extend_from_slice(&ack.nonce);
            body.push(ack.attributes);
            body.extend_from_slice(&(ack.hmac.len() as u16).to_be_bytes());
            body.extend_from_slice(&ack.hmac);
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
        assert_eq!(Rc::LOCKOUT.0, 0x921);
        assert_eq!(Rc::NEEDS_TEST.0, 0x153);
        // Codes of phase B slice A (Table 20 of the same text).
        assert_eq!(Rc::DISABLED.0, 0x120);
        assert_eq!(Rc::AUTH_TYPE.0, 0x124);
        assert_eq!(Rc::AUTH_CONTEXT.0, 0x145);
        assert_eq!(Rc::NV_RANGE.0, 0x146);
        assert_eq!(Rc::NV_SIZE.0, 0x147);
        assert_eq!(Rc::NV_LOCKED.0, 0x148);
        assert_eq!(Rc::NV_AUTHORIZATION.0, 0x149);
        assert_eq!(Rc::NV_UNINITIALIZED.0, 0x14A);
        assert_eq!(Rc::NV_SPACE.0, 0x14B);
        assert_eq!(Rc::NV_DEFINED.0, 0x14C);
        assert_eq!(Rc::HIERARCHY.0, 0x085);
        assert_eq!(Rc::AUTH_FAIL.0, 0x08E);
        assert_eq!(Rc::NONCE.0, 0x08F);
        assert_eq!(Rc::SYMMETRIC.0, 0x096);
        assert_eq!(Rc::SESSION_HANDLES.0, 0x905);
        assert_eq!(Rc::RESERVED_BITS.0, 0x0A1);
        // Codes of phase B slice B (Table 20 of the same text).
        assert_eq!(Rc::SEQUENCE.0, 0x103);
        assert_eq!(Rc::AUTH_UNAVAILABLE.0, 0x12F);
        assert_eq!(Rc::SENSITIVE.0, 0x155);
        assert_eq!(Rc::ATTRIBUTES.0, 0x082);
        assert_eq!(Rc::KEY_SIZE.0, 0x087);
        assert_eq!(Rc::TYPE.0, 0x08A);
        assert_eq!(Rc::KDF.0, 0x08C);
        assert_eq!(Rc::RANGE.0, 0x08D);
        assert_eq!(Rc::SCHEME.0, 0x092);
        assert_eq!(Rc::SIGNATURE.0, 0x09B);
        assert_eq!(Rc::KEY.0, 0x09C);
        assert_eq!(Rc::INTEGRITY.0, 0x09F);
        assert_eq!(Rc::TICKET.0, 0x0A0);
        assert_eq!(Rc::BINDING.0, 0x0A5);
        assert_eq!(Rc::CURVE.0, 0x0A6);
        assert_eq!(Rc::ECC_POINT.0, 0x0A7);
        assert_eq!(Rc::OBJECT_HANDLES.0, 0x906);
        // Codes of phase B slice C2 (Table 20 of the same text).
        assert_eq!(Rc::POLICY.0, 0x126);
        assert_eq!(Rc::PCR_CHANGED.0, 0x128);
        assert_eq!(Rc::CPHASH.0, 0x151);
        assert_eq!(Rc::POLICY_FAIL.0, 0x09D);
        assert_eq!(Rc::EXPIRED.0, 0x0A3);
        assert_eq!(Rc::POLICY_CC.0, 0x0A4);
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
