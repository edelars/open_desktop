//! Demand decoder for the initial long-mode integer slice.
//! Encoding rules: Intel SDM 093 Vol.2A §§2.1.1–2.2.1, ADD/CMP/Jcc;
//! Vol.2B MOV/SUB. This is independent of the strict S0 fragment decoder.

use crate::memory::{AccessContext, ArchitecturalFault, GuestMemory, MemoryError};
use crate::{LongModeState, OperandSize, Register, RegisterView};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressBase {
    None,
    Register(Register),
    NextRip,
}

/// Flat long-mode address expression. CS/DS/ES/SS bases are zero;
/// FS/GS memory forms are unsupported until actual bases are in CPU state.
/// Rules: SDM093 Vol.1 §§3.3.7.1,3.4.2.1,3.7.4.1; Vol.2A §§2.1.3–2.2.1.6.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryOperand {
    pub base: AddressBase,
    pub index: Option<Register>,
    pub scale: u8,
    pub displacement: i64,
    pub address_size: OperandSize,
    pub size: OperandSize,
}

impl MemoryOperand {
    /// Effective offset only. `next_rip` is the guest RIP after the entire
    /// instruction, including immediates. No canonical check (also usable by LEA).
    pub fn effective_address(self, state: &LongModeState, next_rip: u64) -> u64 {
        let base = match self.base {
            AddressBase::None => 0,
            AddressBase::Register(register) => state.gpr[register as usize],
            AddressBase::NextRip => next_rip,
        };
        let index = self
            .index
            .map_or(0, |register| state.gpr[register as usize]);
        base.wrapping_add(index.wrapping_mul(u64::from(self.scale)))
            .wrapping_add_signed(self.displacement)
            & self.address_size.mask()
    }

    /// Start address in the selected zero-base, canonical48, no-LA57 profile.
    /// Full access-range checks and ordering belong to the instruction executor.
    pub fn linear_address_48(
        self,
        state: &LongModeState,
        next_rip: u64,
    ) -> Result<u64, ArchitecturalFault> {
        let address = self.effective_address(state, next_rip);
        if canonical(address) {
            return Ok(address);
        }
        Err(self.canonical_fault())
    }

    /// Validate the entire MOV access before calling a memory backend. Address
    /// size truncates the starting effective address, not each operand byte.
    /// Canonical48 checks apply to every addressed byte. A range extending
    /// past u64 is a diagnostic AddressOverflow until its architectural behavior
    /// is independently established; do not invent #GP/#SS or wrap the access.
    pub fn access_address_48(
        self,
        state: &LongModeState,
        next_rip: u64,
    ) -> Result<u64, MemoryError> {
        let start = self
            .linear_address_48(state, next_rip)
            .map_err(MemoryError::Architectural)?;
        let last = start.checked_add(self.size.bytes() as u64 - 1).ok_or(
            MemoryError::AddressOverflow {
                address: start,
                length: self.size.bytes(),
            },
        )?;
        if canonical(last) {
            return Ok(start);
        }
        Err(MemoryError::Architectural(self.canonical_fault()))
    }

    /// Validate a segment-base-adjusted access. The effective offset is
    /// address-size truncated before the 64-bit base is added.
    pub fn access_address_48_with_segment_base(
        self,
        state: &LongModeState,
        next_rip: u64,
        segment_base: u64,
    ) -> Result<u64, MemoryError> {
        let start = self
            .effective_address(state, next_rip)
            .wrapping_add(segment_base);
        let last = start.checked_add(self.size.bytes() as u64 - 1).ok_or(
            MemoryError::AddressOverflow {
                address: start,
                length: self.size.bytes(),
            },
        )?;
        if canonical(start) && canonical(last) {
            return Ok(start);
        }
        Err(MemoryError::Architectural(ArchitecturalFault {
            vector: 13,
            error_code: Some(0),
            linear_address: None,
        }))
    }

