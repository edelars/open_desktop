//! S6-42b: the thread run of the parallel mode for a host quantum other than the interpreter's
//! (the JIT host loop of `od-host`), a narrow public face over [`crate::parallel_run`].
//!
//! `parallel_run` stays crate-private (its quantum function takes the interpreter's environment:
//! devices of the board, interrupt sources, COM1 log). `od-host` cannot reach it, and the JIT loop
//! does not need that environment: it needs, for the vCPU of the thread, its CPU state, the bus by
//! shared reference (RAM without a lock; the devices behind their one lock, taken for one access at
//! a time by the callee) and the stop poll of the run. This module hands out exactly that:
//!
//! * [`run_host_threads`] runs [`ParallelControl`](crate::parallel_run::ParallelControl)'s threads
//!   (one per vCPU, barrier, kick, `stop_all`, split-`LOCK` gate, clock charge by the clock owner)
//!   with a [`HostQuantumFn`] as the quantum of every thread;
//! * [`HostParallelControl`] is the controller's side: `stop_all`, `resume`, `finish`, the
//!   counters, and the look at a stopped vCPU's state;
//! * [`host_parallel_board`] and [`host_install_vcpu`] build the board of such a run (the board of
//!   the S6-16 fixtures: Linux profile, Local APIC windows, clock owner; no firmware image).
//!
//! A7 of S6-40: no thread holds a lock of the board across a call of the host quantum. The bus is
//! handed out by shared reference (no lock of the bus; the devices behind their one lock, taken by
//! the callee for one access). The boot processor's thread takes the source lock and clock owner's
//! lock to compute its budget. It borrows the source through a per-operation lock while executing
//! the quantum (`run_threads_with`, `source_free`).
//!
//! The quantum polls [`HostQuantumCall::stop`] between instructions (and the run polls it at the
//! barrier): the stop is acknowledged on the safe point after the quantum returned, never inside
//! a native call. A host quantum that returns [`HostQuantumExit::Halted`] or
//! [`HostQuantumExit::Shutdown`] puts its vCPU in that activity; the thread then sleeps as the
//! interpreter's does.
//!
//! S6-42c: the call also hands the quantum what the guest-visible side of an instruction boundary
//! needs. [`HostQuantumCall::lapic`] is the Local APIC of the vCPU and [`HostQuantumCall::gate`] the
//! gate of the split `LOCK` (S6-34 step 8). Two functions use them: [`host_deliver_pending_irq`]
//! delivers a vector waiting in the vCPU's own Local APIC through the guest IDT (the same
//! Local APIC source and delivery as the CPL0-set loop), and
//! [`run_host_set_step`] runs ONE instruction of the CPL0 profile set over `&MachineBus` with the
//! gate, delivering a guest `#PF` or `#GP(0)` through the guest IDT as the set loop does. The base
//! profile of [`crate::run_paged_probe_shared`] stays what it was (it has no `LOCK`, no `IRETQ`).
//! The boot processor also asks the board's interrupt source. The source lock covers the query
//! and acknowledgment separately. The host quantum can service queued COM1 transmission and port
//! ticks between calls. The worker charges device time to the clock owner.

use crate::bus::SplitLockGate;
use crate::firmware_board::vcpu_quantum::{BootProcessorSource, ThreadQuantumEnv};
use crate::firmware_board::{FirmwareBoardError, set_exit_to_probe_exit};
use crate::irq_source::{BoardIrqSource, IrqBoundaryHook, IrqCounters};
use crate::lapic_irq_source::LapicIrqSource;
use crate::paged_memory_adapter::{
    IrqBoundaryOutcome, LoopBus, PagedCpl0SetExit, deliver_pending_irq, run_cpl0_set_loop,
};
use crate::parallel_run::{
    HostNmiPaging, NmiPaging, ParallelConfig, ParallelControl, ParallelFinish, ParallelReport,
    ParallelStopError, QuantumCall, run_threads_rooted,
};
use crate::smp_scheduler::Executed;
use crate::vcpu::BOOT_VCPU_INDEX;
use crate::{
    FirmwareBoardConfig, GuestPhysicalRange, LoadedFirmwareBoard, LocalApicHandle,
    Long64ProbeConfig, MachineBus, PagedProbeQuantumReport, PciEcam,
};
use od_isa::execute::{Cpl0ProfileSet, RunExit};
use od_isa::memory::AccessContext;
use od_isa::{CpuActivity, LongModeState};
use std::sync::atomic::AtomicU8;
use std::time::Duration;

/// The inputs of a host thread run.
#[derive(Clone, Copy, Debug)]
pub struct HostParallelConfig {
    /// Retired instructions a thread may ask of one call of the host quantum (at least 1).
    pub quantum: u64,
    /// Retired instructions of all vCPUs together after which the run ends.
    pub budget: u64,
    /// End the run when every vCPU is idle (halted with nothing to wake it).
    pub end_when_idle: bool,
}

