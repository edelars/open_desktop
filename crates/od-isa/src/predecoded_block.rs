//! PERF-31 (ADR-0018, variant (a), prototype): predecoded straight-line blocks for the fast path.
//!
//! A block is the sequence of decoded instructions from one RIP, all on the 4 KiB page of that RIP,
//! that the fast path (`try_fast_step`, groups G1-G6) may run: it ends after the first instruction
//! that changes the flow (Jcc, JMP, CALL, RET), before the first one outside the fast path (a port,
//! a system instruction, `RDTSC`, a LOCK/REP or FS/GS form ...), before an instruction that would
//! need a byte of another page, at the end of the page or at [`MAX_BLOCK_INSTRUCTIONS`]. Building a
//! block ([`predecode_block`]) only decodes; it does not run anything and needs a side-effect-free
//! view of the code bytes.
//!
//! The run loop of a batched CPL0 set call (`run_with_cpl0_profile_set_and_pio_batched`) with
//! `Cpl0ProfileSet::with_block_cache_prototype` asks its memory for a block at every instruction it
//! does not already run from one ([`GuestMemory::cached_block`]) and runs the instructions of the
//! block through the handler of the fast path stored with each decode (E10, ADR-0018, «Дополнение
//! E10»: [`BlockInstruction`], chosen by `execute::fast_handler` when the block is built, run by the
//! executor `try_fast_step` runs outside blocks too), asking [`GuestMemory::block_holds`] before
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
//!
//! PERF-33 (E5, step 4): the block check (`Cpl0ProfileSet::with_block_cache_verify`, the runner's
//! `--block-cache-verify`). The loop looks the blocks up and walks them as above
//! ([`block_check_expect`]), but runs every instruction through the general step, whose decode
//! reads the bytes (the backend answers no decoded instruction in that mode), and then compares that
//! fresh decode with the instruction the block holds for the RIP, and (E10) the handler the block
//! holds with the one `execute::fast_handler` chooses for the fresh decode ([`block_check_after`]). The
//! comparison reads nothing and changes no cache: the backend hands over the decode it already made
//! ([`GuestMemory::take_decoded_for_block_check`]). The first difference ends the run with
//! `RunExit::MemoryError(Backend(report))` and is kept for [`take_block_check_divergence`]; the
//! counters are [`block_check_counters`] (thread-local, like those of `fast_path_check`). One
//! difference to the run from blocks: the check cannot see a refusal of the fast path (the general
//! step decides it inside), so the walk of a block goes on after such an instruction instead of
//! looking up a new block at the next RIP; both compare the decode of the same bytes.

use crate::execute::{
    FastHandler, StepOutcome, fast_admitted, fast_handler, run_block_instruction,
};
use crate::long_decode::{self, Instruction};
use crate::memory::{AccessContext, GuestMemory, MemoryError};
use std::cell::RefCell;
use std::sync::Arc;

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

/// The role of `instruction` in a block, or `None` when the block ends before it. E10 (ADR-0018,
/// «Дополнение E10»): derived from the static rule of the fast path ([`fast_handler`], its only
/// place): an instruction with a handler is in a block, a control transfer (Jcc, JMP, CALL, RET in
/// the form of `executable`) ends it. The dynamic admission (CPL0, TF/RF, the shadow) still decides
/// every execution.
#[inline(always)]
pub fn fast_block_role(instruction: &Instruction) -> Option<FastBlockRole> {
    fast_handler(instruction).map(|handler| {
        if handler.changes_flow() {
            FastBlockRole::Terminator
        } else {
            FastBlockRole::Straight
        }
    })
}

/// E10 (ADR-0018, «Дополнение E10»): one instruction of a block: its decode and the handler of the
/// fast path chosen for it when the block was built ([`fast_handler`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockInstruction {
    handler: FastHandler,
    instruction: Instruction,
}

impl BlockInstruction {
    /// The entry of `instruction`: panics when the fast path has no handler for it (a block holds
    /// only instructions [`fast_block_role`] admits; building one from another is a host bug).
    pub fn new(instruction: Instruction) -> Self {
        let handler = fast_handler(&instruction).unwrap_or_else(|| {
            panic!("a block instruction without a fast handler: {instruction:?}")
        });
        Self {
            handler,
            instruction,
        }
    }

