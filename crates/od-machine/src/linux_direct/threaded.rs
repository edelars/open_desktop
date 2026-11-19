//! LinuxDirect on the Stage 6 vCPU worker scheduler.

use super::*;
use crate::FirmwareBoardError;
use crate::firmware_board::set_exit_to_probe_exit;
use crate::firmware_board::vcpu_quantum::{
    SharedSetReport, is_quantum_boundary, quantum_progress, run_shared_set_quantum,
};
use crate::paged_memory_adapter::PagedCpl0SetExit;
use crate::parallel_run::{
    NmiPaging, ParallelConfig, ParallelFinish, QuantumCall, ThreadRunOptions, run_threads_rooted,
};
use crate::smp_scheduler::{Executed, RunFailure};
use crate::vcpu::Vcpu;
use od_isa::PrivilegeLevel;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Limits for running LinuxDirect on a vCPU worker. The wall timeout requests a stop at a
/// worker boundary and waits for the worker to join.
#[derive(Clone, Copy, Debug)]
pub struct LinuxThreadedOptions {
    pub quantum: u64,
    pub timeout: Duration,
}

#[derive(Debug, Eq, PartialEq)]
pub struct LinuxThreadedRunReport {
    pub stop: LinuxStopRecord,
    pub threads_started: usize,
    pub threads_exited: usize,
}

#[derive(Default)]
struct ObservedRun {
    irq: IrqCounters,
    rdtsc: [u64; 4],
    changes: Vec<TranslationContextChange>,
    cpl3_retired: u64,
    journal: SyscallJournal,
    exceptions: u64,
    page_faults: u64,
    protection_faults: u64,
    terminal: Option<Terminal>,
}

struct Terminal {
    exit: PagedProbeExit,
    detail: String,
    page_fault: Option<od_isa::memory::PageFaultRecord>,
    memory_error: Option<crate::TrackedAccessError>,
    user_entry: Option<IretqUserEntry>,
}

impl ObservedRun {
    fn record(
        &mut self,
        report: SharedSetReport,
        vcpu: &Vcpu,
        deadline_cut: bool,
    ) -> Result<Executed, RunFailure<FirmwareBoardError>> {
        let cpu = report.cpu;
        let progress = quantum_progress(&cpu, report.device_ns);
        self.irq.irqs_delivered += report.irq.irqs_delivered;
        self.irq.irq_refused_masked += report.irq.irq_refused_masked;
        self.irq.irq_refused_shadow += report.irq.irq_refused_shadow;
        self.irq.quantum_cut_by_deadline += u64::from(deadline_cut);
        if let Some(cpl) = report.rdtsc_cpl {
            self.rdtsc[cpl] += 1;
        }
        self.changes.extend(cpu.translation_context_changes);
        self.cpl3_retired += cpu.cpl3_retired;
        for event in cpu.syscall_events {
            self.journal.record(event);
        }
        self.exceptions += cpu.delivered_exceptions;
        self.page_faults += cpu.delivered_page_faults;
        self.protection_faults += cpu.delivered_general_protection_faults;

        if !is_quantum_boundary(&cpu.exit) {
            let user_entry = match &cpu.exit {
                PagedCpl0SetExit::UserEntry(entry) => Some(entry.clone()),
                _ => None,
            };
            let (exit, detail) = set_exit_to_probe_exit(cpu.exit).map_err(|error| {
                RunFailure::after(progress, error.into_board_error(&vcpu.state))
            })?;
            match exit {
                PagedProbeExit::Fault(_)
                | PagedProbeExit::Unsupported { .. }
                | PagedProbeExit::Stopped => {}
                PagedProbeExit::PortIo(error) => {
                    return Err(RunFailure::after(progress, FirmwareBoardError::Bus(error)));
                }
                _ => {
                    return Err(RunFailure::after(
                        progress,
                        FirmwareBoardError::VcpuQuantum {
                            cpu: vcpu.index,
                            detail,
                        },
                    ));
                }
            }
            self.terminal = Some(Terminal {
                exit,
                detail,
                page_fault: cpu.page_fault,
                memory_error: cpu.memory_error,
                user_entry,
            });
        }
        Ok(progress)
    }
}

