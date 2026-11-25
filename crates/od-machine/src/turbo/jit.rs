//! Experiment (turbo loop): translation of one guest block (straight-line code on one 4 KiB page)
//! to AArch64. See the module documentation of `turbo` for the register convention.

use super::{EXIT_BUDGET, EXIT_SLOW, MAX_BLOCK, OP_WRITE, TLB_BITS, TurboEngine};
use od_isa::Register;
use od_isa::long_decode::{
    self, AddressBase, ArithmeticKind, ConditionCode, Instruction, LogicKind, MemoryOperand,
    OpcodeMap, Operand, Operation, Prefixes, ShiftKind,
};
use od_isa::memory::{AccessContext, GuestMemory, MemoryError};
use od_isa::state::{OperandSize, RegisterView};
use od_jit::arm64::a64::{self, Asm, Cond, LSL, Label, XZR};

pub(crate) enum Compiled {
    /// Code address, instruction count, guest byte ranges (page offset, length) of the
    /// instructions.
    Block(usize, usize, Vec<(u16, u8)>),
    /// The first instruction is not translated (its decode, when it decoded on the page).
    Slow,
    Rdtsc,
    NoSpace,
    /// The first instruction needs bytes of the next page.
    Crossing,
}

/// Instruction fetches from one host page (decode only), and the next page when given.
struct CodePage {
    host: *const u8,
    page: u64,
    next: Option<*const u8>,
    outside: bool,
    crossed: bool,
}

impl GuestMemory for CodePage {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        if address >> 12 == self.page {
            // SAFETY: `host` is the host address of the 4 KiB guest page `page`.
            return Ok(unsafe { *self.host.add((address & 0xfff) as usize) });
        }
        if address >> 12 == self.page.wrapping_add(1)
            && let Some(next) = self.next
        {
            self.crossed = true;
            // SAFETY: `next` is the host address of the following guest page.
            return Ok(unsafe { *next.add((address & 0xfff) as usize) });
        }
        self.outside = true;
        Err(MemoryError::Backend("turbo: outside the page".into()))
    }
    fn read(&mut self, _: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Backend("turbo decode reads no data".into()))
    }
    fn write(&mut self, _: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Backend("turbo decode writes nothing".into()))
    }
    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Err(MemoryError::Backend("turbo decode has no fence".into()))
    }
}

// Flag liveness bits.
const F_CF: u8 = 1;
const F_PF: u8 = 2;
const F_AF: u8 = 4;
const F_ZF: u8 = 8;
const F_SF: u8 = 16;
const F_OF: u8 = 32;
const F_ALL: u8 = 63;

#[derive(Clone, Copy, PartialEq, Eq)]
enum AluKind {
    Arith(ArithmeticKind),
    Logic(LogicKind),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Straight,
    Terminator,
}

/// What the translator does with one instruction.
#[derive(Clone, Copy)]
struct Plan {
    role: Role,
    reads: u8,
    writes: u8,
    can_exit: bool,
}

fn cond_reads(condition: ConditionCode) -> u8 {
    use ConditionCode::*;
    match condition {
        Overflow | NotOverflow => F_OF,
        Below | AboveOrEqual => F_CF,
        Equal | NotEqual => F_ZF,
        BelowOrEqual | Above => F_CF | F_ZF,
        Sign | NotSign => F_SF,
        Parity | NotParity => F_PF,
        Less | GreaterOrEqual => F_SF | F_OF,
        LessOrEqual | Greater => F_ZF | F_SF | F_OF,
    }
}

fn arm_cond(condition: ConditionCode) -> Option<Cond> {
    use ConditionCode::*;
    Some(match condition {
        Overflow => Cond::Vs,
        NotOverflow => Cond::Vc,
        Below => Cond::Lo,
        AboveOrEqual => Cond::Hs,
        Equal => Cond::Eq,
        NotEqual => Cond::Ne,
        BelowOrEqual => Cond::Ls,
        Above => Cond::Hi,
        Sign => Cond::Mi,
        NotSign => Cond::Pl,
        Less => Cond::Lt,
        GreaterOrEqual => Cond::Ge,
        LessOrEqual => Cond::Le,
        Greater => Cond::Gt,
        Parity | NotParity => return None,
    })
}

fn canonical(address: u64) -> bool {
    ((address as i64) >> 47) == 0 || ((address as i64) >> 47) == -1
}

fn mem_ok(mem: &MemoryOperand) -> bool {
    mem.address_size == OperandSize::Qword
}

fn size_bytes(size: OperandSize) -> u32 {
    size.bytes() as u32
}

