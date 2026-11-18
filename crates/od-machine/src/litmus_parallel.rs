//! S6-36: the S6-05 litmus corpus in the parallel mode (S6-34): one host thread per vCPU, the real
//! interpreter and the real RAM access path (`SeqCst` ordinary accesses, `LOCK` as host atomics,
//! host fences for the guest fences), on the machine of the S6-34 threads.
//!
//! A round is one call of the quantum of every vCPU thread, so every round goes through the one
//! thread loop of S6-34 (the barrier, the boundary, the charge to the clock owner). Inside the call
//! the round is made as simultaneous as the host allows:
//!
//! 1. the vCPU of litmus thread `t` is given the initial registers and RIP of `t` (the vCPUs are
//!    created directly in long mode on one identity 2 MiB table set: SMP_CONTRACT 4.4, litmus only,
//!    not an AP start);
//! 2. the start barrier (a host spin barrier over ALL vCPUs of the machine, also those that run no
//!    litmus thread this round) is crossed inside the first poll of the interpreter's stop test, that
//!    is, after the quantum has been set up and before the first guest instruction, so the threads
//!    leave it within the host's cache latency of each other and not microseconds apart;
//! 3. exactly the instructions of the thread are executed (the budget of the call; no `HLT`);
//! 4. the observed registers are kept, the end barrier is crossed, and the last thread to cross it
//!    reads the observed cells from RAM, puts the outcome in the histogram, writes the initial
//!    values back to the cells, and lets the others go to the next round.
//!
//! Which litmus thread runs on which vCPU is part of the configuration: round `r` uses placement
//! `r mod P`, where the placements are all the injective maps of the `T` litmus threads on the `V`
//! vCPUs (2 on 2 vCPUs, 12 on 4 vCPUs for two threads, 24 for four): a test is not tied to the
//! boot processor or to one pair of cores. A vCPU that has no litmus thread in a round only crosses
//! the barriers.
//!
//! The expected set is the x86-TSO set of the fixture (the formula for a counter), read from the
//! fixture file and never from a run. The observed set must be a subset of it; an outcome outside
//! it is reported, with the round and the placement of its first appearance. No finite run proves
//! an outcome impossible, and a run that finds nothing proves nothing about the sensitivity of the
//! check: that is the job of the weakened builds (`weak-host-ordering`, `nonatomic-lock-test`,
//! `SMP.md` 5.1).

use crate::firmware_board::vcpu_quantum::run_vcpu_set_quantum_shared;
use crate::firmware_board::{FIRMWARE_CR0, FIRMWARE_CR4, assemble_board};
use crate::litmus::{LitmusCase, LitmusError};
use crate::litmus_interleaving::{Case, Field, Outcome};
use crate::parallel_run::{ParallelConfig, ParallelFinish, QuantumCall, run_threads_with};
use crate::smp_scheduler::Executed;
use crate::{
    FirmwareBoardConfig, FirmwareBoardError, GuestPhysicalAddress, LoadedFirmwareBoard, PciEcam,
};
use od_isa::execute::{ControlRegisterPolicy, Cpl0ProfileSet, MovCr3Long64V1};
use od_isa::state::Gdtr;
use od_isa::{
    CliProfile, CodeSegmentDescriptorCache, CpuActivity, HaltProfile, LongModeState,
    StackSegmentDescriptorCache, StiProfile, StiShadow,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const SEQ: Ordering = Ordering::SeqCst;
/// The table set (PML4, PDPT, PD): the first 2 MiB identity-mapped by one 2 MiB page.
pub(crate) const TABLES: u64 = 0x1_0000;
const GDT_AT: u64 = 0x1_4000;
pub(crate) const EFER: u64 = 0xD01;
/// The most vCPUs of this harness.
pub const LITMUS_PARALLEL_MAX_VCPUS: usize = 4;
/// How long a thread waits at a barrier for the others before the run is given up.
const BARRIER_TIMEOUT: Duration = Duration::from_secs(60);
/// The largest delay (in `spin_loop` hints) after the start barrier.
const JITTER_MAX: u64 = 256;
/// The steps of one call at most (above any litmus thread).
const QUANTUM: u64 = 1 << 20;

/// One observed outcome of a parallel run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LitmusParallelOutcome {
    pub values: Vec<u64>,
    pub rounds: u64,
    /// The first round (index) that reached it and its placement (litmus thread -> vCPU).
    pub first_round: u64,
    pub first_placement: Vec<usize>,
    /// In the x86-TSO set (fixture) or formula of the case.
    pub in_x86_tso: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LitmusParallelVerdict {
    /// Every observed outcome is in the x86-TSO set.
    Allowed,
    /// At least one observed outcome is outside it.
    Forbidden,
}

impl LitmusParallelVerdict {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Allowed => "allowed_only",
            Self::Forbidden => "forbidden",
        }
    }
}

