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
//! What this module does not do: it is not admitted into `PagedFetchedLoadSession` (S6-41d), it
//! runs one instruction per call, it does not retry or fall back (a declined or faulting
//! instruction is returned to the caller, who owns the interpreter step), and it holds no state
//! between calls: the pointer the helper hands out lives in the call record of one invocation.
//!
//! `unsafe` here is the call of published code and the `extern "C"` helper dereferencing its
//! call record (the same category as `memory_site_helper_v2`); none of it touches RAM bytes: the
//! RAM pointer comes from `od-machine` (`ram_storage.rs`, amendment (в′) of S6-30a) and is
//! accessed only by the generated host-atomic instructions.

use std::ffi::c_void;
use std::sync::atomic::AtomicU8;

use od_isa::LongModeState;
use od_isa::memory::{AccessContext, MemoryError, PageFaultRecord};
use od_jit::atomic_block::AtomicBlock;
use od_jit::memory_bridge::{
    CheckedMemoryOutcomeV1, MEMORY_HELPER_ARCHITECTURAL_FAULT, classify_error,
};
use od_jit::runtime::{
    AtomicLocateHelperV4, AtomicSiteV4, BlockEntryV4, RUNTIME_EXIT_V4_ATOMIC_DECLINED,
    RUNTIME_EXIT_V4_ATOMIC_FAULT, RUNTIME_EXIT_V4_HELPER_FAILURE, RuntimeContextV1,
    RuntimeContextV4, RuntimeExitTag,
};
use od_machine::{
    GuestPhysicalAddress, GuestPhysicalRange, Long64ProbeConfig, PagedAtomicOperand,
    PhysicalMemory, ProbeConfigError, locate_paged_atomic_operand,
};

use crate::{ExecutableMemoryError, PublishedCode};

/// What the block returned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AtomicInlineExit {
    /// The instruction retired (`retired == 1`, RIP is the next instruction).
    Complete,
    /// The budget was zero: nothing ran.
    BudgetExhausted,
    /// The STOP byte was raised before the instruction: nothing ran.
    StopRequested,
    /// The helper reported a guest fault: nothing was changed (the walk's accessed and dirty
    /// bits are committed, as for the interpreter), `retired == 0`.
    Fault,
    /// The helper declined the operand (misaligned or split, not plain RAM, ...): nothing was
    /// changed by the instruction, `retired == 0`; the interpreter owns the instruction.
    Declined,
    /// The helper contract was violated: nothing was changed.
    HelperFailure,
}

#[derive(Debug)]
pub struct AtomicInlineReport {
    pub exit: AtomicInlineExit,
    pub retired: u64,
    pub remaining_budget: u64,
    /// The page fault of a `Fault` exit (with `CR2` and the error code), if it was a `#PF`.
    pub page_fault: Option<PageFaultRecord>,
    /// The fault or decline details written by the helper.
    pub outcome: CheckedMemoryOutcomeV1,
    /// Why the operand was declined, when the helper kept a reason.
    pub declined: Option<MemoryError>,
    /// The range reported through `committed_write` after the block returned (the operand, after
    /// the data and the generation raise), `None` when the block wrote nothing.
    pub committed: Option<GuestPhysicalRange>,
    /// How many times the block called the helper (0 for a fence).
    pub helper_calls: u32,
}

#[derive(Debug)]
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
    page_fault: Option<PageFaultRecord>,
    declined: Option<MemoryError>,
    helper_calls: u32,
}

