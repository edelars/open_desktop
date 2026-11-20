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
    pub const NV_UNDEFINE_SPACE_SPECIAL: u32 = 0x0000_011F;
    /// Slice C2 of phase B (S7-23): attestation, policy, context and `TPM2_HMAC` (Table 14 values,
    /// checked against the v184 text 2026-11-07).
    pub const CERTIFY: u32 = 0x0000_0148;
    pub const POLICY_NV: u32 = 0x0000_0149;
    pub const CERTIFY_CREATION: u32 = 0x0000_014A;
    pub const GET_TIME: u32 = 0x0000_014C;
    pub const GET_SESSION_AUDIT_DIGEST: u32 = 0x0000_014D;
    pub const NV_INCREMENT: u32 = 0x0000_0134;
    pub const NV_SET_BITS: u32 = 0x0000_0135;
    pub const NV_EXTEND: u32 = 0x0000_0136;
    pub const NV_CHANGE_AUTH: u32 = 0x0000_013B;
    pub const POLICY_SECRET: u32 = 0x0000_0151;
    pub const POLICY_SIGNED: u32 = 0x0000_0160;
    pub const HMAC: u32 = 0x0000_0155;
    pub const IMPORT: u32 = 0x0000_0156;
    pub const DUPLICATE: u32 = 0x0000_014B;
    pub const HMAC_START: u32 = 0x0000_015B;
    pub const QUOTE: u32 = 0x0000_0158;
    pub const CONTEXT_LOAD: u32 = 0x0000_0161;
    pub const CONTEXT_SAVE: u32 = 0x0000_0162;
    pub const POLICY_AUTHORIZE: u32 = 0x0000_016A;
    pub const POLICY_AUTH_VALUE: u32 = 0x0000_016B;
    pub const POLICY_COMMAND_CODE: u32 = 0x0000_016C;
    pub const POLICY_COUNTER_TIMER: u32 = 0x0000_016D;
    pub const POLICY_CP_HASH: u32 = 0x0000_016E;
    pub const POLICY_LOCALITY: u32 = 0x0000_016F;
    pub const POLICY_NAME_HASH: u32 = 0x0000_0170;
    pub const POLICY_OR: u32 = 0x0000_0171;
    pub const POLICY_TICKET: u32 = 0x0000_0172;
    pub const POLICY_PCR: u32 = 0x0000_017F;
    pub const POLICY_RESTART: u32 = 0x0000_0180;
    pub const NV_CERTIFY: u32 = 0x0000_0184;
    pub const POLICY_GET_DIGEST: u32 = 0x0000_0189;
    pub const POLICY_DUPLICATION_SELECT: u32 = 0x0000_0188;
    pub const POLICY_PASSWORD: u32 = 0x0000_018C;
    pub const POLICY_NV_WRITTEN: u32 = 0x0000_018F;
    pub const POLICY_TEMPLATE: u32 = 0x0000_0190;
    pub const POLICY_AUTHORIZE_NV: u32 = 0x0000_0192;
    pub const POLICY_PARAMETERS: u32 = 0x0000_019C;
    /// Slice B of phase B (S7-23): object commands (Table 14 values, checked 2026-11-07).
    pub const DICTIONARY_ATTACK_LOCK_RESET: u32 = 0x0000_0139;
    pub const DICTIONARY_ATTACK_PARAMETERS: u32 = 0x0000_013A;
    pub const ECDH_Z_GEN: u32 = 0x0000_0154;
    pub const ECDH_KEY_GEN: u32 = 0x0000_0163;
    pub const ECC_PARAMETERS: u32 = 0x0000_0178;
    pub const RSA_DECRYPT: u32 = 0x0000_0159;
    pub const RSA_ENCRYPT: u32 = 0x0000_0174;
    pub const EVICT_CONTROL: u32 = 0x0000_0120;
    pub const HIERARCHY_CONTROL: u32 = 0x0000_0121;
    pub const NV_UNDEFINE_SPACE: u32 = 0x0000_0122;
    pub const CLEAR: u32 = 0x0000_0126;
    pub const CLEAR_CONTROL: u32 = 0x0000_0127;
    pub const CLOCK_SET: u32 = 0x0000_0128;
    pub const HIERARCHY_CHANGE_AUTH: u32 = 0x0000_0129;
    pub const SET_PRIMARY_POLICY: u32 = 0x0000_012E;
    pub const CLOCK_RATE_ADJUST: u32 = 0x0000_0130;
    pub const CREATE_PRIMARY: u32 = 0x0000_0131;
    pub const NV_DEFINE_SPACE: u32 = 0x0000_012A;
    pub const PCR_ALLOCATE: u32 = 0x0000_012B;
    pub const NV_WRITE: u32 = 0x0000_0137;
    pub const NV_WRITE_LOCK: u32 = 0x0000_0138;
    pub const PCR_EVENT: u32 = 0x0000_013C;
    pub const PCR_RESET: u32 = 0x0000_013D;
    pub const SEQUENCE_COMPLETE: u32 = 0x0000_013E;
    pub const INCREMENTAL_SELF_TEST: u32 = 0x0000_0142;
    pub const SELF_TEST: u32 = 0x0000_0143;
    pub const STARTUP: u32 = 0x0000_0144;
    pub const SHUTDOWN: u32 = 0x0000_0145;
    pub const STIR_RANDOM: u32 = 0x0000_0146;
    pub const ACTIVATE_CREDENTIAL: u32 = 0x0000_0147;
    pub const NV_READ: u32 = 0x0000_014E;
    pub const NV_READ_LOCK: u32 = 0x0000_014F;
    pub const OBJECT_CHANGE_AUTH: u32 = 0x0000_0150;
    pub const CREATE: u32 = 0x0000_0153;
    pub const LOAD: u32 = 0x0000_0157;
    pub const SEQUENCE_UPDATE: u32 = 0x0000_015C;
    pub const SIGN: u32 = 0x0000_015D;
    pub const SIGN_SEQUENCE_COMPLETE: u32 = 0x0000_01A4;
    pub const SIGN_DIGEST: u32 = 0x0000_01A6;
    pub const SIGN_SEQUENCE_START: u32 = 0x0000_01AA;
    pub const ENCAPSULATE: u32 = 0x0000_01A7;
    pub const DECAPSULATE: u32 = 0x0000_01A8;
    pub const UNSEAL: u32 = 0x0000_015E;
    pub const FLUSH_CONTEXT: u32 = 0x0000_0165;
    pub const LOAD_EXTERNAL: u32 = 0x0000_0167;
    pub const MAKE_CREDENTIAL: u32 = 0x0000_0168;
    pub const NV_READ_PUBLIC: u32 = 0x0000_0169;
    pub const READ_PUBLIC: u32 = 0x0000_0173;
    pub const START_AUTH_SESSION: u32 = 0x0000_0176;
    pub const VERIFY_SIGNATURE: u32 = 0x0000_0177;
    pub const VERIFY_SEQUENCE_COMPLETE: u32 = 0x0000_01A3;
    pub const VERIFY_DIGEST_SIGNATURE: u32 = 0x0000_01A5;
    pub const VERIFY_SEQUENCE_START: u32 = 0x0000_01A9;
    pub const GET_CAPABILITY: u32 = 0x0000_017A;
    pub const GET_RANDOM: u32 = 0x0000_017B;
    pub const GET_TEST_RESULT: u32 = 0x0000_017C;
    pub const HASH: u32 = 0x0000_017D;
    pub const PCR_READ: u32 = 0x0000_017E;
    pub const READ_CLOCK: u32 = 0x0000_0181;
    pub const PCR_EXTEND: u32 = 0x0000_0182;
    pub const EVENT_SEQUENCE_COMPLETE: u32 = 0x0000_0185;
    pub const HASH_SEQUENCE_START: u32 = 0x0000_0186;
    pub const TEST_PARMS: u32 = 0x0000_018A;
}

