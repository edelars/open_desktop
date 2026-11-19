//! S6-43a: cross-modifying code on two vCPUs with the JIT on (G-XMC of S6-40, item 2.4).
//!
//! vCPU A rewrites the bytes of an instruction that vCPU B runs, and publishes a signal. B waits
//! for the signal, runs CPUID in the serializing protocol, and runs the changed code. The changed instruction is a form the
//! JIT admits (`mov rax,[rbx+disp8]`, S4-15e, two versions that load two different cells), and
//! the run counts only when B ran it natively before and after the write. The same cycle
//! functions run in three settings:
//!
//! * the CPL0 interpreter of od-isa (`run_host_set_step`, the oracle) on one thread: the expected values
//!   and the final state;
//! * the interleaved mode: one thread, one JIT session per vCPU, a fixed schedule;
//! * the parallel mode: `run_accounted_host_threads`, one thread per vCPU, no lock of the bus.
//!
//! The host drives the cycle and wait loops. Every step executes real guest bytes at the selected
//! RIP. CPUID uses the selected CPL0 interpreter and the next native step performs a fresh lookup.
//! With one guest instruction per block, the no-CPUID guest-wait variant still performs lookup
//! for X, while the host-wait variant retains X's prior admission until that lookup.
use super::*;
use crate::host_threads::JitHostQuantum;
use crate::jit_region::SharedJitRegion;
use od_isa::cpuid::CpuidProfile;
use od_isa::execute::{ControlRegisterPolicy, Cpl0ProfileSet};
use od_machine::{
    ExecutionMode, HostBoardIrqSource, HostCpl0Step, HostDeviceServices, HostParallelConfig,
    HostParallelFinish, HostQuantumCall, HostQuantumExit, HostQuantumFailure, HostQuantumResult,
    HostSplitGate, LocalApic, LocalApicHandle, host_install_vcpu, host_parallel_board,
    run_accounted_host_threads, run_host_set_step,
};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

const CPU_B: usize = 0;
const CPU_A: usize = 1;
const WAIT: Duration = Duration::from_secs(120);
const CPUID_PROFILE: CpuidProfile = CpuidProfile::DiagnosticV1;

fn interpreter_set(cpu: usize) -> Cpl0ProfileSet<'static> {
    Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cpuid(&CPUID_PROFILE)
        .with_cpuid_topology(cpu as u8, 2)
}

/// B's code page (physical 0x8000, `CODE_RIP`): the changed instruction, the guest wait for the
/// signal, and guest CPUID.
const CODE_X: u64 = CODE_RIP;
const CODE_W: u64 = CODE_RIP + 0x10;
const CODE_C: u64 = CODE_RIP + 0x20;
const A_CODE_RIP: u64 = 0x0040_2000;
const A_CODE_PHYSICAL: u64 = 0xa000;
/// The cell B loads in X (data page, `INITIAL_DATA`), the signal cell next to it and the second
/// cell X loads in its odd versions.
const SIGNAL_LINEAR: u64 = DATA_LINEAR + 8;
const SIGNAL_PHYSICAL: u64 = DATA_QWORD + 8;
const SECOND_PHYSICAL: u64 = DATA_QWORD + 16;
const SECOND_VALUE: u64 = 0x5555_6666_7777_8888;
const SENT: u64 = 0xAAAA_AAAA_AAAA_AAAA;
/// The qword store A runs and the RAM range compared by all three execution paths.
const A_STORE: [u8; 8] = [0x48, 0x89, 0x03, 0, 0, 0, 0, 0];
const COMPARED_RAM_END: u64 = A_CODE_PHYSICAL + 0x1000;
/// Writes below this address are page-walk bits; the data stores are above it.
const DATA_WRITES_FROM: u64 = CODE_PHYSICAL;
fn qword(bytes: [u8; 8]) -> u64 {
    u64::from_le_bytes(bytes)
}
/// The two versions of X load the first and second cell via the same base and different disp8.
fn version_bytes(k: u64) -> [u8; 8] {
    [0x48, 0x8b, 0x43, ((k % 2) * 16) as u8, 0, 0, 0, 0]
}
/// The RAX of a run of version `k` of X.
fn version_result(k: u64) -> u64 {
    if k % 2 == 0 {
        INITIAL_DATA
    } else {
        SECOND_VALUE
    }
}

fn b_code() -> Vec<u8> {
    let mut code = vec![0u8; 0x30];
    code[..8].copy_from_slice(&version_bytes(0));
    code[0x10..0x13].copy_from_slice(&[0x48, 0x8b, 0x03]);
    code[0x20..0x22].copy_from_slice(&[0x0f, 0xa2]);
    code
}

