//! Opt-in one-instruction Long64 bridge through the uncached RAM walker.

use crate::console_output_context::capture_frame_chain;
use crate::decoded_instruction_cache::DecodedInstructionCache;
use crate::execution_observations::{
    DeliveredPageFaultObservation, ElfMmuGatherPageWalk, ElfMmuGatherRaw, ElfStackAlignmentPoint,
    ElfStackCopyProbe, ElfStackCopySnapshot, ElfStackCopyStatus, ElfStackCopyTargetCr3Capture,
    ElfStackCopyTargetCr3ChainStatus, ElfStackCopyTargetCr3Frame, FaultingPageFaultContext,
    FaultingPageWalkContext, InstructionPathControlFlow, InstructionPathRegisterValue,
    InstructionPathTraceRecord, LINUX_ELF_STACK_ALIGN_AFTER_RIP, LINUX_ELF_STACK_ALIGN_BEFORE_RIP,
    LINUX_ELF_STACK_COPY_PROBE_CAPACITY, LINUX_ELF_STACK_COPY_RIP,
    LINUX_ELF_STACK_COPY_WALK_ENTRY_CAPACITY, LINUX_RELOCATE_VMA_DOWN_TLB_FINISH_CALL_RIP,
    LINUX_RELOCATE_VMA_DOWN_TLB_FINISH_RETURN_RIP, LINUX_SETUP_ARG_PAGES_STACK_SHIFT_RIP,
    LINUX_TLB_FINISH_MMU_ACTUAL_POST_RIP, LinuxExecutionObservationCollector,
    PendingPostIretObservation, PostIretPageWalkSnapshot,
};
use crate::fetch_cache_report::{C5Split, LinuxWalkCounts, WalkCounter};
use crate::fetch_cache_sweep::FetchCacheSweep;
use crate::fetch_translation_cache::{
    FetchCacheInvalidation, FetchCacheStats, FetchCacheTag, FetchTranslationCache, TranslationKey,
};
use crate::irq_source::{IrqBoundaryHook, PolledVector};
use crate::linux_checkpoints::{BoundaryView, CheckpointRecorder};
use crate::page_walk::{
    VectorRamAccess, access_long64_2m_ram_vector, translate_long64_2m_ram_read_only_with_tables,
    translate_long64_2m_ram_read_only_with_trace,
};
use crate::{
    GuestLinearAddress, GuestPhysicalAddress, GuestPhysicalRange, Long64ProbeConfig, MachineBus,
    MemoryRegionKind, PageFaultReason, PageTableLevel, RamAccessOperation, TrackedAccessError,
    TrackedAccessOutcome, TrackedAccessRequest,
};
use od_isa::execute::{
    Cpl0ProfileSet, Cpl0ProfileSetExit, Cpl0ProfileSetInputError, Cpl0ProfileSetResult, InvlpgExit,
    InvlpgLong64V1, InvlpgResult, RunConfig, RunExit, RunInputError, RunResult,
    TranslationContextChange, run_with_cpl0_profile_set_and_pio, run_with_cpl3_profile_set,
    step_invlpg as cpu_step_invlpg, step_paged_mov as cpu_step_paged_mov,
};
use od_isa::execute::{
    PagedMovCrossPageDwordReadLong64V1, PagedMovCrossPageWordFetchLong64V1,
    PagedMovCrossPageWordReadLong64V1, step_paged_mov_dword_read as cpu_step_paged_mov_dword_read,
    step_paged_mov_word_read as cpu_step_paged_mov_word_read,
    step_paged_mov_word_read_cross_page_fetch as cpu_step_paged_mov_word_read_cross_page_fetch,
};
use od_isa::long_decode::{AddressBase, Instruction, Operand, Operation};
use od_isa::memory::{
    AccessContext, AccessKind, AccessOrigin, ArchitecturalFault, GuestMemory, InstructionMode,
    MemoryError, PageFaultRecord, PrivilegeLevel,
};
use od_isa::{
    Cpl0ExternalInterruptDelivery, Cpl0ExternalInterruptPhase, Cpl0ExternalInterruptRefusal,
    Cpl0GeneralProtectionDelivery, Cpl0GeneralProtectionPhase, Cpl0GeneralProtectionRefusal,
    Cpl0PageFaultDelivery, Cpl0PageFaultDeliveryPhase, Cpl0PageFaultRefusal,
    deliver_external_interrupt_cpl0_set, deliver_general_protection_cpl0_set, deliver_nmi_cpl0_set,
    deliver_page_fault_cpl0_set,
};
use od_isa::{IretqUserEntry, LongModeState};
use std::rc::Rc;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PagedMovStepError {
    Configuration(crate::ProbeConfigError),
    CpuInput(RunInputError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PagedMovStepReport {
    pub cpu: RunResult,
    /// Retains the walker's precise saved RIP, linear address, and PFEC.
    pub page_fault: Option<PageFaultRecord>,
    /// Retains non-architectural walker/backend classifications.
    pub memory_error: Option<TrackedAccessError>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PagedInvlpgStepReport {
    pub cpu: InvlpgResult,
    /// Retains the walker's precise saved RIP, linear address, and PFEC.
    pub page_fault: Option<PageFaultRecord>,
    /// Retains the original tracked-walker classification, including faults.
    pub access_error: Option<TrackedAccessError>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PagedInvlpgStepError {
    Configuration(crate::ProbeConfigError),
}

/// Execute exactly one `INVLPG [RBX]` step through tracked physical RAM fetches.
/// The CPU profile owns decode and retirement semantics; this adapter only
/// supplies the fixed paging snapshot and forwards committed walker writes.
pub fn step_paged_invlpg(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    rflags_defined: u64,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PagedInvlpgStepReport, PagedInvlpgStepError> {
    let paging = Long64ProbeConfig::from_cpu_state(state, cr3, efer)
        .map_err(PagedInvlpgStepError::Configuration)?;
    let instruction_rip = state.rip;
    let mut memory = PagedMovMemoryAdapter {
        bus: bus.into(),
        paging,
        instruction_rip,
        committed_write: &mut committed_write,
        page_fault: None,
        memory_error: None,
        access_error: None,
        page_fault_trace: None,
        capture_page_fault_trace: false,
        committed_ad_updates: None,
        committed_writes: None,
        capture_effects: false,
        descriptor_gate: None,
        descriptor_scope: None,
        dword_read_profile: false,
        word_read_profile: false,
        ram_only: true,
        reject_crossing_qword: false,
        read_only_qword: false,
        large_ram_profile: false,
        dword_write_profile: false,
        cpl0_set_dword_read: false,
        cpl0_set_byte_read: false,
        cpl0_set_word_read: false,
        cpl0_set_narrow_write: false,
        cpl0_set_cross_page_fetch: false,
        cpl0_set_cross_page_store: false,
        cpl0_set_cross_page_read: false,
        fetch_cache: None,
        walks: None,
        decoded: DecodedStep::default(),
    };
    let cpu = cpu_step_invlpg(
        InvlpgLong64V1,
        state,
        &mut memory,
        context,
        efer,
        rflags_defined,
    );
    // A fetch fault may follow committed page-walker A/D writes. Keep those
    // callbacks observable. Rejected inputs are refused before the first fetch.
    let report = PagedInvlpgStepReport {
        cpu,
        page_fault: memory.page_fault,
        access_error: memory.access_error,
    };
    debug_assert!(matches!(report.cpu.exit, InvlpgExit::Fault(_)) || report.page_fault.is_none());
    Ok(report)
}

/// Fetch-only adapter used by the exact control-register steps below (S4-16f):
/// tracked RAM instruction fetch, committed A/D callbacks, no operand access.
fn fetch_only_adapter<'a>(
    bus: &'a mut MachineBus,
    paging: Long64ProbeConfig,
    instruction_rip: u64,
    committed_write: &'a mut dyn FnMut(GuestPhysicalRange),
) -> PagedMovMemoryAdapter<'a> {
    PagedMovMemoryAdapter {
        bus: bus.into(),
        paging,
        instruction_rip,
        committed_write,
        page_fault: None,
        memory_error: None,
        access_error: None,
        page_fault_trace: None,
        capture_page_fault_trace: false,
        committed_ad_updates: None,
        committed_writes: None,
        capture_effects: false,
        descriptor_gate: None,
        descriptor_scope: None,
        dword_read_profile: false,
        word_read_profile: false,
        ram_only: true,
        reject_crossing_qword: false,
        read_only_qword: false,
        large_ram_profile: false,
        dword_write_profile: false,
        cpl0_set_dword_read: false,
        cpl0_set_byte_read: false,
        cpl0_set_word_read: false,
        cpl0_set_narrow_write: false,
        cpl0_set_cross_page_fetch: false,
        cpl0_set_cross_page_store: false,
        cpl0_set_cross_page_read: false,
        fetch_cache: None,
        walks: None,
        decoded: DecodedStep::default(),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PagedMovCr3StepReport {
    pub cpu: od_isa::MovCr3Result,
    /// Retains the walker's precise saved RIP, linear address, and PFEC.
    pub page_fault: Option<PageFaultRecord>,
    /// Retains the original tracked-walker classification, including faults.
    pub access_error: Option<TrackedAccessError>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PagedMovCr3StepError {
    Configuration(crate::ProbeConfigError),
    CpuInput(od_isa::MovCr3InputError),
}

/// Execute exactly one bare `MOV CR3,RAX` (CR0.WP = 0 or 1) through tracked
/// physical RAM fetches (S4-16f). The CPU profile owns decode, validation and
/// the retired `Cr3Write` effect; the adapter supplies the pre-write paging
/// snapshot and forwards committed walker A/D writes. A new snapshot for the
/// written CR3 is the caller's decision.
pub fn step_paged_mov_cr3_wp(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    rflags_defined: u64,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PagedMovCr3StepReport, PagedMovCr3StepError> {
    let paging = Long64ProbeConfig::from_cpu_state(state, cr3, efer)
        .map_err(PagedMovCr3StepError::Configuration)?;
    let instruction_rip = state.rip;
    let mut memory = fetch_only_adapter(bus, paging, instruction_rip, &mut committed_write);
    let cpu = od_isa::step_mov_cr3_wp(
        od_isa::MovCr3WpLong64V1,
        state,
        &mut memory,
        context,
        efer,
        rflags_defined,
    )
    .map_err(PagedMovCr3StepError::CpuInput)?;
    Ok(PagedMovCr3StepReport {
        cpu,
        page_fault: memory.page_fault,
        access_error: memory.access_error,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PagedMovCr0StepReport {
    pub cpu: od_isa::PagedProbeResult<std::convert::Infallible>,
    /// Retains the walker's precise saved RIP, linear address, and PFEC.
    pub page_fault: Option<PageFaultRecord>,
    /// Retains the original tracked-walker classification, including faults.
    pub access_error: Option<TrackedAccessError>,
}

/// Execute exactly one bare `MOV CR0,RAX` WP toggle through tracked physical
/// RAM fetches (S4-16f). The CPU profile commits CR0 and reports a retired
/// `Cr0Write` effect; adopting the new snapshot is the caller's decision.
pub fn step_paged_mov_cr0_wp(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    rflags_defined: u64,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PagedMovCr0StepReport, crate::ProbeConfigError> {
    let paging = Long64ProbeConfig::from_cpu_state(state, cr3, efer)?;
    let instruction_rip = state.rip;
    let mut memory = fetch_only_adapter(bus, paging, instruction_rip, &mut committed_write);
    let cpu = od_isa::step_mov_cr0_wp(
        od_isa::MovCr0WpLong64V1,
        state,
        &mut memory,
        context,
        efer,
        rflags_defined,
    );
    Ok(PagedMovCr0StepReport {
        cpu,
        page_fault: memory.page_fault,
        access_error: memory.access_error,
    })
}

/// Report of one exact paged one-byte state step (`STI` or `HLT`, S4-27i4a).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PagedStateOpStepReport {
    pub cpu: od_isa::PagedProbeResult<std::convert::Infallible>,
    /// Retains the walker's precise saved RIP, linear address, and PFEC.
    pub page_fault: Option<PageFaultRecord>,
    /// Retains the original tracked-walker classification, including faults.
    pub access_error: Option<TrackedAccessError>,
}

/// Execute exactly one `STI` (`FB`) through tracked physical RAM fetches (S4-27i4a).
/// Like [`step_paged_invlpg`] the CPU profile owns decode and retirement and the
/// adapter supplies the paging snapshot and forwards committed walker A/D writes.
pub fn step_paged_sti(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    rflags_defined: u64,
    committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PagedStateOpStepReport, crate::ProbeConfigError> {
    step_paged_state_op(
        StateOp::Sti,
        state,
        bus,
        cr3,
        efer,
        context,
        rflags_defined,
        committed_write,
    )
}

/// Execute exactly one `HLT` (`F4`) through tracked physical RAM fetches (S4-27i4a).
/// See [`step_paged_sti`]; the halted CPU is woken by the caller through the paged
/// interrupt entry.
pub fn step_paged_hlt(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    rflags_defined: u64,
    committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PagedStateOpStepReport, crate::ProbeConfigError> {
    step_paged_state_op(
        StateOp::Hlt,
        state,
        bus,
        cr3,
        efer,
        context,
        rflags_defined,
        committed_write,
    )
}

#[derive(Clone, Copy)]
enum StateOp {
    Sti,
    Hlt,
}

#[allow(clippy::too_many_arguments)]
fn step_paged_state_op(
    op: StateOp,
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    rflags_defined: u64,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PagedStateOpStepReport, crate::ProbeConfigError> {
    let paging = Long64ProbeConfig::from_cpu_state(state, cr3, efer)?;
    let instruction_rip = state.rip;
    let mut memory = PagedMovMemoryAdapter {
        bus: bus.into(),
        paging,
        instruction_rip,
        committed_write: &mut committed_write,
        page_fault: None,
        memory_error: None,
        access_error: None,
        page_fault_trace: None,
        capture_page_fault_trace: false,
        committed_ad_updates: None,
        committed_writes: None,
        capture_effects: false,
        descriptor_gate: None,
        descriptor_scope: None,
        dword_read_profile: false,
        word_read_profile: false,
        ram_only: true,
        reject_crossing_qword: false,
        read_only_qword: false,
        large_ram_profile: false,
        dword_write_profile: false,
        cpl0_set_dword_read: false,
        cpl0_set_byte_read: false,
        cpl0_set_word_read: false,
        cpl0_set_narrow_write: false,
        cpl0_set_cross_page_fetch: false,
        cpl0_set_cross_page_store: false,
        cpl0_set_cross_page_read: false,
        fetch_cache: None,
        walks: None,
        decoded: DecodedStep::default(),
    };
    let cpu = match op {
        StateOp::Sti => od_isa::step_paged_sti(
            od_isa::PagedStiLong64V1,
            state,
            &mut memory,
            context,
            efer,
            rflags_defined,
        ),
        StateOp::Hlt => od_isa::step_paged_hlt(
            od_isa::PagedHltLong64V1,
            state,
            &mut memory,
            context,
            efer,
            rflags_defined,
        ),
    };
    Ok(PagedStateOpStepReport {
        cpu,
        page_fault: memory.page_fault,
        access_error: memory.access_error,
    })
}

/// Result of one synchronous JIT invocation against the tracked paged adapter.
/// The memory borrow ends before this value is returned and is never retained.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PagedMovMemoryScope<R> {
    pub callback_result: R,
    pub page_fault: Option<PageFaultRecord>,
    pub access_error: Option<TrackedAccessError>,
    pub committed_ad_updates: Vec<crate::AdUpdate>,
    pub committed_writes: Vec<GuestPhysicalRange>,
}

/// Run a synchronous callback with a temporary RAM-only, same-page qword MOV
/// memory adapter. The callback cannot retain the adapter beyond this call.
pub fn with_paged_mov_memory<R>(
    bus: &MachineBus,
    paging: Long64ProbeConfig,
    instruction_rip: u64,
    callback: impl FnOnce(&mut dyn GuestMemory) -> R,
) -> PagedMovMemoryScope<R> {
    with_paged_mov_read_memory(bus, paging, instruction_rip, false, callback)
}

/// S4-15e: the same RAM-only, read-only, same-page adapter as
/// [`with_paged_mov_memory`] for a four-byte operand read (`MOV EAX,[RBX]`).
/// Four-byte reads go through the ordinary tracked walker exactly like the
/// qword reads (Accessed bits recorded in `committed_writes`); a qword read is
/// (the operand width is enforced by the host: `execute_paged_checked_load_v2` admits only
/// dword operands); this adapter itself does not refuse an eight-byte read.
/// An operand that would cross a 4 KiB page is refused before any walk.
pub fn with_paged_mov_dword_memory<R>(
    bus: &MachineBus,
    paging: Long64ProbeConfig,
    instruction_rip: u64,
    callback: impl FnOnce(&mut dyn GuestMemory) -> R,
) -> PagedMovMemoryScope<R> {
    with_paged_mov_read_memory(bus, paging, instruction_rip, true, callback)
}

fn with_paged_mov_read_memory<R>(
    bus: &MachineBus,
    paging: Long64ProbeConfig,
    instruction_rip: u64,
    dword: bool,
    callback: impl FnOnce(&mut dyn GuestMemory) -> R,
) -> PagedMovMemoryScope<R> {
    let mut ignore_write = |_| {};
    let mut memory = PagedMovMemoryAdapter {
        bus: bus.into(),
        paging,
        instruction_rip,
        committed_write: &mut ignore_write,
        page_fault: None,
        memory_error: None,
        access_error: None,
        page_fault_trace: None,
        capture_page_fault_trace: false,
        committed_ad_updates: Some(Vec::new()),
        committed_writes: Some(Vec::new()),
        capture_effects: true,
        descriptor_gate: None,
        descriptor_scope: None,
        dword_read_profile: false,
        word_read_profile: false,
        ram_only: true,
        reject_crossing_qword: true,
        read_only_qword: true,
        large_ram_profile: false,
        dword_write_profile: false,
        // The dword read width is the only thing this flag changes without
        // `large_ram_profile`: the read stays an ordinary tracked access.
        cpl0_set_dword_read: dword,
        cpl0_set_byte_read: false,
        cpl0_set_word_read: false,
        cpl0_set_narrow_write: false,
        cpl0_set_cross_page_fetch: false,
        cpl0_set_cross_page_store: false,
        cpl0_set_cross_page_read: false,
        fetch_cache: None,
        walks: None,
        decoded: DecodedStep::default(),
    };
    let callback_result = callback(&mut memory);
    PagedMovMemoryScope {
        callback_result,
        page_fault: memory.page_fault,
        access_error: memory.access_error,
        committed_ad_updates: memory.committed_ad_updates.unwrap_or_default(),
        committed_writes: memory.committed_writes.unwrap_or_default(),
    }
}

/// Why the S4-15d store adapter refused a store before it walked or wrote
/// anything. A refusal is not an architectural result: no RAM byte, Accessed or
/// Dirty bit changed, and no device was reached.
///
/// S4-16i: a store onto the physical page the executing block was fetched from
/// is no longer a refusal. It is walked, checked and committed like any other
/// store; the host reports the resulting code invalidation (see
/// `MemoryOperationExecutionExitV2::CodeInvalidated`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PagedStoreRefusal {
    /// The translated operand is not wholly ordinary RAM (MMIO, ROM,
    /// framebuffer or unmapped). The interpreter owns every such store.
    NonRamTarget { physical: GuestPhysicalAddress },
    /// The read-only probe could not translate the operand at all (a
    /// non-canonical address, a 2 MiB or 1 GiB leaf that the 4 KiB walker does
    /// not serve, or an unreadable table entry). The JIT does not own such a
    /// store: nothing is walked or committed, so the interpreter sees the
    /// untouched page tables.
    ProbeUnsupported,
}

/// Result of [`with_paged_mov_store_memory`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PagedMovStoreScope<R> {
    pub memory: PagedMovMemoryScope<R>,
    /// Set when the adapter refused the store before any walk or write.
    pub refusal: Option<PagedStoreRefusal>,
}

/// Run a synchronous callback with a temporary store-only adapter for one
/// same-page qword store (S4-15d).
///
/// The adapter serves exactly one operation, `write` of 8 bytes inside one
/// 4 KiB page, through the tracked RAM walker (`access_tracked_ram`: the page
/// walk checks Present, Write (WP at CPL0), User (CPL3) and reserved bits,
/// never dispatches a device, and sets the Accessed bits and then the leaf
/// Dirty bit before the operand bytes are written). Everything else (fetch,
/// read, atomics, other widths, a page-crossing operand) is a typed backend
/// error. Before the walk that commits anything, a read-only probe translates
/// the operand; if it translates to anything other than whole ordinary RAM, the
/// store is refused with no effect at all. A store onto the page the executing
/// block was fetched from (S4-16i) is committed like any other: the adapter has
/// no notion of an active page, and the caller derives the code invalidation
/// from the committed write ranges. A probe
/// that cannot translate the operand (any probe error: unsupported page size,
/// non-canonical address, unreadable table) is refused the same way
/// ([`PagedStoreRefusal::ProbeUnsupported`]), so the tracked walk never commits
/// an Accessed bit for a store that nobody would execute. A probe that finds a
/// fault is not a refusal: the tracked access then reports the precise `#PF`
/// with its Accessed-bit effects, exactly like the interpreter.
///
/// A failed RAM write does not partially commit its operand
/// (`PhysicalMemory::store_bytes` validates the whole range first). The
/// callback cannot retain the adapter beyond this call.
pub fn with_paged_mov_store_memory<R>(
    bus: &MachineBus,
    paging: Long64ProbeConfig,
    instruction_rip: u64,
    callback: impl FnOnce(&mut dyn GuestMemory) -> R,
) -> PagedMovStoreScope<R> {
    let mut memory = PagedMovStoreMemory {
        bus,
        paging,
        instruction_rip,
        page_fault: None,
        access_error: None,
        refusal: None,
        committed_ad_updates: Vec::new(),
        committed_writes: Vec::new(),
    };
    let callback_result = callback(&mut memory);
    PagedMovStoreScope {
        memory: PagedMovMemoryScope {
            callback_result,
            page_fault: memory.page_fault,
            access_error: memory.access_error,
            committed_ad_updates: memory.committed_ad_updates,
            committed_writes: memory.committed_writes,
        },
        refusal: memory.refusal,
    }
}

struct PagedMovStoreMemory<'a> {
    bus: &'a MachineBus,
    paging: Long64ProbeConfig,
    instruction_rip: u64,
    page_fault: Option<PageFaultRecord>,
    access_error: Option<TrackedAccessError>,
    refusal: Option<PagedStoreRefusal>,
    committed_ad_updates: Vec<crate::AdUpdate>,
    committed_writes: Vec<GuestPhysicalRange>,
}

impl PagedMovStoreMemory<'_> {
    fn unsupported(operation: &'static str) -> MemoryError {
        MemoryError::Backend(format!(
            "paged MOV store profile does not support {operation}"
        ))
    }

    fn refuse(&mut self, refusal: PagedStoreRefusal) -> MemoryError {
        self.refusal = Some(refusal);
        MemoryError::Backend(format!("paged MOV store refused: {refusal:?}"))
    }
}

impl GuestMemory for PagedMovStoreMemory<'_> {
    fn fetch(&mut self, _address: u64, _context: AccessContext) -> Result<u8, MemoryError> {
        Err(Self::unsupported("instruction fetch"))
    }

    fn read(
        &mut self,
        _address: u64,
        _context: AccessContext,
        _destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        Err(Self::unsupported("read"))
    }

    fn write(
        &mut self,
        address: u64,
        context: AccessContext,
        source: &[u8],
    ) -> Result<(), MemoryError> {
        let Ok(bytes) = <[u8; 8]>::try_from(source) else {
            return Err(Self::unsupported("non-qword writes"));
        };
        if (address & 0xfff) + 8 > 4096 {
            return Err(Self::unsupported("cross-page qword store"));
        }
        // Same normalization as the shared adapter: explicit origin, the
        // CPL of the access as the page privilege.
        let context = AccessContext {
            origin: AccessOrigin::Explicit,
            privilege: context.page_privilege(),
            ..context
        };
        let probe = self.bus.probe_tracked_ram_access(
            &self.paging,
            GuestLinearAddress::new(address),
            AccessKind::Write,
            context,
        );
        match probe.outcome {
            Ok(crate::ProbeOutcome::Translated(translation)) => {
                let physical = translation.physical;
                let is_ram = GuestPhysicalRange::new(physical, 8)
                    .is_ok_and(|range| self.bus.tracked_physical_range_is_ram(range));
                if !is_ram {
                    return Err(self.refuse(PagedStoreRefusal::NonRamTarget { physical }));
                }
            }
            // A probe fault is not decided here: the tracked access below
            // reports the precise #PF with its Accessed-bit effects.
            Ok(crate::ProbeOutcome::PageFault(_)) => {}
            // Any probe error means the operand is not served by this adapter:
            // refuse before the commit walk (no Accessed or Dirty bit, no data).
            Err(_) => return Err(self.refuse(PagedStoreRefusal::ProbeUnsupported)),
        }
        let request = TrackedAccessRequest {
            instruction_rip: self.instruction_rip,
            address: GuestLinearAddress::new(address),
            context,
            operation: RamAccessOperation::Write {
                width: crate::AccessWidth::Qword,
                value: u64::from_le_bytes(bytes),
            },
        };
        let mut writes = Vec::new();
        let report = self
            .bus
            .access_tracked_ram(&self.paging, request, |range| writes.push(range));
        self.committed_writes.extend(writes);
        self.committed_ad_updates.extend(
            report.ad_updates[..report.ad_update_count]
                .iter()
                .flatten()
                .copied(),
        );
        match report.outcome {
            Ok(TrackedAccessOutcome::Written) => Ok(()),
            Ok(_) => Err(Self::unsupported("non-write result")),
            Err(TrackedAccessError::PageFault { record, reason }) => {
                let fault = record.fault().clone();
                self.page_fault = Some(record.clone());
                self.access_error = Some(TrackedAccessError::PageFault { record, reason });
                Err(MemoryError::Architectural(fault))
            }
            Err(error) => {
                let message = format!("tracked Long64 RAM store failed: {error:?}");
                self.access_error = Some(error);
                Err(MemoryError::Backend(message))
            }
        }
    }

    fn atomic_exchange_integer(
        &mut self,
        _address: u64,
        _context: AccessContext,
        _size: od_isa::OperandSize,
        _replacement: u64,
    ) -> Result<u64, MemoryError> {
        Err(Self::unsupported("atomic exchange"))
    }

    fn atomic_fetch_add_integer(
        &mut self,
        _address: u64,
        _context: AccessContext,
        _size: od_isa::OperandSize,
        _addend: u64,
    ) -> Result<u64, MemoryError> {
        Err(Self::unsupported("atomic fetch-add"))
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        _address: u64,
        _context: AccessContext,
        _size: od_isa::OperandSize,
        _expected: u64,
        _replacement: u64,
    ) -> Result<u64, MemoryError> {
        Err(Self::unsupported("atomic compare-exchange"))
    }

    fn mfence(&mut self, _context: AccessContext) -> Result<(), MemoryError> {
        Err(Self::unsupported("MFENCE"))
    }
}

/// Execute the CPU profile's exact one-step MOV through four-level 4 KiB RAM paging.
/// CR3 and EFER are explicit because they are not owned by `LongModeState`.
///
/// The adapter forces `AccessOrigin::Explicit` while preserving the caller's CPL.
/// This entry represents an explicit MOV memory access and must not inherit the
/// supervisor privilege used by descriptor-table accesses.
pub fn step_paged_mov(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    rflags_defined: u64,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PagedMovStepReport, PagedMovStepError> {
    let paging = Long64ProbeConfig::from_cpu_state(state, cr3, efer)
        .map_err(PagedMovStepError::Configuration)?;
    let context = AccessContext {
        origin: od_isa::memory::AccessOrigin::Explicit,
        ..context
    };
    let instruction_rip = state.rip;
    let mut memory = PagedMovMemoryAdapter {
        bus: bus.into(),
        paging,
        instruction_rip,
        committed_write: &mut committed_write,
        page_fault: None,
        memory_error: None,
        access_error: None,
        page_fault_trace: None,
        capture_page_fault_trace: false,
        committed_ad_updates: None,
        committed_writes: None,
        capture_effects: false,
        descriptor_gate: None,
        descriptor_scope: None,
        dword_read_profile: false,
        word_read_profile: false,
        ram_only: false,
        reject_crossing_qword: false,
        read_only_qword: false,
        large_ram_profile: false,
        dword_write_profile: false,
        cpl0_set_dword_read: false,
        cpl0_set_byte_read: false,
        cpl0_set_word_read: false,
        cpl0_set_narrow_write: false,
        cpl0_set_cross_page_fetch: false,
        cpl0_set_cross_page_store: false,
        cpl0_set_cross_page_read: false,
        fetch_cache: None,
        walks: None,
        decoded: DecodedStep::default(),
    };
    let cpu = cpu_step_paged_mov(state, &mut memory, context, rflags_defined)
        .map_err(PagedMovStepError::CpuInput)?;
    Ok(PagedMovStepReport {
        cpu,
        page_fault: memory.page_fault,
        memory_error: memory.memory_error,
    })
}

/// Execute the exact one-step qword MOV through RAM-only 4 KiB/2 MiB paging.
///
/// Like [`step_paged_mov`] but a PDE with PS=1 is a 2 MiB leaf. The profile is
/// bounded: RAM only (no MMIO), exact qword operands that must not cross a
/// 4 KiB boundary, and instruction fetch confined to the 4 KiB page of RIP.
/// Accessed/dirty bits are set on the leaf actually used (PDE for 2 MiB).
pub fn step_paged_mov_2m_ram(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    rflags_defined: u64,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PagedMovStepReport, PagedMovStepError> {
    let paging = Long64ProbeConfig::from_cpu_state(state, cr3, efer)
        .map_err(PagedMovStepError::Configuration)?;
    let context = AccessContext {
        origin: od_isa::memory::AccessOrigin::Explicit,
        ..context
    };
    let instruction_rip = state.rip;
    let mut memory = PagedMovMemoryAdapter {
        bus: bus.into(),
        paging,
        instruction_rip,
        committed_write: &mut committed_write,
        page_fault: None,
        memory_error: None,
        access_error: None,
        page_fault_trace: None,
        capture_page_fault_trace: false,
        committed_ad_updates: None,
        committed_writes: None,
        capture_effects: false,
        descriptor_gate: None,
        descriptor_scope: None,
        dword_read_profile: false,
        word_read_profile: false,
        ram_only: true,
        reject_crossing_qword: false,
        read_only_qword: false,
        large_ram_profile: true,
        dword_write_profile: false,
        cpl0_set_dword_read: false,
        cpl0_set_byte_read: false,
        cpl0_set_word_read: false,
        cpl0_set_narrow_write: false,
        cpl0_set_cross_page_fetch: false,
        cpl0_set_cross_page_store: false,
        cpl0_set_cross_page_read: false,
        fetch_cache: None,
        walks: None,
        decoded: DecodedStep::default(),
    };
    let cpu = cpu_step_paged_mov(state, &mut memory, context, rflags_defined)
        .map_err(PagedMovStepError::CpuInput)?;
    Ok(PagedMovStepReport {
        cpu,
        page_fault: memory.page_fault,
        memory_error: memory.memory_error,
    })
}

/// Final exit of [`run_cpl0_set_paged_2m_ram`].
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum PagedCpl0SetExit {
    /// The set's own exit. `InstructionBudgetExhausted` and `StopRequested` are
    /// raised by the loop between instructions, before any fetch.
    Run(RunExit),
    /// Typed admission failure of the set (nothing was fetched by that step).
    Rejected(Cpl0ProfileSetInputError),
    /// The walker snapshot could not be built from the CPU control registers.
    Configuration(crate::ProbeConfigError),
    /// The retired MOV CR3 does not match the walker that executed it.
    Cr3EffectMismatch {
        active_cr3: u64,
        effect_old_cr3: u64,
        cpu_cr3: Option<u64>,
        effect_new_cr3: u64,
    },
    /// The walker snapshot for the new CR3 was rejected.
    Cr3SnapshotRejected {
        effect: TranslationContextChange,
        error: crate::ProbeConfigError,
    },
    /// The set reported a context change the board does not rebuild for.
    UnexpectedContextChange { effect: TranslationContextChange },
    /// The port of `IN AL, DX` / `OUT DX, AL` (`with_pio_u8`) failed. RIP is on the
    /// instruction, which did not retire.
    PortIo(crate::BusAccessError),
    /// The CPUID boundary (S4-38L4): the instruction at RIP is a `CPUID` whose leaf (EAX) is
    /// one the diagnostic profile must not answer. Nothing was fetched past the opcode bytes
    /// and the instruction did not execute: RIP is on it and no register changed.
    CpuidDivergence { leaf: u32 },
    /// The `IRETQ` user_entry boundary (S4-38L9I, from S4-38L9H): the set stopped at an `IRETQ`
    /// whose validated frame returns to CPL3. The instruction did not retire: RIP is on it and
    /// no register, flag or memory changed. The frame is the one the guest would have been
    /// returned to. The set loop ends the run here; nothing is walked or fetched past it.
    UserEntry(IretqUserEntry),
    /// The set reported an exit variant this board does not know (the exit enum
    /// is `#[non_exhaustive]`).
    UnknownSetExit,
    /// A guest `#PF` (vector 14, S4-38L5c) or `#GP(0)` (vector 13, error code 0, S4-38L9v) could
    /// not be delivered through the guest IDT (the IDT is loaded): the original fault is the
    /// stop, `failure` says why delivery failed. Without a loaded IDT, and for every other fault
    /// vector, the fault is a plain `Run(Fault)` as before.
    ExceptionDeliveryFailed {
        fault: ArchitecturalFault,
        failure: PagedCpl0ExceptionFailure,
    },
    /// An external interrupt of the board source (S4-38L8b) could not be delivered, although the
    /// loop had checked IF and the STI shadow first: a refusal other than
    /// `InterruptsMasked`/`StiShadowActive` (a broken gate, a missing IDT, and also
    /// `ActivityOutsideProfile`, an unsupported privilege/context, or a failed memory access of
    /// the delivery. CPL3 delivery switches through `TSS.RSP0` to CPL0. The run stops with an
    /// error (`run_failed` on the board), never as a quiet skip; nothing was acknowledged. RIP
    /// is on the instruction that was about to run.
    IrqDeliveryFailed {
        vector: u8,
        failure: IrqDeliveryFailure,
    },
}

/// Why an external interrupt of the board was not delivered (S4-38L8b).
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum IrqDeliveryFailure {
    /// The delivery was refused before any write (see `Cpl0ExternalInterruptRefusal`).
    Refused(Cpl0ExternalInterruptRefusal),
    /// A memory access of the delivery failed; the CPU state is unchanged.
    Aborted {
        phase: Cpl0ExternalInterruptPhase,
        cause: MemoryError,
    },
}

/// Why a guest `#PF` or `#GP(0)` did not enter its CPL0 handler.
#[derive(Debug, Eq, PartialEq)]
pub enum PagedCpl0ExceptionFailure {
    /// The walker left no precise `PageFaultRecord` for the fault (integration error).
    NoPageFaultRecord,
    /// The delivery was refused before any write (see `Cpl0PageFaultRefusal`).
    Refused(Cpl0PageFaultRefusal),
    /// A memory access of the delivery failed (a `#PF` there is a double fault, not modeled).
    /// The CPU state is unchanged; frame words may have been stored below the old RSP.
    Aborted {
        phase: Cpl0PageFaultDeliveryPhase,
        cause: MemoryError,
    },
    /// The `#GP(0)` delivery was refused before any write (S4-38L9v).
    GeneralProtectionRefused(Cpl0GeneralProtectionRefusal),
    /// A memory access of the `#GP(0)` delivery failed (S4-38L9v); the CPU state is unchanged.
    GeneralProtectionAborted {
        phase: Cpl0GeneralProtectionPhase,
        cause: MemoryError,
    },
    /// [`MAX_DELIVERIES_WITHOUT_RETIREMENT`] deliveries in a row retired nothing: a fault
    /// that the handler chain does not resolve and that would otherwise never use up the
    /// instruction budget.
    RepeatLimit,
}

/// RFLAGS.RF (resume flag), bit 16.
const RFLAGS_RF: u64 = 1 << 16;

/// Consecutive `#PF` deliveries without a retired instruction in between that the loop
/// allows. A handler that returns to a faulting instruction retires at least its `IRETQ`,
/// so this only bounds a fault at the handler's own first fetch (each delivery pushes a
/// frame, so the stack ends such a chain anyway; this keeps the bound explicit).
pub(crate) const MAX_DELIVERIES_WITHOUT_RETIREMENT: u64 = 256;

/// CPUID leaves the diagnostic identity must not answer on the Linux board (S4-38L4, checkpoint
/// C7): the feature leaves, where a made-up answer would decide which kernel paths are taken.
/// Every other leaf is answered by the profile or stays unsupported.
pub(crate) const fn is_cpuid_divergence_leaf(leaf: u32) -> bool {
    matches!(leaf, 1 | 0x8000_0001)
}

/// Prefix bytes the peek of [`cpuid_leaf_at_rip`] skips before `0F A2` (legacy prefixes and
/// REX); the opcode itself is decided by the set, this only keeps a prefixed `CPUID` from slipping
/// past the boundary.
const fn is_cpuid_peek_prefix(byte: u8) -> bool {
    matches!(
        byte,
        0x26 | 0x2E | 0x36 | 0x3E | 0x64 | 0x65 | 0x66 | 0x67 | 0xF2 | 0xF3 | 0x40..=0x4F
    )
}

/// The leaf (EAX) of a `CPUID` at RIP, or `None` when the bytes at RIP are not a `CPUID` or do
/// not translate (the instruction itself then reports that). Read-only: every byte goes through
/// the same walk as an instruction fetch without writing an accessed/dirty bit, and nothing
/// but RAM is read.
fn cpuid_leaf_at_rip(
    state: &LongModeState,
    memory: &crate::PhysicalMemory,
    paging: &Long64ProbeConfig,
    context: AccessContext,
    cache: Option<&FetchTranslationCache>,
) -> Option<u32> {
    let byte_at = |offset: u64| -> Option<u8> {
        let linear = state.rip.wrapping_add(offset);
        if let Some(byte) = cache
            .and_then(|cache| cache.lookup(linear, paging, context))
            .and_then(|frame| memory.read_ram_frame_u8(frame, linear))
        {
            return Some(byte);
        }
        let physical = crate::page_walk::translate_long64_2m_ram_read_only(
            memory,
            paging,
            state.rip,
            GuestLinearAddress::new(state.rip.wrapping_add(offset)),
            context,
            od_isa::memory::AccessKind::Fetch,
        )
        .ok()?;
        memory.load_u8(physical).ok()
    };
    // Most instructions neither start with a prefix nor with `0F`: one walk decides that.
    let first = byte_at(0)?;
    if first != 0x0F && !is_cpuid_peek_prefix(first) {
        return None;
    }
    let mut offset = 0u64;
    // Up to 13 prefixes and then the two opcode bytes fit the 15-byte instruction limit.
    while offset < 13 && is_cpuid_peek_prefix(byte_at(offset)?) {
        offset += 1;
    }
    if byte_at(offset)? == 0x0F && byte_at(offset + 1)? == 0xA2 {
        Some(state.gpr[od_isa::Register::Rax as usize] as u32)
    } else {
        None
    }
}

/// The fetch translation of RIP for the checkpoint recorder (S4-38L4i): the physical address and
/// up to 16 bytes there, each translated like an instruction fetch through the read-only walk
/// (no accessed/dirty bit), RAM only. Shorter bytes when one does not translate.
fn observed_fetch_view(
    rip: u64,
    memory: &crate::PhysicalMemory,
    paging: &Long64ProbeConfig,
    context: AccessContext,
) -> (Option<u64>, Vec<u8>) {
    let mut first = None;
    let mut bytes = Vec::with_capacity(16);
    for offset in 0..16u64 {
        let Ok(physical) = crate::page_walk::translate_long64_2m_ram_read_only(
            memory,
            paging,
            rip,
            GuestLinearAddress::new(rip.wrapping_add(offset)),
            context,
            od_isa::memory::AccessKind::Fetch,
        ) else {
            break;
        };
        if offset == 0 {
            first = Some(physical.get());
        }
        let Ok(byte) = memory.load_u8(physical) else {
            break;
        };
        bytes.push(byte);
    }
    (first, bytes)
}

/// Up to `length` guest bytes at the linear address `address`, read like a data read through the
/// read-only walk (no accessed or dirty bit), RAM only; shorter when a byte does not translate
/// (S7-31q call trace).
fn observed_read_bytes(
    address: u64,
    length: usize,
    rip: u64,
    memory: &crate::PhysicalMemory,
    paging: &Long64ProbeConfig,
    context: AccessContext,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(length);
    for offset in 0..length as u64 {
        let Ok(physical) = crate::page_walk::translate_long64_2m_ram_read_only(
            memory,
            paging,
            rip,
            GuestLinearAddress::new(address.wrapping_add(offset)),
            context,
            od_isa::memory::AccessKind::Read,
        ) else {
            break;
        };
        let Ok(byte) = memory.load_u8(physical) else {
            break;
        };
        bytes.push(byte);
    }
    bytes
}

#[derive(Clone, Copy)]
struct InstructionPathBefore {
    rip: u64,
    control_flow: Option<InstructionPathControlFlow>,
}

struct InstructionPathBytes<'a> {
    rip: u64,
    bytes: &'a [u8],
}

impl GuestMemory for InstructionPathBytes<'_> {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        let offset = address.wrapping_sub(self.rip) as usize;
        self.bytes
            .get(offset)
            .copied()
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }

    fn read(&mut self, _: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Backend("trace byte view is read-only".into()))
    }

    fn write(&mut self, _: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Backend("trace byte view is read-only".into()))
    }

    fn atomic_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: od_isa::OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend("trace byte view is read-only".into()))
    }

    fn atomic_fetch_add_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: od_isa::OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend("trace byte view is read-only".into()))
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: od_isa::OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend("trace byte view is read-only".into()))
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Err(MemoryError::Backend("trace byte view is read-only".into()))
    }
}

/// PERF-20: the page-lazy read-only instruction fetch of the histogram: each page is translated once
/// through the read-only walk (no accessed/dirty bit, RAM only) and the other bytes of the page
/// follow by offset. Only `fetch` works; the decoder reads nothing else.
struct HistogramFetch<'a> {
    memory: &'a crate::PhysicalMemory,
    paging: &'a Long64ProbeConfig,
    rip: u64,
    /// The linear page base and the physical address of its first byte.
    page: Option<(u64, u64)>,
}

impl GuestMemory for HistogramFetch<'_> {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        let unmapped = || MemoryError::Unmapped {
            address,
            access: AccessKind::Fetch,
        };
        let base = address & !0xFFF;
        let physical = match self.page {
            Some((linear, physical)) if linear == base => physical + (address & 0xFFF),
            _ => {
                let physical = crate::page_walk::translate_long64_2m_ram_read_only(
                    self.memory,
                    self.paging,
                    self.rip,
                    GuestLinearAddress::new(address),
                    context,
                    AccessKind::Fetch,
                )
                .map_err(|_| unmapped())?
                .get();
                self.page = Some((base, physical - (address & 0xFFF)));
                physical
            }
        };
        self.memory
            .load_u8(GuestPhysicalAddress::new(physical))
            .map_err(|_| unmapped())
    }

    fn read(&mut self, _: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Backend("histogram fetch is read-only".into()))
    }

    fn write(&mut self, _: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Backend("histogram fetch is read-only".into()))
    }

    fn atomic_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: od_isa::OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend("histogram fetch is read-only".into()))
    }

    fn atomic_fetch_add_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: od_isa::OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend("histogram fetch is read-only".into()))
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: od_isa::OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend("histogram fetch is read-only".into()))
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Err(MemoryError::Backend("histogram fetch is read-only".into()))
    }
}

/// PERF-20: the instruction at `rip` decoded without any side effect; `None` when its bytes do not
/// translate or the decoder refuses them (the step then reports that itself).
fn histogram_decode(
    rip: u64,
    memory: &crate::PhysicalMemory,
    paging: &Long64ProbeConfig,
    context: AccessContext,
) -> Option<Instruction> {
    let mut fetch = HistogramFetch {
        memory,
        paging,
        rip,
        page: None,
    };
    od_isa::long_decode::decode(&mut fetch, rip, context).ok()
}

fn trace_destination_register(
    registers: &mut [Option<InstructionPathRegisterValue>; 2],
    state: &LongModeState,
    register: od_isa::Register,
) {
    if registers
        .iter()
        .flatten()
        .any(|value| value.register == register as u8)
    {
        return;
    }
    if let Some(slot) = registers.iter_mut().find(|slot| slot.is_none()) {
        *slot = Some(InstructionPathRegisterValue {
            register: register as u8,
            value: state.gpr[register as usize],
        });
    }
}

fn instruction_path_before(
    rip: u64,
    state: &LongModeState,
    bus: &MachineBus,
    paging: &Long64ProbeConfig,
    context: AccessContext,
) -> InstructionPathBefore {
    let (_, bytes) = observed_fetch_view(rip, bus.physical_memory(), paging, context);
    let mut memory = InstructionPathBytes { rip, bytes: &bytes };
    let decoded = od_isa::long_decode::decode(&mut memory, rip, context).ok();
    let control_flow = decoded.and_then(|instruction| {
        let (kind, condition, taken, operand) = match instruction.operation {
            Operation::Branch { condition, .. } => (
                "branch",
                Some(condition as u8),
                condition.evaluate(state.rflags),
                None,
            ),
            Operation::Jump { .. } => ("jump", None, true, None),
            Operation::Call { .. } => ("call", None, true, None),
            Operation::Return { .. } | Operation::FarReturn | Operation::IretQ => {
                ("return", None, true, None)
            }
            Operation::Syscall | Operation::Sysenter => ("syscall", None, true, None),
            Operation::Sysret | Operation::Sysexit => ("sysret", None, true, None),
            Operation::IndirectJump { operand } => ("indirect_jump", None, true, Some(operand)),
            Operation::IndirectCall { operand } => ("indirect_call", None, true, Some(operand)),
            _ => return None,
        };
        let mut destination_registers = [None, None];
        match operand {
            Some(Operand::Register(view)) => {
                trace_destination_register(&mut destination_registers, state, view.register());
            }
            Some(Operand::Memory(memory)) => {
                if let AddressBase::Register(register) = memory.base {
                    trace_destination_register(&mut destination_registers, state, register);
                }
                if let Some(register) = memory.index {
                    trace_destination_register(&mut destination_registers, state, register);
                }
            }
            Some(Operand::Immediate(_)) | None => {}
        }
        if kind == "return" {
            trace_destination_register(&mut destination_registers, state, od_isa::Register::Rsp);
        }
        if kind == "syscall" || kind == "sysret" {
            trace_destination_register(&mut destination_registers, state, od_isa::Register::Rcx);
        }
        Some(InstructionPathControlFlow {
            kind,
            condition,
            taken,
            rflags_before: state.rflags,
            rflags_after: 0,
            destination_registers,
            destination: 0,
        })
    });
    InstructionPathBefore { rip, control_flow }
}

fn finish_instruction_path_record(
    before: InstructionPathBefore,
    state: &LongModeState,
) -> InstructionPathTraceRecord {
    let control_flow = before.control_flow.map(|mut flow| {
        flow.rflags_after = state.rflags;
        flow.destination = state.rip;
        flow
    });
    InstructionPathTraceRecord {
        rip: before.rip,
        next_rip: state.rip,
        control_flow,
    }
}

fn capture_instruction_path_frame_chain(
    state: &LongModeState,
    bus: &MachineBus,
    paging: &Long64ProbeConfig,
    context: AccessContext,
) -> (Vec<crate::ConsoleContextFrame>, crate::FrameChainEnd) {
    let rbp = state.gpr[od_isa::Register::Rbp as usize];
    capture_frame_chain(rbp, |frame_pointer| {
        let mut bytes = [0; 16];
        bus.read_virtual_ram_diagnostic(
            paging,
            state.rip,
            GuestLinearAddress::new(frame_pointer),
            context,
            &mut bytes,
        )
        .map(|()| bytes)
        .map_err(|_| ())
    })
}

fn capture_elf_stack_copy_target_rbp_chain(
    state: &LongModeState,
    bus: &MachineBus,
    paging: &Long64ProbeConfig,
    instruction_rip: u64,
    context: AccessContext,
) -> (
    ElfStackCopyTargetCr3ChainStatus,
    Vec<ElfStackCopyTargetCr3Frame>,
    Option<String>,
) {
    let mut rbp = state.gpr[od_isa::Register::Rbp as usize];
    let mut previous_rbp = None;
    let mut frames = Vec::with_capacity(crate::LINUX_ELF_STACK_COPY_RBP_FRAME_CAPACITY);
    for _ in 0..crate::LINUX_ELF_STACK_COPY_RBP_FRAME_CAPACITY {
        if rbp == 0 {
            return (ElfStackCopyTargetCr3ChainStatus::Complete, frames, None);
        }
        if (rbp & 7) != 0 || !is_canonical_48_bit(rbp) {
            return (
                ElfStackCopyTargetCr3ChainStatus::Malformed,
                frames,
                Some(format!("invalid frame pointer 0x{rbp:016x}")),
            );
        }
        if previous_rbp.is_some_and(|previous| rbp <= previous) {
            return (
                ElfStackCopyTargetCr3ChainStatus::Malformed,
                frames,
                Some(format!("non-monotonic frame pointer 0x{rbp:016x}")),
            );
        }
        let Some(return_address_slot) = rbp.checked_add(8) else {
            return (
                ElfStackCopyTargetCr3ChainStatus::Malformed,
                frames,
                Some("frame slot address overflow".into()),
            );
        };
        let mut bytes = [0u8; 16];
        if let Err(error) = bus.read_virtual_ram_diagnostic(
            paging,
            instruction_rip,
            GuestLinearAddress::new(rbp),
            context,
            &mut bytes,
        ) {
            return (
                ElfStackCopyTargetCr3ChainStatus::Unreadable,
                frames,
                Some(error),
            );
        }
        let saved_rbp = u64::from_le_bytes(bytes[..8].try_into().expect("8-byte slice"));
        let return_address = u64::from_le_bytes(bytes[8..].try_into().expect("8-byte slice"));
        frames.push(ElfStackCopyTargetCr3Frame {
            rbp,
            saved_rbp_slot: rbp,
            saved_rbp,
            return_address_slot,
            return_address,
        });
        if !is_canonical_48_bit(return_address) {
            return (
                ElfStackCopyTargetCr3ChainStatus::Malformed,
                frames,
                Some(format!(
                    "non-canonical return address 0x{return_address:016x}"
                )),
            );
        }
        if saved_rbp == 0 {
            return (ElfStackCopyTargetCr3ChainStatus::Complete, frames, None);
        }
        if (saved_rbp & 7) != 0 || !is_canonical_48_bit(saved_rbp) || saved_rbp <= rbp {
            return (
                ElfStackCopyTargetCr3ChainStatus::Malformed,
                frames,
                Some(format!("malformed saved RBP 0x{saved_rbp:016x}")),
            );
        }
        previous_rbp = Some(rbp);
        rbp = saved_rbp;
    }
    (
        ElfStackCopyTargetCr3ChainStatus::FrameLimit,
        frames,
        Some("frame chain reached the 8-frame capture limit".into()),
    )
}

fn is_canonical_48_bit(address: u64) -> bool {
    let sign = (address >> 47) & 1;
    (sign == 0 && address >> 48 == 0) || (sign == 1 && address >> 48 == 0xffff)
}

#[cfg(test)]
thread_local! {
    static MMU_GATHER_DIAGNOSTIC_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static MMU_GATHER_PAGE_WALKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn capture_elf_mmu_gather_boundary(
    instruction_rip: u64,
    state: &LongModeState,
    bus: &MachineBus,
    paging: &Long64ProbeConfig,
    context: AccessContext,
    cr3: u64,
    observations: &mut LinuxExecutionObservationCollector,
) {
    if !observations.wants_mmu_gather_capture()
        || !matches!(
            instruction_rip,
            LINUX_SETUP_ARG_PAGES_STACK_SHIFT_RIP
                | LINUX_RELOCATE_VMA_DOWN_TLB_FINISH_CALL_RIP
                | LINUX_TLB_FINISH_MMU_ACTUAL_POST_RIP
                | LINUX_RELOCATE_VMA_DOWN_TLB_FINISH_RETURN_RIP
        )
    {
        return;
    }
    use od_isa::Register::{R14, R15, Rbp, Rdi, Rsp};
    let rbp = state.gpr[Rbp as usize];
    let rsp = state.gpr[Rsp as usize];
    let frame = read_mmu_gather_frame(bus, paging, instruction_rip, context, rbp);
    let (saved_rbp, saved_return_rip, frame_error) = match frame {
        Ok((saved_rbp, saved_return_rip)) => (Some(saved_rbp), Some(saved_return_rip), None),
        Err(error) => (None, None, Some(error)),
    };
    let tlb_pointer = matches!(
        instruction_rip,
        LINUX_RELOCATE_VMA_DOWN_TLB_FINISH_CALL_RIP | LINUX_TLB_FINISH_MMU_ACTUAL_POST_RIP
    )
    .then_some(state.gpr[Rdi as usize]);
    let event_context = observations.mmu_gather_context(
        instruction_rip,
        cr3,
        rbp,
        rsp,
        tlb_pointer,
        saved_rbp,
        saved_return_rip,
    );
    match instruction_rip {
        LINUX_SETUP_ARG_PAGES_STACK_SHIFT_RIP => {
            let vma = state.gpr[R15 as usize];
            let bounds = read_mmu_gather_vma_bounds(bus, paging, instruction_rip, context, vma);
            let (vm_start, vm_end, vma_error) = match bounds {
                Ok((start, end)) => (Some(start), Some(end), None),
                Err(error) => (None, None, Some(error)),
            };
            let error = frame_error.or(vma_error);
            let setup_index_and_candidates = observations.record_mmu_gather_setup(
                event_context,
                state.gpr[R14 as usize],
                vma,
                vm_start,
                vm_end,
                if error.is_none() {
                    "captured"
                } else {
                    "read_error"
                },
                error,
            );
            if let Some((setup_index, candidates)) = setup_index_and_candidates {
                let page_walks = capture_mmu_gather_candidate_page_walks(
                    candidates,
                    bus,
                    paging,
                    instruction_rip,
                    context,
                );
                observations.record_mmu_gather_setup_page_walks(setup_index, page_walks);
            }
        }
        LINUX_RELOCATE_VMA_DOWN_TLB_FINISH_CALL_RIP => {
            let candidate_pages =
                observations.mmu_gather_candidate_pages_for_relocate(event_context);
            let page_walks = candidate_pages.map(|candidates| {
                capture_mmu_gather_candidate_page_walks(
                    candidates,
                    bus,
                    paging,
                    instruction_rip,
                    context,
                )
            });
            let (raw, raw_error) = read_mmu_gather_raw(
                bus,
                paging,
                instruction_rip,
                context,
                state.gpr[Rdi as usize],
            );
            let error = frame_error.or(raw_error);
            observations.record_mmu_gather_relocate_with_walks(
                event_context,
                raw,
                if error.is_none() {
                    "captured"
                } else {
                    "read_error"
                },
                error,
                page_walks,
            );
        }
        LINUX_TLB_FINISH_MMU_ACTUAL_POST_RIP => {
            let (raw, raw_error) = read_mmu_gather_raw(
                bus,
                paging,
                instruction_rip,
                context,
                state.gpr[Rdi as usize],
            );
            let error = frame_error.or(raw_error);
            observations.record_mmu_gather_actual_post(
                event_context,
                raw,
                if error.is_none() {
                    "captured"
                } else {
                    "read_error"
                },
                error,
            );
        }
        LINUX_RELOCATE_VMA_DOWN_TLB_FINISH_RETURN_RIP => {
            let candidate_pages = observations.mmu_gather_candidate_pages_for_return(event_context);
            let page_walks = candidate_pages.map(|candidates| {
                capture_mmu_gather_candidate_page_walks(
                    candidates,
                    bus,
                    paging,
                    instruction_rip,
                    context,
                )
            });
            observations.record_mmu_gather_return_with_walks(
                event_context,
                if frame_error.is_none() {
                    "captured"
                } else {
                    "read_error"
                },
                frame_error,
                page_walks,
            );
        }
        _ => {}
    }
}

fn capture_mmu_gather_candidate_page_walks(
    candidates: [Option<u64>; 2],
    bus: &MachineBus,
    paging: &Long64ProbeConfig,
    instruction_rip: u64,
    context: AccessContext,
) -> [ElfMmuGatherPageWalk; 2] {
    let cr0_wp = paging.cr0() & (1 << 16) != 0;
    let efer_nxe = paging.efer() & (1 << 11) != 0;
    candidates.map(|address| {
        let Some(address) = address else {
            return ElfMmuGatherPageWalk {
                address: None,
                access: "Write",
                cpl: 0,
                origin: "Explicit",
                profile: "long64_4k_2m",
                cr0_wp,
                efer_nxe,
                status: "invalid_address",
                entries: Vec::new(),
                terminal_level: None,
                leaf_size: None,
                translated_physical: None,
                error: Some("candidate address derivation failed".into()),
            };
        };
        if !is_canonical_48_bit(address) {
            return ElfMmuGatherPageWalk {
                address: Some(address),
                access: "Write",
                cpl: 0,
                origin: "Explicit",
                profile: "long64_4k_2m",
                cr0_wp,
                efer_nxe,
                status: "invalid_address",
                entries: Vec::new(),
                terminal_level: None,
                leaf_size: None,
                translated_physical: None,
                error: Some("candidate address is noncanonical".into()),
            };
        }
        #[cfg(test)]
        {
            MMU_GATHER_DIAGNOSTIC_READS.with(|count| count.set(count.get() + 1));
            MMU_GATHER_PAGE_WALKS.with(|count| count.set(count.get() + 1));
        }
        let write_context = AccessContext {
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
            ..context
        };
        let (translation, trace) = {
            let ram = bus.physical_memory();
            translate_long64_2m_ram_read_only_with_trace(
                ram,
                paging,
                instruction_rip,
                GuestLinearAddress::new(address),
                write_context,
                AccessKind::Write,
            )
        };
        let entries = trace
            .entries()
            .iter()
            .flatten()
            .take(4)
            .copied()
            .collect::<Vec<_>>();
        let terminal_level = match &translation {
            Ok(_) => entries.last().map(|entry| entry.level),
            Err(TrackedAccessError::PageFault {
                reason: PageFaultReason::NotPresent { level },
                ..
            }) => Some(*level),
            Err(TrackedAccessError::TableRead { level, .. })
            | Err(TrackedAccessError::UnsupportedPageSize { level, .. }) => Some(*level),
            _ => entries.last().map(|entry| entry.level),
        };
        let leaf_size = entries.last().and_then(|entry| match entry.level {
            PageTableLevel::Pd if entry.entry_value & (1 << 7) != 0 => Some(2 * 1024 * 1024),
            PageTableLevel::Pt => Some(4096),
            _ => None,
        });
        let (status, translated_physical, error) = match translation {
            Ok((physical, _)) => ("translated", Some(physical.get()), None),
            Err(TrackedAccessError::PageFault {
                reason: PageFaultReason::NotPresent { .. },
                ..
            }) => ("non_present", None, None),
            Err(TrackedAccessError::PageFault {
                reason: PageFaultReason::Protection { .. },
                ..
            }) => ("protection", None, None),
            Err(error) => ("read_error", None, Some(format!("{error:?}"))),
        };
        ElfMmuGatherPageWalk {
            address: Some(address),
            access: "Write",
            cpl: 0,
            origin: "Explicit",
            profile: "long64_4k_2m",
            cr0_wp,
            efer_nxe,
            status,
            entries,
            terminal_level,
            leaf_size: if matches!(status, "translated" | "protection") {
                leaf_size
            } else {
                None
            },
            translated_physical,
            error,
        }
    })
}

fn read_mmu_gather_frame(
    bus: &MachineBus,
    paging: &Long64ProbeConfig,
    instruction_rip: u64,
    context: AccessContext,
    rbp: u64,
) -> Result<(u64, u64), String> {
    #[cfg(test)]
    MMU_GATHER_DIAGNOSTIC_READS.with(|count| count.set(count.get() + 1));
    if rbp == 0 || (rbp & 7) != 0 || !is_canonical_48_bit(rbp) {
        return Err(format!("invalid RBP frame pointer 0x{rbp:016x}"));
    }
    rbp.checked_add(15)
        .ok_or_else(|| "RBP frame address overflow".to_string())?;
    let mut bytes = [0u8; 16];
    bus.read_virtual_ram_diagnostic(
        paging,
        instruction_rip,
        GuestLinearAddress::new(rbp),
        context,
        &mut bytes,
    )
    .map_err(|error| format!("RBP frame read: {error:?}"))?;
    Ok((
        u64::from_le_bytes(bytes[..8].try_into().expect("8-byte slice")),
        u64::from_le_bytes(bytes[8..].try_into().expect("8-byte slice")),
    ))
}

fn read_mmu_gather_vma_bounds(
    bus: &MachineBus,
    paging: &Long64ProbeConfig,
    instruction_rip: u64,
    context: AccessContext,
    vma: u64,
) -> Result<(u64, u64), String> {
    #[cfg(test)]
    MMU_GATHER_DIAGNOSTIC_READS.with(|count| count.set(count.get() + 1));
    if (vma & 7) != 0 || !is_canonical_48_bit(vma) {
        return Err(format!("invalid VMA pointer 0x{vma:016x}"));
    }
    vma.checked_add(15)
        .ok_or_else(|| "VMA bounds address overflow".to_string())?;
    let mut bytes = [0u8; 16];
    bus.read_virtual_ram_diagnostic(
        paging,
        instruction_rip,
        GuestLinearAddress::new(vma),
        context,
        &mut bytes,
    )
    .map_err(|error| format!("VMA bounds read: {error:?}"))?;
    Ok((
        u64::from_le_bytes(bytes[..8].try_into().expect("8-byte slice")),
        u64::from_le_bytes(bytes[8..].try_into().expect("8-byte slice")),
    ))
}

fn read_mmu_gather_raw(
    bus: &MachineBus,
    paging: &Long64ProbeConfig,
    instruction_rip: u64,
    context: AccessContext,
    tlb: u64,
) -> (Option<ElfMmuGatherRaw>, Option<String>) {
    #[cfg(test)]
    MMU_GATHER_DIAGNOSTIC_READS.with(|count| count.set(count.get() + 1));
    if (tlb & 7) != 0 || !is_canonical_48_bit(tlb) {
        return (None, Some(format!("invalid tlb pointer 0x{tlb:016x}")));
    }
    let Some(last) = tlb.checked_add(0x23) else {
        return (None, Some("tlb field address overflow".into()));
    };
    let mut bytes = [0u8; 0x24];
    if let Err(error) = bus.read_virtual_ram_diagnostic(
        paging,
        instruction_rip,
        GuestLinearAddress::new(tlb),
        context,
        &mut bytes,
    ) {
        return (
            None,
            Some(format!("tlb fields read through 0x{last:016x}: {error:?}")),
        );
    }
    let mm = u64::from_le_bytes(bytes[0..8].try_into().expect("8-byte slice"));
    if (mm & 7) != 0 || !is_canonical_48_bit(mm) {
        return (None, Some(format!("invalid mm pointer 0x{mm:016x}")));
    }
    let Some(nested_address) = mm.checked_add(0x61c) else {
        return (None, Some("mm_tlb_flush_nested address overflow".into()));
    };
    if nested_address.checked_add(3).is_none() {
        return (None, Some("mm_tlb_flush_nested field overflow".into()));
    }
    let mut nested_bytes = [0u8; 4];
    if let Err(error) = bus.read_virtual_ram_diagnostic(
        paging,
        instruction_rip,
        GuestLinearAddress::new(nested_address),
        context,
        &mut nested_bytes,
    ) {
        return (None, Some(format!("mm_tlb_flush_nested read: {error:?}")));
    }
    (
        Some(ElfMmuGatherRaw {
            tlb,
            mm,
            start: u64::from_le_bytes(bytes[0x10..0x18].try_into().expect("8-byte slice")),
            end: u64::from_le_bytes(bytes[0x18..0x20].try_into().expect("8-byte slice")),
            flags: u32::from_le_bytes(bytes[0x20..0x24].try_into().expect("4-byte slice")),
            mm_tlb_flush_nested: i32::from_le_bytes(nested_bytes),
        }),
        None,
    )
}

fn capture_elf_stack_copy_snapshot(
    state: &LongModeState,
    bus: &MachineBus,
    paging: &Long64ProbeConfig,
    context: AccessContext,
    cr3: u64,
) -> ElfStackCopySnapshot {
    use od_isa::Register::{R13, R15, Rbp, Rdi, Rdx, Rsi, Rsp};

    let mut snapshot = ElfStackCopySnapshot::empty(ElfStackCopyStatus::GateMismatch);
    let r15 = state.gpr[R15 as usize];
    let r13 = state.gpr[R13 as usize];
    let rdi = state.gpr[Rdi as usize];
    let rsi = state.gpr[Rsi as usize];
    let rdx = state.gpr[Rdx as usize];
    let cpl = state.cs_selector.map(|selector| (selector & 3) as u8);
    snapshot.rip = Some(state.rip);
    snapshot.cr3 = Some(cr3);
    snapshot.cs_selector = state.cs_selector;
    snapshot.cpl = cpl;
    snapshot.r15 = Some(r15);
    snapshot.rbp = Some(state.gpr[Rbp as usize]);
    snapshot.rsp = Some(state.gpr[Rsp as usize]);
    snapshot.r13 = Some(r13);
    snapshot.rdi = Some(rdi);
    snapshot.rsi = Some(rsi);
    snapshot.rdx = Some(rdx);
    snapshot.aligned_stack_pointer = Some(r13);
    snapshot.copy_destination = Some(rdi);
    snapshot.copy_length = Some(rdx);

    let mut instruction_bytes = [0u8; 5];
    if let Err(error) = bus.read_virtual_ram_diagnostic(
        paging,
        state.rip,
        GuestLinearAddress::new(state.rip),
        context,
        &mut instruction_bytes,
    ) {
        snapshot.status = ElfStackCopyStatus::ReadError;
        snapshot.error = Some(format!("instruction bytes: {error:?}"));
        return snapshot;
    }
    snapshot.instruction_bytes = Some(instruction_bytes.to_vec());

    if instruction_bytes != [0xe8, 0x47, 0xd9, 0x34, 0x00] {
        snapshot
            .mismatch_reasons
            .push("instruction_bytes_mismatch".into());
        return snapshot;
    }
    snapshot.return_rip = state.rip.checked_add(instruction_bytes.len() as u64);

    let bprm_p_address = match r15.checked_add(0x20) {
        Some(address) => address,
        None => {
            snapshot.status = ElfStackCopyStatus::ReadError;
            snapshot.error = Some("R15 + 0x20 overflows".to_string());
            return snapshot;
        }
    };
    snapshot.bprm_p_address = Some(bprm_p_address);
    let mut pointer_bytes = [0u8; 8];
    if let Err(error) = bus.read_virtual_ram_diagnostic(
        paging,
        state.rip,
        GuestLinearAddress::new(bprm_p_address),
        context,
        &mut pointer_bytes,
    ) {
        snapshot.status = ElfStackCopyStatus::ReadError;
        snapshot.error = Some(format!("bprm_p read: {error:?}"));
        return snapshot;
    }
    let bprm_p = u64::from_le_bytes(pointer_bytes);
    snapshot.bprm_p = Some(bprm_p);

    let expected_destination = r13.checked_sub(7);
    if cpl != Some(0) {
        snapshot.mismatch_reasons.push("cpl_not_zero".into());
    }
    if rdx != 7 {
        snapshot.mismatch_reasons.push("rdx_not_7".into());
    }
    if rsi != 0xffff_ffff_82fd_eb8f {
        snapshot.mismatch_reasons.push("rsi_mismatch".into());
    }
    if r13 & 0xf != 0 {
        snapshot
            .mismatch_reasons
            .push("r13_not_16_byte_aligned".into());
    }
    if expected_destination != Some(rdi) {
        snapshot.mismatch_reasons.push("rdi_not_r13_minus_7".into());
    }
    if bprm_p < r13 || bprm_p - r13 > 8206 {
        snapshot
            .mismatch_reasons
            .push("bprm_p_outside_stack_window".into());
    }
    if !snapshot.mismatch_reasons.is_empty() {
        return snapshot;
    }

    let mut platform_string = [0u8; 7];
    if let Err(error) = bus.read_virtual_ram_diagnostic(
        paging,
        state.rip,
        GuestLinearAddress::new(rsi),
        context,
        &mut platform_string,
    ) {
        snapshot.status = ElfStackCopyStatus::ReadError;
        snapshot.error = Some(format!("platform string read: {error:?}"));
        return snapshot;
    }
    snapshot.platform_string_matches = Some(platform_string == *b"x86_64\0");
    if snapshot.platform_string_matches != Some(true) {
        snapshot
            .mismatch_reasons
            .push("platform_string_mismatch".into());
        snapshot.status = ElfStackCopyStatus::GateMismatch;
        return snapshot;
    }

    let probes = [
        ("stack_input", bprm_p, AccessKind::Read),
        ("copy_source", rsi, AccessKind::Read),
        ("copy_destination", rdi, AccessKind::Write),
    ];
    for (name, address, access) in probes {
        if snapshot.probes.len() == LINUX_ELF_STACK_COPY_PROBE_CAPACITY {
            break;
        }
        let (translation, trace) = {
            let ram = bus.physical_memory();
            translate_long64_2m_ram_read_only_with_trace(
                ram,
                paging,
                state.rip,
                GuestLinearAddress::new(address),
                context,
                access,
            )
        };
        let entries = trace
            .entries()
            .iter()
            .flatten()
            .take(LINUX_ELF_STACK_COPY_WALK_ENTRY_CAPACITY)
            .copied()
            .collect();
        let (outcome, translated_physical, error) = match translation {
            Ok((physical, _)) => ("translated", Some(physical.get()), None),
            Err(TrackedAccessError::PageFault { record, reason }) => (
                "page_fault",
                None,
                Some(format!("record={record:?}; reason={reason:?}")),
            ),
            Err(error) => ("error", None, Some(format!("{error:?}"))),
        };
        snapshot.probes.push(ElfStackCopyProbe {
            name: name.to_string(),
            address,
            page_base: address & !0xfff,
            access: if access == AccessKind::Write {
                "Write"
            } else {
                "Read"
            }
            .into(),
            cpl: cpl.unwrap_or(0),
            entries,
            outcome: outcome.into(),
            translated_physical,
            error,
        });
    }
    snapshot.status = ElfStackCopyStatus::Captured;
    snapshot
}

fn capture_elf_stack_alignment_point(
    state: &LongModeState,
    bus: &MachineBus,
    paging: &Long64ProbeConfig,
    context: AccessContext,
    cr3: u64,
    before_call: bool,
) -> ElfStackAlignmentPoint {
    use od_isa::Register::{R15, Rax, Rbp, Rdi, Rsp};

    let mut bytes = [0u8; 5];
    let (instruction_bytes, error) = match bus.read_virtual_ram_diagnostic(
        paging,
        state.rip,
        GuestLinearAddress::new(state.rip),
        context,
        &mut bytes,
    ) {
        Ok(()) => (Some(bytes.to_vec()), None),
        Err(error) => (None, Some(format!("instruction bytes: {error:?}"))),
    };
    ElfStackAlignmentPoint {
        rip: state.rip,
        retired_instructions_before_current: 0,
        cr3,
        cs_selector: state.cs_selector,
        cpl: state.cs_selector.map(|selector| (selector & 3) as u8),
        r15: state.gpr[R15 as usize],
        rbp: state.gpr[Rbp as usize],
        rsp: state.gpr[Rsp as usize],
        pointer: state.gpr[(if before_call { Rdi } else { Rax }) as usize],
        instruction_bytes,
        error,
    }
}

/// Whether the instruction at RIP is exactly `IRETQ` (`48 CF`). Same read-only walk as
/// [`cpuid_leaf_at_rip`] (no accessed/dirty bit, RAM only); an untranslatable byte is `false`.
/// Used for the RF rule of S4-38L5c: `IRETQ` loads RF from its own frame, so the board must not
/// clear it afterwards.
fn iretq_at_rip(
    state: &LongModeState,
    memory: &crate::PhysicalMemory,
    paging: &Long64ProbeConfig,
    context: AccessContext,
    cache: Option<&FetchTranslationCache>,
) -> bool {
    let byte_at = |offset: u64| -> Option<u8> {
        let linear = state.rip.wrapping_add(offset);
        if let Some(byte) = cache
            .and_then(|cache| cache.lookup(linear, paging, context))
            .and_then(|frame| memory.read_ram_frame_u8(frame, linear))
        {
            return Some(byte);
        }
        let physical = crate::page_walk::translate_long64_2m_ram_read_only(
            memory,
            paging,
            state.rip,
            GuestLinearAddress::new(state.rip.wrapping_add(offset)),
            context,
            od_isa::memory::AccessKind::Fetch,
        )
        .ok()?;
        memory.load_u8(physical).ok()
    };
    byte_at(0) == Some(0x48) && byte_at(1) == Some(0xCF)
}

/// S4-14h: whether the instruction at RIP has the shape of `INVLPG m`: an optional `FS`/`GS`
/// override (`64`/`65`), an optional REX byte without W (`40`-`47`), `0F 01`, and a ModRM with
/// `reg = 7` and `mod != 3` (the register forms of `0F 01 /7` are `SWAPGS` and `RDTSCP`). Same
/// read-only walk as [`iretq_at_rip`] (no accessed/dirty bit, RAM only); an untranslatable byte is
/// `false`. It only decides whether the cache is flushed after the instruction retires; the
/// instruction itself is decoded and executed by the set, so a shape the set does not accept
/// retires nothing and flushes nothing.
pub(crate) fn invlpg_at_rip(
    state: &LongModeState,
    memory: &crate::PhysicalMemory,
    paging: &Long64ProbeConfig,
    context: AccessContext,
    cache: Option<&FetchTranslationCache>,
) -> bool {
    let byte_at = |offset: u64| -> Option<u8> {
        let linear = state.rip.wrapping_add(offset);
        if let Some(byte) = cache
            .and_then(|cache| cache.lookup(linear, paging, context))
            .and_then(|frame| memory.read_ram_frame_u8(frame, linear))
        {
            return Some(byte);
        }
        let physical = crate::page_walk::translate_long64_2m_ram_read_only(
            memory,
            paging,
            state.rip,
            GuestLinearAddress::new(state.rip.wrapping_add(offset)),
            context,
            od_isa::memory::AccessKind::Fetch,
        )
        .ok()?;
        memory.load_u8(physical).ok()
    };
    let mut offset = 0u64;
    let mut first = byte_at(offset);
    if matches!(first, Some(0x64 | 0x65)) {
        offset += 1;
        first = byte_at(offset);
    }
    if matches!(first, Some(0x40..=0x47)) {
        offset += 1;
        first = byte_at(offset);
    }
    if first != Some(0x0F) || byte_at(offset + 1) != Some(0x01) {
        return false;
    }
    byte_at(offset + 2).is_some_and(|modrm| (modrm >> 3) & 7 == 7 && modrm >> 6 != 3)
}

/// S4-39f: whether the instruction at `rip` is exactly `SYSRETQ` (`48 0F 07`). The same read-only
/// walk as [`iretq_at_rip`] (no accessed/dirty bit, RAM only, `rip` is the instruction RIP); an
/// untranslatable byte is `false`. Called only after a committed return to CPL3, to tell the
/// SYSRET of a system call from an `IRETQ`.
fn sysretq_at(
    rip: u64,
    memory: &crate::PhysicalMemory,
    paging: &Long64ProbeConfig,
    context: AccessContext,
    cache: Option<&FetchTranslationCache>,
) -> bool {
    let byte_at = |offset: u64| -> Option<u8> {
        let linear = rip.wrapping_add(offset);
        if let Some(byte) = cache
            .and_then(|cache| cache.lookup(linear, paging, context))
            .and_then(|frame| memory.read_ram_frame_u8(frame, linear))
        {
            return Some(byte);
        }
        let physical = crate::page_walk::translate_long64_2m_ram_read_only(
            memory,
            paging,
            rip,
            GuestLinearAddress::new(linear),
            context,
            od_isa::memory::AccessKind::Fetch,
        )
        .ok()?;
        memory.load_u8(physical).ok()
    };
    byte_at(0) == Some(0x48) && byte_at(1) == Some(0x0F) && byte_at(2) == Some(0x07)
}

/// S4-39f: what the board loop saw of a system call: the `SYSCALL` that entered the kernel (RAX
/// is the number, RCX the return address) and the `SYSRETQ` that returned (RAX the result).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SyscallEvent {
    Enter {
        nr: u64,
        caller: SyscallUserContext,
    },
    Exit {
        rax: u64,
        caller: SyscallUserContext,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SyscallUserContext {
    pub(crate) rip: u64,
    pub(crate) rsp: u64,
    pub(crate) cr3: u64,
}

/// The events one call of the set loop keeps at most (the oldest are dropped; the board journal
/// is a ring too).
const SYSCALL_EVENTS_PER_CALL: usize = 64;

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct PagedCpl0SetReport {
    pub(crate) exit: PagedCpl0SetExit,
    pub(crate) instructions_executed: u64,
    pub(crate) rflags_defined: u64,
    pub(crate) page_fault: Option<PageFaultRecord>,
    pub(crate) memory_error: Option<TrackedAccessError>,
    /// Every retired `MOV CR3`, in order, including one whose walker rebuild failed
    /// (`Cr3EffectMismatch`, `Cr3SnapshotRejected`: the failing event is the last entry).
    pub(crate) translation_context_changes: Vec<TranslationContextChange>,
    /// CR3 after the last retired `MOV CR3` (the initial root when there was none). On
    /// `Cr3EffectMismatch` and `Cr3SnapshotRejected` the instruction retired, so this is
    /// already the new CR3 although the walker was not rebuilt for it.
    pub(crate) cr3: u64,
    /// The walker's EFER at the end of the call: the `efer` handed in, with `NXE` as a retired
    /// `WRMSR IA32_EFER` of this call left it (S2-25 NXE slice).
    pub(crate) efer: u64,
    /// `#PF` and `#GP(0)` exceptions the loop delivered through the guest IDT. A delivery is not
    /// a retired instruction: `instructions_executed` does not count it.
    pub(crate) delivered_exceptions: u64,
    /// Successfully delivered guest `#PF` exceptions in this call.
    pub(crate) delivered_page_faults: u64,
    /// Successfully delivered guest `#GP(0)` exceptions in this call.
    pub(crate) delivered_general_protection_faults: u64,
    pub(crate) pit_written: bool,
    pub(crate) port_accessed: bool,
    pub(crate) mmio_written: bool,
    /// S4-13: the counters of the call's fetch translation cache (`None` without one).
    pub(crate) fetch_cache_stats: Option<FetchCacheStats>,
    /// S4-39c: instructions retired through the CPL3 entry (`run_with_cpl3_profile_set`) in this
    /// call; part of `instructions_executed`. Always 0 unless the set is `with_user_mode`.
    pub(crate) cpl3_retired: u64,
    /// S4-39f: the `SYSCALL` / `SYSRETQ` pairs of this call, in order (only with `with_user_mode`).
    pub(crate) syscall_events: Vec<SyscallEvent>,
    /// Last instruction boundary reached when opt-in execution observations were enabled.
    pub(crate) last_instruction_rip: Option<u64>,
    /// S4-13b: the tracked page walks of the instruction adapter in this call (measurement only,
    /// see `fetch_cache_report`). They differ with and without the fetch cache by design.
    pub(crate) walk_counts: LinuxWalkCounts,
    /// S4-13b: checkpoint C5 was already recorded when this call started.
    pub(crate) c5_at_call_start: bool,
    /// S4-13b: the counters at the boundary where this call recorded checkpoint C5.
    pub(crate) c5_split: Option<C5Split>,
}

/// Run the CPL0 profile set through the RAM-only 4 KiB/2 MiB tracked walker,
/// one instruction per set call (S2-45f).
///
/// The adapter is built per instruction because it pins the instruction RIP
/// (saved RIP of a `#PF`, the single code page of the large-RAM fetch window).
/// The walker snapshot is rebuilt from the CPU control registers on every
/// `TranslationContextChange(Cr3Write)`: the set stops right after the retired
/// `MOV CR3, RAX` and the next fetch already walks the new CR3. `cr3` is the
/// initial walker root; with a CPU-owned CR3 (`with_cpu_owned_cr3`) it must
/// equal `state.cr3`. The memory profile is the one of
/// [`step_paged_mov_2m_ram`]: qword operands inside one 4 KiB page and no MMIO.
/// `IN`/`OUT` DX reach the port half of `bus` only for a set built with `with_pio_u8`
/// (S4-38L3); the memory half and the port half are split off the same bus for every
/// instruction, and without that member the forms stay `Unsupported` and no port is touched.
///
/// Stop is polled and the budget is checked before each instruction, like the
/// paged probe quantum.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_cpl0_set_paged_2m_ram(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    instruction_budget: u64,
    rflags_defined: u64,
    set: &Cpl0ProfileSet<'_>,
    rdtsc: Option<&mut dyn FnMut() -> u64>,
    stop: impl FnMut() -> bool,
    committed_write: impl FnMut(GuestPhysicalRange),
) -> PagedCpl0SetReport {
    run_cpl0_set_paged_2m_ram_with_cpuid_boundary(
        state,
        bus,
        cr3,
        efer,
        context,
        instruction_budget,
        rflags_defined,
        set,
        rdtsc,
        stop,
        committed_write,
        false,
        false,
        None,
    )
}

/// `run_cpl0_set_paged_2m_ram` with the CPUID boundary of S4-38L4 when `cpuid_boundary` is
/// set: before each instruction (after the stop and budget checks, before any set call) the
/// bytes at RIP are peeked, and a `CPUID` whose EAX is a divergence leaf
/// ([`is_cpuid_divergence_leaf`]) ends the run as `CpuidDivergence` without executing it. This
/// is an honest stop, not an answer: nothing is substituted for the leaf and the state is left
/// exactly as before the instruction. Other leaves go to the set as usual.
///
/// With `irq` (S4-38L8b) each instruction boundary first asks the board source for a pending
/// external vector, after the stop and budget checks and before the CPUID check; see
/// [`deliver_pending_irq`]. Without it, or with a source that never has a vector, the loop is
/// what it was.
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(crate) fn run_cpl0_set_paged_2m_ram_with_cpuid_boundary(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    instruction_budget: u64,
    rflags_defined: u64,
    set: &Cpl0ProfileSet<'_>,
    rdtsc: Option<&mut dyn FnMut() -> u64>,
    stop: impl FnMut() -> bool,
    committed_write: impl FnMut(GuestPhysicalRange),
    cpuid_boundary: bool,
    fetch_cache: bool,
    irq: Option<IrqBoundaryHook<'_>>,
) -> PagedCpl0SetReport {
    run_cpl0_set_paged_2m_ram_with_policy(
        state,
        bus,
        cr3,
        efer,
        context,
        instruction_budget,
        rflags_defined,
        set,
        rdtsc,
        stop,
        committed_write,
        cpuid_boundary,
        fetch_cache.then_some(FetchCacheInvalidation::WriteWatch),
        irq,
    )
}

/// [`run_cpl0_set_paged_2m_ram_with_cpuid_boundary`] with the invalidation policy of the fetch
/// cache chosen by the caller (S4-14h): `None` is no cache, `Some(WriteWatch)` is the S4-13 cache.
/// With `Some(GuestOnly)` a write to a page table drops nothing; the loop flushes the cache after
/// a retired `INVLPG` (peeked before the step like `IRETQ`, read-only), after a retired
/// `MOV CR3` (as in every policy), when CR0 or CR4 differ after the step, and at every `#PF`
/// delivered to the guest IDT. A step that retires nothing and delivers no `#PF` (a fault that
/// stops the run, a rejection, a `#GP` delivery) keeps the cache. The cache still lives in this
/// call only.
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(crate) fn run_cpl0_set_paged_2m_ram_with_policy(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    instruction_budget: u64,
    rflags_defined: u64,
    set: &Cpl0ProfileSet<'_>,
    rdtsc: Option<&mut dyn FnMut() -> u64>,
    stop: impl FnMut() -> bool,
    committed_write: impl FnMut(GuestPhysicalRange),
    cpuid_boundary: bool,
    fetch_cache: Option<FetchCacheInvalidation>,
    irq: Option<IrqBoundaryHook<'_>>,
) -> PagedCpl0SetReport {
    run_cpl0_set_paged_2m_ram_observed(
        state,
        bus,
        cr3,
        efer,
        context,
        instruction_budget,
        rflags_defined,
        set,
        rdtsc,
        stop,
        committed_write,
        cpuid_boundary,
        fetch_cache,
        irq,
        None,
        None,
    )
}

/// [`run_cpl0_set_paged_2m_ram_with_policy`] with the opt-in checkpoint recorder of S4-38L4i.
/// With `checkpoints` each instruction boundary (after the stop, budget and interrupt checks,
/// before the CPUID boundary and the step) is shown to the recorder, which only observes: it gets
/// the RIP, the CPL, the CR3 and the IDTR of the boundary and the EAX of a `CPUID` at RIP, and
/// is told of a retired `MOV CR3` and of a delivered `#PF`. The CPUID peek is shared with the
/// CPUID boundary (made once per boundary when either is on). The peek reads no accessed/dirty
/// bit and writes nothing to the guest, but with the fetch cache it calls
/// `FetchTranslationCache::lookup`, which moves the cache's `recent` entry and its hit/miss
/// statistics. They are visible only in `PagedCpl0SetReport::fetch_cache_stats` (read by tests
/// of this module, not by the board), so `LinuxSetRunReport` and the run do not change (the
/// cache only speeds up translations that give the same answer). With `None` the
/// loop is the one of [`run_cpl0_set_paged_2m_ram_with_policy`], instruction for instruction.
///
/// With `execution_observations` (S5-07, opt-in diagnostics of the stage 5 line) the loop also
/// feeds the collector with delivered and faulting `#PF` contexts, page walks, fault-time frame
/// chains and the selected post-`IRETQ` boundary; the collector only observes. Checkpoints and
/// execution observations are independent: either, both or none may be given.
///
/// The fetch cache (if any) is created here and lives in this call only; the loop itself is
/// [`run_cpl0_set_paged_2m_ram_with_cache`].
#[allow(clippy::too_many_arguments)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn run_cpl0_set_paged_2m_ram_observed(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    instruction_budget: u64,
    rflags_defined: u64,
    set: &Cpl0ProfileSet<'_>,
    rdtsc: Option<&mut dyn FnMut() -> u64>,
    stop: impl FnMut() -> bool,
    committed_write: impl FnMut(GuestPhysicalRange),
    cpuid_boundary: bool,
    fetch_cache: Option<FetchCacheInvalidation>,
    irq: Option<IrqBoundaryHook<'_>>,
    checkpoints: Option<&mut CheckpointRecorder>,
    execution_observations: Option<&mut LinuxExecutionObservationCollector>,
) -> PagedCpl0SetReport {
    run_cpl0_set_paged_2m_ram_observed_with_sweep(
        state,
        bus,
        cr3,
        efer,
        context,
        instruction_budget,
        rflags_defined,
        set,
        rdtsc,
        stop,
        committed_write,
        cpuid_boundary,
        fetch_cache,
        irq,
        checkpoints,
        execution_observations,
        None,
        FetchCacheTag::Page4K,
        None,
    )
}

/// [`run_cpl0_set_paged_2m_ram_observed`] with the opt-in shadow caches of S4-13c (diagnostics
/// only). With `sweep` and a cache (`fetch_cache` is `Some`) the call's cache forwards every event
/// to the shadows, and the loop tells them of the guest invalidations a `WriteWatch` cache does not
/// act on: a retired `INVLPG` (peeked through the read-only walk WITHOUT the cache, so the counters
/// of the cache do not move), a `#PF` delivered to the guest and a change of CR0/CR4. Nothing the
/// loop reads comes back from the shadows: the run is the one of
/// [`run_cpl0_set_paged_2m_ram_observed`]. Without a cache `sweep` is ignored. `tag` is what one
/// entry of the call's cache covers (S4-13d): `Page4K` is the default of every other caller.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_cpl0_set_paged_2m_ram_observed_with_sweep(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    instruction_budget: u64,
    rflags_defined: u64,
    set: &Cpl0ProfileSet<'_>,
    rdtsc: Option<&mut dyn FnMut() -> u64>,
    stop: impl FnMut() -> bool,
    committed_write: impl FnMut(GuestPhysicalRange),
    cpuid_boundary: bool,
    fetch_cache: Option<FetchCacheInvalidation>,
    irq: Option<IrqBoundaryHook<'_>>,
    checkpoints: Option<&mut CheckpointRecorder>,
    execution_observations: Option<&mut LinuxExecutionObservationCollector>,
    sweep: Option<&Rc<FetchCacheSweep>>,
    tag: FetchCacheTag,
    decoded: Option<&DecodedInstructionCache>,
) -> PagedCpl0SetReport {
    // S4-13: the translation cache of the instruction fetch lives in this call only.
    let cache = fetch_cache.map(|invalidation| {
        let cache = FetchTranslationCache::with_invalidation(invalidation).with_tag(tag);
        match sweep {
            Some(sweep) => cache.with_sweep(Rc::clone(sweep)),
            None => cache,
        }
    });
    run_cpl0_set_paged_2m_ram_with_cache(
        state,
        bus,
        crate::vcpu::BOOT_VCPU_INDEX,
        cr3,
        efer,
        context,
        instruction_budget,
        rflags_defined,
        set,
        rdtsc,
        stop,
        committed_write,
        cpuid_boundary,
        cache.as_ref(),
        irq,
        checkpoints,
        execution_observations,
        decoded,
    )
}

/// The set loop of [`run_cpl0_set_paged_2m_ram_observed`] over a fetch cache owned by the caller
/// (S4-14i). `None` is no cache. A cache built with [`FetchTranslationCache::with_invalidation`]
/// and dropped after the call is what `_observed` does. A cache the caller passes again to the next
/// call of the same machine session (typically one built with [`FetchTranslationCache::persistent`])
/// keeps its entries, flushed only by the events of its policy, which happen inside the loop
/// (`MOV CR3`, and for `GuestOnly` also `INVLPG`, CR0/CR4 changes and a delivered `#PF`; for
/// `WriteWatch` a write to a watched table frame), and by the host events. Every cache handed to
/// this function is sealed when the call ends and checked when it starts, whatever its
/// constructor: the check drops every entry (`host_flushes`) when the translation context handed
/// over (CR0/CR3/CR4/EFER, mode, privilege) differs from the one the seal holds, or when the page
/// generation of a table frame it watches differs (a host or device write between calls);
/// `flush_for_host_event` and `reset` are the explicit forms. For a cache used by one call only
/// the check has nothing to compare. `PagedCpl0SetReport::fetch_cache_stats` of a cache that
/// serves several calls are cumulative over all of them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_cpl0_set_paged_2m_ram_with_cache(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    vcpu: usize,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    instruction_budget: u64,
    rflags_defined: u64,
    set: &Cpl0ProfileSet<'_>,
    rdtsc: Option<&mut dyn FnMut() -> u64>,
    stop: impl FnMut() -> bool,
    committed_write: impl FnMut(GuestPhysicalRange),
    cpuid_boundary: bool,
    cache: Option<&FetchTranslationCache>,
    irq: Option<IrqBoundaryHook<'_>>,
    checkpoints: Option<&mut CheckpointRecorder>,
    execution_observations: Option<&mut LinuxExecutionObservationCollector>,
    decoded: Option<&DecodedInstructionCache>,
) -> PagedCpl0SetReport {
    run_cpl0_set_loop(
        state,
        LoopBus::Exclusive(bus),
        None,
        vcpu,
        cr3,
        efer,
        context,
        instruction_budget,
        rflags_defined,
        set,
        rdtsc,
        stop,
        committed_write,
        cpuid_boundary,
        cache,
        irq,
        checkpoints,
        execution_observations,
        ExceptionDeliveryMode::Quantum,
        false,
        None,
        decoded,
    )
}

/// What the set loop reaches the machine through (S6-34). `Exclusive`: the caller owns the bus
/// (`&mut MachineBus`, every interleaved path): the device table is taken without a lock.
/// `Shared`: a vCPU thread holds only `&MachineBus`: RAM is the shared reference and every device
/// access takes the one device lock of the bus for that access alone.
pub(crate) enum LoopBus<'a> {
    Exclusive(&'a mut MachineBus),
    Shared(&'a MachineBus),
}

impl LoopBus<'_> {
    fn machine(&self) -> &MachineBus {
        match self {
            Self::Exclusive(bus) => bus,
            Self::Shared(bus) => bus,
        }
    }
    fn ram(&self) -> &crate::PhysicalMemory {
        match self {
            Self::Exclusive(bus) => bus.physical_memory(),
            Self::Shared(bus) => bus.physical_memory(),
        }
    }

    fn split_for_set_step_of_vcpu<'b>(
        &'b mut self,
        vcpu: usize,
        gate: Option<&'b dyn crate::bus::SplitLockGate>,
    ) -> (
        &'b crate::PhysicalMemory,
        crate::bus::MachineBusSetStepIo<'b>,
    ) {
        let (ram, io) = match self {
            Self::Exclusive(bus) => bus.split_for_set_step_of_vcpu(vcpu),
            Self::Shared(bus) => bus.split_for_set_step_shared(vcpu),
        };
        (ram, io.with_split_gate(gate))
    }
}

pub(crate) enum ExceptionDeliveryMode<'a> {
    /// The caller owns the machine and uses a fresh limit for this quantum.
    Quantum,
    /// The vCPU keeps the streak across scheduler boundaries.
    Persistent(&'a mut u64),
    /// One step in a host loop that accounts for the streak and observes handler entry.
    HostStep(u64),
}

/// The set loop of one vCPU over a [`LoopBus`]: the body of
/// [`run_cpl0_set_paged_2m_ram_with_cache`] (see its documentation) with the machine given as
/// an enum and, for a vCPU thread of the parallel mode, the gate of the split `LOCK`
/// (`split_gate`, S6-34 step 8; `None` keeps the refusal of S6-31/S6-32).
/// The host passes the number of consecutive delivered faults and resumes at its own
/// IRQ boundary after each new delivery. Other callers continue within this loop.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_cpl0_set_loop(
    state: &mut LongModeState,
    mut bus: LoopBus<'_>,
    split_gate: Option<&dyn crate::bus::SplitLockGate>,
    vcpu: usize,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    instruction_budget: u64,
    rflags_defined: u64,
    set: &Cpl0ProfileSet<'_>,
    rdtsc: Option<&mut dyn FnMut() -> u64>,
    stop: impl FnMut() -> bool,
    committed_write: impl FnMut(GuestPhysicalRange),
    cpuid_boundary: bool,
    cache: Option<&FetchTranslationCache>,
    irq: Option<IrqBoundaryHook<'_>>,
    checkpoints: Option<&mut CheckpointRecorder>,
    execution_observations: Option<&mut LinuxExecutionObservationCollector>,
    exception_delivery: ExceptionDeliveryMode<'_>,
    stop_after_mmio: bool,
    on_syscall: Option<&mut dyn FnMut(SyscallEvent)>,
    decoded: Option<&DecodedInstructionCache>,
) -> PagedCpl0SetReport {
    let trace = execution_observations
        .as_deref()
        .is_some_and(LinuxExecutionObservationCollector::wants_instruction_path_trace);
    let elf = execution_observations
        .as_deref()
        .is_some_and(LinuxExecutionObservationCollector::needs_elf_stack_copy_path);
    // PERF-20: the histogram of retired operations has its own instantiation; every other run
    // keeps the loop without it.
    if checkpoints
        .as_deref()
        .is_some_and(CheckpointRecorder::has_op_histogram)
    {
        // The histogram instantiation has neither the instruction-path trace nor the ELF stack
        // copy path: a run that asks for them together with the histogram would lose them
        // silently. The runner does not combine them (the histogram is a separate diagnostic run).
        debug_assert!(
            !trace && !elf,
            "the op histogram cannot be combined with the instruction-path trace or the ELF stack copy path"
        );
        return run_cpl0_set_loop_impl::<false, false, true>(
            state,
            bus,
            split_gate,
            vcpu,
            cr3,
            efer,
            context,
            instruction_budget,
            rflags_defined,
            set,
            rdtsc,
            stop,
            committed_write,
            cpuid_boundary,
            cache,
            irq,
            checkpoints,
            execution_observations,
            exception_delivery,
            stop_after_mmio,
            on_syscall,
            decoded,
        );
    }
    match (trace, elf) {
        (true, true) => run_cpl0_set_loop_impl::<true, true, false>(
            state,
            bus,
            split_gate,
            vcpu,
            cr3,
            efer,
            context,
            instruction_budget,
            rflags_defined,
            set,
            rdtsc,
            stop,
            committed_write,
            cpuid_boundary,
            cache,
            irq,
            checkpoints,
            execution_observations,
            exception_delivery,
            stop_after_mmio,
            on_syscall,
            decoded,
        ),
        (true, false) => run_cpl0_set_loop_impl::<true, false, false>(
            state,
            bus,
            split_gate,
            vcpu,
            cr3,
            efer,
            context,
            instruction_budget,
            rflags_defined,
            set,
            rdtsc,
            stop,
            committed_write,
            cpuid_boundary,
            cache,
            irq,
            checkpoints,
            execution_observations,
            exception_delivery,
            stop_after_mmio,
            on_syscall,
            decoded,
        ),
        (false, true) => run_cpl0_set_loop_impl::<false, true, false>(
            state,
            bus,
            split_gate,
            vcpu,
            cr3,
            efer,
            context,
            instruction_budget,
            rflags_defined,
            set,
            rdtsc,
            stop,
            committed_write,
            cpuid_boundary,
            cache,
            irq,
            checkpoints,
            execution_observations,
            exception_delivery,
            stop_after_mmio,
            on_syscall,
            decoded,
        ),
        (false, false) => run_cpl0_set_loop_impl::<false, false, false>(
            state,
            bus,
            split_gate,
            vcpu,
            cr3,
            efer,
            context,
            instruction_budget,
            rflags_defined,
            set,
            rdtsc,
            stop,
            committed_write,
            cpuid_boundary,
            cache,
            irq,
            checkpoints,
            execution_observations,
            exception_delivery,
            stop_after_mmio,
            on_syscall,
            decoded,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_cpl0_set_loop_impl<
    const TRACE_ENABLED: bool,
    const ELF_COPY_ENABLED: bool,
    const HISTOGRAM_ENABLED: bool,
>(
    state: &mut LongModeState,
    mut bus: LoopBus<'_>,
    split_gate: Option<&dyn crate::bus::SplitLockGate>,
    vcpu: usize,
    cr3: u64,
    mut efer: u64,
    context: AccessContext,
    instruction_budget: u64,
    rflags_defined: u64,
    set: &Cpl0ProfileSet<'_>,
    mut rdtsc: Option<&mut dyn FnMut() -> u64>,
    mut stop: impl FnMut() -> bool,
    mut committed_write: impl FnMut(GuestPhysicalRange),
    cpuid_boundary: bool,
    cache: Option<&FetchTranslationCache>,
    mut irq: Option<IrqBoundaryHook<'_>>,
    mut checkpoints: Option<&mut CheckpointRecorder>,
    mut execution_observations: Option<&mut LinuxExecutionObservationCollector>,
    exception_delivery: ExceptionDeliveryMode<'_>,
    stop_after_mmio: bool,
    mut on_syscall: Option<&mut dyn FnMut(SyscallEvent)>,
    decoded: Option<&DecodedInstructionCache>,
) -> PagedCpl0SetReport {
    // ADR-0017, section 3: a traced run keeps the general path (PERF-23 fast path off).
    let set_without_fast_path;
    let set = if TRACE_ENABLED {
        set_without_fast_path = set.without_fast_path();
        &set_without_fast_path
    } else {
        set
    };
    let context = AccessContext {
        origin: od_isa::memory::AccessOrigin::Explicit,
        ..context
    };
    // S4-14i: any cache first learns of a changed translation context and of writes made outside
    // the loop since its last call (one-shot caches have no seal yet, so nothing changes for them).
    if let Some(cache) = cache {
        cache.revalidate(continuation_key(state, cr3, efer, context), &|page| {
            bus.ram()
                .code_page_generation(GuestPhysicalAddress::new(page << 12))
                .ok()
        });
    }
    // PERF-10: the decoded-instruction cache serves only an ordinary run over a write-watch fetch
    // cache. A diagnostic (a sweep, a trace, an execution observation) keeps the byte-per-fetch
    // decoding, whose counters and observations the cache would change.
    let decoded = decoded.filter(|_| {
        !TRACE_ENABLED
            && !ELF_COPY_ENABLED
            && execution_observations.is_none()
            && cache.is_some_and(|cache| {
                cache.invalidation() == FetchCacheInvalidation::WriteWatch && !cache.sweep_active()
            })
    });
    if let Some(decoded) = decoded {
        decoded.bind_memory(std::ptr::from_ref(bus.ram()) as usize);
    }
    // Every committed RAM write of the loop (guest stores, walker A/D commits, exception frames)
    // passes through `watch`, which lets the cache drop entries whose tables were written.
    let guest_only =
        cache.is_some_and(|cache| cache.invalidation() == FetchCacheInvalidation::GuestOnly);
    let mut watch = |range: GuestPhysicalRange| {
        if let Some(cache) = cache {
            cache.note_write(range);
        }
        committed_write(range);
    };
    let mut paging = match Long64ProbeConfig::from_cpu_state(state, cr3, efer) {
        Ok(paging) => paging,
        Err(error) => {
            return PagedCpl0SetReport {
                exit: PagedCpl0SetExit::Configuration(error),
                instructions_executed: 0,
                rflags_defined,
                page_fault: None,
                memory_error: None,
                translation_context_changes: Vec::new(),
                cr3,
                efer,
                delivered_exceptions: 0,
                delivered_page_faults: 0,
                delivered_general_protection_faults: 0,
                pit_written: false,
                port_accessed: false,
                mmio_written: false,
                fetch_cache_stats: None,
                cpl3_retired: 0,
                syscall_events: Vec::new(),
                last_instruction_rip: None,
                walk_counts: LinuxWalkCounts::default(),
                c5_at_call_start: false,
                c5_split: None,
            };
        }
    };
    // S4-13b: measurement only; the loop never reads these counters back.
    let walks = WalkCounter::default();
    let c5_at_call_start = checkpoints
        .as_deref()
        .is_some_and(|recorder| recorder.is_recorded(C5_CHECKPOINT));
    let mut c5_split: Option<C5Split> = None;
    let mut syscall_events = std::collections::VecDeque::new();
    let mut instructions_executed = 0u64;
    let mut cpl3_retired = 0u64;
    // S4-39c: under `with_user_mode` a committed `IRETQ` to CPL3 does not end the run: the loop
    // continues with the CPL3 entry on the same state. Without the member there is no CPL3 state.
    let user_mode = set.has_user_mode();
    let mut current_rflags_defined = rflags_defined;
    let mut changes = Vec::new();
    // CR3 of the last retired MOV CR3 (the initial root before any), independent of
    // whether the walker could be rebuilt for it.
    let mut reported_cr3 = cr3;
    let mut delivered_exceptions = 0u64;
    let mut delivered_page_faults = 0u64;
    let mut delivered_general_protection_faults = 0u64;
    // Deliveries since the last retired instruction (see `MAX_DELIVERIES_WITHOUT_RETIREMENT`).
    let (mut local_faults, yield_after_delivery) = match &exception_delivery {
        ExceptionDeliveryMode::HostStep(faults) => (*faults, true),
        _ => (0, false),
    };
    let deliveries_without_retirement = match exception_delivery {
        ExceptionDeliveryMode::Persistent(faults) => faults,
        _ => &mut local_faults,
    };
    let mut last_instruction_rip = None;
    // S4-38L8b: a retired guest write to a PIT port ends the quantum (only when the source
    // depends on the PIT), so that the board asks the source for its deadline again.
    let pit_write_ends_quantum = irq
        .as_ref()
        .is_some_and(|hook| hook.source.pit_write_ends_quantum());
    let mut pit_write_pending = false;
    let mut pit_written = false;
    let mut port_accessed = false;
    let mut mmio_written = false;
    let (exit, page_fault, memory_error) = loop {
        if stop() || (stop_after_mmio && mmio_written) {
            break (PagedCpl0SetExit::Run(RunExit::StopRequested), None, None);
        }
        if pit_write_pending {
            if let Some(hook) = irq.as_mut() {
                hook.counters.pit_write_quantum_ends += 1;
            }
            break (PagedCpl0SetExit::Run(RunExit::StopRequested), None, None);
        }
        if instructions_executed >= instruction_budget {
            break (
                PagedCpl0SetExit::Run(RunExit::InstructionBudgetExhausted),
                None,
                None,
            );
        }
        // S4-39c: the entry follows the CPL of the state (the RPL of the CS selector). RPL 3 is
        // the CPL3 entry under `with_user_mode`; without it a CS with RPL 3 is refused before
        // anything runs, so user code never executes through the CPL0 entry (review S4-39b, 3).
        // Everything below (IRQ delivery, peeks, fault delivery) uses the privilege of the entry.
        // S4-39d/S4-39h: a supported `#PF`, `#GP(0)`, or external IRQ from CPL3 is delivered
        // through `TSS.RSP0` and leaves a CPL0
        // state (CS from the gate, RPL 0); the `continue` after a delivery returns here, so the CPL
        // is computed again from the new CS and the next instruction runs through the CPL0 entry
        // (review S4-39c, 1).
        let cpl3 = state.cs_selector.is_some_and(|selector| selector & 3 == 3);
        if cpl3 && !user_mode {
            break (
                PagedCpl0SetExit::Rejected(Cpl0ProfileSetInputError::WrongPrivilege),
                None,
                None,
            );
        }
        let context = if cpl3 {
            AccessContext {
                privilege: od_isa::memory::PrivilegeLevel::Ring3,
                ..context
            }
        } else {
            context
        };
        if let Some(hook) = irq.as_mut() {
            match deliver_pending_irq(
                hook,
                state,
                bus.ram(),
                &paging,
                context,
                &mut watch,
                cache,
                split_gate,
            ) {
                IrqBoundaryOutcome::None => {}
                IrqBoundaryOutcome::Delivered => {
                    if let Some(observations) = execution_observations.as_deref_mut() {
                        observations.note_nested_delivery();
                    }
                    continue;
                }
                IrqBoundaryOutcome::Failed(failure) => break failure,
            }
        }
        // S4-38L4i: one peek serves the CPUID boundary and the recorder (no guest state is
        // written; the cache statistics move, see the doc of this function).
        // S7-31q: the call trace (opt-in) looks at RIP first; a page offset filter keeps it cheap.
        if let Some(recorder) = checkpoints.as_deref_mut()
            && recorder.is_call_candidate(state.rip)
            && let Ok(physical) = crate::page_walk::translate_long64_2m_ram_read_only(
                bus.ram(),
                &paging,
                state.rip,
                GuestLinearAddress::new(state.rip),
                context,
                od_isa::memory::AccessKind::Fetch,
            )
        {
            use od_isa::Register::{R8, R9, Rcx, Rdx};
            let args = [Rcx, Rdx, R8, R9].map(|register| state.gpr[register as usize]);
            let (rip, memory) = (state.rip, bus.ram());
            recorder.note_call(
                physical.get(),
                args,
                state.gpr[od_isa::Register::Rsp as usize],
                instructions_executed,
                state.cs_selector.map_or(0, |selector| (selector & 3) as u8),
                &mut |address, length| {
                    observed_read_bytes(address, length, rip, memory, &paging, context)
                },
                &mut |pointer| {
                    crate::page_walk::translate_long64_2m_ram_read_only(
                        memory,
                        &paging,
                        rip,
                        GuestLinearAddress::new(pointer),
                        context,
                        od_isa::memory::AccessKind::Fetch,
                    )
                    .ok()
                    .map(|physical| physical.get())
                },
            );
        }
        let wants_boundary = checkpoints
            .as_deref()
            .is_some_and(CheckpointRecorder::wants_boundary);
        let cpuid_leaf = if cpuid_boundary || wants_boundary {
            cpuid_leaf_at_rip(state, bus.ram(), &paging, context, cache)
        } else {
            None
        };
        if wants_boundary && let Some(recorder) = checkpoints.as_deref_mut() {
            let view = BoundaryView {
                rip: state.rip,
                cpl: state.cs_selector.map_or(0, |selector| (selector & 3) as u8),
                cr3: reported_cr3,
                call_retired: instructions_executed,
                idtr: state.idtr.map(|idtr| (idtr.base, idtr.limit)),
                cpuid_leaf,
            };
            recorder.boundary(&view, &mut || {
                observed_fetch_view(state.rip, bus.ram(), &paging, context)
            });
            // S4-13b: the boundary at which C5 is recorded splits the phases of the report.
            if !c5_at_call_start && c5_split.is_none() && recorder.is_recorded(C5_CHECKPOINT) {
                c5_split = Some(C5Split {
                    retired: instructions_executed,
                    walks: walks.snapshot(),
                    cache: cache.map(FetchTranslationCache::stats),
                });
                if let Some(cache) = cache {
                    cache.sweep_mark_c5();
                }
            }
        }
        if cpuid_boundary
            && let Some(leaf) = cpuid_leaf
            && is_cpuid_divergence_leaf(leaf)
        {
            break (PagedCpl0SetExit::CpuidDivergence { leaf }, None, None);
        }
        let instruction_rip = state.rip;
        // PERF-20: the instruction about to run, decoded read-only (no accessed/dirty bit, no cache
        // or counter moves), and the CPL it runs at; counted below once it retired.
        let (histogram_instruction, histogram_cpl) = if HISTOGRAM_ENABLED {
            (
                histogram_decode(instruction_rip, bus.ram(), &paging, context),
                state.cs_selector.map_or(0, |selector| (selector & 3) as u8),
            )
        } else {
            (None, 0)
        };
        let mut trace_before = None;
        if let Some(observations) = execution_observations.as_deref_mut() {
            last_instruction_rip = Some(instruction_rip);
            if ELF_COPY_ENABLED {
                if instruction_rip == LINUX_ELF_STACK_ALIGN_BEFORE_RIP
                    || instruction_rip == LINUX_ELF_STACK_ALIGN_AFTER_RIP
                {
                    let before_call = instruction_rip == LINUX_ELF_STACK_ALIGN_BEFORE_RIP;
                    let point = capture_elf_stack_alignment_point(
                        state,
                        bus.machine(),
                        &paging,
                        context,
                        reported_cr3,
                        before_call,
                    );
                    observations.record_elf_stack_alignment_point(point, before_call);
                }
                capture_elf_mmu_gather_boundary(
                    instruction_rip,
                    state,
                    bus.machine(),
                    &paging,
                    context,
                    reported_cr3,
                    observations,
                );
                observations.observe_elf_stack_copy_boundary(instruction_rip);
                if observations.elf_copy_invocation_active()
                    && observations.elf_copy_return_rip() == Some(instruction_rip)
                {
                    observations.finish_elf_copy_invocation(instruction_rip, reported_cr3);
                }
                if instruction_rip == LINUX_ELF_STACK_COPY_RIP
                    && observations.wants_elf_stack_copy()
                {
                    let snapshot = capture_elf_stack_copy_snapshot(
                        state,
                        bus.machine(),
                        &paging,
                        context,
                        reported_cr3,
                    );
                    observations.record_elf_stack_copy(snapshot);
                }
            }
            if TRACE_ENABLED {
                if observations.needs_instruction_path_header() {
                    let (frame_chain, frame_chain_end) = capture_instruction_path_frame_chain(
                        state,
                        bus.machine(),
                        &paging,
                        context,
                    );
                    observations.begin_instruction_path_header(state, frame_chain, frame_chain_end);
                }
                trace_before = Some(instruction_path_before(
                    instruction_rip,
                    state,
                    bus.machine(),
                    &paging,
                    context,
                ));
            }
        }
        // S4-38L5c: RF set before the instruction (a restarted instruction after a `#PF`
        // frame came back through `IRETQ`) is cleared once the instruction retires, except
        // when that instruction is itself an `IRETQ`: it loads RF from its own frame (SDM, from
        // memory) and the loaded value stays.
        let resume_flag_before = state.rflags & RFLAGS_RF != 0;
        let pending_post_iret = execution_observations
            .as_deref()
            .and_then(LinuxExecutionObservationCollector::pending_post_iret);
        let resume_flag_is_iretq =
            resume_flag_before && iretq_at_rip(state, bus.ram(), &paging, context, cache);
        // The selected diagnostic uses the same opcode check but bypasses the fetch cache so it
        // does not perturb cache hit/miss counters while waiting for the selected return.
        let selected_return_is_iretq =
            pending_post_iret.is_some() && iretq_at_rip(state, bus.ram(), &paging, context, None);
        let iretq_detected = resume_flag_is_iretq || selected_return_is_iretq;
        let clear_resume_flag = resume_flag_before && !resume_flag_is_iretq;
        // S4-14h: under `GuestOnly` the guest's own invalidations flush the cache. `INVLPG` is
        // peeked before the step (read-only, through the cache like the fetch itself); CR0/CR4
        // are compared around it.
        let invlpg_pending = guest_only && invlpg_at_rip(state, bus.ram(), &paging, context, cache);
        // S4-13c: the shadows of a `WriteWatch` cache learn of a retired `INVLPG` too. The peek
        // bypasses the cache (read-only walk), so neither the counters nor the order of the cache
        // move; a `GuestOnly` cache tells them itself.
        let sweep_invlpg_pending = !guest_only
            && cache.is_some_and(FetchTranslationCache::sweep_active)
            && invlpg_at_rip(state, bus.ram(), &paging, context, None);
        let control_before = (state.cr0, state.cr4);
        let (ram, io) = bus.split_for_set_step_of_vcpu(vcpu, split_gate);
        let mut port = SetPortWatch {
            inner: io.port_io(),
            pit_written: false,
            port_accessed: false,
        };
        let mut memory = set_loop_adapter(
            ram,
            Some((&io, vcpu)),
            paging,
            instruction_rip,
            &mut watch,
            cache,
            Some(&walks),
        );
        memory.capture_page_fault_trace = execution_observations.is_some();
        memory.descriptor_gate = split_gate;
        memory.decoded.cache = decoded;
        let run_config = RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context,
        };
        let rdtsc_source = rdtsc
            .as_mut()
            .map(|source| &mut **source as &mut dyn FnMut() -> u64);
        let rax_before = state.gpr[od_isa::Register::Rax as usize];
        let result = if cpl3 {
            // The CPL3 entry has no port and only ever returns `Run` (S4-39a).
            run_with_cpl3_profile_set(
                state,
                &mut memory,
                run_config,
                current_rflags_defined,
                set,
                rdtsc_source,
                || false,
            )
            .map(|done| Cpl0ProfileSetResult {
                exit: match done.exit {
                    Cpl0ProfileSetExit::Run(run) => Cpl0ProfileSetExit::Run(run),
                    // Not reachable (the entry never returns another exit); an honest stop.
                    _ => Cpl0ProfileSetExit::Run(RunExit::Unsupported { opcode: 0x0f }),
                },
                instructions_executed: done.instructions_executed,
                rflags_defined: done.rflags_defined,
            })
        } else {
            run_with_cpl0_profile_set_and_pio(
                state,
                &mut memory,
                run_config,
                current_rflags_defined,
                set,
                rdtsc_source,
                &mut port,
                || false,
            )
        };
        // The instruction has committed its cache/TR or exited with its precise fault. Release
        // the instruction lease before attempting a separate exception-entry transaction.
        memory.finish_descriptor_scope();
        memory.descriptor_gate = None;
        pit_written |= port.pit_written;
        port_accessed |= port.port_accessed;
        mmio_written |= io.mmio_written();
        pit_write_pending |= pit_write_ends_quantum && port.pit_written;
        if let Ok(done) = &result {
            if iretq_detected && done.instructions_executed == 1 {
                if let Some(pending) = pending_post_iret.as_ref() {
                    let restored_context_matches = post_iret_context_matches(pending, state);
                    if let Some(observations) = execution_observations.as_deref_mut() {
                        if observations.accept_post_iret_return(restored_context_matches) {
                            let snapshot = memory.capture_post_iret_page_walk(
                                pending.clone(),
                                state,
                                reported_cr3,
                            );
                            observations.record_post_iret_page_walk(snapshot);
                        }
                    }
                }
            }
            if done.instructions_executed > 0 {
                if let Some(before) = trace_before
                    && let Some(observations) = execution_observations.as_deref_mut()
                {
                    observations.record_instruction_path_retirement(
                        finish_instruction_path_record(before, state),
                    );
                }
                *deliveries_without_retirement = 0;
                if clear_resume_flag {
                    state.rflags &= !RFLAGS_RF;
                }
            }
            // S4-38L5c: a guest `#PF` goes through the guest IDT when one is loaded; without
            // an IDT, and for every other vector, the fault stays the stop it was.
            if let Cpl0ProfileSetExit::Run(RunExit::Fault(fault)) = &done.exit
                && fault.vector == 14
                && state.idtr.is_some()
            {
                let faulting_context = execution_observations.as_ref().map(|_| {
                    let rbp = state.gpr[od_isa::Register::Rbp as usize];
                    let (frame_chain, frame_chain_end) =
                        memory.capture_fault_frame_chain(instruction_rip, rbp);
                    FaultingPageFaultContext {
                        cr3: reported_cr3,
                        rsp: state.gpr[od_isa::Register::Rsp as usize],
                        rbp,
                        rcx: state.gpr[od_isa::Register::Rcx as usize],
                        rsi: state.gpr[od_isa::Register::Rsi as usize],
                        rdi: state.gpr[od_isa::Register::Rdi as usize],
                        cs_selector: state.cs_selector,
                        frame_chain,
                        frame_chain_end,
                        page_walk: memory.page_fault_trace.clone(),
                    }
                });
                let fault = fault.clone();
                let original_record = memory.page_fault.clone();
                let failure = if *deliveries_without_retirement >= MAX_DELIVERIES_WITHOUT_RETIREMENT
                {
                    Some(PagedCpl0ExceptionFailure::RepeatLimit)
                } else {
                    match &original_record {
                        Some(record) if record.fault() == &fault => {
                            match with_gated_cpl0_delivery(&mut memory, split_gate, |memory| {
                                deliver_page_fault_cpl0_set(state, memory, record, context)
                            })
                            .unwrap_or_else(|cause| {
                                Cpl0PageFaultDelivery::Aborted {
                                    phase: Cpl0PageFaultDeliveryPhase::IdtGateRead,
                                    cause,
                                }
                            }) {
                                Cpl0PageFaultDelivery::Delivered { .. } => None,
                                Cpl0PageFaultDelivery::Refused(reason) => {
                                    Some(PagedCpl0ExceptionFailure::Refused(reason))
                                }
                                Cpl0PageFaultDelivery::Aborted { phase, cause } => {
                                    Some(PagedCpl0ExceptionFailure::Aborted { phase, cause })
                                }
                            }
                        }
                        _ => Some(PagedCpl0ExceptionFailure::NoPageFaultRecord),
                    }
                };
                match failure {
                    None => {
                        // S4-14h: a delivered `#PF` invalidates the translations of its linear
                        // address (SDM Vol. 3A, "Invalidation of TLBs and Paging-Structure Caches",
                        // cited from memory, not checked against the primary source). Under
                        // `GuestOnly` the whole cache is dropped, which is always allowed; the
                        // handler may repair the page tables without `INVLPG`.
                        if let Some(cache) = cache {
                            if guest_only {
                                cache.flush_after_page_fault();
                            } else {
                                cache.sweep_guest_page_fault();
                            }
                        }
                        delivered_exceptions += 1;
                        delivered_page_faults += 1;
                        if let Some((observations, faulting_context)) =
                            execution_observations.as_deref_mut().zip(faulting_context)
                        {
                            if pending_post_iret.is_some() {
                                observations.note_nested_delivery();
                            }
                            observations.record_page_fault(DeliveredPageFaultObservation {
                                rip: original_record
                                    .as_ref()
                                    .map_or(instruction_rip, |record| record.saved_rip()),
                                cr2: original_record
                                    .as_ref()
                                    .map_or(fault.linear_address.unwrap_or(0), |record| {
                                        record.cr2_value()
                                    }),
                                error_code: fault.error_code.unwrap_or_default(),
                                access: original_record.as_ref().map_or_else(
                                    || "unknown".to_string(),
                                    |record| format!("{:?}", record.access()),
                                ),
                                cpl: u8::from(cpl3) * 3,
                                faulting_context,
                            });
                        }
                        *deliveries_without_retirement += 1;
                        if let Some(recorder) = checkpoints.as_deref_mut() {
                            recorder.note_page_fault_delivered();
                            recorder.note_page_fault_detail(
                                original_record
                                    .as_ref()
                                    .map_or(fault.linear_address.unwrap_or(0), |record| {
                                        record.cr2_value()
                                    }),
                                fault.error_code.unwrap_or_default(),
                            );
                        }
                        if yield_after_delivery {
                            break (PagedCpl0SetExit::Run(RunExit::StopRequested), None, None);
                        }
                        continue;
                    }
                    Some(failure) => {
                        // `memory.memory_error` is the walker's classification of an access
                        // made by the DELIVERY (IDT/GDT read, frame store); the faulting
                        // instruction itself raised an architectural `#PF`, never a
                        // non-architectural one. `page_fault` is the original record.
                        break (
                            PagedCpl0SetExit::ExceptionDeliveryFailed { fault, failure },
                            original_record,
                            memory.memory_error,
                        );
                    }
                }
            }
            // S4-38L9v: a guest `#GP(0)` (an instruction of the set refused with vector 13 and
            // code 0, for example `RDMSR` of an MSR the set does not model) goes through the
            // guest IDT gate of vector 13 when an IDT is loaded; the instruction did not retire.
            // Without an IDT, and for any other code, the fault stays the stop it was.
            if let Cpl0ProfileSetExit::Run(RunExit::Fault(fault)) = &done.exit
                && fault.vector == 13
                && fault.error_code == Some(0)
                && state.idtr.is_some()
            {
                let fault = fault.clone();
                let original_record = memory.page_fault.clone();
                let failure = if *deliveries_without_retirement >= MAX_DELIVERIES_WITHOUT_RETIREMENT
                {
                    Some(PagedCpl0ExceptionFailure::RepeatLimit)
                } else {
                    match with_gated_cpl0_delivery(&mut memory, split_gate, |memory| {
                        deliver_general_protection_cpl0_set(state, memory, &fault, context)
                    })
                    .unwrap_or_else(|cause| {
                        Cpl0GeneralProtectionDelivery::Aborted {
                            phase: Cpl0GeneralProtectionPhase::IdtGateRead,
                            cause,
                        }
                    }) {
                        Cpl0GeneralProtectionDelivery::Delivered { .. } => None,
                        Cpl0GeneralProtectionDelivery::Refused(reason) => {
                            Some(PagedCpl0ExceptionFailure::GeneralProtectionRefused(reason))
                        }
                        Cpl0GeneralProtectionDelivery::Aborted { phase, cause } => {
                            Some(PagedCpl0ExceptionFailure::GeneralProtectionAborted {
                                phase,
                                cause,
                            })
                        }
                    }
                };
                match failure {
                    None => {
                        delivered_exceptions += 1;
                        delivered_general_protection_faults += 1;
                        if ELF_COPY_ENABLED
                            && let Some(observations) = execution_observations.as_deref_mut()
                            && observations.tracks_elf_copy_counters()
                        {
                            observations.record_elf_copy_general_protection();
                        }
                        if pending_post_iret.is_some()
                            && let Some(observations) = execution_observations.as_deref_mut()
                        {
                            observations.note_nested_delivery();
                        }
                        *deliveries_without_retirement += 1;
                        if yield_after_delivery {
                            break (PagedCpl0SetExit::Run(RunExit::StopRequested), None, None);
                        }
                        continue;
                    }
                    Some(failure) => {
                        break (
                            PagedCpl0SetExit::ExceptionDeliveryFailed { fault, failure },
                            original_record,
                            memory.memory_error,
                        );
                    }
                }
            }
        }
        let (page_fault, memory_error) = (memory.page_fault.take(), memory.memory_error.take());
        drop(memory);
        let result = match result {
            Ok(result) => result,
            Err(error) => break (PagedCpl0SetExit::Rejected(error), page_fault, memory_error),
        };
        // S4-39f: the syscall journal. A retired instruction of the CPL3 entry that left CPL3 is
        // the `SYSCALL` (the only form of the allow-list that changes the CPL); a committed
        // return to CPL3 from the CPL0 entry is a `SYSRETQ` when the instruction is one (an
        // `IRETQ` return is not a system call).
        if user_mode && result.instructions_executed == 1 {
            let event = if cpl3 {
                state
                    .cs_selector
                    .is_some_and(|selector| selector & 3 != 3)
                    .then(|| SyscallEvent::Enter {
                        nr: rax_before,
                        caller: SyscallUserContext {
                            rip: state.gpr[od_isa::Register::Rcx as usize],
                            rsp: state.gpr[od_isa::Register::Rsp as usize],
                            cr3: paging.cr3(),
                        },
                    })
            } else if matches!(result.exit, Cpl0ProfileSetExit::UserModeEntered(_))
                && sysretq_at(instruction_rip, bus.ram(), &paging, context, cache)
            {
                Some(SyscallEvent::Exit {
                    rax: state.gpr[od_isa::Register::Rax as usize],
                    caller: SyscallUserContext {
                        rip: state.rip,
                        rsp: state.gpr[od_isa::Register::Rsp as usize],
                        cr3: paging.cr3(),
                    },
                })
            } else {
                None
            };
            if let Some(event) = event {
                if let Some(publish) = on_syscall.as_mut() {
                    // A task can migrate as soon as a later kernel instruction publishes it.
                    // Publish this observation before the vCPU runs that instruction.
                    publish(event);
                } else {
                    if syscall_events.len() == SYSCALL_EVENTS_PER_CALL {
                        syscall_events.pop_front();
                    }
                    syscall_events.push_back(event);
                }
            }
        }
        if ELF_COPY_ENABLED
            && let Some(observations) = execution_observations.as_deref_mut()
            && observations.tracks_elf_copy_counters()
        {
            observations.record_elf_copy_retired(result.instructions_executed);
            if let Cpl0ProfileSetExit::TranslationContextChange(
                TranslationContextChange::Cr3Write { old_cr3, new_cr3 },
            ) = &result.exit
            {
                let target_capture =
                    if observations.wants_elf_copy_target_cr3_event(instruction_rip) {
                        let rsp = state.gpr[od_isa::Register::Rsp as usize];
                        let rbp = state.gpr[od_isa::Register::Rbp as usize];
                        let chain_paging = Long64ProbeConfig::from_cpu_state(state, *new_cr3, efer);
                        let (chain_status, frames, error) = match chain_paging {
                            Ok(chain_paging) => capture_elf_stack_copy_target_rbp_chain(
                                state,
                                bus.machine(),
                                &chain_paging,
                                instruction_rip,
                                context,
                            ),
                            Err(error) => (
                                ElfStackCopyTargetCr3ChainStatus::Unreadable,
                                Vec::new(),
                                Some(format!(
                                    "post-write paging configuration rejected: {error:?}"
                                )),
                            ),
                        };
                        Some(ElfStackCopyTargetCr3Capture {
                            status: "captured",
                            sequence: None,
                            rip: Some(instruction_rip),
                            cr3: Some(*new_cr3),
                            retired_instructions: None,
                            rsp: Some(rsp),
                            rbp: Some(rbp),
                            chain_status: Some(chain_status),
                            frames,
                            error,
                        })
                    } else {
                        None
                    };
                if let Some(target_capture) = target_capture {
                    observations.record_elf_copy_cr3_with_target(
                        instruction_rip,
                        *old_cr3,
                        *new_cr3,
                        if cpl3 { 3 } else { 0 },
                        Some(target_capture),
                    );
                } else {
                    observations.record_elf_copy_cr3(
                        instruction_rip,
                        *old_cr3,
                        *new_cr3,
                        if cpl3 { 3 } else { 0 },
                    );
                }
            }
        }
        instructions_executed += result.instructions_executed;
        if HISTOGRAM_ENABLED
            && result.instructions_executed > 0
            && let Some(recorder) = checkpoints.as_deref_mut()
        {
            recorder.note_retired_operation(
                instruction_rip,
                histogram_instruction,
                histogram_cpl,
                result.instructions_executed,
            );
        }
        if cpl3 {
            cpl3_retired += result.instructions_executed;
        }
        current_rflags_defined = result.rflags_defined;
        // S4-13: a retired `MOV CR3` (or any other translation context change) invalidates
        // every cached fetch translation.
        if matches!(result.exit, Cpl0ProfileSetExit::TranslationContextChange(_))
            && let Some(cache) = cache
        {
            cache.flush();
        }
        if matches!(
            result.exit,
            Cpl0ProfileSetExit::TranslationContextChange(TranslationContextChange::Cr3Write { .. })
        ) && let Some(recorder) = checkpoints.as_deref_mut()
        {
            recorder.note_cr3_write();
        }
        // S4-14h: a retired `INVLPG` and a change of CR0/CR4 flush the whole cache (`GuestOnly`).
        if guest_only && let Some(cache) = cache {
            if invlpg_pending && result.instructions_executed > 0 {
                cache.flush_after_invlpg();
            }
            if control_before != (state.cr0, state.cr4) {
                cache.flush_after_control_change();
            }
        } else if let Some(cache) = cache {
            // S4-13c: observation only, for the shadows.
            if sweep_invlpg_pending && result.instructions_executed > 0 {
                cache.sweep_guest_invlpg();
            }
            if cache.sweep_active() && control_before != (state.cr0, state.cr4) {
                cache.sweep_guest_control_change();
            }
        }
        if let Some(exit) = handle_set_exit(
            result.exit,
            result.instructions_executed,
            state,
            &mut efer,
            &mut paging,
            &mut reported_cr3,
            &mut changes,
        ) {
            break (exit, page_fault, memory_error);
        }
    };
    // S4-14i: the end of the call seals the cache with the context a continuing call is handed;
    // a change of it or a write by anyone before the next call starts is caught by the check there.
    if let Some(cache) = cache {
        cache.seal(
            continuation_key(state, reported_cr3, efer, context),
            &|page| {
                bus.ram()
                    .code_page_generation(GuestPhysicalAddress::new(page << 12))
                    .ok()
            },
        );
    }
    PagedCpl0SetReport {
        exit,
        instructions_executed,
        rflags_defined: current_rflags_defined,
        page_fault,
        memory_error,
        translation_context_changes: changes,
        cr3: reported_cr3,
        efer,
        delivered_exceptions,
        delivered_page_faults,
        delivered_general_protection_faults,
        pit_written,
        port_accessed,
        mmio_written,
        fetch_cache_stats: cache.map(FetchTranslationCache::stats),
        cpl3_retired,
        syscall_events: syscall_events.into_iter().collect(),
        last_instruction_rip,
        walk_counts: walks.snapshot(),
        c5_at_call_start,
        c5_split,
    }
}

/// The translation context a call that continues this one is handed (S4-14i): the paging snapshot
/// of `cr3` and the state, and the access context of the loop for the CPL of the state. The same
/// function computes the seal of the end of a call and the check of the start of the next.
fn continuation_key(
    state: &LongModeState,
    cr3: u64,
    efer: u64,
    context: AccessContext,
) -> Option<TranslationKey> {
    let paging = Long64ProbeConfig::from_cpu_state(state, cr3, efer).ok()?;
    let privilege = if state.cs_selector.is_some_and(|selector| selector & 3 == 3) {
        od_isa::memory::PrivilegeLevel::Ring3
    } else {
        context.privilege
    };
    Some(TranslationKey::new(
        paging,
        AccessContext {
            privilege,
            ..context
        },
    ))
}

/// The adapter of the set loop for one instruction or one interrupt delivery: RAM only, the
/// whole CPL0 set read and write widths, the optional fetch cache.
fn set_loop_adapter<'a>(
    ram: &'a crate::PhysicalMemory,
    mmio: Option<(&'a dyn crate::bus::SetStepMmio, usize)>,
    paging: Long64ProbeConfig,
    instruction_rip: u64,
    committed_write: &'a mut dyn FnMut(GuestPhysicalRange),
    fetch_cache: Option<&'a FetchTranslationCache>,
    walks: Option<&'a WalkCounter>,
) -> PagedMovMemoryAdapter<'a> {
    PagedMovMemoryAdapter {
        bus: match mmio {
            Some((mmio, vcpu)) => AdapterBus::RamMmio(ram, mmio, vcpu),
            None => AdapterBus::Ram(ram),
        },
        paging,
        instruction_rip,
        committed_write,
        page_fault: None,
        memory_error: None,
        access_error: None,
        page_fault_trace: None,
        capture_page_fault_trace: false,
        committed_ad_updates: None,
        committed_writes: None,
        capture_effects: false,
        descriptor_gate: None,
        descriptor_scope: None,
        dword_read_profile: false,
        word_read_profile: false,
        ram_only: true,
        reject_crossing_qword: false,
        read_only_qword: false,
        large_ram_profile: true,
        dword_write_profile: false,
        cpl0_set_dword_read: true,
        cpl0_set_byte_read: true,
        cpl0_set_word_read: true,
        cpl0_set_narrow_write: true,
        cpl0_set_cross_page_fetch: true,
        cpl0_set_cross_page_store: true,
        cpl0_set_cross_page_read: true,
        fetch_cache,
        walks,
        decoded: DecodedStep::default(),
    }
}

/// S6-41c: the outcome of [`locate_paged_atomic_operand`].
#[derive(Debug)]
pub enum PagedAtomicOperand<'m> {
    /// The operand is naturally aligned plain RAM; the walk has been done and its effects
    /// (accessed and dirty bits, in the order and with the notifications of the interpreter's
    /// locked operation) are committed. The window gives the host cell and the page counter.
    Located(crate::AtomicOperandWindow<'m>),
    /// The operand is not naturally aligned for its width: nothing was walked, counted or
    /// changed. A split `LOCK` is the business of the interpreter step, which carries the gate of
    /// S6-34 step 8 (JIT_CONTRACT S6-41c: the inline path declines it).
    Misaligned,
    /// The walk raised an architectural fault (`#PF` with the effects of the walk before the
    /// fault, as for the interpreter; `#GP`/`#SS` for a non-canonical operand is raised before
    /// this function, by the address computation). No data byte was touched.
    Fault(od_isa::memory::ArchitecturalFault),
    /// The operand is not inline-able (MMIO, ROM, framebuffer, a hole, an exhausted page counter,
    /// a backend error of the walk): the interpreter owns the instruction. The walk may have set
    /// accessed bits, exactly as the interpreter's own walk would.
    Declined(MemoryError),
}

/// S6-41c: the translation half of an inline atomic operation (JIT_CONTRACT S6-41c). One call is
/// the "translate and check with the intent to write" of a `LOCK` form, `XCHG` or `CMPXCHG` on
/// the linear address `linear` of `size` bytes: the walks of [`PagedMovMemoryAdapter`]'s locked
/// operations (a read walk, then a write walk: present, write, user and reserved-bit checks,
/// accessed and dirty bits, the faults of a write) on the whole CPL0-set profile, and then the
/// window of the operand. The write intent applies to every outcome of the operation, so the
/// unequal branch of `CMPXCHG` (x86 writes the destination back, ARM `CAS` does not) takes the
/// same `#PF` on a read-only page and sets the same dirty bit as the interpreter. No data byte is
/// read or written here. `committed_write` receives the page-table writes of the walk.
pub fn locate_paged_atomic_operand<'m>(
    memory: &'m crate::PhysicalMemory,
    paging: Long64ProbeConfig,
    instruction_rip: u64,
    context: AccessContext,
    linear: u64,
    size: od_isa::OperandSize,
    committed_write: &mut dyn FnMut(GuestPhysicalRange),
) -> (PagedAtomicOperand<'m>, Option<PageFaultRecord>) {
    let bytes = size.bytes() as u64;
    if linear % bytes != 0 {
        return (PagedAtomicOperand::Misaligned, None);
    }
    let mut adapter = set_loop_adapter(
        memory,
        None,
        paging,
        instruction_rip,
        committed_write,
        None,
        None,
    );
    let operand = match adapter.locked_target(linear, context, size) {
        Ok(LockedTarget::Host { width, physical }) => {
            match memory.atomic_operand_window(physical, width.bytes() as usize) {
                Ok(window) => PagedAtomicOperand::Located(window),
                Err(error) => PagedAtomicOperand::Declined(MemoryError::Backend(format!(
                    "atomic operand window of {physical:?}: {error:?}"
                ))),
            }
        }
        // Not reachable for an aligned operand; declined rather than guessed.
        Ok(LockedTarget::Serial | LockedTarget::Stopped(_)) => PagedAtomicOperand::Misaligned,
        Err(MemoryError::Architectural(fault)) => PagedAtomicOperand::Fault(fault),
        Err(error) => PagedAtomicOperand::Declined(error),
    };
    (operand, adapter.page_fault.take())
}

/// S6-41c: runs `callback` with the adapter of the CPL0 profile set over `memory` (the form the
/// board's interpreter uses: RAM only, every set width, locked operations on the host atomics of
/// the RAM). It is the interpreter-side reference of [`locate_paged_atomic_operand`] for tests and
/// differential runs: the same tables, the same walks, the same notifications. Returns the
/// callback's result and the page fault record of the last fault, if any.
/// A callback that updates descriptors must have exclusive access to guest RAM. A live worker
/// executes such an instruction through the CPL0 set with its worker gate.
pub fn with_paged_cpl0_set_memory<R>(
    memory: &crate::PhysicalMemory,
    paging: Long64ProbeConfig,
    instruction_rip: u64,
    committed_write: &mut dyn FnMut(GuestPhysicalRange),
    callback: impl FnOnce(&mut PagedMovMemoryAdapter<'_>) -> R,
) -> (R, Option<PageFaultRecord>) {
    let mut adapter = set_loop_adapter(
        memory,
        None,
        paging,
        instruction_rip,
        committed_write,
        None,
        None,
    );
    let result = callback(&mut adapter);
    (result, adapter.page_fault.take())
}

/// The port view of one set call that notes a retired write to the PIT ports `0x40..=0x43`
/// (S4-38L8b). Reads and every other port pass through untouched.
struct SetPortWatch<P> {
    inner: P,
    pit_written: bool,
    port_accessed: bool,
}

impl<P: od_isa::PortIo> od_isa::PortIo for SetPortWatch<P> {
    type Error = P::Error;

    fn read_u8(&mut self, port: u16) -> Result<u8, Self::Error> {
        let result = self.inner.read_u8(port);
        self.port_accessed |= result.is_ok();
        result
    }

    fn write_u8(&mut self, port: u16, value: u8) -> Result<(), Self::Error> {
        let result = self.inner.write_u8(port, value);
        if result.is_ok() {
            self.port_accessed = true;
            self.pit_written |= (0x40..=0x43).contains(&port);
        }
        result
    }

    fn read_u16(&mut self, port: u16) -> Result<u16, Self::Error> {
        let result = self.inner.read_u16(port);
        self.port_accessed |= result.is_ok();
        result
    }

    fn write_u16(&mut self, port: u16, value: u16) -> Result<(), Self::Error> {
        let result = self.inner.write_u16(port, value);
        // A word write covers `port` and `port + 1` (S4-38L9r).
        if result.is_ok() {
            self.port_accessed = true;
            self.pit_written |= (0x3f..=0x43).contains(&port);
        }
        result
    }

    fn read_u32(&mut self, port: u16) -> Result<u32, Self::Error> {
        let result = self.inner.read_u32(port);
        self.port_accessed |= result.is_ok();
        result
    }

    fn write_u32(&mut self, port: u16, value: u32) -> Result<(), Self::Error> {
        let result = self.inner.write_u32(port, value);
        // A dword write covers `port..=port + 3` (S4-38L9u).
        if result.is_ok() {
            self.port_accessed = true;
            self.pit_written |= (0x3d..=0x43).contains(&port);
        }
        result
    }
}

/// Checkpoint C5 (the first fetch at `e_entry`), the split of the phases of the fetch cache report.
const C5_CHECKPOINT: u8 = 5;

/// RFLAGS.IF, bit 9.
const RFLAGS_IF: u64 = 1 << 9;

/// The interrupt check of one instruction boundary (S4-38L8b), after the stop and budget checks
/// and before the CPUID check; `Some(exit)` ends the run.
///
/// Order: 1) the first boundary of a call that resumes a partial REP is skipped (the REP is in
/// the middle; the flag is cleared); 2) `pending_vector` is asked once (no vector: nothing else
/// runs); 3) RFLAGS.IF = 0 counts `irq_refused_masked`, an STI shadow that is not `Clear` counts
/// `irq_refused_shadow` (the vector stays pending, nothing is acknowledged, and the IDT is not
/// read); 4) otherwise `deliver_external_interrupt_cpl0_set`: `Delivered` acknowledges the
/// vector and counts `irqs_delivered`; a refusal of IF or shadow (not expected after 3) counts
/// like 3; any other refusal, and `Aborted`, end the run as `IrqDeliveryFailed`, never silently.
pub(crate) enum IrqBoundaryOutcome {
    None,
    Delivered,
    Failed(
        (
            PagedCpl0SetExit,
            Option<PageFaultRecord>,
            Option<TrackedAccessError>,
        ),
    ),
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct DescriptorLocation {
    physical: GuestPhysicalAddress,
    tables: [Option<GuestPhysicalAddress>; 4],
}

struct DescriptorRead {
    linear: u64,
    context: AccessContext,
    first: DescriptorLocation,
    second: Option<DescriptorLocation>,
    bytes: [u8; 8],
}

impl DescriptorRead {
    fn parts(&self) -> impl Iterator<Item = (DescriptorLocation, std::ops::Range<usize>)> {
        let split = Self::first_len(self.linear);
        [(Some(self.first), 0..split), (self.second, split..8)]
            .into_iter()
            .filter_map(|(location, bytes)| Some((location?, bytes)))
    }

    fn first_len(linear: u64) -> usize {
        (4096 - (linear & 0xfff) as usize).min(8)
    }
}

struct DescriptorScope<'a> {
    _lease: crate::bus::DescriptorGate<'a>,
    reads: Vec<DescriptorRead>,
    // Instructions can derive cache state from more than the descriptor they are updating.
    check_companions: bool,
}

impl<'a> PagedMovMemoryAdapter<'a> {
    fn begin_descriptor_scope(
        &mut self,
        gate: &'a dyn crate::bus::SplitLockGate,
        check_companions: bool,
    ) -> Result<(), MemoryError> {
        let lease = crate::bus::DescriptorGate::acquire(gate)
            .map_err(|error| MemoryError::Backend(format!("descriptor gate: {error}")))?;
        // Crossing set-loop reads also take the journaled byte path. Frames are aligned and
        // crossing descriptor writes are rejected before the ordinary write preflight.
        debug_assert!(self.large_ram_profile && self.cpl0_set_cross_page_read);
        debug_assert!(!self.capture_effects && self.descriptor_scope.is_none());
        debug_assert!(self.committed_writes.is_none() && self.committed_ad_updates.is_none());
        self.capture_effects = true;
        self.committed_writes = Some(Vec::new());
        self.committed_ad_updates = Some(Vec::new());
        self.descriptor_scope = Some(DescriptorScope {
            _lease: lease,
            reads: Vec::new(),
            check_companions,
        });
        Ok(())
    }

    fn finish_descriptor_scope(&mut self) {
        let Some(scope) = self.descriptor_scope.take() else {
            return;
        };
        self.capture_effects = false;
        self.committed_ad_updates = None;
        let writes = self
            .committed_writes
            .take()
            .expect("descriptor write journal");
        drop(scope);
        for range in writes {
            (self.committed_write)(range);
        }
    }

    fn descriptor_location(
        &self,
        address: u64,
        context: AccessContext,
        access: AccessKind,
    ) -> Result<DescriptorLocation, TrackedAccessError> {
        let (physical, tables) = crate::page_walk::translate_long64_2m_ram_read_only_with_tables(
            self.bus.locked_memory(),
            &self.paging,
            self.instruction_rip,
            GuestLinearAddress::new(address),
            context,
            access,
        )?;
        Ok(DescriptorLocation { physical, tables })
    }

    fn record_descriptor_read(&mut self, linear: u64, context: AccessContext, bytes: &[u8]) {
        if self.descriptor_scope.is_none() {
            return;
        }
        let (Ok(bytes), Ok(first)) = (
            <[u8; 8]>::try_from(bytes),
            self.descriptor_location(linear, context, AccessKind::Read),
        ) else {
            return;
        };
        let split = DescriptorRead::first_len(linear);
        let second = if split < 8 {
            let Ok(location) = self.descriptor_location(
                linear.wrapping_add(split as u64),
                context,
                AccessKind::Read,
            ) else {
                return;
            };
            Some(location)
        } else {
            None
        };
        if let Some(scope) = &mut self.descriptor_scope {
            scope.reads.push(DescriptorRead {
                linear,
                context,
                first,
                second,
                bytes,
            });
        }
    }

    fn check_descriptor_write(
        &self,
        address: u64,
        context: AccessContext,
        width: usize,
    ) -> Result<Option<GuestPhysicalAddress>, MemoryError> {
        let Some(scope) = &self.descriptor_scope else {
            return Ok(None);
        };
        if !matches!(width, 1 | 8) || (address & 0xfff) + width as u64 > 4096 {
            return Err(MemoryError::Backend(
                "shared descriptor updates require a byte or single-page qword write".into(),
            ));
        }
        // Leave a failing write-permission probe to the tracked walk. It owns the fault and
        // any page-table updates committed before that fault.
        let Ok(target) = self.descriptor_location(address, context, AccessKind::Write) else {
            return Ok(None);
        };
        let Some((selected, offset)) = scope.reads.iter().rev().find_map(|read| {
            let offset = address.checked_sub(read.linear)?;
            (offset <= (8 - width) as u64).then_some((read, offset as usize))
        }) else {
            return Err(MemoryError::Backend(
                "descriptor was not read before update".into(),
            ));
        };
        let matches_read = selected.parts().any(|(read_location, part)| {
            offset >= part.start
                && offset + width <= part.end
                && target.physical.get()
                    == read_location.physical.get() + (offset - part.start) as u64
                && target.tables == read_location.tables
        });
        let changed = || {
            let update = if scope.check_companions {
                "Accessed/Busy"
            } else {
                "Accessed"
            };
            MemoryError::Backend(format!(
                "descriptor or its translation changed before the {update} write"
            ))
        };
        if !matches_read {
            return Err(changed());
        }

        let memory = self.bus.locked_memory();
        let target_page = GuestPhysicalAddress::new(target.physical.get() & !0xfff);
        for read in &scope.reads {
            if !scope.check_companions && !std::ptr::eq(read, selected) {
                continue;
            }
            for (location, part) in read.parts() {
                let page = GuestPhysicalAddress::new(location.physical.get() & !0xfff);
                // The write's A/D bits must not touch a checked descriptor. Nor may the
                // descriptor update change a table used by any checked read.
                let mut current = [0; 8];
                if target.tables.contains(&Some(page))
                    || location.tables.contains(&Some(target_page))
                    || self.descriptor_location(
                        read.linear.wrapping_add(part.start as u64),
                        read.context,
                        AccessKind::Read,
                    ) != Ok(location)
                    || memory
                        .load_bytes(location.physical, &mut current[..part.len()])
                        .is_err()
                    || current[..part.len()] != read.bytes[part]
                {
                    return Err(changed());
                }
            }
        }
        Ok(Some(target.physical))
    }

    fn record_descriptor_write(&mut self, physical: GuestPhysicalAddress, source: &[u8]) {
        let Some(scope) = &mut self.descriptor_scope else {
            return;
        };
        // IRETQ may update CS before SS. Carry our own successful write into the checked
        // image, also for any other linear alias of the same bytes.
        let start = u128::from(physical.get());
        let end = start + source.len() as u128;
        for read in &mut scope.reads {
            let split = DescriptorRead::first_len(read.linear);
            for (location, part) in [(Some(read.first), 0..split), (read.second, split..8)] {
                let Some(location) = location else {
                    continue;
                };
                let part_start = u128::from(location.physical.get());
                let overlap_start = start.max(part_start);
                let overlap_end = end.min(part_start + part.len() as u128);
                if overlap_start < overlap_end {
                    let offset = (overlap_start - start) as usize;
                    let dest = part.start + (overlap_start - part_start) as usize;
                    let len = (overlap_end - overlap_start) as usize;
                    read.bytes[dest..dest + len].copy_from_slice(&source[offset..offset + len]);
                }
            }
        }
    }
}

fn with_gated_cpl0_delivery<'a, T>(
    memory: &mut PagedMovMemoryAdapter<'a>,
    gate: Option<&'a dyn crate::bus::SplitLockGate>,
    deliver: impl FnOnce(&mut PagedMovMemoryAdapter<'a>) -> T,
) -> Result<T, MemoryError> {
    if let Some(gate) = gate {
        memory.begin_descriptor_scope(gate, false)?;
    }
    let result = deliver(memory);
    memory.finish_descriptor_scope();
    Ok(result)
}

fn with_cpl0_gate_memory<T>(
    ram: &crate::PhysicalMemory,
    paging: &Long64ProbeConfig,
    rip: u64,
    watch: &mut dyn FnMut(GuestPhysicalRange),
    cache: Option<&FetchTranslationCache>,
    gate: Option<&dyn crate::bus::SplitLockGate>,
    deliver: impl FnOnce(&mut PagedMovMemoryAdapter<'_>) -> T,
) -> Result<(T, Option<PageFaultRecord>, Option<TrackedAccessError>), MemoryError> {
    let mut memory = set_loop_adapter(ram, None, *paging, rip, watch, cache, None);
    let delivery = with_gated_cpl0_delivery(&mut memory, gate, deliver)?;
    Ok((delivery, memory.page_fault, memory.memory_error))
}

pub(crate) fn deliver_pending_irq(
    hook: &mut IrqBoundaryHook<'_>,
    state: &mut LongModeState,
    memory: &crate::PhysicalMemory,
    paging: &Long64ProbeConfig,
    context: AccessContext,
    watch: &mut dyn FnMut(GuestPhysicalRange),
    cache: Option<&FetchTranslationCache>,
    gate: Option<&dyn crate::bus::SplitLockGate>,
) -> IrqBoundaryOutcome {
    if hook.skip_first_boundary {
        hook.skip_first_boundary = false;
        return IrqBoundaryOutcome::None;
    }
    // PERF-05: a source that is stable between device accesses is asked again only when a guest
    // access reached a device since the last question or a vector was delivered. The checks below
    // (IF, STI shadow, the refusal counters, the delivery) run at every boundary on the saved answer.
    let reuse = match (hook.polled, hook.device_accesses) {
        (Some(polled), Some(counter))
            if hook.source.stable_between_device_accesses()
                && counter.load(std::sync::atomic::Ordering::Relaxed) == polled.device_accesses =>
        {
            Some(polled.vector)
        }
        _ => None,
    };
    let answer = match reuse {
        Some(answer) => answer,
        None => {
            let device_accesses = hook
                .device_accesses
                .map(|counter| counter.load(std::sync::atomic::Ordering::Relaxed));
            let answer = hook.source.pending_vector();
            hook.polled = device_accesses.map(|device_accesses| PolledVector {
                vector: answer,
                device_accesses,
            });
            answer
        }
    };
    let Some(vector) = answer else {
        return IrqBoundaryOutcome::None;
    };
    if state.rflags & RFLAGS_IF == 0 {
        hook.counters.irq_refused_masked += 1;
        return IrqBoundaryOutcome::None;
    }
    if state.sti_shadow != Some(od_isa::StiShadow::Clear) {
        hook.counters.irq_refused_shadow += 1;
        return IrqBoundaryOutcome::None;
    }
    let (delivery, page_fault, memory_error) =
        with_cpl0_gate_memory(memory, paging, state.rip, watch, cache, gate, |memory| {
            deliver_external_interrupt_cpl0_set(state, memory, vector, context)
        })
        .unwrap_or_else(|cause| (gate_acquisition_failure(cause), None, None));
    let failure = match delivery {
        Cpl0ExternalInterruptDelivery::Delivered { .. } => {
            hook.source.acknowledge(vector);
            // PERF-05: the INTA changed the controller: the next boundary asks again.
            hook.polled = None;
            hook.counters.irqs_delivered += 1;
            if let Some(delivered) = hook.delivered_mirror {
                delivered.set(delivered.get().saturating_add(1));
            }
            return IrqBoundaryOutcome::Delivered;
        }
        Cpl0ExternalInterruptDelivery::Refused(Cpl0ExternalInterruptRefusal::InterruptsMasked) => {
            hook.counters.irq_refused_masked += 1;
            return IrqBoundaryOutcome::None;
        }
        Cpl0ExternalInterruptDelivery::Refused(Cpl0ExternalInterruptRefusal::StiShadowActive) => {
            hook.counters.irq_refused_shadow += 1;
            return IrqBoundaryOutcome::None;
        }
        Cpl0ExternalInterruptDelivery::Refused(reason) => IrqDeliveryFailure::Refused(reason),
        Cpl0ExternalInterruptDelivery::Aborted { phase, cause } => {
            IrqDeliveryFailure::Aborted { phase, cause }
        }
    };
    IrqBoundaryOutcome::Failed((
        PagedCpl0SetExit::IrqDeliveryFailed { vector, failure },
        page_fault,
        memory_error,
    ))
}

fn gate_acquisition_failure(cause: MemoryError) -> Cpl0ExternalInterruptDelivery {
    Cpl0ExternalInterruptDelivery::Aborted {
        phase: Cpl0ExternalInterruptPhase::IdtGateRead,
        cause,
    }
}

/// Deliver a native instruction's already observed CPL0 fault without executing it again.
/// As in the CPL0 set loop, only #PF with its walk record and #GP(0) enter the guest IDT.
/// The caller holds exclusive guest RAM access. Running workers use their `HostSplitGate`.
pub fn deliver_host_native_fault(
    state: &mut LongModeState,
    bus: &MachineBus,
    paging: &Long64ProbeConfig,
    context: AccessContext,
    fault: &ArchitecturalFault,
    page_fault: Option<&PageFaultRecord>,
    consecutive: u64,
    committed_write: impl FnMut(GuestPhysicalRange),
) -> bool {
    deliver_host_native_fault_gated(
        state,
        bus,
        paging,
        context,
        fault,
        page_fault,
        consecutive,
        None,
        committed_write,
    )
    .unwrap_or(false)
}

pub(crate) fn deliver_host_native_fault_gated(
    state: &mut LongModeState,
    bus: &MachineBus,
    paging: &Long64ProbeConfig,
    context: AccessContext,
    fault: &ArchitecturalFault,
    page_fault: Option<&PageFaultRecord>,
    consecutive: u64,
    gate: Option<&dyn crate::bus::SplitLockGate>,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<bool, PagedCpl0ExceptionFailure> {
    if state.idtr.is_none() || consecutive >= MAX_DELIVERIES_WITHOUT_RETIREMENT {
        return Ok(false);
    }
    let mut memory = set_loop_adapter(
        bus.physical_memory(),
        None,
        *paging,
        state.rip,
        &mut committed_write,
        None,
        None,
    );
    match (fault.vector, fault.error_code) {
        (14, _) => {
            let Some(record) = page_fault.filter(|record| record.fault() == fault) else {
                return Ok(false);
            };
            let delivery = with_gated_cpl0_delivery(&mut memory, gate, |memory| {
                deliver_page_fault_cpl0_set(state, memory, record, context)
            })
            .unwrap_or_else(|cause| Cpl0PageFaultDelivery::Aborted {
                phase: Cpl0PageFaultDeliveryPhase::IdtGateRead,
                cause,
            });
            match delivery {
                Cpl0PageFaultDelivery::Delivered { .. } => Ok(true),
                Cpl0PageFaultDelivery::Aborted { phase, cause } if gate.is_some() => {
                    Err(PagedCpl0ExceptionFailure::Aborted { phase, cause })
                }
                _ => Ok(false),
            }
        }
        (13, Some(0)) => {
            let delivery = with_gated_cpl0_delivery(&mut memory, gate, |memory| {
                deliver_general_protection_cpl0_set(state, memory, fault, context)
            })
            .unwrap_or_else(|cause| Cpl0GeneralProtectionDelivery::Aborted {
                phase: Cpl0GeneralProtectionPhase::IdtGateRead,
                cause,
            });
            match delivery {
                Cpl0GeneralProtectionDelivery::Delivered { .. } => Ok(true),
                Cpl0GeneralProtectionDelivery::Aborted { phase, cause } if gate.is_some() => {
                    Err(PagedCpl0ExceptionFailure::GeneralProtectionAborted { phase, cause })
                }
                _ => Ok(false),
            }
        }
        _ => Ok(false),
    }
}

/// Deliver a non-maskable interrupt through vector 2 at an instruction boundary outside a run of
/// the set loop (S6-12b), through the same RAM-only paged adapter as [`deliver_pending_irq`]: the
/// frame goes onto the guest stack, `state` enters the handler of the IDT gate of vector 2 and
/// `state.nmi_blocked` is set until the handler's `IRETQ`. Nothing is retired. The result is the
/// one of [`deliver_nmi_cpl0_set`]: a refusal (blocked, STI shadow, activity) is the caller's to
/// hold the NMI pending for, and every refusal and abort leaves `state` unchanged.
pub(crate) fn deliver_nmi_at_boundary(
    state: &mut LongModeState,
    memory: &crate::PhysicalMemory,
    paging: &Long64ProbeConfig,
    context: AccessContext,
    gate: Option<&dyn crate::bus::SplitLockGate>,
) -> Cpl0ExternalInterruptDelivery {
    let mut committed_write = |_: GuestPhysicalRange| {};
    with_cpl0_gate_memory(
        memory,
        paging,
        state.rip,
        &mut committed_write,
        None,
        gate,
        |memory| deliver_nmi_cpl0_set(state, memory, context),
    )
    .map(|(delivery, _, _)| delivery)
    .unwrap_or_else(gate_acquisition_failure)
}

/// What the set loop does with the exit of one set call: `None` continues with the next
/// instruction, `Some(exit)` ends the run.
///
/// A retired `MOV CR3` is recorded first, before any check: `reported_cr3` becomes the new
/// CR3 and the change is appended to `changes`, so that an integration error on that very
/// instruction (`Cr3EffectMismatch`, `Cr3SnapshotRejected`) still leaves the report with the
/// CR3 of the retired instruction (the instruction retired and was counted; only the walker
/// is stale). On `Cr3EffectMismatch` the CPU and the effect disagree: the report (`cr3` and
/// `changes`) carries `effect_new_cr3` (the `new_cr3` of the effect), and the CPU value is
/// exposed only in the `cpu_cr3` field of the exit. The walker (`paging`) is replaced only
/// when the snapshot for the new CR3 is accepted.
fn handle_set_exit(
    exit: Cpl0ProfileSetExit<crate::BusAccessError>,
    instructions_executed: u64,
    state: &LongModeState,
    efer: &mut u64,
    paging: &mut Long64ProbeConfig,
    reported_cr3: &mut u64,
    changes: &mut Vec<TranslationContextChange>,
) -> Option<PagedCpl0SetExit> {
    match exit {
        Cpl0ProfileSetExit::TranslationContextChange(
            effect @ TranslationContextChange::Cr3Write { old_cr3, new_cr3 },
        ) => {
            *reported_cr3 = new_cr3;
            changes.push(effect);
            if old_cr3 != paging.cr3() || state.cr3 != Some(new_cr3) {
                return Some(PagedCpl0SetExit::Cr3EffectMismatch {
                    active_cr3: paging.cr3(),
                    effect_old_cr3: old_cr3,
                    cpu_cr3: state.cr3,
                    effect_new_cr3: new_cr3,
                });
            }
            match Long64ProbeConfig::from_cpu_state(state, new_cr3, *efer) {
                Ok(candidate) => {
                    *paging = candidate;
                    None
                }
                Err(error) => Some(PagedCpl0SetExit::Cr3SnapshotRejected { effect, error }),
            }
        }
        // S2-25 NXE slice: a retired `WRMSR IA32_EFER` changed NXE. Only that bit of the walker's
        // EFER follows (SCE and the rest of the loop's value stay); the effect is not a CR3 change
        // and is not added to `changes`. A rebuild failure reuses `Cr3SnapshotRejected`, whose
        // `effect` names the EFER write.
        Cpl0ProfileSetExit::TranslationContextChange(
            effect @ TranslationContextChange::EferWrite { old_efer, new_efer },
        ) => {
            const NXE: u64 = 1 << 11;
            if (old_efer ^ *efer) & NXE != 0 || (old_efer ^ new_efer) & NXE == 0 {
                return Some(PagedCpl0SetExit::UnexpectedContextChange { effect });
            }
            let candidate_efer = (*efer & !NXE) | (new_efer & NXE);
            match Long64ProbeConfig::from_cpu_state(state, paging.cr3(), candidate_efer) {
                Ok(candidate) => {
                    *paging = candidate;
                    *efer = candidate_efer;
                    None
                }
                Err(error) => Some(PagedCpl0SetExit::Cr3SnapshotRejected { effect, error }),
            }
        }
        Cpl0ProfileSetExit::TranslationContextChange(effect) => {
            Some(PagedCpl0SetExit::UnexpectedContextChange { effect })
        }
        Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
            if instructions_executed > 0 =>
        {
            None
        }
        Cpl0ProfileSetExit::Run(exit) => Some(PagedCpl0SetExit::Run(exit)),
        Cpl0ProfileSetExit::PortIo(error) => Some(PagedCpl0SetExit::PortIo(error)),
        Cpl0ProfileSetExit::UserEntry(entry) => Some(PagedCpl0SetExit::UserEntry(entry)),
        // S4-39c: the committed `IRETQ` retired and left a CPL3 state; the loop goes on with the
        // CPL3 entry. The exit only exists for a set `with_user_mode`, which is also what
        // lets the loop take the CPL3 branch (otherwise the next boundary refuses the state).
        Cpl0ProfileSetExit::UserModeEntered(_) => None,
        // `Cpl0ProfileSetExit` is `#[non_exhaustive]`: an exit this board does not know
        // cannot be resumed from, so it ends the run as unhandled.
        _ => Some(PagedCpl0SetExit::UnknownSetExit),
    }
}

/// Execute the exact S4-11e AX word-read profile through the uncached RAM walker.
pub fn step_paged_mov_word_read(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    rflags_defined: u64,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PagedMovStepReport, PagedMovStepError> {
    let paging = Long64ProbeConfig::from_cpu_state(state, cr3, efer)
        .map_err(PagedMovStepError::Configuration)?;
    let context = AccessContext {
        origin: od_isa::memory::AccessOrigin::Explicit,
        ..context
    };
    let instruction_rip = state.rip;
    let mut memory = PagedMovMemoryAdapter {
        bus: bus.into(),
        paging,
        instruction_rip,
        committed_write: &mut committed_write,
        page_fault: None,
        memory_error: None,
        access_error: None,
        page_fault_trace: None,
        capture_page_fault_trace: false,
        committed_ad_updates: None,
        committed_writes: None,
        capture_effects: false,
        descriptor_gate: None,
        descriptor_scope: None,
        dword_read_profile: false,
        word_read_profile: true,
        ram_only: false,
        reject_crossing_qword: false,
        read_only_qword: false,
        large_ram_profile: false,
        dword_write_profile: false,
        cpl0_set_dword_read: false,
        cpl0_set_byte_read: false,
        cpl0_set_word_read: false,
        cpl0_set_narrow_write: false,
        cpl0_set_cross_page_fetch: false,
        cpl0_set_cross_page_store: false,
        cpl0_set_cross_page_read: false,
        fetch_cache: None,
        walks: None,
        decoded: DecodedStep::default(),
    };
    let cpu = cpu_step_paged_mov_word_read(
        PagedMovCrossPageWordReadLong64V1,
        state,
        &mut memory,
        context,
        rflags_defined,
    )
    .map_err(PagedMovStepError::CpuInput)?;
    Ok(PagedMovStepReport {
        cpu,
        page_fault: memory.page_fault,
        memory_error: memory.memory_error,
    })
}

/// Execute the exact S4-11f AX word-read profile, including one code-page
/// fetch crossing, through the uncached RAM walker.
pub fn step_paged_mov_word_read_cross_page_fetch(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    rflags_defined: u64,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PagedMovStepReport, PagedMovStepError> {
    let paging = Long64ProbeConfig::from_cpu_state(state, cr3, efer)
        .map_err(PagedMovStepError::Configuration)?;
    if rflags_defined & !od_isa::INTEGER_FLAGS != 0 {
        return Err(PagedMovStepError::CpuInput(
            RunInputError::InvalidRflagsDefined {
                value: rflags_defined,
            },
        ));
    }
    if efer & (1 << 11) == 0 {
        return Ok(PagedMovStepReport {
            cpu: RunResult {
                exit: RunExit::Unsupported { opcode: 0x66 },
                instructions_executed: 0,
                rflags_defined,
            },
            page_fault: None,
            memory_error: None,
        });
    }
    let context = AccessContext {
        origin: od_isa::memory::AccessOrigin::Explicit,
        ..context
    };
    let instruction_rip = state.rip;
    let mut memory = PagedMovMemoryAdapter {
        bus: bus.into(),
        paging,
        instruction_rip,
        committed_write: &mut committed_write,
        page_fault: None,
        memory_error: None,
        access_error: None,
        page_fault_trace: None,
        capture_page_fault_trace: false,
        committed_ad_updates: None,
        committed_writes: None,
        capture_effects: false,
        descriptor_gate: None,
        descriptor_scope: None,
        dword_read_profile: false,
        word_read_profile: true,
        ram_only: false,
        reject_crossing_qword: false,
        read_only_qword: false,
        large_ram_profile: false,
        dword_write_profile: false,
        cpl0_set_dword_read: false,
        cpl0_set_byte_read: false,
        cpl0_set_word_read: false,
        cpl0_set_narrow_write: false,
        cpl0_set_cross_page_fetch: false,
        cpl0_set_cross_page_store: false,
        cpl0_set_cross_page_read: false,
        fetch_cache: None,
        walks: None,
        decoded: DecodedStep::default(),
    };
    let cpu = cpu_step_paged_mov_word_read_cross_page_fetch(
        PagedMovCrossPageWordFetchLong64V1,
        state,
        &mut memory,
        context,
        rflags_defined,
    )
    .map_err(PagedMovStepError::CpuInput)?;
    Ok(PagedMovStepReport {
        cpu,
        page_fault: memory.page_fault,
        memory_error: memory.memory_error,
    })
}

/// Execute the exact S4-11c EAX dword-read profile through the same tracked
/// walker as the existing paged MOV bridge.
pub fn step_paged_mov_dword_read(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    rflags_defined: u64,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PagedMovStepReport, PagedMovStepError> {
    let paging = Long64ProbeConfig::from_cpu_state(state, cr3, efer)
        .map_err(PagedMovStepError::Configuration)?;
    let context = AccessContext {
        origin: od_isa::memory::AccessOrigin::Explicit,
        ..context
    };
    let instruction_rip = state.rip;
    let mut memory = PagedMovMemoryAdapter {
        bus: bus.into(),
        paging,
        instruction_rip,
        committed_write: &mut committed_write,
        page_fault: None,
        memory_error: None,
        access_error: None,
        page_fault_trace: None,
        capture_page_fault_trace: false,
        committed_ad_updates: None,
        committed_writes: None,
        capture_effects: false,
        descriptor_gate: None,
        descriptor_scope: None,
        dword_read_profile: true,
        word_read_profile: false,
        ram_only: false,
        reject_crossing_qword: false,
        read_only_qword: false,
        large_ram_profile: false,
        dword_write_profile: false,
        cpl0_set_dword_read: false,
        cpl0_set_byte_read: false,
        cpl0_set_word_read: false,
        cpl0_set_narrow_write: false,
        cpl0_set_cross_page_fetch: false,
        cpl0_set_cross_page_store: false,
        cpl0_set_cross_page_read: false,
        fetch_cache: None,
        walks: None,
        decoded: DecodedStep::default(),
    };
    let cpu = cpu_step_paged_mov_dword_read(
        PagedMovCrossPageDwordReadLong64V1,
        state,
        &mut memory,
        context,
        rflags_defined,
    )
    .map_err(PagedMovStepError::CpuInput)?;
    Ok(PagedMovStepReport {
        cpu,
        page_fault: memory.page_fault,
        memory_error: memory.memory_error,
    })
}

/// Result of one paged dword store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PagedDwordStoreStepReport {
    pub cpu: od_isa::PagedProbeResult<std::convert::Infallible>,
    /// Retains the walker's precise saved RIP, linear address, and PFEC.
    pub page_fault: Option<PageFaultRecord>,
    /// Retains non-architectural walker/backend/MMIO classifications.
    pub memory_error: Option<TrackedAccessError>,
}

/// Execute the exact S4-27h `MOV [RBX],EAX` (`89 03`) dword store through the
/// tracked walker with MMIO dispatch (the EOI store of a paged interrupt
/// handler). Like [`step_paged_mov_dword_read`] it does not inherit the qword
/// profile; the CPU profile owns decode and retirement, this adapter supplies
/// the paging snapshot, one-dword writes, MMIO routing and walker A/D callbacks.
/// The Dirty bit of the leaf is set before an MMIO store is dispatched.
pub fn step_paged_mov_dword_store(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    rflags_defined: u64,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PagedDwordStoreStepReport, crate::ProbeConfigError> {
    let paging = Long64ProbeConfig::from_cpu_state(state, cr3, efer)?;
    let instruction_rip = state.rip;
    let mut memory = PagedMovMemoryAdapter {
        bus: bus.into(),
        paging,
        instruction_rip,
        committed_write: &mut committed_write,
        page_fault: None,
        memory_error: None,
        access_error: None,
        page_fault_trace: None,
        capture_page_fault_trace: false,
        committed_ad_updates: None,
        committed_writes: None,
        capture_effects: false,
        descriptor_gate: None,
        descriptor_scope: None,
        dword_read_profile: false,
        word_read_profile: false,
        ram_only: false,
        reject_crossing_qword: false,
        read_only_qword: false,
        large_ram_profile: false,
        dword_write_profile: true,
        cpl0_set_dword_read: false,
        cpl0_set_byte_read: false,
        cpl0_set_word_read: false,
        cpl0_set_narrow_write: false,
        cpl0_set_cross_page_fetch: false,
        cpl0_set_cross_page_store: false,
        cpl0_set_cross_page_read: false,
        fetch_cache: None,
        walks: None,
        decoded: DecodedStep::default(),
    };
    let cpu = od_isa::step_paged_mov_dword_store(
        od_isa::PagedMovDwordStoreLong64V1,
        state,
        &mut memory,
        context,
        efer,
        rflags_defined,
    );
    Ok(PagedDwordStoreStepReport {
        cpu,
        page_fault: memory.page_fault,
        memory_error: memory.memory_error,
    })
}

/// What the adapter reads and writes through. `Machine` is the whole bus (every profile);
/// `Ram` is the physical memory alone, split off from the bus so that the port half of the same
/// bus can serve `IN`/`OUT` in the same instruction (large-RAM profile only, S4-38L3).
enum AdapterBus<'a> {
    Machine(&'a mut MachineBus, usize),
    /// S6-42b: the whole bus by shared reference, for the RAM-only adapters of the JIT path of a vCPU
    /// thread (`ram_only` profiles never reach a device, so the exclusive device table is not needed).
    /// It serves what `Machine` serves of those adapters, with `&MachineBus`; a path that needs the
    /// device table (`access_tracked`) is a typed error here.
    Shared(&'a MachineBus, usize),
    Ram(&'a crate::PhysicalMemory),
    /// `Ram` plus the MMIO dispatch of the set loop (S4-38L9s): an operand in an MMIO region
    /// goes to the board's device, as on the `Machine` form; the rest is the large-RAM walk.
    /// The last member is the index of the vCPU whose set loop it is (S6-16a).
    RamMmio(
        &'a crate::PhysicalMemory,
        &'a dyn crate::bus::SetStepMmio,
        usize,
    ),
}

impl<'a> From<&'a mut MachineBus> for AdapterBus<'a> {
    fn from(bus: &'a mut MachineBus) -> Self {
        Self::Machine(bus, crate::vcpu::BOOT_VCPU_INDEX)
    }
}

impl<'a> From<&'a MachineBus> for AdapterBus<'a> {
    /// The index is the boot processor's by default: it only decides which vCPU's per-processor
    /// devices (the Local APIC window) an MMIO access reaches (`AdapterBus::vcpu`). The shared
    /// RAM path of the JIT store helper never reaches MMIO (the probe refuses it first), so the
    /// default is harmless there; a caller that needs another vCPU builds `Shared(bus, cpu)`.
    fn from(bus: &'a MachineBus) -> Self {
        Self::Shared(bus, crate::vcpu::BOOT_VCPU_INDEX)
    }
}

impl<'a> AdapterBus<'a> {
    /// The gate of the split `LOCK` of the vCPU thread this adapter belongs to (S6-34 step 8):
    /// only the set loop's form (`RamMmio`) can carry one.
    fn split_gate(&self) -> Option<&'a dyn crate::bus::SplitLockGate> {
        match self {
            Self::RamMmio(_, mmio, _) => {
                let mmio: &'a dyn crate::bus::SetStepMmio = *mmio;
                mmio.split_gate()
            }
            Self::Machine(..) | Self::Shared(..) | Self::Ram(_) => None,
        }
    }
}

impl AdapterBus<'_> {
    /// Index of the vCPU that owns this adapter: its MMIO accesses reach the per-processor
    /// devices (the Local APIC window, S6-11) as that vCPU's. Only the `Machine` form carries
    /// one, and so does the `RamMmio` form of the set loop (S6-16a); the `Ram` form serves the
    /// boot processor's set loop.
    fn vcpu(&self) -> usize {
        match self {
            Self::Machine(_, vcpu) | Self::Shared(_, vcpu) | Self::RamMmio(_, _, vcpu) => *vcpu,
            Self::Ram(_) => crate::vcpu::BOOT_VCPU_INDEX,
        }
    }

    /// The whole bus. The `Ram` form is built only for the large-RAM walk, which never asks
    /// for it; if it ever does, that is a typed backend error and not a panic.
    fn machine(&mut self) -> Result<&mut MachineBus, MemoryError> {
        match self {
            Self::Machine(bus, _) => Ok(bus),
            Self::Ram(_) | Self::RamMmio(..) | Self::Shared(..) => Err(MemoryError::Backend(
                "the RAM-only adapter bus serves the large-RAM walk only".to_string(),
            )),
        }
    }

    /// The whole bus by shared reference (S6-42b): what the RAM-only tracked accesses need
    /// (`access_tracked_ram`, the bounded read), for the exclusive and the shared form alike.
    fn machine_ref(&self) -> Result<&MachineBus, MemoryError> {
        match self {
            Self::Machine(bus, _) => Ok(bus),
            Self::Shared(bus, _) => Ok(bus),
            Self::Ram(_) | Self::RamMmio(..) => Err(MemoryError::Backend(
                "the RAM-only adapter bus serves the large-RAM walk only".to_string(),
            )),
        }
    }

    fn access_2m_ram(
        &mut self,
        paging: &Long64ProbeConfig,
        request: TrackedAccessRequest,
        committed_write: impl FnMut(GuestPhysicalRange),
        mmio_allowed: bool,
    ) -> crate::TrackedAccessReport {
        match self {
            Self::Machine(bus, _) => bus.access_tracked_2m_ram(paging, request, committed_write),
            Self::Shared(bus, _) => bus.access_tracked_2m_ram(paging, request, committed_write),
            Self::Ram(memory) => {
                crate::page_walk::access_long64_2m_ram(memory, paging, request, committed_write)
            }
            // A split access never reaches a device (S4-38L9s): it is the RAM walk, whose MMIO
            // operand is the typed error it always was.
            Self::RamMmio(memory, _, _) if !mmio_allowed => {
                crate::page_walk::access_long64_2m_ram(memory, paging, request, committed_write)
            }
            Self::RamMmio(memory, mmio, _) => crate::page_walk::access_long64_2m_with_mmio(
                memory,
                paging,
                request,
                committed_write,
                mmio.windows(),
                |address, width, operation| mmio.dispatch(address, width, operation),
            ),
        }
    }

    /// The physical memory a locked operation runs on (S6-31), for every form of the bus.
    fn locked_memory(&self) -> &crate::PhysicalMemory {
        match self {
            Self::Machine(bus, _) => bus.physical_memory(),
            Self::Shared(bus, _) => bus.physical_memory(),
            Self::Ram(memory) | Self::RamMmio(memory, _, _) => memory,
        }
    }

    /// The physical memory of the `Ram` forms: a shared reference (S6-30c), so the adapters of
    /// several vCPUs hold the same memory at once.
    fn ram_ref(&self) -> Option<&crate::PhysicalMemory> {
        match self {
            Self::Ram(memory) | Self::RamMmio(memory, _, _) => Some(memory),
            Self::Machine(..) | Self::Shared(..) => None,
        }
    }

    fn read_virtual_ram_diagnostic(
        &mut self,
        paging: &Long64ProbeConfig,
        rip: u64,
        linear: GuestLinearAddress,
        context: AccessContext,
        output: &mut [u8],
    ) -> Result<(), ()> {
        if let Self::Machine(bus, _) = self {
            return bus
                .read_virtual_ram_diagnostic(paging, rip, linear, context, output)
                .map_err(|_| ());
        }
        if let Self::Shared(bus, _) = self {
            return bus
                .read_virtual_ram_diagnostic(paging, rip, linear, context, output)
                .map_err(|_| ());
        }
        let memory = self.ram_ref().ok_or(())?;
        let mut offset = 0usize;
        while offset < output.len() {
            let linear_address = linear
                .get()
                .checked_add(u64::try_from(offset).map_err(|_| ())?)
                .ok_or(())?;
            let bytes_to_page_end = 4096 - (linear_address as usize & 0xfff);
            let fragment_len = bytes_to_page_end.min(output.len() - offset);
            let physical = crate::page_walk::translate_long64_2m_ram_read_only(
                memory,
                paging,
                rip,
                GuestLinearAddress::new(linear_address),
                context,
                AccessKind::Read,
            )
            .map_err(|_| ())?;
            if !memory
                .locate_region_kind(physical, fragment_len as u64)
                .is_ok_and(|kind| kind == MemoryRegionKind::Ram)
            {
                return Err(());
            }
            memory
                .load_bytes(physical, &mut output[offset..offset + fragment_len])
                .map_err(|_| ())?;
            offset += fragment_len;
        }
        Ok(())
    }
}

/// `GuestMemory` view scoped to one instruction attempt; same-page RAM and
/// MMIO backing is routed through `MachineBus`. A crossing qword load resolves
/// both 4 KiB walks and RAM ranges before reading either operand portion; it
/// never splits a crossing access through MMIO.
///
/// A failed read leaves the caller's destination buffer unchanged because the
/// walker result is copied only after a successful complete read. A failed RAM
/// write does not partially commit its operand. Earlier A/D callbacks remain
/// observable after later faults or device errors, and an operand may alias
/// page-table bytes. MMIO device errors are backend errors, not guest page
/// faults, and are not retried; this adapter makes no rollback claim for device
/// effects.
pub struct PagedMovMemoryAdapter<'a> {
    bus: AdapterBus<'a>,
    paging: Long64ProbeConfig,
    instruction_rip: u64,
    committed_write: &'a mut dyn FnMut(GuestPhysicalRange),
    page_fault: Option<PageFaultRecord>,
    memory_error: Option<TrackedAccessError>,
    access_error: Option<TrackedAccessError>,
    page_fault_trace: Option<FaultingPageWalkContext>,
    capture_page_fault_trace: bool,
    committed_ad_updates: Option<Vec<crate::AdUpdate>>,
    committed_writes: Option<Vec<GuestPhysicalRange>>,
    capture_effects: bool,
    descriptor_gate: Option<&'a dyn crate::bus::SplitLockGate>,
    descriptor_scope: Option<DescriptorScope<'a>>,
    dword_read_profile: bool,
    word_read_profile: bool,
    ram_only: bool,
    reject_crossing_qword: bool,
    read_only_qword: bool,
    large_ram_profile: bool,
    /// S4-27h: the only admitted write is one dword (`MOV [RBX],EAX`).
    dword_write_profile: bool,
    /// S4-38L3: the CPL0 profile set (`run_cpl0_set_paged_2m_ram`) also reads dwords
    /// (`MOV r32, m32`), next to its qword reads. The qword-only shared profiles stay
    /// as they were: this flag is set by the set's adapter alone.
    cpl0_set_dword_read: bool,
    /// The CPL0 set's adapter alone reads one byte (S4-38L4a; the first consumer is
    /// `CMP byte [mem], imm8`, any other byte-reading form the set admits uses the same read).
    /// The shared profiles keep their read widths. Reads never cross a page for one byte.
    cpl0_set_byte_read: bool,
    /// S4-38L4f: the CPL0 profile set's adapter alone reads one word (`MOVZX r32, m16`, and any
    /// other word-reading form the set admits). The word-read probe profile
    /// (`word_read_profile`) and every shared profile keep their own read widths. A word on the
    /// last byte of a 4 KiB page is read as two one-byte accesses, each translated on its own
    /// (accessed bit per page, no dirty bit); a missing second page is a precise data `#PF` at
    /// the first byte on it. Accessed bits set by the first byte stay set after that fault.
    cpl0_set_word_read: bool,
    /// S4-38L3: the CPL0 profile set also stores bytes, words and dwords
    /// (`MOV m8, r8`, `MOV m16, r16`, `MOV m32, r32`), next to its qword stores. Each store
    /// writes exactly its own width (no read-modify-write of the surrounding qword). The
    /// shared profiles keep their fixed store width: this flag is set by the set's adapter alone.
    cpl0_set_narrow_write: bool,
    /// S4-38L4e: the CPL0 profile set's adapter alone lets the instruction fetch continue onto
    /// the next 4 KiB page. The decoder fetches one byte at a time and each byte is translated
    /// on its own, so a page-crossing instruction sees the two pages as a real CPU does: a
    /// missing second page is a precise instruction-fetch `#PF` at the first byte on it, with
    /// the RIP of the instruction. The shared large-RAM profile keeps its one-code-page window.
    cpl0_set_cross_page_fetch: bool,
    /// S4-38L4h: the CPL0 profile set's adapter alone performs a store that crosses a 4 KiB
    /// boundary (word, dword or qword at a page tail). Both pages are translated for a write
    /// first (present, reserved, user and write-protection checks, accessed bits set, no dirty
    /// bit); only if both pass are the bytes written, each through its own write access (the
    /// dirty bit of each touched leaf is set). A fault on either page is a precise data `#PF`
    /// at the first linear address on the faulting page with the write bit set, and under the
    /// ordinary conditions (the tables are not in ROM, regions are whole frames, the operand
    /// does not write the page tables of the second walk) not one byte has been stored.
    /// Known limits, where this is NOT guaranteed: the operand itself writes into the page
    /// tables on the path of the second page's walk; a table lies in ROM and its leaf is already
    /// on the second page; a region boundary falls inside a frame (only the first byte on each
    /// page is checked). A later slice may translate once and write by the physical addresses
    /// of the check. Elsewhere a crossing store stays the typed `CrossesPage`.
    cpl0_set_cross_page_store: bool,
    /// S4-38L5e: the CPL0 profile set's adapter alone performs a dword or qword read that
    /// crosses a 4 KiB boundary (`MOV r64, m64` / `MOV r32, m32` at a page tail). Each byte is
    /// its own one-byte read access with its own translation (accessed bit on the leaf of each
    /// page, never a dirty bit; present, reserved, user and NX-irrelevant read checks per page);
    /// the bytes are assembled little-endian (the byte at the operand address is the lowest) and
    /// handed to the destination only after every byte was read, so a fault leaves the
    /// destination untouched. A fault on a page is a precise data `#PF` with W = 0 at the first
    /// linear address on the faulting page (the operand address when it is the first page).
    /// Accessed bits set by earlier bytes stay set after the fault. Elsewhere (and for a word,
    /// see `cpl0_set_word_read`) a crossing read keeps its previous behaviour (the typed
    /// `CrossesPage`). The flag is `true` for every board that runs the set through
    /// `run_cpl0_set_paged_2m_ram_with_cpuid_boundary`: the Linux board and the firmware board
    /// (`firmware_board.rs`), and it covers every explicit dword/qword read of the set (`MOV`,
    /// `POP`, the `IRETQ` frame, ALU source operands, the read of an RMW).
    cpl0_set_cross_page_read: bool,
    /// S4-13: the CPL0 set loop's opt-in translation cache of the instruction fetch (see
    /// `fetch_translation_cache`). Used by `fetch` only, and only with the set's cross-page
    /// fetch; `None` everywhere else, which keeps the byte-per-walk behaviour.
    fetch_cache: Option<&'a FetchTranslationCache>,
    /// S4-13b: counts the tracked walks of this adapter (measurement only); `None` outside the
    /// set loop's instruction adapter.
    walks: Option<&'a WalkCounter>,
    /// PERF-10: the decoded-instruction cache of this adapter's instruction (off unless the set loop
    /// turns it on; see [`DecodedStep`]).
    decoded: DecodedStep<'a>,
}

/// PERF-10: the state of the decoded-instruction cache for the one instruction an adapter serves.
/// `cache` is `Some` only when the set loop of an ordinary run hands it over; every other adapter
/// has `DecodedStep::default()`, which keeps the byte-per-fetch decoding.
#[derive(Default)]
struct DecodedStep<'a> {
    cache: Option<&'a DecodedInstructionCache>,
    /// Set by a miss of `cached_instruction` whose gate and translation conditions held: the key a
    /// successful decode of the same RIP and context may be remembered under.
    pending: Option<DecodedPending>,
    /// A fetch of the current decode left the 4 KiB page of the RIP (a lookahead, a cross-page
    /// instruction): the decode is not remembered.
    outside_page: bool,
}

#[derive(Clone, Copy)]
struct DecodedPending {
    rip: u64,
    context: AccessContext,
    page: crate::physical_memory::RamCodePage,
}

fn faulting_page_walk_context(
    error: &TrackedAccessError,
    trace: crate::WalkTrace,
) -> Option<FaultingPageWalkContext> {
    let TrackedAccessError::PageFault { reason, .. } = error else {
        return None;
    };
    let entries = trace
        .entries()
        .iter()
        .take(trace.len().min(crate::LINUX_PAGE_FAULT_WALK_ENTRY_CAPACITY))
        .flatten()
        .copied()
        .collect();
    Some(FaultingPageWalkContext {
        entries,
        reason: *reason,
    })
}

fn post_iret_context_matches(pending: &PendingPostIretObservation, state: &LongModeState) -> bool {
    let cpl = state.cs_selector.map_or(0, |selector| (selector & 3) as u8);
    state.rip == pending.fault_rip
        && state.cs_selector == pending.fault_cs
        && cpl == pending.fault_cpl
        && state.gpr[od_isa::Register::Rsp as usize] == pending.fault_rsp
}

impl<'a> PagedMovMemoryAdapter<'a> {
    fn capture_post_iret_page_walk(
        &mut self,
        pending: PendingPostIretObservation,
        state: &LongModeState,
        cr3: u64,
    ) -> PostIretPageWalkSnapshot {
        let context = AccessContext {
            mode: InstructionMode::Long64,
            privilege: if pending.fault_cpl == 3 {
                PrivilegeLevel::Ring3
            } else {
                PrivilegeLevel::Ring0
            },
            origin: AccessOrigin::Explicit,
        };
        let access = match pending.fault_access.as_str() {
            "Read" => AccessKind::Read,
            "Write" => AccessKind::Write,
            "Fetch" => AccessKind::Fetch,
            _ => AccessKind::Read,
        };
        let (translation, trace) = match &mut self.bus {
            AdapterBus::Ram(memory) | AdapterBus::RamMmio(memory, _, _) => {
                translate_long64_2m_ram_read_only_with_trace(
                    memory,
                    &self.paging,
                    pending.fault_rip,
                    GuestLinearAddress::new(pending.fault_cr2),
                    context,
                    access,
                )
            }
            AdapterBus::Shared(bus, _) => translate_long64_2m_ram_read_only_with_trace(
                bus.physical_memory(),
                &self.paging,
                pending.fault_rip,
                GuestLinearAddress::new(pending.fault_cr2),
                context,
                access,
            ),
            AdapterBus::Machine(bus, _) => {
                let (memory, _) = bus.split_for_pio_step();
                translate_long64_2m_ram_read_only_with_trace(
                    memory,
                    &self.paging,
                    pending.fault_rip,
                    GuestLinearAddress::new(pending.fault_cr2),
                    context,
                    access,
                )
            }
        };
        let entries = trace.entries().iter().flatten().copied().collect();
        let (translation_status, translated_physical, translation_error) = match translation {
            Ok((physical, _)) => ("translated", Some(physical.get()), None),
            Err(TrackedAccessError::PageFault { record, reason }) => (
                "page_fault",
                None,
                Some(format!("record={record:?}; reason={reason:?}")),
            ),
            Err(error) => ("error", None, Some(format!("{error:?}"))),
        };
        PostIretPageWalkSnapshot {
            event_ordinal: pending.event_ordinal,
            fault_rip: pending.fault_rip,
            fault_cr2: pending.fault_cr2,
            fault_access: pending.fault_access,
            fault_cpl: pending.fault_cpl,
            post_iret_rip: state.rip,
            post_iret_cs: state.cs_selector,
            post_iret_cpl: state.cs_selector.map_or(0, |selector| (selector & 3) as u8),
            post_iret_rsp: state.gpr[od_isa::Register::Rsp as usize],
            post_iret_cr3: cr3,
            entries,
            translation_status,
            translated_physical,
            translation_error,
        }
    }

    fn capture_fault_frame_chain(
        &mut self,
        rip: u64,
        rbp: u64,
    ) -> (Vec<crate::ConsoleContextFrame>, crate::FrameChainEnd) {
        let context = AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        };
        let paging = self.paging;
        capture_frame_chain(rbp, |frame_pointer| {
            let mut bytes = [0; 16];
            self.bus
                .read_virtual_ram_diagnostic(
                    &paging,
                    rip,
                    GuestLinearAddress::new(frame_pointer),
                    context,
                    &mut bytes,
                )
                .map(|()| bytes)
        })
    }

    fn access(
        &mut self,
        address: u64,
        context: AccessContext,
        operation: RamAccessOperation,
    ) -> Result<TrackedAccessOutcome, MemoryError> {
        self.access_with(address, context, operation, true)
    }

    fn supports_cpl3_vector(&self, context: AccessContext) -> bool {
        self.large_ram_profile
            && self.cpl0_set_cross_page_read
            && self.cpl0_set_cross_page_store
            && !self.capture_effects
            && context.privilege == PrivilegeLevel::Ring3
            && context.origin == AccessOrigin::Explicit
    }

    fn access_vector(
        &mut self,
        address: u64,
        context: AccessContext,
        access: VectorRamAccess<'_>,
    ) -> Result<(), MemoryError> {
        let memory = match &self.bus {
            AdapterBus::Ram(memory) | AdapterBus::RamMmio(memory, _, _) => *memory,
            AdapterBus::Machine(bus, _) => bus.physical_memory(),
            AdapterBus::Shared(bus, _) => bus.physical_memory(),
        };
        let mut trace = None;
        let result = access_long64_2m_ram_vector(
            memory,
            &self.paging,
            self.instruction_rip,
            GuestLinearAddress::new(address),
            context,
            access,
            &mut *self.committed_write,
            |kind| {
                if let Some(walks) = self.walks {
                    match kind {
                        AccessKind::Write => walks.note_write(),
                        _ => walks.note_read(),
                    }
                }
            },
            &mut trace,
        );
        if self.capture_page_fault_trace {
            self.page_fault_trace = match (trace, &result) {
                (Some(trace), Err(error)) => faulting_page_walk_context(error, trace),
                _ => None,
            };
        }
        result.map_err(|error| {
            self.access_error = Some(error.clone());
            self.map_access_error(error)
        })
    }

    /// One byte of an access that is split at a page boundary: never served by a device
    /// (S4-38L9s), so a split access that touches MMIO is the typed error it was before.
    fn access_split_byte(
        &mut self,
        address: u64,
        context: AccessContext,
        operation: RamAccessOperation,
    ) -> Result<TrackedAccessOutcome, MemoryError> {
        self.access_with(address, context, operation, false)
    }

    fn access_with(
        &mut self,
        address: u64,
        context: AccessContext,
        operation: RamAccessOperation,
        mmio_allowed: bool,
    ) -> Result<TrackedAccessOutcome, MemoryError> {
        if self.capture_page_fault_trace {
            self.page_fault_trace = None;
        }
        // S4-39d: the walker takes the CPL of the context; the origin is normalized to `Explicit`
        // as before, but a descriptor-table access (IDT, GDT, TSS) keeps its supervisor page
        // privilege: at CPL3 it is made with CPL0 page rights (also under PTI, where these tables
        // are supervisor pages of the user page tables) and its fault has U/S = 0. At CPL0 and for
        // every other origin `page_privilege()` is the CPL, so nothing changes there.
        let context = AccessContext {
            origin: od_isa::memory::AccessOrigin::Explicit,
            privilege: context.page_privilege(),
            ..context
        };
        let request = TrackedAccessRequest {
            instruction_rip: self.instruction_rip,
            address: GuestLinearAddress::new(address),
            context,
            operation,
        };
        if let Some(walks) = self.walks {
            match operation {
                RamAccessOperation::Fetch => walks.note_fetch(),
                RamAccessOperation::Read { .. } | RamAccessOperation::ReadPseudoDescriptor => {
                    walks.note_read()
                }
                RamAccessOperation::Write { .. } | RamAccessOperation::ReadForWrite { .. } => {
                    walks.note_write()
                }
                // S6-31: the two walks of the serial read-then-write pair, counted as before.
                RamAccessOperation::AtomicTarget { .. } => {
                    walks.note_read();
                    walks.note_write();
                }
            }
        }
        let vcpu = self.bus.vcpu();
        let report = if self.capture_effects {
            // A descriptor transaction only owns RAM. A tracked walk can fault on MMIO but
            // must never dispatch a device access while the other vCPUs are parked.
            let mmio_allowed = mmio_allowed && self.descriptor_scope.is_none();
            let mut writes = Vec::new();
            let report = if self.large_ram_profile {
                self.bus.access_2m_ram(
                    &self.paging,
                    request,
                    |range| writes.push(range),
                    mmio_allowed,
                )
            } else if self.ram_only {
                self.bus
                    .machine_ref()?
                    .access_tracked_ram(&self.paging, request, |range| writes.push(range))
            } else {
                self.bus
                    .machine()?
                    .access_tracked(vcpu, &self.paging, request, |range| writes.push(range))
            };
            self.committed_writes
                .as_mut()
                .expect("capture mode has a write collector")
                .extend(writes);
            self.committed_ad_updates
                .as_mut()
                .expect("capture mode has an A/D collector")
                .extend(
                    report.ad_updates[..report.ad_update_count]
                        .iter()
                        .flatten()
                        .copied(),
                );
            report
        } else if self.large_ram_profile {
            self.bus.access_2m_ram(
                &self.paging,
                request,
                &mut *self.committed_write,
                mmio_allowed,
            )
        } else if self.ram_only {
            self.bus.machine_ref()?.access_tracked_ram(
                &self.paging,
                request,
                &mut *self.committed_write,
            )
        } else {
            self.bus.machine()?.access_tracked(
                vcpu,
                &self.paging,
                request,
                &mut *self.committed_write,
            )
        };
        if self.capture_page_fault_trace {
            self.page_fault_trace = report
                .outcome
                .as_ref()
                .err()
                .and_then(|error| faulting_page_walk_context(error, report.trace));
        }
        match report.outcome {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                self.access_error = Some(error.clone());
                Err(self.map_access_error(error))
            }
        }
    }

    /// S4-13: one instruction byte through the fetch translation cache. A hit reads the byte at
    /// the cached physical address and walks nothing. A miss is the ordinary tracked fetch (full
    /// walk, accessed bits, faults and their records exactly as without the cache); only after it
    /// succeeded is the translation remembered, from a second walk that writes nothing and reads
    /// the same tables (the byte read does not change them).
    fn fetch_through_cache(
        &mut self,
        cache: &FetchTranslationCache,
        address: u64,
        context: AccessContext,
    ) -> Result<u8, MemoryError> {
        let context = AccessContext {
            origin: od_isa::memory::AccessOrigin::Explicit,
            ..context
        };
        if let Some(frame) = cache.lookup_fetch(address, &self.paging, context)
            && let Some(memory) = self.bus.ram_ref()
            && let Some(byte) = memory.read_ram_frame_u8(frame, address)
        {
            return Ok(byte);
        }
        let byte = match self.access(address, context, RamAccessOperation::Fetch)? {
            TrackedAccessOutcome::Fetched(value) => value,
            _ => return Err(Self::unsupported("non-fetch result")),
        };
        self.remember_fetch_translation(cache, address, context);
        Ok(byte)
    }

    /// S4-13 / S6-34 step 9: remember the translation of a fetch that just succeeded. For the
    /// cache of a vCPU thread of the parallel mode (`FetchTranslationCache::is_strict`) three
    /// steps, in this order, so that the generation of every table frame is read BEFORE the walk
    /// whose bytes make the entry (the reader's rule of S6-30b): (1) a read-only walk names the
    /// table frames; (2) the page generation of each is read; (3) a second read-only walk makes the
    /// entry, and the entry is kept only if it walked the same frames. A write to a table frame by
    /// another vCPU after (2) leaves the stored generation older than the page, and the next
    /// `seal`/`revalidate` drops the entry. Any other cache takes the one walk of (3) and reads the
    /// generations at the seal. The walks write nothing.
    fn remember_fetch_translation(
        &self,
        cache: &FetchTranslationCache,
        address: u64,
        context: AccessContext,
    ) {
        let Some(memory) = self.bus.ram_ref() else {
            return;
        };
        let walk = |memory: &crate::PhysicalMemory| {
            translate_long64_2m_ram_read_only_with_tables(
                memory,
                &self.paging,
                self.instruction_rip,
                GuestLinearAddress::new(address),
                context,
                od_isa::memory::AccessKind::Fetch,
            )
        };
        // A cache that is not strict reads the generations at the seal and needs the one walk of
        // (3): the order of the interleaved mode, as before S6-34.
        let strict = cache.is_strict();
        let (first_tables, generations) = if strict {
            let Ok((_, first_tables)) = walk(memory) else {
                return;
            };
            let generations = first_tables
                .map(|table| table.and_then(|base| memory.code_page_generation(base).ok()));
            (first_tables, generations)
        } else {
            ([None; 4], [None; 4])
        };
        let Ok((physical, tables)) = walk(memory) else {
            return;
        };
        if strict && tables != first_tables {
            return;
        }
        // Only plain RAM is cached: a write to it always notifies the cache, and the byte of
        // the frame reads exactly as the walk's fetch read it.
        let is_ram = |base: u64| {
            matches!(
                memory.locate_region_kind(GuestPhysicalAddress::new(base), 0x1000),
                Ok(MemoryRegionKind::Ram)
            )
        };
        let tables = tables.map(|table| table.map(GuestPhysicalAddress::get));
        // S4-13d: with the 2 MiB tag a walk that ended at a 2 MiB leaf (three tables) whose
        // whole 2 MiB span is plain RAM is remembered for the whole leaf; the span's base is
        // the physical address minus the offset of the request inside the leaf. Anything else
        // (a 4 KiB leaf, a span that is not wholly RAM, a base that is not 2 MiB aligned)
        // stays a 4 KiB entry, as without the tag.
        let leaf_frame =
            (cache.tag() == FetchCacheTag::Leaf2M && tables[2].is_some() && tables[3].is_none())
                .then(|| physical.get().wrapping_sub(address & 0x1f_ffff))
                .and_then(|base| memory.ram_leaf_2m(GuestPhysicalAddress::new(base)));
        if let Some(frame) = leaf_frame
            .or_else(|| memory.ram_frame(GuestPhysicalAddress::new(physical.get() & !0xfff)))
            && tables.iter().flatten().all(|table| is_ram(*table & !0xfff))
        {
            cache.insert_with_generations(
                address,
                &self.paging,
                context,
                frame,
                tables,
                generations,
            );
        }
    }

    /// PERF-10: the decoded instruction at `rip`, when every condition of the cache holds, else
    /// `None` (and then, if the gate and the translation conditions held, a pending key that
    /// [`Self::decoded_cache_remember`] turns into an entry after a successful decode).
    ///
    /// The conditions, all at once: (0) the gate, the one that opens the fetch cache path of the
    /// set loop (a fetch cache with the write-watch policy and no sweep, the cross-page fetch and
    /// the large-RAM profile, no capture of effects, no read-only qword) and the RIP is the one of
    /// the instruction this adapter serves; (a) the fetch translation cache holds the translation of
    /// the RIP's page for this paging snapshot and context (a peek: the rights of the fetch, U/S,
    /// SMEP and the access bits are those of the byte fetch that made the entry); (b) the cached
    /// entry has this RIP, this physical place and this context; (c) the generation of the code page
    /// equals the stored one. The generation is read here, before any byte of this decode.
    fn decoded_cache_lookup(&mut self, rip: u64, context: AccessContext) -> Option<Instruction> {
        self.decoded.pending = None;
        self.decoded.outside_page = false;
        let decoded = self.decoded.cache?;
        let fetch_cache = self.fetch_cache?;
        if !(self.cpl0_set_cross_page_fetch
            && self.large_ram_profile
            && !self.capture_effects
            && !self.read_only_qword
            && fetch_cache.invalidation() == FetchCacheInvalidation::WriteWatch
            && !fetch_cache.sweep_active()
            && rip == self.instruction_rip)
        {
            return None;
        }
        let context = AccessContext {
            origin: od_isa::memory::AccessOrigin::Explicit,
            ..context
        };
        let frame = fetch_cache.lookup(rip, &self.paging, context)?;
        let page = self.bus.ram_ref()?.ram_frame_code_page(frame, rip)?;
        if let Some(instruction) = decoded.lookup(rip, context, page) {
            return Some(instruction);
        }
        self.decoded.pending = Some(DecodedPending { rip, context, page });
        None
    }

    /// PERF-10: stores a successful decode under the pending key of [`Self::decoded_cache_lookup`],
    /// only if that key is for this RIP and context and no byte of the decode was fetched off the page
    /// of the RIP.
    fn decoded_cache_remember(
        &mut self,
        rip: u64,
        context: AccessContext,
        instruction: Instruction,
    ) {
        let Some(pending) = self.decoded.pending.take() else {
            return;
        };
        let context = AccessContext {
            origin: od_isa::memory::AccessOrigin::Explicit,
            ..context
        };
        if self.decoded.outside_page || pending.rip != rip || pending.context != context {
            return;
        }
        if let Some(cache) = self.decoded.cache {
            cache.insert(rip, context, pending.page, instruction);
        }
    }

    fn access_cross_page_read(
        &mut self,
        address: u64,
        context: AccessContext,
        width: crate::AccessWidth,
    ) -> Result<u64, MemoryError> {
        if self.capture_page_fault_trace {
            self.page_fault_trace = None;
        }
        let context = AccessContext {
            origin: od_isa::memory::AccessOrigin::Explicit,
            ..context
        };
        let request = TrackedAccessRequest {
            instruction_rip: self.instruction_rip,
            address: GuestLinearAddress::new(address),
            context,
            operation: RamAccessOperation::Read { width },
        };
        if self.capture_page_fault_trace {
            self.page_fault_trace = None;
            let mut trace = None;
            let result = self
                .bus
                .machine_ref()?
                .read_tracked_bounded_ram_read_with_fault_trace(
                    &self.paging,
                    request,
                    &mut *self.committed_write,
                    &mut trace,
                );
            if let (Some(trace), Err(error)) = (trace, &result) {
                self.page_fault_trace = faulting_page_walk_context(error, trace);
            }
            return result.map_err(|error| self.map_access_error(error));
        }
        let result = self.bus.machine_ref()?.read_tracked_bounded_ram_read(
            &self.paging,
            request,
            &mut *self.committed_write,
        );
        result.map_err(|error| self.map_access_error(error))
    }

    /// S4-38L4h: a store across a 4 KiB boundary. Both pages are checked for a write before any
    /// byte is stored (so under ordinary conditions there is no partial store; the known limits
    /// are listed at `cpl0_set_cross_page_store`); then each byte is its own one-byte write
    /// access, which sets the dirty bit of the leaf it lands on and stores it.
    fn write_crossing_page(
        &mut self,
        address: u64,
        context: AccessContext,
        source: &[u8],
    ) -> Result<(), MemoryError> {
        if self.capture_page_fault_trace {
            self.page_fault_trace = None;
        }
        let context = AccessContext {
            origin: od_isa::memory::AccessOrigin::Explicit,
            ..context
        };
        let second_page = (address & !0xfff).wrapping_add(0x1000);
        for check_address in [address, second_page] {
            let mut trace = None;
            let result = match &mut self.bus {
                AdapterBus::Ram(memory) | AdapterBus::RamMmio(memory, _, _) => {
                    if self.capture_page_fault_trace {
                        crate::page_walk::translate_long64_2m_ram_write_check_with_fault_trace(
                            memory,
                            &self.paging,
                            self.instruction_rip,
                            GuestLinearAddress::new(check_address),
                            context,
                            &mut *self.committed_write,
                            Some(&mut trace),
                        )
                    } else {
                        crate::page_walk::translate_long64_2m_ram_write_check(
                            memory,
                            &self.paging,
                            self.instruction_rip,
                            GuestLinearAddress::new(check_address),
                            context,
                            &mut *self.committed_write,
                        )
                    }
                }
                AdapterBus::Machine(..) | AdapterBus::Shared(..) => {
                    return Err(Self::unsupported("cross-page write"));
                }
            };
            if let Err(error) = result {
                if let Some(trace) = trace {
                    self.page_fault_trace = faulting_page_walk_context(&error, trace);
                }
                self.access_error = Some(error.clone());
                return Err(self.map_access_error(error));
            }
        }
        for (index, byte) in source.iter().enumerate() {
            let outcome = self.access_split_byte(
                address.wrapping_add(index as u64),
                context,
                RamAccessOperation::Write {
                    width: crate::AccessWidth::Byte,
                    value: u64::from(*byte),
                },
            )?;
            if outcome != TrackedAccessOutcome::Written {
                return Err(Self::unsupported("non-write result"));
            }
        }
        Ok(())
    }

    /// S6-31: the width of a locked operation of `size` under this adapter's profile: the widths
    /// the ordinary read and the ordinary write of the same adapter admit (a locked operation
    /// replaces that pair), no other.
    fn locked_width(&self, size: od_isa::OperandSize) -> Result<crate::AccessWidth, MemoryError> {
        use crate::AccessWidth;
        // The ordinary read admits (as in `read`): byte with the set's byte read, word with a word
        // profile, dword with a dword profile, qword only without the narrow-read profiles.
        // The ordinary write admits (as in `write`): only a dword with the dword-store profile;
        // 1, 2, 4 and 8 bytes with the narrow-store set; only 8 bytes otherwise.
        let (width, readable, writable) = match size {
            od_isa::OperandSize::Byte => (
                AccessWidth::Byte,
                self.cpl0_set_byte_read,
                self.cpl0_set_narrow_write && !self.dword_write_profile,
            ),
            od_isa::OperandSize::Word => (
                AccessWidth::Word,
                self.word_read_profile || self.cpl0_set_word_read,
                self.cpl0_set_narrow_write && !self.dword_write_profile,
            ),
            od_isa::OperandSize::Dword => (
                AccessWidth::Dword,
                self.dword_read_profile || self.cpl0_set_dword_read,
                self.dword_write_profile || self.cpl0_set_narrow_write,
            ),
            od_isa::OperandSize::Qword => (
                AccessWidth::Qword,
                !self.dword_read_profile && !self.word_read_profile,
                !self.dword_write_profile,
            ),
        };
        if self.read_only_qword {
            return Err(Self::unsupported("locked operation"));
        }
        if !readable {
            return Err(Self::unsupported("read width"));
        }
        if !writable {
            return Err(Self::unsupported("write width"));
        }
        Ok(width)
    }

    /// S6-31: the translation half of a locked operation. A naturally aligned operand: walk like
    /// the serial read and write pair did and return the physical address of the operand (RAM or
    /// framebuffer; MMIO, ROM and holes are refused); the host atomic operation follows in
    /// [`Self::locked_finish`]. A misaligned operand (S6-32, the return condition of S6-31): in
    /// the interleaving mode it is [`LockedTarget::Serial`], and the caller runs the S6-20 pair
    /// (ordinary read, ordinary write: nothing else runs between them, the scheduler switches only
    /// on instruction boundaries), no counter; in the parallel mode it is refused and counted
    /// before any walk (no accessed or dirty bit set), until the stop-all of S6-34 step 8.
    fn locked_target(
        &mut self,
        address: u64,
        context: AccessContext,
        size: od_isa::OperandSize,
    ) -> Result<LockedTarget<'a>, MemoryError> {
        let width = self.locked_width(size)?;
        if address % u64::from(width.bytes()) != 0 {
            if self.bus.locked_memory().execution_mode() == crate::ExecutionMode::Interleaved {
                return Ok(LockedTarget::Serial);
            }
            // S6-34 step 8: a vCPU thread of the parallel mode carries the gate that stops every
            // other vCPU around the operation; without one the refusal of S6-31 stays.
            if let Some(gate) = self.bus.split_gate() {
                return Ok(LockedTarget::Stopped(gate));
            }
            self.bus.locked_memory().count_split_lock_refusal();
            return Err(MemoryError::Backend(format!(
                "locked operation of {} bytes at {address:#x} is not naturally aligned: a split \
                 lock is refused in the parallel mode without the stop-all gate of the vCPU \
                 threads (S6-31: no 16-byte primitive; S6-34 step 8)",
                width.bytes()
            )));
        }
        match self.access(address, context, RamAccessOperation::AtomicTarget { width })? {
            TrackedAccessOutcome::Located(physical) => Ok(LockedTarget::Host { width, physical }),
            _ => Err(Self::unsupported("non-locate result")),
        }
    }

    /// S6-34 step 8: the split `LOCK` of the parallel mode. [`SplitLockGate::acquire`] returns
    /// when every other vCPU is quiet and no visible effect of this instruction has happened yet;
    /// the serial pair of S6-32 then runs with nobody else touching RAM; the gate is released
    /// when the operation is over, whatever its result. If the gate cannot be had in time the
    /// instruction fails before any effect (a typed backend error, never a half operation).
    fn locked_stopped(
        &mut self,
        gate: &dyn crate::bus::SplitLockGate,
        address: u64,
        context: AccessContext,
        size: od_isa::OperandSize,
        update: &mut dyn FnMut(u64) -> u64,
    ) -> Result<u64, MemoryError> {
        struct Release<'g>(&'g dyn crate::bus::SplitLockGate);
        impl Drop for Release<'_> {
            fn drop(&mut self) {
                self.0.release();
            }
        }
        gate.acquire()
            .map_err(|error| MemoryError::Backend(format!("split lock gate: {error}")))?;
        let _release = Release(gate);
        self.locked_serial(address, context, size, update)
    }

    /// The operation of a locked operand that no host atomic covers: the serial pair of the
    /// interleaving mode, or the same pair under the gate of the parallel mode.
    fn locked_split(
        &mut self,
        target: LockedTarget<'_>,
        address: u64,
        context: AccessContext,
        size: od_isa::OperandSize,
        update: &mut dyn FnMut(u64) -> u64,
    ) -> Result<u64, MemoryError> {
        match target {
            LockedTarget::Stopped(gate) => {
                self.locked_stopped(gate, address, context, size, update)
            }
            LockedTarget::Serial | LockedTarget::Host { .. } => {
                self.locked_serial(address, context, size, update)
            }
        }
    }

    /// S6-32: the serial split `LOCK` of the interleaving mode: the body of the default
    /// [`GuestMemory::atomic_rmw_integer`] (an ordinary read, `update`, an ordinary write; the
    /// write is always performed, also when the value is unchanged), through this adapter's own
    /// read and write, so a misaligned or page-crossing operand takes the same walks, accessed and
    /// dirty bits and notifications as before S6-31.
    fn locked_serial(
        &mut self,
        address: u64,
        context: AccessContext,
        size: od_isa::OperandSize,
        update: &mut dyn FnMut(u64) -> u64,
    ) -> Result<u64, MemoryError> {
        let old = self.read_integer(address, context, size)?;
        let new = update(old);
        self.write_integer(address, context, size, new)?;
        Ok(old)
    }

    /// S6-31: runs the host atomic operation of a locked access on the physical address from
    /// [`Self::locked_target`]. The committed-write notification of the operation goes where the
    /// walk's own go (the capture list or the callback of this adapter), after the data and the
    /// generations; a memory error is the typed operand error of the tracked access.
    fn locked_finish<R>(
        &mut self,
        physical: GuestPhysicalAddress,
        operation: impl FnOnce(
            &crate::PhysicalMemory,
            &mut dyn FnMut(GuestPhysicalRange),
        ) -> Result<R, crate::MemoryAccessError>,
    ) -> Result<R, MemoryError> {
        let mut written = None;
        let result = operation(self.bus.locked_memory(), &mut |range| written = Some(range));
        if let Some(range) = written {
            match self.committed_writes.as_mut() {
                Some(collector) if self.capture_effects => collector.push(range),
                _ => (self.committed_write)(range),
            }
        }
        result.map_err(|source| {
            let error = TrackedAccessError::OperandPhysical {
                address: physical,
                source,
            };
            self.access_error = Some(error.clone());
            self.map_access_error(error)
        })
    }

    fn map_access_error(&mut self, error: TrackedAccessError) -> MemoryError {
        match error {
            TrackedAccessError::PageFault { record, .. } => {
                self.page_fault = Some(record.clone());
                MemoryError::Architectural(record.fault().clone())
            }
            error @ TrackedAccessError::MmioBackend { .. } => {
                let message = format!("tracked Long64 MMIO access failed: {error:?}");
                self.memory_error = Some(error);
                MemoryError::Backend(message)
            }
            error => {
                let message = format!("tracked Long64 RAM access failed: {error:?}");
                self.memory_error = Some(error);
                MemoryError::Backend(message)
            }
        }
    }

    fn unsupported(operation: &'static str) -> MemoryError {
        MemoryError::Backend(format!("paged MOV profile does not support {operation}"))
    }
}

/// S6-32: what [`PagedMovMemoryAdapter::locked_target`] decided for a locked operand.
#[derive(Clone, Copy)]
enum LockedTarget<'g> {
    /// Naturally aligned: one host atomic operation on the cell at `physical`.
    Host {
        width: crate::AccessWidth,
        physical: GuestPhysicalAddress,
    },
    /// Misaligned in the interleaving mode: the serial read-update-write pair.
    Serial,
    /// Misaligned in the parallel mode on a vCPU thread: the serial pair under the gate that
    /// stops every other vCPU (S6-34 step 8).
    Stopped(&'g dyn crate::bus::SplitLockGate),
}

impl<'a> GuestMemory for PagedMovMemoryAdapter<'a> {
    fn cached_instruction(&mut self, rip: u64, context: AccessContext) -> Option<Instruction> {
        self.decoded_cache_lookup(rip, context)
    }

    fn remember_instruction(&mut self, rip: u64, context: AccessContext, instruction: Instruction) {
        self.decoded_cache_remember(rip, context, instruction);
    }

    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        // PERF-10: a byte off the page of the RIP (a lookahead, a page-crossing instruction)
        // keeps the decode that is under way out of the decoded-instruction cache. The
        // comparison is the same across the wrap of the address space.
        if let Some(pending) = &self.decoded.pending
            && (address ^ pending.rip) & !0xfff != 0
        {
            self.decoded.outside_page = true;
        }
        if self.read_only_qword {
            return Err(Self::unsupported("instruction fetch"));
        }
        if self.large_ram_profile
            && !self.cpl0_set_cross_page_fetch
            && (address & !0xfff) != (self.instruction_rip & !0xfff)
        {
            // The profile admits one 4 KiB code page: reject decoder lookahead
            // before any page walk for the next page.
            let width = address
                .saturating_sub(self.instruction_rip)
                .saturating_add(1)
                .min(u64::from(u8::MAX)) as u8;
            let error = TrackedAccessError::CrossesPage {
                address: GuestLinearAddress::new(self.instruction_rip),
                width,
            };
            self.access_error = Some(error.clone());
            return Err(self.map_access_error(error));
        }
        if let Some(cache) = self.fetch_cache
            && self.cpl0_set_cross_page_fetch
            && self.large_ram_profile
            // Writes reach the cache through `committed_write`; the capture mode diverts them,
            // so a capturing adapter never uses the cache.
            && !self.capture_effects
        {
            return self.fetch_through_cache(cache, address, context);
        }
        match self.access(address, context, RamAccessOperation::Fetch)? {
            TrackedAccessOutcome::Fetched(value) => Ok(value),
            _ => Err(Self::unsupported("non-fetch result")),
        }
    }

    fn read_integer_for_write(
        &mut self,
        address: u64,
        context: AccessContext,
        size: od_isa::OperandSize,
    ) -> Result<u64, MemoryError> {
        use crate::page_walk::translate_long64_2m_ram_write_span_check_with_fault_trace;

        if !self.cpl0_set_narrow_write
            || self.capture_effects
            || context.privilege != PrivilegeLevel::Ring3
            || context.origin != AccessOrigin::Explicit
        {
            return Err(Self::unsupported("CPL3 write-intent integer read"));
        }
        let length = size.bytes();
        let length_u64 = length as u64;
        if address.checked_add(length_u64 - 1).is_none() {
            return Err(MemoryError::AddressOverflow { address, length });
        }
        if self.capture_page_fault_trace {
            self.page_fault_trace = None;
        }
        let physical_memory = match &self.bus {
            AdapterBus::Ram(memory) | AdapterBus::RamMmio(memory, _, _) => *memory,
            AdapterBus::Machine(bus, _) => bus.physical_memory(),
            AdapterBus::Shared(bus, _) => bus.physical_memory(),
        };
        let mut checked = [(GuestPhysicalAddress::new(0), 0usize, 0usize); 2];
        let mut checked_count = 0usize;
        let mut offset = 0usize;
        while offset < length {
            let linear = address + offset as u64;
            let chunk = ((0x1000 - (linear & 0xfff)).min(length_u64 - offset as u64)) as usize;
            let mut trace = None;
            let result = translate_long64_2m_ram_write_span_check_with_fault_trace(
                physical_memory,
                &self.paging,
                self.instruction_rip,
                GuestLinearAddress::new(linear),
                chunk as u64,
                context,
                &mut *self.committed_write,
                self.capture_page_fault_trace.then_some(&mut trace),
            );
            match result {
                Ok(physical) => {
                    if let Some(walks) = self.walks {
                        walks.note_write();
                    }
                    checked[checked_count] = (physical, offset, chunk);
                    checked_count += 1;
                    offset += chunk;
                }
                Err(error) => {
                    if let Some(trace) = trace {
                        self.page_fault_trace = faulting_page_walk_context(&error, trace);
                    }
                    self.access_error = Some(error.clone());
                    return Err(self.map_access_error(error));
                }
            }
        }
        let mut bytes = [0u8; 8];
        for (physical, offset, chunk) in checked.into_iter().take(checked_count) {
            if let Err(source) =
                physical_memory.load_bytes(physical, &mut bytes[offset..offset + chunk])
            {
                let error = TrackedAccessError::OperandPhysical {
                    address: physical,
                    source,
                };
                self.access_error = Some(error.clone());
                return Err(self.map_access_error(error));
            }
        }
        Ok(u64::from_le_bytes(bytes))
    }

    fn read(
        &mut self,
        address: u64,
        context: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        if let Ok(vector) = <&mut [u8; 16]>::try_from(&mut *destination)
            && self.supports_cpl3_vector(context)
        {
            return self.access_vector(address, context, VectorRamAccess::Read(vector));
        }
        let width = match destination.len() {
            1 if self.cpl0_set_byte_read => crate::AccessWidth::Byte,
            2 if self.word_read_profile || self.cpl0_set_word_read => crate::AccessWidth::Word,
            4 if self.dword_read_profile || self.cpl0_set_dword_read => crate::AccessWidth::Dword,
            8 if !self.dword_read_profile && !self.word_read_profile => crate::AccessWidth::Qword,
            _ => return Err(Self::unsupported("read width")),
        };
        if context.origin == AccessOrigin::DescriptorTable
            && let Some(gate) = self.descriptor_gate.take()
        {
            self.begin_descriptor_scope(gate, true)?;
        }
        if self.reject_crossing_qword
            && matches!(width, crate::AccessWidth::Qword | crate::AccessWidth::Dword)
            && (address & 0xfff) + u64::from(width.bytes()) > 4096
        {
            return Err(Self::unsupported("cross-page qword read"));
        }
        let value = if matches!(width, crate::AccessWidth::Dword | crate::AccessWidth::Qword)
            && self.cpl0_set_cross_page_read
            && self.large_ram_profile
            && (address & 0xfff) + u64::from(width.bytes()) > 4096
        {
            // S4-38L5e: a dword or qword across a page boundary, one translation per byte; the
            // destination is written below, only once every byte has been read.
            let mut bytes = [0u8; 8];
            for (index, byte) in bytes[..destination.len()].iter_mut().enumerate() {
                let access = self.access_split_byte(
                    address.wrapping_add(index as u64),
                    context,
                    RamAccessOperation::Read {
                        width: crate::AccessWidth::Byte,
                    },
                )?;
                match access {
                    TrackedAccessOutcome::Read(value) => *byte = value as u8,
                    _ => return Err(Self::unsupported("non-read result")),
                }
            }
            u64::from_le_bytes(bytes)
        } else if width == crate::AccessWidth::Word
            && self.cpl0_set_word_read
            && !self.word_read_profile
            && self.large_ram_profile
            && address & 0xfff == 0xfff
        {
            // S4-38L4f: a word on the last byte of a page, one translation per byte.
            let mut bytes = [0u8; 2];
            for (index, byte) in bytes.iter_mut().enumerate() {
                let byte_address = address.wrapping_add(index as u64);
                let access = self.access_split_byte(
                    byte_address,
                    context,
                    RamAccessOperation::Read {
                        width: crate::AccessWidth::Byte,
                    },
                )?;
                match access {
                    TrackedAccessOutcome::Read(value) => *byte = value as u8,
                    _ => return Err(Self::unsupported("non-read result")),
                }
            }
            u64::from(u16::from_le_bytes(bytes))
        } else if !self.large_ram_profile
            && (self.dword_read_profile
                || self.word_read_profile
                || (address & 0xfff) + u64::from(width.bytes()) > 4096)
        {
            self.access_cross_page_read(address, context, width)?
        } else {
            match self.access(address, context, RamAccessOperation::Read { width })? {
                TrackedAccessOutcome::Read(value) => value,
                _ => return Err(Self::unsupported("non-read result")),
            }
        };
        destination.copy_from_slice(&value.to_le_bytes()[..destination.len()]);
        if context.origin == AccessOrigin::DescriptorTable {
            self.record_descriptor_read(address, context, destination);
        }
        Ok(())
    }

    fn write(
        &mut self,
        address: u64,
        context: AccessContext,
        source: &[u8],
    ) -> Result<(), MemoryError> {
        if self.read_only_qword {
            return Err(Self::unsupported("write"));
        }
        if let Ok(vector) = <&[u8; 16]>::try_from(source)
            && self.supports_cpl3_vector(context)
        {
            return self.access_vector(address, context, VectorRamAccess::Write(vector));
        }
        // The two store profiles are exclusive: a narrow-capable adapter never keeps the
        // dword-only store width (S4-38L3).
        debug_assert!(!(self.dword_write_profile && self.cpl0_set_narrow_write));
        let (width, expected) = if self.dword_write_profile {
            (crate::AccessWidth::Dword, 4)
        } else if self.cpl0_set_narrow_write {
            match source.len() {
                1 => (crate::AccessWidth::Byte, 1),
                2 => (crate::AccessWidth::Word, 2),
                4 => (crate::AccessWidth::Dword, 4),
                _ => (crate::AccessWidth::Qword, 8),
            }
        } else {
            (crate::AccessWidth::Qword, 8)
        };
        if source.len() != expected {
            return Err(Self::unsupported(if self.dword_write_profile {
                "non-dword writes"
            } else if self.cpl0_set_narrow_write {
                "write width"
            } else {
                "non-qword writes"
            }));
        }
        let mut bytes = [0; 8];
        bytes[..expected].copy_from_slice(source);
        let value = u64::from_le_bytes(bytes);
        let descriptor_target = if context.origin == AccessOrigin::DescriptorTable {
            if self.descriptor_gate.is_some() {
                return Err(MemoryError::Backend(
                    "descriptor write without a checked read".into(),
                ));
            }
            self.check_descriptor_write(address, context, expected)?
        } else {
            None
        };
        if self.cpl0_set_cross_page_store
            && self.cpl0_set_narrow_write
            && self.large_ram_profile
            && (address & 0xfff) + expected as u64 > 4096
        {
            return self.write_crossing_page(address, context, source);
        }
        match self.access(address, context, RamAccessOperation::Write { width, value })? {
            TrackedAccessOutcome::Written => {
                if let Some(physical) = descriptor_target {
                    self.record_descriptor_write(physical, source);
                }
                Ok(())
            }
            _ => Err(Self::unsupported("non-write result")),
        }
    }

    // S6-31: the locked operations run on the host atomics of the RAM (`PhysicalMemory::atomic_*`,
    // `SeqCst`; `CASAL`/`SWPAL`/`LDADDAL` on aarch64 with LSE). The tracked walk (`AtomicTarget`)
    // does what the read and write of the serial S6-20 default did to the page tables and returns
    // the physical address; then ONE host operation on the aligned cell. A misaligned operand
    // (including a page-crossing one): in the interleaving mode the serial S6-20 pair (S6-32, no
    // counter), in the parallel mode refused and counted, never run as two host accesses; an
    // MMIO, ROM or hole operand is refused. `atomic_rmw_integer` is a compare-exchange loop over the
    // pure `update`; the three specializations are the single host operations (S6-06).

    fn atomic_exchange_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: od_isa::OperandSize,
        replacement: u64,
    ) -> Result<u64, MemoryError> {
        let target = self.locked_target(address, context, size)?;
        let LockedTarget::Host { width, physical } = target else {
            return self.locked_split(target, address, context, size, &mut |_| replacement);
        };
        self.locked_finish(physical, |memory, committed| {
            memory.atomic_swap(physical, width, replacement, committed)
        })
    }

    fn atomic_fetch_add_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: od_isa::OperandSize,
        addend: u64,
    ) -> Result<u64, MemoryError> {
        let target = self.locked_target(address, context, size)?;
        let LockedTarget::Host { width, physical } = target else {
            return self.locked_split(target, address, context, size, &mut |old| {
                old.wrapping_add(addend) & size.mask()
            });
        };
        self.locked_finish(physical, |memory, committed| {
            memory.atomic_fetch_add(physical, width, addend, committed)
        })
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: od_isa::OperandSize,
        expected: u64,
        replacement: u64,
    ) -> Result<u64, MemoryError> {
        let target = self.locked_target(address, context, size)?;
        let LockedTarget::Host { width, physical } = target else {
            let expected = expected & size.mask();
            return self.locked_split(target, address, context, size, &mut |old| {
                if old == expected {
                    replacement & size.mask()
                } else {
                    old
                }
            });
        };
        self.locked_finish(physical, |memory, committed| {
            memory.atomic_compare_exchange(physical, width, expected, replacement, committed)
        })
    }

    fn atomic_rmw_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: od_isa::OperandSize,
        update: &mut dyn FnMut(u64) -> u64,
    ) -> Result<u64, MemoryError> {
        let target = self.locked_target(address, context, size)?;
        let LockedTarget::Host { width, physical } = target else {
            return self.locked_split(target, address, context, size, update);
        };
        self.locked_finish(physical, |memory, committed| {
            memory.atomic_rmw(physical, width, update, committed)
        })
    }

    /// S2-51: `CMPXCHG16B`. There is no host 128-bit compare-exchange behind this adapter, so the
    /// parallel mode refuses the operation before any walk (nothing is written, no accessed or
    /// dirty bit, no notification), as it refuses a split `LOCK` without the stop-all gate
    /// (S6-34): the serial read and write pair of the trait default is not atomic across host
    /// threads. The interleaving mode keeps the serial pair of the trait default (the scheduler
    /// switches vCPUs only on instruction boundaries); this adapter's `read` does not serve a
    /// 16-byte operand yet, so that pair ends in the "read width" refusal before any effect.
    /// CX16 is not declared until S2-57.
    fn atomic_rmw_u128(
        &mut self,
        address: u64,
        context: AccessContext,
        update: &mut dyn FnMut(u128) -> u128,
    ) -> Result<u128, MemoryError> {
        if self.bus.locked_memory().execution_mode() == crate::ExecutionMode::Parallel {
            return Err(MemoryError::Backend(format!(
                "CMPXCHG16B at {address:#x} is refused in the parallel mode: there is no host \
                 128-bit compare-exchange, and the serial read and write pair is not atomic \
                 across vCPU threads (S2-51; CX16 is not declared until S2-57)"
            )));
        }
        let mut bytes = [0u8; 16];
        self.read(address, context, &mut bytes)?;
        let old = u128::from_le_bytes(bytes);
        let new = update(old);
        self.write(address, context, &new.to_le_bytes())?;
        Ok(old)
    }

    fn mfence(&mut self, _context: AccessContext) -> Result<(), MemoryError> {
        Err(Self::unsupported("MFENCE"))
    }

    /// S6-32: the CPL0 set's fences end here: the memory places the host fence of its execution
    /// mode (a host fence in the parallel mode, nothing in the interleaving mode); guest RAM is
    /// not touched.
    fn fence(&mut self, kind: od_isa::FenceKind) -> Result<(), MemoryError> {
        self.bus.locked_memory().fence(kind);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AccessWidth, GuestPhysicalAddress, LINUX_ELF_STACK_COPY_RBP_FRAME_CAPACITY,
        LINUX_ELF_STACK_COPY_TARGET_CR3_EVENT_RIP, MachineMemoryMap, MemoryRegionKind,
        MemoryRegionSpec, RamLimits,
    };
    use od_isa::memory::{AccessOrigin, InstructionMode, PrivilegeLevel};
    use od_isa::{
        CodeSegmentDescriptorCache, CpuActivity, DataSegmentDescriptorCache, InvlpgExit, StiShadow,
        TranslationInvalidation,
    };

    const CR0: u64 = 0x8001_0033;
    const CR4: u64 = 1 << 5;
    const EFER: u64 = (1 << 8) | (1 << 10) | (1 << 11);
    const ROOT: u64 = 0x1000;
    const DATA_LINEAR: u64 = 0x0080_0000;
    const RAM_LEN: u64 = 0x20_000;

    fn context() -> AccessContext {
        AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        }
    }

    fn user_context() -> AccessContext {
        AccessContext {
            privilege: PrivilegeLevel::Ring3,
            ..context()
        }
    }

    fn put(bus: &mut MachineBus, address: u64, value: u64) {
        bus.write_physical(
            GuestPhysicalAddress::new(address),
            AccessWidth::Qword,
            value,
            |_| {},
        )
        .unwrap();
    }

    fn range(address: u64) -> GuestPhysicalRange {
        GuestPhysicalRange::new(GuestPhysicalAddress::new(address), 8).unwrap()
    }

    fn bus(data_pte: u64) -> MachineBus {
        let range = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), RAM_LEN).unwrap();
        let map = MachineMemoryMap::new(
            36,
            vec![MemoryRegionSpec::new(range, MemoryRegionKind::Ram)],
        )
        .unwrap();
        let mut bus =
            MachineBus::new(map, RamLimits::new(RAM_LEN), vec![], vec![], vec![]).unwrap();
        put(&mut bus, 0x1000, 0x2007);
        put(&mut bus, 0x2000, 0x3007);
        put(&mut bus, 0x3010, 0x4007);
        put(&mut bus, 0x3020, 0x5007);
        put(&mut bus, 0x4000, 0x7007);
        put(&mut bus, 0x5000, data_pte);
        bus
    }

    #[derive(Default)]
    struct TestDescriptorGate(std::cell::Cell<bool>);

    impl crate::bus::SplitLockGate for TestDescriptorGate {
        fn acquire(&self) -> Result<(), String> {
            assert!(!self.0.replace(true));
            Ok(())
        }

        fn release(&self) {
            assert!(self.0.replace(false));
        }
    }

    #[test]
    fn a_shared_descriptor_delivery_rejects_its_own_remap_and_notifies_after_release() {
        let mut bus = bus(0x8007);
        put(&mut bus, 0x4000, 0x5007); // 4 MiB aliases the PTE for DATA_LINEAR.
        put(&mut bus, 0x5008, 0xa007);
        let descriptor = 0x00AF_9A00_0000_FFFF;
        put(&mut bus, 0x8000, descriptor);
        put(&mut bus, 0x9000, descriptor);
        let ram = bus.physical_memory();
        let paging = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap();
        let gate = TestDescriptorGate::default();
        let mut notified = Vec::new();

        let (result, _, _) = with_cpl0_gate_memory(
            ram,
            &paging,
            0,
            &mut |range| {
                assert!(!gate.0.get());
                notified.push(range);
            },
            None,
            Some(&gate),
            |memory| {
                assert!(gate.0.get());
                let table = context().descriptor_table();
                let mut bytes = [0; 8];
                memory.read(DATA_LINEAR + 4092, table, &mut bytes).unwrap();
                memory.read(DATA_LINEAR, table, &mut bytes).unwrap();
                assert_eq!(u64::from_le_bytes(bytes), descriptor);
                memory
                    .write(0x40_0000, context(), &0x9007u64.to_le_bytes())
                    .unwrap();
                bytes[5] |= 1;
                memory.write(DATA_LINEAR, table, &bytes)
            },
        )
        .unwrap();

        assert!(
            matches!(result, Err(MemoryError::Backend(message)) if message.contains("translation changed"))
        );
        assert!(notified.contains(&range(0x5000)));
        assert!(notified.contains(&range(0x5008)));
        assert!(!notified.contains(&range(0x8000)));
        assert!(!notified.contains(&range(0x9000)));
        assert_eq!(
            ram.load_u64(GuestPhysicalAddress::new(0x5000)).unwrap(),
            0x9007
        );
        assert_eq!(
            ram.load_u64(GuestPhysicalAddress::new(0x8000)).unwrap(),
            descriptor
        );
        assert_eq!(
            ram.load_u64(GuestPhysicalAddress::new(0x9000)).unwrap(),
            descriptor
        );
    }

    #[test]
    fn a_descriptor_instruction_checks_the_companion_qword_before_busy_write() {
        let mut bus = bus(0x8007);
        let low = 0x0000_8900_0000_0067u64;
        put(&mut bus, 0x8000, low);
        put(&mut bus, 0x8008, 0x1234);
        let ram = bus.physical_memory();
        let paging = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap();
        let gate = TestDescriptorGate::default();
        let context = context();
        let table = context.descriptor_table();
        let mut notify = |_| {};
        let mut memory = set_loop_adapter(ram, None, paging, 0, &mut notify, None, None);
        memory.descriptor_gate = Some(&gate);
        let mut bytes = [0; 8];
        memory.read(DATA_LINEAR, table, &mut bytes).unwrap();
        assert!(gate.0.get());
        memory.read(DATA_LINEAR + 8, table, &mut bytes).unwrap();
        memory
            .write(DATA_LINEAR + 8, context, &0x5678u64.to_le_bytes())
            .unwrap();
        let result = memory.write(DATA_LINEAR, table, &(low | (1 << 41)).to_le_bytes());
        assert!(
            matches!(result, Err(MemoryError::Backend(message)) if message.contains("translation changed"))
        );
        memory.finish_descriptor_scope();
        assert!(!gate.0.get());
        assert_eq!(
            ram.load_u64(GuestPhysicalAddress::new(0x8000)).unwrap(),
            low
        );
        assert_eq!(
            ram.load_u64(GuestPhysicalAddress::new(0x8008)).unwrap(),
            0x5678
        );
    }

    #[test]
    fn ordered_descriptor_writes_accept_the_first_update_and_keep_it_on_a_late_fault() {
        let code = 0x00AF_9A00_0000_FFFFu64;
        let data = 0x00CF_9200_0000_FFFFu64;
        for writable in [false, true] {
            let mut bus = bus(0x8007);
            put(&mut bus, 0x5008, 0x9005 | (u64::from(writable) << 1));
            put(&mut bus, 0x8000, code);
            put(&mut bus, 0x9000, data);
            let ram = bus.physical_memory();
            let paging = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap();
            let gate = TestDescriptorGate::default();
            let table = context().descriptor_table();
            let second = DATA_LINEAR + 4096;
            let mut notify = |_| {};
            let mut memory = set_loop_adapter(ram, None, paging, 0, &mut notify, None, None);
            memory.descriptor_gate = Some(&gate);
            let mut bytes = [0; 8];
            memory.read(DATA_LINEAR, table, &mut bytes).unwrap();
            memory.read(second, table, &mut bytes).unwrap();
            memory
                .write(DATA_LINEAR, table, &(code | (1 << 40)).to_le_bytes())
                .unwrap();
            let result = memory.write(second, table, &(data | (1 << 40)).to_le_bytes());
            if writable {
                result.unwrap();
            } else {
                assert!(
                    matches!(result, Err(MemoryError::Architectural(fault)) if fault.vector == 14)
                );
            }
            memory.finish_descriptor_scope();
            assert_eq!(
                ram.load_u64(GuestPhysicalAddress::new(0x8000)).unwrap(),
                code | (1 << 40)
            );
            assert_eq!(
                ram.load_u64(GuestPhysicalAddress::new(0x9000)).unwrap(),
                data | (u64::from(writable) << 40)
            );
        }
    }

    #[test]
    fn cpl3_descriptor_access_byte_can_follow_a_two_page_read() {
        let mut bus = bus(0x8003);
        put(&mut bus, 0x5008, 0x9003);
        let ram = bus.physical_memory();
        let paging = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap();
        let gate = TestDescriptorGate::default();
        let table = AccessContext {
            privilege: PrivilegeLevel::Ring3,
            ..context()
        }
        .descriptor_table();
        let address = DATA_LINEAR + 4093;
        let mut notify = |_| {};
        let mut memory = set_loop_adapter(ram, None, paging, 0, &mut notify, None, None);
        memory.descriptor_gate = Some(&gate);
        let mut bytes = [0; 8];
        memory.read(address, table, &mut bytes).unwrap();
        assert_eq!(bytes, [0; 8]);
        assert!(matches!(
            memory.write(address, table, &[0xff; 8]),
            Err(MemoryError::Backend(ref text)) if text.contains("single-page qword")
        ));
        memory.write(address + 5, table, &[1]).unwrap();
        memory.finish_descriptor_scope();
        assert_eq!(
            ram.load_u64(GuestPhysicalAddress::new(0x9000)).unwrap(),
            1 << 16
        );
    }

    #[test]
    fn descriptor_scope_does_not_dispatch_device_accesses() {
        struct CountingMmio(std::cell::Cell<usize>);
        impl crate::bus::SetStepMmio for CountingMmio {
            fn windows(&self) -> &[crate::page_walk::MmioWindow] {
                &[(crate::LOCAL_APIC_MMIO_BASE, crate::LOCAL_APIC_MMIO_SIZE)]
            }

            fn dispatch(
                &self,
                _address: GuestPhysicalAddress,
                _width: u8,
                _operation: RamAccessOperation,
            ) -> Result<u64, crate::page_walk::MmioDispatchError> {
                self.0.set(self.0.get() + 1);
                Ok(0x00AF_9B00_0000_FFFF)
            }
        }

        let (bus, _) = lapic_bus(1);
        let paging = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap();
        let gate = TestDescriptorGate::default();
        let mmio = CountingMmio(std::cell::Cell::new(0));
        let mut notify = |_| {};
        let mut memory = set_loop_adapter(
            bus.physical_memory(),
            Some((&mmio, 0)),
            paging,
            0,
            &mut notify,
            None,
            None,
        );
        let mut bytes = [0; 8];
        memory.descriptor_gate = Some(&gate);
        assert!(
            memory
                .read(DATA_LINEAR, context().descriptor_table(), &mut bytes)
                .is_err()
        );
        memory.finish_descriptor_scope();
        let stack = AccessContext {
            origin: AccessOrigin::ImplicitStack,
            ..context()
        };
        with_gated_cpl0_delivery(&mut memory, Some(&gate), |memory| {
            assert!(memory.write(DATA_LINEAR, stack, &[0; 8]).is_err());
        })
        .unwrap();
        assert_eq!(mmio.0.get(), 0);
        memory.read(DATA_LINEAR, context(), &mut bytes).unwrap();
        assert_eq!(mmio.0.get(), 1);
    }

    const ELF_COPY_R13: u64 = 0xffff_ffff_83ff_0000;
    const ELF_COPY_RSI: u64 = 0xffff_ffff_82fd_eb8f;

    fn elf_copy_bus(
        map_destination: bool,
        map_stack_input: bool,
    ) -> (MachineBus, Long64ProbeConfig) {
        let range = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), RAM_LEN).unwrap();
        let map = MachineMemoryMap::new(
            36,
            vec![MemoryRegionSpec::new(range, MemoryRegionKind::Ram)],
        )
        .unwrap();
        let mut bus =
            MachineBus::new(map, RamLimits::new(RAM_LEN), vec![], vec![], vec![]).unwrap();
        let mut tables = std::collections::BTreeMap::<Vec<u16>, u64>::new();
        tables.insert(Vec::new(), ROOT);
        let mut next_table = 0x2000;
        let mut map_page = |bus: &mut MachineBus, address: u64, frame: u64, present: bool| {
            let indices = [
                ((address >> 39) & 0x1ff) as u16,
                ((address >> 30) & 0x1ff) as u16,
                ((address >> 21) & 0x1ff) as u16,
                ((address >> 12) & 0x1ff) as u16,
            ];
            let mut prefix = Vec::new();
            for level in 0..3 {
                let table = *tables.get(&prefix).unwrap();
                prefix.push(indices[level]);
                let child = *tables.entry(prefix.clone()).or_insert_with(|| {
                    let child = next_table;
                    next_table += 0x1000;
                    child
                });
                put(bus, table + u64::from(indices[level]) * 8, child | 7);
            }
            let table = *tables.get(&prefix).unwrap();
            put(
                bus,
                table + u64::from(indices[3]) * 8,
                if present { frame | 7 } else { 0 },
            );
        };
        map_page(&mut bus, LINUX_ELF_STACK_COPY_RIP, 0x10_000, true);
        map_page(&mut bus, ELF_COPY_RSI, 0x11_000, true);
        for (offset, byte) in b"x86_64\0".iter().copied().enumerate() {
            bus.write_physical(
                GuestPhysicalAddress::new(0x11_000 + (ELF_COPY_RSI & 0xfff) + offset as u64),
                AccessWidth::Byte,
                u64::from(byte),
                |_| {},
            )
            .unwrap();
        }
        map_page(&mut bus, ELF_COPY_R13, 0x12_000, true);
        if map_destination {
            map_page(&mut bus, ELF_COPY_R13 - 7, 0x13_000, true);
        }
        if !map_stack_input {
            let indices = [
                ((ELF_COPY_R13 >> 39) & 0x1ff) as u16,
                ((ELF_COPY_R13 >> 30) & 0x1ff) as u16,
                ((ELF_COPY_R13 >> 21) & 0x1ff) as u16,
                ((ELF_COPY_R13 >> 12) & 0x1ff) as u16,
            ];
            let mut prefix = Vec::new();
            for index in indices {
                prefix.push(index);
            }
            let table = *tables.get(&prefix[..3]).unwrap();
            put(&mut bus, table + u64::from(indices[3]) * 8, 0);
        }
        let instruction_physical = 0x10_000 + (LINUX_ELF_STACK_COPY_RIP & 0xfff);
        for (offset, byte) in [0xe8u8, 0x47, 0xd9, 0x34, 0x00].into_iter().enumerate() {
            bus.write_physical(
                GuestPhysicalAddress::new(instruction_physical + offset as u64),
                AccessWidth::Byte,
                u64::from(byte),
                |_| {},
            )
            .unwrap();
        }
        let stack_input_physical = 0x12_000 + 0x60;
        put(&mut bus, stack_input_physical, ELF_COPY_R13 + 0x100);
        (bus, Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap())
    }

    fn elf_copy_state(rdx: u64) -> LongModeState {
        let mut state = LongModeState::default();
        state.rip = LINUX_ELF_STACK_COPY_RIP;
        state.cs_selector = Some(0x8);
        state.gpr[od_isa::Register::R15 as usize] = ELF_COPY_R13 + 0x40;
        state.gpr[od_isa::Register::R13 as usize] = ELF_COPY_R13;
        state.gpr[od_isa::Register::Rdi as usize] = ELF_COPY_R13 - 7;
        state.gpr[od_isa::Register::Rsi as usize] = ELF_COPY_RSI;
        state.gpr[od_isa::Register::Rdx as usize] = rdx;
        state
    }

    fn adapter<'a>(
        bus: &'a mut MachineBus,
        callbacks: &'a mut dyn FnMut(GuestPhysicalRange),
    ) -> PagedMovMemoryAdapter<'a> {
        adapter_on(bus.into(), callbacks)
    }

    fn adapter_on<'a>(
        bus: AdapterBus<'a>,
        callbacks: &'a mut dyn FnMut(GuestPhysicalRange),
    ) -> PagedMovMemoryAdapter<'a> {
        PagedMovMemoryAdapter {
            bus,
            paging: Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap(),
            instruction_rip: 0x0040_0000,
            committed_write: callbacks,
            page_fault: None,
            memory_error: None,
            access_error: None,
            page_fault_trace: None,
            capture_page_fault_trace: false,
            committed_ad_updates: None,
            committed_writes: None,
            capture_effects: false,
            descriptor_gate: None,
            descriptor_scope: None,
            dword_read_profile: false,
            word_read_profile: false,
            ram_only: false,
            reject_crossing_qword: false,
            read_only_qword: false,
            large_ram_profile: false,
            dword_write_profile: false,
            cpl0_set_dword_read: false,
            cpl0_set_byte_read: false,
            cpl0_set_word_read: false,
            cpl0_set_narrow_write: false,
            cpl0_set_cross_page_fetch: false,
            cpl0_set_cross_page_store: false,
            cpl0_set_cross_page_read: false,
            fetch_cache: None,
            walks: None,
            decoded: DecodedStep::default(),
        }
    }

    fn pending_post_iret(access: &str, cpl: u8) -> PendingPostIretObservation {
        PendingPostIretObservation {
            event_ordinal: 2,
            fault_rip: 0x0040_0000,
            fault_cr2: DATA_LINEAR + 0x456,
            fault_access: access.into(),
            fault_cpl: cpl,
            fault_rsp: 0x7000,
            fault_cs: Some(if cpl == 3 { 0x23 } else { 0x08 }),
        }
    }

    fn capture_elf_copy(
        state: &LongModeState,
        bus: &mut MachineBus,
        paging: &Long64ProbeConfig,
    ) -> ElfStackCopySnapshot {
        capture_elf_stack_copy_snapshot(state, bus, paging, context(), ROOT)
    }

    fn put_elf_copy_frame(bus: &mut MachineBus, rbp: u64, saved_rbp: u64, return_address: u64) {
        let physical = 0x12_000 + (rbp & 0xfff);
        put(bus, physical, saved_rbp);
        put(bus, physical + 8, return_address);
    }

    fn elf_copy_page_table_snapshot(bus: &mut MachineBus) -> Vec<u64> {
        (0x1000..0x10_000)
            .step_by(8)
            .map(|address| {
                bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                    .unwrap()
            })
            .collect()
    }

    #[test]
    fn selected_cr3_rbp_chain_reader_reports_all_terminal_states_without_writes() {
        let (mut bus, paging) = elf_copy_bus(true, true);
        let first_rbp = ELF_COPY_R13 + 0x200;
        let second_rbp = ELF_COPY_R13 + 0x300;
        let return_address = 0xffff_ffff_8100_1234;
        put_elf_copy_frame(&mut bus, first_rbp, second_rbp, return_address);
        put_elf_copy_frame(&mut bus, second_rbp, 0, return_address + 0x10);
        let before = elf_copy_page_table_snapshot(&mut bus);
        let mut state = LongModeState::default();
        state.rip = LINUX_ELF_STACK_COPY_TARGET_CR3_EVENT_RIP;
        state.gpr[od_isa::Register::Rbp as usize] = first_rbp;
        let (status, frames, error) = capture_elf_stack_copy_target_rbp_chain(
            &state,
            &mut bus,
            &paging,
            LINUX_ELF_STACK_COPY_TARGET_CR3_EVENT_RIP,
            context(),
        );
        assert_eq!(status, ElfStackCopyTargetCr3ChainStatus::Complete);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].saved_rbp_slot, first_rbp);
        assert_eq!(frames[0].saved_rbp, second_rbp);
        assert_eq!(frames[0].return_address_slot, first_rbp + 8);
        assert_eq!(frames[0].return_address, return_address);
        assert_eq!(frames[1].saved_rbp, 0);
        assert_eq!(error, None);
        assert_eq!(elf_copy_page_table_snapshot(&mut bus), before);

        let (mut bus, paging) = elf_copy_bus(true, true);
        let mut state = LongModeState::default();
        state.rip = LINUX_ELF_STACK_COPY_TARGET_CR3_EVENT_RIP;
        let (status, frames, error) = capture_elf_stack_copy_target_rbp_chain(
            &state,
            &mut bus,
            &paging,
            LINUX_ELF_STACK_COPY_TARGET_CR3_EVENT_RIP,
            context(),
        );
        assert_eq!(status, ElfStackCopyTargetCr3ChainStatus::Complete);
        assert!(frames.is_empty());
        assert_eq!(error, None);

        let (mut bus, paging) = elf_copy_bus(true, true);
        state.gpr[od_isa::Register::Rbp as usize] = 0xffff_8880_0000_0000;
        let (status, frames, error) = capture_elf_stack_copy_target_rbp_chain(
            &state,
            &mut bus,
            &paging,
            LINUX_ELF_STACK_COPY_TARGET_CR3_EVENT_RIP,
            context(),
        );
        assert_eq!(status, ElfStackCopyTargetCr3ChainStatus::Unreadable);
        assert!(frames.is_empty());
        assert!(error.is_some());

        let (mut bus, paging) = elf_copy_bus(true, true);
        let malformed_rbp = ELF_COPY_R13 + 0x200;
        put_elf_copy_frame(&mut bus, malformed_rbp, malformed_rbp, return_address);
        state.gpr[od_isa::Register::Rbp as usize] = malformed_rbp;
        let (status, frames, error) = capture_elf_stack_copy_target_rbp_chain(
            &state,
            &mut bus,
            &paging,
            LINUX_ELF_STACK_COPY_TARGET_CR3_EVENT_RIP,
            context(),
        );
        assert_eq!(status, ElfStackCopyTargetCr3ChainStatus::Malformed);
        assert_eq!(frames.len(), 1);
        assert!(error.is_some());

        let (mut bus, paging) = elf_copy_bus(true, true);
        state.gpr[od_isa::Register::Rbp as usize] = 0xffff_ffff_ffff_fff8;
        let (status, frames, error) = capture_elf_stack_copy_target_rbp_chain(
            &state,
            &mut bus,
            &paging,
            LINUX_ELF_STACK_COPY_TARGET_CR3_EVENT_RIP,
            context(),
        );
        assert_eq!(status, ElfStackCopyTargetCr3ChainStatus::Malformed);
        assert!(frames.is_empty());
        assert!(error.is_some());

        let (mut bus, paging) = elf_copy_bus(true, true);
        let prefix_rbp = ELF_COPY_R13 + 0x200;
        put_elf_copy_frame(&mut bus, prefix_rbp, 0xffff_ffff_ffff_fff8, return_address);
        state.gpr[od_isa::Register::Rbp as usize] = prefix_rbp;
        let (status, frames, error) = capture_elf_stack_copy_target_rbp_chain(
            &state,
            &mut bus,
            &paging,
            LINUX_ELF_STACK_COPY_TARGET_CR3_EVENT_RIP,
            context(),
        );
        assert_eq!(status, ElfStackCopyTargetCr3ChainStatus::Malformed);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].saved_rbp, 0xffff_ffff_ffff_fff8);
        assert!(error.is_some());

        let (mut bus, paging) = elf_copy_bus(true, true);
        let initial_rbp = ELF_COPY_R13 + 0x100;
        for index in 0..LINUX_ELF_STACK_COPY_RBP_FRAME_CAPACITY {
            let rbp = initial_rbp + index as u64 * 0x10;
            put_elf_copy_frame(&mut bus, rbp, rbp + 0x10, return_address);
        }
        state.gpr[od_isa::Register::Rbp as usize] = initial_rbp;
        let (status, frames, error) = capture_elf_stack_copy_target_rbp_chain(
            &state,
            &mut bus,
            &paging,
            LINUX_ELF_STACK_COPY_TARGET_CR3_EVENT_RIP,
            context(),
        );
        assert_eq!(status, ElfStackCopyTargetCr3ChainStatus::FrameLimit);
        assert_eq!(frames.len(), LINUX_ELF_STACK_COPY_RBP_FRAME_CAPACITY);
        assert_ne!(frames.last().unwrap().saved_rbp, 0);
        assert!(error.is_some());
    }

    #[test]
    fn elf_stack_copy_records_present_and_absent_destination_leaf_as_probe_data() {
        for (present, expected) in [(true, "translated"), (false, "page_fault")] {
            let (mut bus, paging) = elf_copy_bus(present, true);
            let snapshot = capture_elf_copy(&elf_copy_state(7), &mut bus, &paging);
            assert_eq!(snapshot.status, ElfStackCopyStatus::Captured);
            assert_eq!(snapshot.probes.len(), 3);
            assert_eq!(snapshot.probes[0].name, "stack_input");
            assert_eq!(snapshot.probes[1].name, "copy_source");
            assert_eq!(snapshot.probes[2].name, "copy_destination");
            assert_eq!(snapshot.probes[2].outcome, expected);
            assert_eq!(snapshot.probes[2].access, "Write");
            if !present {
                assert!(snapshot.probes[2].error.is_some());
            }
        }
    }

    #[test]
    fn elf_stack_copy_bprm_pointer_read_error_is_partial_and_diagnostic() {
        let (mut bus, paging) = elf_copy_bus(false, false);
        let snapshot = capture_elf_copy(&elf_copy_state(7), &mut bus, &paging);
        assert_eq!(snapshot.status, ElfStackCopyStatus::ReadError);
        assert_eq!(snapshot.rip, Some(LINUX_ELF_STACK_COPY_RIP));
        assert_eq!(snapshot.r15, Some(ELF_COPY_R13 + 0x40));
        assert_eq!(snapshot.bprm_p_address, Some(ELF_COPY_R13 + 0x60));
        assert!(
            snapshot
                .error
                .as_deref()
                .unwrap()
                .starts_with("bprm_p read:")
        );
        assert!(snapshot.probes.is_empty());
    }

    #[test]
    fn elf_stack_copy_guard_mismatch_does_not_probe_guest_memory() {
        let (mut bus, paging) = elf_copy_bus(false, true);
        let snapshot = capture_elf_copy(&elf_copy_state(8), &mut bus, &paging);
        assert_eq!(snapshot.status, ElfStackCopyStatus::GateMismatch);
        assert_eq!(snapshot.mismatch_reasons, ["rdx_not_7"]);
        assert!(snapshot.probes.is_empty());
    }

    #[test]
    fn elf_stack_copy_requires_an_observed_cpl_selector() {
        let (mut bus, paging) = elf_copy_bus(false, true);
        let mut state = elf_copy_state(7);
        state.cs_selector = None;
        let snapshot = capture_elf_copy(&state, &mut bus, &paging);
        assert_eq!(snapshot.status, ElfStackCopyStatus::GateMismatch);
        assert_eq!(snapshot.cpl, None);
        assert!(
            snapshot
                .mismatch_reasons
                .contains(&"cpl_not_zero".to_string())
        );
        assert!(snapshot.probes.is_empty());
    }

    #[test]
    fn elf_stack_copy_read_only_probe_preserves_state_and_page_table_bytes() {
        let (mut bus, paging) = elf_copy_bus(false, true);
        let state = elf_copy_state(7);
        let state_before = state.clone();
        let table_words_before = (ROOT..0x10_000)
            .step_by(8)
            .map(|address| {
                bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let snapshot = capture_elf_copy(&state, &mut bus, &paging);
        let table_words_after = (ROOT..0x10_000)
            .step_by(8)
            .map(|address| {
                bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(snapshot.status, ElfStackCopyStatus::Captured);
        assert_eq!(state, state_before);
        assert_eq!(
            table_words_after, table_words_before,
            "walk changed table/A-D bytes"
        );
    }

    #[test]
    fn post_iret_snapshot_observes_handler_repaired_missing_leaf_without_a_d_updates() {
        let mut bus = bus(0);
        let paging = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap();
        let (original_fault, original_trace) = {
            let (memory, _) = bus.split_for_pio_step();
            translate_long64_2m_ram_read_only_with_trace(
                memory,
                &paging,
                0x0040_0000,
                GuestLinearAddress::new(DATA_LINEAR + 0x456),
                context(),
                AccessKind::Write,
            )
        };
        assert!(matches!(
            original_fault,
            Err(TrackedAccessError::PageFault { .. })
        ));
        assert_eq!(original_trace.len(), 4);

        // Model the handler repair between delivery and IRET.
        put(&mut bus, 0x5000, 0x9007);
        let before = [0x1000, 0x2000, 0x3010, 0x3020, 0x5000].map(|address| {
            bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                .unwrap()
        });
        let mut writes = Vec::new();
        let mut callback = |range| writes.push(range);
        let mut memory = adapter(&mut bus, &mut callback);
        let pending = pending_post_iret("Write", 3);
        let mut state = LongModeState::default();
        state.rip = pending.fault_rip;
        state.cs_selector = pending.fault_cs;
        state.gpr[od_isa::Register::Rsp as usize] = pending.fault_rsp;
        let snapshot = memory.capture_post_iret_page_walk(pending, &state, 0x1000);
        assert_eq!(snapshot.translation_status, "translated");
        assert_eq!(snapshot.translated_physical, Some(0x9456));
        assert_eq!(snapshot.entries.len(), 4);
        drop(memory);
        drop(callback);
        let after = [0x1000, 0x2000, 0x3010, 0x3020, 0x5000].map(|address| {
            bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                .unwrap()
        });
        assert_eq!(before, after);
        assert!(writes.is_empty());
    }

    #[test]
    fn post_iret_snapshot_uses_the_shared_two_mib_leaf_walk() {
        let mut bus = bus(0);
        put(&mut bus, 0x3020, 0x87);
        let mut writes = |_| {};
        let mut memory = adapter(&mut bus, &mut writes);
        let pending = pending_post_iret("Read", 3);
        let mut state = LongModeState::default();
        state.rip = pending.fault_rip;
        state.cs_selector = pending.fault_cs;
        state.gpr[od_isa::Register::Rsp as usize] = pending.fault_rsp;
        let snapshot = memory.capture_post_iret_page_walk(pending, &state, ROOT);
        assert_eq!(snapshot.translation_status, "translated");
        assert_eq!(snapshot.translated_physical, Some(0x456));
        assert_eq!(snapshot.entries.len(), 3);
        assert_eq!(snapshot.entries[2].entry_value, 0x87);
    }

    #[test]
    fn nested_interrupt_return_context_cannot_complete_the_selected_page_fault() {
        let mut collector = LinuxExecutionObservationCollector::with_post_iret_ordinal(Some(0));
        let event = DeliveredPageFaultObservation {
            rip: 0x0040_0000,
            cr2: DATA_LINEAR,
            error_code: 7,
            access: "Write".into(),
            cpl: 3,
            faulting_context: FaultingPageFaultContext {
                cr3: ROOT,
                rsp: 0x7000,
                rbp: 0,
                rcx: 0,
                rsi: 0,
                rdi: 0,
                cs_selector: Some(0x23),
                frame_chain: Vec::new(),
                frame_chain_end: crate::FrameChainEnd::NullFramePointer,
                page_walk: None,
            },
        };
        collector.record_page_fault(event);
        let pending = collector.pending_post_iret().unwrap();
        let mut nested_irq_return = LongModeState::default();
        nested_irq_return.rip = 0xffff_ffff_8100_1000;
        nested_irq_return.cs_selector = Some(0x08);
        nested_irq_return.gpr[od_isa::Register::Rsp as usize] = 0x9000;
        assert!(!post_iret_context_matches(&pending, &nested_irq_return));
        collector.note_nested_delivery();
        assert!(!collector.accept_post_iret_return(false));
        assert_eq!(collector.pending_post_iret(), Some(pending.clone()));

        let mut selected_fault_return = LongModeState::default();
        selected_fault_return.rip = pending.fault_rip;
        selected_fault_return.cs_selector = pending.fault_cs;
        selected_fault_return.gpr[od_isa::Register::Rsp as usize] = pending.fault_rsp;
        assert!(post_iret_context_matches(&pending, &selected_fault_return));
        assert!(collector.accept_post_iret_return(true));
        collector.record_post_iret_page_walk(PostIretPageWalkSnapshot {
            event_ordinal: pending.event_ordinal,
            fault_rip: pending.fault_rip,
            fault_cr2: pending.fault_cr2,
            fault_access: pending.fault_access,
            fault_cpl: pending.fault_cpl,
            post_iret_rip: selected_fault_return.rip,
            post_iret_cs: selected_fault_return.cs_selector,
            post_iret_cpl: 3,
            post_iret_rsp: pending.fault_rsp,
            post_iret_cr3: ROOT,
            entries: Vec::new(),
            translation_status: "translated",
            translated_physical: Some(0x9000),
            translation_error: None,
        });
        let observations = collector.finish();
        assert_eq!(observations.post_iret_page_walk_status, Some("captured"));
        assert_eq!(
            observations.post_iret_page_walk.unwrap().event_ordinal,
            pending.event_ordinal
        );
    }

    #[test]
    fn post_iret_snapshot_reports_permission_fault_and_preserves_page_tables() {
        let mut bus = bus(0x9005);
        let before = [0x1000, 0x2000, 0x3010, 0x3020, 0x5000].map(|address| {
            bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                .unwrap()
        });
        let mut writes = |_| {};
        let mut memory = adapter(&mut bus, &mut writes);
        let pending = pending_post_iret("Write", 3);
        let mut state = LongModeState::default();
        state.rip = pending.fault_rip;
        state.cs_selector = pending.fault_cs;
        state.gpr[od_isa::Register::Rsp as usize] = pending.fault_rsp;
        let snapshot = memory.capture_post_iret_page_walk(pending, &state, ROOT);
        assert_eq!(snapshot.translation_status, "page_fault");
        assert_eq!(snapshot.entries.len(), 4);
        assert!(
            snapshot
                .translation_error
                .as_deref()
                .is_some_and(|error| error.contains("write_denied: true"))
        );
        drop(memory);
        let after = [0x1000, 0x2000, 0x3010, 0x3020, 0x5000].map(|address| {
            bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                .unwrap()
        });
        assert_eq!(before, after);
    }

    #[test]
    fn adapter_diagnostic_frame_read_refuses_mapped_rom() {
        let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), 0x9000).unwrap();
        let rom = GuestPhysicalRange::new(GuestPhysicalAddress::new(0x9000), 0x1000).unwrap();
        let map = MachineMemoryMap::new(
            36,
            vec![
                MemoryRegionSpec::new(ram, MemoryRegionKind::Ram),
                MemoryRegionSpec::new(rom, MemoryRegionKind::Rom),
            ],
        )
        .unwrap();
        let mut memory = crate::PhysicalMemory::new(&map, RamLimits::new(0x9000)).unwrap();
        for (address, value) in [
            (0x1000, 0x2007),
            (0x2000, 0x3007),
            (0x3020, 0x5007),
            (0x5000, 0x9007),
        ] {
            memory
                .store_u64(GuestPhysicalAddress::new(address), value, |_| {})
                .unwrap();
        }
        let paging = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap();
        let mut bus = AdapterBus::Ram(&mut memory);
        let mut bytes = [0xaa; 16];
        assert_eq!(
            bus.read_virtual_ram_diagnostic(
                &paging,
                0x0040_0000,
                GuestLinearAddress::new(DATA_LINEAR),
                context(),
                &mut bytes,
            ),
            Err(())
        );
        assert_eq!(bytes, [0xaa; 16]);
    }

    #[test]
    fn cross_page_read_keeps_the_failed_second_page_walk_when_opted_in() {
        let mut bus = bus(0x7007);
        let mut committed = |_| {};
        let mut memory = adapter(&mut bus, &mut committed);
        memory.capture_page_fault_trace = true;
        let error = memory
            .access_cross_page_read(DATA_LINEAR + 0xffc, context(), AccessWidth::Qword)
            .unwrap_err();
        assert!(matches!(error, MemoryError::Architectural(_)));
        let walk = memory.page_fault_trace.as_ref().unwrap();
        assert_eq!(
            walk.reason,
            crate::PageFaultReason::NotPresent {
                level: crate::PageTableLevel::Pt,
            }
        );
        assert_eq!(walk.entries.len(), 4);
        assert_eq!(walk.entries[3].entry_address.get(), 0x5008);
        assert_eq!(walk.entries[3].entry_value, 0);
        memory.capture_page_fault_trace = false;
        memory.page_fault_trace = None;
        assert!(
            memory
                .access_cross_page_read(DATA_LINEAR + 0xffc, context(), AccessWidth::Qword,)
                .is_err()
        );
        assert!(memory.page_fault_trace.is_none(), "capture is opt-in");
        drop(memory);
        assert_eq!(
            bus.read_physical(GuestPhysicalAddress::new(0x5000), AccessWidth::Qword)
                .unwrap()
                & 0x60,
            0x20
        );
        assert_eq!(
            bus.read_physical(GuestPhysicalAddress::new(0x5008), AccessWidth::Qword)
                .unwrap(),
            0
        );
    }

    #[test]
    fn cross_page_write_preflight_keeps_failed_walk_without_dirty_or_operand_write() {
        let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), RAM_LEN).unwrap();
        let map =
            MachineMemoryMap::new(36, vec![MemoryRegionSpec::new(ram, MemoryRegionKind::Ram)])
                .unwrap();
        let mut physical = crate::PhysicalMemory::new(&map, RamLimits::new(RAM_LEN)).unwrap();
        for (address, value) in [
            (0x1000, 0x2007),
            (0x2000, 0x3007),
            (0x3020, 0x5007),
            (0x5000, 0x7007),
        ] {
            physical
                .store_u64(GuestPhysicalAddress::new(address), value, |_| {})
                .unwrap();
        }
        physical
            .store_bytes(GuestPhysicalAddress::new(0x7ffc), &[0x11; 4], |_| {})
            .unwrap();
        let paging = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap();
        let mut committed = |_| {};
        let mut memory = set_loop_adapter(
            &mut physical,
            None,
            paging,
            0x0040_0000,
            &mut committed,
            None,
            None,
        );
        memory.capture_page_fault_trace = true;
        let error = memory
            .write_crossing_page(DATA_LINEAR + 0xffc, context(), &[0x22; 8])
            .unwrap_err();
        assert!(matches!(error, MemoryError::Architectural(_)));
        let walk = memory.page_fault_trace.as_ref().unwrap();
        assert_eq!(
            walk.reason,
            crate::PageFaultReason::NotPresent {
                level: crate::PageTableLevel::Pt,
            }
        );
        assert_eq!(walk.entries.len(), 4);
        assert_eq!(walk.entries[3].entry_address.get(), 0x5008);
        assert_eq!(walk.entries[3].entry_value, 0);
        memory.capture_page_fault_trace = false;
        memory.page_fault_trace = None;
        assert!(
            memory
                .write_crossing_page(DATA_LINEAR + 0xffc, context(), &[0x33; 8])
                .is_err()
        );
        assert!(memory.page_fault_trace.is_none(), "capture is opt-in");
        drop(memory);
        assert_eq!(
            physical
                .load_u64(GuestPhysicalAddress::new(0x5000))
                .unwrap()
                & 0x60,
            0x20,
            "the successful preflight sets A but not D"
        );
        assert_eq!(
            physical
                .load_u64(GuestPhysicalAddress::new(0x5008))
                .unwrap(),
            0
        );
        let mut operand = [0; 4];
        physical
            .load_bytes(GuestPhysicalAddress::new(0x7ffc), &mut operand)
            .unwrap();
        assert_eq!(
            operand, [0x11; 4],
            "failed preflight stores no operand bytes"
        );
    }

    fn cpl3_vector_test_memory(ram_len: u64, entries: [(u64, u64); 2]) -> crate::PhysicalMemory {
        let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), ram_len).unwrap();
        let map =
            MachineMemoryMap::new(36, vec![MemoryRegionSpec::new(ram, MemoryRegionKind::Ram)])
                .unwrap();
        let physical = crate::PhysicalMemory::new(&map, RamLimits::new(RAM_LEN)).unwrap();
        for (address, value) in [(0x1000, 0x2007), (0x2000, 0x3007), (0x3020, 0x5007)]
            .into_iter()
            .chain(entries)
        {
            physical
                .store_u64(GuestPhysicalAddress::new(address), value, |_| {})
                .unwrap();
        }
        physical
    }

    #[test]
    fn cpl3_vector_transfers_cover_exact_bytes_in_nonadjacent_frames() {
        let physical = cpl3_vector_test_memory(RAM_LEN, [(0x5000, 0x7007), (0x5008, 0x9007)]);
        let paging = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap();
        let user = AccessContext {
            privilege: PrivilegeLevel::Ring3,
            ..context()
        };
        let bytes = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        let mut writes = Vec::new();
        let mut committed = |range| writes.push(range);
        let mut memory = set_loop_adapter(
            &physical,
            None,
            paging,
            0x400000,
            &mut committed,
            None,
            None,
        );

        for address in [DATA_LINEAR + 0x80, DATA_LINEAR + 0xff8] {
            GuestMemory::write(&mut memory, address, user, &bytes).unwrap();
            let mut read = [0xff; 16];
            GuestMemory::read(&mut memory, address, user, &mut read).unwrap();
            assert_eq!(read, bytes);
        }
        drop(memory);
        for (address, length) in [(0x7080, 16), (0x7ff8, 8), (0x9000, 8)] {
            assert!(writes.contains(
                &GuestPhysicalRange::new(GuestPhysicalAddress::new(address), length).unwrap()
            ));
            assert_eq!(
                physical
                    .load_u8(GuestPhysicalAddress::new(address - 1))
                    .unwrap(),
                0
            );
            assert_eq!(
                physical
                    .load_u8(GuestPhysicalAddress::new(address + length))
                    .unwrap(),
                0
            );
        }
        for entry in [0x5000, 0x5008] {
            assert_eq!(
                physical.load_u64(GuestPhysicalAddress::new(entry)).unwrap() & 0x60,
                0x60
            );
        }
    }

    #[test]
    fn cpl3_vector_checks_both_pages_before_operand_commit() {
        let physical = cpl3_vector_test_memory(RAM_LEN, [(0x5000, 0x7007), (0x5008, 0x9003)]);
        let paging = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap();
        let user = AccessContext {
            privilege: PrivilegeLevel::Ring3,
            ..context()
        };
        let mut committed = |_| {};
        let mut memory = set_loop_adapter(
            &physical,
            None,
            paging,
            0x400000,
            &mut committed,
            None,
            None,
        );
        let address = DATA_LINEAR + 0xff8;
        let mut read = [0x55; 16];
        let load = GuestMemory::read(&mut memory, address, user, &mut read);
        let store = GuestMemory::write(&mut memory, address, user, &[0xaa; 16]);
        for (result, error_code) in [(load, 5), (store, 7)] {
            assert_eq!(
                result,
                Err(MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(error_code),
                    linear_address: Some(DATA_LINEAR + 0x1000),
                }))
            );
        }
        assert_eq!(read, [0x55; 16]);
        assert_eq!(
            physical
                .load_u64(GuestPhysicalAddress::new(0x7ff8))
                .unwrap(),
            0
        );
        assert_eq!(
            physical
                .load_u64(GuestPhysicalAddress::new(0x9000))
                .unwrap(),
            0
        );
        assert_eq!(
            physical
                .load_u64(GuestPhysicalAddress::new(0x5000))
                .unwrap()
                & 0x60,
            0x20
        );

        GuestMemory::write(&mut memory, DATA_LINEAR + 0x80, user, &[0xa5; 16]).unwrap();
        GuestMemory::read(&mut memory, DATA_LINEAR + 0x80, user, &mut read).unwrap();
        assert_eq!(read, [0xa5; 16]);
    }

    #[test]
    fn cpl3_vector_store_checks_entire_second_physical_span() {
        let physical = cpl3_vector_test_memory(0x9004, [(0x5000, 0x7007), (0x5008, 0x9007)]);
        let paging = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap();
        let user = AccessContext {
            privilege: PrivilegeLevel::Ring3,
            ..context()
        };
        let mut committed = |_| {};
        let mut memory = set_loop_adapter(
            &physical,
            None,
            paging,
            0x400000,
            &mut committed,
            None,
            None,
        );
        let store = GuestMemory::write(&mut memory, DATA_LINEAR + 0xff8, user, &[0xaa; 16]);
        assert!(matches!(store, Err(MemoryError::Backend(_))));
        let address = GuestPhysicalAddress::new(0x9000);
        let span = GuestPhysicalRange::new(address, 8).unwrap();
        assert_eq!(
            memory.access_error,
            Some(TrackedAccessError::OperandPhysical {
                address,
                source: crate::MemoryAccessError::AccessCrossesRegion(span),
            })
        );
        assert_eq!(
            physical
                .load_u64(GuestPhysicalAddress::new(0x7ff8))
                .unwrap(),
            0
        );
        assert_eq!(physical.load_u32(address).unwrap(), 0);
    }

    #[test]
    fn cpl3_vector_store_retains_translations_when_operand_aliases_page_table() {
        let physical = cpl3_vector_test_memory(RAM_LEN, [(0x5ff0, 0x5007), (0x5ff8, 0x9007)]);
        let paging = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap();
        let user = AccessContext {
            privilege: PrivilegeLevel::Ring3,
            ..context()
        };
        let mut committed = |_| {};
        let mut memory = set_loop_adapter(
            &physical,
            None,
            paging,
            0x400000,
            &mut committed,
            None,
            None,
        );
        let mut bytes = [0x5a; 16];
        bytes[..8].fill(0);

        GuestMemory::write(&mut memory, DATA_LINEAR + 0x1feff8, user, &bytes).unwrap();
        assert_eq!(
            physical
                .load_u64(GuestPhysicalAddress::new(0x5ff8))
                .unwrap(),
            0
        );
        assert_eq!(
            physical
                .load_u64(GuestPhysicalAddress::new(0x9000))
                .unwrap(),
            u64::from_le_bytes([0x5a; 8])
        );
    }

    /// S6-30c: the adapter of a vCPU holds a shared reference to the RAM, so the adapters of
    /// several vCPUs exist at once on one memory. A store through one is seen through the other,
    /// each adapter reports only its own committed writes, and adapters built on separate host
    /// threads, one per thread, share the same memory. Tables have A and D set already, so no
    /// table write takes part in the comparison.
    #[test]
    fn adapters_of_two_vcpus_share_one_ram() {
        let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), RAM_LEN).unwrap();
        let map =
            MachineMemoryMap::new(36, vec![MemoryRegionSpec::new(ram, MemoryRegionKind::Ram)])
                .unwrap();
        let mut physical = crate::PhysicalMemory::new(&map, RamLimits::new(RAM_LEN)).unwrap();
        physical.set_execution_mode(crate::ExecutionMode::Parallel);
        for (address, value) in [
            (0x1000, 0x2027),
            (0x2000, 0x3027),
            (0x3020, 0x5027),
            (0x5000, 0x7067),
        ] {
            physical
                .store_u64(GuestPhysicalAddress::new(address), value, |_| {})
                .unwrap();
        }
        let paging = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap();
        let operand = |address: u64, len: u64| {
            GuestPhysicalRange::new(GuestPhysicalAddress::new(address), len).unwrap()
        };

        let mut log_a = Vec::new();
        let mut log_b = Vec::new();
        {
            let mut note_a = |written| log_a.push(written);
            let mut note_b = |written| log_b.push(written);
            let mut a = set_loop_adapter(
                &physical,
                None,
                paging,
                0x0040_0000,
                &mut note_a,
                None,
                None,
            );
            let mut b = set_loop_adapter(
                &physical,
                None,
                paging,
                0x0040_0000,
                &mut note_b,
                None,
                None,
            );
            a.write(DATA_LINEAR + 0x10, context(), &[1, 2, 3, 4, 5, 6, 7, 8])
                .unwrap();
            let mut seen = [0u8; 8];
            b.read(DATA_LINEAR + 0x10, context(), &mut seen).unwrap();
            assert_eq!(seen, [1, 2, 3, 4, 5, 6, 7, 8], "B sees the store of A");
            b.write(DATA_LINEAR + 0x14, context(), &[9, 9, 9, 9])
                .unwrap();
            a.read(DATA_LINEAR + 0x10, context(), &mut seen).unwrap();
            assert_eq!(seen, [1, 2, 3, 4, 9, 9, 9, 9], "A sees the store of B");
        }
        assert_eq!(log_a, vec![operand(0x7010, 8)]);
        assert_eq!(log_b, vec![operand(0x7014, 4)]);
        let mut bytes = [0u8; 8];
        physical
            .load_bytes(GuestPhysicalAddress::new(0x7010), &mut bytes)
            .unwrap();
        assert_eq!(bytes, [1, 2, 3, 4, 9, 9, 9, 9]);

        // One adapter per host thread, all on the same memory (no `&mut` is possible here).
        const THREADS: u64 = 4;
        std::thread::scope(|scope| {
            for thread in 0..THREADS {
                let physical = &physical;
                scope.spawn(move || {
                    let mut committed = |_| {};
                    let mut adapter = set_loop_adapter(
                        physical,
                        None,
                        paging,
                        0x0040_0000,
                        &mut committed,
                        None,
                        None,
                    );
                    let at = DATA_LINEAR + 0x100 + thread * 8;
                    for round in 0..200u64 {
                        let value = (thread << 32 | round).to_le_bytes();
                        adapter.write(at, context(), &value).unwrap();
                        let mut back = [0u8; 8];
                        adapter.read(at, context(), &mut back).unwrap();
                        assert_eq!(back, value);
                    }
                });
            }
        });
        for thread in 0..THREADS {
            assert_eq!(
                physical.load_u64(GuestPhysicalAddress::new(0x7100 + thread * 8)),
                Ok(thread << 32 | 199)
            );
        }
    }

    /// S6-34 step 9, with a real second thread (not a `join` after the fact): one thread rewrites
    /// the PTE of the code page again and again (each version maps another frame, whose bytes name
    /// the version) while the reader runs the loop's own sequence on a cache with the `WriteWatch`
    /// policy: `revalidate` at the start of a call, a fetch through the cache (a hit, or a walk and
    /// an insert), `seal` at the end. The reader reads the number of the last write that has
    /// completed BEFORE it starts the call; the byte it fetches must come from that version or a
    /// later one. A stale entry (the bytes of an older version) that survived the write would be
    /// the failure of the order "bytes, then generation": the entry was made from the old PTE, the
    /// write came after, and the generation taken at the seal already counted it. The cache takes
    /// the generation before the bytes of the walk (`insert_with_generations`), so the write is
    /// always seen as newer than the entry.
    #[test]
    fn a_pte_rewritten_by_another_thread_is_never_served_stale_from_the_fetch_cache() {
        use std::sync::atomic::{AtomicU64, Ordering};
        const FRAMES: u64 = 256;
        const WRITES: u64 = 20_000;
        const LEN: u64 = 0x1_0000 + FRAMES * 0x1000;
        let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), LEN).unwrap();
        let map =
            MachineMemoryMap::new(36, vec![MemoryRegionSpec::new(ram, MemoryRegionKind::Ram)])
                .unwrap();
        let mut physical = crate::PhysicalMemory::new(&map, RamLimits::new(LEN)).unwrap();
        physical.set_execution_mode(crate::ExecutionMode::Parallel);
        let pte = |version: u64| 0x1_0000 + (version % FRAMES) * 0x1000 + 0x67;
        for (address, value) in [
            (0x1000, 0x2027),
            (0x2000, 0x3027),
            (0x3020, 0x5027),
            (0x5000, pte(0)),
        ] {
            physical
                .store_u64(GuestPhysicalAddress::new(address), value, |_| {})
                .unwrap();
        }
        for frame in 0..FRAMES {
            // The byte at offset 0 of the frame names the version it belongs to.
            physical
                .store_u8(
                    GuestPhysicalAddress::new(0x1_0000 + frame * 0x1000),
                    frame as u8,
                    |_| {},
                )
                .unwrap();
        }
        let paging = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap();
        let key = Some(crate::fetch_translation_cache::TranslationKey::new(
            paging,
            context(),
        ));
        let generation_of = |page: u64| {
            physical
                .code_page_generation(GuestPhysicalAddress::new(page << 12))
                .ok()
        };
        let published = AtomicU64::new(0);
        let cache = FetchTranslationCache::with_invalidation(FetchCacheInvalidation::WriteWatch)
            .with_insert_generations();
        let mut stale = 0u64;
        let mut hits_after_write = 0u64;
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for version in 1..=WRITES {
                    physical
                        .store_u64(GuestPhysicalAddress::new(0x5000), pte(version), |_| {})
                        .unwrap();
                    published.store(version, Ordering::Release);
                    // Let the reader run a call between most writes.
                    std::thread::yield_now();
                    for _ in 0..(version % 7) * 20 {
                        std::hint::spin_loop();
                    }
                }
            });
            loop {
                let seen = published.load(Ordering::Acquire);
                cache.revalidate(key, &generation_of);
                let mut committed = |_| {};
                let mut adapter = set_loop_adapter(
                    &physical,
                    None,
                    paging,
                    DATA_LINEAR,
                    &mut committed,
                    Some(&cache),
                    None,
                );
                // Twice: the second is a hit unless the first walk was refused.
                for _ in 0..2 {
                    let byte = u64::from(adapter.fetch(DATA_LINEAR, context()).unwrap());
                    let after = published.load(Ordering::Acquire);
                    // The byte names a version modulo the frames; it must be one in the window
                    // `seen..=after + 1` (the writes that completed before the call, the ones that
                    // came during it, and the one whose store is done but not yet published: the
                    // writer is preempted between the two). A window as wide as the ring cannot
                    // tell: not judged.
                    if after + 1 - seen < FRAMES {
                        let in_window = (seen..=after + 1).any(|version| version % FRAMES == byte);
                        if !in_window {
                            stale += 1;
                        }
                    }
                }
                drop(adapter);
                cache.seal(key, &generation_of);
                if seen >= WRITES {
                    break;
                }
            }
        });
        hits_after_write += cache.stats().hits;
        assert_eq!(
            stale, 0,
            "a fetch was served from a PTE older than a completed write"
        );
        assert!(
            hits_after_write > 0,
            "the cache served nothing: the test did not use it"
        );
        assert!(cache.stats().inserts > WRITES / 20, "{:?}", cache.stats());
        eprintln!("fetch cache stress: {:?}", cache.stats());
    }

    fn invlpg_state(target: u64) -> LongModeState {
        let mut state = LongModeState {
            rip: DATA_LINEAR,
            cr0: Some(CR0),
            cr3: Some(ROOT),
            cr4: Some(CR4),
            activity: Some(CpuActivity::Running),
            sti_shadow: Some(StiShadow::Clear),
            ..LongModeState::default()
        };
        state.gpr[od_isa::Register::Rbx as usize] = target;
        state
    }

    fn install_invlpg(bus: &mut MachineBus) {
        put(bus, 0x7000, 0x0000_0000_003b_010f);
    }

    #[test]
    fn step_paged_invlpg_retires_exact_linear_operand_and_forwards_ad_writes() {
        let mut bus = bus(0x7007);
        install_invlpg(&mut bus);
        let target = 0x0000_1234_5678_9abc;
        let mut state = invlpg_state(target);
        let mut callbacks = Vec::new();

        let report = step_paged_invlpg(
            &mut state,
            &mut bus,
            ROOT,
            EFER,
            context(),
            od_isa::INTEGER_FLAGS,
            |range| callbacks.push(range),
        )
        .unwrap();

        assert_eq!(
            report.cpu.exit,
            InvlpgExit::TranslationInvalidation(TranslationInvalidation::Invlpg {
                linear_address: target,
            })
        );
        assert_eq!(report.cpu.instructions_executed, 1);
        assert_eq!(report.cpu.rflags_defined, od_isa::INTEGER_FLAGS);
        assert_eq!(state.rip, DATA_LINEAR + 3);
        assert_eq!(report.page_fault, None);
        assert_eq!(report.access_error, None);
        assert_eq!(
            callbacks,
            [range(0x1000), range(0x2000), range(0x3020), range(0x5000)]
        );
    }

    #[test]
    fn step_paged_invlpg_noncanonical_operand_retires_without_invalidation() {
        let mut bus = bus(0x7007);
        install_invlpg(&mut bus);
        let mut state = invlpg_state(0x0000_8000_0000_0000);
        let mut callbacks = Vec::new();

        let report = step_paged_invlpg(
            &mut state,
            &mut bus,
            ROOT,
            EFER,
            context(),
            od_isa::INTEGER_FLAGS,
            |range| callbacks.push(range),
        )
        .unwrap();

        assert_eq!(report.cpu.exit, InvlpgExit::Retired);
        assert_eq!(report.cpu.instructions_executed, 1);
        assert_eq!(state.rip, DATA_LINEAR + 3);
        assert_eq!(report.page_fault, None);
        assert_eq!(report.access_error, None);
        assert_eq!(
            callbacks,
            [range(0x1000), range(0x2000), range(0x3020), range(0x5000)]
        );
    }

    #[test]
    fn step_paged_invlpg_rejection_does_not_fetch_or_change_cpu_state() {
        let mut bus = bus(0x7007);
        install_invlpg(&mut bus);
        let mut state = invlpg_state(0x1234);
        let before = state.clone();
        let mut callbacks = Vec::new();

        let report = step_paged_invlpg(
            &mut state,
            &mut bus,
            ROOT,
            EFER,
            context(),
            1 << 63,
            |range| callbacks.push(range),
        )
        .unwrap();

        assert_eq!(
            report.cpu.exit,
            InvlpgExit::Rejected(od_isa::PagedProbeRejection::InvalidRflagsDefined {
                value: 1 << 63,
            })
        );
        assert_eq!(report.cpu.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(report.page_fault, None);
        assert_eq!(report.access_error, None);
        assert!(callbacks.is_empty());
    }

    #[test]
    fn step_paged_invlpg_privilege_fault_preserves_completed_fetch_ad_callbacks() {
        let mut bus = bus(0x7007);
        install_invlpg(&mut bus);
        let mut state = invlpg_state(0x1234);
        let mut callbacks = Vec::new();
        let user_context = AccessContext {
            privilege: PrivilegeLevel::Ring3,
            ..context()
        };

        let report = step_paged_invlpg(
            &mut state,
            &mut bus,
            ROOT,
            EFER,
            user_context,
            od_isa::INTEGER_FLAGS,
            |range| callbacks.push(range),
        )
        .unwrap();

        assert_eq!(
            report.cpu.exit,
            InvlpgExit::Fault(od_isa::memory::ArchitecturalFault {
                vector: 13,
                error_code: Some(0),
                linear_address: None,
            })
        );
        assert_eq!(report.cpu.instructions_executed, 0);
        assert_eq!(state.rip, DATA_LINEAR);
        assert_eq!(report.page_fault, None);
        assert_eq!(report.access_error, None);
        assert_eq!(
            callbacks,
            [range(0x1000), range(0x2000), range(0x3020), range(0x5000)]
        );
    }

    #[test]
    fn step_paged_invlpg_fetch_fault_preserves_record_and_prior_ad_callbacks() {
        let mut bus = bus(0);
        let mut state = invlpg_state(0x1234);
        let before = state.clone();
        let mut callbacks = Vec::new();

        let report = step_paged_invlpg(
            &mut state,
            &mut bus,
            ROOT,
            EFER,
            context(),
            od_isa::INTEGER_FLAGS,
            |range| callbacks.push(range),
        )
        .unwrap();

        let record = report
            .page_fault
            .as_ref()
            .expect("tracked fetch page fault");
        assert_eq!(record.saved_rip(), DATA_LINEAR);
        assert_eq!(record.cr2_value(), DATA_LINEAR);
        assert_eq!(record.fault().vector, 14);
        assert_eq!(record.fault().error_code, Some(0x10));
        assert!(matches!(report.cpu.exit, InvlpgExit::Fault(ref fault) if fault == record.fault()));
        assert!(matches!(
            report.access_error,
            Some(TrackedAccessError::PageFault { record: ref error_record, .. })
                if error_record == record
        ));
        assert_eq!(report.cpu.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(callbacks, [range(0x1000), range(0x2000), range(0x3020)]);
    }

    #[test]
    fn failed_read_keeps_sentinel_buffer_for_page_fault_and_backend_failure() {
        let mut page_fault_bus = bus(0);
        let mut callbacks = Vec::new();
        let mut callback = |range| callbacks.push(range);
        let mut memory = adapter(&mut page_fault_bus, &mut callback);
        let mut destination = [0xa5; 8];
        let error = memory.read(DATA_LINEAR, context(), &mut destination);
        assert!(matches!(error, Err(MemoryError::Architectural(_))));
        assert_eq!(destination, [0xa5; 8]);
        drop(memory);
        drop(callback);
        assert_eq!(callbacks, [range(0x1000), range(0x2000), range(0x3020)]);

        let mut backend_bus = bus(0x30_000 | 7);
        callbacks.clear();
        let mut callback = |range| callbacks.push(range);
        let mut memory = adapter(&mut backend_bus, &mut callback);
        let mut destination = [0x5a; 8];
        let error = memory.read(DATA_LINEAR, context(), &mut destination);
        assert!(matches!(error, Err(MemoryError::Backend(_))));
        assert_eq!(destination, [0x5a; 8]);
        drop(memory);
        assert_eq!(
            callbacks,
            [range(0x1000), range(0x2000), range(0x3020), range(0x5000),]
        );
    }

    #[test]
    fn failed_second_cross_page_walk_keeps_caller_buffer_unchanged() {
        let mut bus = bus(0x9007);
        put(&mut bus, 0x5008, 0);
        let mut callbacks = Vec::new();
        let mut callback = |range| callbacks.push(range);
        let mut memory = adapter(&mut bus, &mut callback);
        let mut destination = [0x6d; 8];
        let error = memory.read(DATA_LINEAR + 0xff9, context(), &mut destination);
        assert!(matches!(error, Err(MemoryError::Architectural(_))));
        assert_eq!(destination, [0x6d; 8]);
        drop(memory);
        drop(callback);
        assert_eq!(
            callbacks,
            [range(0x1000), range(0x2000), range(0x3020), range(0x5000)]
        );

        let mut word_bus = self::bus(0x9007);
        put(&mut word_bus, 0x5008, 0);
        callbacks.clear();
        let mut callback = |range| callbacks.push(range);
        let mut memory = adapter(&mut word_bus, &mut callback);
        memory.word_read_profile = true;
        let mut destination = [0x4c; 2];
        let error = memory.read(DATA_LINEAR + 0xfff, context(), &mut destination);
        assert!(matches!(error, Err(MemoryError::Architectural(_))));
        assert_eq!(destination, [0x4c; 2]);
        drop(memory);
        drop(callback);
        assert_eq!(
            callbacks,
            [range(0x1000), range(0x2000), range(0x3020), range(0x5000)]
        );
    }

    #[test]
    fn narrow_stores_belong_to_the_cpl0_set_adapter_alone() {
        // The adapter without the flag keeps its fixed store width: this is the guard of the flag.
        for len in [1usize, 2, 4] {
            let mut bus = bus(0x9007);
            let mut callback = |_| {};
            let mut memory = adapter(&mut bus, &mut callback);
            let error = memory.write(DATA_LINEAR, context(), &vec![0x5a; len]);
            assert!(
                matches!(&error, Err(MemoryError::Backend(message)) if message.contains("non-qword writes")),
                "{error:?}"
            );
            assert_eq!(memory.page_fault, None);
        }
        // The dword-store profile keeps its own message and width.
        let mut bus = self::bus(0x9007);
        let mut callback = |_| {};
        let mut memory = adapter(&mut bus, &mut callback);
        memory.dword_write_profile = true;
        let error = memory.write(DATA_LINEAR, context(), &[0x5a]);
        assert!(
            matches!(&error, Err(MemoryError::Backend(message)) if message.contains("non-dword writes")),
            "{error:?}"
        );
        // The set's adapter names a wrong width differently from the shared profiles.
        let mut bus = self::bus(0x9007);
        let mut callback = |_| {};
        let mut memory = adapter(&mut bus, &mut callback);
        memory.cpl0_set_narrow_write = true;
        let error = memory.write(DATA_LINEAR, context(), &[0; 3]);
        assert!(
            matches!(&error, Err(MemoryError::Backend(message)) if message.contains("write width")),
            "{error:?}"
        );
        // The public JIT adapter is read-only: it refuses every store, narrow ones included.
        for len in [1usize, 2, 4] {
            let mut bus = self::bus(0x9007);
            let scope = with_paged_mov_memory(
                &mut bus,
                Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap(),
                0x0040_0000,
                |memory| memory.write(DATA_LINEAR, context(), &vec![0x5a; len]),
            );
            assert!(
                matches!(&scope.callback_result, Err(MemoryError::Backend(message)) if message.contains("does not support write")),
                "{:?}",
                scope.callback_result
            );
            assert!(scope.committed_writes.is_empty());
        }
    }

    #[test]
    fn cross_page_stores_belong_to_the_cpl0_set_adapter_alone() {
        // S4-38L4h guard: without `cpl0_set_cross_page_store` every profile keeps the typed
        // `CrossesPage` before any table or operand access, narrow stores and the large-RAM
        // walk included; no byte is written.
        let crossing = |len: usize| DATA_LINEAR + 0x1000 - (len as u64 - 1);
        for (narrow, large) in [(false, false), (true, false), (true, true), (false, true)] {
            for len in [8usize, 4, 2] {
                if !narrow && len != 8 {
                    continue;
                }
                let mut bus = bus(0x9007);
                let mut callbacks = Vec::new();
                let mut callback = |range| callbacks.push(range);
                let mut memory = adapter(&mut bus, &mut callback);
                memory.cpl0_set_narrow_write = narrow;
                memory.large_ram_profile = large;
                let error = memory.write(crossing(len), context(), &vec![0x5a; len]);
                assert!(error.is_err(), "{narrow} {large} {len}: {error:?}");
                assert_eq!(
                    memory.access_error,
                    Some(TrackedAccessError::CrossesPage {
                        address: GuestLinearAddress::new(crossing(len)),
                        width: len as u8
                    }),
                    "{narrow} {large} {len}"
                );
                assert_eq!(memory.page_fault, None);
                drop(memory);
                drop(callback);
                assert!(callbacks.is_empty(), "{narrow} {large} {len}");
            }
        }
    }

    #[test]
    fn cpl0_set_adapter_writes_exactly_the_source_width() {
        let mut bus = bus(0x9007);
        put(&mut bus, 0x9000, 0xaabb_ccdd_1122_3344);
        let mut callbacks = Vec::new();
        let mut callback = |range| callbacks.push(range);
        let mut memory = adapter(&mut bus, &mut callback);
        memory.cpl0_set_narrow_write = true;
        memory.write(DATA_LINEAR, context(), &[0x5a]).unwrap();
        memory
            .write(DATA_LINEAR + 2, context(), &[0x01, 0x02])
            .unwrap();
        memory
            .write(DATA_LINEAR + 4, context(), &[9, 8, 7, 6])
            .unwrap();
        // Three source lengths are the only admitted narrow widths.
        assert!(memory.write(DATA_LINEAR, context(), &[0; 3]).is_err());
        drop(memory);
        drop(callback);
        let value = bus
            .read_physical(GuestPhysicalAddress::new(0x9000), AccessWidth::Qword)
            .unwrap();
        assert_eq!(value.to_le_bytes(), [0x5a, 0x33, 0x01, 0x02, 9, 8, 7, 6]);
        // The data writes are exactly the source widths at the source addresses (no read-modify-
        // write of the surrounding qword); the others are the walker's own table writes.
        let data_writes: Vec<_> = callbacks
            .iter()
            .filter(|range| range.start().get() >= 0x9000)
            .map(|range| (range.start().get(), range.len()))
            .collect();
        assert_eq!(data_writes, [(0x9000, 1), (0x9002, 2), (0x9004, 4)]);
        assert!(
            callbacks
                .iter()
                .filter(|range| range.start().get() < 0x9000)
                .all(
                    |range| [0x1000, 0x2000, 0x3020, 0x5000].contains(&range.start().get())
                        && range.len() == 8
                )
        );
    }

    #[test]
    fn cpl3_byte_write_obeys_the_user_supervisor_and_write_bits() {
        // S4-39f6: the byte store of a CPL3 `MOV m8, imm8`. A supervisor leaf (P, W) refuses it
        // with #PF P|W|U (0x7) and the page keeps its byte; a user leaf (P, W, U) takes it.
        let user = AccessContext {
            privilege: PrivilegeLevel::Ring3,
            ..context()
        };
        let mut bus = bus(0x9003);
        put(&mut bus, 0x9000, 0xaabb_ccdd_1122_3344);
        let mut callback = |_| {};
        let mut memory = adapter(&mut bus, &mut callback);
        memory.cpl0_set_narrow_write = true;
        let error = memory.write(DATA_LINEAR + 1, user, &[0x00]);
        assert!(
            matches!(
                error,
                Err(MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(7),
                    linear_address: Some(address),
                })) if address == DATA_LINEAR + 1
            ),
            "{error:?}"
        );
        assert!(memory.page_fault.is_some());
        drop(memory);
        let value = bus
            .read_physical(GuestPhysicalAddress::new(0x9000), AccessWidth::Qword)
            .unwrap();
        assert_eq!(value, 0xaabb_ccdd_1122_3344);
        // S4-39f7: a refused write leaves the dirty bit (bit 6) of the leaf clear.
        let leaf = bus
            .read_physical(GuestPhysicalAddress::new(0x5000), AccessWidth::Qword)
            .unwrap();
        assert_eq!(leaf & 0x40, 0, "dirty bit after a refused write");

        // A user leaf without W (P, U): the CPL3 write is refused with the same code 0x7.
        let mut bus = self::bus(0x9005);
        put(&mut bus, 0x9000, 0xaabb_ccdd_1122_3344);
        let mut callback = |_| {};
        let mut memory = adapter(&mut bus, &mut callback);
        memory.cpl0_set_narrow_write = true;
        let error = memory.write(DATA_LINEAR + 1, user, &[0x00]);
        assert!(
            matches!(
                error,
                Err(MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(7),
                    ..
                }))
            ),
            "{error:?}"
        );
        drop(memory);
        let value = bus
            .read_physical(GuestPhysicalAddress::new(0x9000), AccessWidth::Qword)
            .unwrap();
        assert_eq!(value, 0xaabb_ccdd_1122_3344);
        // S4-39f7: a refused write leaves the dirty bit (bit 6) of the leaf clear.
        let leaf = bus
            .read_physical(GuestPhysicalAddress::new(0x5000), AccessWidth::Qword)
            .unwrap();
        assert_eq!(leaf & 0x40, 0, "dirty bit after a refused write");

        let mut bus = self::bus(0x9007);
        put(&mut bus, 0x9000, 0xaabb_ccdd_1122_3344);
        let mut callback = |_| {};
        let mut memory = adapter(&mut bus, &mut callback);
        memory.cpl0_set_narrow_write = true;
        memory.write(DATA_LINEAR + 1, user, &[0x00]).unwrap();
        assert_eq!(memory.page_fault, None);
        drop(memory);
        let value = bus
            .read_physical(GuestPhysicalAddress::new(0x9000), AccessWidth::Qword)
            .unwrap();
        assert_eq!(value, 0xaabb_ccdd_1122_0044);
        // S4-39f7: the successful write sets the dirty bit of the leaf (the accessed bit too).
        let leaf = bus
            .read_physical(GuestPhysicalAddress::new(0x5000), AccessWidth::Qword)
            .unwrap();
        assert_eq!(leaf & 0x60, 0x60, "accessed and dirty bits after a write");
    }

    #[test]
    fn byte_reads_belong_to_the_cpl0_set_adapter_alone() {
        // Without the flag every shared profile keeps refusing a one-byte read.
        let mut bus = bus(0x9007);
        let mut callback = |_| {};
        let mut memory = adapter(&mut bus, &mut callback);
        let mut destination = [0x77; 1];
        let error = memory.read(DATA_LINEAR, context(), &mut destination);
        assert!(
            matches!(&error, Err(MemoryError::Backend(message)) if message.contains("read width")),
            "{error:?}"
        );
        assert_eq!(destination, [0x77]);
        assert_eq!(memory.page_fault, None);
        // The narrow-profile flags do not admit a byte either.
        memory.word_read_profile = true;
        assert!(memory.read(DATA_LINEAR, context(), &mut [0; 1]).is_err());
        memory.word_read_profile = false;
        memory.dword_read_profile = true;
        assert!(memory.read(DATA_LINEAR, context(), &mut [0; 1]).is_err());
        // The public (probe) adapter refuses it too.
        let mut bus = self::bus(0x9007);
        let scope = with_paged_mov_memory(
            &mut bus,
            Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap(),
            0x0040_0000,
            |memory| memory.read(DATA_LINEAR, context(), &mut [0; 1]),
        );
        assert!(
            matches!(&scope.callback_result, Err(MemoryError::Backend(message)) if message.contains("read width")),
            "{:?}",
            scope.callback_result
        );
        // The set's adapter still refuses widths it does not admit (3 bytes).
        let mut bus = self::bus(0x9007);
        let mut callback = |_| {};
        let mut memory = adapter(&mut bus, &mut callback);
        memory.cpl0_set_byte_read = true;
        let error = memory.read(DATA_LINEAR, context(), &mut [0; 3]);
        assert!(
            matches!(&error, Err(MemoryError::Backend(message)) if message.contains("read width")),
            "{error:?}"
        );
    }

    #[test]
    fn cpl0_set_adapter_reads_one_byte_with_the_ordinary_read_ad_rules() {
        let mut bus = bus(0x9007);
        put(&mut bus, 0x9000, 0xaabb_ccdd_1122_3344);
        let mut callbacks = Vec::new();
        let mut callback = |range| callbacks.push(range);
        let mut memory = adapter(&mut bus, &mut callback);
        memory.cpl0_set_byte_read = true;
        for (offset, expected) in [(0u64, 0x44u8), (1, 0x33), (7, 0xaa)] {
            let mut destination = [0; 1];
            memory
                .read(DATA_LINEAR + offset, context(), &mut destination)
                .unwrap();
            assert_eq!(destination, [expected]);
        }
        drop(memory);
        drop(callback);
        // A read sets Accessed on the leaf PTE and never Dirty; the data page is untouched.
        let pte = bus
            .read_physical(GuestPhysicalAddress::new(0x5000), AccessWidth::Qword)
            .unwrap();
        assert_eq!(pte, 0x9027);
        let data = bus
            .read_physical(GuestPhysicalAddress::new(0x9000), AccessWidth::Qword)
            .unwrap();
        assert_eq!(data, 0xaabb_ccdd_1122_3344);
        assert!(
            callbacks
                .iter()
                .all(|range| range.start().get() < 0x9000 && range.len() == 8),
            "{callbacks:?}"
        );
    }

    #[test]
    fn cpl0_set_adapter_reads_the_last_byte_of_a_page() {
        let mut bus = bus(0x9007);
        put(&mut bus, 0x9ff8, 0x5c00_0000_0000_0000);
        let mut callback = |_| {};
        let mut memory = adapter(&mut bus, &mut callback);
        memory.cpl0_set_byte_read = true;
        let mut destination = [0; 1];
        memory
            .read(DATA_LINEAR + 0xfff, context(), &mut destination)
            .unwrap();
        assert_eq!(destination, [0x5c]);
        assert_eq!(memory.page_fault, None);
    }

    #[test]
    fn a_descriptor_table_read_at_cpl3_has_supervisor_page_rights_but_an_explicit_read_has_not() {
        // S4-39d: the leaf PTE is supervisor-only (P, W). A CPL3 explicit read is a #PF with P
        // and U; the same read as a descriptor-table access (IDT, GDT, TSS) succeeds, and a CPL3
        // frame store made at the new CPL0 is allowed.
        let mut bus = bus(0x9003);
        put(&mut bus, 0x9000, 0x1122_3344_5566_7788);
        let mut callback = |_| {};
        let mut memory = adapter(&mut bus, &mut callback);
        let user = AccessContext {
            privilege: PrivilegeLevel::Ring3,
            ..context()
        };
        let mut destination = [0; 8];
        let error = memory.read(DATA_LINEAR, user, &mut destination);
        assert!(
            matches!(
                error,
                Err(MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(5),
                    ..
                }))
            ),
            "{error:?}"
        );
        assert!(memory.page_fault.is_some());
        memory.page_fault = None;
        memory
            .read(DATA_LINEAR, user.descriptor_table(), &mut destination)
            .unwrap();
        assert_eq!(u64::from_le_bytes(destination), 0x1122_3344_5566_7788);
        assert_eq!(memory.page_fault, None);
        let kernel_stack = AccessContext {
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::ImplicitStack,
            ..user
        };
        memory
            .write(DATA_LINEAR + 8, kernel_stack, &7u64.to_le_bytes())
            .unwrap();
    }

    #[test]
    fn word_reads_belong_to_the_cpl0_set_adapter_alone() {
        // Without the flag every shared profile keeps refusing a two-byte read.
        let mut bus = bus(0x9007);
        let mut callback = |_| {};
        let mut memory = adapter(&mut bus, &mut callback);
        let mut destination = [0x77; 2];
        let error = memory.read(DATA_LINEAR, context(), &mut destination);
        assert!(
            matches!(&error, Err(MemoryError::Backend(message)) if message.contains("read width")),
            "{error:?}"
        );
        assert_eq!(destination, [0x77; 2]);
        assert_eq!(memory.page_fault, None);
        // The large-RAM profile and the byte/dword flags do not admit a word either.
        memory.large_ram_profile = true;
        memory.cpl0_set_byte_read = true;
        memory.cpl0_set_dword_read = true;
        assert!(memory.read(DATA_LINEAR, context(), &mut [0; 2]).is_err());
        memory.dword_read_profile = true;
        assert!(memory.read(DATA_LINEAR, context(), &mut [0; 2]).is_err());
        // The public (probe) adapter refuses it too.
        let mut bus = self::bus(0x9007);
        let scope = with_paged_mov_memory(
            &mut bus,
            Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap(),
            0x0040_0000,
            |memory| memory.read(DATA_LINEAR, context(), &mut [0; 2]),
        );
        assert!(
            matches!(&scope.callback_result, Err(MemoryError::Backend(message)) if message.contains("read width")),
            "{:?}",
            scope.callback_result
        );
        // The word-read probe profile keeps working on its own flag, with no set flag.
        let mut bus = self::bus(0x9007);
        put(&mut bus, 0x9000, 0xaabb_ccdd_1122_3344);
        let mut callback = |_| {};
        let mut memory = adapter(&mut bus, &mut callback);
        memory.word_read_profile = true;
        let mut destination = [0; 2];
        memory
            .read(DATA_LINEAR, context(), &mut destination)
            .unwrap();
        assert_eq!(destination, [0x44, 0x33]);
        // The set's adapter still refuses widths it does not admit (3 bytes).
        let mut bus = self::bus(0x9007);
        let mut callback = |_| {};
        let mut memory = adapter(&mut bus, &mut callback);
        memory.cpl0_set_word_read = true;
        let error = memory.read(DATA_LINEAR, context(), &mut [0; 3]);
        assert!(
            matches!(&error, Err(MemoryError::Backend(message)) if message.contains("read width")),
            "{error:?}"
        );
    }

    #[test]
    fn cpl0_set_adapter_reads_one_word_with_the_ordinary_read_ad_rules() {
        let mut bus = bus(0x9007);
        put(&mut bus, 0x9000, 0xaabb_ccdd_1122_3344);
        let mut callbacks = Vec::new();
        let mut callback = |range| callbacks.push(range);
        let mut memory = adapter(&mut bus, &mut callback);
        memory.large_ram_profile = true;
        memory.cpl0_set_word_read = true;
        for (offset, expected) in [(0u64, [0x44u8, 0x33]), (1, [0x33, 0x22]), (6, [0xbb, 0xaa])] {
            let mut destination = [0; 2];
            memory
                .read(DATA_LINEAR + offset, context(), &mut destination)
                .unwrap();
            assert_eq!(destination, expected);
        }
        drop(memory);
        drop(callback);
        // A read sets Accessed on the leaf PTE and never Dirty; the data page is untouched.
        let pte = bus
            .read_physical(GuestPhysicalAddress::new(0x5000), AccessWidth::Qword)
            .unwrap();
        assert_eq!(pte, 0x9027);
        let data = bus
            .read_physical(GuestPhysicalAddress::new(0x9000), AccessWidth::Qword)
            .unwrap();
        assert_eq!(data, 0xaabb_ccdd_1122_3344);
        assert!(
            callbacks
                .iter()
                .all(|range| range.start().get() < 0x9000 && range.len() == 8),
            "{callbacks:?}"
        );
    }

    #[test]
    fn cpl0_set_adapter_reads_the_last_word_of_a_page_and_splits_a_crossing_word() {
        let mut bus = bus(0x9007);
        put(&mut bus, 0x9ff8, 0x5c5d_0000_0000_0000);
        let mut callback = |_| {};
        let mut memory = adapter(&mut bus, &mut callback);
        memory.large_ram_profile = true;
        memory.cpl0_set_word_read = true;
        let mut destination = [0; 2];
        memory
            .read(DATA_LINEAR + 0xffe, context(), &mut destination)
            .unwrap();
        assert_eq!(destination, [0x5d, 0x5c]);
        assert_eq!(memory.page_fault, None);
        // The word on the last byte: the page of its second byte is not mapped here, so the
        // read is a data `#PF` at the first byte of that page (not a backend error), and the
        // caller's buffer is unchanged.
        let mut destination = [0x77; 2];
        let error = memory.read(DATA_LINEAR + 0xfff, context(), &mut destination);
        assert!(
            matches!(error, Err(MemoryError::Architectural(_))),
            "{error:?}"
        );
        assert_eq!(destination, [0x77; 2]);
        assert_eq!(memory.memory_error, None);
        let record = memory.page_fault.take().expect("data fault");
        assert_eq!(record.fault().linear_address, Some(DATA_LINEAR + 0x1000));
        assert_eq!(record.fault().error_code, Some(0));
        assert_eq!(record.saved_rip(), 0x0040_0000);
        // Without the flag the shared large-RAM walk keeps the typed `CrossesPage` refusal for
        // every width the profile reads across the boundary.
        let mut bus = self::bus(0x9007);
        let mut callback = |_| {};
        let mut memory = adapter(&mut bus, &mut callback);
        memory.large_ram_profile = true;
        memory.cpl0_set_dword_read = true;
        let mut destination = [0x77; 4];
        assert!(
            memory
                .read(DATA_LINEAR + 0xffe, context(), &mut destination)
                .is_err()
        );
        assert!(matches!(
            memory.memory_error,
            Some(TrackedAccessError::CrossesPage { .. })
        ));
    }

    #[test]
    fn cross_page_dword_and_qword_reads_belong_to_the_cpl0_set_adapter_alone() {
        // S4-38L5e guard: without `cpl0_set_cross_page_read` (or without the large-RAM
        // profile) a dword or qword across a 4 KiB boundary keeps the typed `CrossesPage`,
        // and the destination is untouched. With the flag and a missing second page it is a
        // precise data `#PF` (W = 0) at the first byte of that page, destination untouched.
        for len in [8usize, 4] {
            let at = DATA_LINEAR + 0x1000 - 3;
            for (flag, large) in [(false, true), (true, false), (false, false)] {
                let mut bus = bus(0x9007);
                let mut callback = |_| {};
                let mut memory = adapter(&mut bus, &mut callback);
                memory.large_ram_profile = large;
                memory.cpl0_set_dword_read = true;
                memory.cpl0_set_cross_page_read = flag;
                let mut destination = vec![0x77; len];
                let result = memory.read(at, context(), &mut destination);
                assert!(
                    !matches!(result, Ok(())),
                    "{len} {flag} {large}: a crossing read must not succeed here"
                );
                if large {
                    assert!(
                        matches!(
                            memory.memory_error,
                            Some(TrackedAccessError::CrossesPage { .. })
                        ),
                        "{len} {flag} {large}: {:?}",
                        memory.memory_error
                    );
                } else {
                    // Without the large-RAM profile the old two-page path runs, not the byte-wise
                    // read: its error is never the large-RAM `CrossesPage` of the shared walk.
                    assert!(result.is_err(), "{len} {flag} {large}: {result:?}");
                    assert!(
                        !matches!(
                            memory.memory_error,
                            Some(TrackedAccessError::CrossesPage { .. })
                        ),
                        "{len} {flag} {large}: {:?}",
                        memory.memory_error
                    );
                }
                assert_eq!(destination, vec![0x77; len]);
            }
            let mut bus = bus(0x9007);
            let mut callback = |_| {};
            let mut memory = adapter(&mut bus, &mut callback);
            memory.large_ram_profile = true;
            memory.cpl0_set_dword_read = true;
            memory.cpl0_set_cross_page_read = true;
            let mut destination = vec![0x77; len];
            let result = memory.read(at, context(), &mut destination);
            assert!(
                matches!(result, Err(MemoryError::Architectural(_))),
                "{len}: {result:?}"
            );
            assert_eq!(memory.memory_error, None);
            assert_eq!(destination, vec![0x77; len]);
            let record = memory.page_fault.take().expect("data fault");
            assert_eq!(record.fault().linear_address, Some(DATA_LINEAR + 0x1000));
            assert_eq!(record.fault().error_code, Some(0));
        }
    }

    // ---- S4-38L3: the exits of the CPL0 set loop (`handle_set_exit`) ----

    /// The state the walker snapshot is rebuilt from: CR0/CR4 as in the other tests and
    /// the CPU-owned CR3.
    fn set_loop_state(cr3: u64) -> LongModeState {
        LongModeState {
            cr0: Some(CR0),
            cr3: Some(cr3),
            cr4: Some(CR4),
            ..LongModeState::default()
        }
    }

    /// The loop variables of `run_cpl0_set_paged_2m_ram` at its start: the walker on `ROOT`,
    /// nothing reported yet.
    struct SetLoop {
        paging: Long64ProbeConfig,
        efer: u64,
        reported_cr3: u64,
        changes: Vec<TranslationContextChange>,
    }

    impl SetLoop {
        fn new() -> Self {
            Self {
                paging: Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap(),
                efer: EFER,
                reported_cr3: ROOT,
                changes: Vec::new(),
            }
        }

        fn handle(
            &mut self,
            exit: Cpl0ProfileSetExit<crate::BusAccessError>,
            instructions_executed: u64,
            state: &LongModeState,
        ) -> Option<PagedCpl0SetExit> {
            handle_set_exit(
                exit,
                instructions_executed,
                state,
                &mut self.efer,
                &mut self.paging,
                &mut self.reported_cr3,
                &mut self.changes,
            )
        }
    }

    #[test]
    fn set_loop_a_retired_mov_cr3_rebuilds_the_walker_and_is_reported() {
        let mut run = SetLoop::new();
        let effect = TranslationContextChange::Cr3Write {
            old_cr3: ROOT,
            new_cr3: 0x2000,
        };
        let exit = run.handle(
            Cpl0ProfileSetExit::TranslationContextChange(effect),
            1,
            &set_loop_state(0x2000),
        );
        assert_eq!(exit, None);
        assert_eq!(
            run.paging,
            Long64ProbeConfig::new(CR0, 0x2000, CR4, EFER).unwrap()
        );
        assert_eq!((run.reported_cr3, run.changes), (0x2000, vec![effect]));
    }

    #[test]
    fn set_loop_a_cr3_effect_that_disagrees_with_the_cpu_still_reports_the_retired_write() {
        // The CPU says CR3 is 0x3000 although the effect names 0x2000: the walker is not
        // touched, but the MOV CR3 retired, so the report carries it.
        let mut run = SetLoop::new();
        let effect = TranslationContextChange::Cr3Write {
            old_cr3: ROOT,
            new_cr3: 0x2000,
        };
        let exit = run.handle(
            Cpl0ProfileSetExit::TranslationContextChange(effect),
            1,
            &set_loop_state(0x3000),
        );
        assert_eq!(
            exit,
            Some(PagedCpl0SetExit::Cr3EffectMismatch {
                active_cr3: ROOT,
                effect_old_cr3: ROOT,
                cpu_cr3: Some(0x3000),
                effect_new_cr3: 0x2000,
            })
        );
        assert_eq!(
            run.paging,
            Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap(),
            "the walker was not replaced"
        );
        assert_eq!((run.reported_cr3, run.changes), (0x2000, vec![effect]));
    }

    #[test]
    fn set_loop_a_cr3_effect_whose_old_value_is_not_the_walker_is_a_mismatch() {
        let mut run = SetLoop::new();
        let effect = TranslationContextChange::Cr3Write {
            old_cr3: 0x9000,
            new_cr3: 0x2000,
        };
        let exit = run.handle(
            Cpl0ProfileSetExit::TranslationContextChange(effect),
            1,
            &set_loop_state(0x2000),
        );
        assert_eq!(
            exit,
            Some(PagedCpl0SetExit::Cr3EffectMismatch {
                active_cr3: ROOT,
                effect_old_cr3: 0x9000,
                cpu_cr3: Some(0x2000),
                effect_new_cr3: 0x2000,
            })
        );
        assert_eq!(
            run.paging,
            Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap(),
            "the walker was not replaced"
        );
        assert_eq!((run.reported_cr3, run.changes), (0x2000, vec![effect]));
    }

    #[test]
    fn set_loop_a_rejected_snapshot_for_the_new_cr3_still_reports_the_retired_write() {
        // CR3 bit 36 is outside the 36-bit physical address space of the walker.
        let new_cr3 = ROOT | (1 << 36);
        let mut run = SetLoop::new();
        let effect = TranslationContextChange::Cr3Write {
            old_cr3: ROOT,
            new_cr3,
        };
        let exit = run.handle(
            Cpl0ProfileSetExit::TranslationContextChange(effect),
            1,
            &set_loop_state(new_cr3),
        );
        assert_eq!(
            exit,
            Some(PagedCpl0SetExit::Cr3SnapshotRejected {
                effect,
                error: crate::ProbeConfigError::InvalidCr3 {
                    value: new_cr3,
                    offending_bits: 1 << 36,
                },
            })
        );
        assert_eq!(
            run.paging,
            Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap(),
            "the walker was not replaced"
        );
        assert_eq!((run.reported_cr3, run.changes), (new_cr3, vec![effect]));
    }

    #[test]
    fn set_loop_an_efer_write_rebuilds_the_walker_for_nxe_only_and_is_not_a_cr3_change() {
        // S2-25 NXE slice: the guest value carries SCE, the loop's walker EFER does not; only the
        // NXE bit follows, and nothing is added to the CR3 changes.
        let mut run = SetLoop::new();
        let nxe = 1u64 << 11;
        let effect = TranslationContextChange::EferWrite {
            old_efer: EFER | 1,
            new_efer: (EFER | 1) & !nxe,
        };
        let exit = run.handle(
            Cpl0ProfileSetExit::TranslationContextChange(effect),
            1,
            &set_loop_state(ROOT),
        );
        assert_eq!(exit, None);
        assert_eq!(run.efer, EFER & !nxe);
        assert_eq!(
            run.paging,
            Long64ProbeConfig::new(CR0, ROOT, CR4, EFER & !nxe).unwrap()
        );
        assert_eq!((run.reported_cr3, run.changes.clone()), (ROOT, Vec::new()));
        // Setting it again follows the same way.
        let effect = TranslationContextChange::EferWrite {
            old_efer: EFER & !nxe,
            new_efer: EFER,
        };
        let exit = run.handle(
            Cpl0ProfileSetExit::TranslationContextChange(effect),
            1,
            &set_loop_state(ROOT),
        );
        assert_eq!(exit, None);
        assert_eq!(
            run.paging,
            Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap()
        );
        // An effect whose old NXE is not the walker's, or that changes no NXE, is unexpected.
        for effect in [
            TranslationContextChange::EferWrite {
                old_efer: EFER & !nxe,
                new_efer: EFER,
            },
            TranslationContextChange::EferWrite {
                old_efer: EFER,
                new_efer: EFER | 1,
            },
        ] {
            let exit = run.handle(
                Cpl0ProfileSetExit::TranslationContextChange(effect),
                1,
                &set_loop_state(ROOT),
            );
            assert_eq!(
                exit,
                Some(PagedCpl0SetExit::UnexpectedContextChange { effect })
            );
            assert_eq!(run.efer, EFER, "the walker EFER was not replaced");
        }
    }

    #[test]
    fn set_loop_a_cr0_write_is_an_unexpected_context_change_and_nothing_is_reported() {
        let mut run = SetLoop::new();
        let effect = TranslationContextChange::Cr0Write {
            old_cr0: CR0,
            new_cr0: CR0 & !(1 << 16),
        };
        let exit = run.handle(
            Cpl0ProfileSetExit::TranslationContextChange(effect),
            1,
            &set_loop_state(ROOT),
        );
        assert_eq!(
            exit,
            Some(PagedCpl0SetExit::UnexpectedContextChange { effect })
        );
        assert_eq!(
            run.paging,
            Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap(),
            "the walker was not replaced"
        );
        assert_eq!((run.reported_cr3, run.changes), (ROOT, Vec::new()));
    }

    #[test]
    fn set_loop_a_port_error_ends_the_run_with_that_error() {
        let mut run = SetLoop::new();
        let error = || crate::BusAccessError::UnmappedPort(crate::PortAddress::new(0x1234));
        let exit = run.handle(
            Cpl0ProfileSetExit::PortIo(error()),
            0,
            &set_loop_state(ROOT),
        );
        assert_eq!(exit, Some(PagedCpl0SetExit::PortIo(error())));
        assert_eq!((run.reported_cr3, run.changes), (ROOT, Vec::new()));
    }

    #[test]
    fn set_loop_a_user_entry_ends_the_run_with_its_frame_and_changes_nothing_else() {
        // S4-38L9I: the IRETQ user_entry exit is a stop carrying the frame, also when it is the
        // first instruction of the call (nothing retired); the walker and the CR3 report stay.
        let entry = IretqUserEntry {
            rip: 0x0000_7fff_0000_1000,
            cs: 0x33,
            rflags: 0x246,
            rsp: 0x0000_7fff_ffff_f000,
            ss: 0x2b,
        };
        for retired in [0, 7] {
            let mut run = SetLoop::new();
            let exit = run.handle(
                Cpl0ProfileSetExit::UserEntry(entry),
                retired,
                &set_loop_state(ROOT),
            );
            assert_eq!(exit, Some(PagedCpl0SetExit::UserEntry(entry)));
            assert_eq!(
                run.paging,
                Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap(),
                "the walker was not touched"
            );
            assert_eq!((run.reported_cr3, run.changes), (ROOT, Vec::new()));
        }
    }

    #[test]
    fn set_loop_the_one_instruction_budget_continues_only_when_the_instruction_retired() {
        let mut run = SetLoop::new();
        let budget = || Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted);
        assert_eq!(run.handle(budget(), 1, &set_loop_state(ROOT)), None);
        assert_eq!(
            run.handle(budget(), 0, &set_loop_state(ROOT)),
            Some(PagedCpl0SetExit::Run(RunExit::InstructionBudgetExhausted))
        );
        assert_eq!(
            run.handle(
                Cpl0ProfileSetExit::Run(RunExit::Halted),
                1,
                &set_loop_state(ROOT)
            ),
            Some(PagedCpl0SetExit::Run(RunExit::Halted))
        );
    }

    #[test]
    fn page_crossing_fetch_belongs_to_the_cpl0_set_adapter_alone() {
        // The shared large-RAM profile keeps its one-code-page window: the byte on the next
        // page is a typed backend `CrossesPage` before any walk of that page.
        let mut machine = bus(0x6007);
        let mut callback = |_| {};
        let mut memory = adapter(&mut machine, &mut callback);
        memory.large_ram_profile = true;
        memory.instruction_rip = DATA_LINEAR + 0xffe;
        assert!(matches!(
            memory.fetch(DATA_LINEAR + 0xfff, context()),
            Ok(_)
        ));
        assert!(matches!(
            memory.fetch(DATA_LINEAR + 0x1000, context()),
            Err(MemoryError::Backend(_))
        ));
        assert!(matches!(
            memory.memory_error,
            Some(TrackedAccessError::CrossesPage { .. })
        ));
        assert_eq!(memory.page_fault, None);
        // The set's adapter walks the next page on its own: it is unmapped here, so the byte
        // is an instruction-fetch `#PF` at exactly that address, not a backend error.
        let mut machine = bus(0x6007);
        let mut callback = |_| {};
        let mut memory = adapter(&mut machine, &mut callback);
        memory.large_ram_profile = true;
        memory.cpl0_set_cross_page_fetch = true;
        memory.instruction_rip = DATA_LINEAR + 0xffe;
        assert!(matches!(
            memory.fetch(DATA_LINEAR + 0x1000, context()),
            Err(MemoryError::Architectural(_))
        ));
        assert_eq!(memory.memory_error, None);
        let record = memory.page_fault.expect("fetch fault");
        assert_eq!(record.fault().linear_address, Some(DATA_LINEAR + 0x1000));
        assert_eq!(record.fault().error_code, Some(0x10));
        assert_eq!(record.saved_rip(), DATA_LINEAR + 0xffe);
    }

    #[test]
    fn the_ram_only_adapter_bus_refuses_the_whole_bus_with_a_backend_error() {
        let mut machine = bus(0x6007);
        let (ram, _port) = machine.split_for_pio_step();
        let mut adapter_bus = AdapterBus::Ram(ram);
        match adapter_bus.machine() {
            Err(MemoryError::Backend(message)) => assert_eq!(
                message,
                "the RAM-only adapter bus serves the large-RAM walk only"
            ),
            Err(other) => panic!("{other:?}"),
            Ok(_) => panic!("the RAM-only bus has no machine bus"),
        }
        // The whole-bus form hands the bus out.
        let mut adapter_bus = AdapterBus::from(&mut machine);
        assert!(adapter_bus.machine().is_ok());
    }

    /// RAM with the page tables of `bus()` whose data page maps the Local APIC window, and a
    /// board of `cpus` Local APICs behind it.
    fn lapic_bus(cpus: usize) -> (MachineBus, crate::LocalApicHandle) {
        let window = GuestPhysicalAddress::new(crate::LOCAL_APIC_MMIO_BASE);
        let regions = vec![
            MemoryRegionSpec::new(
                GuestPhysicalRange::new(GuestPhysicalAddress::new(0), RAM_LEN).unwrap(),
                MemoryRegionKind::Ram,
            ),
            MemoryRegionSpec::new(
                GuestPhysicalRange::new(window, crate::LOCAL_APIC_MMIO_SIZE).unwrap(),
                MemoryRegionKind::Mmio,
            ),
        ];
        let map = MachineMemoryMap::new(36, regions).unwrap();
        let device = crate::LocalApic::with_cpus(cpus).unwrap();
        let handle = device.handle();
        let mut bus = MachineBus::new(
            map,
            RamLimits::new(RAM_LEN),
            vec![Box::new(device)],
            vec![crate::MmioBinding {
                region_start: window,
                device_index: 0,
            }],
            vec![],
        )
        .unwrap();
        put(&mut bus, 0x1000, 0x2007);
        put(&mut bus, 0x2000, 0x3007);
        put(&mut bus, 0x3010, 0x4007);
        put(&mut bus, 0x3020, 0x5007);
        put(&mut bus, 0x4000, 0x7007);
        put(&mut bus, 0x5000, crate::LOCAL_APIC_MMIO_BASE | 7);
        (bus, handle)
    }

    #[test]
    fn a_paged_mmio_access_reaches_the_lapic_of_the_vcpu_that_owns_the_adapter() {
        let (mut bus, apic) = lapic_bus(2);
        let mut callback = |_| {};
        // The APIC ID register reads the owner's index, whichever processor reads it.
        for vcpu in [1usize, 0] {
            let mut memory = adapter_on(AdapterBus::Machine(&mut bus, vcpu), &mut callback);
            memory.cpl0_set_dword_read = true;
            let mut id = [0xff; 4];
            memory.read(DATA_LINEAR + 0x20, context(), &mut id).unwrap();
            assert_eq!(u32::from_le_bytes(id), (vcpu as u32) << 24, "vcpu {vcpu}");
        }
        // A write lands in the owner's LAPIC and in no other.
        let mut memory = adapter_on(AdapterBus::Machine(&mut bus, 1), &mut callback);
        memory.dword_write_profile = true;
        memory
            .write(DATA_LINEAR + 0x80, context(), &0x30u32.to_le_bytes())
            .unwrap();
        let pair = apic.0.lock().unwrap();
        assert_eq!((pair.cpus[0].local.tpr, pair.cpus[1].local.tpr), (0, 0x30));
        drop(pair);
        drop(memory);
        // An adapter built the ordinary way belongs to the boot processor.
        let mut memory = adapter(&mut bus, &mut callback);
        memory.dword_write_profile = true;
        memory
            .write(DATA_LINEAR + 0x80, context(), &0x40u32.to_le_bytes())
            .unwrap();
        let pair = apic.0.lock().unwrap();
        assert_eq!(
            (pair.cpus[0].local.tpr, pair.cpus[1].local.tpr),
            (0x40, 0x30)
        );
    }

    #[test]
    fn mmu_gather_diagnostic_capture_reads_raw_fields_without_page_table_writes() {
        let mut bus = bus(0x9007);
        put(&mut bus, 0x1000, 0x2007);
        put(&mut bus, 0x2000, 0x3007);
        put(&mut bus, 0x3000, 0x4007);
        put(&mut bus, 0x4028, 0x9007);
        put(&mut bus, 0x4030, 0xa007);
        let mut tlb = [0u8; 0x24];
        tlb[0..8].copy_from_slice(&0x6000u64.to_le_bytes());
        tlb[0x10..0x18].copy_from_slice(&0x1111u64.to_le_bytes());
        tlb[0x18..0x20].copy_from_slice(&0x2222u64.to_le_bytes());
        tlb[0x20..0x24].copy_from_slice(&0x800u32.to_le_bytes());
        bus.patch_physical_ram(GuestPhysicalAddress::new(0x9000), &tlb)
            .unwrap();
        bus.patch_physical_ram(GuestPhysicalAddress::new(0xa61c), &(-7i32).to_le_bytes())
            .unwrap();
        let page_table_addresses = [0x1000, 0x2000, 0x3000, 0x4028, 0x4030];
        let before = page_table_addresses.map(|address| {
            bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                .unwrap()
        });
        let paging = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap();
        let (raw, error) = read_mmu_gather_raw(
            &mut bus,
            &paging,
            LINUX_RELOCATE_VMA_DOWN_TLB_FINISH_CALL_RIP,
            context(),
            0x5000,
        );
        assert_eq!(error, None);
        let raw = raw.unwrap();
        assert_eq!(
            (raw.mm, raw.start, raw.end, raw.flags),
            (0x6000, 0x1111, 0x2222, 0x800)
        );
        assert_eq!(raw.mm_tlb_flush_nested, -7);
        let after = page_table_addresses.map(|address| {
            bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                .unwrap()
        });
        assert_eq!(after, before);
    }

    #[test]
    fn mmu_gather_walks_two_fixed_candidate_pages_at_setup_initial_and_return_only() {
        use od_isa::Register::{R14, R15, Rbp, Rdi, Rsp};

        let (mut machine_bus, paging) = elf_copy_bus(true, true);
        let setup_rbp = ELF_COPY_R13 + 0x100;
        let relocate_rbp = ELF_COPY_R13 + 0x180;
        let actual_post_rbp = ELF_COPY_R13 + 0x240;
        let tlb = ELF_COPY_R13 + 0x200;
        let mm = ELF_COPY_R13 + 0x400;
        let mut setup_bytes = [0u8; 16];
        setup_bytes[..8].copy_from_slice(&(ELF_COPY_R13 + 0x300).to_le_bytes());
        setup_bytes[8..].copy_from_slice(&0xffff_ffff_818f_24e5u64.to_le_bytes());
        machine_bus
            .patch_physical_ram(GuestPhysicalAddress::new(0x12100), &setup_bytes)
            .unwrap();
        machine_bus
            .patch_physical_ram(GuestPhysicalAddress::new(0x12000), &[0u8; 0x1000])
            .unwrap();
        machine_bus
            .patch_physical_ram(
                GuestPhysicalAddress::new(0x12000),
                &(ELF_COPY_R13 + 0x1000).to_le_bytes(),
            )
            .unwrap();
        machine_bus
            .patch_physical_ram(
                GuestPhysicalAddress::new(0x12008),
                &(ELF_COPY_R13 + 0x2000).to_le_bytes(),
            )
            .unwrap();
        let mut relocate_frame = [0u8; 16];
        relocate_frame[..8].copy_from_slice(&setup_rbp.to_le_bytes());
        relocate_frame[8..].copy_from_slice(&0xffff_ffff_818f_24e5u64.to_le_bytes());
        machine_bus
            .patch_physical_ram(GuestPhysicalAddress::new(0x12180), &relocate_frame)
            .unwrap();
        let mut actual_post_frame = [0u8; 16];
        actual_post_frame[..8].copy_from_slice(&relocate_rbp.to_le_bytes());
        actual_post_frame[8..].copy_from_slice(&0xffff_ffff_8182_bfe0u64.to_le_bytes());
        machine_bus
            .patch_physical_ram(GuestPhysicalAddress::new(0x12240), &actual_post_frame)
            .unwrap();
        let mut tlb_bytes = [0u8; 0x24];
        tlb_bytes[..8].copy_from_slice(&mm.to_le_bytes());
        tlb_bytes[0x10..0x18].copy_from_slice(&0xffff_ffff_83fe_f000u64.to_le_bytes());
        tlb_bytes[0x18..0x20].copy_from_slice(&0xffff_ffff_83ff_1000u64.to_le_bytes());
        tlb_bytes[0x20..0x24].copy_from_slice(&0x800u32.to_le_bytes());
        machine_bus
            .patch_physical_ram(GuestPhysicalAddress::new(0x12200), &tlb_bytes)
            .unwrap();
        machine_bus
            .patch_physical_ram(GuestPhysicalAddress::new(0x12a1c), &(-1i32).to_le_bytes())
            .unwrap();

        let ram_before = (0..RAM_LEN)
            .step_by(8)
            .map(|address| {
                machine_bus
                    .read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let mut observations =
            LinuxExecutionObservationCollector::default().with_elf_stack_copy(true);
        let mut state = LongModeState::default();
        state.gpr[R14 as usize] = 0x1000;
        state.gpr[R15 as usize] = ELF_COPY_R13;
        state.gpr[Rbp as usize] = setup_rbp;
        state.gpr[Rsp as usize] = ELF_COPY_R13 + 0x800;
        state.rip = LINUX_SETUP_ARG_PAGES_STACK_SHIFT_RIP;
        let before_setup = state.clone();
        capture_elf_mmu_gather_boundary(
            state.rip,
            &state,
            &mut machine_bus,
            &paging,
            context(),
            ROOT,
            &mut observations,
        );
        assert_eq!(state, before_setup);
        let setup = &observations.mmu_gather_capture_for_test().setups[0];
        assert!(matches!(
            &setup.setup,
            crate::ElfMmuGatherPhase::Captured { value, .. }
                if value.relocated_start == Some(ELF_COPY_R13)
                    && value.aligned_start == Some(ELF_COPY_R13)
                    && value.candidate_pages == [Some(ELF_COPY_R13 - 0x1000), Some(ELF_COPY_R13)]
        ));
        assert_eq!(
            setup
                .setup_page_walks
                .iter()
                .map(|walk| walk.status)
                .collect::<Vec<_>>(),
            vec!["translated", "translated"]
        );

        state.gpr[Rbp as usize] = relocate_rbp;
        state.gpr[Rdi as usize] = tlb;
        state.gpr[Rsp as usize] = ELF_COPY_R13 + 0x7f0;
        state.rip = LINUX_RELOCATE_VMA_DOWN_TLB_FINISH_CALL_RIP;
        let before_initial = state.clone();
        capture_elf_mmu_gather_boundary(
            state.rip,
            &state,
            &mut machine_bus,
            &paging,
            context(),
            ROOT,
            &mut observations,
        );
        assert_eq!(state, before_initial);
        let setup = &observations.mmu_gather_capture_for_test().setups[0];
        assert_eq!(
            setup
                .initial_page_walks
                .as_ref()
                .unwrap()
                .iter()
                .map(|walk| walk.address)
                .collect::<Vec<_>>(),
            vec![Some(ELF_COPY_R13 - 0x1000), Some(ELF_COPY_R13)]
        );

        let page_walks_before_actual = MMU_GATHER_PAGE_WALKS.with(std::cell::Cell::get);
        state.gpr[Rbp as usize] = actual_post_rbp;
        state.gpr[Rdi as usize] = tlb;
        state.rip = LINUX_TLB_FINISH_MMU_ACTUAL_POST_RIP;
        let before_actual_post = state.clone();
        capture_elf_mmu_gather_boundary(
            state.rip,
            &state,
            &mut machine_bus,
            &paging,
            context(),
            ROOT,
            &mut observations,
        );
        assert_eq!(state, before_actual_post);
        assert_eq!(
            MMU_GATHER_PAGE_WALKS.with(std::cell::Cell::get),
            page_walks_before_actual,
            "actual_post captures gather fields without candidate page walks"
        );
        let setup = &observations.mmu_gather_capture_for_test().setups[0];
        assert!(matches!(
            setup.actual_post,
            crate::ElfMmuGatherPhase::Captured { .. }
        ));
        assert!(setup.initial_page_walks.is_some());
        assert!(setup.return_page_walks.is_none());
        state.rip = LINUX_RELOCATE_VMA_DOWN_TLB_FINISH_RETURN_RIP;
        state.gpr[Rbp as usize] = relocate_rbp;
        let before_return = state.clone();
        capture_elf_mmu_gather_boundary(
            state.rip,
            &state,
            &mut machine_bus,
            &paging,
            context(),
            ROOT,
            &mut observations,
        );
        assert_eq!(state, before_return);
        let setup = &observations.mmu_gather_capture_for_test().setups[0];
        assert_eq!(
            setup
                .return_page_walks
                .as_ref()
                .unwrap()
                .iter()
                .map(|walk| walk.status)
                .collect::<Vec<_>>(),
            vec!["translated", "translated"]
        );
        assert!(matches!(
            setup.actual_post,
            crate::ElfMmuGatherPhase::Captured { .. }
        ));

        let mut copy_snapshot = ElfStackCopySnapshot::empty(ElfStackCopyStatus::Captured);
        copy_snapshot.copy_destination = Some(ELF_COPY_R13 - 7);
        observations.record_elf_stack_copy(copy_snapshot);
        let capture = observations
            .finish()
            .elf_stack_copy
            .unwrap()
            .pre_copy_cr3_ledger
            .unwrap()
            .mmu_gather;
        assert_eq!(capture.copy_destination, Some(ELF_COPY_R13 - 7));
        assert_eq!(capture.destination_in_candidate_pages, Some(true));
        let ram_after = (0..RAM_LEN)
            .step_by(8)
            .map(|address| {
                machine_bus
                    .read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            ram_after, ram_before,
            "diagnostic walk changed guest RAM/A-D bits"
        );
    }

    #[test]
    fn mmu_gather_candidate_walks_distinguish_nonpresent_protection_and_read_errors() {
        let candidate = ELF_COPY_R13 - 0x1000;
        let candidates = [Some(candidate), Some(ELF_COPY_R13)];
        let mut absent_bus = elf_copy_bus(false, true).0;
        let absent = capture_mmu_gather_candidate_page_walks(
            candidates,
            &mut absent_bus,
            &Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap(),
            LINUX_SETUP_ARG_PAGES_STACK_SHIFT_RIP,
            context(),
        );
        assert_eq!(absent[0].status, "non_present");
        assert_eq!(absent[0].translated_physical, None);

        let mut protected_bus = elf_copy_bus(true, true).0;
        let indices = [
            ((candidate >> 39) & 0x1ff) as u64,
            ((candidate >> 30) & 0x1ff) as u64,
            ((candidate >> 21) & 0x1ff) as u64,
            ((candidate >> 12) & 0x1ff) as u64,
        ];
        let pml4e = protected_bus
            .read_physical(
                GuestPhysicalAddress::new(ROOT + indices[0] * 8),
                AccessWidth::Qword,
            )
            .unwrap();
        let pdpte = protected_bus
            .read_physical(
                GuestPhysicalAddress::new((pml4e & !0xfff) + indices[1] * 8),
                AccessWidth::Qword,
            )
            .unwrap();
        let pde = protected_bus
            .read_physical(
                GuestPhysicalAddress::new((pdpte & !0xfff) + indices[2] * 8),
                AccessWidth::Qword,
            )
            .unwrap();
        let pte_address = (pde & !0xfff) + indices[3] * 8;
        let pte = protected_bus
            .read_physical(GuestPhysicalAddress::new(pte_address), AccessWidth::Qword)
            .unwrap();
        put(&mut protected_bus, pte_address, pte & !2);
        let protected = capture_mmu_gather_candidate_page_walks(
            candidates,
            &mut protected_bus,
            &Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap(),
            LINUX_SETUP_ARG_PAGES_STACK_SHIFT_RIP,
            context(),
        );
        assert_eq!(protected[0].status, "protection");
        assert_eq!(protected[0].translated_physical, None);

        let mut error_bus = bus(0);
        let root_index = ((candidate >> 39) & 0x1ff) as u64;
        put(&mut error_bus, ROOT + root_index * 8, 0x1_0000_0007);
        let read_error = capture_mmu_gather_candidate_page_walks(
            [Some(candidate), None],
            &mut error_bus,
            &Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap(),
            LINUX_SETUP_ARG_PAGES_STACK_SHIFT_RIP,
            context(),
        );
        assert_eq!(read_error[0].status, "read_error");
        assert!(read_error[0].error.is_some());
        assert_eq!(read_error[1].status, "invalid_address");
    }

    #[test]
    fn non_anchor_and_frozen_mmu_gather_capture_do_zero_guest_reads() {
        let mut machine_bus = bus(0x7007);
        let paging = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap();
        let mut state = LongModeState::default();
        let mut observations =
            LinuxExecutionObservationCollector::default().with_elf_stack_copy(true);

        MMU_GATHER_DIAGNOSTIC_READS.with(|count| count.set(0));
        state.rip = LINUX_SETUP_ARG_PAGES_STACK_SHIFT_RIP + 1;
        capture_elf_mmu_gather_boundary(
            state.rip,
            &state,
            &mut machine_bus,
            &paging,
            context(),
            ROOT,
            &mut observations,
        );
        assert_eq!(MMU_GATHER_DIAGNOSTIC_READS.with(std::cell::Cell::get), 0);

        observations.record_elf_stack_copy(ElfStackCopySnapshot::empty(
            ElfStackCopyStatus::GateMismatch,
        ));
        state.rip = LINUX_SETUP_ARG_PAGES_STACK_SHIFT_RIP;
        capture_elf_mmu_gather_boundary(
            state.rip,
            &state,
            &mut machine_bus,
            &paging,
            context(),
            ROOT,
            &mut observations,
        );
        assert_eq!(MMU_GATHER_DIAGNOSTIC_READS.with(std::cell::Cell::get), 0);
    }

    // ---- S6-31: locked operations on the host atomics of the shared RAM ----

    const LOCK_ROM: u64 = 0x2_0000;
    const LOCK_MMIO: u64 = 0x2_1000;
    /// Accessed and dirty bits of a table entry.
    const AD: u64 = 0x60;

    /// RAM 0..0x20000 with ROM and MMIO frames behind it, the tables of `bus()` (code at linear
    /// 0x400000 and data at `DATA_LINEAR` both reach frame 0x7000 unless `data_pte` says otherwise)
    /// with `ad` in every table entry, and `data_pte` as the leaf of `DATA_LINEAR`.
    fn lock_memory(ad: u64, data_pte: u64) -> crate::PhysicalMemory {
        let map = MachineMemoryMap::new(
            36,
            vec![
                MemoryRegionSpec::new(
                    GuestPhysicalRange::new(GuestPhysicalAddress::new(0), RAM_LEN).unwrap(),
                    MemoryRegionKind::Ram,
                ),
                MemoryRegionSpec::new(
                    GuestPhysicalRange::new(GuestPhysicalAddress::new(LOCK_ROM), 0x1000).unwrap(),
                    MemoryRegionKind::Rom,
                ),
                MemoryRegionSpec::new(
                    GuestPhysicalRange::new(GuestPhysicalAddress::new(LOCK_MMIO), 0x1000).unwrap(),
                    MemoryRegionKind::Mmio,
                ),
            ],
        )
        .unwrap();
        let memory = crate::PhysicalMemory::new(&map, RamLimits::new(RAM_LEN + 0x1000)).unwrap();
        for (address, value) in [
            (0x1000, 0x2007 | ad),
            (0x2000, 0x3007 | ad),
            (0x3010, 0x4007 | ad),
            (0x3020, 0x5007 | ad),
            (0x4000, 0x7007 | ad),
            (0x5000, data_pte),
        ] {
            memory
                .store_u64(GuestPhysicalAddress::new(address), value, |_| {})
                .unwrap();
        }
        memory
    }

    fn table_entries(memory: &crate::PhysicalMemory) -> Vec<u64> {
        [0x1000u64, 0x2000, 0x3010, 0x3020, 0x4000, 0x5000]
            .map(|address| memory.load_u64(GuestPhysicalAddress::new(address)).unwrap())
            .to_vec()
    }

    fn lock_memory_2m(data_pde: u64) -> crate::PhysicalMemory {
        const LARGE_RAM: u64 = 0x40_0000;
        let map = MachineMemoryMap::new(
            36,
            vec![MemoryRegionSpec::new(
                GuestPhysicalRange::new(GuestPhysicalAddress::new(0), LARGE_RAM).unwrap(),
                MemoryRegionKind::Ram,
            )],
        )
        .unwrap();
        let memory = crate::PhysicalMemory::new(&map, RamLimits::new(LARGE_RAM)).unwrap();
        for (address, value) in [
            (0x1000, 0x2007),
            (0x2000, 0x3007),
            (0x3010, 0x87),     // code VA 0x400000 -> physical 0, 2 MiB leaf
            (0x3020, data_pde), // data VA 0x800000 -> physical 0x200000, 2 MiB leaf
        ] {
            memory
                .store_u64(GuestPhysicalAddress::new(address), value, |_| {})
                .unwrap();
        }
        memory
    }

    fn two_mib_entries(memory: &crate::PhysicalMemory) -> [u64; 4] {
        [0x1000u64, 0x2000, 0x3010, 0x3020]
            .map(|address| memory.load_u64(GuestPhysicalAddress::new(address)).unwrap())
    }

    fn data_frame(memory: &crate::PhysicalMemory) -> Vec<u8> {
        let mut bytes = vec![0u8; 0x1000];
        memory
            .load_bytes(GuestPhysicalAddress::new(0x7000), &mut bytes)
            .unwrap();
        bytes
    }

    fn lock_adapter_with_cr0<'a>(
        memory: &'a crate::PhysicalMemory,
        committed: &'a mut dyn FnMut(GuestPhysicalRange),
        cr0: u64,
    ) -> PagedMovMemoryAdapter<'a> {
        set_loop_adapter(
            memory,
            None,
            Long64ProbeConfig::new(cr0, ROOT, CR4, EFER).unwrap(),
            0x0040_0000,
            committed,
            None,
            None,
        )
    }

    fn lock_adapter<'a>(
        memory: &'a crate::PhysicalMemory,
        committed: &'a mut dyn FnMut(GuestPhysicalRange),
    ) -> PagedMovMemoryAdapter<'a> {
        lock_adapter_with_cr0(memory, committed, CR0)
    }

    /// S6-41c: the translation half of an inline atomic operation does to the page tables, the
    /// notifications and the fault exactly what the interpreter's locked operation did, also for
    /// the unequal branch of `CMPXCHG` (the interpreter's compare that does not match still takes
    /// the write walk: `#PF` on a read-only page, dirty bit set on a writable one); the window it
    /// returns is the cell the interpreter's operation would have used.
    #[test]
    fn locating_an_atomic_operand_leaves_what_the_interpreters_unequal_cmpxchg_left() {
        use od_isa::OperandSize::{Byte, Dword, Qword, Word};
        let mut located = 0;
        let mut faulted = 0;
        for ad in [0, AD] {
            for data_pte in [0x7007u64, 0x7027, 0x7067, 0x7005, 0x7006, 0x7025, 0x0] {
                for size in [Byte, Word, Dword, Qword] {
                    let address = DATA_LINEAR + 0x40 + size.bytes() as u64 * 2;
                    let (memory_a, memory_b) =
                        (lock_memory(ad, data_pte), lock_memory(ad, data_pte));
                    let (mut log_a, mut log_b) = (Vec::new(), Vec::new());
                    let mut note_a = |range| log_a.push(range);
                    let mut note_b = |range| log_b.push(range);
                    let config = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap();
                    let (operand, fault_a) = locate_paged_atomic_operand(
                        &memory_a,
                        config,
                        0x0040_0000,
                        context(),
                        address,
                        size,
                        &mut note_a,
                    );
                    let (result_b, fault_b) = {
                        let mut b = lock_adapter(&memory_b, &mut note_b);
                        // A compare that cannot match: x86 still writes the destination back.
                        let result = b.atomic_compare_exchange_integer(
                            address,
                            context(),
                            size,
                            !0x0101_0101_0101_0101u64,
                            0x55,
                        );
                        (result, b.page_fault.take())
                    };
                    let label = format!("ad {ad:#x} pte {data_pte:#x} size {}", size.bytes());
                    assert_eq!(
                        table_entries(&memory_a),
                        table_entries(&memory_b),
                        "{label}"
                    );
                    assert_eq!(format!("{fault_a:?}"), format!("{fault_b:?}"), "{label}");
                    // The interpreter's compare writes the unchanged value back and notifies it;
                    // the walk part of the log is the common prefix.
                    assert!(
                        log_b.starts_with(&log_a) && log_b.len() <= log_a.len() + 1,
                        "{label}: walk notifications {log_a:?} vs {log_b:?}"
                    );
                    match (&operand, &result_b) {
                        (PagedAtomicOperand::Located(window), Ok(_)) => {
                            located += 1;
                            // The window is the cell the interpreter used: bytes of frame 0x7000.
                            let physical = 0x7000 + (address & 0xfff);
                            assert_eq!(window.range().start().get(), physical, "{label}");
                            assert_eq!(window.range().len(), size.bytes() as u64, "{label}");
                            // Locating raises no counter. The interpreter's unequal compare
                            // does: `PhysicalMemory::atomic_compare_exchange` reports a write
                            // always (x86 writes the destination back), so the page counter
                            // grows by one and one range is notified. The inline sequence does
                            // the same after its atomic operation, unconditionally.
                            let counter = |memory: &crate::PhysicalMemory| {
                                memory
                                    .code_page_generation(GuestPhysicalAddress::new(physical))
                                    .unwrap()
                            };
                            assert_eq!(counter(&memory_b), counter(&memory_a) + 1, "{label}");
                            assert_eq!(data_frame(&memory_a), data_frame(&memory_b), "{label}");
                        }
                        (
                            PagedAtomicOperand::Fault(fault),
                            Err(MemoryError::Architectural(other)),
                        ) => {
                            faulted += 1;
                            assert_eq!(fault, other, "{label}");
                            assert_eq!(data_frame(&memory_a), data_frame(&memory_b), "{label}");
                        }
                        other => panic!("{label}: located vs interpreter disagree: {other:?}"),
                    }
                }
            }
        }
        assert!(
            located > 0 && faulted > 0,
            "{located} located, {faulted} faulted"
        );
    }

    /// S6-41c: a misaligned operand is declined with no walk at all: no accessed or dirty bit, no
    /// notification, no split-lock refusal counted; a located operand sets the dirty bit a write
    /// sets and leaves the data alone.
    #[test]
    fn a_misaligned_atomic_operand_is_declined_before_any_walk() {
        use od_isa::OperandSize::{Dword, Qword, Word};
        let memory = lock_memory(0, 0x7027);
        let before = table_entries(&memory);
        let config = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap();
        for (offset, size) in [
            (1u64, Word),
            (2, Dword),
            (4, Qword),
            (0x3d, Qword),
            (0xfff, Word),
        ] {
            let mut log = Vec::new();
            let mut note = |range| log.push(range);
            let (operand, fault) = locate_paged_atomic_operand(
                &memory,
                config,
                0x0040_0000,
                context(),
                DATA_LINEAR + offset,
                size,
                &mut note,
            );
            assert!(
                matches!(operand, PagedAtomicOperand::Misaligned),
                "{offset} {size:?}"
            );
            assert!(fault.is_none() && log.is_empty());
            assert_eq!(table_entries(&memory), before, "no accessed or dirty bit");
            assert_eq!(memory.split_lock_refusals(), 0, "not counted");
        }
        let mut log = Vec::new();
        let mut note = |range| log.push(range);
        let (operand, _) = locate_paged_atomic_operand(
            &memory,
            config,
            0x0040_0000,
            context(),
            DATA_LINEAR + 0x40,
            Dword,
            &mut note,
        );
        assert!(matches!(operand, PagedAtomicOperand::Located(_)));
        assert_ne!(
            table_entries(&memory),
            before,
            "the write walk set the dirty bit"
        );
        assert_eq!(table_entries(&memory)[5] & 0x60, 0x60);
    }

    /// The serial S6-20 sequence a locked operation replaced: an ordinary read of the operand,
    /// the update, an ordinary write of the result.
    fn serial_rmw(
        adapter: &mut PagedMovMemoryAdapter<'_>,
        address: u64,
        size: od_isa::OperandSize,
        update: &mut dyn FnMut(u64) -> u64,
    ) -> Result<u64, MemoryError> {
        let old = adapter.read_integer(address, context(), size)?;
        let new = update(old);
        adapter.write_integer(address, context(), size, new)?;
        Ok(old)
    }

    /// S6-31: the translation half of a locked operation does to the page tables, the callbacks
    /// and the error state exactly what the serial read and write did: the same accessed and dirty
    /// bits (also on a fault, where the accessed bits of the read walk stay), the same fault, the
    /// same committed-write sequence, the same data; only the data access is one host operation.
    /// Cases: tables with and without accessed/dirty bits, a leaf without dirty, a read-only page
    /// (the fault of the write), a not-present page (the fault of the read).
    #[test]
    fn a_locked_operation_leaves_what_the_serial_read_and_write_left() {
        use od_isa::OperandSize::{Byte, Dword, Qword, Word};
        let sizes = [Byte, Word, Dword, Qword];
        for ad in [0, AD] {
            for data_pte in [0x7007u64, 0x7027, 0x7067, 0x7005, 0x7006, 0x0] {
                for size in sizes {
                    for kind in 0..5u64 {
                        let operand = 0x1122_3344_5566_7788u64 ^ (kind << 8);
                        let address = DATA_LINEAR + 0x40 + size.bytes() as u64 * 2;
                        let fill = |memory: &crate::PhysicalMemory| {
                            memory
                                .store_u64(
                                    GuestPhysicalAddress::new(0x7000 + (address & 0xfff & !7)),
                                    0x0807_0605_0403_0201,
                                    |_| {},
                                )
                                .unwrap();
                        };
                        let (memory_a, memory_b) =
                            (lock_memory(ad, data_pte), lock_memory(ad, data_pte));
                        fill(&memory_a);
                        fill(&memory_b);
                        let (mut log_a, mut log_b) = (Vec::new(), Vec::new());
                        let (result_a, result_b, fault_a, fault_b, errors_a, errors_b);
                        {
                            let mut note_a = |r| log_a.push(r);
                            let mut note_b = |r| log_b.push(r);
                            let mut a = lock_adapter(&memory_a, &mut note_a);
                            let mut b = lock_adapter(&memory_b, &mut note_b);
                            let mask = size.mask();
                            (result_a, result_b) = match kind {
                                0 => (
                                    a.atomic_exchange_integer(address, context(), size, operand),
                                    serial_rmw(&mut b, address, size, &mut |_| operand & mask),
                                ),
                                1 => (
                                    a.atomic_fetch_add_integer(address, context(), size, operand),
                                    serial_rmw(&mut b, address, size, &mut |old| {
                                        old.wrapping_add(operand) & mask
                                    }),
                                ),
                                2 => (
                                    // matches: the bytes are the fill
                                    a.atomic_compare_exchange_integer(
                                        address,
                                        context(),
                                        size,
                                        0x0807_0605_0403_0201 >> (8 * (address & 7)),
                                        operand,
                                    ),
                                    serial_rmw(&mut b, address, size, &mut |old| {
                                        if old
                                            == (0x0807_0605_0403_0201u64 >> (8 * (address & 7)))
                                                & mask
                                        {
                                            operand & mask
                                        } else {
                                            old
                                        }
                                    }),
                                ),
                                3 => (
                                    a.atomic_compare_exchange_integer(
                                        address,
                                        context(),
                                        size,
                                        !0x0807_0605_0403_0201u64,
                                        operand,
                                    ),
                                    serial_rmw(&mut b, address, size, &mut |old| old),
                                ),
                                _ => (
                                    a.atomic_rmw_integer(address, context(), size, &mut |old| {
                                        (old ^ operand) & mask
                                    }),
                                    serial_rmw(&mut b, address, size, &mut |old| {
                                        (old ^ operand) & mask
                                    }),
                                ),
                            };
                            fault_a = format!("{:?}", a.page_fault);
                            fault_b = format!("{:?}", b.page_fault);
                            errors_a = format!("{:?} {:?}", a.access_error, a.memory_error);
                            errors_b = format!("{:?} {:?}", b.access_error, b.memory_error);
                        }
                        let label = format!(
                            "ad {ad:#x} pte {data_pte:#x} size {} kind {kind}",
                            size.bytes()
                        );
                        assert_eq!(
                            format!("{result_a:?}"),
                            format!("{result_b:?}"),
                            "result, {label}"
                        );
                        assert_eq!(fault_a, fault_b, "page fault record, {label}");
                        assert_eq!(errors_a, errors_b, "error state, {label}");
                        assert_eq!(
                            table_entries(&memory_a),
                            table_entries(&memory_b),
                            "tables, {label}"
                        );
                        assert_eq!(
                            data_frame(&memory_a),
                            data_frame(&memory_b),
                            "data, {label}"
                        );
                        assert_eq!(log_a, log_b, "committed writes, {label}");
                    }
                }
            }
        }
    }

    /// S6-31: what a locked operation refuses it refuses before it sets any accessed or dirty bit
    /// (a misaligned or page-crossing operand in the PARALLEL mode: counted, never run as two
    /// accesses; S6-32: in the interleaving mode it is the serial pair, see
    /// `a_misaligned_locked_operation_in_the_interleaving_mode_is_the_serial_pair`) or before the
    /// dirty bit and the device (MMIO), and ROM is the error of a write; nothing is written and
    /// nothing is notified except the accessed bits the read half already set for ROM and MMIO.
    #[test]
    fn a_locked_operation_refuses_a_split_or_a_device_operand_and_counts_the_split() {
        use od_isa::OperandSize::{Dword, Qword, Word};
        let mut memory = lock_memory(0, 0x7007);
        memory.set_execution_mode(crate::ExecutionMode::Parallel);
        let before = (table_entries(&memory), data_frame(&memory));
        let mut notes = Vec::new();
        let mut refused = 0;
        {
            let mut note = |r| notes.push(r);
            let mut adapter = lock_adapter(&memory, &mut note);
            for (size, offsets) in [
                (Word, vec![1u64, 0x3, 0x7, 0xfff]),
                (Dword, vec![1, 2, 3, 0xffd, 0xffe, 0xfff]),
                (Qword, vec![1, 4, 7, 0xff9, 0xffc, 0xfff]),
            ] {
                for offset in offsets {
                    let address = DATA_LINEAR + offset;
                    for call in 0..4 {
                        let result = match call {
                            0 => adapter.atomic_exchange_integer(address, context(), size, 1),
                            1 => adapter.atomic_fetch_add_integer(address, context(), size, 1),
                            2 => adapter.atomic_compare_exchange_integer(
                                address,
                                context(),
                                size,
                                0,
                                1,
                            ),
                            _ => adapter.atomic_rmw_integer(address, context(), size, &mut |v| v),
                        };
                        let Err(MemoryError::Backend(message)) = result else {
                            panic!("{size:?} at +{offset:#x} call {call}: {result:?}");
                        };
                        assert!(message.contains("not naturally aligned"), "{message}");
                        refused += 1;
                    }
                }
            }
        }
        assert_eq!(memory.split_lock_refusals(), refused);
        assert!(notes.is_empty(), "no notification: {notes:?}");
        assert_eq!(
            (table_entries(&memory), data_frame(&memory)),
            before,
            "no accessed bit, no dirty bit, no data"
        );

        // MMIO and ROM operands: refused; the dirty bit is not set and no byte moves.
        for (frame, rom) in [(LOCK_MMIO, false), (LOCK_ROM, true)] {
            let memory = lock_memory(0, frame | 7);
            let mut note = |_| {};
            let mut adapter = lock_adapter(&memory, &mut note);
            let result = adapter.atomic_fetch_add_integer(DATA_LINEAR + 8, context(), Dword, 1);
            assert!(
                matches!(result, Err(MemoryError::Backend(_))),
                "frame {frame:#x}: {result:?}"
            );
            drop(adapter);
            let leaf = table_entries(&memory)[5];
            assert_eq!(
                leaf & 0x40,
                0,
                "no dirty bit for a refused operand (rom {rom})"
            );
            assert_eq!(memory.split_lock_refusals(), 0, "not a split lock");
        }
    }

    /// The byte at linear data offset `offset` (0..0x2000): page 0 is frame 0x7000, page 1 is
    /// frame 0x8000 (set by `split_memory`).
    fn data_byte(memory: &crate::PhysicalMemory, offset: u64) -> u8 {
        memory
            .load_u8(GuestPhysicalAddress::new(
                0x7000 + (offset >> 12) * 0x1000 + (offset & 0xfff),
            ))
            .unwrap()
    }

    /// `lock_memory` with a second data page (frame 0x8000, leaf `pte1`) behind `DATA_LINEAR`,
    /// both data frames filled with a byte pattern, in the given mode.
    fn split_memory(
        ad: u64,
        pte0: u64,
        pte1: u64,
        mode: crate::ExecutionMode,
    ) -> crate::PhysicalMemory {
        let mut memory = lock_memory(ad, pte0);
        memory.set_execution_mode(mode);
        memory
            .store_u64(GuestPhysicalAddress::new(0x5008), pte1, |_| {})
            .unwrap();
        for offset in 0..0x2000u64 {
            memory
                .store_u8(
                    GuestPhysicalAddress::new(0x7000 + (offset >> 12) * 0x1000 + (offset & 0xfff)),
                    (offset * 7 + 3) as u8,
                    |_| {},
                )
                .unwrap();
        }
        memory
    }

    fn split_state(memory: &crate::PhysicalMemory) -> (Vec<u64>, u64, Vec<u8>) {
        (
            table_entries(memory),
            memory.load_u64(GuestPhysicalAddress::new(0x5008)).unwrap(),
            (0..0x2000)
                .map(|offset| data_byte(memory, offset))
                .collect(),
        )
    }

    /// S6-32 (the return condition of S6-31): in the interleaving mode a misaligned locked operand
    /// (inside a cache line, across a line, across a page) runs as the serial S6-20 pair, no
    /// counter. Compared with the pair itself (result, fault, error state, tables of both pages,
    /// both data frames, committed-write log: equal), and with the table of the aligned
    /// equivalent (the old value is the little-endian bytes at the address, the new value is what
    /// the operation defines), also on a read-only page (the fault of the write, no change) and a
    /// not-present one.
    #[test]
    fn a_misaligned_locked_operation_in_the_interleaving_mode_is_the_serial_pair() {
        use od_isa::OperandSize::{Dword, Qword, Word};
        let operand = 0x1122_3344_5566_7788u64;
        let apply = |kind: u64, old: u64, expected_old: u64, size: od_isa::OperandSize| {
            let mask = size.mask();
            match kind {
                0 => operand & mask,
                1 => old.wrapping_add(operand) & mask,
                2 | 3 => {
                    let expected = if kind == 2 {
                        expected_old
                    } else {
                        !expected_old
                    } & mask;
                    if old == expected { operand & mask } else { old }
                }
                _ => (old ^ operand) & mask,
            }
        };
        let mut executed = 0;
        for ad in [0, AD] {
            for (pte0, pte1) in [
                (0x7007u64, 0x8007u64),
                (0x7067, 0x8067),
                (0x7005, 0x8007),
                (0x7007, 0x0),
            ] {
                for (size, offsets) in [
                    (Word, vec![1u64, 0x3f, 0xfff]),
                    (Dword, vec![1, 0x3e, 0xffd, 0xfff]),
                    (Qword, vec![3, 0x3d, 0xff9, 0xffc]),
                ] {
                    for offset in offsets {
                        for kind in 0..5u64 {
                            let address = DATA_LINEAR + offset;
                            let mode = crate::ExecutionMode::Interleaved;
                            let memory_a = split_memory(ad, pte0, pte1, mode);
                            let memory_b = split_memory(ad, pte0, pte1, mode);
                            let initial = {
                                let mut bytes = [0u8; 8];
                                for (index, byte) in bytes[..size.bytes()].iter_mut().enumerate() {
                                    *byte = data_byte(&memory_a, offset + index as u64);
                                }
                                u64::from_le_bytes(bytes)
                            };
                            let (mut log_a, mut log_b) = (Vec::new(), Vec::new());
                            let (result_a, result_b, fault_a, fault_b, errors_a, errors_b);
                            {
                                let mut note_a = |r| log_a.push(r);
                                let mut note_b = |r| log_b.push(r);
                                let mut a = lock_adapter(&memory_a, &mut note_a);
                                let mut b = lock_adapter(&memory_b, &mut note_b);
                                result_a = match kind {
                                    0 => {
                                        a.atomic_exchange_integer(address, context(), size, operand)
                                    }
                                    1 => a.atomic_fetch_add_integer(
                                        address,
                                        context(),
                                        size,
                                        operand,
                                    ),
                                    2 => a.atomic_compare_exchange_integer(
                                        address,
                                        context(),
                                        size,
                                        initial,
                                        operand,
                                    ),
                                    3 => a.atomic_compare_exchange_integer(
                                        address,
                                        context(),
                                        size,
                                        !initial,
                                        operand,
                                    ),
                                    _ => {
                                        a.atomic_rmw_integer(address, context(), size, &mut |old| {
                                            (old ^ operand) & size.mask()
                                        })
                                    }
                                };
                                result_b = serial_rmw(&mut b, address, size, &mut |old| {
                                    apply(kind, old, initial, size)
                                });
                                fault_a = format!("{:?}", a.page_fault);
                                fault_b = format!("{:?}", b.page_fault);
                                errors_a = format!("{:?} {:?}", a.access_error, a.memory_error);
                                errors_b = format!("{:?} {:?}", b.access_error, b.memory_error);
                            }
                            let label = format!(
                                "ad {ad:#x} ptes {pte0:#x}/{pte1:#x} {} bytes at +{offset:#x} kind {kind}",
                                size.bytes()
                            );
                            assert_eq!(
                                format!("{result_a:?}"),
                                format!("{result_b:?}"),
                                "result, {label}"
                            );
                            assert_eq!(fault_a, fault_b, "page fault record, {label}");
                            assert_eq!(errors_a, errors_b, "error state, {label}");
                            assert_eq!(
                                split_state(&memory_a),
                                split_state(&memory_b),
                                "state, {label}"
                            );
                            assert_eq!(log_a, log_b, "committed writes, {label}");
                            assert_eq!(
                                memory_a.split_lock_refusals(),
                                0,
                                "no refusal in the interleaving mode, {label}"
                            );
                            if let Ok(old) = result_a {
                                // The table of the aligned equivalent: old value is the bytes at
                                // the address; the stored value is the operation's result.
                                assert_eq!(old, initial, "old value, {label}");
                                let new = apply(kind, initial, initial, size);
                                for index in 0..size.bytes() as u64 {
                                    assert_eq!(
                                        data_byte(&memory_a, offset + index),
                                        (new >> (8 * index)) as u8,
                                        "byte {index}, {label}"
                                    );
                                }
                                executed += 1;
                            } else {
                                // A refusal by fault: no byte moved.
                                assert_eq!(
                                    split_state(&memory_a).2,
                                    split_state(&split_memory(ad, pte0, pte1, mode)).2,
                                    "data unchanged on error, {label}"
                                );
                            }
                        }
                    }
                }
            }
        }
        assert!(executed > 100, "the successful cases ran: {executed}");
    }

    /// S6-32: a failed `CMPXCHG` of the serial split path still writes the destination (the S6-20
    /// contract: the instruction form has a read-modify-write destination, so a read-only page
    /// faults, and the dirty bit is set): the same value is written back, the committed-write
    /// log has one entry, the data is unchanged; on a read-only page a mismatch faults and
    /// writes nothing.
    #[test]
    fn a_failed_split_compare_exchange_writes_the_destination_like_s6_20() {
        use od_isa::OperandSize::Dword;
        for (offset, name) in [
            (0x3eu64, "inside the line pair"),
            (0xffe, "across the page"),
        ] {
            let address = DATA_LINEAR + offset;
            let memory = split_memory(0, 0x7027, 0x8027, crate::ExecutionMode::Interleaved);
            let before = split_state(&memory);
            let mut log = Vec::new();
            {
                let mut note = |r| log.push(r);
                let mut adapter = lock_adapter(&memory, &mut note);
                let old = adapter
                    .atomic_compare_exchange_integer(address, context(), Dword, 0xdead_beef, 0x55)
                    .unwrap();
                assert_ne!(old, 0xdead_beef, "a mismatch");
            }
            let after = split_state(&memory);
            assert_eq!(after.2, before.2, "{name}: the data is unchanged");
            assert_eq!(
                after.0[5] & 0x40,
                0x40,
                "{name}: dirty bit of the first page"
            );
            if offset == 0xffe {
                assert_eq!(after.1 & 0x40, 0x40, "{name}: dirty bit of the second page");
            }
            assert!(
                !log.is_empty(),
                "{name}: the write was committed (same value)"
            );
            assert_eq!(memory.split_lock_refusals(), 0);
        }
        // Read-only page: the fault of the write, nothing stored, nothing notified.
        let memory = split_memory(0, 0x7005, 0x8005, crate::ExecutionMode::Interleaved);
        let before = split_state(&memory);
        let mut log = Vec::new();
        let result = {
            let mut note = |r| log.push(r);
            let mut adapter = lock_adapter(&memory, &mut note);
            adapter.atomic_compare_exchange_integer(
                DATA_LINEAR + 0x3e,
                context(),
                Dword,
                0xdead_beef,
                0x55,
            )
        };
        assert!(
            matches!(result, Err(MemoryError::Architectural(_))),
            "{result:?}"
        );
        assert_eq!(split_state(&memory).2, before.2);
        // The accessed-bit writes of the read walk may be notified; no data frame is.
        assert!(
            log.iter().all(|range| range.start().get() < 0x7000),
            "no notification of a data frame: {log:?}"
        );
    }

    /// S6-32: the same misaligned operand in the parallel mode is the refusal with the counter
    /// (until the stop-all of S6-34 step 8): nothing is written, no accessed or dirty bit, no
    /// notification, and the counter counts each refused call.
    #[test]
    fn a_misaligned_locked_operation_in_the_parallel_mode_is_refused_and_counted() {
        use od_isa::OperandSize::Dword;
        let memory = split_memory(0, 0x7007, 0x8007, crate::ExecutionMode::Parallel);
        let before = split_state(&memory);
        let mut log = Vec::new();
        {
            let mut note = |r| log.push(r);
            let mut adapter = lock_adapter(&memory, &mut note);
            for offset in [1u64, 0x3e, 0xffe] {
                let result =
                    adapter.atomic_fetch_add_integer(DATA_LINEAR + offset, context(), Dword, 1);
                assert!(
                    matches!(&result, Err(MemoryError::Backend(m)) if m.contains("not naturally aligned")),
                    "+{offset:#x}: {result:?}"
                );
            }
        }
        assert_eq!(memory.split_lock_refusals(), 3);
        assert_eq!(split_state(&memory), before);
        assert!(log.is_empty());
        // The aligned operand of the same memory still runs on the host atomic.
        let mut note = |_| {};
        let mut adapter = lock_adapter(&memory, &mut note);
        adapter
            .atomic_fetch_add_integer(DATA_LINEAR + 0x40, context(), Dword, 1)
            .unwrap();
        assert_eq!(memory.split_lock_refusals(), 3);
    }

    /// S2-51: `atomic_rmw_u128` (CMPXCHG16B) has no host 128-bit primitive, so the parallel mode
    /// refuses it before any walk: memory, tables and notifications stay untouched and the
    /// update closure is never called. The interleaving mode keeps the trait's serial pair, which
    /// this adapter's `read` does not serve for 16 bytes: a refusal before any effect too.
    #[test]
    fn a_locked_u128_operation_is_refused_in_the_parallel_mode_and_changes_nothing() {
        let memory = split_memory(0, 0x7007, 0x8007, crate::ExecutionMode::Parallel);
        let before = split_state(&memory);
        let mut log = Vec::new();
        {
            let mut note = |r| log.push(r);
            let mut adapter = lock_adapter(&memory, &mut note);
            let mut called = false;
            let result = adapter.atomic_rmw_u128(DATA_LINEAR + 0x40, context(), &mut |old| {
                called = true;
                old
            });
            assert!(
                matches!(&result, Err(MemoryError::Backend(m)) if m.contains("parallel mode")),
                "{result:?}"
            );
            assert!(!called);
        }
        assert_eq!(split_state(&memory), before);
        assert!(log.is_empty());

        let memory = split_memory(0, 0x7007, 0x8007, crate::ExecutionMode::Interleaved);
        let before = split_state(&memory);
        let mut note = |_| {};
        let mut adapter = lock_adapter(&memory, &mut note);
        let result = adapter.atomic_rmw_u128(DATA_LINEAR + 0x40, context(), &mut |old| !old);
        assert!(
            matches!(&result, Err(MemoryError::Backend(m)) if m.contains("read")),
            "{result:?}"
        );
        assert_eq!(split_state(&memory), before);
    }

    /// S2-21: `CMPXCHG8B` reaches this adapter as an eight-byte `atomic_compare_exchange_integer`,
    /// which has a host primitive: unlike `CMPXCHG16B` it is not refused in the parallel mode.
    /// An aligned operand runs on the host atomic (equal: replaced; unequal: the old value is
    /// returned and the data is unchanged), a misaligned one (a split lock) keeps the S6-31
    /// refusal in the parallel mode without the stop-all gate and runs as the serial S6-20 pair
    /// in the interleaving mode.
    #[test]
    fn an_eight_byte_compare_exchange_is_a_host_atomic_when_aligned_and_a_split_follows_s6_31() {
        use od_isa::OperandSize::Qword;
        let pattern = |offset: u64| -> u64 {
            (0..8u64).fold(0, |acc, i| {
                acc | (u64::from(((offset + i) * 7 + 3) as u8) << (8 * i))
            })
        };
        let memory = split_memory(0, 0x7007, 0x8007, crate::ExecutionMode::Parallel);
        let mut log = Vec::new();
        {
            let mut note = |r| log.push(r);
            let mut adapter = lock_adapter(&memory, &mut note);
            let old = pattern(0x40);
            // Unequal: the old value comes back, the data is unchanged.
            let observed = adapter
                .atomic_compare_exchange_integer(DATA_LINEAR + 0x40, context(), Qword, !old, 7)
                .unwrap();
            assert_eq!(observed, old);
            assert_eq!(
                memory.load_u64(GuestPhysicalAddress::new(0x7040)).unwrap(),
                old
            );
            // Equal: the replacement is stored.
            let observed = adapter
                .atomic_compare_exchange_integer(
                    DATA_LINEAR + 0x40,
                    context(),
                    Qword,
                    old,
                    0x0123_4567_89ab_cdef,
                )
                .unwrap();
            assert_eq!(observed, old);
            assert_eq!(
                memory.load_u64(GuestPhysicalAddress::new(0x7040)).unwrap(),
                0x0123_4567_89ab_cdef
            );
        }
        assert_eq!(memory.split_lock_refusals(), 0);
        // A misaligned operand in the parallel mode: refused before any effect, counted.
        let before = split_state(&memory);
        {
            let mut note = |_| {};
            let mut adapter = lock_adapter(&memory, &mut note);
            for offset in [1u64, 4, 0x3c] {
                let result = adapter.atomic_compare_exchange_integer(
                    DATA_LINEAR + 0x40 + offset,
                    context(),
                    Qword,
                    0,
                    1,
                );
                assert!(
                    matches!(&result, Err(MemoryError::Backend(m)) if m.contains("not naturally aligned")),
                    "+{offset:#x}: {result:?}"
                );
            }
        }
        assert_eq!(memory.split_lock_refusals(), 3);
        assert_eq!(split_state(&memory), before);
        // The interleaving mode runs the same misaligned operand as the serial pair.
        let memory = split_memory(0, 0x7007, 0x8007, crate::ExecutionMode::Interleaved);
        let mut note = |_| {};
        let mut adapter = lock_adapter(&memory, &mut note);
        let old = pattern(0x41);
        let observed = adapter
            .atomic_compare_exchange_integer(
                DATA_LINEAR + 0x41,
                context(),
                Qword,
                old,
                0x1122_3344_5566_7788,
            )
            .unwrap();
        assert_eq!(observed, old);
        assert_eq!(memory.split_lock_refusals(), 0);
        assert_eq!(
            (0..8u64)
                .map(|i| u64::from(data_byte(&memory, 0x41 + i)) << (8 * i))
                .fold(0, |a, b| a | b),
            0x1122_3344_5566_7788
        );
    }

    #[test]
    fn cpl3_aligned_qword_atomic_rmw_walks_for_write_and_commits_data_last() {
        use od_isa::OperandSize::Qword;
        let memory = lock_memory(0, 0x7007);
        memory
            .store_u64(GuestPhysicalAddress::new(0x7040), 5, |_| {})
            .unwrap();
        let data_generation = memory
            .code_page_generation(GuestPhysicalAddress::new(0x7040))
            .unwrap();
        let mut log = Vec::new();
        let old = {
            let mut note = |range| log.push(range);
            let mut adapter = lock_adapter(&memory, &mut note);
            adapter
                .atomic_rmw_integer(DATA_LINEAR + 0x40, user_context(), Qword, &mut |old| {
                    old + 1
                })
                .unwrap()
        };

        assert_eq!(old, 5);
        assert_eq!(
            memory.load_u64(GuestPhysicalAddress::new(0x7040)).unwrap(),
            6
        );
        assert_eq!(
            memory.code_page_generation(GuestPhysicalAddress::new(0x7040)),
            Ok(data_generation + 1)
        );
        assert!(
            log.contains(&range(0x7040)),
            "data commit is notified: {log:?}"
        );
        assert_eq!(
            table_entries(&memory),
            vec![0x2027, 0x3027, 0x4007, 0x5027, 0x7007, 0x7067]
        );
    }

    #[test]
    fn cpl3_atomic_rmw_faults_have_write_user_bits_without_operand_commit() {
        use od_isa::OperandSize::Qword;
        for (pte, expected_error, clear_wp) in [
            (0x7006, 0x06, false), // not present
            (0x7003, 0x07, false), // supervisor page
            (0x7005, 0x07, false), // read-only user page, WP enabled
            (0x7005, 0x07, true),  // CPL3 still cannot write with WP clear
        ] {
            let memory = lock_memory(0, pte);
            memory
                .store_u64(
                    GuestPhysicalAddress::new(0x7040),
                    0x1122_3344_5566_7788,
                    |_| {},
                )
                .unwrap();
            let before_frame = data_frame(&memory);
            let mut log = Vec::new();
            let result = {
                let mut note = |range| log.push(range);
                let mut adapter = lock_adapter_with_cr0(
                    &memory,
                    &mut note,
                    if clear_wp { CR0 & !(1 << 16) } else { CR0 },
                );
                adapter.atomic_rmw_integer(DATA_LINEAR + 0x40, user_context(), Qword, &mut |old| {
                    old + 1
                })
            };
            let Err(MemoryError::Architectural(fault)) = result else {
                panic!("expected an architectural write fault, got {result:?}");
            };
            assert_eq!(fault.vector, 14);
            assert_eq!(fault.error_code, Some(expected_error));
            assert_eq!(fault.linear_address, Some(DATA_LINEAR + 0x40));
            assert_eq!(data_frame(&memory), before_frame);
            let entries = table_entries(&memory);
            assert_eq!(entries[0] & 0x20, 0x20, "ancestor PML4 A may commit");
            assert_eq!(entries[1] & 0x20, 0x20, "ancestor PDP A may commit");
            assert_eq!(entries[3] & 0x20, 0x20, "ancestor PD A may commit");
            assert_eq!(entries[5] & 0x40, 0, "leaf D is not committed on a fault");
            assert!(
                log.iter().all(|range| range.start().get() < 0x7000),
                "walk may update paging prefixes, but the operand is not notified: {log:?}"
            );
        }
    }

    #[test]
    fn cpl3_write_intent_integer_read_walks_write_and_reads_only_after_full_validation() {
        use od_isa::OperandSize::Dword;
        let user = user_context();
        for (pte, expected_error) in [(0x7006, 0x06), (0x7003, 0x07), (0x7005, 0x07)] {
            let memory = lock_memory(0, pte);
            memory
                .store_u32(GuestPhysicalAddress::new(0x7040), 0x1122_3344, |_| {})
                .unwrap();
            let before = data_frame(&memory);
            let mut log = Vec::new();
            let result = {
                let mut note = |range| log.push(range);
                let mut adapter = lock_adapter(&memory, &mut note);
                adapter.read_integer_for_write(DATA_LINEAR + 0x40, user, Dword)
            };
            let Err(MemoryError::Architectural(fault)) = result else {
                panic!("expected write-intent page fault, got {result:?}");
            };
            assert_eq!(fault.vector, 14);
            assert_eq!(fault.error_code, Some(expected_error));
            assert_eq!(fault.linear_address, Some(DATA_LINEAR + 0x40));
            assert_eq!(data_frame(&memory), before);
            let entries = table_entries(&memory);
            assert_eq!(entries[5] & 0x40, 0, "no operand dirty bit on fault");
            assert!(log.iter().all(|range| range.start().get() < 0x7000));
        }

        let memory = lock_memory(0, 0x7007);
        memory
            .store_u32(GuestPhysicalAddress::new(0x7040), 0x89ab_cdef, |_| {})
            .unwrap();
        let before = data_frame(&memory);
        let mut log = Vec::new();
        let value = {
            let mut note = |range| log.push(range);
            let mut adapter = lock_adapter(&memory, &mut note);
            adapter
                .read_integer_for_write(DATA_LINEAR + 0x40, user, Dword)
                .unwrap()
        };
        assert_eq!(value, 0x89ab_cdef);
        assert_eq!(
            data_frame(&memory),
            before,
            "read-intent does not store or set D"
        );
        assert_eq!(table_entries(&memory)[5] & 0x40, 0);
        assert!(log.iter().all(|range| range.start().get() < 0x7000));

        // The first two bytes map, but the second 4 KiB translation is not present. No first-page
        // data read is issued until every write-intent span has passed its permission walk.
        let memory = lock_memory(0, 0x7007);
        memory
            .store_u16(GuestPhysicalAddress::new(0x7ffe), 0xa55a, |_| {})
            .unwrap();
        let before = data_frame(&memory);
        let mut log = Vec::new();
        let result = {
            let mut note = |range| log.push(range);
            let mut adapter = lock_adapter(&memory, &mut note);
            adapter.read_integer_for_write(DATA_LINEAR + 0xffe, user, Dword)
        };
        let Err(MemoryError::Architectural(fault)) = result else {
            panic!("expected second-page write fault, got {result:?}");
        };
        assert_eq!(fault.vector, 14);
        assert_eq!(fault.error_code, Some(0x06));
        assert_eq!(fault.linear_address, Some(DATA_LINEAR + 0x1000));
        assert_eq!(data_frame(&memory), before);
        assert_eq!(table_entries(&memory)[5] & 0x40, 0);
        assert!(log.iter().all(|range| range.start().get() < 0x7000));
    }

    #[test]
    fn cpl3_write_intent_integer_read_accepts_last_canonical_dword() {
        let memory = lock_memory(0, 0x7007);
        let address = u64::MAX - 3;
        let mut log = Vec::new();
        let result = {
            let mut note = |range| log.push(range);
            lock_adapter(&memory, &mut note).read_integer_for_write(
                address,
                user_context(),
                od_isa::OperandSize::Dword,
            )
        };
        let Err(MemoryError::Architectural(fault)) = result else {
            panic!("expected a page fault for the absent high PML4 entry, got {result:?}");
        };
        assert_eq!(fault.vector, 14);
        assert_eq!(fault.error_code, Some(0x06));
        assert_eq!(fault.linear_address, Some(address));
        assert!(log.is_empty());
    }

    #[test]
    fn cpl3_write_intent_integer_read_refuses_the_read_only_scope_without_effects() {
        let bus = bus(0x7007);
        let paging = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap();
        let report = with_paged_mov_dword_memory(&bus, paging, 0x0040_0000, |memory| {
            memory.read_integer_for_write(
                DATA_LINEAR + 0x40,
                user_context(),
                od_isa::OperandSize::Dword,
            )
        });
        assert!(matches!(
            report.callback_result,
            Err(MemoryError::Backend(_))
        ));
        assert!(report.committed_ad_updates.is_empty());
        assert!(report.committed_writes.is_empty());
        assert_eq!(
            bus.physical_memory()
                .load_u64(GuestPhysicalAddress::new(0x5000))
                .unwrap(),
            0x7007
        );
    }

    #[test]
    fn cpl3_write_intent_integer_read_supports_two_mib_leaf_permissions() {
        use od_isa::OperandSize::Dword;
        let user = user_context();
        let memory = lock_memory_2m(0x200087); // present, writable, user, 2 MiB leaf
        memory
            .store_u32(GuestPhysicalAddress::new(0x200040), 0x7654_3210, |_| {})
            .unwrap();
        let mut note = |_| {};
        let mut adapter = lock_adapter(&memory, &mut note);
        assert_eq!(
            adapter
                .read_integer_for_write(DATA_LINEAR + 0x40, user, Dword)
                .unwrap(),
            0x7654_3210
        );
        assert_eq!(two_mib_entries(&memory)[3] & 0x40, 0);

        let memory = lock_memory_2m(0x200085); // present, user, read-only 2 MiB leaf
        let mut note = |_| {};
        let mut adapter = lock_adapter(&memory, &mut note);
        let result = adapter.read_integer_for_write(DATA_LINEAR + 0x40, user, Dword);
        assert!(matches!(
            result,
            Err(MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(7),
                linear_address: Some(address),
            })) if address == DATA_LINEAR + 0x40
        ));
        assert_eq!(two_mib_entries(&memory)[3] & 0x40, 0);
    }

    fn active_cpl3_lock_state() -> LongModeState {
        use od_isa::{CpuActivity, StiShadow};
        let mut state = LongModeState::default();
        state.rip = 0x0040_0000;
        state.cr0 = Some(CR0);
        state.cr4 = Some(CR4);
        state.activity = Some(CpuActivity::Running);
        state.sti_shadow = Some(StiShadow::Clear);
        state.rflags = 0x202 | 0x0cd5;
        state.cs_selector = Some(0x33);
        state.cs_cache = Some(CodeSegmentDescriptorCache {
            base: 0,
            limit: 0xffff_ffff,
            access: 0xfb,
            flags: 0x2,
        });
        state.ss_selector = Some(0x2b);
        state.ss_cache = Some(DataSegmentDescriptorCache {
            base: 0,
            limit: 0xffff_ffff,
            access: 0xf3,
            flags: 0xc,
        });
        state
    }

    #[test]
    fn aligned_cpl3_glibc_dword_atomics_use_the_live_write_walk() {
        use od_isa::Register::{Rax, Rcx, Rdx, Rsi};
        use od_isa::execute::{ControlRegisterPolicy, Cpl0ProfileSetExit};
        use od_isa::memory::ArchitecturalFault;

        for (code, accumulator, expected_data, expected_rax) in [
            (
                &[0xf0, 0x0f, 0xb1, 0x11][..],
                0xdead_0000_0000_0005,
                9,
                0xdead_0000_0000_0005,
            ),
            (&[0xf0, 0x0f, 0xb1, 0x11][..], 0xdead_0000_0000_000a, 5, 5),
            (&[0x87, 0x06][..], 0xdead_0000_0000_0008, 8, 5),
        ] {
            for (data_pte, offset, expected_error) in [
                (0x7007, 0x40, None),
                (0x7006, 0x40, Some(0x06)),
                (0x7005, 0x40, Some(0x07)),
                (0x7007, 0x41, None),
            ] {
                let memory = lock_memory(0, data_pte);
                memory
                    .store_bytes(GuestPhysicalAddress::new(0x7000), code, |_| {})
                    .unwrap();
                memory
                    .store_u64(
                        GuestPhysicalAddress::new(0x7040),
                        0xaabb_ccdd_0000_0005,
                        |_| {},
                    )
                    .unwrap();
                let address = DATA_LINEAR + offset;
                let mut state = active_cpl3_lock_state();
                state.gpr[Rax as usize] = accumulator;
                state.gpr[Rcx as usize] = address;
                state.gpr[Rdx as usize] = 9;
                state.gpr[Rsi as usize] = address;
                let before = state.clone();
                let mut writes = Vec::new();
                let result = {
                    let mut note = |range| writes.push(range);
                    let mut adapter = lock_adapter(&memory, &mut note);
                    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
                    run_with_cpl3_profile_set(
                        &mut state,
                        &mut adapter,
                        RunConfig {
                            instruction_budget: 1,
                            stop_rip: None,
                            context: user_context(),
                        },
                        od_isa::INTEGER_FLAGS,
                        &set,
                        None,
                        || false,
                    )
                    .unwrap()
                };
                let operand = GuestPhysicalAddress::new(0x7040);
                if expected_error.is_none() && offset == 0x40 {
                    assert_eq!(result.instructions_executed, 1, "{code:02x?}");
                    assert_eq!(
                        result.exit,
                        Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
                    );
                    assert_eq!(state.gpr[Rax as usize], expected_rax);
                    assert_eq!(state.rip, before.rip + code.len() as u64);
                    if code[0] == 0x87 {
                        assert_eq!(state.rflags, before.rflags);
                    } else {
                        assert_eq!(state.rflags & 0x40 != 0, accumulator as u32 == 5);
                    }
                    assert_eq!(
                        memory.load_u64(operand).unwrap(),
                        0xaabb_ccdd_0000_0000 | expected_data
                    );
                    assert_ne!(table_entries(&memory)[5] & 0x40, 0);
                    assert!(writes.contains(&GuestPhysicalRange::new(operand, 4).unwrap()));
                } else {
                    assert_eq!(result.instructions_executed, 0, "{code:02x?}");
                    assert_eq!(state.gpr, before.gpr);
                    assert_eq!(state.rip, before.rip);
                    assert_eq!(state.rflags, before.rflags);
                    let expected = match expected_error {
                        Some(error_code) => RunExit::Fault(ArchitecturalFault {
                            vector: 14,
                            error_code: Some(error_code),
                            linear_address: Some(address),
                        }),
                        None => RunExit::Unsupported {
                            opcode: if code[0] == 0x87 { 0x87 } else { 0xb1 },
                        },
                    };
                    assert_eq!(result.exit, Cpl0ProfileSetExit::Run(expected));
                    assert_eq!(memory.load_u64(operand).unwrap(), 0xaabb_ccdd_0000_0005);
                    assert_eq!(table_entries(&memory)[5] & 0x40, 0);
                    assert!(writes.iter().all(|range| range.start() != operand));
                }
            }
        }
    }

    #[test]
    fn cpl3_lock_add_bytes_execute_through_the_paged_memory_adapter() {
        use od_isa::execute::{ControlRegisterPolicy, Cpl0ProfileSetExit};
        let code = [0xf0, 0x48, 0x83, 0x03, 0x01]; // lock add qword [rbx], 1
        let initial = 0x7fff_ffff_ffff_ffffu64;
        let memory = lock_memory(0, 0x7007);
        memory
            .store_bytes(GuestPhysicalAddress::new(0x7000), &code, |_| {})
            .unwrap();
        memory
            .store_u64(GuestPhysicalAddress::new(0x7040), initial, |_| {})
            .unwrap();
        let mut state = active_cpl3_lock_state();
        state.gpr[od_isa::Register::Rbx as usize] = DATA_LINEAR + 0x40;
        let mut log = Vec::new();
        let result = {
            let mut note = |range| log.push(range);
            let mut adapter = lock_adapter(&memory, &mut note);
            let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
            run_with_cpl3_profile_set(
                &mut state,
                &mut adapter,
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: None,
                    context: user_context(),
                },
                od_isa::INTEGER_FLAGS,
                &set,
                None,
                || false,
            )
            .unwrap()
        };

        assert_eq!(result.instructions_executed, 1, "{result:#?}");
        assert!(matches!(
            result.exit,
            Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
        ));
        assert_eq!(state.rip, 0x0040_0005);
        assert_eq!(state.rflags & 0x0cd5, 0x0c94);
        assert_eq!(
            memory.load_u64(GuestPhysicalAddress::new(0x7040)).unwrap(),
            0x8000_0000_0000_0000
        );
        assert!(log.contains(&range(0x7040)));
    }

    #[test]
    fn cpl3_atomic_rmw_2m_leaf_sets_dirty_only_after_successful_write_intent() {
        use od_isa::OperandSize::Qword;
        const DATA_PHYS: u64 = 0x20_0000;
        let memory = lock_memory_2m(DATA_PHYS | 0x87);
        memory
            .store_u64(GuestPhysicalAddress::new(DATA_PHYS + 0x40), 9, |_| {})
            .unwrap();
        let mut log = Vec::new();
        let old = {
            let mut note = |range| log.push(range);
            let mut adapter = lock_adapter(&memory, &mut note);
            adapter
                .atomic_rmw_integer(DATA_LINEAR + 0x40, user_context(), Qword, &mut |old| {
                    old + 1
                })
                .unwrap()
        };
        assert_eq!(old, 9);
        assert_eq!(
            memory
                .load_u64(GuestPhysicalAddress::new(DATA_PHYS + 0x40))
                .unwrap(),
            10
        );
        let entries = two_mib_entries(&memory);
        assert_eq!(entries[0] & 0x20, 0x20);
        assert_eq!(entries[1] & 0x20, 0x20);
        assert_eq!(entries[3] & 0x60, 0x60, "large leaf A and D are committed");
        assert!(log.contains(&range(DATA_PHYS + 0x40)));

        let memory = lock_memory_2m(DATA_PHYS | 0x85); // present, user, read-only 2 MiB leaf
        memory
            .store_u64(
                GuestPhysicalAddress::new(DATA_PHYS + 0x40),
                0x1122_3344_5566_7788,
                |_| {},
            )
            .unwrap();
        let before = memory
            .load_u64(GuestPhysicalAddress::new(DATA_PHYS + 0x40))
            .unwrap();
        let mut log = Vec::new();
        let result = {
            let mut note = |range| log.push(range);
            let mut adapter = lock_adapter(&memory, &mut note);
            adapter.atomic_rmw_integer(DATA_LINEAR + 0x40, user_context(), Qword, &mut |old| {
                old + 1
            })
        };
        let Err(MemoryError::Architectural(fault)) = result else {
            panic!("expected a CPL3 write-protection fault, got {result:?}");
        };
        assert_eq!(fault.vector, 14);
        assert_eq!(fault.error_code, Some(0x07));
        assert_eq!(fault.linear_address, Some(DATA_LINEAR + 0x40));
        assert_eq!(
            memory
                .load_u64(GuestPhysicalAddress::new(DATA_PHYS + 0x40))
                .unwrap(),
            before
        );
        let entries = two_mib_entries(&memory);
        assert_eq!(entries[0] & 0x20, 0x20);
        assert_eq!(entries[1] & 0x20, 0x20);
        assert_eq!(entries[3] & 0x40, 0, "large leaf D stays clear on fault");
        assert!(
            log.iter().all(|range| range.start().get() < DATA_PHYS),
            "page-walk A updates may commit, operand data is untouched: {log:?}"
        );
    }

    #[test]
    fn two_cpl3_adapters_share_exact_aligned_qword_increment_updates() {
        use od_isa::OperandSize::Qword;
        const VCPUS: usize = 2;
        const ROUNDS: usize = 1_000;
        let mut memory = lock_memory(0, 0x7007);
        memory
            .store_u64(GuestPhysicalAddress::new(0x7040), 0, |_| {})
            .unwrap();
        memory.set_execution_mode(crate::ExecutionMode::Parallel);
        let barrier = std::sync::Barrier::new(VCPUS);
        std::thread::scope(|scope| {
            for _ in 0..VCPUS {
                let (memory, barrier) = (&memory, &barrier);
                scope.spawn(move || {
                    let mut note = |_| {};
                    let mut adapter = lock_adapter(memory, &mut note);
                    barrier.wait();
                    for _ in 0..ROUNDS {
                        adapter
                            .atomic_rmw_integer(
                                DATA_LINEAR + 0x40,
                                user_context(),
                                Qword,
                                &mut |old| old + 1,
                            )
                            .unwrap();
                    }
                });
            }
        });
        assert_eq!(
            memory.load_u64(GuestPhysicalAddress::new(0x7040)).unwrap(),
            (VCPUS * ROUNDS) as u64
        );
        assert_eq!(memory.split_lock_refusals(), 0);
    }

    /// S6-31: four host threads, each with its own adapter on one shared memory, run the four
    /// locked entry points at every width on tables whose accessed and dirty bits are all clear:
    /// the first walks race to set the bits with compare-and-replace and lose none; every sum and
    /// every counter protected by an `XCHG`-style lock is exact.
    #[test]
    fn locked_operations_of_four_adapters_on_one_ram_are_indivisible() {
        use od_isa::OperandSize::{Byte, Dword, Qword, Word};
        const THREADS: u64 = 4;
        const ROUNDS: u64 = 1_500;
        let mut memory = lock_memory(0, 0x7007);
        memory.set_execution_mode(crate::ExecutionMode::Parallel);
        for (index, size) in [Byte, Word, Dword, Qword].into_iter().enumerate() {
            let line = |slot: u64| DATA_LINEAR + 0x100 + index as u64 * 0x100 + slot * 0x40;
            let (sum, rmw, lock, data) = (line(0), line(1), line(2), line(3));
            let start = std::sync::Barrier::new(THREADS as usize);
            std::thread::scope(|scope| {
                for _ in 0..THREADS {
                    let (memory, start) = (&memory, &start);
                    scope.spawn(move || {
                        let mut note = |_| {};
                        let mut adapter = lock_adapter(memory, &mut note);
                        start.wait();
                        for _ in 0..ROUNDS {
                            adapter
                                .atomic_fetch_add_integer(sum, context(), size, 1)
                                .unwrap();
                            adapter
                                .atomic_rmw_integer(rmw, context(), size, &mut |old| old + 1)
                                .unwrap();
                            // Lock by compare-exchange, critical section of ordinary accesses,
                            // release by exchange.
                            while adapter
                                .atomic_compare_exchange_integer(lock, context(), size, 0, 1)
                                .unwrap()
                                != 0
                            {
                                std::hint::spin_loop();
                            }
                            let value = adapter.read_integer(data, context(), size).unwrap();
                            std::hint::spin_loop();
                            adapter
                                .write_integer(data, context(), size, value + 1)
                                .unwrap();
                            adapter
                                .atomic_exchange_integer(lock, context(), size, 0)
                                .unwrap();
                        }
                    });
                }
            });
            let total = (THREADS * ROUNDS) & size.mask();
            let mut note = |_| {};
            let mut adapter = lock_adapter(&memory, &mut note);
            for (name, address) in [("sum", sum), ("rmw", rmw), ("data", data), ("lock", lock)] {
                let expected = if name == "lock" { 0 } else { total };
                assert_eq!(
                    adapter.read_integer(address, context(), size).unwrap(),
                    expected,
                    "{name} at width {}",
                    size.bytes()
                );
            }
        }
        // The racing walks set every accessed bit and the leaf's dirty bit, once, and kept the
        // rest of the entries.
        assert_eq!(
            table_entries(&memory),
            vec![
                0x2007 | 0x20,
                0x3007 | 0x20,
                0x4007,
                0x5007 | 0x20,
                0x7007,
                0x7007 | 0x60
            ]
        );
        assert_eq!(memory.split_lock_refusals(), 0);
    }

    /// S6-31, the instruction level: four host threads, each interpreting real `LOCK` and `XCHG`
    /// instructions through its own adapter on one memory. A `lock cmpxchg` retry loop, a
    /// `lock xadd`, a `lock inc` and an `xchg` spin lock guarding an ordinary load-increment-store:
    /// no update is lost. The bytes are the forms of the S6-05 corpus (S6-20).
    #[test]
    fn locked_instructions_of_four_threads_lose_no_update() {
        use od_isa::Register;
        use od_isa::execute::{RunExit, run};
        const THREADS: usize = 4;
        const ROUNDS: u64 = 3_000;
        const CODE: u64 = 0x0040_0800;
        let counter = (DATA_LINEAR + 0x100) as u32;
        let (cell, inc_cell, xadd_cell, lock_cell, guarded) = (
            counter,
            counter + 0x40,
            counter + 0x80,
            counter + 0xc0,
            counter + 0x100,
        );
        let abs = |value: u32| value.to_le_bytes();
        let mut program = Vec::new();
        // outer:
        let outer = program.len();
        // mov eax, [cell]
        program.extend([0x8b, 0x04, 0x25]);
        program.extend(abs(cell));
        let retry = program.len();
        // lea ecx, [rax+1]
        program.extend([0x8d, 0x48, 0x01]);
        // lock cmpxchg [cell], ecx
        program.extend([0xf0, 0x0f, 0xb1, 0x0c, 0x25]);
        program.extend(abs(cell));
        // jnz retry
        let jnz = program.len() + 2;
        program.extend([0x75, (retry as i64 - jnz as i64) as i8 as u8]);
        // lock inc dword [inc_cell]
        program.extend([0xf0, 0xff, 0x04, 0x25]);
        program.extend(abs(inc_cell));
        // mov edx, 3 ; lock xadd [xadd_cell], edx
        program.extend([0xba, 3, 0, 0, 0]);
        program.extend([0xf0, 0x0f, 0xc1, 0x14, 0x25]);
        program.extend(abs(xadd_cell));
        // acquire: mov ebx, 1 ; xchg [lock_cell], ebx ; test ebx, ebx ; jnz acquire
        let acquire = program.len();
        program.extend([0xbb, 1, 0, 0, 0]);
        program.extend([0x87, 0x1c, 0x25]);
        program.extend(abs(lock_cell));
        program.extend([0x85, 0xdb]);
        let jnz = program.len() + 2;
        program.extend([0x75, (acquire as i64 - jnz as i64) as i8 as u8]);
        // mov edi, [guarded] ; inc edi ; mov [guarded], edi
        program.extend([0x8b, 0x3c, 0x25]);
        program.extend(abs(guarded));
        program.extend([0xff, 0xc7]);
        program.extend([0x89, 0x3c, 0x25]);
        program.extend(abs(guarded));
        // release: xchg [lock_cell], ebx with ebx = 0 (ebx is zero here: the acquire loop
        // exits only when the exchanged-out value was 0 and nothing below touches ebx);
        // an atomic write, so the test does not depend on the order of ordinary accesses.
        // Future litmus (S6-36): the variant with a release by an ordinary
        // `mov dword [lock_cell], 0` = [0xc7, 0x04, 0x25, abs(lock_cell), 0u32 LE] in the parallel
        // mode. The order of ordinary accesses is set by S6-32 (SeqCst in the parallel mode); this
        // test keeps the atomic release, so it does not depend on it.
        program.extend([0x87, 0x1c, 0x25]);
        program.extend(abs(lock_cell));
        // dec esi ; jnz outer
        program.extend([0xff, 0xce]);
        let jnz = program.len() + 2;
        program.extend([0x75, (outer as i64 - jnz as i64) as i8 as u8]);
        let end = CODE + program.len() as u64;

        let mut memory = lock_memory(AD, 0x7067);
        memory.set_execution_mode(crate::ExecutionMode::Parallel);
        for (offset, byte) in program.iter().enumerate() {
            memory
                .store_u8(
                    GuestPhysicalAddress::new(0x7800 + offset as u64),
                    *byte,
                    |_| {},
                )
                .unwrap();
        }
        let start = std::sync::Barrier::new(THREADS);
        std::thread::scope(|scope| {
            for _ in 0..THREADS {
                let (memory, start) = (&memory, &start);
                scope.spawn(move || {
                    let mut note = |_| {};
                    let mut adapter = lock_adapter(memory, &mut note);
                    start.wait();
                    let mut state = LongModeState {
                        rip: CODE,
                        rflags: 0x2,
                        ..LongModeState::default()
                    };
                    state.gpr[Register::Rsi as usize] = ROUNDS;
                    let result = run(
                        &mut state,
                        &mut adapter,
                        od_isa::execute::RunConfig {
                            instruction_budget: 1_000_000,
                            stop_rip: Some(end),
                            context: context(),
                        },
                        || false,
                    );
                    assert_eq!(
                        result.exit,
                        RunExit::StopAddress,
                        "{:?}",
                        adapter.memory_error
                    );
                    assert_eq!(state.rip, end);
                });
            }
        });
        let load = |address: u32| {
            memory
                .load_u32(GuestPhysicalAddress::new(
                    0x7000 + u64::from(address & 0xfff),
                ))
                .unwrap()
        };
        let total = (THREADS as u32) * (ROUNDS as u32);
        assert_eq!(load(cell), total, "cmpxchg retry loop");
        assert_eq!(load(inc_cell), total, "lock inc");
        assert_eq!(load(xadd_cell), 3 * total, "lock xadd");
        assert_eq!(
            load(guarded),
            total,
            "xchg spin lock around an ordinary increment"
        );
        assert_eq!(load(lock_cell), 0, "lock released");
    }

    /// S6-32: the adapter's `fence` reaches the memory's host fence, which exists only in the
    /// parallel mode; no byte and no generation changes in either mode.
    #[test]
    fn the_adapter_fence_places_the_host_fence_of_the_mode_only() {
        use od_isa::FenceKind;
        for (mode, issued) in [
            (crate::ExecutionMode::Interleaved, 0u64),
            (crate::ExecutionMode::Parallel, 3),
        ] {
            let mut memory = lock_memory(AD, 0x7067);
            memory.set_execution_mode(mode);
            let before: Vec<u64> = (0..0x8000u64)
                .step_by(8)
                .map(|at| memory.load_u64(GuestPhysicalAddress::new(at)).unwrap())
                .collect();
            let generation = memory.code_page_generation(GuestPhysicalAddress::new(0x7000));
            let mut note = |_| panic!("a fence notifies no write");
            let mut adapter = lock_adapter(&memory, &mut note);
            for kind in [FenceKind::Full, FenceKind::Store, FenceKind::Load] {
                adapter.fence(kind).unwrap();
            }
            drop(adapter);
            assert_eq!(memory.host_fences_issued(), issued, "{mode:?}");
            let after: Vec<u64> = (0..0x8000u64)
                .step_by(8)
                .map(|at| memory.load_u64(GuestPhysicalAddress::new(at)).unwrap())
                .collect();
            assert_eq!(after, before, "{mode:?}: RAM unchanged");
            assert_eq!(
                memory.code_page_generation(GuestPhysicalAddress::new(0x7000)),
                generation,
                "{mode:?}"
            );
        }
    }

    /// S2-32f: the decoder looks one or two bytes past `9B` for the spellings `9B D9 30` and `9B DD 30` of
    /// FSTENV/FSAVE. A bare FWAIT on the last byte of its page must not reach into the next page: no fetch,
    /// no walk of the next page (its accessed bit stays clear), no recorded page fault or memory error of the
    /// adapter. The next page holds `D9 30` (mapped case) or nothing (unmapped case); either way the FWAIT
    /// retires alone and RIP is the first byte of the next page.
    #[test]
    fn a_bare_fwait_on_the_last_byte_of_a_page_leaves_no_record_of_the_next_page() {
        use od_isa::execute::{
            ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, GuestFpState, RunConfig,
            run_with_cpl0_profile_set,
        };
        use od_isa::{CpuActivity, StiShadow};
        const LAST_BYTE: u64 = 0x0040_0fff;
        // The leaf of linear 0x401000 (the second entry of the page table at 0x4000): unmapped, or frame 0x8000.
        for next_leaf in [0u64, 0x8007] {
            let memory = lock_memory(0, 0x7007);
            memory
                .store_u8(GuestPhysicalAddress::new(0x7fff), 0x9b, |_| {})
                .unwrap();
            memory
                .store_u64(GuestPhysicalAddress::new(0x4008), next_leaf, |_| {})
                .unwrap();
            for (offset, byte) in [0xd9u8, 0x30].into_iter().enumerate() {
                memory
                    .store_u8(
                        GuestPhysicalAddress::new(0x8000 + offset as u64),
                        byte,
                        |_| {},
                    )
                    .unwrap();
            }
            let leaf_before = memory.load_u64(GuestPhysicalAddress::new(0x4008)).unwrap();
            assert_eq!(leaf_before, next_leaf);
            let mut fp = GuestFpState::initial();
            // The member of the set refuses a cell whose FCS/FDS are not represented.
            fp.x87.instruction_selector = Some(0);
            fp.x87.data_selector = Some(0);
            let cell = std::cell::RefCell::new(fp);
            let mut log = Vec::new();
            let mut note = |range| log.push(range);
            let mut adapter = lock_adapter(&memory, &mut note);
            let mut state = LongModeState {
                rip: LAST_BYTE,
                rflags: 0x2,
                cr0: Some(CR0),
                cr4: Some(CR4),
                activity: Some(CpuActivity::Running),
                sti_shadow: Some(StiShadow::Clear),
                ..LongModeState::default()
            };
            let result = run_with_cpl0_profile_set(
                &mut state,
                &mut adapter,
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: None,
                    context: context(),
                },
                od_isa::INTEGER_FLAGS,
                &Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
                    .with_cpuid(&od_isa::cpuid::CpuidProfile::X8664V1Stage4A)
                    .with_fp_state(&cell),
                None,
                || false,
            )
            .unwrap();
            let label = format!("next leaf {next_leaf:#x}");
            assert_eq!(result.instructions_executed, 1, "{label}");
            assert!(
                matches!(
                    result.exit,
                    Cpl0ProfileSetExit::Run(od_isa::execute::RunExit::InstructionBudgetExhausted)
                ),
                "{label}: {:?}",
                result.exit
            );
            assert_eq!(state.rip, 0x0040_1000, "{label}: the bare FWAIT retired");
            assert!(
                adapter.page_fault.is_none(),
                "{label}: no page fault record"
            );
            assert!(
                adapter.memory_error.is_none(),
                "{label}: no memory error record"
            );
            assert!(
                adapter.access_error.is_none(),
                "{label}: no access error record"
            );
            drop(adapter);
            // The notifications are those of the accessed bits of the code page's own entries: no range covers
            // the next leaf (a start-only comparison would miss a range that begins below it and extends over it).
            assert!(
                log.iter().all(|range| {
                    !(range.start().get() as u128 <= 0x4008 && 0x4008 < range.end_exclusive())
                }),
                "{label}: no notification for the leaf of the next page: {log:?}"
            );
            // The walk of the code page sets the accessed bit of its own entries; the entry of the next page stays.
            assert_eq!(
                memory.load_u64(GuestPhysicalAddress::new(0x4008)).unwrap(),
                leaf_before,
                "{label}: the leaf of the next page is untouched"
            );
        }
    }

    /// S6-32, the instruction level: the interpreter's CPL0 set runs real `MFENCE`, `SFENCE`
    /// and `LFENCE` bytes (and an ordinary store and load around them) through the adapter. The
    /// final state, the retired count and every byte of RAM are the same in both modes; the
    /// parallel mode issued exactly three host fences, the interleaving mode none.
    #[test]
    fn the_cpl0_set_fences_give_the_same_result_in_both_modes() {
        use od_isa::execute::{
            ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, RunConfig,
            run_with_cpl0_profile_set,
        };
        use od_isa::{CpuActivity, Register, StiShadow};
        const CODE: u64 = 0x0040_0800;
        let data = DATA_LINEAR + 0x300;
        let abs = |value: u64| (value as u32).to_le_bytes();
        let mut program = Vec::new();
        // mov dword [data], 0x11223344 ; mfence ; sfence ; mov eax, [data] ; lfence ; mfence
        program.extend([0xc7, 0x04, 0x25]);
        program.extend(abs(data));
        program.extend(0x1122_3344u32.to_le_bytes());
        program.extend([0x0f, 0xae, 0xf0, 0x0f, 0xae, 0xf8]);
        program.extend([0x8b, 0x04, 0x25]);
        program.extend(abs(data));
        program.extend([0x0f, 0xae, 0xe8, 0x0f, 0xae, 0xf0]);
        let instructions = 6;
        let mut results = Vec::new();
        for (mode, issued) in [
            (crate::ExecutionMode::Interleaved, 0u64),
            (crate::ExecutionMode::Parallel, 4),
        ] {
            let mut memory = lock_memory(AD, 0x7067);
            memory.set_execution_mode(mode);
            for (offset, byte) in program.iter().enumerate() {
                memory
                    .store_u8(
                        GuestPhysicalAddress::new(0x7800 + offset as u64),
                        *byte,
                        |_| {},
                    )
                    .unwrap();
            }
            let mut log = Vec::new();
            let mut note = |range| log.push(range);
            let mut adapter = lock_adapter(&memory, &mut note);
            let mut state = LongModeState {
                rip: CODE,
                rflags: 0x2,
                cr0: Some(CR0),
                cr4: Some(CR4),
                activity: Some(CpuActivity::Running),
                sti_shadow: Some(StiShadow::Clear),
                ..LongModeState::default()
            };
            let result = run_with_cpl0_profile_set(
                &mut state,
                &mut adapter,
                RunConfig {
                    instruction_budget: instructions,
                    stop_rip: None,
                    context: context(),
                },
                od_isa::INTEGER_FLAGS,
                &Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1),
                None,
                || false,
            )
            .unwrap();
            drop(adapter);
            assert_eq!(result.instructions_executed, instructions, "{mode:?}");
            assert!(
                matches!(
                    result.exit,
                    Cpl0ProfileSetExit::Run(od_isa::execute::RunExit::InstructionBudgetExhausted)
                ),
                "{mode:?}: {:?}",
                result.exit
            );
            assert_eq!(state.gpr[Register::Rax as usize], 0x1122_3344, "{mode:?}");
            assert_eq!(memory.host_fences_issued(), issued, "{mode:?}");
            let ram: Vec<u64> = (0..0x8000u64)
                .step_by(8)
                .map(|at| memory.load_u64(GuestPhysicalAddress::new(at)).unwrap())
                .collect();
            let generations: Vec<_> = (0..8u64)
                .map(|page| memory.code_page_generation(GuestPhysicalAddress::new(page * 0x1000)))
                .collect();
            assert!(!log.is_empty(), "{mode:?}: the store was notified");
            results.push((
                format!("{state:?}"),
                result.instructions_executed,
                ram,
                format!("{log:?}"),
                format!("{generations:?}"),
            ));
        }
        assert_eq!(
            results[0], results[1],
            "both modes: state, count and every RAM byte"
        );
    }
}

#[cfg(test)]
mod pit_word_watch_tests {
    use super::SetPortWatch;
    use od_isa::PortIo;

    #[derive(Default)]
    struct Recorder {
        words: Vec<(u16, u16)>,
    }

    impl PortIo for Recorder {
        type Error = ();

        fn read_u8(&mut self, _port: u16) -> Result<u8, ()> {
            Err(())
        }

        fn write_u8(&mut self, _port: u16, _value: u8) -> Result<(), ()> {
            Err(())
        }

        fn read_u16(&mut self, _port: u16) -> Result<u16, ()> {
            Ok(0x1234)
        }

        fn write_u16(&mut self, port: u16, value: u16) -> Result<(), ()> {
            self.words.push((port, value));
            Ok(())
        }
    }

    /// S4-38L9r: the word forwarders keep the width (the byte default would split it) and a
    /// retired word write that covers a PIT port counts as a PIT write.
    #[test]
    fn word_accesses_pass_through_and_a_word_write_over_the_pit_ports_is_noted() {
        for (port, noted) in [(0x3e_u16, false), (0x3f, true), (0x43, true), (0x44, false)] {
            let mut watch = SetPortWatch {
                inner: Recorder::default(),
                pit_written: false,
                port_accessed: false,
            };
            assert_eq!(watch.read_u16(port), Ok(0x1234));
            assert_eq!(watch.write_u16(port, 0xbeef), Ok(()));
            assert_eq!(watch.inner.words, [(port, 0xbeef)]);
            assert_eq!(watch.pit_written, noted, "{port:#x}");
        }
    }
}

#[cfg(test)]
mod pit_dword_watch_tests {
    use super::SetPortWatch;
    use od_isa::PortIo;

    #[derive(Default)]
    struct Recorder {
        dwords: Vec<(u16, u32)>,
    }

    impl PortIo for Recorder {
        type Error = ();

        fn read_u8(&mut self, _port: u16) -> Result<u8, ()> {
            Err(())
        }

        fn write_u8(&mut self, _port: u16, _value: u8) -> Result<(), ()> {
            Err(())
        }

        fn read_u32(&mut self, _port: u16) -> Result<u32, ()> {
            Ok(0x1234_5678)
        }

        fn write_u32(&mut self, port: u16, value: u32) -> Result<(), ()> {
            self.dwords.push((port, value));
            Ok(())
        }
    }

    /// S4-38L9u: the dword forwarders keep the width (the word default would split it) and a
    /// retired dword write that covers a PIT port counts as a PIT write.
    #[test]
    fn dword_accesses_pass_through_and_a_dword_write_over_the_pit_ports_is_noted() {
        for (port, noted) in [(0x3c_u16, false), (0x3d, true), (0x43, true), (0x44, false)] {
            let mut watch = SetPortWatch {
                inner: Recorder::default(),
                pit_written: false,
                port_accessed: false,
            };
            assert_eq!(watch.read_u32(port), Ok(0x1234_5678));
            assert_eq!(watch.write_u32(port, 0xdead_beef), Ok(()));
            assert_eq!(watch.inner.dwords, [(port, 0xdead_beef)]);
            assert_eq!(watch.pit_written, noted, "{port:#x}");
        }
    }
}