fn run_error(detail: impl std::fmt::Debug, retired: u64) -> LinuxDirectError {
    let mut error = LinuxDirectError::new("run_failed", "run", detail);
    error.retired = Some(retired);
    error
}

/// Run a Stage4A or Stage4AApic LinuxDirect guest in user mode on one dedicated vCPU thread. The
/// instruction budget and virtual time follow the Stage 6 worker scheduler. The board can be
/// passed back to this entry to continue from its final vCPU state.
pub fn run_linux_direct_board_set_user_mode_threaded(
    boot: &mut LinuxDirectBoot,
    input: &LinuxDirectInput<'_>,
    instruction_budget: u64,
    mut members: LinuxCpl0SetMembers,
    options: LinuxThreadedOptions,
) -> Result<LinuxThreadedRunReport, LinuxDirectError> {
    if options.quantum == 0
        || options.timeout.is_zero()
        || Instant::now().checked_add(options.timeout).is_none()
    {
        return Err(run_error(
            "invalid threaded LinuxDirect quantum or timeout",
            0,
        ));
    }
    if boot.board.vcpu_count() != 1
        || boot.board.linux_interleaved()
        || !matches!(
            members.cpuid_profile,
            CpuidProfile::X8664V1Stage4A | CpuidProfile::X8664V1Stage4AApic
        )
        || members.diagnostic_past_c7
    {
        return Err(run_error(
            "threaded LinuxDirect requires one Stage4A or Stage4AApic BSP and no interleaved schedule",
            0,
        ));
    }
    let apic = members.cpuid_profile == CpuidProfile::X8664V1Stage4AApic;
    check_nvme_interrupt_profile(boot, apic)?;
    if boot
        .board
        .vcpu(BOOT_VCPU_INDEX)
        .is_none_or(|cpu| cpu.legacy.is_some())
    {
        return Err(run_error(
            "threaded LinuxDirect requires a long-mode BSP",
            0,
        ));
    }
    if apic {
        prepare_apic_profile(&boot.board, &mut members)?;
    } else {
        if !boot.board.enable_pit_pic_irq() {
            return Err(run_error("threaded Stage4A requires the PIT/PIC source", 0));
        }
        boot.board.enable_uart_irq4();
    }
    boot.board
        .enable_clock_owner()
        .map_err(|error| run_error(error, 0))?;
    if apic {
        install_linux_ioapic_sources(&mut boot.board)?;
    }
    boot.board
        .import_threaded_linux_pci_address()
        .map_err(|error| run_error(error, 0))?;

    let root = boot.board.cr3();
    let vcpu = boot.board.vcpu_mut(BOOT_VCPU_INDEX).expect("validated BSP");
    vcpu.state.cr3.get_or_insert(root);
    members.halt = true;
    members.irq = true;
    members.clock_owner = true;
    let profile = LinuxCpl0Profile::new(members, true, boot.board.efer());
    let mut names = Vec::new();
    profile.bind(
        &LinuxVcpuCells::from_vcpu(boot.board.vcpu(BOOT_VCPU_INDEX).expect("validated BSP")),
        &Cell::new(0),
        |member| names.push(member.to_string()),
    );

    let pm_before = boot.board.acpi_pm().map(|pm| pm.timer_reads());
    let pit_before = boot.board.pit_read_counts();
    let observed = Mutex::new(ObservedRun::default());
    let quantum = |call: QuantumCall<'_, '_>| {
        let cells = LinuxVcpuCells::from_vcpu(call.vcpu);
        let pci_config = Cell::new(0);
        let set = profile.bind(&cells, &pci_config, |_| {});
        let result = run_shared_set_quantum(
            call.env,
            call.vcpu,
            call.cpu,
            call.lapic,
            call.source,
            Some(call.gate),
            call.budget,
            call.stop,
            &set,
            call.consecutive_faults,
            call.machine_stop,
            call.nvme_pending,
            call.fetch_cache,
            |report, cpu| {
                let mut observed = observed.lock().expect("BSP report accumulator");
                let progress = observed.record(report, cpu, call.deadline_cut)?;
                *call.guest_exit = observed.terminal.is_some();
                Ok(progress)
            },
        );
        cells.commit(call.vcpu);
        result
    };
    let source_free = boot.board.nvme().is_some();
    let (parallel, _) = run_threads_rooted(
        &mut boot.board,
        ParallelConfig {
            quantum: options.quantum,
            budget: instruction_budget,
            ..ParallelConfig::default()
        },
        &quantum,
        None,
        ThreadRunOptions {
            source_free,
            nmi_paging: NmiPaging::Board,
            discard_startup_port_ticks: true,
            fetch_cache: members.fetch_cache,
        },
        |control| control.wait_finished(options.timeout),
    )
    .map_err(|error| run_error(error, 0))?;
    let bsp = &parallel.cpus[BOOT_VCPU_INDEX];
    let retired = bsp.retired;
    // Keep both views usable for diagnosis or another call, even after a late worker error.
    let views = boot.board.sync_threaded_linux_boot_views();
    let (exit, stop_reason) = match parallel.finished {
        ParallelFinish::GuestExit { .. } => (None, None),
        ParallelFinish::GuestStop(reason) => (Some(PagedProbeExit::Stopped), Some(reason)),
        ParallelFinish::BudgetExhausted => (Some(PagedProbeExit::BudgetExhausted), None),
        ParallelFinish::AllIdle => (Some(PagedProbeExit::Halted), None),
        ParallelFinish::Error { error, .. } => return Err(run_error(error, retired)),
        ParallelFinish::Panicked { .. } => {
            return Err(run_error("LinuxDirect vCPU worker panicked", retired));
        }
        ParallelFinish::Stopped => {
            return Err(run_error("threaded LinuxDirect timed out", retired));
        }
    };
    views.map_err(|error| run_error(error, retired))?;

    let mut observed = observed
        .into_inner()
        .expect("joined BSP report accumulator");
    let terminal = match exit {
        Some(exit) => Terminal {
            detail: format!("{exit:?}"),
            exit,
            page_fault: None,
            memory_error: None,
            user_entry: None,
        },
        None => observed
            .terminal
            .take()
            .ok_or_else(|| run_error("missing guest exit report", retired))?,
    };
    observed.irq.irq0_edges = boot.board.irq0_edges();
    observed.irq.halt_wakes = bsp.wakes;
    observed.irq.halt_idle_ns = parallel.idle_ns;
    let pit_reads = pit_before
        .zip(boot.board.pit_read_counts())
        .map(|(before, after)| [after[0] - before[0], after[1] - before[1]]);
    let clock = boot.board.clock();
    let run = FirmwareRunReport {
        exit: terminal.exit,
        page_fault: terminal.page_fault,
        memory_error: terminal.memory_error,
        retired,
        quanta: bsp.calls,
        com1: boot.board.com1_transcript().to_vec(),
        virtual_ns: clock.now_ns(),
        tsc_hz: clock.tsc_hz(),
        rdtsc_reads: Some(observed.rdtsc.iter().sum()),
        rdtsc_reads_by_cpl: Some(observed.rdtsc),
        pit_port_b_reads: pit_reads.map(|read| read[0]),
        pit_counter_port_reads: pit_reads.map(|read| read[1]),
        stop_reason,
    };
    let privilege = if boot.board.state().cs_selector.is_some_and(|cs| cs & 3 == 3) {
        PrivilegeLevel::Ring3
    } else {
        PrivilegeLevel::Ring0
    };
    let view = boot.board.fetch_view_2m_with_privilege(
        boot.board.state().rip,
        LINUX_STOP_RIP_BYTES,
        privilege,
        boot.board.guest_efer(),
    );
    let stop = stop_record_with_view(
        boot,
        input,
        run,
        pm_before,
        SetExtras {
            path: "cpl0_set_threaded",
            members: names,
            exit_detail: Some(terminal.detail),
            changes: &observed.changes,
            cpuid_divergence_leaf: None,
            user_entry: terminal.user_entry,
            user_mode: true,
            cpl3_retired: observed.cpl3_retired,
            syscall_journal: observed.journal,
            exceptions_delivered: observed.exceptions,
            page_faults_delivered: observed.page_faults,
            general_protection_faults_delivered: observed.protection_faults,
            execution_observations: None,
            irq: observed.irq,
            diagnostic_past_c7: false,
            checkpoints: None,
        },
        view,
    );
    Ok(LinuxThreadedRunReport {
        stop,
        threads_started: parallel.threads_started,
        threads_exited: parallel.threads_exited,
    })
}