fn add_code_a(bus: &mut MachineBus) {
    put_qword(bus, 0x6010, A_CODE_PHYSICAL | 0x7);
    bus.patch_physical_ram(GuestPhysicalAddress::new(A_CODE_PHYSICAL), &A_STORE)
        .unwrap();
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Wait {
    /// B loads the signal cell with a guest instruction (native) until it sees the cycle number.
    Guest,
    /// B reads the signal cell with the host's `SeqCst` load of the memory (no guest step), so
    /// the block of X that B ran just before stays admitted: the lookup after the signal is the
    /// only thing that can drop it.
    Host,
}

#[derive(Clone, Copy, Debug)]
struct Protocol {
    name: &'static str,
    wait: Wait,
    /// Execute CPUID between observing the signal and executing the changed code.
    serialize: bool,
}

const PROTOCOLS: [Protocol; 3] = [
    Protocol {
        name: "guest-wait+serialize",
        wait: Wait::Guest,
        serialize: true,
    },
    Protocol {
        name: "guest-wait",
        wait: Wait::Guest,
        serialize: false,
    },
    Protocol {
        name: "host-wait (block kept)",
        wait: Wait::Host,
        serialize: false,
    },
];

/// What one step of a vCPU did.
#[derive(Clone, Copy, Debug, Default)]
struct Step {
    jit_steps: u64,
    interpreter_steps: u64,
}

trait Stepper {
    /// One instruction at `rip` with RAX = RCX = `rax` and RBX = `rbx`.
    fn step(&mut self, rip: u64, rax: u64, rbx: u64) -> Result<Step, String>;
    fn state(&self) -> &LongModeState;
    fn rax(&self) -> u64 {
        self.state().gpr[Register::Rax as usize]
    }
}

fn load_registers(state: &mut LongModeState, rip: u64, rax: u64, rbx: u64) {
    state.rip = rip;
    state.gpr[Register::Rax as usize] = rax;
    state.gpr[Register::Rbx as usize] = rbx;
    state.gpr[Register::Rcx as usize] = rax;
}

/// A vCPU of the interleaved mode: its own JIT session (A1), the shared bus by reference.
struct Direct<'a> {
    session: PagedFetchedLoadSession,
    control: PagedHostLoopControl,
    state: LongModeState,
    rflags_defined: u64,
    bus: &'a MachineBus,
    lapic: LocalApicHandle,
    writes: Vec<(u64, u64)>,
}

impl Stepper for Direct<'_> {
    fn step(&mut self, rip: u64, rax: u64, rbx: u64) -> Result<Step, String> {
        load_registers(&mut self.state, rip, rax, rbx);
        let Self {
            session,
            control,
            state,
            rflags_defined,
            bus,
            lapic,
            writes,
        } = self;
        let vcpu = lapic.cpu_index();
        let set = interpreter_set(vcpu);
        let report = run_paged_host_loop_shared_set(
            session,
            control,
            config(),
            state,
            bus,
            vcpu,
            PagedHostSetEnv {
                lapic,
                board_irq: HostBoardIrqSource::none(),
                devices: HostDeviceServices::none(),
                gate: HostSplitGate::none(),
                set: &set,
                skip_first_boundary: &mut false,
            },
            rflags_defined,
            1,
            |range| writes.push((range.start().get(), range.len())),
            || false,
        );
        if report.exit != PagedHostLoopExit::BudgetExhausted || report.retired_total() != 1 {
            return Err(format!("vCPU {vcpu} at {rip:#x}: {:?}", report.exit));
        }
        Ok(Step {
            jit_steps: report.steps_jit,
            interpreter_steps: report.steps_interpreter,
        })
    }

    fn state(&self) -> &LongModeState {
        &self.state
    }
}

/// The oracle: one interpreted CPL0 instruction at a time, over the same guest bytes.
struct Interp<'a> {
    bus: &'a MachineBus,
    lapic: LocalApicHandle,
    state: LongModeState,
    writes: Vec<(u64, u64)>,
}

impl Stepper for Interp<'_> {
    fn step(&mut self, rip: u64, rax: u64, rbx: u64) -> Result<Step, String> {
        load_registers(&mut self.state, rip, rax, rbx);
        let writes = &mut self.writes;
        let vcpu = self.lapic.cpu_index();
        let step = run_host_set_step(
            &mut self.state,
            self.bus,
            HostCpl0Step {
                vcpu,
                lapic: &self.lapic,
                gate: HostSplitGate::none(),
                cr3: CR3,
                efer: EFER,
                context: context(),
                rflags_defined: RFLAGS_DEFINED,
                consecutive_faults: 0,
            },
            &interpreter_set(vcpu),
            |range| writes.push((range.start().get(), range.len())),
        )?;
        let report = step.report;
        if report.exit != od_machine::PagedProbeExit::BudgetExhausted
            || report.instructions_executed != 1
        {
            return Err(format!("interpreter at {rip:#x}: {:?}", report.exit));
        }
        Ok(Step {
            jit_steps: 0,
            interpreter_steps: 1,
        })
    }

    fn state(&self) -> &LongModeState {
        &self.state
    }
}

/// A vCPU of the parallel mode inside one call of the thread's quantum: every step is a call of
/// the JIT host quantum with a budget of one instruction.
struct Quantum<'q, 'c> {
    quantum: &'q JitHostQuantum,
    call: HostQuantumCall<'c>,
    retired: u64,
    device_ns: u64,
}

