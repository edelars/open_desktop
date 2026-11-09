//! Opt-in one-instruction Long64 bridge through the uncached RAM walker.

use crate::{
    GuestLinearAddress, GuestPhysicalRange, Long64ProbeConfig, MachineBus, RamAccessOperation,
    TrackedAccessError, TrackedAccessOutcome, TrackedAccessRequest,
};
use od_isa::LongModeState;
use od_isa::execute::{
    InvlpgExit, InvlpgLong64V1, InvlpgResult, RunExit, RunInputError, RunResult,
    step_invlpg as cpu_step_invlpg, step_paged_mov as cpu_step_paged_mov,
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
        bus,
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
        bus,
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
        bus,
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
        bus,
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
    };
    let cpu = cpu_step_paged_mov(state, &mut memory, context, rflags_defined)
        .map_err(PagedMovStepError::CpuInput)?;
    Ok(PagedMovStepReport {
        cpu,
        page_fault: memory.page_fault,
        memory_error: memory.memory_error,
    })
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
        bus,
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
        bus,
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
        bus,
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
        bus,
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
    bus: &'a mut MachineBus,
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
                    .access_tracked_2m_ram(&self.paging, request, |range| writes.push(range))
            } else if self.ram_only {
                self.bus
                    .access_tracked_ram(&self.paging, request, |range| writes.push(range))
            } else {
                self.bus
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
                .access_tracked_2m_ram(&self.paging, request, &mut *self.committed_write)
        } else if self.ram_only {
            self.bus
                .access_tracked_ram(&self.paging, request, &mut *self.committed_write)
        } else {
            self.bus
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
        let result = self.bus.read_tracked_bounded_ram_read(
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
            4 if self.dword_read_profile => crate::AccessWidth::Dword,
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
        let (width, expected) = if self.dword_write_profile {
            (crate::AccessWidth::Dword, 4)
        } else {
            (crate::AccessWidth::Qword, 8)
        };
        if source.len() != expected {
            return Err(Self::unsupported(if self.dword_write_profile {
                "non-dword writes"
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
            bus,
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
}