/// The plan of `insn`, or `None` when the block ends before it.
fn plan(insn: &Instruction) -> Option<Plan> {
    let fast = od_isa::execute::fast_handler(insn).is_some();
    let size = insn.operand_size;
    let p = |role, reads, writes, can_exit| {
        Some(Plan {
            role,
            reads,
            writes,
            can_exit,
        })
    };
    match insn.operation {
        Operation::Arithmetic { kind, dst, src } if fast => {
            let mem = matches!(dst, Operand::Memory(_)) || matches!(src, Operand::Memory(_));
            if let Operand::Memory(m) = dst {
                if !mem_ok(&m) {
                    return None;
                }
            }
            if let Operand::Memory(m) = src {
                if !mem_ok(&m) {
                    return None;
                }
            }
            let narrow = matches!(size, OperandSize::Byte | OperandSize::Word);
            match kind {
                ArithmeticKind::Adc | ArithmeticKind::Sbb if narrow => None,
                ArithmeticKind::Adc | ArithmeticKind::Sbb => p(Role::Straight, F_CF, F_ALL, mem),
                ArithmeticKind::Inc | ArithmeticKind::Dec => {
                    p(Role::Straight, F_CF, F_ALL & !F_CF, mem)
                }
                _ => p(Role::Straight, 0, F_ALL, mem),
            }
        }
        Operation::Logic { kind, dst, src } if fast => {
            if let Operand::Memory(m) = dst {
                if !mem_ok(&m) {
                    return None;
                }
            }
            if let Operand::Memory(m) = src {
                if !mem_ok(&m) {
                    return None;
                }
            }
            let mem = matches!(dst, Operand::Memory(_)) || matches!(src, Operand::Memory(_));
            if kind == LogicKind::Not {
                return p(Role::Straight, 0, 0, mem);
            }
            p(Role::Straight, 0, F_ALL & !F_AF, mem)
        }
        Operation::Shift {
            kind: kind @ (ShiftKind::Shld | ShiftKind::Shrd),
            dst: Operand::Register(_),
            source: Some(Operand::Register(_)),
            count: Operand::Immediate(count),
            ..
        } if fast && matches!(size, OperandSize::Dword | OperandSize::Qword) => {
            let _ = kind;
            let width = size_bytes(size) * 8;
            let masked = (count as u32) & if width == 64 { 0x3f } else { 0x1f };
            if masked == 0 {
                return p(Role::Straight, 0, 0, false);
            }
            if masked == 1 {
                p(Role::Straight, 0, F_CF | F_PF | F_ZF | F_SF | F_OF, false)
            } else {
                p(Role::Straight, F_OF, F_CF | F_PF | F_ZF | F_SF, false)
            }
        }
        Operation::Shift {
            kind,
            dst: Operand::Register(_),
            source: None,
            count,
            ..
        } if fast => {
            let width = size_bytes(size) * 8;
            if width < 32 {
                // 8/16-bit SHL/SHR/SAR by an immediate below the width.
                let Operand::Immediate(count) = count else {
                    return None;
                };
                let masked = (count as u32) & 0x1f;
                if !matches!(kind, ShiftKind::Shl | ShiftKind::Shr | ShiftKind::Sar)
                    || masked >= width
                {
                    return None;
                }
                if masked == 0 {
                    return p(Role::Straight, 0, 0, false);
                }
                return if masked == 1 {
                    p(Role::Straight, 0, F_CF | F_PF | F_ZF | F_SF | F_OF, false)
                } else {
                    p(Role::Straight, F_OF, F_CF | F_PF | F_ZF | F_SF, false)
                };
            }
            match count {
                Operand::Immediate(count) => {
                    let masked = (count as u32) & if width == 64 { 0x3f } else { 0x1f };
                    match kind {
                        ShiftKind::Shl | ShiftKind::Shr | ShiftKind::Sar => {
                            if masked == 0 {
                                return p(Role::Straight, 0, 0, false);
                            }
                            if masked == 1 {
                                p(Role::Straight, 0, F_CF | F_PF | F_ZF | F_SF | F_OF, false)
                            } else {
                                p(Role::Straight, F_OF, F_CF | F_PF | F_ZF | F_SF, false)
                            }
                        }
                        ShiftKind::Rol | ShiftKind::Ror => {
                            if masked == 0 {
                                return p(Role::Straight, 0, 0, false);
                            }
                            if masked == 1 {
                                p(Role::Straight, 0, F_CF | F_OF, false)
                            } else {
                                p(Role::Straight, 0, F_CF, false)
                            }
                        }
                        _ => None,
                    }
                }
                Operand::Register(view)
                    if view.register() == Register::Rcx
                        && view.size() == OperandSize::Byte
                        && view.shift() == 0
                        && matches!(kind, ShiftKind::Shl | ShiftKind::Shr | ShiftKind::Sar) =>
                {
                    // A count of 0 changes no flag: everything before stays needed.
                    p(Role::Straight, F_ALL, 0, false)
                }
                _ => None,
            }
        }
        Operation::Mov { dst, src } if fast => {
            for operand in [dst, src] {
                if let Operand::Memory(m) = operand {
                    if !mem_ok(&m) {
                        return None;
                    }
                }
            }
            let mem = matches!(dst, Operand::Memory(_)) || matches!(src, Operand::Memory(_));
            p(Role::Straight, 0, 0, mem)
        }
        Operation::MovImmediate { dst, .. } if fast => {
            if let Operand::Memory(m) = dst {
                if !mem_ok(&m) {
                    return None;
                }
            }
            p(Role::Straight, 0, 0, matches!(dst, Operand::Memory(_)))
        }
        Operation::Extend { src, .. } if fast => {
            if let Operand::Memory(m) = src {
                if !mem_ok(&m) {
                    return None;
                }
            }
            p(Role::Straight, 0, 0, matches!(src, Operand::Memory(_)))
        }
        Operation::Lea { src, .. } if fast => {
            if src.address_size != OperandSize::Qword && src.address_size != OperandSize::Dword {
                return None;
            }
            p(Role::Straight, 0, 0, false)
        }
        Operation::Imul { src, .. } if fast => {
            if !matches!(size, OperandSize::Dword | OperandSize::Qword) {
                return None;
            }
            if let Operand::Memory(m) = src {
                if !mem_ok(&m) {
                    return None;
                }
            }
            p(
                Role::Straight,
                0,
                F_CF | F_OF,
                matches!(src, Operand::Memory(_)),
            )
        }
        Operation::StackRegister { .. } if fast => {
            if !matches!(size, OperandSize::Qword | OperandSize::Word) {
                return None;
            }
            p(Role::Straight, 0, 0, true)
        }
        Operation::Branch { condition, .. } if fast => {
            p(Role::Terminator, cond_reads(condition), 0, false)
        }
        Operation::Jump { .. } if fast => p(Role::Terminator, 0, 0, false),
        Operation::Call { .. } if fast => p(Role::Terminator, 0, 0, true),
        Operation::Return { .. } if fast => p(Role::Terminator, 0, 0, true),
        Operation::Nop | Operation::NopModrm if fast => p(Role::Straight, 0, 0, false),
        Operation::SetCondition { condition, dst, .. }
            if insn.prefixes.group1.is_none()
                && !matches!(insn.prefixes.segment, Some(0x64 | 0x65)) =>
        {
            match dst {
                Operand::Register(_) => p(Role::Straight, cond_reads(condition), 0, false),
                Operand::Memory(m) if mem_ok(&m) => {
                    p(Role::Straight, cond_reads(condition), 0, true)
                }
                _ => None,
            }
        }
        Operation::ConditionalMove {
            condition,
            dst,
            src,
        } if insn.opcode.map == OpcodeMap::Escape0f
            && (0x40..=0x4f).contains(&insn.opcode.byte)
            && insn.prefixes.group1.is_none()
            && !matches!(insn.prefixes.segment, Some(0x64 | 0x65))
            && matches!(
                dst.size(),
                OperandSize::Word | OperandSize::Dword | OperandSize::Qword
            ) =>
        {
            match src {
                Operand::Register(_) => p(Role::Straight, cond_reads(condition), 0, false),
                Operand::Memory(m) if mem_ok(&m) => {
                    p(Role::Straight, cond_reads(condition), 0, true)
                }
                _ => None,
            }
        }
        Operation::IndirectCall { operand }
            if insn.opcode.map == OpcodeMap::Primary
                && insn.opcode.byte == 0xff
                && insn.prefixes.group1.is_none()
                && !insn.prefixes.operand_override =>
        {
            match operand {
                Operand::Register(view)
                    if insn.prefixes.segment.is_none()
                        && !insn.prefixes.address_override
                        && insn.prefixes.rex.is_none_or(|rex| rex == 0x41)
                        && view.size() == OperandSize::Qword =>
                {
                    p(Role::Terminator, 0, 0, true)
                }
                Operand::Memory(m)
                    if !matches!(insn.prefixes.segment, Some(0x64 | 0x65))
                        && mem_ok(&m)
                        && m.size == OperandSize::Qword =>
                {
                    p(Role::Terminator, 0, 0, true)
                }
                _ => None,
            }
        }
        Operation::IndirectJump { operand }
            if insn.opcode.map == OpcodeMap::Primary
                && insn.opcode.byte == 0xff
                && insn.prefixes.group1.is_none()
                && insn.prefixes.segment.is_none()
                && !insn.prefixes.operand_override =>
        {
            match operand {
                Operand::Register(view) if view.size() == OperandSize::Qword => {
                    p(Role::Terminator, 0, 0, true)
                }
                Operand::Memory(m) if mem_ok(&m) && m.size == OperandSize::Qword => {
                    p(Role::Terminator, 0, 0, true)
                }
                _ => None,
            }
        }
        Operation::Leave => p(Role::Straight, 0, 0, true),
        Operation::Rdtsc if is_rdtsc(insn) => p(Role::Straight, 0, 0, true),
        Operation::RepMovs { size: _ } | Operation::RepStos { size: _ }
            if insn.opcode.map == OpcodeMap::Primary
                && insn.prefixes.group1 == Some(0xf3)
                && insn.prefixes.segment.is_none()
                && insn.address_size == OperandSize::Qword
                && rep_admitted(insn) =>
        {
            p(Role::Straight, 0, 0, true)
        }
        Operation::Pause | Operation::Endbr64Nop => p(Role::Straight, 0, 0, false),
        Operation::AddGsLoad { dst, src } | Operation::CmpGsLoad { dst, src }
            if insn.prefixes.group1.is_none()
                && mem_ok(&src)
                && matches!(dst.size(), OperandSize::Dword | OperandSize::Qword) =>
        {
            p(Role::Straight, 0, F_ALL, true)
        }
        Operation::LogicGsLoad { dst, src, .. }
            if insn.prefixes.group1.is_none()
                && mem_ok(&src)
                && matches!(dst.size(), OperandSize::Dword | OperandSize::Qword) =>
        {
            p(Role::Straight, 0, F_ALL & !F_AF, true)
        }
        Operation::CmpGsMemImm { dst, .. }
        | Operation::AddGsMemImm { dst, .. }
        | Operation::SubGsMemImm { dst, .. }
            if insn.prefixes.group1.is_none() && mem_ok(&dst) =>
        {
            p(Role::Straight, 0, F_ALL, true)
        }
        Operation::AddGsMem { destination, .. }
            if insn.prefixes.group1.is_none() && mem_ok(&destination) =>
        {
            p(Role::Straight, 0, F_ALL, true)
        }
        Operation::LogicGsMemImm { dst, .. } if insn.prefixes.group1.is_none() && mem_ok(&dst) => {
            p(Role::Straight, 0, F_ALL & !F_AF, true)
        }
        Operation::TestGsMemByte { src, .. }
        | Operation::TestGsMemWord { src, .. }
        | Operation::TestGsMemWide { src, .. }
            if insn.prefixes.group1.is_none() && mem_ok(&src) =>
        {
            p(Role::Straight, 0, F_ALL & !F_AF, true)
        }
        Operation::MovGsStore { dst, .. }
        | Operation::MovGsMemByteImm { dst, .. }
        | Operation::MovGsMemImm { dst, .. }
            if insn.prefixes.group1.is_none() && mem_ok(&dst) =>
        {
            p(Role::Straight, 0, 0, true)
        }
        Operation::MovzxGsLoadB { src, .. }
        | Operation::MovzxGsLoadW { src, .. }
        | Operation::MovsxGsLoadB { src, .. }
        | Operation::MovsxdGsLoad { src, .. }
            if insn.prefixes.group1.is_none() && mem_ok(&src) =>
        {
            p(Role::Straight, 0, 0, true)
        }
        Operation::LockIncMem { destination } | Operation::LockDecMem { destination }
            if mem_ok(&destination) && !matches!(insn.prefixes.segment, Some(0x64 | 0x65)) =>
        {
            p(Role::Straight, F_CF, F_ALL & !F_CF, true)
        }
        Operation::LockAddMem { destination, .. }
        | Operation::LockSubMem { destination, .. }
        | Operation::LockSubDwordImm8 { destination, .. }
        | Operation::LockAddMemReg { destination, .. }
        | Operation::LockSubMemReg { destination, .. }
            if mem_ok(&destination) && !matches!(insn.prefixes.segment, Some(0x64 | 0x65)) =>
        {
            p(Role::Straight, 0, F_ALL, true)
        }
        Operation::LockOrMem { destination, .. }
        | Operation::LockAndMem { destination, .. }
        | Operation::LockXorMem { destination, .. }
        | Operation::LockLogicMemReg { destination, .. }
            if mem_ok(&destination) && !matches!(insn.prefixes.segment, Some(0x64 | 0x65)) =>
        {
            p(Role::Straight, 0, F_ALL & !F_AF, true)
        }
        Operation::Bswap { dst }
            if matches!(dst.size(), OperandSize::Dword | OperandSize::Qword)
                && insn.prefixes.segment.is_none()
                && insn.prefixes.group1.is_none() =>
        {
            p(Role::Straight, 0, 0, false)
        }
        Operation::Invlpg | Operation::Lfence => p(Role::Straight, 0, 0, false),
        Operation::ClearDirectionFlag | Operation::SetDirectionFlag
            if insn.opcode.map == OpcodeMap::Primary
                && insn.length == 1
                && insn.prefixes == Prefixes::default() =>
        {
            p(Role::Straight, 0, 0, false)
        }
        Operation::Bit { dst, offset, .. }
            if insn.prefixes.group1.is_none()
                && !matches!(insn.prefixes.segment, Some(0x64 | 0x65)) =>
        {
            let ok_size = |size: OperandSize| {
                matches!(
                    size,
                    OperandSize::Word | OperandSize::Dword | OperandSize::Qword
                )
            };
            match (dst, offset) {
                (Operand::Register(view), _) if ok_size(view.size()) => {
                    p(Role::Straight, 0, F_CF, false)
                }
                (Operand::Memory(m), _) if ok_size(m.size) && mem_ok(&m) => {
                    p(Role::Straight, 0, F_CF, true)
                }
                _ => None,
            }
        }
        Operation::Divide { divisor, .. }
            if matches!(size, OperandSize::Dword | OperandSize::Qword)
                && insn.prefixes.segment.is_none()
                && insn.prefixes.group1.is_none() =>
        {
            match divisor {
                Operand::Register(_) => p(Role::Straight, 0, 0, true),
                Operand::Memory(m) if mem_ok(&m) => p(Role::Straight, 0, 0, true),
                _ => None,
            }
        }
        Operation::SubGsMem { dst, src }
            if insn.prefixes.group1.is_none()
                && mem_ok(&src)
                && dst.size() == OperandSize::Qword =>
        {
            p(Role::Straight, 0, F_ALL, true)
        }
        Operation::CmpGsMem { dst, src }
            if insn.prefixes.group1.is_none()
                && mem_ok(&dst)
                && matches!(dst.size, OperandSize::Dword | OperandSize::Qword)
                && src.size() == dst.size =>
        {
            p(Role::Straight, 0, F_ALL, true)
        }
        Operation::BitScan { dst, src, .. }
            if matches!(dst.size(), OperandSize::Dword | OperandSize::Qword)
                && insn.prefixes.segment.is_none()
                && !matches!(src, Operand::Immediate(_)) =>
        {
            match src {
                Operand::Memory(m) if !mem_ok(&m) => None,
                Operand::Memory(_) => p(Role::Straight, 0, F_ALL, true),
                _ => p(Role::Straight, 0, F_ALL, false),
            }
        }
        Operation::PushFlags
            if size == OperandSize::Qword
                && insn.prefixes.segment.is_none()
                && insn.prefixes.group1.is_none() =>
        {
            p(Role::Straight, F_ALL, 0, true)
        }
        Operation::PushImmediate { .. }
            if size == OperandSize::Qword
                && insn.prefixes.segment.is_none()
                && insn.prefixes.group1.is_none() =>
        {
            p(Role::Straight, 0, 0, true)
        }
        Operation::SignExtendAccumulator { .. }
            if matches!(size, OperandSize::Dword | OperandSize::Qword)
                && insn.prefixes.segment.is_none()
                && insn.prefixes.group1.is_none() =>
        {
            p(Role::Straight, 0, 0, false)
        }
        Operation::ImulImmediate { dst, src, .. }
            if matches!(dst.size(), OperandSize::Dword | OperandSize::Qword)
                && insn.prefixes.segment.is_none()
                && insn.prefixes.group1.is_none() =>
        {
            match src {
                Operand::Register(_) => p(Role::Straight, 0, F_CF | F_OF, false),
                Operand::Memory(m) if mem_ok(&m) => p(Role::Straight, 0, F_CF | F_OF, true),
                _ => None,
            }
        }
        Operation::Mul { operand, .. }
            if matches!(size, OperandSize::Dword | OperandSize::Qword)
                && insn.prefixes.segment.is_none()
                && insn.prefixes.group1.is_none() =>
        {
            match operand {
                Operand::Register(_) => p(Role::Straight, 0, F_CF | F_OF, false),
                Operand::Memory(m) if mem_ok(&m) => p(Role::Straight, 0, F_CF | F_OF, true),
                _ => None,
            }
        }
        Operation::Cli if insn.prefixes == Prefixes::default() && insn.length == 1 => {
            p(Role::Straight, 0, 0, false)
        }
        Operation::Sti if insn.prefixes == Prefixes::default() && insn.length == 1 => {
            p(Role::Terminator, 0, 0, true)
        }
        Operation::MovSregStore {
            dst: Operand::Register(view),
            sreg,
            ..
        } if sreg <= 5
            && matches!(
                view.size(),
                OperandSize::Dword | OperandSize::Qword | OperandSize::Word
            ) =>
        {
            p(Role::Straight, 0, 0, true)
        }
        Operation::LockCmpxchgMem {
            destination,
            source,
        }
        | Operation::CmpxchgMemory {
            destination,
            source,
        } if mem_ok(&destination)
            && matches!(destination.size, OperandSize::Dword | OperandSize::Qword)
            && source.size() == destination.size
            && !matches!(insn.prefixes.segment, Some(0x64 | 0x65)) =>
        {
            p(Role::Straight, 0, F_ALL, true)
        }
        Operation::LockXaddMem {
            destination,
            source,
        }
        | Operation::XaddDsMem {
            destination,
            source,
        }
        | Operation::XaddMemory {
            destination,
            source,
        } if mem_ok(&destination)
            && matches!(destination.size, OperandSize::Dword | OperandSize::Qword)
            && source.size() == destination.size
            && !matches!(insn.prefixes.segment, Some(0x64 | 0x65)) =>
        {
            p(Role::Straight, 0, F_ALL, true)
        }
        Operation::IncDecGsMem { destination, kind }
            if insn.prefixes.group1.is_none()
                && mem_ok(&destination)
                && matches!(destination.size, OperandSize::Dword | OperandSize::Qword)
                && matches!(kind, ArithmeticKind::Inc | ArithmeticKind::Dec) =>
        {
            p(Role::Straight, F_CF, F_ALL & !F_CF, true)
        }
        Operation::MovGsLoadD { src, .. } | Operation::MovGsLoadQ { src, .. }
            if insn.prefixes.group1.is_none() && mem_ok(&src) =>
        {
            p(Role::Straight, 0, 0, true)
        }
        _ => None,
    }
}

/// The REP MOVS/STOS forms the general path admits with 64-bit addresses.
fn rep_admitted(insn: &Instruction) -> bool {
    let p = insn.prefixes;
    match insn.operation {
        Operation::RepMovs { size } => {
            (insn.opcode.byte == 0xa4
                && insn.length == 2
                && p == (Prefixes {
                    group1: Some(0xf3),
                    ..Prefixes::default()
                })
                && size == OperandSize::Byte)
                || (insn.opcode.byte == 0xa5
                    && p.rex.map_or(true, |rex| rex & 8 != 0)
                    && ((p.rex.is_some_and(|rex| rex & 8 != 0) && size == OperandSize::Qword)
                        || (p.rex.is_none() && p.operand_override && size == OperandSize::Word)
                        || (p.rex.is_none() && !p.operand_override && size == OperandSize::Dword)))
        }
        Operation::RepStos { size } => {
            (insn.opcode.byte == 0xaa
                && !p.operand_override
                && p.rex.is_none()
                && insn.length == 2
                && size == OperandSize::Byte)
                || (insn.opcode.byte == 0xab
                    && p.rex.map_or(true, |rex| rex & 8 != 0)
                    && ((p.rex.is_some_and(|rex| rex & 8 != 0) && size == OperandSize::Qword)
                        || (p.rex.is_none() && p.operand_override && size == OperandSize::Word)
                        || (p.rex.is_none() && !p.operand_override && size == OperandSize::Dword)))
        }
        _ => false,
    }
}

fn is_rdtsc(insn: &Instruction) -> bool {
    matches!(insn.operation, Operation::Rdtsc)
        && insn.opcode.map == OpcodeMap::Escape0f
        && insn.opcode.byte == 0x31
        && insn.length == 2
        && insn.prefixes == Prefixes::default()
}

/// Composition of flag definedness over the arithmetic bits: x -> (x & keep) | set.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Defined {
    keep: u64,
    set: u64,
}

impl Defined {
    const IDENTITY: Defined = Defined {
        keep: od_isa::ARITHMETIC_FLAGS,
        set: 0,
    };
    fn then(self, effects: long_decode::FlagEffects) -> Defined {
        let arith = od_isa::ARITHMETIC_FLAGS;
        // f2(x) = (x | d) & !u
        let d = effects.defined & arith;
        let u = effects.undefined & arith;
        let keep2 = arith & !(d | u);
        let set2 = d & !u;
        Defined {
            keep: self.keep & keep2,
            set: ((self.set & keep2) | set2) & arith,
        }
    }
}

fn effects_of(insn: &Instruction) -> long_decode::FlagEffects {
    match insn.operation {
        Operation::Shift {
            count: Operand::Immediate(count),
            ..
        } => insn.flag_effects_for_count(count as u8),
        // A register count: applied by the generated code itself.
        Operation::Shift { .. } => long_decode::FlagEffects::default(),
        _ => insn.flag_effects(),
    }
}

struct SlowStub {
    entry: Label,
    resume: Label,
    exit: Label,
    op: u64,
    value: Option<u32>,
    dst: Option<u32>,
    save: Vec<u32>,
    rip: u64,
}

struct Ctx<'e> {
    asm: Asm,
    engine: &'e TurboEngine,
    gpr: u32,
    gs_base: u32,
    slow: Vec<SlowStub>,
    exits: Vec<(Label, u64, usize, Defined)>,
    /// Exits after an instruction that retired and changed code or tables: (label, next RIP,
    /// index, definedness).
    leaves: Vec<(Label, u64, usize, Defined)>,
    /// Exits of a partial REP (the instruction did not retire).
    partials: Vec<(Label, u64, usize, Defined)>,
    /// After an RDTSC: (boundary exit, short-budget exit, next RIP, index, definedness so far).
    boundaries: Vec<(Label, Label, u64, usize, Defined)>,
    /// The exit before the current instruction.
    exit_here: Option<Label>,
    rip: u64,
    next_rip: u64,
    k: usize,
    len: usize,
    defined: Defined,
}

