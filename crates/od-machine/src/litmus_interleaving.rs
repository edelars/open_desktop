//! S6-19: the S6-05 litmus corpus under the interleaving scheduler (S6-15), test-only.
//!
//! One host thread runs one vCPU per litmus thread through [`SmpScheduler`] on the real
//! `od_isa::execute::run` and a flat identity RAM. The vCPUs are created directly in long mode with
//! a flat map (SMP_CONTRACT 4.4, allowed for litmus only): this is **not** an AP start. A step is
//! one whole guest instruction (or one return of the interpreter from a partly executed `REP`), so
//! the memory model under test is "SC with every instruction indivisible", the set
//! `interleaving_instruction_atomic` of the fixture. It is neither x86-TSO nor a parallel run.
//!
//! Two ways to explore, both through the scheduler:
//! - **exhaustive**: every order-preserving schedule of all steps (not only the memory-touching
//!   ones) as an explicit [`QuantumPolicy::Schedule`], when the number of schedules is at most the
//!   threshold of `SMP.md` (1,000,000). The observed set must EQUAL the fixture set;
//! - **seed**: seeds `0..=9999` otherwise (and as a cross-check on the finite tests). A seed gives a
//!   RANDOM ORDER OF STEPS ([`random_schedule`]: at every step a thread is drawn from the threads
//!   that still have steps, by SplitMix64(seed)), run as an explicit [`QuantumPolicy::Schedule`].
//!   [`QuantumPolicy::Seeded`] is not used: it visits the vCPUs round-robin from cpu0 and so cannot
//!   reach an outcome in which a later thread goes before cpu0 (SB `[1,0]`, most of IRIW).
//!   The observed set must be a SUBSET of the fixture set.
//!
//! The expected sets are read from the fixture files (derived by `tools/build/build_stage6_litmus.py`
//! from the cited sources), never from a run of this machine. An outcome outside the set is
//! reported as forbidden; an expected outcome that no schedule reached is reported as "not covered
//! by the enumeration", not as an error of the machine.
//!
//! Not covered: parallel host threads (S6-36), x86-TSO reordering (the machine has none in this
//! mode), the `LOOP` instruction (the interpreter does not run it: the formula counters are the
//! corpus body repeated K times, see [`Case::from_formula`]).

use crate::clock_owner::ClockOwner;
use crate::idle_arbiter::SchedActivity;
use crate::interleaved_harness::HZ;
use crate::smp_scheduler::{
    BoundaryReport, Executed, Finished, QuantumPolicy, SmpError, SmpHost, SmpScheduler, SplitMix64,
    StopRequest,
};
use crate::{LocalApic, LocalApicHandle, VirtualClockHandle};
use od_isa::execute::{RunConfig, RunExit, run};
use od_isa::memory::{AccessContext, AccessKind, GuestMemory, MemoryError, PrivilegeLevel};
use od_isa::{CpuActivity, LongModeState, OperandSize, Register};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Bytes of the flat identity RAM (code at `0x2000 + 0x1000 * cpu`, cells below `0x2000`, and the
/// buffer of the REP tests at `0x8000`).
const RAM_SIZE: usize = 0x1_0000;
/// The threshold of complete schedules up to which the exploration is exhaustive (`SMP.md` §5).
pub(crate) const EXHAUSTIVE_THRESHOLD: u128 = 1_000_000;
/// The seeds of the seed mode, both ends included (`SMP.md` §5).
pub(crate) const SEEDS: std::ops::RangeInclusive<u64> = 0..=9_999;
/// Shared budget of one run (retired instructions of all vCPUs): far above any litmus.
const RUN_BUDGET: u64 = 100_000;
/// The REP slice, `REP_ITERATION_WORK_LIMIT`, SMP_CONTRACT 8.
pub(crate) const REP_SLICE: u64 = od_isa::execute::REP_ITERATION_WORK_LIMIT;

pub(crate) type Outcome = Vec<u64>;

/// One instruction as the fixture records it.
#[derive(Clone, Debug)]
pub(crate) struct Instruction {
    pub(crate) asm: String,
    pub(crate) bytes: Vec<u8>,
    /// Steps of the scheduler this instruction takes: 1, or `ceil(count / REP_SLICE)` for a
    /// `REP` of a known count (the REP tests only).
    pub(crate) steps: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct Thread {
    pub(crate) code_start: u64,
    pub(crate) registers: Vec<(Register, u64)>,
    pub(crate) instructions: Vec<Instruction>,
}

impl Thread {
    fn steps(&self) -> usize {
        self.instructions.iter().map(|i| i.steps).sum()
    }
}

/// A place whose final value is part of the outcome key.
#[derive(Clone, Debug)]
pub(crate) enum Field {
    Register { cpu: usize, register: Register },
    Cell { address: u64 },
}

/// What the harness expects: a set, or (for the formula counters) the set the formula gives for the
/// chosen N and K.
#[derive(Clone, Debug)]
pub(crate) struct Case {
    pub(crate) id: String,
    pub(crate) cells: Vec<(String, u64, u64)>,
    pub(crate) threads: Vec<Thread>,
    pub(crate) observe: Vec<Field>,
    pub(crate) observe_names: Vec<String>,
    /// The `interleaving_instruction_atomic` set.
    pub(crate) expected: BTreeSet<Outcome>,
    /// Sets of the other two models, for the record only (fixtures; empty for formula cases).
    pub(crate) sc: BTreeSet<Outcome>,
    pub(crate) x86_tso: BTreeSet<Outcome>,
    pub(crate) interesting: BTreeSet<Outcome>,
}

pub(crate) fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/guest/stage6/litmus")
}

fn parse_hex(text: &str) -> u64 {
    u64::from_str_radix(text.trim_start_matches("0x"), 16).expect("hex number")
}

fn parse_bytes(text: &str) -> Vec<u8> {
    text.split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).expect("hex byte"))
        .collect()
}

fn register(name: &str) -> Register {
    match name {
        "eax" | "rax" => Register::Rax,
        "ecx" | "rcx" => Register::Rcx,
        "edx" | "rdx" => Register::Rdx,
        "ebx" | "rbx" => Register::Rbx,
        other => panic!("register {other} is not in the corpus"),
    }
}

fn outcome_set(values: &Value) -> BTreeSet<Outcome> {
    values
        .as_array()
        .expect("outcome array")
        .iter()
        .map(|row| {
            row.as_array()
                .expect("outcome row")
                .iter()
                .map(|v| v.as_u64().expect("outcome value"))
                .collect()
        })
        .collect()
}

fn observe_fields(names: &[String], cells: &[(String, u64, u64)]) -> Vec<Field> {
    names
        .iter()
        .map(|name| {
            let (owner, what) = name.split_once('.').expect("owner.name");
            if owner == "mem" {
                let (_, address, _) = cells
                    .iter()
                    .find(|(cell, _, _)| cell == what)
                    .expect("observed cell exists");
                Field::Cell { address: *address }
            } else {
                Field::Register {
                    cpu: owner.strip_prefix("cpu").unwrap().parse().unwrap(),
                    register: register(what),
                }
            }
        })
        .collect()
}

fn instruction(value: &Value) -> Instruction {
    Instruction {
        asm: value["asm"].as_str().expect("asm").to_string(),
        bytes: parse_bytes(value["bytes"].as_str().expect("bytes")),
        steps: 1,
    }
}