impl Stepper for Quantum<'_, '_> {
    fn step(&mut self, rip: u64, rax: u64, rbx: u64) -> Result<Step, String> {
        let cpu = self.call.cpu;
        load_registers(self.call.state, rip, rax, rbx);
        let before = self.quantum.stats(cpu);
        let result = self.quantum.run_accounted(HostQuantumCall {
            cpu,
            state: &mut *self.call.state,
            skip_first_boundary: &mut *self.call.skip_first_boundary,
            bus: self.call.bus,
            budget: 1,
            consecutive_faults: &mut *self.call.consecutive_faults,
            stop: &mut *self.call.stop,
            native_stop: self.call.native_stop,
            lapic: self.call.lapic,
            board_irq: self.call.board_irq.reborrow(),
            devices: self.call.devices,
            nvme_only: false,
            gate: self.call.gate,
        });
        let (retired, device_ns) = match &result {
            Ok(result) => (result.retired, result.device_ns),
            Err(failure) => (failure.retired, failure.device_ns),
        };
        self.retired += retired;
        self.device_ns = self.device_ns.saturating_add(device_ns);
        let result = result.map_err(|failure| failure.message.to_owned())?;
        if result.retired != 1 || result.exit != HostQuantumExit::Running {
            return Err(format!("vCPU {cpu} at {rip:#x}: {result:?}"));
        }
        let after = self.quantum.stats(cpu);
        Ok(Step {
            jit_steps: after.steps_jit - before.steps_jit,
            interpreter_steps: after.steps_interpreter - before.steps_interpreter,
        })
    }

    fn state(&self) -> &LongModeState {
        self.call.state
    }
}

/// What the cycle functions need besides the steppers.
struct Env<'a> {
    protocol: Protocol,
    /// The signal cell, read by the host (the `Wait::Host` protocol).
    read_signal: &'a dyn Fn() -> u64,
    /// The run counts only when X ran natively (false for the interpreter: nothing is native).
    require_jit: bool,
    /// Require A's code write and signal to take the native path.
    require_native_writes: bool,
    abort: &'a AtomicBool,
}

/// What B did, summed over the cycles.
#[derive(Clone, Debug, Default)]
struct BRecord {
    cycles: u64,
    /// Runs of X before A's write (the old code) and after the signal (the new code), natively.
    x_old_jit: u64,
    x_new_jit: u64,
    /// Runs of X that the JIT did not execute (they make the run not counted).
    x_not_jit: u64,
    /// Runs after the signal that executed the code from before A's write.
    stale: u64,
    /// Runs whose result is neither version.
    wrong: u64,
    wait_steps: u64,
    wait_max: u64,
    serialize_steps: u64,
    failures: Vec<String>,
}

impl BRecord {
    /// The run counts only when B ran X natively before and after every write and nothing of X
    /// fell back to the interpreter: one old run and two new runs per cycle, all native.
    fn counted(&self) -> bool {
        self.x_not_jit == 0
            && self.cycles > 0
            && self.x_old_jit == self.cycles
            && self.x_new_jit == 2 * self.cycles
    }

    fn fail(&mut self, message: String) {
        if self.failures.len() < 6 {
            self.failures.push(message);
        }
    }
}

fn spin_until(
    abort: &AtomicBool,
    what: &str,
    mut condition: impl FnMut() -> bool,
) -> Result<(), String> {
    let deadline = Instant::now() + WAIT;
    while !condition() {
        if abort.load(Ordering::Relaxed) {
            return Err(format!("aborted while waiting for {what}"));
        }
        if Instant::now() > deadline {
            return Err(format!("timeout waiting for {what}"));
        }
        std::hint::spin_loop();
    }
    Ok(())
}

/// B runs X once: `old` says the run is before A's write of cycle `k` (the result is version
/// `k - 1`) or after the signal (version `k`; version `k - 1` means stale code).
fn run_x(
    b: &mut dyn Stepper,
    env: &Env<'_>,
    k: u64,
    old: bool,
    rec: &mut BRecord,
) -> Result<(), String> {
    let version = if old { k - 1 } else { k };
    let step = b.step(CODE_X, SENT, DATA_LINEAR)?;
    let native = step.jit_steps == 1 && step.interpreter_steps == 0;
    match (native, old) {
        (true, true) => rec.x_old_jit += 1,
        (true, false) => rec.x_new_jit += 1,
        (false, _) => rec.x_not_jit += u64::from(env.require_jit),
    }
    let got = b.rax();
    let (wanted, previous) = (version_result(version), version_result(k - 1));
    if got == wanted {
        return Ok(());
    }
    if !old && got == previous {
        rec.stale += 1;
        rec.fail(format!(
            "cycle {k}: after the signal B ran the code from before the write (RAX {got:#x})"
        ));
    } else {
        rec.wrong += 1;
        rec.fail(format!(
            "cycle {k}: X gave RAX {got:#x}, wanted {wanted:#x} (old run: {old})"
        ));
    }
    Ok(())
}

/// B before A's write of cycle `k`: runs the old code (and so admits its block).
fn b_before_write(
    b: &mut dyn Stepper,
    env: &Env<'_>,
    k: u64,
    rec: &mut BRecord,
) -> Result<(), String> {
    run_x(b, env, k, true, rec)
}

/// A writes version `k` of X (a native qword store into B's code page), then publishes the
/// signal (a native qword store of `k` into the signal cell).
fn a_write_and_signal(a: &mut dyn Stepper, env: &Env<'_>, k: u64) -> Result<(), String> {
    for (rax, rbx) in [(qword(version_bytes(k)), CODE_X), (k, SIGNAL_LINEAR)] {
        let step = a.step(A_CODE_RIP, rax, rbx)?;
        // The oracle does not check A's store natively on purpose: the interpreter has no native
        // path, so the condition applies to the JIT runs only.
        if env.require_native_writes && (step.jit_steps != 1 || step.interpreter_steps != 0) {
            return Err(format!("A's store of cycle {k} did not run natively"));
        }
    }
    Ok(())
}