const T0: u32 = 9;
const T1: u32 = 10;
const T2: u32 = 11;
const ADDR: u32 = 12;

impl<'e> Ctx<'e> {
    fn exit_before(&mut self) -> Label {
        if let Some(label) = self.exit_here {
            return label;
        }
        let label = self.asm.label();
        self.exits.push((label, self.rip, self.k, self.defined));
        self.exit_here = Some(label);
        label
    }

    fn gpr_off(&self, register: Register) -> u32 {
        self.gpr + 8 * register as u32
    }

    fn load_reg(&mut self, rd: u32, view: RegisterView) {
        let off = self.gpr_off(view.register()) + u32::from(view.shift() / 8);
        let size = size_bytes(view.size());
        self.asm.emit(a64::ldr_imm(size, rd, 19, off));
    }

    /// Store `rs` into `view`; a dword view stores the zero-extended low half.
    fn store_reg(&mut self, rs: u32, view: RegisterView, zext32: bool) {
        let off = self.gpr_off(view.register()) + u32::from(view.shift() / 8);
        match view.size() {
            OperandSize::Qword => self.asm.emit(a64::str_imm(8, rs, 19, off)),
            OperandSize::Dword => {
                if !zext32 {
                    self.asm.emit(a64::uxtw(rs, rs));
                }
                self.asm.emit(a64::str_imm(8, rs, 19, off));
            }
            OperandSize::Word => self.asm.emit(a64::str_imm(2, rs, 19, off)),
            OperandSize::Byte => self.asm.emit(a64::str_imm(1, rs, 19, off)),
        }
    }

    fn load_gpr64(&mut self, rd: u32, register: Register) {
        let off = self.gpr_off(register);
        self.asm.emit(a64::ldr_imm(8, rd, 19, off));
    }

    fn store_gpr64(&mut self, rs: u32, register: Register) {
        let off = self.gpr_off(register);
        self.asm.emit(a64::str_imm(8, rs, 19, off));
    }

    fn mov_imm(&mut self, rd: u32, value: u64) {
        self.asm.mov_imm64(rd, value);
    }

    /// Effective address of `mem` (+ the GS base when `gs`) into ADDR.
    fn address(&mut self, mem: &MemoryOperand, gs: bool) {
        let scale_shift = match mem.scale {
            1 => 0,
            2 => 1,
            4 => 2,
            _ => 3,
        };
        match (mem.base, mem.index) {
            (AddressBase::NextRip, None) => {
                let value =
                    self.next_rip.wrapping_add_signed(mem.displacement) & mem.address_size.mask();
                self.mov_imm(ADDR, value);
            }
            (AddressBase::None, None) => {
                let value = (mem.displacement as u64) & mem.address_size.mask();
                self.mov_imm(ADDR, value);
            }
            (base, index) => {
                let mut have = false;
                match base {
                    AddressBase::Register(register) => {
                        self.load_gpr64(ADDR, register);
                        have = true;
                    }
                    AddressBase::NextRip => {
                        self.mov_imm(ADDR, self.next_rip);
                        have = true;
                    }
                    AddressBase::None => {}
                }
                if let Some(index) = index {
                    self.load_gpr64(13, index);
                    if have {
                        self.asm
                            .emit(a64::add_reg(true, ADDR, ADDR, 13, LSL, scale_shift));
                    } else {
                        self.asm.emit(a64::lsl_imm(true, ADDR, 13, scale_shift));
                    }
                    have = true;
                }
                if !have {
                    self.asm.emit(a64::movz(true, ADDR, 0, 0));
                }
                self.asm.add_any(ADDR, ADDR, mem.displacement, 13);
                if mem.address_size == OperandSize::Dword {
                    self.asm.emit(a64::uxtw(ADDR, ADDR));
                }
            }
        }
        if gs {
            self.asm.emit(a64::ldr_imm(8, 13, 19, self.gs_base));
            self.asm.emit(a64::add_reg(true, ADDR, ADDR, 13, LSL, 0));
        }
    }

    /// TLB lookup of an access of `size` bytes at ADDR; on a hit x14 holds the addend. Branches to
    /// a slow stub on a miss.
    fn tlb_check(&mut self, size: u32, write: bool, slow: Label) {
        let asm = &mut self.asm;
        if size > 1 {
            asm.emit(a64::add_imm(true, 13, ADDR, size - 1, false));
            asm.emit(a64::lsr_imm(true, 13, 13, 12));
        } else {
            asm.emit(a64::lsr_imm(true, 13, ADDR, 12));
        }
        asm.emit(a64::ubfx(true, 14, ADDR, 12, TLB_BITS));
        asm.emit(a64::add_reg(
            true,
            14,
            if write { 23 } else { 22 },
            14,
            LSL,
            5,
        ));
        asm.emit(a64::ldp(15, 14, 14, 0));
        asm.emit(a64::eor_reg(true, 15, 15, 13));
        asm.cbnz_to(true, 15, slow);
    }

    /// Load `size` bytes at ADDR into `dst` (zero-extended). `save`: temps live across the access.
    fn load_mem(&mut self, size: u32, dst: u32, save: &[u32]) {
        let slow = self.asm.label();
        let resume = self.asm.label();
        self.tlb_check(size, false, slow);
        self.asm.emit(a64::ldr_reg(size, dst, ADDR, 14));
        self.asm.bind(resume);
        let exit = self.exit_before();
        self.slow.push(SlowStub {
            entry: slow,
            resume,
            exit,
            op: u64::from(size),
            value: None,
            dst: Some(dst),
            save: save.to_vec(),
            rip: self.rip,
        });
    }

    /// Store the low `size` bytes of `value` at ADDR.
    fn store_mem(&mut self, size: u32, value: u32, save: &[u32]) {
        let slow = self.asm.label();
        let resume = self.asm.label();
        self.tlb_check(size, true, slow);
        self.asm.emit(a64::str_reg(size, value, ADDR, 14));
        self.asm.bind(resume);
        let exit = self.exit_before();
        self.slow.push(SlowStub {
            entry: slow,
            resume,
            exit,
            op: u64::from(size) | OP_WRITE,
            value: Some(value),
            dst: None,
            save: save.to_vec(),
            rip: self.rip,
        });
    }

    /// Jump to the code of guest `target` (through the lookup stub).
    fn goto(&mut self, target: u64) {
        self.mov_imm(0, target);
        self.asm.b_abs(self.engine.lookup_stub);
    }

    fn emit_defined(&mut self, defined: Defined) {
        if defined == Defined::IDENTITY {
            return;
        }
        if defined.keep == 0 {
            self.asm.mov_imm32(24, defined.set as u32);
            return;
        }
        if defined.keep != od_isa::ARITHMETIC_FLAGS {
            let clear = od_isa::ARITHMETIC_FLAGS & !defined.keep;
            self.asm.mov_imm32(T2, clear as u32);
            self.asm.emit(a64::bic_reg(true, 24, 24, T2));
        }
        if defined.set != 0 {
            self.asm.mov_imm32(T2, defined.set as u32);
            self.asm.emit(a64::orr_reg(true, 24, 24, T2));
        }
    }

    // ---- flags -------------------------------------------------------------------------------

    /// Flag-setting ADD/SUB family of `a` (T0) and `b` (T1), result into x26 (and `result`),
    /// with AF into x27. `narrow`: 8/16-bit operands (values zero-extended).
    fn arith_flags(&mut self, kind: ArithmeticKind, size: OperandSize, a: u32, b: u32, live: bool) {
        let x64 = size == OperandSize::Qword;
        let asm = &mut self.asm;
        let narrow = matches!(size, OperandSize::Byte | OperandSize::Word);
        let shift = match size {
            OperandSize::Byte => 24,
            OperandSize::Word => 16,
            _ => 0,
        };
        let subtract = matches!(
            kind,
            ArithmeticKind::Sub
                | ArithmeticKind::Cmp
                | ArithmeticKind::Sbb
                | ArithmeticKind::Dec
                | ArithmeticKind::Neg
        );
        if !live {
            // Result only (x26 is not touched: the flags of this instruction are dead).
            let r = T2;
            match kind {
                ArithmeticKind::Add => asm.emit(a64::add_reg(x64, r, a, b, LSL, 0)),
                ArithmeticKind::Sub | ArithmeticKind::Cmp => {
                    asm.emit(a64::sub_reg(x64, r, a, b, LSL, 0))
                }
                ArithmeticKind::Inc => asm.emit(a64::add_imm(x64, r, a, 1, false)),
                ArithmeticKind::Dec => asm.emit(a64::sub_imm(x64, r, a, 1, false)),
                ArithmeticKind::Neg => asm.emit(a64::sub_reg(x64, r, XZR, a, LSL, 0)),
                ArithmeticKind::Adc => {
                    asm.emit(a64::cfinv());
                    asm.emit(a64::adc(x64, r, a, b));
                    asm.emit(a64::cfinv());
                }
                ArithmeticKind::Sbb => asm.emit(a64::sbc(x64, r, a, b)),
            }
            return;
        }
        // Old NZCV for INC/DEC (CF kept).
        if matches!(kind, ArithmeticKind::Inc | ArithmeticKind::Dec) {
            asm.emit(a64::mrs_nzcv(25));
        }
        let (la, lb) = if narrow {
            match kind {
                ArithmeticKind::Inc | ArithmeticKind::Dec => {
                    asm.emit(a64::lsl_imm(false, 13, a, shift));
                    asm.emit(a64::movz(false, 14, 1 << (shift - 16), 1));
                    (13, 14)
                }
                ArithmeticKind::Neg => {
                    asm.emit(a64::lsl_imm(false, 14, a, shift));
                    (XZR, 14)
                }
                _ => {
                    asm.emit(a64::lsl_imm(false, 13, a, shift));
                    asm.emit(a64::lsl_imm(false, 14, b, shift));
                    (13, 14)
                }
            }
        } else {
            match kind {
                ArithmeticKind::Neg => (XZR, a),
                _ => (a, b),
            }
        };
        let fx = x64; // flag op width
        let dst = if narrow { 15 } else { 26 };
        match kind {
            ArithmeticKind::Add => {
                asm.emit(a64::adds_reg(fx, dst, la, lb));
                asm.emit(a64::cfinv());
            }
            ArithmeticKind::Sub | ArithmeticKind::Cmp | ArithmeticKind::Neg => {
                asm.emit(a64::subs_reg(fx, dst, la, lb));
            }
            ArithmeticKind::Inc => {
                if narrow {
                    asm.emit(a64::adds_reg(false, dst, la, lb));
                } else {
                    asm.emit(a64::adds_imm(fx, dst, la, 1, false));
                }
                asm.emit(a64::rmif(25, 28, 2));
            }
            ArithmeticKind::Dec => {
                if narrow {
                    asm.emit(a64::subs_reg(false, dst, la, lb));
                } else {
                    asm.emit(a64::subs_imm(fx, dst, la, 1, false));
                }
                asm.emit(a64::rmif(25, 28, 2));
            }
            ArithmeticKind::Adc => {
                asm.emit(a64::cfinv());
                asm.emit(a64::adcs(fx, dst, la, lb));
                asm.emit(a64::cfinv());
            }
            ArithmeticKind::Sbb => {
                asm.emit(a64::sbcs(fx, dst, la, lb));
            }
        }
        if narrow {
            asm.emit(a64::lsr_imm(false, 26, 15, shift));
        }
        let _ = subtract;
        // AF: bit 4 of a ^ b ^ result (b is 1 for INC/DEC, a is 0 for NEG).
        match kind {
            ArithmeticKind::Inc | ArithmeticKind::Dec => {
                asm.emit(a64::eor_imm(true, 27, a, 1).unwrap());
                asm.emit(a64::eor_reg(true, 27, 27, 26));
            }
            ArithmeticKind::Neg => {
                asm.emit(a64::eor_reg(true, 27, a, 26));
            }
            _ => {
                asm.emit(a64::eor_reg(true, 27, a, b));
                asm.emit(a64::eor_reg(true, 27, 27, 26));
            }
        }
        // The result in T2 for the write-back.
        asm.emit(a64::mov_reg(true, T2, 26));
    }

    /// AND/OR/XOR/TEST of T0, T1 into T2; with `live` the flags (CF = OF = 0, SZP of the result).
    fn logic(&mut self, kind: LogicKind, size: OperandSize, a: u32, b: u32, live: bool) {
        let x64 = size == OperandSize::Qword;
        let asm = &mut self.asm;
        let r = if live { 26 } else { T2 };
        match kind {
            LogicKind::And | LogicKind::Test => asm.emit(a64::and_reg(x64, r, a, b)),
            LogicKind::Or => asm.emit(a64::orr_reg(x64, r, a, b)),
            LogicKind::Xor => asm.emit(a64::eor_reg(x64, r, a, b)),
            LogicKind::Not => {
                asm.emit(a64::mvn(x64, r, a));
                if let Some(mask) = match size {
                    OperandSize::Byte => Some(0xffu64),
                    OperandSize::Word => Some(0xffff),
                    _ => None,
                } {
                    asm.emit(a64::and_imm(false, r, r, mask).unwrap());
                }
                if live {
                    asm.emit(a64::mov_reg(true, T2, 26));
                }
                return;
            }
        }
        if live {
            match size {
                OperandSize::Byte => {
                    asm.emit(a64::lsl_imm(false, 13, 26, 24));
                    asm.emit(a64::tst_reg(false, 13, 13));
                }
                OperandSize::Word => {
                    asm.emit(a64::lsl_imm(false, 13, 26, 16));
                    asm.emit(a64::tst_reg(false, 13, 13));
                }
                _ => asm.emit(a64::tst_reg(x64, 26, 26)),
            }
            asm.emit(a64::cfinv());
            asm.emit(a64::mov_reg(true, T2, 26));
        }
    }