    /// The stored decode.
    #[inline(always)]
    pub fn instruction(&self) -> &Instruction {
        &self.instruction
    }

    /// The stored handler.
    #[inline(always)]
    pub fn handler(&self) -> FastHandler {
        self.handler
    }
}

/// PERF-31 (ADR-0018, variant (a)): a predecoded straight-line block, the instructions in
/// execution order from its first RIP, each with its handler (E10). Shared, so that the run loop
/// holds the block it executes while the backend's cache keeps it too. `Arc`, not `Rc` (PERF-33,
/// E2): the board that owns the cache must be `Send` (`assert_send` in `od-machine`'s
/// `bus/shared_tests.rs` and `stage10_display_probe.rs`), and an `Rc` in its table is not. Built
/// from decodes ([`From<Vec<Instruction>>`], [`FromIterator<Instruction>`]), each of which must
/// have a handler ([`BlockInstruction::new`]).
#[derive(Clone, Debug)]
pub struct DecodedBlock(Arc<[BlockInstruction]>);

impl std::ops::Deref for DecodedBlock {
    type Target = [BlockInstruction];

    #[inline(always)]
    fn deref(&self) -> &[BlockInstruction] {
        &self.0
    }
}

impl FromIterator<Instruction> for DecodedBlock {
    fn from_iter<I: IntoIterator<Item = Instruction>>(instructions: I) -> Self {
        Self(
            instructions
                .into_iter()
                .map(BlockInstruction::new)
                .collect(),
        )
    }
}

impl From<Vec<Instruction>> for DecodedBlock {
    fn from(instructions: Vec<Instruction>) -> Self {
        instructions.into_iter().collect()
    }
}

#[cfg(test)]
impl DecodedBlock {
    /// Test-only: the block with the handler of entry `index` replaced (a block whose handler
    /// was not chosen from its own decode, for the check of `--block-cache-verify`).
    pub(crate) fn with_handler(&self, index: usize, handler: FastHandler) -> Self {
        let mut entries = self.0.to_vec();
        entries[index].handler = handler;
        Self(entries.into())
    }
}

/// PERF-33 (E4, step 2): why [`predecode_block_with_end`] ended a block (also an empty one: then
/// the first instruction is the one it ended before). A diagnostic of `--block-stats` only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockEnd {
    /// After a Jcc, JMP, CALL or RET (the last instruction of the block).
    Branch,
    /// Before `instruction` at `rip`, which [`fast_block_role`] does not admit (an operation
    /// outside the fast path, or a Jcc/JMP/CALL/RET outside the form of `executable`: a prefix
    /// other than the default ones).
    OutsideFastPath { rip: u64, instruction: Instruction },
    /// At the end of the page of the first RIP.
    PageEnd,
    /// Before an instruction that needs a byte of the next page.
    PageCrossing,
    /// At [`MAX_BLOCK_INSTRUCTIONS`] (`max_len`).
    LengthLimit,
    /// Before bytes that do not decode.
    DecodeError,
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
    predecode_block_with_end(code, rip, context, max_len).0
}