impl Case {
    /// A finite fixture (`2+2w.json`, `sb.json`, ...).
    pub(crate) fn from_fixture(file: &str) -> Self {
        let text = std::fs::read_to_string(corpus_dir().join(file)).expect("fixture file");
        let doc: Value = serde_json::from_str(&text).expect("fixture json");
        assert_eq!(doc["schema"], "od.smp-litmus.v1");
        let cells: Vec<(String, u64, u64)> = doc["memory"]["cells"]
            .as_array()
            .expect("cells")
            .iter()
            .map(|c| {
                assert_eq!(
                    c["size_bits"], 32,
                    "the driver loads and stores dword cells"
                );
                (
                    c["name"].as_str().unwrap().to_string(),
                    parse_hex(c["address"].as_str().unwrap()),
                    c["initial"].as_u64().unwrap(),
                )
            })
            .collect();
        let regions = doc["memory"]["code_regions"].as_array().expect("regions");
        let threads: Vec<Thread> = doc["threads"]
            .as_array()
            .expect("threads")
            .iter()
            .enumerate()
            .map(|(index, thread)| {
                assert_eq!(thread["cpu"].as_u64().unwrap() as usize, index);
                let region = &regions[index];
                assert_eq!(region["cpu"].as_u64().unwrap() as usize, index);
                let instructions: Vec<Instruction> = thread["instructions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(instruction)
                    .collect();
                let total: usize = instructions.iter().map(|i| i.bytes.len()).sum();
                assert_eq!(total as u64, region["size_bytes"].as_u64().unwrap());
                Thread {
                    code_start: parse_hex(region["start"].as_str().unwrap()),
                    registers: thread["initial_registers"]
                        .as_object()
                        .unwrap()
                        .iter()
                        .map(|(name, v)| (register(name), v.as_u64().unwrap()))
                        .collect(),
                    instructions,
                }
            })
            .collect();
        let observe_names: Vec<String> = doc["observe"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let observe = observe_fields(&observe_names, &cells);
        let outcomes = &doc["outcomes"];
        Self {
            id: doc["id"].as_str().unwrap().to_string(),
            cells,
            threads,
            observe,
            observe_names,
            expected: outcome_set(&outcomes["interleaving_instruction_atomic"]["values"]),
            sc: outcome_set(&outcomes["sc"]["values"]),
            x86_tso: outcome_set(&outcomes["x86_tso"]["values"]),
            interesting: outcome_set(&doc["interesting"]),
        }
    }

    /// The names of the finite fixtures of the corpus: every `*.json` except the formula group.
    pub(crate) fn fixture_files() -> Vec<String> {
        let mut files: Vec<String> = std::fs::read_dir(corpus_dir())
            .expect("corpus dir")
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.ends_with(".json") && name != "counters-formula.json")
            .collect();
        files.sort();
        files
    }

    /// One counter of `counters-formula.json` (`LOCK-INC`, `UNLOCKED-INC`, `THREE-INSN-COUNTER`)
    /// for `n` threads and `k` rounds. The interpreter does not run `LOOP`, so a thread is the
    /// corpus body (every instruction but the `loop`) written `k` times; the memory-touching
    /// instruction sequence of a thread is the same as the loop's. The expected set is the formula
    /// of the fixture for `interleaving_instruction_atomic`.
    pub(crate) fn from_formula(id: &str, n: u64, k: u64) -> Self {
        let text = std::fs::read_to_string(corpus_dir().join("counters-formula.json")).unwrap();
        let doc: Value = serde_json::from_str(&text).unwrap();
        let case = doc["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .expect("formula case");
        let template = &case["thread_template"];
        let body: Vec<Instruction> = template["instructions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["op"] != "loop")
            .map(instruction)
            .collect();
        let cell = &case["initial_memory"]["cells"][0];
        assert_eq!(
            cell["size_bits"], 32,
            "the driver loads and stores dword cells"
        );
        let cells = vec![(
            cell["name"].as_str().unwrap().to_string(),
            parse_hex(cell["address"].as_str().unwrap()),
            cell["initial"].as_u64().unwrap(),
        )];
        let threads = (0..n)
            .map(|i| Thread {
                code_start: 0x2000 + 0x1000 * i,
                registers: Vec::new(),
                instructions: (0..k).flat_map(|_| body.clone()).collect(),
            })
            .collect();
        let observe_names = vec!["mem.x".to_string()];
        let observe = observe_fields(&observe_names, &cells);
        let oracle = &case["formula_oracle"]["interleaving_instruction_atomic"];
        let evaluate = |expression: &str| -> u64 {
            match expression {
                "N*K" => n * k,
                "1 if K == 1 else 2" => {
                    if k == 1 {
                        1
                    } else {
                        2
                    }
                }
                other => panic!("formula {other} is not known to the driver"),
            }
        };
        let expected: BTreeSet<Outcome> = match oracle["kind"].as_str().unwrap() {
            "exact_singleton" => {
                BTreeSet::from([vec![evaluate(oracle["value"].as_str().unwrap())]])
            }
            "exact_integer_range" => {
                assert_eq!(oracle["step"], 1);
                let (min, max) = (
                    evaluate(oracle["minimum"].as_str().unwrap()),
                    evaluate(oracle["maximum"].as_str().unwrap()),
                );
                (min..=max).map(|v| vec![v]).collect()
            }
            other => panic!("formula kind {other}"),
        };
        Self {
            id: format!("{id}(N={n},K={k})"),
            cells,
            threads,
            observe,
            observe_names,
            expected,
            sc: BTreeSet::new(),
            x86_tso: BTreeSet::new(),
            interesting: BTreeSet::new(),
        }
    }

    pub(crate) fn steps_per_thread(&self) -> Vec<usize> {
        self.threads.iter().map(Thread::steps).collect()
    }

    /// `(Σ n_i)! / Π n_i!`, `None` when it does not fit in 128 bits.
    pub(crate) fn schedule_count(&self) -> Option<u128> {
        multinomial(&self.steps_per_thread())
    }
}

/// `(Σ n_i)! / Π n_i!` as a product of binomials; `None` on overflow of `u128`.
pub(crate) fn multinomial(parts: &[usize]) -> Option<u128> {
    let mut total = 0u128;
    let mut result = 1u128;
    for &part in parts {
        for i in 1..=part as u128 {
            total += 1;
            // result * total / i is exact at every step: it is a binomial coefficient chain.
            result = result.checked_mul(total)? / i;
        }
    }
    Some(result)
}

/// Calls `visit` with every order-preserving interleaving of `lengths` (the thread index of each
/// step), in lexicographic order of the thread indexes.
pub(crate) fn for_each_schedule(lengths: &[usize], visit: &mut impl FnMut(&[usize])) {
    fn walk(left: &mut [usize], prefix: &mut Vec<usize>, visit: &mut impl FnMut(&[usize])) {
        if left.iter().all(|&n| n == 0) {
            visit(prefix);
            return;
        }
        for cpu in 0..left.len() {
            if left[cpu] > 0 {
                left[cpu] -= 1;
                prefix.push(cpu);
                walk(left, prefix, visit);
                prefix.pop();
                left[cpu] += 1;
            }
        }
    }
    walk(&mut lengths.to_vec(), &mut Vec::new(), visit);
}

/// Flat identity RAM. `LOCK` forms are one backend call each, as the trait demands.
struct LitmusRam(Vec<u8>);

impl LitmusRam {
    fn range(&self, address: u64, len: usize, access: AccessKind) -> Result<usize, MemoryError> {
        let at = address as usize;
        if at.checked_add(len).is_some_and(|end| end <= self.0.len()) {
            Ok(at)
        } else {
            Err(MemoryError::Unmapped { address, access })
        }
    }

    fn load(&self, address: u64, size: OperandSize) -> Result<u64, MemoryError> {
        let at = self.range(address, size.bytes(), AccessKind::Read)?;
        let mut raw = [0u8; 8];
        raw[..size.bytes()].copy_from_slice(&self.0[at..at + size.bytes()]);
        Ok(u64::from_le_bytes(raw))
    }

    fn store(&mut self, address: u64, size: OperandSize, value: u64) -> Result<(), MemoryError> {
        let at = self.range(address, size.bytes(), AccessKind::Write)?;
        self.0[at..at + size.bytes()].copy_from_slice(&value.to_le_bytes()[..size.bytes()]);
        Ok(())
    }

    fn cell(&self, address: u64) -> u64 {
        self.load(address, OperandSize::Dword).unwrap()
    }
}

impl GuestMemory for LitmusRam {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        let at = self.range(address, 1, AccessKind::Fetch)?;
        Ok(self.0[at])
    }

    fn read(&mut self, address: u64, _: AccessContext, out: &mut [u8]) -> Result<(), MemoryError> {
        let at = self.range(address, out.len(), AccessKind::Read)?;
        out.copy_from_slice(&self.0[at..at + out.len()]);
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, source: &[u8]) -> Result<(), MemoryError> {
        let at = self.range(address, source.len(), AccessKind::Write)?;
        self.0[at..at + source.len()].copy_from_slice(source);
        Ok(())
    }

    fn atomic_exchange_integer(
        &mut self,
        address: u64,
        _: AccessContext,
        size: OperandSize,
        replacement: u64,
    ) -> Result<u64, MemoryError> {
        let old = self.load(address, size)?;
        self.store(address, size, replacement & size.mask())?;
        Ok(old)
    }

    fn atomic_fetch_add_integer(
        &mut self,
        address: u64,
        _: AccessContext,
        size: OperandSize,
        addend: u64,
    ) -> Result<u64, MemoryError> {
        let old = self.load(address, size)?;
        self.store(address, size, old.wrapping_add(addend) & size.mask())?;
        Ok(old)
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        address: u64,
        _: AccessContext,
        size: OperandSize,
        expected: u64,
        replacement: u64,
    ) -> Result<u64, MemoryError> {
        let old = self.load(address, size)?;
        if old == expected & size.mask() {
            self.store(address, size, replacement & size.mask())?;
        }
        Ok(old)
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        // Nothing is buffered in the interleaving mode: every instruction is complete when the
        // next step starts.
        Ok(())
    }
}

struct CpuSlot {
    state: LongModeState,
    /// Address just past the last instruction: the thread has ended when RIP is here.
    end_rip: u64,
    /// RIP of every instruction start (and `end_rip`): the only places a step may end on.
    boundaries: BTreeSet<u64>,
}

/// What one run saw besides the outcome: proof that no instruction was cut.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct RunStats {
    /// Calls of the interpreter that ended on a RIP which is neither an instruction start nor the
    /// restart point of the same instruction (a `REP` slice). Must stay 0.
    pub(crate) cut_instructions: u64,
    /// Calls that ended on a `REP` slice (RIP unchanged, partial progress).
    pub(crate) rep_slices: u64,
    /// Calls that retired an instruction.
    pub(crate) retired_calls: u64,
    pub(crate) retired: u64,
}

struct LitmusMachine {
    ram: LitmusRam,
    cpus: Vec<CpuSlot>,
    device: LocalApic,
    owner: ClockOwner,
    stats: RunStats,
    /// `(address, asm)` of every instruction, to name the one the interpreter refuses.
    names: Vec<Vec<(u64, String)>>,
}

impl LitmusMachine {
    fn new(case: &Case) -> Self {
        let mut ram = LitmusRam(vec![0; RAM_SIZE]);
        for (_, address, initial) in &case.cells {
            ram.store(*address, OperandSize::Dword, *initial).unwrap();
        }
        let mut cpus = Vec::new();
        let mut names = Vec::new();
        for thread in &case.threads {
            let mut at = thread.code_start;
            let mut boundaries = BTreeSet::new();
            let mut thread_names = Vec::new();
            for instruction in &thread.instructions {
                boundaries.insert(at);
                thread_names.push((at, instruction.asm.clone()));
                ram.0[at as usize..at as usize + instruction.bytes.len()]
                    .copy_from_slice(&instruction.bytes);
                at += instruction.bytes.len() as u64;
            }
            boundaries.insert(at);
            names.push(thread_names);
            let mut gpr = [0u64; 16];
            for (register, value) in &thread.registers {
                gpr[*register as usize] = *value;
            }
            cpus.push(CpuSlot {
                state: LongModeState {
                    gpr,
                    rip: thread.code_start,
                    // Reserved bit 1 only: DF = 0 (the REP tests store upwards), IF = 0 (no
                    // interrupts exist in a litmus).
                    rflags: 0x2,
                    sti_shadow: Some(od_isa::StiShadow::Clear),
                    activity: Some(CpuActivity::Running),
                    ..LongModeState::default()
                },
                end_rip: at,
                boundaries,
            });
        }
        let clock = VirtualClockHandle::new(HZ).unwrap();
        let device = LocalApic::with_shared_timers(clock.clone(), HZ, cpus.len()).unwrap();
        let apics = (0..cpus.len())
            .map(|cpu| device.handle_for(cpu).unwrap())
            .collect();
        let owner = ClockOwner::new(&clock, apics).unwrap();
        Self {
            ram,
            cpus,
            device,
            owner,
            stats: RunStats::default(),
            names,
        }
    }

