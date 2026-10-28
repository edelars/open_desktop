//! Strict long-mode integer fixtures/results. V1 readers and digests are unchanged.

use crate::case::{
    MAX_INSTRUCTION_BUDGET, MAX_WALL_TIMEOUT_MS, MemoryComparisonScope, MemoryPermissions,
    MemoryRegionSpec, Provenance, validate_case_id, validate_hex_bytes,
};
use crate::result::{
    Backend, BuildMetadata, ExceptionSnapshot, MachineMetadata, MemoryRegionSnapshot, Observation,
};
use crate::{FormatError, HexU64, MAX_RESULT_JSON_BYTES, sha256_hex};
use od_isa::LongModeState;
use od_isa::OperandSize;
use od_isa::long_decode::Operation;
use od_isa::memory::{AccessContext, AccessKind, GuestMemory, MemoryError};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub const SCHEMA_VERSION: u32 = 2;
/// Accommodates 64 KiB of bytes plus per-byte comparison masks and metadata.
pub const MAX_FIXTURE_JSON_BYTES: usize = 512 * 1024;
pub const PROFILE: &str = "s2-long-integer-v1";
pub const PROFILE_FS_BASE: &str = "s2-long-fs-base-v1";
pub const PROFILE_GS_BASE: &str = "s2-long-gs-base-v1";
pub const PROFILE_FS_SELECTOR: &str = "s2-long-fs-selector-v1";
pub const PROFILE_GS_SELECTOR: &str = "s2-long-gs-selector-v1";
pub const PROFILE_VERW_GDT: &str = "s2-long-verw-gdt-v1";
pub const PROFILE_FS_LOAD: &str = "s2-long-fs-load-v1";
pub const PROFILE_GS_LOAD: &str = "s2-long-gs-load-v1";
pub const PROFILE_NULL_DS_ES: &str = "s2-long-null-ds-es-v1";
pub const PROFILE_X87_STATE: &str = "s2-long-x87-state-v1";
pub const PROFILE_X87_SSE_STATE: &str = "s2-long-x87-sse-state-v1";
pub const PROFILE_MOVUPS_REGISTER: &str = "s2-long-movups-reg-v1";
pub const PROFILE_XORPS_REGISTER: &str = "s2-long-xorps-reg-v1";
pub const PROFILE_PXOR_REGISTER: &str = "s2-long-pxor-reg-v1";
pub const PROFILE_PADDB_REGISTER: &str = "s2-long-paddb-reg-v1";
pub const PROFILE_PADDUSW_REGISTER: &str = "s2-long-paddusw-reg-v1";
pub const PROFILE_PSUBD_REGISTER: &str = "s2-long-psubd-reg-v1";
pub const PROFILE_PSUBQ_REGISTER: &str = "s2-long-psubq-reg-v1";
pub const PROFILE_PCMPEQB_REGISTER: &str = "s2-long-pcmpeqb-reg-v1";
pub const PROFILE_PCMPEQW_REGISTER: &str = "s2-long-pcmpeqw-reg-v1";
pub const PROFILE_PCMPEQD_REGISTER: &str = "s2-long-pcmpeqd-reg-v1";
pub const PROFILE_PCMPGTB_REGISTER: &str = "s2-long-pcmpgtb-reg-v1";
pub const PROFILE_PCMPGTW_REGISTER: &str = "s2-long-pcmpgtw-reg-v1";
pub const PROFILE_PCMPGTD_REGISTER: &str = "s2-long-pcmpgtd-reg-v1";
pub const PROFILE_PUNPCKLBW_REGISTER: &str = "s2-long-punpcklbw-reg-v1";
pub const PROFILE_PUNPCKHBW_REGISTER: &str = "s2-long-punpckhbw-reg-v1";
pub const PROFILE_PUNPCKLWD_REGISTER: &str = "s2-long-punpcklwd-reg-v1";
pub const PROFILE_PUNPCKHWD_REGISTER: &str = "s2-long-punpckhwd-reg-v1";
pub const PROFILE_PUNPCKLDQ_REGISTER: &str = "s2-long-punpckldq-reg-v1";
pub const PROFILE_PUNPCKHDQ_REGISTER: &str = "s2-long-punpckhdq-reg-v1";
pub const PROFILE_PUNPCKLQDQ_REGISTER: &str = "s2-long-punpcklqdq-reg-v1";
pub const PROFILE_PSUBUSW_REGISTER: &str = "s2-long-psubusw-reg-v1";
pub const PROFILE_PSUBW_REGISTER: &str = "s2-long-psubw-reg-v1";
pub const PROFILE_PSUBB_REGISTER: &str = "s2-long-psubb-reg-v1";
pub const PROFILE_PADDUSB_REGISTER: &str = "s2-long-paddusb-reg-v1";
pub const PROFILE_PADDSB_REGISTER: &str = "s2-long-paddsb-reg-v1";
pub const PROFILE_PSUBSB_REGISTER: &str = "s2-long-psubsb-reg-v1";
pub const PROFILE_PADDSW_REGISTER: &str = "s2-long-paddsw-reg-v1";
pub const PROFILE_PSUBSW_REGISTER: &str = "s2-long-psubsw-reg-v1";
pub const PROFILE_PSUBUSB_REGISTER: &str = "s2-long-psubusb-reg-v1";
pub const PROFILE_ANDPS_REGISTER: &str = "s2-long-andps-reg-v1";
pub const PROFILE_ORPS_REGISTER: &str = "s2-long-orps-reg-v1";
pub const PROFILE_ANDNPS_REGISTER: &str = "s2-long-andnps-reg-v1";
pub const PROFILE_MOVSS_REGISTER: &str = "s2-long-movss-reg-v1";
pub const PROFILE_MOVSD_REGISTER: &str = "s2-long-movsd-reg-v1";
pub const PROFILE_MOVSD_STORE_REGISTER: &str = "s2-long-movsd-store-reg-v1";
pub const PROFILE_MOVSD_MEMORY_LOAD: &str = "s2-long-movsd-mem-load-v1";
pub const PROFILE_MOVSS_STORE_REGISTER: &str = "s2-long-movss-store-reg-v1";
pub const PROFILE_MOVSS_MEMORY_STORE: &str = "s2-long-movss-mem-store-v1";
pub const PROFILE_MOVUPS_MEMORY_LOAD: &str = "s2-long-movups-mem-load-v1";
pub const PROFILE_MOVUPS_MEMORY_STORE: &str = "s2-long-movups-mem-store-v1";
pub const PROFILE_MOVUPS_MEMORY_BASE: &str = "s2-long-movups-mem-base-v1";
/// Diagnostic mask for this bounded local fixture profile only. It is not a
/// universal hardware CPUID mask and does not model guest #GP behavior.
pub const PROFILE_SSE_MXCSR_DIAGNOSTIC_MASK: u32 = 0x0000_ffbf;
/// Integer arithmetic flags, reserved bit 1 and DF. System flags are not yet modeled.
pub const INTEGER_FLAGS: u64 = 0x8d7 | (1 << 10);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Profile {
    #[serde(rename = "s2-long-integer-v1")]
    LongIntegerV1,
    #[serde(rename = "s2-long-fs-base-v1")]
    LongFsBaseV1,
    #[serde(rename = "s2-long-gs-base-v1")]
    LongGsBaseV1,
    #[serde(rename = "s2-long-fs-selector-v1")]
    LongFsSelectorV1,
    #[serde(rename = "s2-long-gs-selector-v1")]
    LongGsSelectorV1,
    #[serde(rename = "s2-long-verw-gdt-v1")]
    LongVerwGdtV1,
    #[serde(rename = "s2-long-fs-load-v1")]
    LongFsLoadV1,
    #[serde(rename = "s2-long-gs-load-v1")]
    LongGsLoadV1,
    #[serde(rename = "s2-long-null-ds-es-v1")]
    LongNullDsEsV1,
    #[serde(rename = "s2-long-x87-state-v1")]
    LongX87StateV1,
    #[serde(rename = "s2-long-x87-sse-state-v1")]
    LongX87SseStateV1,
    #[serde(rename = "s2-long-movups-reg-v1")]
    LongMovupsRegV1,
    #[serde(rename = "s2-long-xorps-reg-v1")]
    LongXorpsRegV1,
    #[serde(rename = "s2-long-pxor-reg-v1")]
    LongPxorRegV1,
    #[serde(rename = "s2-long-paddb-reg-v1")]
    LongPaddbRegV1,
    #[serde(rename = "s2-long-paddw-reg-v1")]
    LongPaddwRegV1,
    #[serde(rename = "s2-long-psubw-reg-v1")]
    LongPsubwRegV1,
    #[serde(rename = "s2-long-paddusw-reg-v1")]
    LongPadduswRegV1,
    #[serde(rename = "s2-long-psubusw-reg-v1")]
    LongPsubuswRegV1,
    #[serde(rename = "s2-long-psubb-reg-v1")]
    LongPsubbRegV1,
    #[serde(rename = "s2-long-paddusb-reg-v1")]
    LongPaddusbRegV1,
    #[serde(rename = "s2-long-paddsb-reg-v1")]
    LongPaddsbRegV1,
    #[serde(rename = "s2-long-psubsb-reg-v1")]
    LongPsubsbRegV1,
    #[serde(rename = "s2-long-paddsw-reg-v1")]
    LongPaddswRegV1,
    #[serde(rename = "s2-long-psubsw-reg-v1")]
    LongPsubswRegV1,
    #[serde(rename = "s2-long-paddd-reg-v1")]
    LongPadddRegV1,
    #[serde(rename = "s2-long-psubd-reg-v1")]
    LongPsubdRegV1,
    #[serde(rename = "s2-long-paddq-reg-v1")]
    LongPaddqRegV1,
    #[serde(rename = "s2-long-psubq-reg-v1")]
    LongPsubqRegV1,
    #[serde(rename = "s2-long-pcmpeqb-reg-v1")]
    LongPcmpeqbRegV1,
    #[serde(rename = "s2-long-pcmpeqw-reg-v1")]
    LongPcmpeqwRegV1,
    #[serde(rename = "s2-long-pcmpeqd-reg-v1")]
    LongPcmpeqdRegV1,
    #[serde(rename = "s2-long-pcmpgtb-reg-v1")]
    LongPcmpgtbRegV1,
    #[serde(rename = "s2-long-pcmpgtw-reg-v1")]
    LongPcmpgtwRegV1,
    #[serde(rename = "s2-long-pcmpgtd-reg-v1")]
    LongPcmpgtdRegV1,
    #[serde(rename = "s2-long-punpcklbw-reg-v1")]
    LongPunpcklbwRegV1,
    #[serde(rename = "s2-long-punpckhbw-reg-v1")]
    LongPunpckhbwRegV1,
    #[serde(rename = "s2-long-punpcklwd-reg-v1")]
    LongPunpcklwdRegV1,
    #[serde(rename = "s2-long-punpckhwd-reg-v1")]
    LongPunpckhwdRegV1,
    #[serde(rename = "s2-long-punpckldq-reg-v1")]
    LongPunpckldqRegV1,
    #[serde(rename = "s2-long-punpckhdq-reg-v1")]
    LongPunpckhdqRegV1,
    #[serde(rename = "s2-long-punpcklqdq-reg-v1")]
    LongPunpcklqdqRegV1,
    #[serde(rename = "s2-long-punpckhqdq-reg-v1")]
    LongPunpckhqdqRegV1,
    #[serde(rename = "s2-long-packsswb-reg-v1")]
    LongPacksswbRegV1,
    #[serde(rename = "s2-long-packuswb-reg-v1")]
    LongPackuswbRegV1,
    #[serde(rename = "s2-long-packssdw-reg-v1")]
    LongPackssdwRegV1,
    #[serde(rename = "s2-long-pshufd-reg-v1")]
    LongPshufdRegV1,
    #[serde(rename = "s2-long-psllw-imm-reg-v1")]
    LongPsllwImmRegV1,
    #[serde(rename = "s2-long-pslld-imm-reg-v1")]
    LongPslldImmRegV1,
    #[serde(rename = "s2-long-psrld-imm-reg-v1")]
    LongPsrldImmRegV1,
    #[serde(rename = "s2-long-psrlw-imm-reg-v1")]
    LongPsrlwImmRegV1,
    #[serde(rename = "s2-long-psraw-imm-reg-v1")]
    LongPsrawImmRegV1,
    #[serde(rename = "s2-long-psrad-imm-reg-v1")]
    LongPsradImmRegV1,
    #[serde(rename = "s2-long-psubusb-reg-v1")]
    LongPsubusbRegV1,
    #[serde(rename = "s2-long-andps-reg-v1")]
    LongAndpsRegV1,
    #[serde(rename = "s2-long-orps-reg-v1")]
    LongOrpsRegV1,
    #[serde(rename = "s2-long-andnps-reg-v1")]
    LongAndnpsRegV1,
    #[serde(rename = "s2-long-movss-reg-v1")]
    LongMovssRegV1,
    #[serde(rename = "s2-long-movsd-reg-v1")]
    LongMovsdRegV1,
    #[serde(rename = "s2-long-movsd-store-reg-v1")]
    LongMovsdStoreRegV1,
    #[serde(rename = "s2-long-movss-store-reg-v1")]
    LongMovssStoreRegV1,
    #[serde(rename = "s2-long-movss-mem-load-v1")]
    LongMovssMemLoadV1,
    #[serde(rename = "s2-long-movsd-mem-load-v1")]
    LongMovsdMemLoadV1,
    #[serde(rename = "s2-long-movsd-mem-store-v1")]
    LongMovsdMemStoreV1,
    #[serde(rename = "s2-long-movss-mem-store-v1")]
    LongMovssMemStoreV1,
    #[serde(rename = "s2-long-movups-mem-load-v1")]
    LongMovupsMemLoadV1,
    #[serde(rename = "s2-long-movups-mem-store-v1")]
    LongMovupsMemStoreV1,
    #[serde(rename = "s2-long-movups-mem-base-v1")]
    LongMovupsMemBaseV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Long64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IntegerState {
    pub mode: Mode,
    /// x86 encoding order: RAX,RCX,RDX,RBX,RSP,RBP,RSI,RDI,R8…R15.
    pub gpr: [HexU64; 16],
    pub rip: HexU64,
    pub rflags: HexU64,
    /// Present only in `s2-long-fs-base-v1`; omitted to preserve the old profile wire shape.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub fs_base: Option<HexU64>,
    /// Present only in `s2-long-gs-base-v1`; omitted for older profile wire shapes.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub gs_base: Option<HexU64>,
    /// Present only in `s2-long-fs-selector-v1` and constrained to a 16-bit selector.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub fs_selector: Option<HexU64>,
    /// Present only in `s2-long-gs-selector-v1` and constrained to a 16-bit selector.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub gs_selector: Option<HexU64>,
    /// Present only in `s2-long-null-ds-es-v1`.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub ds_selector: Option<HexU64>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub es_selector: Option<HexU64>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_bool",
        skip_serializing_if = "Option::is_none"
    )]
    pub ds_unusable: Option<bool>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_bool",
        skip_serializing_if = "Option::is_none"
    )]
    pub es_unusable: Option<bool>,
    /// Present only in the GDT-only VERW profile.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub gdtr_base: Option<HexU64>,
    /// Present only in the GDT-only VERW profile; constrained to 16 bits.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub gdtr_limit: Option<HexU64>,
    /// Present only in the local descriptor-loading profile; effective hidden limit.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub fs_cache_limit: Option<HexU64>,
    /// Present only in the local descriptor-loading profile; raw descriptor access byte.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub fs_cache_access: Option<HexU64>,
    /// Present only in the local descriptor-loading profile; descriptor flag nibble.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub fs_cache_flags: Option<HexU64>,
    /// Present only in the local MOV-GS-load profile.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub gs_cache_limit: Option<HexU64>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub gs_cache_access: Option<HexU64>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub gs_cache_flags: Option<HexU64>,
    /// Full x87 state only in `s2-long-x87-state-v1`; absent from older wire profiles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x87: Option<X87Snapshot>,
    /// Present only in `s2-long-x87-sse-state-v1`.
    #[serde(
        default,
        deserialize_with = "deserialize_present_sse_snapshot",
        skip_serializing_if = "Option::is_none"
    )]
    pub sse: Option<SseSnapshot>,
}

/// Lossless Long64 x87 observation. Register hex strings contain ten bytes in
/// increasing memory-address order; FIP/FDP are 64-bit offsets, FOP is 11 bits.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct X87Snapshot {
    pub registers: [String; 8],
    pub control_word: HexU64,
    pub status_word: HexU64,
    pub tag_word: HexU64,
    pub instruction_pointer: HexU64,
    pub data_pointer: HexU64,
    pub last_opcode: HexU64,
}

impl From<&od_isa::state::X87State> for X87Snapshot {
    fn from(state: &od_isa::state::X87State) -> Self {
        Self {
            registers: state
                .registers
                .map(|bytes| bytes.iter().map(|byte| format!("{byte:02x}")).collect()),
            control_word: HexU64(u64::from(state.control_word)),
            status_word: HexU64(u64::from(state.status_word)),
            tag_word: HexU64(u64::from(state.tag_word)),
            instruction_pointer: HexU64(state.instruction_pointer),
            data_pointer: HexU64(state.data_pointer),
            last_opcode: HexU64(u64::from(state.last_opcode)),
        }
    }
}

impl X87Snapshot {
    pub fn to_cpu(&self) -> Result<od_isa::state::X87State, FormatError> {
        self.validate()?;
        let mut registers = [[0; 10]; 8];
        for (source, destination) in self.registers.iter().zip(&mut registers) {
            for (index, byte) in destination.iter_mut().enumerate() {
                let pair = &source[index * 2..index * 2 + 2];
                *byte = u8::from_str_radix(pair, 16)
                    .map_err(|_| invalid("state.x87.registers", "invalid byte encoding"))?;
            }
        }
        Ok(od_isa::state::X87State {
            registers,
            control_word: self.control_word.get() as u16,
            status_word: self.status_word.get() as u16,
            tag_word: self.tag_word.get() as u16,
            instruction_pointer: self.instruction_pointer.get(),
            data_pointer: self.data_pointer.get(),
            last_opcode: self.last_opcode.get() as u16,
        })
    }

    fn validate(&self) -> Result<(), FormatError> {
        for register in &self.registers {
            if register.len() != 20
                || !register
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(invalid(
                    "state.x87.registers",
                    "each register must be exactly 20 lowercase hex digits",
                ));
            }
        }
        for (field, value) in [
            ("state.x87.control_word", self.control_word),
            ("state.x87.status_word", self.status_word),
            ("state.x87.tag_word", self.tag_word),
        ] {
            if value.get() > u16::MAX as u64 {
                return Err(invalid(field, "must fit in 16 bits"));
            }
        }
        if self.last_opcode.get() > 0x7ff {
            return Err(invalid("state.x87.last_opcode", "must fit in 11 bits"));
        }
        Ok(())
    }
}

/// Lossless Long64 SSE snapshot. `diagnostic_mxcsr_mask` is profile metadata,
/// not part of architectural state; `mxcsr` remains an unrestricted raw u32.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SseSnapshot {
    pub xmm: [String; 16],
    pub mxcsr: HexU64,
    pub diagnostic_mxcsr_mask: HexU64,
}

impl From<&od_isa::state::SseState> for SseSnapshot {
    fn from(state: &od_isa::state::SseState) -> Self {
        Self {
            xmm: state
                .xmm
                .map(|bytes| bytes.iter().map(|byte| format!("{byte:02x}")).collect()),
            mxcsr: HexU64(u64::from(state.mxcsr)),
            diagnostic_mxcsr_mask: HexU64(u64::from(PROFILE_SSE_MXCSR_DIAGNOSTIC_MASK)),
        }
    }
}

impl SseSnapshot {
    pub fn to_cpu(&self) -> Result<od_isa::state::SseState, FormatError> {
        self.validate()?;
        let mut xmm = [[0; 16]; 16];
        for (source, destination) in self.xmm.iter().zip(&mut xmm) {
            for (index, byte) in destination.iter_mut().enumerate() {
                let pair = &source[index * 2..index * 2 + 2];
                *byte = u8::from_str_radix(pair, 16)
                    .map_err(|_| invalid("state.sse.xmm", "invalid byte encoding"))?;
            }
        }
        Ok(od_isa::state::SseState {
            xmm,
            mxcsr: self.mxcsr.get() as u32,
        })
    }

    fn validate(&self) -> Result<(), FormatError> {
        for register in &self.xmm {
            if register.len() != 32
                || !register
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(invalid(
                    "state.sse.xmm",
                    "each register must be exactly 32 lowercase hex digits",
                ));
            }
        }
        if self.mxcsr.get() > u32::MAX as u64 {
            return Err(invalid("state.sse.mxcsr", "must fit in 32 bits"));
        }
        if self.diagnostic_mxcsr_mask.get() != u64::from(PROFILE_SSE_MXCSR_DIAGNOSTIC_MASK) {
            return Err(invalid(
                "state.sse.diagnostic_mxcsr_mask",
                "must equal the bounded profile metadata mask 0x0000ffbf",
            ));
        }
        Ok(())
    }
}

fn deserialize_present_hex_u64<'de, D>(deserializer: D) -> Result<Option<HexU64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    HexU64::deserialize(deserializer).map(Some)
}

fn deserialize_present_sse_snapshot<'de, D>(
    deserializer: D,
) -> Result<Option<SseSnapshot>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    SseSnapshot::deserialize(deserializer).map(Some)
}

fn deserialize_present_bool<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    bool::deserialize(deserializer).map(Some)
}

impl From<&LongModeState> for IntegerState {
    fn from(state: &LongModeState) -> Self {
        Self {
            mode: Mode::Long64,
            gpr: state.gpr.map(HexU64),
            rip: HexU64(state.rip),
            rflags: HexU64(state.rflags),
            fs_base: None,
            gs_base: None,
            fs_selector: None,
            gs_selector: None,
            ds_selector: None,
            es_selector: None,
            ds_unusable: None,
            es_unusable: None,
            gdtr_base: None,
            gdtr_limit: None,
            fs_cache_limit: None,
            fs_cache_access: None,
            fs_cache_flags: None,
            gs_cache_limit: None,
            gs_cache_access: None,
            gs_cache_flags: None,
            x87: None,
            sse: None,
        }
    }
}

