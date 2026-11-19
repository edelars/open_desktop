//! LinuxDirect on the Stage 6 vCPU worker scheduler.

use super::observed::{ObservedRun, RunEnd, RunTermination, finish_stop, run_error};
use super::*;
use crate::firmware_board::vcpu_quantum::run_shared_set_quantum;
use crate::paged_memory_adapter::SyscallEvent;
use crate::parallel_run::{
    NmiPaging, ParallelConfig, ParallelFinish, QuantumCall, ThreadRunOptions, run_threads_rooted,
};
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

#[derive(Debug, Eq, PartialEq)]
pub struct LinuxParallelRunReport {
    pub stop: LinuxStopRecord,
    pub vcpus: Vec<LinuxVcpuRunState>,
    pub terminal_vcpu: Option<usize>,
    pub threads_started: usize,
    pub threads_exited: usize,
}

/// Run a Stage4A or Stage4AApic LinuxDirect guest in user mode on one dedicated vCPU thread. The
/// instruction budget and virtual time follow the Stage 6 worker scheduler. The board can be
/// passed back to this entry to continue from its final vCPU state.
pub fn run_linux_direct_board_set_user_mode_threaded(
    boot: &mut LinuxDirectBoot,
    input: &LinuxDirectInput<'_>,
    instruction_budget: u64,
    members: LinuxCpl0SetMembers,
    options: LinuxThreadedOptions,
) -> Result<LinuxThreadedRunReport, LinuxDirectError> {
    if boot.board.vcpu_count() != 1 {
        return Err(run_error("threaded LinuxDirect requires one vCPU", 0));
    }
    let report = run_threaded(boot, input, instruction_budget, members, options)?;
    Ok(LinuxThreadedRunReport {
        stop: report.stop,
        threads_started: report.threads_started,
        threads_exited: report.threads_exited,
    })
}

/// Run a Stage4AApic LinuxDirect board on one worker per vCPU, with a shared budget.
pub fn run_linux_direct_board_set_user_mode_parallel(
    boot: &mut LinuxDirectBoot,
    input: &LinuxDirectInput<'_>,
    instruction_budget: u64,
    members: LinuxCpl0SetMembers,
    options: LinuxThreadedOptions,
) -> Result<LinuxParallelRunReport, LinuxDirectError> {
    if boot.board.vcpu_count() < 2 || members.cpuid_profile != CpuidProfile::X8664V1Stage4AApic {
        return Err(run_error(
            "parallel LinuxDirect requires a Stage4AApic SMP board",
            0,
        ));
    }
    run_threaded(boot, input, instruction_budget, members, options)
}

fn run_threaded(
    boot: &mut LinuxDirectBoot,
    input: &LinuxDirectInput<'_>,
    instruction_budget: u64,
    mut members: LinuxCpl0SetMembers,
    options: LinuxThreadedOptions,
) -> Result<LinuxParallelRunReport, LinuxDirectError> {
    if options.quantum == 0
        || options.timeout.is_zero()
        || Instant::now().checked_add(options.timeout).is_none()
    {
        return Err(run_error(
            "invalid threaded LinuxDirect quantum or timeout",
            0,
        ));
    }
    if boot.board.linux_interleaved()
        || !matches!(
            members.cpuid_profile,
            CpuidProfile::X8664V1Stage4A | CpuidProfile::X8664V1Stage4AApic
        )
        || members.diagnostic_past_c7
    {
        return Err(run_error(
            "threaded LinuxDirect requires Stage4A or Stage4AApic and no interleaved schedule",
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
        .import_linux_pci_address()
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
    let smp = boot.board.vcpu_count() > 1;
    let observed = Mutex::new(if smp {
        ObservedRun::parallel()
    } else {
        ObservedRun::default()
    });
    let quantum = |call: QuantumCall<'_, '_>| {
        let cells = LinuxVcpuCells::from_vcpu(call.vcpu);
        let pci_config = Cell::new(0);
        let set = profile.bind(&cells, &pci_config, |_| {});
        let mut publish_syscall = |event| {
            observed
                .lock()
                .expect("Linux report accumulator")
                .record_syscall(event);
        };
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
            smp.then_some(&mut publish_syscall as &mut dyn FnMut(SyscallEvent)),
            |report, cpu| {
                let mut observed = observed.lock().expect("Linux report accumulator");
                let progress = observed.record(report, cpu, call.deadline_cut)?;
                *call.guest_exit = observed.has_terminal(cpu.index);
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
    let retired = parallel.cpus.iter().map(|cpu| cpu.retired).sum();
    // Keep both views usable for diagnosis or another call, even after a late worker error.
    let views = boot.board.sync_linux_boot_views();
    let termination = match parallel.finished {
        ParallelFinish::GuestExit { cpu } => RunTermination::Guest(cpu),
        ParallelFinish::GuestStop(reason) => {
            RunTermination::Scheduler(PagedProbeExit::Stopped, Some(reason))
        }
        ParallelFinish::BudgetExhausted => {
            RunTermination::Scheduler(PagedProbeExit::BudgetExhausted, None)
        }
        ParallelFinish::AllIdle => RunTermination::Scheduler(PagedProbeExit::Halted, None),
        ParallelFinish::Error { error, .. } => return Err(run_error(error, retired)),
        ParallelFinish::Panicked { .. } => {
            return Err(run_error("LinuxDirect vCPU worker panicked", retired));
        }
        ParallelFinish::Stopped => {
            return Err(run_error("threaded LinuxDirect timed out", retired));
        }
    };
    views.map_err(|error| run_error(error, retired))?;
    let vcpus = parallel
        .cpus
        .iter()
        .enumerate()
        .map(|(index, cpu)| LinuxVcpuRunState::capture(&boot.board, index, cpu.retired))
        .collect();
    let terminal_vcpu = if let RunTermination::Guest(cpu) = &termination {
        Some(*cpu)
    } else {
        None
    };

    let observed = observed
        .into_inner()
        .expect("joined Linux report accumulator");
    let stop = finish_stop(
        boot,
        input,
        observed,
        RunEnd {
            path: "cpl0_set_threaded",
            names,
            retired,
            quanta: parallel.cpus.iter().map(|cpu| cpu.calls).sum(),
            wakes: parallel.cpus.iter().map(|cpu| cpu.wakes).sum(),
            idle_ns: parallel.idle_ns,
            termination,
            pm_before,
            pit_before,
        },
    )?;
    Ok(LinuxParallelRunReport {
        stop,
        vcpus,
        terminal_vcpu,
        threads_started: parallel.threads_started,
        threads_exited: parallel.threads_exited,
    })
}