/// Table 23 (v184): `TPM_ST` structure tags.
pub mod st {
    pub const NO_SESSIONS: u16 = 0x8001;
    pub const SESSIONS: u16 = 0x8002;
    /// Tickets (Part 2 v185, Table 23).
    pub const CREATION: u16 = 0x8021;
    pub const VERIFIED: u16 = 0x8022;
    pub const MESSAGE_VERIFIED: u16 = 0x8026;
    pub const DIGEST_VERIFIED: u16 = 0x8027;
    pub const HASHCHECK: u16 = 0x8024;
    /// Attestation structure tags (Table 23).
    pub const ATTEST_NV: u16 = 0x8014;
    pub const ATTEST_SESSION_AUDIT: u16 = 0x8016;
    pub const ATTEST_CERTIFY: u16 = 0x8017;
    pub const ATTEST_QUOTE: u16 = 0x8018;
    /// `TPM2_GetTime` (Table 23).
    pub const ATTEST_TIME: u16 = 0x8019;
    pub const ATTEST_CREATION: u16 = 0x801A;
    pub const ATTEST_NV_DIGEST: u16 = 0x801C;
    /// Ticket tag of `TPM2_PolicySecret` (Table 23).
    pub const AUTH_SECRET: u16 = 0x8023;
    pub const AUTH_SIGNED: u16 = 0x8025;
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
    pub const PCR_PROPERTIES: u32 = 0x0000_0007;
    pub const ECC_CURVES: u32 = 0x0000_0008;
    pub const AUTH_POLICIES: u32 = 0x0000_0009;
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
    pub const HR_LOADED_MIN: u32 = FIXED + 16;
    pub const ACTIVE_SESSIONS_MAX: u32 = FIXED + 17;
    pub const PCR_COUNT: u32 = FIXED + 18;
    pub const PCR_SELECT_MIN: u32 = FIXED + 19;
    pub const CONTEXT_GAP_MAX: u32 = FIXED + 20;
    pub const NV_COUNTERS_MAX: u32 = FIXED + 22;
    pub const CLOCK_UPDATE: u32 = FIXED + 25;
    pub const CONTEXT_HASH: u32 = FIXED + 26;
    pub const CONTEXT_SYM: u32 = FIXED + 27;
    pub const CONTEXT_SYM_SIZE: u32 = FIXED + 28;
    pub const ORDERLY_COUNT: u32 = FIXED + 29;
    pub const MAX_OBJECT_CONTEXT: u32 = FIXED + 33;
    pub const MAX_SESSION_CONTEXT: u32 = FIXED + 34;
    pub const SPLIT_MAX: u32 = FIXED + 40;
    pub const MODES: u32 = FIXED + 45;
    pub const MAX_COMMAND_SIZE: u32 = FIXED + 30;
    pub const MAX_RESPONSE_SIZE: u32 = FIXED + 31;
    pub const MAX_DIGEST: u32 = FIXED + 32;
    pub const NV_INDEX_MAX: u32 = FIXED + 23;
    pub const PS_FAMILY_INDICATOR: u32 = FIXED + 35;
    pub const PS_LEVEL: u32 = FIXED + 36;
    pub const PS_REVISION: u32 = FIXED + 37;
    pub const PS_DAY_OF_YEAR: u32 = FIXED + 38;
    pub const PS_YEAR: u32 = FIXED + 39;
    pub const TOTAL_COMMANDS: u32 = FIXED + 41;
    pub const LIBRARY_COMMANDS: u32 = FIXED + 42;
    pub const VENDOR_COMMANDS: u32 = FIXED + 43;
    pub const NV_BUFFER_MAX: u32 = FIXED + 44;
    pub const MAX_CAP_BUFFER: u32 = FIXED + 46;
    pub const ML_PARAMETER_SETS: u32 = FIXED + 49;
    pub const PERMANENT: u32 = VAR;
    pub const STARTUP_CLEAR: u32 = VAR + 1;
    pub const HR_NV_INDEX: u32 = VAR + 2;
    pub const HR_LOADED: u32 = VAR + 3;
    pub const HR_LOADED_AVAIL: u32 = VAR + 4;
    pub const HR_ACTIVE: u32 = VAR + 5;
    pub const HR_ACTIVE_AVAIL: u32 = VAR + 6;
    pub const HR_TRANSIENT_AVAIL: u32 = VAR + 7;
    pub const HR_PERSISTENT: u32 = VAR + 8;
    pub const HR_PERSISTENT_AVAIL: u32 = VAR + 9;
    pub const NV_COUNTERS: u32 = VAR + 10;
    pub const NV_COUNTERS_AVAIL: u32 = VAR + 11;
    pub const LOADED_CURVES: u32 = VAR + 13;
    pub const HR_PERSISTENT_MIN: u32 = FIXED + 15;
    /// Dictionary attack properties (Part 2 v184, Table 30): `failedTries`, `maxTries`,
    /// `recoveryTime` (seconds), `lockoutRecovery` (seconds).
    pub const LOCKOUT_COUNTER: u32 = VAR + 14;
    pub const MAX_AUTH_FAIL: u32 = VAR + 15;
    pub const LOCKOUT_INTERVAL: u32 = VAR + 16;
    pub const LOCKOUT_RECOVERY: u32 = VAR + 17;
    pub const NV_WRITE_RECOVERY: u32 = VAR + 18;
    pub const AUDIT_COUNTER_0: u32 = VAR + 19;
    pub const AUDIT_COUNTER_1: u32 = VAR + 20;
}