impl Default for HostParallelConfig {
    fn default() -> Self {
        Self {
            quantum: 64,
            budget: u64::MAX,
            end_when_idle: true,
        }
    }
}

/// The gate of the split `LOCK` of the run (S6-34 step 8, MEMORY_MODEL.md D8), opaque to the host
/// quantum: it only hands the gate back to [`run_host_set_step`]. A locked operand that is not
/// naturally aligned makes the executing vCPU stop every other one around the operation; without a
/// gate ([`Self::none`]) such an operand is a refusal (`MisalignedAtomic`) and an error of the run.
#[derive(Clone, Copy)]
pub struct HostSplitGate<'a>(Option<&'a dyn SplitLockGate>);

impl HostSplitGate<'static> {
    /// No gate: a guest split `LOCK` is refused (what the host run did before S6-42c).
    pub const fn none() -> Self {
        Self(None)
    }
}

impl std::fmt::Debug for HostSplitGate<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "HostSplitGate(gate)"
        } else {
            "HostSplitGate(none)"
        })
    }
}

/// What one call of the host quantum is handed: the vCPU of the calling thread.
pub struct HostQuantumCall<'a> {
    /// Index of the vCPU (equal to its APIC ID).
    pub cpu: usize,
    /// The CPU state of the vCPU; the thread owns it for the call.
    pub state: &'a mut LongModeState,
    /// The machine by shared reference: RAM without a lock, devices behind their one lock.
    pub bus: &'a MachineBus,
    /// At most this many instructions (the shared budget and the timer deadline cut it).
    pub budget: u64,
    /// The run's stop poll: the end of the run, a `stop_all` request or another vCPU's split
    /// `LOCK` or a kick. The quantum polls it between instructions and returns when it is true.
    pub stop: &'a mut dyn FnMut() -> bool,
    /// The target vCPU's STOP byte, borrowed for the call. Clear it before polling
    /// `stop` and pass it to the native entry together with the addressed guest state.
    pub native_stop: Option<&'a AtomicU8>,
    /// The Local APIC of this vCPU (S6-42c): the source of the vectors the quantum delivers.
    pub lapic: &'a LocalApicHandle,
    /// The boot processor's source of board vectors. Other vCPUs have only their Local APIC.
    pub board_irq: HostBoardIrqSource<'a>,
    /// Board device work available at a quantum boundary.
    pub devices: HostDeviceServices<'a>,
    /// The gate of the split `LOCK` of this vCPU's thread (S6-42c).
    pub gate: HostSplitGate<'a>,
}

/// Why a call of the host quantum returned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostQuantumExit {
    /// The budget is spent or the stop poll fired: the vCPU is still running.
    Running,
    /// The vCPU executed `HLT`: it is put in `Halted`.
    Halted,
    /// The vCPU shut down: it is put in `Shutdown`.
    Shutdown,
}

/// The result of one call of the host quantum.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostQuantumResult {
    pub retired: u64,
    pub exit: HostQuantumExit,
    /// Port-device time collected by this quantum, to be charged by the clock owner.
    pub device_ns: u64,
}

trait HostDeviceWork {
    fn pending(&self) -> bool;
    fn service(&self) -> u64;
}

impl HostDeviceWork for ThreadQuantumEnv<'_> {
    fn pending(&self) -> bool {
        self.uart.holding_occupied()
            || self
                .acpi_pm
                .as_ref()
                .is_some_and(|pm| pm.timer_tick_pending())
            || self.pit.as_ref().is_some_and(|pit| pit.port_tick_pending())
    }

    fn service(&self) -> u64 {
        self.pump_uart_transmit();
        let pm = self
            .acpi_pm
            .as_ref()
            .map_or(0, |pm| pm.take_timer_tick_ns());
        let pit = self.pit.as_ref().map_or(0, |pit| pit.take_port_tick_ns());
        pm + pit
    }
}

/// Access to queued COM1 transmission and port-device time in a running host quantum.
#[derive(Clone, Copy)]
pub struct HostDeviceServices<'a>(Option<&'a dyn HostDeviceWork>);

impl HostDeviceServices<'_> {
    /// Use the instruction loop without servicing board devices.
    pub const fn none() -> Self {
        Self(None)
    }

    pub fn pending(&self) -> bool {
        self.0.is_some_and(HostDeviceWork::pending)
    }

    /// Complete any COM1 transmission and take pending port ticks. Return the ticks with the
    /// quantum result so that the worker charges their time exactly once.
    pub fn service(&self) -> u64 {
        self.0.map_or(0, HostDeviceWork::service)
    }
}