    fn outcome(&self, case: &Case) -> Outcome {
        case.observe
            .iter()
            .map(|field| match field {
                Field::Register { cpu, register } => {
                    self.cpus[*cpu].state.gpr[*register as usize] & 0xFFFF_FFFF
                }
                Field::Cell { address } => u64::from(self.ram.cell(*address) as u32),
            })
            .collect()
    }
}

/// The interpreter does not run a form of the corpus (a gap of the interpreter, not a result).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Unsupported {
    pub(crate) cpu: usize,
    pub(crate) rip: u64,
    pub(crate) asm: String,
    pub(crate) opcode: u8,
}

impl SmpHost for LitmusMachine {
    type Error = Unsupported;

    fn vcpu_count(&self) -> usize {
        self.cpus.len()
    }

    fn activity(&self, cpu: usize) -> SchedActivity {
        SchedActivity::from(self.cpus[cpu].state.activity.unwrap())
    }

    fn if_set(&self, _cpu: usize) -> bool {
        false
    }

    fn apic(&self, cpu: usize) -> LocalApicHandle {
        self.device.handle_for(cpu).unwrap()
    }

    fn clock_owner(&mut self) -> &mut ClockOwner {
        &mut self.owner
    }

    fn boundary(&mut self, _cpu: usize) -> Result<BoundaryReport, Unsupported> {
        Ok(BoundaryReport::default())
    }

    fn wake(&mut self, _cpu: usize) -> Result<(), Unsupported> {
        unreachable!("IF is clear: no litmus vCPU is woken")
    }