impl IntegerState {
    pub fn to_cpu(&self) -> Result<LongModeState, FormatError> {
        self.to_cpu_for(Profile::LongIntegerV1)
    }

    pub fn to_cpu_for(&self, profile: Profile) -> Result<LongModeState, FormatError> {
        self.validate_for(profile)?;
        Ok(LongModeState {
            gpr: self.gpr.map(HexU64::get),
            rip: self.rip.get(),
            rflags: self.rflags.get(),
            fs_base: self.fs_base.map_or(0, HexU64::get),
            gs_base: self.gs_base.map_or(0, HexU64::get),
            fs_selector: self.fs_selector.map_or(0, |selector| selector.get() as u16),
            gs_selector: self.gs_selector.map_or(0, |selector| selector.get() as u16),
            ds_selector: self.ds_selector.map_or(0, |selector| selector.get() as u16),
            es_selector: self.es_selector.map_or(0, |selector| selector.get() as u16),
            ds_unusable: self.ds_unusable.unwrap_or(false),
            es_unusable: self.es_unusable.unwrap_or(false),
            gdtr: match (self.gdtr_base, self.gdtr_limit) {
                (Some(base), Some(limit)) => Some(od_isa::state::Gdtr {
                    base: base.get(),
                    limit: limit.get() as u16,
                }),
                _ => None,
            },
            fs_cache: match (
                self.fs_cache_limit,
                self.fs_cache_access,
                self.fs_cache_flags,
            ) {
                (Some(limit), Some(access), Some(flags)) => {
                    Some(od_isa::state::FsDescriptorCache {
                        limit: limit.get() as u32,
                        access: access.get() as u8,
                        flags: flags.get() as u8,
                    })
                }
                _ => None,
            },
            gs_cache: match (
                self.gs_cache_limit,
                self.gs_cache_access,
                self.gs_cache_flags,
            ) {
                (Some(limit), Some(access), Some(flags)) => {
                    Some(od_isa::state::FsDescriptorCache {
                        limit: limit.get() as u32,
                        access: access.get() as u8,
                        flags: flags.get() as u8,
                    })
                }
                _ => None,
            },
            x87: match (profile, &self.x87) {
                (Profile::LongX87StateV1, Some(x87)) => Some(x87.to_cpu()?),
                (
                    Profile::LongX87SseStateV1
                    | Profile::LongMovupsRegV1
                    | Profile::LongXorpsRegV1
                    | Profile::LongPxorRegV1
                    | Profile::LongPaddbRegV1
                    | Profile::LongPaddwRegV1
                    | Profile::LongPsubwRegV1
                    | Profile::LongPadduswRegV1
                    | Profile::LongPsubuswRegV1
                    | Profile::LongPsubbRegV1
                    | Profile::LongPaddusbRegV1
                    | Profile::LongPaddsbRegV1
                    | Profile::LongPsubsbRegV1
                    | Profile::LongPaddswRegV1
                    | Profile::LongPsubswRegV1
                    | Profile::LongPadddRegV1
                    | Profile::LongPsubdRegV1
                    | Profile::LongPaddqRegV1
                    | Profile::LongPsubqRegV1
                    | Profile::LongPcmpeqbRegV1
                    | Profile::LongPcmpeqwRegV1
                    | Profile::LongPcmpeqdRegV1
                    | Profile::LongPcmpgtbRegV1
                    | Profile::LongPcmpgtwRegV1
                    | Profile::LongPcmpgtdRegV1
                    | Profile::LongPunpcklbwRegV1
                    | Profile::LongPunpckhbwRegV1
                    | Profile::LongPunpcklwdRegV1
                    | Profile::LongPunpckhwdRegV1
                    | Profile::LongPunpckldqRegV1
                    | Profile::LongPunpckhdqRegV1
                    | Profile::LongPunpcklqdqRegV1
                    | Profile::LongPunpckhqdqRegV1
                    | Profile::LongPacksswbRegV1
                    | Profile::LongPackuswbRegV1
                    | Profile::LongPackssdwRegV1
                    | Profile::LongPshufdRegV1
                    | Profile::LongPsllwImmRegV1
                    | Profile::LongPslldImmRegV1
                    | Profile::LongPsrldImmRegV1
                    | Profile::LongPsrlwImmRegV1
                    | Profile::LongPsrawImmRegV1
                    | Profile::LongPsradImmRegV1
                    | Profile::LongPsubusbRegV1
                    | Profile::LongAndpsRegV1
                    | Profile::LongOrpsRegV1
                    | Profile::LongAndnpsRegV1
                    | Profile::LongMovsdRegV1
                    | Profile::LongMovsdStoreRegV1
                    | Profile::LongMovssRegV1
                    | Profile::LongMovssMemLoadV1
                    | Profile::LongMovsdMemLoadV1
                    | Profile::LongMovsdMemStoreV1
                    | Profile::LongMovssMemStoreV1
                    | Profile::LongMovssStoreRegV1
                    | Profile::LongMovupsMemLoadV1
                    | Profile::LongMovupsMemStoreV1
                    | Profile::LongMovupsMemBaseV1,
                    Some(x87),
                ) => Some(x87.to_cpu()?),
                _ => None,
            },
            sse: match (profile, &self.sse) {
                (
                    Profile::LongX87SseStateV1
                    | Profile::LongMovupsRegV1
                    | Profile::LongXorpsRegV1
                    | Profile::LongPxorRegV1
                    | Profile::LongPaddbRegV1
                    | Profile::LongPaddwRegV1
                    | Profile::LongPsubwRegV1
                    | Profile::LongPadduswRegV1
                    | Profile::LongPsubuswRegV1
                    | Profile::LongPsubbRegV1
                    | Profile::LongPaddusbRegV1
                    | Profile::LongPaddsbRegV1
                    | Profile::LongPsubsbRegV1
                    | Profile::LongPaddswRegV1
                    | Profile::LongPsubswRegV1
                    | Profile::LongPadddRegV1
                    | Profile::LongPsubdRegV1
                    | Profile::LongPaddqRegV1
                    | Profile::LongPsubqRegV1
                    | Profile::LongPcmpeqbRegV1
                    | Profile::LongPcmpeqwRegV1
                    | Profile::LongPcmpeqdRegV1
                    | Profile::LongPcmpgtbRegV1
                    | Profile::LongPcmpgtwRegV1
                    | Profile::LongPcmpgtdRegV1
                    | Profile::LongPunpcklbwRegV1
                    | Profile::LongPunpckhbwRegV1
                    | Profile::LongPunpcklwdRegV1
                    | Profile::LongPunpckhwdRegV1
                    | Profile::LongPunpckldqRegV1
                    | Profile::LongPunpckhdqRegV1
                    | Profile::LongPunpcklqdqRegV1
                    | Profile::LongPunpckhqdqRegV1
                    | Profile::LongPacksswbRegV1
                    | Profile::LongPackuswbRegV1
                    | Profile::LongPackssdwRegV1
                    | Profile::LongPshufdRegV1
                    | Profile::LongPsllwImmRegV1
                    | Profile::LongPslldImmRegV1
                    | Profile::LongPsrldImmRegV1
                    | Profile::LongPsrlwImmRegV1
                    | Profile::LongPsrawImmRegV1
                    | Profile::LongPsradImmRegV1
                    | Profile::LongPsubusbRegV1
                    | Profile::LongAndpsRegV1
                    | Profile::LongOrpsRegV1
                    | Profile::LongAndnpsRegV1
                    | Profile::LongMovsdRegV1
                    | Profile::LongMovsdStoreRegV1
                    | Profile::LongMovssRegV1
                    | Profile::LongMovssMemLoadV1
                    | Profile::LongMovsdMemLoadV1
                    | Profile::LongMovsdMemStoreV1
                    | Profile::LongMovssMemStoreV1
                    | Profile::LongMovssStoreRegV1
                    | Profile::LongMovupsMemLoadV1
                    | Profile::LongMovupsMemStoreV1
                    | Profile::LongMovupsMemBaseV1,
                    Some(sse),
                ) => Some(sse.to_cpu()?),
                _ => None,
            },
        })
    }

    pub fn from_cpu_for(state: &LongModeState, profile: Profile) -> Self {
        let mut result = Self::from(state);
        result.x87 = if profile == Profile::LongX87StateV1 {
            state.x87.as_ref().map(X87Snapshot::from)
        } else if matches!(
            profile,
            Profile::LongX87SseStateV1
                | Profile::LongMovupsRegV1
                | Profile::LongXorpsRegV1
                | Profile::LongPxorRegV1
                | Profile::LongPaddbRegV1
                | Profile::LongPaddwRegV1
                | Profile::LongPsubwRegV1
                | Profile::LongPadduswRegV1
                | Profile::LongPsubuswRegV1
                | Profile::LongPsubbRegV1
                | Profile::LongPaddusbRegV1
                | Profile::LongPaddsbRegV1
                | Profile::LongPsubsbRegV1
                | Profile::LongPaddswRegV1
                | Profile::LongPsubswRegV1
                | Profile::LongPadddRegV1
                | Profile::LongPsubdRegV1
                | Profile::LongPaddqRegV1
                | Profile::LongPsubqRegV1
                | Profile::LongPcmpeqbRegV1
                | Profile::LongPcmpeqwRegV1
                | Profile::LongPcmpeqdRegV1
                | Profile::LongPcmpgtbRegV1
                | Profile::LongPcmpgtwRegV1
                | Profile::LongPcmpgtdRegV1
                | Profile::LongPunpcklbwRegV1
                | Profile::LongPunpckhbwRegV1
                | Profile::LongPunpcklwdRegV1
                | Profile::LongPunpckhwdRegV1
                | Profile::LongPunpckldqRegV1
                | Profile::LongPunpckhdqRegV1
                | Profile::LongPunpcklqdqRegV1
                | Profile::LongPunpckhqdqRegV1
                | Profile::LongPacksswbRegV1
                | Profile::LongPackuswbRegV1
                | Profile::LongPackssdwRegV1
                | Profile::LongPshufdRegV1
                | Profile::LongPsllwImmRegV1
                | Profile::LongPslldImmRegV1
                | Profile::LongPsrldImmRegV1
                | Profile::LongPsrlwImmRegV1
                | Profile::LongPsrawImmRegV1
                | Profile::LongPsradImmRegV1
                | Profile::LongPsubusbRegV1
                | Profile::LongAndpsRegV1
                | Profile::LongOrpsRegV1
                | Profile::LongAndnpsRegV1
                | Profile::LongMovsdRegV1
                | Profile::LongMovsdStoreRegV1
                | Profile::LongMovssRegV1
                | Profile::LongMovssMemLoadV1
                | Profile::LongMovsdMemLoadV1
                | Profile::LongMovsdMemStoreV1
                | Profile::LongMovssMemStoreV1
                | Profile::LongMovssStoreRegV1
                | Profile::LongMovupsMemLoadV1
                | Profile::LongMovupsMemStoreV1
                | Profile::LongMovupsMemBaseV1
        ) {
            state.x87.as_ref().map(X87Snapshot::from)
        } else {
            None
        };
        result.sse = if matches!(
            profile,
            Profile::LongX87SseStateV1
                | Profile::LongMovupsRegV1
                | Profile::LongXorpsRegV1
                | Profile::LongPxorRegV1
                | Profile::LongPaddbRegV1
                | Profile::LongPaddwRegV1
                | Profile::LongPsubwRegV1
                | Profile::LongPadduswRegV1
                | Profile::LongPsubuswRegV1
                | Profile::LongPsubbRegV1
                | Profile::LongPaddusbRegV1
                | Profile::LongPaddsbRegV1
                | Profile::LongPsubsbRegV1
                | Profile::LongPaddswRegV1
                | Profile::LongPsubswRegV1
                | Profile::LongPadddRegV1
                | Profile::LongPsubdRegV1
                | Profile::LongPaddqRegV1
                | Profile::LongPsubqRegV1
                | Profile::LongPcmpeqbRegV1
                | Profile::LongPcmpeqwRegV1
                | Profile::LongPcmpeqdRegV1
                | Profile::LongPcmpgtbRegV1
                | Profile::LongPcmpgtwRegV1
                | Profile::LongPcmpgtdRegV1
                | Profile::LongPunpcklbwRegV1
                | Profile::LongPunpckhbwRegV1
                | Profile::LongPunpcklwdRegV1
                | Profile::LongPunpckhwdRegV1
                | Profile::LongPunpckldqRegV1
                | Profile::LongPunpckhdqRegV1
                | Profile::LongPunpcklqdqRegV1
                | Profile::LongPunpckhqdqRegV1
                | Profile::LongPacksswbRegV1
                | Profile::LongPackuswbRegV1
                | Profile::LongPackssdwRegV1
                | Profile::LongPshufdRegV1
                | Profile::LongPsllwImmRegV1
                | Profile::LongPslldImmRegV1
                | Profile::LongPsrldImmRegV1
                | Profile::LongPsrlwImmRegV1
                | Profile::LongPsrawImmRegV1
                | Profile::LongPsradImmRegV1
                | Profile::LongPsubusbRegV1
                | Profile::LongAndpsRegV1
                | Profile::LongOrpsRegV1
                | Profile::LongAndnpsRegV1
                | Profile::LongMovsdRegV1
                | Profile::LongMovsdStoreRegV1
                | Profile::LongMovssRegV1
                | Profile::LongMovssMemLoadV1
                | Profile::LongMovsdMemLoadV1
                | Profile::LongMovsdMemStoreV1
                | Profile::LongMovssMemStoreV1
                | Profile::LongMovssStoreRegV1
                | Profile::LongMovupsMemLoadV1
                | Profile::LongMovupsMemStoreV1
                | Profile::LongMovupsMemBaseV1
        ) {
            state.sse.as_ref().map(SseSnapshot::from)
        } else {
            None
        };
        if profile == Profile::LongFsBaseV1 {
            result.fs_base = Some(HexU64(state.fs_base));
        } else if profile == Profile::LongGsBaseV1 {
            result.gs_base = Some(HexU64(state.gs_base));
        } else if profile == Profile::LongFsSelectorV1 {
            result.fs_base = Some(HexU64(state.fs_base));
            result.fs_selector = Some(HexU64(u64::from(state.fs_selector)));
        } else if profile == Profile::LongGsSelectorV1 {
            result.gs_base = Some(HexU64(state.gs_base));
            result.gs_selector = Some(HexU64(u64::from(state.gs_selector)));
        } else if profile == Profile::LongVerwGdtV1 {
            if let Some(gdtr) = state.gdtr {
                result.gdtr_base = Some(HexU64(gdtr.base));
                result.gdtr_limit = Some(HexU64(u64::from(gdtr.limit)));
            }
        } else if profile == Profile::LongFsLoadV1 {
            result.fs_base = Some(HexU64(state.fs_base));
            result.fs_selector = Some(HexU64(u64::from(state.fs_selector)));
            if let Some(gdtr) = state.gdtr {
                result.gdtr_base = Some(HexU64(gdtr.base));
                result.gdtr_limit = Some(HexU64(u64::from(gdtr.limit)));
            }
            if let Some(cache) = state.fs_cache {
                result.fs_cache_limit = Some(HexU64(u64::from(cache.limit)));
                result.fs_cache_access = Some(HexU64(u64::from(cache.access)));
                result.fs_cache_flags = Some(HexU64(u64::from(cache.flags)));
            }
        } else if profile == Profile::LongGsLoadV1 {
            result.gs_base = Some(HexU64(state.gs_base));
            result.gs_selector = Some(HexU64(u64::from(state.gs_selector)));
            if let Some(gdtr) = state.gdtr {
                result.gdtr_base = Some(HexU64(gdtr.base));
                result.gdtr_limit = Some(HexU64(u64::from(gdtr.limit)));
            }
            if let Some(cache) = state.gs_cache {
                result.gs_cache_limit = Some(HexU64(u64::from(cache.limit)));
                result.gs_cache_access = Some(HexU64(u64::from(cache.access)));
                result.gs_cache_flags = Some(HexU64(u64::from(cache.flags)));
            }
        } else if profile == Profile::LongNullDsEsV1 {
            result.ds_selector = Some(HexU64(u64::from(state.ds_selector)));
            result.es_selector = Some(HexU64(u64::from(state.es_selector)));
            result.ds_unusable = Some(state.ds_unusable);
            result.es_unusable = Some(state.es_unusable);
        }
        result
    }

