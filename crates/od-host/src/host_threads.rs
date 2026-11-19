//! S6-42b: the JIT host loop on the vCPU threads of the parallel mode (S6-34).
//!
//! [`JitHostQuantum`] is the quantum of every vCPU thread of `od_machine::run_host_threads`
//! (which runs the threads, the barrier, `stop_all`/`resume`, the kick and the clock charge of
//! `ParallelControl`). Each thread owns its session and its part of the process `MAP_JIT` region
//! (S6-42, A1: nothing of the session, the block or the handle crosses a thread; the session is
//! created on the vCPU's own thread from the part the owner handed out and dropped there, so a slot
//! is released by its owner). Each call runs the shared host loop: the JIT path and the
//! interpreter fallback over `&MachineBus` (RAM through the shared reference, the device lock for one
//! port access at a time, A7 of S6-40), with the run's stop poll next to the loop's own flags.
//!
//! The worker supplies its STOP byte and request poll. The loop clears the byte,
//! reads the requests, then gives the same byte to native code. A stop or a kick
//! returns to the worker on a safe point after native execution and accounting.
//!
//! S6-42c: [`JitHostQuantum::with_cpl0_set`] switches the quantum to the guest side of the
//! boundary (`run_paged_host_loop_shared_set`): a vector waiting in the vCPU's own Local APIC is
//! delivered through the guest IDT before every step, the interpreter step is the CPL0 profile set
//! (so `IRETQ`, `STI`, `LOCK` forms and the rest of the set run), a guest `#PF`/`#GP(0)` is
//! delivered through the guest IDT, and the split `LOCK` goes through the gate of the run. The
//! default quantum is the base profile as before (a guest fault or a refusal of the interpreter is
//! an error of the run). [`JitHostQuantum::nmi_paging`] gives the paging inputs for NMI delivery
//! through `od_machine::run_accounted_host_threads`.

use std::cell::RefCell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use od_isa::cpuid::CpuidProfile;
use od_isa::execute::{ControlRegisterPolicy, Cpl0ProfileSet};
use od_isa::{CliProfile, HaltProfile, MovCr3Long64V1, StiProfile};
use od_machine::{
    HostNmiPaging, HostQuantumCall, HostQuantumExit, HostQuantumFailure, HostQuantumResult,
    PagedProbeExit,
};

use crate::PagedFetchedLoadSession;
use crate::jit_region::{JitPartStats, JitRegionPart};
use crate::paged_host_loop::{
    LoopBusRef, PagedHostLoopConfig, PagedHostLoopControl, PagedHostLoopExit, PagedHostSetEnv,
    run_paged_host_loop_core,
};

/// What one vCPU's JIT host loop did, summed over its calls (published after every call, read by
/// any thread).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HostJitVcpuStats {
    pub calls: u64,
    pub retired_jit: u64,
    pub native_stop_exits: u64,
    pub retired_interpreter: u64,
    pub steps_jit: u64,
    pub steps_interpreter: u64,
    pub jit_refusals: u64,
    /// Calls that ended because the run's stop poll fired.
    pub stop_exits: u64,
    /// S6-42c (CPL0-set mode): external interrupts delivered to the guest.
    pub irqs_delivered: u64,
    /// S6-42c: guest `#PF`/`#GP(0)` delivered through the guest IDT.
    pub exceptions_delivered: u64,
    /// Native faults delivered through the guest IDT from their original fault records.
    pub jit_faults_delivered: u64,
    /// Retirements via the V4 atomic JIT, included in `retired_jit`.
    pub atomic_jit_steps: u64,
    /// Calls ended to reconcile a timer deadline or queued device work.
    pub device_boundaries: u64,
    /// Blocks compiled and published by the session so far.
    pub admissions: u64,
    /// The counters of the vCPU's part of the region at the end of the latest call.
    pub part: Option<JitPartStats>,
}