#[derive(Clone, Debug)]
pub struct LitmusParallelReport {
    pub id: String,
    /// Litmus threads the case needs (all concurrent).
    pub threads: usize,
    /// vCPUs of the machine.
    pub vcpus: usize,
    /// The placements cycled through (`P`).
    pub placements: usize,
    pub rounds: u64,
    pub observe: Vec<String>,
    pub histogram: Vec<LitmusParallelOutcome>,
    pub x86_tso: Vec<Vec<u64>>,
    pub sc: Vec<Vec<u64>>,
    pub interesting: Vec<Vec<u64>>,
    pub forbidden: Vec<Vec<u64>>,
    /// Information only (S6-36 review, nit): observed outcomes outside the sequentially consistent
    /// set of the fixture, that is, outside SC but (as the verdict says) inside x86-TSO; empty when
    /// the fixture has no SC set (the counters). Never a failure: x86-TSO allows them.
    pub outside_sc: Vec<Vec<u64>>,
    /// Outcomes of the x86-TSO set that no round reached (not an error).
    pub not_observed: Vec<Vec<u64>>,
    /// Rounds of the threads that ran a litmus thread in which the start barrier was not crossed
    /// inside the first poll of the stop test (the threads then ran one after another).
    pub late_barrier_rounds: u64,
    pub retired: Vec<u64>,
    /// S6-41e: instructions of all rounds executed by the step hook and by the interpreter (the
    /// hook's are 0 in a run without one).
    pub hooked_retired: u64,
    pub interpreted_retired: u64,
    pub elapsed_ns: u128,
    pub verdict: LitmusParallelVerdict,
}

/// S6-41e: one instruction offered to a [`LitmusStepHook`] (the JIT of `od-host`): the vCPU's
/// state, the machine by shared reference and the translation context the board's interpreter
/// uses for this litmus (`cr3` of the identity table set, `efer`).
pub struct LitmusStep<'a> {
    /// The vCPU that runs the step.
    pub cpu: usize,
    /// The litmus thread the vCPU runs this round.
    pub thread: usize,
    /// Index of the instruction in the thread (the program of [`LitmusCase::program`]).
    pub index: usize,
    /// At most this many instructions may be executed (at least 1; never beyond the thread's end).
    pub budget: u64,
    pub state: &'a mut LongModeState,
    pub bus: &'a crate::MachineBus,
    pub cr3: u64,
    pub efer: u64,
}

/// What a [`LitmusStepHook`] did with a step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LitmusStepOutcome {
    /// The hook executed `retired` (at least 1, at most `budget`) instructions from the step's RIP.
    Executed { retired: u64 },
    /// The hook does not take this instruction: the interpreter executes it.
    Declined,
}