/// PERF-33 (E4, step 2): [`predecode_block`] and why the block ended there.
pub fn predecode_block_with_end<M: GuestMemory>(
    code: &mut M,
    rip: u64,
    context: AccessContext,
    max_len: usize,
) -> (Vec<Instruction>, BlockEnd) {
    let page = rip >> 12;
    let mut fence = PageFence {
        code,
        page,
        outside: false,
    };
    let mut block = Vec::new();
    let mut at = rip;
    loop {
        if block.len() >= max_len {
            return (block, BlockEnd::LengthLimit);
        }
        if at >> 12 != page {
            return (block, BlockEnd::PageEnd);
        }
        fence.outside = false;
        let decoded = long_decode::decode(&mut fence, at, context);
        if fence.outside {
            return (block, BlockEnd::PageCrossing);
        }
        let Ok(instruction) = decoded else {
            return (block, BlockEnd::DecodeError);
        };
        let Some(role) = fast_block_role(&instruction) else {
            return (
                block,
                BlockEnd::OutsideFastPath {
                    rip: at,
                    instruction,
                },
            );
        };
        block.push(instruction);
        at = at.wrapping_add(u64::from(instruction.length));
        if role == FastBlockRole::Terminator {
            return (block, BlockEnd::Branch);
        }
    }
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
/// refuses it, and the loop then takes the general step. Otherwise the outcome of the stored
/// handler of the fast path (E10, [`run_block_instruction`]), which is the outcome of the general
/// step (ADR-0017, section 3). `admit_once` (Д2, chosen by the screening F100: ADR-0018,
/// «Дополнение E10», decision 3; the loop asks [`GuestMemory::block_admission_once`] once per
/// call): the dynamic admission only at the first instruction of a pass; between two
/// instructions of one pass only the earlier one runs, and no handler writes TF, RF or the shadow.
/// Otherwise (Д1) at every instruction.
#[inline(always)]
pub(crate) fn block_step<M: GuestMemory>(
    cursor: &mut Option<BlockCursor>,
    state: &mut crate::LongModeState,
    memory: &mut M,
    context: AccessContext,
    rflags_defined: &mut u64,
    admit_once: bool,
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
    let admitted = if admit_once && continues {
        debug_assert!(fast_admitted(state, context));
        true
    } else {
        fast_admitted(state, context)
    };
    let outcome = if admitted {
        run_block_instruction(
            state,
            memory,
            context,
            rflags_defined,
            &current.block[index],
        )
    } else {
        None
    };
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
        // (the instruction did not retire): the run of the block ends (PERF-33, E4: the memory is
        // told why, for its `--block-stats` counters only).
        _ => {
            memory.block_run_cut(current.block[index].instruction(), outcome.is_none());
            *cursor = None;
        }
    }
    outcome
}

/// E9 (ADR-0019, section 2.1): after an instruction of the current block retired from it with
/// `Advanced` ([`block_step`]), its index in the block when the next instruction of the loop is
/// the next one of the same block (the pass goes on), else `None` (the block ends here).
#[inline(always)]
pub(crate) fn block_pass_goes_on(cursor: &Option<BlockCursor>) -> Option<usize> {
    cursor
        .as_ref()
        .filter(|current| current.next < current.block.len())
        .map(|current| current.next - 1)
}

/// PERF-33 (E5, step 4): what the block check counted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BlockCheckCounters {
    /// Instructions a block held whose fresh decode was compared with it (and agreed, except the
    /// one of a divergence).
    pub compared: u64,
    /// Instructions a block held that the step ended without decoding (a check before the decode,
    /// a fetch error or fault): nothing to compare, and the walk of the block ends.
    pub not_decoded: u64,
    /// Fresh decodes that differ from the block (0 or 1: the first ends the run).
    pub divergences: u64,
}

/// PERF-33 (E5, step 4): the first difference between a block and the fresh decode of the general
/// step.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockCheckDivergence {
    /// The RIP of the instruction.
    pub rip: u64,
    /// Its index in the block and the length of the block.
    pub index: usize,
    pub block_len: usize,
    /// The instruction the block holds for the RIP.
    pub block: Instruction,
    /// The fresh decode of the general step (`None`: the step retired the instruction without a
    /// decode the check could see).
    pub decoded: Option<Instruction>,
    /// E10: the handler the block holds for the RIP.
    pub block_handler: FastHandler,
    /// E10: the handler [`fast_handler`] chooses for the fresh decode (`None`: no decode, or no
    /// handler for it).
    pub fresh_handler: Option<FastHandler>,
}

impl std::fmt::Display for BlockCheckDivergence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "block check divergence at rip=0x{:x} (instruction {} of a block of {}): block {:?}, fresh decode {:?}, block handler {:?}, fresh handler {:?}",
            self.rip,
            self.index,
            self.block_len,
            self.block,
            self.decoded,
            self.block_handler,
            self.fresh_handler
        )
    }
}

thread_local! {
    static BLOCK_CHECK: RefCell<(BlockCheckCounters, Option<BlockCheckDivergence>)> =
        const { RefCell::new((BlockCheckCounters { compared: 0, not_decoded: 0, divergences: 0 }, None)) };
}

/// PERF-33 (E5, step 4): zero the counters of the block check of this thread and forget its
/// divergence.
pub fn reset_block_check() {
    BLOCK_CHECK.with(|check| *check.borrow_mut() = Default::default());
}

/// PERF-33 (E5, step 4): the counters of the block check of this thread.
pub fn block_check_counters() -> BlockCheckCounters {
    BLOCK_CHECK.with(|check| check.borrow().0)
}