/// The quantum of every vCPU thread of a host run. Called by every thread (so `Sync`); state that
/// belongs to one vCPU lives on its thread (a `thread_local!`), as the JIT session does.
pub type HostQuantumFn<'f> =
    dyn Fn(HostQuantumCall<'_>) -> Result<HostQuantumResult, &'static str> + Sync + 'f;

/// What every vCPU thread runs on its own thread after its last quantum (the argument is the vCPU
/// index): the owner of per-thread state releases it here (a `thread_local!` destructor may run
/// after `thread::scope` has returned, so a lease held there outlives the run).
pub type HostThreadExitFn<'f> = dyn Fn(usize) + Sync + 'f;

/// Why a host thread run ended.
#[derive(Debug, Eq, PartialEq)]
pub enum HostParallelFinish {
    /// The controller asked for the end (or its closure returned).
    Stopped,
    BudgetExhausted,
    /// Every vCPU is halted with nothing that could wake it.
    AllIdle,
    /// A vCPU's quantum, boundary or clock charge failed; every thread was stopped.
    Error {
        cpu: usize,
        message: String,
    },
    /// The thread of vCPU `cpu` panicked; every thread was stopped.
    Panicked {
        cpu: usize,
    },
}

/// What a host thread run did.
#[derive(Debug, Eq, PartialEq)]
pub struct HostParallelReport {
    pub finished: HostParallelFinish,
    /// Instructions retired by each vCPU.
    pub retired: Vec<u64>,
    /// Calls of the host quantum by each vCPU.
    pub calls: Vec<u64>,
    /// Threads started and threads that ran to their end: both are the number of vCPUs when the
    /// run returns.
    pub threads_started: usize,
    pub threads_exited: usize,
    /// NMIs each vCPU's thread delivered through vector 2 at its boundaries (S6-42c).
    pub nmi_delivered: Vec<u64>,
    /// Split `LOCK` operations each vCPU executed behind the gate (S6-42c).
    pub split_locks: Vec<u64>,
}

/// The proof of a stop of every vCPU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostStopProof {
    /// How many vCPUs acknowledged (all of them).
    pub acknowledged: usize,
    /// The virtual time at which the machine stood still.
    pub now_ns: u64,
}

#[derive(Debug, Eq, PartialEq)]
pub enum HostStopError {
    /// These vCPUs (index order) did not reach a safe point in time; the request was withdrawn.
    Timeout { unacked: Vec<usize> },
    /// The run is already over.
    Finished,
}

/// The controller's side of a host thread run (the caller's thread).
pub struct HostParallelControl<'s, 'a> {
    inner: &'s ParallelControl<'s, 'a>,
}

impl HostParallelControl<'_, '_> {
    /// The bus of the machine (RAM by shared reference, devices behind their lock).
    pub fn bus(&self) -> &MachineBus {
        self.inner.bus()
    }

    pub fn is_finished(&self) -> bool {
        self.inner.is_finished()
    }

    /// Retired instructions of every vCPU so far (a running vCPU's are a moment old).
    pub fn retired(&self) -> Vec<u64> {
        self.inner.retired()
    }

    /// Calls of the host quantum of vCPU `cpu` so far. Panics when `cpu >= self.vcpu_count()`.
    pub fn calls(&self, cpu: usize) -> u64 {
        self.inner.cpu_report(cpu).calls
    }

    pub fn vcpu_count(&self) -> usize {
        self.inner.vcpu_count()
    }

    /// The Local APIC of vCPU `cpu` (S6-42c): the controller posts vectors to it
    /// (`request_fixed_edge`) and looks at its IRR and ISR.
    pub fn lapic(&self, cpu: usize) -> Option<LocalApicHandle> {
        self.inner.lapic(cpu)
    }

    /// End the run: every thread ends at its next boundary.
    pub fn finish(&self) {
        self.inner.finish();
    }

    /// Wait until the run is over or `timeout` has passed. `true` when it is over.
    pub fn wait_finished(&self, timeout: Duration) -> bool {
        self.inner.wait_finished(timeout)
    }

    /// Wait until `condition` is true, `timeout` has passed or the run is over.
    pub fn wait_until(&self, timeout: Duration, condition: impl FnMut() -> bool) -> bool {
        self.inner.wait_until(timeout, condition)
    }

    /// Stop every vCPU on a safe point (the protocol of S6-34: request, then wait for the
    /// acknowledgement of each thread, which it gives only after its quantum returned). On success
    /// no vCPU executes an instruction until [`Self::resume`].
    pub fn stop_all(&self, timeout: Duration) -> Result<HostStopProof, HostStopError> {
        match self.inner.stop_all(timeout) {
            Ok(proof) => Ok(HostStopProof {
                acknowledged: proof.acks.len(),
                now_ns: proof.now_ns,
            }),
            Err(ParallelStopError::Timeout { unacked }) => Err(HostStopError::Timeout { unacked }),
            Err(ParallelStopError::Finished) => Err(HostStopError::Finished),
        }
    }

