//! Opt-in one-instruction Long64 bridge through the uncached RAM walker.

use crate::console_output_context::capture_frame_chain;
use crate::execution_observations::{
    DeliveredPageFaultObservation, ElfStackCopyProbe, ElfStackCopySnapshot, ElfStackCopyStatus,
    ElfStackCopyTargetCr3Capture, ElfStackCopyTargetCr3ChainStatus, ElfStackCopyTargetCr3Frame,
    FaultingPageFaultContext, FaultingPageWalkContext, InstructionPathControlFlow,
    InstructionPathRegisterValue, InstructionPathTraceRecord, LINUX_ELF_STACK_COPY_PROBE_CAPACITY,
    LINUX_ELF_STACK_COPY_RIP, LINUX_ELF_STACK_COPY_WALK_ENTRY_CAPACITY,
    LinuxExecutionObservationCollector, PendingPostIretObservation, PostIretPageWalkSnapshot,
};
use crate::fetch_cache_report::{C5Split, LinuxWalkCounts, WalkCounter};
use crate::fetch_cache_sweep::FetchCacheSweep;
use crate::fetch_translation_cache::{
    FetchCacheInvalidation, FetchCacheStats, FetchCacheTag, FetchTranslationCache, TranslationKey,
};
use crate::irq_source::IrqBoundaryHook;
use crate::linux_checkpoints::{BoundaryView, CheckpointRecorder};
use crate::page_walk::{
    translate_long64_2m_ram_read_only_with_tables, translate_long64_2m_ram_read_only_with_trace,
};
use crate::{
    GuestLinearAddress, GuestPhysicalAddress, GuestPhysicalRange, Long64ProbeConfig, MachineBus,
    MemoryRegionKind, RamAccessOperation, TrackedAccessError, TrackedAccessOutcome,
    TrackedAccessRequest,
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
use od_isa::long_decode::{AddressBase, Operand, Operation};
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
    bus: &mut MachineBus,
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
    bus: &mut MachineBus,
    paging: Long64ProbeConfig,
    instruction_rip: u64,
    callback: impl FnOnce(&mut dyn GuestMemory) -> R,
) -> PagedMovMemoryScope<R> {
    with_paged_mov_read_memory(bus, paging, instruction_rip, true, callback)
}

