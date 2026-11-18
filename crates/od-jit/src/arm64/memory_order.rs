//! S6-41: lowering of the three memory-effect classes of the IR (ordinary
//! access, atomic read-modify-write, fence) to ARM64, per memory-ordering mode.
//!
//! The mode is a compile parameter, never a global (`MemoryOrderMode`):
//!
//! | IR effect | `Interleaved` (one host thread) | `Parallel` (one thread per vCPU) |
//! |---|---|---|
//! | `CheckedLoad` | `LDR{B,H,W,X}` | `LDAR{B,H,W,X}` |
//! | `CheckedStore` | `STR{B,H,W,X}` | `STLR{B,H,W,X}` |
//! | `AtomicRmw::Add` | `LDADD{B,H,}` | `LDADDAL{B,H,}` |
//! | `AtomicRmw::Exchange` | `SWP{B,H,}` | `SWPAL{B,H,}` |
//! | `AtomicRmw::CompareExchange` | `CAS{B,H,}` | `CASAL{B,H,}` |
//! | `Fence::Full` (`MFENCE`) | no instruction | `DMB ISH` |
//! | `Fence::Store` (`SFENCE`) | no instruction | `DMB ISH` |
//! | `Fence::Load` (`LFENCE`) | no instruction | `DMB ISHLD` |
//!
//! Sources: MEMORY_MODEL.md "Выбор по замеру (S6-06)" and "Норма fence
//! (S6-32)" (the fence mapping is the norm's own: `MFENCE`/`SFENCE`/`LFENCE` as
//! `DMB ISH`/`DMB ISH`/`DMB ISHLD`; the `SeqCst` order of every parallel-mode
//! access is `LDAR`/`STLR` on AArch64; `LDADDAL`/`SWPAL`/`CASAL` are the
//! `SeqCst` forms of `fetch_add`/`swap`/`compare_exchange` that S6-06 measured
//! and S6-31 uses). In the interleaved mode every vCPU shares one host thread
//! and the instruction boundary already orders accesses, so no ordering bits
//! and no barrier are emitted; a fence there emits nothing, but the IR effect is
//! kept (nothing here removes or merges effects).
//!
//! Preconditions of every lowered access, owned by the caller (the inline
//! memory emitter, which does not exist yet; see JIT_CONTRACT S6-41):
//! `host.address` holds a host pointer that is naturally aligned for the width
//! (`LDAR`/`STLR`/LSE fault otherwise; split `LOCK` never reaches this code and
//! stays on the helper and gateway path of S6-34), points into guest RAM backed
//! by Normal Inner Shareable memory, and the access has already been
//! permission-checked. Requires FEAT_LSE (Apple M1 and later; the S6-06 host).

use crate::arm64::{
    AccessWidth, AddressBase, AtomicOrder, BarrierOption, EncodeError, Encoder, XReg, XRegOrZero,
};
use crate::ir::{AtomicRmwKind, FenceKind, IntWidth, MemoryClass, Operation};
use od_isa::OperandSize;

/// Memory-ordering mode of one compilation (S6-15 / S6-34).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryOrderMode {
    /// All vCPUs on one host thread: no ordering bits, no barriers.
    Interleaved,
    /// One host thread per vCPU: sequentially consistent accesses and fences.
    Parallel,
}