/// Table 8 (v184): `TPM_ALG_ID` values used in phase A.
pub mod alg {
    pub const RSA: u16 = 0x0001;
    /// `TPM_ALG_HMAC`: the scheme of a keyed-hash signing key (`TPM2_HMAC`, slice C2).
    pub const HMAC: u16 = 0x0005;
    pub const KEYEDHASH: u16 = 0x0008;
    pub const SHA256: u16 = 0x000B;
    pub const SHA384: u16 = 0x000C;
    pub const SHA512: u16 = 0x000D;
    pub const NULL: u16 = 0x0010;
    pub const RSASSA: u16 = 0x0014;
    /// RSA encryption padding schemes (`TPM2_RSA_Encrypt/Decrypt`) and the ECDH key exchange.
    pub const RSAES: u16 = 0x0015;
    /// `TPM_ALG_RSAPSS`: RSASSA-PSS (RFC 8017, 8.1) signed with a salt as long as the digest (Part 1, Annex B.7); verification accepts any salt length, like the reference TPM.
    pub const RSAPSS: u16 = 0x0016;
    pub const OAEP: u16 = 0x0017;
    pub const ECDSA: u16 = 0x0018;
    pub const ECDH: u16 = 0x0019;
    pub const KDF1_SP800_56A: u16 = 0x0020;
    pub const ECC: u16 = 0x0023;
    /// `TPM_ALG_SYMCIPHER`: the object type of a symmetric block cipher key (Part 2 v185, Table 8).
    pub const SYMCIPHER: u16 = 0x0025;
    pub const MLKEM: u16 = 0x00A0;
    /// The symmetric algorithm of a storage key template; protects `outPrivate` (`object.rs`).
    pub const AES: u16 = 0x0006;
    pub const CFB: u16 = 0x0043;
    /// `TPM_ALG_XOR`: the XOR obfuscation of parameters (`TPMT_SYM_DEF` of a session).
    pub const XOR: u16 = 0x000A;
}

