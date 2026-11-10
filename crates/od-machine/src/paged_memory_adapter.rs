//! Opt-in one-instruction Long64 bridge through the uncached RAM walker.

use crate::{
    GuestLinearAddress, GuestPhysicalRange, Long64ProbeConfig, MachineBus, RamAccessOperation,
    TrackedAccessError, TrackedAccessOutcome, TrackedAccessRequest,
};
use od_isa::LongModeState;
use od_isa::execute::{
    Cpl0ProfileSet, Cpl0ProfileSetExit, Cpl0ProfileSetInputError, InvlpgExit, InvlpgLong64V1,
    InvlpgResult, RunConfig, RunExit, RunInputError, RunResult, TranslationContextChange,
    run_with_cpl0_profile_set_and_pio, step_invlpg as cpu_step_invlpg,
    step_paged_mov as cpu_step_paged_mov,
};
use od_isa::execute::{
    PagedMovCrossPageDwordReadLong64V1, PagedMovCrossPageWordFetchLong64V1,
    PagedMovCrossPageWordReadLong64V1, step_paged_mov_dword_read as cpu_step_paged_mov_dword_read,
    step_paged_mov_word_read as cpu_step_paged_mov_word_read,
    step_paged_mov_word_read_cross_page_fetch as cpu_step_paged_mov_word_read_cross_page_fetch,
};
use od_isa::memory::{AccessContext, GuestMemory, MemoryError, PageFaultRecord};

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
        cpl0_set_narrow_write: false,
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
        cpl0_set_narrow_write: false,
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
    let mut ignore_write = |_| {};
    let mut memory = PagedMovMemoryAdapter {
        bus: bus.into(),
        paging,
        instruction_rip,
        committed_write: &mut ignore_write,
        page_fault: None,
        memory_error: None,
        access_error: None,
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
        cpl0_set_dword_read: false,
        cpl0_set_narrow_write: false,
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
        cpl0_set_narrow_write: false,
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
        cpl0_set_narrow_write: false,
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
    /// The set reported an exit variant this board does not know (the exit enum
    /// is `#[non_exhaustive]`).
    UnknownSetExit,
}

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
    mut rdtsc: Option<&mut dyn FnMut() -> u64>,
    mut stop: impl FnMut() -> bool,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> PagedCpl0SetReport {
    let context = AccessContext {
        origin: od_isa::memory::AccessOrigin::Explicit,
        ..context
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
            };
        }
    };
    let mut instructions_executed = 0u64;
    let mut current_rflags_defined = rflags_defined;
    let mut changes = Vec::new();
    // CR3 of the last retired MOV CR3 (the initial root before any), independent of
    // whether the walker could be rebuilt for it.
    let mut reported_cr3 = cr3;
    let (exit, page_fault, memory_error) = loop {
        if stop() {
            break (PagedCpl0SetExit::Run(RunExit::StopRequested), None, None);
        }
        if instructions_executed >= instruction_budget {
            break (
                PagedCpl0SetExit::Run(RunExit::InstructionBudgetExhausted),
                None,
                None,
            );
        }
        let instruction_rip = state.rip;
        let (ram, mut port) = bus.split_for_pio_step();
        let mut memory = PagedMovMemoryAdapter {
            bus: AdapterBus::Ram(ram),
            paging,
            instruction_rip,
            committed_write: &mut committed_write,
            page_fault: None,
            memory_error: None,
            access_error: None,
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
            cpl0_set_narrow_write: true,
        };
        let result = run_with_cpl0_profile_set_and_pio(
            state,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context,
            },
            current_rflags_defined,
            set,
            rdtsc
                .as_mut()
                .map(|source| &mut **source as &mut dyn FnMut() -> u64),
            &mut port,
            || false,
        );
        let (page_fault, memory_error) = (memory.page_fault, memory.memory_error);
        let result = match result {
            Ok(result) => result,
            Err(error) => break (PagedCpl0SetExit::Rejected(error), page_fault, memory_error),
        };
        instructions_executed += result.instructions_executed;
        current_rflags_defined = result.rflags_defined;
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
    PagedCpl0SetReport {
        exit,
        instructions_executed,
        rflags_defined: current_rflags_defined,
        page_fault,
        memory_error,
        translation_context_changes: changes,
        cr3: reported_cr3,
    }
}

/// What the set loop does with the exit of one set call: `None` continues with the next
/// instruction, `Some(exit)` ends the run.
///
/// A retired `MOV CR3` is recorded first, before any check: `reported_cr3` becomes the new
/// CR3 and the change is appended to `changes`, so that an integration error on that very
/// instruction (`Cr3EffectMismatch`, `Cr3SnapshotRejected`) still leaves the report with the
/// CR3 the guest actually has (the instruction retired and was counted; only the walker
/// is stale). The walker (`paging`) is replaced only when the snapshot for the new CR3 is
/// accepted.
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
        cpl0_set_narrow_write: false,
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
        cpl0_set_narrow_write: false,
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
        cpl0_set_narrow_write: false,
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
        cpl0_set_narrow_write: false,
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
    Machine(&'a mut MachineBus),
    Ram(&'a mut crate::PhysicalMemory),
}

impl<'a> From<&'a mut MachineBus> for AdapterBus<'a> {
    fn from(bus: &'a mut MachineBus) -> Self {
        Self::Machine(bus)
    }
}