    pub fn resume(&self) {
        self.inner.resume();
    }

    /// Whether vCPU `cpu` last published `Halted` (it executed `HLT` and nothing woke it).
    pub fn halted(&self, cpu: usize) -> bool {
        self.inner.activity(cpu) == crate::idle_arbiter::SchedActivity::Halted
    }

    /// Whether a stop is in force.
    pub fn stopped(&self) -> bool {
        self.inner.stopped()
    }

    /// Look at (or change) the CPU state of vCPU `cpu` while the machine is stopped. `None` when
    /// no stop is in force or the vCPU has not acknowledged it.
    pub fn with_state<R>(&self, cpu: usize, f: impl FnOnce(&mut LongModeState) -> R) -> Option<R> {
        self.inner.with_vcpu(cpu, |vcpu| f(&mut vcpu.state))
    }
}

/// Run the vCPUs of `board` on one thread each, every thread calling `run` as its quantum, with the
/// caller's thread as the controller (see [`crate::parallel_run::run_threads`]: the board is put
/// in [`crate::ExecutionMode::Parallel`] for the run and back afterwards, and every thread is
/// joined before this returns).
///
/// `nmi_roots[cpu]` is the fallback page-table root through which an NMI is delivered to vCPU `cpu`
/// when its state carries no CR3 of its own. A vCPU without a root (`None`, or past the end
/// of the slice) that has an NMI to take at a boundary fails the run with an explicit error; the
/// board's own root is never used (S6-42c).
/// This entry uses the guest vCPU's EFER for NMI delivery.
pub fn run_host_threads<R>(
    board: &mut LoadedFirmwareBoard,
    config: HostParallelConfig,
    nmi_roots: &[Option<u64>],
    run: &HostQuantumFn<'_>,
    on_thread_exit: &HostThreadExitFn<'_>,
    controller: impl FnOnce(&HostParallelControl<'_, '_>) -> R,
) -> Result<(HostParallelReport, R), FirmwareBoardError> {
    run_host_threads_inner(
        board,
        config,
        NmiPaging::Roots(nmi_roots),
        run,
        on_thread_exit,
        controller,
    )
}

/// Run host threads with the same CR3 and EFER inputs for NMI delivery as for each host quantum.
/// CPU-owned CR3 takes precedence over the configured root. For a missing entry, the vCPU must
/// provide its own CR3 and guest EFER when taking an NMI. These inputs are fixed for the run.
pub fn run_host_threads_with_nmi_paging<R>(
    board: &mut LoadedFirmwareBoard,
    config: HostParallelConfig,
    nmi_paging: &[Option<HostNmiPaging>],
    run: &HostQuantumFn<'_>,
    on_thread_exit: &HostThreadExitFn<'_>,
    controller: impl FnOnce(&HostParallelControl<'_, '_>) -> R,
) -> Result<(HostParallelReport, R), FirmwareBoardError> {
    run_host_threads_inner(
        board,
        config,
        NmiPaging::Host(nmi_paging),
        run,
        on_thread_exit,
        controller,
    )
}

fn run_host_threads_inner<R>(
    board: &mut LoadedFirmwareBoard,
    config: HostParallelConfig,
    nmi_paging: NmiPaging<'_>,
    run: &HostQuantumFn<'_>,
    on_thread_exit: &HostThreadExitFn<'_>,
    controller: impl FnOnce(&HostParallelControl<'_, '_>) -> R,
) -> Result<(HostParallelReport, R), FirmwareBoardError> {
    let config = ParallelConfig {
        quantum: config.quantum,
        budget: config.budget,
        end_when_idle: config.end_when_idle,
        ..ParallelConfig::default()
    };
    let quantum = move |call: QuantumCall<'_, '_>| -> Result<Executed, FirmwareBoardError> {
        let result = run(HostQuantumCall {
            cpu: call.cpu,
            state: &mut call.vcpu.state,
            bus: call.env.bus,
            budget: call.budget,
            stop: call.stop,
            native_stop: call.native_stop,
            lapic: call.lapic,
            board_irq: HostBoardIrqSource((call.cpu == BOOT_VCPU_INDEX).then_some(call.source)),
            devices: HostDeviceServices(Some(call.env)),
            gate: HostSplitGate(Some(call.gate)),
        })
        .map_err(FirmwareBoardError::Config)?;
        match result.exit {
            HostQuantumExit::Running => {}
            HostQuantumExit::Halted => call.vcpu.state.activity = Some(CpuActivity::Halted),
            HostQuantumExit::Shutdown => call.vcpu.state.activity = Some(CpuActivity::Shutdown),
        }
        Ok(Executed {
            retired: result.retired,
            partial_rep: false,
            device_ns: result.device_ns,
        })
    };
    let (report, value) = run_threads_rooted(
        board,
        config,
        true,
        &quantum,
        Some(on_thread_exit),
        nmi_paging,
        |control| controller(&HostParallelControl { inner: control }),
    )?;
    Ok((host_report(report), value))
}

