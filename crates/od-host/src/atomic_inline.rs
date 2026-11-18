//! S6-41c: the host side of an inline atomic block (JIT_CONTRACT, S6-41c).
//!
//! [`AtomicInlineBlock`] owns the published code of one `od_jit::atomic_block::AtomicBlock` and
//! runs it for one guest instruction. The block calls [`locate_helper`] (the "translate and check
//! with the intent to write" helper) before the atomic operation; the helper walks the guest page
//! tables of the CPL0 profile set through `od_machine::locate_paged_atomic_operand` (the walks of
//! the interpreter's locked operation, so rights, accessed and dirty bits, and the faults are
//! the interpreter's; the unequal branch of `CMPXCHG` takes the same write walk), and returns the
//! host pointer of the operand and the page-generation counter. The block then performs the host
//! atomic operation, raises the counter with a release read-modify-write and sets the report flag;
//! after the block returned, this module reports `committed_write` for the operand.
//!
//! S6-41d: a block holds up to `MAX_ATOMIC_BLOCK_INSTRUCTIONS` instructions; the helper serves one
//! site per memory instruction (the block stores the index before each call), the report of the
//! call lists every operand written (`committed_writes`, in program order, after the block
//! returned), and a write into the physical range `run_guarded` guards (the code page of the
//! block) ends the call in [`AtomicInlineExit::CodeInvalidated`] with the instruction retired.
//!
//! What this module does not do: it does not retry or fall back (a declined or faulting
//! instruction is returned to the caller, who owns the interpreter step; the instructions that
//! retired before it stay retired), and it holds no state between calls: the pointer the helper
//! hands out lives in the call record of one invocation. Admission into
//! `PagedFetchedLoadSession` (the key, the mode, `CodeInvalidated` of the session) is the
//! session's (`PagedFetchedLoadSession::execute_atomic`).
//!
//! `unsafe` here is the call of published code and the `extern "C"` helper dereferencing its
//! call record (the same category as `memory_site_helper_v2`); none of it touches RAM bytes: the
//! RAM pointer comes from `od-machine` (`ram_storage.rs`, amendment (в′) of S6-30a) and is
//! accessed only by the generated host-atomic instructions.

use std::ffi::c_void;
use std::sync::atomic::AtomicU8;

use od_isa::LongModeState;
use od_isa::memory::{AccessContext, ArchitecturalFault, MemoryError, PageFaultRecord};
use od_jit::atomic_block::AtomicBlock;
use od_jit::memory_bridge::{CheckedMemoryOutcomeV1, classify_error};
use od_jit::runtime::{
    AtomicLocateHelperV4, AtomicSiteV4, BlockEntryV4, RUNTIME_EXIT_V4_ATOMIC_DECLINED,
    RUNTIME_EXIT_V4_ATOMIC_FAULT, RUNTIME_EXIT_V4_CODE_INVALIDATED, RUNTIME_EXIT_V4_HELPER_FAILURE,
    RuntimeContextV1, RuntimeContextV4, RuntimeExitTag,
};
use od_machine::{
    GuestPhysicalRange, Long64ProbeConfig, PagedAtomicOperand, PhysicalMemory, ProbeConfigError,
    locate_paged_atomic_operand,
};

use crate::{ExecutableMemoryError, PublishedCode};

/// What the block returned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AtomicInlineExit {
    /// Every instruction of the block retired (`retired == instruction count`).
    Complete,
    /// The budget ran out before an instruction: that instruction did not run.
    BudgetExhausted,
    /// The STOP byte was raised before an instruction: that instruction did not run (the ones
    /// before it, if any, retired).
    StopRequested,
    /// The helper reported a guest fault: nothing of the faulting instruction was changed (the
    /// walk's accessed and dirty bits are committed, as for the interpreter); the instructions
    /// before it retired (`retired` counts them) and RIP is the faulting instruction's.
    Fault,
    /// The helper declined the operand (misaligned or split, not plain RAM, ...): nothing was
    /// changed by that instruction; the interpreter owns it.
    Declined,
    /// S6-41d: an instruction wrote into the range `run_guarded` guards (the code page): it and
    /// every instruction before it retired, the later ones did not run, and the block must not
    /// run again (the exit of the store into its own code, S4-16i).
    CodeInvalidated,
    /// The helper contract was violated. The instructions before the failing one completed (and
    /// retired); the failing one did not change the state.
    HelperFailure,
}