    /// x`rd` bit 0 = XOR of the low 8 bits of x26 (odd parity of the PF source).
    fn parity_bit(&mut self, rd: u32) {
        let asm = &mut self.asm;
        asm.emit(a64::eor_reg_shift(false, rd, 26, 26, a64::LSR, 4));
        asm.emit(a64::eor_reg_shift(false, rd, rd, rd, a64::LSR, 2));
        asm.emit(a64::eor_reg_shift(false, rd, rd, rd, a64::LSR, 1));
    }

    /// `rd` = 1 when `condition` holds on the guest flags, else 0 (host flags kept).
    fn cond_value(&mut self, condition: ConditionCode, rd: u32) {
        match arm_cond(condition) {
            Some(cond) => self.asm.emit(a64::cset(false, rd, cond)),
            None => {
                self.parity_bit(rd);
                self.asm.emit(a64::and_imm(false, rd, rd, 1).unwrap());
                if condition == ConditionCode::Parity {
                    self.asm.emit(a64::eor_imm(false, rd, rd, 1).unwrap());
                }
            }
        }
    }

    /// BT/BTS/BTR/BTC with a register base, or a memory base and an immediate offset.
    fn bit(
        &mut self,
        kind: od_isa::long_decode::BitKind,
        dst: Operand,
        offset: od_isa::long_decode::BitOffset,
        live_after: u8,
    ) -> bool {
        use od_isa::long_decode::{BitKind, BitOffset};
        let size = match dst {
            Operand::Register(view) => view.size(),
            Operand::Memory(mem) => mem.size,
            Operand::Immediate(_) => return false,
        };
        let width = size_bytes(size) * 8;
        let x64 = size == OperandSize::Qword;
        if let (Operand::Memory(mem), BitOffset::Register(view)) = (dst, offset) {
            // The signed offset picks the element: floor(offset / width) elements from the
            // operand, the bit is offset mod width (Euclidean, a power of two).
            if !mem_ok(&mem) {
                return false;
            }
            let log_width = width.trailing_zeros();
            let log_bytes = size_bytes(size).trailing_zeros();
            self.load_reg(17, view);
            match view.size() {
                OperandSize::Dword => self.asm.emit(a64::sxtw(17, 17)),
                OperandSize::Word => self.asm.emit(a64::sxth(17, 17)),
                _ => {}
            }
            self.address(&mem, false);
            let asm = &mut self.asm;
            asm.emit(a64::asr_imm(true, 16, 17, log_width));
            asm.emit(a64::add_reg(true, ADDR, ADDR, 16, LSL, log_bytes));
            asm.emit(a64::and_imm(true, 17, 17, u64::from(width - 1)).unwrap());
            self.load_mem(size_bytes(size), T0, &[17]);
            let asm = &mut self.asm;
            asm.emit(a64::lsrv(true, T1, T0, 17));
            asm.emit(a64::and_imm(false, T1, T1, 1).unwrap());
            asm.emit(a64::movz(true, 16, 1, 0));
            asm.emit(a64::lslv(true, 16, 16, 17));
            match kind {
                BitKind::Test => {}
                BitKind::Set => asm.emit(a64::orr_reg(x64, T2, T0, 16)),
                BitKind::Reset => asm.emit(a64::bic_reg(x64, T2, T0, 16)),
                BitKind::Complement => asm.emit(a64::eor_reg(x64, T2, T0, 16)),
            }
            if kind != BitKind::Test {
                self.store_mem(size_bytes(size), T2, &[T1]);
            }
            if live_after & F_CF != 0 {
                let asm = &mut self.asm;
                asm.emit(a64::eor_imm(false, T1, T1, 1).unwrap());
                asm.emit(a64::rmif(T1, 63, 2));
            }
            return true;
        }
        // The base into T0 first (memory: a fault exits before anything changes); the TLB check
        // uses x13-x15, so the offset goes to x17 after it.
        match dst {
            Operand::Register(view) => self.load_reg(T0, view),
            Operand::Memory(mem) => {
                if !mem_ok(&mem) {
                    return false;
                }
                self.address(&mem, false);
                self.load_mem(size_bytes(size), T0, &[]);
            }
            Operand::Immediate(_) => return false,
        }
        match offset {
            BitOffset::Register(view) => {
                self.load_reg(17, view);
                self.asm
                    .emit(a64::and_imm(false, 17, 17, u64::from(width - 1)).unwrap());
            }
            BitOffset::Immediate(value) => {
                self.asm
                    .emit(a64::movz(false, 17, u32::from(value) & (width - 1), 0));
            }
        }
        let asm = &mut self.asm;
        // T1 = old bit, x16 = mask.
        asm.emit(a64::lsrv(true, T1, T0, 17));
        asm.emit(a64::and_imm(false, T1, T1, 1).unwrap());
        asm.emit(a64::movz(true, 16, 1, 0));
        asm.emit(a64::lslv(true, 16, 16, 17));
        let writes = kind != BitKind::Test;
        match kind {
            BitKind::Test => {}
            BitKind::Set => asm.emit(a64::orr_reg(x64, T2, T0, 16)),
            BitKind::Reset => asm.emit(a64::bic_reg(x64, T2, T0, 16)),
            BitKind::Complement => asm.emit(a64::eor_reg(x64, T2, T0, 16)),
        }
        if writes {
            match dst {
                Operand::Register(view) => self.store_reg(T2, view, true),
                Operand::Memory(_) => self.store_mem(size_bytes(size), T2, &[T1]),
                Operand::Immediate(_) => return false,
            }
        }
        if live_after & F_CF != 0 {
            // C (ARM sense) = !CF.
            let asm = &mut self.asm;
            asm.emit(a64::eor_imm(false, T1, T1, 1).unwrap());
            asm.emit(a64::rmif(T1, 63, 2));
        }
        true
    }

    /// T2 = T0 * T1 truncated to `size` (dword or qword); with `live` CF = OF = overflow.
    fn imul_into(&mut self, size: OperandSize, live: bool) {
        let asm = &mut self.asm;
        if size == OperandSize::Qword {
            asm.emit(a64::mul(true, T2, T0, T1));
            if live {
                asm.emit(a64::smulh(13, T0, T1));
                asm.emit(a64::eor_reg_shift(true, 13, 13, T2, a64::ASR, 63));
            }
        } else {
            asm.emit(a64::smull(T2, T0, T1));
            if live {
                asm.emit(a64::sxtw(13, T2));
                asm.emit(a64::eor_reg(true, 13, 13, T2));
            }
            asm.emit(a64::uxtw(T2, T2));
        }
        if live {
            asm.emit(a64::clz(true, 14, 13));
            asm.emit(a64::lsr_imm(true, 14, 14, 6));
            asm.emit(a64::eor_imm(false, 15, 14, 1).unwrap());
            asm.emit(a64::orr_reg_shift(false, 15, 15, 14, LSL, 1));
            asm.emit(a64::rmif(15, 0, 3));
        }
    }

    /// The write TLB for a read-modify-write of `bytes` at ADDR (x14 = addend on a hit). A miss
    /// makes the locked walk of the general path (accessed and dirty bits) and starts the
    /// instruction again at `restart`; a refusal exits before it.
    fn rmw_check(&mut self, bytes: u32, restart: Label) {
        let slow = self.asm.label();
        self.tlb_check(bytes, true, slow);
        let exit = self.exit_before();
        self.slow.push(SlowStub {
            entry: slow,
            resume: restart,
            exit,
            op: u64::from(bytes) | OP_WRITE | super::OP_ATOMIC,
            value: None,
            dst: None,
            save: Vec::new(),
            rip: self.rip,
        });
    }

    /// An ALU operation on `dst` and `src` of `size` (memory through the GS base with `gs`; a
    /// memory destination through the locked walk with `atomic`).
    fn alu(
        &mut self,
        kind: AluKind,
        dst: Operand,
        src: Operand,
        size: OperandSize,
        gs: bool,
        atomic: bool,
        live_after: u8,
    ) -> bool {
        let writes = !matches!(
            kind,
            AluKind::Arith(ArithmeticKind::Cmp) | AluKind::Logic(LogicKind::Test)
        );
        let live = match kind {
            AluKind::Arith(_) => live_after != 0,
            AluKind::Logic(_) => live_after & (F_ALL & !F_AF) != 0,
        };
        let not = kind == AluKind::Logic(LogicKind::Not);
        match dst {
            Operand::Register(view) => {
                if !not {
                    match src {
                        Operand::Register(view) => self.load_reg(T1, view),
                        Operand::Immediate(value) => self.mov_imm(T1, value & size.mask()),
                        Operand::Memory(mem) => {
                            if !mem_ok(&mem) {
                                return false;
                            }
                            self.address(&mem, gs);
                            self.load_mem(size_bytes(mem.size), T1, &[]);
                        }
                    }
                }
                self.load_reg(T0, view);
                self.compute(kind, size, live && !not);
                if writes {
                    self.store_reg(T2, view, size == OperandSize::Dword);
                }
                true
            }
            Operand::Memory(mem) => {
                if !mem_ok(&mem) {
                    return false;
                }
                let bytes = size_bytes(size);
                if atomic {
                    let start = self.asm.label();
                    self.asm.bind(start);
                    self.address(&mem, gs);
                    self.rmw_check(bytes, start);
                    // ADDR becomes the host address (no slow path after this point).
                    self.asm.emit(a64::add_reg(true, ADDR, ADDR, 14, LSL, 0));
                    self.asm.emit(a64::ldr_imm(bytes, T0, ADDR, 0));
                } else {
                    self.address(&mem, gs);
                    self.load_mem(bytes, T0, &[]);
                }
                if !not {
                    match src {
                        Operand::Register(view) => self.load_reg(T1, view),
                        Operand::Immediate(value) => self.mov_imm(T1, value & size.mask()),
                        Operand::Memory(_) => return false,
                    }
                }
                if writes {
                    self.compute(kind, size, false);
                    if atomic {
                        self.asm.emit(a64::str_imm(bytes, T2, ADDR, 0));
                    } else {
                        self.store_mem(bytes, T2, &[T0, T1]);
                    }
                    if live && !not {
                        self.compute(kind, size, true);
                    }
                } else if live {
                    self.compute(kind, size, true);
                }
                true
            }
            Operand::Immediate(_) => false,
        }
    }

    fn compute(&mut self, kind: AluKind, size: OperandSize, live: bool) {
        match kind {
            AluKind::Arith(kind) => self.arith_flags(kind, size, T0, T1, live),
            AluKind::Logic(kind) => self.logic(kind, size, T0, T1, live),
        }
    }

    /// A load of `mem` (through the GS base with `gs`) into `dst`, zero- or sign-extended.
    fn load_into(&mut self, dst: RegisterView, mem: MemoryOperand, gs: bool, signed: bool) -> bool {
        if !mem_ok(&mem) {
            return false;
        }
        self.address(&mem, gs);
        self.load_mem(size_bytes(mem.size), T0, &[]);
        if signed {
            match mem.size {
                OperandSize::Byte => self.asm.emit(a64::sxtb(T0, T0)),
                OperandSize::Word => self.asm.emit(a64::sxth(T0, T0)),
                OperandSize::Dword => self.asm.emit(a64::sxtw(T0, T0)),
                OperandSize::Qword => {}
            }
            self.store_reg(T0, dst, false);
        } else {
            self.store_reg(T0, dst, true);
        }
        true
    }

    /// A store of `value` (register or immediate) of `mem.size` bytes (GS base with `gs`).
    fn store_into(&mut self, mem: MemoryOperand, value: Operand, gs: bool) -> bool {
        if !mem_ok(&mem) {
            return false;
        }
        match value {
            Operand::Register(view) => self.load_reg(T0, view),
            Operand::Immediate(value) => self.mov_imm(T0, value & mem.size.mask()),
            Operand::Memory(_) => return false,
        }
        self.address(&mem, gs);
        self.store_mem(size_bytes(mem.size), T0, &[]);
        true
    }

    // ---- instructions -----------------------------------------------------------------------

    fn source_value(&mut self, operand: Operand, rd: u32, size: OperandSize, save: &[u32]) {
        match operand {
            Operand::Register(view) => self.load_reg(rd, view),
            Operand::Immediate(value) => self.mov_imm(rd, value & size.mask()),
            Operand::Memory(mem) => {
                self.address(&mem, false);
                self.load_mem(size_bytes(mem.size), rd, save);
            }
        }
    }

