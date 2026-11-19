//! Bounded mixed-instruction quantum through the tracked Long64 walker.

#[cfg(test)]
mod firmware_tests;

use crate::display_device::{DISPLAY_REGISTERS_BASE, DISPLAY_REGISTERS_BYTES, DisplayDevice};
use crate::nvme::{NVME_MMIO_BASE, NVME_MMIO_BYTES, NvmeDevice};
use crate::nvram_device::{NVRAM_MMIO_BASE, NVRAM_MMIO_BYTES, NvramHandle};
use crate::page_walk::MmioDispatchError;
use crate::tpm_crb::{TPM_CRB_BASE, TPM_CRB_LEN, TpmCrbHandle};
use crate::{
    DeviceAccess, DeviceError, GuestLinearAddress, GuestPhysicalAddress, GuestPhysicalRange,
    IoSpace, Long64ProbeConfig, MachineBus, RamAccessOperation, TrackedAccessError,
    TrackedAccessOutcome, TrackedAccessRequest, VirtualClock,
};
use od_isa::LongModeState;
use od_isa::execute::{
    GuestFpState, PagedCpl0Cr0WpProbeV1, PagedCpl0Cr3ProbeV1, PagedCpl0FirmwareBootV1,
    PagedCpl0ProbeV1, PagedCpl0StackProbeV1, PagedProbeExit as CpuProbeExit,
    PagedProbeRejection as CpuProbeRejection, TranslationContextChange, step_paged_cr0_wp_probe,
    step_paged_cr3_probe, step_paged_firmware_probe_with_guest_fp, step_paged_probe,
    step_paged_probe_with_rdtsc, step_paged_stack_probe,
};
use od_isa::interrupt::{
    Cpl0GeneralProtectionDelivery, Cpl0Int3Delivery, Cpl0PageFaultDelivery,
    deliver_firmware_page_fault, deliver_firmware_synchronous_fault,
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
    FirmwareBreakpointDelivery {
        failure: Cpl0Int3Delivery,
    },
    FirmwarePageFaultDelivery {
        failure: Cpl0PageFaultDelivery,
        delivery_fault: Option<PageFaultRecord>,
    },
    FirmwareSynchronousFaultDelivery {
        fault: ArchitecturalFault,
        failure: Cpl0GeneralProtectionDelivery,
        delivery_fault: Option<PageFaultRecord>,
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
        false,
        state,
        ProbeBus::Exclusive(bus),
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
        false,
        state,
        ProbeBus::Exclusive(bus),
        cr3,
        efer,
        context,
        instruction_budget,
        rflags_defined,
        stop,
        committed_write,
        Some(ProbeInputs::External(source)),
    )
}

/// Devices whose exact register apertures are available to loaded firmware.
#[derive(Clone, Copy, Default)]
pub(crate) struct FirmwareMmio<'a> {
    pub(crate) nvram: Option<&'a NvramHandle>,
    pub(crate) display: Option<&'a DisplayDevice>,
    pub(crate) nvme: Option<&'a NvmeDevice>,
    pub(crate) tpm: Option<&'a TpmCrbHandle>,
}