#[derive(Debug)]
pub struct AtomicInlineReport {
    pub exit: AtomicInlineExit,
    pub retired: u64,
    pub remaining_budget: u64,
    /// Architectural exception identified by the Rust helper before the operand's effects.
    pub fault: Option<ArchitecturalFault>,
    /// The page fault of a `Fault` exit (with `CR2` and the error code), if it was a `#PF`.
    pub page_fault: Option<PageFaultRecord>,
    /// The fault or decline details written by the helper.
    pub outcome: CheckedMemoryOutcomeV1,
    /// Why the operand was declined, when the helper kept a reason.
    pub declined: Option<MemoryError>,
    /// The last range reported through `committed_write` after the block returned (the operand,
    /// after the data and the generation raise), `None` when the block wrote nothing.
    pub committed: Option<GuestPhysicalRange>,
    /// Every range reported that way, in program order (one per memory instruction that ran).
    pub committed_writes: Vec<GuestPhysicalRange>,
    /// How many times the block called the helper (0 for fences only).
    pub helper_calls: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AtomicInlineError {
    Paging(ProbeConfigError),
    /// The block returned a value outside the V4 exit namespace.
    UnknownExit(u32),
    /// The block completed a memory instruction without setting (or with an invalid) committed
    /// report. A host error, not a guest outcome: it is raised after the effects of the block
    /// (data, generation) are already done, so nothing is rolled back.
    MissingCommittedReport,
}

/// Where the guest page tables are and who is running: what the helper needs to walk.
#[derive(Clone, Copy, Debug)]
pub struct AtomicInlineEnv {
    pub cr3: u64,
    pub efer: u64,
    pub access_context: AccessContext,
}

/// The adapter of one invocation, reached through `RuntimeContextV4::host`.
struct HelperHost<'a> {
    memory: &'a PhysicalMemory,
    paging: Long64ProbeConfig,
    access_context: AccessContext,
    committed_write: &'a mut dyn FnMut(GuestPhysicalRange),
    fault: Option<ArchitecturalFault>,
    page_fault: Option<PageFaultRecord>,
    declined: Option<MemoryError>,
    helper_calls: u32,
    /// The physical range the caller guards (S6-41d; the session names the 4 KiB code page of the
    /// block, the granularity of S4-16i): a write of the walk or of the operand into it ends the
    /// call in `CodeInvalidated`.
    code_range: Option<GuestPhysicalRange>,
    /// The operand ranges of every successful locate, in program order. A located operand is
    /// always written by the block (nothing can fail between the helper and the data word).
    located: Vec<GuestPhysicalRange>,
}

fn ranges_overlap(a: GuestPhysicalRange, b: GuestPhysicalRange) -> bool {
    u128::from(a.start().get()) < b.end_exclusive()
        && u128::from(b.start().get()) < a.end_exclusive()
}

