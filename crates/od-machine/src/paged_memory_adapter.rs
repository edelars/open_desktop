//! Opt-in one-instruction Long64 MOV bridge through the uncached RAM walker.

use crate::{
    GuestLinearAddress, GuestPhysicalRange, Long64ProbeConfig, MachineBus, RamAccessOperation,
    TrackedAccessError, TrackedAccessOutcome, TrackedAccessRequest,
};
use od_isa::LongModeState;
use od_isa::execute::{
    PagedMovCrossPageDwordReadLong64V1, PagedMovCrossPageWordReadLong64V1,
    step_paged_mov_dword_read as cpu_step_paged_mov_dword_read,
    step_paged_mov_word_read as cpu_step_paged_mov_word_read,
};
use od_isa::execute::{RunInputError, RunResult, step_paged_mov as cpu_step_paged_mov};
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
        dword_read_profile: false,
        word_read_profile: false,
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
        dword_read_profile: false,
        word_read_profile: true,
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
        dword_read_profile: true,
        word_read_profile: false,
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
    dword_read_profile: bool,
    word_read_profile: bool,
}

impl PagedMovMemoryAdapter<'_> {
    fn access(
        &mut self,
        address: u64,
        context: AccessContext,
        operation: RamAccessOperation,
    ) -> Result<TrackedAccessOutcome, MemoryError> {
        let report = self.bus.access_tracked(
            &self.paging,
            TrackedAccessRequest {
                instruction_rip: self.instruction_rip,
                address: GuestLinearAddress::new(address),
                context,
                operation,
            },
            &mut *self.committed_write,
        );
        match report.outcome {
            Ok(outcome) => Ok(outcome),
            Err(error) => Err(self.map_access_error(error)),
        }
    }

    fn access_cross_page_read(
        &mut self,
        address: u64,
        context: AccessContext,
        width: crate::AccessWidth,
    ) -> Result<u64, MemoryError> {
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
        let value = if self.dword_read_profile
            || self.word_read_profile
            || (address & 0xfff) + u64::from(width.bytes()) > 4096
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
        if source.len() != 8 {
            return Err(Self::unsupported("non-qword writes"));
        }
        let mut bytes = [0; 8];
        bytes.copy_from_slice(source);
        let value = u64::from_le_bytes(bytes);
        match self.access(
            address,
            context,
            RamAccessOperation::Write {
                width: crate::AccessWidth::Qword,
                value,
            },
        )? {
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
            dword_read_profile: false,
            word_read_profile: false,
        }
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