    fn validate_for(&self, profile: Profile) -> Result<(), FormatError> {
        let state_profile = match profile {
            Profile::LongX87StateV1
            | Profile::LongX87SseStateV1
            | Profile::LongMovupsRegV1
            | Profile::LongXorpsRegV1
            | Profile::LongPxorRegV1
            | Profile::LongPaddbRegV1
            | Profile::LongPaddwRegV1
            | Profile::LongPsubwRegV1
            | Profile::LongPadduswRegV1
            | Profile::LongPsubuswRegV1
            | Profile::LongPsubbRegV1
            | Profile::LongPaddusbRegV1
            | Profile::LongPaddsbRegV1
            | Profile::LongPsubsbRegV1
            | Profile::LongPaddswRegV1
            | Profile::LongPsubswRegV1
            | Profile::LongPadddRegV1
            | Profile::LongPsubdRegV1
            | Profile::LongPaddqRegV1
            | Profile::LongPsubqRegV1
            | Profile::LongPcmpeqbRegV1
            | Profile::LongPcmpeqwRegV1
            | Profile::LongPcmpeqdRegV1
            | Profile::LongPcmpgtbRegV1
            | Profile::LongPcmpgtwRegV1
            | Profile::LongPcmpgtdRegV1
            | Profile::LongPunpcklbwRegV1
            | Profile::LongPunpckhbwRegV1
            | Profile::LongPunpcklwdRegV1
            | Profile::LongPunpckhwdRegV1
            | Profile::LongPunpckldqRegV1
            | Profile::LongPunpckhdqRegV1
            | Profile::LongPunpcklqdqRegV1
            | Profile::LongPunpckhqdqRegV1
            | Profile::LongPacksswbRegV1
            | Profile::LongPackuswbRegV1
            | Profile::LongPackssdwRegV1
            | Profile::LongPshufdRegV1
            | Profile::LongPsllwImmRegV1
            | Profile::LongPslldImmRegV1
            | Profile::LongPsrldImmRegV1
            | Profile::LongPsrlwImmRegV1
            | Profile::LongPsrawImmRegV1
            | Profile::LongPsradImmRegV1
            | Profile::LongPsubusbRegV1
            | Profile::LongAndpsRegV1
            | Profile::LongOrpsRegV1
            | Profile::LongAndnpsRegV1
            | Profile::LongMovsdRegV1
            | Profile::LongMovsdStoreRegV1
            | Profile::LongMovssRegV1
            | Profile::LongMovssMemLoadV1
            | Profile::LongMovsdMemLoadV1
            | Profile::LongMovsdMemStoreV1
            | Profile::LongMovssMemStoreV1
            | Profile::LongMovssStoreRegV1
            | Profile::LongMovupsMemLoadV1
            | Profile::LongMovupsMemStoreV1
            | Profile::LongMovupsMemBaseV1 => Profile::LongIntegerV1,
            other => other,
        };
        match (state_profile, self.fs_base) {
            (
                Profile::LongIntegerV1
                | Profile::LongX87StateV1
                | Profile::LongX87SseStateV1
                | Profile::LongMovupsRegV1
                | Profile::LongXorpsRegV1
                | Profile::LongPxorRegV1
                | Profile::LongPaddbRegV1
                | Profile::LongPaddwRegV1
                | Profile::LongPsubwRegV1
                | Profile::LongPadduswRegV1
                | Profile::LongPsubuswRegV1
                | Profile::LongPsubbRegV1
                | Profile::LongPaddusbRegV1
                | Profile::LongPaddsbRegV1
                | Profile::LongPsubsbRegV1
                | Profile::LongPaddswRegV1
                | Profile::LongPsubswRegV1
                | Profile::LongPadddRegV1
                | Profile::LongPsubdRegV1
                | Profile::LongPaddqRegV1
                | Profile::LongPsubqRegV1
                | Profile::LongPcmpeqbRegV1
                | Profile::LongPcmpeqwRegV1
                | Profile::LongPcmpeqdRegV1
                | Profile::LongPcmpgtbRegV1
                | Profile::LongPcmpgtwRegV1
                | Profile::LongPcmpgtdRegV1
                | Profile::LongPunpcklbwRegV1
                | Profile::LongPunpckhbwRegV1
                | Profile::LongPunpcklwdRegV1
                | Profile::LongPunpckhwdRegV1
                | Profile::LongPunpckldqRegV1
                | Profile::LongPunpckhdqRegV1
                | Profile::LongPunpcklqdqRegV1
                | Profile::LongPunpckhqdqRegV1
                | Profile::LongPacksswbRegV1
                | Profile::LongPackuswbRegV1
                | Profile::LongPackssdwRegV1
                | Profile::LongPshufdRegV1
                | Profile::LongPsllwImmRegV1
                | Profile::LongPslldImmRegV1
                | Profile::LongPsrldImmRegV1
                | Profile::LongPsrlwImmRegV1
                | Profile::LongPsrawImmRegV1
                | Profile::LongPsradImmRegV1
                | Profile::LongPsubusbRegV1
                | Profile::LongAndpsRegV1
                | Profile::LongOrpsRegV1
                | Profile::LongAndnpsRegV1
                | Profile::LongMovsdRegV1
                | Profile::LongMovsdStoreRegV1
                | Profile::LongMovssRegV1
                | Profile::LongMovssMemLoadV1
                | Profile::LongMovsdMemLoadV1
                | Profile::LongMovsdMemStoreV1
                | Profile::LongMovssMemStoreV1
                | Profile::LongMovssStoreRegV1
                | Profile::LongMovupsMemLoadV1
                | Profile::LongMovupsMemStoreV1
                | Profile::LongMovupsMemBaseV1
                | Profile::LongGsBaseV1
                | Profile::LongGsSelectorV1
                | Profile::LongVerwGdtV1
                | Profile::LongGsLoadV1
                | Profile::LongNullDsEsV1,
                None,
            )
            | (Profile::LongFsBaseV1 | Profile::LongFsLoadV1, Some(_)) => {}
            (Profile::LongFsSelectorV1, Some(_)) => {}
            (
                Profile::LongIntegerV1
                | Profile::LongX87StateV1
                | Profile::LongX87SseStateV1
                | Profile::LongMovupsRegV1
                | Profile::LongXorpsRegV1
                | Profile::LongPxorRegV1
                | Profile::LongPaddbRegV1
                | Profile::LongPaddwRegV1
                | Profile::LongPsubwRegV1
                | Profile::LongPadduswRegV1
                | Profile::LongPsubuswRegV1
                | Profile::LongPsubbRegV1
                | Profile::LongPaddusbRegV1
                | Profile::LongPaddsbRegV1
                | Profile::LongPsubsbRegV1
                | Profile::LongPaddswRegV1
                | Profile::LongPsubswRegV1
                | Profile::LongPadddRegV1
                | Profile::LongPsubdRegV1
                | Profile::LongPaddqRegV1
                | Profile::LongPsubqRegV1
                | Profile::LongPcmpeqbRegV1
                | Profile::LongPcmpeqwRegV1
                | Profile::LongPcmpeqdRegV1
                | Profile::LongPcmpgtbRegV1
                | Profile::LongPcmpgtwRegV1
                | Profile::LongPcmpgtdRegV1
                | Profile::LongPunpcklbwRegV1
                | Profile::LongPunpckhbwRegV1
                | Profile::LongPunpcklwdRegV1
                | Profile::LongPunpckhwdRegV1
                | Profile::LongPunpckldqRegV1
                | Profile::LongPunpckhdqRegV1
                | Profile::LongPunpcklqdqRegV1
                | Profile::LongPunpckhqdqRegV1
                | Profile::LongPacksswbRegV1
                | Profile::LongPackuswbRegV1
                | Profile::LongPackssdwRegV1
                | Profile::LongPshufdRegV1
                | Profile::LongPsllwImmRegV1
                | Profile::LongPslldImmRegV1
                | Profile::LongPsrldImmRegV1
                | Profile::LongPsrlwImmRegV1
                | Profile::LongPsrawImmRegV1
                | Profile::LongPsradImmRegV1
                | Profile::LongPsubusbRegV1
                | Profile::LongAndpsRegV1
                | Profile::LongOrpsRegV1
                | Profile::LongAndnpsRegV1
                | Profile::LongMovsdRegV1
                | Profile::LongMovsdStoreRegV1
                | Profile::LongMovssRegV1
                | Profile::LongMovssMemLoadV1
                | Profile::LongMovsdMemLoadV1
                | Profile::LongMovsdMemStoreV1
                | Profile::LongMovssMemStoreV1
                | Profile::LongMovssStoreRegV1
                | Profile::LongMovupsMemLoadV1
                | Profile::LongMovupsMemStoreV1
                | Profile::LongMovupsMemBaseV1
                | Profile::LongGsBaseV1
                | Profile::LongGsSelectorV1
                | Profile::LongVerwGdtV1
                | Profile::LongGsLoadV1
                | Profile::LongNullDsEsV1,
                Some(_),
            ) => {
                return Err(invalid("state.fs_base", "not part of this profile"));
            }
            (Profile::LongFsBaseV1 | Profile::LongFsSelectorV1 | Profile::LongFsLoadV1, None) => {
                return Err(invalid("state.fs_base", "required by this profile"));
            }
        }
        match (state_profile, self.gs_base) {
            (
                Profile::LongIntegerV1
                | Profile::LongX87StateV1
                | Profile::LongX87SseStateV1
                | Profile::LongMovupsRegV1
                | Profile::LongXorpsRegV1
                | Profile::LongPxorRegV1
                | Profile::LongPaddbRegV1
                | Profile::LongPaddwRegV1
                | Profile::LongPsubwRegV1
                | Profile::LongPadduswRegV1
                | Profile::LongPsubuswRegV1
                | Profile::LongPsubbRegV1
                | Profile::LongPaddusbRegV1
                | Profile::LongPaddsbRegV1
                | Profile::LongPsubsbRegV1
                | Profile::LongPaddswRegV1
                | Profile::LongPsubswRegV1
                | Profile::LongPadddRegV1
                | Profile::LongPsubdRegV1
                | Profile::LongPaddqRegV1
                | Profile::LongPsubqRegV1
                | Profile::LongPcmpeqbRegV1
                | Profile::LongPcmpeqwRegV1
                | Profile::LongPcmpeqdRegV1
                | Profile::LongPcmpgtbRegV1
                | Profile::LongPcmpgtwRegV1
                | Profile::LongPcmpgtdRegV1
                | Profile::LongPunpcklbwRegV1
                | Profile::LongPunpckhbwRegV1
                | Profile::LongPunpcklwdRegV1
                | Profile::LongPunpckhwdRegV1
                | Profile::LongPunpckldqRegV1
                | Profile::LongPunpckhdqRegV1
                | Profile::LongPunpcklqdqRegV1
                | Profile::LongPunpckhqdqRegV1
                | Profile::LongPacksswbRegV1
                | Profile::LongPackuswbRegV1
                | Profile::LongPackssdwRegV1
                | Profile::LongPshufdRegV1
                | Profile::LongPsllwImmRegV1
                | Profile::LongPslldImmRegV1
                | Profile::LongPsrldImmRegV1
                | Profile::LongPsrlwImmRegV1
                | Profile::LongPsrawImmRegV1
                | Profile::LongPsradImmRegV1
                | Profile::LongPsubusbRegV1
                | Profile::LongAndpsRegV1
                | Profile::LongOrpsRegV1
                | Profile::LongAndnpsRegV1
                | Profile::LongMovsdRegV1
                | Profile::LongMovsdStoreRegV1
                | Profile::LongMovssRegV1
                | Profile::LongMovssMemLoadV1
                | Profile::LongMovsdMemLoadV1
                | Profile::LongMovsdMemStoreV1
                | Profile::LongMovssMemStoreV1
                | Profile::LongMovssStoreRegV1
                | Profile::LongMovupsMemLoadV1
                | Profile::LongMovupsMemStoreV1
                | Profile::LongMovupsMemBaseV1
                | Profile::LongFsBaseV1
                | Profile::LongFsSelectorV1
                | Profile::LongVerwGdtV1
                | Profile::LongFsLoadV1
                | Profile::LongNullDsEsV1,
                None,
            )
            | (
                Profile::LongGsBaseV1 | Profile::LongGsSelectorV1 | Profile::LongGsLoadV1,
                Some(_),
            ) => {}
            (_, Some(_)) => return Err(invalid("state.gs_base", "not part of this profile")),
            (Profile::LongGsBaseV1 | Profile::LongGsSelectorV1 | Profile::LongGsLoadV1, None) => {
                return Err(invalid("state.gs_base", "required by this profile"));
            }
        }
        match (state_profile, self.fs_selector) {
            (
                Profile::LongIntegerV1
                | Profile::LongX87StateV1
                | Profile::LongX87SseStateV1
                | Profile::LongMovupsRegV1
                | Profile::LongXorpsRegV1
                | Profile::LongPxorRegV1
                | Profile::LongPaddbRegV1
                | Profile::LongPaddwRegV1
                | Profile::LongPsubwRegV1
                | Profile::LongPadduswRegV1
                | Profile::LongPsubuswRegV1
                | Profile::LongPsubbRegV1
                | Profile::LongPaddusbRegV1
                | Profile::LongPaddsbRegV1
                | Profile::LongPsubsbRegV1
                | Profile::LongPaddswRegV1
                | Profile::LongPsubswRegV1
                | Profile::LongPadddRegV1
                | Profile::LongPsubdRegV1
                | Profile::LongPaddqRegV1
                | Profile::LongPsubqRegV1
                | Profile::LongPcmpeqbRegV1
                | Profile::LongPcmpeqwRegV1
                | Profile::LongPcmpeqdRegV1
                | Profile::LongPcmpgtbRegV1
                | Profile::LongPcmpgtwRegV1
                | Profile::LongPcmpgtdRegV1
                | Profile::LongPunpcklbwRegV1
                | Profile::LongPunpckhbwRegV1
                | Profile::LongPunpcklwdRegV1
                | Profile::LongPunpckhwdRegV1
                | Profile::LongPunpckldqRegV1
                | Profile::LongPunpckhdqRegV1
                | Profile::LongPunpcklqdqRegV1
                | Profile::LongPunpckhqdqRegV1
                | Profile::LongPacksswbRegV1
                | Profile::LongPackuswbRegV1
                | Profile::LongPackssdwRegV1
                | Profile::LongPshufdRegV1
                | Profile::LongPsllwImmRegV1
                | Profile::LongPslldImmRegV1
                | Profile::LongPsrldImmRegV1
                | Profile::LongPsrlwImmRegV1
                | Profile::LongPsrawImmRegV1
                | Profile::LongPsradImmRegV1
                | Profile::LongPsubusbRegV1
                | Profile::LongAndpsRegV1
                | Profile::LongOrpsRegV1
                | Profile::LongAndnpsRegV1
                | Profile::LongMovsdRegV1
                | Profile::LongMovsdStoreRegV1
                | Profile::LongMovssRegV1
                | Profile::LongMovssMemLoadV1
                | Profile::LongMovsdMemLoadV1
                | Profile::LongMovsdMemStoreV1
                | Profile::LongMovssMemStoreV1
                | Profile::LongMovssStoreRegV1
                | Profile::LongMovupsMemLoadV1
                | Profile::LongMovupsMemStoreV1
                | Profile::LongMovupsMemBaseV1
                | Profile::LongFsBaseV1
                | Profile::LongGsBaseV1
                | Profile::LongVerwGdtV1
                | Profile::LongGsLoadV1
                | Profile::LongNullDsEsV1,
                None,
            )
            | (Profile::LongFsSelectorV1 | Profile::LongFsLoadV1, Some(_)) => {}
            (Profile::LongGsSelectorV1, None) => {}
            (_, Some(_)) => {
                return Err(invalid("state.fs_selector", "not part of this profile"));
            }
            (Profile::LongFsSelectorV1 | Profile::LongFsLoadV1, None) => {
                return Err(invalid("state.fs_selector", "required by this profile"));
            }
        }
        if self
            .fs_selector
            .is_some_and(|selector| selector.get() > u16::MAX as u64)
        {
            return Err(invalid("state.fs_selector", "must fit in 16 bits"));
        }
        match (state_profile, self.gs_selector) {
            (Profile::LongGsSelectorV1 | Profile::LongGsLoadV1, Some(_)) => {}
            (Profile::LongGsSelectorV1 | Profile::LongGsLoadV1, None) => {
                return Err(invalid("state.gs_selector", "required by this profile"));
            }
            (_, None) => {}
            (_, Some(_)) => return Err(invalid("state.gs_selector", "not part of this profile")),
        }
        if self
            .gs_selector
            .is_some_and(|selector| selector.get() > u16::MAX as u64)
        {
            return Err(invalid("state.gs_selector", "must fit in 16 bits"));
        }
        match (
            state_profile,
            self.ds_selector,
            self.es_selector,
            self.ds_unusable,
            self.es_unusable,
        ) {
            (Profile::LongNullDsEsV1, Some(ds), Some(es), Some(_), Some(_)) => {
                if ds.get() > u16::MAX as u64 {
                    return Err(invalid("state.ds_selector", "must fit in 16 bits"));
                }
                if es.get() > u16::MAX as u64 {
                    return Err(invalid("state.es_selector", "must fit in 16 bits"));
                }
            }
            (Profile::LongNullDsEsV1, _, _, _, _) => {
                return Err(invalid(
                    "state.ds_es",
                    "selectors and unusable state are required by this profile",
                ));
            }
            (_, None, None, None, None) => {}
            _ => return Err(invalid("state.ds_es", "not part of this profile")),
        }
        match (state_profile, self.gdtr_base, self.gdtr_limit) {
            (
                Profile::LongVerwGdtV1 | Profile::LongFsLoadV1 | Profile::LongGsLoadV1,
                Some(_),
                Some(_),
            ) => {}
            (Profile::LongVerwGdtV1 | Profile::LongFsLoadV1 | Profile::LongGsLoadV1, _, _) => {
                return Err(invalid(
                    "state.gdtr",
                    "base and limit are required by this profile",
                ));
            }
            (_, None, None) => {}
            (_, _, _) => return Err(invalid("state.gdtr", "not part of this profile")),
        }
        if self
            .gdtr_limit
            .is_some_and(|limit| limit.get() > u16::MAX as u64)
        {
            return Err(invalid("state.gdtr_limit", "must fit in 16 bits"));
        }
        if let (Some(base), Some(limit)) = (self.gdtr_base, self.gdtr_limit) {
            let base = base.get();
            let upper = base >> 47;
            if upper != 0 && upper != 0x1ffff {
                return Err(invalid("state.gdtr_base", "must be canonical48"));
            }
            let last = base
                .checked_add(limit.get())
                .ok_or_else(|| invalid("state.gdtr", "base plus limit overflows"))?;
            let upper = last >> 47;
            if upper != 0 && upper != 0x1ffff {
                return Err(invalid(
                    "state.gdtr",
                    "entire table range must be canonical48",
                ));
            }
        }
        if let Some(base) = self.fs_base.map(HexU64::get) {
            let upper = base >> 47;
            if upper != 0 && upper != 0x1ffff {
                return Err(invalid("state.fs_base", "must be canonical48"));
            }
        }
        if let Some(base) = self.gs_base.map(HexU64::get) {
            let upper = base >> 47;
            if upper != 0 && upper != 0x1ffff {
                return Err(invalid("state.gs_base", "must be canonical48"));
            }
        }
        match (
            state_profile,
            self.fs_cache_limit,
            self.fs_cache_access,
            self.fs_cache_flags,
        ) {
            (Profile::LongFsLoadV1, Some(limit), Some(access), Some(flags)) => {
                if limit.get() > u32::MAX as u64 {
                    return Err(invalid("state.fs_cache_limit", "must fit in 32 bits"));
                }
                if access.get() > u8::MAX as u64 {
                    return Err(invalid("state.fs_cache_access", "must fit in 8 bits"));
                }
                if flags.get() > 0x0f {
                    return Err(invalid("state.fs_cache_flags", "must fit in 4 bits"));
                }
            }
            (Profile::LongFsLoadV1, _, _, _) => {
                return Err(invalid(
                    "state.fs_cache",
                    "limit, access and flags are required by this profile",
                ));
            }
            (_, None, None, None) => {}
            _ => return Err(invalid("state.fs_cache", "not part of this profile")),
        }
        match (
            state_profile,
            self.gs_cache_limit,
            self.gs_cache_access,
            self.gs_cache_flags,
        ) {
            (Profile::LongGsLoadV1, Some(limit), Some(access), Some(flags)) => {
                if limit.get() > u32::MAX as u64 {
                    return Err(invalid("state.gs_cache_limit", "must fit in 32 bits"));
                }
                if access.get() > u8::MAX as u64 {
                    return Err(invalid("state.gs_cache_access", "must fit in 8 bits"));
                }
                if flags.get() > 0x0f {
                    return Err(invalid("state.gs_cache_flags", "must fit in 4 bits"));
                }
            }
            (Profile::LongGsLoadV1, _, _, _) => {
                return Err(invalid(
                    "state.gs_cache",
                    "limit, access and flags are required by this profile",
                ));
            }
            (_, None, None, None) => {}
            _ => return Err(invalid("state.gs_cache", "not part of this profile")),
        }
        if self.rflags.get() & !INTEGER_FLAGS != 0 || self.rflags.get() & 2 == 0 {
            return Err(invalid(
                "state.rflags",
                "outside the integer profile or missing reserved bit 1",
            ));
        }
        match (profile, &self.x87) {
            (Profile::LongX87StateV1, Some(x87)) => x87.validate()?,
            (
                Profile::LongX87SseStateV1
                | Profile::LongMovupsRegV1
                | Profile::LongXorpsRegV1
                | Profile::LongPxorRegV1
                | Profile::LongPaddbRegV1
                | Profile::LongPaddwRegV1
                | Profile::LongPsubwRegV1
                | Profile::LongPadduswRegV1
                | Profile::LongPsubuswRegV1
                | Profile::LongPsubbRegV1
                | Profile::LongPaddusbRegV1
                | Profile::LongPaddsbRegV1
                | Profile::LongPsubsbRegV1
                | Profile::LongPaddswRegV1
                | Profile::LongPsubswRegV1
                | Profile::LongPadddRegV1
                | Profile::LongPsubdRegV1
                | Profile::LongPaddqRegV1
                | Profile::LongPsubqRegV1
                | Profile::LongPcmpeqbRegV1
                | Profile::LongPcmpeqwRegV1
                | Profile::LongPcmpeqdRegV1
                | Profile::LongPcmpgtbRegV1
                | Profile::LongPcmpgtwRegV1
                | Profile::LongPcmpgtdRegV1
                | Profile::LongPunpcklbwRegV1
                | Profile::LongPunpckhbwRegV1
                | Profile::LongPunpcklwdRegV1
                | Profile::LongPunpckhwdRegV1
                | Profile::LongPunpckldqRegV1
                | Profile::LongPunpckhdqRegV1
                | Profile::LongPunpcklqdqRegV1
                | Profile::LongPunpckhqdqRegV1
                | Profile::LongPacksswbRegV1
                | Profile::LongPackuswbRegV1
                | Profile::LongPackssdwRegV1
                | Profile::LongPshufdRegV1
                | Profile::LongPsllwImmRegV1
                | Profile::LongPslldImmRegV1
                | Profile::LongPsrldImmRegV1
                | Profile::LongPsrlwImmRegV1
                | Profile::LongPsrawImmRegV1
                | Profile::LongPsradImmRegV1
                | Profile::LongPsubusbRegV1
                | Profile::LongAndpsRegV1
                | Profile::LongOrpsRegV1
                | Profile::LongAndnpsRegV1
                | Profile::LongMovsdRegV1
                | Profile::LongMovsdStoreRegV1
                | Profile::LongMovssRegV1
                | Profile::LongMovssMemLoadV1
                | Profile::LongMovsdMemLoadV1
                | Profile::LongMovsdMemStoreV1
                | Profile::LongMovssMemStoreV1
                | Profile::LongMovssStoreRegV1
                | Profile::LongMovupsMemLoadV1
                | Profile::LongMovupsMemStoreV1
                | Profile::LongMovupsMemBaseV1,
                Some(x87),
            ) => x87.validate()?,
            (Profile::LongX87StateV1, None) => {
                return Err(invalid("state.x87", "required by this profile"));
            }
            (
                Profile::LongX87SseStateV1
                | Profile::LongMovupsRegV1
                | Profile::LongXorpsRegV1
                | Profile::LongPxorRegV1
                | Profile::LongPaddbRegV1
                | Profile::LongPaddwRegV1
                | Profile::LongPsubwRegV1
                | Profile::LongPadduswRegV1
                | Profile::LongPsubuswRegV1
                | Profile::LongPsubbRegV1
                | Profile::LongPaddusbRegV1
                | Profile::LongPaddsbRegV1
                | Profile::LongPsubsbRegV1
                | Profile::LongPaddswRegV1
                | Profile::LongPsubswRegV1
                | Profile::LongPadddRegV1
                | Profile::LongPsubdRegV1
                | Profile::LongPaddqRegV1
                | Profile::LongPsubqRegV1
                | Profile::LongPcmpeqbRegV1
                | Profile::LongPcmpeqwRegV1
                | Profile::LongPcmpeqdRegV1
                | Profile::LongPcmpgtbRegV1
                | Profile::LongPcmpgtwRegV1
                | Profile::LongPcmpgtdRegV1
                | Profile::LongPunpcklbwRegV1
                | Profile::LongPunpckhbwRegV1
                | Profile::LongPunpcklwdRegV1
                | Profile::LongPunpckhwdRegV1
                | Profile::LongPunpckldqRegV1
                | Profile::LongPunpckhdqRegV1
                | Profile::LongPunpcklqdqRegV1
                | Profile::LongPunpckhqdqRegV1
                | Profile::LongPacksswbRegV1
                | Profile::LongPackuswbRegV1
                | Profile::LongPackssdwRegV1
                | Profile::LongPshufdRegV1
                | Profile::LongPsllwImmRegV1
                | Profile::LongPslldImmRegV1
                | Profile::LongPsrldImmRegV1
                | Profile::LongPsrlwImmRegV1
                | Profile::LongPsrawImmRegV1
                | Profile::LongPsradImmRegV1
                | Profile::LongPsubusbRegV1
                | Profile::LongAndpsRegV1
                | Profile::LongOrpsRegV1
                | Profile::LongAndnpsRegV1
                | Profile::LongMovsdRegV1
                | Profile::LongMovsdStoreRegV1
                | Profile::LongMovssRegV1
                | Profile::LongMovssMemLoadV1
                | Profile::LongMovsdMemLoadV1
                | Profile::LongMovsdMemStoreV1
                | Profile::LongMovssMemStoreV1
                | Profile::LongMovssStoreRegV1
                | Profile::LongMovupsMemLoadV1
                | Profile::LongMovupsMemStoreV1
                | Profile::LongMovupsMemBaseV1,
                None,
            ) => {
                return Err(invalid("state.x87", "required by this profile"));
            }
            (_, None) => {}
            (_, Some(_)) => return Err(invalid("state.x87", "not part of this profile")),
        }
        match (profile, &self.sse) {
            (
                Profile::LongX87SseStateV1
                | Profile::LongMovupsRegV1
                | Profile::LongXorpsRegV1
                | Profile::LongPxorRegV1
                | Profile::LongPaddbRegV1
                | Profile::LongPaddwRegV1
                | Profile::LongPsubwRegV1
                | Profile::LongPadduswRegV1
                | Profile::LongPsubuswRegV1
                | Profile::LongPsubbRegV1
                | Profile::LongPaddusbRegV1
                | Profile::LongPaddsbRegV1
                | Profile::LongPsubsbRegV1
                | Profile::LongPaddswRegV1
                | Profile::LongPsubswRegV1
                | Profile::LongPadddRegV1
                | Profile::LongPsubdRegV1
                | Profile::LongPaddqRegV1
                | Profile::LongPsubqRegV1
                | Profile::LongPcmpeqbRegV1
                | Profile::LongPcmpeqwRegV1
                | Profile::LongPcmpeqdRegV1
                | Profile::LongPcmpgtbRegV1
                | Profile::LongPcmpgtwRegV1
                | Profile::LongPcmpgtdRegV1
                | Profile::LongPunpcklbwRegV1
                | Profile::LongPunpckhbwRegV1
                | Profile::LongPunpcklwdRegV1
                | Profile::LongPunpckhwdRegV1
                | Profile::LongPunpckldqRegV1
                | Profile::LongPunpckhdqRegV1
                | Profile::LongPunpcklqdqRegV1
                | Profile::LongPunpckhqdqRegV1
                | Profile::LongPacksswbRegV1
                | Profile::LongPackuswbRegV1
                | Profile::LongPackssdwRegV1
                | Profile::LongPshufdRegV1
                | Profile::LongPsllwImmRegV1
                | Profile::LongPslldImmRegV1
                | Profile::LongPsrldImmRegV1
                | Profile::LongPsrlwImmRegV1
                | Profile::LongPsrawImmRegV1
                | Profile::LongPsradImmRegV1
                | Profile::LongPsubusbRegV1
                | Profile::LongAndpsRegV1
                | Profile::LongOrpsRegV1
                | Profile::LongAndnpsRegV1
                | Profile::LongMovsdRegV1
                | Profile::LongMovsdStoreRegV1
                | Profile::LongMovssRegV1
                | Profile::LongMovssMemLoadV1
                | Profile::LongMovsdMemLoadV1
                | Profile::LongMovsdMemStoreV1
                | Profile::LongMovssMemStoreV1
                | Profile::LongMovssStoreRegV1
                | Profile::LongMovupsMemLoadV1
                | Profile::LongMovupsMemStoreV1
                | Profile::LongMovupsMemBaseV1,
                Some(sse),
            ) => sse.validate()?,
            (
                Profile::LongX87SseStateV1
                | Profile::LongMovupsRegV1
                | Profile::LongXorpsRegV1
                | Profile::LongPxorRegV1
                | Profile::LongPaddbRegV1
                | Profile::LongPaddwRegV1
                | Profile::LongPsubwRegV1
                | Profile::LongPadduswRegV1
                | Profile::LongPsubuswRegV1
                | Profile::LongPsubbRegV1
                | Profile::LongPaddusbRegV1
                | Profile::LongPaddsbRegV1
                | Profile::LongPsubsbRegV1
                | Profile::LongPaddswRegV1
                | Profile::LongPsubswRegV1
                | Profile::LongPadddRegV1
                | Profile::LongPsubdRegV1
                | Profile::LongPaddqRegV1
                | Profile::LongPsubqRegV1
                | Profile::LongPcmpeqbRegV1
                | Profile::LongPcmpeqwRegV1
                | Profile::LongPcmpeqdRegV1
                | Profile::LongPcmpgtbRegV1
                | Profile::LongPcmpgtwRegV1
                | Profile::LongPcmpgtdRegV1
                | Profile::LongPunpcklbwRegV1
                | Profile::LongPunpckhbwRegV1
                | Profile::LongPunpcklwdRegV1
                | Profile::LongPunpckhwdRegV1
                | Profile::LongPunpckldqRegV1
                | Profile::LongPunpckhdqRegV1
                | Profile::LongPunpcklqdqRegV1
                | Profile::LongPunpckhqdqRegV1
                | Profile::LongPacksswbRegV1
                | Profile::LongPackuswbRegV1
                | Profile::LongPackssdwRegV1
                | Profile::LongPshufdRegV1
                | Profile::LongPsllwImmRegV1
                | Profile::LongPslldImmRegV1
                | Profile::LongPsrldImmRegV1
                | Profile::LongPsrlwImmRegV1
                | Profile::LongPsrawImmRegV1
                | Profile::LongPsradImmRegV1
                | Profile::LongPsubusbRegV1
                | Profile::LongAndpsRegV1
                | Profile::LongOrpsRegV1
                | Profile::LongAndnpsRegV1
                | Profile::LongMovsdRegV1
                | Profile::LongMovsdStoreRegV1
                | Profile::LongMovssRegV1
                | Profile::LongMovssMemLoadV1
                | Profile::LongMovsdMemLoadV1
                | Profile::LongMovsdMemStoreV1
                | Profile::LongMovssMemStoreV1
                | Profile::LongMovssStoreRegV1
                | Profile::LongMovupsMemLoadV1
                | Profile::LongMovupsMemStoreV1
                | Profile::LongMovupsMemBaseV1,
                None,
            ) => {
                return Err(invalid("state.sse", "required by this profile"));
            }
            (_, None) => {}
            (_, Some(_)) => return Err(invalid("state.sse", "not part of this profile")),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedExit {
    StopAddress,
    Unsupported,
    InstructionBudgetExhausted,
    RepIterationBudgetExhausted,
    StopRequested,
    Exception,
    /// The bounded fixture backend could not complete an access; this is not a guest exception.
    HarnessError,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub schema_version: u32,
    pub case_id: String,
    pub profile: Profile,
    pub provenance: Provenance,
    pub seed: Option<HexU64>,
    /// Set only by explicit conversion; never reuse the old input digest as this fixture's digest.
    pub converted_from_v1_sha256: Option<String>,
    pub initial_state: IntegerState,
    /// Code and data share the same guest address space. No implicit host buffer.
    pub memory: Vec<MemoryRegionSpec>,
    /// Optional external harness boundary, checked before fetch, not an x86 opcode.
    pub stop_rip: Option<HexU64>,
    pub instruction_budget: u64,
    pub wall_timeout_ms: u32,
    pub rflags_mask: HexU64,
    pub expected_exit: ExpectedExit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedFixture {
    pub fixture: Fixture,
    pub input_sha256: String,
}

impl Fixture {
    pub fn from_json(bytes: &[u8]) -> Result<LoadedFixture, FormatError> {
        let fixture: Self = load(bytes, MAX_FIXTURE_JSON_BYTES)?;
        fixture.validate()?;
        Ok(LoadedFixture {
            fixture,
            input_sha256: sha256_hex(bytes),
        })
    }

    pub fn to_json(&self) -> Result<Vec<u8>, FormatError> {
        self.validate()?;
        save(self, MAX_FIXTURE_JSON_BYTES)
    }

    pub fn validate(&self) -> Result<(), FormatError> {
        version(self.schema_version)?;
        validate_case_id(&self.case_id)?;
        self.initial_state.validate_for(self.profile)?;
        for value in [
            &self.provenance.kind,
            &self.provenance.source,
            &self.provenance.specification,
            &self.provenance.specification_version,
        ] {
            text(value, "provenance")?;
        }
        if let Some(digest) = &self.converted_from_v1_sha256 {
            digest_field(digest)?;
        }
        MemoryComparisonScope::Compare {
            regions: self.memory.clone(),
        }
        .validate()?;
        if self.memory.is_empty() {
            return Err(invalid("memory", "must contain a mapping"));
        }
        if self.instruction_budget > MAX_INSTRUCTION_BUDGET {
            return Err(invalid(
                "instruction_budget",
                "must be between 0 and 10000000",
            ));
        }
        if !(1..=MAX_WALL_TIMEOUT_MS).contains(&self.wall_timeout_ms) {
            return Err(invalid("wall_timeout_ms", "must be between 1 and 30000"));
        }
        if self.rflags_mask.get() & !INTEGER_FLAGS != 0 {
            return Err(invalid("rflags_mask", "outside the integer profile"));
        }
        if self.expected_exit == ExpectedExit::StopAddress && self.stop_rip.is_none() {
            return Err(invalid("stop_rip", "required for stop_address"));
        }
        if matches!(
            self.profile,
            Profile::LongFsLoadV1 | Profile::LongGsLoadV1 | Profile::LongNullDsEsV1
        ) {
            let code = self.memory.iter().find(|region| region.region_id == "code");
            let Some(code) = code else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let start = self.initial_state.rip.get();
            let admitted = match (self.profile, bytes.as_str()) {
                (Profile::LongFsLoadV1, "8ee0") => (1, 2),
                (Profile::LongFsLoadV1, "8ee064488b00") => (2, 6),
                (Profile::LongGsLoadV1, "8ee8") => (1, 2),
                (Profile::LongGsLoadV1, "8ee865488b00") => (2, 6),
                (Profile::LongNullDsEsV1, "8ed8" | "8ec0") => (1, 2),
                (Profile::LongNullDsEsV1, "8ed88ed8" | "8ed88ec0") => (2, 4),
                _ => {
                    return Err(invalid(
                        "memory.code",
                        "outside the bounded segment-load profile",
                    ));
                }
            };
            let expected_stop = start.checked_add(u64::from(admitted.1));
            if code.guest_address.get() != start
                || code.length != admitted.1
                || !code.permissions.execute
                || self.instruction_budget != admitted.0
                || self.stop_rip.map(HexU64::get) != expected_stop
            {
                return Err(invalid(
                    "memory.code",
                    "address, budget and stop_rip must match the exact admitted program",
                ));
            }
        }
        if matches!(
            self.profile,
            Profile::LongXorpsRegV1
                | Profile::LongPxorRegV1
                | Profile::LongPaddbRegV1
                | Profile::LongPaddwRegV1
                | Profile::LongPsubwRegV1
                | Profile::LongPadduswRegV1
                | Profile::LongPsubuswRegV1
                | Profile::LongPsubbRegV1
                | Profile::LongPaddusbRegV1
                | Profile::LongPaddsbRegV1
                | Profile::LongPsubsbRegV1
                | Profile::LongPaddswRegV1
                | Profile::LongPsubswRegV1
                | Profile::LongPadddRegV1
                | Profile::LongPsubdRegV1
                | Profile::LongPaddqRegV1
                | Profile::LongPsubqRegV1
                | Profile::LongPcmpeqbRegV1
                | Profile::LongPcmpeqwRegV1
                | Profile::LongPcmpeqdRegV1
                | Profile::LongPcmpgtbRegV1
                | Profile::LongPcmpgtwRegV1
                | Profile::LongPcmpgtdRegV1
                | Profile::LongPunpcklbwRegV1
                | Profile::LongPunpckhbwRegV1
                | Profile::LongPunpcklwdRegV1
                | Profile::LongPunpckhwdRegV1
                | Profile::LongPunpckldqRegV1
                | Profile::LongPunpckhdqRegV1
                | Profile::LongPunpcklqdqRegV1
                | Profile::LongPunpckhqdqRegV1
                | Profile::LongPacksswbRegV1
                | Profile::LongPackuswbRegV1
                | Profile::LongPackssdwRegV1
                | Profile::LongPshufdRegV1
                | Profile::LongPsllwImmRegV1
                | Profile::LongPslldImmRegV1
                | Profile::LongPsrldImmRegV1
                | Profile::LongPsrlwImmRegV1
                | Profile::LongPsrawImmRegV1
                | Profile::LongPsradImmRegV1
                | Profile::LongPsubusbRegV1
                | Profile::LongAndpsRegV1
                | Profile::LongOrpsRegV1
                | Profile::LongAndnpsRegV1
                | Profile::LongMovsdRegV1
                | Profile::LongMovsdStoreRegV1
                | Profile::LongMovssRegV1
                | Profile::LongMovssStoreRegV1
        ) {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let code_bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let length = match self.profile {
                Profile::LongXorpsRegV1 => xorps_register_length(&code_bytes),
                Profile::LongPxorRegV1 => pxor_register_length(&code_bytes),
                Profile::LongPaddbRegV1 => paddb_register_length(&code_bytes),
                Profile::LongPaddwRegV1 => paddw_register_length(&code_bytes),
                Profile::LongPsubwRegV1 => psubw_register_length(&code_bytes),
                Profile::LongPadduswRegV1 => paddusw_register_length(&code_bytes),
                Profile::LongPsubuswRegV1 => psubusw_register_length(&code_bytes),
                Profile::LongPsubbRegV1 => psubb_register_length(&code_bytes),
                Profile::LongPaddusbRegV1 => paddusb_register_length(&code_bytes),
                Profile::LongPaddsbRegV1 => paddsb_register_length(&code_bytes),
                Profile::LongPsubsbRegV1 => psubsb_register_length(&code_bytes),
                Profile::LongPaddswRegV1 => paddsw_register_length(&code_bytes),
                Profile::LongPsubswRegV1 => psubsw_register_length(&code_bytes),
                Profile::LongPadddRegV1 => paddd_register_length(&code_bytes),
                Profile::LongPsubdRegV1 => psubd_register_length(&code_bytes),
                Profile::LongPaddqRegV1 => paddq_register_length(&code_bytes),
                Profile::LongPsubqRegV1 => psubq_register_length(&code_bytes),
                Profile::LongPcmpeqbRegV1 => pcmpeqb_register_length(&code_bytes),
                Profile::LongPcmpeqwRegV1 => pcmpeqw_register_length(&code_bytes),
                Profile::LongPcmpeqdRegV1 => pcmpeqd_register_length(&code_bytes),
                Profile::LongPcmpgtbRegV1 => pcmpgtb_register_length(&code_bytes),
                Profile::LongPcmpgtwRegV1 => pcmpgtw_register_length(&code_bytes),
                Profile::LongPcmpgtdRegV1 => pcmpgtd_register_length(&code_bytes),
                Profile::LongPunpcklbwRegV1 => punpcklbw_register_length(&code_bytes),
                Profile::LongPunpckhbwRegV1 => punpckhbw_register_length(&code_bytes),
                Profile::LongPunpcklwdRegV1 => punpcklwd_register_length(&code_bytes),
                Profile::LongPunpckhwdRegV1 => punpckhwd_register_length(&code_bytes),
                Profile::LongPunpckldqRegV1 => punpckldq_register_length(&code_bytes),
                Profile::LongPunpckhdqRegV1 => punpckhdq_register_length(&code_bytes),
                Profile::LongPunpcklqdqRegV1 => punpcklqdq_register_length(&code_bytes),
                Profile::LongPunpckhqdqRegV1 => punpckhqdq_register_length(&code_bytes),
                Profile::LongPacksswbRegV1 => packsswb_register_length(&code_bytes),
                Profile::LongPackuswbRegV1 => packuswb_register_length(&code_bytes),
                Profile::LongPackssdwRegV1 => packssdw_register_length(&code_bytes),
                Profile::LongPshufdRegV1 => pshufd_register_length(&code_bytes),
                Profile::LongPsllwImmRegV1 => psllw_immediate_register_length(&code_bytes),
                Profile::LongPslldImmRegV1 => pslld_immediate_register_length(&code_bytes),
                Profile::LongPsrldImmRegV1 => psrld_immediate_register_length(&code_bytes),
                Profile::LongPsrlwImmRegV1 => psrlw_immediate_register_length(&code_bytes),
                Profile::LongPsrawImmRegV1 => psraw_immediate_register_length(&code_bytes),
                Profile::LongPsradImmRegV1 => psrad_immediate_register_length(&code_bytes),
                Profile::LongPsubusbRegV1 => psubusb_register_length(&code_bytes),
                Profile::LongAndpsRegV1 => andps_register_length(&code_bytes),
                Profile::LongOrpsRegV1 => orps_register_length(&code_bytes),
                Profile::LongAndnpsRegV1 => andnps_register_length(&code_bytes),
                Profile::LongMovssRegV1 => movss_register_length(&code_bytes),
                Profile::LongMovsdRegV1 => movsd_register_length(&code_bytes),
                Profile::LongMovsdStoreRegV1 => movsd_store_register_length(&code_bytes),
                Profile::LongMovssStoreRegV1 => movss_store_register_length(&code_bytes),
                _ => unreachable!("matched SSE logic profile above"),
            }
            .ok_or_else(|| invalid("memory.code", "outside the register-only SSE profile"))?;
            if code.guest_address != self.initial_state.rip
                || code.length != length
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get)
                    != self.initial_state.rip.get().checked_add(u64::from(length))
                || self.expected_exit != ExpectedExit::StopAddress
            {
                return Err(invalid(
                    "memory.code",
                    "SSE logic requires one executable instruction, budget 1, and stop_rip immediately afterward",
                ));
            }
        }
        if self.profile == Profile::LongMovupsRegV1 {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let code_bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let length = movups_register_length(&code_bytes).ok_or_else(|| {
                invalid("memory.code", "outside the register-only MOVUPS profile")
            })?;
            if code.guest_address != self.initial_state.rip
                || code.length != length
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get)
                    != self.initial_state.rip.get().checked_add(u64::from(length))
                || self.expected_exit != ExpectedExit::StopAddress
            {
                return Err(invalid(
                    "memory.code",
                    "MOVUPS requires one executable instruction, budget 1, and stop_rip immediately afterward",
                ));
            }
        }
        if self.profile == Profile::LongMovupsMemLoadV1 {
            let code = self.memory.iter().find(|region| region.region_id == "code");
            let Some(code) = code else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let code_bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let (length, _) = movups_memory_load_destination(&code_bytes).ok_or_else(|| {
                invalid(
                    "memory.code",
                    "outside the [RAX] MOVUPS memory-load profile",
                )
            })?;
            let start = self.initial_state.gpr[0].get();
            let end = start
                .checked_add(15)
                .ok_or_else(|| invalid("state.gpr[0]", "MOVUPS source range overflows u64"))?;
            let data = self.memory.iter().find(|region| region.region_id == "data");
            let Some(data) = data else {
                return Err(invalid("memory.data", "required by this profile"));
            };
            let data_start = data.guest_address.get();
            let data_end = data_start
                .checked_add(u64::from(data.length))
                .ok_or_else(|| invalid("memory.data", "range overflows u64"))?;
            let code_end = self.initial_state.rip.get().checked_add(u64::from(length));
            if self.memory.len() != 2
                || code.region_id != "code"
                || code.guest_address.get() != self.initial_state.rip.get()
                || code.length != length
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != code_end
                || self.expected_exit != ExpectedExit::StopAddress
                || !data.permissions.read
                || data.region_id != "data"
                || data.permissions.write
                || data.permissions.execute
                || start < data_start
                || end >= data_end
            {
                return Err(invalid(
                    "memory",
                    "MOVUPS memory-load profile requires one code instruction and one readable, non-writable data region covering [RAX..RAX+15]",
                ));
            }
        }
        if self.profile == Profile::LongMovssMemLoadV1 {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let (length, _) = movss_memory_load_destination(&bytes)
                .ok_or_else(|| invalid("memory.code", "outside the F3 MOVSS [RAX] load profile"))?;
            let start = self.initial_state.gpr[0].get();
            let end = start
                .checked_add(3)
                .ok_or_else(|| invalid("state.gpr[0]", "MOVSS source range overflows u64"))?;
            let Some(data) = self.memory.iter().find(|region| region.region_id == "data") else {
                return Err(invalid("memory.data", "required by this profile"));
            };
            let data_start = data.guest_address.get();
            let data_end = data_start
                .checked_add(u64::from(data.length))
                .ok_or_else(|| invalid("memory.data", "range overflows u64"))?;
            if self.memory.len() != 2
                || code.guest_address != self.initial_state.rip
                || code.length != length
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get)
                    != self.initial_state.rip.get().checked_add(u64::from(length))
                || self.expected_exit != ExpectedExit::StopAddress
                || data.permissions
                    != (MemoryPermissions {
                        read: true,
                        write: false,
                        execute: false,
                    })
                || data_start > start
                || end >= data_end
            {
                return Err(invalid(
                    "memory",
                    "MOVSS load requires one code instruction, one read-only data region covering [RAX..RAX+3], budget 1, and stop immediately afterward",
                ));
            }
        }
        if self.profile == Profile::LongMovsdMemLoadV1 {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let (length, _) = movsd_memory_load_destination(&bytes)
                .ok_or_else(|| invalid("memory.code", "outside the F2 MOVSD [RAX] load profile"))?;
            let start = self.initial_state.gpr[0].get();
            let end = start
                .checked_add(7)
                .ok_or_else(|| invalid("state.gpr[0]", "MOVSD source range overflows u64"))?;
            let Some(data) = self.memory.iter().find(|region| region.region_id == "data") else {
                return Err(invalid("memory.data", "required by this profile"));
            };
            let data_start = data.guest_address.get();
            let data_end = data_start
                .checked_add(u64::from(data.length))
                .ok_or_else(|| invalid("memory.data", "range overflows u64"))?;
            if self.memory.len() != 2
                || code.guest_address != self.initial_state.rip
                || code.length != length
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get)
                    != self.initial_state.rip.get().checked_add(u64::from(length))
                || self.expected_exit != ExpectedExit::StopAddress
                || data.permissions
                    != (MemoryPermissions {
                        read: true,
                        write: false,
                        execute: false,
                    })
                || data_start > start
                || end >= data_end
            {
                return Err(invalid(
                    "memory",
                    "MOVSD load requires one code instruction, one read-only data region covering [RAX..RAX+7], budget 1, and stop immediately afterward",
                ));
            }
        }
        if self.profile == Profile::LongMovsdMemStoreV1 {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let (length, _) = movsd_memory_store_source(&bytes).ok_or_else(|| {
                invalid("memory.code", "outside the F2 MOVSD [RAX] store profile")
            })?;
            let start = self.initial_state.gpr[0].get();
            let end = start
                .checked_add(7)
                .ok_or_else(|| invalid("state.gpr[0]", "MOVSD destination range overflows u64"))?;
            let Some(data) = self.memory.iter().find(|region| region.region_id == "data") else {
                return Err(invalid("memory.data", "required by this profile"));
            };
            let data_start = data.guest_address.get();
            let data_end = data_start
                .checked_add(u64::from(data.length))
                .ok_or_else(|| invalid("memory.data", "range overflows u64"))?;
            if self.memory.len() != 2
                || code.guest_address != self.initial_state.rip
                || code.length != length
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get)
                    != self.initial_state.rip.get().checked_add(u64::from(length))
                || self.expected_exit != ExpectedExit::StopAddress
                || data.permissions
                    != (MemoryPermissions {
                        read: true,
                        write: true,
                        execute: false,
                    })
                || data_start > start
                || end >= data_end
            {
                return Err(invalid(
                    "memory",
                    "MOVSD store requires one code instruction, one writable data region covering [RAX..RAX+7], budget 1, and stop immediately afterward",
                ));
            }
        }
        if self.profile == Profile::LongMovssMemStoreV1 {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let (length, _) = movss_memory_store_source(&bytes).ok_or_else(|| {
                invalid("memory.code", "outside the F3 MOVSS [RAX] store profile")
            })?;
            let start = self.initial_state.gpr[0].get();
            let end = start
                .checked_add(3)
                .ok_or_else(|| invalid("state.gpr[0]", "MOVSS destination range overflows u64"))?;
            let Some(data) = self.memory.iter().find(|region| region.region_id == "data") else {
                return Err(invalid("memory.data", "required by this profile"));
            };
            let data_start = data.guest_address.get();
            let data_end = data_start
                .checked_add(u64::from(data.length))
                .ok_or_else(|| invalid("memory.data", "range overflows u64"))?;
            if self.memory.len() != 2
                || code.guest_address != self.initial_state.rip
                || code.length != length
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get)
                    != self.initial_state.rip.get().checked_add(u64::from(length))
                || self.expected_exit != ExpectedExit::StopAddress
                || data.permissions
                    != (MemoryPermissions {
                        read: true,
                        write: true,
                        execute: false,
                    })
                || data_start > start
                || end >= data_end
            {
                return Err(invalid(
                    "memory",
                    "MOVSS store requires one code instruction, one readable/writable data region covering [RAX..RAX+3], budget 1, and stop immediately afterward",
                ));
            }
        }
        if self.profile == Profile::LongMovupsMemStoreV1 {
            let code = self.memory.iter().find(|region| region.region_id == "code");
            let Some(code) = code else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let code_bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let length = movups_memory_store_length(&code_bytes).ok_or_else(|| {
                invalid(
                    "memory.code",
                    "outside the [RAX] MOVUPS memory-store profile",
                )
            })?;
            let start = self.initial_state.gpr[0].get();
            let end = start
                .checked_add(15)
                .ok_or_else(|| invalid("state.gpr[0]", "MOVUPS destination range overflows u64"))?;
            let data = self.memory.iter().find(|region| region.region_id == "data");
            let Some(data) = data else {
                return Err(invalid("memory.data", "required by this profile"));
            };
            let data_start = data.guest_address.get();
            let data_end = data_start
                .checked_add(u64::from(data.length))
                .ok_or_else(|| invalid("memory.data", "range overflows u64"))?;
            let code_end = self.initial_state.rip.get().checked_add(u64::from(length));
            if self.memory.len() != 2
                || code.region_id != "code"
                || code.guest_address.get() != self.initial_state.rip.get()
                || code.length != length
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != code_end
                || self.expected_exit != ExpectedExit::StopAddress
                || !data.permissions.write
                || data.permissions.execute
                || data.region_id != "data"
                || start < data_start
                || end >= data_end
            {
                return Err(invalid(
                    "memory",
                    "MOVUPS memory-store profile requires one code instruction and one writable, non-executable data region covering [RAX..RAX+15]",
                ));
            }
        }
        if self.profile == Profile::LongMovupsMemBaseV1 {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let code_bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let cpu = self.initial_state.to_cpu_for(self.profile)?;
            let (length, store, start) =
                movups_memory_base_form(&code_bytes, &cpu).ok_or_else(|| {
                    invalid(
                        "memory.code",
                        "outside the base+displacement MOVUPS profile",
                    )
                })?;
            let end = start
                .checked_add(15)
                .ok_or_else(|| invalid("state.gpr", "MOVUPS access range overflows u64"))?;
            let data = self.memory.iter().find(|region| region.region_id == "data");
            let Some(data) = data else {
                return Err(invalid("memory.data", "required by this profile"));
            };
            let data_start = data.guest_address.get();
            let data_end = data_start
                .checked_add(u64::from(data.length))
                .ok_or_else(|| invalid("memory.data", "range overflows u64"))?;
            let code_end = self.initial_state.rip.get().checked_add(u64::from(length));
            let permission_ok = if store {
                data.permissions.write
            } else {
                data.permissions.read
            };
            if self.memory.len() != 2
                || code.region_id != "code"
                || code.guest_address.get() != self.initial_state.rip.get()
                || code.length != length
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != code_end
                || self.expected_exit != ExpectedExit::StopAddress
                || !permission_ok
                || data.permissions.execute
                || start < data_start
                || end >= data_end
            {
                return Err(invalid(
                    "memory",
                    "MOVUPS base profile requires one code instruction and one direction-appropriate data region covering the effective 16-byte range",
                ));
            }
        }
        if self.profile == Profile::LongX87StateV1 || self.profile == Profile::LongX87SseStateV1 {
            let code = self.memory.iter().find(|region| region.region_id == "code");
            let Some(code) = code else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let code_bytes = code.initial_bytes_hex.to_ascii_lowercase();
            if self.profile == Profile::LongX87SseStateV1 && code_bytes != "90" {
                return Err(invalid(
                    "memory.code",
                    "the local x87/SSE snapshot profile admits exactly one NOP",
                ));
            }
            let (expected_bytes, expected_length) = match code_bytes.as_str() {
                "90" => ("90", 1),
                "dbe3" => ("dbe3", 2),
                "dbe2" => ("dbe2", 2),
                "dbe9" => ("dbe9", 2),
                "dbe9dfe0" => ("dbe9dfe0", 4),
                "dbf1" => ("dbf1", 2),
                "dbf1dfe0" => ("dbf1dfe0", 4),
                _ if fcmovu_stream(&code_bytes) => {
                    (code_bytes.as_str(), (code_bytes.len() / 2) as u32)
                }
                _ if fcmovnu_stream(&code_bytes) => {
                    (code_bytes.as_str(), (code_bytes.len() / 2) as u32)
                }
                "dac9dac9dfe0"
                | "d9e8d9e8dbf1d9e0dac9dfe0"
                | "d9e8d9e8d9e0dbf1dac9dfe0"
                | "dbc9dbc9dfe0"
                | "d9e8d9e8dbf1d9e0dbc9dfe0"
                | "d9e8d9e8d9e0dbf1dbc9dfe0" => {
                    (code_bytes.as_str(), (code_bytes.len() / 2) as u32)
                }
                _ if fcmovbe_stream(&code_bytes) => {
                    (code_bytes.as_str(), (code_bytes.len() / 2) as u32)
                }
                _ if fcmovnbe_stream(&code_bytes) => {
                    (code_bytes.as_str(), (code_bytes.len() / 2) as u32)
                }
                _ if fcmove_family_source(&code_bytes).is_some() => {
                    (code_bytes.as_str(), (code_bytes.len() / 2) as u32)
                }
                _ if fcmovne_family_source(&code_bytes).is_some() => {
                    (code_bytes.as_str(), (code_bytes.len() / 2) as u32)
                }
                _ if fcmovbe_stream(&code_bytes) => {
                    (code_bytes.as_str(), (code_bytes.len() / 2) as u32)
                }
                _ if fcmovb_family_source(&code_bytes).is_some() => {
                    (code_bytes.as_str(), (code_bytes.len() / 2) as u32)
                }
                _ if fcmovnb_family_source(&code_bytes).is_some() => {
                    (code_bytes.as_str(), (code_bytes.len() / 2) as u32)
                }
                "dbc1dbc1dfe0" => ("dbc1dbc1dfe0", 6),
                "d9e8d9e8d9e0dbf1dbc1dfe0" => ("d9e8d9e8d9e0dbf1dbc1dfe0", 12),
                "d9e8d9e0d9e8dbf1dbc1dfe0" => ("d9e8d9e0d9e8dbf1dbc1dfe0", 12),
                "dac1dac1dfe0" => ("dac1dac1dfe0", 6),
                "d9e8d9e0dbf1dac1dfe0" => ("d9e8d9e0dbf1dac1dfe0", 10),
                "d9e8d9e0d9e8dbf1dac1dfe0" => ("d9e8d9e0d9e8dbf1dac1dfe0", 12),
                "dff1" => ("dff1", 2),
                "dff1dfe0" => ("dff1dfe0", 4),
                "dfe9" => ("dfe9", 2),
                "dfe9dfe0" => ("dfe9dfe0", 4),
                "dfe0" => ("dfe0", 2),
                _ if fcomi_family_source(&code_bytes).is_some() => {
                    (code_bytes.as_str(), (code_bytes.len() / 2) as u32)
                }
                "d9e0" => ("d9e0", 2),
                "d9e1" => ("d9e1", 2),
                "d9e4" => ("d9e4", 2),
                "d9e5" => ("d9e5", 2),
                "d9e5dfe0" => ("d9e5dfe0", 4),
                "d9e5d9e5dfe0" => ("d9e5d9e5dfe0", 6),
                "d9e8d9e5dfe0" => ("d9e8d9e5dfe0", 6),
                "d9eed9e5dfe0" => ("d9eed9e5dfe0", 6),
                "d9e8d9e0d9e5dfe0" => ("d9e8d9e0d9e5dfe0", 8),
                "d9eed9e0d9e5dfe0" => ("d9eed9e0d9e5dfe0", 8),
                "ded9" => ("ded9", 2),
                "dae9" => ("dae9", 2),
                "dae9dfe0" => ("dae9dfe0", 4),
                "dde1" => ("dde1", 2),
                "dde1dfe0" => ("dde1dfe0", 4),
                "dde9" => ("dde9", 2),
                "dde9dfe0" => ("dde9dfe0", 4),
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("d8d")
                    && matches!(code_bytes.as_bytes()[3], b'8'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("d8d")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'7') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 8
                    && code_bytes.starts_with("d8d")
                    && matches!(code_bytes.as_bytes()[3], b'8'..=b'f')
                    && code_bytes.ends_with("dfe0") =>
                {
                    (code_bytes.as_str(), 4)
                }
                _ if code_bytes.len() == 8
                    && code_bytes.starts_with("d8d")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'7')
                    && code_bytes.ends_with("dfe0") =>
                {
                    (code_bytes.as_str(), 4)
                }
                "ded9dfe0" => ("ded9dfe0", 4),
                "d9fcdfe0" => ("d9fcdfe0", 4),
                "d9fcdfe0dbe2" => ("d9fcdfe0dbe2", 6),
                "d9fcd9e0dfe0" => ("d9fcd9e0dfe0", 6),
                "d9fcd9e1dfe0" => ("d9fcd9e1dfe0", 6),
                "d910d9fcdfe0" => ("d910d9fcdfe0", 6),
                "d900d9fcdfe0" => ("d900d9fcdfe0", 6),
                "dd00d9fcdfe0" => ("dd00d9fcdfe0", 6),
                "d910d900dfe0dbe2" => ("d910d900dfe0dbe2", 8),
                "dd10dd00dfe0dbe2" => ("dd10dd00dfe0dbe2", 8),
                "db28d9fcdfe0" => ("db28d9fcdfe0", 6),
                "db28d9fcdfe0dbe2d9fc" => ("db28d9fcdfe0dbe2d9fc", 10),
                "db28d9fcd9fcdfe0dbe2d9fc" => ("db28d9fcd9fcdfe0dbe2d9fc", 12),
                "d9fc" => ("d9fc", 2),
                "d9f7" => ("d9f7", 2),
                "d9f6" => ("d9f6", 2),
                "d9e8" => ("d9e8", 2),
                "d9ee" => ("d9ee", 2),
                "d9e8dfe0" => ("d9e8dfe0", 4),
                "d9eedfe0" => ("d9eedfe0", 4),
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("d9c")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'6') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 8
                    && code_bytes.starts_with("d9c")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'6')
                    && code_bytes.ends_with("dfe0") =>
                {
                    (code_bytes.as_str(), 4)
                }
                "d9e8d9c0dfe0" => ("d9e8d9c0dfe0", 6),
                "d9eed9c0dfe0" => ("d9eed9c0dfe0", 6),
                "d9f7dfe0" => ("d9f7dfe0", 4),
                "d9f6dfe0" => ("d9f6dfe0", 4),
                "d9f7d9f6dfe0" => ("d9f7d9f6dfe0", 6),
                "d9d0" => ("d9d0", 2),
                "d9d0dfe0" => ("d9d0dfe0", 4),
                "d9d0d9d0dfe0" => ("d9d0d9d0dfe0", 6),
                "ddc7d9d0d9e8dfe0" => ("ddc7d9d0d9e8dfe0", 8),
                "ddc0" => ("ddc0", 2),
                "ddc0dfe0" => ("ddc0dfe0", 4),
                "ddc0d9c0dfe0" => ("ddc0d9c0dfe0", 6),
                "ddc0d9f7d9e8dfe0" => ("ddc0d9f7d9e8dfe0", 8),
                "ddc7d9e8dfe0" => ("ddc7d9e8dfe0", 6),
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("ddc")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'7') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 8
                    && code_bytes.starts_with("ddc")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'7')
                    && code_bytes.ends_with("dfe0") =>
                {
                    (code_bytes.as_str(), 4)
                }
                "d9f6d9f7dfe0" => ("d9f6d9f7dfe0", 6),
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("d9c")
                    && matches!(code_bytes.as_bytes()[3], b'8'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 8
                    && code_bytes.starts_with("d9c")
                    && matches!(code_bytes.as_bytes()[3], b'8'..=b'f')
                    && code_bytes.ends_with("dfe0") =>
                {
                    (code_bytes.as_str(), 4)
                }
                _ if code_bytes.len() == 12
                    && code_bytes.starts_with("d9c")
                    && matches!(code_bytes.as_bytes()[3], b'8'..=b'f')
                    && code_bytes[0..4] == code_bytes[4..8]
                    && code_bytes.ends_with("dfe0") =>
                {
                    (code_bytes.as_str(), 6)
                }
                "d928" => ("d928", 2),
                "d928d9fcdfe0" => ("d928d9fcdfe0", 6),
                "d938" => ("d938", 2),
                "d938dfe0" => ("d938dfe0", 4),
                "dd38" => ("dd38", 2),
                "dd38dfe0" => ("dd38dfe0", 4),
                "dd18" => ("dd18", 2),
                "dd18dfe0" => ("dd18dfe0", 4),
                "dd10" => ("dd10", 2),
                "dd10dfe0" => ("dd10dfe0", 4),
                "dd00" => ("dd00", 2),
                "dd00dfe0" => ("dd00dfe0", 4),
                "d918" => ("d918", 2),
                "d918dfe0" => ("d918dfe0", 4),
                "d910" => ("d910", 2),
                "d910dfe0" => ("d910dfe0", 4),
                "db18" => ("db18", 2),
                "db18dfe0" => ("db18dfe0", 4),
                "db38" => ("db38", 2),
                "db38dfe0" => ("db38dfe0", 4),
                "db28" => ("db28", 2),
                "db28dfe0" => ("db28dfe0", 4),
                "d900" => ("d900", 2),
                "d900dfe0" => ("d900dfe0", 4),
                "d900d9e0d910dfe0" => ("d900d9e0d910dfe0", 8),
                "d900d9e1d910dfe0" => ("d900d9e1d910dfe0", 8),
                "d900d9e4dfe0" => ("d900d9e4dfe0", 6),
                "d910d9e0dfe0" => ("d910d9e0dfe0", 6),
                "dd10d9e1dfe0" => ("dd10d9e1dfe0", 6),
                "d900ddd1dfe0" => ("d900ddd1dfe0", 6),
                "d900ddd9dfe0" => ("d900ddd9dfe0", 6),
                "d900dd10" => ("d900dd10", 4),
                "d900dd18dfe0" => ("d900dd18dfe0", 6),
                "db2848b80140000000000000df38dfe0dbe2" => {
                    ("db2848b80140000000000000df38dfe0dbe2", 18)
                }
                "db10" => ("db10", 2),
                "db10dfe0" => ("db10dfe0", 4),
                "db00" => ("db00", 2),
                "db00dfe0" => ("db00dfe0", 4),
                "df00" => ("df00", 2),
                "df00dfe0" => ("df00dfe0", 4),
                "df28" => ("df28", 2),
                "df28dfe0" => ("df28dfe0", 4),
                "df18" => ("df18", 2),
                "df18dfe0" => ("df18dfe0", 4),
                "df38" => ("df38", 2),
                "df38dfe0" => ("df38dfe0", 4),
                "df38dfe0dbe2" => ("df38dfe0dbe2", 6),
                "df10" => ("df10", 2),
                "df10dfe0" => ("df10dfe0", 4),
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("d8c")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("d8e")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("d8f")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'7') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("d8f")
                    && matches!(code_bytes.as_bytes()[3], b'8'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                // Bare seeded FST/FSTP ST(i) fixtures are restricted to the
                // exact DD D0+i and DD D8+i register encodings.
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("ddd")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'7' | b'8'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("dcc")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'7') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("dcc")
                    && matches!(code_bytes.as_bytes()[3], b'8'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("dce")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'7') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("dce")
                    && matches!(code_bytes.as_bytes()[3], b'8'..=b'9' | b'a'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("dcf")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'7') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("dcf")
                    && matches!(code_bytes.as_bytes()[3], b'8'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("dec")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'7') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("dec")
                    && matches!(code_bytes.as_bytes()[3], b'8'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("dee")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("def")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'7') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("def")
                    && matches!(code_bytes.as_bytes()[3], b'8'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("ddd")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'7') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("ddd")
                    && matches!(code_bytes.as_bytes()[3], b'8'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ => {
                    return Err(invalid(
                        "memory.code",
                        "x87 state profile admits one instruction or an exact listed composition",
                    ));
                }
            };
            let is_fsub_alias_unsupported = code_bytes == "d8e0"
                || code_bytes == "d8e8"
                || code_bytes == "dce8"
                || code_bytes == "dce0"
                || code_bytes == "dee0"
                || code_bytes == "dee8";
            let is_fmulp_i0_unsupported = code_bytes == "dec8" && self.stop_rip.is_none();
            let is_fdiv_inexact_unsupported = code_bytes == "d8f1" && self.stop_rip.is_none();
            let is_fdivr_inexact_unsupported = code_bytes == "d8f9" && self.stop_rip.is_none();
            let is_fdivr_sti_st0_inexact_unsupported =
                code_bytes == "dcf1" && self.stop_rip.is_none();
            let is_fdiv_sti_st0_inexact_unsupported =
                code_bytes == "dcf9" && self.stop_rip.is_none();
            let is_fdivp_inexact_unsupported = code_bytes == "def9" && self.stop_rip.is_none();
            let is_fdivrp_inexact_unsupported = code_bytes == "def1" && self.stop_rip.is_none();
            let is_fstp_source_rejected = code_bytes == "ddd9" && self.stop_rip.is_none();
            let is_fst_source_rejected = code_bytes == "ddd1" && self.stop_rip.is_none();
            let is_fxch_source_rejected = code_bytes.len() == 4
                && code_bytes.starts_with("d9c")
                && matches!(code_bytes.as_bytes()[3], b'8'..=b'f')
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u8;
                    let source = (code_bytes.as_bytes()[3] as char).to_digit(16).unwrap() as u8 & 7;
                    let indexed = (top + source) & 7;
                    let tag = |index: u8| (x87.tag_word.get() >> (u32::from(index) * 2)) & 3;
                    let valid_operand = |index: u8| {
                        let Some(encoded) = x87.registers.get(usize::from(index)) else {
                            return false;
                        };
                        let Ok(bytes) = decode_bytes(encoded) else {
                            return false;
                        };
                        if bytes.len() != 10 {
                            return false;
                        }
                        let significand = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                        let exponent = (u16::from(bytes[9] & 0x7f) << 8) | u16::from(bytes[8]);
                        match tag(index) {
                            0 => (1..0x7fff).contains(&exponent) && significand & (1 << 63) != 0,
                            1 => significand == 0 && exponent == 0,
                            2 | 3 => false,
                            _ => false,
                        }
                    };
                    !valid_operand(top) || (source != 0 && !valid_operand(indexed))
                });
            let is_top_adjustment_source_rejected = matches!(code_bytes.as_str(), "d9f7" | "d9f6")
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    x87.control_word.get() != 0x037f || x87.status_word.get() & 0x80ff != 0
                });
            let is_fld_constant_destination_rejected =
                matches!(code_bytes.as_str(), "d9e8" | "d9ee")
                    && self.stop_rip.is_none()
                    && self.instruction_budget == 1
                    && self.initial_state.x87.as_ref().is_some_and(|x87| {
                        let top = ((x87.status_word.get() >> 11) & 7) as u8;
                        let destination = top.wrapping_add(7) & 7;
                        x87.control_word.get() != 0x037f
                            || x87.status_word.get() & 0x80ff != 0
                            || (x87.tag_word.get() >> (u32::from(destination) * 2)) & 3 != 3
                    });
            let is_fld_sti_rejected = code_bytes.len() == 4
                && code_bytes.starts_with("d9c")
                && matches!(code_bytes.as_bytes()[3], b'0'..=b'6')
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as usize;
                    let source = (top + usize::from(code_bytes.as_bytes()[3] - b'0')) & 7;
                    let destination = (top + 7) & 7;
                    let tag = |index: usize| (x87.tag_word.get() >> (index * 2)) & 3;
                    let raw_is_canonical = |index: usize| {
                        let Some(encoded) = x87.registers.get(index) else {
                            return false;
                        };
                        let Ok(bytes) = decode_bytes(encoded) else {
                            return false;
                        };
                        if bytes.len() != 10 {
                            return false;
                        }
                        let significand = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                        let exponent = (u16::from(bytes[9] & 0x7f) << 8) | u16::from(bytes[8]);
                        match tag(index) {
                            0 => (1..0x7fff).contains(&exponent) && significand & (1 << 63) != 0,
                            1 => significand == 0 && exponent == 0,
                            _ => false,
                        }
                    };
                    x87.control_word.get() != 0x037f
                        || x87.status_word.get() & 0x80ff != 0
                        || !raw_is_canonical(source)
                        || tag(destination) != 3
                });
            let is_fchs_source_rejected = code_bytes == "d9e0" && self.stop_rip.is_none();
            let is_fabs_source_rejected = code_bytes == "d9e1" && self.stop_rip.is_none();
            let is_ftst_source_rejected = code_bytes == "d9e4" && self.stop_rip.is_none();
            let is_frndint_source_rejected = code_bytes == "d9fc"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u8;
                    (x87.tag_word.get() >> (u32::from(top) * 2)) & 3 != 0
                });
            let is_fnstcw_noncanonical_range_exception = code_bytes == "d938"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.gpr[0].get() == 0x0000_7fff_ffff_ffff;
            let is_fnstsw_noncanonical_range_exception = code_bytes == "dd38"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.gpr[0].get() == 0x0000_7fff_ffff_ffff;
            let is_fldcw_old_environment_rejected = code_bytes == "d928"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    !matches!(x87.control_word.get(), 0x037f | 0x077f | 0x0b7f | 0x0f7f)
                        || x87.status_word.get() & 0x80df != 0
                });
            let is_fldcw_noncanonical_range_exception = code_bytes == "d928"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && !is_fldcw_old_environment_rejected
                && self.initial_state.gpr[0].get() == 0x0000_7fff_ffff_ffff;
            let is_fistp_overflow_rejected = code_bytes == "db18"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u8;
                    let raw = &x87.registers[usize::from(top)];
                    // This schema gate recognizes the literal +2^32 overflow
                    // witness only; execution owns general RC/range semantics.
                    raw.eq_ignore_ascii_case("00000000000000801f40")
                });
            let is_fistp_tag_rejected = code_bytes == "db18"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u32;
                    (x87.tag_word.get() >> (top * 2)) & 3 != 0
                });
            let is_fist_overflow_rejected = code_bytes == "db10"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u8;
                    let raw = &x87.registers[usize::from(top)];
                    // This schema gate recognizes the literal +2^32 overflow
                    // witness only; execution owns general RC/range semantics.
                    raw.eq_ignore_ascii_case("00000000000000801f40")
                });
            let is_fist_tag_rejected = code_bytes == "db10"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u32;
                    (x87.tag_word.get() >> (top * 2)) & 3 != 0
                });
            let is_fistp_word_overflow_rejected = code_bytes == "df18"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u8;
                    let raw = &x87.registers[usize::from(top)];
                    raw.eq_ignore_ascii_case("00000000000000800e40")
                });
            let is_fistp_word_exponent_rejected = code_bytes == "df18"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u8;
                    let raw = &x87.registers[usize::from(top)];
                    raw.eq_ignore_ascii_case("00000000000000800f40")
                });
            let is_fistp_word_tag_rejected = code_bytes == "df18"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u32;
                    (x87.tag_word.get() >> (top * 2)) & 3 != 0
                });
            let is_fistp_word_environment_rejected = code_bytes == "df18"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    !matches!(x87.control_word.get(), 0x037f | 0x077f | 0x0b7f | 0x0f7f)
                        || x87.status_word.get() & 0x80df != 0
                });
            let is_fistp_qword_overflow_rejected = code_bytes == "df38"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as usize;
                    x87.registers[top].eq_ignore_ascii_case("00000000000000803e40")
                });
            let is_fist_word_overflow_rejected = code_bytes == "df10"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as usize;
                    x87.registers[top].eq_ignore_ascii_case("00000000000000800e40")
                });
            let is_fist_word_exponent_rejected = code_bytes == "df10"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as usize;
                    x87.registers[top].eq_ignore_ascii_case("00000000000000800f40")
                });
            let is_fist_word_tag_rejected = code_bytes == "df10"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u32;
                    (x87.tag_word.get() >> (top * 2)) & 3 != 0
                });
            let is_fist_word_environment_rejected = code_bytes == "df10"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    !matches!(x87.control_word.get(), 0x037f | 0x077f | 0x0b7f | 0x0f7f)
                        || x87.status_word.get() & 0x80df != 0
                });
            let is_fld_m80_pseudo_infinity_rejected = code_bytes == "db28"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.memory.iter().any(|region| {
                    let address = self.initial_state.gpr[0].get();
                    let offset = address.checked_sub(region.guest_address.get());
                    offset.is_some_and(|offset| {
                        let start = usize::try_from(offset)
                            .ok()
                            .and_then(|offset| offset.checked_mul(2));
                        start.is_some_and(|start| {
                            start.checked_add(20).is_some_and(|end| {
                                region.initial_bytes_hex.len() >= end && {
                                    match region.initial_bytes_hex.get(start..end) {
                                        Some(raw) => {
                                            let mut bytes = [0_u8; 10];
                                            let decoded = bytes
                                                .iter_mut()
                                                .zip(raw.as_bytes().chunks_exact(2))
                                                .all(|(byte, pair)| {
                                                    std::str::from_utf8(pair)
                                                        .ok()
                                                        .and_then(|pair| {
                                                            u8::from_str_radix(pair, 16).ok()
                                                        })
                                                        .map(|value| *byte = value)
                                                        .is_some()
                                                });
                                            if !decoded {
                                                false
                                            } else {
                                                let significand = u64::from_le_bytes(
                                                    bytes[..8].try_into().expect("binary80 width"),
                                                );
                                                let sign_exponent =
                                                    u16::from_le_bytes([bytes[8], bytes[9]]);
                                                sign_exponent == 0x7fff && significand == 0
                                            }
                                        }
                                        None => false,
                                    }
                                }
                            })
                        })
                    })
                });
            let is_fld_m32_environment_rejected = code_bytes == "d900"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    !matches!(x87.control_word.get(), 0x037f | 0x077f | 0x0b7f | 0x0f7f)
                        || x87.status_word.get() & 0x80c4 != 0
                });
            let is_fld_m64_environment_rejected = code_bytes == "dd00"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    !matches!(x87.control_word.get(), 0x037f | 0x077f | 0x0b7f | 0x0f7f)
                        || x87.status_word.get() & 0x80c4 != 0
                });
            let is_fcom_source_rejected = code_bytes.len() == 4
                && code_bytes.starts_with("d8d")
                && matches!(code_bytes.as_bytes()[3], b'0'..=b'7')
                && self.stop_rip.is_none();
            let is_fcomp_source_rejected = code_bytes.len() == 4
                && code_bytes.starts_with("d8d")
                && matches!(code_bytes.as_bytes()[3], b'8'..=b'f')
                && self.stop_rip.is_none();
            let is_fcompp_source_rejected = code_bytes == "ded9"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u8;
                    let rhs = (top + 1) & 7;
                    (x87.tag_word.get() >> (u32::from(rhs) * 2)) & 3 != 0
                });
            let is_fucompp_source_rejected = code_bytes == "dae9"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u8;
                    let rhs = (top + 1) & 7;
                    (x87.tag_word.get() >> (u32::from(rhs) * 2)) & 3 != 0
                });
            let is_fucom_source_rejected = code_bytes == "dde1"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u8;
                    let rhs = (top + 1) & 7;
                    (x87.tag_word.get() >> (u32::from(rhs) * 2)) & 3 != 0
                });
            let is_fucomp_source_rejected = code_bytes == "dde9"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u8;
                    let rhs = (top + 1) & 7;
                    (x87.tag_word.get() >> (u32::from(rhs) * 2)) & 3 != 0
                });
            let is_fcomi_source_rejected = fcomi_family_source(&code_bytes).is_some_and(|source| {
                self.stop_rip.is_none()
                    && self.instruction_budget == 1
                    && self.initial_state.x87.as_ref().is_some_and(|x87| {
                        let top = ((x87.status_word.get() >> 11) & 7) as u8;
                        let actual = (top + source) & 7;
                        let tag = |index: u8| (x87.tag_word.get() >> (u32::from(index) * 2)) & 3;
                        x87.control_word.get() == 0x037f
                            && x87.status_word.get() & 0x80ff == 0
                            && tag(top) == 0
                            && if source == 0 {
                                x87.registers[usize::from(top)] == "00000000000000000000"
                                    && tag((top + 1) & 7) == 0
                            } else {
                                tag(actual) != 0 && (source == 1 || tag((top + 1) & 7) == 0)
                            }
                    })
            });
            let is_fcmovb_operand_rejected = fcmovb_family_source(&code_bytes).is_some()
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u8;
                    let source = (top + fcmovb_family_source(&code_bytes).unwrap()) & 7;
                    let valid_operand = |index: u8| {
                        let tag = (x87.tag_word.get() >> (u32::from(index) * 2)) & 3;
                        let Some(encoded) = x87.registers.get(usize::from(index)) else {
                            return false;
                        };
                        let Ok(bytes) = decode_bytes(encoded) else {
                            return false;
                        };
                        if bytes.len() != 10 {
                            return false;
                        }
                        let significand = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                        let exponent = u16::from_le_bytes([bytes[8], bytes[9] & 0x7f]);
                        match tag {
                            0 => (1..0x7fff).contains(&exponent) && significand & (1 << 63) != 0,
                            1 => significand == 0 && exponent == 0 && matches!(bytes[9], 0 | 0x80),
                            _ => false,
                        }
                    };
                    !valid_operand(top) || !valid_operand(source)
                });
            let is_fcmovb_environment_rejected = fcmovb_family_source(&code_bytes).is_some()
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    x87.control_word.get() != 0x037f || x87.status_word.get() & 0x82ff != 0
                });
            let is_fcmovnb_operand_rejected = fcmovnb_family_source(&code_bytes).is_some()
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as usize;
                    let source =
                        (top + usize::from(fcmovnb_family_source(&code_bytes).unwrap())) & 7;
                    let valid_operand = |index: usize| {
                        let tag = (x87.tag_word.get() >> (index * 2)) & 3;
                        let Ok(bytes) = decode_bytes(&x87.registers[index]) else {
                            return false;
                        };
                        if bytes.len() != 10 {
                            return false;
                        }
                        let significand = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                        let exponent = u16::from_le_bytes([bytes[8], bytes[9] & 0x7f]);
                        match tag {
                            0 => (1..0x7fff).contains(&exponent) && significand & (1 << 63) != 0,
                            1 => significand == 0 && exponent == 0 && matches!(bytes[9], 0 | 0x80),
                            _ => false,
                        }
                    };
                    !valid_operand(top) || (source != top && !valid_operand(source))
                });
            let is_fcmovnb_environment_rejected = fcmovnb_family_source(&code_bytes).is_some()
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    x87.control_word.get() != 0x037f || x87.status_word.get() & 0x82ff != 0
                });
            let fcmove_index = (code_bytes.len() == 4
                && code_bytes.starts_with("dac")
                && matches!(code_bytes.as_bytes()[3], b'8'..=b'9' | b'a'..=b'f'))
            .then(|| u8::from_str_radix(&code_bytes[3..4], 16).unwrap() & 7);
            let is_fcmove_operand_rejected = fcmove_index.is_some()
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as usize;
                    let source = (top + usize::from(fcmove_index.unwrap())) & 7;
                    let valid_operand = |index: usize| {
                        let tag = (x87.tag_word.get() >> (index * 2)) & 3;
                        let Ok(bytes) = decode_bytes(&x87.registers[index]) else {
                            return false;
                        };
                        if bytes.len() != 10 {
                            return false;
                        }
                        let significand = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                        let exponent = u16::from_le_bytes([bytes[8], bytes[9] & 0x7f]);
                        match tag {
                            0 => (1..0x7fff).contains(&exponent) && significand & (1 << 63) != 0,
                            1 => significand == 0 && exponent == 0 && matches!(bytes[9], 0 | 0x80),
                            _ => false,
                        }
                    };
                    !valid_operand(top) || (fcmove_index != Some(0) && !valid_operand(source))
                });
            let is_fcmove_environment_rejected = fcmove_index.is_some()
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    x87.control_word.get() != 0x037f || x87.status_word.get() & 0x82ff != 0
                });
            let is_fcmovne_operand_rejected = fcmovne_family_source(&code_bytes).is_some()
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as usize;
                    let source =
                        (top + usize::from(fcmovne_family_source(&code_bytes).unwrap())) & 7;
                    let valid_operand = |index: usize| {
                        let tag = (x87.tag_word.get() >> (index * 2)) & 3;
                        let Ok(bytes) = decode_bytes(&x87.registers[index]) else {
                            return false;
                        };
                        if bytes.len() != 10 {
                            return false;
                        }
                        let significand = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                        let exponent = u16::from_le_bytes([bytes[8], bytes[9] & 0x7f]);
                        match tag {
                            0 => (1..0x7fff).contains(&exponent) && significand & (1 << 63) != 0,
                            1 => significand == 0 && exponent == 0 && matches!(bytes[9], 0 | 0x80),
                            _ => false,
                        }
                    };
                    !valid_operand(top) || !valid_operand(source)
                });
            let is_fcmovne_environment_rejected = fcmovne_family_source(&code_bytes).is_some()
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    x87.control_word.get() != 0x037f || x87.status_word.get() & 0x82ff != 0
                });
            let fcmovbe_source = fcmovbe_bare_source(&code_bytes);
            let is_fcmovbe_operand_rejected = fcmovbe_source.is_some()
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as usize;
                    let source = (top + usize::from(fcmovbe_source.unwrap())) & 7;
                    let valid_operand = |index: usize| {
                        let tag = (x87.tag_word.get() >> (index * 2)) & 3;
                        let Ok(bytes) = decode_bytes(&x87.registers[index]) else {
                            return false;
                        };
                        if bytes.len() != 10 {
                            return false;
                        }
                        let significand = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                        let exponent = u16::from_le_bytes([bytes[8], bytes[9] & 0x7f]);
                        match tag {
                            0 => (1..0x7fff).contains(&exponent) && significand & (1 << 63) != 0,
                            1 => significand == 0 && exponent == 0 && matches!(bytes[9], 0 | 0x80),
                            _ => false,
                        }
                    };
                    !valid_operand(top) || (fcmovbe_source != Some(0) && !valid_operand(source))
                });
            let is_fcmovbe_environment_rejected = fcmovbe_source.is_some()
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    x87.control_word.get() != 0x037f || x87.status_word.get() & 0x82ff != 0
                });
            let fcmovnbe_source = fcmovnbe_bare_source(&code_bytes);
            let is_fcmovnbe_operand_rejected = fcmovnbe_source.is_some()
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as usize;
                    let source = (top + usize::from(fcmovnbe_source.unwrap())) & 7;
                    let valid_operand = |index: usize| {
                        let tag = (x87.tag_word.get() >> (index * 2)) & 3;
                        let Ok(bytes) = decode_bytes(&x87.registers[index]) else {
                            return false;
                        };
                        if bytes.len() != 10 {
                            return false;
                        }
                        let significand = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                        let exponent = u16::from_le_bytes([bytes[8], bytes[9] & 0x7f]);
                        match tag {
                            0 => (1..0x7fff).contains(&exponent) && significand & (1 << 63) != 0,
                            1 => significand == 0 && exponent == 0 && matches!(bytes[9], 0 | 0x80),
                            _ => false,
                        }
                    };
                    !valid_operand(top) || !valid_operand(source)
                });
            let is_fcmovnbe_environment_rejected = fcmovnbe_source.is_some()
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    x87.control_word.get() != 0x037f || x87.status_word.get() & 0x82ff != 0
                });
            let fcmovnu_source = fcmovnu_bare_source(&code_bytes);
            let fcmovnu_operand_rejected = fcmovnu_source.is_some()
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as usize;
                    let source = (top + usize::from(fcmovnu_source.unwrap())) & 7;
                    let valid_operand = |index: usize| {
                        let tag = (x87.tag_word.get() >> (index * 2)) & 3;
                        let Ok(bytes) = decode_bytes(&x87.registers[index]) else {
                            return false;
                        };
                        if bytes.len() != 10 {
                            return false;
                        }
                        let significand = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                        let exponent = u16::from_le_bytes([bytes[8], bytes[9] & 0x7f]);
                        match tag {
                            0 => (1..0x7fff).contains(&exponent) && significand & (1 << 63) != 0,
                            1 => significand == 0 && exponent == 0 && matches!(bytes[9], 0 | 0x80),
                            _ => false,
                        }
                    };
                    !valid_operand(top) || (fcmovnu_source.unwrap() != 0 && !valid_operand(source))
                });
            let fcmovnu_environment_rejected = fcmovnu_source.is_some()
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    x87.control_word.get() != 0x037f || x87.status_word.get() & 0x82ff != 0
                });
            let fcmovu_source = fcmovu_bare_source(&code_bytes);
            let is_fcmovu_operand_rejected = fcmovu_source.is_some()
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as usize;
                    let source = (top + usize::from(fcmovu_source.unwrap())) & 7;
                    let valid_operand = |index: usize| {
                        let tag = (x87.tag_word.get() >> (index * 2)) & 3;
                        let Ok(bytes) = decode_bytes(&x87.registers[index]) else {
                            return false;
                        };
                        if bytes.len() != 10 {
                            return false;
                        }
                        let significand = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                        let exponent = u16::from_le_bytes([bytes[8], bytes[9] & 0x7f]);
                        match tag {
                            0 => (1..0x7fff).contains(&exponent) && significand & (1 << 63) != 0,
                            1 => significand == 0 && exponent == 0 && matches!(bytes[9], 0 | 0x80),
                            _ => false,
                        }
                    };
                    !valid_operand(top) || (fcmovu_source != Some(0) && !valid_operand(source))
                });
            let is_fcmovu_environment_rejected = fcmovu_source.is_some()
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    x87.control_word.get() != 0x037f || x87.status_word.get() & 0x82ff != 0
                });
            let is_ffree_environment_rejected = code_bytes.len() == 4
                && code_bytes.starts_with("ddc")
                && matches!(code_bytes.as_bytes()[3], b'0'..=b'7')
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    x87.control_word.get() != 0x037f || x87.status_word.get() & 0x80ff != 0
                });
            let is_fnop_environment_rejected = code_bytes == "d9d0"
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    x87.control_word.get() != 0x037f || x87.status_word.get() & 0x80ff != 0
                });
            let is_fxam_environment_rejected = code_bytes == "d9e5"
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    x87.control_word.get() != 0x037f
                        || x87.status_word.get() & 0x80ff != 0
                        || x87.tag_word.get() >> (2 * (((x87.status_word.get() >> 11) & 7) as u32))
                            & 3
                            != 0
                });
            let is_ffree_fld_st0_rejected = code_bytes == "ddc0d9c0dfe0"
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 3;
            let expected_budget = match code_bytes.as_str() {
                _ if fcmovu_stream(&code_bytes) => fcmovu_budget(&code_bytes).unwrap(),
                _ if fcmovnu_stream(&code_bytes) => fcmovnu_budget(&code_bytes).unwrap(),
                "db2848b80140000000000000df38dfe0dbe2" | "db28d9fcdfe0dbe2d9fc" => 5,
                "db28d9fcd9fcdfe0dbe2d9fc"
                | "d9e8d9e0dbf1dac1dfe0"
                | "d9e8d9e0d9e8dbf1dac1dfe0"
                | "d9e8d9e8dbf1d9e0dac9dfe0"
                | "d9e8d9e8d9e0dbf1dac9dfe0"
                | "d9e8d9e8dbf1d9e0dbc9dfe0"
                | "d9e8d9e8d9e0dbf1dbc9dfe0"
                | "d9e8d9e0d9e8dbf1dad1dfe0"
                | "d9e8d9e8dbf1d9e0dad1dfe0"
                | "d9e8d9e8d9e0dbf1dad1dfe0"
                | "d9e8d9e8d9e0dbf1dbc1dfe0"
                | "d9e8d9e0d9e8dbf1dbc1dfe0" => 6,
                _ if fcmovnb_family_source(&code_bytes).is_some() => {
                    if code_bytes.len() == 4 {
                        1
                    } else {
                        2
                    }
                }
                "dac1dac1dfe0" => 3,
                "dac9dac9dfe0" => 3,
                "dad1dad1dfe0" => 3,
                "dbd1dbd1dfe0" => 3,
                "dbc9dbc9dfe0" => 3,
                _ if fcmove_family_source(&code_bytes).is_some() => {
                    if code_bytes.len() == 4 {
                        1
                    } else {
                        2
                    }
                }
                _ if fcmovne_family_source(&code_bytes).is_some() => {
                    if code_bytes.len() == 4 {
                        1
                    } else {
                        2
                    }
                }
                _ if fcmovnbe_stream(&code_bytes) => match code_bytes.as_str() {
                    _ if code_bytes.len() == 4 => 1,
                    _ if code_bytes.len() == 8 && code_bytes.ends_with("dfe0") => 2,
                    "dbd1dbd1dfe0" => 3,
                    _ => 6,
                },
                _ if fcmovbe_stream(&code_bytes) => {
                    if code_bytes.len() == 4 {
                        1
                    } else if code_bytes.len() == 8 && code_bytes.starts_with("da") {
                        2
                    } else if code_bytes == "dad1dad1dfe0" {
                        3
                    } else {
                        6
                    }
                }
                "dac1dfe0" | "dbc1dfe0" => 2,
                "dbc1dbc1dfe0" => 3,
                "dbc1" => 1,
                "d9fcdfe0dbe2" => 3,
                "ddc0d9c0dfe0" => 3,
                "ddc0d9f7d9e8dfe0" => 4,
                "d9d0" | "d9e5" => 1,
                "d9d0dfe0" => 2,
                "d9d0d9d0dfe0" => 3,
                "ddc7d9d0d9e8dfe0" => 4,
                "d9e5dfe0" => 2,
                "d9e5d9e5dfe0" => 3,
                "d9e8d9e5dfe0" | "d9eed9e5dfe0" => 3,
                "d9e8d9e0d9e5dfe0" | "d9eed9e0d9e5dfe0" => 4,
                "d910d900dfe0dbe2" | "dd10dd00dfe0dbe2" => 4,
                "d900d9e0d910dfe0" | "d900d9e1d910dfe0" => 4,
                "d900d9e4dfe0" | "d900ddd1dfe0" | "d900ddd9dfe0" | "d910d9e0dfe0"
                | "dd10d9e1dfe0" => 3,
                _ => match code_bytes.len() {
                    8 => 2,
                    12 => 3,
                    _ => 1,
                },
            };
            if code.guest_address != self.initial_state.rip
                || code.length != expected_length
                || !code.permissions.execute
                || !code.initial_bytes_hex.eq_ignore_ascii_case(expected_bytes)
                || self.instruction_budget != expected_budget
                || if is_fsub_alias_unsupported
                    || is_fmulp_i0_unsupported
                    || is_fdiv_inexact_unsupported
                    || is_fdivr_inexact_unsupported
                    || is_fdivr_sti_st0_inexact_unsupported
                    || is_fdiv_sti_st0_inexact_unsupported
                    || is_fdivp_inexact_unsupported
                    || is_fdivrp_inexact_unsupported
                    || is_fstp_source_rejected
                    || is_fst_source_rejected
                    || is_fxch_source_rejected
                    || is_top_adjustment_source_rejected
                    || is_fld_constant_destination_rejected
                    || is_fld_sti_rejected
                    || is_fchs_source_rejected
                    || is_fabs_source_rejected
                    || is_ftst_source_rejected
                    || is_frndint_source_rejected
                    || is_fnstcw_noncanonical_range_exception
                    || is_fnstsw_noncanonical_range_exception
                    || is_fldcw_noncanonical_range_exception
                    || is_fldcw_old_environment_rejected
                    || is_fistp_overflow_rejected
                    || is_fistp_tag_rejected
                    || is_fist_overflow_rejected
                    || is_fist_tag_rejected
                    || is_fistp_word_overflow_rejected
                    || is_fistp_word_exponent_rejected
                    || is_fistp_word_tag_rejected
                    || is_fistp_word_environment_rejected
                    || is_fistp_qword_overflow_rejected
                    || is_fist_word_overflow_rejected
                    || is_fist_word_exponent_rejected
                    || is_fist_word_tag_rejected
                    || is_fist_word_environment_rejected
                    || is_fld_m80_pseudo_infinity_rejected
                    || is_fld_m32_environment_rejected
                    || is_fld_m64_environment_rejected
                    || is_fcom_source_rejected
                    || is_fcomp_source_rejected
                    || is_fcompp_source_rejected
                    || is_fucompp_source_rejected
                    || is_fucom_source_rejected
                    || is_fucomp_source_rejected
                    || is_fcomi_source_rejected
                    || is_fcmovb_operand_rejected
                    || is_fcmovb_environment_rejected
                    || is_fcmovnb_operand_rejected
                    || is_fcmovnb_environment_rejected
                    || is_fcmove_operand_rejected
                    || is_fcmove_environment_rejected
                    || is_fcmovne_operand_rejected
                    || is_fcmovne_environment_rejected
                    || is_fcmovbe_operand_rejected
                    || is_fcmovbe_environment_rejected
                    || is_fcmovnbe_operand_rejected
                    || is_fcmovnbe_environment_rejected
                    || is_fcmovu_operand_rejected
                    || is_fcmovu_environment_rejected
                    || fcmovnu_operand_rejected
                    || fcmovnu_environment_rejected
                    || is_ffree_environment_rejected
                    || is_fnop_environment_rejected
                    || is_fxam_environment_rejected
                    || is_ffree_fld_st0_rejected
                {
                    self.stop_rip.is_some()
                        || self.expected_exit
                            != if is_fnstcw_noncanonical_range_exception
                                || is_fnstsw_noncanonical_range_exception
                                || is_fldcw_noncanonical_range_exception
                            {
                                ExpectedExit::Exception
                            } else {
                                ExpectedExit::Unsupported
                            }
                } else {
                    self.stop_rip.map(HexU64::get)
                        != self
                            .initial_state
                            .rip
                            .get()
                            .checked_add(u64::from(expected_length))
                        || self.expected_exit != ExpectedExit::StopAddress
                }
            {
                return Err(invalid(
                    "memory.code",
                    "x87 state profile admits one exact instruction with budget 1 and the following stop RIP",
                ));
            }
        }
        // RIP is architectural u64. Unmapped/noncanonical entries are valid fault tests.
        Ok(())
    }

    pub fn convert_v1(old: &crate::case::LoadedFixture) -> Result<Self, FormatError> {
        old.fixture.validate()?;
        digest_field(&old.input_sha256)?;
        let source = &old.fixture;
        let initial = &source.initial_state;
        let mut state = LongModeState::default();
        state.gpr[..4].copy_from_slice(&[
            initial.rax.get(),
            initial.rcx.get(),
            initial.rdx.get(),
            initial.rbx.get(),
        ]);
        state.rip = initial.rip.get();
        state.rflags = initial.rflags.get();
        let length = source.program.bytes()?.len() as u32;
        let fixture = Self {
            schema_version: SCHEMA_VERSION,
            case_id: source.case_id.clone(),
            profile: Profile::LongIntegerV1,
            provenance: source.provenance.clone(),
            seed: source.seed.map(HexU64),
            converted_from_v1_sha256: Some(old.input_sha256.clone()),
            initial_state: IntegerState::from(&state),
            memory: vec![MemoryRegionSpec {
                region_id: "code".into(),
                guest_address: HexU64(0),
                length,
                permissions: MemoryPermissions {
                    read: true,
                    write: false,
                    execute: true,
                },
                initial_bytes_hex: source.program.bytes_hex.clone(),
                comparison_mask_hex: "ff".repeat(length as usize),
            }],
            stop_rip: Some(HexU64(u64::from(length))),
            instruction_budget: source.instruction_budget,
            wall_timeout_ms: source.wall_timeout_ms,
            rflags_mask: source.comparison.rflags_mask,
            expected_exit: match source.expected_exit {
                crate::case::ExpectedExit::ReachedCodeEnd => ExpectedExit::StopAddress,
                crate::case::ExpectedExit::InstructionBudgetExhausted => {
                    ExpectedExit::InstructionBudgetExhausted
                }
            },
        };
        fixture.validate()?;
        Ok(fixture)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Outcome {
    StopAddress {},
    InstructionBudgetExhausted {},
    RepIterationBudgetExhausted {},
    StopRequested {},
    Exception {},
    Unsupported { reason: String },
    TimedOut { reason: String },
    Signaled { signal: u32, reason: String },
    HarnessError { reason: String },
}

impl Outcome {
    fn architectural(&self) -> bool {
        matches!(
            self,
            Self::StopAddress {}
                | Self::InstructionBudgetExhausted {}
                | Self::RepIterationBudgetExhausted {}
                | Self::StopRequested {}
                | Self::Exception {}
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub state: IntegerState,
    pub rflags_defined: HexU64,
    pub memory: Vec<MemoryRegionSnapshot>,
    /// Available(None) means observed no exception; unavailable is not an empty result.
    pub exception: Observation<Option<ExceptionSnapshot>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunResult {
    pub schema_version: u32,
    pub case_id: String,
    pub input_sha256: String,
    pub profile: Profile,
    pub backend: Backend,
    pub outcome: Outcome,
    pub instructions_executed: Observation<HexU64>,
    pub snapshot: Observation<Snapshot>,
    pub build: BuildMetadata,
    pub machine: MachineMetadata,
}

impl RunResult {
    pub fn from_json(bytes: &[u8]) -> Result<Self, FormatError> {
        let result: Self = load(bytes, MAX_RESULT_JSON_BYTES)?;
        result.validate()?;
        Ok(result)
    }

    pub fn to_json(&self) -> Result<Vec<u8>, FormatError> {
        self.validate()?;
        save(self, MAX_RESULT_JSON_BYTES)
    }

    pub fn validate(&self) -> Result<(), FormatError> {
        version(self.schema_version)?;
        validate_case_id(&self.case_id)?;
        digest_field(&self.input_sha256)?;
        for value in [
            &self.build.revision,
            &self.build.target,
            &self.build.profile,
            &self.build.compiler,
            &self.machine.host_os,
            &self.machine.host_arch,
        ] {
            text(value, "metadata")?;
        }
        for value in [&self.machine.cpu_model, &self.machine.kernel]
            .into_iter()
            .flatten()
        {
            text(value, "metadata")?;
        }
        if let Outcome::Unsupported { reason }
        | Outcome::TimedOut { reason }
        | Outcome::Signaled { reason, .. }
        | Outcome::HarnessError { reason } = &self.outcome
        {
            text(reason, "outcome.reason")?;
        }
        if let Outcome::Signaled { signal: 0, .. } = self.outcome {
            return Err(invalid("outcome.signal", "must be nonzero"));
        }
        if let Observation::Unavailable { reason } = &self.instructions_executed {
            text(reason, "instructions_executed")?;
        }
        let snapshot = match &self.snapshot {
            Observation::Available { value } => value,
            Observation::Unavailable { reason } => {
                text(reason, "snapshot")?;
                if self.outcome.architectural() {
                    return Err(invalid(
                        "snapshot",
                        "architectural exit requires observed state",
                    ));
                }
                return Ok(());
            }
        };
        snapshot.state.validate_for(self.profile)?;
        if snapshot.rflags_defined.get() & !INTEGER_FLAGS != 0 {
            return Err(invalid("snapshot.rflags_defined", "outside profile"));
        }
        if snapshot.memory.len() > crate::MAX_MEMORY_REGIONS {
            return Err(invalid("snapshot.memory", "too many regions"));
        }
        let mut total = 0usize;
        let mut ids = std::collections::HashSet::new();
        let mut ranges = Vec::new();
        for region in &snapshot.memory {
            validate_case_id(&region.region_id)?;
            if !ids.insert(&region.region_id) {
                return Err(invalid("snapshot.memory", "duplicate region"));
            }
            validate_hex_bytes(&region.bytes_hex, "snapshot.memory.bytes_hex")?;
            if region.bytes_hex.is_empty() {
                return Err(invalid("snapshot.memory", "empty region"));
            }
            let start = region.guest_address.get();
            let end = start
                .checked_add((region.bytes_hex.len() / 2) as u64)
                .ok_or_else(|| invalid("snapshot.memory", "address overflow"))?;
            if ranges.iter().any(|(lo, hi)| start < *hi && *lo < end) {
                return Err(invalid("snapshot.memory", "overlapping regions"));
            }
            ranges.push((start, end));
            total = total.saturating_add(region.bytes_hex.len() / 2);
            if total > crate::MAX_MEMORY_BYTES
                || region
                    .guest_address
                    .get()
                    .checked_add((region.bytes_hex.len() / 2) as u64)
                    .is_none()
            {
                return Err(invalid("snapshot.memory", "size limit or address overflow"));
            }
        }
        match &snapshot.exception {
            Observation::Available { value } => {
                if value.is_some() != matches!(self.outcome, Outcome::Exception {}) {
                    return Err(invalid("snapshot.exception", "conflicts with outcome"));
                }
                if let Some(fault) = value {
                    if fault.vector > 31 || fault.faulting_rip != snapshot.state.rip {
                        return Err(invalid("snapshot.exception", "invalid vector or fault RIP"));
                    }
                    for field in [&fault.error_code, &fault.fault_address] {
                        if let Observation::Unavailable { reason } = field {
                            text(reason, "snapshot.exception")?;
                        }
                    }
                    if matches!(fault.vector, 8 | 10 | 11 | 12 | 13 | 14 | 17 | 21 | 29 | 30)
                        && !fault.error_code.is_available()
                    {
                        return Err(invalid(
                            "snapshot.exception.error_code",
                            "architectural error code is unavailable",
                        ));
                    }
                    if fault.vector == 14 && !fault.fault_address.is_available() {
                        return Err(invalid(
                            "snapshot.exception.fault_address",
                            "page-fault address is unavailable",
                        ));
                    }
                }
            }
            Observation::Unavailable { reason } => {
                text(reason, "snapshot.exception")?;
                if self.outcome.architectural() {
                    return Err(invalid(
                        "snapshot.exception",
                        "architectural exit requires exception observation",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn validate_for(&self, loaded: &LoadedFixture) -> Result<(), FormatError> {
        loaded.fixture.validate()?;
        self.validate()?;
        if self.input_sha256 != loaded.input_sha256 {
            return Err(FormatError::CaseMismatch("input_sha256"));
        }
        if self.case_id != loaded.fixture.case_id {
            return Err(FormatError::CaseMismatch("case_id"));
        }
        if self.profile != loaded.fixture.profile {
            return Err(FormatError::CaseMismatch("profile"));
        }
        if let Observation::Available { value } = &self.snapshot {
            // A terminal outcome describes why execution stopped, not whether
            // earlier instructions committed. MOV FS may retire before a later
            // exception, unsupported instruction, or harness/backend error.
            // The observed retirement count is the evidence that its state
            // transition may have occurred.
            let fs_load_retired = loaded.fixture.profile == Profile::LongFsLoadV1
                && matches!(
                    self.instructions_executed,
                    Observation::Available { value } if value.get() > 0
                );
            let gs_load_retired = loaded.fixture.profile == Profile::LongGsLoadV1
                && matches!(
                    self.instructions_executed,
                    Observation::Available { value } if value.get() > 0
                );
            if !fs_load_retired && value.state.fs_base != loaded.fixture.initial_state.fs_base {
                return Err(FormatError::CaseMismatch("state.fs_base"));
            }
            if !gs_load_retired && value.state.gs_base != loaded.fixture.initial_state.gs_base {
                return Err(FormatError::CaseMismatch("state.gs_base"));
            }
            if !fs_load_retired
                && value.state.fs_selector != loaded.fixture.initial_state.fs_selector
            {
                return Err(FormatError::CaseMismatch("state.fs_selector"));
            }
            if !gs_load_retired
                && value.state.gs_selector != loaded.fixture.initial_state.gs_selector
            {
                return Err(FormatError::CaseMismatch("state.gs_selector"));
            }
            if value.state.gdtr_base != loaded.fixture.initial_state.gdtr_base
                || value.state.gdtr_limit != loaded.fixture.initial_state.gdtr_limit
            {
                return Err(FormatError::CaseMismatch("state.gdtr"));
            }
            if !fs_load_retired
                && (value.state.fs_cache_limit != loaded.fixture.initial_state.fs_cache_limit
                    || value.state.fs_cache_access != loaded.fixture.initial_state.fs_cache_access
                    || value.state.fs_cache_flags != loaded.fixture.initial_state.fs_cache_flags)
            {
                return Err(FormatError::CaseMismatch("state.fs_cache"));
            }
            if !gs_load_retired
                && (value.state.gs_cache_limit != loaded.fixture.initial_state.gs_cache_limit
                    || value.state.gs_cache_access != loaded.fixture.initial_state.gs_cache_access
                    || value.state.gs_cache_flags != loaded.fixture.initial_state.gs_cache_flags)
            {
                return Err(FormatError::CaseMismatch("state.gs_cache"));
            }
            if loaded.fixture.profile == Profile::LongNullDsEsV1 {
                let code = loaded
                    .fixture
                    .memory
                    .iter()
                    .find(|region| region.region_id == "code")
                    .ok_or(FormatError::CaseMismatch("memory.code"))?;
                let code = decode_bytes(&code.initial_bytes_hex)
                    .map_err(|_| FormatError::CaseMismatch("memory.code"))?;
                let retired = match self.instructions_executed {
                    Observation::Available { value } => value.get() as usize,
                    _ => 0,
                };
                let mut ds = (
                    loaded.fixture.initial_state.ds_selector.unwrap(),
                    loaded.fixture.initial_state.ds_unusable.unwrap(),
                );
                let mut es = (
                    loaded.fixture.initial_state.es_selector.unwrap(),
                    loaded.fixture.initial_state.es_unusable.unwrap(),
                );
                for instruction in code.chunks_exact(2).take(retired) {
                    match instruction {
                        [0x8e, 0xd8] => ds = (HexU64(0), true),
                        [0x8e, 0xc0] => es = (HexU64(0), true),
                        _ => break,
                    }
                }
                if value.state.ds_selector != Some(ds.0)
                    || value.state.ds_unusable != Some(ds.1)
                    || value.state.es_selector != Some(es.0)
                    || value.state.es_unusable != Some(es.1)
                {
                    return Err(FormatError::CaseMismatch("state.ds_es"));
                }
            }
            if value.memory.len() != loaded.fixture.memory.len() {
                return Err(FormatError::CaseMismatch("memory"));
            }
            for region in &loaded.fixture.memory {
                let found = value
                    .memory
                    .iter()
                    .find(|r| r.region_id == region.region_id)
                    .ok_or(FormatError::CaseMismatch("memory.region_id"))?;
                if found.guest_address != region.guest_address
                    || found.bytes_hex.len() != region.length as usize * 2
                {
                    return Err(FormatError::CaseMismatch("memory.layout"));
                }
            }
        }
        if let Observation::Available { value } = self.instructions_executed {
            if value.get() > loaded.fixture.instruction_budget {
                return Err(FormatError::CaseMismatch("instructions_executed"));
            }
        }
        Ok(())
    }
}

/// Compare only completed architectural observations; unavailable/unsupported never matches.
pub fn compare(
    loaded: &LoadedFixture,
    left: &RunResult,
    right: &RunResult,
) -> Result<Vec<String>, FormatError> {
    left.validate_for(loaded)?;
    right.validate_for(loaded)?;
    if !left.outcome.architectural() || !right.outcome.architectural() {
        return Err(invalid(
            "outcome",
            "incomplete or unsupported results cannot be compared",
        ));
    }
    let (Observation::Available { value: a }, Observation::Available { value: b }) =
        (&left.snapshot, &right.snapshot)
    else {
        return Err(invalid("snapshot", "unavailable"));
    };
    let mask = loaded.fixture.rflags_mask.get();
    if a.rflags_defined.get() & mask != mask || b.rflags_defined.get() & mask != mask {
        return Err(invalid("rflags_defined", "requested flags are not defined"));
    }
    let mut differences = Vec::new();
    for (side, result, snapshot) in [("left", left, a), ("right", right, b)] {
        if matches!(result.outcome, Outcome::StopAddress {})
            && Some(snapshot.state.rip) != loaded.fixture.stop_rip
        {
            differences.push(format!("{side}.stop_rip"));
        }
        if matches!(result.outcome, Outcome::InstructionBudgetExhausted {}) {
            if let Observation::Available { value } = result.instructions_executed {
                if value.get() != loaded.fixture.instruction_budget {
                    differences.push(format!("{side}.budget_count"));
                }
            }
        }
    }
    let expected = loaded.fixture.expected_exit;
    for (side, outcome) in [
        ("left.expected_exit", &left.outcome),
        ("right.expected_exit", &right.outcome),
    ] {
        let matches = matches!(
            (expected, outcome),
            (ExpectedExit::StopAddress, Outcome::StopAddress {})
                | (
                    ExpectedExit::InstructionBudgetExhausted,
                    Outcome::InstructionBudgetExhausted {}
                )
                | (
                    ExpectedExit::RepIterationBudgetExhausted,
                    Outcome::RepIterationBudgetExhausted {}
                )
                | (ExpectedExit::StopRequested, Outcome::StopRequested {})
                | (ExpectedExit::Exception, Outcome::Exception {})
                | (ExpectedExit::HarnessError, Outcome::HarnessError { .. })
                | (ExpectedExit::Unsupported, Outcome::Unsupported { .. })
        );
        if !matches {
            differences.push(side.into());
        }
    }
    if left.outcome != right.outcome {
        differences.push("outcome".into());
    }
    for i in 0..16 {
        if a.state.gpr[i] != b.state.gpr[i] {
            differences.push(format!("gpr[{i}]"));
        }
    }
    if a.state.rip != b.state.rip {
        differences.push("rip".into());
    }
    if a.state.mode != b.state.mode {
        differences.push("mode".into());
    }
    if a.state.fs_base != b.state.fs_base {
        differences.push("fs_base".into());
    }
    if a.state.gs_base != b.state.gs_base {
        differences.push("gs_base".into());
    }
    if a.state.fs_selector != b.state.fs_selector {
        differences.push("fs_selector".into());
    }
    if a.state.gs_selector != b.state.gs_selector {
        differences.push("gs_selector".into());
    }
    if a.state.gdtr_base != b.state.gdtr_base {
        differences.push("gdtr_base".into());
    }
    if a.state.gdtr_limit != b.state.gdtr_limit {
        differences.push("gdtr_limit".into());
    }
    if a.state.fs_cache_limit != b.state.fs_cache_limit {
        differences.push("fs_cache_limit".into());
    }
    if a.state.fs_cache_access != b.state.fs_cache_access {
        differences.push("fs_cache_access".into());
    }
    if a.state.fs_cache_flags != b.state.fs_cache_flags {
        differences.push("fs_cache_flags".into());
    }
    if a.state.gs_cache_limit != b.state.gs_cache_limit {
        differences.push("gs_cache_limit".into());
    }
    if a.state.gs_cache_access != b.state.gs_cache_access {
        differences.push("gs_cache_access".into());
    }
    if a.state.gs_cache_flags != b.state.gs_cache_flags {
        differences.push("gs_cache_flags".into());
    }
    if a.state.x87 != b.state.x87 {
        differences.push("x87".into());
    }
    if a.state.sse != b.state.sse {
        differences.push("sse".into());
    }
    if (a.state.rflags.get() ^ b.state.rflags.get()) & mask != 0 {
        differences.push("rflags".into());
    }
    if a.rflags_defined != b.rflags_defined {
        differences.push("rflags_defined".into());
    }
    let same_fault_field = |left: &Observation<HexU64>, right: &Observation<HexU64>| {
        match (left, right) {
            (Observation::Available { value: x }, Observation::Available { value: y }) => x == y,
            // Reasons describe the harness; required architectural fields were checked above.
            (Observation::Unavailable { .. }, Observation::Unavailable { .. }) => true,
            _ => false,
        }
    };
    let same_exception = match (&a.exception, &b.exception) {
        (Observation::Available { value: None }, Observation::Available { value: None }) => true,
        (Observation::Available { value: Some(x) }, Observation::Available { value: Some(y) }) => {
            x.vector == y.vector
                && x.faulting_rip == y.faulting_rip
                && same_fault_field(&x.error_code, &y.error_code)
                && same_fault_field(&x.fault_address, &y.fault_address)
        }
        _ => false,
    };
    if !same_exception {
        differences.push("exception".into());
    }
    // Hardware retired count can be unavailable; it must not substitute a guessed value.
    if let (Observation::Available { value: x }, Observation::Available { value: y }) =
        (&left.instructions_executed, &right.instructions_executed)
    {
        if x != y {
            differences.push("instructions_executed".into());
        }
    }
    for region in &loaded.fixture.memory {
        let x = a
            .memory
            .iter()
            .find(|r| r.region_id == region.region_id)
            .unwrap();
        let y = b
            .memory
            .iter()
            .find(|r| r.region_id == region.region_id)
            .unwrap();
        let x = decode_bytes(&x.bytes_hex)?;
        let y = decode_bytes(&y.bytes_hex)?;
        let masks = decode_bytes(&region.comparison_mask_hex)?;
        for (i, ((x, y), mask)) in x.into_iter().zip(y).zip(masks).enumerate() {
            if (x ^ y) & mask != 0 {
                differences.push(format!("memory.{}[{i}]", region.region_id));
            }
        }
    }
    Ok(differences)
}

pub fn decode_bytes(hex: &str) -> Result<Vec<u8>, FormatError> {
    if hex.len() / 2 > crate::MAX_MEMORY_BYTES {
        return Err(invalid("bytes_hex", "exceeds memory limit"));
    }
    validate_hex_bytes(hex, "bytes_hex")?;
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| invalid("bytes_hex", &e.to_string()))
        })
        .collect()
}

fn load<T: DeserializeOwned>(bytes: &[u8], limit: usize) -> Result<T, FormatError> {
    if bytes.len() > limit {
        return Err(FormatError::TooLarge {
            kind: "v2 JSON",
            max_bytes: limit,
        });
    }
    serde_json::from_slice(bytes).map_err(|e| FormatError::Json(e.to_string()))
}

fn save<T: Serialize>(value: &T, limit: usize) -> Result<Vec<u8>, FormatError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| FormatError::Json(e.to_string()))?;
    if bytes.len() > limit {
        return Err(FormatError::TooLarge {
            kind: "v2 JSON",
            max_bytes: limit,
        });
    }
    Ok(bytes)
}

fn version(value: u32) -> Result<(), FormatError> {
    if value == SCHEMA_VERSION {
        Ok(())
    } else {
        Err(FormatError::UnsupportedSchema(value))
    }
}

fn text(value: &str, field: &'static str) -> Result<(), FormatError> {
    if value.trim().is_empty() || value.len() > 512 {
        Err(invalid(field, "must be nonempty and at most 512 bytes"))
    } else {
        Ok(())
    }
}

fn digest_field(value: &str) -> Result<(), FormatError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("sha256", "expected 64 lowercase hex digits"));
    }
    Ok(())
}

fn invalid(field: &'static str, reason: &str) -> FormatError {
    FormatError::InvalidField {
        field,
        reason: reason.into(),
    }
}

/// Exact DB/DF indexed register bytes, optionally followed by FNSTSW AX.
/// Returns the encoded ST(i) source index for the bounded FCOMI profile.
fn fcomi_family_source(code_bytes: &str) -> Option<u8> {
    if !(code_bytes.len() == 4 || (code_bytes.len() == 8 && code_bytes.ends_with("dfe0"))) {
        return None;
    }
    let opcode = u8::from_str_radix(&code_bytes[0..2], 16).ok()?;
    let modrm = u8::from_str_radix(&code_bytes[2..4], 16).ok()?;
    if !matches!(opcode, 0xdb | 0xdf) {
        return None;
    }
    matches!(modrm, 0xe8..=0xef | 0xf0..=0xf7).then_some(modrm & 7)
}

/// Exact indexed DA C0+i bytes, optionally followed by FNSTSW AX.
fn fcmovb_family_source(code_bytes: &str) -> Option<u8> {
    if !(code_bytes.len() == 4 || (code_bytes.len() == 8 && code_bytes.ends_with("dfe0"))) {
        return None;
    }
    let opcode = u8::from_str_radix(&code_bytes[0..2], 16).ok()?;
    let modrm = u8::from_str_radix(&code_bytes[2..4], 16).ok()?;
    (opcode == 0xda && (0xc0..=0xc7).contains(&modrm)).then_some(modrm & 7)
}

/// Exact indexed DB C0+i bytes, optionally followed by FNSTSW AX.
fn fcmovnb_family_source(code_bytes: &str) -> Option<u8> {
    if !(code_bytes.len() == 4 || (code_bytes.len() == 8 && code_bytes.ends_with("dfe0"))) {
        return None;
    }
    let opcode = u8::from_str_radix(&code_bytes[0..2], 16).ok()?;
    let modrm = u8::from_str_radix(&code_bytes[2..4], 16).ok()?;
    (opcode == 0xdb && (0xc0..=0xc7).contains(&modrm)).then_some(modrm & 7)
}

/// Exact indexed DA C8+i bytes, optionally followed by FNSTSW AX.
fn fcmove_family_source(code_bytes: &str) -> Option<u8> {
    if !(code_bytes.len() == 4 || (code_bytes.len() == 8 && code_bytes.ends_with("dfe0"))) {
        return None;
    }
    let opcode = u8::from_str_radix(&code_bytes[0..2], 16).ok()?;
    let modrm = u8::from_str_radix(&code_bytes[2..4], 16).ok()?;
    (opcode == 0xda && (0xc8..=0xcf).contains(&modrm)).then_some(modrm & 7)
}

/// Exact bare DB C8+i FCMOVNE ST(0), ST(i), optionally followed by FNSTSW AX.
fn fcmovne_family_source(code_bytes: &str) -> Option<u8> {
    if !(code_bytes.len() == 4 || (code_bytes.len() == 8 && code_bytes.ends_with("dfe0"))) {
        return None;
    }
    let opcode = u8::from_str_radix(&code_bytes[0..2], 16).ok()?;
    let modrm = u8::from_str_radix(&code_bytes[2..4], 16).ok()?;
    (opcode == 0xdb && (0xc8..=0xcf).contains(&modrm)).then_some(modrm & 7)
}

/// Exact twenty-stream S2-30cd FCMOVBE schema, including the S2-30cc producers.
fn fcmovbe_stream(code_bytes: &str) -> bool {
    if code_bytes.len() == 4 || (code_bytes.len() == 8 && code_bytes.ends_with("dfe0")) {
        return fcmovbe_bare_source(&code_bytes[..4]).is_some();
    }
    matches!(
        code_bytes,
        "dad1dad1dfe0"
            | "d9e8d9e0d9e8dbf1dad1dfe0"
            | "d9e8d9e8dbf1d9e0dad1dfe0"
            | "d9e8d9e8d9e0dbf1dad1dfe0"
    )
}

fn fcmovbe_bare_source(code_bytes: &str) -> Option<u8> {
    if code_bytes.len() != 4 {
        return None;
    }
    let opcode = u8::from_str_radix(&code_bytes[0..2], 16).ok()?;
    let modrm = u8::from_str_radix(&code_bytes[2..4], 16).ok()?;
    (opcode == 0xda && (0xd0..=0xd7).contains(&modrm)).then_some(modrm & 7)
}

/// Exact twenty-stream S2-30cf FCMOVNBE ST(i) schema, retaining the producers.
fn fcmovnbe_stream(code_bytes: &str) -> bool {
    if code_bytes.len() == 4 || (code_bytes.len() == 8 && code_bytes.ends_with("dfe0")) {
        return fcmovnbe_bare_source(&code_bytes[..4]).is_some();
    }
    matches!(
        code_bytes,
        "dbd1dbd1dfe0"
            | "d9e8d9e0d9e8dbf1dbd1dfe0"
            | "d9e8d9e8dbf1d9e0dbd1dfe0"
            | "d9e8d9e8d9e0dbf1dbd1dfe0"
    )
}

fn fcmovnbe_bare_source(code_bytes: &str) -> Option<u8> {
    if code_bytes.len() != 4 {
        return None;
    }
    let opcode = u8::from_str_radix(&code_bytes[0..2], 16).ok()?;
    let modrm = u8::from_str_radix(&code_bytes[2..4], 16).ok()?;
    (opcode == 0xdb && (0xd0..=0xd7).contains(&modrm)).then_some(modrm & 7)
}

/// Exact S2-30ci FCMOVNU bare, status, repeat, and TEST producer streams.
fn fcmovnu_stream(code_bytes: &str) -> bool {
    fcmovnu_budget(code_bytes).is_some()
}

fn fcmovnu_budget(code_bytes: &str) -> Option<u64> {
    if code_bytes.len() == 4 {
        return fcmovnu_bare_source(code_bytes).map(|_| 1);
    }
    if code_bytes.len() == 8 && code_bytes.ends_with("dfe0") {
        return fcmovnu_bare_source(&code_bytes[..4]).map(|_| 2);
    }
    match code_bytes {
        "dbd9dbd9dfe0" | "d9e8d9ee4885c0dbd9dfe0" => {
            Some(if code_bytes.starts_with("dbd9") { 3 } else { 5 })
        }
        _ => None,
    }
}

fn fcmovnu_bare_source(code_bytes: &str) -> Option<u8> {
    if code_bytes.len() != 4 {
        return None;
    }
    let opcode = u8::from_str_radix(&code_bytes[0..2], 16).ok()?;
    let modrm = u8::from_str_radix(&code_bytes[2..4], 16).ok()?;
    (opcode == 0xdb && (0xd8..=0xdf).contains(&modrm)).then_some(modrm & 7)
}

fn fcmovu_bare_source(code_bytes: &str) -> Option<u8> {
    if code_bytes.len() != 4 {
        return None;
    }
    let opcode = u8::from_str_radix(&code_bytes[0..2], 16).ok()?;
    let modrm = u8::from_str_radix(&code_bytes[2..4], 16).ok()?;
    (opcode == 0xda && (0xd8..=0xdf).contains(&modrm)).then_some(modrm & 7)
}

fn xorps_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x0f, 0x57, modrm] if modrm >> 6 == 3 => Some(3),
        [rex, 0x0f, 0x57, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(4)
        }
        _ => None,
    }
}

