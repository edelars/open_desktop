//! Numeric constants of TPM 2.0 Library Part 2 that phase A uses.
//!
//! Source check (S7-21, accessed 2026-11-07): every value below was compared with the open copy
//! "Trusted Platform Module 2.0 Library Part 2: Structures, v184, 2025/03/20"
//! (<https://trustedcomputinggroup.org/wp-content/uploads/Trusted-Platform-Module-2.0-Library-Part-2-Version-184_pub.pdf>)
//! and the PCR attributes with "TCG PC Client Platform TPM Profile for TPM 2.0, Version 1.07,
//! 3/23/2026" (<https://medias.cdn.trustedcomputinggroup.org/production/PC_Client_Specific_Platform_TPM_Profile_for_TPM_2p0_v1p07_Pub_5643562943.pdf>).
//! The project targets Library v185; the v185 text was not reachable (HTTP 403 on the guessed URL),
//! so a value that v185 might have changed is covered by the same `NOT VERIFIED against v185` note
//! in `docs/specs/VTPM.md`. Values that no open source confirmed carry an explicit
//! `NOT VERIFIED` comment at the place of use.

/// Table 14 (v184): `TPM_CC` command codes of the phase A profile.
pub mod cc {
    pub const PCR_EVENT: u32 = 0x0000_013C;
    pub const PCR_RESET: u32 = 0x0000_013D;
    pub const SEQUENCE_COMPLETE: u32 = 0x0000_013E;
    pub const SELF_TEST: u32 = 0x0000_0143;
    pub const STARTUP: u32 = 0x0000_0144;
    pub const SHUTDOWN: u32 = 0x0000_0145;
    pub const STIR_RANDOM: u32 = 0x0000_0146;
    pub const SEQUENCE_UPDATE: u32 = 0x0000_015C;
    pub const GET_CAPABILITY: u32 = 0x0000_017A;
    pub const GET_RANDOM: u32 = 0x0000_017B;
    pub const GET_TEST_RESULT: u32 = 0x0000_017C;
    pub const HASH: u32 = 0x0000_017D;
    pub const PCR_READ: u32 = 0x0000_017E;
    pub const READ_CLOCK: u32 = 0x0000_0181;
    pub const PCR_EXTEND: u32 = 0x0000_0182;
    pub const EVENT_SEQUENCE_COMPLETE: u32 = 0x0000_0185;
    pub const HASH_SEQUENCE_START: u32 = 0x0000_0186;
}

/// Table 23 (v184): `TPM_ST` structure tags.
pub mod st {
    pub const NO_SESSIONS: u16 = 0x8001;
    pub const SESSIONS: u16 = 0x8002;
    pub const HASHCHECK: u16 = 0x8024;
}

/// Table 26 (v184): `TPM_CAP` capability selectors.
pub mod cap {
    pub const ALGS: u32 = 0x0000_0000;
    pub const HANDLES: u32 = 0x0000_0001;
    pub const COMMANDS: u32 = 0x0000_0002;
    pub const PP_COMMANDS: u32 = 0x0000_0003;
    pub const AUDIT_COMMANDS: u32 = 0x0000_0004;
    pub const PCRS: u32 = 0x0000_0005;
    pub const TPM_PROPERTIES: u32 = 0x0000_0006;
}

/// Table 30 (v184): `TPM_PT` property tags (PT_FIXED = 0x100, PT_VAR = 0x200).
pub mod pt {
    pub const FIXED: u32 = 0x0000_0100;
    pub const VAR: u32 = 0x0000_0200;
    pub const GROUP: u32 = 0x0000_0100;
    pub const FAMILY_INDICATOR: u32 = FIXED;
    pub const LEVEL: u32 = FIXED + 1;
    pub const REVISION: u32 = FIXED + 2;
    pub const DAY_OF_YEAR: u32 = FIXED + 3;
    pub const YEAR: u32 = FIXED + 4;
    pub const MANUFACTURER: u32 = FIXED + 5;
    pub const VENDOR_STRING_1: u32 = FIXED + 6;
    pub const VENDOR_STRING_2: u32 = FIXED + 7;
    pub const VENDOR_STRING_3: u32 = FIXED + 8;
    pub const VENDOR_STRING_4: u32 = FIXED + 9;
    pub const VENDOR_TPM_TYPE: u32 = FIXED + 10;
    pub const FIRMWARE_VERSION_1: u32 = FIXED + 11;
    pub const FIRMWARE_VERSION_2: u32 = FIXED + 12;
    pub const INPUT_BUFFER: u32 = FIXED + 13;
    pub const HR_TRANSIENT_MIN: u32 = FIXED + 14;
    pub const PCR_COUNT: u32 = FIXED + 18;
    pub const PCR_SELECT_MIN: u32 = FIXED + 19;
    pub const MAX_COMMAND_SIZE: u32 = FIXED + 30;
    pub const MAX_RESPONSE_SIZE: u32 = FIXED + 31;
    pub const MAX_DIGEST: u32 = FIXED + 32;
    pub const PS_FAMILY_INDICATOR: u32 = FIXED + 35;
    pub const PS_LEVEL: u32 = FIXED + 36;
    pub const PS_REVISION: u32 = FIXED + 37;
    pub const PS_DAY_OF_YEAR: u32 = FIXED + 38;
    pub const PS_YEAR: u32 = FIXED + 39;
    pub const TOTAL_COMMANDS: u32 = FIXED + 41;
    pub const LIBRARY_COMMANDS: u32 = FIXED + 42;
    pub const VENDOR_COMMANDS: u32 = FIXED + 43;
    pub const MAX_CAP_BUFFER: u32 = FIXED + 46;
    pub const HR_TRANSIENT_AVAIL: u32 = VAR + 7;
}

/// Table 8 (v184): `TPM_ALG_ID` values used in phase A.
pub mod alg {
    pub const SHA256: u16 = 0x000B;
    pub const NULL: u16 = 0x0010;
}

/// Table 24 (v184): `TPM_SU` startup/shutdown types.
pub mod su {
    pub const CLEAR: u16 = 0x0000;
    pub const STATE: u16 = 0x0001;
}

/// Table 35/36 (v184): handle types and permanent handles.
pub mod handle {
    pub const HT_PCR: u8 = 0x00;
    pub const HT_TRANSIENT: u8 = 0x80;
    pub const HT_PERSISTENT: u8 = 0x81;
    pub const RH_OWNER: u32 = 0x4000_0001;
    pub const RH_NULL: u32 = 0x4000_0007;
    pub const RS_PW: u32 = 0x4000_0009;
    pub const RH_ENDORSEMENT: u32 = 0x4000_000B;
    pub const RH_PLATFORM: u32 = 0x4000_000C;

    /// Most significant octet of a handle (`TPM_HT`).
    pub const fn handle_type(h: u32) -> u8 {
        (h >> 24) as u8
    }
}

/// Table 3 (v184): `TPM_SPEC_*`. The project targets v185 (ADR-0009); `REVISION` reports 185.
/// NOT VERIFIED against TPM 2.0 Library v185: v185 `TPM_SPEC_YEAR`/`TPM_SPEC_DAY_OF_YEAR` and the
/// exact `TPM_PT_REVISION` encoding (v184 states version number = TPM_SPEC_VERSION from v184 on);
/// the v184 year/day (2025, 79) are used as placeholders.
pub mod spec {
    pub const FAMILY: u32 = 0x322E_3000;
    pub const LEVEL: u32 = 0;
    pub const REVISION: u32 = 185;
    pub const YEAR: u32 = 2025;
    pub const DAY_OF_YEAR: u32 = 79;
}
