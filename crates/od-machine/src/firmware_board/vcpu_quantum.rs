//! The long-mode quantum of any vCPU of the board (S6-16d).
//!
//! [`LoadedFirmwareBoard::run_vcpu_set_quantum`] is the host call of the scheduler of S6-15 for a
//! vCPU in long mode: one call of the CPL0-set loop
//! ([`run_cpl0_set_paged_2m_ram_with_cache`], S6-16a) for the vCPU `cpu`, on what the vCPU itself
//! owns and not on what the Linux boot loop of the boot processor owns:
//!
//! * `CR3` is `Vcpu::state.cr3` (an application processor builds its own tables; the board's
//!   `page_table_root` is not read and not written) and `EFER` is `Vcpu::guest_efer`, not the
//!   constant [`FIRMWARE_EFER`];
//! * the MMIO accesses of the loop reach the Local APIC of this vCPU (`vcpu` index), and the
//!   boundary hook is a [`LapicIrqSource`] of the same Local APIC, so a Fixed vector waiting in its
//!   IRR is delivered through this vCPU's IDT and acknowledged in this LAPIC only. The boot
//!   processor (`BOOT_VCPU_INDEX`) has in front of it the interrupt source of the board (PIC, IOAPIC,
//!   PIT: [`BoardIrqSource`] of `irq_source`), asked as the Linux loop asks it (S6-21 step 1); no
//!   other vCPU is offered the board's source;
//! * a call that ends inside a `REP` ([`Vcpu::skip_first_boundary`]) makes the next call of this
//!   vCPU skip the delivery at its first boundary;
//! * the call ends, after the instruction, on a write of the guest to THR of COM1 (the bytes that
//!   completed go to `com1_log`, through the function the Linux loop uses), on a write to the timer
//!   registers of its own Local APIC (`0x320`, `0x380`, `0x3E0`: the new deadline may be nearer
//!   than the rest of the quantum), and on a PM-timer or PIT-port read (their device time is the
//!   call's `device_ns`).
//!
//! The call never touches the clock: not `charge_retired`, not `charge_ns`, not `idle_to`. It
//! returns what it retired and the device nanoseconds, and the scheduler charges them (D7, the
//! condition (c) of the plan S6-16 6). It has no fetch cache.
//!
//! Not here: the NVMe doorbell, the PM stop request and the RDTSC source (the guest of an
//! application processor has no time source: `RDTSC` is not an instruction of the quantum), the
//! checkpoint recorder and the observation collectors: they belong to the Linux boot loop of the
//! boot processor, which this call does not replace. The MSR and FP cells of the vCPU are the
//! caller's: the `set` it hands in is built over the cells of `cpu` (loaded from the `Vcpu::guest_*`
//! fields before the call and stored back after it, as `linux_direct` does for the boot processor).

use super::{
    AccessContext, AccessOrigin, BOOT_VCPU_INDEX, FirmwareBoardError, INTEGER_FLAGS,
    InstructionMode, IrqCounters, LoadedFirmwareBoard, PagedCpl0SetExit, PrivilegeLevel, RunExit,
    set_exit_to_probe_exit,
};
use crate::bus::SplitLockGate;
use crate::irq_source::{BoardIrqSource, IrqBoundaryHook};
use crate::lapic_irq_source::LapicIrqSource;
use crate::paged_memory_adapter::{
    ExceptionDeliveryMode, LoopBus, PagedCpl0SetReport, run_cpl0_set_loop,
};
use crate::smp_scheduler::Executed;
use crate::uart_ioapic_irq_source::UartIoapicIrqSource;
use crate::uart_pic_irq_source::UartPicIrqSource;
use crate::vcpu::Vcpu;
use crate::{AcpiPmHandle, LocalApicHandle, MachineBus, Pit8254Handle, Uart16550Handle};
use od_isa::execute::Cpl0ProfileSet;
use std::cell::Cell;
use std::sync::{Mutex, PoisonError};

/// The source of the boot processor in the quantum of the scheduler (S6-21 step 1): the interrupt
/// source of the board first (the line of the PIC or of the IOAPIC, as the Linux loop asks it), then
/// the vectors waiting in the processor's own Local APIC. A source that already reports the Local
/// APIC's vectors (the IOAPIC source) answers first, so the Local APIC is asked only for what the
/// board's source does not have. Only the boot processor gets it; an application processor keeps the
/// [`LapicIrqSource`] alone.
pub(crate) struct BootProcessorSource<'a> {
    board: &'a mut dyn BoardIrqSource,
    lapic: LapicIrqSource,
    selected_board_vector: Cell<Option<u8>>,
}

impl<'a> BootProcessorSource<'a> {
    pub(crate) fn new(board: &'a mut dyn BoardIrqSource, lapic: LapicIrqSource) -> Self {
        Self {
            board,
            lapic,
            selected_board_vector: Cell::new(None),
        }
    }
}

impl BoardIrqSource for BootProcessorSource<'_> {
    fn pending_vector(&self) -> Option<u8> {
        let board_vector = self.board.pending_vector();
        self.selected_board_vector.set(board_vector);
        board_vector.or_else(|| self.lapic.pending_vector())
    }

    fn acknowledge(&mut self, vector: u8) {
        // The guest frame is already committed. A later request can change
        // the board's answer, but the original source still owns this delivery.
        if self.selected_board_vector.take() == Some(vector) {
            self.board.acknowledge(vector);
        } else {
            self.lapic.acknowledge(vector);
        }
    }

    fn next_deadline_ns(&self) -> Option<u64> {
        self.board.next_deadline_ns()
    }

    fn pit_write_ends_quantum(&self) -> bool {
        self.board.pit_write_ends_quantum()
    }

    fn irq0_edges(&self) -> u64 {
        self.board.irq0_edges()
    }

    fn wake_ns(&self) -> Option<u64> {
        self.board.wake_ns()
    }
}

/// What the quantum of one vCPU needs besides the vCPU and the machine it runs on (S6-34).
struct QuantumEnv<'a> {
    lapic: LocalApicHandle,
    uart: Uart16550Handle,
    acpi_pm: Option<AcpiPmHandle>,
    pit: Option<Pit8254Handle>,
    /// The interrupt source of the board, offered to the boot processor only.
    board_source: &'a mut dyn BoardIrqSource,
    /// The gate of the split `LOCK` of a vCPU thread of the parallel mode.
    gate: Option<&'a dyn SplitLockGate>,
}

/// What a vCPU has to have before a quantum can run (checked before anything changes).
fn quantum_inputs(vcpu: &Vcpu) -> Result<(u64, u64, bool), FirmwareBoardError> {
    if vcpu.legacy.is_some() {
        return Err(FirmwareBoardError::Config(
            "vcpu quantum: the vCPU is in a legacy mode",
        ));
    }
    let cr3 = vcpu.state.cr3.ok_or(FirmwareBoardError::Config(
        "vcpu quantum: the vCPU has no CR3",
    ))?;
    Ok((cr3, vcpu.guest_efer, vcpu.skip_first_boundary))
}

/// The call of the CPL0-set loop of one vCPU (see
/// [`LoadedFirmwareBoard::run_vcpu_set_quantum`]): CR2 in and out, the interrupt source of the
/// vCPU, the loop over `bus`, and what the call leaves in the vCPU (CR2, CR3, the REP restart
/// mark). The caller does the rest: the ticks and the transmit pump around it, the result.
#[allow(clippy::too_many_arguments)]
fn quantum_loop(
    vcpu: &mut Vcpu,
    cpu: usize,
    bus: LoopBus<'_>,
    env: QuantumEnv<'_>,
    (cr3, efer, skip_first_boundary): (u64, u64, bool),
    budget: u64,
    stop: &mut dyn FnMut() -> bool,
    set: &Cpl0ProfileSet<'_>,
    exception_delivery: ExceptionDeliveryMode<'_>,
) -> PagedCpl0SetReport {
    let QuantumEnv {
        lapic,
        uart,
        acpi_pm,
        pit,
        board_source,
        gate,
    } = env;
    // CR2 is a cell of the vCPU between calls and the state's during the call (S4-38L5c).
    let prior_cr2 = vcpu.state.cr2;
    vcpu.state.cr2 = Some(vcpu.guest_cr2.or(prior_cr2).unwrap_or(0));
    let set = set.with_cpuid_topology(vcpu.cpuid.apic_id, vcpu.cpuid.cpu_count);

    let timer_writes = lapic.timer_register_writes();
    let mut own = LapicIrqSource::new(lapic.clone());
    let mut boot_source;
    let source: &mut dyn BoardIrqSource = if cpu == BOOT_VCPU_INDEX {
        boot_source = BootProcessorSource::new(board_source, own.clone());
        &mut boot_source
    } else {
        &mut own
    };
    let mut counters = IrqCounters::default();
    let report = run_cpl0_set_loop(
        &mut vcpu.state,
        bus,
        gate,
        cpu,
        cr3,
        efer,
        AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        },
        budget,
        INTEGER_FLAGS,
        &set,
        None,
        || {
            stop()
                || uart.holding_occupied()
                || lapic.timer_register_writes() != timer_writes
                || acpi_pm.as_ref().is_some_and(|pm| pm.timer_tick_pending())
                || pit.as_ref().is_some_and(|pit| pit.port_tick_pending())
        },
        |_| {},
        false,
        None,
        Some(IrqBoundaryHook {
            source,
            delivered_mirror: None,
            counters: &mut counters,
            skip_first_boundary,
        }),
        None,
        None,
        exception_delivery,
    );

    vcpu.guest_cr2 = vcpu.state.cr2;
    vcpu.state.cr2 = prior_cr2;
    // The CR3 the guest left (`MOV CR3` of its own tables) is the next call's.
    vcpu.state.cr3 = Some(report.cr3);
    vcpu.skip_first_boundary = matches!(
        report.exit,
        PagedCpl0SetExit::Run(RunExit::RepIterationBudgetExhausted)
    );
    report
}