fn pxor_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xef, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xef, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn paddb_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xfc, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xfc, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn paddw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xfd, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xfd, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn paddusw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xdd, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xdd, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn psubusw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xd9, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xd9, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn psubw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xf9, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xf9, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn psubb_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xf8, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xf8, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn paddusb_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xdc, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xdc, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn paddsb_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xec, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xec, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn psubsb_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xe8, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xe8, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn paddsw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xed, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xed, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn paddd_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xfe, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xfe, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn psubd_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xfa, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xfa, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn psubsw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xe9, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xe9, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn punpckhbw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x68, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x68, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn punpcklwd_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x61, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x61, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn punpckhwd_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x69, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x69, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn punpckldq_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x62, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x62, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn punpcklqdq_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x6c, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x6c, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn packsswb_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x63, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x63, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}
fn packssdw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x6b, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x6b, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pshufd_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x70, modrm, _imm8] if modrm >> 6 == 3 => Some(5),
        [0x66, rex, 0x0f, 0x70, modrm, _imm8]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(6)
        }
        _ => None,
    }
}

fn psllw_immediate_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x71, modrm, _imm8] if modrm >> 6 == 3 && (modrm >> 3) & 7 == 6 => Some(5),
        [0x66, rex, 0x0f, 0x71, modrm, _imm8]
            if *rex == 0x41 && modrm >> 6 == 3 && (modrm >> 3) & 7 == 6 =>
        {
            Some(6)
        }
        _ => None,
    }
}

