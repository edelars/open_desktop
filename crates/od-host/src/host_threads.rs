//! S6-42b: the JIT host loop on the vCPU threads of the parallel mode (S6-34).
//!
//! [`JitHostQuantum`] is the quantum of every vCPU thread of `od_machine::run_host_threads`
//! (which runs the threads, the barrier, `stop_all`/`resume`, the kick and the clock charge of
//! `ParallelControl`). Each thread owns its session and its part of the process `MAP_JIT` region
//! (S6-42, A1: nothing of the session, the block or the handle crosses a thread; the session is
//! created on the vCPU's own thread from the part the owner handed out and dropped there, so a slot
//! is released by its owner). Every call runs [`run_paged_host_loop_shared`]: the JIT path and the
//! interpreter fallback over `&MachineBus` (RAM through the shared reference, the device lock for one
//! port access at a time, A7 of S6-40), with the run's stop poll next to the loop's own flags.
//!
//! The stop of `ParallelControl::stop_all` reaches the loop as the poll at the top of every
//! iteration: a block is one instruction, so a native call is never longer than one instruction and
//! the thread acknowledges on the safe point after its quantum returned, never inside a native call.
//! The STOP byte that native code polls (`RuntimeContextV1::stop_requested`) is not raised by this
//! binding: it is needed only for blocks of several instructions (S3-24) and is open (S6-42c).

use std::cell::RefCell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use od_machine::{HostQuantumCall, HostQuantumExit, HostQuantumResult, PagedProbeExit};

use crate::PagedFetchedLoadSession;
use crate::jit_region::{JitPartStats, JitRegionPart};
use crate::paged_host_loop::{
    PagedHostLoopConfig, PagedHostLoopControl, PagedHostLoopExit, run_paged_host_loop_shared,
};

/// What one vCPU's JIT host loop did, summed over its calls (published after every call, read by
/// any thread).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HostJitVcpuStats {
    pub calls: u64,
    pub retired_jit: u64,
    pub retired_interpreter: u64,
    pub steps_jit: u64,
    pub steps_interpreter: u64,
    pub jit_refusals: u64,
    /// Calls that ended because the run's stop poll fired.
    pub stop_exits: u64,
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
}

impl JitHostQuantum {
    /// `parts[cpu]` is the part of vCPU `cpu` (`None`: the vCPU's session has no part and the
    /// native attempts are refused while a region is live, the interpreter runs everything);
    /// `configs[cpu]` is its translation and access context; `rflags_defined` is the interpreter's
    /// initial definedness mask. The three vectors have one element per vCPU.
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
        }
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

    /// The counters of vCPU `cpu` (the totals of the calls that have returned).
    pub fn stats(&self, cpu: usize) -> HostJitVcpuStats {
        *self.stats[cpu]
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// One call of the quantum on the vCPU thread: pass this as `run` of
    /// `od_machine::run_host_threads` (`&|call| quantum.run(call)`).
    pub fn run(&self, call: HostQuantumCall<'_>) -> Result<HostQuantumResult, &'static str> {
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
            let report = run_paged_host_loop_shared(
                &mut thread.session,
                &thread.control,
                config,
                call.state,
                call.bus,
                cpu,
                &mut thread.rflags_defined,
                call.budget,
                |range| {
                    if logging {
                        logged.push((range.start().get(), range.len()));
                    }
                },
                &mut *call.stop,
            );
            if let Some(log) = &self.write_log {
                log[cpu]
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .extend(logged);
            }
            {
                let mut stats = self.stats[cpu]
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                stats.calls += 1;
                stats.retired_jit += report.retired_jit;
                stats.retired_interpreter += report.retired_interpreter;
                stats.steps_jit += report.steps_jit;
                stats.steps_interpreter += report.steps_interpreter;
                stats.jit_refusals += report.jit_refusals;
                stats.stop_exits += u64::from(report.exit == PagedHostLoopExit::StopRequested);
                stats.admissions = thread.session.admissions();
                stats.part = thread.session.region_part().map(|part| part.stats());
            }
            let retired = report.retired_total();
            let exit = match report.exit {
                PagedHostLoopExit::BudgetExhausted
                | PagedHostLoopExit::StopRequested
                | PagedHostLoopExit::FinishRequested
                | PagedHostLoopExit::EventPending => HostQuantumExit::Running,
                PagedHostLoopExit::Interpreter(PagedProbeExit::Halted) => HostQuantumExit::Halted,
                PagedHostLoopExit::Interpreter(PagedProbeExit::Shutdown) => {
                    HostQuantumExit::Shutdown
                }
                PagedHostLoopExit::Interpreter(_) => {
                    return Err("host JIT quantum: the interpreter refused the instruction");
                }
                PagedHostLoopExit::GuestFault(_) => {
                    return Err("host JIT quantum: a guest fault (no delivery in this quantum)");
                }
                PagedHostLoopExit::JitHostFailure(_) => {
                    return Err("host JIT quantum: the JIT boundary failed");
                }
                PagedHostLoopExit::InvalidRflagsDefined { .. } => {
                    return Err("host JIT quantum: invalid definedness mask");
                }
            };
            Ok(HostQuantumResult { retired, exit })
        })
    }
}