    fn canonical_fault(self) -> ArchitecturalFault {
        // SDM093 Vol.1 §3.3.7.1: RSP/RBP bases use SS. The extended
        // GPRs R12/R13 remain data references; an index alone does not select SS.
        let stack = matches!(
            self.base,
            AddressBase::Register(Register::Rsp | Register::Rbp)
        );
        ArchitecturalFault {
            vector: if stack { 12 } else { 13 },
            error_code: Some(0),
            linear_address: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operand {
    Register(RegisterView),
    Memory(MemoryOperand),
    Immediate(u64),
}

/// Integer operations whose six arithmetic flags are eagerly materialized.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArithmeticKind {
    Add,
    Adc,
    Sub,
    Sbb,
    Cmp,
    Inc,
    Dec,
    Neg,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogicKind {
    And,
    Or,
    Xor,
    Test,
    Not,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShiftKind {
    Shl,
    Shr,
    Sar,
    Rol,
    Ror,
    Rcl,
    Rcr,
    Shld,
    Shrd,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BitKind {
    Test,
    Set,
    Reset,
    Complement,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BitOffset {
    Register(RegisterView),
    Immediate(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BitScanKind {
    Forward,
    Reverse,
}

/// Legacy Jcc conditions, selected by the opcode's low four bits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ConditionCode {
    Overflow = 0,
    NotOverflow = 1,
    Below = 2,
    AboveOrEqual = 3,
    Equal = 4,
    NotEqual = 5,
    BelowOrEqual = 6,
    Above = 7,
    Sign = 8,
    NotSign = 9,
    Parity = 10,
    NotParity = 11,
    Less = 12,
    GreaterOrEqual = 13,
    LessOrEqual = 14,
    Greater = 15,
}

impl ConditionCode {
    pub const fn from_encoding(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::Overflow,
            1 => Self::NotOverflow,
            2 => Self::Below,
            3 => Self::AboveOrEqual,
            4 => Self::Equal,
            5 => Self::NotEqual,
            6 => Self::BelowOrEqual,
            7 => Self::Above,
            8 => Self::Sign,
            9 => Self::NotSign,
            10 => Self::Parity,
            11 => Self::NotParity,
            12 => Self::Less,
            13 => Self::GreaterOrEqual,
            14 => Self::LessOrEqual,
            15 => Self::Greater,
            _ => return None,
        })
    }

    pub const fn evaluate(self, rflags: u64) -> bool {
        let carry = rflags & crate::CF != 0;
        let parity = rflags & crate::PF != 0;
        let zero = rflags & crate::ZF != 0;
        let sign = rflags & crate::SF != 0;
        let overflow = rflags & crate::OF != 0;
        match self {
            Self::Overflow => overflow,
            Self::NotOverflow => !overflow,
            Self::Below => carry,
            Self::AboveOrEqual => !carry,
            Self::Equal => zero,
            Self::NotEqual => !zero,
            Self::BelowOrEqual => carry || zero,
            Self::Above => !carry && !zero,
            Self::Sign => sign,
            Self::NotSign => !sign,
            Self::Parity => parity,
            Self::NotParity => !parity,
            Self::Less => sign != overflow,
            Self::GreaterOrEqual => sign == overflow,
            Self::LessOrEqual => zero || sign != overflow,
            Self::Greater => !zero && sign == overflow,
        }
    }
}

/// Architectural definedness changes, independent of concrete RFLAGS values.
/// Apply only after the corresponding instruction successfully retires.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FlagEffects {
    pub defined: u64,
    pub undefined: u64,
}

impl FlagEffects {
    pub const fn apply(self, previous: u64) -> u64 {
        (previous | self.defined) & !self.undefined
    }
}

fn logic_kind(group: u8) -> Option<LogicKind> {
    match group {
        1 => Some(LogicKind::Or),
        4 => Some(LogicKind::And),
        6 => Some(LogicKind::Xor),
        _ => None,
    }
}

fn direct_logic(byte: u8) -> Option<LogicKind> {
    if byte <= 0x35 && byte & 7 <= 5 {
        logic_kind(byte >> 3)
    } else {
        None
    }
}

fn binary_operation(group: u8, dst: Operand, src: Operand) -> Operation {
    if let Some(kind) = binary_kind(group) {
        Operation::Arithmetic { kind, dst, src }
    } else {
        Operation::Logic {
            kind: logic_kind(group).unwrap(),
            dst,
            src,
        }
    }
}

fn binary_kind(group: u8) -> Option<ArithmeticKind> {
    match group {
        0 => Some(ArithmeticKind::Add),
        2 => Some(ArithmeticKind::Adc),
        3 => Some(ArithmeticKind::Sbb),
        5 => Some(ArithmeticKind::Sub),
        7 => Some(ArithmeticKind::Cmp),
        _ => None,
    }
}

fn direct_arithmetic(byte: u8) -> Option<ArithmeticKind> {
    if byte <= 0x3d && byte & 7 <= 5 {
        binary_kind(byte >> 3)
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpcodeMap {
    Primary,
    Escape0f,
    Escape0f38,
    Escape0f3a,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Opcode {
    pub map: OpcodeMap,
    pub byte: u8,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Prefixes {
    pub group1: Option<u8>,
    pub segment: Option<u8>,
    pub operand_override: bool,
    pub address_override: bool,
    /// Only the REX immediately preceding the opcode/escape byte is effective.
    pub rex: Option<u8>,
}

impl Prefixes {
    fn operand_size(self) -> OperandSize {
        if self.rex.unwrap_or(0) & 8 != 0 {
            OperandSize::Qword
        } else if self.operand_override {
            OperandSize::Word
        } else {
            OperandSize::Dword
        }
    }
}

fn flags_stack_size(prefixes: Prefixes) -> Option<OperandSize> {
    if prefixes.group1.is_some() || prefixes.segment.is_some() || prefixes.address_override {
        return None;
    }
    Some(
        if prefixes.rex.is_some_and(|rex| rex & 8 != 0) || !prefixes.operand_override {
            OperandSize::Qword
        } else {
            OperandSize::Word
        },
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    MovImmediate {
        dst: Operand,
        value: u64,
    },
    Mov {
        dst: Operand,
        src: Operand,
    },
    /// The sole supported FS-based memory form in S2-22a.
    MovFsLoad {
        dst: RegisterView,
        src: MemoryOperand,
    },
    /// The sole supported GS-based memory form in S2-22b.
    MovGsLoad {
        dst: RegisterView,
        src: MemoryOperand,
    },
    /// The sole supported segment-register read in S2-22c: `48 8C E0`.
    MovFsSelectorToRax,
    MovGsSelectorToRax,
    /// The sole S2-22e descriptor-rights check: bare `0F 00 E8` (VERW AX).
    VerwGdt,
    /// The sole S2-22f selector load: bare `8E E0` (MOV FS, AX).
    LoadFsSelector,
    /// The sole S2-22h selector load: bare `8E E8` (MOV GS, AX).
    LoadGsSelector,
    Extend {
        dst: RegisterView,
        src: Operand,
        signed: bool,
    },
    Lea {
        dst: RegisterView,
        src: MemoryOperand,
    },
    Xchg {
        left: RegisterView,
        right: RegisterView,
    },
    /// Bare long-mode qword XCHG between a GPR and ordinary memory.
    XchgMemory {
        register: RegisterView,
        memory: MemoryOperand,
    },
    /// Bare long-mode qword XADD between two GPRs.
    Xadd {
        destination: RegisterView,
        source: RegisterView,
    },
    /// Bare long-mode qword XADD from a GPR into ordinary memory.
    XaddMemory {
        destination: MemoryOperand,
        source: RegisterView,
    },
    /// Bare long-mode qword register-register CMPXCHG, using RAX as accumulator.
    Cmpxchg {
        destination: RegisterView,
        source: RegisterView,
    },
    /// Bare long-mode qword memory-destination CMPXCHG, using RAX as accumulator.
    CmpxchgMemory {
        destination: MemoryOperand,
        source: RegisterView,
    },
    /// Bare `0F AE F0` completion fence for explicitly opted-in memory backends.
    Mfence,
    /// Bare `DB E3` FNINIT in the local profile with an existing x87 state.
    Fninit,
    /// Bare `DB E2` FNCLEX in the local profile with an existing x87 state.
    Fnclex,
    /// Bare `DF E0` FNSTSW AX in the local profile with an existing x87 state.
    FnstswAx,
    /// Bare `D9 38` FNSTCW m2byte at [RAX], preserving the x87 snapshot.
    FnstcwM16Rax,
    /// Bare `D9 28` FLDCW m2byte at [RAX], bounded all-masked control words.
    FldcwM16Rax,
    /// Bare `DD 38` FNSTSW m2byte at [RAX], preserving the x87 snapshot.
    FnstswM16Rax,
    /// Bare `DD 18` FSTP m64fp at [RAX], bounded finite profile.
    FstpM64FpRax,
    /// Bare `DB 38` FSTP m80fp at [RAX], bounded raw binary80 profile.
    FstpM80FpRax,
    /// Bare `DB 28` FLD m80fp at [RAX], bounded Normal binary80 stack push.
    FldM80FpRax,
    /// Bare `DD 10` FST m64fp at [RAX], bounded finite profile.
    FstM64FpRax,
    /// Bare `DD 00` FLD m64fp at [RAX], bounded Normal binary64 stack push.
    FldM64FpRax,
    /// Bare `D9 E0` FCHS, flipping the sign of canonical-Normal ST(0) only.
    Fchs,
    /// Bare `D9 E1` FABS, clearing the sign of canonical-Normal ST(0) only.
    Fabs,
    /// Bare `D9 E4` FTST, comparing canonical-Normal ST(0) with zero.
    Ftst,
    /// Bare `D9 FC` FRNDINT in the bounded PC64/finite Normal profile.
    Frndint,
    /// Bare `D9 18` FSTP m32fp at [RAX], bounded finite profile.
    FstpM32FpRax,
    /// Bare `D9 00` FLD m32fp at [RAX], bounded Normal binary32 stack push.
    FldM32FpRax,
    /// Bare `D9 10` FST m32fp at [RAX], bounded finite profile.
    FstM32FpRax,
    /// Bare `DB 18` FISTP dword integer at [RAX], bounded finite profile.
    FistpM32IntRax,
    /// Bare `DF 18` FISTP word integer at [RAX], bounded finite profile.
    FistpM16IntRax,
    /// Bare `DF 38` FISTP qword integer at [RAX], bounded finite profile.
    FistpM64IntRax,
    /// Bare `DF 10` FIST word integer at [RAX], bounded finite profile.
    FistM16IntRax,
    /// Bare `DB 10` FIST dword integer at [RAX], bounded finite profile.
    FistM32IntRax,
    /// Bare `DB 00` FILD dword integer at [RAX], bounded stack-push profile.
    FildM32IntRax,
    /// Bare `DF 00` FILD word integer at [RAX], bounded stack-push profile.
    FildM16IntRax,
    /// Bare `DF 28` FILD qword integer at [RAX], bounded stack-push profile.
    FildM64IntRax,
    /// Bare `D8 D0+i` FCOM ST(i), bounded canonical-Normal register compare.
    FcomSt {
        source: u8,
    },
    /// Bare DB F0+i FCOMI ST(0), ST(i), bounded canonical-Normal compare.
    Fcomi {
        source: u8,
    },
    /// Bare DB E8+i FUCOMI ST(0), ST(i), bounded canonical-Normal compare.
    Fucomi {
        source: u8,
    },
    /// Bare DF F0+i FCOMIP ST(0), ST(i), bounded compare then physical TOP pop.
    Fcomip {
        source: u8,
    },
    /// Bare DF E8+i FUCOMIP ST(0), ST(i), bounded compare then physical TOP pop.
    Fucomip {
        source: u8,
    },
    /// Bare `D8 D8+i` FCOMP ST(i), bounded compare then physical TOP pop.
    FcompSt {
        source: u8,
    },
    /// Bare `DE D9` FCOMPP, bounded compare then two-entry physical TOP pop.
    Fcompp,
    /// Bare `DA E9` FUCOMPP, bounded compare then two-entry physical TOP pop.
    Fucompp,
    /// Bare `DD E1` FUCOM ST(0), ST(1), bounded canonical-Normal comparison.
    Fucom,
    /// Bare `DD E9` FUCOMP ST(0), ST(1), bounded compare then physical TOP pop.
    Fucomp,
    /// Bare `DD D8+i` FSTP ST(i), exact Normal source and physical TOP pop only.
    FstpSti {
        destination: u8,
    },
    /// Bare `DD D0+i` FST ST(i), raw Normal copy without changing TOP.
    FstSti {
        destination: u8,
    },
    /// Bare `D8 C8+i` FMUL ST(0), ST(i), exact finite local profile only.
    FmulSt0St {
        source: u8,
    },
    /// Bare `D8 F0+i` FDIV ST(0), ST(i), exact finite local profile only.
    FdivSt0St {
        source: u8,
    },
    /// Bare `D8 F8+i` FDIVR ST(0), ST(i), exact finite local profile only.
    FdivrSt0St {
        source: u8,
    },
    /// Bare `DC F0+i` FDIVR ST(i), ST(0), exact finite local profile only.
    FdivrStSt0 {
        destination: u8,
    },
    /// Bare `DC F8+i` FDIV ST(i), ST(0), exact finite local profile only.
    FdivStSt0 {
        destination: u8,
    },
    /// Bare `DE F8+i` FDIVP ST(i), ST(0), exact finite local profile only.
    FdivpStSt0 {
        destination: u8,
    },
    /// Bare `DE F0+i` FDIVRP ST(i), ST(0), exact finite local profile only.
    FdivrpStSt0 {
        destination: u8,
    },
    /// Bare `D8 C0+i` FADD ST(0), ST(i), exact finite local profile only.
    FaddSt0St {
        source: u8,
    },
    /// Bare `D8 E0+i` FSUB ST(0), ST(i), exact finite local profile only.
    FsubSt0St {
        source: u8,
    },
    /// Bare `D8 E8+i` FSUBR ST(0), ST(i), exact finite local profile only.
    FsubrSt0St {
        source: u8,
    },
    /// Bare `DC E8+i` FSUB ST(i), ST(0), exact finite local profile only.
    FsubStSt0 {
        destination: u8,
    },
    /// Bare `DC E0+i` FSUBR ST(i), ST(0), exact finite local profile only.
    FsubrStSt0 {
        destination: u8,
    },
    /// Bare `DC C0+i` FADD ST(i), ST(0), exact finite local profile only.
    FaddStSt0 {
        destination: u8,
    },
    /// Bare `DC C8+i` FMUL ST(i), ST(0), exact finite local profile only.
    FmulStSt0 {
        destination: u8,
    },
    /// Bare `DE C0+i` FADDP ST(i), ST(0), exact finite local profile only.
    FaddpStSt0 {
        destination: u8,
    },
    /// Bare `DE C8+i` FMULP ST(i), ST(0), exact finite local profile only.
    FmulpStSt0 {
        destination: u8,
    },
    /// Bare `DE E8+i` FSUBP ST(i), ST(0), exact finite local profile only.
    FsubpStSt0 {
        destination: u8,
    },
    /// Bare `DE E0+i` FSUBRP ST(i), ST(0), exact finite local profile only.
    FsubrpStSt0 {
        destination: u8,
    },
    Nop,
    /// Long-mode 16- or 64-bit PUSH/POP of an opcode-encoded GPR.
    StackRegister {
        push: bool,
        register: Register,
    },
    /// Long-mode POPF/POPFQ (`9D`), limited to the documented profile flags.
    PopFlags,
    /// Long-mode PUSHF/PUSHFQ (`9C`), with width selected by operand_size.
    PushFlags,
    /// One accepted long-mode MOVS iteration using implicit RSI and RDI.
    Movs {
        size: OperandSize,
    },
    /// One accepted long-mode STOS iteration using implicit AL and RDI.
    Stos {
        size: OperandSize,
    },
    /// One bounded invocation of long-mode REP STOSB (`F3 AA`).
    RepStos {
        size: OperandSize,
    },
    /// Long-mode 16- or 64-bit PUSH immediate; `value` is extended to the stack width.
    PushImmediate {
        value: u64,
    },
    /// Long-mode qword PUSH from an ordinary memory effective address.
    PushMemory {
        source: MemoryOperand,
    },
    /// Long-mode qword POP to an ordinary memory effective address.
    PopMemory {
        destination: MemoryOperand,
    },
    /// CBW/CWDE/CDQE (double_width = false) or CWD/CDQ/CQO
    /// (double_width = true), selected by opcode 98/99 and operand size.
    SignExtendAccumulator {
        double_width: bool,
    },
    /// BSWAP reverses the bytes in one opcode-encoded 32- or 64-bit GPR.
    Bswap {
        dst: RegisterView,
    },
    /// BT/BTS/BTR/BTC. The destination is ModR/M.r/m; the bit offset is
    /// ModR/M.reg for register-offset forms or the immediate byte for grouped
    /// forms. Memory register offsets select adjacent operand-sized elements.
    Bit {
        kind: BitKind,
        dst: Operand,
        offset: BitOffset,
    },
    /// BSF/BSR register and memory source forms. F3-prefixed forms are outside this slice.
    BitScan {
        kind: BitScanKind,
        dst: RegisterView,
        src: Operand,
    },
    Arithmetic {
        kind: ArithmeticKind,
        dst: Operand,
        src: Operand,
    },
    Logic {
        kind: LogicKind,
        dst: Operand,
        src: Operand,
    },
    /// One-operand MUL/IMUL. The operand is also the explicit r/m source;
    /// accumulator and high-half destinations are implicit in the width.
    Mul {
        signed: bool,
        operand: Operand,
    },
    /// Two-operand IMUL r, r/m (0F AF): multiply the old destination by src.
    Imul {
        dst: RegisterView,
        src: Operand,
    },
    /// Three-operand IMUL r, r/m, immediate (69/6B). The immediate has
    /// already been sign-extended to the destination width.
    ImulImmediate {
        dst: RegisterView,
        src: Operand,
        immediate: u64,
    },
    /// One-operand DIV/IDIV; accumulator pair and quotient/remainder
    /// destinations are implicit in the width.
    Divide {
        signed: bool,
        divisor: Operand,
    },
    Shift {
        kind: ShiftKind,
        dst: Operand,
        /// Present only for SHLD/SHRD; these take a second register source.
        source: Option<Operand>,
        count: Operand,
    },
    Branch {
        condition: ConditionCode,
        displacement: i32,
    },
    /// SETcc stores one condition result byte into a register or memory.
    SetCondition {
        condition: ConditionCode,
        dst: Operand,
    },
    /// CMOVcc reads its r/m source regardless of the condition result.
    ConditionalMove {
        condition: ConditionCode,
        dst: RegisterView,
        src: Operand,
    },
    Jump {
        displacement: i32,
    },
    /// Long-mode near direct CALL rel32, pushing the next instruction RIP.
    Call {
        displacement: i32,
    },
    /// Long-mode near RET, popping a 64-bit target and optional byte count.
    Return {
        stack_adjust: u16,
    },
    /// Long-mode near indirect JMP through a 64-bit register or memory operand.
    IndirectJump {
        operand: Operand,
    },
    /// Long-mode near indirect CALL through a 64-bit register or memory operand.
    IndirectCall {
        operand: Operand,
    },
}

/// Exact register-only FCOMI-family encodings admitted by the bounded x87
/// profile. The low three ModR/M bits are the architectural source index.
fn indexed_x87_compare(opcode: u8, modrm: u8) -> Option<Operation> {
    match (opcode, modrm) {
        (0xdb, 0xe8..=0xef) => Some(Operation::Fucomi { source: modrm & 7 }),
        (0xdb, 0xf0..=0xf7) => Some(Operation::Fcomi { source: modrm & 7 }),
        (0xdf, 0xe8..=0xef) => Some(Operation::Fucomip { source: modrm & 7 }),
        (0xdf, 0xf0..=0xf7) => Some(Operation::Fcomip { source: modrm & 7 }),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Instruction {
    pub length: u8,
    pub opcode: Opcode,
    pub prefixes: Prefixes,
    pub operand_size: OperandSize,
    pub address_size: OperandSize,
    pub operation: Operation,
}

impl Instruction {
    /// SDM093 scalar instruction flag effects. Undefined bits are metadata,
    /// not assertions about the concrete bits a particular CPU produces.
    pub fn flag_effects(self) -> FlagEffects {
        match self.operation {
            Operation::PopFlags => FlagEffects {
                defined: crate::POPFQ_INTEGER_FLAGS,
                undefined: 0,
            },
            Operation::Arithmetic { kind, .. } => FlagEffects {
                defined: if matches!(kind, ArithmeticKind::Inc | ArithmeticKind::Dec) {
                    crate::ARITHMETIC_FLAGS & !crate::CF
                } else {
                    crate::ARITHMETIC_FLAGS
                },
                undefined: 0,
            },
            Operation::Xadd { .. } => FlagEffects {
                defined: crate::ARITHMETIC_FLAGS,
                undefined: 0,
            },
            Operation::XaddMemory { .. } => FlagEffects {
                defined: crate::ARITHMETIC_FLAGS,
                undefined: 0,
            },
            Operation::Cmpxchg { .. } => FlagEffects {
                defined: crate::ARITHMETIC_FLAGS,
                undefined: 0,
            },
            Operation::CmpxchgMemory { .. } => FlagEffects {
                defined: crate::ARITHMETIC_FLAGS,
                undefined: 0,
            },
            Operation::Logic {
                kind: LogicKind::Not,
                ..
            } => FlagEffects::default(),
            Operation::Logic { .. } => FlagEffects {
                defined: crate::ARITHMETIC_FLAGS & !crate::AF,
                undefined: crate::AF,
            },
            Operation::Mul { .. } | Operation::Imul { .. } | Operation::ImulImmediate { .. } => {
                FlagEffects {
                    defined: crate::CF | crate::OF,
                    undefined: crate::PF | crate::AF | crate::ZF | crate::SF,
                }
            }
            Operation::Divide { .. } => FlagEffects {
                defined: 0,
                undefined: crate::ARITHMETIC_FLAGS,
            },
            Operation::Shift { .. } => FlagEffects::default(),
            Operation::Bit { .. } => FlagEffects {
                defined: crate::CF,
                undefined: crate::PF | crate::AF | crate::SF | crate::OF,
            },
            Operation::BitScan { .. } => FlagEffects {
                defined: crate::ARITHMETIC_FLAGS,
                undefined: 0,
            },
            Operation::VerwGdt => FlagEffects {
                defined: crate::ZF,
                undefined: 0,
            },
            Operation::Fcomi { .. }
            | Operation::Fcomip { .. }
            | Operation::Fucomi { .. }
            | Operation::Fucomip { .. } => FlagEffects {
                defined: 0x8d5,
                undefined: 0,
            },
            _ => FlagEffects::default(),
        }
    }

    /// Flag definedness for a shift/rotate after the architectural count
    /// mask and rotate reduction. `raw_count` is the value read from imm8/CL.
    pub fn flag_effects_for_count(self, raw_count: u8) -> FlagEffects {
        let Operation::Shift { kind, .. } = self.operation else {
            return self.flag_effects();
        };
        let width = match self.operand_size {
            OperandSize::Byte => 8,
            OperandSize::Word => 16,
            OperandSize::Dword => 32,
            OperandSize::Qword => 64,
        };
        let masked = raw_count & if width == 64 { 0x3f } else { 0x1f };
        if masked == 0 {
            return FlagEffects::default();
        }
        let count = match kind {
            ShiftKind::Rol | ShiftKind::Ror => masked % width,
            ShiftKind::Rcl | ShiftKind::Rcr if width == 8 => masked % 9,
            ShiftKind::Rcl | ShiftKind::Rcr if width == 16 => masked % 17,
            _ => masked,
        };
        let result_flags = crate::SF | crate::ZF | crate::PF;
        let mut effects = match kind {
            ShiftKind::Rol | ShiftKind::Ror => FlagEffects {
                defined: crate::CF | if masked == 1 { crate::OF } else { 0 },
                undefined: if masked == 1 { 0 } else { crate::OF },
            },
            ShiftKind::Rcl | ShiftKind::Rcr => FlagEffects {
                defined: (if count != 0 { crate::CF } else { 0 })
                    | if masked == 1 { crate::OF } else { 0 },
                undefined: if masked == 1 { 0 } else { crate::OF },
            },
            ShiftKind::Shld | ShiftKind::Shrd if count > width => FlagEffects {
                defined: 0,
                undefined: crate::ARITHMETIC_FLAGS,
            },
            ShiftKind::Shl
            | ShiftKind::Shr
            | ShiftKind::Sar
            | ShiftKind::Shld
            | ShiftKind::Shrd
                if count == 0 =>
            {
                FlagEffects::default()
            }
            _ => FlagEffects {
                defined: result_flags
                    | if matches!(kind, ShiftKind::Sar)
                        || count < width
                        || (matches!(kind, ShiftKind::Shld | ShiftKind::Shrd) && count <= width)
                    {
                        crate::CF
                    } else {
                        0
                    }
                    | if count == 1 { crate::OF } else { 0 },
                undefined: crate::AF
                    | if matches!(kind, ShiftKind::Shl | ShiftKind::Shr) && count >= width {
                        crate::CF
                    } else {
                        0
                    }
                    | if count != 1 { crate::OF } else { 0 },
            },
        };
        // For rotates, SF/ZF/PF/AF are unchanged; the effects above only name
        // CF and (when undefined) OF.
        effects.defined &= crate::INTEGER_FLAGS;
        effects
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecodeError {
    /// Outside this decoder's slice, including duplicate legacy prefixes.
    /// Length is the consumed prefix/opcode boundary, not a full instruction length.
    Unsupported {
        opcode: Opcode,
        length: u8,
    },
    /// A known architecturally invalid form (#UD), distinct from unknown encodings.
    Invalid {
        opcode: Opcode,
        length: u8,
    },
    Memory(MemoryError),
}

impl From<MemoryError> for DecodeError {
    fn from(error: MemoryError) -> Self {
        Self::Memory(error)
    }
}

struct Fetch<'a, M> {
    memory: &'a mut M,
    start: u64,
    length: u8,
    context: AccessContext,
}

impl<M: GuestMemory> Fetch<'_, M> {
    fn byte(&mut self) -> Result<u8, DecodeError> {
        if self.length == 15 {
            return Err(general_protection().into());
        }
        let address = self.start.wrapping_add(u64::from(self.length));
        if !canonical(address) {
            return Err(general_protection().into());
        }
        let value = self.memory.fetch(address, self.context)?;
        self.length += 1;
        Ok(value)
    }

    fn immediate(&mut self, size: OperandSize) -> Result<u64, DecodeError> {
        let mut bytes = [0; 8];
        for byte in &mut bytes[..size.bytes()] {
            *byte = self.byte()?;
        }
        Ok(u64::from_le_bytes(bytes))
    }

    fn rm_operand(
        &mut self,
        modrm: u8,
        prefixes: Prefixes,
        size: OperandSize,
    ) -> Result<Operand, DecodeError> {
        let mode = modrm >> 6;
        let rm = modrm & 7;
        let rex = prefixes.rex.unwrap_or(0);
        if mode == 3 {
            return Ok(Operand::Register(view(
                rm | ((rex & 1) << 3),
                size,
                prefixes.rex.is_some(),
            )));
        }
        let mut index = None;
        let mut scale = 1;
        let base = if rm == 4 {
            let sib = self.byte()?;
            scale = 1 << (sib >> 6);
            let index_encoding = ((sib >> 3) & 7) | ((rex & 2) << 2);
            if index_encoding != 4 {
                index = Register::from_encoding(index_encoding);
            }
            if mode == 0 && sib & 7 == 5 {
                AddressBase::None
            } else {
                AddressBase::Register(
                    Register::from_encoding((sib & 7) | ((rex & 1) << 3)).unwrap(),
                )
            }
        } else if mode == 0 && rm == 5 {
            AddressBase::NextRip
        } else {
            AddressBase::Register(Register::from_encoding(rm | ((rex & 1) << 3)).unwrap())
        };
        let displacement = if mode == 1 {
            i64::from(self.byte()? as i8)
        } else if mode == 2 || matches!(base, AddressBase::None | AddressBase::NextRip) {
            i64::from(self.immediate(OperandSize::Dword)? as i32)
        } else {
            0
        };
        Ok(Operand::Memory(MemoryOperand {
            base,
            index,
            scale,
            displacement,
            address_size: if prefixes.address_override {
                OperandSize::Dword
            } else {
                OperandSize::Qword
            },
            size,
        }))
    }

    fn decode(&mut self) -> Result<Instruction, DecodeError> {
        let mut prefixes = Prefixes::default();
        let mut groups = 0u8;
        let mut duplicate = false;
        let first = loop {
            let byte = self.byte()?;
            let group = match byte {
                0xf0 | 0xf2 | 0xf3 => {
                    prefixes.group1 = Some(byte);
                    1
                }
                0x26 | 0x2e | 0x36 | 0x3e | 0x64 | 0x65 => {
                    prefixes.segment = Some(byte);
                    2
                }
                0x66 => {
                    prefixes.operand_override = true;
                    4
                }
                0x67 => {
                    prefixes.address_override = true;
                    8
                }
                0x40..=0x4f => {
                    prefixes.rex = Some(byte);
                    continue;
                }
                _ => break byte,
            };
            duplicate |= groups & group != 0;
            groups |= group;
            prefixes.rex = None;
        };
        let opcode = if first == 0x0f {
            match self.byte()? {
                0x38 => Opcode {
                    map: OpcodeMap::Escape0f38,
                    byte: self.byte()?,
                },
                0x3a => Opcode {
                    map: OpcodeMap::Escape0f3a,
                    byte: self.byte()?,
                },
                byte => Opcode {
                    map: OpcodeMap::Escape0f,
                    byte,
                },
            }
        } else {
            Opcode {
                map: OpcodeMap::Primary,
                byte: first,
            }
        };
        // §2.1.1 specifies useful encodings with at most one prefix per group.
        // Duplicate groups are outside this slice: do not manufacture #UD or
        // claim a universal last-prefix-wins rule.
        if duplicate {
            return Err(self.unsupported(opcode));
        }
        let known = (opcode.map == OpcodeMap::Primary
            && (direct_arithmetic(opcode.byte).is_some() || direct_logic(opcode.byte).is_some()))
            || matches!((opcode.map, opcode.byte),
            (OpcodeMap::Primary, 0x50..=0x5f | 0x68 | 0x6a | 0x69 | 0x6b | 0x70..=0x7f | 0x8c | 0x8e | 0x8f | 0x9c | 0x9d | 0x98 | 0x99 | 0xb0..=0xbf | 0x84..=0x8b | 0x8d | 0x90..=0x97 | 0xa0..=0xa5 | 0xa8 | 0xa9 | 0xaa | 0xc0 | 0xc1 | 0xc2 | 0xc3 | 0xd0..=0xd3 | 0xd8 | 0xd9 | 0xda | 0xdb | 0xdc | 0xdd | 0xde | 0xdf | 0xc6 | 0xc7 | 0x63 | 0xe8 | 0xe9 | 0xeb | 0x80 | 0x81 | 0x83 | 0xfe | 0xff | 0xf6 | 0xf7)
            | (OpcodeMap::Escape0f, 0x00 | 0x40..=0x4f | 0x80..=0x8f | 0x90..=0x9f | 0xa3 | 0xab | 0xae | 0xaf | 0xb1 | 0xb3 | 0xbb | 0xba | 0xa4 | 0xa5 | 0xac | 0xad | 0xb6 | 0xb7 | 0xbc | 0xbd | 0xbe | 0xbf | 0xc1 | 0xc8..=0xcf));
        if opcode.map == OpcodeMap::Escape0f && opcode.byte == 0x0b {
            return Err(DecodeError::Invalid {
                opcode,
                length: self.length,
            });
        }
        // LOCK is admitted only for the one qword memory XADD form modeled in
        // this slice. Other LOCK forms remain outside scope; returning #UD at
        // the prefix/opcode boundary would assert an unverified priority over
        // a later instruction-fetch fault. REP/mandatory SIMD prefixes likewise
        // require the specific instruction semantics.
        let locked_qword_memory_xadd_candidate = opcode.map == OpcodeMap::Escape0f
            && opcode.byte == 0xc1
            && prefixes.group1 == Some(0xf0);
        let rep_stosb = opcode.map == OpcodeMap::Primary
            && opcode.byte == 0xaa
            && prefixes
                == (Prefixes {
                    group1: Some(0xf3),
                    ..Prefixes::default()
                });
        if !known
            || (prefixes.group1.is_some() && !rep_stosb && !locked_qword_memory_xadd_candidate)
        {
            return Err(self.unsupported(opcode));
        }
        let mut size = prefixes.operand_size();
        let rex = prefixes.rex.unwrap_or(0);
        let operation = match (opcode.map, opcode.byte) {
            (OpcodeMap::Primary, 0xdb) => {
                if prefixes != Prefixes::default() {
                    return Err(self.unsupported(opcode));
                }
                let modrm = self.byte()?;
                match modrm {
                    0x00 => Operation::FildM32IntRax,
                    0x28 => Operation::FldM80FpRax,
                    0x38 => Operation::FstpM80FpRax,
                    0xe2 => Operation::Fnclex,
                    0xe3 => Operation::Fninit,
                    0x10 => Operation::FistM32IntRax,
                    0x18 => Operation::FistpM32IntRax,
                    _ => indexed_x87_compare(opcode.byte, modrm)
                        .ok_or_else(|| self.unsupported(opcode))?,
                }
            }
            (OpcodeMap::Primary, 0xdf) => {
                if prefixes != Prefixes::default() {
                    return Err(self.unsupported(opcode));
                }
                let modrm = self.byte()?;
                match modrm {
                    0x00 => Operation::FildM16IntRax,
                    0x28 => Operation::FildM64IntRax,
                    0xe0 => Operation::FnstswAx,
                    0x10 => Operation::FistM16IntRax,
                    0x18 => Operation::FistpM16IntRax,
                    0x38 => Operation::FistpM64IntRax,
                    _ => indexed_x87_compare(opcode.byte, modrm)
                        .ok_or_else(|| self.unsupported(opcode))?,
                }
            }
            (OpcodeMap::Primary, 0xd8) => {
                if prefixes != Prefixes::default() {
                    return Err(self.unsupported(opcode));
                }
                let modrm = self.byte()?;
                match modrm {
                    0xd0..=0xd7 => Operation::FcomSt { source: modrm & 7 },
                    0xd8..=0xdf => Operation::FcompSt { source: modrm & 7 },
                    0xc0..=0xc7 => Operation::FaddSt0St { source: modrm & 7 },
                    0xc8..=0xcf => Operation::FmulSt0St { source: modrm & 7 },
                    0xe0..=0xe7 => Operation::FsubSt0St { source: modrm & 7 },
                    0xe8..=0xef => Operation::FsubrSt0St { source: modrm & 7 },
                    0xf0..=0xf7 => Operation::FdivSt0St { source: modrm & 7 },
                    0xf8..=0xff => Operation::FdivrSt0St { source: modrm & 7 },
                    _ => return Err(self.unsupported(opcode)),
                }
            }
            (OpcodeMap::Primary, 0xdc) => {
                if prefixes != Prefixes::default() {
                    return Err(self.unsupported(opcode));
                }
                let modrm = self.byte()?;
                match modrm {
                    0xc8..=0xcf => Operation::FmulStSt0 {
                        destination: modrm & 7,
                    },
                    0xe0..=0xe7 => Operation::FsubrStSt0 {
                        destination: modrm & 7,
                    },
                    0xc0..=0xc7 => Operation::FaddStSt0 {
                        destination: modrm & 7,
                    },
                    0xe8..=0xef => Operation::FsubStSt0 {
                        destination: modrm & 7,
                    },
                    0xf0..=0xf7 => Operation::FdivrStSt0 {
                        destination: modrm & 7,
                    },
                    0xf8..=0xff => Operation::FdivStSt0 {
                        destination: modrm & 7,
                    },
                    _ => return Err(self.unsupported(opcode)),
                }
            }
            (OpcodeMap::Primary, 0xde) => {
                if prefixes != Prefixes::default() {
                    return Err(self.unsupported(opcode));
                }
                let modrm = self.byte()?;
                match modrm {
                    0xc0..=0xc7 => Operation::FaddpStSt0 {
                        destination: modrm & 7,
                    },
                    0xc8..=0xcf => Operation::FmulpStSt0 {
                        destination: modrm & 7,
                    },
                    0xf0..=0xf7 => Operation::FdivrpStSt0 {
                        destination: modrm & 7,
                    },
                    0xe0..=0xe7 => Operation::FsubrpStSt0 {
                        destination: modrm & 7,
                    },
                    0xe8..=0xef => Operation::FsubpStSt0 {
                        destination: modrm & 7,
                    },
                    0xd9 => Operation::Fcompp,
                    0xf8..=0xff => Operation::FdivpStSt0 {
                        destination: modrm & 7,
                    },
                    _ => return Err(self.unsupported(opcode)),
                }
            }
            (OpcodeMap::Primary, 0xda) => {
                if prefixes != Prefixes::default() {
                    return Err(self.unsupported(opcode));
                }
                if self.byte()? == 0xe9 {
                    Operation::Fucompp
                } else {
                    return Err(self.unsupported(opcode));
                }
            }
            (OpcodeMap::Primary, 0xdd) => {
                if prefixes != Prefixes::default() {
                    return Err(self.unsupported(opcode));
                }
                let modrm = self.byte()?;
                if modrm == 0x00 {
                    Operation::FldM64FpRax
                } else if modrm == 0x38 {
                    Operation::FnstswM16Rax
                } else if modrm == 0x10 {
                    Operation::FstM64FpRax
                } else if modrm == 0x18 {
                    Operation::FstpM64FpRax
                } else if modrm == 0xe1 {
                    Operation::Fucom
                } else if modrm == 0xe9 {
                    Operation::Fucomp
                } else if (0xd0..=0xd7).contains(&modrm) {
                    Operation::FstSti {
                        destination: modrm & 7,
                    }
                } else if (0xd8..=0xdf).contains(&modrm) {
                    Operation::FstpSti {
                        destination: modrm & 7,
                    }
                } else {
                    return Err(self.unsupported(opcode));
                }
            }
            (OpcodeMap::Primary, 0xd9) => {
                if prefixes != Prefixes::default() {
                    return Err(self.unsupported(opcode));
                }
                match self.byte()? {
                    0x00 => Operation::FldM32FpRax,
                    0x28 => Operation::FldcwM16Rax,
                    0x38 => Operation::FnstcwM16Rax,
                    0x10 => Operation::FstM32FpRax,
                    0x18 => Operation::FstpM32FpRax,
                    0xe0 => Operation::Fchs,
                    0xe1 => Operation::Fabs,
                    0xe4 => Operation::Ftst,
                    0xfc => Operation::Frndint,
                    _ => return Err(self.unsupported(opcode)),
                }
            }
            (OpcodeMap::Escape0f, 0x00) => {
                if prefixes != Prefixes::default() || self.byte()? != 0xe8 {
                    return Err(self.unsupported(opcode));
                }
                Operation::VerwGdt
            }
            (OpcodeMap::Escape0f, 0xae) => {
                if prefixes != Prefixes::default() {
                    return Err(self.unsupported(opcode));
                }
                if self.byte()? != 0xf0 {
                    return Err(self.unsupported(opcode));
                }
                Operation::Mfence
            }
            (OpcodeMap::Escape0f, 0xc1) => {
                if size != OperandSize::Qword
                    || !prefixes.rex.is_some_and(|rex| rex & 8 != 0)
                    || prefixes.group1.is_some_and(|prefix| prefix != 0xf0)
                    || prefixes.segment.is_some()
                    || prefixes.operand_override
                    || prefixes.address_override
                {
                    return Err(self.unsupported(opcode));
                }
                let modrm = self.byte()?;
                let source = view(((modrm >> 3) & 7) | ((rex & 4) << 1), size, true);
                match self.rm_operand(modrm, prefixes, size)? {
                    Operand::Register(destination) if prefixes.group1.is_none() => {
                        Operation::Xadd {
                            destination,
                            source,
                        }
                    }
                    Operand::Memory(destination)
                        if size == OperandSize::Qword && destination.size == OperandSize::Qword =>
                    {
                        Operation::XaddMemory {
                            destination,
                            source,
                        }
                    }
                    _ => return Err(self.unsupported(opcode)),
                }
            }
            (OpcodeMap::Escape0f, 0xb1) => {
                if size != OperandSize::Qword
                    || !prefixes.rex.is_some_and(|rex| rex & 8 != 0)
                    || prefixes.group1.is_some()
                    || prefixes.segment.is_some()
                    || prefixes.operand_override
                    || prefixes.address_override
                {
                    return Err(self.unsupported(opcode));
                }
                let modrm = self.byte()?;
                let source = view(((modrm >> 3) & 7) | ((rex & 4) << 1), size, true);
                match self.rm_operand(modrm, prefixes, size)? {
                    Operand::Register(destination) => Operation::Cmpxchg {
                        destination,
                        source,
                    },
                    Operand::Memory(destination) if destination.size == OperandSize::Qword => {
                        Operation::CmpxchgMemory {
                            destination,
                            source,
                        }
                    }
                    _ => return Err(self.unsupported(opcode)),
                }
            }
            (OpcodeMap::Primary, 0xa4) => {
                size = OperandSize::Byte;
                Operation::Movs { size }
            }
            (OpcodeMap::Primary, 0xa5) => Operation::Movs { size },
            (OpcodeMap::Primary, 0xaa) => {
                size = OperandSize::Byte;
                if rep_stosb {
                    Operation::RepStos { size }
                } else {
                    Operation::Stos { size }
                }
            }
            (OpcodeMap::Primary, 0x9c) => {
                // In 64-bit mode REX.W selects qword and overrides 66H;
                // without REX.W, 66H selects the word form.
                size = flags_stack_size(prefixes).ok_or_else(|| self.unsupported(opcode))?;
                Operation::PushFlags
            }
            (OpcodeMap::Primary, 0x9d) => {
                size = flags_stack_size(prefixes).ok_or_else(|| self.unsupported(opcode))?;
                Operation::PopFlags
            }
            (OpcodeMap::Primary, 0x8f) => {
                let modrm = self.byte()?;
                if (modrm >> 3) & 7 != 0
                    || modrm >> 6 == 3
                    || matches!(prefixes.segment, Some(0x64 | 0x65))
                {
                    return Err(self.unsupported(opcode));
                }
                size = if rex & 8 != 0 || !prefixes.operand_override {
                    OperandSize::Qword
                } else {
                    OperandSize::Word
                };
                let Operand::Memory(destination) = self.rm_operand(modrm, prefixes, size)? else {
                    unreachable!("register-mode POP r/m is rejected above")
                };
                Operation::PopMemory { destination }
            }
            (OpcodeMap::Primary, byte @ 0x50..=0x5f) => {
                // In long mode these opcode-register forms default to an
                // eight-byte stack slot. 66 selects the supported word form;
                // REX.W takes precedence and REX.B extends the opcode register.
                if prefixes.segment.is_some() || prefixes.address_override {
                    return Err(self.unsupported(opcode));
                }
                size = match prefixes.operand_size() {
                    OperandSize::Word => OperandSize::Word,
                    _ => OperandSize::Qword,
                };
                Operation::StackRegister {
                    push: byte < 0x58,
                    register: Register::from_encoding((byte & 7) | ((rex & 1) << 3)).unwrap(),
                }
            }
            (OpcodeMap::Primary, byte @ (0x68 | 0x6a)) => {
                if prefixes.segment.is_some() || prefixes.address_override {
                    return Err(self.unsupported(opcode));
                }
                size = match prefixes.operand_size() {
                    OperandSize::Word => OperandSize::Word,
                    _ => OperandSize::Qword,
                };
                let immediate = if size == OperandSize::Word {
                    if byte == 0x6a {
                        i64::from(self.byte()? as i8) as i16 as u16 as u64
                    } else {
                        self.immediate(OperandSize::Word)? as u16 as u64
                    }
                } else if byte == 0x6a {
                    i64::from(self.byte()? as i8) as u64
                } else {
                    self.immediate(OperandSize::Dword)? as u32 as i32 as i64 as u64
                };
                Operation::PushImmediate { value: immediate }
            }
            (OpcodeMap::Escape0f, 0xc8..=0xcf) => {
                // Intel SDM 093 Vol.2A defines BSWAP for r32 and r64. The
                // 16-bit result is undefined, so keep that form outside the
                // executable slice instead of assigning it invented semantics.
                if size == OperandSize::Word {
                    return Err(self.unsupported(opcode));
                }
                let encoding = (opcode.byte & 7) | ((rex & 1) << 3);
                Operation::Bswap {
                    dst: view(encoding, size, prefixes.rex.is_some()),
                }
            }
            (OpcodeMap::Escape0f, byte @ (0xbc | 0xbd)) => {
                let modrm = self.byte()?;
                let dst_encoding = ((modrm >> 3) & 7) | ((rex & 4) << 1);
                let src = self.rm_operand(modrm, prefixes, size)?;
                Operation::BitScan {
                    kind: if byte == 0xbc {
                        BitScanKind::Forward
                    } else {
                        BitScanKind::Reverse
                    },
                    dst: RegisterView::low(Register::from_encoding(dst_encoding).unwrap(), size),
                    src,
                }
            }
            (OpcodeMap::Escape0f, byte @ (0xa3 | 0xab | 0xb3 | 0xbb)) => {
                let modrm = self.byte()?;
                let offset_encoding = ((modrm >> 3) & 7) | ((rex & 4) << 1);
                let dst = self.rm_operand(modrm, prefixes, size)?;
                let offset =
                    RegisterView::low(Register::from_encoding(offset_encoding).unwrap(), size);
                Operation::Bit {
                    kind: match byte {
                        0xa3 => BitKind::Test,
                        0xab => BitKind::Set,
                        0xb3 => BitKind::Reset,
                        0xbb => BitKind::Complement,
                        _ => unreachable!(),
                    },
                    dst,
                    offset: BitOffset::Register(offset),
                }
            }
            (OpcodeMap::Escape0f, 0xba) => {
                let modrm = self.byte()?;
                if !matches!((modrm >> 3) & 7, 4..=7) {
                    return Err(self.unsupported(opcode));
                }
                let dst = self.rm_operand(modrm, prefixes, size)?;
                let offset = self.byte()?;
                Operation::Bit {
                    kind: match (modrm >> 3) & 7 {
                        4 => BitKind::Test,
                        5 => BitKind::Set,
                        6 => BitKind::Reset,
                        7 => BitKind::Complement,
                        _ => unreachable!(),
                    },
                    dst,
                    offset: BitOffset::Immediate(offset),
                }
            }
            (OpcodeMap::Primary, 0x98) => Operation::SignExtendAccumulator {
                double_width: false,
            },
            (OpcodeMap::Primary, 0x99) => Operation::SignExtendAccumulator { double_width: true },
            (OpcodeMap::Primary, byte)
                if direct_arithmetic(byte).is_some() || direct_logic(byte).is_some() =>
            {
                if byte & 1 == 0 {
                    size = OperandSize::Byte;
                }
                if byte & 7 <= 3 {
                    let modrm = self.byte()?;
                    let reg = Operand::Register(view(
                        ((modrm >> 3) & 7) | ((rex & 4) << 1),
                        size,
                        prefixes.rex.is_some(),
                    ));
                    let rm = self.rm_operand(modrm, prefixes, size)?;
                    let (dst, src) = if byte & 2 == 0 { (rm, reg) } else { (reg, rm) };
                    binary_operation(byte >> 3, dst, src)
                } else {
                    let imm_size = if size == OperandSize::Qword {
                        OperandSize::Dword
                    } else {
                        size
                    };
                    let immediate = self.immediate(imm_size)?;
                    let immediate = if size == OperandSize::Qword {
                        immediate as u32 as i32 as i64 as u64
                    } else {
                        immediate
                    };
                    binary_operation(
                        byte >> 3,
                        Operand::Register(RegisterView::low(Register::Rax, size)),
                        Operand::Immediate(immediate),
                    )
                }
            }
            (OpcodeMap::Primary, byte @ (0x80 | 0x81 | 0x83)) => {
                if byte == 0x80 {
                    size = OperandSize::Byte;
                }
                let modrm = self.byte()?;
                // REX.R does not extend a group opcode selector. All eight
                // scalar arithmetic/logical group1 selectors are now supported.
                let group = (modrm >> 3) & 7;
                let dst = self.rm_operand(modrm, prefixes, size)?;
                let imm_size = if byte == 0x83 {
                    OperandSize::Byte
                } else if size == OperandSize::Qword {
                    OperandSize::Dword
                } else {
                    size
                };
                let immediate = self.immediate(imm_size)?;
                let immediate = if byte == 0x83 {
                    immediate as u8 as i8 as i64 as u64
                } else if size == OperandSize::Qword {
                    immediate as u32 as i32 as i64 as u64
                } else {
                    immediate
                };
                binary_operation(group, dst, Operand::Immediate(immediate))
            }
            (OpcodeMap::Primary, byte @ (0xc0 | 0xc1 | 0xd0 | 0xd1 | 0xd2 | 0xd3)) => {
                if byte == 0xc0 || byte == 0xd0 || byte == 0xd2 {
                    size = OperandSize::Byte;
                }
                let modrm = self.byte()?;
                let group = (modrm >> 3) & 7;
                let kind = match group {
                    0 => ShiftKind::Rol,
                    1 => ShiftKind::Ror,
                    2 => ShiftKind::Rcl,
                    3 => ShiftKind::Rcr,
                    4 | 6 => ShiftKind::Shl,
                    5 => ShiftKind::Shr,
                    7 => ShiftKind::Sar,
                    _ => unreachable!(),
                };
                let dst = self.rm_operand(modrm, prefixes, size)?;
                let count = match byte {
                    0xc0 | 0xc1 => Operand::Immediate(u64::from(self.byte()?)),
                    0xd0 | 0xd1 => Operand::Immediate(1),
                    _ => Operand::Register(RegisterView::low(Register::Rcx, OperandSize::Byte)),
                };
                Operation::Shift {
                    kind,
                    dst,
                    source: None,
                    count,
                }
            }
            (OpcodeMap::Primary, byte @ (0x84 | 0x85 | 0xa8 | 0xa9)) => {
                if byte & 1 == 0 {
                    size = OperandSize::Byte;
                }
                if byte == 0x84 || byte == 0x85 {
                    let modrm = self.byte()?;
                    let src = Operand::Register(view(
                        ((modrm >> 3) & 7) | ((rex & 4) << 1),
                        size,
                        prefixes.rex.is_some(),
                    ));
                    let dst = self.rm_operand(modrm, prefixes, size)?;
                    Operation::Logic {
                        kind: LogicKind::Test,
                        dst,
                        src,
                    }
                } else {
                    let imm_size = if size == OperandSize::Qword {
                        OperandSize::Dword
                    } else {
                        size
                    };
                    let immediate = self.immediate(imm_size)?;
                    let immediate = if size == OperandSize::Qword {
                        immediate as u32 as i32 as i64 as u64
                    } else {
                        immediate
                    };
                    Operation::Logic {
                        kind: LogicKind::Test,
                        dst: Operand::Register(RegisterView::low(Register::Rax, size)),
                        src: Operand::Immediate(immediate),
                    }
                }
            }
            (OpcodeMap::Primary, byte @ (0xfe | 0xff | 0xf6 | 0xf7)) => {
                if byte == 0xfe || byte == 0xf6 {
                    size = OperandSize::Byte;
                }
                let modrm = self.byte()?;
                let group = (modrm >> 3) & 7;
                if matches!(byte, 0xf6 | 0xf7) && matches!(group, 0 | 2) {
                    let dst = self.rm_operand(modrm, prefixes, size)?;
                    if group == 0 {
                        let imm_size = if size == OperandSize::Qword {
                            OperandSize::Dword
                        } else {
                            size
                        };
                        let immediate = self.immediate(imm_size)?;
                        let immediate = if size == OperandSize::Qword {
                            immediate as u32 as i32 as i64 as u64
                        } else {
                            immediate
                        };
                        Operation::Logic {
                            kind: LogicKind::Test,
                            dst,
                            src: Operand::Immediate(immediate),
                        }
                    } else {
                        Operation::Logic {
                            kind: LogicKind::Not,
                            dst,
                            src: Operand::Immediate(0),
                        }
                    }
                } else if matches!(byte, 0xf6 | 0xf7) && (4..=7).contains(&group) {
                    let operand = self.rm_operand(modrm, prefixes, size)?;
                    match group {
                        4 | 5 => Operation::Mul {
                            signed: group == 5,
                            operand,
                        },
                        6 | 7 => Operation::Divide {
                            signed: group == 7,
                            divisor: operand,
                        },
                        _ => unreachable!(),
                    }
                } else if byte == 0xff && group == 2 {
                    // Near CALL uses r/m64 in long mode. Register operands keep
                    // the narrow S2-17d prefix profile. Memory operands reuse
                    // the ordinary EA profile also used by FF /4.
                    let register_form = modrm >> 6 == 3;
                    let bad_common_prefix = prefixes.group1.is_some()
                        || matches!(prefixes.segment, Some(0x64 | 0x65))
                        || prefixes.operand_override;
                    let bad_register_prefix = register_form
                        && (prefixes.segment.is_some()
                            || prefixes.address_override
                            || prefixes.rex.is_some_and(|value| value != 0x41));
                    if bad_common_prefix || bad_register_prefix {
                        return Err(self.unsupported(opcode));
                    }
                    size = OperandSize::Qword;
                    Operation::IndirectCall {
                        operand: self.rm_operand(modrm, prefixes, OperandSize::Qword)?,
                    }
                } else if byte == 0xff && group == 6 {
                    // Memory PUSH reads its complete source before changing
                    // RSP. Only qword default and explicit word operands use
                    // the ordinary flat EA profile in this slice.
                    if modrm >> 6 == 3
                        || prefixes.group1.is_some()
                        || matches!(prefixes.segment, Some(0x64 | 0x65))
                    {
                        return Err(self.unsupported(opcode));
                    }
                    size = if rex & 8 != 0 || !prefixes.operand_override {
                        OperandSize::Qword
                    } else {
                        OperandSize::Word
                    };
                    let Operand::Memory(source) = self.rm_operand(modrm, prefixes, size)? else {
                        unreachable!("register-mode PUSH r/m is rejected above")
                    };
                    Operation::PushMemory { source }
                } else if byte == 0xff && group == 4 {
                    // In long mode near indirect JMP uses a 64-bit operand.
                    // The memory case uses only the existing ordinary flat EA
                    // profile; the other FF group extensions stay out.
                    size = OperandSize::Qword;
                    if prefixes.group1.is_some()
                        || matches!(prefixes.segment, Some(0x64 | 0x65))
                        || prefixes.operand_override
                        || (modrm >> 6 == 3
                            && (prefixes.segment.is_some() || prefixes.address_override))
                    {
                        return Err(self.unsupported(opcode));
                    }
                    Operation::IndirectJump {
                        operand: self.rm_operand(modrm, prefixes, OperandSize::Qword)?,
                    }
                } else {
                    let kind = match (byte, group) {
                        (0xfe | 0xff, 0) => ArithmeticKind::Inc,
                        (0xfe | 0xff, 1) => ArithmeticKind::Dec,
                        (0xf6 | 0xf7, 3) => ArithmeticKind::Neg,
                        _ => return Err(self.unsupported(opcode)),
                    };
                    let dst = self.rm_operand(modrm, prefixes, size)?;
                    let implied = if kind == ArithmeticKind::Neg { 0 } else { 1 };
                    Operation::Arithmetic {
                        kind,
                        dst,
                        src: Operand::Immediate(implied),
                    }
                }
            }
            (OpcodeMap::Primary, byte @ 0xb0..=0xbf) => {
                if byte < 0xb8 {
                    size = OperandSize::Byte;
                }
                let dst = view((byte & 7) | ((rex & 1) << 3), size, prefixes.rex.is_some());
                Operation::MovImmediate {
                    dst: Operand::Register(dst),
                    value: self.immediate(size)?,
                }
            }
            (OpcodeMap::Primary, byte @ (0xc6 | 0xc7)) => {
                if byte == 0xc6 {
                    size = OperandSize::Byte;
                }
                let modrm = self.byte()?;
                // /0 is MOV. Other extensions include XABORT/XBEGIN, not an
                // assumed invalid MOV: leave these outside this slice.
                if (modrm >> 3) & 7 != 0 {
                    return Err(self.unsupported(opcode));
                }
                let dst = self.rm_operand(modrm, prefixes, size)?;
                let immediate_size = if size == OperandSize::Qword {
                    OperandSize::Dword
                } else {
                    size
                };
                let raw = self.immediate(immediate_size)?;
                let value = if size == OperandSize::Qword {
                    raw as u32 as i32 as i64 as u64
                } else {
                    raw
                };
                Operation::MovImmediate { dst, value }
            }
            (OpcodeMap::Primary, byte @ 0xa0..=0xa3) => {
                if byte & 1 == 0 {
                    size = OperandSize::Byte;
                }
                let address_size = if prefixes.address_override {
                    OperandSize::Dword
                } else {
                    OperandSize::Qword
                };
                let offset = self.immediate(address_size)?;
                // No-base addressing is modular. For moffs64 displacement holds
                // the absolute offset's two's-complement bit pattern, not disp32.
                let mem = Operand::Memory(MemoryOperand {
                    base: AddressBase::None,
                    index: None,
                    scale: 1,
                    displacement: offset as i64,
                    address_size,
                    size,
                });
                let accumulator = Operand::Register(RegisterView::low(Register::Rax, size));
                let (dst, src) = if byte & 2 == 0 {
                    (accumulator, mem)
                } else {
                    (mem, accumulator)
                };
                Operation::Mov { dst, src }
            }
            (OpcodeMap::Primary, 0x8d) => {
                let modrm = self.byte()?;
                let dst = view(
                    ((modrm >> 3) & 7) | ((rex & 4) << 1),
                    size,
                    prefixes.rex.is_some(),
                );
                let src = self.rm_operand(modrm, prefixes, size)?;
                let Operand::Memory(src) = src else {
                    return Err(DecodeError::Invalid {
                        opcode,
                        length: self.length,
                    });
                };
                Operation::Lea { dst, src }
            }
            (OpcodeMap::Primary, byte @ (0x86 | 0x87)) => {
                if byte == 0x86 {
                    size = OperandSize::Byte;
                }
                let modrm = self.byte()?;
                let left = view(
                    ((modrm >> 3) & 7) | ((rex & 4) << 1),
                    size,
                    prefixes.rex.is_some(),
                );
                let right = self.rm_operand(modrm, prefixes, size)?;
                match right {
                    Operand::Register(right) => Operation::Xchg { left, right },
                    Operand::Memory(memory)
                        if byte == 0x87
                            && size == OperandSize::Qword
                            && !prefixes.operand_override
                            && !prefixes.address_override
                            && prefixes.segment.is_none() =>
                    {
                        Operation::XchgMemory {
                            register: left,
                            memory,
                        }
                    }
                    _ => return Err(self.unsupported(opcode)),
                }
            }
            (OpcodeMap::Primary, byte @ 0x90..=0x97) => {
                let encoding = (byte & 7) | ((rex & 1) << 3);
                if encoding == 0 {
                    Operation::Nop
                } else {
                    Operation::Xchg {
                        left: RegisterView::low(Register::Rax, size),
                        right: view(encoding, size, prefixes.rex.is_some()),
                    }
                }
            }
            (OpcodeMap::Primary, 0x63) | (OpcodeMap::Escape0f, 0xb6 | 0xb7 | 0xbe | 0xbf) => {
                let modrm = self.byte()?;
                let source_size = if opcode.map == OpcodeMap::Primary {
                    if size == OperandSize::Word {
                        OperandSize::Word
                    } else {
                        OperandSize::Dword
                    }
                } else if opcode.byte & 1 == 0 {
                    OperandSize::Byte
                } else {
                    OperandSize::Word
                };
                // The SDM lists 16->32/64, not 16->16, for B7/BF. MOVSXD
                // explicitly does list non-REX.W 16->16 and 32->32 encodings.
                if opcode.map == OpcodeMap::Escape0f
                    && source_size == OperandSize::Word
                    && size == OperandSize::Word
                {
                    return Err(self.unsupported(opcode));
                }
                let dst = view(
                    ((modrm >> 3) & 7) | ((rex & 4) << 1),
                    size,
                    prefixes.rex.is_some(),
                );
                let src = self.rm_operand(modrm, prefixes, source_size)?;
                Operation::Extend {
                    dst,
                    src,
                    signed: opcode.map == OpcodeMap::Primary || opcode.byte >= 0xbe,
                }
            }
            (OpcodeMap::Primary, byte @ 0x88..=0x8b) => {
                if byte & 1 == 0 {
                    size = OperandSize::Byte;
                }
                let modrm = self.byte()?;
                let reg = view(
                    ((modrm >> 3) & 7) | ((rex & 4) << 1),
                    size,
                    prefixes.rex.is_some(),
                );
                let rm = self.rm_operand(modrm, prefixes, size)?;
                let reg = Operand::Register(reg);
                let (dst, src) = if byte & 2 == 0 { (rm, reg) } else { (reg, rm) };
                if matches!(prefixes.segment, Some(0x64 | 0x65)) {
                    if byte != 0x8b
                        || self.length != 4
                        || size != OperandSize::Qword
                        || prefixes.rex != Some(0x48)
                        || prefixes.group1.is_some()
                        || prefixes.operand_override
                        || prefixes.address_override
                    {
                        return Err(self.unsupported(opcode));
                    }
                    let (Operand::Register(dst), Operand::Memory(src)) = (dst, src) else {
                        return Err(self.unsupported(opcode));
                    };
                    if dst != RegisterView::low(Register::Rax, OperandSize::Qword)
                        || src.base != AddressBase::Register(Register::Rax)
                        || src.index.is_some()
                        || src.scale != 1
                        || src.displacement != 0
                        || src.address_size != OperandSize::Qword
                    {
                        return Err(self.unsupported(opcode));
                    }
                    if prefixes.segment == Some(0x64) {
                        Operation::MovFsLoad { dst, src }
                    } else {
                        Operation::MovGsLoad { dst, src }
                    }
                } else {
                    Operation::Mov { dst, src }
                }
            }
            (OpcodeMap::Primary, 0x8c) => {
                let modrm = self.byte()?;
                if self.length != 3
                    || prefixes.rex != Some(0x48)
                    || prefixes.group1.is_some()
                    || prefixes.segment.is_some()
                    || prefixes.operand_override
                    || prefixes.address_override
                    || !matches!(modrm, 0xe0 | 0xe8)
                {
                    return Err(self.unsupported(opcode));
                }
                if modrm == 0xe0 {
                    Operation::MovFsSelectorToRax
                } else {
                    Operation::MovGsSelectorToRax
                }
            }
            (OpcodeMap::Primary, 0x8e) => {
                let modrm = self.byte()?;
                if self.length != 2
                    || prefixes != Prefixes::default()
                    || !matches!(modrm, 0xe0 | 0xe8)
                {
                    return Err(self.unsupported(opcode));
                }
                if modrm == 0xe0 {
                    Operation::LoadFsSelector
                } else {
                    Operation::LoadGsSelector
                }
            }
            (OpcodeMap::Primary, byte @ 0x70..=0x7f) => {
                size = OperandSize::Qword;
                Operation::Branch {
                    condition: ConditionCode::from_encoding(byte & 0x0f).unwrap(),
                    displacement: i32::from(self.byte()? as i8),
                }
            }
            (OpcodeMap::Escape0f, byte @ 0x80..=0x8f) => {
                size = OperandSize::Qword;
                Operation::Branch {
                    condition: ConditionCode::from_encoding(byte & 0x0f).unwrap(),
                    displacement: self.immediate(OperandSize::Dword)? as i32,
                }
            }
            (OpcodeMap::Escape0f, byte @ 0x90..=0x9f) => {
                size = OperandSize::Byte;
                let modrm = self.byte()?;
                Operation::SetCondition {
                    condition: ConditionCode::from_encoding(byte & 0x0f).unwrap(),
                    dst: self.rm_operand(modrm, prefixes, OperandSize::Byte)?,
                }
            }
            (OpcodeMap::Escape0f, byte @ 0x40..=0x4f) => {
                let modrm = self.byte()?;
                let dst = view(
                    ((modrm >> 3) & 7) | ((rex & 4) << 1),
                    size,
                    prefixes.rex.is_some(),
                );
                Operation::ConditionalMove {
                    condition: ConditionCode::from_encoding(byte & 0x0f).unwrap(),
                    dst,
                    src: self.rm_operand(modrm, prefixes, size)?,
                }
            }
            (OpcodeMap::Primary, 0xeb) => {
                size = OperandSize::Qword;
                Operation::Jump {
                    displacement: i32::from(self.byte()? as i8),
                }
            }
            (OpcodeMap::Primary, 0xe9) => {
                size = OperandSize::Qword;
                Operation::Jump {
                    displacement: self.immediate(OperandSize::Dword)? as i32,
                }
            }
            (OpcodeMap::Primary, 0xe8) => {
                if prefixes != Prefixes::default() {
                    return Err(self.unsupported(opcode));
                }
                size = OperandSize::Qword;
                Operation::Call {
                    displacement: self.immediate(OperandSize::Dword)? as i32,
                }
            }
            (OpcodeMap::Primary, 0xc3) => {
                // In 64-bit mode a near RET always uses a 64-bit target and
                // stack pop. Intel SDM 093 Vol.1 §6.4.7 specifies that the
                // operand-size prefix is ignored for near branches.
                if prefixes != Prefixes::default()
                    && prefixes
                        != (Prefixes {
                            operand_override: true,
                            ..Prefixes::default()
                        })
                {
                    return Err(self.unsupported(opcode));
                }
                size = OperandSize::Qword;
                Operation::Return { stack_adjust: 0 }
            }
            (OpcodeMap::Primary, 0xc2) => {
                if prefixes != Prefixes::default() {
                    return Err(self.unsupported(opcode));
                }
                size = OperandSize::Qword;
                Operation::Return {
                    stack_adjust: self.immediate(OperandSize::Word)? as u16,
                }
            }
            (OpcodeMap::Escape0f, byte @ (0xa4 | 0xa5 | 0xac | 0xad)) => {
                let modrm = self.byte()?;
                let source = Operand::Register(view(
                    ((modrm >> 3) & 7) | ((rex & 4) << 1),
                    size,
                    prefixes.rex.is_some(),
                ));
                let dst = self.rm_operand(modrm, prefixes, size)?;
                let kind = if byte == 0xa4 || byte == 0xa5 {
                    ShiftKind::Shld
                } else {
                    ShiftKind::Shrd
                };
                let count = if byte == 0xa4 || byte == 0xac {
                    Operand::Immediate(u64::from(self.byte()?))
                } else {
                    Operand::Register(RegisterView::low(Register::Rcx, OperandSize::Byte))
                };
                Operation::Shift {
                    kind,
                    dst,
                    source: Some(source),
                    count,
                }
            }
            (OpcodeMap::Escape0f, 0xaf) => {
                let modrm = self.byte()?;
                let dst = view(
                    ((modrm >> 3) & 7) | ((rex & 4) << 1),
                    size,
                    prefixes.rex.is_some(),
                );
                let src = self.rm_operand(modrm, prefixes, size)?;
                Operation::Imul { dst, src }
            }
            (OpcodeMap::Primary, byte @ (0x69 | 0x6b)) => {
                let modrm = self.byte()?;
                let dst = view(
                    ((modrm >> 3) & 7) | ((rex & 4) << 1),
                    size,
                    prefixes.rex.is_some(),
                );
                let src = self.rm_operand(modrm, prefixes, size)?;
                let immediate_size = if byte == 0x6b {
                    OperandSize::Byte
                } else if size == OperandSize::Qword {
                    OperandSize::Dword
                } else {
                    size
                };
                let immediate = self.immediate(immediate_size)?;
                let immediate = match immediate_size {
                    OperandSize::Byte => immediate as u8 as i8 as i64 as u64,
                    OperandSize::Word => immediate as u16 as i16 as i64 as u64,
                    OperandSize::Dword if size == OperandSize::Qword => {
                        immediate as u32 as i32 as i64 as u64
                    }
                    _ => immediate,
                };
                Operation::ImulImmediate {
                    dst,
                    src,
                    immediate,
                }
            }
            _ => unreachable!("known forms handled above"),
        };
        // FS/GS bases are modeled only by the exact S2-22a/22b MOV loads above.
        // LEA ignores segment bases and evaluates only its effective offset.
        if matches!(prefixes.segment, Some(0x64 | 0x65))
            && match operation {
                Operation::MovImmediate { dst, .. } => matches!(dst, Operand::Memory(_)),
                Operation::Mov { dst, src } => {
                    matches!(dst, Operand::Memory(_)) || matches!(src, Operand::Memory(_))
                }
                Operation::MovFsLoad { .. } => false,
                Operation::MovGsLoad { .. } => false,
                Operation::MovFsSelectorToRax => false,
                Operation::MovGsSelectorToRax => false,
                Operation::LoadFsSelector => false,
                Operation::LoadGsSelector => false,
                Operation::Extend { src, .. } => {
                    matches!(src, Operand::Memory(_))
                }
                Operation::Arithmetic { dst, src, .. } | Operation::Logic { dst, src, .. } => {
                    matches!(dst, Operand::Memory(_)) || matches!(src, Operand::Memory(_))
                }
                Operation::Mul { operand, .. } => matches!(operand, Operand::Memory(_)),
                Operation::Imul { src, .. } | Operation::ImulImmediate { src, .. } => {
                    matches!(src, Operand::Memory(_))
                }
                Operation::Divide { divisor, .. } => matches!(divisor, Operand::Memory(_)),
                Operation::Shift { dst, source, .. } => {
                    matches!(dst, Operand::Memory(_))
                        || source.is_some_and(|source| matches!(source, Operand::Memory(_)))
                }
                Operation::Bit { dst, .. } => matches!(dst, Operand::Memory(_)),
                Operation::BitScan { src, .. } => matches!(src, Operand::Memory(_)),
                Operation::SetCondition { dst, .. } => matches!(dst, Operand::Memory(_)),
                Operation::ConditionalMove { src, .. } => matches!(src, Operand::Memory(_)),
                _ => false,
            }
        {
            return Err(self.unsupported(opcode));
        }
        Ok(Instruction {
            length: self.length,
            opcode,
            prefixes,
            operand_size: size,
            address_size: if prefixes.address_override {
                OperandSize::Dword
            } else {
                OperandSize::Qword
            },
            operation,
        })
    }

    fn unsupported(&self, opcode: Opcode) -> DecodeError {
        DecodeError::Unsupported {
            opcode,
            length: self.length,
        }
    }
}

fn view(encoding: u8, size: OperandSize, rex: bool) -> RegisterView {
    if size == OperandSize::Byte {
        RegisterView::decode_byte(encoding, rex).unwrap()
    } else {
        RegisterView::low(Register::from_encoding(encoding).unwrap(), size)
    }
}

pub fn decode(
    memory: &mut impl GuestMemory,
    rip: u64,
    context: AccessContext,
) -> Result<Instruction, DecodeError> {
    Fetch {
        memory,
        start: rip,
        length: 0,
        context,
    }
    .decode()
}

pub(crate) fn canonical(address: u64) -> bool {
    (address as i64) >> 47 == 0 || (address as i64) >> 47 == -1
}

pub(crate) fn general_protection() -> MemoryError {
    MemoryError::Architectural(ArchitecturalFault {
        vector: 13,
        error_code: Some(0),
        linear_address: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::AccessKind;

    struct Memory {
        base: u64,
        bytes: Vec<u8>,
        fetched: Vec<u64>,
        fault_at: Option<usize>,
    }
    impl Memory {
        fn new(bytes: &[u8]) -> Self {
            Self {
                base: 0xfff,
                bytes: bytes.to_vec(),
                fetched: vec![],
                fault_at: None,
            }
        }
        fn decode(&mut self) -> Result<Instruction, DecodeError> {
            decode(self, self.base, AccessContext::default())
        }
    }
    impl GuestMemory for Memory {
        fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
            self.fetched.push(address);
            let offset = address.wrapping_sub(self.base) as usize;
            if self.fault_at == Some(offset) {
                return Err(MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(20),
                    linear_address: Some(address),
                }));
            }
            self.bytes
                .get(offset)
                .copied()
                .ok_or(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Fetch,
                })
        }
        fn read(&mut self, _: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
            panic!("decoder read data")
        }
        fn write(&mut self, _: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
            panic!("decoder wrote data")
        }
        fn atomic_exchange_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: OperandSize,
            _: u64,
        ) -> Result<u64, MemoryError> {
            panic!("decoder performed atomic data exchange")
        }
        fn atomic_fetch_add_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: OperandSize,
            _: u64,
        ) -> Result<u64, MemoryError> {
            panic!("decoder performed atomic data add")
        }

        fn atomic_compare_exchange_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: OperandSize,
            _: u64,
            _: u64,
        ) -> Result<u64, MemoryError> {
            panic!("decoder performed atomic data compare-exchange")
        }

        fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
            panic!("decoder performed a memory fence")
        }
    }

    #[test]
    fn fninit_accepts_only_bare_db_e3() {
        let mut memory = Memory::new(&[0xdb, 0xe3]);
        let instruction = memory.decode().unwrap();
        assert_eq!(instruction.operation, Operation::Fninit);
        assert_eq!(instruction.length, 2);
        assert_eq!(instruction.prefixes, Prefixes::default());
        assert_eq!(memory.fetched, [0xfff, 0x1000]);

        let mut fnclex_memory = Memory::new(&[0xdb, 0xe2]);
        let fnclex = fnclex_memory.decode().unwrap();
        assert_eq!(fnclex.operation, Operation::Fnclex);
        assert_eq!(fnclex.length, 2);
        assert_eq!(fnclex.prefixes, Prefixes::default());
        assert_eq!(fnclex_memory.fetched, [0xfff, 0x1000]);

        let mut fstp_m80_memory = Memory::new(&[0xdb, 0x38]);
        let fstp_m80 = fstp_m80_memory.decode().unwrap();
        assert_eq!(fstp_m80.operation, Operation::FstpM80FpRax);
        assert_eq!(fstp_m80.length, 2);
        assert_eq!(fstp_m80.prefixes, Prefixes::default());
        assert_eq!(fstp_m80_memory.fetched, [0xfff, 0x1000]);

        let fld_m80 = Memory::new(&[0xdb, 0x28]).decode().unwrap();
        assert_eq!(fld_m80.operation, Operation::FldM80FpRax);
        assert_eq!(fld_m80.length, 2);
        assert_eq!(fld_m80.prefixes, Prefixes::default());

        let mut fcomi_memory = Memory::new(&[0xdb, 0xf1]);
        let fcomi = fcomi_memory.decode().unwrap();
        assert_eq!(fcomi.operation, Operation::Fcomi { source: 1 });
        assert_eq!(fcomi.length, 2);
        assert_eq!(fcomi.prefixes, Prefixes::default());
        assert_eq!(
            fcomi.flag_effects(),
            FlagEffects {
                defined: 0x8d5,
                undefined: 0
            }
        );

        let mut fist_memory = Memory::new(&[0xdb, 0x10]);
        let fist = fist_memory.decode().unwrap();
        assert_eq!(fist.operation, Operation::FistM32IntRax);
        assert_eq!(fist.length, 2);
        assert_eq!(fist.prefixes, Prefixes::default());

        let fistp_word = Memory::new(&[0xdf, 0x18]).decode().unwrap();
        assert_eq!(fistp_word.operation, Operation::FistpM16IntRax);
        assert_eq!(fistp_word.length, 2);
        assert_eq!(fistp_word.prefixes, Prefixes::default());

        let fist_word = Memory::new(&[0xdf, 0x10]).decode().unwrap();
        assert_eq!(fist_word.operation, Operation::FistM16IntRax);
        assert_eq!(fist_word.length, 2);
        assert_eq!(fist_word.prefixes, Prefixes::default());

        let fistp_qword = Memory::new(&[0xdf, 0x38]).decode().unwrap();
        assert_eq!(fistp_qword.operation, Operation::FistpM64IntRax);
        assert_eq!(fistp_qword.length, 2);
        assert_eq!(fistp_qword.prefixes, Prefixes::default());

        for second in 0u8..=u8::MAX {
            let decoded = Memory::new(&[0xdb, second]).decode();
            assert_eq!(
                decoded.map(|instruction| instruction.operation),
                match second {
                    0x00 => Ok(Operation::FildM32IntRax),
                    0x28 => Ok(Operation::FldM80FpRax),
                    0x38 => Ok(Operation::FstpM80FpRax),
                    0xe2 => Ok(Operation::Fnclex),
                    0xe3 => Ok(Operation::Fninit),
                    0x10 => Ok(Operation::FistM32IntRax),
                    0x18 => Ok(Operation::FistpM32IntRax),
                    _ if indexed_x87_compare(0xdb, second).is_some() => {
                        Ok(indexed_x87_compare(0xdb, second).unwrap())
                    }
                    _ => Err(DecodeError::Unsupported {
                        opcode: Opcode {
                            map: OpcodeMap::Primary,
                            byte: 0xdb
                        },
                        length: 2
                    }),
                },
                "DB {second:02x}"
            );
        }

        for prefix in [
            0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
        ]
        .into_iter()
        .chain(0x40..=0x4f)
        {
            for second in [0x00, 0x10, 0x18, 0x38, 0xe2, 0xe3, 0xe9, 0xf1] {
                assert!(matches!(
                    Memory::new(&[prefix, 0xdb, second]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ));
            }
        }

        for bytes in [
            &[0xdb, 0xe0][..],
            &[0xf0, 0xdb, 0xe3][..],
            &[0xf2, 0xdb, 0xe3][..],
            &[0xf3, 0xdb, 0xe3][..],
            &[0x26, 0xdb, 0xe3][..],
            &[0x2e, 0xdb, 0xe3][..],
            &[0x36, 0xdb, 0xe3][..],
            &[0x3e, 0xdb, 0xe3][..],
            &[0x64, 0xdb, 0xe3][..],
            &[0x65, 0xdb, 0xe3][..],
            &[0x66, 0xdb, 0xe3][..],
            &[0x67, 0xdb, 0xe3][..],
            &[0x40, 0xdb, 0xe3][..],
            &[0x48, 0xdb, 0xe3][..],
            &[0x9b, 0xdb, 0xe3][..],
        ] {
            assert!(
                matches!(
                    Memory::new(bytes).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "{bytes:02x?}"
            );
        }
        for prefix in [
            0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
        ]
        .into_iter()
        .chain(0x40..=0x4f)
        {
            assert!(
                matches!(
                    Memory::new(&[prefix, 0xdf, 0x28]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "FILD m64 prefix {prefix:02x}"
            );
        }
        let m64 = Memory::new(&[0xdf, 0x28]).decode().unwrap();
        assert_eq!(m64.operation, Operation::FildM64IntRax);
        assert_eq!(m64.length, 2);
        assert_eq!(m64.prefixes, Prefixes::default());
        for prefix in [0xf0, 0xf2, 0xf3, 0x2e, 0x64, 0x66, 0x67, 0x40, 0x48, 0x9b] {
            let bytes = [prefix, 0xd9, 0xe4];
            assert!(
                matches!(
                    Memory::new(&bytes).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "{bytes:02x?}"
            );
        }
        for bytes in [
            &[0x9b, 0xdb, 0xe2][..],
            &[0x66, 0xdb, 0xe2][..],
            &[0x67, 0xdb, 0xe2][..],
            &[0x26, 0xdb, 0xe2][..],
            &[0xf0, 0xdb, 0xe2][..],
            &[0xf2, 0xdb, 0xe2][..],
            &[0xf3, 0xdb, 0xe2][..],
            &[0x48, 0xdb, 0xe2][..],
        ] {
            assert!(
                matches!(
                    Memory::new(bytes).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "{bytes:02x?}"
            );
        }
        for modrm in 0u8..=u8::MAX {
            let bytes = [0xdb, modrm];
            let decoded = Memory::new(&bytes).decode();
            assert_eq!(
                decoded.map(|instruction| instruction.operation),
                match modrm {
                    0x00 => Ok(Operation::FildM32IntRax),
                    0x28 => Ok(Operation::FldM80FpRax),
                    0x38 => Ok(Operation::FstpM80FpRax),
                    0x10 => Ok(Operation::FistM32IntRax),
                    0x18 => Ok(Operation::FistpM32IntRax),
                    0xe2 => Ok(Operation::Fnclex),
                    0xe3 => Ok(Operation::Fninit),
                    _ if indexed_x87_compare(0xdb, modrm).is_some() => {
                        Ok(indexed_x87_compare(0xdb, modrm).unwrap())
                    }
                    _ => Err(DecodeError::Unsupported {
                        opcode: Opcode {
                            map: OpcodeMap::Primary,
                            byte: 0xdb,
                        },
                        length: 2,
                    }),
                },
                "DB {modrm:02x}"
            );
        }
        for prefix in 0x40..=0x4f {
            assert!(
                matches!(
                    Memory::new(&[prefix, 0xdf, 0x00]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "REX {prefix:02x}"
            );
        }
        assert!(matches!(
            Memory::new(&[0xdb]).decode(),
            Err(DecodeError::Memory(MemoryError::Unmapped {
                address: 0x1000,
                access: AccessKind::Fetch,
            }))
        ));
    }

    #[test]
    fn df_admits_only_bare_fnstsw_ax_and_fcomi_pop_forms() {
        let mut memory = Memory::new(&[0xdf, 0xe0]);
        let instruction = memory.decode().unwrap();
        assert_eq!(instruction.operation, Operation::FnstswAx);
        assert_eq!(instruction.length, 2);
        assert_eq!(instruction.prefixes, Prefixes::default());
        assert_eq!(memory.fetched, [0xfff, 0x1000]);

        for modrm in 0u8..=u8::MAX {
            let decoded = Memory::new(&[0xdf, modrm]).decode();
            assert_eq!(
                decoded.map(|instruction| instruction.operation),
                match modrm {
                    0x00 => Ok(Operation::FildM16IntRax),
                    0x28 => Ok(Operation::FildM64IntRax),
                    0xe0 => Ok(Operation::FnstswAx),
                    0x38 => Ok(Operation::FistpM64IntRax),
                    0x18 => Ok(Operation::FistpM16IntRax),
                    0x10 => Ok(Operation::FistM16IntRax),
                    _ if indexed_x87_compare(0xdf, modrm).is_some() => {
                        Ok(indexed_x87_compare(0xdf, modrm).unwrap())
                    }
                    _ => Err(DecodeError::Unsupported {
                        opcode: Opcode {
                            map: OpcodeMap::Primary,
                            byte: 0xdf,
                        },
                        length: 2,
                    }),
                },
                "DF {modrm:02x}"
            );
        }
        for bytes in [
            &[0xf0, 0xdf, 0x00][..],
            &[0xf2, 0xdf, 0x00],
            &[0xf3, 0xdf, 0x00],
            &[0x26, 0xdf, 0x00],
            &[0x2e, 0xdf, 0x00],
            &[0x36, 0xdf, 0x00],
            &[0x3e, 0xdf, 0x00],
            &[0x64, 0xdf, 0x00],
            &[0x65, 0xdf, 0x00],
            &[0x66, 0xdf, 0x00],
            &[0x67, 0xdf, 0x00],
            &[0x40, 0xdf, 0x00],
            &[0x48, 0xdf, 0x00],
            &[0x9b, 0xdf, 0x00],
            &[0xf0, 0xdf, 0xe0],
            &[0xf2, 0xdf, 0xe0],
            &[0xf3, 0xdf, 0xe0],
            &[0x26, 0xdf, 0xe0],
            &[0x2e, 0xdf, 0xe0],
            &[0x36, 0xdf, 0xe0],
            &[0x3e, 0xdf, 0xe0],
            &[0x64, 0xdf, 0xe0],
            &[0x65, 0xdf, 0xe0],
            &[0x66, 0xdf, 0xe0],
            &[0x67, 0xdf, 0xe0],
            &[0x40, 0xdf, 0xe0],
            &[0x48, 0xdf, 0xe0],
            &[0x9b, 0xdf, 0xe0],
        ] {
            assert!(
                matches!(
                    Memory::new(bytes).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "{bytes:02x?}"
            );
        }
        assert!(matches!(
            Memory::new(&[0xdf]).decode(),
            Err(DecodeError::Memory(MemoryError::Unmapped {
                address: 0x1000,
                access: AccessKind::Fetch,
            }))
        ));
    }

    #[test]
    fn fcomip_accepts_only_bare_df_f1() {
        let mut memory = Memory::new(&[0xdf, 0xf1]);
        let instruction = memory.decode().unwrap();
        assert_eq!(instruction.operation, Operation::Fcomip { source: 1 });
        assert_eq!(instruction.length, 2);
        assert_eq!(instruction.prefixes, Prefixes::default());
        assert_eq!(
            instruction.flag_effects(),
            FlagEffects {
                defined: 0x8d5,
                undefined: 0
            }
        );

        for modrm in 0u8..=u8::MAX {
            let decoded = Memory::new(&[0xdf, modrm]).decode();
            assert_eq!(
                decoded.map(|instruction| instruction.operation),
                match modrm {
                    0x00 => Ok(Operation::FildM16IntRax),
                    0x28 => Ok(Operation::FildM64IntRax),
                    0xe0 => Ok(Operation::FnstswAx),
                    0x38 => Ok(Operation::FistpM64IntRax),
                    0x18 => Ok(Operation::FistpM16IntRax),
                    0x10 => Ok(Operation::FistM16IntRax),
                    _ if indexed_x87_compare(0xdf, modrm).is_some() => {
                        Ok(indexed_x87_compare(0xdf, modrm).unwrap())
                    }
                    _ => Err(DecodeError::Unsupported {
                        opcode: Opcode {
                            map: OpcodeMap::Primary,
                            byte: 0xdf,
                        },
                        length: 2,
                    }),
                },
                "DF {modrm:02x}"
            );
        }

        for prefix in [
            0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
        ]
        .into_iter()
        .chain(0x40..=0x4f)
        {
            assert!(matches!(
                Memory::new(&[prefix, 0xdf, 0xf1]).decode(),
                Err(DecodeError::Unsupported { .. })
            ));
            assert!(matches!(
                Memory::new(&[prefix, 0xdf, 0xe9]).decode(),
                Err(DecodeError::Unsupported { .. })
            ));
        }
        assert!(matches!(
            Memory::new(&[0xdf]).decode(),
            Err(DecodeError::Memory(MemoryError::Unmapped {
                address: 0x1000,
                access: AccessKind::Fetch,
            }))
        ));
    }

    #[test]
    fn fucomip_accepts_only_bare_df_e9_and_defines_fcomi_flags() {
        let instruction = Memory::new(&[0xdf, 0xe9]).decode().unwrap();
        assert_eq!(instruction.operation, Operation::Fucomip { source: 1 });
        assert_eq!(instruction.length, 2);
        assert_eq!(instruction.prefixes, Prefixes::default());
        assert_eq!(
            instruction.flag_effects(),
            FlagEffects {
                defined: 0x8d5,
                undefined: 0
            }
        );

        let instruction = Memory::new(&[0xdf, 0x18]).decode().unwrap();
        assert_eq!(instruction.operation, Operation::FistpM16IntRax);
        assert_eq!(instruction.length, 2);
        assert_eq!(instruction.prefixes, Prefixes::default());
        let qword = Memory::new(&[0xdf, 0x38]).decode().unwrap();
        assert_eq!(qword.operation, Operation::FistpM64IntRax);
        assert_eq!(qword.length, 2);
        assert_eq!(qword.prefixes, Prefixes::default());
        for prefix in [
            0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
        ]
        .into_iter()
        .chain(0x40..=0x4f)
        {
            assert!(
                matches!(
                    Memory::new(&[prefix, 0xdf, 0x38]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "prefix {prefix:02x}"
            );
        }
    }

    #[test]
    fn fcomi_family_decodes_only_the_32_bare_indexed_register_forms() {
        for opcode in [0xdb, 0xdf] {
            for modrm in 0u8..=u8::MAX {
                let decoded = Memory::new(&[opcode, modrm]).decode();
                let expected = if opcode == 0xdb && modrm == 0xe2 {
                    Ok(Operation::Fnclex)
                } else if opcode == 0xdb && modrm == 0xe3 {
                    Ok(Operation::Fninit)
                } else if opcode == 0xdb && modrm == 0x00 {
                    Ok(Operation::FildM32IntRax)
                } else if opcode == 0xdb && modrm == 0x38 {
                    Ok(Operation::FstpM80FpRax)
                } else if opcode == 0xdb && modrm == 0x28 {
                    Ok(Operation::FldM80FpRax)
                } else if opcode == 0xdf && modrm == 0x00 {
                    Ok(Operation::FildM16IntRax)
                } else if opcode == 0xdf && modrm == 0x28 {
                    Ok(Operation::FildM64IntRax)
                } else if opcode == 0xdb && modrm == 0x10 {
                    Ok(Operation::FistM32IntRax)
                } else if opcode == 0xdb && modrm == 0x18 {
                    Ok(Operation::FistpM32IntRax)
                } else if opcode == 0xdf && modrm == 0xe0 {
                    Ok(Operation::FnstswAx)
                } else if opcode == 0xdf && modrm == 0x10 {
                    Ok(Operation::FistM16IntRax)
                } else if opcode == 0xdf && modrm == 0x18 {
                    Ok(Operation::FistpM16IntRax)
                } else if opcode == 0xdf && modrm == 0x38 {
                    Ok(Operation::FistpM64IntRax)
                } else if let Some(operation) = indexed_x87_compare(opcode, modrm) {
                    Ok(operation)
                } else {
                    Err(DecodeError::Unsupported {
                        opcode: Opcode {
                            map: OpcodeMap::Primary,
                            byte: opcode,
                        },
                        length: 2,
                    })
                };
                assert_eq!(
                    decoded.map(|instruction| instruction.operation),
                    expected,
                    "{opcode:02x} {modrm:02x}"
                );
            }
        }

        let indexed: Vec<_> = [0xdb, 0xdf]
            .into_iter()
            .flat_map(|opcode| {
                (0xe8..=0xef)
                    .chain(0xf0..=0xf7)
                    .map(move |modrm| [opcode, modrm])
            })
            .collect();
        for prefix in [
            0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
        ]
        .into_iter()
        .chain(0x40..=0x4f)
        {
            for bytes in &indexed {
                assert!(
                    matches!(
                        Memory::new(&[prefix, bytes[0], bytes[1]]).decode(),
                        Err(DecodeError::Unsupported { .. })
                    ),
                    "{prefix:02x} {:02x} {:02x}",
                    bytes[0],
                    bytes[1]
                );
            }
        }
    }

    #[test]
    fn dd_accepts_only_bounded_m64fp_fst_fstp_fnstsw_fst_fstp_fucom_and_fucomp_encodings() {
        let fld = Memory::new(&[0xdd, 0x00]).decode().unwrap();
        assert_eq!(fld.operation, Operation::FldM64FpRax);
        assert_eq!(fld.length, 2);
        assert_eq!(fld.prefixes, Prefixes::default());
        let decoded = Memory::new(&[0xdd, 0x10]).decode().unwrap();
        assert_eq!(decoded.operation, Operation::FstM64FpRax);
        assert_eq!(decoded.length, 2);
        assert_eq!(decoded.prefixes, Prefixes::default());
        let decoded = Memory::new(&[0xdd, 0x18]).decode().unwrap();
        assert_eq!(decoded.operation, Operation::FstpM64FpRax);
        assert_eq!(decoded.length, 2);
        assert_eq!(decoded.prefixes, Prefixes::default());
        let mut memory = Memory::new(&[0xdd, 0x38]);
        let instruction = memory.decode().unwrap();
        assert_eq!(instruction.operation, Operation::FnstswM16Rax);
        assert_eq!(instruction.length, 2);
        assert_eq!(instruction.prefixes, Prefixes::default());
        assert_eq!(memory.fetched, [0xfff, 0x1000]);

        for index in 0u8..=7 {
            let bytes = [0xdd, 0xd0 | index];
            let mut memory = Memory::new(&bytes);
            let instruction = memory.decode().unwrap();
            assert_eq!(
                instruction.operation,
                Operation::FstSti { destination: index }
            );
            assert_eq!(instruction.length, 2);
            assert_eq!(instruction.prefixes, Prefixes::default());
            assert_eq!(memory.fetched, [0xfff, 0x1000]);

            let bytes = [0xdd, 0xd8 | index];
            let mut memory = Memory::new(&bytes);
            let instruction = memory.decode().unwrap();
            assert_eq!(
                instruction.operation,
                Operation::FstpSti { destination: index }
            );
            assert_eq!(instruction.length, 2);
            assert_eq!(instruction.prefixes, Prefixes::default());
            assert_eq!(memory.fetched, [0xfff, 0x1000]);
        }

        for modrm in 0u8..=u8::MAX {
            let decoded = Memory::new(&[0xdd, modrm]).decode();
            assert_eq!(
                decoded.map(|instruction| instruction.operation),
                if (0xd0..=0xd7).contains(&modrm) {
                    Ok(Operation::FstSti {
                        destination: modrm & 7,
                    })
                } else if (0xd8..=0xdf).contains(&modrm) {
                    Ok(Operation::FstpSti {
                        destination: modrm & 7,
                    })
                } else if modrm == 0x00 {
                    Ok(Operation::FldM64FpRax)
                } else if modrm == 0x38 {
                    Ok(Operation::FnstswM16Rax)
                } else if modrm == 0x10 {
                    Ok(Operation::FstM64FpRax)
                } else if modrm == 0x18 {
                    Ok(Operation::FstpM64FpRax)
                } else if modrm == 0xe1 {
                    Ok(Operation::Fucom)
                } else if modrm == 0xe9 {
                    Ok(Operation::Fucomp)
                } else {
                    Err(DecodeError::Unsupported {
                        opcode: Opcode {
                            map: OpcodeMap::Primary,
                            byte: 0xdd,
                        },
                        length: 2,
                    })
                },
                "DD {modrm:02x}"
            );
        }

        for bytes in [
            &[0x9b, 0xdd, 0x38][..],
            &[0x9b, 0xdd, 0x10][..],
            &[0x9b, 0xdd, 0x18][..],
            &[0xf0, 0xdd, 0x38],
            &[0xf0, 0xdd, 0x10],
            &[0xf0, 0xdd, 0x18],
            &[0x66, 0xdd, 0x38],
            &[0x66, 0xdd, 0x10],
            &[0x66, 0xdd, 0x18],
            &[0xf0, 0xdd, 0xd9][..],
            &[0xf2, 0xdd, 0xd9],
            &[0xf3, 0xdd, 0xd9],
            &[0x26, 0xdd, 0xd9],
            &[0x2e, 0xdd, 0xd9],
            &[0x36, 0xdd, 0xd9],
            &[0x3e, 0xdd, 0xd9],
            &[0x64, 0xdd, 0xd9],
            &[0x65, 0xdd, 0xd9],
            &[0x66, 0xdd, 0xd9],
            &[0x67, 0xdd, 0xd9],
            &[0x40, 0xdd, 0xd9],
            &[0x48, 0xdd, 0xd9],
            &[0x9b, 0xdd, 0xd9],
            &[0xf1, 0xdd, 0x18],
            &[0xf2, 0xdd, 0x18],
            &[0xf3, 0xdd, 0x18],
            &[0x26, 0xdd, 0x18],
            &[0x2e, 0xdd, 0x18],
            &[0x36, 0xdd, 0x18],
            &[0x3e, 0xdd, 0x18],
            &[0x64, 0xdd, 0x18],
            &[0x65, 0xdd, 0x18],
            &[0x67, 0xdd, 0x18],
            &[0x40, 0xdd, 0x18],
            &[0x48, 0xdd, 0x18],
        ] {
            assert!(
                matches!(
                    Memory::new(bytes).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "{bytes:02x?}"
            );
        }
        assert!(matches!(
            Memory::new(&[0xdd]).decode(),
            Err(DecodeError::Memory(MemoryError::Unmapped {
                address: 0x1000,
                access: AccessKind::Fetch,
            }))
        ));
        for bytes in [
            &[0xdd, 0x18][..],
            &[0xdd, 0xd8],
            &[0xdd, 0xdf],
            &[0xdd, 0xe1],
        ] {
            let mut memory = Memory::new(bytes);
            memory.fault_at = Some(1);
            assert!(matches!(
                memory.decode(),
                Err(DecodeError::Memory(MemoryError::Architectural(_)))
            ));
        }
    }

    #[test]
    fn d9_accepts_only_bare_fldcw_fst_fstp_fchs_fabs_ftst_frndint_and_fnstcw_m16_rax() {
        let mut memory = Memory::new(&[0xd9, 0xe0]);
        let instruction = memory.decode().unwrap();
        assert_eq!(instruction.operation, Operation::Fchs);
        assert_eq!(instruction.length, 2);
        assert_eq!(instruction.prefixes, Prefixes::default());
        assert_eq!(memory.fetched, [0xfff, 0x1000]);

        for second in 0u8..=u8::MAX {
            let decoded = Memory::new(&[0xd9, second]).decode();
            assert_eq!(
                decoded.map(|instruction| instruction.operation),
                if second == 0x00 {
                    Ok(Operation::FldM32FpRax)
                } else if second == 0x28 {
                    Ok(Operation::FldcwM16Rax)
                } else if second == 0x10 {
                    Ok(Operation::FstM32FpRax)
                } else if second == 0x18 {
                    Ok(Operation::FstpM32FpRax)
                } else if second == 0x38 {
                    Ok(Operation::FnstcwM16Rax)
                } else if second == 0xe0 {
                    Ok(Operation::Fchs)
                } else if second == 0xe1 {
                    Ok(Operation::Fabs)
                } else if second == 0xe4 {
                    Ok(Operation::Ftst)
                } else if second == 0xfc {
                    Ok(Operation::Frndint)
                } else {
                    Err(DecodeError::Unsupported {
                        opcode: Opcode {
                            map: OpcodeMap::Primary,
                            byte: 0xd9,
                        },
                        length: 2,
                    })
                },
                "D9 {second:02x}"
            );
        }

        for prefix in [
            0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
        ]
        .into_iter()
        .chain(0x40..=0x4f)
        {
            for second in [0x00, 0x10, 0x18, 0x28, 0x38, 0xe0, 0xe1, 0xe4, 0xfc] {
                assert!(matches!(
                    Memory::new(&[prefix, 0xd9, second]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ));
            }
        }

        assert!(matches!(
            Memory::new(&[0xd9]).decode(),
            Err(DecodeError::Memory(MemoryError::Unmapped { .. }))
        ));
        assert!(matches!(
            Memory::new(&[0xd9]).decode(),
            Err(DecodeError::Memory(MemoryError::Unmapped {
                address: 0x1000,
                access: AccessKind::Fetch,
            }))
        ));

        for bytes in [
            &[0xf0, 0xd9, 0xe0][..],
            &[0xf2, 0xd9, 0xe0],
            &[0xf3, 0xd9, 0xe0],
            &[0x2e, 0xd9, 0xe0],
            &[0x64, 0xd9, 0xe0],
            &[0x66, 0xd9, 0xe0],
            &[0x67, 0xd9, 0xe0],
            &[0x40, 0xd9, 0xe0],
            &[0x48, 0xd9, 0xe0],
            &[0x9b, 0xd9, 0xe0],
            &[0xf0, 0xd9, 0xe1],
            &[0xf2, 0xd9, 0xe1],
            &[0xf3, 0xd9, 0xe1],
            &[0x2e, 0xd9, 0xe1],
            &[0x64, 0xd9, 0xe1],
            &[0x66, 0xd9, 0xe1],
            &[0x67, 0xd9, 0xe1],
            &[0x40, 0xd9, 0xe1],
            &[0x48, 0xd9, 0xe1],
            &[0x9b, 0xd9, 0xe1],
        ] {
            assert!(
                matches!(
                    Memory::new(bytes).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "{bytes:02x?}"
            );
        }

        for prefix in [
            0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x40, 0x48, 0x9b,
        ] {
            assert!(
                matches!(
                    Memory::new(&[prefix, 0xd9, 0xfc]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "prefix {prefix:02x}"
            );
            assert!(
                matches!(
                    Memory::new(&[prefix, 0xd9, 0x38]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "FNSTCW prefix {prefix:02x}"
            );
        }
        assert!(matches!(
            Memory::new(&[0xd9]).decode(),
            Err(DecodeError::Memory(MemoryError::Unmapped {
                address: 0x1000,
                access: AccessKind::Fetch,
            }))
        ));
        let mut memory = Memory::new(&[0xd9, 0xe1]);
        memory.fault_at = Some(1);
        assert!(matches!(
            memory.decode(),
            Err(DecodeError::Memory(MemoryError::Architectural(_)))
        ));
    }

    #[test]
    fn fadd_st0_sti_accepts_only_bare_d8_c0_through_c7() {
        for source in 0u8..=7 {
            let mut memory = Memory::new(&[0xd8, 0xc0 | source]);
            let instruction = memory.decode().unwrap();
            assert_eq!(instruction.operation, Operation::FaddSt0St { source });
            assert_eq!(instruction.length, 2);
            assert_eq!(instruction.prefixes, Prefixes::default());
            assert_eq!(memory.fetched, [0xfff, 0x1000]);
        }

        for modrm in 0u8..=u8::MAX {
            let decoded = Memory::new(&[0xd8, modrm]).decode();
            assert_eq!(
                decoded.map(|instruction| instruction.operation),
                match modrm {
                    0xd0..=0xd7 => Ok(Operation::FcomSt { source: modrm & 7 }),
                    0xd8..=0xdf => Ok(Operation::FcompSt { source: modrm & 7 }),
                    0xc0..=0xc7 => Ok(Operation::FaddSt0St { source: modrm & 7 }),
                    0xc8..=0xcf => Ok(Operation::FmulSt0St { source: modrm & 7 }),
                    0xe0..=0xe7 => Ok(Operation::FsubSt0St { source: modrm & 7 }),
                    0xe8..=0xef => Ok(Operation::FsubrSt0St { source: modrm & 7 }),
                    0xf0..=0xf7 => Ok(Operation::FdivSt0St { source: modrm & 7 }),
                    0xf8..=0xff => Ok(Operation::FdivrSt0St { source: modrm & 7 }),
                    _ => Err(DecodeError::Unsupported {
                        opcode: Opcode {
                            map: OpcodeMap::Primary,
                            byte: 0xd8,
                        },
                        length: 2,
                    }),
                },
                "D8 {modrm:02x}"
            );
        }
        for bytes in [
            &[0xf0, 0xd8, 0xc1][..],
            &[0xf2, 0xd8, 0xc1][..],
            &[0xf3, 0xd8, 0xc1][..],
            &[0x26, 0xd8, 0xc1][..],
            &[0x64, 0xd8, 0xc1][..],
            &[0x66, 0xd8, 0xc1][..],
            &[0x67, 0xd8, 0xc1][..],
            &[0x40, 0xd8, 0xc1][..],
            &[0x48, 0xd8, 0xc1][..],
            &[0x9b, 0xd8, 0xc1][..],
        ] {
            assert!(
                matches!(
                    Memory::new(bytes).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "{bytes:02x?}"
            );
        }
        assert!(matches!(
            Memory::new(&[0xd8]).decode(),
            Err(DecodeError::Memory(MemoryError::Unmapped {
                address: 0x1000,
                access: AccessKind::Fetch,
            }))
        ));
        for source in 0u8..=7 {
            let instruction = [0xd8, 0xc0 | source];
            for prefix in [
                0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
            ]
            .into_iter()
            .chain(0x40..=0x4f)
            {
                assert!(matches!(
                    Memory::new(&[prefix, instruction[0], instruction[1]]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ));
            }
            assert!(matches!(
                Memory::new(&instruction[..1]).decode(),
                Err(DecodeError::Memory(MemoryError::Unmapped { .. }))
            ));
            let fmul = [0xd8, 0xc8 | source];
            assert!(matches!(
                Memory::new(&fmul).decode(),
                Ok(Instruction {
                    length: 2,
                    operation: Operation::FmulSt0St { source: decoded },
                    prefixes: Prefixes { .. },
                    ..
                }) if decoded == source
            ));
            for prefix in [
                0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
            ]
            .into_iter()
            .chain(0x40..=0x4f)
            {
                assert!(matches!(
                    Memory::new(&[prefix, 0xd8, 0xc8 | source]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ));
            }
        }
        let mut truncated_prefixed = Memory::new(&[0x66, 0xd8]);
        assert!(matches!(
            truncated_prefixed.decode(),
            Err(DecodeError::Unsupported { .. })
        ));
        assert_eq!(truncated_prefixed.fetched, [0xfff, 0x1000]);
        for modrm in 0u8..=u8::MAX {
            if (0xc0..=0xc7).contains(&modrm)
                || (0xc8..=0xcf).contains(&modrm)
                || (0xe0..=0xe7).contains(&modrm)
                || (0xe8..=0xef).contains(&modrm)
                || (0xf0..=0xf7).contains(&modrm)
                || (0xf8..=0xff).contains(&modrm)
            {
                continue;
            }
            assert!(matches!(
                Memory::new(&[0xdc, modrm]).decode(),
                Err(DecodeError::Unsupported { .. })
            ));
        }
        assert!(matches!(
            Memory::new(&[0x9b]).decode(),
            Err(DecodeError::Unsupported { .. })
        ));
    }

    #[test]
    fn fcom_sti_accepts_only_bare_d8_d0_through_d7() {
        for source in 0u8..=7 {
            let bytes = [0xd8, 0xd0 | source];
            let mut memory = Memory::new(&bytes);
            let instruction = memory.decode().unwrap();
            assert_eq!(instruction.operation, Operation::FcomSt { source });
            assert_eq!(instruction.length, 2);
            assert_eq!(instruction.prefixes, Prefixes::default());
            assert_eq!(memory.fetched, [0xfff, 0x1000]);

            for prefix in [
                0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
            ]
            .into_iter()
            .chain(0x40..=0x4f)
            {
                assert!(matches!(
                    Memory::new(&[prefix, bytes[0], bytes[1]]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ));
            }
        }
    }

    #[test]
    fn fcomp_sti_accepts_only_bare_d8_d8_through_df() {
        for source in 0u8..=7 {
            let mut memory = Memory::new(&[0xd8, 0xd8 | source]);
            let instruction = memory.decode().unwrap();
            assert_eq!(instruction.operation, Operation::FcompSt { source });
            assert_eq!(instruction.length, 2);
            assert_eq!(instruction.prefixes, Prefixes::default());
            assert_eq!(memory.fetched, [0xfff, 0x1000]);
        }
        for prefix in [
            0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
        ]
        .into_iter()
        .chain(0x40..=0x4f)
        {
            assert!(
                matches!(
                    Memory::new(&[prefix, 0xd8, 0xd8]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "prefix {prefix:02x}"
            );
        }
        assert!(matches!(
            Memory::new(&[0xd8]).decode(),
            Err(DecodeError::Memory(MemoryError::Unmapped {
                access: AccessKind::Fetch,
                ..
            }))
        ));
    }

    #[test]
    fn fsub_st0_sti_accepts_only_bare_d8_and_fsubrp_alias_bare_de() {
        for source in 0u8..=7 {
            let mut memory = Memory::new(&[0xd8, 0xe0 | source]);
            let instruction = memory.decode().unwrap();
            assert_eq!(instruction.operation, Operation::FsubSt0St { source });
            assert_eq!(instruction.length, 2);
            assert_eq!(instruction.prefixes, Prefixes::default());
            assert_eq!(memory.fetched, [0xfff, 0x1000]);
        }
        for destination in 0u8..=7 {
            let mut memory = Memory::new(&[0xde, 0xe0 | destination]);
            let instruction = memory.decode().unwrap();
            assert_eq!(
                instruction.operation,
                Operation::FsubrpStSt0 { destination }
            );
            assert_eq!(instruction.length, 2);
            assert_eq!(instruction.prefixes, Prefixes::default());
            assert_eq!(memory.fetched, [0xfff, 0x1000]);
            for prefix in [
                0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
            ]
            .into_iter()
            .chain(0x40..=0x4f)
            {
                assert!(matches!(
                    Memory::new(&[prefix, 0xde, 0xe0 | destination]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ));
            }
        }
        assert!(matches!(
            Memory::new(&[0xde]).decode(),
            Err(DecodeError::Memory(MemoryError::Unmapped { .. }))
        ));
        assert!(matches!(
            Memory::new(&[0x9b, 0xde, 0xe1]).decode(),
            Err(DecodeError::Unsupported { .. })
        ));

        for modrm in 0u8..=u8::MAX {
            let decoded = Memory::new(&[0xd8, modrm]).decode();
            assert_eq!(
                decoded.map(|instruction| instruction.operation),
                match modrm {
                    0xd0..=0xd7 => Ok(Operation::FcomSt { source: modrm & 7 }),
                    0xd8..=0xdf => Ok(Operation::FcompSt { source: modrm & 7 }),
                    0xc0..=0xc7 => Ok(Operation::FaddSt0St { source: modrm & 7 }),
                    0xc8..=0xcf => Ok(Operation::FmulSt0St { source: modrm & 7 }),
                    0xe0..=0xe7 => Ok(Operation::FsubSt0St { source: modrm & 7 }),
                    0xe8..=0xef => Ok(Operation::FsubrSt0St { source: modrm & 7 }),
                    0xf0..=0xf7 => Ok(Operation::FdivSt0St { source: modrm & 7 }),
                    0xf8..=0xff => Ok(Operation::FdivrSt0St { source: modrm & 7 }),
                    _ => Err(DecodeError::Unsupported {
                        opcode: Opcode {
                            map: OpcodeMap::Primary,
                            byte: 0xd8,
                        },
                        length: 2,
                    }),
                },
                "D8 {modrm:02x}"
            );
        }
        for source in 0u8..=7 {
            for prefix in [
                0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
            ]
            .into_iter()
            .chain(0x40..=0x4f)
            {
                assert!(matches!(
                    Memory::new(&[prefix, 0xd8, 0xe0 | source]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ));
            }
        }
        assert!(matches!(
            Memory::new(&[0xd8]).decode(),
            Err(DecodeError::Memory(MemoryError::Unmapped {
                address: 0x1000,
                access: AccessKind::Fetch,
            }))
        ));
        assert!(matches!(
            Memory::new(&[0x9b]).decode(),
            Err(DecodeError::Unsupported { .. })
        ));
        assert_eq!(
            Memory::new(&[0xdc, 0xf0]).decode().unwrap().operation,
            Operation::FdivrStSt0 { destination: 0 }
        );
        assert_eq!(
            Memory::new(&[0xde, 0xe0]).decode().unwrap().operation,
            Operation::FsubrpStSt0 { destination: 0 }
        );
    }

    #[test]
    fn fdiv_st0_sti_accepts_only_bare_d8_f0_through_f7() {
        for source in 0u8..=7 {
            let mut memory = Memory::new(&[0xd8, 0xf0 | source]);
            let instruction = memory.decode().unwrap();
            assert_eq!(instruction.operation, Operation::FdivSt0St { source });
            assert_eq!(instruction.length, 2);
            assert_eq!(instruction.prefixes, Prefixes::default());
            assert_eq!(memory.fetched, [0xfff, 0x1000]);
            for prefix in [
                0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
            ]
            .into_iter()
            .chain(0x40..=0x4f)
            {
                assert!(matches!(
                    Memory::new(&[prefix, 0xd8, 0xf0 | source]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ));
            }
        }
        for modrm in 0u8..=u8::MAX {
            let decoded = Memory::new(&[0xd8, modrm]).decode();
            assert_eq!(
                decoded.map(|instruction| instruction.operation),
                match modrm {
                    0xd0..=0xd7 => Ok(Operation::FcomSt { source: modrm & 7 }),
                    0xd8..=0xdf => Ok(Operation::FcompSt { source: modrm & 7 }),
                    0xc0..=0xc7 => Ok(Operation::FaddSt0St { source: modrm & 7 }),
                    0xc8..=0xcf => Ok(Operation::FmulSt0St { source: modrm & 7 }),
                    0xe0..=0xe7 => Ok(Operation::FsubSt0St { source: modrm & 7 }),
                    0xe8..=0xef => Ok(Operation::FsubrSt0St { source: modrm & 7 }),
                    0xf0..=0xf7 => Ok(Operation::FdivSt0St { source: modrm & 7 }),
                    0xf8..=0xff => Ok(Operation::FdivrSt0St { source: modrm & 7 }),
                    _ => Err(DecodeError::Unsupported {
                        opcode: Opcode {
                            map: OpcodeMap::Primary,
                            byte: 0xd8,
                        },
                        length: 2,
                    }),
                },
                "D8 {modrm:02x}"
            );
        }
        assert!(matches!(
            Memory::new(&[0xd8]).decode(),
            Err(DecodeError::Memory(MemoryError::Unmapped {
                address: 0x1000,
                access: AccessKind::Fetch,
            }))
        ));
        assert!(matches!(
            Memory::new(&[0x9b, 0xd8, 0xf1]).decode(),
            Err(DecodeError::Unsupported { .. })
        ));
        for source in 0u8..=7 {
            for prefix in [
                0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
            ]
            .into_iter()
            .chain(0x40..=0x4f)
            {
                assert!(matches!(
                    Memory::new(&[prefix, 0xd8, 0xf8 | source]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ));
            }
        }
    }

    #[test]
    fn fsubr_st0_sti_accepts_only_bare_d8_e8_through_ef() {
        for source in 0u8..=7 {
            let mut memory = Memory::new(&[0xd8, 0xe8 | source]);
            let instruction = memory.decode().unwrap();
            assert_eq!(instruction.operation, Operation::FsubrSt0St { source });
            assert_eq!(instruction.length, 2);
            assert_eq!(instruction.prefixes, Prefixes::default());
            assert_eq!(memory.fetched, [0xfff, 0x1000]);
        }

        for modrm in 0u8..=u8::MAX {
            let decoded = Memory::new(&[0xd8, modrm]).decode();
            assert_eq!(
                decoded.map(|instruction| instruction.operation),
                match modrm {
                    0xd0..=0xd7 => Ok(Operation::FcomSt { source: modrm & 7 }),
                    0xd8..=0xdf => Ok(Operation::FcompSt { source: modrm & 7 }),
                    0xc0..=0xc7 => Ok(Operation::FaddSt0St { source: modrm & 7 }),
                    0xc8..=0xcf => Ok(Operation::FmulSt0St { source: modrm & 7 }),
                    0xe0..=0xe7 => Ok(Operation::FsubSt0St { source: modrm & 7 }),
                    0xe8..=0xef => Ok(Operation::FsubrSt0St { source: modrm & 7 }),
                    0xf0..=0xf7 => Ok(Operation::FdivSt0St { source: modrm & 7 }),
                    0xf8..=0xff => Ok(Operation::FdivrSt0St { source: modrm & 7 }),
                    _ => Err(DecodeError::Unsupported {
                        opcode: Opcode {
                            map: OpcodeMap::Primary,
                            byte: 0xd8,
                        },
                        length: 2,
                    }),
                },
                "D8 {modrm:02x}"
            );
        }
        for source in 0u8..=7 {
            for prefix in [
                0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
            ]
            .into_iter()
            .chain(0x40..=0x4f)
            {
                assert!(matches!(
                    Memory::new(&[prefix, 0xd8, 0xe8 | source]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ));
            }
        }
        assert!(matches!(
            Memory::new(&[0xd8]).decode(),
            Err(DecodeError::Memory(MemoryError::Unmapped {
                address: 0x1000,
                access: AccessKind::Fetch,
            }))
        ));
        assert!(matches!(
            Memory::new(&[0xd8, 0xe8]).decode(),
            Ok(Instruction {
                length: 2,
                operation: Operation::FsubrSt0St { source: 0 },
                ..
            })
        ));
        assert!(matches!(
            Memory::new(&[0xdc, 0xe8]).decode(),
            Ok(Instruction {
                operation: Operation::FsubStSt0 { destination: 0 },
                ..
            })
        ));
        assert!(matches!(
            Memory::new(&[0xde, 0xe8]).decode(),
            Ok(Instruction {
                operation: Operation::FsubpStSt0 { destination: 0 },
                ..
            })
        ));
    }

    #[test]
    fn fsub_sti_st0_accepts_only_bare_dc_e8_through_ef() {
        for destination in 0u8..=7 {
            let mut memory = Memory::new(&[0xdc, 0xe8 | destination]);
            let instruction = memory.decode().unwrap();
            assert_eq!(instruction.operation, Operation::FsubStSt0 { destination });
            assert_eq!(instruction.length, 2);
            assert_eq!(instruction.prefixes, Prefixes::default());
        }
        for modrm in 0u8..=u8::MAX {
            let decoded = Memory::new(&[0xdc, modrm]).decode();
            assert_eq!(
                decoded.map(|instruction| instruction.operation),
                match modrm {
                    0xc0..=0xc7 => Ok(Operation::FaddStSt0 {
                        destination: modrm & 7
                    }),
                    0xc8..=0xcf => Ok(Operation::FmulStSt0 {
                        destination: modrm & 7
                    }),
                    0xe0..=0xe7 => Ok(Operation::FsubrStSt0 {
                        destination: modrm & 7
                    }),
                    0xe8..=0xef => Ok(Operation::FsubStSt0 {
                        destination: modrm & 7
                    }),
                    0xf0..=0xf7 => Ok(Operation::FdivrStSt0 {
                        destination: modrm & 7
                    }),
                    0xf8..=0xff => Ok(Operation::FdivStSt0 {
                        destination: modrm & 7
                    }),
                    _ => Err(DecodeError::Unsupported {
                        opcode: Opcode {
                            map: OpcodeMap::Primary,
                            byte: 0xdc
                        },
                        length: 2,
                    }),
                },
                "DC {modrm:02x}"
            );
        }
        for destination in 0u8..=7 {
            for prefix in [
                0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
            ]
            .into_iter()
            .chain(0x40..=0x4f)
            {
                assert!(matches!(
                    Memory::new(&[prefix, 0xdc, 0xe8 | destination]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ));
                assert!(matches!(
                    Memory::new(&[prefix, 0xdc, 0xe0 | destination]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ));
            }
        }
        assert!(matches!(
            Memory::new(&[0xdc]).decode(),
            Err(DecodeError::Memory(MemoryError::Unmapped { .. }))
        ));
        assert!(matches!(
            Memory::new(&[0x9b]).decode(),
            Err(DecodeError::Unsupported { .. })
        ));
    }

    #[test]
    fn fdiv_sti_st0_accepts_only_bare_dc_f8_through_ff() {
        for destination in 0u8..=7 {
            let mut memory = Memory::new(&[0xdc, 0xf8 | destination]);
            let instruction = memory.decode().unwrap();
            assert_eq!(instruction.operation, Operation::FdivStSt0 { destination });
            assert_eq!(instruction.length, 2);
            assert_eq!(instruction.prefixes, Prefixes::default());
            assert_eq!(memory.fetched, [0xfff, 0x1000]);
        }

        for modrm in 0u8..=u8::MAX {
            let decoded = Memory::new(&[0xdc, modrm]).decode();
            assert_eq!(
                decoded.map(|instruction| instruction.operation),
                match modrm {
                    0xc0..=0xc7 => Ok(Operation::FaddStSt0 {
                        destination: modrm & 7
                    }),
                    0xc8..=0xcf => Ok(Operation::FmulStSt0 {
                        destination: modrm & 7
                    }),
                    0xe0..=0xe7 => Ok(Operation::FsubrStSt0 {
                        destination: modrm & 7
                    }),
                    0xe8..=0xef => Ok(Operation::FsubStSt0 {
                        destination: modrm & 7
                    }),
                    0xf0..=0xf7 => Ok(Operation::FdivrStSt0 {
                        destination: modrm & 7
                    }),
                    0xf8..=0xff => Ok(Operation::FdivStSt0 {
                        destination: modrm & 7
                    }),
                    _ => Err(DecodeError::Unsupported {
                        opcode: Opcode {
                            map: OpcodeMap::Primary,
                            byte: 0xdc,
                        },
                        length: 2,
                    }),
                },
                "DC {modrm:02x}"
            );
        }

        for destination in 0u8..=7 {
            for prefix in [
                0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
            ]
            .into_iter()
            .chain(0x40..=0x4f)
            {
                assert!(matches!(
                    Memory::new(&[prefix, 0xdc, 0xf8 | destination]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ));
            }
        }
        assert!(matches!(
            Memory::new(&[0xdc]).decode(),
            Err(DecodeError::Memory(MemoryError::Unmapped { .. }))
        ));
    }

    #[test]
    fn fdivr_sti_st0_accepts_only_bare_dc_f0_through_f7() {
        for destination in 0u8..=7 {
            let mut memory = Memory::new(&[0xdc, 0xf0 | destination]);
            let instruction = memory.decode().unwrap();
            assert_eq!(instruction.operation, Operation::FdivrStSt0 { destination });
            assert_eq!(instruction.length, 2);
            assert_eq!(instruction.prefixes, Prefixes::default());
            assert_eq!(memory.fetched, [0xfff, 0x1000]);
        }

        for modrm in 0u8..=u8::MAX {
            let decoded = Memory::new(&[0xdc, modrm]).decode();
            assert_eq!(
                decoded.map(|instruction| instruction.operation),
                match modrm {
                    0xc0..=0xc7 => Ok(Operation::FaddStSt0 {
                        destination: modrm & 7
                    }),
                    0xc8..=0xcf => Ok(Operation::FmulStSt0 {
                        destination: modrm & 7
                    }),
                    0xe0..=0xe7 => Ok(Operation::FsubrStSt0 {
                        destination: modrm & 7
                    }),
                    0xe8..=0xef => Ok(Operation::FsubStSt0 {
                        destination: modrm & 7
                    }),
                    0xf0..=0xf7 => Ok(Operation::FdivrStSt0 {
                        destination: modrm & 7
                    }),
                    0xf8..=0xff => Ok(Operation::FdivStSt0 {
                        destination: modrm & 7
                    }),
                    _ => Err(DecodeError::Unsupported {
                        opcode: Opcode {
                            map: OpcodeMap::Primary,
                            byte: 0xdc,
                        },
                        length: 2,
                    }),
                },
                "DC {modrm:02x}"
            );
        }

        for destination in 0u8..=7 {
            for prefix in [
                0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
            ]
            .into_iter()
            .chain(0x40..=0x4f)
            {
                assert!(matches!(
                    Memory::new(&[prefix, 0xdc, 0xf0 | destination]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ));
            }
        }
        assert!(matches!(
            Memory::new(&[0xdc]).decode(),
            Err(DecodeError::Memory(MemoryError::Unmapped { .. }))
        ));
    }

    #[test]
    fn fadd_sti_st0_accepts_only_bare_dc_c0_through_c7() {
        for destination in 0u8..=7 {
            let mut memory = Memory::new(&[0xdc, 0xc0 | destination]);
            let instruction = memory.decode().unwrap();
            assert_eq!(instruction.operation, Operation::FaddStSt0 { destination });
            assert_eq!(instruction.length, 2);
            assert_eq!(instruction.prefixes, Prefixes::default());
            assert_eq!(memory.fetched, [0xfff, 0x1000]);
        }

        for destination in 0u8..=7 {
            let mut memory = Memory::new(&[0xdc, 0xc8 | destination]);
            let instruction = memory.decode().unwrap();
            assert_eq!(instruction.operation, Operation::FmulStSt0 { destination });
            assert_eq!(instruction.length, 2);
            assert_eq!(instruction.prefixes, Prefixes::default());
            assert_eq!(memory.fetched, [0xfff, 0x1000]);
            for prefix in [
                0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
            ]
            .into_iter()
            .chain(0x40..=0x4f)
            {
                assert!(matches!(
                    Memory::new(&[prefix, 0xdc, 0xc8 | destination]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ));
            }
        }

        for modrm in 0u8..=u8::MAX {
            let decoded = Memory::new(&[0xdc, modrm]).decode();
            assert_eq!(
                decoded.map(|instruction| instruction.operation),
                match modrm {
                    0xc0..=0xc7 => Ok(Operation::FaddStSt0 {
                        destination: modrm & 7,
                    }),
                    0xc8..=0xcf => Ok(Operation::FmulStSt0 {
                        destination: modrm & 7,
                    }),
                    0xe0..=0xe7 => Ok(Operation::FsubrStSt0 {
                        destination: modrm & 7,
                    }),
                    0xe8..=0xef => Ok(Operation::FsubStSt0 {
                        destination: modrm & 7,
                    }),
                    0xf0..=0xf7 => Ok(Operation::FdivrStSt0 {
                        destination: modrm & 7,
                    }),
                    0xf8..=0xff => Ok(Operation::FdivStSt0 {
                        destination: modrm & 7,
                    }),
                    _ => Err(DecodeError::Unsupported {
                        opcode: Opcode {
                            map: OpcodeMap::Primary,
                            byte: 0xdc,
                        },
                        length: 2,
                    }),
                },
                "DC {modrm:02x}"
            );
        }
        for destination in 0u8..=7 {
            for prefix in [
                0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
            ]
            .into_iter()
            .chain(0x40..=0x4f)
            {
                assert!(matches!(
                    Memory::new(&[prefix, 0xdc, 0xc0 | destination]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ));
            }
            assert!(matches!(
                Memory::new(&[0xdc]).decode(),
                Err(DecodeError::Memory(MemoryError::Unmapped { .. }))
            ));
        }
    }

    #[test]
    fn faddp_sti_st0_accepts_only_bare_de_c0_through_c7() {
        for destination in 0u8..=7 {
            let mut memory = Memory::new(&[0xde, 0xc0 | destination]);
            let instruction = memory.decode().unwrap();
            assert_eq!(instruction.operation, Operation::FaddpStSt0 { destination });
            assert_eq!(instruction.length, 2);
            assert_eq!(instruction.prefixes, Prefixes::default());
            assert_eq!(memory.fetched, [0xfff, 0x1000]);
        }

        for destination in 0u8..=7 {
            let mut memory = Memory::new(&[0xde, 0xc8 | destination]);
            let instruction = memory.decode().unwrap();
            assert_eq!(instruction.operation, Operation::FmulpStSt0 { destination });
            assert_eq!(instruction.length, 2);
            assert_eq!(instruction.prefixes, Prefixes::default());
            assert_eq!(memory.fetched, [0xfff, 0x1000]);
        }

        for modrm in 0u8..=u8::MAX {
            let decoded = Memory::new(&[0xde, modrm]).decode();
            assert_eq!(
                decoded.map(|instruction| instruction.operation),
                match modrm {
                    0xc0..=0xc7 => Ok(Operation::FaddpStSt0 {
                        destination: modrm & 7,
                    }),
                    0xc8..=0xcf => Ok(Operation::FmulpStSt0 {
                        destination: modrm & 7,
                    }),
                    0xe0..=0xe7 => Ok(Operation::FsubrpStSt0 {
                        destination: modrm & 7,
                    }),
                    0xe8..=0xef => Ok(Operation::FsubpStSt0 {
                        destination: modrm & 7,
                    }),
                    0xf0..=0xf7 => Ok(Operation::FdivrpStSt0 {
                        destination: modrm & 7,
                    }),
                    0xf8..=0xff => Ok(Operation::FdivpStSt0 {
                        destination: modrm & 7,
                    }),
                    0xd9 => Ok(Operation::Fcompp),
                    _ => Err(DecodeError::Unsupported {
                        opcode: Opcode {
                            map: OpcodeMap::Primary,
                            byte: 0xde
                        },
                        length: 2,
                    }),
                },
                "DE {modrm:02x}"
            );
        }
        for destination in 0u8..=7 {
            for prefix in [
                0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
            ]
            .into_iter()
            .chain(0x40..=0x4f)
            {
                assert!(matches!(
                    Memory::new(&[prefix, 0xde, 0xc0 | destination]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ));
                assert!(matches!(
                    Memory::new(&[prefix, 0xde, 0xc8 | destination]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ));
            }
            assert!(matches!(
                Memory::new(&[0xde]).decode(),
                Err(DecodeError::Memory(MemoryError::Unmapped { .. }))
            ));
        }
    }

    #[test]
    fn fcompp_accepts_only_bare_de_d9_and_preserves_existing_de_ranges() {
        let mut memory = Memory::new(&[0xde, 0xd9]);
        let instruction = memory.decode().unwrap();
        assert_eq!(instruction.operation, Operation::Fcompp);
        assert_eq!(instruction.length, 2);
        assert_eq!(instruction.prefixes, Prefixes::default());
        assert_eq!(memory.fetched, [0xfff, 0x1000]);

        for modrm in 0u8..=u8::MAX {
            let decoded = Memory::new(&[0xde, modrm]).decode();
            assert_eq!(
                decoded.map(|instruction| instruction.operation),
                match modrm {
                    0xc0..=0xc7 => Ok(Operation::FaddpStSt0 {
                        destination: modrm & 7
                    }),
                    0xc8..=0xcf => Ok(Operation::FmulpStSt0 {
                        destination: modrm & 7
                    }),
                    0xd9 => Ok(Operation::Fcompp),
                    0xe0..=0xe7 => Ok(Operation::FsubrpStSt0 {
                        destination: modrm & 7
                    }),
                    0xe8..=0xef => Ok(Operation::FsubpStSt0 {
                        destination: modrm & 7
                    }),
                    0xf0..=0xf7 => Ok(Operation::FdivrpStSt0 {
                        destination: modrm & 7
                    }),
                    0xf8..=0xff => Ok(Operation::FdivpStSt0 {
                        destination: modrm & 7
                    }),
                    _ => Err(DecodeError::Unsupported {
                        opcode: Opcode {
                            map: OpcodeMap::Primary,
                            byte: 0xde,
                        },
                        length: 2,
                    }),
                },
                "DE {modrm:02x}"
            );
        }
        for prefix in [0xf0, 0xf2, 0xf3, 0x2e, 0x64, 0x66, 0x67, 0x9b]
            .into_iter()
            .chain(0x40..=0x4f)
        {
            assert!(matches!(
                Memory::new(&[prefix, 0xde, 0xd9]).decode(),
                Err(DecodeError::Unsupported { .. })
            ));
        }
        assert!(matches!(
            Memory::new(&[0xde]).decode(),
            Err(DecodeError::Memory(MemoryError::Unmapped {
                access: AccessKind::Fetch,
                ..
            }))
        ));
        assert!(matches!(
            Memory::new(&[0x9b, 0xde, 0xd9]).decode(),
            Err(DecodeError::Unsupported { .. })
        ));
    }

    #[test]
    fn fucompp_accepts_only_bare_da_e9() {
        let mut memory = Memory::new(&[0xda, 0xe9]);
        let instruction = memory.decode().unwrap();
        assert_eq!(instruction.operation, Operation::Fucompp);
        assert_eq!(instruction.length, 2);
        assert_eq!(instruction.prefixes, Prefixes::default());
        assert_eq!(memory.fetched, [0xfff, 0x1000]);

        for modrm in 0u8..=u8::MAX {
            let decoded = Memory::new(&[0xda, modrm]).decode();
            assert_eq!(
                decoded.map(|instruction| instruction.operation),
                if modrm == 0xe9 {
                    Ok(Operation::Fucompp)
                } else {
                    Err(DecodeError::Unsupported {
                        opcode: Opcode {
                            map: OpcodeMap::Primary,
                            byte: 0xda,
                        },
                        length: 2,
                    })
                },
                "DA {modrm:02x}"
            );
        }
        for prefix in [
            0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
        ]
        .into_iter()
        .chain(0x40..=0x4f)
        {
            assert!(matches!(
                Memory::new(&[prefix, 0xda, 0xe9]).decode(),
                Err(DecodeError::Unsupported { .. })
            ));
        }
        assert!(matches!(
            Memory::new(&[0xda]).decode(),
            Err(DecodeError::Memory(MemoryError::Unmapped {
                access: AccessKind::Fetch,
                ..
            }))
        ));
        assert!(matches!(
            Memory::new(&[0x9b, 0xda, 0xe9]).decode(),
            Err(DecodeError::Unsupported { .. })
        ));
    }

    #[test]
    fn fsubp_sti_st0_accepts_only_bare_de_e8_through_ef() {
        for destination in 0u8..=7 {
            let mut memory = Memory::new(&[0xde, 0xe8 | destination]);
            let instruction = memory.decode().unwrap();
            assert_eq!(instruction.operation, Operation::FsubpStSt0 { destination });
            assert_eq!(instruction.length, 2);
            assert_eq!(instruction.prefixes, Prefixes::default());
            assert_eq!(memory.fetched, [0xfff, 0x1000]);
        }
        for modrm in 0u8..=u8::MAX {
            let decoded = Memory::new(&[0xde, modrm]).decode();
            assert_eq!(
                decoded.map(|instruction| instruction.operation),
                match modrm {
                    0xc0..=0xc7 => Ok(Operation::FaddpStSt0 {
                        destination: modrm & 7
                    }),
                    0xc8..=0xcf => Ok(Operation::FmulpStSt0 {
                        destination: modrm & 7
                    }),
                    0xe0..=0xe7 => Ok(Operation::FsubrpStSt0 {
                        destination: modrm & 7
                    }),
                    0xe8..=0xef => Ok(Operation::FsubpStSt0 {
                        destination: modrm & 7
                    }),
                    0xf0..=0xf7 => Ok(Operation::FdivrpStSt0 {
                        destination: modrm & 7
                    }),
                    0xf8..=0xff => Ok(Operation::FdivpStSt0 {
                        destination: modrm & 7
                    }),
                    0xd9 => Ok(Operation::Fcompp),
                    _ => Err(DecodeError::Unsupported {
                        opcode: Opcode {
                            map: OpcodeMap::Primary,
                            byte: 0xde
                        },
                        length: 2,
                    }),
                },
                "DE {modrm:02x}"
            );
        }
        for destination in 0u8..=7 {
            for prefix in [
                0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
            ]
            .into_iter()
            .chain(0x40..=0x4f)
            {
                assert!(matches!(
                    Memory::new(&[prefix, 0xde, 0xe8 | destination]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ));
            }
        }
        assert!(matches!(
            Memory::new(&[0xde]).decode(),
            Err(DecodeError::Memory(_))
        ));
        assert!(matches!(
            Memory::new(&[0x9b]).decode(),
            Err(DecodeError::Unsupported { .. })
        ));
    }

    #[test]
    fn fdivp_sti_st0_accepts_only_bare_de_f8_through_ff() {
        for destination in 0u8..=7 {
            let mut memory = Memory::new(&[0xde, 0xf8 | destination]);
            let instruction = memory.decode().unwrap();
            assert_eq!(instruction.operation, Operation::FdivpStSt0 { destination });
            assert_eq!(instruction.length, 2);
            assert_eq!(instruction.prefixes, Prefixes::default());
            assert_eq!(memory.fetched, [0xfff, 0x1000]);
        }
        for destination in 0u8..=7 {
            for prefix in [
                0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
            ]
            .into_iter()
            .chain(0x40..=0x4f)
            {
                assert!(matches!(
                    Memory::new(&[prefix, 0xde, 0xf8 | destination]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ));
            }
        }
        for modrm in 0u8..=u8::MAX {
            let decoded = Memory::new(&[0xde, modrm]).decode();
            assert_eq!(
                decoded.map(|instruction| instruction.operation),
                match modrm {
                    0xc0..=0xc7 => Ok(Operation::FaddpStSt0 {
                        destination: modrm & 7
                    }),
                    0xc8..=0xcf => Ok(Operation::FmulpStSt0 {
                        destination: modrm & 7
                    }),
                    0xe0..=0xe7 => Ok(Operation::FsubrpStSt0 {
                        destination: modrm & 7
                    }),
                    0xe8..=0xef => Ok(Operation::FsubpStSt0 {
                        destination: modrm & 7
                    }),
                    0xf0..=0xf7 => Ok(Operation::FdivrpStSt0 {
                        destination: modrm & 7
                    }),
                    0xf8..=0xff => Ok(Operation::FdivpStSt0 {
                        destination: modrm & 7
                    }),
                    0xd9 => Ok(Operation::Fcompp),
                    _ => Err(DecodeError::Unsupported {
                        opcode: Opcode {
                            map: OpcodeMap::Primary,
                            byte: 0xde,
                        },
                        length: 2,
                    }),
                },
                "DE {modrm:02x}"
            );
        }
        assert!(matches!(
            Memory::new(&[0xde]).decode(),
            Err(DecodeError::Memory(MemoryError::Unmapped { .. }))
        ));
    }

    #[test]
    fn fdivrp_sti_st0_accepts_only_bare_de_f0_through_f7() {
        for destination in 0u8..=7 {
            let mut memory = Memory::new(&[0xde, 0xf0 | destination]);
            let instruction = memory.decode().unwrap();
            assert_eq!(
                instruction.operation,
                Operation::FdivrpStSt0 { destination }
            );
            assert_eq!(instruction.length, 2);
            assert_eq!(instruction.prefixes, Prefixes::default());
            assert_eq!(memory.fetched, [0xfff, 0x1000]);
            for prefix in [
                0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65, 0x66, 0x67, 0x9b,
            ]
            .into_iter()
            .chain(0x40..=0x4f)
            {
                assert!(matches!(
                    Memory::new(&[prefix, 0xde, 0xf0 | destination]).decode(),
                    Err(DecodeError::Unsupported { .. })
                ));
            }
        }
        assert!(matches!(
            Memory::new(&[0xde]).decode(),
            Err(DecodeError::Memory(MemoryError::Unmapped { .. }))
        ));
        assert!(matches!(
            Memory::new(&[0x9b, 0xde, 0xf1]).decode(),
            Err(DecodeError::Unsupported { .. })
        ));
    }

    #[test]
    fn mfence_accepts_only_the_bare_f0_modrm_form_in_this_profile() {
        let mut memory = Memory::new(&[0x0f, 0xae, 0xf0]);
        let instruction = memory.decode().unwrap();
        assert_eq!(instruction.operation, Operation::Mfence);
        assert_eq!(instruction.length, 3);
        assert_eq!(instruction.prefixes, Prefixes::default());
        assert_eq!(memory.fetched, [0xfff, 0x1000, 0x1001]);

        for bytes in [
            &[0x0f, 0xae, 0xe8][..], // LFENCE
            &[0x0f, 0xae, 0xf8][..], // SFENCE
            &[0x0f, 0xae, 0xf1][..], // alternate hardware-accepted MFENCE encoding
            &[0x0f, 0xae, 0x00][..], // memory ModR/M form
            &[0x66, 0x0f, 0xae, 0xf0][..],
            &[0x48, 0x0f, 0xae, 0xf0][..],
        ] {
            let mut memory = Memory::new(bytes);
            assert!(matches!(
                memory.decode(),
                Err(DecodeError::Unsupported { .. })
            ));
        }
    }

    #[test]
    fn every_fetch_boundary_preserves_fault_or_mapping_error_without_overfetch() {
        for bytes in [
            vec![0x66, 0x67, 0x49, 0xb8, 1, 2, 3, 4, 5, 6, 7, 8],
            vec![0x4d, 0x03, 0xc7],
            vec![0x74, 0xfe],
            vec![0x66, 0x0f, 0x84, 0, 0, 0, 0],
            vec![0x7f, 0x02],
            vec![0x0f, 0x8f, 0, 0, 0, 0],
            vec![0xeb, 0xfe],
            vec![0xe9, 0, 0, 0, 0],
            vec![0x6a, 0x80],
            vec![0x68, 0, 0, 0, 0x80],
            vec![0x0f, 0x9f, 0xc0],
            vec![0x0f, 0x94, 0x45, 0x7f],
            vec![0x48, 0x0f, 0xc1, 0x00],
            vec![0xf0, 0x48, 0x0f, 0xc1, 0x00],
            vec![0x0f, 0xae, 0xf0],
            vec![0x48, 0x0f, 0xb1, 0x18],
            vec![0xb4, 0xff],
            vec![0x0f, 0x38, 0xff],
            vec![0x0f, 0x3a, 0xff],
        ] {
            for cut in 0..bytes.len() {
                let mut memory = Memory::new(&bytes[..cut]);
                assert_eq!(
                    memory.decode(),
                    Err(DecodeError::Memory(MemoryError::Unmapped {
                        address: memory.base + cut as u64,
                        access: AccessKind::Fetch
                    }))
                );
                assert_eq!(memory.fetched.len(), cut + 1);
                let mut memory = Memory::new(&bytes);
                memory.fault_at = Some(cut);
                let result = memory.decode();
                assert_eq!(
                    result,
                    Err(DecodeError::Memory(MemoryError::Architectural(
                        ArchitecturalFault {
                            vector: 14,
                            error_code: Some(20),
                            linear_address: Some(memory.base + cut as u64)
                        }
                    )))
                );
                assert_eq!(memory.fetched.len(), cut + 1);
            }
            let mut memory = Memory::new(&bytes);
            let _ = memory.decode();
            assert_eq!(memory.fetched.len(), bytes.len());
        }
    }

    #[test]
    fn all_condition_codes_match_literal_truth_table_for_cf_zf_sf_of_pf() {
        // Rows are the five input flags in CF, ZF, SF, OF, PF bit order;
        // each literal result bit is one condition code in architectural order.
        const EXPECTED: [u16; 32] = [
            0xaaaa, 0xaa66, 0x6a5a, 0x6a56, 0x59aa, 0x5966, 0x595a, 0x5956, 0x5aa9, 0x5a65, 0x5a59,
            0x5a55, 0xa9a9, 0xa965, 0x6959, 0x6955, 0xa6aa, 0xa666, 0x665a, 0x6656, 0x55aa, 0x5566,
            0x555a, 0x5556, 0x56a9, 0x5665, 0x5659, 0x5655, 0xa5a9, 0xa565, 0x6559, 0x6555,
        ];
        let flag_bits = [crate::CF, crate::ZF, crate::SF, crate::OF, crate::PF];
        for (input, expected) in EXPECTED.into_iter().enumerate() {
            let flags = flag_bits
                .into_iter()
                .enumerate()
                .fold(0, |flags, (bit, flag)| {
                    flags | if input & (1 << bit) != 0 { flag } else { 0 }
                });
            for encoding in 0..16 {
                let condition = ConditionCode::from_encoding(encoding).unwrap();
                assert_eq!(
                    condition.evaluate(flags),
                    expected & (1 << encoding) != 0,
                    "input flags {input:05b}, condition {encoding:x}"
                );
            }
        }
    }

    #[test]
    fn direct_relative_control_flow_decodes_signed_offsets_at_full_length() {
        for (bytes, expected) in [
            (
                &[0x70, 0xfe][..],
                Operation::Branch {
                    condition: ConditionCode::Overflow,
                    displacement: -2,
                },
            ),
            (
                &[0x0f, 0x8f, 0, 0, 0, 0][..],
                Operation::Branch {
                    condition: ConditionCode::Greater,
                    displacement: 0,
                },
            ),
            (&[0xeb, 0x80][..], Operation::Jump { displacement: -128 }),
            (&[0xe9, 0, 0, 0, 0][..], Operation::Jump { displacement: 0 }),
            (&[0xe8, 0, 0, 0, 0][..], Operation::Call { displacement: 0 }),
            (&[0xc3][..], Operation::Return { stack_adjust: 0 }),
            (
                &[0xc2, 0x34, 0x12][..],
                Operation::Return {
                    stack_adjust: 0x1234,
                },
            ),
        ] {
            let mut memory = Memory::new(bytes);
            let instruction = memory.decode().unwrap();
            assert_eq!(instruction.length as usize, bytes.len());
            assert_eq!(instruction.operation, expected);
            assert_eq!(memory.fetched.len(), bytes.len());
        }
    }

    #[test]
    fn qword_memory_xchg_decodes_only_the_bare_profile_form() {
        let mut memory = Memory::new(&[0x48, 0x87, 0x00]);
        let instruction = memory.decode().unwrap();
        assert_eq!(instruction.length, 3);
        assert_eq!(
            instruction.operation,
            Operation::XchgMemory {
                register: RegisterView::low(Register::Rax, OperandSize::Qword),
                memory: MemoryOperand {
                    base: AddressBase::Register(Register::Rax),
                    index: None,
                    scale: 1,
                    displacement: 0,
                    address_size: OperandSize::Qword,
                    size: OperandSize::Qword,
                },
            }
        );
        for bytes in [
            &[0x87, 0x00][..],
            &[0x66, 0x48, 0x87, 0x00],
            &[0x67, 0x48, 0x87, 0x00],
            &[0x2e, 0x48, 0x87, 0x00],
            &[0xf0, 0x48, 0x87, 0x00],
        ] {
            assert!(
                matches!(
                    Memory::new(bytes).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "{bytes:02x?}"
            );
        }
        let mut register_form = Memory::new(&[0x48, 0x87, 0xc0]);
        assert!(matches!(
            register_form.decode().unwrap().operation,
            Operation::Xchg { .. }
        ));
    }

    #[test]
    fn qword_register_xadd_decodes_only_the_bare_profile_form() {
        let mut memory = Memory::new(&[0x48, 0x0f, 0xc1, 0xc0]);
        let instruction = memory.decode().unwrap();
        assert_eq!(instruction.length, 4);
        assert_eq!(
            instruction.operation,
            Operation::Xadd {
                destination: RegisterView::low(Register::Rax, OperandSize::Qword),
                source: RegisterView::low(Register::Rax, OperandSize::Qword),
            }
        );
        assert_eq!(instruction.flag_effects().defined, crate::ARITHMETIC_FLAGS);

        let mut memory_form = Memory::new(&[0x48, 0x0f, 0xb1, 0x18]);
        let memory_instruction = memory_form.decode().unwrap();
        assert_eq!(memory_instruction.length, 4);
        assert_eq!(
            memory_instruction.operation,
            Operation::CmpxchgMemory {
                destination: MemoryOperand {
                    base: AddressBase::Register(Register::Rax),
                    index: None,
                    scale: 1,
                    displacement: 0,
                    address_size: OperandSize::Qword,
                    size: OperandSize::Qword,
                },
                source: RegisterView::low(Register::Rbx, OperandSize::Qword),
            }
        );
        assert_eq!(
            memory_instruction.flag_effects().defined,
            crate::ARITHMETIC_FLAGS
        );

        let mut extended = Memory::new(&[0x4d, 0x0f, 0xc1, 0xc8]);
        assert_eq!(
            extended.decode().unwrap().operation,
            Operation::Xadd {
                destination: RegisterView::low(Register::R8, OperandSize::Qword),
                source: RegisterView::low(Register::R9, OperandSize::Qword),
            }
        );

        let mut memory_form = Memory::new(&[0x48, 0x0f, 0xc1, 0x00]);
        assert_eq!(
            memory_form.decode().unwrap().operation,
            Operation::XaddMemory {
                destination: MemoryOperand {
                    base: AddressBase::Register(Register::Rax),
                    index: None,
                    scale: 1,
                    displacement: 0,
                    address_size: OperandSize::Qword,
                    size: OperandSize::Qword,
                },
                source: RegisterView::low(Register::Rax, OperandSize::Qword),
            }
        );

        for bytes in [
            &[0x0f, 0xc1, 0xc0][..], // default dword width
            &[0x66, 0x48, 0x0f, 0xc1, 0xc0],
            &[0x67, 0x48, 0x0f, 0xc1, 0xc0],
            &[0x2e, 0x48, 0x0f, 0xc1, 0xc0],
            &[0xf0, 0x48, 0x0f, 0xc1, 0xc0],
            &[0x0f, 0xc1, 0x00], // default dword memory destination
            &[0x66, 0x48, 0x0f, 0xc1, 0x00],
            &[0x67, 0x48, 0x0f, 0xc1, 0x00],
            &[0x2e, 0x48, 0x0f, 0xc1, 0x00],
        ] {
            assert!(
                matches!(
                    Memory::new(bytes).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "{bytes:02x?}"
            );
        }
    }

    #[test]
    fn lock_qword_memory_xadd_decodes_only_the_profile_form() {
        let mut memory = Memory::new(&[0xf0, 0x49, 0x0f, 0xc1, 0x0c, 0x24]);
        let instruction = memory.decode().unwrap();
        assert_eq!(instruction.length, 6);
        assert_eq!(instruction.prefixes.group1, Some(0xf0));
        assert_eq!(
            instruction.operation,
            Operation::XaddMemory {
                destination: MemoryOperand {
                    base: AddressBase::Register(Register::R12),
                    index: None,
                    scale: 1,
                    displacement: 0,
                    address_size: OperandSize::Qword,
                    size: OperandSize::Qword,
                },
                source: RegisterView::low(Register::Rcx, OperandSize::Qword),
            }
        );

        for bytes in [
            &[0xf0, 0x48, 0x0f, 0xc1, 0xc0][..], // LOCK register destination
            &[0xf0, 0x0f, 0xc1, 0x00][..],       // default dword width
            &[0xf0, 0x66, 0x48, 0x0f, 0xc1, 0x00],
            &[0xf0, 0x67, 0x48, 0x0f, 0xc1, 0x00],
            &[0xf0, 0x2e, 0x48, 0x0f, 0xc1, 0x00],
            &[0xf2, 0x48, 0x0f, 0xc1, 0x00],
            &[0xf3, 0x48, 0x0f, 0xc1, 0x00],
        ] {
            assert!(
                matches!(
                    Memory::new(bytes).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "{bytes:02x?}"
            );
        }
    }

    #[test]
    fn qword_register_cmpxchg_decodes_only_the_bare_profile_form() {
        let mut memory = Memory::new(&[0x48, 0x0f, 0xb1, 0xd9]);
        let instruction = memory.decode().unwrap();
        assert_eq!(instruction.length, 4);
        assert_eq!(
            instruction.operation,
            Operation::Cmpxchg {
                destination: RegisterView::low(Register::Rcx, OperandSize::Qword),
                source: RegisterView::low(Register::Rbx, OperandSize::Qword),
            }
        );
        assert_eq!(instruction.flag_effects().defined, crate::ARITHMETIC_FLAGS);

        let mut extended = Memory::new(&[0x4d, 0x0f, 0xb1, 0xc8]);
        assert_eq!(
            extended.decode().unwrap().operation,
            Operation::Cmpxchg {
                destination: RegisterView::low(Register::R8, OperandSize::Qword),
                source: RegisterView::low(Register::R9, OperandSize::Qword),
            }
        );

        for bytes in [
            &[0x0f, 0xb1, 0xd9][..], // default dword width
            &[0x66, 0x48, 0x0f, 0xb1, 0xd9],
            &[0x67, 0x48, 0x0f, 0xb1, 0xd9],
            &[0x2e, 0x48, 0x0f, 0xb1, 0xd9],
            &[0xf0, 0x48, 0x0f, 0xb1, 0xd9],
            &[0xf3, 0x48, 0x0f, 0xb1, 0x18],
            &[0x0f, 0xb1, 0x18], // default dword memory destination
        ] {
            assert!(
                matches!(
                    Memory::new(bytes).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "{bytes:02x?}"
            );
        }
    }

    #[test]
    fn direct_call_and_ret_reject_prefixes_and_truncated_immediates() {
        for bytes in [
            &[0x66, 0xe8, 0, 0, 0, 0][..],
            &[0x2e, 0xe8, 0, 0, 0, 0][..],
            &[0x67, 0xe8, 0, 0, 0, 0][..],
            &[0x48, 0xe8, 0, 0, 0, 0][..],
            &[0xf3, 0xe8, 0, 0, 0, 0][..],
            &[0x48, 0xc3][..],
            &[0x2e, 0x66, 0xc3][..],
            &[0x66, 0x48, 0xc3][..],
            &[0x66, 0x66, 0xc3][..],
            &[0x66, 0xc2, 0, 0][..],
            &[0x48, 0xc2, 0, 0][..],
        ] {
            let mut memory = Memory::new(bytes);
            assert!(
                matches!(memory.decode(), Err(DecodeError::Unsupported { .. })),
                "{bytes:02x?}"
            );
        }
        for bytes in [&[0xc2][..], &[0xc2, 0x34][..]] {
            let mut memory = Memory::new(bytes);
            assert!(matches!(memory.decode(), Err(DecodeError::Memory(_))));
            assert_eq!(memory.fetched.len(), bytes.len() + 1);
        }

        for bytes in [
            &[0xe8][..],
            &[0xe8, 0][..],
            &[0xe8, 0, 0][..],
            &[0xe8, 0, 0, 0][..],
        ] {
            let mut memory = Memory::new(bytes);
            assert!(
                matches!(memory.decode(), Err(DecodeError::Memory(_))),
                "{bytes:02x?}"
            );
        }

        for bytes in [&[0x66, 0xc3][..], &[0x48, 0x66, 0xc3][..]] {
            let mut memory = Memory::new(bytes);
            let instruction = memory.decode().unwrap();
            assert_eq!(instruction.length as usize, bytes.len());
            assert_eq!(instruction.operand_size, OperandSize::Qword);
            assert_eq!(instruction.operation, Operation::Return { stack_adjust: 0 });
        }
    }

    #[test]
    fn indirect_jump_decodes_registers_and_existing_memory_address_forms() {
        for (bytes, register) in [
            (&[0xff, 0xe0][..], Register::Rax),
            (&[0x41, 0xff, 0xe0][..], Register::R8),
            (&[0x49, 0xff, 0xe0][..], Register::R8),
        ] {
            let mut memory = Memory::new(bytes);
            let instruction = memory.decode().unwrap();
            assert_eq!(instruction.length as usize, bytes.len());
            assert_eq!(instruction.operand_size, OperandSize::Qword);
            assert_eq!(
                instruction.operation,
                Operation::IndirectJump {
                    operand: Operand::Register(RegisterView::low(register, OperandSize::Qword))
                }
            );
        }

        for (bytes, expected) in [
            (
                &[0xff, 0x20][..],
                MemoryOperand {
                    base: AddressBase::Register(Register::Rax),
                    index: None,
                    scale: 1,
                    displacement: 0,
                    address_size: OperandSize::Qword,
                    size: OperandSize::Qword,
                },
            ),
            (
                &[0x41, 0xff, 0x20][..],
                MemoryOperand {
                    base: AddressBase::Register(Register::R8),
                    index: None,
                    scale: 1,
                    displacement: 0,
                    address_size: OperandSize::Qword,
                    size: OperandSize::Qword,
                },
            ),
            (
                &[0x67, 0xff, 0x20][..],
                MemoryOperand {
                    base: AddressBase::Register(Register::Rax),
                    index: None,
                    scale: 1,
                    displacement: 0,
                    address_size: OperandSize::Dword,
                    size: OperandSize::Qword,
                },
            ),
            (
                &[0xff, 0x64, 0x88, 0x10][..],
                MemoryOperand {
                    base: AddressBase::Register(Register::Rax),
                    index: Some(Register::Rcx),
                    scale: 4,
                    displacement: 0x10,
                    address_size: OperandSize::Qword,
                    size: OperandSize::Qword,
                },
            ),
            (
                &[0xff, 0x25, 0, 0, 0, 0][..],
                MemoryOperand {
                    base: AddressBase::NextRip,
                    index: None,
                    scale: 1,
                    displacement: 0,
                    address_size: OperandSize::Qword,
                    size: OperandSize::Qword,
                },
            ),
        ] {
            let mut memory = Memory::new(bytes);
            let instruction = memory.decode().unwrap();
            assert_eq!(
                instruction.operation,
                Operation::IndirectJump {
                    operand: Operand::Memory(expected)
                },
                "{bytes:02x?}"
            );
            assert_eq!(instruction.length as usize, bytes.len());
        }

        for bytes in [
            &[0xff, 0xe8][..], // /5 far JMP
            &[0x66, 0xff, 0xe0],
            &[0x67, 0xff, 0xe0],
            &[0x2e, 0xff, 0xe0],
            &[0xf3, 0xff, 0xe0],
            &[0x64, 0xff, 0x20],
            &[0x65, 0xff, 0x20],
        ] {
            assert!(
                matches!(
                    Memory::new(bytes).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "{bytes:02x?}"
            );
        }
        for bytes in [
            &[0xff][..],
            &[0xff, 0x20][..],
            &[0xff, 0x24][..],
            &[0xff, 0x25][..],
        ] {
            let mut memory = Memory::new(bytes);
            if bytes.len() == 1 {
                assert!(matches!(memory.decode(), Err(DecodeError::Memory(_))));
            } else {
                let mut memory = Memory::new(bytes);
                memory.fault_at = Some(1);
                assert!(matches!(memory.decode(), Err(DecodeError::Memory(_))));
            }
        }
    }

    #[test]
    fn indirect_call_decodes_register_and_ordinary_memory_r64_forms() {
        for (bytes, register) in [
            (&[0xff, 0xd0][..], Register::Rax),
            (&[0x41, 0xff, 0xd0][..], Register::R8),
        ] {
            let mut memory = Memory::new(bytes);
            let instruction = memory.decode().unwrap();
            assert_eq!(instruction.length as usize, bytes.len());
            assert_eq!(instruction.operand_size, OperandSize::Qword);
            assert_eq!(
                instruction.operation,
                Operation::IndirectCall {
                    operand: Operand::Register(RegisterView::low(register, OperandSize::Qword))
                }
            );
        }

        for bytes in [
            &[0xff, 0x10][..],             // [RAX]
            &[0x41, 0xff, 0x10][..],       // [R8]
            &[0xff, 0x54, 0x88, 0x10][..], // [RAX + RCX*4 + 0x10]
            &[0xff, 0x15, 0, 0, 0, 0][..], // RIP-relative
            &[0x67, 0xff, 0x10][..],       // 32-bit address size
        ] {
            let mut memory = Memory::new(bytes);
            let instruction = memory.decode().unwrap();
            assert_eq!(instruction.length as usize, bytes.len());
            assert_eq!(instruction.operand_size, OperandSize::Qword);
            assert!(matches!(
                instruction.operation,
                Operation::IndirectCall {
                    operand: Operand::Memory(MemoryOperand {
                        size: OperandSize::Qword,
                        ..
                    })
                }
            ));
        }

        for bytes in [
            &[0xff, 0xd8][..], // far /3
            &[0x48, 0xff, 0xd0][..],
            &[0x40, 0xff, 0xd0][..],
            &[0x66, 0xff, 0xd0][..],
            &[0x67, 0xff, 0xd0][..],
            &[0xf3, 0xff, 0xd0][..],
            &[0xf0, 0xff, 0x10][..], // LOCK memory CALL
            &[0x64, 0xff, 0x10][..], // FS-relative profile is unsupported
            &[0x66, 0xff, 0x10][..], // 16-bit operand override
        ] {
            let mut memory = Memory::new(bytes);
            assert!(
                matches!(memory.decode(), Err(DecodeError::Unsupported { .. })),
                "{bytes:02x?}"
            );
        }
    }

    #[test]
    fn memory_push_decodes_qword_base_sib_rip_relative_and_address32_forms() {
        for bytes in [
            &[0xff, 0x30][..],             // [RAX]
            &[0x41, 0xff, 0x30][..],       // [R8]
            &[0xff, 0x34, 0x8b][..],       // [RBX + RCX*4]
            &[0xff, 0x35, 0, 0, 0, 0][..], // RIP-relative
            &[0x67, 0xff, 0x30][..],       // [EAX]
        ] {
            let instruction = Memory::new(bytes).decode().unwrap();
            assert_eq!(instruction.length as usize, bytes.len());
            assert_eq!(instruction.operand_size, OperandSize::Qword);
            assert!(
                matches!(
                    instruction.operation,
                    Operation::PushMemory {
                        source: MemoryOperand {
                            size: OperandSize::Qword,
                            ..
                        }
                    }
                ),
                "decoded {bytes:02x?} as {instruction:?}"
            );
        }

        for bytes in [
            &[0x66, 0xff, 0x30][..],             // word [RAX]
            &[0x66, 0x41, 0xff, 0x30][..],       // word [R8]
            &[0x66, 0xff, 0x34, 0x8b][..],       // word [RBX + RCX*4]
            &[0x66, 0xff, 0x35, 0, 0, 0, 0][..], // word RIP-relative
            &[0x67, 0x66, 0xff, 0x30][..],       // word [EAX]
            &[0x66, 0xff, 0x34, 0x24][..],       // word [RSP]
        ] {
            let instruction = Memory::new(bytes).decode().unwrap();
            assert_eq!(instruction.length as usize, bytes.len());
            assert_eq!(instruction.operand_size, OperandSize::Word);
            assert!(
                matches!(
                    instruction.operation,
                    Operation::PushMemory {
                        source: MemoryOperand {
                            size: OperandSize::Word,
                            ..
                        }
                    }
                ),
                "decoded {bytes:02x?} as {instruction:?}"
            );
        }

        for bytes in [
            &[0xff, 0xf0][..],       // register mode is not PUSH r/m
            &[0xf0, 0xff, 0x30][..], // LOCK
            &[0x64, 0xff, 0x30][..], // FS
            &[0x65, 0xff, 0x30][..], // GS
        ] {
            assert!(
                matches!(
                    Memory::new(bytes).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "{bytes:02x?}"
            );
        }
        let instruction = Memory::new(&[0x66, 0x48, 0xff, 0x30]).decode().unwrap();
        assert_eq!(instruction.operand_size, OperandSize::Qword);
        assert!(matches!(
            instruction.operation,
            Operation::PushMemory {
                source: MemoryOperand {
                    size: OperandSize::Qword,
                    ..
                }
            }
        ));
        for (bytes, expected_size) in [
            (&[0x66, 0x40, 0x48, 0xff, 0x30][..], OperandSize::Qword),
            (&[0x66, 0x48, 0x40, 0xff, 0x30][..], OperandSize::Word),
        ] {
            let instruction = Memory::new(bytes).decode().unwrap();
            assert_eq!(instruction.operand_size, expected_size, "{bytes:02x?}");
        }
    }

    #[test]
    fn memory_pop_decodes_qword_eas_and_rejects_other_8f_forms() {
        for bytes in [
            &[0x8f, 0x00][..],
            &[0x41, 0x8f, 0x00][..],
            &[0x8f, 0x04, 0x8b][..],
            &[0x8f, 0x05, 0, 0, 0, 0][..],
            &[0x67, 0x8f, 0x00][..],
            &[0x8f, 0x04, 0x24][..],
        ] {
            let mut memory = Memory::new(bytes);
            let instruction = memory.decode().unwrap();
            assert_eq!(instruction.operand_size, OperandSize::Qword);
            assert!(matches!(
                instruction.operation,
                Operation::PopMemory {
                    destination: MemoryOperand {
                        size: OperandSize::Qword,
                        ..
                    }
                }
            ));
        }
        for bytes in [
            &[0x66, 0x8f, 0x00][..],
            &[0x66, 0x41, 0x8f, 0x00][..],
            &[0x66, 0x8f, 0x04, 0x8b][..],
            &[0x66, 0x8f, 0x05, 0, 0, 0, 0][..],
            &[0x67, 0x66, 0x8f, 0x00][..],
            &[0x66, 0x8f, 0x04, 0x24][..],
        ] {
            let mut memory = Memory::new(bytes);
            let instruction = memory.decode().unwrap();
            assert_eq!(instruction.operand_size, OperandSize::Word);
            assert!(matches!(
                instruction.operation,
                Operation::PopMemory {
                    destination: MemoryOperand {
                        size: OperandSize::Word,
                        ..
                    }
                }
            ));
        }
        for bytes in [
            &[0x8f, 0x08][..], // /1
            &[0x8f, 0xc0][..], // register destination
            &[0x64, 0x8f, 0x00][..],
            &[0x65, 0x8f, 0x00][..],
        ] {
            let mut memory = Memory::new(bytes);
            assert!(matches!(
                memory.decode(),
                Err(DecodeError::Unsupported { .. })
            ));
        }
        let instruction = Memory::new(&[0x66, 0x48, 0x8f, 0x00]).decode().unwrap();
        assert_eq!(instruction.operand_size, OperandSize::Qword);
        assert!(matches!(
            instruction.operation,
            Operation::PopMemory {
                destination: MemoryOperand {
                    size: OperandSize::Qword,
                    ..
                }
            }
        ));
        for (bytes, expected_size) in [
            (&[0x66, 0x40, 0x48, 0x8f, 0x00][..], OperandSize::Qword),
            (&[0x66, 0x48, 0x40, 0x8f, 0x00][..], OperandSize::Word),
        ] {
            let instruction = Memory::new(bytes).decode().unwrap();
            assert_eq!(instruction.operand_size, expected_size, "{bytes:02x?}");
        }
        for bytes in [&[0x8f, 0x04][..], &[0x8f, 0x05, 0, 0][..]] {
            let mut memory = Memory::new(bytes);
            assert!(memory.decode().is_err());
        }
    }

    #[test]
    fn push_pop_opcode_register_forms_decode_word_and_qword_registers() {
        for code in 0x50..=0x5f {
            let bytes = [code];
            let instruction = Memory::new(&bytes).decode().unwrap();
            assert_eq!(instruction.operand_size, OperandSize::Qword);
            assert_eq!(
                instruction.operation,
                Operation::StackRegister {
                    push: code < 0x58,
                    register: Register::from_encoding(code & 7).unwrap(),
                }
            );

            let bytes = [0x41, code];
            let instruction = Memory::new(&bytes).decode().unwrap();
            assert_eq!(instruction.length, 2);
            assert_eq!(
                instruction.operation,
                Operation::StackRegister {
                    push: code < 0x58,
                    register: Register::from_encoding((code & 7) + 8).unwrap(),
                }
            );
        }
        for bytes in [&[0x48, 0x50][..], &[0x49, 0x5f][..]] {
            let instruction = Memory::new(bytes).decode().unwrap();
            assert_eq!(instruction.operand_size, OperandSize::Qword);
        }
        for (bytes, register, push) in [
            (&[0x66, 0x50][..], Register::Rax, true),
            (&[0x66, 0x41, 0x50][..], Register::R8, true),
            (&[0x66, 0x41, 0x59][..], Register::R9, false),
        ] {
            let instruction = Memory::new(bytes).decode().unwrap();
            assert_eq!(instruction.operand_size, OperandSize::Word);
            assert_eq!(
                instruction.operation,
                Operation::StackRegister { push, register }
            );
        }
        let instruction = Memory::new(&[0x66, 0x48, 0x50]).decode().unwrap();
        assert_eq!(instruction.operand_size, OperandSize::Qword);
        for (bytes, size, register, push) in [
            (
                &[0x66, 0x49, 0x50][..],
                OperandSize::Qword,
                Register::R8,
                true,
            ),
            (
                &[0x66, 0x48, 0x40, 0x50],
                OperandSize::Word,
                Register::Rax,
                true,
            ),
            (
                &[0x66, 0x49, 0x41, 0x59],
                OperandSize::Word,
                Register::R9,
                false,
            ),
            (&[0x66, 0x49, 0x59], OperandSize::Qword, Register::R9, false),
        ] {
            let instruction = Memory::new(bytes).decode().unwrap();
            assert_eq!(instruction.operand_size, size, "{bytes:02x?}");
            assert_eq!(
                instruction.operation,
                Operation::StackRegister { push, register },
                "{bytes:02x?}"
            );
        }
        for bytes in [&[0x67, 0x50][..], &[0x2e, 0x50][..], &[0xf3, 0x50][..]] {
            assert!(matches!(
                Memory::new(bytes).decode(),
                Err(DecodeError::Unsupported { .. })
            ));
        }
    }

    #[test]
    fn popf_decodes_word_and_qword_widths_from_66_and_effective_rex_w() {
        let instruction = Memory::new(&[0x9d]).decode().unwrap();
        assert_eq!(instruction.length, 1);
        assert_eq!(instruction.operand_size, OperandSize::Qword);
        assert_eq!(instruction.operation, Operation::PopFlags);
        assert_eq!(
            instruction.flag_effects(),
            FlagEffects {
                defined: crate::POPFQ_INTEGER_FLAGS,
                undefined: 0,
            }
        );

        let word = Memory::new(&[0x66, 0x9d]).decode().unwrap();
        assert_eq!(word.length, 2);
        assert_eq!(word.operand_size, OperandSize::Word);
        assert_eq!(word.operation, Operation::PopFlags);
        assert_eq!(word.flag_effects(), instruction.flag_effects());

        let rex_w = Memory::new(&[0x48, 0x9d]).decode().unwrap();
        assert_eq!(rex_w.operand_size, OperandSize::Qword);
        assert_eq!(rex_w.operation, Operation::PopFlags);
        let rex_w_overrides_66 = Memory::new(&[0x66, 0x48, 0x9d]).decode().unwrap();
        assert_eq!(rex_w_overrides_66.length, 3);
        assert_eq!(rex_w_overrides_66.operand_size, OperandSize::Qword);
        assert_eq!(rex_w_overrides_66.operation, Operation::PopFlags);

        let rex_without_w = Memory::new(&[0x66, 0x41, 0x9d]).decode().unwrap();
        assert_eq!(rex_without_w.operand_size, OperandSize::Word);
        assert_eq!(rex_without_w.operation, Operation::PopFlags);
        let final_rex_w = Memory::new(&[0x66, 0x40, 0x48, 0x9d]).decode().unwrap();
        assert_eq!(final_rex_w.operand_size, OperandSize::Qword);
        let final_rex_without_w = Memory::new(&[0x66, 0x48, 0x40, 0x9d]).decode().unwrap();
        assert_eq!(final_rex_without_w.operand_size, OperandSize::Word);

        for bytes in [&[0x67, 0x9d][..], &[0x2e, 0x9d][..], &[0xf3, 0x9d][..]] {
            assert!(
                matches!(
                    Memory::new(bytes).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "{bytes:02x?}"
            );
        }
    }

    #[test]
    fn pushf_decodes_word_and_qword_widths_from_66_and_effective_rex_w() {
        let instruction = Memory::new(&[0x9c]).decode().unwrap();
        assert_eq!(instruction.length, 1);
        assert_eq!(instruction.operand_size, OperandSize::Qword);
        assert_eq!(instruction.operation, Operation::PushFlags);
        assert_eq!(instruction.flag_effects(), FlagEffects::default());

        for bytes in [
            &[0x48, 0x9c][..],
            &[0x66, 0x48, 0x9c][..],
            &[0x66, 0x4f, 0x9c][..],
        ] {
            let instruction = Memory::new(bytes).decode().unwrap();
            assert_eq!(instruction.operand_size, OperandSize::Qword, "{bytes:02x?}");
            assert_eq!(instruction.operation, Operation::PushFlags, "{bytes:02x?}");
        }
        let rex_without_w = Memory::new(&[0x66, 0x41, 0x9c]).decode().unwrap();
        assert_eq!(rex_without_w.operand_size, OperandSize::Word);
        let final_rex_w = Memory::new(&[0x66, 0x40, 0x48, 0x9c]).decode().unwrap();
        assert_eq!(final_rex_w.operand_size, OperandSize::Qword);
        let final_rex_without_w = Memory::new(&[0x66, 0x48, 0x40, 0x9c]).decode().unwrap();
        assert_eq!(final_rex_without_w.operand_size, OperandSize::Word);

        for bytes in [&[0x67, 0x9c][..], &[0x2e, 0x9c][..], &[0xf3, 0x9c][..]] {
            assert!(
                matches!(
                    Memory::new(bytes).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "{bytes:02x?}"
            );
        }
    }

    #[test]
    fn pushfw_decodes_when_effective_rex_w_is_clear_and_rex_w_overrides_66() {
        let instruction = Memory::new(&[0x66, 0x9c]).decode().unwrap();
        assert_eq!(instruction.length, 2);
        assert_eq!(instruction.operand_size, OperandSize::Word);
        assert_eq!(instruction.operation, Operation::PushFlags);

        // A REX before a legacy prefix is architecturally ignored by the
        // shared prefix parser, so this still decodes as the word form.
        let ignored_rex = Memory::new(&[0x48, 0x66, 0x9c]).decode().unwrap();
        assert_eq!(ignored_rex.length, 3);
        assert_eq!(ignored_rex.operand_size, OperandSize::Word);

        let rex_w = Memory::new(&[0x66, 0x48, 0x9c]).decode().unwrap();
        assert_eq!(rex_w.length, 3);
        assert_eq!(rex_w.operand_size, OperandSize::Qword);
        assert_eq!(rex_w.operation, Operation::PushFlags);

        for bytes in [
            &[0x67, 0x66, 0x9c][..],
            &[0x2e, 0x66, 0x9c][..],
            &[0xf3, 0x66, 0x9c][..],
        ] {
            assert!(
                matches!(
                    Memory::new(bytes).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "{bytes:02x?}"
            );
        }
    }

    #[test]
    fn bare_a5_decodes_as_dword_and_prefix_parser_normalizes_rex() {
        let instruction = Memory::new(&[0xa5]).decode().unwrap();
        assert_eq!(instruction.length, 1);
        assert_eq!(instruction.operand_size, OperandSize::Dword);
        assert_eq!(
            instruction.operation,
            Operation::Movs {
                size: OperandSize::Dword
            }
        );

        let ignored_rex = Memory::new(&[0x48, 0x66, 0xa5]).decode().unwrap();
        assert!(ignored_rex.prefixes.operand_override);
        assert_eq!(ignored_rex.prefixes.rex, None);
        assert_eq!(ignored_rex.operand_size, OperandSize::Word);

        let effective_rex = Memory::new(&[0x48, 0xa5]).decode().unwrap();
        assert_eq!(effective_rex.prefixes.rex, Some(0x48));
        assert_eq!(effective_rex.operand_size, OperandSize::Qword);

        // In long mode an effective REX.W overrides 66 for non-byte forms.
        let rex_w_overrides_66 = Memory::new(&[0x66, 0x48, 0xa5]).decode().unwrap();
        assert!(rex_w_overrides_66.prefixes.operand_override);
        assert_eq!(rex_w_overrides_66.prefixes.rex, Some(0x48));
        assert_eq!(rex_w_overrides_66.operand_size, OperandSize::Qword);
        assert_eq!(
            rex_w_overrides_66.operation,
            Operation::Movs {
                size: OperandSize::Qword
            }
        );

        let address_override = Memory::new(&[0x67, 0xa5]).decode().unwrap();
        assert!(address_override.prefixes.address_override);
        let segment_override = Memory::new(&[0x2e, 0xa5]).decode().unwrap();
        assert_eq!(segment_override.prefixes.segment, Some(0x2e));
        assert!(matches!(
            Memory::new(&[0xf3, 0xa5]).decode(),
            Err(DecodeError::Unsupported { .. })
        ));
    }

    #[test]
    fn push_immediate_forms_sign_extend_to_the_selected_stack_width() {
        for (bytes, expected) in [
            (&[0x6a, 0x7f][..], 0x0000_0000_0000_007f),
            (&[0x6a, 0x80][..], 0xffff_ffff_ffff_ff80),
            (&[0x6a, 0xff][..], 0xffff_ffff_ffff_ffff),
            (&[0x68, 0xff, 0xff, 0xff, 0x7f][..], 0x0000_0000_7fff_ffff),
            (&[0x68, 0, 0, 0, 0x80][..], 0xffff_ffff_8000_0000),
            (&[0x68, 0xff, 0xff, 0xff, 0xff][..], 0xffff_ffff_ffff_ffff),
        ] {
            let instruction = Memory::new(bytes).decode().unwrap();
            assert_eq!(instruction.operand_size, OperandSize::Qword);
            assert_eq!(
                instruction.operation,
                Operation::PushImmediate { value: expected }
            );
        }
        for (bytes, expected) in [
            (&[0x66, 0x6a, 0][..], 0x0000),
            (&[0x66, 0x6a, 0x7f][..], 0x007f),
            (&[0x66, 0x6a, 0x80][..], 0xff80),
            (&[0x66, 0x6a, 0xff][..], 0xffff),
            (&[0x66, 0x68, 0x00, 0x00][..], 0x0000),
            (&[0x66, 0x68, 0xff, 0x7f][..], 0x7fff),
            (&[0x66, 0x68, 0x00, 0x80][..], 0x8000),
            (&[0x66, 0x68, 0xff, 0xff][..], 0xffff),
        ] {
            let instruction = Memory::new(bytes).decode().unwrap();
            assert_eq!(instruction.operand_size, OperandSize::Word);
            assert_eq!(
                instruction.operation,
                Operation::PushImmediate { value: expected }
            );
        }
        for bytes in [&[0x66, 0x6a][..], &[0x66, 0x68, 0x34][..]] {
            assert!(matches!(
                Memory::new(bytes).decode(),
                Err(DecodeError::Memory(_))
            ));
        }
    }

    #[test]
    fn setcc_decodes_byte_registers_memory_addresses_and_ignored_modrm_reg() {
        for encoding in 0..16 {
            let bytes = [0x0f, 0x90 + encoding, 0xd2]; // ModRM.reg is ignored; destination is DL.
            let instruction = Memory::new(&bytes).decode().unwrap();
            assert_eq!(instruction.operand_size, OperandSize::Byte);
            assert_eq!(instruction.length, 3);
            assert_eq!(
                instruction.operation,
                Operation::SetCondition {
                    condition: ConditionCode::from_encoding(encoding).unwrap(),
                    dst: Operand::Register(RegisterView::low(Register::Rdx, OperandSize::Byte)),
                }
            );
        }

        let high = Memory::new(&[0x0f, 0x94, 0xc4]).decode().unwrap();
        assert_eq!(
            high.operation,
            Operation::SetCondition {
                condition: ConditionCode::Equal,
                dst: Operand::Register(RegisterView::high_byte(Register::Rax, false).unwrap()),
            }
        );
        let rex_low = Memory::new(&[0x40, 0x0f, 0x94, 0xc4]).decode().unwrap();
        assert_eq!(
            rex_low.operation,
            Operation::SetCondition {
                condition: ConditionCode::Equal,
                dst: Operand::Register(RegisterView::low(Register::Rsp, OperandSize::Byte)),
            }
        );
        let extended = Memory::new(&[0x41, 0x0f, 0x95, 0xc7]).decode().unwrap();
        assert_eq!(
            extended.operation,
            Operation::SetCondition {
                condition: ConditionCode::NotEqual,
                dst: Operand::Register(RegisterView::low(Register::R15, OperandSize::Byte)),
            }
        );

        let memory = Memory::new(&[0x67, 0x66, 0x0f, 0x94, 0x45, 0x80])
            .decode()
            .unwrap();
        assert_eq!(memory.length, 6);
        assert_eq!(memory.operand_size, OperandSize::Byte);
        assert_eq!(memory.address_size, OperandSize::Dword);
        assert_eq!(
            memory.operation,
            Operation::SetCondition {
                condition: ConditionCode::Equal,
                dst: Operand::Memory(MemoryOperand {
                    base: AddressBase::Register(Register::Rbp),
                    index: None,
                    scale: 1,
                    displacement: -128,
                    address_size: OperandSize::Dword,
                    size: OperandSize::Byte,
                }),
            }
        );
        let rip_relative = Memory::new(&[0x0f, 0x94, 0x05, 0, 0, 0, 0])
            .decode()
            .unwrap();
        let Operand::Memory(address) = (match rip_relative.operation {
            Operation::SetCondition { dst, .. } => dst,
            _ => unreachable!(),
        }) else {
            unreachable!()
        };
        assert_eq!(address.base, AddressBase::NextRip);
        assert_eq!(address.size, OperandSize::Byte);
    }

    #[test]
    fn setcc_prefix_boundaries_and_fetch_faults_are_explicit() {
        for code in [
            &[0xf0, 0x0f, 0x94, 0xc0][..],
            &[0xf2, 0x0f, 0x94, 0xc0][..],
            &[0xf3, 0x0f, 0x94, 0xc0][..],
            &[0x64, 0x0f, 0x94, 0x00][..],
            &[0x65, 0x0f, 0x94, 0x00][..],
        ] {
            assert!(matches!(
                Memory::new(code).decode(),
                Err(DecodeError::Unsupported { .. })
            ));
        }
        for bytes in [
            vec![0x0f, 0x94, 0xc0],
            vec![0x0f, 0x94, 0x45, 0x7f],
            vec![0x0f, 0x94, 0x05, 0, 0, 0, 0],
        ] {
            for cut in 0..bytes.len() {
                let mut memory = Memory::new(&bytes[..cut]);
                assert!(matches!(memory.decode(), Err(DecodeError::Memory(_))));
                assert_eq!(memory.fetched.len(), cut + 1);
            }
        }
    }

    #[test]
    fn cmovcc_decodes_all_conditions_widths_and_r_m_sources() {
        for encoding in 0..16 {
            let bytes = [0x0f, 0x40 + encoding, 0xdb]; // CMOVcc EBX, EBX
            let instruction = Memory::new(&bytes).decode().unwrap();
            assert_eq!(instruction.operand_size, OperandSize::Dword);
            assert_eq!(
                instruction.operation,
                Operation::ConditionalMove {
                    condition: ConditionCode::from_encoding(encoding).unwrap(),
                    dst: RegisterView::low(Register::Rbx, OperandSize::Dword),
                    src: Operand::Register(RegisterView::low(Register::Rbx, OperandSize::Dword)),
                }
            );
        }

        for (prefixes, size) in [
            (vec![0x66], OperandSize::Word),
            (vec![0x48], OperandSize::Qword),
            (vec![0x4d], OperandSize::Qword),
        ] {
            let extended_source = prefixes[0] == 0x4d;
            let mut bytes = prefixes;
            bytes.extend_from_slice(&[0x0f, 0x44, if extended_source { 0xd3 } else { 0x03 }]); // CMOVE RAX, [RBX] / CMOVE R10, R11
            let instruction = Memory::new(&bytes).decode().unwrap();
            assert_eq!(instruction.operand_size, size);
            let Operation::ConditionalMove { dst, src, .. } = instruction.operation else {
                panic!("expected CMOVcc")
            };
            assert_eq!(dst.size(), size);
            assert_eq!(
                src,
                if extended_source {
                    Operand::Register(RegisterView::low(Register::R11, size))
                } else {
                    Operand::Memory(MemoryOperand {
                        base: AddressBase::Register(Register::Rbx),
                        index: None,
                        scale: 1,
                        displacement: 0,
                        address_size: OperandSize::Qword,
                        size,
                    })
                }
            );
        }

        let address32 = Memory::new(&[0x67, 0x0f, 0x45, 0x03]).decode().unwrap();
        assert_eq!(address32.address_size, OperandSize::Dword);
        let rip_relative = Memory::new(&[0x0f, 0x45, 0x0d, 0, 0, 0, 0])
            .decode()
            .unwrap();
        assert!(matches!(
            rip_relative.operation,
            Operation::ConditionalMove {
                src: Operand::Memory(MemoryOperand {
                    base: AddressBase::NextRip,
                    size: OperandSize::Dword,
                    ..
                }),
                ..
            }
        ));
    }

    #[test]
    fn cmovcc_prefix_boundaries_are_explicit() {
        for bytes in [
            &[0xf0, 0x0f, 0x44, 0xc0][..],
            &[0xf2, 0x0f, 0x44, 0xc0][..],
            &[0xf3, 0x0f, 0x44, 0xc0][..],
            &[0x64, 0x0f, 0x44, 0x00][..],
            &[0x65, 0x0f, 0x44, 0x00][..],
        ] {
            assert!(matches!(
                Memory::new(bytes).decode(),
                Err(DecodeError::Unsupported { .. })
            ));
        }
    }

    #[test]
    fn sizes_rex_precedence_and_extended_registers() {
        for (prefix, size, address) in [
            (vec![], OperandSize::Dword, OperandSize::Qword),
            (vec![0x66], OperandSize::Word, OperandSize::Qword),
            (
                vec![0x66, 0x67, 0x49],
                OperandSize::Qword,
                OperandSize::Dword,
            ),
            (vec![0x48, 0x66], OperandSize::Word, OperandSize::Qword),
            (vec![0x48, 0x40], OperandSize::Dword, OperandSize::Qword),
        ] {
            let extended = prefix.last().is_some_and(|byte| byte & 0xf1 == 0x41);
            let mut bytes = prefix;
            bytes.push(0xb8);
            bytes.extend(vec![0xff; size.bytes()]);
            let mut memory = Memory::new(&bytes);
            let result = memory.decode().unwrap();
            assert_eq!(result.length as usize, bytes.len());
            assert_eq!((result.operand_size, result.address_size), (size, address));
            let Operation::MovImmediate {
                dst: Operand::Register(dst),
                value,
            } = result.operation
            else {
                panic!()
            };
            assert_eq!(
                dst.register(),
                if extended {
                    Register::R8
                } else {
                    Register::Rax
                }
            );
            assert_eq!(value, size.mask());
        }
        let mut memory = Memory::new(&[0x4d, 0x03, 0xc7]);
        let Operation::Arithmetic { dst, src, .. } = memory.decode().unwrap().operation else {
            panic!()
        };
        assert_eq!(
            dst,
            Operand::Register(RegisterView::low(Register::R8, OperandSize::Qword))
        );
        assert_eq!(
            src,
            Operand::Register(RegisterView::low(Register::R15, OperandSize::Qword))
        );
    }

    #[test]
    fn sign_extension_accumulator_opcodes_use_effective_operand_size() {
        for (bytes, size, double_width) in [
            (&[0x98][..], OperandSize::Dword, false),
            (&[0x66, 0x98][..], OperandSize::Word, false),
            (&[0x48, 0x98][..], OperandSize::Qword, false),
            (&[0x66, 0x48, 0x98][..], OperandSize::Qword, false),
            (&[0x48, 0x66, 0x98][..], OperandSize::Word, false),
            (&[0x99][..], OperandSize::Dword, true),
            (&[0x66, 0x99][..], OperandSize::Word, true),
            (&[0x48, 0x99][..], OperandSize::Qword, true),
        ] {
            let instruction = Memory::new(bytes).decode().unwrap();
            assert_eq!(instruction.length as usize, bytes.len());
            assert_eq!(instruction.operand_size, size);
            assert_eq!(
                instruction.operation,
                Operation::SignExtendAccumulator { double_width }
            );
            assert_eq!(instruction.flag_effects(), FlagEffects::default());
        }
    }

    #[test]
    fn bswap_decodes_opcode_register_and_long_mode_operand_size_rules() {
        for (bytes, size, register) in [
            (&[0x0f, 0xc8][..], OperandSize::Dword, Register::Rax),
            (&[0x0f, 0xcf][..], OperandSize::Dword, Register::Rdi),
            (&[0x41, 0x0f, 0xc8][..], OperandSize::Dword, Register::R8),
            (&[0x4f, 0x0f, 0xcf][..], OperandSize::Qword, Register::R15),
            (&[0x48, 0x0f, 0xcc][..], OperandSize::Qword, Register::Rsp),
            (&[0x44, 0x0f, 0xc9][..], OperandSize::Dword, Register::Rcx),
            (
                &[0x66, 0x49, 0x0f, 0xc9][..],
                OperandSize::Qword,
                Register::R9,
            ),
        ] {
            let instruction = Memory::new(bytes).decode().unwrap();
            assert_eq!(instruction.length as usize, bytes.len());
            assert_eq!(instruction.operand_size, size);
            assert_eq!(
                instruction.operation,
                Operation::Bswap {
                    dst: RegisterView::low(register, size)
                }
            );
            assert_eq!(instruction.flag_effects(), FlagEffects::default());
        }
        for bytes in [&[0x66, 0x0f, 0xc8][..], &[0x66, 0x0f, 0xcf][..]] {
            assert!(matches!(
                Memory::new(bytes).decode(),
                Err(DecodeError::Unsupported { .. })
            ));
        }
        assert!(matches!(
            Memory::new(&[0x48, 0x66, 0x0f, 0xc9]).decode(),
            Err(DecodeError::Unsupported { .. })
        ));
    }

    #[test]
    fn bit_register_forms_decode_register_indices_widths_and_rex_extensions() {
        for (bytes, kind, size, dst, offset) in [
            (
                &[0x0f, 0xa3, 0xc8][..],
                BitKind::Test,
                OperandSize::Dword,
                Register::Rax,
                Register::Rcx,
            ),
            (
                &[0x66, 0x0f, 0xab, 0xd3][..],
                BitKind::Set,
                OperandSize::Word,
                Register::Rbx,
                Register::Rdx,
            ),
            (
                &[0x48, 0x0f, 0xb3, 0xe5][..],
                BitKind::Reset,
                OperandSize::Qword,
                Register::Rbp,
                Register::Rsp,
            ),
            (
                &[0x45, 0x0f, 0xbb, 0xd1][..],
                BitKind::Complement,
                OperandSize::Dword,
                Register::R9,
                Register::R10,
            ),
            (
                &[0x66, 0x45, 0x0f, 0xa3, 0xff][..],
                BitKind::Test,
                OperandSize::Word,
                Register::R15,
                Register::R15,
            ),
            (
                &[0x66, 0x48, 0x0f, 0xab, 0xc8][..],
                BitKind::Set,
                OperandSize::Qword,
                Register::Rax,
                Register::Rcx,
            ),
        ] {
            let instruction = Memory::new(bytes).decode().unwrap();
            assert_eq!(instruction.length as usize, bytes.len());
            assert_eq!(instruction.operand_size, size);
            assert_eq!(
                instruction.operation,
                Operation::Bit {
                    kind,
                    dst: Operand::Register(RegisterView::low(dst, size)),
                    offset: BitOffset::Register(RegisterView::low(offset, size)),
                }
            );
            assert_eq!(
                instruction.flag_effects(),
                FlagEffects {
                    defined: crate::CF,
                    undefined: crate::PF | crate::AF | crate::SF | crate::OF,
                }
            );
        }
    }

    #[test]
    fn bit_register_forms_decode_memory_and_reject_immediate_and_lock_boundaries() {
        for opcode in [0xa3, 0xab, 0xb3, 0xbb] {
            let instruction = Memory::new(&[0x0f, opcode, 0x08]).decode().unwrap();
            let Operation::Bit { dst, offset, .. } = instruction.operation else {
                panic!("expected BT-family operation")
            };
            assert_eq!(
                dst,
                Operand::Memory(MemoryOperand {
                    base: AddressBase::Register(Register::Rax),
                    index: None,
                    scale: 1,
                    displacement: 0,
                    address_size: OperandSize::Qword,
                    size: OperandSize::Dword,
                })
            );
            assert_eq!(
                offset,
                BitOffset::Register(RegisterView::low(Register::Rcx, OperandSize::Dword))
            );
        }
        for bytes in [
            &[0x0f, 0xba, 0x18, 0][..],    // /3 remains outside the immediate group
            &[0x0f, 0xba, 0xd8, 0][..],    // register immediate /3
            &[0xf0, 0x0f, 0xab, 0xc8][..], // LOCK register form
            &[0xf0, 0x0f, 0xa3, 0x08][..], // LOCK memory form
            &[0xf2, 0x0f, 0xa3, 0x08][..],
            &[0xf3, 0x0f, 0xbb, 0x08][..],
            &[0xf0, 0x0f, 0xba, 0x20, 0][..],
            &[0xf2, 0x0f, 0xba, 0xe0, 0][..],
            &[0xf3, 0x0f, 0xba, 0xe0, 0][..],
        ] {
            assert!(matches!(
                Memory::new(bytes).decode(),
                Err(DecodeError::Unsupported { .. })
            ));
        }
    }

    #[test]
    fn bit_register_memory_forms_decode_rex_sib_displacements_and_address_override() {
        let cases = [
            // BTS qword [R12 + R13*4 - 16], R14; exercises REX.R/X/B/W.
            (
                &[0x4f, 0x0f, 0xab, 0x74, 0xac, 0xf0][..],
                BitKind::Set,
                OperandSize::Qword,
                AddressBase::Register(Register::R12),
                Some(Register::R13),
                4,
                -16,
                Register::R14,
            ),
            // BTC dword [RIP + disp32], R9; next_rip includes all disp bytes.
            (
                &[0x45, 0x0f, 0xbb, 0x0d, 0x10, 0, 0, 0][..],
                BitKind::Complement,
                OperandSize::Dword,
                AddressBase::NextRip,
                None,
                1,
                16,
                Register::R9,
            ),
            // BT word [EAX + ECX*2 + disp8], DX, with 67 address wrapping.
            (
                &[0x67, 0x66, 0x0f, 0xa3, 0x54, 0x48, 0x7f][..],
                BitKind::Test,
                OperandSize::Word,
                AddressBase::Register(Register::Rax),
                Some(Register::Rcx),
                2,
                127,
                Register::Rdx,
            ),
        ];
        for (bytes, kind, size, base, index, scale, displacement, offset) in cases {
            let instruction = Memory::new(bytes).decode().unwrap();
            assert_eq!(instruction.length as usize, bytes.len());
            assert_eq!(instruction.operand_size, size);
            assert_eq!(
                instruction.operation,
                Operation::Bit {
                    kind,
                    dst: Operand::Memory(MemoryOperand {
                        base,
                        index,
                        scale,
                        displacement,
                        address_size: if bytes[0] == 0x67 {
                            OperandSize::Dword
                        } else {
                            OperandSize::Qword
                        },
                        size,
                    }),
                    offset: BitOffset::Register(RegisterView::low(offset, size)),
                }
            );
        }
    }

    #[test]
    fn bit_immediate_register_forms_decode_groups_widths_and_extensions() {
        for (bytes, kind, size, dst, immediate) in [
            (
                &[0x0f, 0xba, 0xe0, 0xff][..],
                BitKind::Test,
                OperandSize::Dword,
                Register::Rax,
                0xff,
            ),
            (
                &[0x66, 0x41, 0x0f, 0xba, 0xe9, 0x0f][..],
                BitKind::Set,
                OperandSize::Word,
                Register::R9,
                15,
            ),
            (
                &[0x41, 0x0f, 0xba, 0xf1, 0x20][..],
                BitKind::Reset,
                OperandSize::Dword,
                Register::R9,
                32,
            ),
            (
                &[0x49, 0x0f, 0xba, 0xf9, 0x3f][..],
                BitKind::Complement,
                OperandSize::Qword,
                Register::R9,
                63,
            ),
        ] {
            let instruction = Memory::new(bytes).decode().unwrap();
            assert_eq!(instruction.length as usize, bytes.len());
            assert_eq!(
                instruction.operation,
                Operation::Bit {
                    kind,
                    dst: Operand::Register(RegisterView::low(dst, size)),
                    offset: BitOffset::Immediate(immediate),
                }
            );
            assert_eq!(
                instruction.flag_effects(),
                FlagEffects {
                    defined: crate::CF,
                    undefined: crate::PF | crate::AF | crate::SF | crate::OF,
                }
            );
        }

        assert!(matches!(
            Memory::new(&[0x0f, 0xba, 0xe0]).decode(),
            Err(DecodeError::Memory(MemoryError::Unmapped {
                address: 0x1002,
                access: crate::memory::AccessKind::Fetch,
            }))
        ));
    }

    #[test]
    fn bit_immediate_memory_forms_decode_full_address_before_immediate() {
        for (bytes, kind, size, expected, immediate) in [
            (
                &[0x0f, 0xba, 0x20, 0xff][..],
                BitKind::Test,
                OperandSize::Dword,
                MemoryOperand {
                    base: AddressBase::Register(Register::Rax),
                    index: None,
                    scale: 1,
                    displacement: 0,
                    address_size: OperandSize::Qword,
                    size: OperandSize::Dword,
                },
                0xff,
            ),
            (
                &[0x66, 0x0f, 0xba, 0x69, 0x7f, 0x10][..],
                BitKind::Set,
                OperandSize::Word,
                MemoryOperand {
                    base: AddressBase::Register(Register::Rcx),
                    index: None,
                    scale: 1,
                    displacement: 127,
                    address_size: OperandSize::Qword,
                    size: OperandSize::Word,
                },
                0x10,
            ),
            (
                &[0x49, 0x0f, 0xba, 0xbc, 0x24, 0x78, 0x56, 0x34, 0x12, 0x80][..],
                BitKind::Complement,
                OperandSize::Qword,
                MemoryOperand {
                    base: AddressBase::Register(Register::R12),
                    index: None,
                    scale: 1,
                    displacement: 0x1234_5678,
                    address_size: OperandSize::Qword,
                    size: OperandSize::Qword,
                },
                0x80,
            ),
            (
                &[0x0f, 0xba, 0x3d, 0x10, 0, 0, 0, 0xff][..],
                BitKind::Complement,
                OperandSize::Dword,
                MemoryOperand {
                    base: AddressBase::NextRip,
                    index: None,
                    scale: 1,
                    displacement: 0x10,
                    address_size: OperandSize::Qword,
                    size: OperandSize::Dword,
                },
                0xff,
            ),
        ] {
            let instruction = Memory::new(bytes).decode().unwrap();
            assert_eq!(instruction.length as usize, bytes.len());
            assert_eq!(instruction.operand_size, size);
            assert_eq!(
                instruction.operation,
                Operation::Bit {
                    kind,
                    dst: Operand::Memory(expected),
                    offset: BitOffset::Immediate(immediate),
                }
            );
        }

        // RIP-relative EA uses next_rip after the immediate byte.
        let instruction = Memory::new(&[0x0f, 0xba, 0x3d, 0x10, 0, 0, 0, 0xff])
            .decode()
            .unwrap();
        let destination = match instruction.operation {
            Operation::Bit { dst, .. } => dst,
            _ => unreachable!(),
        };
        let Operand::Memory(operand) = destination else {
            unreachable!()
        };
        let state = crate::LongModeState::default();
        assert_eq!(
            operand.effective_address(&state, instruction.length as u64),
            24
        );

        assert!(matches!(
            Memory::new(&[0x0f, 0xba, 0x3d, 0x10, 0, 0, 0]).decode(),
            Err(DecodeError::Memory(MemoryError::Unmapped {
                address: 0x1006,
                access: crate::memory::AccessKind::Fetch,
            }))
        ));
    }

    #[test]
    fn bit_scan_register_forms_decode_sizes_and_rex_registers() {
        use BitScanKind::{Forward, Reverse};
        for (bytes, kind, size, dst, src) in [
            (
                &[0x66, 0x0f, 0xbc, 0xc1][..],
                Forward,
                OperandSize::Word,
                Register::Rax,
                Register::Rcx,
            ),
            (
                &[0x0f, 0xbd, 0xd3][..],
                Reverse,
                OperandSize::Dword,
                Register::Rdx,
                Register::Rbx,
            ),
            (
                &[0x48, 0x0f, 0xbc, 0xc1][..],
                Forward,
                OperandSize::Qword,
                Register::Rax,
                Register::Rcx,
            ),
            (
                &[0x45, 0x0f, 0xbc, 0xc1][..],
                Forward,
                OperandSize::Dword,
                Register::R8,
                Register::R9,
            ),
            (
                &[0x4d, 0x0f, 0xbd, 0xc1][..],
                Reverse,
                OperandSize::Qword,
                Register::R8,
                Register::R9,
            ),
        ] {
            let instruction = Memory::new(bytes).decode().unwrap();
            assert_eq!(instruction.length as usize, bytes.len());
            assert_eq!(instruction.operand_size, size);
            assert_eq!(
                instruction.operation,
                Operation::BitScan {
                    kind,
                    dst: RegisterView::low(dst, size),
                    src: Operand::Register(RegisterView::low(src, size)),
                }
            );
            assert_eq!(
                instruction.flag_effects(),
                FlagEffects {
                    defined: crate::ARITHMETIC_FLAGS,
                    undefined: 0
                }
            );
        }
    }

    #[test]
    fn bit_scan_memory_forms_decode_addressing_and_f3_remains_unsupported() {
        for (bytes, kind, size, dst, base, index, scale, displacement) in [
            (
                &[0x0f, 0xbc, 0x4d, 0x08][..],
                BitScanKind::Forward,
                OperandSize::Dword,
                Register::Rcx,
                Register::Rbp,
                None,
                1,
                8,
            ),
            (
                &[0x66, 0x0f, 0xbd, 0x0c, 0x8b][..],
                BitScanKind::Reverse,
                OperandSize::Word,
                Register::Rcx,
                Register::Rbx,
                Some(Register::Rcx),
                4,
                0,
            ),
            (
                &[0x4d, 0x0f, 0xbc, 0x4c, 0x8b, 0x10][..],
                BitScanKind::Forward,
                OperandSize::Qword,
                Register::R9,
                Register::R11,
                Some(Register::Rcx),
                4,
                16,
            ),
        ] {
            let instruction = Memory::new(bytes).decode().unwrap();
            assert_eq!(instruction.length as usize, bytes.len());
            assert_eq!(
                instruction.operation,
                Operation::BitScan {
                    kind,
                    dst: RegisterView::low(dst, size),
                    src: Operand::Memory(MemoryOperand {
                        base: AddressBase::Register(base),
                        index,
                        scale,
                        displacement,
                        address_size: OperandSize::Qword,
                        size,
                    }),
                }
            );
        }
        for bytes in [&[0xf3, 0x0f, 0xbc, 0xc1][..], &[0xf3, 0x0f, 0xbd, 0xc1][..]] {
            assert!(
                matches!(
                    Memory::new(bytes).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "{bytes:02x?}"
            );
        }
    }

    #[test]
    fn byte_aliases_and_byte_width_ignore_w_and_66() {
        for (prefix, expected, high) in [
            (vec![], Register::Rax, true),
            (vec![0x40], Register::Rsp, false),
            (vec![0x66, 0x48], Register::Rsp, false),
            (vec![0x41], Register::R12, false),
        ] {
            let mut bytes = prefix;
            bytes.extend([0xb4, 0xaa]);
            let mut memory = Memory::new(&bytes);
            let decoded = memory.decode().unwrap();
            assert_eq!(decoded.operand_size, OperandSize::Byte);
            let Operation::MovImmediate {
                dst: Operand::Register(dst),
                value,
            } = decoded.operation
            else {
                panic!()
            };
            assert_eq!(dst.register(), expected);
            let mut state = crate::LongModeState::default();
            state.write(dst, value);
            assert_eq!(
                state.gpr[expected as usize],
                if high { 0xaa00 } else { 0xaa }
            );
        }
    }

    #[test]
    fn fifteen_byte_limit_checks_before_sixteenth_fetch() {
        // Earlier REX bytes have no effect; only the final 48 immediately
        // preceding B8 selects imm64. This produces exactly 15 valid bytes.
        let mut bytes = vec![0x40; 5];
        bytes.extend([0x48, 0xb8, 1, 2, 3, 4, 5, 6, 7, 8]);
        let mut memory = Memory::new(&bytes);
        assert_eq!(memory.decode().unwrap().length, 15);
        assert_eq!(memory.fetched.len(), 15);
        for bytes in [vec![0x40; 16], {
            let mut b = bytes;
            b.insert(0, 0x40);
            b
        }] {
            let mut memory = Memory::new(&bytes);
            memory.fault_at = Some(15);
            assert_eq!(
                memory.decode(),
                Err(DecodeError::Memory(general_protection()))
            );
            assert_eq!(memory.fetched.len(), 15);
        }
    }

    #[test]
    fn unsupported_is_not_invalid_and_maps_do_not_alias() {
        let mut fs_memory = Memory::new(&[0x64, 0x48, 0x8b, 0x00]);
        let fs_load = fs_memory.decode().unwrap();
        assert!(matches!(
            fs_load.operation,
            Operation::MovFsLoad { dst, src }
                if dst == RegisterView::low(Register::Rax, OperandSize::Qword)
                    && src.base == AddressBase::Register(Register::Rax)
        ));
        for (bytes, length, map) in [
            (vec![0x66, 0x66, 0xb8], 3, OpcodeMap::Primary),
            (vec![0xf2, 0xf3, 0xb8], 3, OpcodeMap::Primary),
            (vec![0x26, 0x64, 0xb8], 3, OpcodeMap::Primary),
            (vec![0x67, 0x67, 0xb8], 3, OpcodeMap::Primary),
            (vec![0xf3, 0x0f, 0x10], 3, OpcodeMap::Escape0f),
            (vec![0x66, 0x0f, 0x38, 0xb8], 4, OpcodeMap::Escape0f38),
            (vec![0x0f, 0x3a, 0xb8], 3, OpcodeMap::Escape0f3a),
            (vec![0xf0, 0xb8], 2, OpcodeMap::Primary),
            (vec![0xf0, 0x03], 2, OpcodeMap::Primary),
        ] {
            let mut memory = Memory::new(&bytes);
            assert!(
                matches!(memory.decode(), Err(DecodeError::Unsupported { opcode: Opcode { map: actual, .. }, length: actual_length }) if actual == map && actual_length == length)
            );
            assert_eq!(memory.fetched.len(), length as usize);
        }
        for bytes in [&[0x0f, 0x0b][..]] {
            let mut memory = Memory::new(bytes);
            assert!(matches!(memory.decode(), Err(DecodeError::Invalid { .. })));
        }
    }

    #[test]
    fn gs_base_profile_admits_only_exact_qword_rax_load() {
        let bytes = [0x65, 0x48, 0x8b, 0x00];
        let mut memory = Memory::new(&bytes);
        assert!(matches!(
            memory.decode().unwrap().operation,
            Operation::MovGsLoad { dst, src }
                if dst == RegisterView::low(Register::Rax, OperandSize::Qword)
                    && src.base == AddressBase::Register(Register::Rax)
        ));
        for bytes in [
            &[0x65, 0x48, 0x89, 0x00][..],
            &[0x65, 0x48, 0x8b, 0x08][..],
            &[0x65, 0x48, 0x8b, 0x40, 0x00][..],
            &[0x65, 0x48, 0x8b, 0x04, 0x08][..],
            &[0x65, 0x67, 0x48, 0x8b, 0x00][..],
        ] {
            let mut memory = Memory::new(bytes);
            assert!(matches!(
                memory.decode(),
                Err(DecodeError::Unsupported { .. })
            ));
        }
        for length in 1..bytes.len() {
            let mut memory = Memory::new(&bytes[..length]);
            assert!(matches!(
                memory.decode(),
                Err(DecodeError::Memory(MemoryError::Unmapped {
                    access: AccessKind::Fetch,
                    ..
                }))
            ));
        }
    }

    #[test]
    fn fs_selector_read_and_gs_selector_read_admit_only_exact_forms() {
        let mut memory = Memory::new(&[0x48, 0x8c, 0xe0]);
        assert!(matches!(
            memory.decode().unwrap().operation,
            Operation::MovFsSelectorToRax
        ));
        let mut memory = Memory::new(&[0x48, 0x8c, 0xe8]);
        assert!(matches!(
            memory.decode().unwrap().operation,
            Operation::MovGsSelectorToRax
        ));
        for bytes in [
            &[0x8c, 0xe0][..],
            &[0x48, 0x8c, 0xc0][..],
            &[0x48, 0x8c, 0xe1][..],
            &[0x49, 0x8c, 0xe8][..],
            &[0x48, 0x8c, 0x20][..],
            &[0x48, 0x8c, 0x28][..],
            &[0x66, 0x48, 0x8c, 0xe0][..],
            &[0x66, 0x48, 0x8c, 0xe8][..],
            &[0x64, 0x48, 0x8c, 0xe0][..],
            &[0x65, 0x48, 0x8c, 0xe8][..],
            &[0xf3, 0x48, 0x8c, 0xe0][..],
            &[0xf3, 0x48, 0x8c, 0xe8][..],
        ] {
            let mut memory = Memory::new(bytes);
            assert!(matches!(
                memory.decode(),
                Err(DecodeError::Unsupported { .. })
            ));
        }
        for length in 1..3 {
            let mut memory = Memory::new(&[0x48, 0x8c, 0xe0][..length]);
            assert!(matches!(memory.decode(), Err(DecodeError::Memory(_))));
            assert_eq!(memory.fetched.len(), length + 1);
        }
        for length in 1..3 {
            let mut memory = Memory::new(&[0x48, 0x8c, 0xe8][..length]);
            assert!(matches!(memory.decode(), Err(DecodeError::Memory(_))));
            assert_eq!(memory.fetched.len(), length + 1);
        }
    }

    #[test]
    fn verw_decodes_only_bare_gdt_register_ax_form_and_fetches_on_demand() {
        let mut memory = Memory::new(&[0x0f, 0x00, 0xe8]);
        let decoded = memory.decode().unwrap();
        assert_eq!(decoded.length, 3);
        assert!(matches!(decoded.operation, Operation::VerwGdt));
        assert_eq!(memory.fetched, vec![0xfff, 0x1000, 0x1001]);

        for bytes in [
            &[0x0f, 0x00, 0xe0][..], // VERR AX
            &[0x0f, 0x00, 0xf0][..], // another group extension
            &[0xf3, 0x0f, 0x00, 0xe8][..],
            &[0x66, 0x0f, 0x00, 0xe8][..],
            &[0x48, 0x0f, 0x00, 0xe8][..],
            &[0x0f, 0x00, 0x68][..], // memory operand
        ] {
            let mut memory = Memory::new(bytes);
            assert!(matches!(
                memory.decode(),
                Err(DecodeError::Unsupported { .. })
            ));
        }
        for length in 1..3 {
            let mut memory = Memory::new(&[0x0f, 0x00, 0xe8][..length]);
            assert!(matches!(memory.decode(), Err(DecodeError::Memory(_))));
            assert_eq!(memory.fetched.len(), length + 1);
        }
    }

    #[test]
    fn fs_selector_load_decodes_only_bare_8e_e0() {
        let mut memory = Memory::new(&[0x8e, 0xe0]);
        let decoded = memory.decode().unwrap();
        assert_eq!(decoded.length, 2);
        assert!(matches!(decoded.operation, Operation::LoadFsSelector));
        for bytes in [
            &[0x8e, 0xd0][..], // SS
            &[0x8e, 0xc0][..], // ES
            &[0x8e, 0xc8][..], // CS is not a MOV destination
            &[0x8e, 0xd8][..], // DS
            &[0x8e, 0x20][..], // memory source
            &[0x8e, 0xe1][..], // another GPR
            &[0x48, 0x8e, 0xe0][..],
            &[0x66, 0x8e, 0xe0][..],
            &[0x64, 0x8e, 0xe0][..],
            &[0xf0, 0x8e, 0xe0][..],
            &[0xf3, 0x8e, 0xe0][..],
        ] {
            let mut memory = Memory::new(bytes);
            assert!(matches!(
                memory.decode(),
                Err(DecodeError::Unsupported { .. })
            ));
        }
        for length in 1..2 {
            let mut memory = Memory::new(&[0x8e, 0xe0][..length]);
            assert!(matches!(memory.decode(), Err(DecodeError::Memory(_))));
            assert_eq!(memory.fetched.len(), length + 1);
        }
        let mut truncated = Memory::new(&[0x8e]);
        assert!(matches!(truncated.decode(), Err(DecodeError::Memory(_))));
        assert_eq!(truncated.fetched, vec![0x0fff, 0x1000]);
    }

    #[test]
    fn gs_selector_load_decodes_only_bare_8e_e8() {
        let mut memory = Memory::new(&[0x8e, 0xe8]);
        let decoded = memory.decode().unwrap();
        assert_eq!(decoded.length, 2);
        assert!(matches!(decoded.operation, Operation::LoadGsSelector));
        for bytes in [
            &[0x8e, 0xd0][..],
            &[0x8e, 0xc0][..],
            &[0x8e, 0xc8][..],
            &[0x8e, 0xd8][..],
            &[0x8e, 0x28][..],
            &[0x8e, 0xe9][..],
            &[0x48, 0x8e, 0xe8][..],
            &[0x66, 0x8e, 0xe8][..],
            &[0x65, 0x8e, 0xe8][..],
            &[0xf0, 0x8e, 0xe8][..],
            &[0xf3, 0x8e, 0xe8][..],
        ] {
            let mut memory = Memory::new(bytes);
            assert!(matches!(
                memory.decode(),
                Err(DecodeError::Unsupported { .. })
            ));
        }
        let mut truncated = Memory::new(&[0x8e]);
        assert!(matches!(truncated.decode(), Err(DecodeError::Memory(_))));
        assert_eq!(truncated.fetched, vec![0x0fff, 0x1000]);
    }

    #[test]
    fn canonical_boundary_does_not_wrap_into_a_fetch() {
        let mut memory = Memory::new(&[0x48, 0xb8]);
        memory.base = 0x7fff_ffff_ffff;
        assert_eq!(
            memory.decode(),
            Err(DecodeError::Memory(general_protection()))
        );
        assert_eq!(memory.fetched, [memory.base]);
    }

    #[test]
    fn expanded_decode_executes_direct_branches_but_keeps_unimplemented_ud2_unsupported() {
        for bytes in [vec![0x0f, 0x84, 0, 0, 0, 0]] {
            let mut memory = Memory::new(&bytes);
            assert!(memory.decode().is_ok());
            let mut state = crate::LongModeState {
                rip: memory.base,
                gpr: [u64::MAX; 16],
                rflags: 2,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,

                x87: None,
            };
            let before = state.clone();
            assert_eq!(
                crate::execute::step(&mut state, &mut memory, AccessContext::default()),
                Ok(crate::execute::StepOutcome::Advanced)
            );
            assert_eq!(state.rip, memory.base + 6);
            assert_eq!(state.gpr, before.gpr);
            assert_eq!(state.rflags, before.rflags);
        }
        let mut memory = Memory::new(&[0x0f, 0x0b]);
        let mut state = crate::LongModeState {
            rip: memory.base,
            ..crate::LongModeState::default()
        };
        let before = state.clone();
        assert_eq!(
            crate::execute::step(&mut state, &mut memory, AccessContext::default()),
            Ok(crate::execute::StepOutcome::Fault(ArchitecturalFault {
                vector: 6,
                error_code: None,
                linear_address: None
            }))
        );
        assert_eq!(state, before);
    }

    #[test]
    fn multiply_divide_encodings_decode_the_supported_widths_and_implicit_forms() {
        for (bytes, expected_size, expected) in [
            (
                &[0xf6, 0xe3][..],
                OperandSize::Byte,
                Operation::Mul {
                    signed: false,
                    operand: Operand::Register(RegisterView::low(Register::Rbx, OperandSize::Byte)),
                },
            ),
            (
                &[0x66, 0xf7, 0xe9][..],
                OperandSize::Word,
                Operation::Mul {
                    signed: true,
                    operand: Operand::Register(RegisterView::low(Register::Rcx, OperandSize::Word)),
                },
            ),
            (
                &[0xf7, 0xf1][..],
                OperandSize::Dword,
                Operation::Divide {
                    signed: false,
                    divisor: Operand::Register(RegisterView::low(
                        Register::Rcx,
                        OperandSize::Dword,
                    )),
                },
            ),
            (
                &[0x48, 0xf7, 0xf9][..],
                OperandSize::Qword,
                Operation::Divide {
                    signed: true,
                    divisor: Operand::Register(RegisterView::low(
                        Register::Rcx,
                        OperandSize::Qword,
                    )),
                },
            ),
            (
                &[0x0f, 0xaf, 0xc3][..],
                OperandSize::Dword,
                Operation::Imul {
                    dst: RegisterView::low(Register::Rax, OperandSize::Dword),
                    src: Operand::Register(RegisterView::low(Register::Rbx, OperandSize::Dword)),
                },
            ),
            (
                &[0x66, 0x6b, 0xc3, 0x80][..],
                OperandSize::Word,
                Operation::ImulImmediate {
                    dst: RegisterView::low(Register::Rax, OperandSize::Word),
                    src: Operand::Register(RegisterView::low(Register::Rbx, OperandSize::Word)),
                    immediate: (-128i64) as u64,
                },
            ),
            (
                &[0x48, 0x69, 0xc3, 0xff, 0xff, 0xff, 0xff][..],
                OperandSize::Qword,
                Operation::ImulImmediate {
                    dst: RegisterView::low(Register::Rax, OperandSize::Qword),
                    src: Operand::Register(RegisterView::low(Register::Rbx, OperandSize::Qword)),
                    immediate: (-1i64) as u64,
                },
            ),
        ] {
            let mut memory = Memory::new(bytes);
            let instruction = memory.decode().unwrap();
            assert_eq!(instruction.operand_size, expected_size, "{bytes:02x?}");
            assert_eq!(instruction.operation, expected, "{bytes:02x?}");
            assert_eq!(instruction.length as usize, bytes.len());
        }

        let mut memory = Memory::new(&[0x66, 0xf6, 0xe3]);
        let instruction = memory.decode().unwrap();
        assert_eq!(instruction.operand_size, OperandSize::Byte);
        assert_eq!(
            instruction.operation,
            Operation::Mul {
                signed: false,
                operand: Operand::Register(RegisterView::low(Register::Rbx, OperandSize::Byte)),
            }
        );
    }

    #[test]
    fn multiply_divide_flags_match_sdm_definedness() {
        let mut mul_memory = Memory::new(&[0xf7, 0xe3]);
        assert_eq!(
            mul_memory.decode().unwrap().flag_effects(),
            FlagEffects {
                defined: crate::CF | crate::OF,
                undefined: crate::PF | crate::AF | crate::ZF | crate::SF,
            }
        );
        let mut div_memory = Memory::new(&[0xf7, 0xf3]);
        assert_eq!(
            div_memory.decode().unwrap().flag_effects(),
            FlagEffects {
                defined: 0,
                undefined: crate::ARITHMETIC_FLAGS,
            }
        );
    }

    fn memory_source(memory: &mut Memory) -> (Instruction, MemoryOperand) {
        let instruction = memory.decode().unwrap();
        let Operation::Mov {
            src: Operand::Memory(operand),
            ..
        } = instruction.operation
        else {
            panic!("expected MOV memory source")
        };
        (instruction, operand)
    }

    #[test]
    fn hand_written_modrm_sib_vectors_cover_special_bases_and_signed_displacements() {
        // Bytes are transcribed directly from SDM093 Vol2A Tables2-2/2-3/2-5/2-7,
        // not produced by a project assembler or encoder. Expected addresses are
        // literal arithmetic examples, independent of decoder output.
        let mut state = LongModeState::default();
        state.gpr[Register::Rax as usize] = 0x1000;
        state.gpr[Register::Rcx as usize] = 3;
        state.gpr[Register::Rsp as usize] = 0x2000;
        state.gpr[Register::Rbp as usize] = 0x3000;
        state.gpr[Register::R12 as usize] = 0x4000;
        state.gpr[Register::R13 as usize] = 0x5000;
        for (bytes, expected) in [
            (&[0x48, 0x8b, 0x00][..], 0x1000),  // [RAX]
            (&[0x48, 0x8b, 0x40, 0x80], 0xf80), // [RAX-128]
            (&[0x48, 0x8b, 0x80, 0, 0, 0, 0x80], 0xffff_ffff_8000_1000),
            (&[0x48, 0x8b, 0x04, 0x24], 0x2000), // [RSP], SIB mandatory
            (&[0x49, 0x8b, 0x04, 0x24], 0x4000), // [R12], SIB mandatory
            (&[0x48, 0x8b, 0x45, 0], 0x3000),    // [RBP+0]
            (&[0x49, 0x8b, 0x45, 0], 0x5000),    // [R13+0]
            (&[0x48, 0x8b, 0x04, 0x88], 0x100c), // [RAX+RCX*4]
            (&[0x4a, 0x8b, 0x04, 0x60], 0x9000), // [RAX+R12*2]
            (
                &[0x48, 0x8b, 0x04, 0x25, 0, 0, 0, 0x80],
                0xffff_ffff_8000_0000,
            ),
            (
                &[0x49, 0x8b, 0x04, 0x25, 0, 0, 0, 0x80],
                0xffff_ffff_8000_0000,
            ), // no-base ignores B
            (&[0x48, 0x8b, 0x04, 0xcd, 0xf0, 0xff, 0xff, 0xff], 8), // RCX*8-16
            (&[0x48, 0x8b, 0x05, 0xf0, 0xff, 0xff, 0xff], 0xff6),   // next RIP=1006
            (&[0x49, 0x8b, 0x05, 0xf0, 0xff, 0xff, 0xff], 0xff6),   // RIP-relative ignores B
        ] {
            let mut memory = Memory::new(bytes);
            let (instruction, operand) = memory_source(&mut memory);
            assert_eq!(
                operand.effective_address(&state, memory.base + u64::from(instruction.length)),
                expected,
                "{bytes:02x?}"
            );
            assert_eq!(memory.fetched.len(), bytes.len());
        }
    }

    #[test]
    fn address_override_wraps_base_index_displacement_and_next_rip_to_32_bits() {
        let mut state = LongModeState::default();
        state.gpr[0] = 0xffff_ffff_ffff_fff0;
        state.gpr[1] = 0x1_0000_0008;
        for (bytes, next, expected) in [
            (&[0x67, 0x48, 0x8b, 0x44, 0x88, 0x20][..], 0, 0x30),
            (
                &[0x67, 0x48, 0x8b, 0x05, 0xf0, 0xff, 0xff, 0xff],
                0x1_0000_0008,
                0xffff_fff8,
            ),
            (&[0x67, 0x49, 0x8b, 0x05, 0x20, 0, 0, 0], 0xffff_fff0, 0x10),
            (
                &[0x67, 0x48, 0x8b, 0x04, 0x25, 0, 0, 0, 0x80],
                0,
                0x8000_0000,
            ),
        ] {
            let mut memory = Memory::new(bytes);
            let (_, operand) = memory_source(&mut memory);
            assert_eq!(operand.address_size, OperandSize::Dword);
            assert_eq!(operand.effective_address(&state, next), expected);
            assert_eq!(operand.linear_address_48(&state, next), Ok(expected));
        }
        let mut memory = Memory::new(&[0x48, 0x8b, 0x44, 0x88, 0x20]);
        let (_, operand) = memory_source(&mut memory);
        assert_eq!(operand.effective_address(&state, 0), 0x4_0000_0030);
        state.gpr[1] = 0;
        assert_eq!(operand.effective_address(&state, 0), 0x10);
    }

    #[test]
    fn canonical48_checks_select_ss_only_for_rsp_rbp_bases() {
        let mut state = LongModeState {
            gpr: [0x8000_0000_0000; 16],
            ..LongModeState::default()
        };
        for (bytes, vector) in [
            (&[0x48, 0x8b, 0x00][..], 13),
            (&[0x48, 0x8b, 0x04, 0x24], 12),
            (&[0x48, 0x8b, 0x45, 0], 12),
            (&[0x49, 0x8b, 0x04, 0x24], 13),
            (&[0x49, 0x8b, 0x45, 0], 13),
            (&[0x36, 0x48, 0x8b, 0x00], 13),    // SS override ignored
            (&[0x3e, 0x48, 0x8b, 0x45, 0], 12), // DS override ignored
        ] {
            let mut memory = Memory::new(bytes);
            let (_, operand) = memory_source(&mut memory);
            assert_eq!(
                operand.linear_address_48(&state, 0),
                Err(ArchitecturalFault {
                    vector,
                    error_code: Some(0),
                    linear_address: None
                })
            );
            assert_eq!(operand.effective_address(&state, 0), 0x8000_0000_0000);
        }
        let mut memory = Memory::new(&[0x48, 0x8b, 0x00]);
        let (_, operand) = memory_source(&mut memory);
        for address in [0, 0x7fff_ffff_ffff, 0xffff_8000_0000_0000, u64::MAX] {
            state.gpr[0] = address;
            assert_eq!(operand.linear_address_48(&state, 0), Ok(address));
        }
        for prefix in [0x64, 0x65] {
            let mut memory = Memory::new(&[prefix, 0x48, 0x8b, 0x45, 0]);
            assert!(matches!(
                memory.decode(),
                Err(DecodeError::Unsupported { length: 5, .. })
            ));
        }
        for bytes in [&[0x64, 0x0f, 0xbc, 0x00][..], &[0x65, 0x0f, 0xbd, 0x00]] {
            let mut memory = Memory::new(bytes);
            assert!(matches!(
                memory.decode(),
                Err(DecodeError::Unsupported { .. })
            ));
        }
        for bytes in [&[0x64, 0x0f, 0xbc, 0xc0][..], &[0x65, 0x0f, 0xbd, 0xc1]] {
            let mut memory = Memory::new(bytes);
            assert!(matches!(
                memory.decode().unwrap().operation,
                Operation::BitScan {
                    src: Operand::Register(_),
                    ..
                }
            ));
        }
    }

    #[test]
    fn memory_decoder_faults_at_every_sib_and_displacement_byte_without_data_access() {
        for bytes in [
            vec![0x48, 0x8b, 0x04, 0x24],
            vec![0x4f, 0x8b, 0x84, 0x8d, 0, 0, 0, 0x80],
            vec![0x67, 0x48, 0x89, 0x05, 1, 2, 3, 4],
            vec![0x48, 0x8b, 0x45, 0x80],
        ] {
            for cut in 0..bytes.len() {
                let mut memory = Memory::new(&bytes[..cut]);
                assert!(
                    matches!(memory.decode(), Err(DecodeError::Memory(MemoryError::Unmapped { address, .. })) if address == memory.base + cut as u64)
                );
                assert_eq!(memory.fetched.len(), cut + 1);
                let mut memory = Memory::new(&bytes);
                memory.fault_at = Some(cut);
                assert!(matches!(
                    memory.decode(),
                    Err(DecodeError::Memory(MemoryError::Architectural(
                        ArchitecturalFault { vector: 14, .. }
                    )))
                ));
                assert_eq!(memory.fetched.len(), cut + 1);
            }
            let mut memory = Memory::new(&bytes);
            assert!(memory.decode().is_ok());
            assert_eq!(memory.fetched.len(), bytes.len());
        }
    }

    #[test]
    fn modrm_sib_matrix_covers_modes_extensions_scales_and_address_sizes() {
        // Exhaust the encoded fields directly. This uses no production encoder.
        // Fixed register-number tables describe the operand names from the SDM.
        let register_numbers = [[0, 1, 2, 3, 4, 5, 6, 7], [8, 9, 10, 11, 12, 13, 14, 15]];
        let mut state = LongModeState::default();
        for (number, value) in state.gpr.iter_mut().enumerate() {
            *value = 0xffff_ffff_ffff_0000u64.wrapping_add((number as u64 + 1) * 0x321);
        }
        for override32 in [false, true] {
            for mode in 0..3u8 {
                for b in 0..2u8 {
                    for x in 0..2u8 {
                        for sib in 0..=255u8 {
                            let mut bytes = vec![];
                            if override32 {
                                bytes.push(0x67);
                            }
                            // W+R choose R8 as destination, independently of X/B.
                            bytes.extend([0x4c | (x << 1) | b, 0x8b, (mode << 6) | 4, sib]);
                            let base_bits = (sib & 7) as usize;
                            let index_bits = ((sib >> 3) & 7) as usize;
                            let no_base = mode == 0 && base_bits == 5;
                            let disp: i64 = if mode == 1 {
                                -128
                            } else if mode == 2 || no_base {
                                -2147483648
                            } else {
                                0
                            };
                            if mode == 1 {
                                bytes.push(0x80);
                            } else if mode == 2 || no_base {
                                bytes.extend([0, 0, 0, 0x80]);
                            }
                            let expected_base = if no_base {
                                0
                            } else {
                                state.gpr[register_numbers[b as usize][base_bits]]
                            };
                            let expected_index = if x == 0 && index_bits == 4 {
                                0
                            } else {
                                state.gpr[register_numbers[x as usize][index_bits]]
                            };
                            let multiplier = [1, 2, 4, 8][(sib >> 6) as usize];
                            let mut expected = expected_base
                                .wrapping_add(expected_index.wrapping_mul(multiplier))
                                .wrapping_add_signed(disp);
                            if override32 {
                                expected &= 0xffff_ffff;
                            }
                            let mut memory = Memory::new(&bytes);
                            let (instruction, operand) = memory_source(&mut memory);
                            assert_eq!(
                                operand.effective_address(&state, 0x123456),
                                expected,
                                "{bytes:02x?}"
                            );
                            assert_eq!(operand.displacement, disp);
                            let Operation::Mov {
                                dst: Operand::Register(dst),
                                ..
                            } = instruction.operation
                            else {
                                panic!()
                            };
                            assert_eq!(dst.register(), Register::R8);
                            assert_eq!(instruction.length as usize, bytes.len());
                            assert_eq!(memory.fetched.len(), bytes.len());
                        }
                    }
                    for rm in [0, 1, 2, 3, 5, 6, 7] {
                        let mut bytes = vec![];
                        if override32 {
                            bytes.push(0x67);
                        }
                        bytes.extend([0x4c | b, 0x8b, (mode << 6) | rm]);
                        let rip_relative = mode == 0 && rm == 5;
                        let displacement = if mode == 1 {
                            -128
                        } else if mode == 2 || rip_relative {
                            -2147483648
                        } else {
                            0
                        };
                        if mode == 1 {
                            bytes.push(0x80);
                        } else if mode == 2 || rip_relative {
                            bytes.extend([0, 0, 0, 0x80]);
                        }
                        let base = if rip_relative {
                            0x123456
                        } else {
                            state.gpr[register_numbers[b as usize][rm as usize]]
                        };
                        let mut expected = base.wrapping_add_signed(displacement);
                        if override32 {
                            expected &= 0xffff_ffff;
                        }
                        let mut memory = Memory::new(&bytes);
                        let (_, operand) = memory_source(&mut memory);
                        assert_eq!(
                            operand.effective_address(&state, 0x123456),
                            expected,
                            "{bytes:02x?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn memory_width_and_mov_direction_are_independent_of_address_expression() {
        for (prefix, load, store, size) in [
            (&[][..], 0x8a, 0x88, OperandSize::Byte),
            (&[0x66][..], 0x8b, 0x89, OperandSize::Word),
            (&[][..], 0x8b, 0x89, OperandSize::Dword),
            (&[0x48][..], 0x8b, 0x89, OperandSize::Qword),
        ] {
            for opcode in [load, store] {
                let mut bytes = prefix.to_vec();
                bytes.extend([opcode, 0x08]); // RCX alias, [RAX]
                let mut memory = Memory::new(&bytes);
                let instruction = memory.decode().unwrap();
                let Operation::Mov { dst, src } = instruction.operation else {
                    panic!()
                };
                let (Operand::Register(reg), Operand::Memory(mem)) = (if opcode == load {
                    (dst, src)
                } else {
                    (src, dst)
                }) else {
                    panic!()
                };
                assert_eq!(reg, RegisterView::low(Register::Rcx, size));
                assert_eq!(mem.size, size);
                assert_eq!(mem.base, AddressBase::Register(Register::Rax));
            }
        }
        // High byte and REX-low byte have identical memory addressing.
        for (bytes, expected) in [
            (
                &[0x88, 0x20][..],
                RegisterView::high_byte(Register::Rax, false).unwrap(),
            ),
            (
                &[0x40, 0x88, 0x20][..],
                RegisterView::low(Register::Rsp, OperandSize::Byte),
            ),
        ] {
            let mut memory = Memory::new(bytes);
            let instruction = memory.decode().unwrap();
            let Operation::Mov {
                dst: Operand::Memory(_),
                src: Operand::Register(actual),
            } = instruction.operation
            else {
                panic!()
            };
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn displacement_boundaries_and_memory_instruction_length_limit() {
        let mut state = LongModeState::default();
        state.gpr[0] = 0x1000;
        for (bytes, expected) in [
            (&[0x48, 0x8b, 0x40, 0x7f][..], 0x107f),
            (&[0x48, 0x8b, 0x80, 0xff, 0xff, 0xff, 0x7f], 0x8000_0fff),
        ] {
            let mut memory = Memory::new(bytes);
            let (_, operand) = memory_source(&mut memory);
            assert_eq!(operand.effective_address(&state, 0), expected);
        }
        let mut bytes = vec![0x48; 8];
        bytes.extend([0x8b, 0x84, 0x20, 1, 2, 3, 4]);
        let mut memory = Memory::new(&bytes);
        assert_eq!(memory.decode().unwrap().length, 15);
        assert_eq!(memory.fetched.len(), 15);
        bytes.insert(0, 0x48);
        let mut memory = Memory::new(&bytes);
        memory.fault_at = Some(15);
        assert_eq!(
            memory.decode(),
            Err(DecodeError::Memory(general_protection()))
        );
        assert_eq!(memory.fetched.len(), 15);
    }

    #[test]
    fn transfer_neighbors_stay_unsupported_and_lea_register_source_is_invalid() {
        // C6/C7 /7 includes XABORT/XBEGIN, not an invalid MOV encoding.
        for bytes in [
            &[0xc6, 0xf8, 0][..],
            &[0xc7, 0xf8, 0, 0, 0, 0],
            &[0xc7, 0xc8, 0, 0, 0, 0],
            &[0x66, 0x0f, 0xb7, 0xc0],
            &[0x66, 0x0f, 0xbf, 0xc0],
            &[0x86, 0x00],
            &[0x66, 0x48, 0x87, 0x03],
            &[0x87, 0x03],
            &[0xf0, 0x48, 0x87, 0xc3],
        ] {
            let mut memory = Memory::new(bytes);
            assert!(
                matches!(memory.decode(), Err(DecodeError::Unsupported { .. })),
                "{bytes:02x?}"
            );
        }
        for bytes in [&[0x8d, 0xc0][..], &[0x48, 0x8d, 0xc0]] {
            let mut memory = Memory::new(bytes);
            assert!(matches!(memory.decode(), Err(DecodeError::Invalid { .. })));
            assert_eq!(memory.fetched.len(), bytes.len());
        }
        for bytes in [
            &[0x64, 0xc6, 0x03, 1][..],
            &[0x65, 0x48, 0xc7, 0x03, 1, 2, 3, 4],
            &[0x64, 0x0f, 0xbe, 0x03],
        ] {
            let mut memory = Memory::new(bytes);
            assert!(matches!(
                memory.decode(),
                Err(DecodeError::Unsupported { .. })
            ));
            assert_eq!(memory.fetched.len(), bytes.len());
        }
    }

    #[test]
    fn arithmetic_groups_and_prefixes_have_explicit_supported_boundaries() {
        for code in [
            &[0xfe, 0xd0][..],
            &[0x82, 0xc0, 1],
            &[0xf0, 0x48, 0x01, 0x03],
            &[0xf3, 0x48, 0x01, 0xc8],
            &[0x64, 0x48, 0x01, 0x03],
            &[0x65, 0x48, 0x03, 0x03],
            &[0x64, 0x80, 0x03, 1],
            &[0x65, 0xfe, 0x03],
        ] {
            assert!(
                matches!(
                    Memory::new(code).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "{code:02x?}"
            );
        }
        for (code, kind, size, register, immediate) in [
            (
                &[0x44, 0x83, 0xc0, 1][..],
                ArithmeticKind::Add,
                OperandSize::Dword,
                Register::Rax,
                1,
            ),
            (
                &[0x44, 0xff, 0xc0],
                ArithmeticKind::Inc,
                OperandSize::Dword,
                Register::Rax,
                1,
            ),
            (
                &[0x49, 0xff, 0xc0],
                ArithmeticKind::Inc,
                OperandSize::Qword,
                Register::R8,
                1,
            ),
            (
                &[0x66, 0x83, 0xd8, 0x80],
                ArithmeticKind::Sbb,
                OperandSize::Word,
                Register::Rax,
                0xffff_ffff_ffff_ff80,
            ),
            (
                &[0x48, 0x81, 0xf8, 0, 0, 0, 0x80],
                ArithmeticKind::Cmp,
                OperandSize::Qword,
                Register::Rax,
                0xffff_ffff_8000_0000,
            ),
        ] {
            let decoded = Memory::new(code).decode().unwrap();
            assert_eq!(decoded.operand_size, size);
            assert_eq!(
                decoded.operation,
                Operation::Arithmetic {
                    kind,
                    dst: Operand::Register(RegisterView::low(register, size)),
                    src: Operand::Immediate(immediate)
                }
            );
            assert_eq!(decoded.length as usize, code.len());
        }
        // 40..4F remain REX in long64; they do not become legacy INC/DEC.
        for code in [&[0x40, 0x90][..], &[0x48, 0x90]] {
            assert_eq!(
                Memory::new(code).decode().unwrap().operation,
                Operation::Nop
            );
        }
    }

    #[test]
    fn logic_groups_widths_and_immediates_use_normalized_operands() {
        for (kind, opcode, group) in [
            (LogicKind::And, 0x20, 4),
            (LogicKind::Or, 0x08, 1),
            (LogicKind::Xor, 0x30, 6),
        ] {
            for (size, prefix) in [
                (OperandSize::Byte, &[][..]),
                (OperandSize::Word, &[0x66]),
                (OperandSize::Dword, &[]),
                (OperandSize::Qword, &[0x48]),
            ] {
                let wide = u8::from(size != OperandSize::Byte);
                let reg = Operand::Register(RegisterView::low(Register::Rax, size));
                let rcx = Operand::Register(RegisterView::low(Register::Rcx, size));
                let immediate_size = if size == OperandSize::Qword {
                    4
                } else {
                    size.bytes()
                };
                for (suffix, dst, src) in [
                    (vec![opcode + wide, 0xc8], reg, rcx),
                    (vec![opcode + 2 + wide, 0xc1], reg, rcx),
                    (
                        [vec![opcode + 4 + wide], vec![0xff; immediate_size]].concat(),
                        reg,
                        Operand::Immediate(size.mask()),
                    ),
                    (
                        [
                            vec![0x80 + wide, 0xc0 | (group << 3)],
                            vec![0xff; immediate_size],
                        ]
                        .concat(),
                        reg,
                        Operand::Immediate(size.mask()),
                    ),
                ] {
                    let bytes = [prefix, &suffix].concat();
                    let mut memory = Memory::new(&bytes);
                    let decoded = memory.decode().unwrap();
                    assert_eq!(decoded.operation, Operation::Logic { kind, dst, src });
                    assert_eq!(decoded.operand_size, size);
                    assert_eq!(memory.fetched.len(), bytes.len());
                }
                if size != OperandSize::Byte {
                    let bytes = [prefix, &[0x83, 0xc0 | (group << 3), 0x80]].concat();
                    assert_eq!(
                        Memory::new(&bytes).decode().unwrap().operation,
                        Operation::Logic {
                            kind,
                            dst: reg,
                            src: Operand::Immediate(0xffff_ffff_ffff_ff80)
                        }
                    );
                }
            }
        }

        // Group selectors ignore REX.R. TEST has no F6/F7 /1 alias in this
        // documented slice; accepting undocumented encodings is not required.
        for (code, kind) in [
            (&[0x44, 0xf7, 0xc0, 1, 2, 3, 4][..], LogicKind::Test),
            (&[0x44, 0xf7, 0xd0], LogicKind::Not),
        ] {
            assert!(
                matches!(Memory::new(code).decode().unwrap().operation,Operation::Logic{kind:k,..} if k==kind)
            );
        }
        for code in [
            &[0xf6, 0xc8, 1][..],
            &[0xf7, 0xc8, 1, 2, 3, 4],
            &[0xf0, 0x21, 0x03],
            &[0xf3, 0x85, 0xc0],
            &[0x64, 0x21, 0x03],
            &[0x65, 0xf7, 0x13],
            &[0x64, 0xf7, 0x03, 1, 2, 3, 4],
        ] {
            assert!(
                matches!(
                    Memory::new(code).decode(),
                    Err(DecodeError::Unsupported { .. })
                ),
                "{code:02x?}"
            );
        }
    }

    #[test]
    fn logic_demand_fetch_faults_at_each_byte_without_overfetch() {
        for code in [
            &[0x48, 0x81, 0x63, 0x7f, 1, 2, 3, 4][..],
            &[0x67, 0x66, 0x83, 0x74, 0x8b, 0xfe, 0x80],
            &[0x48, 0xf7, 0x05, 1, 2, 3, 4, 0xff, 0xff, 0xff, 0xff],
            &[0xa8, 0x80],
            &[0x48, 0xa9, 1, 2, 3, 4],
            &[0x40, 0x84, 0xe0],
            &[0x48, 0xf7, 0x94, 0x8b, 1, 2, 3, 4],
        ] {
            for fault in 0..=code.len() {
                let mut memory = Memory::new(code);
                memory.fault_at = Some(fault);
                if fault == code.len() {
                    assert!(memory.decode().is_ok());
                } else {
                    assert!(
                        matches!(
                            memory.decode(),
                            Err(DecodeError::Memory(MemoryError::Architectural(_)))
                        ),
                        "{code:02x?} byte{fault}"
                    );
                }
                assert_eq!(
                    memory.fetched.len(),
                    if fault == code.len() {
                        code.len()
                    } else {
                        fault + 1
                    }
                );
            }
        }
    }

    #[test]
    fn shift_decode_and_count_dependent_flag_effects_match_architectural_masks() {
        let cases: &[(&[u8], ShiftKind, OperandSize, bool)] = &[
            (&[0xc0, 0xc0, 1], ShiftKind::Rol, OperandSize::Byte, false),
            (
                &[0x48, 0xd3, 0x38],
                ShiftKind::Sar,
                OperandSize::Qword,
                false,
            ),
            (
                &[0x66, 0x0f, 0xa4, 0xc8, 0x10],
                ShiftKind::Shld,
                OperandSize::Word,
                true,
            ),
            (
                &[0x48, 0x0f, 0xad, 0xc8],
                ShiftKind::Shrd,
                OperandSize::Qword,
                true,
            ),
        ];
        for (bytes, kind, size, has_source) in cases {
            let instruction = Memory::new(bytes).decode().unwrap();
            let Operation::Shift {
                kind: actual_kind,
                source,
                count,
                ..
            } = instruction.operation
            else {
                panic!("expected shift operation: {bytes:02x?}");
            };
            assert_eq!(actual_kind, *kind);
            assert_eq!(instruction.operand_size, *size);
            assert_eq!(source.is_some(), *has_source);
            assert!(matches!(
                count,
                Operand::Immediate(_) | Operand::Register(_)
            ));
        }

        let shl = Memory::new(&[0xc0, 0xe0, 1]).decode().unwrap();
        assert_eq!(shl.flag_effects_for_count(0), FlagEffects::default());
        assert_eq!(shl.flag_effects_for_count(32), FlagEffects::default()); // masked to zero
        let at_width = shl.flag_effects_for_count(8);
        assert_eq!(at_width.defined, crate::SF | crate::ZF | crate::PF);
        assert_eq!(at_width.undefined, crate::CF | crate::AF | crate::OF);
        let above_width = shl.flag_effects_for_count(9);
        assert_eq!(above_width.defined, crate::SF | crate::ZF | crate::PF);
        assert_eq!(above_width.undefined, crate::CF | crate::AF | crate::OF);
        let sar = Memory::new(&[0xc0, 0xf8, 1]).decode().unwrap();
        assert_eq!(
            sar.flag_effects_for_count(9).defined,
            crate::CF | crate::SF | crate::ZF | crate::PF
        );
        assert_eq!(
            sar.flag_effects_for_count(9).undefined,
            crate::AF | crate::OF
        );

        let rol = Memory::new(&[0xc0, 0xc0, 1]).decode().unwrap();
        assert_eq!(
            rol.flag_effects_for_count(8),
            FlagEffects {
                defined: crate::CF,
                undefined: crate::OF,
            }
        );
        assert_eq!(rol.flag_effects_for_count(9).defined, crate::CF);
        assert_eq!(rol.flag_effects_for_count(9).undefined, crate::OF);

        let rcl = Memory::new(&[0xc0, 0xd0, 1]).decode().unwrap();
        assert_eq!(
            rcl.flag_effects_for_count(9),
            FlagEffects {
                defined: 0,
                undefined: crate::OF,
            }
        );
        assert_eq!(rcl.flag_effects_for_count(10).defined, crate::CF);
        assert_eq!(rcl.flag_effects_for_count(10).undefined, crate::OF);

        let shld = Memory::new(&[0x66, 0x0f, 0xa4, 0xc8, 1]).decode().unwrap();
        let at_width = shld.flag_effects_for_count(16);
        assert_eq!(
            at_width.defined,
            crate::CF | crate::SF | crate::ZF | crate::PF
        );
        assert_eq!(at_width.undefined, crate::AF | crate::OF);
        let above_width = shld.flag_effects_for_count(17);
        assert_eq!(above_width.defined, 0);
        assert_eq!(above_width.undefined, crate::ARITHMETIC_FLAGS);
    }
}