/// PERF-33 (E5, step 4): the divergence that ended the last checked run of this thread, if any.
pub fn take_block_check_divergence() -> Option<BlockCheckDivergence> {
    BLOCK_CHECK.with(|check| check.borrow_mut().1.take())
}

/// PERF-33 (E5, step 4): before the step of the checked loop, the instruction (and its index) the
/// block walk has for `state.rip`: the next one of the current block when it is at `state.rip`
/// and the block still holds, else the first one of the block the memory has for `state.rip`
/// (exactly the choice of [`block_step`]); `None` (the cursor dropped) when there is none. Nothing
/// runs.
#[inline(always)]
pub(crate) fn block_check_expect<M: GuestMemory>(
    cursor: &mut Option<BlockCursor>,
    state: &crate::LongModeState,
    memory: &mut M,
    context: AccessContext,
) -> Option<(usize, BlockInstruction)> {
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
    let current = cursor.as_ref()?;
    Some((current.next, current.block[current.next]))
}

/// PERF-33 (E5, step 4): after the general step of the instruction at `rip` (`outcome`), the
/// comparison of its fresh decode with `expected` (from [`block_check_expect`]) and the walk of
/// the block as [`block_step`] makes it; the fresh decode is taken from the memory either way. The
/// report of a divergence (also kept for [`take_block_check_divergence`]), else `None`.
#[inline(always)]
pub(crate) fn block_check_after<M: GuestMemory>(
    cursor: &mut Option<BlockCursor>,
    expected: Option<(usize, BlockInstruction)>,
    rip: u64,
    outcome: &Result<StepOutcome, MemoryError>,
    state: &crate::LongModeState,
    memory: &mut M,
) -> Option<String> {
    let fresh = memory.take_decoded_for_block_check();
    let (index, block) = expected?;
    let decoded = fresh
        .filter(|(at, _)| *at == rip)
        .map(|(_, instruction)| instruction);
    let retired = matches!(outcome, Ok(StepOutcome::Advanced | StepOutcome::Branched));
    if decoded.is_none() && !retired {
        *cursor = None;
        BLOCK_CHECK.with(|check| check.borrow_mut().0.not_decoded += 1);
        return None;
    }
    // E10: the handler the block holds must be the one the static rule chooses for the fresh
    // decode (a guard on the place of the build: with equal decodes it can differ only when the
    // block's handler was not chosen from its own decode).
    let fresh_handler = decoded.as_ref().and_then(fast_handler);
    if decoded != Some(*block.instruction()) || fresh_handler != Some(block.handler()) {
        let block_len = cursor.as_ref().map_or(0, |current| current.block.len());
        *cursor = None;
        let divergence = BlockCheckDivergence {
            rip,
            index,
            block_len,
            block: *block.instruction(),
            decoded,
            block_handler: block.handler(),
            fresh_handler,
        };
        let report = divergence.to_string();
        BLOCK_CHECK.with(|check| {
            let mut check = check.borrow_mut();
            check.0.compared += 1;
            check.0.divergences += 1;
            check.1 = Some(divergence);
        });
        return Some(report);
    }
    BLOCK_CHECK.with(|check| check.borrow_mut().0.compared += 1);
    match outcome {
        Ok(StepOutcome::Advanced) => {
            if let Some(current) = cursor.as_mut() {
                current.next = index + 1;
                current.rip = state.rip;
            }
            memory.block_retired(index);
        }
        Ok(StepOutcome::Branched) => {
            *cursor = None;
            memory.block_retired(index);
        }
        _ => *cursor = None,
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::long_decode::Operation;
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

    fn end(base: u64, bytes: &[u8], rip: u64) -> BlockEnd {
        let mut code = Code {
            base,
            bytes: bytes.to_vec(),
        };
        let (block, end) =
            predecode_block_with_end(&mut code, rip, ring0(), MAX_BLOCK_INSTRUCTIONS);
        assert_eq!(
            block,
            predecode_block(&mut code, rip, ring0(), MAX_BLOCK_INSTRUCTIONS),
            "the same block"
        );
        end
    }

    /// PERF-33 (E4, step 2): the build says why the block ended, and the block is the one of
    /// `predecode_block`.
    #[test]
    fn the_end_of_a_block_says_why_it_ended() {
        // MOV EAX; JNZ -2.
        let mut code = MOV_EAX.to_vec();
        code.extend([0x75, 0xFE]);
        assert_eq!(end(0x1000, &code, 0x1000), BlockEnd::Branch);
        // MOV EAX; OUT DX, AL: before the port, at its RIP.
        let mut code = MOV_EAX.to_vec();
        code.push(0xEE);
        assert!(matches!(
            end(0x1000, &code, 0x1000),
            BlockEnd::OutsideFastPath { rip: 0x1005, instruction } if instruction.length == 1
        ));
        // FS JMP +0: a branch the filter does not admit (empty block).
        let fs_jmp = end(0x1000, &[0x64, 0xEB, 0x00], 0x1000);
        assert!(
            matches!(
                fs_jmp,
                BlockEnd::OutsideFastPath { rip: 0x1000, instruction }
                    if matches!(instruction.operation, Operation::Jump { .. })
            ),
            "{fs_jmp:?}"
        );
        let movs: Vec<u8> = std::iter::repeat_n(MOV_EAX, 100).flatten().collect();
        // Three MOV EAX from 0xFF1 end exactly at the page end; from 0xFF4 the third crosses it.
        assert_eq!(end(0xFF1, &movs, 0xFF1), BlockEnd::PageEnd);
        assert_eq!(end(0xFF4, &movs, 0xFF4), BlockEnd::PageCrossing);
        assert_eq!(end(0x1000, &movs, 0x1000), BlockEnd::LengthLimit);
        // MOV EAX and then bytes that are not there.
        assert_eq!(end(0x1000, &MOV_EAX, 0x1000), BlockEnd::DecodeError);
    }
    /// E10 (ADR-0018, «Дополнение E10», decision 4): the block check compares the handler the
    /// block holds with the one the static rule chooses for the fresh decode: a block whose handler
    /// was not chosen from its own decode diverges although the decodes agree.
    #[test]
    fn the_block_check_compares_the_stored_handler_with_the_fresh_one() {
        struct Fresh(Option<(u64, Instruction)>);
        impl GuestMemory for Fresh {
            fn fetch(&mut self, _: u64, _: AccessContext) -> Result<u8, MemoryError> {
                unreachable!()
            }
            fn read(&mut self, _: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
                unreachable!()
            }
            fn write(&mut self, _: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
                unreachable!()
            }
            fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
                unreachable!()
            }
            fn take_decoded_for_block_check(&mut self) -> Option<(u64, Instruction)> {
                self.0.take()
            }
        }
        // MOV EAX, imm32; ADD EAX, EBX; JNZ -2.
        let mut bytes = MOV_EAX.to_vec();
        bytes.extend([0x01, 0xD8, 0x75, 0xFE]);
        let mut code = Code {
            base: 0x1000,
            bytes,
        };
        let block = DecodedBlock::from(predecode_block(
            &mut code,
            0x1000,
            ring0(),
            MAX_BLOCK_INSTRUCTIONS,
        ));
        assert_eq!(block.len(), 3);
        let state = crate::fast_step_g2_tests::state(1, 2);
        let check = |block: &DecodedBlock| {
            reset_block_check();
            let mut cursor = Some(BlockCursor {
                block: block.clone(),
                next: 0,
                rip: 0x1000,
            });
            let mut memory = Fresh(Some((0x1000, *block[0].instruction())));
            let report = block_check_after(
                &mut cursor,
                Some((0, block[0])),
                0x1000,
                &Ok(StepOutcome::Advanced),
                &state,
                &mut memory,
            );
            (
                report,
                take_block_check_divergence(),
                block_check_counters(),
            )
        };
        let (report, divergence, counters) = check(&block);
        assert_eq!((report, divergence), (None, None));
        assert_eq!((counters.compared, counters.divergences), (1, 0));
        // The first entry keeps its decode (MOV EAX, imm32) with the handler of ADD EAX, EBX.
        let mutated = block.with_handler(0, block[1].handler());
        let (report, divergence, counters) = check(&mutated);
        let divergence = divergence.expect("a divergence");
        assert!(report.is_some_and(|report| report.contains("block handler")));
        assert_eq!(divergence.block, *block[0].instruction());
        assert_eq!(divergence.decoded, Some(*block[0].instruction()));
        assert_eq!(divergence.block_handler, block[1].handler());
        assert_eq!(divergence.fresh_handler, Some(block[0].handler()));
        assert_eq!((counters.compared, counters.divergences), (1, 1));
    }
}