    fn emit_insn(&mut self, insn: &Instruction, live_after: u8) -> bool {
        let size = insn.operand_size;
        match insn.operation {
            Operation::Arithmetic { kind, dst, src } => self.alu(
                AluKind::Arith(kind),
                dst,
                src,
                size,
                false,
                false,
                live_after,
            ),
            Operation::Logic { kind, dst, src } => self.alu(
                AluKind::Logic(kind),
                dst,
                src,
                size,
                false,
                false,
                live_after,
            ),
            Operation::Shift {
                kind: kind @ (ShiftKind::Shld | ShiftKind::Shrd),
                dst: Operand::Register(view),
                source: Some(Operand::Register(source)),
                count: Operand::Immediate(count),
                ..
            } => {
                let x64 = size == OperandSize::Qword;
                let width = size_bytes(size) * 8;
                let c = (count as u32) & if width == 64 { 0x3f } else { 0x1f };
                if c == 0 {
                    if size == OperandSize::Dword {
                        self.load_reg(T0, view);
                        self.store_reg(T0, view, true);
                    }
                    return true;
                }
                let live = live_after & (F_CF | F_PF | F_ZF | F_SF | F_OF) != 0;
                self.load_reg(T0, view);
                self.load_reg(T1, source);
                let asm = &mut self.asm;
                let r = if live { 26 } else { T2 };
                if kind == ShiftKind::Shld {
                    asm.emit(a64::extr(x64, r, T0, T1, width - c));
                } else {
                    asm.emit(a64::extr(x64, r, T1, T0, c));
                }
                if live {
                    if kind == ShiftKind::Shld {
                        asm.emit(a64::ubfx(x64, 13, T0, width - c, 1));
                    } else {
                        asm.emit(a64::ubfx(x64, 13, T0, c - 1, 1));
                    }
                    if c != 1 {
                        asm.emit(a64::mrs_nzcv(25));
                    }
                    asm.emit(a64::tst_reg(x64, 26, 26));
                    asm.emit(a64::eor_imm(false, 14, 13, 1).unwrap());
                    if c == 1 {
                        if kind == ShiftKind::Shld {
                            asm.emit(a64::lsr_imm(x64, 15, 26, width - 1));
                            asm.emit(a64::eor_reg(false, 15, 15, 13));
                        } else {
                            asm.emit(a64::lsr_imm(x64, 15, T0, width - 1));
                            asm.emit(a64::lsr_imm(x64, 16, 26, width - 1));
                            asm.emit(a64::eor_reg(false, 15, 15, 16));
                        }
                        asm.emit(a64::orr_reg_shift(false, 14, 15, 14, LSL, 1));
                        asm.emit(a64::rmif(14, 0, 3));
                    } else {
                        asm.emit(a64::rmif(14, 63, 2));
                        asm.emit(a64::rmif(25, 28, 1));
                    }
                    asm.emit(a64::mov_reg(true, T2, 26));
                }
                self.store_reg(T2, view, size == OperandSize::Dword);
                true
            }
            Operation::Shift {
                kind,
                dst: Operand::Register(view),
                count: Operand::Immediate(count),
                ..
            } if matches!(size, OperandSize::Byte | OperandSize::Word) => {
                // 8/16-bit SHL/SHR/SAR by 1..width-1 (the plan admits nothing else).
                let width = size_bytes(size) * 8;
                let c = (count as u32) & 0x1f;
                if c == 0 {
                    return true;
                }
                let live = live_after & (F_CF | F_PF | F_ZF | F_SF | F_OF) != 0;
                self.load_reg(T0, view);
                let asm = &mut self.asm;
                let r = if live { 26 } else { T2 };
                match kind {
                    ShiftKind::Shl => {
                        asm.emit(a64::lsl_imm(false, r, T0, c));
                        asm.emit(a64::and_imm(false, r, r, (1u64 << width) - 1).unwrap());
                    }
                    ShiftKind::Shr => asm.emit(a64::lsr_imm(false, r, T0, c)),
                    _ => {
                        // Arithmetic: sign-extend the narrow value first.
                        asm.emit(a64::sbfx(false, r, T0, 0, width));
                        asm.emit(a64::asr_imm(false, r, r, c));
                        asm.emit(a64::and_imm(false, r, r, (1u64 << width) - 1).unwrap());
                    }
                }
                if live {
                    match kind {
                        ShiftKind::Shl => asm.emit(a64::ubfx(false, 13, T0, width - c, 1)),
                        ShiftKind::Shr => asm.emit(a64::ubfx(false, 13, T0, c - 1, 1)),
                        _ => {
                            asm.emit(a64::sbfx(false, 13, T0, 0, width));
                            asm.emit(a64::asr_imm(false, 13, 13, c - 1));
                            asm.emit(a64::and_imm(false, 13, 13, 1).unwrap());
                        }
                    }
                    if c != 1 {
                        asm.emit(a64::mrs_nzcv(25));
                    }
                    asm.emit(a64::lsl_imm(false, 14, 26, 32 - width));
                    asm.emit(a64::tst_reg(false, 14, 14));
                    asm.emit(a64::eor_imm(false, 14, 13, 1).unwrap());
                    if c == 1 {
                        match kind {
                            ShiftKind::Shl => {
                                asm.emit(a64::lsr_imm(false, 15, 26, width - 1));
                                asm.emit(a64::eor_reg(false, 15, 15, 13));
                            }
                            ShiftKind::Shr => asm.emit(a64::ubfx(false, 15, T0, width - 1, 1)),
                            _ => asm.emit(a64::movz(false, 15, 0, 0)),
                        }
                        asm.emit(a64::orr_reg_shift(false, 14, 15, 14, LSL, 1));
                        asm.emit(a64::rmif(14, 0, 3));
                    } else {
                        asm.emit(a64::rmif(14, 63, 2));
                        asm.emit(a64::rmif(25, 28, 1));
                    }
                    asm.emit(a64::mov_reg(true, T2, 26));
                }
                self.store_reg(T2, view, true);
                true
            }
            Operation::Shift {
                kind,
                dst: Operand::Register(view),
                count: Operand::Immediate(count),
                ..
            } => {
                let x64 = size == OperandSize::Qword;
                let width = size_bytes(size) * 8;
                let c = (count as u32) & if width == 64 { 0x3f } else { 0x1f };
                if c == 0 {
                    // No flag, but the destination is written (a dword one zero-extends).
                    if size == OperandSize::Dword {
                        self.load_reg(T0, view);
                        self.store_reg(T0, view, true);
                    }
                    return true;
                }
                if matches!(kind, ShiftKind::Rol | ShiftKind::Ror) {
                    let live = live_after & (F_CF | F_OF) != 0;
                    self.load_reg(T0, view);
                    let asm = &mut self.asm;
                    let rotate = if kind == ShiftKind::Rol { width - c } else { c };
                    asm.emit(a64::ror_imm(x64, T2, T0, rotate));
                    if live {
                        // CF: ROL the low bit of the result, ROR its high bit.
                        if kind == ShiftKind::Rol {
                            asm.emit(a64::and_imm(false, 13, T2, 1).unwrap());
                        } else {
                            asm.emit(a64::lsr_imm(x64, 13, T2, width - 1));
                        }
                        asm.emit(a64::eor_imm(false, 14, 13, 1).unwrap());
                        if c == 1 {
                            if kind == ShiftKind::Rol {
                                asm.emit(a64::lsr_imm(x64, 15, T2, width - 1));
                                asm.emit(a64::eor_reg(false, 15, 15, 13));
                            } else {
                                asm.emit(a64::lsr_imm(x64, 15, T2, width - 1));
                                asm.emit(a64::ubfx(x64, 16, T2, width - 2, 1));
                                asm.emit(a64::eor_reg(false, 15, 15, 16));
                            }
                            asm.emit(a64::orr_reg_shift(false, 14, 15, 14, LSL, 1));
                            asm.emit(a64::rmif(14, 0, 3));
                        } else {
                            asm.emit(a64::rmif(14, 63, 2));
                        }
                    }
                    self.store_reg(T2, view, size == OperandSize::Dword);
                    return true;
                }
                let live = live_after & (F_CF | F_PF | F_ZF | F_SF | F_OF) != 0;
                self.load_reg(T0, view);
                let asm = &mut self.asm;
                let r = if live { 26 } else { T2 };
                match kind {
                    ShiftKind::Shl => asm.emit(a64::lsl_imm(x64, r, T0, c)),
                    ShiftKind::Shr => asm.emit(a64::lsr_imm(x64, r, T0, c)),
                    _ => asm.emit(a64::asr_imm(x64, r, T0, c)),
                }
                if live {
                    // CF: the last bit out.
                    match kind {
                        ShiftKind::Shl => asm.emit(a64::ubfx(x64, 13, T0, width - c, 1)),
                        _ => asm.emit(a64::ubfx(x64, 13, T0, c - 1, 1)),
                    }
                    if c != 1 {
                        asm.emit(a64::mrs_nzcv(25));
                    }
                    asm.emit(a64::tst_reg(x64, 26, 26));
                    // C (ARM sense) = !CF: bit 1 of x14.
                    asm.emit(a64::eor_imm(false, 14, 13, 1).unwrap());
                    if c == 1 {
                        // OF into bit 0.
                        match kind {
                            ShiftKind::Shl => {
                                // MSB(result) ^ CF
                                asm.emit(a64::lsr_imm(x64, 15, 26, width - 1));
                                asm.emit(a64::eor_reg(false, 15, 15, 13));
                            }
                            ShiftKind::Shr => asm.emit(a64::lsr_imm(x64, 15, T0, width - 1)),
                            _ => asm.emit(a64::movz(false, 15, 0, 0)),
                        }
                        asm.emit(a64::orr_reg_shift(false, 14, 15, 14, LSL, 1));
                        asm.emit(a64::rmif(14, 0, 3));
                    } else {
                        asm.emit(a64::rmif(14, 63, 2));
                        asm.emit(a64::rmif(25, 28, 1));
                    }
                    asm.emit(a64::mov_reg(true, T2, 26));
                }
                self.store_reg(T2, view, size == OperandSize::Dword);
                true
            }
            Operation::Shift {
                kind,
                dst: Operand::Register(view),
                count: Operand::Register(count_view),
                ..
            } => {
                let x64 = size == OperandSize::Qword;
                let width = size_bytes(size) * 8;
                // The composition so far is applied first: this one is decided at run time.
                let defined = self.defined;
                self.emit_defined(defined);
                self.defined = Defined::IDENTITY;
                self.load_reg(T0, view);
                self.load_reg(13, count_view);
                let zero = self.asm.label();
                let done = self.asm.label();
                let asm = &mut self.asm;
                asm.emit(a64::and_imm(false, 13, 13, u64::from(width - 1)).unwrap());
                asm.cbz_to(false, 13, zero);
                asm.emit(a64::mrs_nzcv(25));
                match kind {
                    ShiftKind::Shl => {
                        asm.emit(a64::lslv(x64, 26, T0, 13));
                        // CF = bit (width - c): shift right by (width - c) mod width.
                        asm.emit(a64::sub_reg(false, 14, XZR, 13, LSL, 0));
                        asm.emit(a64::lsrv(x64, 15, T0, 14));
                    }
                    ShiftKind::Shr => {
                        asm.emit(a64::lsrv(x64, 26, T0, 13));
                        asm.emit(a64::sub_imm(false, 14, 13, 1, false));
                        asm.emit(a64::lsrv(x64, 15, T0, 14));
                    }
                    _ => {
                        asm.emit(a64::asrv(x64, 26, T0, 13));
                        asm.emit(a64::sub_imm(false, 14, 13, 1, false));
                        asm.emit(a64::lsrv(x64, 15, T0, 14));
                    }
                }
                asm.emit(a64::and_imm(false, 15, 15, 1).unwrap());
                // OF of a count of 1 (x16), the old OF (x17).
                match kind {
                    ShiftKind::Shl => {
                        asm.emit(a64::lsr_imm(x64, 16, 26, width - 1));
                        asm.emit(a64::eor_reg(false, 16, 16, 15));
                    }
                    ShiftKind::Shr => asm.emit(a64::lsr_imm(x64, 16, T0, width - 1)),
                    _ => asm.emit(a64::movz(false, 16, 0, 0)),
                }
                asm.emit(a64::ubfx(false, 17, 25, 28, 1));
                asm.emit(a64::subs_imm(false, XZR, 13, 1, false));
                asm.emit(a64::csel(false, 16, 16, 17, Cond::Eq));
                // Definedness: a count of 1 defines OF, a larger one makes it undefined; AF is
                // undefined either way.
                let arith = od_isa::ARITHMETIC_FLAGS;
                let one = ((0x8u64 | 0x80 | 0x40 | 0x4 | 0x1 | 0x800) & arith) as u32; // SF ZF PF CF OF
                let many = ((0x80u64 | 0x40 | 0x4 | 0x1) & arith) as u32; // SF ZF PF CF
                asm.mov_imm32(T1, one);
                asm.emit(a64::orr_reg(false, T1, 24, T1));
                asm.mov_imm32(T2, 0x10);
                asm.emit(a64::bic_reg(false, T1, T1, T2));
                asm.mov_imm32(T2, many);
                asm.emit(a64::orr_reg(false, T2, 24, T2));
                asm.mov_imm32(12, 0x10 | 0x800);
                asm.emit(a64::bic_reg(false, T2, T2, 12));
                asm.emit(a64::csel(false, 24, T1, T2, Cond::Eq));
                // N, Z from the result; C = !CF, V = OF.
                asm.emit(a64::tst_reg(x64, 26, 26));
                asm.emit(a64::eor_imm(false, 14, 15, 1).unwrap());
                asm.emit(a64::orr_reg_shift(false, 14, 16, 14, LSL, 1));
                asm.emit(a64::rmif(14, 0, 3));
                asm.emit(a64::mov_reg(true, T2, 26));
                asm.b_to(done);
                asm.bind(zero);
                asm.emit(a64::mov_reg(true, T2, T0));
                asm.bind(done);
                self.store_reg(T2, view, size == OperandSize::Dword);
                true
            }
            _ => self.emit_rest(insn, live_after, size),
        }
    }

