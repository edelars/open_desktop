//! S4-27i4b: the paged backend of the event pump.
//!
//! `PagedBackend` runs the pump's CPU steps on the paged Long64/CPL0 profile of
//! S4-27h/S4-27i4a (4 KiB pages, CR0/CR4 = BASE, EFER = 0xD00, one vCPU, private
//! synchronous RAM): the exact paged `STI`, `HLT`, `IRETQ` and the handler's EOI
//! store `MOV [RBX],EAX` (`89 03`), and the wake and boundary entry through
//! `poll_paged_apic_entry` (the S4-27h delivery). It adds no instruction and does
//! not widen any allowlist: an instruction outside these four is a terminal
//! `Unsupported`, as on the flat pump. The opcode at RIP is read with tracked
//! RAM fetches of the same walker: one fetch for a one-byte opcode, two
//! byte-wise fetches for `48 CF` and `89 03` (A bits are set exactly as by the
//! instruction's own fetch of the same bytes).
//!
//! Differences from the flat backend, all recorded as limits:
//! - a boundary entry only enters the handler (the first handler instruction is the
//!   next step), like the flat pump in `with_entry_only_delivery` mode;
//! - the backend owns a plain virtual clock (the paged board has no session); the
//!   Local APIC of the paged image has no timer, so only the IRQ queue moves time;
//! - the owner hook (`PumpHost`) is not used, `CLI`, REP and ordinary instructions
//!   do not exist here, and the injections `SkipShadowCheck` and `HltAsOrdinary`
//!   have no paged counterpart (the S4-27h entry checks the shadow itself);
//! - a refused entry before commit is `Rejected(HaltWake | InterruptEntry)` and an
//!   aborted entry (table or stack access fault) is terminal
//!   `InterruptDeliveryAborted` (model; hardware raises #PF/#NP, nested delivery
//!   is S4-33).

use crate::paged_apic_entry::{PagedApicEntryResult, poll_paged_apic_entry};
use crate::stage4_paged_irq_probe::context;
use crate::{
    AccessWidth, BackendEnv, BackendFlow, BackendStep, DiagnosticHaltWaitError,
    DiagnosticTerminalExit, GuestLinearAddress, GuestPhysicalAddress, Long64ProbeConfig,
    MachineBus, PumpBackend, PumpEntrySource, PumpExit, PumpRejection, PumpTraceKind,
    RamAccessOperation, TrackedAccessError, TrackedAccessOutcome, TrackedAccessRequest,
    VirtualClock, step_paged_hlt, step_paged_iretq, step_paged_mov_dword_store, step_paged_sti,
};
use od_isa::memory::MemoryError;
use od_isa::{CpuActivity, INTEGER_FLAGS, LongModeState};

pub struct PagedBackend {
    state: LongModeState,
    bus: MachineBus,
    clock: Option<VirtualClock>,
    cr3: u64,
    efer: u64,
    ram_bytes: u64,
    stopped: bool,
    /// Ranges reported through the committed-write callback (walker A/D stores
    /// and RAM stores), for evidence.
    committed_writes: u64,
}

impl PagedBackend {
    /// The caller certifies the S4-27h profile: the state is Long64/CPL0 with the
    /// paged CR0/CR4, `cr3` and `efer` are the profile's, and RAM is `[0, ram_bytes)`.
    pub fn new(
        state: LongModeState,
        bus: MachineBus,
        clock: Option<VirtualClock>,
        cr3: u64,
        efer: u64,
        ram_bytes: u64,
    ) -> Self {
        Self {
            state,
            bus,
            clock,
            cr3,
            efer,
            ram_bytes,
            stopped: false,
            committed_writes: 0,
        }
    }

    /// The guest-physical bus, for owner-side handoff writes between pump runs
    /// (for example re-programming the Local APIC after a global disable).
    pub fn bus_mut(&mut self) -> &mut MachineBus {
        &mut self.bus
    }

    pub fn committed_writes(&self) -> u64 {
        self.committed_writes
    }

    fn backend_error(detail: String) -> BackendStep {
        BackendStep::exit(PumpExit::Terminal(DiagnosticTerminalExit::BackendError(
            MemoryError::Backend(detail),
        )))
    }

