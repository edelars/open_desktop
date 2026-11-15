//! Bounded mixed-instruction quantum through the tracked Long64 RAM walker.

use crate::{
    GuestLinearAddress, GuestPhysicalRange, Long64ProbeConfig, MachineBus, RamAccessOperation,
    TrackedAccessError, TrackedAccessOutcome, TrackedAccessRequest,
};
use od_isa::LongModeState;
use od_isa::execute::{
    PagedCpl0Cr0WpProbeV1, PagedCpl0Cr3ProbeV1, PagedCpl0ProbeV1, PagedCpl0StackProbeV1,
    PagedProbeExit as CpuProbeExit, PagedProbeRejection as CpuProbeRejection,
    TranslationContextChange, step_paged_cr0_wp_probe, step_paged_cr3_probe, step_paged_probe,
    step_paged_probe_with_rdtsc, step_paged_stack_probe,
};
use od_isa::memory::{
    AccessContext, AccessOrigin, ArchitecturalFault, GuestMemory, MemoryError, PageFaultRecord,
};

#[derive(Debug, Eq, PartialEq)]
pub enum PagedProbeRejection {
    Configuration(crate::ProbeConfigError),
    /// The CPL0 profile set refused the state before any fetch (S4-38L3, set path only).
    Cpl0Set(od_isa::Cpl0ProfileSetInputError),
    Cpu(CpuProbeRejection),
    CpuOwnedCr3State {
        value: u64,
    },
    CpuCr3Required,
    Cr3ProfileMismatch {
        cr0: Option<u64>,
        cr4: Option<u64>,
        efer: u64,
    },
    Cr0WpProfileMismatch {
        cr0: Option<u64>,
        cr4: Option<u64>,
        efer: u64,
    },
}

#[derive(Debug, Eq, PartialEq)]
pub enum PagedProbeIntegrationError {
    Cr3EffectMismatch {
        active_cr3: u64,
        effect_old_cr3: u64,
        cpu_cr3: Option<u64>,
        effect_new_cr3: u64,
    },
    Cr3SnapshotRejected {
        effect: TranslationContextChange,
        error: crate::ProbeConfigError,
    },
    Cr0EffectMismatch {
        active_cr0: u64,
        effect_old_cr0: u64,
        cpu_cr0: Option<u64>,
        effect_new_cr0: u64,
    },
    Cr0SnapshotRejected {
        effect: TranslationContextChange,
        error: crate::ProbeConfigError,
    },
    UnexpectedContextChange {
        effect: TranslationContextChange,
    },
}

#[derive(Debug, Eq, PartialEq)]
pub enum PagedProbeExit {
    BudgetExhausted,
    Stopped,
    Halted,
    Shutdown,
    Rejected(PagedProbeRejection),
    IntegrationError(PagedProbeIntegrationError),
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
    /// CPU-owned CR0/CR3 changes applied to the active immutable snapshot
    /// during this quantum, in retirement order.
    pub translation_context_changes: Vec<TranslationContextChange>,
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
        false,
        false,
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
        None,
    )
}

/// Run the base quantum like [`run_paged_probe`], with an explicit time source
/// for the exact bare `RDTSC` (`0F 31`, S4-37a6). The source is called once per
/// retired `RDTSC`; the machine does not advance time itself. Base profile only.
pub fn run_paged_probe_with_rdtsc(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    instruction_budget: u64,
    rflags_defined: u64,
    stop: impl FnMut() -> bool,
    committed_write: impl FnMut(GuestPhysicalRange),
    source: &mut dyn FnMut() -> u64,
) -> PagedProbeQuantumReport {
    run_paged_probe_inner(
        false,
        false,
        false,
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
        Some(source),
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
        false,
        false,
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
        None,
    )
}

