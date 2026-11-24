//! PERF-31 (ADR-0018, variant (a), prototype): predecoded straight-line blocks for the fast path.
//!
//! A block is the sequence of decoded instructions from one RIP, all on the 4 KiB page of that RIP,
//! that the fast path (`try_fast_step`, groups G1-G5) may run: it ends after the first instruction
//! that changes the flow (Jcc, JMP, CALL, RET), before the first one outside the fast path (a port,
//! a system instruction, `RDTSC`, a LOCK/REP or FS/GS form ...), before an instruction that would
//! need a byte of another page, at the end of the page or at [`MAX_BLOCK_INSTRUCTIONS`]. Building a
//! block ([`predecode_block`]) only decodes; it does not run anything and needs a side-effect-free
//! view of the code bytes.
//!
//! The run loop of a batched CPL0 set call (`run_with_cpl0_profile_set_and_pio_batched`) with
//! `Cpl0ProfileSet::with_block_cache_prototype` asks its memory for a block at every instruction it
//! does not already run from one ([`GuestMemory::cached_block`]) and runs the instructions of the
//! block through `try_fast_step` with the stored decode, asking [`GuestMemory::block_holds`] before
//! every further one. Everything else of the loop is unchanged: the STOP, budget and stop-address
//! checks before every instruction, the retire accounting and the batch boundary (admission inputs
//! and the caller's check: device access, STOP, IRQ) after every retired one. A refusal of the fast
//! path (its admission reads only, before any effect), a branch (the last instruction of a block), the
//! end of the block or a block that no longer holds ends the run of the block; the instruction then
//! goes the general way (the decode and the whole step), exactly as without blocks. A fault or an
//! error of a block instruction is the outcome of the step itself, returned as the general path
//! returns it.
//!
//! What the block replaces for every instruction is only the source of the decoded instruction (the
//! decoded-instruction cache or the decoder) and the checks of the step before the decode. Those
//! checks read only CR0, CR3, CR4, the activity and the STI shadow (and the fast path itself needs
//! a clear shadow): they passed at the entry of the call, and the batch boundary ends the call as
//! soon as any of them changes.

use crate::execute::{StepOutcome, try_fast_step};
use crate::long_decode::{self, DecodedBlock, Instruction, Operand, Operation};
use crate::memory::{AccessContext, GuestMemory, MemoryError};

/// The longest block [`predecode_block`] builds.
pub const MAX_BLOCK_INSTRUCTIONS: usize = 64;

/// What an instruction is in a block (see [`fast_block_role`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FastBlockRole {
    /// The block goes on after it.
    Straight,
    /// It changes the flow: the block ends after it.
    Terminator,
}

/// The role of `instruction` in a block, or `None` when the block ends before it: the operation
/// classes of the fast path (`try_fast_step`, G1-G5) without a LOCK/REP/REPNE prefix and without an
/// FS/GS segment prefix, and `Shift` only without a memory operand. Only a filter: the fast path's
/// own admission still decides every execution, and a form this admits but the fast path refuses
/// ends the run of the block there (before any effect).
pub fn fast_block_role(instruction: &Instruction) -> Option<FastBlockRole> {
    if instruction.prefixes.group1.is_some()
        || matches!(instruction.prefixes.segment, Some(0x64 | 0x65))
    {
        return None;
    }
    match instruction.operation {
        Operation::Arithmetic { .. }
        | Operation::Logic { .. }
        | Operation::Mov { .. }
        | Operation::MovImmediate { .. }
        | Operation::Lea { .. }
        | Operation::StackRegister { .. } => Some(FastBlockRole::Straight),
        Operation::Extend { src, .. } if !matches!(src, Operand::Immediate(_)) => {
            Some(FastBlockRole::Straight)
        }
        Operation::Shift {
            dst, source, count, ..
        } if !matches!(dst, Operand::Memory(_))
            && !matches!(source, Some(Operand::Memory(_)))
            && !matches!(count, Operand::Memory(_)) =>
        {
            Some(FastBlockRole::Straight)
        }
        Operation::Branch { .. }
        | Operation::Jump { .. }
        | Operation::Call { .. }
        | Operation::Return { .. } => Some(FastBlockRole::Terminator),
        _ => None,
    }
}