/// Table 12 (v184): `TPM_ECC_CURVE`.
pub mod curve {
    pub const NIST_P256: u16 = 0x0003;
    pub const NIST_P384: u16 = 0x0004;
}

/// Table 39 (v184): `TPMA_OBJECT` bits.
pub mod object_attr {
    pub const FIXED_TPM: u32 = 1 << 1;
    pub const ST_CLEAR: u32 = 1 << 2;
    pub const FIXED_PARENT: u32 = 1 << 4;
    pub const SENSITIVE_DATA_ORIGIN: u32 = 1 << 5;
    pub const USER_WITH_AUTH: u32 = 1 << 6;
    pub const ADMIN_WITH_POLICY: u32 = 1 << 7;
    pub const FIRMWARE_LIMITED: u32 = 1 << 8;
    pub const SVN_LIMITED: u32 = 1 << 9;
    pub const NO_DA: u32 = 1 << 10;
    pub const ENCRYPTED_DUPLICATION: u32 = 1 << 11;
    pub const RESTRICTED: u32 = 1 << 16;
    pub const DECRYPT: u32 = 1 << 17;
    pub const SIGN: u32 = 1 << 18;
    pub const X509_SIGN: u32 = 1 << 19;
    /// Reserved bits: 0, 3, 15:12, 31:20.
    pub const RESERVED: u32 = 0x1 | 0x8 | 0xF000 | 0xFFF0_0000;
}