struct ThreadVcpu {
    owner: u64,
    session: PagedFetchedLoadSession,
    control: PagedHostLoopControl,
    rflags_defined: u64,
}

thread_local! {
    /// The session of the vCPU the current thread runs. A thread of the parallel run is made for
    /// one run and one vCPU, so the slot is empty at its first call. The slot is emptied by
    /// [`JitHostQuantum::thread_exit`] on the vCPU's own thread before the thread ends (the slot
    /// releases happen on the owner thread, A4); a `thread_local!` destructor would run after
    /// `thread::scope` has returned and keep the region live past the run (M1 of S6-42b).
    static THREAD_VCPU: RefCell<Option<ThreadVcpu>> = const { RefCell::new(None) };
}

/// Whether the calling thread holds a vCPU's session (the tests of the exit hook look at it).
#[cfg(test)]
pub(crate) fn thread_slot_occupied() -> bool {
    THREAD_VCPU.with(|cell| cell.borrow().is_some())
}

static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);

/// The host quantum of the JIT loop: one value shared by the threads of a run, holding the parts of
/// the region that the threads take and the per-vCPU configuration.
pub struct JitHostQuantum {
    owner: u64,
    parts: Mutex<Vec<Option<JitRegionPart>>>,
    configs: Vec<PagedHostLoopConfig>,
    rflags_defined: u64,
    stats: Vec<Mutex<HostJitVcpuStats>>,
    /// S6-43a: when enabled, the ranges every vCPU's calls reported as committed (page-walk
    /// Accessed/Dirty bits and data stores), in the order of that vCPU's program.
    write_log: Option<Vec<Mutex<Vec<(u64, u64)>>>>,
    /// S6-42c: the interpreter step is the CPL0 profile set with the guest side of the boundary.
    cpl0_set: bool,
    cpuid: Option<CpuidProfile>,
    /// S6-42c: the text of the latest failure a call returned as an error (the run's error is a
    /// `&'static str`; the detail of an undelivered interrupt or fault stays here).
    failure: Mutex<Option<String>>,
}

impl JitHostQuantum {
    /// `parts[cpu]` is the part of vCPU `cpu` (`None` uses the interpreter for the parallel run);
    /// `configs[cpu]` is its translation and access context; `rflags_defined` is the interpreter's
    /// initial definedness mask. Supply one entry in each vector per vCPU.
    pub fn new(
        parts: Vec<Option<JitRegionPart>>,
        configs: Vec<PagedHostLoopConfig>,
        rflags_defined: u64,
    ) -> Self {
        assert_eq!(
            parts.len(),
            configs.len(),
            "one part and one config per vCPU"
        );
        let stats = (0..configs.len())
            .map(|_| Mutex::new(HostJitVcpuStats::default()))
            .collect();
        Self {
            owner: NEXT_OWNER.fetch_add(1, Ordering::Relaxed),
            parts: Mutex::new(parts),
            configs,
            rflags_defined,
            stats,
            write_log: None,
            cpl0_set: false,
            cpuid: None,
            failure: Mutex::new(None),
        }
    }

    /// S6-42c: run the quantum with the guest side of the boundary (see the module documentation):
    /// interrupts of the vCPU's Local APIC, the CPL0 profile set as the interpreter step, guest
    /// faults delivered, the split `LOCK` behind the gate. The vCPU owns its CR3. The configuration
    /// supplies the initial CR3 for a state that has none.
    #[must_use]
    pub fn with_cpl0_set(mut self) -> Self {
        self.cpl0_set = true;
        self
    }

    /// Enable the CPL0 set and execute guest CPUID with the selected profile and the board's
    /// topology supplied by the host call.
    #[must_use]
    pub fn with_cpl0_cpuid(mut self, profile: CpuidProfile) -> Self {
        self.cpuid = Some(profile);
        self.with_cpl0_set()
    }