/// The fetches of [`predecode_block`], held on the 4 KiB page of the first RIP: a fetch of another
/// page fails and is noted, so that the instruction that needed it is not taken.
struct PageFence<'m, M> {
    code: &'m mut M,
    page: u64,
    outside: bool,
}

impl<M: GuestMemory> GuestMemory for PageFence<'_, M> {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        if address >> 12 != self.page {
            self.outside = true;
            return Err(MemoryError::Backend("predecode: outside the page".into()));
        }
        self.code.fetch(address, context)
    }

    fn read(&mut self, _: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Backend("predecode reads no data".into()))
    }

    fn write(&mut self, _: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Backend("predecode writes nothing".into()))
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Err(MemoryError::Backend("predecode has no fence".into()))
    }
}

/// The block at `rip` (see the module documentation), decoded with `long_decode::decode` from
/// `code`, whose `fetch` must have no effect (the backend reads the bytes of the page it has
/// validated, nothing else); empty when the first instruction is not one a block may hold. An
/// instruction whose decode failed or fetched a byte outside the page of `rip` ends the block before
/// it, as does the end of that page.
pub fn predecode_block<M: GuestMemory>(
    code: &mut M,
    rip: u64,
    context: AccessContext,
    max_len: usize,
) -> Vec<Instruction> {
    let page = rip >> 12;
    let mut fence = PageFence {
        code,
        page,
        outside: false,
    };
    let mut block = Vec::new();
    let mut at = rip;
    while block.len() < max_len && at >> 12 == page {
        fence.outside = false;
        let Ok(instruction) = long_decode::decode(&mut fence, at, context) else {
            break;
        };
        if fence.outside {
            break;
        }
        let Some(role) = fast_block_role(&instruction) else {
            break;
        };
        block.push(instruction);
        at = at.wrapping_add(u64::from(instruction.length));
        if role == FastBlockRole::Terminator {
            break;
        }
    }
    block
}

/// The block a run loop is executing and where it is in it.
pub(crate) struct BlockCursor {
    block: DecodedBlock,
    /// The index of the next instruction to run.
    next: usize,
    /// Its RIP.
    rip: u64,
}