/// B after the signal of cycle `k`: waits, serializes when required, runs the changed code twice.
fn b_after_signal(
    b: &mut dyn Stepper,
    env: &Env<'_>,
    k: u64,
    rec: &mut BRecord,
) -> Result<(), String> {
    let mut polls = 0u64;
    match env.protocol.wait {
        Wait::Guest => loop {
            b.step(CODE_W, SENT, SIGNAL_LINEAR)?;
            polls += 1;
            let seen = b.rax();
            if seen >= k {
                break;
            }
            if env.abort.load(Ordering::Relaxed) || polls > 400_000_000 {
                return Err(format!("cycle {k}: the signal never arrived (read {seen})"));
            }
            std::hint::spin_loop();
        },
        Wait::Host => {
            spin_until(env.abort, "the signal", || (env.read_signal)() >= k)?;
            polls = 1;
        }
    }
    rec.wait_steps += polls;
    rec.wait_max = rec.wait_max.max(polls);
    if env.protocol.serialize {
        let flags = b.state().rflags;
        // EAX=ECX=0 selects leaf 0/subleaf 0, with deliberately dirty upper halves.
        let step = b.step(CODE_C, 0xffff_ffff_0000_0000, u64::MAX)?;
        let state = b.state();
        let results = [Register::Rax, Register::Rbx, Register::Rcx, Register::Rdx]
            .map(|register| state.gpr[register as usize]);
        let expected = [
            1,
            u64::from(u32::from_le_bytes(*b"Open")),
            u64::from(u32::from_le_bytes(*b"top ")),
            u64::from(u32::from_le_bytes(*b"Desk")),
        ];
        if step.interpreter_steps != 1
            || step.jit_steps != 0
            || state.rip != CODE_C + 2
            || state.rflags != flags
            || results != expected
        {
            return Err(format!(
                "cycle {k}: invalid CPUID: {step:?}, RIP {:#x}, RFLAGS {flags:#x}->{:#x}, outputs {results:08x?}",
                state.rip, state.rflags
            ));
        }
        rec.serialize_steps += 1;
    }
    run_x(b, env, k, false, rec)?;
    run_x(b, env, k, false, rec)?;
    rec.cycles += 1;
    Ok(())
}

/// One thread, a fixed schedule: B runs the old code, A writes and signals, B runs the new code.
fn run_schedule(
    a: &mut dyn Stepper,
    b: &mut dyn Stepper,
    env: &Env<'_>,
    cycles: u64,
) -> Result<BRecord, String> {
    let mut rec = BRecord::default();
    for k in 1..=cycles {
        b_before_write(b, env, k, &mut rec)?;
        a_write_and_signal(a, env, k)?;
        b_after_signal(b, env, k, &mut rec)?;
    }
    Ok(rec)
}

#[derive(Debug, PartialEq)]
struct CpuView {
    gpr: [u64; 16],
    rip: u64,
    rflags: u64,
}

impl CpuView {
    fn of(state: &LongModeState) -> Self {
        Self {
            gpr: state.gpr,
            rip: state.rip,
            rflags: state.rflags & RFLAGS_DEFINED,
        }
    }
}

/// The final machine state: both vCPUs and the RAM below A's code page.
#[derive(Debug, PartialEq)]
struct Final {
    a: CpuView,
    b: CpuView,
    ram: Vec<u64>,
}

fn ram_qwords(bus: &MachineBus) -> Vec<u64> {
    let mut bytes = vec![0u8; COMPARED_RAM_END as usize];
    bus.atomic_inline_memory()
        .load_bytes(GuestPhysicalAddress::new(0), &mut bytes)
        .unwrap();
    bytes
        .chunks_exact(8)
        .map(|chunk| u64::from_le_bytes(chunk.try_into().unwrap()))
        .collect()
}

const X_INDEX: usize = (CODE_PHYSICAL / 8) as usize;
fn assert_same_final(actual: &Final, expected: &Final, what: &str) {
    assert_eq!(actual.a, expected.a, "{what}: vCPU A");
    assert_eq!(actual.b, expected.b, "{what}: vCPU B");
    let first = actual
        .ram
        .iter()
        .zip(&expected.ram)
        .enumerate()
        .position(|(_, (left, right))| left != right);
    assert_eq!(
        first.map(|index| (index * 8, actual.ram[index], expected.ram[index])),
        None,
        "{what}: first differing RAM qword (address, actual, expected)"
    );
}
fn b_state() -> LongModeState {
    let mut state = make_state(SENT, DATA_LINEAR, SENT);
    state.rip = CODE_X;
    state
}

fn a_state() -> LongModeState {
    let mut state = make_state(0, CODE_X, 0);
    state.rip = A_CODE_RIP;
    state
}

/// A's code and the empty signal cell, on top of a bus whose tables and B's code are there.
fn add_a_and_signal(bus: &mut MachineBus) {
    add_code_a(bus);
    put_qword(bus, SIGNAL_PHYSICAL, 0);
    put_qword(bus, SECOND_PHYSICAL, SECOND_VALUE);
}

fn fresh_bus() -> MachineBus {
    let mut bus = make_bus(&b_code());
    add_a_and_signal(&mut bus);
    bus
}