/// S6-41e: the point where a host-side executor (the JIT of `od-host`, which `od-machine` does not
/// depend on) takes instructions of a litmus run before the interpreter. The run, the barriers, the
/// placements, the histogram and the reference sets are those of S6-36/S6-19 unchanged; only the
/// execution of an instruction the hook accepts differs. `Sync`: in the parallel mode every vCPU
/// thread calls it (state of one vCPU lives on its thread).
pub trait LitmusStepHook: Sync {
    /// Offered an instruction on the vCPU's own thread. `Err` ends the run as a guest failure.
    fn step(&self, step: LitmusStep<'_>) -> Result<LitmusStepOutcome, String>;
    /// Called on the vCPU's own thread after its last step (the owner of per-thread state releases
    /// it here); in the interleaved run, once on the calling thread for every vCPU.
    fn thread_exit(&self, cpu: usize);
}

/// All the injective maps of `threads` litmus threads on `vcpus` vCPUs, as `vcpu -> Some(thread)`.
fn placements(threads: usize, vcpus: usize) -> Vec<Vec<Option<usize>>> {
    fn extend(
        thread: usize,
        threads: usize,
        current: &mut Vec<Option<usize>>,
        out: &mut Vec<Vec<Option<usize>>>,
    ) {
        if thread == threads {
            out.push(current.clone());
            return;
        }
        for cpu in 0..current.len() {
            if current[cpu].is_none() {
                current[cpu] = Some(thread);
                extend(thread + 1, threads, current, out);
                current[cpu] = None;
            }
        }
    }
    let mut out = Vec::new();
    extend(0, threads, &mut vec![None; vcpus], &mut out);
    out
}

/// A host barrier of `parties` threads that spins (then yields): the last to arrive may do work
/// before it lets the others go.
struct Barrier {
    parties: usize,
    count: AtomicUsize,
    generation: AtomicU64,
}

enum Arrival {
    /// This thread arrived last; call [`Barrier::release`] when its work is done.
    Last,
    /// Wait for the release with [`Barrier::wait`].
    Early(u64),
}

impl Barrier {
    fn new(parties: usize) -> Self {
        Self {
            parties,
            count: AtomicUsize::new(0),
            generation: AtomicU64::new(0),
        }
    }

    fn arrive(&self) -> Arrival {
        let generation = self.generation.load(SEQ);
        if self.count.fetch_add(1, SEQ) + 1 == self.parties {
            Arrival::Last
        } else {
            Arrival::Early(generation)
        }
    }

    fn release(&self) {
        self.count.store(0, SEQ);
        self.generation.fetch_add(1, SEQ);
    }

    /// `false` when the run was given up (`abort`, or the timeout: then `abort` is set).
    fn wait(&self, generation: u64, abort: &AtomicBool) -> bool {
        let mut spins = 0u64;
        let mut deadline: Option<Instant> = None;
        while self.generation.load(SEQ) == generation {
            spins += 1;
            if spins < 4_000 {
                std::hint::spin_loop();
                continue;
            }
            std::thread::yield_now();
            if spins.is_multiple_of(1_024) {
                if abort.load(SEQ) {
                    return false;
                }
                let limit = *deadline.get_or_insert_with(|| Instant::now() + BARRIER_TIMEOUT);
                if Instant::now() >= limit {
                    abort.store(true, SEQ);
                    return false;
                }
            }
        }
        true
    }

    /// Cross with nothing to do by the last thread.
    fn cross(&self, abort: &AtomicBool) -> bool {
        match self.arrive() {
            Arrival::Last => {
                self.release();
                true
            }
            Arrival::Early(generation) => self.wait(generation, abort),
        }
    }
}

/// Host spins a thread waits after the start barrier: SplitMix64 of the round and the thread, from
/// `0` to [`JITTER_MAX`].
fn jitter(round: u64, thread: usize) -> u64 {
    let mut x = round
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add((thread as u64 + 1).wrapping_mul(0xBF58_476D_1CE4_E5B9));
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (x ^ (x >> 31)) % (JITTER_MAX + 1)
}

struct Seen {
    rounds: u64,
    first_round: u64,
    first_placement: Vec<usize>,
}

struct Driver<'c> {
    case: &'c Case,
    rounds: u64,
    placements: Vec<Vec<Option<usize>>>,
    start: Barrier,
    end: Barrier,
    /// Rounds each vCPU has begun.
    next_round: Vec<AtomicU64>,
    /// The observed registers, one per observed field (read only for register fields).
    registers: Vec<AtomicU64>,
    histogram: Mutex<BTreeMap<Outcome, Seen>>,
    completed: AtomicU64,
    done: AtomicBool,
    abort: AtomicBool,
    late_barrier_rounds: AtomicU64,
    failure: Mutex<Option<String>>,
    /// S6-41e: the step hook (the JIT of `od-host`), `None` for the interpreter-only run of S6-36.
    hook: Option<&'c dyn LitmusStepHook>,
    /// Instructions the hook executed and instructions the interpreter executed, all rounds.
    hooked_retired: AtomicU64,
    interpreted_retired: AtomicU64,
}

