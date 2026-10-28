//! Long-mode integer state and register aliases (Intel SDM 093, Vol.1 §3.4.1.1).
//! This is not yet a system CPU: only selected FS/GS-base data forms are modeled;
//! other segment, control and FP state remains outside this integer profile.

use crate::{ARITHMETIC_FLAGS, FragmentState, Register};

/// Raw Long64 x87 state for local snapshot profiles. Register bytes are kept in
/// architectural low-to-high memory order; no floating-point conversion occurs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct X87State {
    pub registers: [[u8; 10]; 8],
    pub control_word: u16,
    pub status_word: u16,
    pub tag_word: u16,
    pub instruction_pointer: u64,
    pub data_pointer: u64,
    /// The architectural opcode register is 11 bits wide.
    pub last_opcode: u16,
}

/// Raw Long64 SSE state for local snapshot profiles. XMM lanes are stored as
/// sixteen bytes in architectural memory order; MXCSR is kept verbatim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SseState {
    pub xmm: [[u8; 16]; 16],
    pub mxcsr: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum X87Tag {
    Valid = 0b00,
    Zero = 0b01,
    Special = 0b10,
    Empty = 0b11,
}

impl X87State {
    /// A reproducible, non-architectural seed for local fixtures and tests.
    pub const fn fixture_seed() -> Self {
        Self {
            registers: [
                [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x10, 0x80],
                [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f],
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0xff, 0x3f],
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff],
                [0x55, 0xaa, 0x55, 0xaa, 0x55, 0xaa, 0x55, 0xaa, 0x55, 0xaa],
                [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa],
                [0xde, 0xad, 0xbe, 0xef, 0xca, 0xfe, 0xba, 0xbe, 0x01, 0x80],
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            ],
            control_word: 0x027f,
            status_word: 0x3805,
            tag_word: 0x1be4,
            instruction_pointer: 0x0123_4567_89ab_cdef,
            data_pointer: 0xfedc_ba98_7654_3210,
            last_opcode: 0x05aa,
        }
    }

    pub const fn top(&self) -> u8 {
        ((self.status_word >> 11) & 7) as u8
    }

    pub const fn physical_register_for_st(&self, logical_index: u8) -> Option<u8> {
        if logical_index >= 8 {
            None
        } else {
            Some((self.top() + logical_index) & 7)
        }
    }

    pub const fn tag(&self, physical_index: u8) -> Option<X87Tag> {
        if physical_index >= 8 {
            return None;
        }
        Some(match (self.tag_word >> (physical_index * 2)) & 3 {
            0 => X87Tag::Valid,
            1 => X87Tag::Zero,
            2 => X87Tag::Special,
            _ => X87Tag::Empty,
        })
    }

    pub fn set_tag(&mut self, physical_index: u8, tag: X87Tag) -> bool {
        if physical_index >= 8 {
            return false;
        }
        let shift = physical_index * 2;
        self.tag_word = (self.tag_word & !(3 << shift)) | ((tag as u16) << shift);
        true
    }

    /// Apply the architectural FNINIT transition. Data-register payloads survive.
    pub fn fninit(&mut self) {
        self.control_word = 0x037f;
        self.status_word = 0;
        self.tag_word = 0xffff;
        self.instruction_pointer = 0;
        self.data_pointer = 0;
        self.last_opcode = 0;
    }

    /// Apply the bounded FNCLEX status-word transition. C0/C1/C2/C3 retain
    /// their concrete local values although Intel leaves them undefined.
    pub fn fnclex(&mut self) {
        self.status_word &= !0x80ff;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperandSize {
    Byte,
    Word,
    Dword,
    Qword,
}

impl OperandSize {
    pub const fn bytes(self) -> usize {
        match self {
            Self::Byte => 1,
            Self::Word => 2,
            Self::Dword => 4,
            Self::Qword => 8,
        }
    }

    pub const fn mask(self) -> u64 {
        match self {
            Self::Byte => 0xff,
            Self::Word => 0xffff,
            Self::Dword => 0xffff_ffff,
            Self::Qword => u64::MAX,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegisterError {
    InvalidEncoding(u8),
    RexRequired,
    HighByteWithRex,
    NoHighByte(Register),
}

/// A validated alias; fields are private so an invalid high-byte view cannot occur.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegisterView {
    register: Register,
    size: OperandSize,
    shift: u8,
}

impl RegisterView {
    /// A semantic low alias. Instruction encoding validation belongs to decode.
    pub const fn low(register: Register, size: OperandSize) -> Self {
        Self {
            register,
            size,
            shift: 0,
        }
    }

    pub fn high_byte(register: Register, rex_present: bool) -> Result<Self, RegisterError> {
        if rex_present {
            return Err(RegisterError::HighByteWithRex);
        }
        if register as u8 >= 4 {
            return Err(RegisterError::NoHighByte(register));
        }
        Ok(Self {
            register,
            size: OperandSize::Byte,
            shift: 8,
        })
    }

    /// `encoding` includes any REX extension bit, not just the three ModR/M bits.
    pub fn decode_byte(encoding: u8, rex_present: bool) -> Result<Self, RegisterError> {
        let register =
            Register::from_encoding(encoding).ok_or(RegisterError::InvalidEncoding(encoding))?;
        if encoding >= 8 && !rex_present {
            return Err(RegisterError::RexRequired);
        }
        if !rex_present && (4..8).contains(&encoding) {
            return Self::high_byte(Register::from_encoding(encoding - 4).unwrap(), false);
        }
        Ok(Self::low(register, OperandSize::Byte))
    }

    pub const fn register(self) -> Register {
        self.register
    }

    pub const fn size(self) -> OperandSize {
        self.size
    }
}

/// Architectural integer values, independent of fragment/JIT/host layout.
/// All accesses below apply the 64-bit instruction-mode aliasing rules.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LongModeState {
    pub gpr: [u64; 16],
    pub rip: u64,
    pub rflags: u64,
    /// FS base in the limited canonical48 data-address profile.
    pub fs_base: u64,
    /// GS base in the limited canonical48 data-address profile.
    pub gs_base: u64,
    /// FS selector in local selector/load profiles.
    pub fs_selector: u16,
    /// GS selector in the local selector-read profile; hidden descriptor state is not modeled.
    pub gs_selector: u16,
    /// Visible DS/ES selectors and unusable state for the bounded null-load profile.
    pub ds_selector: u16,
    pub es_selector: u16,
    pub ds_unusable: bool,
    pub es_unusable: bool,
    /// Hidden DS descriptor cache only in the bounded MOV-DS-load profile.
    pub ds_cache: Option<DsDescriptorCache>,
    /// Hidden ES descriptor cache only in the bounded MOV-ES-load profile.
    pub es_cache: Option<DataSegmentDescriptorCache>,
    /// GDTR state used by bounded LGDT and GDT-only VERW profiles; no general descriptor support or LDTR is modeled.
    pub gdtr: Option<Gdtr>,
    /// IDTR state populated only by the bounded local LIDT profile.
    pub idtr: Option<Idtr>,
    /// Hidden FS descriptor cache only in the local MOV-FS-load profile.
    pub fs_cache: Option<FsDescriptorCache>,
    /// Hidden GS descriptor cache only in the local MOV-GS-load profile.
    pub gs_cache: Option<FsDescriptorCache>,
    /// Raw x87 state only for profiles that explicitly carry it.
    pub x87: Option<X87State>,
    /// Raw SSE state only for profiles that explicitly carry it.
    pub sse: Option<SseState>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Gdtr {
    pub base: u64,
    pub limit: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Idtr {
    pub base: u64,
    pub limit: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DataSegmentDescriptorCache {
    /// The descriptor's 32-bit base; Long64 ordinary data addressing ignores it.
    pub base: u32,
    /// Effective limit after applying descriptor granularity.
    pub limit: u32,
    /// Descriptor access byte (type, S, DPL, P, A).
    pub access: u8,
    /// Descriptor flag nibble (AVL, L, D/B, G).
    pub flags: u8,
}

pub type DsDescriptorCache = DataSegmentDescriptorCache;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FsDescriptorCache {
    /// Effective limit after applying descriptor granularity.
    pub limit: u32,
    /// Descriptor access byte (type, S, DPL, P, A).
    pub access: u8,
    /// Descriptor flag nibble (AVL, L, D/B, G).
    pub flags: u8,
}

impl Default for LongModeState {
    fn default() -> Self {
        Self {
            gpr: [0; 16],
            rip: 0,
            rflags: 2,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            ds_selector: 0,
            es_selector: 0,
            ds_unusable: false,
            es_unusable: false,
            ds_cache: None,
            es_cache: None,
            gdtr: None,
            idtr: None,
            fs_cache: None,
            gs_cache: None,
            x87: None,
            sse: None,
        }
    }
}

impl LongModeState {
    pub fn read(&self, view: RegisterView) -> u64 {
        (self.gpr[view.register as usize] >> view.shift) & view.size.mask()
    }

    pub fn write(&mut self, view: RegisterView, value: u64) {
        let destination = &mut self.gpr[view.register as usize];
        let value = value & view.size.mask();
        *destination = match view.size {
            OperandSize::Dword | OperandSize::Qword => value,
            OperandSize::Byte | OperandSize::Word => {
                let mask = view.size.mask() << view.shift;
                (*destination & !mask) | (value << view.shift)
            }
        };
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FragmentAdapterError {
    ExtraRegister(Register),
    UnsupportedFsBase(u64),
    UnsupportedGsBase(u64),
    UnsupportedFsSelector(u16),
    UnsupportedGsSelector(u16),
    UnsupportedGdtr(Gdtr),
    UnsupportedIdtr(Idtr),
    UnsupportedDsCache(DsDescriptorCache),
    UnsupportedEsCache(DataSegmentDescriptorCache),
    UnsupportedFsCache(FsDescriptorCache),
    UnsupportedGsCache(FsDescriptorCache),
    UnsupportedX87,
    UnsupportedSse,
    InstructionPointerOutOfRange(u64),
    UnsupportedFlags(u64),
    ReservedFlagMissing,
}

impl From<&FragmentState> for LongModeState {
    fn from(fragment: &FragmentState) -> Self {
        let mut state = Self::default();
        state.gpr[..4].copy_from_slice(&fragment.gpr);
        state.rip = fragment.rip as u64;
        state.rflags = fragment.rflags;
        state
    }
}

impl TryFrom<&LongModeState> for FragmentState {
    type Error = FragmentAdapterError;

    fn try_from(state: &LongModeState) -> Result<Self, Self::Error> {
        if state.x87.is_some() {
            return Err(FragmentAdapterError::UnsupportedX87);
        }
        if state.sse.is_some() {
            return Err(FragmentAdapterError::UnsupportedSse);
        }
        if state.fs_base != 0 {
            return Err(FragmentAdapterError::UnsupportedFsBase(state.fs_base));
        }
        if state.gs_base != 0 {
            return Err(FragmentAdapterError::UnsupportedGsBase(state.gs_base));
        }
        if state.fs_selector != 0 {
            return Err(FragmentAdapterError::UnsupportedFsSelector(
                state.fs_selector,
            ));
        }
        if state.gs_selector != 0 {
            return Err(FragmentAdapterError::UnsupportedGsSelector(
                state.gs_selector,
            ));
        }
        if let Some(gdtr) = state.gdtr {
            return Err(FragmentAdapterError::UnsupportedGdtr(gdtr));
        }
        if let Some(idtr) = state.idtr {
            return Err(FragmentAdapterError::UnsupportedIdtr(idtr));
        }
        if let Some(fs_cache) = state.fs_cache {
            return Err(FragmentAdapterError::UnsupportedFsCache(fs_cache));
        }
        if let Some(gs_cache) = state.gs_cache {
            return Err(FragmentAdapterError::UnsupportedGsCache(gs_cache));
        }
        if let Some(ds_cache) = state.ds_cache {
            return Err(FragmentAdapterError::UnsupportedDsCache(ds_cache));
        }
        if let Some(es_cache) = state.es_cache {
            return Err(FragmentAdapterError::UnsupportedEsCache(es_cache));
        }
        for encoding in 4..16 {
            if state.gpr[encoding] != 0 {
                return Err(FragmentAdapterError::ExtraRegister(
                    Register::from_encoding(encoding as u8).unwrap(),
                ));
            }
        }
        let unsupported = state.rflags & !(ARITHMETIC_FLAGS | 2);
        if unsupported != 0 {
            return Err(FragmentAdapterError::UnsupportedFlags(unsupported));
        }
        if state.rflags & 2 == 0 {
            return Err(FragmentAdapterError::ReservedFlagMissing);
        }
        Ok(Self {
            gpr: state.gpr[..4].try_into().unwrap(),
            rip: usize::try_from(state.rip)
                .map_err(|_| FragmentAdapterError::InstructionPointerOutOfRange(state.rip))?,
            rflags: state.rflags,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x87_top_maps_all_logical_registers_to_physical_slots() {
        for top in 0..8u16 {
            let state = X87State {
                status_word: top << 11,
                ..X87State::fixture_seed()
            };
            assert_eq!(state.top(), top as u8);
            for logical in 0..8u8 {
                assert_eq!(
                    state.physical_register_for_st(logical),
                    Some((top as u8 + logical) & 7)
                );
            }
            assert_eq!(state.physical_register_for_st(8), None);
        }
    }

    #[test]
    fn x87_tags_are_two_bit_codes_on_physical_registers() {
        let mut state = X87State::fixture_seed();
        for (index, tag) in [
            X87Tag::Valid,
            X87Tag::Zero,
            X87Tag::Special,
            X87Tag::Empty,
            X87Tag::Empty,
            X87Tag::Special,
            X87Tag::Zero,
            X87Tag::Valid,
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(state.tag(index as u8), Some(tag));
            assert!(state.set_tag(index as u8, tag));
            assert_eq!(state.tag(index as u8), Some(tag));
        }
        assert_eq!(state.tag(8), None);
        assert!(!state.set_tag(8, X87Tag::Empty));
    }

    #[test]
    fn fninit_resets_environment_and_preserves_every_raw_register_payload() {
        let mut state = X87State::fixture_seed();
        let payloads = state.registers;
        state.fninit();
        assert_eq!(state.registers, payloads);
        assert_eq!(state.control_word, 0x037f);
        assert_eq!(state.status_word, 0);
        assert_eq!(state.top(), 0);
        assert_eq!(state.tag_word, 0xffff);
        assert_eq!(state.instruction_pointer, 0);
        assert_eq!(state.data_pointer, 0);
        assert_eq!(state.last_opcode, 0);
        for index in 0..8 {
            assert_eq!(state.tag(index), Some(X87Tag::Empty));
        }
    }

    #[test]
    fn fragment_adapter_rejects_x87_even_when_integer_state_fits() {
        let mut state = LongModeState::default();
        state.x87 = Some(X87State::fixture_seed());
        assert!(matches!(
            FragmentState::try_from(&state),
            Err(FragmentAdapterError::UnsupportedX87)
        ));
    }

    #[test]
    fn fragment_adapter_rejects_sse_even_when_integer_state_fits() {
        let mut state = LongModeState::default();
        state.sse = Some(SseState {
            xmm: [[0; 16]; 16],
            mxcsr: 0,
        });
        assert!(matches!(
            FragmentState::try_from(&state),
            Err(FragmentAdapterError::UnsupportedSse)
        ));
    }

    #[test]
    fn every_register_obeys_long_mode_aliasing_and_preserves_other_state() {
        for encoding in 0..16 {
            let register = Register::from_encoding(encoding).unwrap();
            for value in [0, 1, 0x80, 0xffff, 0x8000_0000, u64::MAX] {
                for size in [
                    OperandSize::Byte,
                    OperandSize::Word,
                    OperandSize::Dword,
                    OperandSize::Qword,
                ] {
                    let mut state = LongModeState {
                        gpr: [0x1234_5678_9abc_def0; 16],
                        rip: u64::MAX,
                        rflags: 0x8d7,
                        fs_base: 0,
                        gs_base: 0,
                        fs_selector: 0,
                        gs_selector: 0,
                        ds_selector: 0,
                        es_selector: 0,
                        ds_unusable: false,
                        es_unusable: false,
                        ds_cache: None,
                        gdtr: None,
                        idtr: None,
                        fs_cache: None,
                        gs_cache: None,

                        x87: None,
                        sse: None,
                        es_cache: None,
                    };
                    let before = state.clone();
                    let view = RegisterView::low(register, size);
                    state.write(view, value);
                    let expected = match size {
                        OperandSize::Byte => 0x1234_5678_9abc_de00 | (value & 0xff),
                        OperandSize::Word => 0x1234_5678_9abc_0000 | (value & 0xffff),
                        OperandSize::Dword => value & 0xffff_ffff,
                        OperandSize::Qword => value,
                    };
                    assert_eq!(state.gpr[encoding as usize], expected);
                    assert_eq!(state.read(view), value & size.mask());
                    state.gpr[encoding as usize] = before.gpr[encoding as usize];
                    assert_eq!(state, before);
                }
            }
        }
        assert_eq!(Register::from_encoding(16), None);
        assert_eq!(Register::from_encoding(u8::MAX), None);
    }

    #[test]
    fn byte_encodings_distinguish_high_bytes_rex_low_bytes_and_extensions() {
        for encoding in 0..16 {
            let rex = RegisterView::decode_byte(encoding, true).unwrap();
            assert_eq!(rex.register() as u8, encoding);
            let mut state = LongModeState {
                gpr: [u64::MAX; 16],
                ..LongModeState::default()
            };
            state.write(rex, 0x12);
            assert_eq!(state.gpr[encoding as usize], 0xffff_ffff_ffff_ff12);
            if encoding < 8 {
                let legacy = RegisterView::decode_byte(encoding, false).unwrap();
                let target = if encoding < 4 { encoding } else { encoding - 4 };
                assert_eq!(legacy.register() as u8, target);
                state.gpr = [0x1122_3344_5566_7788; 16];
                state.write(legacy, 0x1aa);
                assert_eq!(
                    state.gpr[target as usize],
                    if encoding < 4 {
                        0x1122_3344_5566_77aa
                    } else {
                        0x1122_3344_5566_aa88
                    }
                );
                assert_eq!(state.read(legacy), 0xaa);
            } else {
                assert_eq!(
                    RegisterView::decode_byte(encoding, false),
                    Err(RegisterError::RexRequired)
                );
            }
        }
        for encoding in 0..16 {
            let register = Register::from_encoding(encoding).unwrap();
            assert_eq!(
                RegisterView::high_byte(register, true),
                Err(RegisterError::HighByteWithRex)
            );
            if encoding >= 4 {
                assert_eq!(
                    RegisterView::high_byte(register, false),
                    Err(RegisterError::NoHighByte(register))
                );
            }
        }
        assert_eq!(
            RegisterView::decode_byte(16, true),
            Err(RegisterError::InvalidEncoding(16))
        );
    }

    #[test]
    fn fragment_adapter_roundtrips_without_truncating_new_state() {
        let fragment = FragmentState {
            gpr: [u64::MAX, 1, 2, 3],
            rip: 42,
            rflags: ARITHMETIC_FLAGS | 2,
        };
        let state = LongModeState::from(&fragment);
        assert_eq!(FragmentState::try_from(&state), Ok(fragment));
        let mut fs_state = state.clone();
        fs_state.fs_base = 0x1000;
        assert_eq!(
            FragmentState::try_from(&fs_state),
            Err(FragmentAdapterError::UnsupportedFsBase(0x1000))
        );
        let mut gs_state = state.clone();
        gs_state.gs_base = 0x2000;
        assert_eq!(
            FragmentState::try_from(&gs_state),
            Err(FragmentAdapterError::UnsupportedGsBase(0x2000))
        );
        let mut selector_state = state.clone();
        selector_state.fs_selector = 0x33;
        assert_eq!(
            FragmentState::try_from(&selector_state),
            Err(FragmentAdapterError::UnsupportedFsSelector(0x33))
        );
        let mut gs_selector_state = state.clone();
        gs_selector_state.gs_selector = 0x53;
        assert_eq!(
            FragmentState::try_from(&gs_selector_state),
            Err(FragmentAdapterError::UnsupportedGsSelector(0x53))
        );
        let mut gdtr_state = state.clone();
        let gdtr = Gdtr {
            base: 0x2000,
            limit: 0x2f,
        };
        gdtr_state.gdtr = Some(gdtr);
        assert_eq!(
            FragmentState::try_from(&gdtr_state),
            Err(FragmentAdapterError::UnsupportedGdtr(gdtr))
        );
        let mut idtr_state = state.clone();
        let idtr = Idtr { base: 0, limit: 0 };
        idtr_state.idtr = Some(idtr);
        assert_eq!(
            FragmentState::try_from(&idtr_state),
            Err(FragmentAdapterError::UnsupportedIdtr(idtr))
        );
        let mut cache_state = state.clone();
        let cache = FsDescriptorCache {
            limit: 0xffff,
            access: 0xf3,
            flags: 0,
        };
        cache_state.fs_cache = Some(cache);
        assert_eq!(
            FragmentState::try_from(&cache_state),
            Err(FragmentAdapterError::UnsupportedFsCache(cache))
        );
        let mut ds_cache_state = state.clone();
        let ds_cache = DsDescriptorCache {
            base: 0x1234_5678,
            limit: 0xffff,
            access: 0x93,
            flags: 0,
        };
        ds_cache_state.ds_cache = Some(ds_cache);
        assert_eq!(
            FragmentState::try_from(&ds_cache_state),
            Err(FragmentAdapterError::UnsupportedDsCache(ds_cache))
        );
        let mut es_cache_state = LongModeState::default();
        let es_cache = DataSegmentDescriptorCache {
            base: 0x8765_4321,
            limit: 0xffff,
            access: 0x93,
            flags: 0,
        };
        es_cache_state.es_cache = Some(es_cache);
        assert_eq!(
            FragmentState::try_from(&es_cache_state),
            Err(FragmentAdapterError::UnsupportedEsCache(es_cache))
        );
        let mut combined_cache_state = LongModeState::default();
        combined_cache_state.ds_cache = Some(ds_cache);
        combined_cache_state.es_cache = Some(es_cache);
        assert_eq!(
            FragmentState::try_from(&combined_cache_state),
            Err(FragmentAdapterError::UnsupportedDsCache(ds_cache))
        );
        for encoding in 4..16 {
            let mut extra = state.clone();
            extra.gpr[encoding] = 1;
            assert_eq!(
                FragmentState::try_from(&extra),
                Err(FragmentAdapterError::ExtraRegister(
                    Register::from_encoding(encoding as u8).unwrap()
                ))
            );
        }
        let mut extra = state.clone();
        extra.rflags |= 1 << 10;
        assert_eq!(
            FragmentState::try_from(&extra),
            Err(FragmentAdapterError::UnsupportedFlags(1 << 10))
        );
        extra.rflags = 0;
        assert_eq!(
            FragmentState::try_from(&extra),
            Err(FragmentAdapterError::ReservedFlagMissing)
        );
        #[cfg(target_pointer_width = "32")]
        {
            extra = state;
            extra.rip = u64::MAX;
            assert_eq!(
                FragmentState::try_from(&extra),
                Err(FragmentAdapterError::InstructionPointerOutOfRange(u64::MAX))
            );
        }
    }
}