/// Host registers bound to the temporaries of one IR effect. Register
/// allocation is the caller's; the lowering only checks the overlaps that
/// would make the emitted sequence wrong.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostOperands {
    /// Host address of the operand, already translated and aligned.
    pub address: AddressBase,
    /// Result register (`dst` of a load or an atomic effect).
    pub dst: Option<XReg>,
    /// Stored value, addend, replacement or new value.
    pub value: Option<XRegOrZero>,
    /// `CompareExchange` expected value; left intact.
    pub expected: Option<XReg>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MemoryOrderError {
    Encode(EncodeError),
    /// The effect is not in a memory class this module lowers.
    NotAMemoryEffect,
    /// The effect needs a host operand the caller did not bind.
    MissingOperand(&'static str),
    /// `CompareExchange` copies `expected` into `dst` before the instruction;
    /// `dst` must therefore not alias the address or `value` register.
    CompareExchangeOverlap,
}

impl From<EncodeError> for MemoryOrderError {
    fn from(error: EncodeError) -> Self {
        Self::Encode(error)
    }
}

const fn width_of(size: OperandSize) -> AccessWidth {
    match size {
        OperandSize::Byte => AccessWidth::Byte,
        OperandSize::Word => AccessWidth::Half,
        OperandSize::Dword => AccessWidth::Word,
        OperandSize::Qword => AccessWidth::Dword,
    }
}

const fn width_of_int(width: IntWidth) -> AccessWidth {
    match width {
        IntWidth::W8 => AccessWidth::Byte,
        IntWidth::W16 => AccessWidth::Half,
        IntWidth::W32 => AccessWidth::Word,
        IntWidth::W64 => AccessWidth::Dword,
    }
}

/// Lower one `CheckedLoad`, `CheckedStore`, `AtomicRmw` or `Fence` effect.
/// Effects of any other class are refused, not skipped. The width of an
/// ordinary access is the width of its memory operand.
pub fn lower_memory_effect(
    encoder: &mut Encoder,
    mode: MemoryOrderMode,
    operation: &Operation,
    host: &HostOperands,
) -> Result<(), MemoryOrderError> {
    match operation.memory_class() {
        Some(MemoryClass::Ordinary) => match operation {
            Operation::CheckedLoad { operand, .. } => {
                let dst = host.dst.ok_or(MemoryOrderError::MissingOperand("dst"))?;
                lower_ordinary_load(encoder, mode, width_of(operand.size), dst, host.address)
            }
            Operation::CheckedStore { operand, .. } => {
                let value = host
                    .value
                    .ok_or(MemoryOrderError::MissingOperand("value"))?;
                lower_ordinary_store(encoder, mode, width_of(operand.size), value, host.address)
            }
            _ => Err(MemoryOrderError::NotAMemoryEffect),
        },
        Some(MemoryClass::Atomic) => match operation {
            Operation::AtomicRmw { kind, width, .. } => {
                lower_atomic_rmw(encoder, mode, *kind, width_of_int(*width), host)
            }
            _ => Err(MemoryOrderError::NotAMemoryEffect),
        },
        Some(MemoryClass::Fence) => match operation {
            Operation::Fence { kind } => {
                lower_fence(encoder, mode, *kind);
                Ok(())
            }
            _ => Err(MemoryOrderError::NotAMemoryEffect),
        },
        None => Err(MemoryOrderError::NotAMemoryEffect),
    }
}

pub fn lower_ordinary_load(
    encoder: &mut Encoder,
    mode: MemoryOrderMode,
    width: AccessWidth,
    dst: XReg,
    address: AddressBase,
) -> Result<(), MemoryOrderError> {
    let dst = XRegOrZero::Reg(dst);
    match mode {
        MemoryOrderMode::Interleaved => encoder.ldr_sized(width, dst, address, 0)?,
        MemoryOrderMode::Parallel => encoder.ldar(width, dst, address),
    }
    Ok(())
}

pub fn lower_ordinary_store(
    encoder: &mut Encoder,
    mode: MemoryOrderMode,
    width: AccessWidth,
    value: XRegOrZero,
    address: AddressBase,
) -> Result<(), MemoryOrderError> {
    match mode {
        MemoryOrderMode::Interleaved => encoder.str_sized(width, value, address, 0)?,
        MemoryOrderMode::Parallel => encoder.stlr(width, value, address),
    }
    Ok(())
}

pub fn lower_atomic_rmw(
    encoder: &mut Encoder,
    mode: MemoryOrderMode,
    kind: AtomicRmwKind,
    width: AccessWidth,
    host: &HostOperands,
) -> Result<(), MemoryOrderError> {
    let order = match mode {
        MemoryOrderMode::Interleaved => AtomicOrder::Relaxed,
        MemoryOrderMode::Parallel => AtomicOrder::AcquireRelease,
    };
    let dst = host.dst.ok_or(MemoryOrderError::MissingOperand("dst"))?;
    let value = host
        .value
        .ok_or(MemoryOrderError::MissingOperand("value"))?;
    match kind {
        AtomicRmwKind::Add => encoder.ldadd(width, order, value, dst, host.address),
        AtomicRmwKind::Exchange => encoder.swp(width, order, value, dst, host.address),
        AtomicRmwKind::CompareExchange => {
            let expected = host
                .expected
                .ok_or(MemoryOrderError::MissingOperand("expected"))?;
            // When `expected == dst` the overlap check below is skipped on purpose:
            // no `MOV` is emitted, `dst` is only the CAS comparand/result register, and
            // CAS permits its register operands to coincide architecturally. The check
            // only guards the `MOV dst, expected` that would clobber the address or the
            // value register otherwise.
            if expected != dst {
                let aliases_address = matches!(host.address, AddressBase::Reg(base) if base == dst);
                let aliases_value = value == XRegOrZero::Reg(dst);
                if aliases_address || aliases_value {
                    return Err(MemoryOrderError::CompareExchangeOverlap);
                }
                encoder.mov_x(dst, XRegOrZero::Reg(expected));
            }
            // CAS compares `[address]` with its first register and writes the
            // old memory value back into it: `dst` is both the comparand and
            // the result, so `expected` itself stays intact.
            encoder.cas(width, order, dst, value, host.address);
        }
    }
    Ok(())
}

/// A fence is lowered in `Parallel` mode to its barrier and in `Interleaved`
/// mode to nothing; the IR effect exists in both and is never dropped earlier.
pub fn lower_fence(encoder: &mut Encoder, mode: MemoryOrderMode, kind: FenceKind) {
    if mode == MemoryOrderMode::Interleaved {
        return;
    }
    encoder.dmb(match kind {
        FenceKind::Full | FenceKind::Store => BarrierOption::Ish,
        FenceKind::Load => BarrierOption::IshLd,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::TempId;
    use od_isa::long_decode::{AddressBase as GuestBase, MemoryOperand};

    fn x(number: u8) -> XReg {
        XReg::new(number).unwrap()
    }

    fn operand(size: OperandSize) -> MemoryOperand {
        MemoryOperand {
            base: GuestBase::None,
            index: None,
            scale: 1,
            displacement: 0x1000,
            address_size: OperandSize::Qword,
            size,
        }
    }

    fn host() -> HostOperands {
        HostOperands {
            address: AddressBase::Reg(x(0)),
            dst: Some(x(3)),
            value: Some(XRegOrZero::Reg(x(1))),
            expected: Some(x(2)),
        }
    }

    fn lowered(mode: MemoryOrderMode, operation: &Operation, host: &HostOperands) -> Vec<u32> {
        let mut encoder = Encoder::new();
        lower_memory_effect(&mut encoder, mode, operation, host).unwrap();
        encoder.finish().unwrap().into_words()
    }

    fn rmw(kind: AtomicRmwKind, width: IntWidth, size: OperandSize) -> Operation {
        Operation::AtomicRmw {
            kind,
            width,
            operand: operand(size),
            value: TempId(1),
            expected: (kind == AtomicRmwKind::CompareExchange).then_some(TempId(2)),
            dst: TempId(3),
        }
    }

    #[test]
    fn memory_classes_are_distinct() {
        let load = Operation::CheckedLoad {
            dst: TempId(0),
            operand: operand(OperandSize::Qword),
        };
        let atomic = rmw(AtomicRmwKind::Add, IntWidth::W32, OperandSize::Dword);
        let fence = Operation::Fence {
            kind: FenceKind::Full,
        };
        assert_eq!(load.memory_class(), Some(MemoryClass::Ordinary));
        assert_eq!(atomic.memory_class(), Some(MemoryClass::Atomic));
        assert_eq!(fence.memory_class(), Some(MemoryClass::Fence));
        assert_eq!(
            Operation::BeginInstruction { index: 0 }.memory_class(),
            None
        );
    }

    #[test]
    fn ordinary_accesses_by_mode_and_width() {
        for (size, ldr, ldar, str_, stlr) in [
            (
                OperandSize::Byte,
                0x3940_0000u32,
                0x08df_fc00u32,
                0x3900_0000u32,
                0x089f_fc00u32,
            ),
            (
                OperandSize::Word,
                0x7940_0000,
                0x48df_fc00,
                0x7900_0000,
                0x489f_fc00,
            ),
            (
                OperandSize::Dword,
                0xb940_0000,
                0x88df_fc00,
                0xb900_0000,
                0x889f_fc00,
            ),
            (
                OperandSize::Qword,
                0xf940_0000,
                0xc8df_fc00,
                0xf900_0000,
                0xc89f_fc00,
            ),
        ] {
            // base x0, dst x3 / value x1.
            let load = Operation::CheckedLoad {
                dst: TempId(3),
                operand: operand(size),
            };
            let store = Operation::CheckedStore {
                src: crate::ir::GuestGpr(0),
                operand: operand(size),
            };
            assert_eq!(
                lowered(MemoryOrderMode::Interleaved, &load, &host()),
                [ldr | 3]
            );
            assert_eq!(
                lowered(MemoryOrderMode::Parallel, &load, &host()),
                [ldar | 3]
            );
            assert_eq!(
                lowered(MemoryOrderMode::Interleaved, &store, &host()),
                [str_ | 1]
            );
            assert_eq!(
                lowered(MemoryOrderMode::Parallel, &store, &host()),
                [stlr | 1]
            );
        }
    }

    #[test]
    fn atomic_forms_by_mode_and_width() {
        // base x0, value (Rs) x1, expected x2, dst (Rt) x3.
        for (width, size, hi) in [
            (IntWidth::W8, OperandSize::Byte, 0x0000_0000u32),
            (IntWidth::W16, OperandSize::Word, 0x4000_0000),
            (IntWidth::W32, OperandSize::Dword, 0x8000_0000),
            (IntWidth::W64, OperandSize::Qword, 0xc000_0000),
        ] {
            let add = rmw(AtomicRmwKind::Add, width, size);
            assert_eq!(
                lowered(MemoryOrderMode::Interleaved, &add, &host()),
                [hi | 0x3820_0000 | (1 << 16) | 3]
            );
            assert_eq!(
                lowered(MemoryOrderMode::Parallel, &add, &host()),
                [hi | 0x38e0_0000 | (1 << 16) | 3]
            );
            let swap = rmw(AtomicRmwKind::Exchange, width, size);
            assert_eq!(
                lowered(MemoryOrderMode::Interleaved, &swap, &host()),
                [hi | 0x3820_8000 | (1 << 16) | 3]
            );
            assert_eq!(
                lowered(MemoryOrderMode::Parallel, &swap, &host()),
                [hi | 0x38e0_8000 | (1 << 16) | 3]
            );
            // CMPXCHG: `mov x3, x2` (comparand in the result register), then
            // CAS Rs = x3, Rt = x1 (new value), base x0.
            let cmpxchg = rmw(AtomicRmwKind::CompareExchange, width, size);
            let mov = 0xaa00_03e0 | (2 << 16) | 3;
            assert_eq!(
                lowered(MemoryOrderMode::Interleaved, &cmpxchg, &host()),
                [mov, hi | 0x08a0_7c00 | (3 << 16) | 1]
            );
            assert_eq!(
                lowered(MemoryOrderMode::Parallel, &cmpxchg, &host()),
                [mov, hi | 0x08e0_fc00 | (3 << 16) | 1]
            );
        }
    }

    #[test]
    fn compare_exchange_skips_the_copy_when_expected_is_the_result_register() {
        let cmpxchg = rmw(
            AtomicRmwKind::CompareExchange,
            IntWidth::W32,
            OperandSize::Dword,
        );
        let mut regs = host();
        regs.expected = regs.dst;
        assert_eq!(
            lowered(MemoryOrderMode::Parallel, &cmpxchg, &regs),
            [0x88e0_fc00 | (3 << 16) | 1]
        );
    }

    #[test]
    fn compare_exchange_refuses_a_result_register_that_aliases_an_input() {
        let cmpxchg = rmw(
            AtomicRmwKind::CompareExchange,
            IntWidth::W64,
            OperandSize::Qword,
        );
        let mut regs = host();
        regs.dst = Some(x(1)); // aliases `value`
        let mut encoder = Encoder::new();
        assert_eq!(
            lower_memory_effect(&mut encoder, MemoryOrderMode::Parallel, &cmpxchg, &regs),
            Err(MemoryOrderError::CompareExchangeOverlap)
        );
        regs.dst = Some(x(0)); // aliases the address
        assert_eq!(
            lower_memory_effect(&mut encoder, MemoryOrderMode::Parallel, &cmpxchg, &regs),
            Err(MemoryOrderError::CompareExchangeOverlap)
        );
        assert_eq!(encoder.position(), 0);
    }

    #[test]
    fn fences_follow_the_s6_32_norm_and_vanish_only_in_interleaved_mode() {
        for (kind, word) in [
            (FenceKind::Full, 0xd503_3bbfu32), // MFENCE  -> DMB ISH
            (FenceKind::Store, 0xd503_3bbf),   // SFENCE  -> DMB ISH
            (FenceKind::Load, 0xd503_39bf),    // LFENCE  -> DMB ISHLD
        ] {
            let fence = Operation::Fence { kind };
            assert_eq!(lowered(MemoryOrderMode::Parallel, &fence, &host()), [word]);
            assert!(lowered(MemoryOrderMode::Interleaved, &fence, &host()).is_empty());
        }
    }

    #[test]
    fn interleaved_mode_never_emits_an_ordering_bit_or_a_barrier() {
        let effects = [
            Operation::CheckedLoad {
                dst: TempId(3),
                operand: operand(OperandSize::Qword),
            },
            Operation::CheckedStore {
                src: crate::ir::GuestGpr(0),
                operand: operand(OperandSize::Dword),
            },
            rmw(AtomicRmwKind::Add, IntWidth::W64, OperandSize::Qword),
            rmw(AtomicRmwKind::Exchange, IntWidth::W32, OperandSize::Dword),
            rmw(
                AtomicRmwKind::CompareExchange,
                IntWidth::W32,
                OperandSize::Dword,
            ),
            Operation::Fence {
                kind: FenceKind::Full,
            },
        ];
        for effect in &effects {
            for word in lowered(MemoryOrderMode::Interleaved, effect, &host()) {
                let is_dmb = word & 0xffff_f0ff == 0xd503_30bf;
                let is_acquire_release_load_store =
                    word & 0x3fdf_fc00 == 0x089f_fc00 || word & 0x3fff_fc00 == 0x08df_fc00;
                let is_lse = word & 0x3f20_0c00 == 0x3820_0000;
                let lse_ordered = is_lse && (word >> 22) & 0b11 != 0;
                let is_cas = word & 0x3fa0_7c00 == 0x08a0_7c00;
                let cas_ordered = is_cas && ((word >> 22) & 1 != 0 || (word >> 15) & 1 != 0);
                assert!(
                    !is_dmb && !is_acquire_release_load_store && !lse_ordered && !cas_ordered,
                    "{effect:?} lowered to ordered word {word:#010x}"
                );
            }
        }
    }

    #[test]
    fn missing_operands_and_non_memory_effects_are_refused() {
        let mut encoder = Encoder::new();
        let mut regs = host();
        regs.dst = None;
        let add = rmw(AtomicRmwKind::Add, IntWidth::W32, OperandSize::Dword);
        assert_eq!(
            lower_memory_effect(&mut encoder, MemoryOrderMode::Parallel, &add, &regs),
            Err(MemoryOrderError::MissingOperand("dst"))
        );
        let mut regs = host();
        regs.expected = None;
        let cmpxchg = rmw(
            AtomicRmwKind::CompareExchange,
            IntWidth::W32,
            OperandSize::Dword,
        );
        assert_eq!(
            lower_memory_effect(&mut encoder, MemoryOrderMode::Parallel, &cmpxchg, &regs),
            Err(MemoryOrderError::MissingOperand("expected"))
        );
        assert_eq!(
            lower_memory_effect(
                &mut encoder,
                MemoryOrderMode::Parallel,
                &Operation::BeginInstruction { index: 0 },
                &host()
            ),
            Err(MemoryOrderError::NotAMemoryEffect)
        );
        assert_eq!(encoder.position(), 0);
    }
}