impl Driver<'_> {
    fn fail(&self, detail: String) {
        let mut failure = self.failure.lock().unwrap_or_else(|e| e.into_inner());
        failure.get_or_insert(detail);
        self.abort.store(true, SEQ);
    }

    /// The call of the quantum of one vCPU thread: one round.
    fn round(&self, call: QuantumCall<'_, '_>) -> Result<Executed, FirmwareBoardError> {
        let QuantumCall {
            env,
            vcpu,
            cpu,
            lapic,
            source,
            gate,
            budget,
            consecutive_faults,
            stop,
            ..
        } = call;
        let round = self.next_round[cpu].fetch_add(1, SEQ);
        if round >= self.rounds || self.abort.load(SEQ) {
            // The rounds are over (the controller ends the run), or the run is given up.
            self.next_round[cpu].store(self.rounds, SEQ);
            std::thread::yield_now();
            return Ok(Executed::default());
        }
        let placement = &self.placements[(round % self.placements.len() as u64) as usize];
        let memory = env.bus.physical_memory();
        let mut retired = 0u64;
        let mut retired_by_hook = 0u64;
        match placement[cpu] {
            Some(thread) => {
                let spec = &self.case.threads[thread];
                vcpu.state.gpr = [0; 16];
                for (register, value) in &spec.registers {
                    vcpu.state.gpr[*register as usize] = *value;
                }
                vcpu.state.rip = spec.code_start;
                vcpu.state.rflags = 0x2;
                vcpu.skip_first_boundary = false;
                vcpu.state.activity = Some(CpuActivity::Running);
                let total = spec.instructions.len() as u64;
                let end_rip = spec.code_start
                    + spec
                        .instructions
                        .iter()
                        .map(|i| i.bytes.len() as u64)
                        .sum::<u64>();
                let mut crossed = false;
                let mut barrier_failed = false;
                let (cli, sti, halt) = (
                    CliProfile::NonPviLong64V1,
                    StiProfile::NonPviLong64V1,
                    HaltProfile::Long64V1,
                );
                let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
                    .with_cli(&cli)
                    .with_cpu_owned_cr3(MovCr3Long64V1)
                    .with_pio_u8()
                    .with_halt(&halt)
                    .with_sti(&sti);
                // `stop` is polled between guest instructions, the first time before the first
                // one: the start barrier is crossed there.
                let mut poll = || {
                    if !crossed {
                        crossed = true;
                        if !self.start.cross(&self.abort) {
                            barrier_failed = true;
                            return true;
                        }
                        // A pseudo-random delay of this thread in this round, so that the
                        // threads do not stand in lock step after the barrier.
                        for _ in 0..jitter(round, thread) {
                            std::hint::spin_loop();
                        }
                    }
                    stop()
                };
                while retired < total {
                    if let Some(hook) = self.hook {
                        // S6-41e: the hook runs an instruction without the interpreter's stop
                        // test, so the start barrier and the stop poll are taken here, before the
                        // hook as before the interpreter.
                        if poll() {
                            break;
                        }
                        let step_budget = budget.max(1).min(total - retired);
                        let step = LitmusStep {
                            cpu,
                            thread,
                            index: retired as usize,
                            budget: step_budget,
                            state: &mut vcpu.state,
                            bus: env.bus,
                            cr3: TABLES,
                            efer: EFER,
                        };
                        match hook.step(step) {
                            Ok(LitmusStepOutcome::Executed { retired: done })
                                if done > step_budget =>
                            {
                                self.fail(format!(
                                    "cpu{cpu} thread {thread} round {round}: hook retired {done} \
                                     over a budget of {step_budget}"
                                ));
                                return Err(FirmwareBoardError::Config(
                                    "litmus parallel: the step hook exceeded its budget",
                                ));
                            }
                            Ok(LitmusStepOutcome::Executed { retired: done }) if done > 0 => {
                                retired += done;
                                retired_by_hook += done;
                                continue;
                            }
                            Ok(_) => {}
                            Err(detail) => {
                                self.fail(format!(
                                    "cpu{cpu} thread {thread} round {round}: hook: {detail}"
                                ));
                                return Err(FirmwareBoardError::Config(
                                    "litmus parallel: the step hook failed",
                                ));
                            }
                        }
                    }
                    // With a hook the interpreter takes ONE instruction (the next one may be the
                    // hook's); without it, what is left of the thread.
                    let interpreter_budget = if self.hook.is_some() {
                        1
                    } else {
                        budget.max(1).min(total - retired)
                    };
                    let executed = run_vcpu_set_quantum_shared(
                        env,
                        vcpu,
                        cpu,
                        lapic,
                        &mut *source,
                        Some(gate),
                        interpreter_budget,
                        &mut poll,
                        &set,
                        consecutive_faults,
                    );
                    let executed = match executed {
                        Ok(executed) => executed,
                        Err(error) => {
                            self.fail(format!("cpu{cpu} thread {thread} round {round}: {error:?}"));
                            return Err(error);
                        }
                    };
                    retired += executed.retired;
                    if executed.retired == 0 {
                        break;
                    }
                }
                if barrier_failed {
                    return Err(FirmwareBoardError::Config("litmus parallel: aborted"));
                }
                if !crossed {
                    // The instructions ran before the barrier: the round is sequential. Counted.
                    self.late_barrier_rounds.fetch_add(1, SEQ);
                    if !self.start.cross(&self.abort) {
                        return Err(FirmwareBoardError::Config("litmus parallel: aborted"));
                    }
                }
                if retired != total || vcpu.state.rip != end_rip {
                    self.fail(format!(
                        "cpu{cpu} thread {thread} round {round}: retired {retired} of {total}, rip {:#x}, end {end_rip:#x}",
                        vcpu.state.rip
                    ));
                    return Err(FirmwareBoardError::Config(
                        "litmus parallel: a thread did not run to its end",
                    ));
                }
                for (index, field) in self.case.observe.iter().enumerate() {
                    if let Field::Register {
                        cpu: owner,
                        register,
                    } = field
                        && *owner == thread
                    {
                        self.registers[index]
                            .store(vcpu.state.gpr[*register as usize] & 0xFFFF_FFFF, SEQ);
                    }
                }
            }
            None => {
                if !self.start.cross(&self.abort) {
                    return Err(FirmwareBoardError::Config("litmus parallel: aborted"));
                }
            }
        }
        // The end barrier: the last thread to arrive makes the outcome and resets the cells.
        match self.end.arrive() {
            Arrival::Last => {
                self.finish_round(round, placement, memory);
                self.end.release();
            }
            Arrival::Early(generation) => {
                if !self.end.wait(generation, &self.abort) {
                    return Err(FirmwareBoardError::Config("litmus parallel: aborted"));
                }
            }
        }
        // Publish accounting between rounds so it cannot order the guest accesses.
        if retired_by_hook != 0 {
            self.hooked_retired.fetch_add(retired_by_hook, SEQ);
        }
        let retired_by_interpreter = retired - retired_by_hook;
        if retired_by_interpreter != 0 {
            self.interpreted_retired
                .fetch_add(retired_by_interpreter, SEQ);
        }
        Ok(Executed {
            retired,
            partial_rep: false,
            device_ns: 0,
        })
    }

    /// The outcome of round `round` into the histogram; the cells back to their initial values.
    fn finish_round(
        &self,
        round: u64,
        placement: &[Option<usize>],
        memory: &crate::PhysicalMemory,
    ) {
        let read = |address: u64| {
            memory
                .load_u32(GuestPhysicalAddress::new(address))
                .map(u64::from)
        };
        let mut outcome = Vec::with_capacity(self.case.observe.len());
        for (index, field) in self.case.observe.iter().enumerate() {
            match field {
                Field::Register { .. } => outcome.push(self.registers[index].load(SEQ)),
                Field::Cell { address } => match read(*address) {
                    Ok(value) => outcome.push(value),
                    Err(error) => {
                        self.fail(format!("round {round}: reading a cell: {error:?}"));
                        return;
                    }
                },
            }
        }
        {
            let mut histogram = self.histogram.lock().unwrap_or_else(|e| e.into_inner());
            let mut by_thread = vec![0usize; self.case.threads.len()];
            for (vcpu, slot) in placement.iter().enumerate() {
                if let Some(thread) = slot {
                    by_thread[*thread] = vcpu;
                }
            }
            histogram
                .entry(outcome)
                .and_modify(|seen| seen.rounds += 1)
                .or_insert(Seen {
                    rounds: 1,
                    first_round: round,
                    first_placement: by_thread,
                });
        }
        for (_, address, initial) in &self.case.cells {
            if let Err(error) =
                memory.store_u32(GuestPhysicalAddress::new(*address), *initial as u32, |_| {})
            {
                self.fail(format!("round {round}: resetting a cell: {error:?}"));
                return;
            }
        }
        if self.completed.fetch_add(1, SEQ) + 1 == self.rounds {
            self.done.store(true, SEQ);
        }
    }
}