fn host_report(report: ParallelReport) -> HostParallelReport {
    let finished = match report.finished {
        ParallelFinish::Stopped => HostParallelFinish::Stopped,
        ParallelFinish::BudgetExhausted => HostParallelFinish::BudgetExhausted,
        ParallelFinish::AllIdle => HostParallelFinish::AllIdle,
        ParallelFinish::Error { cpu, error } => HostParallelFinish::Error {
            cpu,
            message: format!("{error:?}"),
        },
        ParallelFinish::Panicked { cpu } => HostParallelFinish::Panicked { cpu },
    };
    HostParallelReport {
        finished,
        retired: report.cpus.iter().map(|cpu| cpu.retired).collect(),
        calls: report.cpus.iter().map(|cpu| cpu.calls).collect(),
        threads_started: report.threads_started,
        threads_exited: report.threads_exited,
        nmi_delivered: report.cpus.iter().map(|cpu| cpu.nmi_delivered).collect(),
        split_locks: report.cpus.iter().map(|cpu| cpu.split_locks).collect(),
    }
}

/// The page-table root of the board's own boot (`into_linux_board`). The board is built with it,
/// but a host run never walks it: a host quantum walks the root of its own configuration, and the
/// delivery of an NMI uses the root named for the vCPU in `run_host_threads` (S6-42c).
const HOST_BOARD_ROOT: u64 = 0x1000;

/// A board for a host thread run: `vcpu_count` vCPUs, Linux profile with the Local APIC windows
/// and the clock owner, no firmware image and no guest memory content. The caller writes guest
/// memory through [`LoadedFirmwareBoard::bus_mut`] and installs the vCPUs with
/// [`host_install_vcpu`]. The board starts in the interleaved mode; [`run_host_threads`] switches
/// it to the parallel mode for the run.
pub fn host_parallel_board(vcpu_count: usize) -> Result<LoadedFirmwareBoard, FirmwareBoardError> {
    let config = FirmwareBoardConfig {
        vcpu_count,
        pci_ecam: true,
        acpi_pm: true,
        acpi_tables: true,
        linux_profile: true,
        ..FirmwareBoardConfig::default()
    };
    let mut board = crate::firmware_board::assemble_board(&config, PciEcam::new(), None, None)?
        .into_linux_board(HOST_BOARD_ROOT, 0x1000)?;
    board.bus_mut().enable_apic_mmio_windows();
    board.enable_clock_owner()?;
    Ok(board)
}

/// Make vCPU `cpu` a running processor on `state` (no legacy model). `false` when the board has no
/// such vCPU. The state must carry no CPU-owned `CR3` (`state.cr3 == None`) when the host quantum
/// runs the base interpreter profile.
pub fn host_install_vcpu(
    board: &mut LoadedFirmwareBoard,
    cpu: usize,
    state: LongModeState,
) -> bool {
    match board.vcpu_mut(cpu) {
        Some(vcpu) => {
            vcpu.state = state;
            vcpu.legacy = None;
            true
        }
        None => false,
    }
}

/// What [`host_deliver_pending_irq`] did at one instruction boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostIrqBoundary {
    /// Nothing was delivered: no vector is eligible, RFLAGS.IF is clear, or the STI shadow holds.
    None,
    /// A vector was delivered through the guest IDT (the frame is on the guest stack, the state is
    /// in the handler, the vector moved from IRR to ISR). Nothing retired.
    Delivered,
    /// The vector could not be delivered (no IDT, a gate or a stack that faults, a refused state):
    /// an error of the run, never a quiet drop. The text names the vector and the reason.
    Failed(String),
}

/// The interrupt boundary of a host quantum (S6-42c), one instruction boundary of vCPU `lapic`'s
/// processor: the vector waiting in the vCPU's own Local APIC (the highest IRR vector above PPR,
/// what the Local APIC source of the set loop reports) is delivered through the
/// guest IDT by the delivery the CPL0-set loop uses, then acknowledged in that Local APIC only.
/// `skip_first_boundary` is the caller's cell of the REP rule (a call that ended inside a partial
/// `REP` does not deliver at its first boundary): it is cleared here as the set loop clears it.
/// The page tables are those of `cr3`/`efer` (the host quantum's own configuration); `state.cr3`
/// stays what the caller keeps it. The board's source (PIC, PIT) is not offered: only what waits in
/// the Local APIC (an IPI, the Local APIC timer, an IOAPIC line routed to it).
pub fn host_deliver_pending_irq(
    state: &mut LongModeState,
    bus: &MachineBus,
    lapic: &LocalApicHandle,
    cr3: u64,
    efer: u64,
    context: AccessContext,
    skip_first_boundary: &mut bool,
    committed_write: impl FnMut(GuestPhysicalRange),
) -> HostIrqBoundary {
    HostBoardIrqSource::none().deliver_pending(
        state,
        bus,
        lapic,
        cr3,
        efer,
        context,
        skip_first_boundary,
        committed_write,
    )
}