/// Run a bounded Long64 quantum whose CR3 is CPU-owned. A successful MOV CR3
/// is consumed at its retirement boundary before STOP polling or the next
/// instruction fetch; the next instruction uses a validated snapshot built
/// from the committed CPU state.
pub fn run_paged_cr3_probe(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    efer: u64,
    context: AccessContext,
    instruction_budget: u64,
    rflags_defined: u64,
    stop: impl FnMut() -> bool,
    committed_write: impl FnMut(GuestPhysicalRange),
) -> PagedProbeQuantumReport {
    run_paged_probe_inner(
        false,
        true,
        false,
        false,
        state,
        bus,
        0,
        efer,
        context,
        instruction_budget,
        rflags_defined,
        stop,
        committed_write,
        None,
    )
}

/// Run the CPU-owned CR3 quantum through mixed 4 KiB/2 MiB RAM mappings.
///
/// Same CPU profile, report and context-change semantics as
/// [`run_paged_cr3_probe`]; the tracked walk additionally accepts a PDE with
/// PS=1 as a 2 MiB leaf. RAM only. The CPU profile itself keeps instruction
/// fetch inside the 4 KiB page of RIP (an RIP too close to the page end is
/// `Unsupported` before any fetch) and each operand within one 4 KiB page (a
/// crossing is a backend `CrossesPage` error, not a guest `#PF`).
pub fn run_paged_cr3_probe_2m_ram(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    efer: u64,
    context: AccessContext,
    instruction_budget: u64,
    rflags_defined: u64,
    stop: impl FnMut() -> bool,
    committed_write: impl FnMut(GuestPhysicalRange),
) -> PagedProbeQuantumReport {
    run_paged_probe_inner(
        false,
        true,
        false,
        true,
        state,
        bus,
        0,
        efer,
        context,
        instruction_budget,
        rflags_defined,
        stop,
        committed_write,
        None,
    )
}

/// Run the base quantum like [`run_paged_probe`] (explicit `cr3`, `state.cr3` must be `None`),
/// but walk through mixed 4 KiB/2 MiB RAM mappings like [`run_paged_cr3_probe_2m_ram`]
/// (S4-37L5: the Linux direct boot maps low RAM with 2 MiB leaves). The CPU profile is the base
/// one, so `MOV r8,imm8`, `MOV r16,imm16`, `IN/OUT DX`, `HLT` and (with a `rdtsc` source) bare
/// `RDTSC` are admitted; MOV CR3 is not (the CR3 is the board's, not the CPU's). The 4 KiB
/// fetch and operand limits of the 2 MiB walk apply. Crate-internal, not re-exported (S4-38L1: `LoadedFirmwareBoard::run_linux_boot` runs it).
pub(crate) fn run_paged_probe_2m_ram(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    instruction_budget: u64,
    rflags_defined: u64,
    stop: impl FnMut() -> bool,
    committed_write: impl FnMut(GuestPhysicalRange),
    rdtsc: Option<&mut dyn FnMut() -> u64>,
) -> PagedProbeQuantumReport {
    run_paged_probe_inner(
        false,
        false,
        false,
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
        rdtsc,
    )
}

/// Run the opt-in CPU-owned CR0.WP quantum. A successful MOV CR0 effect is
/// applied to the immutable paging snapshot before the next STOP poll or
/// instruction access.
pub fn run_paged_cr0_wp_probe(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    efer: u64,
    context: AccessContext,
    instruction_budget: u64,
    rflags_defined: u64,
    stop: impl FnMut() -> bool,
    committed_write: impl FnMut(GuestPhysicalRange),
) -> PagedProbeQuantumReport {
    run_paged_probe_inner(
        false,
        false,
        true,
        false,
        state,
        bus,
        0,
        efer,
        context,
        instruction_budget,
        rflags_defined,
        stop,
        committed_write,
        None,
    )
}