pub(crate) fn long_state(rip: u64) -> LongModeState {
    LongModeState {
        rip,
        rflags: 0x2,
        cr0: Some(FIRMWARE_CR0),
        cr4: Some(FIRMWARE_CR4),
        cr3: Some(TABLES),
        cs_selector: Some(0x08),
        cs_cache: Some(CodeSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x9B,
            flags: 2,
        }),
        ss_selector: Some(0x10),
        ss_cache: Some(StackSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x93,
            flags: 0,
        }),
        gdtr: Some(Gdtr {
            base: GDT_AT,
            limit: 0x17,
        }),
        sti_shadow: Some(StiShadow::Clear),
        activity: Some(CpuActivity::Running),
        ..LongModeState::default()
    }
}

/// A board of `vcpus` vCPUs with the litmus case loaded: the code of every litmus thread at its
/// region, the cells at their initial values, one identity table set, every vCPU in long mode.
///
/// `pages_4k` (S6-41e): the first 2 MiB are mapped by 512 identity 4 KiB pages (a page table at
/// `TABLES + 0x3000`) instead of one 2 MiB page, because the JIT's fetch window and helper walk
/// 4 KiB pages (`UnsupportedPageSize` for a 2 MiB leaf); S6-36 keeps the 2 MiB page.
pub(crate) fn build_board(
    case: &Case,
    vcpus: usize,
    pages_4k: bool,
) -> Result<LoadedFirmwareBoard, String> {
    let config = FirmwareBoardConfig {
        vcpu_count: vcpus,
        ..FirmwareBoardConfig::default()
    };
    let mut board = assemble_board(&config, PciEcam::new(), None, None)
        .and_then(|assembled| assembled.into_linux_board(TABLES, 0x3000))
        .map_err(|error| format!("board: {error:?}"))?;
    board.bus_mut().enable_apic_mmio_windows();
    board
        .enable_clock_owner()
        .map_err(|error| format!("clock owner: {error:?}"))?;
    let mut patch = |at: u64, bytes: &[u8]| {
        board
            .bus_mut()
            .patch_physical_ram(GuestPhysicalAddress::new(at), bytes)
            .map_err(|error| format!("patch {at:#x}: {error:?}"))
    };
    let entry = |value: u64| value.to_le_bytes();
    patch(TABLES, &entry((TABLES + 0x1000) | 3))?;
    patch(TABLES + 0x1000, &entry((TABLES + 0x2000) | 3))?;
    if pages_4k {
        patch(TABLES + 0x2000, &entry((TABLES + 0x3000) | 3))?;
        let mut table = vec![0u8; 0x1000];
        for page in 0..512u64 {
            // Present, writable, accessed, dirty: the walks set no bit in a table.
            let leaf = (page << 12) | 0x63;
            table[page as usize * 8..page as usize * 8 + 8].copy_from_slice(&leaf.to_le_bytes());
        }
        patch(TABLES + 0x3000, &table)?;
    } else {
        patch(TABLES + 0x2000, &entry(0x83))?;
    }
    let mut gdt = [0u8; 24];
    gdt[8..16].copy_from_slice(&0x00AF_9B00_0000_FFFFu64.to_le_bytes());
    gdt[16..24].copy_from_slice(&0x00CF_9300_0000_FFFFu64.to_le_bytes());
    patch(GDT_AT, &gdt)?;
    for (_, address, initial) in &case.cells {
        patch(*address, &(*initial as u32).to_le_bytes())?;
    }
    for thread in &case.threads {
        let mut at = thread.code_start;
        for instruction in &thread.instructions {
            patch(at, &instruction.bytes)?;
            at += instruction.bytes.len() as u64;
        }
    }
    for cpu in 0..vcpus {
        let vcpu = board.vcpu_mut(cpu).ok_or("a vCPU of the board")?;
        vcpu.state = long_state(0x2000);
        vcpu.guest_efer = EFER;
        vcpu.legacy = None;
    }
    Ok(board)
}