    fn run(
        &mut self,
        cpu: usize,
        budget: u64,
        stop: &mut dyn FnMut() -> bool,
    ) -> Result<Executed, Unsupported> {
        assert!(budget >= 1);
        let slot = &mut self.cpus[cpu];
        if slot.state.rip == slot.end_rip {
            // The thread has ended: it stops running. Nothing is retired and nothing is a step.
            slot.state.activity = Some(CpuActivity::Halted);
            return Ok(Executed::default());
        }
        let before = slot.state.rip;
        let result = run(
            &mut slot.state,
            &mut self.ram,
            RunConfig {
                instruction_budget: budget,
                stop_rip: Some(slot.end_rip),
                context: AccessContext {
                    privilege: PrivilegeLevel::Ring0,
                    ..AccessContext::default()
                },
            },
            stop,
        );
        if let RunExit::Unsupported { opcode } = result.exit {
            let asm = self.names[cpu]
                .iter()
                .find(|(start, _)| *start == before)
                .map_or_else(String::new, |(_, asm)| asm.clone());
            return Err(Unsupported {
                cpu,
                rip: before,
                asm,
                opcode,
            });
        }
        let partial = result.exit == RunExit::RepIterationBudgetExhausted;
        assert!(
            matches!(
                result.exit,
                RunExit::StopAddress
                    | RunExit::InstructionBudgetExhausted
                    | RunExit::RepIterationBudgetExhausted
            ),
            "litmus on cpu {cpu} at {before:#x}: {:?}",
            result.exit
        );
        let after = slot.state.rip;
        // A step ends on an instruction boundary, or on the restart point of the same `REP`.
        let on_boundary = if partial {
            after == before
        } else {
            slot.boundaries.contains(&after)
        };
        self.stats.cut_instructions += u64::from(!on_boundary);
        self.stats.rep_slices += u64::from(partial);
        self.stats.retired_calls += u64::from(result.instructions_executed > 0);
        self.stats.retired += result.instructions_executed;
        Ok(Executed {
            retired: result.instructions_executed,
            partial_rep: partial,
            device_ns: 0,
        })
    }
}

/// What one run left behind.
#[derive(Clone, Debug)]
pub(crate) struct RunReport {
    pub(crate) outcome: Outcome,
    pub(crate) finished: Finished,
    pub(crate) stats: RunStats,
    /// Every thread reached the end of its code.
    pub(crate) all_ended: bool,
    pub(crate) steps: Vec<u64>,
}

/// One run of `case` under `policy`; `Err` when the interpreter refuses a form of the corpus.
pub(crate) fn try_run_case(case: &Case, policy: QuantumPolicy) -> Result<RunReport, Unsupported> {
    let mut machine = LitmusMachine::new(case);
    let mut scheduler = SmpScheduler::new(case.threads.len(), policy, StopRequest::new()).unwrap();
    let finished = match scheduler.run(&mut machine, RUN_BUDGET) {
        Ok(finished) => finished,
        Err(SmpError::Host(unsupported)) => return Err(unsupported),
        Err(other) => panic!("{}: scheduler error {other:?}", case.id),
    };
    let all_ended = machine.cpus.iter().all(|cpu| cpu.state.rip == cpu.end_rip);
    Ok(RunReport {
        outcome: machine.outcome(case),
        finished,
        stats: machine.stats.clone(),
        all_ended,
        steps: scheduler.counters().iter().map(|c| c.steps).collect(),
    })
}

/// [`try_run_case`] for a case the interpreter runs.
pub(crate) fn run_case(case: &Case, policy: QuantumPolicy) -> RunReport {
    try_run_case(case, policy).unwrap_or_else(|u| panic!("{}: unsupported {u:?}", case.id))
}

/// `Some` when the interpreter refuses a form of `case` (probed by one round-robin run of one
/// step per turn, which reaches every instruction of every thread).
pub(crate) fn unsupported_form(case: &Case) -> Option<Unsupported> {
    try_run_case(case, QuantumPolicy::Fixed(1)).err()
}

/// The first witness of an outcome: a schedule or a seed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Witness {
    Schedule(Vec<usize>),
    Seed(u64),
}