fn run_paged_probe_inner(
    stack_profile: bool,
    cpu_cr3_profile: bool,
    cpu_cr0_wp_profile: bool,
    large_ram_profile: bool,
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    instruction_budget: u64,
    rflags_defined: u64,
    mut stop: impl FnMut() -> bool,
    mut committed_write: impl FnMut(GuestPhysicalRange),
    mut rdtsc: Option<&mut dyn FnMut() -> u64>,
) -> PagedProbeQuantumReport {
    let mut instructions_executed = 0;
    let mut current_rflags_defined = rflags_defined;
    let mut translation_context_changes = Vec::new();
    if !cpu_cr3_profile
        && !cpu_cr0_wp_profile
        && let Some(value) = state.cr3
    {
        return report(
            PagedProbeExit::Rejected(PagedProbeRejection::CpuOwnedCr3State { value }),
            instructions_executed,
            current_rflags_defined,
            None,
            None,
            &translation_context_changes,
        );
    }
    if (cpu_cr3_profile || cpu_cr0_wp_profile) && state.cr3.is_none() {
        return report(
            PagedProbeExit::Rejected(PagedProbeRejection::CpuCr3Required),
            instructions_executed,
            current_rflags_defined,
            None,
            None,
            &translation_context_changes,
        );
    }
    if cpu_cr3_profile || cpu_cr0_wp_profile {
        if context.mode != od_isa::memory::InstructionMode::Long64 {
            return report(
                PagedProbeExit::Rejected(PagedProbeRejection::Cpu(CpuProbeRejection::WrongMode)),
                instructions_executed,
                current_rflags_defined,
                None,
                None,
                &translation_context_changes,
            );
        }
        if context.privilege != od_isa::memory::PrivilegeLevel::Ring0 {
            return report(
                PagedProbeExit::Rejected(PagedProbeRejection::Cpu(
                    CpuProbeRejection::WrongPrivilege,
                )),
                instructions_executed,
                current_rflags_defined,
                None,
                None,
                &translation_context_changes,
            );
        }
        if current_rflags_defined & !od_isa::INTEGER_FLAGS != 0 {
            return report(
                PagedProbeExit::Rejected(PagedProbeRejection::Cpu(
                    CpuProbeRejection::InvalidRflagsDefined {
                        value: current_rflags_defined,
                    },
                )),
                instructions_executed,
                current_rflags_defined,
                None,
                None,
                &translation_context_changes,
            );
        }
        let cr_profile_matches = if cpu_cr0_wp_profile {
            state.cr0.is_some_and(|cr0| {
                cr0 == od_isa::execute::Cr0Profile::BASE
                    || cr0 == (od_isa::execute::Cr0Profile::BASE & !(1 << 16))
            })
        } else {
            state.cr0 == Some(od_isa::execute::Cr0Profile::BASE)
        };
        if !cr_profile_matches
            || state.cr4 != Some(od_isa::execute::Cr4Profile::BASE)
            || efer != 0xd00
        {
            let rejection = if cpu_cr0_wp_profile {
                PagedProbeRejection::Cr0WpProfileMismatch {
                    cr0: state.cr0,
                    cr4: state.cr4,
                    efer,
                }
            } else {
                PagedProbeRejection::Cr3ProfileMismatch {
                    cr0: state.cr0,
                    cr4: state.cr4,
                    efer,
                }
            };
            return report(
                PagedProbeExit::Rejected(rejection),
                instructions_executed,
                current_rflags_defined,
                None,
                None,
                &translation_context_changes,
            );
        }
    }
    let mut active_paging = if cpu_cr3_profile || cpu_cr0_wp_profile {
        let cr3 = state.cr3.expect("CPU CR3 presence was checked above");
        match Long64ProbeConfig::from_cpu_state(state, cr3, efer) {
            Ok(paging) => Some(paging),
            Err(error) => {
                return report(
                    PagedProbeExit::Rejected(PagedProbeRejection::Configuration(error)),
                    instructions_executed,
                    current_rflags_defined,
                    None,
                    None,
                    &translation_context_changes,
                );
            }
        }
    } else {
        None
    };
    loop {
        if stop() {
            return report(
                PagedProbeExit::Stopped,
                instructions_executed,
                current_rflags_defined,
                None,
                None,
                &translation_context_changes,
            );
        }
        if instructions_executed >= instruction_budget {
            return report(
                PagedProbeExit::BudgetExhausted,
                instructions_executed,
                current_rflags_defined,
                None,
                None,
                &translation_context_changes,
            );
        }

        let paging = if let Some(paging) = active_paging {
            paging
        } else {
            match Long64ProbeConfig::from_cpu_state(state, cr3, efer) {
                Ok(paging) => paging,
                Err(error) => {
                    return report(
                        PagedProbeExit::Rejected(PagedProbeRejection::Configuration(error)),
                        instructions_executed,
                        current_rflags_defined,
                        None,
                        None,
                        &translation_context_changes,
                    );
                }
            }
        };
        let instruction_rip = state.rip;
        let (memory, mut pio) = bus.split_for_pio_step();
        let mut memory = PagedProbeMemory {
            memory,
            paging,
            instruction_rip,
            large_ram_profile,
            committed_write: &mut committed_write,
            page_fault: None,
            memory_error: None,
        };
        // One selected CPU entry owns decode and instruction choice. The
        // machine never peeks at guest opcodes or retries another entry.
        let cpu = if cpu_cr0_wp_profile {
            step_paged_cr0_wp_probe(
                PagedCpl0Cr0WpProbeV1,
                state,
                &mut memory,
                &mut pio,
                AccessContext {
                    origin: AccessOrigin::Explicit,
                    ..context
                },
                efer,
                current_rflags_defined,
            )
        } else if cpu_cr3_profile {
            step_paged_cr3_probe(
                PagedCpl0Cr3ProbeV1,
                state,
                &mut memory,
                &mut pio,
                AccessContext {
                    origin: AccessOrigin::Explicit,
                    ..context
                },
                efer,
                current_rflags_defined,
            )
        } else if stack_profile {
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
        } else if let Some(source) = rdtsc.as_mut() {
            step_paged_probe_with_rdtsc(
                PagedCpl0ProbeV1,
                state,
                &mut memory,
                &mut pio,
                AccessContext {
                    origin: AccessOrigin::Explicit,
                    ..context
                },
                current_rflags_defined,
                &mut **source,
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
                    &translation_context_changes,
                );
            }
            CpuProbeExit::Shutdown => {
                return report(
                    PagedProbeExit::Shutdown,
                    instructions_executed,
                    current_rflags_defined,
                    page_fault,
                    memory_error,
                    &translation_context_changes,
                );
            }
            CpuProbeExit::Unsupported { opcode } => {
                return report(
                    PagedProbeExit::Unsupported { opcode },
                    instructions_executed,
                    current_rflags_defined,
                    page_fault,
                    memory_error,
                    &translation_context_changes,
                );
            }
            CpuProbeExit::Fault(fault) => {
                return report(
                    PagedProbeExit::Fault(fault),
                    instructions_executed,
                    current_rflags_defined,
                    page_fault,
                    memory_error,
                    &translation_context_changes,
                );
            }
            CpuProbeExit::Backend(error) => {
                return report(
                    PagedProbeExit::Backend(error),
                    instructions_executed,
                    current_rflags_defined,
                    page_fault,
                    memory_error,
                    &translation_context_changes,
                );
            }
            CpuProbeExit::PortIo(error) => {
                return report(
                    PagedProbeExit::PortIo(error),
                    instructions_executed,
                    current_rflags_defined,
                    page_fault,
                    memory_error,
                    &translation_context_changes,
                );
            }
            CpuProbeExit::Rejected(reason) => {
                return report(
                    PagedProbeExit::Rejected(PagedProbeRejection::Cpu(reason)),
                    instructions_executed,
                    current_rflags_defined,
                    page_fault,
                    memory_error,
                    &translation_context_changes,
                );
            }
            CpuProbeExit::TranslationContextChange(effect) => {
                let candidate = match effect {
                    TranslationContextChange::Cr3Write { old_cr3, new_cr3 }
                        if !cpu_cr0_wp_profile =>
                    {
                        let active_cr3 = paging.cr3();
                        if old_cr3 != active_cr3 || state.cr3 != Some(new_cr3) {
                            return report(
                                PagedProbeExit::IntegrationError(
                                    PagedProbeIntegrationError::Cr3EffectMismatch {
                                        active_cr3,
                                        effect_old_cr3: old_cr3,
                                        cpu_cr3: state.cr3,
                                        effect_new_cr3: new_cr3,
                                    },
                                ),
                                instructions_executed,
                                current_rflags_defined,
                                page_fault,
                                memory_error,
                                &translation_context_changes,
                            );
                        }
                        match Long64ProbeConfig::from_cpu_state(state, new_cr3, efer) {
                            Ok(candidate) => candidate,
                            Err(error) => {
                                return report(
                                    PagedProbeExit::IntegrationError(
                                        PagedProbeIntegrationError::Cr3SnapshotRejected {
                                            effect,
                                            error,
                                        },
                                    ),
                                    instructions_executed,
                                    current_rflags_defined,
                                    page_fault,
                                    memory_error,
                                    &translation_context_changes,
                                );
                            }
                        }
                    }
                    TranslationContextChange::Cr0Write { old_cr0, new_cr0 }
                        if cpu_cr0_wp_profile =>
                    {
                        let active_cr0 = paging.cr0();
                        if old_cr0 != active_cr0 || state.cr0 != Some(new_cr0) {
                            return report(
                                PagedProbeExit::IntegrationError(
                                    PagedProbeIntegrationError::Cr0EffectMismatch {
                                        active_cr0,
                                        effect_old_cr0: old_cr0,
                                        cpu_cr0: state.cr0,
                                        effect_new_cr0: new_cr0,
                                    },
                                ),
                                instructions_executed,
                                current_rflags_defined,
                                page_fault,
                                memory_error,
                                &translation_context_changes,
                            );
                        }
                        let cr3 = state.cr3.expect("CPU CR3 was admitted before execution");
                        match Long64ProbeConfig::from_cpu_state(state, cr3, efer) {
                            Ok(candidate) => candidate,
                            Err(error) => {
                                return report(
                                    PagedProbeExit::IntegrationError(
                                        PagedProbeIntegrationError::Cr0SnapshotRejected {
                                            effect,
                                            error,
                                        },
                                    ),
                                    instructions_executed,
                                    current_rflags_defined,
                                    page_fault,
                                    memory_error,
                                    &translation_context_changes,
                                );
                            }
                        }
                    }
                    _ => {
                        return report(
                            PagedProbeExit::IntegrationError(
                                PagedProbeIntegrationError::UnexpectedContextChange { effect },
                            ),
                            instructions_executed,
                            current_rflags_defined,
                            page_fault,
                            memory_error,
                            &translation_context_changes,
                        );
                    }
                };
                active_paging = Some(candidate);
                translation_context_changes.push(effect);
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
    translation_context_changes: &[TranslationContextChange],
) -> PagedProbeQuantumReport {
    PagedProbeQuantumReport {
        exit,
        instructions_executed,
        rflags_defined,
        page_fault,
        memory_error,
        translation_context_changes: translation_context_changes.to_vec(),
    }
}

struct PagedProbeMemory<'a> {
    memory: &'a mut crate::PhysicalMemory,
    paging: Long64ProbeConfig,
    instruction_rip: u64,
    large_ram_profile: bool,
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
        let request = TrackedAccessRequest {
            instruction_rip: self.instruction_rip,
            address: GuestLinearAddress::new(address),
            context: AccessContext {
                origin: AccessOrigin::Explicit,
                ..context
            },
            operation,
        };
        let result = if self.large_ram_profile {
            crate::access_long64_2m_ram(
                self.memory,
                &self.paging,
                request,
                &mut *self.committed_write,
            )
        } else {
            crate::access_long64_4k_ram(
                self.memory,
                &self.paging,
                request,
                &mut *self.committed_write,
            )
        };
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
        let width = match destination.len() {
            2 => crate::AccessWidth::Word,
            4 => crate::AccessWidth::Dword,
            8 => crate::AccessWidth::Qword,
            _ => {
                return Err(MemoryError::Backend(
                    "paged probe only supports word, dword, and qword reads".into(),
                ));
            }
        };
        let value = match self.access(address, context, RamAccessOperation::Read { width })? {
            TrackedAccessOutcome::Read(value) => value,
            _ => {
                return Err(MemoryError::Backend(
                    "paged probe read returned a non-read result".into(),
                ));
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