impl AdapterBus<'_> {
    /// The whole bus. The `Ram` form is built only for the large-RAM walk, which never asks
    /// for it; if it ever does, that is a typed backend error and not a panic.
    fn machine(&mut self) -> Result<&mut MachineBus, MemoryError> {
        match self {
            Self::Machine(bus) => Ok(bus),
            Self::Ram(_) => Err(MemoryError::Backend(
                "the RAM-only adapter bus serves the large-RAM walk only".to_string(),
            )),
        }
    }

    fn access_2m_ram(
        &mut self,
        paging: &Long64ProbeConfig,
        request: TrackedAccessRequest,
        committed_write: impl FnMut(GuestPhysicalRange),
    ) -> crate::TrackedAccessReport {
        match self {
            Self::Machine(bus) => bus.access_tracked_2m_ram(paging, request, committed_write),
            Self::Ram(memory) => {
                crate::page_walk::access_long64_2m_ram(memory, paging, request, committed_write)
            }
        }
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
    /// S4-38L3: the CPL0 profile set also stores bytes, words and dwords
    /// (`MOV m8, r8`, `MOV m16, r16`, `MOV m32, r32`), next to its qword stores. Each store
    /// writes exactly its own width (no read-modify-write of the surrounding qword). The
    /// shared profiles keep their fixed store width: this flag is set by the set's adapter alone.
    cpl0_set_narrow_write: bool,
}

impl PagedMovMemoryAdapter<'_> {
    fn access(
        &mut self,
        address: u64,
        context: AccessContext,
        operation: RamAccessOperation,
    ) -> Result<TrackedAccessOutcome, MemoryError> {
        let context = AccessContext {
            origin: od_isa::memory::AccessOrigin::Explicit,
            ..context
        };
        let request = TrackedAccessRequest {
            instruction_rip: self.instruction_rip,
            address: GuestLinearAddress::new(address),
            context,
            operation,
        };
        let report = if self.capture_effects {
            let mut writes = Vec::new();
            let report = if self.large_ram_profile {
                self.bus
                    .access_2m_ram(&self.paging, request, |range| writes.push(range))
            } else if self.ram_only {
                self.bus
                    .machine()?
                    .access_tracked_ram(&self.paging, request, |range| writes.push(range))
            } else {
                self.bus
                    .machine()?
                    .access_tracked(&self.paging, request, |range| writes.push(range))
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
            self.bus
                .access_2m_ram(&self.paging, request, &mut *self.committed_write)
        } else if self.ram_only {
            self.bus.machine()?.access_tracked_ram(
                &self.paging,
                request,
                &mut *self.committed_write,
            )
        } else {
            self.bus
                .machine()?
                .access_tracked(&self.paging, request, &mut *self.committed_write)
        };
        match report.outcome {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                self.access_error = Some(error.clone());
                Err(self.map_access_error(error))
            }
        }
    }

    fn access_cross_page_read(
        &mut self,
        address: u64,
        context: AccessContext,
        width: crate::AccessWidth,
    ) -> Result<u64, MemoryError> {
        let context = AccessContext {
            origin: od_isa::memory::AccessOrigin::Explicit,
            ..context
        };
        let result = self.bus.machine()?.read_tracked_bounded_ram_read(
            &self.paging,
            TrackedAccessRequest {
                instruction_rip: self.instruction_rip,
                address: GuestLinearAddress::new(address),
                context,
                operation: RamAccessOperation::Read { width },
            },
            &mut *self.committed_write,
        );
        result.map_err(|error| self.map_access_error(error))
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
        if self.large_ram_profile && (address & !0xfff) != (self.instruction_rip & !0xfff) {
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
            2 if self.word_read_profile => crate::AccessWidth::Word,
            4 if self.dword_read_profile || self.cpl0_set_dword_read => crate::AccessWidth::Dword,
            8 if !self.dword_read_profile && !self.word_read_profile => crate::AccessWidth::Qword,
            _ => return Err(Self::unsupported("read width")),
        };
        if self.reject_crossing_qword
            && width == crate::AccessWidth::Qword
            && (address & 0xfff) + 8 > 4096
        {
            return Err(Self::unsupported("cross-page qword read"));
        }
        let value = if !self.large_ram_profile
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
        match self.access(address, context, RamAccessOperation::Write { width, value })? {
            TrackedAccessOutcome::Written => Ok(()),
            _ => Err(Self::unsupported("non-write result")),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AccessWidth, GuestPhysicalAddress, MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec,
        RamLimits,
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

    fn adapter<'a>(
        bus: &'a mut MachineBus,
        callbacks: &'a mut dyn FnMut(GuestPhysicalRange),
    ) -> PagedMovMemoryAdapter<'a> {
        PagedMovMemoryAdapter {
            bus: bus.into(),
            paging: Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap(),
            instruction_rip: 0x0040_0000,
            committed_write: callbacks,
            page_fault: None,
            memory_error: None,
            access_error: None,
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
            cpl0_set_narrow_write: false,
        }
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
        assert_eq!(run.paging.cr3(), 0x2000);
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
        assert_eq!(run.paging.cr3(), ROOT, "the walker was not rebuilt");
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
        assert_eq!(run.paging.cr3(), ROOT);
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
        assert_eq!(run.paging.cr3(), ROOT, "the walker was not rebuilt");
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
        assert_eq!(run.paging.cr3(), ROOT);
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
}