/// Run `case` for `rounds` rounds in the parallel mode on a machine of `vcpus` vCPUs.
///
/// `Err(Policy)` when the case needs more threads than vCPUs, `vcpus` is not `2..=4` or `rounds`
/// is 0; `Err(Guest)` when a thread leaves the way a litmus cannot (a fault, a thread that did not
/// run to its end) or the run is given up.
pub fn run_litmus_parallel(
    case: &LitmusCase,
    vcpus: usize,
    rounds: u64,
) -> Result<LitmusParallelReport, LitmusError> {
    run_litmus_parallel_with(case, vcpus, rounds, None)
}

/// S6-41e: [`run_litmus_parallel`] with a [`LitmusStepHook`] that takes instructions before the
/// interpreter (the JIT of `od-host`). Everything else is the run of S6-36.
pub fn run_litmus_parallel_hooked(
    case: &LitmusCase,
    vcpus: usize,
    rounds: u64,
    hook: &dyn LitmusStepHook,
) -> Result<LitmusParallelReport, LitmusError> {
    run_litmus_parallel_with(case, vcpus, rounds, Some(hook))
}

fn run_litmus_parallel_with(
    case: &LitmusCase,
    vcpus: usize,
    rounds: u64,
    hook: Option<&dyn LitmusStepHook>,
) -> Result<LitmusParallelReport, LitmusError> {
    let case = case.case();
    let threads = case.threads.len();
    if !(2..=LITMUS_PARALLEL_MAX_VCPUS).contains(&vcpus) {
        return Err(LitmusError::Policy(format!(
            "--vcpus is 2..={LITMUS_PARALLEL_MAX_VCPUS}, got {vcpus}"
        )));
    }
    if threads > vcpus {
        return Err(LitmusError::Policy(format!(
            "{}: {threads} concurrent threads need at least {threads} vCPUs, the machine has {vcpus}",
            case.id
        )));
    }
    if rounds == 0 {
        return Err(LitmusError::Policy(
            "the number of rounds is at least 1".into(),
        ));
    }
    let mut board = build_board(case, vcpus, hook.is_some()).map_err(LitmusError::Load)?;
    let driver = Driver {
        case,
        rounds,
        placements: placements(threads, vcpus),
        start: Barrier::new(vcpus),
        end: Barrier::new(vcpus),
        next_round: (0..vcpus).map(|_| AtomicU64::new(0)).collect(),
        registers: (0..case.observe.len()).map(|_| AtomicU64::new(0)).collect(),
        histogram: Mutex::new(BTreeMap::new()),
        completed: AtomicU64::new(0),
        done: AtomicBool::new(false),
        abort: AtomicBool::new(false),
        late_barrier_rounds: AtomicU64::new(0),
        failure: Mutex::new(None),
        hook,
        hooked_retired: AtomicU64::new(0),
        interpreted_retired: AtomicU64::new(0),
    };
    let quantum = |call: QuantumCall<'_, '_>| driver.round(call);
    // The spread fills the test-only fields of the struct.
    #[allow(clippy::needless_update)]
    let config = ParallelConfig {
        quantum: QUANTUM,
        budget: u64::MAX,
        split_timeout: Duration::from_secs(30),
        end_when_idle: false,
        ..ParallelConfig::default()
    };
    let started = Instant::now();
    // The hook's per-thread state is released on the vCPU's own thread, after its last quantum.
    let exit = |cpu: usize| {
        if let Some(hook) = hook {
            hook.thread_exit(cpu);
        }
    };
    let (report, finished) = run_threads_with(
        &mut board,
        config,
        false,
        &quantum,
        Some(&exit),
        |control| {
            control.wait_until(Duration::from_secs(24 * 3600), || {
                driver.done.load(SEQ) || driver.abort.load(SEQ)
            })
        },
    )
    .map_err(|error| LitmusError::Load(format!("parallel run: {error:?}")))?;
    let elapsed_ns = started.elapsed().as_nanos();
    if let Some(detail) = driver
        .failure
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    {
        return Err(LitmusError::Guest {
            cpu: 0,
            rip: 0,
            exit: detail,
        });
    }
    if !finished || driver.abort.load(SEQ) || !matches!(report.finished, ParallelFinish::Stopped) {
        return Err(LitmusError::Guest {
            cpu: 0,
            rip: 0,
            exit: format!("the run did not complete: {:?}", report.finished),
        });
    }
    let histogram = driver
        .histogram
        .into_inner()
        .unwrap_or_else(|e| e.into_inner());
    let total: u64 = histogram.values().map(|seen| seen.rounds).sum();
    if total != rounds {
        return Err(LitmusError::Guest {
            cpu: 0,
            rip: 0,
            exit: format!("{total} outcomes counted in {rounds} rounds"),
        });
    }
    let observed: BTreeSet<&Outcome> = histogram.keys().collect();
    let forbidden: Vec<Vec<u64>> = histogram
        .keys()
        .filter(|values| !case.reference_tso.contains(*values))
        .cloned()
        .collect();
    let outside_sc: Vec<Vec<u64>> = if case.sc.is_empty() {
        Vec::new()
    } else {
        histogram
            .keys()
            .filter(|values| !case.sc.contains(*values))
            .cloned()
            .collect()
    };
    let not_observed: Vec<Vec<u64>> = case
        .reference_tso
        .iter()
        .filter(|values| !observed.contains(values))
        .cloned()
        .collect();
    let verdict = if forbidden.is_empty() {
        LitmusParallelVerdict::Allowed
    } else {
        LitmusParallelVerdict::Forbidden
    };
    Ok(LitmusParallelReport {
        id: case.id.clone(),
        threads,
        vcpus,
        placements: driver.placements.len(),
        rounds,
        observe: case.observe_names.clone(),
        histogram: histogram
            .into_iter()
            .map(|(values, seen)| LitmusParallelOutcome {
                in_x86_tso: case.reference_tso.contains(&values),
                values,
                rounds: seen.rounds,
                first_round: seen.first_round,
                first_placement: seen.first_placement,
            })
            .collect(),
        x86_tso: case.reference_tso.iter().cloned().collect(),
        sc: case.sc.iter().cloned().collect(),
        interesting: case.interesting.iter().cloned().collect(),
        forbidden,
        outside_sc,
        not_observed,
        late_barrier_rounds: driver.late_barrier_rounds.load(SEQ),
        retired: report.cpus.iter().map(|cpu| cpu.retired).collect(),
        hooked_retired: driver.hooked_retired.load(SEQ),
        interpreted_retired: driver.interpreted_retired.load(SEQ),
        elapsed_ns,
        verdict,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::litmus_interleaving::corpus_dir;

    fn fixture(file: &str) -> LitmusCase {
        LitmusCase::from_fixture_file(&corpus_dir().join(file)).expect("fixture")
    }

    #[test]
    fn placements_are_all_the_injective_maps() {
        assert_eq!(placements(2, 2).len(), 2);
        assert_eq!(placements(2, 4).len(), 12);
        assert_eq!(placements(4, 4).len(), 24);
        assert!(
            placements(2, 4)
                .iter()
                .all(|p| p.iter().flatten().count() == 2)
        );
    }

    #[test]
    fn every_two_thread_fixture_runs_on_two_and_four_vcpus_and_stays_in_the_x86_tso_set() {
        for file in [
            "mp.json",
            "sb.json",
            "2+2w.json",
            "lock-xadd.json",
            "sb-mfence.json",
        ] {
            for vcpus in [2, 4] {
                let report = run_litmus_parallel(&fixture(file), vcpus, 3_000).expect(file);
                assert_eq!(
                    report.verdict,
                    LitmusParallelVerdict::Allowed,
                    "{file}: {report:?}"
                );
                assert_eq!(report.late_barrier_rounds, 0, "{file}: {report:?}");
                assert_eq!(
                    report.histogram.iter().map(|o| o.rounds).sum::<u64>(),
                    3_000
                );
            }
        }
    }

    #[test]
    fn a_four_thread_case_does_not_fit_two_vcpus_and_runs_on_four() {
        let iriw = fixture("iriw.json");
        assert!(matches!(
            run_litmus_parallel(&iriw, 2, 10),
            Err(LitmusError::Policy(_))
        ));
        let report = run_litmus_parallel(&iriw, 4, 3_000).unwrap();
        assert_eq!(report.verdict, LitmusParallelVerdict::Allowed, "{report:?}");
        assert_eq!(report.placements, 24);
    }
}
