//! Bounded mixed-instruction quantum through the tracked Long64 RAM walker.

use crate::{
    GuestLinearAddress, GuestPhysicalRange, Long64ProbeConfig, MachineBus, RamAccessOperation,
    TrackedAccessError, TrackedAccessOutcome, TrackedAccessRequest,
};
use od_isa::LongModeState;
use od_isa::execute::{
    PagedCpl0ProbeV1, PagedCpl0StackProbeV1, PagedProbeExit as CpuProbeExit,
    PagedProbeRejection as CpuProbeRejection, step_paged_probe, step_paged_stack_probe,
};
use od_isa::memory::{
    AccessContext, AccessOrigin, ArchitecturalFault, GuestMemory, MemoryError, PageFaultRecord,
};

#[derive(Debug, Eq, PartialEq)]
pub enum PagedProbeRejection {
    Configuration(crate::ProbeConfigError),
    Cpu(CpuProbeRejection),
}

#[derive(Debug, Eq, PartialEq)]
pub enum PagedProbeExit {
    BudgetExhausted,
    Stopped,
    Halted,
    Rejected(PagedProbeRejection),
    Unsupported { opcode: u8 },
    Fault(ArchitecturalFault),
    Backend(MemoryError),
    PortIo(crate::BusAccessError),
}

#[derive(Debug, Eq, PartialEq)]
pub struct PagedProbeQuantumReport {
    pub exit: PagedProbeExit,
    pub instructions_executed: u64,
    pub rflags_defined: u64,
    /// The exact last architectural page fault from the tracked walker.
    pub page_fault: Option<PageFaultRecord>,
    /// The exact last non-architectural RAM/backend classification.
    pub memory_error: Option<TrackedAccessError>,
}

/// Run a finite number of one-instruction CPU steps through one supplied
/// CR3/EFER context. At each boundary, STOP is polled before the budget and
/// both are checked before any next instruction fetch.
pub fn run_paged_probe(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    instruction_budget: u64,
    rflags_defined: u64,
    stop: impl FnMut() -> bool,
    committed_write: impl FnMut(GuestPhysicalRange),
) -> PagedProbeQuantumReport {
    run_paged_probe_inner(
        false,
        state,
        bus,
        cr3,
        efer,
        context,
        instruction_budget,
        rflags_defined,
        stop,
        committed_write,
    )
}

/// Run the opt-in stack-extended quantum. The machine reuses the same
/// page-aware loop and selects only the CPU profile marker that admits PUSH
/// RCX and POP RAX.
pub fn run_paged_stack_probe(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    instruction_budget: u64,
    rflags_defined: u64,
    stop: impl FnMut() -> bool,
    committed_write: impl FnMut(GuestPhysicalRange),
) -> PagedProbeQuantumReport {
    run_paged_probe_inner(
        true,
        state,
        bus,
        cr3,
        efer,
        context,
        instruction_budget,
        rflags_defined,
        stop,
        committed_write,
    )
}

fn run_paged_probe_inner(
    stack_profile: bool,
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    instruction_budget: u64,
    rflags_defined: u64,
    mut stop: impl FnMut() -> bool,
    mut committed_write: impl FnMut(GuestPhysicalRange),
) -> PagedProbeQuantumReport {
    let mut instructions_executed = 0;
    let mut current_rflags_defined = rflags_defined;
    loop {
        if stop() {
            return report(
                PagedProbeExit::Stopped,
                instructions_executed,
                current_rflags_defined,
                None,
                None,
            );
        }
        if instructions_executed >= instruction_budget {
            return report(
                PagedProbeExit::BudgetExhausted,
                instructions_executed,
                current_rflags_defined,
                None,
                None,
            );
        }

        let paging = match Long64ProbeConfig::from_cpu_state(state, cr3, efer) {
            Ok(paging) => paging,
            Err(error) => {
                return report(
                    PagedProbeExit::Rejected(PagedProbeRejection::Configuration(error)),
                    instructions_executed,
                    current_rflags_defined,
                    None,
                    None,
                );
            }
        };
        let instruction_rip = state.rip;
        let (memory, mut pio) = bus.split_for_pio_step();
        let mut memory = PagedProbeMemory {
            memory,
            paging,
            instruction_rip,
            committed_write: &mut committed_write,
            page_fault: None,
            memory_error: None,
        };
        // One selected CPU entry owns decode and instruction choice. The
        // machine never peeks at guest opcodes or retries another entry.
        let cpu = if stack_profile {
            step_paged_stack_probe(
                PagedCpl0StackProbeV1,
                state,
                &mut memory,
                &mut pio,
                AccessContext {
                    origin: AccessOrigin::Explicit,
                    ..context
                },
                current_rflags_defined,
            )
        } else {
            step_paged_probe(
                PagedCpl0ProbeV1,
                state,
                &mut memory,
                &mut pio,
                AccessContext {
                    origin: AccessOrigin::Explicit,
                    ..context
                },
                current_rflags_defined,
            )
        };
        current_rflags_defined = cpu.rflags_defined;
        let page_fault = memory.page_fault;
        let memory_error = memory.memory_error;
        instructions_executed += cpu.instructions_executed;

        match cpu.exit {
            CpuProbeExit::Retired => {}
            CpuProbeExit::Halted => {
                return report(
                    PagedProbeExit::Halted,
                    instructions_executed,
                    current_rflags_defined,
                    page_fault,
                    memory_error,
                );
            }
            CpuProbeExit::Unsupported { opcode } => {
                return report(
                    PagedProbeExit::Unsupported { opcode },
                    instructions_executed,
                    current_rflags_defined,
                    page_fault,
                    memory_error,
                );
            }
            CpuProbeExit::Fault(fault) => {
                return report(
                    PagedProbeExit::Fault(fault),
                    instructions_executed,
                    current_rflags_defined,
                    page_fault,
                    memory_error,
                );
            }
            CpuProbeExit::Backend(error) => {
                return report(
                    PagedProbeExit::Backend(error),
                    instructions_executed,
                    current_rflags_defined,
                    page_fault,
                    memory_error,
                );
            }
            CpuProbeExit::PortIo(error) => {
                return report(
                    PagedProbeExit::PortIo(error),
                    instructions_executed,
                    current_rflags_defined,
                    page_fault,
                    memory_error,
                );
            }
            CpuProbeExit::Rejected(reason) => {
                return report(
                    PagedProbeExit::Rejected(PagedProbeRejection::Cpu(reason)),
                    instructions_executed,
                    current_rflags_defined,
                    page_fault,
                    memory_error,
                );
            }
        }
    }
}