    /// Fallback roots for the root-only `od_machine::run_host_threads` entry.
    /// Use [`Self::nmi_paging`] to also supply the quantum's EFER.
    pub fn nmi_roots(&self) -> Vec<Option<u64>> {
        self.configs.iter().map(|config| Some(config.cr3)).collect()
    }

    /// Supplies NMI delivery with the configured EFER and fallback CR3. A CPU-owned CR3 takes
    /// precedence after the vCPU starts running the CPL0 set.
    pub fn nmi_paging(&self) -> Vec<Option<HostNmiPaging>> {
        self.configs
            .iter()
            .map(|config| {
                Some(HostNmiPaging {
                    root: config.cr3,
                    efer: config.efer,
                })
            })
            .collect()
    }

    /// S6-43a: keep, per vCPU, the committed write ranges `(start, length)` of every call
    /// ([`Self::committed_writes`]). Off by default: the hot path then collects nothing.
    #[must_use]
    pub fn with_write_log(mut self) -> Self {
        self.write_log = Some(
            (0..self.configs.len())
                .map(|_| Mutex::new(Vec::new()))
                .collect(),
        );
        self
    }

    /// The committed write ranges of vCPU `cpu` in program order (empty unless
    /// [`Self::with_write_log`] was used).
    pub fn committed_writes(&self, cpu: usize) -> Vec<(u64, u64)> {
        self.write_log
            .as_ref()
            .map(|log| {
                log[cpu]
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
            })
            .unwrap_or_default()
    }

    /// The thread's exit hook: pass `&|_| quantum.thread_exit()` as `on_thread_exit` of
    /// `od_machine::run_host_threads`. It runs on the vCPU's own thread after its last call and
    /// drops the session and the part of the region there, so the lease of the slot and the
    /// region's `MAP_JIT` mapping are released before the run returns, not by a `thread_local!`
    /// destructor at some later time. Does nothing on a thread that ran no call of this quantum.
    pub fn thread_exit(&self) {
        let session = THREAD_VCPU
            .try_with(|cell| {
                let mut slot = cell.borrow_mut();
                if slot
                    .as_ref()
                    .is_some_and(|thread| thread.owner == self.owner)
                {
                    slot.take()
                } else {
                    None
                }
            })
            .ok()
            .flatten();
        drop(session);
    }