/// The result of a quantum from its report, after the pump and the device ticks: an exit of a
/// quantum is benign (the budget, a halt, STOP, a REP restart) or an error that keeps the state.
fn quantum_result(
    report: PagedCpl0SetReport,
    cpu: usize,
    vcpu: &Vcpu,
    device_ns: u64,
) -> Result<Executed, FirmwareBoardError> {
    let partial_rep = matches!(
        report.exit,
        PagedCpl0SetExit::Run(RunExit::RepIterationBudgetExhausted)
    );
    let benign = matches!(
        report.exit,
        PagedCpl0SetExit::Run(
            RunExit::Halted
                | RunExit::InstructionBudgetExhausted
                | RunExit::StopRequested
                | RunExit::RepIterationBudgetExhausted
        )
    );
    if !benign {
        let (page_fault, memory_error) = (report.page_fault, report.memory_error);
        return match set_exit_to_probe_exit(report.exit) {
            Err(error) => Err(error.into_board_error(&vcpu.state)),
            Ok((_, text)) => Err(FirmwareBoardError::VcpuQuantum {
                cpu,
                detail: format!(
                    "{text}; page_fault: {page_fault:?}; memory_error: {memory_error:?}"
                ),
            }),
        };
    }
    Ok(Executed {
        retired: report.instructions_executed,
        partial_rep,
        device_ns,
    })
}

impl LoadedFirmwareBoard {
    /// Run vCPU `cpu` (in long mode, `Running`) for at most `budget` steps (at least 1) with the
    /// CPL0 set `set`, on the RAM and devices of the board. `stop` is polled between steps.
    ///
    /// Ends on a boundary: the budget is used up, `stop()` or a device condition above answered
    /// true, the vCPU halted (`HLT`, its `state.activity` is then `Halted`), or a `REP` made
    /// progress without finishing (`Executed::partial_rep`). Any other exit of the loop (a fault, a
    /// page fault the guest IDT did not take, an unsupported instruction, a refused configuration, an
    /// interrupt that could not be delivered) is an error that keeps the state the call left:
    /// [`FirmwareBoardError::VcpuQuantum`], or `ExternalInterruptDelivery` for the last.
    ///
    /// Before the call the vCPU must have a CR3 of its own (`state.cr3`, `Config` otherwise) and no
    /// legacy model (`legacy` is `None`, `Config` otherwise: the trampoline runs in
    /// [`crate::ap_legacy_exec`]).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn run_vcpu_set_quantum(
        &mut self,
        cpu: usize,
        budget: u64,
        stop: &mut dyn FnMut() -> bool,
        set: &Cpl0ProfileSet<'_>,
    ) -> Result<Executed, FirmwareBoardError> {
        let lapic = self
            .lapic
            .for_cpu(cpu)
            .ok_or(FirmwareBoardError::Config("vcpu quantum: no such vCPU"))?;
        let vcpu = self
            .vcpus
            .get(cpu)
            .ok_or(FirmwareBoardError::Config("vcpu quantum: no such vCPU"))?;
        let inputs = quantum_inputs(vcpu)?;

        let uart = self.uart.clone();
        let acpi_pm = self.acpi_pm.clone();
        let pit = self.pit.clone();
        // Ticks queued before the call came from the host, not from this guest (as in the Linux
        // loop); a THR byte left by another vCPU is moved to the log first, so that the call does
        // not end at once on a state it did not make.
        if let Some(pm) = &acpi_pm {
            pm.take_timer_tick_ns();
        }
        if let Some(pit) = &pit {
            pit.take_port_tick_ns();
        }
        self.pump_uart_transmit(0);

        let report = quantum_loop(
            &mut self.vcpus[cpu],
            cpu,
            LoopBus::Exclusive(&mut self.bus),
            QuantumEnv {
                lapic,
                uart,
                acpi_pm,
                pit,
                board_source: &mut *self.irq_source,
                gate: None,
            },
            inputs,
            budget,
            stop,
            set,
            ExceptionDeliveryMode::Quantum,
        );

        self.pump_uart_transmit(report.instructions_executed);
        let mut device_ns = 0u64;
        if let Some(pm) = &self.acpi_pm {
            device_ns += pm.take_timer_tick_ns();
        }
        if let Some(pit) = &self.pit {
            device_ns += pit.take_port_tick_ns();
        }
        quantum_result(report, cpu, &self.vcpus[cpu], device_ns)
    }
}

/// What a vCPU thread of the parallel mode (S6-34) runs a quantum on: the machine by shared
/// references, the vCPU it owns, the log of COM1 behind its lock. Built by
/// [`LoadedFirmwareBoard::thread_parts`].
pub(crate) struct ThreadQuantumEnv<'a> {
    pub(crate) bus: &'a MachineBus,
    pub(crate) uart: Uart16550Handle,
    pub(crate) acpi_pm: Option<AcpiPmHandle>,
    pub(crate) pit: Option<Pit8254Handle>,
    pub(crate) nvme: Option<crate::NvmeHandle>,
    pub(crate) uart_irq_source: Option<&'a UartPicIrqSource>,
    pub(crate) uart_ioapic_source: Option<&'a UartIoapicIrqSource>,
    /// The COM1 log. The lock is taken for the whole pump (it is above the UART lock: the pump
    /// takes the UART's lock under it, nothing takes this one under the UART's).
    pub(crate) com1: &'a Mutex<&'a mut Vec<u8>>,
}

impl ThreadQuantumEnv<'_> {
    /// Take the outstanding device-time requests from the shared board.
    pub(crate) fn take_device_ticks(&self) -> u64 {
        let pm_ns = self
            .acpi_pm
            .as_ref()
            .map_or(0, AcpiPmHandle::take_timer_tick_ns);
        let pit_ns = self
            .pit
            .as_ref()
            .map_or(0, Pit8254Handle::take_port_tick_ns);
        pm_ns.saturating_add(pit_ns)
    }

    /// [`LoadedFirmwareBoard::pump_uart_transmit`] of the threads: every byte the guest left in
    /// the holding register goes to the log, in the order they complete, with the log's lock held
    /// so that two threads never interleave one byte's two steps.
    pub(crate) fn pump_uart_transmit(&self) {
        let mut log = self.com1.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(source) = self.uart_irq_source {
            source.sample();
        }
        if let Some(source) = self.uart_ioapic_source {
            source.sample();
        }
        while self.uart.transfer_holding_to_shift() {
            if let Some(byte) = self.uart.complete_transmission() {
                log.push(byte);
            }
        }
        if let Some(source) = self.uart_irq_source {
            source.sample();
        }
        if let Some(source) = self.uart_ioapic_source {
            source.sample();
        }
    }
}

/// [`LoadedFirmwareBoard::run_vcpu_set_quantum`] for a vCPU thread of the parallel mode (S6-34):
/// the same call of the CPL0-set loop, on the shared machine, for the vCPU the thread owns.
/// `board_source` is the interrupt source of the board for the boot processor; it is not asked
/// for any other vCPU. `gate` is the stop-all of the split `LOCK`. Nothing else differs: the
/// device lock of the bus is taken for each device access and not across an instruction.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_vcpu_set_quantum_shared(
    env: &ThreadQuantumEnv<'_>,
    vcpu: &mut Vcpu,
    cpu: usize,
    lapic: &LocalApicHandle,
    board_source: &mut dyn BoardIrqSource,
    gate: Option<&dyn SplitLockGate>,
    budget: u64,
    stop: &mut dyn FnMut() -> bool,
    set: &Cpl0ProfileSet<'_>,
    consecutive_faults: &mut u64,
) -> Result<Executed, FirmwareBoardError> {
    let inputs = quantum_inputs(vcpu)?;
    env.pump_uart_transmit();

    let report = quantum_loop(
        vcpu,
        cpu,
        LoopBus::Shared(env.bus),
        QuantumEnv {
            lapic: lapic.clone(),
            uart: env.uart.clone(),
            acpi_pm: env.acpi_pm.clone(),
            pit: env.pit.clone(),
            board_source,
            gate,
        },
        inputs,
        budget,
        stop,
        set,
        ExceptionDeliveryMode::Thread(consecutive_faults),
    );

    env.pump_uart_transmit();
    // A completed quantum takes pending board requests for the clock owner.
    // A peer may have queued them before this call.
    quantum_result(report, cpu, vcpu, env.take_device_ticks())
}