    fn emit_rest(&mut self, insn: &Instruction, live_after: u8, size: OperandSize) -> bool {
        match insn.operation {
            Operation::Mov { dst, src } => {
                match (dst, src) {
                    (Operand::Register(d), Operand::Register(s)) => {
                        self.load_reg(T0, s);
                        self.store_reg(T0, d, true);
                    }
                    (Operand::Register(d), Operand::Immediate(value)) => {
                        self.mov_imm(T0, value & d.size().mask());
                        self.store_reg(T0, d, true);
                    }
                    (Operand::Register(d), Operand::Memory(mem)) => {
                        self.address(&mem, false);
                        self.load_mem(size_bytes(mem.size), T0, &[]);
                        self.store_reg(T0, d, true);
                    }
                    (Operand::Memory(mem), Operand::Register(s)) => {
                        self.load_reg(T0, s);
                        self.address(&mem, false);
                        self.store_mem(size_bytes(mem.size), T0, &[]);
                    }
                    (Operand::Memory(mem), Operand::Immediate(value)) => {
                        self.mov_imm(T0, value & mem.size.mask());
                        self.address(&mem, false);
                        self.store_mem(size_bytes(mem.size), T0, &[]);
                    }
                    _ => return false,
                }
                true
            }
            Operation::MovImmediate { dst, value } => {
                match dst {
                    Operand::Register(d) => {
                        self.mov_imm(T0, value & d.size().mask());
                        self.store_reg(T0, d, true);
                    }
                    Operand::Memory(mem) => {
                        self.mov_imm(T0, value & mem.size.mask());
                        self.address(&mem, false);
                        self.store_mem(size_bytes(mem.size), T0, &[]);
                    }
                    Operand::Immediate(_) => return false,
                }
                true
            }
            Operation::Extend { dst, src, signed } => {
                let source_size = match src {
                    Operand::Register(view) => {
                        self.load_reg(T0, view);
                        view.size()
                    }
                    Operand::Memory(mem) => {
                        self.address(&mem, false);
                        self.load_mem(size_bytes(mem.size), T0, &[]);
                        mem.size
                    }
                    Operand::Immediate(_) => return false,
                };
                if signed {
                    match source_size {
                        OperandSize::Byte => self.asm.emit(a64::sxtb(T0, T0)),
                        OperandSize::Word => self.asm.emit(a64::sxth(T0, T0)),
                        OperandSize::Dword => self.asm.emit(a64::sxtw(T0, T0)),
                        OperandSize::Qword => {}
                    }
                    self.store_reg(T0, dst, false);
                } else {
                    self.store_reg(T0, dst, true);
                }
                true
            }
            Operation::Lea { dst, src } => {
                self.address(&src, false);
                self.store_reg(ADDR, dst, src.address_size == OperandSize::Dword);
                true
            }
            Operation::Imul { dst, src } => {
                let live = live_after & (F_CF | F_OF) != 0;
                self.source_value(src, T1, size, &[]);
                self.load_reg(T0, dst);
                let asm = &mut self.asm;
                if size == OperandSize::Qword {
                    asm.emit(a64::mul(true, T2, T0, T1));
                    if live {
                        asm.emit(a64::smulh(13, T0, T1));
                        asm.emit(a64::eor_reg_shift(true, 13, 13, T2, a64::ASR, 63));
                    }
                } else {
                    asm.emit(a64::smull(T2, T0, T1));
                    if live {
                        asm.emit(a64::sxtw(13, T2));
                        asm.emit(a64::eor_reg(true, 13, 13, T2));
                    }
                    asm.emit(a64::uxtw(T2, T2));
                }
                if live {
                    // x14 = 1 when no overflow (C, ARM sense), x15 = overflow (V).
                    asm.emit(a64::clz(true, 14, 13));
                    asm.emit(a64::lsr_imm(true, 14, 14, 6));
                    asm.emit(a64::eor_imm(false, 15, 14, 1).unwrap());
                    asm.emit(a64::orr_reg_shift(false, 15, 15, 14, LSL, 1));
                    asm.emit(a64::rmif(15, 0, 3));
                }
                self.store_reg(T2, dst, true);
                true
            }
            Operation::StackRegister { push, register } => {
                let bytes = size_bytes(size);
                if push {
                    let view = RegisterView::low(register, size);
                    self.load_reg(T0, view);
                    self.load_gpr64(ADDR, Register::Rsp);
                    self.asm.emit(a64::sub_imm(true, ADDR, ADDR, bytes, false));
                    self.store_mem(bytes, T0, &[]);
                    self.store_gpr64(ADDR, Register::Rsp);
                } else {
                    self.load_gpr64(ADDR, Register::Rsp);
                    self.load_mem(bytes, T0, &[]);
                    self.asm.emit(a64::add_imm(true, ADDR, ADDR, bytes, false));
                    self.store_gpr64(ADDR, Register::Rsp);
                    let view = RegisterView::low(register, size);
                    self.store_reg(T0, view, true);
                }
                true
            }
            Operation::Nop | Operation::NopModrm | Operation::Pause | Operation::Endbr64Nop => true,
            Operation::SetCondition { condition, dst, .. } => {
                self.cond_value(condition, T0);
                match dst {
                    Operand::Register(view) => self.store_reg(T0, view, true),
                    Operand::Memory(mem) => {
                        self.address(&mem, false);
                        self.store_mem(1, T0, &[]);
                    }
                    Operand::Immediate(_) => return false,
                }
                true
            }
            Operation::ConditionalMove {
                condition,
                dst,
                src,
            } => {
                // The source is read whatever the condition.
                let size = dst.size();
                self.source_value(src, T1, size, &[]);
                self.load_reg(T0, dst);
                match arm_cond(condition) {
                    Some(cond) => self.asm.emit(a64::csel(true, T2, T1, T0, cond)),
                    None => {
                        self.parity_bit(13);
                        // x13 bit 0 = odd parity; PF = even.
                        let keep = if condition == ConditionCode::Parity {
                            Cond::Ne
                        } else {
                            Cond::Eq
                        };
                        self.asm.emit(a64::and_imm(false, 13, 13, 1).unwrap());
                        self.asm.emit(a64::mrs_nzcv(25));
                        self.asm.emit(a64::tst_reg(false, 13, 13));
                        self.asm.emit(a64::csel(true, T2, T0, T1, keep));
                        self.asm.emit(a64::msr_nzcv(25));
                    }
                }
                self.store_reg(T2, dst, size != OperandSize::Dword);
                true
            }
            Operation::Rdtsc => {
                let exit = self.exit_before();
                let boundary = self.asm.label();
                let short = self.asm.label();
                self.boundaries
                    .push((boundary, short, self.next_rip, self.k, self.defined));
                let rest = (self.len - self.k - 1) as u32;
                let here = (self.len - self.k) as u32;
                let asm = &mut self.asm;
                asm.emit(a64::mrs_nzcv(25));
                asm.emit(a64::mov_reg(true, 0, 21));
                asm.emit(a64::mov_reg(true, 1, 19));
                asm.mov_imm64(2, self.rip);
                asm.emit(a64::add_imm(true, 3, 20, here, false));
                asm.emit(a64::ldr_imm(8, 16, 21, super::CTX_RDTSC_HELPER));
                asm.emit(a64::blr(16));
                asm.emit(a64::msr_nzcv(25));
                let negative = asm.label();
                asm.tbnz_to(0, 63, negative);
                asm.emit(a64::sub_imm(true, 20, 0, rest, false));
                asm.tbnz_to(20, 63, short);
                let done = asm.label();
                asm.b_to(done);
                asm.bind(negative);
                // -3: nothing happened, the general path runs it.
                asm.emit(a64::add_imm(true, 9, 0, 3, false));
                asm.cbz_to(true, 9, exit);
                asm.b_to(boundary);
                asm.bind(done);
                true
            }
            Operation::RepMovs { size } | Operation::RepStos { size } => {
                let movs = matches!(insn.operation, Operation::RepMovs { .. });
                let exit = self.exit_before();
                let partial = self.asm.label();
                self.partials
                    .push((partial, self.rip, self.k, self.defined));
                let done = self.asm.label();
                let asm = &mut self.asm;
                asm.emit(a64::mrs_nzcv(25));
                asm.emit(a64::mov_reg(true, 0, 21));
                asm.emit(a64::mov_reg(true, 1, 19));
                asm.mov_imm64(2, size.bytes() as u64 | if movs { 0x10 } else { 0 });
                asm.mov_imm64(3, u64::from(insn.length));
                asm.mov_imm64(4, self.rip);
                asm.emit(a64::ldr_imm(8, 16, 21, super::CTX_REP_HELPER));
                asm.emit(a64::blr(16));
                asm.emit(a64::msr_nzcv(25));
                asm.cbz_to(true, 0, done);
                asm.tbnz_to(0, 1, partial);
                let leave = asm.label();
                asm.tbnz_to(0, 2, leave);
                asm.b_to(exit);
                self.leaves
                    .push((leave, self.next_rip, self.k, self.defined));
                self.asm.bind(done);
                true
            }
            Operation::Leave => {
                self.load_gpr64(ADDR, Register::Rbp);
                self.load_mem(8, T0, &[]);
                self.asm.emit(a64::add_imm(true, ADDR, ADDR, 8, false));
                self.store_gpr64(ADDR, Register::Rsp);
                self.store_gpr64(T0, Register::Rbp);
                true
            }
            Operation::AddGsLoad { dst, src } => self.alu(
                AluKind::Arith(ArithmeticKind::Add),
                Operand::Register(dst),
                Operand::Memory(src),
                dst.size(),
                true,
                false,
                live_after,
            ),
            Operation::CmpGsLoad { dst, src } => self.alu(
                AluKind::Arith(ArithmeticKind::Cmp),
                Operand::Register(dst),
                Operand::Memory(src),
                dst.size(),
                true,
                false,
                live_after,
            ),
            Operation::LogicGsLoad { kind, dst, src } => self.alu(
                AluKind::Logic(kind),
                Operand::Register(dst),
                Operand::Memory(src),
                dst.size(),
                true,
                false,
                live_after,
            ),
            Operation::CmpGsMemImm { dst, immediate } => self.alu(
                AluKind::Arith(ArithmeticKind::Cmp),
                Operand::Memory(dst),
                Operand::Immediate(immediate),
                dst.size,
                true,
                false,
                live_after,
            ),
            Operation::AddGsMemImm { dst, immediate } => self.alu(
                AluKind::Arith(ArithmeticKind::Add),
                Operand::Memory(dst),
                Operand::Immediate(immediate),
                dst.size,
                true,
                false,
                live_after,
            ),
            Operation::SubGsMemImm { dst, immediate } => self.alu(
                AluKind::Arith(ArithmeticKind::Sub),
                Operand::Memory(dst),
                Operand::Immediate(immediate),
                dst.size,
                true,
                false,
                live_after,
            ),
            Operation::LogicGsMemImm {
                kind,
                dst,
                immediate,
            } => self.alu(
                AluKind::Logic(kind),
                Operand::Memory(dst),
                Operand::Immediate(immediate),
                dst.size,
                true,
                false,
                live_after,
            ),
            Operation::AddGsMem {
                destination,
                source,
            } => self.alu(
                AluKind::Arith(ArithmeticKind::Add),
                Operand::Memory(destination),
                Operand::Register(source),
                destination.size,
                true,
                false,
                live_after,
            ),
            Operation::TestGsMemByte { src, immediate } => self.alu(
                AluKind::Logic(LogicKind::Test),
                Operand::Memory(src),
                Operand::Immediate(u64::from(immediate)),
                OperandSize::Byte,
                true,
                false,
                live_after,
            ),
            Operation::TestGsMemWord { src, immediate } => self.alu(
                AluKind::Logic(LogicKind::Test),
                Operand::Memory(src),
                Operand::Immediate(u64::from(immediate)),
                OperandSize::Word,
                true,
                false,
                live_after,
            ),
            Operation::TestGsMemWide { src, immediate } => self.alu(
                AluKind::Logic(LogicKind::Test),
                Operand::Memory(src),
                Operand::Immediate(immediate),
                src.size,
                true,
                false,
                live_after,
            ),
            Operation::MovGsStore { dst, src } => {
                self.store_into(dst, Operand::Register(src), true)
            }
            Operation::MovGsMemByteImm { dst, immediate } => {
                self.store_into(dst, Operand::Immediate(u64::from(immediate)), true)
            }
            Operation::MovGsMemImm { dst, immediate } => {
                self.store_into(dst, Operand::Immediate(immediate), true)
            }
            Operation::MovzxGsLoadB { dst, src } | Operation::MovzxGsLoadW { dst, src } => {
                self.load_into(dst, src, true, false)
            }
            Operation::MovsxGsLoadB { dst, src } | Operation::MovsxdGsLoad { dst, src } => {
                self.load_into(dst, src, true, true)
            }
            Operation::LockIncMem { destination } => self.alu(
                AluKind::Arith(ArithmeticKind::Inc),
                Operand::Memory(destination),
                Operand::Immediate(1),
                destination.size,
                false,
                true,
                live_after,
            ),
            Operation::LockDecMem { destination } => self.alu(
                AluKind::Arith(ArithmeticKind::Dec),
                Operand::Memory(destination),
                Operand::Immediate(1),
                destination.size,
                false,
                true,
                live_after,
            ),
            Operation::LockAddMem {
                destination,
                immediate,
            } => self.alu(
                AluKind::Arith(ArithmeticKind::Add),
                Operand::Memory(destination),
                Operand::Immediate(immediate),
                destination.size,
                false,
                true,
                live_after,
            ),
            Operation::LockSubMem {
                destination,
                immediate,
            }
            | Operation::LockSubDwordImm8 {
                destination,
                immediate,
            } => self.alu(
                AluKind::Arith(ArithmeticKind::Sub),
                Operand::Memory(destination),
                Operand::Immediate(immediate),
                destination.size,
                false,
                true,
                live_after,
            ),
            Operation::LockAddMemReg {
                destination,
                source,
            } => self.alu(
                AluKind::Arith(ArithmeticKind::Add),
                Operand::Memory(destination),
                Operand::Register(source),
                destination.size,
                false,
                true,
                live_after,
            ),
            Operation::LockSubMemReg {
                destination,
                source,
            } => self.alu(
                AluKind::Arith(ArithmeticKind::Sub),
                Operand::Memory(destination),
                Operand::Register(source),
                destination.size,
                false,
                true,
                live_after,
            ),
            Operation::LockOrMem {
                destination,
                immediate,
            } => self.alu(
                AluKind::Logic(LogicKind::Or),
                Operand::Memory(destination),
                Operand::Immediate(immediate),
                destination.size,
                false,
                true,
                live_after,
            ),
            Operation::LockAndMem {
                destination,
                immediate,
            } => self.alu(
                AluKind::Logic(LogicKind::And),
                Operand::Memory(destination),
                Operand::Immediate(immediate),
                destination.size,
                false,
                true,
                live_after,
            ),
            Operation::LockXorMem {
                destination,
                immediate,
            } => self.alu(
                AluKind::Logic(LogicKind::Xor),
                Operand::Memory(destination),
                Operand::Immediate(immediate),
                destination.size,
                false,
                true,
                live_after,
            ),
            Operation::LockLogicMemReg {
                kind,
                destination,
                source,
            } => self.alu(
                AluKind::Logic(kind),
                Operand::Memory(destination),
                Operand::Register(source),
                destination.size,
                false,
                true,
                live_after,
            ),
            Operation::Bswap { dst } => {
                self.load_reg(T0, dst);
                self.asm
                    .emit(a64::rev(dst.size() == OperandSize::Qword, T0, T0));
                self.store_reg(T0, dst, true);
                true
            }
            Operation::Invlpg | Operation::Lfence => true,
            Operation::ClearDirectionFlag | Operation::SetDirectionFlag => {
                let rflags = self.engine.offsets.rflags;
                let asm = &mut self.asm;
                asm.emit(a64::ldr_imm(8, T0, 19, rflags));
                if matches!(insn.operation, Operation::ClearDirectionFlag) {
                    asm.emit(a64::and_imm(true, T0, T0, !(1u64 << 10)).unwrap());
                } else {
                    asm.emit(a64::orr_imm(true, T0, T0, 1u64 << 10).unwrap());
                }
                asm.emit(a64::str_imm(8, T0, 19, rflags));
                true
            }
            Operation::Bit { kind, dst, offset } => self.bit(kind, dst, offset, live_after),
            Operation::Divide { signed, divisor } => {
                // The source first (an exit happens before anything changes), then the helper.
                self.source_value(divisor, T1, size, &[]);
                let exit = self.exit_before();
                let asm = &mut self.asm;
                asm.emit(a64::mrs_nzcv(25));
                asm.emit(a64::mov_reg(true, 0, 19));
                asm.emit(a64::mov_reg(true, 1, T1));
                asm.mov_imm64(
                    2,
                    u64::from(size_bytes(size)) | if signed { 0x100 } else { 0 },
                );
                asm.emit(a64::ldr_imm(8, 16, 21, super::CTX_DIV_HELPER));
                asm.emit(a64::blr(16));
                asm.emit(a64::msr_nzcv(25));
                asm.cbnz_to(true, 0, exit);
                true
            }
            Operation::SubGsMem { dst, src } => {
                self.address(&src, true);
                self.load_mem(8, T1, &[]);
                self.load_reg(T0, dst);
                self.arith_flags(
                    ArithmeticKind::Sub,
                    OperandSize::Qword,
                    T0,
                    T1,
                    live_after != 0,
                );
                self.store_reg(T2, dst, false);
                true
            }
            Operation::CmpGsMem { dst, src } => {
                self.address(&dst, true);
                self.load_mem(size_bytes(dst.size), T0, &[]);
                self.load_reg(T1, src);
                if live_after != 0 {
                    self.arith_flags(ArithmeticKind::Cmp, dst.size, T0, T1, true);
                }
                true
            }
            Operation::BitScan { kind, dst, src } => {
                self.source_value(src, T1, dst.size(), &[]);
                self.load_gpr64(T0, dst.register());
                let asm = &mut self.asm;
                match kind {
                    od_isa::long_decode::BitScanKind::Forward => {
                        asm.emit(a64::rbit(true, 13, T1));
                        asm.emit(a64::clz(true, 13, 13));
                    }
                    od_isa::long_decode::BitScanKind::Reverse => {
                        asm.emit(a64::clz(true, 13, T1));
                        asm.emit(a64::movz(false, 14, 63, 0));
                        asm.emit(a64::sub_reg(false, 13, 14, 13, LSL, 0));
                    }
                }
                // The destination only for a nonzero source (a zero one leaves all 64 bits).
                asm.emit(a64::tst_reg(true, T1, T1));
                asm.emit(a64::csel(true, T2, T0, 13, Cond::Eq));
                // PF: parity of the whole source folded into the low byte of x26; AF clear.
                asm.emit(a64::eor_reg_shift(true, 26, T1, T1, a64::LSR, 32));
                asm.emit(a64::eor_reg_shift(false, 26, 26, 26, a64::LSR, 16));
                asm.emit(a64::eor_reg_shift(false, 26, 26, 26, a64::LSR, 8));
                asm.emit(a64::and_imm(false, 26, 26, 0xff).unwrap());
                asm.emit(a64::movz(true, 27, 0, 0));
                // N = 0, C = 1 (CF = 0), V = 0; Z from the tst above.
                asm.emit(a64::movz(false, 14, 2, 0));
                asm.emit(a64::rmif(14, 0, 0b1011));
                let off = self.gpr_off(dst.register());
                match dst.size() {
                    OperandSize::Word => self.asm.emit(a64::str_imm(2, T2, 19, off)),
                    _ => self.asm.emit(a64::str_imm(8, T2, 19, off)),
                }
                true
            }
            Operation::PushFlags => {
                // RFLAGS: the bits outside the arithmetic ones from the state, those from the
                // host flags, PF and AF from x26/x27; VM and RF clear in the image.
                let rflags = self.engine.offsets.rflags;
                let asm = &mut self.asm;
                asm.emit(a64::ldr_imm(8, T0, 19, rflags));
                asm.mov_imm64(13, 0x00fc_ffff & !od_isa::ARITHMETIC_FLAGS);
                asm.emit(a64::and_reg(true, T0, T0, 13));
                asm.emit(a64::mrs_nzcv(15));
                asm.emit(a64::ubfx(false, 16, 15, 31, 1));
                asm.emit(a64::orr_reg_shift(true, T0, T0, 16, LSL, 7));
                asm.emit(a64::ubfx(false, 16, 15, 30, 1));
                asm.emit(a64::orr_reg_shift(true, T0, T0, 16, LSL, 6));
                asm.emit(a64::ubfx(false, 16, 15, 29, 1));
                asm.emit(a64::eor_imm(false, 16, 16, 1).unwrap());
                asm.emit(a64::orr_reg(true, T0, T0, 16));
                asm.emit(a64::ubfx(false, 16, 15, 28, 1));
                asm.emit(a64::orr_reg_shift(true, T0, T0, 16, LSL, 11));
                asm.emit(a64::eor_reg_shift(false, 16, 26, 26, a64::LSR, 4));
                asm.emit(a64::eor_reg_shift(false, 16, 16, 16, a64::LSR, 2));
                asm.emit(a64::eor_reg_shift(false, 16, 16, 16, a64::LSR, 1));
                asm.emit(a64::and_imm(false, 16, 16, 1).unwrap());
                asm.emit(a64::eor_imm(false, 16, 16, 1).unwrap());
                asm.emit(a64::orr_reg_shift(true, T0, T0, 16, LSL, 2));
                asm.emit(a64::and_imm(false, 16, 27, 0x10).unwrap());
                asm.emit(a64::orr_reg(true, T0, T0, 16));
                self.load_gpr64(ADDR, Register::Rsp);
                self.asm.emit(a64::sub_imm(true, ADDR, ADDR, 8, false));
                self.store_mem(8, T0, &[]);
                self.store_gpr64(ADDR, Register::Rsp);
                true
            }
            Operation::PushImmediate { value } => {
                self.mov_imm(T0, value);
                self.load_gpr64(ADDR, Register::Rsp);
                self.asm.emit(a64::sub_imm(true, ADDR, ADDR, 8, false));
                self.store_mem(8, T0, &[]);
                self.store_gpr64(ADDR, Register::Rsp);
                true
            }
            Operation::SignExtendAccumulator { double_width } => {
                let asm = &mut self.asm;
                let rax = self.gpr + 8 * Register::Rax as u32;
                let rdx = self.gpr + 8 * Register::Rdx as u32;
                match (size, double_width) {
                    (OperandSize::Qword, true) => {
                        asm.emit(a64::ldr_imm(8, T0, 19, rax));
                        asm.emit(a64::asr_imm(true, T0, T0, 63));
                        asm.emit(a64::str_imm(8, T0, 19, rdx));
                    }
                    (OperandSize::Dword, true) => {
                        asm.emit(a64::ldr_imm(4, T0, 19, rax));
                        asm.emit(a64::asr_imm(false, T0, T0, 31));
                        asm.emit(a64::str_imm(8, T0, 19, rdx));
                    }
                    (OperandSize::Qword, false) => {
                        asm.emit(a64::ldrs_imm(4, T0, 19, rax));
                        asm.emit(a64::str_imm(8, T0, 19, rax));
                    }
                    _ => {
                        // CWDE: EAX = sign-extended AX, upper half of RAX clear.
                        asm.emit(a64::ldr_imm(2, T0, 19, rax));
                        asm.emit(a64::sbfx(false, T0, T0, 0, 16));
                        asm.emit(a64::str_imm(8, T0, 19, rax));
                    }
                }
                true
            }
            Operation::ImulImmediate {
                dst,
                src,
                immediate,
            } => {
                let live = live_after & (F_CF | F_OF) != 0;
                self.source_value(src, T0, dst.size(), &[]);
                self.mov_imm(T1, immediate & dst.size().mask());
                self.imul_into(dst.size(), live);
                self.store_reg(T2, dst, true);
                true
            }
            Operation::Mul { signed, operand } => {
                let live = live_after & (F_CF | F_OF) != 0;
                self.source_value(operand, T1, size, &[]);
                let rax = self.gpr + 8 * Register::Rax as u32;
                let rdx = self.gpr + 8 * Register::Rdx as u32;
                let asm = &mut self.asm;
                if size == OperandSize::Qword {
                    asm.emit(a64::ldr_imm(8, T0, 19, rax));
                    asm.emit(a64::mul(true, T2, T0, T1));
                    if signed {
                        asm.emit(a64::smulh(13, T0, T1));
                    } else {
                        asm.emit(a64::umulh(13, T0, T1));
                    }
                    asm.emit(a64::str_imm(8, T2, 19, rax));
                    asm.emit(a64::str_imm(8, 13, 19, rdx));
                    if live {
                        if signed {
                            asm.emit(a64::eor_reg_shift(true, 13, 13, T2, a64::ASR, 63));
                        }
                    }
                } else {
                    asm.emit(a64::ldr_imm(4, T0, 19, rax));
                    if signed {
                        asm.emit(a64::smull(T2, T0, T1));
                    } else {
                        asm.emit(a64::mul(true, T2, T0, T1));
                    }
                    asm.emit(a64::lsr_imm(true, 13, T2, 32));
                    asm.emit(a64::uxtw(14, T2));
                    asm.emit(a64::str_imm(8, 14, 19, rax));
                    asm.emit(a64::uxtw(13, 13));
                    asm.emit(a64::str_imm(8, 13, 19, rdx));
                    if live && signed {
                        asm.emit(a64::sxtw(13, T2));
                        asm.emit(a64::eor_reg(true, 13, 13, T2));
                    }
                }
                if live {
                    // x13 != 0: overflow.
                    asm.emit(a64::clz(true, 14, 13));
                    asm.emit(a64::lsr_imm(true, 14, 14, 6));
                    asm.emit(a64::eor_imm(false, 15, 14, 1).unwrap());
                    asm.emit(a64::orr_reg_shift(false, 15, 15, 14, LSL, 1));
                    asm.emit(a64::rmif(15, 0, 3));
                }
                true
            }
            Operation::Cli => {
                let rflags = self.engine.offsets.rflags;
                let asm = &mut self.asm;
                asm.emit(a64::ldr_imm(8, T0, 19, rflags));
                asm.emit(a64::and_imm(true, T0, T0, !(1u64 << 9)).unwrap());
                asm.emit(a64::str_imm(8, T0, 19, rflags));
                true
            }
            Operation::MovSregStore {
                dst: Operand::Register(view),
                sreg,
                ..
            } => {
                // The visible selector from the context (copied at the start of the run; bit 16
                // says it is modelled).
                let exit = self.exit_before();
                let slot = super::CTX_SELECTORS + 4 * u32::from(sreg);
                self.asm.emit(a64::ldr_imm(4, T0, 21, slot));
                self.asm.tbz_to(T0, 16, exit);
                self.asm.emit(a64::and_imm(false, T0, T0, 0xffff).unwrap());
                self.store_reg(T0, view, true);
                true
            }
            Operation::LockCmpxchgMem {
                destination,
                source,
            }
            | Operation::CmpxchgMemory {
                destination,
                source,
            } => {
                let bytes = size_bytes(destination.size);
                let start = self.asm.label();
                self.asm.bind(start);
                self.address(&destination, false);
                self.rmw_check(bytes, start);
                // Host address in x15; x9 = expected (accumulator), x10 = replacement.
                let accumulator = RegisterView::low(Register::Rax, destination.size);
                self.load_reg(T0, accumulator);
                self.load_reg(T1, source);
                let asm = &mut self.asm;
                asm.emit(a64::add_reg(true, ADDR, ADDR, 14, LSL, 0));
                asm.emit(a64::ldr_imm(bytes, T2, ADDR, 0));
                asm.emit(a64::subs_reg(
                    destination.size == OperandSize::Qword,
                    XZR,
                    T0,
                    T2,
                ));
                asm.emit(a64::csel(true, 13, T1, T2, Cond::Eq));
                asm.emit(a64::str_imm(bytes, 13, ADDR, 0));
                self.arith_flags(ArithmeticKind::Cmp, destination.size, T0, T2, true);
                // RAX only on inequality (a dword write zero-extends RAX).
                let equal = self.asm.label();
                self.asm.b_cond_to(Cond::Eq, equal);
                self.store_reg(T2, accumulator, true);
                self.asm.bind(equal);
                true
            }
            Operation::LockXaddMem {
                destination,
                source,
            }
            | Operation::XaddDsMem {
                destination,
                source,
            }
            | Operation::XaddMemory {
                destination,
                source,
            } => {
                let bytes = size_bytes(destination.size);
                let start = self.asm.label();
                self.asm.bind(start);
                self.address(&destination, false);
                self.rmw_check(bytes, start);
                self.load_reg(T1, source);
                self.asm.emit(a64::add_reg(true, ADDR, ADDR, 14, LSL, 0));
                self.asm.emit(a64::ldr_imm(bytes, T0, ADDR, 0));
                self.arith_flags(ArithmeticKind::Add, destination.size, T0, T1, true);
                self.asm.emit(a64::str_imm(bytes, T2, ADDR, 0));
                self.store_reg(T0, source, true);
                true
            }
            Operation::IncDecGsMem { destination, kind } => {
                let live = live_after & (F_ALL & !F_CF) != 0;
                let bytes = size_bytes(destination.size);
                self.address(&destination, true);
                self.load_mem(bytes, T0, &[]);
                self.arith_flags(kind, destination.size, T0, T1, false);
                self.store_mem(bytes, T2, &[T0]);
                if live {
                    self.arith_flags(kind, destination.size, T0, T1, true);
                }
                true
            }
            Operation::MovGsLoadD { dst, src } | Operation::MovGsLoadQ { dst, src } => {
                self.address(&src, true);
                self.load_mem(size_bytes(src.size), T0, &[]);
                self.store_reg(T0, dst, true);
                true
            }
            _ => false,
        }
    }