    /// The detail of the latest failure of a call (an interrupt that could not be delivered, a
    /// guest fault the guest IDT did not take, an exit the CPL0 set cannot resume), if any.
    pub fn last_failure(&self) -> Option<String> {
        self.failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The counters of vCPU `cpu` (the totals of the calls that have returned).
    pub fn stats(&self, cpu: usize) -> HostJitVcpuStats {
        *self.stats[cpu]
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Compatibility entry for callers that consume only the terminal error message.
    pub fn run(&self, call: HostQuantumCall<'_>) -> Result<HostQuantumResult, &'static str> {
        self.run_inner(call, false)
            .map_err(|failure| failure.message)
    }

    /// One quantum for `od_machine::run_accounted_host_threads`.
    pub fn run_accounted(
        &self,
        call: HostQuantumCall<'_>,
    ) -> Result<HostQuantumResult, HostQuantumFailure> {
        self.run_inner(call, true)
    }

    fn run_inner(
        &self,
        mut call: HostQuantumCall<'_>,
        account_failure: bool,
    ) -> Result<HostQuantumResult, HostQuantumFailure> {
        let cpu = call.cpu;
        let config = *self
            .configs
            .get(cpu)
            .ok_or("host JIT quantum: no configuration for the vCPU")?;
        THREAD_VCPU.with(|cell| {
            let mut slot = cell.borrow_mut();
            if slot
                .as_ref()
                .is_none_or(|thread| thread.owner != self.owner)
            {
                let part = self
                    .parts
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get_mut(cpu)
                    .and_then(Option::take);
                let session = match part {
                    Some(part) => PagedFetchedLoadSession::with_region_part(part),
                    None => PagedFetchedLoadSession::new(),
                };
                *slot = Some(ThreadVcpu {
                    owner: self.owner,
                    session,
                    control: PagedHostLoopControl::new(),
                    rflags_defined: self.rflags_defined,
                });
            }
            let thread = slot.as_mut().expect("the thread's vCPU was just set");
            let logging = self.write_log.is_some();
            let mut logged = Vec::new();
            let mut record = |range: od_machine::GuestPhysicalRange| {
                if logging {
                    logged.push((range.start().get(), range.len()));
                }
            };
            let save_log = |logged| {
                if let Some(log) = &self.write_log {
                    log[cpu]
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .extend(logged);
                }
            };
            if call.nvme_only {
                let pending = call
                    .devices
                    .service_nvme_until(&mut *call.stop, &mut record);
                let exit = if let Some(reason) = call.devices.stop_request() {
                    HostQuantumExit::MachineStop(reason)
                } else if pending {
                    HostQuantumExit::NvmePending { halted: false }
                } else {
                    HostQuantumExit::Running
                };
                save_log(logged);
                self.stats[cpu]
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .calls += 1;
                return Ok(HostQuantumResult {
                    retired: 0,
                    exit,
                    device_ns: call.devices.service(),
                });
            }
            let (cli, sti, halt) = (
                CliProfile::NonPviLong64V1,
                StiProfile::NonPviLong64V1,
                HaltProfile::Long64V1,
            );
            let set;
            let bus = if self.cpl0_set {
                // The set the interpreter step runs: the Linux board's CPL0 profile without the
                // members that need the board's devices or MSR cells.
                let base_set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
                    .with_cpu_owned_cr3(MovCr3Long64V1)
                    .with_cli(&cli)
                    .with_sti(&sti)
                    .with_halt(&halt)
                    .with_pio_u8();
                set = match &self.cpuid {
                    Some(profile) => base_set
                        .with_cpuid(profile)
                        .with_cpuid_topology(cpu as u8, call.vcpu_count),
                    None => base_set,
                };
                LoopBusRef::SharedSet(
                    call.bus,
                    cpu,
                    PagedHostSetEnv {
                        lapic: call.lapic,
                        board_irq: call.board_irq.reborrow(),
                        devices: call.devices,
                        gate: call.gate,
                        set: &set,
                        skip_first_boundary: &mut *call.skip_first_boundary,
                    },
                )
            } else {
                LoopBusRef::Shared(call.bus, cpu)
            };
            let report = run_paged_host_loop_core(
                &mut thread.session,
                &thread.control,
                config,
                call.state,
                bus,
                &mut thread.rflags_defined,
                call.consecutive_faults,
                call.budget,
                &mut record,
                |_, _| {},
                &mut *call.stop,
                call.native_stop,
            );
            {
                let mut stats = self.stats[cpu]
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                stats.calls += 1;
                stats.retired_jit += report.retired_jit;
                stats.native_stop_exits += report.native_stop_exits;
                stats.retired_interpreter += report.retired_interpreter;
                stats.steps_jit += report.steps_jit;
                stats.steps_interpreter += report.steps_interpreter;
                stats.jit_refusals += report.jit_refusals;
                stats.stop_exits += u64::from(report.exit == PagedHostLoopExit::StopRequested);
                stats.irqs_delivered += report.irqs_delivered;
                stats.exceptions_delivered += report.exceptions_delivered;
                stats.jit_faults_delivered += report.jit_faults_delivered;
                stats.atomic_jit_steps += report.atomic_jit_steps;
                stats.device_boundaries +=
                    u64::from(report.exit == PagedHostLoopExit::DeviceBoundary);
                stats.admissions = thread.session.admissions();
                stats.part = thread.session.region_part().map(|part| part.stats());
            }
            let retired = report.retired_total();
            let detail = match &report.exit {
                PagedHostLoopExit::IrqDeliveryFailed(text)
                | PagedHostLoopExit::InterpreterIntegration(text) => Some(text.clone()),
                PagedHostLoopExit::GuestFault(fault) => Some(format!(
                    "{fault:?}; page_fault: {:?}; rip {:#x}",
                    report.page_fault, call.state.rip
                )),
                PagedHostLoopExit::FaultDeliveryFailed { fault, failure } => Some(format!(
                    "{fault:?}; delivery: {failure:?}; page_fault: {:?}; rip {:#x}",
                    report.page_fault, call.state.rip
                )),
                PagedHostLoopExit::Interpreter(exit)
                    if !matches!(exit, PagedProbeExit::Halted | PagedProbeExit::Shutdown) =>
                {
                    Some(format!("{exit:?}; page_fault: {:?}", report.page_fault))
                }
                PagedHostLoopExit::AtomicHostFailure(error) => Some(format!("{error:?}")),
                _ => None,
            };
            if let Some(detail) = detail {
                *self
                    .failure
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(detail);
            }
            let service_nvme = matches!(
                report.exit,
                PagedHostLoopExit::DeviceBoundary
                    | PagedHostLoopExit::BudgetExhausted
                    | PagedHostLoopExit::Interpreter(PagedProbeExit::Halted)
            );
            let mut exit = match report.exit {
                PagedHostLoopExit::BudgetExhausted
                | PagedHostLoopExit::StopRequested
                | PagedHostLoopExit::FinishRequested
                | PagedHostLoopExit::EventPending
                | PagedHostLoopExit::DeviceBoundary => Ok(HostQuantumExit::Running),
                PagedHostLoopExit::Interpreter(PagedProbeExit::Halted) => {
                    Ok(HostQuantumExit::Halted)
                }
                PagedHostLoopExit::Interpreter(PagedProbeExit::Shutdown) => {
                    Ok(HostQuantumExit::Shutdown)
                }
                PagedHostLoopExit::MachineStop(reason) => Ok(HostQuantumExit::MachineStop(reason)),
                PagedHostLoopExit::Interpreter(_) => {
                    Err("host JIT quantum: the interpreter refused the instruction")
                }
                PagedHostLoopExit::GuestFault(_) => Err(if self.cpl0_set {
                    "host JIT quantum: a guest fault the guest IDT did not take"
                } else {
                    "host JIT quantum: a guest fault (no delivery in this quantum)"
                }),
                PagedHostLoopExit::IrqDeliveryFailed(_) => {
                    Err("host JIT quantum: an interrupt could not be delivered")
                }
                PagedHostLoopExit::FaultDeliveryFailed { .. } => {
                    Err("host JIT quantum: a fault could not be delivered")
                }
                PagedHostLoopExit::InterpreterIntegration(_) => {
                    Err("host JIT quantum: the CPL0 set returned an exit it cannot resume")
                }
                PagedHostLoopExit::JitHostFailure(_) | PagedHostLoopExit::AtomicHostFailure(_) => {
                    Err("host JIT quantum: the JIT boundary failed")
                }
                PagedHostLoopExit::InvalidRflagsDefined { .. } => {
                    Err("host JIT quantum: invalid definedness mask")
                }
            };
            let device_ns = if self.cpl0_set && (account_failure || exit.is_ok()) {
                if service_nvme
                    && exit.is_ok()
                    && call
                        .devices
                        .service_nvme_until(&mut *call.stop, &mut record)
                {
                    exit = Ok(HostQuantumExit::NvmePending {
                        halted: exit == Ok(HostQuantumExit::Halted),
                    });
                }
                call.devices.service()
            } else {
                0
            };
            save_log(logged);
            match exit {
                Ok(exit) => Ok(HostQuantumResult {
                    retired,
                    exit,
                    device_ns,
                }),
                Err(message) => Err(HostQuantumFailure {
                    retired,
                    device_ns,
                    message,
                }),
            }
        })
    }
}
