//! LinuxDirect on the interleaved board scheduler.
use super::observed::{ObservedRun, RunEnd, RunTermination, finish_stop, run_error};
use super::*;
use crate::fetch_translation_cache::{FetchCacheInvalidation, FetchTranslationCache};
use crate::smp_scheduler::{BoardSmpHost, Finished, QuantumPolicy, SmpScheduler, StopRequest};

#[derive(Debug, Eq, PartialEq)]
pub struct LinuxInterleavedRunReport {
    pub stop: LinuxStopRecord,
    pub vcpus: Vec<LinuxInterleavedVcpu>,
    /// The CPU that produced a terminal guest instruction, if the scheduler stopped on one.
    pub terminal_vcpu: Option<usize>,
}

pub type LinuxInterleavedVcpu = LinuxVcpuRunState;

/// Run a diskless APIC LinuxDirect board with seeded quanta and one shared instruction budget.
/// INIT/SIPI and the AP trampoline run inside the same scheduler as long-mode Linux.
pub fn run_linux_direct_board_set_user_mode_interleaved(
    boot: &mut LinuxDirectBoot,
    input: &LinuxDirectInput<'_>,
    instruction_budget: u64,
    mut members: LinuxCpl0SetMembers,
    seed: u64,
) -> Result<LinuxInterleavedRunReport, LinuxDirectError> {
    if members.cpuid_profile != CpuidProfile::X8664V1Stage4AApic
        || boot.board.nvme().is_some()
        || members.diagnostic_past_c7
    {
        return Err(run_error(
            "interleaved SMP LinuxDirect requires the diskless APIC profile",
            0,
        ));
    }
    prepare_apic_profile(&boot.board, &mut members)?;
    boot.board
        .enable_clock_owner()
        .map_err(|error| run_error(error, 0))?;
    install_linux_ioapic_sources(&mut boot.board)?;
    boot.board
        .import_linux_pci_address()
        .map_err(|error| run_error(error, 0))?;
    let count = boot.board.vcpu_count();
    let root = boot.board.cr3();
    let bsp = boot.board.vcpu_mut(BOOT_VCPU_INDEX).expect("validated BSP");
    bsp.state.cr3.get_or_insert(root);
    let profile = LinuxCpl0Profile::new(members, true, boot.board.efer());
    let mut names = Vec::new();
    let unused_pci_cell = Cell::new(0);
    profile.bind(
        &LinuxVcpuCells::from_vcpu(boot.board.vcpu(BOOT_VCPU_INDEX).expect("validated BSP")),
        &unused_pci_cell,
        |member| names.push(member.to_string()),
    );
    let pit_before = boot.board.pit_read_counts();
    let pm_before = boot.board.acpi_pm().map(|pm| pm.timer_reads());
    let mut observed = ObservedRun::multiprocessor();
    let mut quanta = 0;
    let mut faults = vec![0; count];
    let fetch_caches = members.fetch_cache.then(|| {
        (0..count)
            .map(|_| {
                FetchTranslationCache::persistent(FetchCacheInvalidation::WriteWatch)
                    .with_insert_generations()
            })
            .collect::<Vec<_>>()
    });
    let mut stop_reason = boot.board.acpi_pm().and_then(|pm| pm.stop_request());
    let stop_request = StopRequest::new();
    if stop_reason.is_some() {
        stop_request.request();
    }
    let mut scheduler = SmpScheduler::new(count, QuantumPolicy::seeded(seed), stop_request.clone())
        .map_err(|error| run_error(error, 0))?;
    let schedule = {
        let mut host = BoardSmpHost::new(
            &mut boot.board,
            |board: &mut LoadedFirmwareBoard,
             cpu: usize,
             budget,
             stop: &mut dyn FnMut() -> bool| {
                quanta += 1;
                let cells = LinuxVcpuCells::from_vcpu(board.vcpu(cpu).expect("scheduled vCPU"));
                let set = profile.bind(&cells, &unused_pci_cell, |_| {});
                let report = board.run_exclusive_set_quantum(
                    cpu,
                    budget,
                    stop,
                    &set,
                    Some(&mut faults[cpu]),
                    fetch_caches.as_ref().map(|caches| &caches[cpu]),
                );
                let vcpu = board.vcpu_mut(cpu).expect("scheduled vCPU");
                cells.commit(vcpu);
                let progress = observed.record(report?, vcpu, false)?;
                stop_reason = board.acpi_pm().and_then(|pm| pm.stop_request());
                if stop_reason.is_some() || observed.terminal_vcpu().is_some() {
                    stop_request.request();
                }
                Ok(progress)
            },
        )
        .map_err(|error| run_error(error, 0))?;
        scheduler.run(&mut host, instruction_budget)
    };
    let retired = scheduler.retired_total();
    let views = boot.board.sync_linux_boot_views();
    let terminal_vcpu = observed.terminal_vcpu();
    let termination = match (
        schedule.map_err(|error| run_error(error, retired))?,
        terminal_vcpu,
    ) {
        (Finished::Stopped, Some(cpu)) => RunTermination::Guest(cpu),
        (Finished::Stopped, None) if stop_reason.is_some() => {
            RunTermination::Scheduler(PagedProbeExit::Stopped, stop_reason)
        }
        (Finished::BudgetExhausted, _) => {
            RunTermination::Scheduler(PagedProbeExit::BudgetExhausted, None)
        }
        (Finished::AllIdle, _) => RunTermination::Scheduler(PagedProbeExit::Halted, None),
        (reason, _) => return Err(run_error(reason, retired)),
    };
    views.map_err(|error| run_error(error, retired))?;
    let counters = scheduler.counters();
    let vcpus = counters
        .iter()
        .enumerate()
        .map(|(index, counter)| LinuxVcpuRunState::capture(&boot.board, index, counter.retired))
        .collect();
    let stop = finish_stop(
        boot,
        input,
        observed,
        RunEnd {
            path: "cpl0_set_interleaved",
            names,
            retired,
            quanta,
            wakes: counters.iter().map(|cpu| cpu.wakes).sum(),
            idle_ns: scheduler.idle().0,
            termination,
            pm_before,
            pit_before,
        },
    )?;
    Ok(LinuxInterleavedRunReport {
        stop,
        vcpus,
        terminal_vcpu,
    })
}