fn report(
    exit: PagedProbeExit,
    instructions_executed: u64,
    rflags_defined: u64,
    page_fault: Option<PageFaultRecord>,
    memory_error: Option<TrackedAccessError>,
) -> PagedProbeQuantumReport {
    PagedProbeQuantumReport {
        exit,
        instructions_executed,
        rflags_defined,
        page_fault,
        memory_error,
    }
}

struct PagedProbeMemory<'a> {
    memory: &'a mut crate::PhysicalMemory,
    paging: Long64ProbeConfig,
    instruction_rip: u64,
    committed_write: &'a mut dyn FnMut(GuestPhysicalRange),
    page_fault: Option<PageFaultRecord>,
    memory_error: Option<TrackedAccessError>,
}

impl PagedProbeMemory<'_> {
    fn access(
        &mut self,
        address: u64,
        context: AccessContext,
        operation: RamAccessOperation,
    ) -> Result<TrackedAccessOutcome, MemoryError> {
        let result = crate::access_long64_4k_ram(
            self.memory,
            &self.paging,
            TrackedAccessRequest {
                instruction_rip: self.instruction_rip,
                address: GuestLinearAddress::new(address),
                context: AccessContext {
                    origin: AccessOrigin::Explicit,
                    ..context
                },
                operation,
            },
            &mut *self.committed_write,
        );
        match result.outcome {
            Ok(outcome) => Ok(outcome),
            Err(TrackedAccessError::PageFault { record, .. }) => {
                self.page_fault = Some(record.clone());
                Err(MemoryError::Architectural(record.fault().clone()))
            }
            Err(error) => {
                let memory_error =
                    MemoryError::Backend(format!("tracked RAM access failed: {error:?}"));
                self.memory_error = Some(error);
                Err(memory_error)
            }
        }
    }
}

impl GuestMemory for PagedProbeMemory<'_> {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        match self.access(address, context, RamAccessOperation::Fetch)? {
            TrackedAccessOutcome::Fetched(value) => Ok(value),
            _ => Err(MemoryError::Backend(
                "paged probe fetch returned a non-fetch result".into(),
            )),
        }
    }

    fn read(
        &mut self,
        address: u64,
        context: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        if destination.len() != 8 {
            return Err(MemoryError::Backend(
                "paged probe only supports qword reads".into(),
            ));
        }
        let value = match self.access(
            address,
            context,
            RamAccessOperation::Read {
                width: crate::AccessWidth::Qword,
            },
        )? {
            TrackedAccessOutcome::Read(value) => value,
            _ => {
                return Err(MemoryError::Backend(
                    "paged probe read returned a non-read result".into(),
                ));
            }
        };
        destination.copy_from_slice(&value.to_le_bytes());
        Ok(())
    }

    fn write(
        &mut self,
        address: u64,
        context: AccessContext,
        source: &[u8],
    ) -> Result<(), MemoryError> {
        if source.len() != 8 {
            return Err(MemoryError::Backend(
                "paged probe only supports qword writes".into(),
            ));
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
            _ => Err(MemoryError::Backend(
                "paged probe write returned a non-write result".into(),
            )),
        }
    }

    fn atomic_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: od_isa::OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unsupported("atomic exchange")
    }
    fn atomic_fetch_add_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: od_isa::OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unsupported("atomic fetch-add")
    }
    fn atomic_compare_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: od_isa::OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unsupported("atomic compare-exchange")
    }
    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Err(MemoryError::Backend(
            "paged probe does not support MFENCE".into(),
        ))
    }
}

fn unsupported(operation: &'static str) -> Result<u64, MemoryError> {
    Err(MemoryError::Backend(format!(
        "paged probe does not support {operation}"
    )))
}