    /// The terminator `insn` (it retires; the block ends).
    fn emit_terminator(&mut self, insn: &Instruction) -> bool {
        match insn.operation {
            Operation::Branch {
                condition,
                displacement,
            } => {
                let target = self.next_rip.wrapping_add_signed(i64::from(displacement));
                if !canonical(target) {
                    return false;
                }
                let next = self.next_rip;
                let defined = self.defined;
                self.emit_defined(defined);
                let taken = self.asm.label();
                match arm_cond(condition) {
                    Some(cond) => self.asm.b_cond_to(cond, taken),
                    None => {
                        // PF: even parity of the low byte of x26.
                        let asm = &mut self.asm;
                        asm.emit(a64::eor_reg_shift(false, 13, 26, 26, a64::LSR, 4));
                        asm.emit(a64::eor_reg_shift(false, 13, 13, 13, a64::LSR, 2));
                        asm.emit(a64::eor_reg_shift(false, 13, 13, 13, a64::LSR, 1));
                        // bit 0 of x13 = odd parity
                        if condition == ConditionCode::Parity {
                            asm.tbz_to(13, 0, taken);
                        } else {
                            asm.tbnz_to(13, 0, taken);
                        }
                    }
                }
                self.goto(next);
                self.asm.bind(taken);
                self.goto(target);
                true
            }
            Operation::Jump { displacement } => {
                let target = self.next_rip.wrapping_add_signed(i64::from(displacement));
                if !canonical(target) {
                    return false;
                }
                let defined = self.defined;
                self.emit_defined(defined);
                self.goto(target);
                true
            }
            Operation::Call { displacement } => {
                let target = self.next_rip.wrapping_add_signed(i64::from(displacement));
                if !canonical(target) {
                    return false;
                }
                self.mov_imm(T0, self.next_rip);
                self.load_gpr64(ADDR, Register::Rsp);
                self.asm.emit(a64::sub_imm(true, ADDR, ADDR, 8, false));
                self.store_mem(8, T0, &[]);
                self.store_gpr64(ADDR, Register::Rsp);
                let defined = self.defined;
                self.emit_defined(defined);
                self.goto(target);
                true
            }
            Operation::IndirectCall { operand } | Operation::IndirectJump { operand } => {
                let call = matches!(insn.operation, Operation::IndirectCall { .. });
                match operand {
                    Operand::Register(view) => self.load_reg(T1, view),
                    Operand::Memory(mem) => {
                        self.address(&mem, false);
                        self.load_mem(8, T1, &[]);
                    }
                    Operand::Immediate(_) => return false,
                }
                // A non-canonical target is #GP(0) before the push.
                let exit = self.exit_before();
                self.asm.emit(a64::sbfx(true, 13, T1, 0, 48));
                self.asm.emit(a64::eor_reg(true, 13, 13, T1));
                self.asm.cbnz_to(true, 13, exit);
                if call {
                    self.mov_imm(T0, self.next_rip);
                    self.load_gpr64(ADDR, Register::Rsp);
                    self.asm.emit(a64::sub_imm(true, ADDR, ADDR, 8, false));
                    self.store_mem(8, T0, &[T1]);
                    self.store_gpr64(ADDR, Register::Rsp);
                }
                let defined = self.defined;
                self.emit_defined(defined);
                self.asm.emit(a64::mov_reg(true, 0, T1));
                self.asm.b_abs(self.engine.lookup_stub);
                true
            }
            Operation::Sti => {
                // IF = 1 and the shadow (the helper); the boundary after it is the loop's.
                let defined = self.defined;
                self.emit_defined(defined);
                let asm = &mut self.asm;
                asm.emit(a64::mrs_nzcv(25));
                asm.emit(a64::mov_reg(true, 0, 19));
                asm.emit(a64::ldr_imm(8, 16, 21, super::CTX_STI_HELPER));
                asm.emit(a64::blr(16));
                asm.emit(a64::msr_nzcv(25));
                asm.mov_imm64(1, self.next_rip);
                asm.emit(a64::movz(true, 0, super::EXIT_BOUNDARY as u32, 0));
                let exit_common = self.engine.exit_common;
                self.asm.b_abs(exit_common);
                true
            }
            Operation::Return { stack_adjust } => {
                self.load_gpr64(ADDR, Register::Rsp);
                self.load_mem(8, 0, &[]);
                // A non-canonical return address is #GP(0) with nothing committed.
                let exit = self.exit_before();
                self.asm.emit(a64::sbfx(true, 13, 0, 0, 48));
                self.asm.emit(a64::eor_reg(true, 13, 13, 0));
                self.asm.cbnz_to(true, 13, exit);
                self.asm
                    .add_any(ADDR, ADDR, 8 + i64::from(stack_adjust), 13);
                self.store_gpr64(ADDR, Register::Rsp);
                let defined = self.defined;
                self.emit_defined(defined);
                self.asm.b_abs(self.engine.lookup_stub);
                true
            }
            _ => false,
        }
    }
}