impl FirmwareMmio<'_> {
    fn windows(self) -> &'static [(u64, u64)] {
        const DISPLAY: (u64, u64) = (DISPLAY_REGISTERS_BASE, DISPLAY_REGISTERS_BYTES);
        const NVRAM: (u64, u64) = (NVRAM_MMIO_BASE, NVRAM_MMIO_BYTES);
        const NVME: (u64, u64) = (NVME_MMIO_BASE, NVME_MMIO_BYTES);
        const TPM: (u64, u64) = (TPM_CRB_BASE, TPM_CRB_LEN);
        const WINDOWS: [&[(u64, u64)]; 16] = [
            &[],
            &[DISPLAY],
            &[NVRAM],
            &[DISPLAY, NVRAM],
            &[NVME],
            &[DISPLAY, NVME],
            &[NVRAM, NVME],
            &[DISPLAY, NVRAM, NVME],
            &[TPM],
            &[DISPLAY, TPM],
            &[NVRAM, TPM],
            &[DISPLAY, NVRAM, TPM],
            &[NVME, TPM],
            &[DISPLAY, NVME, TPM],
            &[NVRAM, NVME, TPM],
            &[DISPLAY, NVRAM, NVME, TPM],
        ];
        let index = usize::from(self.display.is_some())
            | (usize::from(self.nvram.is_some()) << 1)
            | (usize::from(self.nvme.is_some()) << 2)
            | (usize::from(self.tpm.is_some()) << 3);
        WINDOWS[index]
    }

    fn access(
        self,
        address: GuestPhysicalAddress,
        operation: RamAccessOperation,
        sample_time: &dyn Fn() -> Result<u64, crate::DeviceError>,
    ) -> Result<u64, MmioDispatchError> {
        if let Some(display) = self.display
            && (DISPLAY_REGISTERS_BASE..DISPLAY_REGISTERS_BASE + DISPLAY_REGISTERS_BYTES)
                .contains(&address.get())
        {
            return access_device_registers(
                "display",
                address.get() - DISPLAY_REGISTERS_BASE,
                operation,
                |access| display.read_mmio(access),
                |access, value| display.write_mmio(access, value),
            );
        }
        if let Some(nvram) = self.nvram
            && (NVRAM_MMIO_BASE..NVRAM_MMIO_BASE + NVRAM_MMIO_BYTES).contains(&address.get())
        {
            return access_device_registers(
                "NVRAM",
                address.get() - NVRAM_MMIO_BASE,
                operation,
                |access| nvram.read(access),
                |access, value| nvram.write(access, value),
            );
        }
        if let Some(nvme) = self.nvme
            && (NVME_MMIO_BASE..NVME_MMIO_BASE + NVME_MMIO_BYTES).contains(&address.get())
        {
            return access_device_registers(
                "NVMe",
                address.get() - NVME_MMIO_BASE,
                operation,
                |access| nvme.read_mmio(access),
                |access, value| nvme.write_mmio(access, value),
            );
        }
        if let Some(tpm) = self.tpm
            && (TPM_CRB_BASE..TPM_CRB_BASE + TPM_CRB_LEN).contains(&address.get())
        {
            return access_device_registers(
                "TPM CRB",
                address.get() - TPM_CRB_BASE,
                operation,
                |access| tpm.read(access),
                |access, value| tpm.write(access, value, sample_time),
            );
        }
        Err(MmioDispatchError::Backend(
            "unbound firmware MMIO access".into(),
        ))
    }
}