/// The "translate and check with the intent to write" helper of V4 blocks.
///
/// # Safety
/// `context` must point to a live, exclusively borrowed `RuntimeContextV4` whose `state` points
/// to a live `LongModeState`, whose `host` points to the `HelperHost` of this invocation and
/// whose `site` is the base of an array of `site_count` immutable sites of the block (a
/// `Box<[_]>`; the index is checked against `site_count`), all valid for the whole call. The
/// helper does not re-enter generated code and does not unwind.
pub unsafe extern "C" fn locate_helper(context: *mut RuntimeContextV4) -> u32 {
    if context.is_null() {
        return RUNTIME_EXIT_V4_HELPER_FAILURE;
    }
    // SAFETY: the caller contract above.
    let context = unsafe { &mut *context };
    context.outcome = CheckedMemoryOutcomeV1::default();
    context.generation_count = 0;
    context.generation = [std::ptr::null(); 2];
    context.operand_host = std::ptr::null_mut();
    if context.host.is_null()
        || context.site.is_null()
        || context.base.state.is_null()
        || context.site_index >= context.site_count
    {
        return RUNTIME_EXIT_V4_HELPER_FAILURE;
    }
    // SAFETY: the caller contract above; the three pointers are checked non-null.
    let host = unsafe { &mut *context.host.cast::<HelperHost<'_>>() };
    // SAFETY: the caller contract above: `site` is the base of `site_count` immutable sites, and
    // the index was checked against the count.
    let site = unsafe { *context.site.add(context.site_index as usize) };
    // SAFETY: as above.
    let state = unsafe { &*context.base.state };
    host.helper_calls += 1;
    if state.rip != site.guest_rip {
        return RUNTIME_EXIT_V4_HELPER_FAILURE;
    }
    let linear = match site.operand.access_address_48(state, site.next_rip) {
        Ok(linear) => linear,
        Err(error) => {
            classify_error(error.clone(), &mut context.outcome);
            if let MemoryError::Architectural(fault) = error {
                host.fault = Some(fault);
                return RUNTIME_EXIT_V4_ATOMIC_FAULT;
            }
            host.declined = Some(error);
            return RUNTIME_EXIT_V4_ATOMIC_DECLINED;
        }
    };
    // A write of the walk (accessed and dirty bits) into the guarded range counts as a write
    // into it, as it does for the store of S4-16i (the committed writes of the whole call).
    let guarded = host.code_range;
    let mut walk_hit = false;
    let committed_write = &mut *host.committed_write;
    let mut note = |range: GuestPhysicalRange| {
        walk_hit |= guarded.is_some_and(|code| ranges_overlap(code, range));
        committed_write(range);
    };
    let (operand, page_fault) = locate_paged_atomic_operand(
        host.memory,
        host.paging,
        state.rip,
        host.access_context,
        linear,
        site.operand.size,
        &mut note,
    );
    match operand {
        PagedAtomicOperand::Located(window) => {
            context.operand_host = window.host_ptr();
            context.generation = [window.generation_ptr(), std::ptr::null()];
            context.generation_count = 1;
            context.committed_start = window.range().start().get();
            context.committed_len = window.range().len();
            host.located.push(window.range());
            if walk_hit
                || host
                    .code_range
                    .is_some_and(|code| ranges_overlap(code, window.range()))
            {
                context.code_dirty = 1;
            }
            0
        }
        PagedAtomicOperand::Misaligned => {
            host.declined = Some(MemoryError::Backend(format!(
                "atomic operand at {linear:#x} is not naturally aligned: the interpreter owns \
                 the split lock"
            )));
            RUNTIME_EXIT_V4_ATOMIC_DECLINED
        }
        PagedAtomicOperand::Fault(fault) => {
            classify_error(
                MemoryError::Architectural(fault.clone()),
                &mut context.outcome,
            );
            host.fault = Some(fault);
            host.page_fault = page_fault;
            RUNTIME_EXIT_V4_ATOMIC_FAULT
        }
        PagedAtomicOperand::Declined(error) => {
            host.declined = Some(error);
            RUNTIME_EXIT_V4_ATOMIC_DECLINED
        }
    }
}

/// The published code of one atomic block.
pub struct AtomicInlineBlock {
    code: PublishedCode,
    block: AtomicBlock,
    /// The sites the helper reads (one per memory instruction); boxed so their address is stable
    /// for the context pointers.
    sites: Box<[AtomicSiteV4]>,
}

impl AtomicInlineBlock {
    /// Publishes the block into the process `MAP_JIT` region (one live owner at a time, as
    /// everywhere in this crate).
    pub fn publish(block: AtomicBlock) -> Result<Self, ExecutableMemoryError> {
        Self::publish_in(block, None)
    }

    /// [`Self::publish`] into a slot of `part` (S6-42), when the caller has one, instead of a
    /// region of its own.
    pub fn publish_in(
        block: AtomicBlock,
        part: Option<&crate::jit_region::ActiveJitPart>,
    ) -> Result<Self, ExecutableMemoryError> {
        let bytes: Vec<u8> = block
            .words
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .collect();
        let code = PublishedCode::publish_with_metadata_in(&bytes, None, None, part)?;
        let sites = block.sites.clone().into_boxed_slice();
        Ok(Self { code, block, sites })
    }

    pub fn block(&self) -> &AtomicBlock {
        &self.block
    }

    /// Calls the block with a caller-built record. For tests that supply their own helper.
    ///
    /// # Safety
    /// `context` must satisfy the contract of a V4 block: `base.state` points to a live state,
    /// `base.stop_requested` is null or a live atomic byte, `locate_helper` is a function that
    /// honours [`AtomicLocateHelperV4`] (it may be `None` only for a block without a memory
    /// operand), and `host` and `site` are valid for what that helper reads.
    pub unsafe fn call_raw(&self, context: &mut RuntimeContextV4) -> u32 {
        // SAFETY: `PublishedCode::as_ptr` is the entry of the words this value published, which
        // are a V4 block (`compile_atomic_block`) with the signature of `BlockEntryV4`.
        let entry: BlockEntryV4 = unsafe { std::mem::transmute(self.code.as_ptr()) };
        // SAFETY: the caller contract above.
        unsafe { entry(context) }
    }

    /// The call record of an invocation, before the helper filled it.
    pub fn new_context(
        &self,
        state: &mut LongModeState,
        budget: u64,
        stop: Option<&AtomicU8>,
    ) -> RuntimeContextV4 {
        RuntimeContextV4 {
            base: RuntimeContextV1 {
                state,
                initial_budget: budget,
                stop_rip_enabled: 0,
                reserved: 0,
                stop_rip: 0,
                stop_requested: stop.map_or(std::ptr::null(), |byte| {
                    byte as *const AtomicU8 as *const u8
                }),
                remaining_budget: 0,
                retired_instructions: 0,
            },
            locate_helper: Some(locate_helper as AtomicLocateHelperV4),
            host: std::ptr::null_mut(),
            site: if self.sites.is_empty() {
                std::ptr::null()
            } else {
                self.sites.as_ptr()
            },
            operand_host: std::ptr::null_mut(),
            generation: [std::ptr::null(); 2],
            generation_count: 0,
            site_index: 0,
            committed_start: 0,
            committed_len: 0,
            committed_valid: 0,
            site_count: self.sites.len() as u32,
            code_dirty: 0,
            reserved2: 0,
            outcome: CheckedMemoryOutcomeV1::default(),
        }
    }

    /// Runs the instructions of the block on `state` over `memory`. `budget` is the instruction
    /// budget of the call (0 runs nothing; every instruction costs 1), `stop` the cooperative
    /// STOP byte. Page-table writes of the walks go to `committed_write` as they happen; the
    /// committed writes of the operands follow, in program order, after the data and the
    /// generation raises (all done by the block).
    pub fn run(
        &self,
        state: &mut LongModeState,
        memory: &PhysicalMemory,
        env: AtomicInlineEnv,
        budget: u64,
        stop: Option<&AtomicU8>,
        committed_write: &mut dyn FnMut(GuestPhysicalRange),
    ) -> Result<AtomicInlineReport, AtomicInlineError> {
        self.run_guarded(state, memory, env, budget, stop, None, committed_write)
    }

    /// [`Self::run`] with a guarded physical range named (S6-41d; the code page of the block): a
    /// write of the walk or an operand that overlaps it is committed, the instruction retires,
    /// and the call ends in
    /// [`AtomicInlineExit::CodeInvalidated`] without running the later instructions.
    #[allow(clippy::too_many_arguments)]
    pub fn run_guarded(
        &self,
        state: &mut LongModeState,
        memory: &PhysicalMemory,
        env: AtomicInlineEnv,
        budget: u64,
        stop: Option<&AtomicU8>,
        code_range: Option<GuestPhysicalRange>,
        committed_write: &mut dyn FnMut(GuestPhysicalRange),
    ) -> Result<AtomicInlineReport, AtomicInlineError> {
        let paging = Long64ProbeConfig::from_cpu_state(state, env.cr3, env.efer)
            .map_err(AtomicInlineError::Paging)?;
        let mut context = self.new_context(state, budget, stop);
        let (status, fault, page_fault, declined, helper_calls, located) = {
            let mut host = HelperHost {
                memory,
                paging,
                access_context: env.access_context,
                committed_write: &mut *committed_write,
                fault: None,
                page_fault: None,
                declined: None,
                helper_calls: 0,
                code_range,
                located: Vec::new(),
            };
            context.host = (&mut host as *mut HelperHost<'_>).cast::<c_void>();
            // SAFETY: `context` is the record of a block built by `compile_atomic_block`: the
            // state pointer is the caller's live `&mut LongModeState` (not used by the caller
            // during the call), the stop byte is a live atomic byte or null, `locate_helper` is
            // this module's helper, `host` points to the live `host` above and `site` to the
            // boxed sites that outlive the call.
            let status = unsafe { self.call_raw(&mut context) };
            context.host = std::ptr::null_mut();
            (
                status,
                host.fault.take(),
                host.page_fault.take(),
                host.declined.take(),
                host.helper_calls,
                std::mem::take(&mut host.located),
            )
        };
        let exit = match status {
            s if s == RuntimeExitTag::BlockComplete as u32 => AtomicInlineExit::Complete,
            s if s == RuntimeExitTag::BudgetExhausted as u32 => AtomicInlineExit::BudgetExhausted,
            s if s == RuntimeExitTag::StopRequested as u32 => AtomicInlineExit::StopRequested,
            RUNTIME_EXIT_V4_ATOMIC_FAULT => AtomicInlineExit::Fault,
            RUNTIME_EXIT_V4_ATOMIC_DECLINED => AtomicInlineExit::Declined,
            RUNTIME_EXIT_V4_HELPER_FAILURE => AtomicInlineExit::HelperFailure,
            RUNTIME_EXIT_V4_CODE_INVALIDATED => AtomicInlineExit::CodeInvalidated,
            other => return Err(AtomicInlineError::UnknownExit(other)),
        };
        // Every successful locate was followed by the data word and the generation raise of its
        // instruction (nothing can fail in between), so every located operand is reported, also
        // when a later instruction faulted, was declined or was stopped. The report comes after
        // the data and the generation raise (S6-30b order).
        if !located.is_empty() && context.committed_valid != 1 {
            return Err(AtomicInlineError::MissingCommittedReport);
        }
        for range in &located {
            committed_write(*range);
        }
        Ok(AtomicInlineReport {
            exit,
            retired: context.base.retired_instructions,
            remaining_budget: context.base.remaining_budget,
            fault,
            page_fault,
            outcome: context.outcome,
            declined,
            committed: located.last().copied(),
            committed_writes: located,
            helper_calls,
        })
    }
}