/// The interrupt source lent to the boot processor for this quantum. Selection and acknowledgment
/// use the same board-first routing as the interpreter. Each source operation has its own lock scope.
pub struct HostBoardIrqSource<'a>(Option<&'a mut dyn BoardIrqSource>);

impl HostBoardIrqSource<'_> {
    /// Use only the Local APIC at the instruction boundary.
    pub const fn none() -> Self {
        Self(None)
    }

    /// Borrow the source for an instruction loop with shorter-lived state.
    pub fn reborrow(&mut self) -> HostBoardIrqSource<'_> {
        match self.0.as_mut() {
            Some(source) => HostBoardIrqSource(Some(&mut **source)),
            None => HostBoardIrqSource::none(),
        }
    }

    /// Whether a successful guest write to the PIT ports calls for a new scheduler deadline.
    pub fn pit_write_ends_quantum(&self) -> bool {
        self.0
            .as_deref()
            .is_some_and(BoardIrqSource::pit_write_ends_quantum)
    }

    /// Deliver an eligible board or Local APIC vector through the guest IDT.
    pub fn deliver_pending(
        &mut self,
        state: &mut LongModeState,
        bus: &MachineBus,
        lapic: &LocalApicHandle,
        cr3: u64,
        efer: u64,
        context: AccessContext,
        skip_first_boundary: &mut bool,
        mut committed_write: impl FnMut(GuestPhysicalRange),
    ) -> HostIrqBoundary {
        let mut lapic = LapicIrqSource::new(lapic.clone());
        let mut boot;
        let source: &mut dyn BoardIrqSource = if let Some(board) = self.0.as_deref_mut() {
            boot = BootProcessorSource::new(board, lapic);
            &mut boot
        } else {
            &mut lapic
        };
        // The cheap question first: the paging configuration is built only when something can be
        // delivered (or the REP rule has a flag to clear).
        if !*skip_first_boundary && source.pending_vector().is_none() {
            return HostIrqBoundary::None;
        }
        let context = AccessContext {
            origin: od_isa::memory::AccessOrigin::Explicit,
            ..context
        };
        let paging = match Long64ProbeConfig::from_cpu_state(state, cr3, efer) {
            Ok(paging) => paging,
            Err(error) => {
                return HostIrqBoundary::Failed(format!("paging configuration refused: {error:?}"));
            }
        };
        let mut counters = IrqCounters::default();
        let mut hook = IrqBoundaryHook {
            source,
            counters: &mut counters,
            delivered_mirror: None,
            skip_first_boundary: *skip_first_boundary,
        };
        let outcome = deliver_pending_irq(
            &mut hook,
            state,
            bus.physical_memory(),
            &paging,
            context,
            &mut committed_write,
            None,
        );
        *skip_first_boundary = hook.skip_first_boundary;
        match outcome {
            IrqBoundaryOutcome::None => HostIrqBoundary::None,
            IrqBoundaryOutcome::Delivered => HostIrqBoundary::Delivered,
            IrqBoundaryOutcome::Failed((exit, page_fault, memory_error)) => {
                HostIrqBoundary::Failed(format!(
                    "{exit:?}; page_fault: {page_fault:?}; memory_error: {memory_error:?}"
                ))
            }
        }
    }
}

/// What [`run_host_set_step`] needs of the vCPU and the run besides the state and the bus.
pub struct HostCpl0Step<'a> {
    /// Index of the vCPU (its MMIO accesses reach its own Local APIC).
    pub vcpu: usize,
    /// The Local APIC of the vCPU (the timer-register write count is read from it).
    pub lapic: &'a LocalApicHandle,
    /// The gate of the split `LOCK` of the thread; [`HostSplitGate::none`] refuses a split operand.
    pub gate: HostSplitGate<'a>,
    pub cr3: u64,
    pub efer: u64,
    pub context: AccessContext,
    /// The interpreter's definedness mask of the flags, read and returned across steps.
    pub rflags_defined: u64,
}