    fn fault(record: &od_isa::memory::PageFaultRecord) -> BackendStep {
        BackendStep::exit(PumpExit::Terminal(DiagnosticTerminalExit::Fault(
            record.fault().clone(),
        )))
    }

    /// One opcode byte at `RIP + offset` through the tracked RAM walker.
    fn fetch(&mut self, offset: u64) -> Result<u8, BackendStep> {
        let paging = Long64ProbeConfig::from_cpu_state(&self.state, self.cr3, self.efer)
            .map_err(|error| Self::backend_error(format!("{error:?}")))?;
        let mut writes = 0;
        let report = self.bus.access_tracked_ram(
            &paging,
            TrackedAccessRequest {
                instruction_rip: self.state.rip,
                address: GuestLinearAddress::new(self.state.rip.wrapping_add(offset)),
                context: context(),
                operation: RamAccessOperation::Fetch,
            },
            |_| writes += 1,
        );
        self.committed_writes += writes;
        match report.outcome {
            Ok(TrackedAccessOutcome::Fetched(byte)) => Ok(byte),
            Ok(other) => Err(Self::backend_error(format!("non-fetch result {other:?}"))),
            Err(TrackedAccessError::PageFault { record, .. }) => Err(Self::fault(&record)),
            Err(error) => Err(Self::backend_error(format!("{error:?}"))),
        }
    }

    /// Account a one-instruction CPU step; `event` is recorded when it retired.
    fn retire(
        &mut self,
        retired: bool,
        page_fault: Option<od_isa::memory::PageFaultRecord>,
        detail: impl FnOnce() -> String,
        event: Option<PumpTraceKind>,
    ) -> BackendStep {
        if !retired {
            return match page_fault {
                Some(record) => Self::fault(&record),
                None => Self::backend_error(detail()),
            };
        }
        let mut out = BackendStep::new(BackendFlow::Continue);
        out.retired = 1;
        out.events.extend(event);
        out
    }

    fn execute(&mut self) -> BackendStep {
        let first = match self.fetch(0) {
            Ok(byte) => byte,
            Err(step) => return step,
        };
        let (cr3, efer) = (self.cr3, self.efer);
        let mut writes = 0;
        let step = match first {
            0xfb | 0xf4 => {
                let report = if first == 0xfb {
                    step_paged_sti(
                        &mut self.state,
                        &mut self.bus,
                        cr3,
                        efer,
                        context(),
                        INTEGER_FLAGS,
                        |_| writes += 1,
                    )
                } else {
                    step_paged_hlt(
                        &mut self.state,
                        &mut self.bus,
                        cr3,
                        efer,
                        context(),
                        INTEGER_FLAGS,
                        |_| writes += 1,
                    )
                };
                match report {
                    Ok(report) => self.retire(
                        report.cpu.instructions_executed == 1,
                        report.page_fault,
                        || format!("{:?} {:?}", report.cpu.exit, report.access_error),
                        Some(if first == 0xfb {
                            PumpTraceKind::Sti
                        } else {
                            PumpTraceKind::Hlt
                        }),
                    ),
                    Err(error) => Self::backend_error(format!("{error:?}")),
                }
            }
            0x48 | 0x89 => {
                let second = match self.fetch(1) {
                    Ok(byte) => byte,
                    Err(step) => return step,
                };
                match (first, second) {
                    (0x48, 0xcf) => {
                        match step_paged_iretq(
                            &mut self.state,
                            &mut self.bus,
                            cr3,
                            efer,
                            context(),
                            INTEGER_FLAGS,
                            |_| writes += 1,
                        ) {
                            Ok(report) => match report.cpu {
                                od_isa::iretq::IretqResult::Retired => {
                                    self.retire(true, None, String::new, Some(PumpTraceKind::Iretq))
                                }
                                od_isa::iretq::IretqResult::Unsupported => BackendStep::exit(
                                    PumpExit::Terminal(DiagnosticTerminalExit::IretqUnsupported),
                                ),
                                od_isa::iretq::IretqResult::Fault(fault) => BackendStep::exit(
                                    PumpExit::Terminal(DiagnosticTerminalExit::Fault(fault)),
                                ),
                                od_isa::iretq::IretqResult::ReadAborted { phase, cause } => {
                                    BackendStep::exit(PumpExit::Terminal(
                                        DiagnosticTerminalExit::IretqReadAborted { phase, cause },
                                    ))
                                }
                            },
                            Err(error) => Self::backend_error(format!("{error:?}")),
                        }
                    }
                    (0x89, 0x03) => {
                        match step_paged_mov_dword_store(
                            &mut self.state,
                            &mut self.bus,
                            cr3,
                            efer,
                            context(),
                            INTEGER_FLAGS,
                            |_| writes += 1,
                        ) {
                            Ok(report) => self.retire(
                                report.cpu.instructions_executed == 1,
                                report.page_fault,
                                || format!("{:?} {:?}", report.cpu.exit, report.memory_error),
                                None,
                            ),
                            Err(error) => Self::backend_error(format!("{error:?}")),
                        }
                    }
                    _ => {
                        BackendStep::exit(PumpExit::Terminal(DiagnosticTerminalExit::Unsupported {
                            opcode: first,
                        }))
                    }
                }
            }
            opcode => BackendStep::exit(PumpExit::Terminal(DiagnosticTerminalExit::Unsupported {
                opcode,
            })),
        };
        self.committed_writes += writes;
        step
    }
}