fn form(operand: &Operand) -> &'static str {
    match operand {
        Operand::Register(_) => "r",
        Operand::Memory(_) => "m",
        Operand::Immediate(_) => "i",
    }
}

/// Translate the block at `rip` (on physical page `page`, host bytes at `host_page`).
pub(crate) fn compile(
    engine: &mut TurboEngine,
    rip: u64,
    page: u64,
    host_page: *const u8,
    context: AccessContext,
) -> Compiled {
    compile_limited(engine, rip, page, host_page, context, MAX_BLOCK, None)
}

/// [`compile`] with at most `max` instructions.
pub(crate) fn compile_limited(
    engine: &mut TurboEngine,
    rip: u64,
    page: u64,
    host_page: *const u8,
    context: AccessContext,
    max: usize,
    next_host: Option<*const u8>,
) -> Compiled {
    let mut fetch = CodePage {
        host: host_page,
        page: rip >> 12,
        next: next_host,
        outside: false,
        crossed: false,
    };
    let max = if next_host.is_some() { 1 } else { max };
    let _ = page;
    let mut insns: Vec<(u64, Instruction, Plan)> = Vec::new();
    let mut at = rip;
    while insns.len() < max && (at >> 12 == rip >> 12 || (next_host.is_some() && insns.is_empty()))
    {
        fetch.outside = false;
        let decoded = long_decode::decode(&mut fetch, at, context);
        if fetch.outside {
            if insns.is_empty() && next_host.is_none() {
                return Compiled::Crossing;
            }
            break;
        }
        let Ok(insn) = decoded else {
            break;
        };
        if fetch.crossed && !insns.is_empty() {
            break;
        }
        let Some(plan) = plan(&insn) else {
            engine.pending_decodes.push((at, insn));
            break;
        };
        engine.pending_decodes.push((at, insn));
        insns.push((at, insn, plan));
        at = at.wrapping_add(u64::from(insn.length));
        if plan.role == Role::Terminator {
            break;
        }
    }
    if insns.is_empty() {
        return Compiled::Slow;
    }
    let terminated = insns
        .last()
        .is_some_and(|(_, _, plan)| plan.role == Role::Terminator);
    let end_rip = at;
    // Flag liveness (backward): everything is live at the end and at every possible exit.
    let mut live_after = vec![0u8; insns.len()];
    let mut live = F_ALL;
    for (index, (_, _, plan)) in insns.iter().enumerate().rev() {
        live_after[index] = live;
        let mut before = (live & !plan.writes) | plan.reads;
        if plan.can_exit {
            before = F_ALL;
        }
        live = before;
    }
    let len = insns.len();
    let offsets = engine.offsets;
    let mut ctx = Ctx {
        asm: Asm::new(),
        engine: &*engine,
        gpr: offsets.gpr,
        gs_base: offsets.gs_base,
        slow: Vec::new(),
        exits: Vec::new(),
        leaves: Vec::new(),
        partials: Vec::new(),
        boundaries: Vec::new(),
        exit_here: None,
        rip,
        next_rip: rip,
        k: 0,
        len,
        defined: Defined::IDENTITY,
    };
    // Prologue: the budget.
    let budget_exit = ctx.asm.label();
    ctx.asm.emit(a64::sub_imm(true, 20, 20, len as u32, false));
    ctx.asm.tbnz_to(20, 63, budget_exit);
    for (index, (insn_rip, insn, plan)) in insns.iter().enumerate() {
        ctx.rip = *insn_rip;
        ctx.next_rip = insn_rip.wrapping_add(u64::from(insn.length));
        ctx.k = index;
        ctx.exit_here = None;
        let ok = if plan.role == Role::Terminator {
            // The terminator's definedness is applied before it (none of them change it).
            ctx.defined = ctx.defined.then(effects_of(insn));
            ctx.emit_terminator(insn)
        } else {
            let ok = ctx.emit_insn(insn, live_after[index]);
            ctx.defined = ctx.defined.then(effects_of(insn));
            ok
        };
        if !ok {
            // Should not happen (plan and emitter agree); give the block up.
            return Compiled::Slow;
        }
    }
    if !terminated {
        let defined = ctx.defined;
        ctx.emit_defined(defined);
        ctx.goto(end_rip);
    }
    // Budget exit: nothing ran.
    ctx.asm.bind(budget_exit);
    ctx.asm.emit(a64::add_imm(true, 20, 20, len as u32, false));
    ctx.asm.mov_imm64(1, rip);
    ctx.asm.emit(a64::movz(true, 0, EXIT_BUDGET as u32, 0));
    let exit_common = ctx.engine.exit_common;
    ctx.asm.b_abs(exit_common);
    // Slow stubs.
    let slow = std::mem::take(&mut ctx.slow);
    for stub in slow {
        let asm = &mut ctx.asm;
        asm.bind(stub.entry);
        asm.emit(a64::mrs_nzcv(25));
        let mut saves = stub.save.clone();
        if !saves.contains(&ADDR) {
            saves.push(ADDR);
        }
        if saves.len() % 2 == 1 {
            saves.push(XZR);
        }
        for pair in saves.chunks(2) {
            let second = if pair[1] == XZR { 15 } else { pair[1] };
            asm.emit(a64::stp_pre(pair[0], second, a64::SP, -16));
        }
        asm.emit(a64::mov_reg(true, 0, 21));
        asm.emit(a64::mov_reg(true, 1, ADDR));
        asm.mov_imm64(2, stub.op);
        match stub.value {
            Some(value) => asm.emit(a64::mov_reg(true, 3, value)),
            None => asm.emit(a64::movz(true, 3, 0, 0)),
        }
        asm.mov_imm64(4, stub.rip);
        asm.emit(a64::ldr_imm(8, 16, 21, super::CTX_MEM_HELPER));
        asm.emit(a64::blr(16));
        for pair in saves.chunks(2).rev() {
            let second = if pair[1] == XZR { 15 } else { pair[1] };
            asm.emit(a64::ldp_post(pair[0], second, a64::SP, 16));
        }
        asm.emit(a64::msr_nzcv(25));
        asm.cbnz_to(true, 1, stub.exit);
        if let Some(dst) = stub.dst {
            if dst != 0 {
                asm.emit(a64::mov_reg(true, dst, 0));
            }
        }
        asm.b_to(stub.resume);
    }
    // Exits before an instruction.
    let exits = std::mem::take(&mut ctx.exits);
    for (label, exit_rip, k, defined) in exits {
        ctx.asm.bind(label);
        ctx.emit_defined(defined);
        ctx.asm
            .emit(a64::add_imm(true, 20, 20, (len - k) as u32, false));
        ctx.asm.mov_imm64(1, exit_rip);
        ctx.asm.emit(a64::movz(true, 0, EXIT_SLOW as u32, 0));
        ctx.asm.b_abs(exit_common);
    }
    let boundaries = std::mem::take(&mut ctx.boundaries);
    for (boundary, short, next, k, defined) in boundaries {
        // The loop owns the boundary after the RDTSC: the budget the helper left in the context.
        ctx.asm.bind(boundary);
        ctx.emit_defined(defined);
        ctx.asm.emit(a64::ldr_imm(8, 20, 21, super::CTX_SEG_START));
        ctx.asm.mov_imm64(1, next);
        ctx.asm
            .emit(a64::movz(true, 0, super::EXIT_BOUNDARY as u32, 0));
        ctx.asm.b_abs(exit_common);
        // The new quantum is shorter than the rest of the block: one instruction at a time.
        ctx.asm.bind(short);
        ctx.emit_defined(defined);
        ctx.asm
            .emit(a64::add_imm(true, 20, 20, (len - k - 1) as u32, false));
        ctx.asm.mov_imm64(1, next);
        ctx.asm.emit(a64::movz(true, 0, EXIT_BUDGET as u32, 0));
        ctx.asm.b_abs(exit_common);
    }
    let leaves = std::mem::take(&mut ctx.leaves);
    for (label, next, k, defined) in leaves {
        ctx.asm.bind(label);
        ctx.emit_defined(defined);
        ctx.asm
            .emit(a64::add_imm(true, 20, 20, (len - k - 1) as u32, false));
        ctx.asm.mov_imm64(1, next);
        ctx.asm
            .emit(a64::movz(true, 0, super::EXIT_LOOKUP as u32, 0));
        ctx.asm.b_abs(exit_common);
    }
    let partials = std::mem::take(&mut ctx.partials);
    for (label, exit_rip, k, defined) in partials {
        ctx.asm.bind(label);
        ctx.emit_defined(defined);
        ctx.asm
            .emit(a64::add_imm(true, 20, 20, (len - k) as u32, false));
        ctx.asm.mov_imm64(1, exit_rip);
        ctx.asm
            .emit(a64::movz(true, 0, super::EXIT_REP_PARTIAL as u32, 0));
        ctx.asm.b_abs(exit_common);
    }
    let words = ctx.asm.words.len();
    let Some(base) = ctx.engine.arena.reserve(words) else {
        return Compiled::NoSpace;
    };
    if ctx.asm.finish(base).is_err() {
        return Compiled::Slow;
    }
    let code = std::mem::take(&mut ctx.asm.words);
    engine.arena.commit(base, &code);
    let ranges = insns
        .iter()
        .map(|(insn_rip, insn, _)| ((insn_rip & 0xfff) as u16, insn.length))
        .collect();
    Compiled::Block(base, len, ranges)
}