/// What [`run_host_set_step`] reports.
#[derive(Debug, Eq, PartialEq)]
pub struct HostSetStepReport {
    /// The step as the base probe reports it: the exit (a `Fault` is a `#PF`/`#GP` the guest IDT did
    /// not take: no IDT, or a delivery that failed; the record is in `page_fault`), the instructions
    /// retired (at most 1; a delivery of an exception retires nothing and is followed, in the same
    /// call, by the handler's first instruction), the flags mask and the walker's records.
    pub report: PagedProbeQuantumReport,
    /// The call ended inside a `REP` that made progress: the next boundary is inside it and takes no
    /// interrupt (`skip_first_boundary`).
    pub partial_rep: bool,
    /// `#PF` and `#GP(0)` exceptions delivered through the guest IDT in this step.
    pub delivered_exceptions: u64,
    /// The guest wrote a timer register of its Local APIC (`0x320`, `0x380`, `0x3E0`): the deadline
    /// the call was budgeted for may be stale, so the caller ends its call (as the set quantum does).
    pub timer_register_written: bool,
    /// The guest successfully wrote to a PIT programming port.
    pub pit_written: bool,
    /// The guest successfully accessed a port. The caller can check for queued device work.
    pub port_accessed: bool,
}

/// One instruction of vCPU `step.vcpu` through the CPL0 profile set `set` over `&MachineBus`
/// (S6-42c): the set loop of the interpreter quantum ([`run_cpl0_set_loop`] over the shared bus,
/// budget one, no interrupt hook: the caller delivers at its own boundary with
/// [`host_deliver_pending_irq`]), with the gate of the split `LOCK` of `step.gate` and with the
/// delivery of a guest `#PF` or `#GP(0)` through the guest IDT when one is loaded, exactly as the
/// loop does. `state.cr3` must be empty (the root is `step.cr3`) unless the set owns CR3. `Err`: an
/// exit the board does not know how to resume from (an integration error of the set).
pub fn run_host_set_step(
    state: &mut LongModeState,
    bus: &MachineBus,
    step: HostCpl0Step<'_>,
    set: &Cpl0ProfileSet<'_>,
    committed_write: impl FnMut(GuestPhysicalRange),
) -> Result<HostSetStepReport, String> {
    let timer_writes = step.lapic.timer_register_writes();
    let report = run_cpl0_set_loop(
        state,
        LoopBus::Shared(bus),
        step.gate.0,
        step.vcpu,
        step.cr3,
        step.efer,
        step.context,
        1,
        step.rflags_defined,
        set,
        None,
        || false,
        committed_write,
        false,
        None,
        None,
        None,
        None,
    );
    let partial_rep = matches!(
        report.exit,
        PagedCpl0SetExit::Run(RunExit::RepIterationBudgetExhausted)
    );
    let timer_register_written = step.lapic.timer_register_writes() != timer_writes;
    let (exit, _) = set_exit_to_probe_exit(report.exit)
        .map_err(|error| format!("host CPL0 step: {error:?}"))?;
    Ok(HostSetStepReport {
        report: PagedProbeQuantumReport {
            exit,
            instructions_executed: report.instructions_executed,
            rflags_defined: report.rflags_defined,
            page_fault: report.page_fault,
            memory_error: report.memory_error,
            translation_context_changes: report.translation_context_changes,
        },
        partial_rep,
        delivered_exceptions: report.delivered_exceptions,
        timer_register_written,
        pit_written: report.pit_written,
        port_accessed: report.port_accessed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AccessWidth, ExecutionMode, GuestPhysicalAddress, PagedProbeExit, run_paged_probe,
        run_paged_probe_shared,
    };
    use od_isa::Register;
    use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, PrivilegeLevel};

    const CODE_RIP: u64 = 0x0040_1000;
    const CODE_PHYSICAL: u64 = 0x8000;

    /// `mov dx,0x3FD; in al,dx; mov dx,0x3F8; mov al,'A'; out dx,al; hlt`: a COM1 line status
    /// read and a transmit-holding write, the port accesses of one program.
    const PORT_PROGRAM: [u8; 15] = [
        0x66, 0xBA, 0xFD, 0x03, 0xEC, 0x66, 0xBA, 0xF8, 0x03, 0xB0, 0x41, 0xEE, 0xF4, 0, 0,
    ];

    fn context() -> AccessContext {
        AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        }
    }

    fn put(board: &mut LoadedFirmwareBoard, address: u64, value: u64) {
        board
            .bus_mut()
            .write_physical(
                GuestPhysicalAddress::new(address),
                AccessWidth::Qword,
                value,
                |_| {},
            )
            .unwrap();
    }

    /// A board with 4 KiB tables (linear 0x401000 -> physical 0x8000) and the program.
    fn board_with_program() -> LoadedFirmwareBoard {
        let mut board = host_parallel_board(1).unwrap();
        for (address, value) in [
            (0x1000, 0x2007),
            (0x2000, 0x4007),
            (0x4010, 0x6007),
            (0x6008, CODE_PHYSICAL | 0x7),
        ] {
            put(&mut board, address, value);
        }
        board
            .bus_mut()
            .patch_physical_ram(GuestPhysicalAddress::new(CODE_PHYSICAL), &PORT_PROGRAM)
            .unwrap();
        board
    }

    fn state() -> LongModeState {
        LongModeState {
            rip: CODE_RIP,
            cr0: Some(0x8001_0033),
            cr3: None,
            cr4: Some(1 << 5),
            rflags: 0x202,
            activity: Some(CpuActivity::Running),
            sti_shadow: Some(od_isa::state::StiShadow::Clear),
            ..Default::default()
        }
    }

    /// S6-42b: the base interpreter over `&MachineBus` (`run_paged_probe_shared`, the device lock
    /// taken for one port access at a time) equals the exclusive one on a program with port reads
    /// and writes: the same registers, the same exit, the same device state.
    #[test]
    fn the_shared_probe_equals_the_exclusive_probe_with_port_io() {
        const EFER: u64 = 0xD00;
        let mut exclusive = board_with_program();
        let mut exclusive_state = state();
        let exclusive_report = run_paged_probe(
            &mut exclusive_state,
            exclusive.bus_mut(),
            0x1000,
            EFER,
            context(),
            100,
            od_isa::INTEGER_FLAGS & !(1 << 7),
            || false,
            |_| {},
        );
        assert_eq!(exclusive_report.exit, PagedProbeExit::Halted);
        assert_eq!(exclusive_report.instructions_executed, 6);

        let mut shared = board_with_program();
        shared.bus_mut().set_execution_mode(ExecutionMode::Parallel);
        let mut shared_state = state();
        let shared_report = run_paged_probe_shared(
            &mut shared_state,
            shared.bus_mut(),
            0,
            0x1000,
            EFER,
            context(),
            100,
            od_isa::INTEGER_FLAGS & !(1 << 7),
            || false,
            |_| {},
        );
        assert_eq!(shared_report, exclusive_report);
        assert_eq!(shared_state, exclusive_state);
        // The status read went to COM1 and the write reached its holding register.
        assert_eq!(shared_state.gpr[Register::Rax as usize] & 0xFF, 0x41);
        assert_eq!(shared_state.gpr[Register::Rdx as usize] & 0xFFFF, 0x3F8);
        assert!(shared.uart().holding_occupied());
        assert_eq!(
            shared.uart().holding_occupied(),
            exclusive.uart().holding_occupied()
        );
    }

    /// The stop poll of the shared probe ends the quantum before the next instruction.
    #[test]
    fn the_shared_probe_polls_stop_between_instructions() {
        let mut board = board_with_program();
        board.bus_mut().set_execution_mode(ExecutionMode::Parallel);
        let mut state = state();
        let mut polls = 0;
        let report = run_paged_probe_shared(
            &mut state,
            board.bus_mut(),
            0,
            0x1000,
            0xD00,
            context(),
            100,
            od_isa::INTEGER_FLAGS & !(1 << 7),
            || {
                polls += 1;
                polls > 2
            },
            |_| {},
        );
        assert_eq!(report.exit, PagedProbeExit::Stopped);
        assert_eq!(report.instructions_executed, 2);
    }

    /// S6-42c: an NMI waiting for a vCPU that has no CR3 of its own and no root named for the NMI
    /// is an explicit error of the run; the board's own root (0x1000, never written here) is not
    /// read. With a root named for it the walk is attempted (and fails on the empty IDT, an error
    /// of another kind), which shows the root is the one used.
    #[test]
    fn an_nmi_needs_a_root_named_for_the_vcpu() {
        let run = |roots: &[Option<u64>]| {
            let mut board = board_with_program();
            assert!(host_install_vcpu(&mut board, 0, state()));
            let quantum = |_: HostQuantumCall<'_>| -> Result<HostQuantumResult, &'static str> {
                Ok(HostQuantumResult {
                    retired: 1,
                    exit: HostQuantumExit::Running,
                    device_ns: 0,
                })
            };
            let (report, ()) = run_host_threads(
                &mut board,
                HostParallelConfig {
                    quantum: 4,
                    budget: u64::MAX,
                    end_when_idle: false,
                },
                roots,
                &quantum,
                &|_| {},
                |control| {
                    control.lapic(0).unwrap().write_icr(0, 0, 0x0400).unwrap();
                    assert!(control.wait_finished(Duration::from_secs(30)));
                },
            )
            .unwrap();
            report
        };
        let HostParallelFinish::Error { cpu: 0, message } = run(&[]).finished else {
            panic!("an NMI without a root must fail the run");
        };
        assert!(message.contains("no page-table root"), "{message}");
        let HostParallelFinish::Error { cpu: 0, message } = run(&[Some(0x1000)]).finished else {
            panic!("an NMI through an empty IDT must fail the run");
        };
        assert!(
            !message.contains("no page-table root"),
            "a named root is walked: {message}"
        );
    }
}