/// Table 24 (v184): `TPM_SU` startup/shutdown types.
pub mod su {
    pub const CLEAR: u16 = 0x0000;
    pub const STATE: u16 = 0x0001;
}

/// Table 25 (v184): `TPM_SE` session types.
pub mod se {
    pub const HMAC: u8 = 0x00;
    pub const POLICY: u8 = 0x01;
    pub const TRIAL: u8 = 0x03;
}

/// Table 40 (v184): `TPMA_SESSION` bits.
pub mod session_attr {
    pub const CONTINUE_SESSION: u8 = 1 << 0;
    pub const AUDIT_EXCLUSIVE: u8 = 1 << 1;
    pub const AUDIT_RESET: u8 = 1 << 2;
    pub const RESERVED: u8 = (1 << 3) | (1 << 4);
    pub const DECRYPT: u8 = 1 << 5;
    pub const ENCRYPT: u8 = 1 << 6;
    pub const AUDIT: u8 = 1 << 7;
}

/// Table 35/36 (v184): handle types and permanent handles.
pub mod handle {
    pub const HT_PCR: u8 = 0x00;
    pub const HT_NV_INDEX: u8 = 0x01;
    /// `TPM_HT_HMAC_SESSION` (also `TPM_HT_LOADED_SESSION` in `TPM2_GetCapability`).
    pub const HT_HMAC_SESSION: u8 = 0x02;
    /// `TPM_HT_POLICY_SESSION` (also `TPM_HT_SAVED_SESSION` in `TPM2_GetCapability`).
    pub const HT_POLICY_SESSION: u8 = 0x03;
    pub const HT_PERMANENT: u8 = 0x40;
    pub const HT_TRANSIENT: u8 = 0x80;
    pub const HT_PERSISTENT: u8 = 0x81;
    /// `PERSISTENT_FIRST`, `PLATFORM_PERSISTENT`, `PERSISTENT_LAST` (Part 2, Table 36).
    pub const PERSISTENT_FIRST: u32 = 0x8100_0000;
    pub const PLATFORM_PERSISTENT: u32 = 0x8180_0000;
    pub const PERSISTENT_LAST: u32 = 0x81FF_FFFF;
    pub const RH_OWNER: u32 = 0x4000_0001;
    pub const RH_NULL: u32 = 0x4000_0007;
    pub const RS_PW: u32 = 0x4000_0009;
    pub const RH_LOCKOUT: u32 = 0x4000_000A;
    pub const RH_ENDORSEMENT: u32 = 0x4000_000B;
    pub const RH_PLATFORM: u32 = 0x4000_000C;
    pub const RH_PLATFORM_NV: u32 = 0x4000_000D;

    /// Most significant octet of a handle (`TPM_HT`).
    pub const fn handle_type(h: u32) -> u8 {
        (h >> 24) as u8
    }
}

/// Part 2 v184, `TPM_CONSTANTS32`: `TPM_GENERATED_VALUE`, the first field of every attestation
/// structure.
pub const TPM_GENERATED_VALUE: u32 = 0xFF54_4347;

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