fn signal_reader(bus: &MachineBus) -> impl Fn() -> u64 + '_ {
    move || {
        bus.atomic_inline_memory()
            .load_u64(GuestPhysicalAddress::new(SIGNAL_PHYSICAL))
            .unwrap()
    }
}

/// The oracle run: the interpreter of od-isa, the fixed schedule.
struct OracleRun {
    record: BRecord,
    finished: Final,
    writes_a: Vec<(u64, u64)>,
    writes_b: Vec<(u64, u64)>,
}

/// `require_jit` makes the oracle ask for native execution, which an interpreter never gives:
/// the run is then not counted (the test of the counting rule).
fn run_oracle(protocol: Protocol, cycles: u64, require_jit: bool) -> OracleRun {
    let mut bus = fresh_bus();
    bus.set_execution_mode(ExecutionMode::Interleaved);
    let lapics = LocalApic::with_cpus(2).unwrap();
    let read_signal = || {
        bus.atomic_inline_memory()
            .load_u64(GuestPhysicalAddress::new(SIGNAL_PHYSICAL))
            .unwrap()
    };
    let abort = AtomicBool::new(false);
    let env = Env {
        protocol,
        read_signal: &read_signal,
        require_jit,
        require_native_writes: false,
        abort: &abort,
    };
    let mut a = Interp {
        bus: &bus,
        lapic: lapics.handle_for(CPU_A).unwrap(),
        state: a_state(),
        writes: Vec::new(),
    };
    let mut b = Interp {
        bus: &bus,
        lapic: lapics.handle_for(CPU_B).unwrap(),
        state: b_state(),
        writes: Vec::new(),
    };
    let record = run_schedule(&mut a, &mut b, &env, cycles).expect("the oracle runs");
    let finished = Final {
        a: CpuView::of(&a.state),
        b: CpuView::of(&b.state),
        ram: ram_qwords(&bus),
    };
    OracleRun {
        record,
        finished,
        writes_a: a.writes,
        writes_b: b.writes,
    }
}

/// What a JIT run reports.
#[derive(Debug)]
struct JitRun {
    mode: &'static str,
    protocol: Protocol,
    record: BRecord,
    b_jit_steps: u64,
    b_interpreter_steps: u64,
    a_jit_steps: u64,
    a_interpreter_steps: u64,
    b_admissions: u64,
    finished: Final,
    writes_a: Vec<(u64, u64)>,
    writes_b: Vec<(u64, u64)>,
    elapsed: Duration,
}

impl JitRun {
    fn counted(&self) -> bool {
        self.record.counted()
    }

    fn fallback_percent(&self) -> f64 {
        let total = self.b_jit_steps + self.b_interpreter_steps;
        100.0 * self.b_interpreter_steps as f64 / total.max(1) as f64
    }

    fn line(&self) -> String {
        format!(
            "S6-43a {} / {}: {} cycles, B native {} interpreter {} (fallback {:.1}% of B's steps: \
             {} CPUID, X native {} of {}), wait steps {} (max {}), admissions {}, A native {} \
             interpreter {}, stale {}, wrong {}, {:.2}s{}",
            self.mode,
            self.protocol.name,
            self.record.cycles,
            self.b_jit_steps,
            self.b_interpreter_steps,
            self.fallback_percent(),
            self.record.serialize_steps,
            self.record.x_old_jit + self.record.x_new_jit,
            3 * self.record.cycles,
            self.record.wait_steps,
            self.record.wait_max,
            self.b_admissions,
            self.a_jit_steps,
            self.a_interpreter_steps,
            self.record.stale,
            self.record.wrong,
            self.elapsed.as_secs_f64(),
            if self.counted() { "" } else { ", NOT COUNTED" }
        )
    }

    fn assert_clean(&self) {
        assert!(
            self.record.failures.is_empty() && self.record.stale == 0 && self.record.wrong == 0,
            "{}: {:?}",
            self.line(),
            self.record.failures
        );
        assert!(self.counted(), "{}", self.line());
    }
}

/// Sums the steps a vCPU's stepper reports.
struct Counting<'s> {
    inner: &'s mut dyn Stepper,
    jit: u64,
    interpreter: u64,
}

impl Stepper for Counting<'_> {
    fn step(&mut self, rip: u64, rax: u64, rbx: u64) -> Result<Step, String> {
        let step = self.inner.step(rip, rax, rbx)?;
        self.jit += step.jit_steps;
        self.interpreter += step.interpreter_steps;
        Ok(step)
    }

    fn state(&self) -> &LongModeState {
        self.inner.state()
    }
}