/// Run the firmware instruction profile through the shared 4 KiB tracked walker.
/// Firmware may switch its active CR3 and enter its installed exception handlers.
pub(crate) fn run_paged_firmware_probe(
    state: &mut LongModeState,
    fp: &mut GuestFpState,
    bus: &mut MachineBus,
    mmio: FirmwareMmio<'_>,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    instruction_budget: u64,
    rflags_defined: u64,
    stop: impl FnMut() -> bool,
    committed_write: impl FnMut(GuestPhysicalRange),
    clock: Option<VirtualClock>,
) -> PagedProbeQuantumReport {
    run_paged_probe_inner(
        false,
        false,
        false,
        false,
        true,
        state,
        ProbeBus::Exclusive(bus),
        cr3,
        efer,
        context,
        instruction_budget,
        rflags_defined,
        stop,
        committed_write,
        Some(ProbeInputs::Firmware { clock, fp, mmio }),
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
        false,
        state,
        ProbeBus::Exclusive(bus),
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
        false,
        state,
        ProbeBus::Exclusive(bus),
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
        false,
        state,
        ProbeBus::Exclusive(bus),
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
        false,
        state,
        ProbeBus::Exclusive(bus),
        cr3,
        efer,
        context,
        instruction_budget,
        rflags_defined,
        stop,
        committed_write,
        rdtsc.map(ProbeInputs::External),
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
        false,
        state,
        ProbeBus::Exclusive(bus),
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

/// What the probe loop reaches the machine through (S6-42b). `Exclusive`: the caller owns the bus
/// (`&mut MachineBus`: the device table is taken without a lock). `Shared`: a vCPU thread holds
/// only `&MachineBus`: RAM is the shared reference and every port access takes the one device lock
/// of the bus for that access alone (`split_for_set_step_shared`), never across an instruction.
pub(crate) enum ProbeBus<'a> {
    Exclusive(&'a mut MachineBus),
    Shared(&'a MachineBus, usize),
}

/// The base quantum of [`run_paged_probe`] for a vCPU thread that holds only `&MachineBus`
/// (S6-42b, A7 of S6-40): the same loop and the same CPU profile, RAM through the shared
/// reference (the generations of `commit_write` are the machine's), the device lock taken for one
/// port access at a time. `vcpu` is the index of the calling vCPU. No lock of the bus is held
/// between two instructions.
pub fn run_paged_probe_shared(
    state: &mut LongModeState,
    bus: &MachineBus,
    vcpu: usize,
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
        false,
        state,
        ProbeBus::Shared(bus, vcpu),
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

enum ProbeInputs<'a> {
    External(&'a mut dyn FnMut() -> u64),
    Firmware {
        clock: Option<VirtualClock>,
        fp: &'a mut GuestFpState,
        mmio: FirmwareMmio<'a>,
    },
}

fn run_paged_probe_inner(
    stack_profile: bool,
    cpu_cr3_profile: bool,
    cpu_cr0_wp_profile: bool,
    large_ram_profile: bool,
    firmware_boot_profile: bool,
    state: &mut LongModeState,
    mut bus: ProbeBus<'_>,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    instruction_budget: u64,
    rflags_defined: u64,
    mut stop: impl FnMut() -> bool,
    mut committed_write: impl FnMut(GuestPhysicalRange),
    mut inputs: Option<ProbeInputs<'_>>,
) -> PagedProbeQuantumReport {
    let mut instructions_executed = 0;
    let mut current_rflags_defined = rflags_defined;
    let mut translation_context_changes = Vec::new();
    let mut fault_delivered_at = None;
    if !cpu_cr3_profile
        && !cpu_cr0_wp_profile
        && !firmware_boot_profile
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
    let owns_cr3 = cpu_cr3_profile || cpu_cr0_wp_profile || firmware_boot_profile;
    let mut active_paging = if let (true, Some(cr3)) = (owns_cr3, state.cr3) {
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
    let rtc_clock_start_ns = match &inputs {
        Some(ProbeInputs::Firmware {
            clock: Some(clock), ..
        }) => Some(clock.now_ns()),
        _ => None,
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
        let step = ProbeStep {
            stack_profile,
            cpu_cr3_profile,
            cpu_cr0_wp_profile,
            large_ram_profile,
            firmware_boot_profile,
            paging,
            instruction_rip,
            context,
            efer,
            rflags_defined: current_rflags_defined,
            retired_before: instructions_executed,
        };
        let (cpu, page_fault, memory_error) = match &mut bus {
            ProbeBus::Exclusive(bus) => {
                let (memory, mut pio) = bus.split_for_pio_step();
                pio.set_firmware_time(rtc_clock_start_ns, step.retired_before);
                step.run(
                    state,
                    memory,
                    &mut pio,
                    &mut committed_write,
                    inputs.as_mut(),
                )
            }
            ProbeBus::Shared(bus, vcpu) => {
                let (memory, io) = bus.split_for_set_step_shared(*vcpu);
                let mut pio = io.port_io();
                step.run(
                    state,
                    memory,
                    &mut pio,
                    &mut committed_write,
                    inputs.as_mut(),
                )
            }
        };
        current_rflags_defined = cpu.rflags_defined;
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
                if firmware_boot_profile
                    && state.idtr.is_some()
                    && fault_delivered_at != Some(instructions_executed)
                    && let ProbeBus::Exclusive(bus) = &mut bus
                {
                    let (physical_memory, _) = bus.split_for_pio_step();
                    let mut delivery_memory = step.memory(
                        physical_memory,
                        &mut committed_write,
                        FirmwareMmio::default(),
                    );
                    match delivery_memory.deliver_firmware_fault(
                        state,
                        &fault,
                        page_fault.as_ref(),
                        context,
                    ) {
                        Some(Ok(())) => {
                            // The fault did not retire. Refuse another delivery
                            // until the handler retires, so a faulting entry cannot spin.
                            fault_delivered_at = Some(instructions_executed);
                            continue;
                        }
                        Some(Err(error)) => {
                            return report(
                                PagedProbeExit::IntegrationError(error),
                                instructions_executed,
                                current_rflags_defined,
                                page_fault,
                                delivery_memory.memory_error,
                                &translation_context_changes,
                            );
                        }
                        None => {}
                    }
                }
                return report(
                    PagedProbeExit::Fault(fault),
                    instructions_executed,
                    current_rflags_defined,
                    page_fault,
                    memory_error,
                    &translation_context_changes,
                );
            }
            CpuProbeExit::FirmwareBreakpointDelivery(failure) => {
                return report(
                    PagedProbeExit::IntegrationError(
                        PagedProbeIntegrationError::FirmwareBreakpointDelivery { failure },
                    ),
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
                        if cpu_cr0_wp_profile || firmware_boot_profile =>
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
                        let cr3 = paging.cr3();
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

/// One CPU step of the probe loop with its profile flags (the body that was inline in the loop,
/// now generic over the port view so the exclusive and the shared bus run the same code).
struct ProbeStep {
    stack_profile: bool,
    cpu_cr3_profile: bool,
    cpu_cr0_wp_profile: bool,
    large_ram_profile: bool,
    firmware_boot_profile: bool,
    paging: Long64ProbeConfig,
    instruction_rip: u64,
    context: AccessContext,
    efer: u64,
    rflags_defined: u64,
    retired_before: u64,
}

impl ProbeStep {
    fn memory<'a>(
        &self,
        memory: &'a crate::PhysicalMemory,
        committed_write: &'a mut dyn FnMut(GuestPhysicalRange),
        mmio: FirmwareMmio<'a>,
    ) -> PagedProbeMemory<'a> {
        PagedProbeMemory {
            memory,
            paging: self.paging,
            instruction_rip: self.instruction_rip,
            large_ram_profile: self.large_ram_profile,
            firmware_boot_profile: self.firmware_boot_profile,
            mmio,
            mmio_clock: None,
            retired_before: self.retired_before,
            committed_write,
            page_fault: None,
            memory_error: None,
        }
    }

    fn run<P: od_isa::PortIo>(
        &self,
        state: &mut LongModeState,
        memory: &crate::PhysicalMemory,
        pio: &mut P,
        committed_write: &mut dyn FnMut(GuestPhysicalRange),
        inputs: Option<&mut ProbeInputs<'_>>,
    ) -> (
        od_isa::PagedProbeResult<P::Error>,
        Option<PageFaultRecord>,
        Option<TrackedAccessError>,
    ) {
        let mmio = match inputs.as_deref() {
            Some(ProbeInputs::Firmware { mmio, .. }) => *mmio,
            _ => FirmwareMmio::default(),
        };
        let mut memory = self.memory(memory, committed_write, mmio);
        let context = AccessContext {
            origin: AccessOrigin::Explicit,
            ..self.context
        };
        // One selected CPU entry owns decode and instruction choice. The
        // machine never peeks at guest opcodes or retries another entry.
        let cpu = if self.cpu_cr0_wp_profile {
            step_paged_cr0_wp_probe(
                PagedCpl0Cr0WpProbeV1,
                state,
                &mut memory,
                pio,
                context,
                self.efer,
                self.rflags_defined,
            )
        } else if self.cpu_cr3_profile {
            step_paged_cr3_probe(
                PagedCpl0Cr3ProbeV1,
                state,
                &mut memory,
                pio,
                context,
                self.efer,
                self.rflags_defined,
            )
        } else if self.stack_profile {
            step_paged_stack_probe(
                PagedCpl0StackProbeV1,
                state,
                &mut memory,
                pio,
                context,
                self.rflags_defined,
            )
        } else if self.firmware_boot_profile {
            let Some(ProbeInputs::Firmware { clock, fp, .. }) = inputs else {
                unreachable!("firmware execution uses its vCPU context");
            };
            memory.mmio_clock = clock.as_ref().map(VirtualClock::now_ns);
            let mut sample = clock.as_ref().map(|snapshot| {
                move || {
                    let mut clock = *snapshot;
                    clock
                        .advance_time_ns(self.retired_before)
                        .map_err(|error| {
                            MemoryError::Backend(format!("firmware RDTSC clock: {error:?}"))
                        })?;
                    Ok(clock.tsc())
                }
            });
            step_paged_firmware_probe_with_guest_fp(
                PagedCpl0FirmwareBootV1,
                state,
                &mut memory,
                pio,
                context,
                self.rflags_defined,
                self.paging.cr3(),
                fp,
                sample
                    .as_mut()
                    .map(|sample| sample as &mut dyn FnMut() -> Result<u64, MemoryError>),
            )
        } else if let Some(ProbeInputs::External(source)) = inputs {
            step_paged_probe_with_rdtsc(
                PagedCpl0ProbeV1,
                state,
                &mut memory,
                pio,
                context,
                self.rflags_defined,
                *source,
            )
        } else {
            step_paged_probe(
                PagedCpl0ProbeV1,
                state,
                &mut memory,
                pio,
                context,
                self.rflags_defined,
            )
        };
        (cpu, memory.page_fault, memory.memory_error)
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
    memory: &'a crate::PhysicalMemory,
    paging: Long64ProbeConfig,
    instruction_rip: u64,
    large_ram_profile: bool,
    firmware_boot_profile: bool,
    mmio: FirmwareMmio<'a>,
    mmio_clock: Option<u64>,
    retired_before: u64,
    committed_write: &'a mut dyn FnMut(GuestPhysicalRange),
    page_fault: Option<PageFaultRecord>,
    memory_error: Option<TrackedAccessError>,
}

impl PagedProbeMemory<'_> {
    fn deliver_firmware_fault(
        &mut self,
        state: &mut LongModeState,
        fault: &ArchitecturalFault,
        original_page_fault: Option<&PageFaultRecord>,
        context: AccessContext,
    ) -> Option<Result<(), PagedProbeIntegrationError>> {
        if fault.vector == 14 {
            let record = original_page_fault.filter(|record| record.fault() == fault)?;
            return Some(
                match deliver_firmware_page_fault(state, self, record, context) {
                    Cpl0PageFaultDelivery::Delivered { .. } => Ok(()),
                    failure => Err(PagedProbeIntegrationError::FirmwarePageFaultDelivery {
                        failure,
                        delivery_fault: self.page_fault.take(),
                    }),
                },
            );
        }
        Some(
            match deliver_firmware_synchronous_fault(state, self, fault, context)? {
                Cpl0GeneralProtectionDelivery::Delivered { .. } => Ok(()),
                failure => Err(
                    PagedProbeIntegrationError::FirmwareSynchronousFaultDelivery {
                        fault: fault.clone(),
                        failure,
                        delivery_fault: self.page_fault.take(),
                    },
                ),
            },
        )
    }

    fn access(
        &mut self,
        address: u64,
        context: AccessContext,
        operation: RamAccessOperation,
    ) -> Result<TrackedAccessOutcome, MemoryError> {
        let request = TrackedAccessRequest {
            instruction_rip: self.instruction_rip,
            address: GuestLinearAddress::new(address),
            context: if self.firmware_boot_profile {
                context
            } else {
                AccessContext {
                    origin: AccessOrigin::Explicit,
                    ..context
                }
            },
            operation,
        };
        let windows = self.mmio.windows();
        let result = if self.large_ram_profile {
            crate::access_long64_2m_ram(
                self.memory,
                &self.paging,
                request,
                &mut *self.committed_write,
            )
        } else if !windows.is_empty() {
            crate::page_walk::access_long64_4k_with_mmio(
                self.memory,
                &self.paging,
                request,
                &mut *self.committed_write,
                Some(windows),
                |address, _, operation| {
                    self.mmio.access(address, operation, &|| {
                        self.mmio_clock
                            .unwrap_or(0)
                            .checked_add(self.retired_before)
                            .ok_or(crate::DeviceError::ClockOverflow)
                    })
                },
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
            Err(error) => self.handle_access_error(request, error),
        }
    }

    #[cold]
    #[inline(never)]
    fn handle_access_error(
        &mut self,
        request: TrackedAccessRequest,
        error: TrackedAccessError,
    ) -> Result<TrackedAccessOutcome, MemoryError> {
        // Measured firmware reads its immutable ROM, which is never a target
        // of fetches or writes. Only reuse an already translated whole operand.
        if self.firmware_boot_profile
            && self.mmio.tpm.is_some()
            && !self.large_ram_profile
            && request.context.origin == AccessOrigin::Explicit
            && let RamAccessOperation::Read { width } = request.operation
            && (request.address.get() & 0xfff) + u64::from(width.bytes()) <= 4096
            && let TrackedAccessError::OperandPhysical {
                address,
                source: crate::MemoryAccessError::UnreadableRegion(crate::MemoryRegionKind::Rom),
            } = &error
        {
            let mut bytes = [0; 8];
            if self
                .memory
                .load_bytes(*address, &mut bytes[..usize::from(width.bytes())])
                .is_ok()
            {
                return Ok(TrackedAccessOutcome::Read(u64::from_le_bytes(bytes)));
            }
        }
        let error = if matches!(error, TrackedAccessError::CrossesPage { .. })
            && self.firmware_boot_profile
            && !self.large_ram_profile
            && request.context.origin == AccessOrigin::Explicit
        {
            let split = match request.operation {
                RamAccessOperation::Read { .. } => {
                    crate::page_walk::read_long64_4k_bounded_ram_read(
                        self.memory,
                        &self.paging,
                        request,
                        &mut *self.committed_write,
                    )
                    .map(TrackedAccessOutcome::Read)
                }
                RamAccessOperation::Write { .. } => crate::page_walk::write_long64_4k_bounded_ram(
                    self.memory,
                    &self.paging,
                    request,
                    &mut *self.committed_write,
                )
                .map(|()| TrackedAccessOutcome::Written),
                _ => Err(error),
            };
            match split {
                Ok(outcome) => return Ok(outcome),
                Err(error) => error,
            }
        } else {
            error
        };
        match error {
            TrackedAccessError::PageFault { record, .. } => {
                self.page_fault = Some(record.clone());
                Err(MemoryError::Architectural(record.fault().clone()))
            }
            error => {
                let memory_error =
                    MemoryError::Backend(format!("tracked RAM access failed: {error:?}"));
                self.memory_error = Some(error);
                Err(memory_error)
            }
        }
    }
}

// The tracked walker checks that the entire operand is inside the device aperture.
fn access_device_registers(
    name: &str,
    offset: u64,
    operation: RamAccessOperation,
    read: impl FnOnce(DeviceAccess) -> Result<u64, DeviceError>,
    write: impl FnOnce(DeviceAccess, u64) -> Result<(), DeviceError>,
) -> Result<u64, MmioDispatchError> {
    let access = |width| DeviceAccess {
        space: IoSpace::Mmio,
        offset,
        width,
    };
    let result = match operation {
        RamAccessOperation::Read { width } => read(access(width)),
        RamAccessOperation::Write { width, value } => write(access(width), value).map(|()| 0),
        _ => {
            return Err(MmioDispatchError::Backend(format!(
                "{name} accepts only plain data reads and writes"
            )));
        }
    };
    result.map_err(|error| {
        MmioDispatchError::Backend(format!("{name} access at {offset:#x}: {error:?}"))
    })
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
            1 => crate::AccessWidth::Byte,
            2 => crate::AccessWidth::Word,
            4 => crate::AccessWidth::Dword,
            8 => crate::AccessWidth::Qword,
            _ => {
                return Err(MemoryError::Backend(
                    "paged probe only supports byte, word, dword, and qword reads".into(),
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
        let width = match source.len() {
            1 => crate::AccessWidth::Byte,
            2 => crate::AccessWidth::Word,
            4 => crate::AccessWidth::Dword,
            8 => crate::AccessWidth::Qword,
            _ => {
                return Err(MemoryError::Backend(
                    "paged probe only supports byte, word, dword, and qword writes".into(),
                ));
            }
        };
        let mut bytes = [0; 8];
        bytes[..source.len()].copy_from_slice(source);
        let value = u64::from_le_bytes(bytes);
        match self.access(address, context, RamAccessOperation::Write { width, value })? {
            TrackedAccessOutcome::Written => Ok(()),
            _ => Err(MemoryError::Backend(
                "paged probe write returned a non-write result".into(),
            )),
        }
    }

    fn read_for_write_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: od_isa::OperandSize,
    ) -> Result<u64, MemoryError> {
        let width = match size {
            od_isa::OperandSize::Byte => crate::AccessWidth::Byte,
            od_isa::OperandSize::Word => crate::AccessWidth::Word,
            od_isa::OperandSize::Dword => crate::AccessWidth::Dword,
            od_isa::OperandSize::Qword => crate::AccessWidth::Qword,
        };
        match self.access(address, context, RamAccessOperation::ReadForWrite { width })? {
            TrackedAccessOutcome::Read(value) => Ok(value),
            _ => Err(MemoryError::Backend(
                "paged probe write-intent read returned a non-read result".into(),
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