/// The CPL0 interpreter for a litmus probe with no board-device or IRQ polling.
pub(crate) fn run_litmus_ram_quantum(
    bus: &MachineBus,
    vcpu: &mut Vcpu,
    cpu: usize,
    gate: &dyn SplitLockGate,
    budget: u64,
    stop: &mut dyn FnMut() -> bool,
    set: &Cpl0ProfileSet<'_>,
) -> Result<Executed, FirmwareBoardError> {
    let (cr3, efer, _) = quantum_inputs(vcpu)?;
    let report = run_cpl0_set_loop(
        &mut vcpu.state,
        LoopBus::Shared(bus),
        Some(gate),
        cpu,
        cr3,
        efer,
        AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        },
        budget,
        INTEGER_FLAGS,
        set,
        None,
        stop,
        |_| {},
        false,
        None,
        None,
        None,
        None,
        ExceptionDeliveryMode::Quantum,
    );
    quantum_result(report, cpu, vcpu, 0)
}

#[cfg(test)]
/// The machine and the guests the tests of this module and of the SIPI path
/// (`ap_legacy_exec`) run on.
pub(crate) mod fixture {
    use super::super::*;
    use crate::smp_scheduler::{
        BoardSmpHost, Executed, Finished, QuantumPolicy, SmpScheduler, StopRequest, VcpuCounters,
    };
    use crate::{
        FirmwareBoardConfig, GuestPhysicalAddress, LOCAL_APIC_MMIO_BASE, PciEcam,
        firmware_board::{FIRMWARE_CR0, FIRMWARE_CR4, assemble_board},
    };
    use od_isa::execute::{ControlRegisterPolicy, MovCr3Long64V1};
    use od_isa::{
        CliProfile, CodeSegmentDescriptorCache, CpuActivity, HaltProfile, LongModeState, Register,
        StackSegmentDescriptorCache, StiProfile, StiShadow,
    };

    // ---- the machine the tests run on ------------------------------------------------------------
    //
    // Physical memory below 2 MiB is identity-mapped by a 2 MiB page of every table set; the
    // Local APIC window is a 4 KiB page of its own. The board's own `page_table_root` (0x1000) is
    // never written, so a quantum that used it would fault at its first fetch.

    pub(crate) const BOARD_ROOT: u64 = 0x1000;
    pub(crate) const TABLES_0: u64 = 0x1_0000;
    pub(crate) const TABLES_1: u64 = 0x1_8000;
    pub(crate) const CODE_0: u64 = 0x2_0000;
    pub(crate) const CODE_1: u64 = 0x2_1000;
    pub(crate) const HANDLER_0: u64 = 0x2_3000;
    pub(crate) const HANDLER_1: u64 = 0x2_2000;
    pub(crate) const IDT_AT: u64 = 0x3_0000;
    pub(crate) const GDT_AT: u64 = 0x3_1000;
    pub(crate) const COUNTER_0: u64 = 0x4_0000;
    pub(crate) const COUNTER_1: u64 = 0x4_0004;
    pub(crate) const FLAG: u64 = 0x4_0010;
    pub(crate) const DATA: u64 = 0x4_1000;
    pub(crate) const STACK_0: u64 = 0x6_0000;
    pub(crate) const STACK_1: u64 = 0x7_0000;
    pub(crate) const TO_AP: u8 = 0x30;
    pub(crate) const TO_BSP: u8 = 0x31;
    /// ICR_LOW bits of a Fixed IPI to the destination of ICR_HIGH besides the vector: delivery mode
    /// 000, physical destination mode, no shorthand, edge (the matrix of SMP_CONTRACT.md 5 takes
    /// Fixed without a shorthand only with bits 15:14 clear).
    pub(crate) const FIXED: u32 = 0;
    /// The exchanges the probe of S6-16f will run; here the same.
    pub(crate) const ROUNDS: u32 = 3;

    pub(crate) fn board(vcpus: usize) -> LoadedFirmwareBoard {
        let config = FirmwareBoardConfig {
            vcpu_count: vcpus,
            pci_ecam: true,
            acpi_pm: true,
            acpi_tables: true,
            linux_profile: true,
            ..FirmwareBoardConfig::default()
        };
        let mut board = assemble_board(&config, PciEcam::new(), None, None)
            .unwrap()
            .into_linux_board(BOARD_ROOT, 0x1000)
            .unwrap();
        board.bus_mut().enable_apic_mmio_windows();
        board.enable_clock_owner().unwrap();
        board
    }

    pub(crate) fn patch(board: &mut LoadedFirmwareBoard, at: u64, bytes: &[u8]) {
        board
            .bus_mut()
            .patch_physical_ram(GuestPhysicalAddress::new(at), bytes)
            .unwrap();
    }

    pub(crate) fn read(board: &mut LoadedFirmwareBoard, at: u64, len: usize) -> Vec<u8> {
        let mut bytes = vec![0; len];
        let (memory, _) = board.bus_mut().split_for_pio_step();
        memory
            .load_bytes(GuestPhysicalAddress::new(at), &mut bytes)
            .unwrap();
        bytes
    }

    pub(crate) fn dword(board: &mut LoadedFirmwareBoard, at: u64) -> u32 {
        u32::from_le_bytes(read(board, at, 4).try_into().unwrap())
    }

    /// A four-level table set at `base`: the first 2 MiB identity, the Local APIC window, and the
    /// linear 0x40_0000 (a 2 MiB page) at `data_page` with `extra` in the entry (for example NX).
    pub(crate) fn tables(
        board: &mut LoadedFirmwareBoard,
        base: u64,
        data_page: u64,
        extra: u64,
    ) -> u64 {
        let (pml4, pdpt, pd, pd_l, pt_l) = (
            base,
            base + 0x1000,
            base + 0x2000,
            base + 0x3000,
            base + 0x4000,
        );
        let entry = |value: u64| value.to_le_bytes();
        patch(board, pml4, &entry(pdpt | 3));
        patch(board, pdpt, &entry(pd | 3));
        patch(board, pdpt + 3 * 8, &entry(pd_l | 3));
        patch(board, pd, &entry(0x83));
        patch(board, pd + 2 * 8, &entry(data_page | 0x83 | extra));
        patch(board, pd_l + 0x1F7 * 8, &entry(pt_l | 3));
        patch(board, pt_l, &entry(LOCAL_APIC_MMIO_BASE | 0x13));
        pml4
    }

    /// The GDT (null, 64-bit ring-0 code 0x08, ring-0 data 0x10), the IDT gates of both vectors
    /// and the two handlers.
    pub(crate) fn idt(board: &mut LoadedFirmwareBoard, at: u64, selector: u16) {
        for (vector, handler) in [(TO_AP, HANDLER_1), (TO_BSP, HANDLER_0)] {
            let mut gate = [0u8; 16];
            gate[0..2].copy_from_slice(&(handler as u16).to_le_bytes());
            gate[2..4].copy_from_slice(&selector.to_le_bytes());
            gate[5] = 0x8E; // present, DPL 0, 64-bit interrupt gate
            gate[6..8].copy_from_slice(&((handler >> 16) as u16).to_le_bytes());
            patch(board, at + u64::from(vector) * 16, &gate);
        }
    }

    pub(crate) fn system(board: &mut LoadedFirmwareBoard, reply: bool) {
        let mut gdt = [0u8; 24];
        gdt[8..16].copy_from_slice(&0x00AF_9B00_0000_FFFFu64.to_le_bytes());
        gdt[16..24].copy_from_slice(&0x00CF_9300_0000_FFFFu64.to_le_bytes());
        patch(board, GDT_AT, &gdt);
        idt(board, IDT_AT, 0x08);
        patch(board, HANDLER_1, &handler_1(reply));
        patch(board, HANDLER_0, &handler_0());
    }

    /// A processor in 64-bit mode at CPL0 with IF = 1 on its own tables. `idt` loads the IDT.
    pub(crate) fn long_state(rip: u64, rsp: u64, cr3: u64, idt: bool) -> LongModeState {
        let mut state = LongModeState {
            rip,
            rflags: 2 | (1 << 9),
            cr0: Some(FIRMWARE_CR0),
            cr4: Some(FIRMWARE_CR4),
            cr3: Some(cr3),
            cs_selector: Some(0x08),
            cs_cache: Some(CodeSegmentDescriptorCache {
                base: 0,
                limit: u32::MAX,
                access: 0x9B,
                flags: 2,
            }),
            ss_selector: Some(0x10),
            ss_cache: Some(StackSegmentDescriptorCache {
                base: 0,
                limit: u32::MAX,
                access: 0x93,
                flags: 0,
            }),
            gdtr: Some(od_isa::state::Gdtr {
                base: GDT_AT,
                limit: 0x17,
            }),
            sti_shadow: Some(StiShadow::Clear),
            activity: Some(CpuActivity::Running),
            ..LongModeState::default()
        };
        state.gpr[Register::Rsp as usize] = rsp;
        if idt {
            state.idtr = Some(od_isa::state::Idtr {
                base: IDT_AT,
                limit: 0xFFF,
            });
        }
        state
    }

    /// Make vCPU `cpu` a long-mode processor (the harness of the unit tests, SMP plan 4.4: it
    /// proves nothing about starting an AP, which S6-16c and S6-16f do with INIT and SIPI).
    pub(crate) fn install(
        board: &mut LoadedFirmwareBoard,
        cpu: usize,
        state: LongModeState,
        efer: u64,
    ) {
        let vcpu = board.vcpu_mut(cpu).unwrap();
        vcpu.state = state;
        vcpu.guest_efer = efer;
        vcpu.legacy = None;
    }