/// The "translate and check with the intent to write" helper of V4 blocks.
///
/// # Safety
/// `context` must point to a live, exclusively borrowed `RuntimeContextV4` whose `state` points
/// to a live `LongModeState`, whose `host` points to the `HelperHost` of this invocation and
/// whose `site` points to the immutable site of the block, all valid for the whole call. The
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
    if context.host.is_null() || context.site.is_null() || context.base.state.is_null() {
        return RUNTIME_EXIT_V4_HELPER_FAILURE;
    }
    // SAFETY: the caller contract above; the three pointers are checked non-null.
    let host = unsafe { &mut *context.host.cast::<HelperHost<'_>>() };
    // SAFETY: as above.
    let site = unsafe { *context.site };
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
            if context.outcome.status == MEMORY_HELPER_ARCHITECTURAL_FAULT {
                return RUNTIME_EXIT_V4_ATOMIC_FAULT;
            }
            host.declined = Some(error);
            return RUNTIME_EXIT_V4_ATOMIC_DECLINED;
        }
    };
    let (operand, page_fault) = locate_paged_atomic_operand(
        host.memory,
        host.paging,
        state.rip,
        host.access_context,
        linear,
        site.operand.size,
        &mut *host.committed_write,
    );
    match operand {
        PagedAtomicOperand::Located(window) => {
            context.operand_host = window.host_ptr();
            context.generation = [window.generation_ptr(), std::ptr::null()];
            context.generation_count = 1;
            context.committed_start = window.range().start().get();
            context.committed_len = window.range().len();
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
            classify_error(MemoryError::Architectural(fault), &mut context.outcome);
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
    /// The site the helper reads; boxed so its address is stable for the context pointers.
    site: Option<Box<AtomicSiteV4>>,
}

impl AtomicInlineBlock {
    /// Publishes the block into the process `MAP_JIT` region (one live owner at a time, as
    /// everywhere in this crate).
    pub fn publish(block: AtomicBlock) -> Result<Self, ExecutableMemoryError> {
        let bytes: Vec<u8> = block
            .words
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .collect();
        let code = PublishedCode::publish(&bytes)?;
        let site = block.site.map(Box::new);
        Ok(Self { code, block, site })
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
            site: self
                .site
                .as_deref()
                .map_or(std::ptr::null(), |site| site as *const AtomicSiteV4),
            operand_host: std::ptr::null_mut(),
            generation: [std::ptr::null(); 2],
            generation_count: 0,
            reserved: 0,
            committed_start: 0,
            committed_len: 0,
            committed_valid: 0,
            reserved2: 0,
            outcome: CheckedMemoryOutcomeV1::default(),
        }
    }

    /// Runs the one instruction of the block on `state` over `memory`. `budget` is the
    /// instruction budget of the call (0 runs nothing, 1 runs the instruction), `stop` the
    /// cooperative STOP byte. Page-table writes of the walk and the committed write of the
    /// operand go to `committed_write`, the latter after the data and the generation raise.
    pub fn run(
        &self,
        state: &mut LongModeState,
        memory: &PhysicalMemory,
        env: AtomicInlineEnv,
        budget: u64,
        stop: Option<&AtomicU8>,
        committed_write: &mut dyn FnMut(GuestPhysicalRange),
    ) -> Result<AtomicInlineReport, AtomicInlineError> {
        let paging = Long64ProbeConfig::from_cpu_state(state, env.cr3, env.efer)
            .map_err(AtomicInlineError::Paging)?;
        let mut context = self.new_context(state, budget, stop);
        let (status, page_fault, declined, helper_calls) = {
            let mut host = HelperHost {
                memory,
                paging,
                access_context: env.access_context,
                committed_write: &mut *committed_write,
                page_fault: None,
                declined: None,
                helper_calls: 0,
            };
            context.host = (&mut host as *mut HelperHost<'_>).cast::<c_void>();
            // SAFETY: `context` is the record of a block built by `compile_atomic_block`: the
            // state pointer is the caller's live `&mut LongModeState` (not used by the caller
            // during the call), the stop byte is a live atomic byte or null, `locate_helper` is
            // this module's helper, `host` points to the live `host` above and `site` to the
            // boxed site that outlives the call.
            let status = unsafe { self.call_raw(&mut context) };
            context.host = std::ptr::null_mut();
            (
                status,
                host.page_fault.take(),
                host.declined.take(),
                host.helper_calls,
            )
        };
        let exit = match status {
            s if s == RuntimeExitTag::BlockComplete as u32 => AtomicInlineExit::Complete,
            s if s == RuntimeExitTag::BudgetExhausted as u32 => AtomicInlineExit::BudgetExhausted,
            s if s == RuntimeExitTag::StopRequested as u32 => AtomicInlineExit::StopRequested,
            RUNTIME_EXIT_V4_ATOMIC_FAULT => AtomicInlineExit::Fault,
            RUNTIME_EXIT_V4_ATOMIC_DECLINED => AtomicInlineExit::Declined,
            RUNTIME_EXIT_V4_HELPER_FAILURE => AtomicInlineExit::HelperFailure,
            other => return Err(AtomicInlineError::UnknownExit(other)),
        };
        let mut committed = None;
        if exit == AtomicInlineExit::Complete && self.site.is_some() {
            if context.committed_valid != 1 {
                return Err(AtomicInlineError::MissingCommittedReport);
            }
            let range = GuestPhysicalRange::new(
                GuestPhysicalAddress::new(context.committed_start),
                context.committed_len,
            )
            .map_err(|_| AtomicInlineError::MissingCommittedReport)?;
            // After the data and the generation raise (both done by the block): the report.
            committed_write(range);
            committed = Some(range);
        }
        Ok(AtomicInlineReport {
            exit,
            retired: context.base.retired_instructions,
            remaining_budget: context.base.remaining_budget,
            page_fault,
            outcome: context.outcome,
            declined,
            committed,
            helper_calls,
        })
    }
}