/// The interleaved mode: one thread, a JIT session per vCPU on its own part of one region.
fn run_interleaved(protocol: Protocol, cycles: u64) -> JitRun {
    let region = SharedJitRegion::new(2, 4).unwrap();
    let mut bus = fresh_bus();
    bus.set_execution_mode(ExecutionMode::Interleaved);
    let read_signal = signal_reader(&bus);
    let abort = AtomicBool::new(false);
    let env = Env {
        protocol,
        read_signal: &read_signal,
        require_jit: true,
        require_native_writes: true,
        abort: &abort,
    };
    let lapics = LocalApic::with_cpus(2).unwrap();
    let part_b = region.take_part().unwrap();
    let part_a = region.take_part().unwrap();
    let mut b = Direct {
        session: PagedFetchedLoadSession::with_region_part(part_b),
        control: PagedHostLoopControl::new(),
        state: b_state(),
        rflags_defined: RFLAGS_DEFINED,
        bus: &bus,
        lapic: lapics.handle_for(CPU_B).unwrap(),
        writes: Vec::new(),
    };
    let mut a = Direct {
        session: PagedFetchedLoadSession::with_region_part(part_a),
        control: PagedHostLoopControl::new(),
        state: a_state(),
        rflags_defined: RFLAGS_DEFINED,
        bus: &bus,
        lapic: lapics.handle_for(CPU_A).unwrap(),
        writes: Vec::new(),
    };
    let started = Instant::now();
    let (record, a_steps, b_steps) = {
        let mut counting_a = Counting {
            inner: &mut a,
            jit: 0,
            interpreter: 0,
        };
        let mut counting_b = Counting {
            inner: &mut b,
            jit: 0,
            interpreter: 0,
        };
        let record = run_schedule(&mut counting_a, &mut counting_b, &env, cycles).expect("the run");
        (
            record,
            (counting_a.jit, counting_a.interpreter),
            (counting_b.jit, counting_b.interpreter),
        )
    };
    let elapsed = started.elapsed();
    let finished = Final {
        a: CpuView::of(&a.state),
        b: CpuView::of(&b.state),
        ram: ram_qwords(&bus),
    };
    let b_admissions = b.session.admissions();
    let (writes_a, writes_b) = (std::mem::take(&mut a.writes), std::mem::take(&mut b.writes));
    drop((a, b));
    drop(region);
    JitRun {
        mode: "interleaved",
        protocol,
        record,
        b_jit_steps: b_steps.0,
        b_interpreter_steps: b_steps.1,
        a_jit_steps: a_steps.0,
        a_interpreter_steps: a_steps.1,
        b_admissions,
        finished,
        writes_a,
        writes_b,
        elapsed,
    }
}

/// The state the two driver threads of the parallel mode share.
struct Shared {
    protocol: Protocol,
    cycles: u64,
    /// B's count of finished runs of the old code (A may write cycle `k` once it reaches `k`) and
    /// of finished cycles (A may write the next one).
    old_done: AtomicU64,
    cycle_done: AtomicU64,
    next: [AtomicU64; 2],
    finished_cpus: AtomicU64,
    abort: AtomicBool,
    record: Mutex<BRecord>,
    finals: Mutex<[Option<LongModeState>; 2]>,
    error: Mutex<Option<String>>,
}

impl Shared {
    fn fail(&self, message: String) -> &'static str {
        self.abort.store(true, Ordering::Relaxed);
        self.error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_or_insert(message);
        "S6-43a driver failed"
    }
}

/// One call of a vCPU thread's quantum: one cycle of its role.
fn drive(
    shared: &Shared,
    quantum: &JitHostQuantum,
    call: HostQuantumCall<'_>,
) -> Result<HostQuantumResult, HostQuantumFailure> {
    let cpu = call.cpu;
    let k = shared.next[cpu].load(Ordering::Relaxed) + 1;
    if k > shared.cycles {
        return Ok(HostQuantumResult {
            retired: 0,
            exit: HostQuantumExit::Halted,
            device_ns: 0,
        });
    }
    let bus = call.bus;
    let read_signal = signal_reader(bus);
    let env = Env {
        protocol: shared.protocol,
        read_signal: &read_signal,
        require_jit: true,
        require_native_writes: true,
        abort: &shared.abort,
    };
    let mut vcpu = Quantum {
        quantum,
        call,
        retired: 0,
        device_ns: 0,
    };
    let outcome = if cpu == CPU_A {
        spin_until(&shared.abort, "B's run of the old code", || {
            shared.old_done.load(Ordering::Acquire) >= k
        })
        .and_then(|()| a_write_and_signal(&mut vcpu, &env, k))
        .and_then(|()| {
            spin_until(&shared.abort, "B's cycle", || {
                shared.cycle_done.load(Ordering::Acquire) >= k
            })
        })
    } else {
        let mut rec = BRecord::default();
        let outcome = b_before_write(&mut vcpu, &env, k, &mut rec).and_then(|()| {
            shared.old_done.store(k, Ordering::Release);
            b_after_signal(&mut vcpu, &env, k, &mut rec)
        });
        shared.cycle_done.store(k, Ordering::Release);
        let mut total = shared
            .record
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        total.cycles += rec.cycles;
        total.x_old_jit += rec.x_old_jit;
        total.x_new_jit += rec.x_new_jit;
        total.x_not_jit += rec.x_not_jit;
        total.stale += rec.stale;
        total.wrong += rec.wrong;
        total.wait_steps += rec.wait_steps;
        total.wait_max = total.wait_max.max(rec.wait_max);
        total.serialize_steps += rec.serialize_steps;
        for failure in rec.failures {
            total.fail(failure);
        }
        outcome
    };
    if let Err(message) = outcome {
        return Err(HostQuantumFailure {
            retired: vcpu.retired,
            device_ns: vcpu.device_ns,
            message: shared.fail(format!("vCPU {cpu}, cycle {k}: {message}")),
        });
    }
    shared.next[cpu].store(k, Ordering::Relaxed);
    let mut exit = HostQuantumExit::Running;
    if k == shared.cycles {
        shared
            .finals
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[cpu] = Some(vcpu.state().clone());
        shared.finished_cpus.fetch_add(1, Ordering::Release);
        exit = HostQuantumExit::Halted;
    }
    Ok(HostQuantumResult {
        retired: vcpu.retired,
        exit,
        device_ns: vcpu.device_ns,
    })
}