impl PumpBackend for PagedBackend {
    fn state(&self) -> &LongModeState {
        &self.state
    }

    fn is_waiting(&self) -> bool {
        self.state.activity == Some(CpuActivity::Halted)
    }

    fn virtual_time_ns(&self) -> Option<u64> {
        self.clock.as_ref().map(|clock| clock.now_ns())
    }

    fn advance_time_ns(&mut self, delta_ns: u64) -> Result<(), PumpRejection> {
        let clock = self.clock.as_mut().ok_or(PumpRejection::TimerAdvance(
            DiagnosticHaltWaitError::Clock(crate::VirtualClockError::ClockNotConfigured),
        ))?;
        clock
            .advance_time_ns(delta_ns)
            .map_err(|error| PumpRejection::TimerAdvance(DiagnosticHaltWaitError::Clock(error)))
    }

    fn stop(&mut self) {
        self.stopped = true;
    }

    fn peek_ram_qword(&mut self, gpa: u64) -> Option<u64> {
        // RAM only: an MMIO read would reach a device.
        if gpa.checked_add(8)? > self.ram_bytes {
            return None;
        }
        self.bus
            .read_physical(GuestPhysicalAddress::new(gpa), AccessWidth::Qword)
            .ok()
    }

    fn step(&mut self, env: BackendEnv<'_>) -> BackendStep {
        if self.stopped {
            return BackendStep::exit(PumpExit::Stopped);
        }
        let halted = self.is_waiting();
        let mut writes = 0;
        let polled = poll_paged_apic_entry(
            &mut self.state,
            &mut self.bus,
            env.apic,
            self.cr3,
            self.efer,
            context(),
            INTEGER_FLAGS,
            |_| writes += 1,
        );
        self.committed_writes += writes;
        let report = match polled {
            Ok(report) => report,
            Err(error) => return Self::backend_error(format!("{error:?}")),
        };
        match report.result {
            PagedApicEntryResult::Entered { vector, .. } => {
                let mut out = BackendStep::new(BackendFlow::Continue);
                out.events.push(PumpTraceKind::Entry {
                    vector,
                    source: if halted {
                        PumpEntrySource::HaltWake
                    } else {
                        PumpEntrySource::Boundary
                    },
                });
                out
            }
            PagedApicEntryResult::Idle if halted => BackendStep::new(BackendFlow::Idle),
            PagedApicEntryResult::Idle => self.execute(),
            PagedApicEntryResult::OutsideProfile(reason) => {
                BackendStep::exit(PumpExit::Rejected(if halted {
                    PumpRejection::HaltWake(reason)
                } else {
                    PumpRejection::InterruptEntry(reason)
                }))
            }
            PagedApicEntryResult::Aborted { phase, cause } => {
                BackendStep::exit(PumpExit::Terminal(
                    DiagnosticTerminalExit::InterruptDeliveryAborted { phase, cause },
                ))
            }
            PagedApicEntryResult::SelectionChanged => BackendStep::exit(PumpExit::Terminal(
                DiagnosticTerminalExit::InterruptSelectionChanged,
            )),
        }
    }
}