/// One instruction of the run loop from a block: the next instruction of the current block when it
/// is at `state.rip` and the block still holds, else the first one of the block the memory has for
/// `state.rip`; `None` (nothing done, the cursor dropped) when there is none or the fast path
/// refuses it, and the loop then takes the general step. Otherwise the outcome of `try_fast_step`
/// on the stored decode, which is the outcome of the general step (ADR-0017, section 3).
#[inline(always)]
pub(crate) fn block_step<M: GuestMemory>(
    cursor: &mut Option<BlockCursor>,
    state: &mut crate::LongModeState,
    memory: &mut M,
    context: AccessContext,
    rflags_defined: &mut u64,
) -> Option<Result<StepOutcome, MemoryError>> {
    let continues = cursor
        .as_ref()
        .is_some_and(|current| current.rip == state.rip && current.next < current.block.len())
        && memory.block_holds();
    if !continues {
        *cursor = None;
        let block = memory.cached_block(state.rip, context)?;
        *cursor = Some(BlockCursor {
            block,
            next: 0,
            rip: state.rip,
        });
    }
    let current = cursor.as_mut()?;
    let index = current.next;
    let outcome = try_fast_step(
        state,
        memory,
        context,
        rflags_defined,
        &current.block[index],
        true,
    );
    match outcome {
        Some(Ok(StepOutcome::Advanced)) => {
            current.next = index + 1;
            current.rip = state.rip;
            memory.block_retired(index);
        }
        Some(Ok(StepOutcome::Branched)) => {
            *cursor = None;
            memory.block_retired(index);
        }
        // A refusal (nothing happened: the general step runs the instruction), a fault or an error
        // (the instruction did not retire): the run of the block ends.
        _ => *cursor = None,
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{AccessKind, AccessOrigin, InstructionMode, PrivilegeLevel};

    struct Code {
        base: u64,
        bytes: Vec<u8>,
    }

    impl GuestMemory for Code {
        fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
            address
                .checked_sub(self.base)
                .and_then(|offset| self.bytes.get(offset as usize).copied())
                .ok_or(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Fetch,
                })
        }
        fn read(&mut self, _: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
            unreachable!("predecode reads no data")
        }
        fn write(&mut self, _: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
            unreachable!("predecode writes nothing")
        }
        fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
            unreachable!()
        }
    }

    fn ring0() -> AccessContext {
        AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        }
    }

    fn lengths(base: u64, bytes: &[u8], rip: u64) -> Vec<u8> {
        let mut code = Code {
            base,
            bytes: bytes.to_vec(),
        };
        predecode_block(&mut code, rip, ring0(), MAX_BLOCK_INSTRUCTIONS)
            .iter()
            .map(|instruction| instruction.length)
            .collect()
    }

    /// `MOV EAX, imm32`.
    const MOV_EAX: [u8; 5] = [0xB8, 1, 2, 3, 4];

    #[test]
    fn a_block_ends_after_a_branch_and_before_a_form_outside_the_fast_path() {
        // MOV EAX; ADD EAX, EBX; JNZ -2; MOV EAX (not reached)
        let mut code = MOV_EAX.to_vec();
        code.extend([0x01, 0xD8, 0x75, 0xFE]);
        code.extend(MOV_EAX);
        assert_eq!(lengths(0x1000, &code, 0x1000), [5, 2, 2]);
        // MOV EAX; OUT DX, AL; MOV EAX: the port ends the block before it.
        let mut code = MOV_EAX.to_vec();
        code.push(0xEE);
        code.extend(MOV_EAX);
        assert_eq!(lengths(0x1000, &code, 0x1000), [5]);
        // A first instruction outside the fast path: no block (CLI; MOV EAX).
        let mut code = vec![0xFA];
        code.extend(MOV_EAX);
        assert!(lengths(0x1000, &code, 0x1000).is_empty());
        // LOCK ADD [RAX], EBX and REP STOSB are not block forms.
        assert!(lengths(0x1000, &[0xF0, 0x01, 0x18], 0x1000).is_empty());
        assert!(lengths(0x1000, &[0xF3, 0xAA], 0x1000).is_empty());
        // An FS-prefixed MOV is not either.
        assert!(lengths(0x1000, &[0x64, 0x8B, 0x00], 0x1000).is_empty());
    }

    #[test]
    fn a_block_never_crosses_the_page_of_its_first_rip() {
        // Three MOV EAX from 0xFF1: the third ends exactly at the page end (0xFFF), so it is in;
        // the fourth (0x1000) starts on the next page and is out.
        let mut code = Vec::new();
        for _ in 0..4 {
            code.extend(MOV_EAX);
        }
        assert_eq!(lengths(0xFF1, &code, 0xFF1), [5, 5, 5]);
        // From 0xFF4: the third MOV would need two bytes of the next page: it is out, although the
        // bytes are there and decode to a block form.
        assert_eq!(lengths(0xFF4, &code, 0xFF4), [5, 5]);
        // A bare FWAIT on the last byte of the page (the one look-ahead of the decoder) is not a
        // block form: the block ends before it.
        let mut code = MOV_EAX.to_vec();
        code.push(0x9B);
        code.extend([0xD9, 0x30]);
        assert_eq!(lengths(0xFFA, &code, 0xFFA), [5]);
    }

    #[test]
    fn a_block_has_at_most_the_maximum_length() {
        let code: Vec<u8> = std::iter::repeat_n(MOV_EAX, 100).flatten().collect();
        assert_eq!(lengths(0x1000, &code, 0x1000).len(), MAX_BLOCK_INSTRUCTIONS);
    }
}