/// The parallel mode: `run_accounted_host_threads`, one thread per vCPU, the bus by shared reference.
fn run_parallel(protocol: Protocol, cycles: u64) -> JitRun {
    let region = SharedJitRegion::new(2, 4).unwrap();
    let parts = vec![
        Some(region.take_part().unwrap()),
        Some(region.take_part().unwrap()),
    ];
    let quantum = JitHostQuantum::new(parts, vec![config(), config()], RFLAGS_DEFINED)
        .with_cpl0_cpuid(CPUID_PROFILE)
        .with_write_log();
    let mut board = host_parallel_board(2).unwrap();
    populate_tables(board.bus_mut(), &b_code());
    add_a_and_signal(board.bus_mut());
    assert!(host_install_vcpu(&mut board, CPU_B, b_state()));
    assert!(host_install_vcpu(&mut board, CPU_A, a_state()));
    let shared = Shared {
        protocol,
        cycles,
        old_done: AtomicU64::new(0),
        cycle_done: AtomicU64::new(0),
        next: [AtomicU64::new(0), AtomicU64::new(0)],
        finished_cpus: AtomicU64::new(0),
        abort: AtomicBool::new(false),
        record: Mutex::new(BRecord::default()),
        finals: Mutex::new([None, None]),
        error: Mutex::new(None),
    };
    let started = Instant::now();
    let (report, ()) = run_accounted_host_threads(
        &mut board,
        HostParallelConfig {
            quantum: 4096,
            budget: u64::MAX,
            end_when_idle: false,
        },
        &quantum.nmi_paging(),
        &|call| drive(&shared, &quantum, call),
        &|_| quantum.thread_exit(),
        |control| {
            let done = control.wait_until(Duration::from_secs(3600), || {
                shared.finished_cpus.load(Ordering::Acquire) == 2
                    || shared.abort.load(Ordering::Relaxed)
            });
            if !done {
                shared.abort.store(true, Ordering::Relaxed);
            }
        },
    )
    .expect("the board runs");
    let elapsed = started.elapsed();
    let error = shared.error.lock().unwrap().clone();
    assert_eq!(error, None, "a driver failed");
    assert_eq!(report.finished, HostParallelFinish::Stopped, "{report:?}");
    assert_eq!(report.threads_exited, 2, "every thread is joined");
    let record = shared.record.lock().unwrap().clone();
    let finals = shared.finals.lock().unwrap();
    let (b_state, a_state) = (
        finals[CPU_B].clone().expect("B finished"),
        finals[CPU_A].clone().expect("A finished"),
    );
    let finished = Final {
        a: CpuView::of(&a_state),
        b: CpuView::of(&b_state),
        ram: ram_qwords(board.bus_mut()),
    };
    let (b, a) = (quantum.stats(CPU_B), quantum.stats(CPU_A));
    let writes = (
        quantum.committed_writes(CPU_A),
        quantum.committed_writes(CPU_B),
    );
    drop(quantum);
    drop(region);
    JitRun {
        mode: "parallel",
        protocol,
        record,
        b_jit_steps: b.steps_jit,
        b_interpreter_steps: b.steps_interpreter,
        a_jit_steps: a.steps_jit,
        a_interpreter_steps: a.steps_interpreter,
        b_admissions: b.admissions,
        finished,
        writes_a: writes.0,
        writes_b: writes.1,
        elapsed,
    }
}

fn data_writes(writes: &[(u64, u64)]) -> Vec<(u64, u64)> {
    writes
        .iter()
        .copied()
        .filter(|(start, _)| *start >= DATA_WRITES_FROM)
        .collect()
}

/// The JIT run against the oracle: B's results are checked inside the cycle functions; here the
/// final state, the RAM and the committed writes.
fn assert_matches_the_oracle(run: &JitRun, oracle: &OracleRun, complete_writes: bool) {
    run.assert_clean();
    assert_eq!(oracle.record.stale + oracle.record.wrong, 0, "the oracle");
    assert!(
        oracle.record.failures.is_empty(),
        "{:?}",
        oracle.record.failures
    );
    let what = format!("{} / {}", run.mode, run.protocol.name);
    let cycles = run.record.cycles;
    let cpuid_steps = if run.protocol.serialize { cycles } else { 0 };
    assert_eq!(run.record.serialize_steps, cpuid_steps, "{what}: CPUID");
    assert_eq!(
        oracle.record.serialize_steps, cpuid_steps,
        "{what}: CPUID in the interpreter"
    );
    assert_same_final(&run.finished, &oracle.finished, &what);
    // The same final encoding of X is in B's code page in both runs.
    assert_eq!(
        run.finished.ram[X_INDEX],
        qword(version_bytes(cycles)),
        "{what}: X in the JIT run"
    );
    if complete_writes {
        assert_eq!(
            run.writes_a, oracle.writes_a,
            "{what}: A's committed writes"
        );
        assert_eq!(
            run.writes_b, oracle.writes_b,
            "{what}: B's committed writes"
        );
    } else {
        // Both vCPUs walk the same page tables and the first one to touch an entry commits its
        // Accessed/Dirty bit: only the data stores are compared, per vCPU, in program order.
        assert_eq!(
            data_writes(&run.writes_a),
            data_writes(&oracle.writes_a),
            "{what}: A's data stores"
        );
        assert_eq!(
            data_writes(&run.writes_b),
            data_writes(&oracle.writes_b),
            "{what}: B's data stores"
        );
    }
}