fn pslld_immediate_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x72, modrm, _imm8] if modrm >> 6 == 3 && (modrm >> 3) & 7 == 6 => Some(5),
        [0x66, rex, 0x0f, 0x72, modrm, _imm8]
            if *rex == 0x41 && modrm >> 6 == 3 && (modrm >> 3) & 7 == 6 =>
        {
            Some(6)
        }
        _ => None,
    }
}

fn psrld_immediate_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x72, modrm, _imm8] if modrm >> 6 == 3 && (modrm >> 3) & 7 == 2 => Some(5),
        [0x66, rex, 0x0f, 0x72, modrm, _imm8]
            if *rex == 0x41 && modrm >> 6 == 3 && (modrm >> 3) & 7 == 2 =>
        {
            Some(6)
        }
        _ => None,
    }
}

fn psrlw_immediate_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x71, modrm, _imm8] if modrm >> 6 == 3 && (modrm >> 3) & 7 == 2 => Some(5),
        [0x66, rex, 0x0f, 0x71, modrm, _imm8]
            if *rex == 0x41 && modrm >> 6 == 3 && (modrm >> 3) & 7 == 2 =>
        {
            Some(6)
        }
        _ => None,
    }
}

fn psraw_immediate_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x71, modrm, _imm8] if modrm >> 6 == 3 && (modrm >> 3) & 7 == 4 => Some(5),
        [0x66, rex, 0x0f, 0x71, modrm, _imm8]
            if *rex == 0x41 && modrm >> 6 == 3 && (modrm >> 3) & 7 == 4 =>
        {
            Some(6)
        }
        _ => None,
    }
}

