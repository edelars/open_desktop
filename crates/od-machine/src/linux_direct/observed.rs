use super::*;
use crate::FirmwareBoardError;
use crate::firmware_board::set_exit_to_probe_exit;
use crate::firmware_board::vcpu_quantum::{
    SetQuantumReport, is_quantum_boundary, quantum_progress,
};
use crate::paged_memory_adapter::{PagedCpl0SetExit, SyscallEvent, SyscallUserContext};
use crate::smp_scheduler::{Executed, RunFailure};
use crate::vcpu::Vcpu;
use od_isa::PrivilegeLevel;

#[derive(Default)]
pub(super) struct ObservedRun {
    irq: IrqCounters,
    rdtsc: [u64; 4],
    changes: Vec<TranslationContextChange>,
    cpl3_retired: u64,
    journal: SyscallJournal,
    syscall_callers: Option<[Option<SyscallUserContext>; SYSCALL_JOURNAL_CAPACITY]>,
    exceptions: u64,
    page_faults: u64,
    protection_faults: u64,
    terminal: Option<Terminal>,
}

struct Terminal {
    cpu: usize,
    exit: PagedProbeExit,
    detail: String,
    page_fault: Option<od_isa::memory::PageFaultRecord>,
    memory_error: Option<crate::TrackedAccessError>,
    user_entry: Option<IretqUserEntry>,
}

impl ObservedRun {
    pub(super) fn interleaved() -> Self {
        Self {
            syscall_callers: Some([None; SYSCALL_JOURNAL_CAPACITY]),
            ..Self::default()
        }
    }

    fn record_syscall(&mut self, event: SyscallEvent) {
        let Some(callers) = &mut self.syscall_callers else {
            self.journal.record(event);
            return;
        };
        let capacity = SYSCALL_JOURNAL_CAPACITY as u64;
        let next = self.journal.entered;
        match event {
            SyscallEvent::Enter { caller, .. } => {
                callers[(next % capacity) as usize] = Some(caller);
                self.journal.record(event);
            }
            SyscallEvent::Exit { caller, .. } => {
                // A Linux task can return on a different vCPU. Match its restored user context.
                let first = next - self.journal.entries.len() as u64;
                self.journal.record_matching(event, |index| {
                    callers[((first + index as u64) % capacity) as usize] == Some(caller)
                });
            }
        }
    }

    pub(super) fn record(
        &mut self,
        report: SetQuantumReport,
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
            self.record_syscall(event);
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
                cpu: vcpu.index,
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

pub(super) fn run_error(detail: impl std::fmt::Debug, retired: u64) -> LinuxDirectError {
    let mut error = LinuxDirectError::new("run_failed", "run", detail);
    error.retired = Some(retired);
    error
}

impl ObservedRun {
    pub(super) fn terminal_vcpu(&self) -> Option<usize> {
        self.terminal.as_ref().map(|terminal| terminal.cpu)
    }
}

/// Device observations and the scheduler outcome for a completed LinuxDirect run.
pub(super) struct RunEnd {
    pub(super) path: &'static str,
    pub(super) names: Vec<String>,
    pub(super) retired: u64,
    pub(super) quanta: u64,
    pub(super) wakes: u64,
    pub(super) idle_ns: u64,
    pub(super) exit: Option<PagedProbeExit>,
    pub(super) stop_reason: Option<crate::MachineStopReason>,
    pub(super) pm_before: Option<u64>,
    pub(super) pit_before: Option<[u64; 2]>,
}

pub(super) fn finish_stop(
    boot: &mut LinuxDirectBoot,
    input: &LinuxDirectInput<'_>,
    mut observed: ObservedRun,
    end: RunEnd,
) -> Result<LinuxStopRecord, LinuxDirectError> {
    let retired = end.retired;
    let terminal = match end.exit {
        Some(exit) => Terminal {
            cpu: BOOT_VCPU_INDEX,
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
    observed.irq.halt_wakes = end.wakes;
    observed.irq.halt_idle_ns = end.idle_ns;
    let pit_reads = end
        .pit_before
        .zip(boot.board.pit_read_counts())
        .map(|(before, after)| [after[0] - before[0], after[1] - before[1]]);
    let clock = boot.board.clock();
    let run = FirmwareRunReport {
        exit: terminal.exit,
        page_fault: terminal.page_fault,
        memory_error: terminal.memory_error,
        retired,
        quanta: end.quanta,
        com1: boot.board.com1_transcript().to_vec(),
        virtual_ns: clock.now_ns(),
        tsc_hz: clock.tsc_hz(),
        rdtsc_reads: Some(observed.rdtsc.iter().sum()),
        rdtsc_reads_by_cpl: Some(observed.rdtsc),
        pit_port_b_reads: pit_reads.map(|read| read[0]),
        pit_counter_port_reads: pit_reads.map(|read| read[1]),
        stop_reason: end.stop_reason,
    };
    let vcpu = boot
        .board
        .vcpu(terminal.cpu)
        .expect("reporting vCPU exists");
    let privilege = if vcpu.state.cs_selector.is_some_and(|cs| cs & 3 == 3) {
        PrivilegeLevel::Ring3
    } else {
        PrivilegeLevel::Ring0
    };
    let view = boot.board.fetch_vcpu_view_2m(
        terminal.cpu,
        vcpu.state.rip,
        LINUX_STOP_RIP_BYTES,
        privilege,
        vcpu.guest_efer,
    );
    Ok(stop_record_with_view(
        boot,
        input,
        run,
        end.pm_before,
        SetExtras {
            path: end.path,
            members: end.names,
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
        terminal.cpu,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleaved_syscalls_match_the_restored_user_context() {
        let mut run = ObservedRun::interleaved();
        let first = SyscallUserContext {
            cr3: 0x1000,
            rip: 0x2000,
            rsp: 0x3000,
        };
        let second = SyscallUserContext {
            cr3: 0x4000,
            ..first
        };
        run.record_syscall(SyscallEvent::Enter {
            nr: 1,
            caller: first,
        });
        run.record_syscall(SyscallEvent::Enter {
            nr: 2,
            caller: second,
        });
        run.record_syscall(SyscallEvent::Exit {
            rax: 3,
            caller: first,
        });
        run.record_syscall(SyscallEvent::Exit {
            rax: 4,
            caller: second,
        });
        assert_eq!(run.journal.entries[0].rax_return, Some(3));
        assert_eq!(run.journal.entries[1].rax_return, Some(4));

        for nr in 0..SYSCALL_JOURNAL_CAPACITY as u64 {
            run.record_syscall(SyscallEvent::Enter {
                nr,
                caller: SyscallUserContext {
                    rsp: 0x5000 + nr,
                    ..first
                },
            });
        }
        run.record_syscall(SyscallEvent::Exit {
            rax: 5,
            caller: first,
        });
        assert!(
            run.journal
                .entries
                .iter()
                .all(|entry| entry.rax_return.is_none())
        );
        assert_eq!(run.journal.returned, 3);
    }
}
