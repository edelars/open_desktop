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
pub const PROFILE_X87_STATE: &str = "s2-long-x87-state-v1";
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
    #[serde(rename = "s2-long-x87-state-v1")]
    LongX87StateV1,
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

fn deserialize_present_hex_u64<'de, D>(deserializer: D) -> Result<Option<HexU64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    HexU64::deserialize(deserializer).map(Some)
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
            gdtr_base: None,
            gdtr_limit: None,
            fs_cache_limit: None,
            fs_cache_access: None,
            fs_cache_flags: None,
            gs_cache_limit: None,
            gs_cache_access: None,
            gs_cache_flags: None,
            x87: None,
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
                _ => None,
            },
        })
    }

    pub fn from_cpu_for(state: &LongModeState, profile: Profile) -> Self {
        let mut result = Self::from(state);
        result.x87 = if profile == Profile::LongX87StateV1 {
            state.x87.as_ref().map(X87Snapshot::from)
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
        }
        result
    }

    fn validate_for(&self, profile: Profile) -> Result<(), FormatError> {
        let state_profile = match profile {
            Profile::LongX87StateV1 => Profile::LongIntegerV1,
            other => other,
        };
        match (state_profile, self.fs_base) {
            (
                Profile::LongIntegerV1
                | Profile::LongX87StateV1
                | Profile::LongGsBaseV1
                | Profile::LongGsSelectorV1
                | Profile::LongVerwGdtV1
                | Profile::LongGsLoadV1,
                None,
            )
            | (Profile::LongFsBaseV1 | Profile::LongFsLoadV1, Some(_)) => {}
            (Profile::LongFsSelectorV1, Some(_)) => {}
            (
                Profile::LongIntegerV1
                | Profile::LongX87StateV1
                | Profile::LongGsBaseV1
                | Profile::LongGsSelectorV1
                | Profile::LongVerwGdtV1
                | Profile::LongGsLoadV1,
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
                | Profile::LongFsBaseV1
                | Profile::LongFsSelectorV1
                | Profile::LongVerwGdtV1
                | Profile::LongFsLoadV1,
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
                | Profile::LongFsBaseV1
                | Profile::LongGsBaseV1
                | Profile::LongVerwGdtV1
                | Profile::LongGsLoadV1,
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
            (Profile::LongX87StateV1, None) => {
                return Err(invalid("state.x87", "required by this profile"));
            }
            (_, None) => {}
            (_, Some(_)) => return Err(invalid("state.x87", "not part of this profile")),
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
        if !(1..=MAX_INSTRUCTION_BUDGET).contains(&self.instruction_budget) {
            return Err(invalid(
                "instruction_budget",
                "must be between 1 and 10000000",
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
        if matches!(self.profile, Profile::LongFsLoadV1 | Profile::LongGsLoadV1) {
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
                _ => {
                    return Err(invalid(
                        "memory.code",
                        "outside the bounded FS/GS-load profile",
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
        if self.profile == Profile::LongX87StateV1 {
            let code = self.memory.iter().find(|region| region.region_id == "code");
            let Some(code) = code else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let code_bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let (expected_bytes, expected_length) = match code_bytes.as_str() {
                "90" => ("90", 1),
                "dbe3" => ("dbe3", 2),
                "dbe2" => ("dbe2", 2),
                "dbf1" => ("dbf1", 2),
                "dbf1dfe0" => ("dbf1dfe0", 4),
                "dfe0" => ("dfe0", 2),
                "d9e0" => ("d9e0", 2),
                "d9e1" => ("d9e1", 2),
                "d9e4" => ("d9e4", 2),
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
                        "x87 state profile admits one NOP or exact bare x87 instruction form",
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
            let is_fchs_source_rejected = code_bytes == "d9e0" && self.stop_rip.is_none();
            let is_fabs_source_rejected = code_bytes == "d9e1" && self.stop_rip.is_none();
            let is_ftst_source_rejected = code_bytes == "d9e4" && self.stop_rip.is_none();
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
            let is_fcomi_source_rejected = code_bytes == "dbf1"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u8;
                    let rhs = (top + 1) & 7;
                    (x87.tag_word.get() >> (u32::from(rhs) * 2)) & 3 != 0
                });
            let expected_budget = if code_bytes.len() == 8 { 2 } else { 1 };
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
                    || is_fchs_source_rejected
                    || is_fabs_source_rejected
                    || is_ftst_source_rejected
                    || is_fcom_source_rejected
                    || is_fcomp_source_rejected
                    || is_fcompp_source_rejected
                    || is_fucompp_source_rejected
                    || is_fucom_source_rejected
                    || is_fucomp_source_rejected
                    || is_fcomi_source_rejected
                {
                    self.stop_rip.is_some() || self.expected_exit != ExpectedExit::Unsupported
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

#[cfg(test)]
#[path = "v2_tests.rs"]
mod tests;