fn psrad_immediate_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x72, modrm, _imm8] if modrm >> 6 == 3 && (modrm >> 3) & 7 == 4 => Some(5),
        [0x66, rex, 0x0f, 0x72, modrm, _imm8]
            if *rex == 0x41 && modrm >> 6 == 3 && (modrm >> 3) & 7 == 4 =>
        {
            Some(6)
        }
        _ => None,
    }
}

fn packuswb_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x67, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x67, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn punpckhqdq_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x6d, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x6d, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn punpckhdq_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x6a, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x6a, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn psubusb_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xd8, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xd8, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn andps_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x0f, 0x54, modrm] if modrm >> 6 == 3 => Some(3),
        [rex, 0x0f, 0x54, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(4)
        }
        _ => None,
    }
}

fn orps_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x0f, 0x56, modrm] if modrm >> 6 == 3 => Some(3),
        [rex, 0x0f, 0x56, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(4)
        }
        _ => None,
    }
}

fn movss_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0xf3, 0x0f, 0x10, modrm] if modrm >> 6 == 3 => Some(4),
        [0xf3, rex, 0x0f, 0x10, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn movsd_register_length(code_bytes: &str) -> Option<u32> {
    let bytes = decode_bytes(code_bytes).ok()?;
    match bytes.as_slice() {
        [0xf2, 0x0f, 0x10, modrm] if modrm >> 6 == 3 => Some(4),
        [0xf2, rex, 0x0f, 0x10, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn movsd_store_register_length(code_bytes: &str) -> Option<u32> {
    let bytes = decode_bytes(code_bytes).ok()?;
    match bytes.as_slice() {
        [0xf2, 0x0f, 0x11, modrm] if modrm >> 6 == 3 => Some(4),
        [0xf2, rex, 0x0f, 0x11, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn movss_memory_load_destination(code_bytes: &str) -> Option<(u32, u8)> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    let (length, rex, modrm) = match bytes.as_slice() {
        [0xf3, 0x0f, 0x10, modrm] => (4, 0, *modrm),
        [0xf3, rex, 0x0f, 0x10, modrm] if (0x40..=0x4f).contains(rex) && rex & 0x0b == 0 => {
            (5, *rex, *modrm)
        }
        _ => return None,
    };
    (modrm >> 6 == 0 && modrm & 7 == 0).then_some((length, ((modrm >> 3) & 7) | ((rex & 4) << 1)))
}

fn movsd_memory_load_destination(code_bytes: &str) -> Option<(u32, u8)> {
    let bytes = decode_bytes(code_bytes).ok()?;
    let (length, rex, modrm) = match bytes.as_slice() {
        [0xf2, 0x0f, 0x10, modrm] => (4, 0, *modrm),
        [0xf2, rex, 0x0f, 0x10, modrm] if (0x40..=0x4f).contains(rex) && rex & 0x0b == 0 => {
            (5, *rex, *modrm)
        }
        _ => return None,
    };
    (modrm >> 6 == 0 && modrm & 7 == 0).then_some((length, ((modrm >> 3) & 7) | ((rex & 4) << 1)))
}

fn movsd_memory_store_source(code_bytes: &str) -> Option<(u32, u8)> {
    let bytes = decode_bytes(code_bytes).ok()?;
    let (length, rex, modrm) = match bytes.as_slice() {
        [0xf2, 0x0f, 0x11, modrm] => (4, 0, *modrm),
        [0xf2, rex, 0x0f, 0x11, modrm] if (0x40..=0x4f).contains(rex) && rex & 0x0b == 0 => {
            (5, *rex, *modrm)
        }
        _ => return None,
    };
    (modrm >> 6 == 0 && modrm & 7 == 0).then_some((length, ((modrm >> 3) & 7) | ((rex & 4) << 1)))
}

fn movss_memory_store_source(code_bytes: &str) -> Option<(u32, u8)> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    let (length, rex, modrm) = match bytes.as_slice() {
        [0xf3, 0x0f, 0x11, modrm] => (4, 0, *modrm),
        [0xf3, rex, 0x0f, 0x11, modrm] if (0x40..=0x4f).contains(rex) && rex & 0x0b == 0 => {
            (5, *rex, *modrm)
        }
        _ => return None,
    };
    (modrm >> 6 == 0 && modrm & 7 == 0).then_some((length, ((modrm >> 3) & 7) | ((rex & 4) << 1)))
}

fn movss_store_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0xf3, 0x0f, 0x11, modrm] if modrm >> 6 == 3 => Some(4),
        [0xf3, rex, 0x0f, 0x11, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn andnps_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x0f, 0x55, modrm] if modrm >> 6 == 3 => Some(3),
        [rex, 0x0f, 0x55, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(4)
        }
        _ => None,
    }
}

fn movups_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    let opcode_at = match bytes.as_slice() {
        [0x0f, 0x10 | 0x11, modrm] if modrm >> 6 == 3 => 0,
        [rex, 0x0f, 0x10 | 0x11, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            1
        }
        _ => return None,
    };
    Some((opcode_at + 3) as u32)
}

fn movups_memory_load_destination(code_bytes: &str) -> Option<(u32, u8)> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    let (prefix_len, modrm) = match bytes.as_slice() {
        [0x0f, 0x10, modrm] => (0, *modrm),
        [rex, 0x0f, 0x10, modrm] if (0x40..=0x4f).contains(rex) && rex & 0x0b == 0 => (1, *modrm),
        _ => return None,
    };
    (modrm >> 6 == 0 && modrm & 7 == 0).then_some((
        (prefix_len + 3) as u32,
        ((modrm >> 3) & 7) | ((bytes.first().copied().unwrap_or(0) & 4) << 1),
    ))
}

