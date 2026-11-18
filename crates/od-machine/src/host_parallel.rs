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
//! the callee for one access), the quantum does not use the board's interrupt source, so the boot
//! processor's thread takes that source's lock and the clock owner's lock only to compute its
//! budget (`run_threads_with`, `source_free`), and the APIC lock is not held either.
//!
//! The quantum polls [`HostQuantumCall::stop`] between instructions (and the run polls it at the
//! barrier): the stop is acknowledged on the safe point after the quantum returned, never inside
//! a native call. A host quantum that returns [`HostQuantumExit::Halted`] or
//! [`HostQuantumExit::Shutdown`] puts its vCPU in that activity; the thread then sleeps as the
//! interpreter's does. Not here (S6-42c): interrupt delivery into the host quantum, the STOP byte
//! of native blocks (a one-instruction block has the loop's poll between every two instructions),
//! the NVMe doorbell and the PM stop request.

use crate::firmware_board::FirmwareBoardError;
use crate::parallel_run::{
    ParallelConfig, ParallelControl, ParallelFinish, ParallelReport, ParallelStopError,
    QuantumCall, ThreadExitFn, run_threads_with,
};
use crate::smp_scheduler::Executed;
use crate::{FirmwareBoardConfig, LoadedFirmwareBoard, MachineBus, PciEcam};
use od_isa::{CpuActivity, LongModeState};
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
    /// `LOCK`. Only atomics. The quantum polls it between instructions and returns when it is true.
    pub stop: &'a mut dyn FnMut() -> bool,
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
pub fn run_host_threads<R>(
    board: &mut LoadedFirmwareBoard,
    config: HostParallelConfig,
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
            device_ns: 0,
        })
    };
    let (report, value) = run_threads_with(
        board,
        config,
        true,
        &quantum,
        Some(on_thread_exit),
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
    }
}

/// The page-table root the board reports for its own use. The host quantum walks the tables of the
/// state it is given, but the delivery of a pending NMI (`deliver_pending_nmi_of` in the thread's
/// loop) goes through this root for a vCPU without a CR3, so the address is read when an NMI is
/// delivered: no NMI is delivered in the tests of S6-42b, and the NMI path of the host run is open
/// (S6-42c).
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
}