fn cycles_from_env(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

/// The oracle alone: the interpreter of od-isa gives the version results the JIT runs are held to
/// (old code before the write, new code after the signal, for every protocol). The reference
/// gives the expected values but does not check cross-modifying code: every version runs the same
/// bytes `48 8B 03`, and RBX (`version_rbx`) sets the result.
#[test]
fn s6_43a_the_interpreter_reference_gives_the_expected_values_around_the_signal() {
    for protocol in PROTOCOLS {
        let oracle = run_oracle(protocol, 40, false);
        assert_eq!(
            oracle.record.failures,
            Vec::<String>::new(),
            "{}",
            protocol.name
        );
        assert_eq!(oracle.record.cycles, 40);
        // A's list: per cycle the code store and the signal store, as data writes.
        assert_eq!(
            data_writes(&oracle.writes_a).len(),
            2 * 40,
            "{}",
            protocol.name
        );
        assert_eq!(
            oracle.finished.b.gpr[Register::Rax as usize],
            version_result(40)
        );
    }
}

/// Interleaved mode, fixed schedule (B old, A writes and signals, B waits/CPUID step/new), every
/// protocol: B runs the old code before and only the new code after; the registers, the RAM and
/// the committed writes equal the interpreter's run of the same schedule.
#[test]
fn s6_43a_interleaved_b_runs_only_the_new_bytes_after_the_signal_and_equals_the_interpreter() {
    let _guard = jit_guard();
    let cycles = cycles_from_env("OD_S6_43A_INTERLEAVED_CYCLES", 600);
    for protocol in PROTOCOLS {
        let run = run_interleaved(protocol, cycles);
        eprintln!("{}", run.line());
        let oracle = run_oracle(protocol, cycles, false);
        assert_matches_the_oracle(&run, &oracle, true);
        assert_eq!(run.record.cycles, cycles);
        // One signal read per cycle in this schedule: the signal is already there.
        assert_eq!(run.record.wait_steps, cycles);
    }
}

/// Parallel mode (`run_accounted_host_threads`, no lock of the bus): the same, with A and B on their own
/// threads; B waits for the signal in a loop, A waits for B's run of the old code and for B's
/// cycle.
#[test]
fn s6_43a_parallel_b_runs_only_the_new_bytes_after_the_signal_and_equals_the_interpreter() {
    let _guard = jit_guard();
    let cycles = cycles_from_env("OD_S6_43A_PARALLEL_CYCLES", 2000);
    for protocol in PROTOCOLS {
        let run = run_parallel(protocol, cycles);
        eprintln!("{}", run.line());
        let oracle = run_oracle(protocol, cycles, false);
        assert_matches_the_oracle(&run, &oracle, false);
        assert_eq!(run.record.cycles, cycles);
        assert_eq!(run.a_interpreter_steps, 0, "A ran natively only");
    }
}

/// The long form, release: `OD_S6_43A_CYCLES` signals per protocol (10 000 at least, the
/// executor's choice; the default is 2 000 000) in the parallel mode.
#[test]
#[ignore = "long: release run of the S6-43a probe"]
fn s6_43a_long_parallel_run() {
    let _guard = jit_guard();
    let cycles = cycles_from_env("OD_S6_43A_CYCLES", 2_000_000);
    assert!(cycles >= 10_000);
    for protocol in PROTOCOLS {
        let run = run_parallel(protocol, cycles);
        eprintln!("{}", run.line());
        let oracle = run_oracle(protocol, cycles, false);
        assert_matches_the_oracle(&run, &oracle, false);
        eprintln!(
            "S6-43a parallel / {}: equals the interpreter (registers, RAM, data stores): yes",
            protocol.name
        );
    }
}

/// A run in which the JIT did not execute the changed code does not count. The interpreter runs
/// the whole schedule with native execution required: every run of X is a fallback, the results
/// are right and the record is not counted; a JIT run of the same schedule is counted and its
/// share of the interpreter is only the CPUID step.
#[test]
fn s6_43a_a_run_without_native_execution_of_the_changed_code_is_not_counted() {
    let _guard = jit_guard();
    let cycles = 20;
    let fallback = run_oracle(PROTOCOLS[0], cycles, true);
    assert!(fallback.record.failures.is_empty());
    assert_eq!(fallback.record.x_old_jit + fallback.record.x_new_jit, 0);
    assert_eq!(fallback.record.x_not_jit, 3 * cycles);
    assert!(!fallback.record.counted());
    let counted = run_interleaved(PROTOCOLS[0], cycles);
    assert!(counted.counted(), "{}", counted.line());
    assert_eq!(counted.record.x_not_jit, 0);
    // B's steps per cycle: the old run, the signal read, the CPUID step, two new runs.
    assert_eq!(counted.b_interpreter_steps, cycles);
    assert_eq!(counted.b_jit_steps, 4 * cycles);
    assert!((counted.fallback_percent() - 20.0).abs() < 1e-9);
    // One native run short of the rule is not counted either.
    let mut short = counted.record.clone();
    short.x_new_jit -= 1;
    assert!(!short.counted());
}
