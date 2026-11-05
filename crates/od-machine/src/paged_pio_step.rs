//! One-instruction byte-PIO bridge with instruction fetch through guest paging.

use crate::{
    GuestLinearAddress, GuestPhysicalRange, Long64ProbeConfig, MachineBus, RamAccessOperation,
    TrackedAccessError, TrackedAccessOutcome, TrackedAccessRequest,
};
use od_isa::LongModeState;
use od_isa::execute::{PagedCpl0PioU8V1, PioU8Result, step_paged_pio_u8 as cpu_step_paged_pio_u8};
use od_isa::memory::{AccessContext, AccessOrigin, GuestMemory, MemoryError, PageFaultRecord};

#[derive(Debug, Eq, PartialEq)]
pub struct PagedPioU8StepReport {
    pub cpu: PioU8Result<crate::BusAccessError>,
    /// Preserves the walker’s precise saved RIP, linear address, and PFEC.
    pub page_fault: Option<PageFaultRecord>,
    /// Preserves non-architectural walker/backend classifications.
    pub memory_error: Option<TrackedAccessError>,
}

/// Execute one bare byte PIO instruction after fetching it through the supplied
/// four-level 4 KiB page-table snapshot. CR3 and EFER are explicit because the
/// CPU state does not own them. The fetch origin is forced to `Explicit`.
pub fn step_paged_pio_u8(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    rflags_defined: u64,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<PagedPioU8StepReport, crate::ProbeConfigError> {
    let paging = Long64ProbeConfig::from_cpu_state(state, cr3, efer)?;
    let instruction_rip = state.rip;
    let (memory, mut pio) = bus.split_for_pio_step();
    let mut memory = PagedPioFetchAdapter {
        memory,
        paging,
        instruction_rip,
        committed_write: &mut committed_write,
        page_fault: None,
        memory_error: None,
    };
    let cpu = cpu_step_paged_pio_u8(
        PagedCpl0PioU8V1,
        state,
        &mut memory,
        &mut pio,
        AccessContext {
            origin: AccessOrigin::Explicit,
            ..context
        },
        rflags_defined,
    );
    Ok(PagedPioU8StepReport {
        cpu,
        page_fault: memory.page_fault,
        memory_error: memory.memory_error,
    })
}

struct PagedPioFetchAdapter<'a> {
    memory: &'a mut crate::PhysicalMemory,
    paging: Long64ProbeConfig,
    instruction_rip: u64,
    committed_write: &'a mut dyn FnMut(GuestPhysicalRange),
    page_fault: Option<PageFaultRecord>,
    memory_error: Option<TrackedAccessError>,
}

impl PagedPioFetchAdapter<'_> {
    fn fetch_byte(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        let report = crate::access_long64_4k_ram(
            self.memory,
            &self.paging,
            TrackedAccessRequest {
                instruction_rip: self.instruction_rip,
                address: GuestLinearAddress::new(address),
                context,
                operation: RamAccessOperation::Fetch,
            },
            &mut *self.committed_write,
        );
        match report.outcome {
            Ok(TrackedAccessOutcome::Fetched(value)) => Ok(value),
            Ok(_) => Err(MemoryError::Backend(
                "paged PIO fetch returned a non-fetch result".into(),
            )),
            Err(TrackedAccessError::PageFault { record, .. }) => {
                self.page_fault = Some(record.clone());
                Err(MemoryError::Architectural(record.fault().clone()))
            }
            Err(error) => {
                let message = format!("tracked Long64 PIO fetch failed: {error:?}");
                self.memory_error = Some(error);
                Err(MemoryError::Backend(message))
            }
        }
    }
}

impl GuestMemory for PagedPioFetchAdapter<'_> {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        self.fetch_byte(
            address,
            AccessContext {
                origin: AccessOrigin::Explicit,
                ..context
            },
        )
    }

    fn read(
        &mut self,
        _address: u64,
        _context: AccessContext,
        _destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        Err(MemoryError::Backend(
            "paged PIO profile does not support operand reads".into(),
        ))
    }

    fn write(
        &mut self,
        _address: u64,
        _context: AccessContext,
        _source: &[u8],
    ) -> Result<(), MemoryError> {
        Err(MemoryError::Backend(
            "paged PIO profile does not support operand writes".into(),
        ))
    }

    fn atomic_exchange_integer(
        &mut self,
        _address: u64,
        _context: AccessContext,
        _size: od_isa::OperandSize,
        _replacement: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend(
            "paged PIO profile does not support atomic exchange".into(),
        ))
    }

    fn atomic_fetch_add_integer(
        &mut self,
        _address: u64,
        _context: AccessContext,
        _size: od_isa::OperandSize,
        _addend: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend(
            "paged PIO profile does not support atomic fetch-add".into(),
        ))
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        _address: u64,
        _context: AccessContext,
        _size: od_isa::OperandSize,
        _expected: u64,
        _replacement: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend(
            "paged PIO profile does not support atomic compare-exchange".into(),
        ))
    }

    fn mfence(&mut self, _context: AccessContext) -> Result<(), MemoryError> {
        Err(MemoryError::Backend(
            "paged PIO profile does not support MFENCE".into(),
        ))
    }
}