fn movups_memory_store_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    let (prefix_len, modrm) = match bytes.as_slice() {
        [0x0f, 0x11, modrm] => (0, *modrm),
        [rex, 0x0f, 0x11, modrm] if (0x40..=0x4f).contains(rex) && rex & 0x0b == 0 => (1, *modrm),
        _ => return None,
    };
    (modrm >> 6 == 0 && modrm & 7 == 0).then_some((prefix_len + 3) as u32)
}

fn movups_memory_base_form(code_bytes: &str, state: &LongModeState) -> Option<(u32, bool, u64)> {
    let bytes = decode_bytes(code_bytes).ok()?;
    let mut memory = InstructionBytes { bytes: &bytes };
    let instruction = od_isa::long_decode::decode(&mut memory, 0, AccessContext::default()).ok()?;
    if usize::from(instruction.length) != bytes.len() {
        return None;
    }
    let (store, operand) = match instruction.operation {
        Operation::MovupsLoadMemory { source, .. } => (false, source),
        Operation::MovupsStoreMemory { destination, .. } => (true, destination),
        _ => return None,
    };
    let next_rip = state.rip.wrapping_add(u64::from(instruction.length));
    let start = operand.effective_address(state, next_rip);
    Some((u32::from(instruction.length), store, start))
}