fn with_paged_mov_read_memory<R>(
    bus: &mut MachineBus,
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
/// (`PhysicalMemory::write_bytes` validates the whole range first). The
/// callback cannot retain the adapter beyond this call.
pub fn with_paged_mov_store_memory<R>(
    bus: &mut MachineBus,
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
    bus: &'a mut MachineBus,
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

/// Why a `#PF` of the guest was not delivered (S4-38L5c).
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum PagedCpl0ExceptionFailure {
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
    bus: &mut MachineBus,
    paging: &Long64ProbeConfig,
    context: AccessContext,
    cache: Option<&FetchTranslationCache>,
) -> Option<u32> {
    let (memory, _) = bus.split_for_pio_step();
    let mut byte_at = |offset: u64| -> Option<u8> {
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
        memory.read_u8(physical).ok()
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
    bus: &mut MachineBus,
    paging: &Long64ProbeConfig,
    context: AccessContext,
) -> (Option<u64>, Vec<u8>) {
    let (memory, _) = bus.split_for_pio_step();
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
        let Ok(byte) = memory.read_u8(physical) else {
            break;
        };
        bytes.push(byte);
    }
    (first, bytes)
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
    bus: &mut MachineBus,
    paging: &Long64ProbeConfig,
    context: AccessContext,
) -> InstructionPathBefore {
    let (_, bytes) = observed_fetch_view(rip, bus, paging, context);
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
    bus: &mut MachineBus,
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
    bus: &mut MachineBus,
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

fn capture_elf_stack_copy_snapshot(
    state: &LongModeState,
    bus: &mut MachineBus,
    paging: &Long64ProbeConfig,
    context: AccessContext,
    cr3: u64,
) -> ElfStackCopySnapshot {
    use od_isa::Register::{R13, R15, Rdi, Rdx, Rsi};

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
            let (ram, _) = bus.split_for_pio_step();
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

/// Whether the instruction at RIP is exactly `IRETQ` (`48 CF`). Same read-only walk as
/// [`cpuid_leaf_at_rip`] (no accessed/dirty bit, RAM only); an untranslatable byte is `false`.
/// Used for the RF rule of S4-38L5c: `IRETQ` loads RF from its own frame, so the board must not
/// clear it afterwards.
fn iretq_at_rip(
    state: &LongModeState,
    bus: &mut MachineBus,
    paging: &Long64ProbeConfig,
    context: AccessContext,
    cache: Option<&FetchTranslationCache>,
) -> bool {
    let (memory, _) = bus.split_for_pio_step();
    let mut byte_at = |offset: u64| -> Option<u8> {
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
        memory.read_u8(physical).ok()
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
    bus: &mut MachineBus,
    paging: &Long64ProbeConfig,
    context: AccessContext,
    cache: Option<&FetchTranslationCache>,
) -> bool {
    let (memory, _) = bus.split_for_pio_step();
    let mut byte_at = |offset: u64| -> Option<u8> {
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
        memory.read_u8(physical).ok()
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
    bus: &mut MachineBus,
    paging: &Long64ProbeConfig,
    context: AccessContext,
    cache: Option<&FetchTranslationCache>,
) -> bool {
    let (memory, _) = bus.split_for_pio_step();
    let mut byte_at = |offset: u64| -> Option<u8> {
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
        memory.read_u8(physical).ok()
    };
    byte_at(0) == Some(0x48) && byte_at(1) == Some(0x0F) && byte_at(2) == Some(0x07)
}

/// S4-39f: what the board loop saw of a system call: the `SYSCALL` that entered the kernel (RAX
/// is the number, RCX the return address) and the `SYSRETQ` that returned (RAX the result).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SyscallEvent {
    Enter { nr: u64, rip: u64 },
    Exit { rax: u64 },
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
    /// `#PF` and `#GP(0)` exceptions the loop delivered through the guest IDT. A delivery is not
    /// a retired instruction: `instructions_executed` does not count it.
    pub(crate) delivered_exceptions: u64,
    /// Successfully delivered guest `#PF` exceptions in this call.
    pub(crate) delivered_page_faults: u64,
    /// Successfully delivered guest `#GP(0)` exceptions in this call.
    pub(crate) delivered_general_protection_faults: u64,
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
) -> PagedCpl0SetReport {
    let capture_instruction_path = execution_observations
        .as_deref()
        .is_some_and(LinuxExecutionObservationCollector::wants_instruction_path_trace);
    let capture_elf_stack_copy = execution_observations
        .as_deref()
        .is_some_and(LinuxExecutionObservationCollector::needs_elf_stack_copy_path);
    if capture_instruction_path && capture_elf_stack_copy {
        run_cpl0_set_paged_2m_ram_with_cache_impl::<true, true>(
            state,
            bus,
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
        )
    } else if capture_instruction_path {
        run_cpl0_set_paged_2m_ram_with_cache_impl::<true, false>(
            state,
            bus,
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
        )
    } else if capture_elf_stack_copy {
        run_cpl0_set_paged_2m_ram_with_cache_impl::<false, true>(
            state,
            bus,
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
        )
    } else {
        run_cpl0_set_paged_2m_ram_with_cache_impl::<false, false>(
            state,
            bus,
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
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn run_cpl0_set_paged_2m_ram_with_cache_impl<
    const TRACE_ENABLED: bool,
    const ELF_COPY_ENABLED: bool,
>(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    vcpu: usize,
    cr3: u64,
    efer: u64,
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
) -> PagedCpl0SetReport {
    let context = AccessContext {
        origin: od_isa::memory::AccessOrigin::Explicit,
        ..context
    };
    // S4-14i: any cache first learns of a changed translation context and of writes made outside
    // the loop since its last call (one-shot caches have no seal yet, so nothing changes for them).
    if let Some(cache) = cache {
        cache.revalidate(continuation_key(state, cr3, efer, context), &|page| {
            bus.code_page_generation(GuestPhysicalAddress::new(page << 12))
                .ok()
        });
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
                delivered_exceptions: 0,
                delivered_page_faults: 0,
                delivered_general_protection_faults: 0,
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
    let mut deliveries_without_retirement = 0u64;
    let mut last_instruction_rip = None;
    // S4-38L8b: a retired guest write to a PIT port ends the quantum (only when the source
    // depends on the PIT), so that the board asks the source for its deadline again.
    let pit_write_ends_quantum = irq
        .as_ref()
        .is_some_and(|hook| hook.source.pit_write_ends_quantum());
    let mut pit_write_pending = false;
    let (exit, page_fault, memory_error) = loop {
        if stop() {
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
            match deliver_pending_irq(hook, state, bus, &paging, context, &mut watch, cache) {
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
        let cpuid_leaf = if cpuid_boundary || checkpoints.is_some() {
            cpuid_leaf_at_rip(state, bus, &paging, context, cache)
        } else {
            None
        };
        if let Some(recorder) = checkpoints.as_deref_mut() {
            let view = BoundaryView {
                rip: state.rip,
                cpl: state.cs_selector.map_or(0, |selector| (selector & 3) as u8),
                cr3: reported_cr3,
                call_retired: instructions_executed,
                idtr: state.idtr.map(|idtr| (idtr.base, idtr.limit)),
                cpuid_leaf,
            };
            recorder.boundary(&view, &mut || {
                observed_fetch_view(state.rip, bus, &paging, context)
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
        let mut trace_before = None;
        if let Some(observations) = execution_observations.as_deref_mut() {
            last_instruction_rip = Some(instruction_rip);
            if ELF_COPY_ENABLED {
                observations.observe_elf_stack_copy_boundary(instruction_rip);
                if observations.elf_copy_invocation_active()
                    && observations.elf_copy_return_rip() == Some(instruction_rip)
                {
                    observations.finish_elf_copy_invocation(instruction_rip, reported_cr3);
                }
                if instruction_rip == LINUX_ELF_STACK_COPY_RIP
                    && observations.wants_elf_stack_copy()
                {
                    let snapshot =
                        capture_elf_stack_copy_snapshot(state, bus, &paging, context, reported_cr3);
                    observations.record_elf_stack_copy(snapshot);
                }
            }
            if TRACE_ENABLED {
                if observations.needs_instruction_path_header() {
                    let (frame_chain, frame_chain_end) =
                        capture_instruction_path_frame_chain(state, bus, &paging, context);
                    observations.begin_instruction_path_header(state, frame_chain, frame_chain_end);
                }
                trace_before = Some(instruction_path_before(
                    instruction_rip,
                    state,
                    bus,
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
            resume_flag_before && iretq_at_rip(state, bus, &paging, context, cache);
        // The selected diagnostic uses the same opcode check but bypasses the fetch cache so it
        // does not perturb cache hit/miss counters while waiting for the selected return.
        let selected_return_is_iretq =
            pending_post_iret.is_some() && iretq_at_rip(state, bus, &paging, context, None);
        let iretq_detected = resume_flag_is_iretq || selected_return_is_iretq;
        let clear_resume_flag = resume_flag_before && !resume_flag_is_iretq;
        // S4-14h: under `GuestOnly` the guest's own invalidations flush the cache. `INVLPG` is
        // peeked before the step (read-only, through the cache like the fetch itself); CR0/CR4
        // are compared around it.
        let invlpg_pending = guest_only && invlpg_at_rip(state, bus, &paging, context, cache);
        // S4-13c: the shadows of a `WriteWatch` cache learn of a retired `INVLPG` too. The peek
        // bypasses the cache (read-only walk), so neither the counters nor the order of the cache
        // move; a `GuestOnly` cache tells them itself.
        let sweep_invlpg_pending = !guest_only
            && cache.is_some_and(FetchTranslationCache::sweep_active)
            && invlpg_at_rip(state, bus, &paging, context, None);
        let control_before = (state.cr0, state.cr4);
        let (ram, io) = bus.split_for_set_step_of_vcpu(vcpu);
        let mut port = PitWriteWatch {
            inner: io.port_io(),
            pit_written: false,
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
        if pit_write_ends_quantum && port.pit_written {
            pit_write_pending = true;
        }
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
                deliveries_without_retirement = 0;
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
                let failure = if deliveries_without_retirement >= MAX_DELIVERIES_WITHOUT_RETIREMENT
                {
                    Some(PagedCpl0ExceptionFailure::RepeatLimit)
                } else {
                    match &original_record {
                        Some(record) if record.fault() == &fault => {
                            match deliver_page_fault_cpl0_set(state, &mut memory, record, context) {
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
                        deliveries_without_retirement += 1;
                        if let Some(recorder) = checkpoints.as_deref_mut() {
                            recorder.note_page_fault_delivered();
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
                let failure = if deliveries_without_retirement >= MAX_DELIVERIES_WITHOUT_RETIREMENT
                {
                    Some(PagedCpl0ExceptionFailure::RepeatLimit)
                } else {
                    match deliver_general_protection_cpl0_set(state, &mut memory, &fault, context) {
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
                        deliveries_without_retirement += 1;
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
        let (page_fault, memory_error) = (memory.page_fault, memory.memory_error);
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
                    .then_some(SyscallEvent::Enter {
                        nr: rax_before,
                        rip: state.gpr[od_isa::Register::Rcx as usize],
                    })
            } else if matches!(result.exit, Cpl0ProfileSetExit::UserModeEntered(_))
                && sysretq_at(instruction_rip, bus, &paging, context, cache)
            {
                Some(SyscallEvent::Exit {
                    rax: state.gpr[od_isa::Register::Rax as usize],
                })
            } else {
                None
            };
            if let Some(event) = event {
                if syscall_events.len() == SYSCALL_EVENTS_PER_CALL {
                    syscall_events.pop_front();
                }
                syscall_events.push_back(event);
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
                                bus,
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
            efer,
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
                bus.code_page_generation(GuestPhysicalAddress::new(page << 12))
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
        delivered_exceptions,
        delivered_page_faults,
        delivered_general_protection_faults,
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
    ram: &'a mut crate::PhysicalMemory,
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
    }
}

/// The port view of one set call that notes a retired write to the PIT ports `0x40..=0x43`
/// (S4-38L8b). Reads and every other port pass through untouched.
struct PitWriteWatch<P> {
    inner: P,
    pit_written: bool,
}

impl<P: od_isa::PortIo> od_isa::PortIo for PitWriteWatch<P> {
    type Error = P::Error;

    fn read_u8(&mut self, port: u16) -> Result<u8, Self::Error> {
        self.inner.read_u8(port)
    }

    fn write_u8(&mut self, port: u16, value: u8) -> Result<(), Self::Error> {
        let result = self.inner.write_u8(port, value);
        if result.is_ok() && (0x40..=0x43).contains(&port) {
            self.pit_written = true;
        }
        result
    }

    fn read_u16(&mut self, port: u16) -> Result<u16, Self::Error> {
        self.inner.read_u16(port)
    }

    fn write_u16(&mut self, port: u16, value: u16) -> Result<(), Self::Error> {
        let result = self.inner.write_u16(port, value);
        // A word write covers `port` and `port + 1` (S4-38L9r).
        if result.is_ok() && (0x3f..=0x43).contains(&port) {
            self.pit_written = true;
        }
        result
    }

    fn read_u32(&mut self, port: u16) -> Result<u32, Self::Error> {
        self.inner.read_u32(port)
    }

    fn write_u32(&mut self, port: u16, value: u32) -> Result<(), Self::Error> {
        let result = self.inner.write_u32(port, value);
        // A dword write covers `port..=port + 3` (S4-38L9u).
        if result.is_ok() && (0x3d..=0x43).contains(&port) {
            self.pit_written = true;
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
enum IrqBoundaryOutcome {
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

fn deliver_pending_irq(
    hook: &mut IrqBoundaryHook<'_>,
    state: &mut LongModeState,
    bus: &mut MachineBus,
    paging: &Long64ProbeConfig,
    context: AccessContext,
    watch: &mut dyn FnMut(GuestPhysicalRange),
    cache: Option<&FetchTranslationCache>,
) -> IrqBoundaryOutcome {
    if hook.skip_first_boundary {
        hook.skip_first_boundary = false;
        return IrqBoundaryOutcome::None;
    }
    let Some(vector) = hook.source.pending_vector() else {
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
    let (ram, _port) = bus.split_for_pio_step();
    let mut memory = set_loop_adapter(ram, None, *paging, state.rip, watch, cache, None);
    let delivery = deliver_external_interrupt_cpl0_set(state, &mut memory, vector, context);
    let failure = match delivery {
        Cpl0ExternalInterruptDelivery::Delivered { .. } => {
            hook.source.acknowledge(vector);
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
        memory.page_fault,
        memory.memory_error,
    ))
}

/// Deliver a non-maskable interrupt through vector 2 at an instruction boundary outside a run of
/// the set loop (S6-12b), through the same RAM-only paged adapter as [`deliver_pending_irq`]: the
/// frame goes onto the guest stack, `state` enters the handler of the IDT gate of vector 2 and
/// `state.nmi_blocked` is set until the handler's `IRETQ`. Nothing is retired. The result is the
/// one of [`deliver_nmi_cpl0_set`]: a refusal (blocked, STI shadow, activity) is the caller's to
/// hold the NMI pending for, and every refusal and abort leaves `state` unchanged.
pub(crate) fn deliver_nmi_at_boundary(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    paging: &Long64ProbeConfig,
    context: AccessContext,
) -> Cpl0ExternalInterruptDelivery {
    let (ram, _port) = bus.split_for_pio_step();
    let mut committed_write = |_: GuestPhysicalRange| {};
    let mut memory = set_loop_adapter(
        ram,
        None,
        *paging,
        state.rip,
        &mut committed_write,
        None,
        None,
    );
    deliver_nmi_cpl0_set(state, &mut memory, context)
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
    efer: u64,
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
            match Long64ProbeConfig::from_cpu_state(state, new_cr3, efer) {
                Ok(candidate) => {
                    *paging = candidate;
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
    Ram(&'a mut crate::PhysicalMemory),
    /// `Ram` plus the MMIO dispatch of the set loop (S4-38L9s): an operand in an MMIO region
    /// goes to the board's device, as on the `Machine` form; the rest is the large-RAM walk.
    /// The last member is the index of the vCPU whose set loop it is (S6-16a).
    RamMmio(
        &'a mut crate::PhysicalMemory,
        &'a dyn crate::bus::SetStepMmio,
        usize,
    ),
}

impl<'a> From<&'a mut MachineBus> for AdapterBus<'a> {
    fn from(bus: &'a mut MachineBus) -> Self {
        Self::Machine(bus, crate::vcpu::BOOT_VCPU_INDEX)
    }
}

impl AdapterBus<'_> {
    /// Index of the vCPU that owns this adapter: its MMIO accesses reach the per-processor
    /// devices (the Local APIC window, S6-11) as that vCPU's. Only the `Machine` form carries
    /// one, and so does the `RamMmio` form of the set loop (S6-16a); the `Ram` form serves the
    /// boot processor's set loop.
    fn vcpu(&self) -> usize {
        match self {
            Self::Machine(_, vcpu) | Self::RamMmio(_, _, vcpu) => *vcpu,
            Self::Ram(_) => crate::vcpu::BOOT_VCPU_INDEX,
        }
    }

    /// The whole bus. The `Ram` form is built only for the large-RAM walk, which never asks
    /// for it; if it ever does, that is a typed backend error and not a panic.
    fn machine(&mut self) -> Result<&mut MachineBus, MemoryError> {
        match self {
            Self::Machine(bus, _) => Ok(bus),
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

    /// The physical memory of the `Ram` forms.
    fn ram_mut(&mut self) -> Option<&mut crate::PhysicalMemory> {
        match self {
            Self::Ram(memory) | Self::RamMmio(memory, _, _) => Some(memory),
            Self::Machine(..) => None,
        }
    }

    fn ram_ref(&self) -> Option<&crate::PhysicalMemory> {
        match self {
            Self::Ram(memory) | Self::RamMmio(memory, _, _) => Some(memory),
            Self::Machine(..) => None,
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
        let memory = self.ram_mut().ok_or(())?;
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
                .read_bytes(physical, &mut output[offset..offset + fragment_len])
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

impl PagedMovMemoryAdapter<'_> {
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
                RamAccessOperation::Write { .. } => walks.note_write(),
            }
        }
        let vcpu = self.bus.vcpu();
        let report = if self.capture_effects {
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
                    .machine()?
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
            self.bus.machine()?.access_tracked_ram(
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
        if let Some(memory) = self.bus.ram_mut()
            && let Ok((physical, tables)) = translate_long64_2m_ram_read_only_with_tables(
                memory,
                &self.paging,
                self.instruction_rip,
                GuestLinearAddress::new(address),
                context,
                od_isa::memory::AccessKind::Fetch,
            )
        {
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
            let leaf_frame = (cache.tag() == FetchCacheTag::Leaf2M
                && tables[2].is_some()
                && tables[3].is_none())
            .then(|| physical.get().wrapping_sub(address & 0x1f_ffff))
            .and_then(|base| memory.ram_leaf_2m(GuestPhysicalAddress::new(base)));
            if let Some(frame) = leaf_frame
                .or_else(|| memory.ram_frame(GuestPhysicalAddress::new(physical.get() & !0xfff)))
                && tables.iter().flatten().all(|table| is_ram(*table & !0xfff))
            {
                cache.insert(address, &self.paging, context, frame, tables);
            }
        }
        Ok(byte)
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
                .machine()?
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
        let result = self.bus.machine()?.read_tracked_bounded_ram_read(
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
                AdapterBus::Machine(..) => return Err(Self::unsupported("cross-page write")),
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

impl GuestMemory for PagedMovMemoryAdapter<'_> {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
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

    fn read(
        &mut self,
        address: u64,
        context: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        let width = match destination.len() {
            1 if self.cpl0_set_byte_read => crate::AccessWidth::Byte,
            2 if self.word_read_profile || self.cpl0_set_word_read => crate::AccessWidth::Word,
            4 if self.dword_read_profile || self.cpl0_set_dword_read => crate::AccessWidth::Dword,
            8 if !self.dword_read_profile && !self.word_read_profile => crate::AccessWidth::Qword,
            _ => return Err(Self::unsupported("read width")),
        };
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
        if self.cpl0_set_cross_page_store
            && self.cpl0_set_narrow_write
            && self.large_ram_profile
            && (address & 0xfff) + expected as u64 > 4096
        {
            return self.write_crossing_page(address, context, source);
        }
        match self.access(address, context, RamAccessOperation::Write { width, value })? {
            TrackedAccessOutcome::Written => Ok(()),
            _ => Err(Self::unsupported("non-write result")),
        }
    }

    // S6-20: no atomic override. `atomic_rmw_integer` and the three atomic operations built on
    // it use the trait's serial default (read, then write, through this adapter's own
    // width-gated `read`/`write`). In the interleaving mode the vCPU never switches inside an
    // instruction, so that sequence is indivisible; the single definition of LOCK atomicity is
    // `GuestMemory::atomic_rmw_integer` (CPU_CONTRACT.md, S6-20), replaced by S6-31 for a
    // concurrent backend without touching the instructions.

    fn mfence(&mut self, _context: AccessContext) -> Result<(), MemoryError> {
        Err(Self::unsupported("MFENCE"))
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
    use od_isa::{CpuActivity, InvlpgExit, StiShadow, TranslationInvalidation};

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
                .write_u64(GuestPhysicalAddress::new(address), value, |_| {})
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
                .write_u64(GuestPhysicalAddress::new(address), value, |_| {})
                .unwrap();
        }
        physical
            .write_bytes(GuestPhysicalAddress::new(0x7ffc), &[0x11; 4], |_| {})
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
                .read_u64(GuestPhysicalAddress::new(0x5000))
                .unwrap()
                & 0x60,
            0x20,
            "the successful preflight sets A but not D"
        );
        assert_eq!(
            physical
                .read_u64(GuestPhysicalAddress::new(0x5008))
                .unwrap(),
            0
        );
        let mut operand = [0; 4];
        physical
            .read_bytes(GuestPhysicalAddress::new(0x7ffc), &mut operand)
            .unwrap();
        assert_eq!(
            operand, [0x11; 4],
            "failed preflight stores no operand bytes"
        );
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
        reported_cr3: u64,
        changes: Vec<TranslationContextChange>,
    }

    impl SetLoop {
        fn new() -> Self {
            Self {
                paging: Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap(),
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
                EFER,
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
}

#[cfg(test)]
mod pit_word_watch_tests {
    use super::PitWriteWatch;
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
            let mut watch = PitWriteWatch {
                inner: Recorder::default(),
                pit_written: false,
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
    use super::PitWriteWatch;
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
            let mut watch = PitWriteWatch {
                inner: Recorder::default(),
                pit_written: false,
            };
            assert_eq!(watch.read_u32(port), Ok(0x1234_5678));
            assert_eq!(watch.write_u32(port, 0xdead_beef), Ok(()));
            assert_eq!(watch.inner.dwords, [(port, 0xdead_beef)]);
            assert_eq!(watch.pit_written, noted, "{port:#x}");
        }
    }
}