impl Witness {
    fn json(&self) -> Value {
        match self {
            Self::Schedule(list) => {
                json!({"schedule": list.iter().map(|c| c.to_string()).collect::<String>()})
            }
            Self::Seed(seed) => json!({"seed": seed}),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Seen {
    pub(crate) runs: u64,
    pub(crate) first: Witness,
}

/// The result of exploring one case in one mode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Exploration {
    pub(crate) runs: u64,
    pub(crate) outcomes: BTreeMap<Outcome, Seen>,
    pub(crate) cut_instructions: u64,
    pub(crate) rep_slices: u64,
    /// Runs in which a thread did not end, or the run did not end as the mode requires.
    pub(crate) incomplete_runs: u64,
}

impl Exploration {
    fn new() -> Self {
        Self {
            runs: 0,
            outcomes: BTreeMap::new(),
            cut_instructions: 0,
            rep_slices: 0,
            incomplete_runs: 0,
        }
    }

    fn add(&mut self, report: RunReport, witness: impl FnOnce() -> Witness, wanted: Finished) {
        self.runs += 1;
        self.cut_instructions += report.stats.cut_instructions;
        self.rep_slices += report.stats.rep_slices;
        self.incomplete_runs += u64::from(!report.all_ended || report.finished != wanted);
        self.outcomes
            .entry(report.outcome)
            .and_modify(|seen| seen.runs += 1)
            .or_insert_with(|| Seen {
                runs: 1,
                first: witness(),
            });
    }

    pub(crate) fn observed(&self) -> BTreeSet<Outcome> {
        self.outcomes.keys().cloned().collect()
    }
}

/// Every order-preserving schedule of all steps of `case`, as explicit schedules.
pub(crate) fn explore_exhaustive(case: &Case) -> Exploration {
    let mut exploration = Exploration::new();
    let lengths = case.steps_per_thread();
    for_each_schedule(&lengths, &mut |schedule| {
        let report = run_case(case, QuantumPolicy::Schedule(schedule.to_vec()));
        exploration.add(
            report,
            || Witness::Schedule(schedule.to_vec()),
            Finished::ScheduleEnd,
        );
    });
    exploration
}

/// The random order of steps of `seed`: at every step one thread is drawn (`SplitMix64(seed)`,
/// `next_u64() % open`) from the threads that have steps left, until every thread has run
/// `lengths[i]` steps. Deterministic in `seed`; every order-preserving schedule has a non-zero
/// probability (a later thread may go first), which is the point of not using the round-robin
/// `QuantumPolicy::Seeded`.
pub(crate) fn random_schedule(lengths: &[usize], seed: u64) -> Vec<usize> {
    let mut rng = SplitMix64::new(seed);
    let mut left = lengths.to_vec();
    let mut schedule = Vec::with_capacity(left.iter().sum());
    loop {
        let open: Vec<usize> = (0..left.len()).filter(|&cpu| left[cpu] > 0).collect();
        if open.is_empty() {
            return schedule;
        }
        let cpu = open[(rng.next_u64() % open.len() as u64) as usize];
        left[cpu] -= 1;
        schedule.push(cpu);
    }
}

/// One run per seed of `seeds`, each under its [`random_schedule`].
pub(crate) fn explore_seeds(case: &Case, seeds: std::ops::RangeInclusive<u64>) -> Exploration {
    let mut exploration = Exploration::new();
    let lengths = case.steps_per_thread();
    for seed in seeds {
        let report = run_case(
            case,
            QuantumPolicy::Schedule(random_schedule(&lengths, seed)),
        );
        exploration.add(report, || Witness::Seed(seed), Finished::ScheduleEnd);
    }
    exploration
}

/// The comparison with the fixture set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Verdict {
    /// Observed, not in the expected set: a defect, never adjusted.
    pub(crate) forbidden: BTreeSet<Outcome>,
    /// Expected, not reached by the enumeration: "not covered", not an error of the machine.
    pub(crate) not_covered: BTreeSet<Outcome>,
}

pub(crate) fn compare(case: &Case, exploration: &Exploration) -> Verdict {
    let observed = exploration.observed();
    Verdict {
        forbidden: observed.difference(&case.expected).cloned().collect(),
        not_covered: case.expected.difference(&observed).cloned().collect(),
    }
}

fn set_json(set: &BTreeSet<Outcome>) -> Value {
    Value::Array(set.iter().map(|o| json!(o)).collect())
}

/// The JSON record of one exploration (histogram with the first witness of every outcome).
pub(crate) fn exploration_json(
    case: &Case,
    mode: &str,
    exploration: &Exploration,
    schedule_count: Option<u128>,
) -> Value {
    let verdict = compare(case, exploration);
    let histogram: Vec<Value> = exploration
        .outcomes
        .iter()
        .map(|(outcome, seen)| {
            let mut entry = json!({"outcome": outcome, "runs": seen.runs});
            entry["first"] = seen.first.json();
            entry["in_expected_set"] = json!(case.expected.contains(outcome));
            entry
        })
        .collect();
    json!({
        "id": case.id,
        "mode": mode,
        "vcpus": case.threads.len(),
        "steps_per_thread": case.steps_per_thread(),
        "threads": case
            .threads
            .iter()
            .map(|t| t.instructions.iter().map(|i| i.asm.clone()).collect::<Vec<_>>())
            .collect::<Vec<_>>(),
        "complete_schedules": schedule_count.map(|n| n.to_string()),
        "runs": exploration.runs,
        "observe": case.observe_names,
        "histogram": histogram,
        "expected_interleaving_instruction_atomic": set_json(&case.expected),
        "forbidden_outcomes": set_json(&verdict.forbidden),
        "not_covered_by_enumeration": set_json(&verdict.not_covered),
        "instructions_cut": exploration.cut_instructions,
        "rep_slices": exploration.rep_slices,
        "incomplete_runs": exploration.incomplete_runs,
        "x86_tso_only_outcomes": set_json(&case.x86_tso.difference(&case.expected).cloned().collect()),
    })
}

/// Abstract model of the REP tests: an independent enumeration of "SC with a `REP` cut after L, 2L,
/// ... iterations" (SMP_CONTRACT 8, item 3) over a map of cells, used as the oracle of the REP
/// cases. It shares no code with the machine.
#[derive(Clone, Debug)]
pub(crate) enum Abstract {
    /// `REP STOSD` of `count` dwords of `value` from `base` (cell index of the first dword).
    RepStore { base: u64, count: u64, value: u64 },
    /// `mov reg, [cell]`: the value goes to the next result slot of the thread.
    Load { cell: u64 },
}

pub(crate) fn abstract_outcomes(threads: &[Vec<Abstract>], cut: u64) -> BTreeSet<Outcome> {
    // Expand every REP into the slices a step covers: cells [from, to).
    #[derive(Clone)]
    enum Step {
        Store { from: u64, to: u64, value: u64 },
        Load { cell: u64 },
    }
    let steps: Vec<Vec<Step>> = threads
        .iter()
        .map(|thread| {
            thread
                .iter()
                .flat_map(|op| match op {
                    Abstract::Load { cell } => vec![Step::Load { cell: *cell }],
                    Abstract::RepStore { base, count, value } => {
                        let mut slices = Vec::new();
                        let mut done = 0;
                        while done < *count {
                            let next = (done + cut).min(*count);
                            slices.push(Step::Store {
                                from: base + done,
                                to: base + next,
                                value: *value,
                            });
                            done = next;
                        }
                        slices
                    }
                })
                .collect()
        })
        .collect();
    let lengths: Vec<usize> = steps.iter().map(Vec::len).collect();
    let mut outcomes = BTreeSet::new();
    for_each_schedule(&lengths, &mut |schedule| {
        let mut position = vec![0usize; steps.len()];
        let mut memory: BTreeMap<u64, u64> = BTreeMap::new();
        let mut loads: Vec<Vec<u64>> = vec![Vec::new(); steps.len()];
        for &cpu in schedule {
            match &steps[cpu][position[cpu]] {
                Step::Store { from, to, value } => {
                    for cell in *from..*to {
                        memory.insert(cell, *value);
                    }
                }
                Step::Load { cell } => loads[cpu].push(memory.get(cell).copied().unwrap_or(0)),
            }
            position[cpu] += 1;
        }
        outcomes.insert(loads.into_iter().flatten().collect());
    });
    outcomes
}

const BUFFER: u64 = 0x8000;

fn load_dword(register_modrm: u8, address: u64) -> Instruction {
    let mut bytes = vec![0x8b, register_modrm, 0x25];
    bytes.extend_from_slice(&(address as u32).to_le_bytes());
    Instruction {
        asm: format!("mov r32, dword ptr [{address:#x}]"),
        bytes,
        steps: 1,
    }
}

/// `rep stosd` of `count` dwords of 1 from the buffer on cpu 0 and, on cpu 1, two loads (into
/// `eax` and `ebx`) of the dwords `first` and `second` of the buffer. The observed values
/// are the two registers.
fn rep_case(count: u64, first: u64, second: u64) -> Case {
    let rep = Instruction {
        asm: "rep stosd".into(),
        bytes: vec![0xf3, 0xab],
        steps: count.div_ceil(REP_SLICE).max(1) as usize,
    };
    let loads = vec![
        load_dword(0x04, BUFFER + 4 * first),
        load_dword(0x1c, BUFFER + 4 * second),
    ];
    Case {
        id: format!("REP-CUT(count={count},cells={first},{second})"),
        cells: Vec::new(),
        threads: vec![
            Thread {
                code_start: 0x2000,
                registers: vec![
                    (Register::Rax, 1),
                    (Register::Rcx, count),
                    (Register::Rdi, BUFFER),
                ],
                instructions: vec![rep],
            },
            Thread {
                code_start: 0x3000,
                registers: Vec::new(),
                instructions: loads,
            },
        ],
        observe: vec![
            Field::Register {
                cpu: 1,
                register: Register::Rax,
            },
            Field::Register {
                cpu: 1,
                register: Register::Rbx,
            },
        ],
        observe_names: vec!["cpu1.eax".into(), "cpu1.ebx".into()],
        expected: rep_expected(count, first, second, REP_SLICE),
        sc: BTreeSet::new(),
        x86_tso: BTreeSet::new(),
        interesting: BTreeSet::new(),
    }
}

fn rep_expected(count: u64, first: u64, second: u64, cut: u64) -> BTreeSet<Outcome> {
    abstract_outcomes(
        &[
            vec![Abstract::RepStore {
                base: 0,
                count,
                value: 1,
            }],
            vec![
                Abstract::Load { cell: first },
                Abstract::Load { cell: second },
            ],
        ],
        cut,
    )
}

fn assert_exhaustive_equal(case: &Case) -> Exploration {
    let count = case.schedule_count().expect("count fits");
    assert!(count <= EXHAUSTIVE_THRESHOLD, "{}: {count}", case.id);
    let exploration = explore_exhaustive(case);
    assert_eq!(u128::from(exploration.runs), count, "{}", case.id);
    assert_eq!(
        exploration.cut_instructions, 0,
        "{}: an instruction was cut",
        case.id
    );
    assert_eq!(exploration.incomplete_runs, 0, "{}", case.id);
    let verdict = compare(case, &exploration);
    assert!(
        verdict.forbidden.is_empty(),
        "{}: FORBIDDEN outcomes {:?} (observed {:?}, expected {:?})",
        case.id,
        verdict.forbidden,
        exploration.observed(),
        case.expected
    );
    assert!(
        verdict.not_covered.is_empty(),
        "{}: expected outcomes not covered {:?}",
        case.id,
        verdict.not_covered
    );
    exploration
}

#[test]
fn multinomial_matches_the_corpus_schedule_counts_and_hand_values() {
    assert_eq!(multinomial(&[3, 3]), Some(20));
    assert_eq!(multinomial(&[1, 1, 2, 2]), Some(180));
    assert_eq!(multinomial(&[12, 12]), Some(2_704_156));
    assert_eq!(multinomial(&[6, 6, 6]), Some(17_153_136));
    assert_eq!(multinomial(&[0, 0]), Some(1));
    let mut seen = 0u128;
    for_each_schedule(&[2, 1, 2], &mut |_| seen += 1);
    assert_eq!(Some(seen), multinomial(&[2, 1, 2]));
}

#[test]
fn every_schedule_keeps_the_order_of_each_thread_and_the_set_is_distinct() {
    let mut all = BTreeSet::new();
    for_each_schedule(&[2, 3], &mut |s| {
        assert_eq!(s.iter().filter(|&&c| c == 0).count(), 2);
        assert_eq!(s.iter().filter(|&&c| c == 1).count(), 3);
        assert!(all.insert(s.to_vec()), "a schedule is listed twice");
    });
    assert_eq!(all.len(), 10);
}

#[test]
fn sb_exhaustive_equals_the_instruction_atomic_set_and_both_zero_is_not_reached() {
    let case = Case::from_fixture("sb.json");
    assert_eq!(case.schedule_count(), Some(6));
    let exploration = assert_exhaustive_equal(&case);
    // The fixture: (0,0) is allowed by x86-TSO (Sewell et al., section 1) and is not SC.
    let both_zero = vec![0, 0, 1, 1];
    assert!(case.x86_tso.contains(&both_zero));
    assert!(!case.sc.contains(&both_zero));
    assert_eq!(case.interesting, BTreeSet::from([both_zero.clone()]));
    // Interleaving mode is SC with indivisible instructions: the TSO outcome never appears. This
    // is expected and is a difference from a physical x86, not a defect.
    assert!(!exploration.outcomes.contains_key(&both_zero));
    assert_eq!(exploration.runs, 6);
    assert_eq!(exploration.outcomes.len(), 3);
}

#[test]
fn mp_exhaustive_never_reaches_the_forbidden_outcome() {
    let case = Case::from_fixture("mp.json");
    assert_eq!(
        case.observe_names,
        ["cpu1.eax", "cpu1.ebx", "mem.x", "mem.y"]
    );
    let exploration = assert_exhaustive_equal(&case);
    // cpu1 reads y then x: seeing the flag y = 1 with a stale x = 0 is forbidden in x86-TSO
    // (Sewell et al.), so it is in no fixture set.
    let forbidden = vec![1, 0, 1, 1];
    assert!(!case.x86_tso.contains(&forbidden));
    assert!(!case.expected.contains(&forbidden));
    assert!(!exploration.outcomes.contains_key(&forbidden));
    assert_eq!(exploration.outcomes.len(), 3);
}

/// Fixtures the interpreter does not run yet, with the form it refuses. The bare interpreter
/// runs `LOCK XADD` and `XCHG` with memory only in their qword `[rax]` forms and does not run
/// `LOCK INC` or `LOCK CMPXCHG` at all (`lock_cmpxchg_stays_unsupported_in_bare_run_and_step`);
/// the corpus uses the dword forms with an absolute address. S6-20 moves the `LOCK` forms to the
/// atomic API and must admit these forms. The corpus bytes are not changed to fit the
/// interpreter. When S6-20 lands this list becomes empty and the test says so.
const BLOCKED_BY_S6_20: [(&str, &str); 7] = [
    ("lock-cmpxchg-mutex.json", "lock cmpxchg"),
    ("lock-xadd.json", "lock xadd"),
    ("sb-lock-local.json", "lock inc"),
    ("sb-lock-xadd.json", "lock xadd"),
    ("sb-lock.json", "lock inc"),
    ("sb-xchg.json", "xchg"),
    ("xchg-mutex.json", "xchg"),
];

#[test]
fn the_whole_finite_corpus_is_exhaustive_and_equals_its_set() {
    let files = Case::fixture_files();
    assert_eq!(files.len(), 15, "{files:?}");
    let mut blocked = Vec::new();
    let mut run = 0;
    for file in files {
        let case = Case::from_fixture(&file);
        let count = case.schedule_count().unwrap();
        assert!(
            count <= EXHAUSTIVE_THRESHOLD,
            "{}: {count} schedules",
            case.id
        );
        if let Some(unsupported) = unsupported_form(&case) {
            blocked.push((file, unsupported));
            continue;
        }
        assert_exhaustive_equal(&case);
        run += 1;
    }
    let blocked: Vec<(String, String)> =
        blocked.into_iter().map(|(file, u)| (file, u.asm)).collect();
    eprintln!("run {run}, blocked {blocked:?}");
    let pinned: Vec<(String, String)> = BLOCKED_BY_S6_20
        .iter()
        .map(|(file, asm)| ((*file).to_string(), (*asm).to_string()))
        .collect();
    assert_eq!(
        blocked.iter().map(|(f, _)| f).collect::<Vec<_>>(),
        pinned.iter().map(|(f, _)| f).collect::<Vec<_>>(),
        "the set of fixtures the interpreter refuses changed (S6-20?): update the list"
    );
    for ((file, asm), (_, pinned_asm)) in blocked.iter().zip(&pinned) {
        assert!(asm.starts_with(pinned_asm), "{file}: refused {asm}");
    }
    assert_eq!(run, 8);
}

#[test]
fn unlocked_inc_is_exact_here_unlike_on_a_physical_x86() {
    // The instruction is indivisible in this mode: the lost update of the SC and x86-TSO sets
    // does not exist. The difference from real hardware is recorded in the output.
    let case = Case::from_fixture("unlocked-inc.json");
    assert!(case.sc.contains(&vec![1]) && case.x86_tso.contains(&vec![1]));
    assert_eq!(case.expected, BTreeSet::from([vec![2]]));
    let exploration = assert_exhaustive_equal(&case);
    assert_eq!(exploration.observed(), BTreeSet::from([vec![2]]));
}

#[test]
fn counters_by_formula_are_exhaustive_for_every_n_and_k_below_the_threshold() {
    for (n, k) in [(2, 1), (2, 2), (2, 3), (3, 1), (4, 1)] {
        let unlocked = Case::from_formula("UNLOCKED-INC", n, k);
        assert_eq!(unlocked.expected, BTreeSet::from([vec![n * k]]));
        let exploration = assert_exhaustive_equal(&unlocked);
        assert_eq!(exploration.observed(), BTreeSet::from([vec![n * k]]));
    }
}

#[test]
fn three_instruction_counter_loses_updates_between_instructions() {
    // N = 2, K = 1: load, add, store on both: the schedule L0 L1 A0 A1 S0 S1 loses one update.
    let case = Case::from_formula("THREE-INSN-COUNTER", 2, 1);
    assert_eq!(case.expected, BTreeSet::from([vec![1], vec![2]]));
    let exploration = assert_exhaustive_equal(&case);
    assert_eq!(exploration.runs, 20);
    // Every witness replays to its outcome: the explicit schedule is the whole input.
    for (outcome, seen) in &exploration.outcomes {
        let Witness::Schedule(schedule) = &seen.first else {
            unreachable!("exhaustive witnesses are schedules")
        };
        let replay = run_case(&case, QuantumPolicy::Schedule(schedule.clone()));
        assert_eq!(&replay.outcome, outcome);
    }
    assert!(
        exploration.outcomes.contains_key(&vec![1]),
        "a lost update is reached"
    );
    // The same loss with a quantum of one step, round-robin (Fixed(1)): load0 load1 add0 add1
    // store0 store1.
    let one_step = run_case(&case, QuantumPolicy::Fixed(1));
    assert_eq!(one_step.outcome, vec![1]);
    // A quantum that holds a whole thread keeps the interleaving out: the count is exact.
    let long = run_case(&case, QuantumPolicy::Fixed(1024));
    assert_eq!(long.outcome, vec![2]);
    // K = 2: the minimum is 2 and the count 4 is reached too.
    let case = Case::from_formula("THREE-INSN-COUNTER", 2, 2);
    assert_eq!(case.expected, BTreeSet::from([vec![2], vec![3], vec![4]]));
    assert_exhaustive_equal(&case);
}

#[test]
fn a_schedule_never_cuts_an_instruction_and_a_run_is_reproducible() {
    let case = Case::from_fixture("sb-mfence.json");
    let lengths = case.steps_per_thread();
    assert_eq!(lengths, [3, 3]);
    let mut checked = 0;
    for_each_schedule(&lengths, &mut |schedule| {
        let report = run_case(&case, QuantumPolicy::Schedule(schedule.to_vec()));
        // One call, one instruction; every call ended on a boundary.
        assert_eq!(report.stats.cut_instructions, 0);
        assert_eq!(report.stats.retired, 6);
        assert_eq!(report.stats.retired_calls, 6);
        assert_eq!(report.steps, vec![3, 3]);
        assert!(report.all_ended);
        assert_eq!(report.finished, Finished::ScheduleEnd);
        checked += 1;
    });
    assert_eq!(checked, 20);
    let again = run_case(&case, QuantumPolicy::seeded(7));
    let same = run_case(&case, QuantumPolicy::seeded(7));
    assert_eq!(again.outcome, same.outcome);
    assert_eq!(again.stats, same.stats);
}

#[test]
fn the_same_seed_gives_the_same_exploration_and_every_outcome_is_expected() {
    for file in ["sb.json", "mp.json", "iriw.json"] {
        let case = Case::from_fixture(file);
        let first = explore_seeds(&case, 0..=199);
        let second = explore_seeds(&case, 0..=199);
        assert_eq!(
            first, second,
            "{}: the same seeds, the same result",
            case.id
        );
        assert_eq!(first.runs, 200);
        assert_eq!(first.cut_instructions, 0);
        assert_eq!(first.incomplete_runs, 0);
        let verdict = compare(&case, &first);
        assert!(
            verdict.forbidden.is_empty(),
            "{}: {:?}",
            case.id,
            verdict.forbidden
        );
    }
}

#[test]
fn a_big_counter_is_explored_by_seeds_as_a_subset_of_the_formula() {
    // N = 3, K = 2: 17,153,136 complete schedules, above the threshold: seed mode.
    let case = Case::from_formula("THREE-INSN-COUNTER", 3, 2);
    assert!(case.schedule_count().unwrap() > EXHAUSTIVE_THRESHOLD);
    let first = explore_seeds(&case, SEEDS);
    let again = explore_seeds(&case, 0..=99);
    assert_eq!(first.runs, 10_000);
    assert_eq!(first.cut_instructions, 0);
    assert_eq!(first.incomplete_runs, 0);
    let verdict = compare(&case, &first);
    assert!(verdict.forbidden.is_empty(), "{:?}", verdict.forbidden);
    for (outcome, seen) in &again.outcomes {
        // A seed below 100 reaches the same outcome in both runs (same seeds, same result).
        let Witness::Seed(seed) = seen.first else {
            unreachable!()
        };
        assert!(seed < 100);
        let replay = run_case(
            &case,
            QuantumPolicy::Schedule(random_schedule(&case.steps_per_thread(), seed)),
        );
        assert_eq!(&replay.outcome, outcome);
    }
    // Lost updates happen under random orders of steps. Which values of the formula (2..=6) the
    // seeds reach is recorded in the proof data, not asserted: a value not reached is "not covered".
    assert!(
        first.observed().iter().any(|o| o[0] < 6),
        "an update was lost"
    );
}

#[test]
fn the_lock_inc_counter_is_blocked_by_the_interpreter_not_replaced() {
    // "LOCK INC is exact under every schedule" cannot be shown yet: the bare interpreter does
    // not run `lock inc` (S6-20). The corpus body is not swapped for another instruction.
    // The expected set (N*K) is still read from the fixture, for the day S6-20 lands.
    let case = Case::from_formula("LOCK-INC", 4, 4);
    assert_eq!(case.expected, BTreeSet::from([vec![16]]));
    let unsupported = unsupported_form(&case).expect("S6-20 has landed: run this counter");
    assert!(unsupported.asm.starts_with("lock inc"), "{unsupported:?}");
    assert_eq!(unsupported.opcode, 0xff);
}

#[test]
fn rep_is_cut_after_1024_iterations_and_not_inside_a_slice() {
    // Cells 1023 and 1024 straddle the first cut: (1, 0) is reachable only because the REP
    // returns after 1024 iterations. It is a step boundary: the reader sees 1024 dwords stored.
    let straddle = rep_case(2500, 1023, 1024);
    assert_eq!(straddle.threads[0].steps(), 3);
    assert_eq!(
        straddle.expected,
        BTreeSet::from([vec![0, 0], vec![0, 1], vec![1, 0], vec![1, 1]])
    );
    let exploration = assert_exhaustive_equal(&straddle);
    assert_eq!(exploration.runs, 10);
    assert!(exploration.rep_slices > 0);

    // Cells 1022 and 1023 are in the same slice: the reader can never see (1, 0).
    let inside = rep_case(2500, 1022, 1023);
    assert_eq!(
        inside.expected,
        BTreeSet::from([vec![0, 0], vec![0, 1], vec![1, 1]])
    );
    assert_exhaustive_equal(&inside);
    // The oracle discriminates the cut (small numbers, to keep the abstract enumeration short):
    // cells (1, 2) straddle a cut of 2 and sit in different slices of a cut of 1, cells (2, 3)
    // share a slice of a cut of 2 and not of a cut of 1, and an indivisible REP (cut 4) has no
    // such pair.
    assert!(rep_expected(4, 1, 2, 2).contains(&vec![1, 0]));
    assert!(!rep_expected(4, 2, 3, 2).contains(&vec![1, 0]));
    assert!(rep_expected(4, 2, 3, 1).contains(&vec![1, 0]));
    assert!(!rep_expected(4, 1, 2, 4).contains(&vec![1, 0]));

    // The second cut is after 2048 iterations.
    let second_cut = rep_case(2500, 2047, 2048);
    assert!(second_cut.expected.contains(&vec![1, 0]));
    assert_exhaustive_equal(&second_cut);
    let before_second_cut = rep_case(2500, 1024, 2047);
    assert!(!before_second_cut.expected.contains(&vec![1, 0]));
    assert_exhaustive_equal(&before_second_cut);
}

#[test]
fn a_rep_with_count_up_to_the_slice_is_one_step_and_indivisible() {
    // count = 1024 (L): the REP ends on its L-th iteration: one step, no empty step after it.
    let at_slice = rep_case(REP_SLICE, 0, REP_SLICE - 1);
    assert_eq!(at_slice.threads[0].steps(), 1);
    assert_eq!(
        at_slice.expected,
        BTreeSet::from([vec![0, 0], vec![0, 1], vec![1, 1]])
    );
    let exploration = assert_exhaustive_equal(&at_slice);
    assert_eq!(exploration.rep_slices, 0);
    // count = 0: one step that does nothing, the buffer stays zero.
    let empty = rep_case(0, 0, 1);
    assert_eq!(empty.expected, BTreeSet::from([vec![0, 0]]));
    let report = run_case(&empty, QuantumPolicy::Schedule(vec![0, 1, 1]));
    assert_eq!(report.outcome, vec![0, 0]);
    // count = L + 1: two steps (1024 iterations, then the last one).
    let over = rep_case(REP_SLICE + 1, 1023, 1024);
    assert_eq!(over.threads[0].steps(), 2);
    assert!(over.expected.contains(&vec![1, 0]));
    assert_exhaustive_equal(&over);
    // count = 2L: two steps, and the last slice ends on the 2L-th iteration (no third step).
    let double = rep_case(2 * REP_SLICE, 1023, 1024);
    assert_eq!(double.threads[0].steps(), 2);
    assert_exhaustive_equal(&double);
}

#[test]
fn a_wrong_expected_set_is_reported_as_forbidden_and_a_missing_one_as_not_covered() {
    // Negative control of the comparison itself: SB with the TSO set as the expectation sees a
    // set that is smaller (the both-zero outcome is not covered); with the SC set minus one
    // outcome as the expectation, an observed outcome is forbidden.
    let mut case = Case::from_fixture("sb.json");
    let exploration = explore_exhaustive(&case);
    case.expected = case.x86_tso.clone();
    let verdict = compare(&case, &exploration);
    assert!(verdict.forbidden.is_empty());
    assert_eq!(verdict.not_covered, BTreeSet::from([vec![0, 0, 1, 1]]));
    case.expected = BTreeSet::from([vec![1, 1, 1, 1]]);
    let verdict = compare(&case, &exploration);
    assert_eq!(verdict.forbidden.len(), 2);
}

#[test]
fn random_schedule_is_deterministic_keeps_thread_order_and_lets_a_later_thread_go_first() {
    let lengths = [3, 2, 4];
    for seed in 0..200 {
        let schedule = random_schedule(&lengths, seed);
        assert_eq!(schedule, random_schedule(&lengths, seed));
        for (cpu, &n) in lengths.iter().enumerate() {
            assert_eq!(schedule.iter().filter(|&&c| c == cpu).count(), n);
        }
    }
    let firsts: BTreeSet<usize> = (0..200)
        .map(|seed| random_schedule(&lengths, seed)[0])
        .collect();
    assert_eq!(firsts, BTreeSet::from([0, 1, 2]), "any thread can start");
}

#[test]
fn seeds_reach_every_expected_outcome_of_every_runnable_finite_fixture() {
    // The seed mode (0..=9999 random orders of steps) is not weaker than the exhaustive one on
    // the corpus the interpreter runs today: SB reaches [1, 0] (cpu1 before cpu0), IRIW reaches
    // both readers' orders. A forbidden outcome fails; an expected one not reached would be a
    // recorded coverage gap (none exists today).
    let mut checked = 0;
    for file in Case::fixture_files() {
        let case = Case::from_fixture(&file);
        if unsupported_form(&case).is_some() {
            continue;
        }
        let exploration = explore_seeds(&case, SEEDS);
        let verdict = compare(&case, &exploration);
        assert!(verdict.forbidden.is_empty(), "{}: {:?}", case.id, verdict);
        assert_eq!(exploration.incomplete_runs, 0, "{}", case.id);
        assert_eq!(exploration.cut_instructions, 0, "{}", case.id);
        assert!(
            verdict.not_covered.is_empty(),
            "{}: seeds do not cover {:?}",
            case.id,
            verdict.not_covered
        );
        checked += 1;
    }
    assert_eq!(checked, 8);
    let sb = explore_seeds(&Case::from_fixture("sb.json"), SEEDS);
    assert!(
        sb.outcomes.contains_key(&vec![1, 0, 1, 1]),
        "cpu1 before cpu0"
    );
    assert!(sb.outcomes.contains_key(&vec![0, 1, 1, 1]));
    let iriw = explore_seeds(&Case::from_fixture("iriw.json"), SEEDS);
    assert_eq!(iriw.outcomes.len(), 15);
}

/// The proof run: the whole corpus, exhaustive where the count allows and seeds 0..=9999
/// everywhere, written as JSON for `docs/reports/data/stage6/`. Run in release:
/// `OD_S6_LITMUS_OUT=$PWD/docs/reports/data/stage6/s6-19-litmus-interleaving.json cargo test
/// --release -p od-machine --offline --lib -- --ignored litmus_interleaving_proof --nocapture`.
#[test]
#[ignore = "proof run, writes the S6-19 data file"]
fn litmus_interleaving_proof() {
    let Ok(out) = std::env::var("OD_S6_LITMUS_OUT") else {
        panic!("set OD_S6_LITMUS_OUT to the output file");
    };
    let mut cases: Vec<Case> = Case::fixture_files()
        .iter()
        .map(|file| Case::from_fixture(file))
        .collect();
    for id in ["LOCK-INC", "UNLOCKED-INC", "THREE-INSN-COUNTER"] {
        for (n, k) in [(2, 1), (2, 2), (2, 3), (3, 1), (4, 1), (3, 2), (4, 4)] {
            cases.push(Case::from_formula(id, n, k));
        }
    }
    let started = std::time::Instant::now();
    let mut records = Vec::new();
    let mut forbidden_total = 0usize;
    let mut not_covered_total = 0usize;
    let mut blocked_total = 0usize;
    for case in &cases {
        let count = case.schedule_count();
        if let Some(unsupported) = unsupported_form(case) {
            blocked_total += 1;
            records.push(json!({
                "id": case.id,
                "mode": "not run: the interpreter does not execute this form (S6-20)",
                "refused_instruction": unsupported.asm,
                "cpu": unsupported.cpu,
                "rip": unsupported.rip,
                "opcode": unsupported.opcode,
                "expected_interleaving_instruction_atomic": set_json(&case.expected),
            }));
            continue;
        }
        let exhaustive = count.is_some_and(|n| n <= EXHAUSTIVE_THRESHOLD);
        if exhaustive {
            let exploration = explore_exhaustive(case);
            let verdict = compare(case, &exploration);
            forbidden_total += verdict.forbidden.len();
            not_covered_total += verdict.not_covered.len();
            assert_eq!(exploration.cut_instructions, 0);
            records.push(exploration_json(case, "exhaustive", &exploration, count));
        }
        let seeds = explore_seeds(case, SEEDS);
        let verdict = compare(case, &seeds);
        forbidden_total += verdict.forbidden.len();
        assert_eq!(seeds.cut_instructions, 0);
        let mode = if exhaustive {
            "seeds (cross-check)"
        } else {
            "seeds"
        };
        records.push(exploration_json(case, mode, &seeds, count));
    }
    let document = json!({
        "schema": "od.s6-19-litmus-interleaving.v1",
        "harness": "one vCPU per litmus thread, created directly in long mode on a flat identity RAM (SMP_CONTRACT 4.4, litmus only): not an AP start",
        "mode": "interleaving: one host thread, SmpScheduler, a step is one whole instruction or one REP slice of 1024 iterations",
        "memory_model_proved": "SC with every instruction indivisible (interleaving_instruction_atomic); not x86-TSO, not a parallel run",
        "difference_from_x86": "INC [x] without LOCK is indivisible here (x86 can lose the update); SB both-zero is not reachable (it is x86-TSO only); LOOP is not run by the interpreter, the formula counters repeat the corpus body K times",
        "expected_sets": "fixtures tests/fixtures/guest/stage6/litmus (S6-05, derived from the cited sources)",
        "exhaustive_threshold": EXHAUSTIVE_THRESHOLD.to_string(),
        "seeds": [SEEDS.start(), SEEDS.end()],
        "seed_policy": "random order of steps: per step a thread drawn by SplitMix64(seed) among the threads with steps left, run as QuantumPolicy::Schedule",
        "rep_slice": REP_SLICE,
        "forbidden_outcomes_total": forbidden_total,
        "not_covered_by_exhaustive_total": not_covered_total,
        "not_run_blocked_by_interpreter": blocked_total,
        "elapsed_seconds": started.elapsed().as_secs_f64(),
        "cases": records,
    });
    std::fs::write(
        &out,
        serde_json::to_string_pretty(&document).unwrap() + "\n",
    )
    .unwrap();
    assert_eq!(forbidden_total, 0, "forbidden outcomes: see {out}");
    assert_eq!(not_covered_total, 0, "outcomes not covered: see {out}");
}