/// Fetch-only source used by schema validation to apply the core x86 decoder.
struct InstructionBytes<'a> {
    bytes: &'a [u8],
}

impl GuestMemory for InstructionBytes<'_> {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        self.bytes
            .get(usize::try_from(address).unwrap_or(usize::MAX))
            .copied()
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }

    fn read(&mut self, address: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })
    }

    fn write(&mut self, address: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Write,
        })
    }

    fn atomic_exchange_integer(
        &mut self,
        address: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })
    }

    fn atomic_fetch_add_integer(
        &mut self,
        address: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        address: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

/// Exact S2-30ch FCMOVU bare, FNSTSW, repeat, and retained producer streams.
fn fcmovu_stream(code_bytes: &str) -> bool {
    fcmovu_budget(code_bytes).is_some()
}

fn fcmovu_budget(code_bytes: &str) -> Option<u64> {
    if code_bytes == "dad9dad9dfe0" {
        return Some(3);
    }
    if code_bytes == "d9e8d9ee4885c0dad9dfe0" {
        return Some(5);
    }
    if code_bytes.len() == 4 {
        let bytes = u16::from_str_radix(code_bytes, 16).ok()?;
        return (bytes >> 8 == 0xda && (0xd8..=0xdf).contains(&(bytes as u8))).then_some(1);
    }
    if code_bytes.len() == 8 {
        let instruction = &code_bytes[..4];
        let suffix = &code_bytes[4..];
        if suffix == "dfe0" {
            let bytes = u16::from_str_radix(instruction, 16).ok()?;
            if bytes >> 8 == 0xda && (0xd8..=0xdf).contains(&(bytes as u8)) {
                return Some(2);
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "v2_tests.rs"]
mod tests;

fn paddq_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xd4, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xd4, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn psubq_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xfb, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xfb, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pcmpeqb_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x74, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x74, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pcmpeqw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x75, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x75, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pcmpeqd_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x76, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x76, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pcmpgtb_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x64, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x64, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pcmpgtw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x65, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x65, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pcmpgtd_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x66, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x66, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn punpcklbw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x60, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x60, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}