    pub(crate) fn quantum(
        board: &mut LoadedFirmwareBoard,
        cpu: usize,
        budget: u64,
    ) -> Result<Executed, FirmwareBoardError> {
        with_set(|set| board.run_vcpu_set_quantum(cpu, budget, &mut || false, set))
    }

    pub(crate) fn with_set<R>(f: impl FnOnce(&Cpl0ProfileSet<'_>) -> R) -> R {
        let (cli, sti, halt) = (
            CliProfile::NonPviLong64V1,
            StiProfile::NonPviLong64V1,
            HaltProfile::Long64V1,
        );
        let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
            .with_cli(&cli)
            .with_cpu_owned_cr3(MovCr3Long64V1)
            .with_pio_u8()
            .with_halt(&halt)
            .with_sti(&sti);
        f(&set)
    }

    // ---- code ------------------------------------------------------------------------------------

    pub(crate) fn imm(opcode: &[u8], value: u32) -> Vec<u8> {
        let mut code = opcode.to_vec();
        code.extend(value.to_le_bytes());
        code
    }

    /// `MOV EBX, imm32`.
    pub(crate) fn mov_ebx(value: u32) -> Vec<u8> {
        imm(&[0xBB], value)
    }

    /// `MOV EAX, imm32`.
    pub(crate) fn mov_eax(value: u32) -> Vec<u8> {
        imm(&[0xB8], value)
    }

    /// `MOV [RBX + disp32], EAX`.
    pub(crate) fn store_eax(offset: u32) -> Vec<u8> {
        imm(&[0x89, 0x83], offset)
    }

    pub(crate) const HLT_LOOP: [u8; 3] = [0xF4, 0xEB, 0xFD]; // hlt; jmp -3 (back to the hlt)

    /// Enable the Local APIC of the running processor (SVR bit 8, spurious vector 0xFF).
    pub(crate) fn enable_lapic() -> Vec<u8> {
        [
            mov_ebx(LOCAL_APIC_MMIO_BASE as u32),
            mov_eax(0x1FF),
            store_eax(0xF0),
        ]
        .concat()
    }

    /// The application processor: enable the LAPIC, raise the flag, sleep.
    pub(crate) fn ap_program(sti: bool) -> Vec<u8> {
        let mut code = enable_lapic();
        code.extend(imm(&[0xB9], FLAG as u32)); // mov ecx, FLAG
        code.extend(mov_eax(1));
        code.extend([0x89, 0x01]); // mov [rcx], eax
        if sti {
            code.push(0xFB); // sti: an AP that came out of the trampoline has IF = 0
        }
        code.extend(HLT_LOOP);
        code
    }

    /// The boot processor: enable the LAPIC, wait for the flag, send the first IPI, sleep.
    pub(crate) fn bsp_program() -> Vec<u8> {
        let mut code = enable_lapic();
        code.extend(imm(&[0xB9], FLAG as u32)); // mov ecx, FLAG
        code.extend([0x8B, 0x01, 0x85, 0xC0, 0x74, 0xFA]); // wait: mov eax,[rcx]; test; jz wait
        code.extend(send_ipi_from_ebx(TO_AP, 1));
        code.extend(HLT_LOOP);
        code
    }

    /// ICR_HIGH = `destination << 24`, then ICR_LOW = Fixed `vector` (EBX holds the LAPIC base).
    pub(crate) fn send_ipi_from_ebx(vector: u8, destination: u32) -> Vec<u8> {
        [
            mov_eax(destination << 24),
            imm(&[0x89, 0x83], 0x310),
            mov_eax(FIXED | u32::from(vector)),
            store_eax(0x300),
        ]
        .concat()
    }

    /// The same with R8 as the LAPIC base and R10 as the register (the handlers use r8..r10 only,
    /// the programs do not).
    pub(crate) fn send_ipi_from_r8(vector: u8, destination: u32) -> Vec<u8> {
        [
            imm(&[0x41, 0xBA], destination << 24), // mov r10d, imm32
            imm(&[0x45, 0x89, 0x90], 0x310),       // mov [r8+0x310], r10d
            imm(&[0x41, 0xBA], FIXED | u32::from(vector)),
            imm(&[0x45, 0x89, 0x90], 0x300),
        ]
        .concat()
    }

    /// `counter += 1` (R9D holds the new value afterwards) and the EOI (R8 the LAPIC base).
    pub(crate) fn count_and_eoi(counter: u64) -> Vec<u8> {
        [
            imm(&[0x41, 0xB8], counter as u32), // mov r8d, counter
            vec![0x45, 0x8B, 0x08],             // mov r9d, [r8]
            vec![0x41, 0xFF, 0xC1],             // inc r9d
            vec![0x45, 0x89, 0x08],             // mov [r8], r9d
            imm(&[0x41, 0xB8], LOCAL_APIC_MMIO_BASE as u32),
            vec![0x45, 0x31, 0xD2],         // xor r10d, r10d
            imm(&[0x45, 0x89, 0x90], 0xB0), // mov [r8+0xB0], r10d  (EOI)
        ]
        .concat()
    }

    /// The handler of the vector sent to the AP: count, EOI, and (with `reply`) the IPI back to
    /// the boot processor.
    pub(crate) fn handler_1(reply: bool) -> Vec<u8> {
        let mut code = count_and_eoi(COUNTER_1);
        if reply {
            code.extend(send_ipi_from_r8(TO_BSP, 0));
        }
        code.extend([0x48, 0xCF]); // iretq
        code
    }

    /// The handler of the vector sent to the boot processor: count, EOI, and the next IPI to the
    /// AP while fewer than `ROUNDS` replies came.
    pub(crate) fn handler_0() -> Vec<u8> {
        let mut code = count_and_eoi(COUNTER_0);
        let send = send_ipi_from_r8(TO_AP, 1);
        code.extend([0x41, 0x83, 0xF9, ROUNDS as u8]); // cmp r9d, ROUNDS
        code.extend([0x73, send.len() as u8]); // jae over the send
        code.extend(send);
        code.extend([0x48, 0xCF]);
        code
    }

    /// Two vCPUs on tables of their own, the exchange programs in RAM.
    pub(crate) fn exchange_board() -> LoadedFirmwareBoard {
        let mut board = board(2);
        system(&mut board, true);
        patch(&mut board, CODE_0, &bsp_program());
        patch(&mut board, CODE_1, &ap_program(false));
        let (cr3_0, cr3_1) = (
            tables(&mut board, TABLES_0, 0x60_0000, 0),
            tables(&mut board, TABLES_1, 0x80_0000, 0),
        );
        install(
            &mut board,
            0,
            long_state(CODE_0, STACK_0, cr3_0, true),
            0xD01,
        );
        install(
            &mut board,
            1,
            long_state(CODE_1, STACK_1, cr3_1, true),
            0xD01,
        );
        board
    }

    pub(crate) fn run_scheduler(
        board: &mut LoadedFirmwareBoard,
        policy: QuantumPolicy,
        budget: u64,
    ) -> (Finished, Vec<VcpuCounters>, Vec<usize>) {
        let mut host = BoardSmpHost::new(
            board,
            |board: &mut LoadedFirmwareBoard, cpu, budget, stop: &mut dyn FnMut() -> bool| {
                with_set(|set| board.run_vcpu_set_quantum(cpu, budget, stop, set))
            },
        )
        .unwrap();
        let mut scheduler = SmpScheduler::new(2, policy, StopRequest::new()).unwrap();
        scheduler.record_turns();
        let finished = scheduler.run(&mut host, budget).unwrap();
        (
            finished,
            scheduler.counters().to_vec(),
            scheduler.executed_schedule(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::*;
    use super::*;
    use crate::irq_source::BoardIrqSource;
    use crate::smp_scheduler::{BoardSmpHost, Finished, QuantumPolicy, SmpScheduler, StopRequest};
    use crate::{LOCAL_APIC_MMIO_BASE, firmware_board::FIRMWARE_EFER};
    use od_isa::{CpuActivity, Register};

    fn vector_state(board: &LoadedFirmwareBoard, cpu: usize, vector: u8) -> (bool, bool) {
        board.local_apic_of(cpu).unwrap().vector_state(vector)
    }

    #[test]
    fn two_vcpus_exchange_fixed_ipis_through_their_icrs_and_end_every_one_with_an_eoi() {
        let mut board = exchange_board();
        let (finished, counters, _) = run_scheduler(&mut board, QuantumPolicy::Fixed(32), 100_000);
        // Both end asleep with nothing in flight.
        assert_eq!(finished, Finished::AllIdle);
        assert_eq!(board.vcpu(0).unwrap().activity(), CpuActivity::Halted);
        assert_eq!(board.vcpu(1).unwrap().activity(), CpuActivity::Halted);
        // The AP took ROUNDS IPIs, the boot processor ROUNDS replies (each handler counts once).
        assert_eq!(dword(&mut board, COUNTER_1), ROUNDS);
        assert_eq!(dword(&mut board, COUNTER_0), ROUNDS);
        // Each EOI cleared the in-service bit of its own LAPIC, and no request is left in an IRR.
        for cpu in 0..2 {
            for vector in [TO_AP, TO_BSP] {
                assert_eq!(
                    vector_state(&board, cpu, vector),
                    (false, false),
                    "cpu {cpu} vector {vector:#x}"
                );
            }
        }
        // Each sleeper was woken by every IPI but the first of the AP (it may not yet be asleep
        // when the first one is taken): at least ROUNDS - 1 wakes each.
        assert!(counters[0].wakes >= u64::from(ROUNDS) - 1, "{counters:?}");
        assert!(counters[1].wakes >= u64::from(ROUNDS) - 1, "{counters:?}");
    }

    #[test]
    fn a_halted_vcpu_is_woken_by_a_fixed_ipi_runs_its_handler_and_sleeps_again() {
        let mut board = board(2);
        system(&mut board, false);
        patch(&mut board, CODE_1, &ap_program(false));
        let cr3_1 = tables(&mut board, TABLES_1, 0x80_0000, 0);
        install(
            &mut board,
            1,
            long_state(CODE_1, STACK_1, cr3_1, true),
            0xD01,
        );
        // The boot processor is a sleeper that is never woken (IF = 0, halted).
        let mut sleeper = long_state(CODE_0, STACK_0, cr3_1, false);
        sleeper.rflags = 2;
        sleeper.activity = Some(CpuActivity::Halted);
        install(&mut board, 0, sleeper, 0xD01);

        let (finished, counters, _) = run_scheduler(&mut board, QuantumPolicy::default(), 10_000);
        assert_eq!(finished, Finished::AllIdle);
        assert_eq!(board.vcpu(1).unwrap().activity(), CpuActivity::Halted);
        assert_eq!(dword(&mut board, COUNTER_1), 0, "no IPI yet");
        assert_eq!(counters[1].wakes, 0);

        // The IPI of vCPU 0 to vCPU 1 (the harness writes the ICR as the sender).
        board
            .local_apic_of(0)
            .unwrap()
            .write_icr(0, 1, FIXED | u32::from(TO_AP))
            .unwrap();
        assert_eq!(board.local_apic_of(1).unwrap().eligible(), Some(TO_AP));
        let (finished, counters, _) = run_scheduler(&mut board, QuantumPolicy::default(), 10_000);
        assert_eq!(finished, Finished::AllIdle);
        assert_eq!(counters[1].wakes, 1);
        assert_eq!(dword(&mut board, COUNTER_1), 1);
        assert_eq!(vector_state(&board, 1, TO_AP), (false, false));
        assert_eq!(board.vcpu(1).unwrap().activity(), CpuActivity::Halted);
        assert!(
            board.vcpu(1).unwrap().interrupts_enabled(),
            "IRETQ restored IF"
        );
        // The handler's registers did not leak into the interrupted program.
        assert_eq!(
            board.vcpu(1).unwrap().state.gpr[Register::Rbx as usize],
            LOCAL_APIC_MMIO_BASE
        );
    }

    #[test]
    fn a_write_to_a_timer_register_of_its_own_lapic_ends_the_call_and_the_call_moves_no_clock() {
        let mut board = board(2);
        let cr3 = tables(&mut board, TABLES_1, 0x80_0000, 0);
        let mut program = enable_lapic();
        program.extend(mov_eax(0x40)); // LVT Timer: vector 0x40, unmasked, one-shot
        program.extend(store_eax(0x320));
        program.extend(mov_eax(0xB)); // divide by 1 (SDM Vol. 3A 11.5.4: 1011b)
        program.extend(store_eax(0x3E0));
        program.extend(mov_eax(1000));
        program.extend(store_eax(0x380));
        program.extend(imm(&[0xB9], 7)); // mov ecx, 7: after the last timer write
        program.extend([0xF4]);
        patch(&mut board, CODE_1, &program);
        install(
            &mut board,
            1,
            long_state(CODE_1, STACK_1, cr3, false),
            0xD01,
        );
        let lapic = board.local_apic_of(1).unwrap();
        let now = board.clock().now_ns();

        // mov ebx / mov eax / mov [SVR] (not a timer register) / mov eax / mov [LVT Timer]
        assert_eq!(quantum(&mut board, 1, 100).unwrap().retired, 5);
        assert_eq!(lapic.timer_vector(), Some(0x40));
        assert_eq!(lapic.next_timer_deadline(), None, "not armed yet");
        // mov eax / mov [divide]
        assert_eq!(quantum(&mut board, 1, 100).unwrap().retired, 2);
        // mov eax / mov [initial count]: arms the timer
        assert_eq!(quantum(&mut board, 1, 100).unwrap().retired, 2);
        let deadline = lapic.next_timer_deadline().expect("armed by the write");
        assert!(deadline > now);
        // mov ecx / hlt
        let last = quantum(&mut board, 1, 100).unwrap();
        assert_eq!(last.retired, 2);
        assert_eq!(board.vcpu(1).unwrap().activity(), CpuActivity::Halted);
        assert_eq!(board.vcpu(1).unwrap().state.gpr[Register::Rcx as usize], 7);
        // D7: the call charged nothing, the scheduler does that.
        assert_eq!(board.clock().now_ns(), now);
        assert_eq!(board.local_apic_of(0).unwrap().next_timer_deadline(), None);
    }

    /// A source whose edge is `deadline` ns.
    struct EdgeAt(u64);

    impl BoardIrqSource for EdgeAt {
        fn pending_vector(&self) -> Option<u8> {
            None
        }
        fn acknowledge(&mut self, _vector: u8) {}
        fn next_deadline_ns(&self) -> Option<u64> {
            Some(self.0)
        }
    }

    #[test]
    fn the_deadline_of_the_board_source_cuts_the_call_of_any_vcpu() {
        let spin = [0xEB, 0xFE]; // jmp -2
        let calls = |edge: Option<u64>| {
            let mut board = board(2);
            let cr3 = tables(&mut board, TABLES_1, 0x80_0000, 0);
            patch(&mut board, CODE_1, &spin);
            install(
                &mut board,
                1,
                long_state(CODE_1, STACK_1, cr3, false),
                0xD01,
            );
            // The boot processor is out of the way (halted, IF = 0).
            let mut sleeper = long_state(CODE_0, STACK_0, cr3, false);
            sleeper.rflags = 2;
            sleeper.activity = Some(CpuActivity::Halted);
            install(&mut board, 0, sleeper, 0xD01);
            if let Some(edge) = edge {
                board.irq_source = Box::new(EdgeAt(edge));
            }
            let mut budgets = Vec::new();
            {
                let mut host = BoardSmpHost::new(
                    &mut board,
                    |board: &mut LoadedFirmwareBoard,
                     cpu,
                     budget,
                     stop: &mut dyn FnMut() -> bool| {
                        budgets.push((cpu, budget));
                        with_set(|set| board.run_vcpu_set_quantum(cpu, budget, stop, set))
                    },
                )
                .unwrap();
                let mut scheduler =
                    SmpScheduler::new(2, QuantumPolicy::Fixed(1000), StopRequest::new()).unwrap();
                assert_eq!(
                    scheduler.run(&mut host, 100).unwrap(),
                    Finished::BudgetExhausted
                );
            }
            budgets
        };
        assert_eq!(calls(None)[0], (1, 100));
        // The edge is 40 ns away and one retired instruction is one ns: 40 steps, then the clock
        // is at the edge. A deadline the clock has reached is lifted by the host (S6-21 step 1: the
        // boot processor takes the edge at its own boundary), so the rest of the budget is one call.
        let cut = calls(Some(40));
        assert_eq!(cut[0], (1, 40));
        assert_eq!(cut[1], (1, 60));
    }

    #[test]
    fn one_vcpu_gives_the_result_of_a_direct_call_of_the_set_loop_under_every_policy() {
        // eax = 0; ecx = 20; do { eax += 3; ecx -= 1 } while (ecx != 0); [DATA] = eax; hlt
        let mut program = mov_eax(0);
        program.extend(imm(&[0xB9], 20));
        program.extend([0x83, 0xC0, 0x03, 0xFF, 0xC9, 0x75, 0xF9]); // add eax,3; dec ecx; jnz -7
        program.extend(mov_ebx(DATA as u32));
        program.extend([0x89, 0x03, 0xF4]); // mov [rbx], eax; hlt
        let steps = 2 + 20 * 3 + 1 + 1 + 1;
        let setup = || {
            let mut board = board(1);
            let cr3 = tables(&mut board, TABLES_0, 0x60_0000, 0);
            patch(&mut board, CODE_0, &program);
            install(
                &mut board,
                0,
                long_state(CODE_0, STACK_0, cr3, false),
                0xD00,
            );
            board
        };
        // The oracle is the loop itself, called once with the same inputs.
        let mut direct = setup();
        let retired = quantum(&mut direct, 0, 1_000).unwrap().retired;
        assert_eq!(retired, steps);
        // By the program: 20 additions of 3.
        assert_eq!(dword(&mut direct, DATA), 60);
        let direct_state = direct.vcpu(0).unwrap().state.clone();
        for policy in [
            QuantumPolicy::Fixed(1),
            QuantumPolicy::Fixed(7),
            QuantumPolicy::default(),
            QuantumPolicy::seeded(1),
            QuantumPolicy::seeded(99),
        ] {
            let mut board = setup();
            let mut host = BoardSmpHost::new(
                &mut board,
                |board: &mut LoadedFirmwareBoard, cpu, budget, stop: &mut dyn FnMut() -> bool| {
                    with_set(|set| board.run_vcpu_set_quantum(cpu, budget, stop, set))
                },
            )
            .unwrap();
            let mut scheduler = SmpScheduler::new(1, policy.clone(), StopRequest::new()).unwrap();
            assert_eq!(scheduler.run(&mut host, 1_000).unwrap(), Finished::AllIdle);
            assert_eq!(scheduler.retired_total(), steps, "{policy:?}");
            assert_eq!(board.clock().now_ns(), steps, "{policy:?}: one ns each");
            assert_eq!(board.vcpu(0).unwrap().state, direct_state, "{policy:?}");
            assert_eq!(dword(&mut board, DATA), 60, "{policy:?}");
        }
    }

    #[test]
    fn the_same_seed_gives_the_same_counters_schedule_and_time() {
        let run = |seed: u64| {
            let mut board = exchange_board();
            let (finished, counters, schedule) =
                run_scheduler(&mut board, QuantumPolicy::seeded(seed), 100_000);
            assert_eq!(finished, Finished::AllIdle, "seed {seed}");
            assert_eq!(dword(&mut board, COUNTER_0), ROUNDS, "seed {seed}");
            assert_eq!(dword(&mut board, COUNTER_1), ROUNDS, "seed {seed}");
            (counters, schedule, board.clock().now_ns())
        };
        let first = run(5);
        assert_eq!(first, run(5));
        // Another seed cuts the turns elsewhere (the schedule differs), the exchange still ends.
        assert_ne!(first.1, run(6).1);
    }

    #[test]
    fn a_vcpu_runs_on_the_cr3_of_its_own_tables_and_not_on_the_board_root() {
        let mut board = board(2);
        // The same code and the same linear address, two table sets that map that address to
        // different pages; the board's own root (0x1000) is an empty table.
        for (cpu, base, page, mark, rsp, code_at) in [
            (
                0usize,
                TABLES_0,
                0x60_0000u64,
                0x1111_1111u32,
                STACK_0,
                CODE_0,
            ),
            (1, TABLES_1, 0x80_0000, 0x2222_2222, STACK_1, CODE_1),
        ] {
            let mut program = mov_eax(mark);
            program.extend(mov_ebx(0x40_0000));
            program.extend([0x89, 0x03, 0xF4]); // mov [rbx], eax; hlt
            patch(&mut board, code_at, &program);
            let cr3 = tables(&mut board, base, page, 0);
            install(&mut board, cpu, long_state(code_at, rsp, cr3, false), 0xD01);
        }
        for cpu in 0..2 {
            assert_eq!(
                quantum(&mut board, cpu, 10).unwrap().retired,
                4,
                "cpu {cpu}"
            );
        }
        assert_eq!(dword(&mut board, 0x60_0000), 0x1111_1111);
        assert_eq!(dword(&mut board, 0x80_0000), 0x2222_2222);
        assert_eq!(board.cr3(), BOARD_ROOT, "the board's root is not touched");
        // A vCPU without a CR3 of its own is refused, not run on the board's.
        board.vcpu_mut(1).unwrap().state.cr3 = None;
        assert_eq!(
            quantum(&mut board, 1, 10),
            Err(FirmwareBoardError::Config(
                "vcpu quantum: the vCPU has no CR3"
            ))
        );
    }

    #[test]
    fn the_efer_of_the_vcpu_decides_the_walk_and_not_the_constant_of_the_board() {
        // The data page entry has bit 63 (XD) set. With IA32_EFER.NXE = 1 it is a valid entry (the
        // page is not executable, a store is fine); with NXE = 0 bit 63 is reserved and a store
        // is a #PF with the RSVD bit (SDM Vol. 3A 4.6, 4.7: error code P | W | RSVD = 0b1011).
        const XD: u64 = 1 << 63;
        let mut board = board(2);
        let mut program = mov_eax(0xABCD);
        program.extend(mov_ebx(0x40_0000));
        program.extend([0x89, 0x03, 0xF4]);
        for (cpu, base, code_at, rsp) in [
            (0, TABLES_0, CODE_0, STACK_0),
            (1, TABLES_1, CODE_1, STACK_1),
        ] {
            patch(&mut board, code_at, &program);
            let cr3 = tables(&mut board, base, 0x60_0000 + 0x20_0000 * cpu as u64, XD);
            let efer = if cpu == 0 { 0xD01 } else { 0x501 };
            install(&mut board, cpu, long_state(code_at, rsp, cr3, false), efer);
        }
        // 0xD01 (NXE set; it is not FIRMWARE_EFER, which has no SCE): the store lands.
        assert_ne!(board.vcpu(0).unwrap().guest_efer, FIRMWARE_EFER);
        assert_eq!(quantum(&mut board, 0, 10).unwrap().retired, 4);
        assert_eq!(dword(&mut board, 0x60_0000), 0xABCD);
        // 0x501 (NXE clear): the same entry is a reserved-bit fault. With FIRMWARE_EFER (NXE set)
        // the store would have landed, as on vCPU 0.
        let error = quantum(&mut board, 1, 10).unwrap_err();
        let FirmwareBoardError::VcpuQuantum { cpu: 1, detail } = error else {
            panic!("{error:?}");
        };
        assert!(detail.contains("error_code: Some(11)"), "{detail}");
        assert_eq!(dword(&mut board, 0x80_0000), 0, "nothing was stored");
    }

    #[test]
    fn a_thr_write_ends_the_call_and_its_byte_goes_to_the_log_by_the_function_of_the_linux_loop() {
        let mut board = board(2);
        let cr3 = tables(&mut board, TABLES_1, 0x80_0000, 0);
        // mov edx, 0x3F8; mov eax, 'A'; out dx, al; mov eax, 'B'; out dx, al; hlt
        let mut program = imm(&[0xBA], 0x3F8);
        program.extend(mov_eax(u32::from(b'A')));
        program.push(0xEE);
        program.extend(mov_eax(u32::from(b'B')));
        program.push(0xEE);
        program.push(0xF4);
        patch(&mut board, CODE_1, &program);
        install(
            &mut board,
            1,
            long_state(CODE_1, STACK_1, cr3, false),
            0xD01,
        );
        assert_eq!(quantum(&mut board, 1, 100).unwrap().retired, 3);
        assert_eq!(board.com1_log, b"A");
        assert_eq!(quantum(&mut board, 1, 100).unwrap().retired, 2);
        assert_eq!(board.com1_log, b"AB");
        assert_eq!(quantum(&mut board, 1, 100).unwrap().retired, 1);
        assert_eq!(board.vcpu(1).unwrap().activity(), CpuActivity::Halted);
    }

    #[test]
    fn a_call_that_ends_inside_a_rep_makes_the_next_call_of_that_vcpu_skip_its_first_boundary() {
        // enable the LAPIC; rep stosb of 3000 bytes (the interpreter ends a REP step after 1024
        // iterations); hlt.
        let mut program = enable_lapic();
        program.extend(imm(&[0xB9], 3000)); // mov ecx, 3000
        program.extend(imm(&[0xBF], DATA as u32)); // mov edi, DATA
        program.extend([0xF3, 0xAA, 0xF4]); // rep stosb; hlt
        // `flag_kept`: the second call goes on with the flag the first call left (true).
        let second_call = |flag_kept: bool| {
            let mut board = board(2);
            system(&mut board, false);
            let cr3 = tables(&mut board, TABLES_1, 0x80_0000, 0);
            patch(&mut board, CODE_1, &program);
            install(&mut board, 1, long_state(CODE_1, STACK_1, cr3, true), 0xD01);
            let first = quantum(&mut board, 1, 100).unwrap();
            // 3 for the LAPIC, 2 movs; the REP made progress and did not retire.
            assert_eq!((first.retired, first.partial_rep), (5, true));
            assert!(board.vcpu(1).unwrap().skip_first_boundary);
            assert!(
                !board.vcpu(0).unwrap().skip_first_boundary,
                "the flag is per vCPU"
            );
            // A Fixed vector arrives while the REP is half done; IF = 1, the handler is in the IDT.
            board
                .local_apic_of(0)
                .unwrap()
                .write_icr(0, 1, FIXED | u32::from(TO_AP))
                .unwrap();
            board.vcpu_mut(1).unwrap().skip_first_boundary = flag_kept;
            let second = quantum(&mut board, 1, 3).unwrap();
            (board, second)
        };
        // With the flag: the boundary inside the REP delivers nothing, the REP goes on and ends
        // the call again; the vector still waits in the IRR.
        let (board_kept, second) = second_call(true);
        assert_eq!((second.retired, second.partial_rep), (0, true));
        assert_eq!(vector_state(&board_kept, 1, TO_AP), (true, false));
        assert!(board_kept.vcpu(1).unwrap().skip_first_boundary);
        // Without it that boundary would have taken the vector (the contrast: the flag is what
        // holds it back): in service, three handler instructions in.
        let (board_cleared, second) = second_call(false);
        assert_eq!((second.retired, second.partial_rep), (3, false));
        assert_eq!(vector_state(&board_cleared, 1, TO_AP), (false, true));
        assert!(!board_cleared.vcpu(1).unwrap().skip_first_boundary);
        // The REP completes on the third call (1024 + 1024 + 952 iterations): it retires, the flag
        // goes, and the bytes are AL = 0xFF (EAX = 0x1FF) 3000 times.
        let (mut board, _) = second_call(true);
        let third = quantum(&mut board, 1, 1).unwrap();
        assert_eq!((third.retired, third.partial_rep), (1, false));
        assert!(!board.vcpu(1).unwrap().skip_first_boundary);
        assert_eq!(read(&mut board, DATA, 3000), vec![0xFFu8; 3000]);
    }

    // ---- S6-21 step 1: the source of the board reaches the boot processor -----------------------
    //
    // Expectations are from the SDM and the 8259/8254 data sheets (by memory), not from the model:
    // after ICW1..ICW4 and an unmasked IR0 the PIC raises INTR for the PIT's OUT0 edge and the CPU,
    // with IF = 1, takes vector ICW2 + 0 through its IDT; the guest's EOI (OCW2 0x20 to port 0x20)
    // frees the line for the next edge; mode 2 gives one edge per `count` PIT ticks (1_193_182 Hz).

    /// ICW2 of the master 8259 the board models (`Pic8259Role::icw2`, the base Linux uses).
    const BOARD_VECTOR: u8 = 0x30;
    const IDT_1: u64 = 0x3_8000;
    const HANDLER_BOARD: u64 = 0x2_4000;
    const HANDLER_LAPIC: u64 = 0x2_5000;
    const HANDLER_AP_BOARD: u64 = 0x2_6000;
    const COUNTER_LAPIC: u64 = 0x4_0008;
    const LAPIC_VECTOR: u8 = 0x31;

    fn gate(board: &mut LoadedFirmwareBoard, idt_at: u64, vector: u8, handler: u64) {
        let mut gate = [0u8; 16];
        gate[0..2].copy_from_slice(&(handler as u16).to_le_bytes());
        gate[2..4].copy_from_slice(&0x08u16.to_le_bytes());
        gate[5] = 0x8E;
        gate[6..8].copy_from_slice(&((handler >> 16) as u16).to_le_bytes());
        patch(board, idt_at + u64::from(vector) * 16, &gate);
    }

    /// `counter += 1` (R8, R9 only), then with `eoi_to_pic` the EOI to the master 8259, then IRETQ.
    fn counting_handler(counter: u64, eoi_to_pic: bool) -> Vec<u8> {
        let mut code = [
            imm(&[0x41, 0xB8], counter as u32), // mov r8d, counter
            vec![0x45, 0x8B, 0x08],             // mov r9d, [r8]
            vec![0x41, 0xFF, 0xC1],             // inc r9d
            vec![0x45, 0x89, 0x08],             // mov [r8], r9d
        ]
        .concat();
        if eoi_to_pic {
            code.extend([0xB0, 0x20, 0xE6, 0x20]); // mov al, 0x20; out 0x20, al
        }
        code.extend([0x48, 0xCF]);
        code
    }

    /// The boot processor as the kernel's setup does it: the master 8259 (ICW1 0x11, ICW2 0x30,
    /// ICW3 0x04, ICW4 0x01, IMR 0xFE), PIT channel 0 in mode 2 with count 10 (control 0x34), STI,
    /// then HLT.
    fn pit_irq0_program() -> Vec<u8> {
        let mut code = Vec::new();
        for (port, value) in [
            (0x20u8, 0x11u8),
            (0x21, BOARD_VECTOR),
            (0x21, 0x04),
            (0x21, 0x01),
            (0x21, 0xFE),
            (0x43, 0x34),
            (0x40, 10),
            (0x40, 0),
        ] {
            code.extend([0xB0, value, 0xE6, port]); // mov al, value; out port, al
        }
        code.push(0xFB); // sti
        code.extend(HLT_LOOP);
        code
    }

    /// A two-processor board with the PIT -> 8259 source installed. The boot processor runs
    /// `pit_irq0_program` with IF = 0 at the start; the application processor spins (`spin`) with
    /// IF = 1 and an IDT of its own, or sleeps halted with IF = 0.
    fn pit_board(spin: bool) -> LoadedFirmwareBoard {
        let mut board = board(2);
        assert!(board.enable_pit_pic_irq());
        system(&mut board, false);
        patch(&mut board, CODE_0, &pit_irq0_program());
        patch(&mut board, CODE_1, &[0xEB, 0xFE]);
        patch(
            &mut board,
            HANDLER_BOARD,
            &counting_handler(COUNTER_0, true),
        );
        patch(
            &mut board,
            HANDLER_AP_BOARD,
            &counting_handler(COUNTER_1, false),
        );
        gate(&mut board, IDT_AT, BOARD_VECTOR, HANDLER_BOARD);
        // The application processor has its own IDT: the same vector would count in COUNTER_1.
        gate(&mut board, IDT_1, BOARD_VECTOR, HANDLER_AP_BOARD);
        let (cr3_0, cr3_1) = (
            tables(&mut board, TABLES_0, 0x60_0000, 0),
            tables(&mut board, TABLES_1, 0x80_0000, 0),
        );
        let mut bsp = long_state(CODE_0, STACK_0, cr3_0, true);
        bsp.rflags = 2;
        install(&mut board, 0, bsp, 0xD01);
        let mut ap = long_state(CODE_1, STACK_1, cr3_1, false);
        ap.idtr = Some(od_isa::state::Idtr {
            base: IDT_1,
            limit: 0xFFF,
        });
        if !spin {
            ap.rflags = 2;
            ap.activity = Some(CpuActivity::Halted);
        }
        install(&mut board, 1, ap, 0xD01);
        board
    }

    #[test]
    fn irq0_of_the_pit_reaches_the_boot_processor_at_the_boundary_and_not_the_application_processor()
     {
        let mut board = pit_board(true);
        let source = board.pit_pic_source.clone().unwrap();
        // The boot processor sets up and halts; while it sleeps the other vCPU spins, so the
        // clock moves by retired instructions only. One period is about 8.4 us = 8381 ns.
        let (finished, counters, _) = run_scheduler(&mut board, QuantumPolicy::Fixed(20), 40_000);
        assert_eq!(finished, Finished::BudgetExhausted);
        let (bsp, ap) = (dword(&mut board, COUNTER_0), dword(&mut board, COUNTER_1));
        // 40 000 ns of a 8381 ns period: at least three edges, at most five.
        assert!(
            (3..=5).contains(&bsp),
            "handler of the boot processor: {bsp}"
        );
        assert_eq!(
            ap, 0,
            "the application processor is never offered the board source"
        );
        // The INTA cycle of the chip is the board source's acknowledge: once per delivery (a
        // delivery cut by the end of the budget may have run its handler only partly).
        let acknowledged = source.stats().acknowledged;
        assert!(
            acknowledged == u64::from(bsp) || acknowledged == u64::from(bsp) + 1,
            "{acknowledged} acknowledged, {bsp} handled"
        );
        assert_eq!(source.stats().ack_refused, 0);
        assert!(
            counters[0].wakes >= u64::from(bsp),
            "the halted boot processor was woken"
        );
        assert_eq!(counters[1].wakes, 0);
    }

    #[test]
    fn the_idle_jump_goes_to_the_edge_of_the_source_and_the_halted_boot_processor_takes_it() {
        // Both vCPUs are halted after the setup: every edge is reached by an idle jump (nobody
        // runs), the boot processor is woken by the board source (no vector in any Local APIC), and
        // the deadline is consumed by that, so the scheduler never meets a due deadline.
        let mut board = pit_board(false);
        let (finished, counters, _) = run_scheduler(&mut board, QuantumPolicy::Fixed(20), 200);
        assert_eq!(finished, Finished::BudgetExhausted);
        let handled = dword(&mut board, COUNTER_0);
        assert!(
            handled >= 5,
            "{handled} interrupts in 200 instructions of budget"
        );
        assert_eq!(dword(&mut board, COUNTER_1), 0);
        assert!(counters[0].wakes >= u64::from(handled));
        // Each handled edge is a period of the PIT further on the board clock.
        assert!(board.clock().now_ns() >= u64::from(handled - 1) * 8381);
    }

    fn pit_ioapic_ap_board(bsp_if: bool) -> LoadedFirmwareBoard {
        use crate::{AccessWidth, GuestPhysicalAddress, IO_APIC_MMIO_BASE, PortAddress};

        let mut board = board(2);
        assert!(board.enable_pit_ioapic_irq());
        system(&mut board, false);
        patch(&mut board, CODE_1, &[0xfa, 0xf4]); // CLI; HLT
        let mut handler = count_and_eoi(COUNTER_1);
        handler.extend([0x48, 0xcf]); // IRETQ
        patch(&mut board, HANDLER_AP_BOARD, &handler);
        gate(&mut board, IDT_AT, BOARD_VECTOR, HANDLER_AP_BOARD);
        for (cpu, code, stack, tables_at) in [
            (0, CODE_0, STACK_0, TABLES_0),
            (1, CODE_1, STACK_1, TABLES_1),
        ] {
            let cr3 = tables(&mut board, tables_at, 0x60_0000 + cpu as u64 * 0x20_0000, 0);
            let mut state = long_state(code, stack, cr3, true);
            state.activity = Some(CpuActivity::Halted);
            if cpu == 0 && !bsp_if {
                state.rflags = 2;
            }
            install(&mut board, cpu, state, 0xd01);
        }
        let bus = board.bus_mut();
        bus.write_mmio_shared(
            1,
            GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE + 0xf0),
            AccessWidth::Dword,
            0x1ff,
        )
        .unwrap();
        // INTIN2 high dword selects APIC ID 1. The low dword unmasks the edge.
        for (offset, value) in [
            (0, 0x15),
            (0x10, 1 << 24),
            (0, 0x14),
            (0x10, u64::from(BOARD_VECTOR)),
        ] {
            bus.write_mmio_shared(
                0,
                GuestPhysicalAddress::new(IO_APIC_MMIO_BASE + offset),
                AccessWidth::Dword,
                value,
            )
            .unwrap();
        }
        for (port, value) in [(0x43, 0x34), (0x40, 10), (0x40, 0)] {
            bus.write_port(PortAddress::new(port), AccessWidth::Byte, value)
                .unwrap();
        }
        board
    }

    #[test]
    fn the_idle_jump_wakes_the_ap_targeted_by_the_pit_ioapic_route() {
        for bsp_if in [false, true] {
            let mut board = pit_ioapic_ap_board(bsp_if);
            let (finished, counters, _) = run_scheduler(&mut board, QuantumPolicy::Fixed(20), 200);
            assert_eq!(finished, Finished::AllIdle);
            assert_eq!(
                (counters[0].wakes, counters[1].wakes),
                (0, 1),
                "BSP.IF={bsp_if}"
            );
            assert_eq!(dword(&mut board, COUNTER_1), 1);
            assert_eq!(vector_state(&board, 1, BOARD_VECTOR), (false, false));
            assert!(board.clock().now_ns() >= 8381);
        }
    }

    #[test]
    fn the_boot_processor_acknowledges_the_source_that_supplied_the_frame() {
        use crate::{AccessWidth, GuestPhysicalAddress};

        let mut board = board(2);
        assert!(board.enable_pit_ioapic_irq());
        board
            .bus_mut()
            .write_mmio_shared(
                0,
                GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE + 0xf0),
                AccessWidth::Dword,
                0x1ff,
            )
            .unwrap();
        let boot_lapic = board.local_apic_of(0).unwrap();
        let sender = board.local_apic_of(1).unwrap();
        let mut ioapic_source = board.pit_ioapic_source().unwrap();
        let stats = ioapic_source.clone();
        let mut source = BootProcessorSource {
            board: &mut ioapic_source,
            lapic: LapicIrqSource::new(boot_lapic.clone()),
            selected_board_vector: Cell::new(None),
        };
        sender.write_icr(1, 0, 0x30).unwrap();
        assert_eq!(source.pending_vector(), Some(0x30));
        sender.write_icr(1, 0, 0x50).unwrap();
        source.acknowledge(0x30);
        assert_eq!(boot_lapic.vector_state(0x30), (false, true));
        assert_eq!(boot_lapic.vector_state(0x50), (true, false));
        assert_eq!(stats.stats().acknowledged, 1);
        assert_eq!(stats.stats().ack_refused, 0);
    }

    /// A source with one vector, pending until it was acknowledged once.
    struct OneShot {
        vector: u8,
        acks: std::sync::Arc<std::sync::atomic::AtomicU32>,
    }

    impl BoardIrqSource for OneShot {
        fn pending_vector(&self) -> Option<u8> {
            (self.acks.load(std::sync::atomic::Ordering::Relaxed) == 0).then_some(self.vector)
        }
        fn acknowledge(&mut self, vector: u8) {
            assert_eq!(vector, self.vector);
            self.acks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        fn next_deadline_ns(&self) -> Option<u64> {
            None
        }
    }

    /// Two vCPUs (spinning, IF = 1) with a counting handler for the board vector in each IDT.
    fn one_shot_board(
        vector: u8,
    ) -> (
        LoadedFirmwareBoard,
        std::sync::Arc<std::sync::atomic::AtomicU32>,
    ) {
        let mut board = board(2);
        system(&mut board, false);
        patch(&mut board, CODE_0, &[0xEB, 0xFE]);
        patch(&mut board, CODE_1, &[0xEB, 0xFE]);
        patch(
            &mut board,
            HANDLER_BOARD,
            &counting_handler(COUNTER_0, false),
        );
        patch(
            &mut board,
            HANDLER_AP_BOARD,
            &counting_handler(COUNTER_1, false),
        );
        gate(&mut board, IDT_AT, vector, HANDLER_BOARD);
        gate(&mut board, IDT_1, vector, HANDLER_AP_BOARD);
        let (cr3_0, cr3_1) = (
            tables(&mut board, TABLES_0, 0x60_0000, 0),
            tables(&mut board, TABLES_1, 0x80_0000, 0),
        );
        install(
            &mut board,
            0,
            long_state(CODE_0, STACK_0, cr3_0, true),
            0xD01,
        );
        let mut ap = long_state(CODE_1, STACK_1, cr3_1, false);
        ap.idtr = Some(od_isa::state::Idtr {
            base: IDT_1,
            limit: 0xFFF,
        });
        install(&mut board, 1, ap, 0xD01);
        let acks = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        board.irq_source = Box::new(OneShot {
            vector,
            acks: acks.clone(),
        });
        (board, acks)
    }

    #[test]
    fn the_board_source_is_offered_to_the_boot_processor_only() {
        let (mut board, acks) = one_shot_board(BOARD_VECTOR);
        quantum(&mut board, 1, 50).unwrap();
        assert_eq!(
            acks.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "an application processor is not offered the source"
        );
        assert_eq!(dword(&mut board, COUNTER_1), 0);
        quantum(&mut board, 0, 50).unwrap();
        assert_eq!(
            acks.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "the boot processor takes it at its first boundary"
        );
        assert_eq!(dword(&mut board, COUNTER_0), 1);
        // The source is spent: nothing more is delivered.
        quantum(&mut board, 0, 50).unwrap();
        assert_eq!(dword(&mut board, COUNTER_0), 1);
    }

    #[test]
    fn the_board_vector_is_acknowledged_by_its_own_source() {
        let (mut board, acks) = one_shot_board(BOARD_VECTOR);
        // The boot processor also has a Fixed vector in its own Local APIC (software-enabled by the
        // guest's first instructions, a self-IPI would do the same).
        patch(
            &mut board,
            HANDLER_LAPIC,
            &counting_handler(COUNTER_LAPIC, false),
        );
        gate(&mut board, IDT_AT, LAPIC_VECTOR, HANDLER_LAPIC);
        let mut program = enable_lapic();
        program.extend([0x90, 0x90, 0x90]); // nops: boundaries after the LAPIC is enabled
        program.extend([0xEB, 0xFE]);
        patch(&mut board, CODE_0, &program);
        board
            .local_apic_of(1)
            .unwrap()
            .write_icr(1, 0, FIXED | u32::from(LAPIC_VECTOR))
            .unwrap();
        quantum(&mut board, 0, 60).unwrap();
        // The board's vector was delivered and acknowledged in the board's source ...
        assert_eq!(
            (
                acks.load(std::sync::atomic::Ordering::Relaxed),
                dword(&mut board, COUNTER_0)
            ),
            (1, 1)
        );
        // ... the Local APIC's vector through its own IDT gate and acknowledged in the LAPIC of the
        // boot processor (IRR -> ISR; the handler does not EOI), not in the board's source.
        assert_eq!(dword(&mut board, COUNTER_LAPIC), 1);
        assert_eq!(vector_state(&board, 0, LAPIC_VECTOR), (false, true));
    }

    #[test]
    fn an_unwakeable_board_ends_all_idle_without_a_jump() {
        // Both vCPUs halted (the boot processor with IF = 1), the source has an edge at 40 ns but no
        // vector and no wake time (a masked line): nothing can wake anybody. The jump to the edge is
        // not offered (no wake time), the run ends AllIdle, not IdleNoProgress and not a loop of
        // jumps.
        let mut board = board(2);
        let cr3 = tables(&mut board, TABLES_1, 0x80_0000, 0);
        for cpu in 0..2 {
            let mut state = long_state(CODE_1, STACK_1, cr3, false);
            state.activity = Some(CpuActivity::Halted);
            state.rflags = if cpu == 0 { 2 | (1 << 9) } else { 2 };
            install(&mut board, cpu, state, 0xD01);
        }
        board.irq_source = Box::new(EdgeAt(40));
        let mut host = BoardSmpHost::new(
            &mut board,
            |board: &mut LoadedFirmwareBoard, cpu, budget, stop: &mut dyn FnMut() -> bool| {
                with_set(|set| board.run_vcpu_set_quantum(cpu, budget, stop, set))
            },
        )
        .unwrap();
        let mut scheduler =
            SmpScheduler::new(2, QuantumPolicy::Fixed(10), StopRequest::new()).unwrap();
        assert_eq!(scheduler.run(&mut host, 100).unwrap(), Finished::AllIdle);
        assert_eq!(scheduler.idle(), (0, 0));
    }
}
