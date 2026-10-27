#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::AtomicU8;

use od_host::{MemoryOperationExecutionResultV2, PublishedCode};
use od_isa::memory::{AccessContext, AccessKind, ArchitecturalFault, GuestMemory, MemoryError};
use od_isa::{LongModeState, OperandSize};

const ENTRY_RIP: u64 = 0x9e00;
const RAM_LEN: usize = 128;
const CASES_PER_SEED: usize = 400;
const SEEDS: [u64; 3] = [
    0x5343_3441_0000_0001,
    0x5343_3441_0000_0002,
    0x5343_3441_0000_0003,
];
static JIT_TEST_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Sequence {
    StoreThenLoad,
    LoadThenStore,
}

impl Sequence {
    fn bytes(self) -> [u8; 6] {
        match self {
            Self::StoreThenLoad => [0x48, 0x89, 0x03, 0x48, 0x8b, 0x0b],
            Self::LoadThenStore => [0x48, 0x8b, 0x03, 0x48, 0x89, 0x02],
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::StoreThenLoad => "store_then_load",
            Self::LoadThenStore => "load_then_store",
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CorpusInput {
    sequence: Sequence,
    gpr: [u64; 16],
    flags: u64,
    first_address: u64,
    second_address: u64,
    ram: [u8; RAM_LEN],
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Observation {
    state: LongModeState,
    ram: [u8; RAM_LEN],
    callbacks: Vec<&'static str>,
    status: String,
    remaining_budget: u64,
    retired: u64,
}

#[derive(Clone)]
struct Ram {
    code: [u8; 6],
    bytes: [u8; RAM_LEN],
    callbacks: Vec<&'static str>,
    fail_read: Option<MemoryError>,
    fail_write: Option<MemoryError>,
}

impl GuestMemory for Ram {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        let offset = address
            .checked_sub(ENTRY_RIP)
            .and_then(|n| usize::try_from(n).ok());
        offset
            .and_then(|n| self.code.get(n).copied())
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }

    fn read(
        &mut self,
        address: u64,
        _: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        self.callbacks.push("read");
        if let Some(error) = self.fail_read.take() {
            return Err(error);
        }
        let start = usize::try_from(address).map_err(|_| MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })?;
        let source = self
            .bytes
            .get(start..start.saturating_add(destination.len()))
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Read,
            })?;
        destination.copy_from_slice(source);
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, source: &[u8]) -> Result<(), MemoryError> {
        self.callbacks.push("write");
        if let Some(error) = self.fail_write.take() {
            return Err(error);
        }
        let start = usize::try_from(address).map_err(|_| MemoryError::Unmapped {
            address,
            access: AccessKind::Write,
        })?;
        let destination = self
            .bytes
            .get_mut(start..start.saturating_add(source.len()))
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Write,
            })?;
        destination.copy_from_slice(source);
        Ok(())
    }

    fn atomic_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!("the corpus executes ordinary MOV only")
    }

    fn atomic_fetch_add_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!("the corpus executes ordinary MOV only")
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!("the corpus executes ordinary MOV only")
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        unreachable!("the corpus executes ordinary MOV only")
    }
}

#[derive(Clone, Copy)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

fn generated_input(seed: u64, index: usize, forced_sequence: Option<Sequence>) -> CorpusInput {
    let mut rng = Rng(seed ^ (index as u64).wrapping_mul(0xd6e8_feb8_6659_fd93));
    let generated_sequence = if rng.next() & 1 == 0 {
        Sequence::StoreThenLoad
    } else {
        Sequence::LoadThenStore
    };
    let sequence = forced_sequence.unwrap_or(generated_sequence);
    let mut ram = [0; RAM_LEN];
    for chunk in ram.chunks_mut(8) {
        let bytes = rng.next().to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }

    let first_address = 16 + (rng.next() % 88);
    let mode = index % 3;
    let second_address = match mode {
        0 => first_address,
        1 => {
            let candidate = 16 + (rng.next() % 87);
            if candidate >= first_address {
                candidate + 1
            } else {
                candidate
            }
        }
        _ => {
            let distance = (rng.next() % 7 + 1) as i64;
            let delta = if first_address <= 22 {
                distance
            } else if first_address >= 97 || rng.next() & 1 == 0 {
                -distance
            } else {
                distance
            };
            (first_address as i64 + delta) as u64
        }
    };
    let mut gpr = [0; 16];
    for value in &mut gpr {
        *value = rng.next();
    }
    // Both code sequences use RBX for the first address. The load-then-store
    // sequence uses RDX for the second address; the other sequence accesses the
    // same RBX range twice by definition of the admitted artifact.
    gpr[3] = first_address;
    gpr[2] = if sequence == Sequence::LoadThenStore {
        second_address
    } else {
        rng.next()
    };

    CorpusInput {
        sequence,
        gpr,
        flags: rng.next() & 0x8d5,
        first_address,
        second_address: if sequence == Sequence::LoadThenStore {
            second_address
        } else {
            first_address
        },
        ram,
    }
}

fn initial_state(input: &CorpusInput, rip: u64) -> LongModeState {
    LongModeState {
        gpr: input.gpr,
        rip,
        rflags: input.flags,
        ..Default::default()
    }
}

fn initial_ram(input: &CorpusInput) -> Ram {
    Ram {
        code: input.sequence.bytes(),
        bytes: input.ram,
        callbacks: Vec::new(),
        fail_read: None,
        fail_write: None,
    }
}

fn outcome(
    state: LongModeState,
    ram: Ram,
    status: String,
    remaining_budget: u64,
    retired: u64,
) -> Observation {
    Observation {
        state,
        ram: ram.bytes,
        callbacks: ram.callbacks,
        status,
        remaining_budget,
        retired,
    }
}

fn native_run(
    code: &PublishedCode,
    input: &CorpusInput,
    budget: u64,
    stop_rip: Option<u64>,
    stop: &AtomicU8,
    fault: Option<MemoryError>,
) -> Observation {
    let mut state = initial_state(input, ENTRY_RIP);
    let mut ram = initial_ram(input);
    match fault {
        Some(MemoryError::Architectural(_)) | Some(MemoryError::Backend(_)) => {
            if input.sequence == Sequence::StoreThenLoad {
                ram.fail_write = fault;
            } else {
                ram.fail_read = fault;
            }
        }
        Some(_) => panic!("unexpected fault injection"),
        None => {}
    }
    // SAFETY: this fixture owns private synchronous RAM separate from its code
    // bytes; its qword writes are all-or-nothing and it has no concurrent users.
    match unsafe {
        code.execute_ir_memory_two_site_v2(
            &mut state,
            &mut ram,
            AccessContext::default(),
            budget,
            stop_rip,
            stop,
        )
    } {
        Ok(MemoryOperationExecutionResultV2 {
            exit,
            remaining_budget,
            retired_instructions,
        }) => outcome(
            state,
            ram,
            format!("{exit:?}"),
            remaining_budget,
            retired_instructions,
        ),
        Err(error) => outcome(state, ram, format!("error:{error:?}"), budget, 0),
    }
}

fn native_continue(code: &PublishedCode, input: &CorpusInput, prior: &Observation) -> Observation {
    let mut state = prior.state.clone();
    let mut ram = initial_ram(input);
    ram.bytes = prior.ram;
    ram.callbacks.clone_from(&prior.callbacks);
    let stop = AtomicU8::new(0);
    // SAFETY: the continuation uses the same private synchronous fixture RAM
    // contract, carrying forward the committed snapshot from the first entry.
    match unsafe {
        code.execute_ir_memory_two_site_v2(
            &mut state,
            &mut ram,
            AccessContext::default(),
            1,
            None,
            &stop,
        )
    } {
        Ok(result) => outcome(
            state,
            ram,
            format!("{:?}", result.exit),
            result.remaining_budget,
            result.retired_instructions,
        ),
        Err(error) => outcome(state, ram, format!("error:{error:?}"), 1, 0),
    }
}

fn interpreter_run(
    input: &CorpusInput,
    budget: u64,
    stop_rip: Option<u64>,
    stop_requested: bool,
    fault: Option<MemoryError>,
) -> Observation {
    let mut state = initial_state(input, ENTRY_RIP);
    let mut ram = initial_ram(input);
    match fault {
        Some(MemoryError::Architectural(_)) | Some(MemoryError::Backend(_)) => {
            if input.sequence == Sequence::StoreThenLoad {
                ram.fail_write = fault;
            } else {
                ram.fail_read = fault;
            }
        }
        Some(_) => panic!("unexpected fault injection"),
        None => {}
    }
    let result = od_isa::execute::run(
        &mut state,
        &mut ram,
        od_isa::execute::RunConfig {
            instruction_budget: budget,
            stop_rip,
            context: AccessContext::default(),
        },
        || stop_requested,
    );
    let status = match result.exit {
        od_isa::execute::RunExit::Fault(fault) => format!("GuestFault({fault:?})"),
        od_isa::execute::RunExit::InstructionBudgetExhausted
            if result.instructions_executed == 2 && state.rip == ENTRY_RIP + 6 =>
        {
            "Complete".into()
        }
        od_isa::execute::RunExit::InstructionBudgetExhausted => "BudgetExhausted".into(),
        other => format!("{other:?}"),
    };
    outcome(
        state,
        ram,
        status,
        budget.saturating_sub(result.instructions_executed),
        result.instructions_executed,
    )
}

fn interpreter_continue(input: &CorpusInput, prior: &Observation) -> Observation {
    let mut state = prior.state.clone();
    let mut ram = initial_ram(input);
    ram.bytes = prior.ram;
    ram.callbacks.clone_from(&prior.callbacks);
    let status = match od_isa::execute::step(&mut state, &mut ram, AccessContext::default()) {
        Ok(od_isa::execute::StepOutcome::Advanced | od_isa::execute::StepOutcome::Branched) => {
            "Complete".to_owned()
        }
        Ok(od_isa::execute::StepOutcome::Fault(fault)) => format!("GuestFault({fault:?})"),
        Ok(other) => format!("{other:?}"),
        Err(error) => format!("error:{error:?}"),
    };
    outcome(state, ram, status, 0, 1)
}

fn assert_matches(
    seed: u64,
    case_id: usize,
    input: &CorpusInput,
    native: &Observation,
    interp: &Observation,
) {
    assert_matches_named("generated", seed, case_id, input, native, interp);
}

fn compare_observations(native: &Observation, interp: &Observation) -> Result<(), ()> {
    if native == interp { Ok(()) } else { Err(()) }
}

fn assert_matches_named(
    case: &str,
    seed: u64,
    case_id: usize,
    input: &CorpusInput,
    native: &Observation,
    interp: &Observation,
) {
    if compare_observations(native, interp).is_ok() {
        return;
    }
    save_mismatch(
        case,
        seed,
        case_id,
        input,
        native,
        interp,
        "backend results differ",
    );
}

fn assert_named_condition(
    case: &str,
    seed: u64,
    case_id: usize,
    input: &CorpusInput,
    native: &Observation,
    interp: &Observation,
    condition: bool,
    reason: &str,
) {
    if !condition {
        save_mismatch(case, seed, case_id, input, native, interp, reason);
    }
}

fn save_mismatch(
    case: &str,
    seed: u64,
    case_id: usize,
    input: &CorpusInput,
    native: &Observation,
    interp: &Observation,
    reason: &str,
) -> ! {
    let report = mismatch_report(case, seed, case_id, input, native, interp, reason);
    let path = std::env::temp_dir().join(format!(
        "od-host-memory-corpus-{seed:016x}-{case_id}-{case}.json"
    ));
    let write_result = std::fs::write(&path, &report);
    panic!(
        "memory corpus mismatch case={case} seed={seed:#018x} case_id={case_id}; saved={path:?}; write={write_result:?}\n{report}"
    );
}

fn mismatch_report(
    case: &str,
    seed: u64,
    case_id: usize,
    input: &CorpusInput,
    native: &Observation,
    interp: &Observation,
    reason: &str,
) -> String {
    format!(
        "{{\"case\":\"{}\",\"seed\":{seed},\"case_id\":{case_id},\"reason\":\"{}\",\"input\":{},\"native\":{},\"interpreter\":{}}}",
        escape_json(case),
        escape_json(reason),
        input.to_json(),
        native.to_json(),
        interp.to_json(),
    )
}

impl CorpusInput {
    fn to_json(&self) -> String {
        format!(
            "{{\"sequence\":\"{}\",\"gpr\":{},\"flags\":{},\"first_address\":{},\"second_address\":{},\"ram_hex\":\"{}\"}}",
            self.sequence.label(),
            json_numbers(&self.gpr),
            self.flags,
            self.first_address,
            self.second_address,
            hex(&self.ram),
        )
    }
}

impl Observation {
    fn to_json(&self) -> String {
        format!(
            "{{\"state_debug\":\"{}\",\"gpr\":{},\"flags\":{},\"rip\":{},\"ram_hex\":\"{}\",\"callbacks\":[{}],\"status\":\"{}\",\"remaining_budget\":{},\"retired\":{}}}",
            escape_json(&format!("{:?}", self.state)),
            json_numbers(&self.state.gpr),
            self.state.rflags,
            self.state.rip,
            hex(&self.ram),
            self.callbacks
                .iter()
                .map(|s| format!("\"{s}\""))
                .collect::<Vec<_>>()
                .join(","),
            escape_json(&self.status),
            self.remaining_budget,
            self.retired,
        )
    }
}

fn json_numbers(values: &[u64]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn escape_json(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[test]
fn fixed_seed_memory_corpus_matches_interpreter_twice_per_input() {
    let _guard = JIT_TEST_LOCK.lock().unwrap();
    let mut unique = HashSet::new();
    let mut address_relations = [0usize; 3];
    // Publication is process-exclusive. Compile and execute each sequence in
    // separate scopes below, retaining only its immutable IR artifact.
    let blocks: Vec<_> = [Sequence::StoreThenLoad, Sequence::LoadThenStore]
        .into_iter()
        .map(|sequence| {
            let source = sequence.bytes();
            od_jit::memory_load_v2::compile_ir_memory_two_site_block_v2(
                od_jit::block::CodeView::new(ENTRY_RIP, &source).unwrap(),
                ENTRY_RIP,
            )
            .unwrap()
        })
        .collect();
    let mut total = 0;
    for ((sequence, block), seed) in [Sequence::StoreThenLoad, Sequence::LoadThenStore]
        .into_iter()
        .zip(blocks.iter())
        .flat_map(|pair| SEEDS.into_iter().map(move |seed| (pair, seed)))
    {
        let _ = sequence;
        let published = PublishedCode::publish_ir_memory_two_site_v2(block).unwrap();
        for case_index in 0..CASES_PER_SEED {
            let input = generated_input(seed, case_index, Some(sequence));
            if sequence == Sequence::LoadThenStore {
                let distance = input.first_address.abs_diff(input.second_address);
                let relation = if distance == 0 {
                    0
                } else if distance < 8 {
                    1
                } else {
                    2
                };
                address_relations[relation] += 1;
            }
            assert!(
                unique.insert(input.clone()),
                "duplicate corpus input: seed={seed:#x} case={case_index}"
            );
            let case_id = total;
            total += 1;
            for replay in 0..2 {
                let stop = AtomicU8::new(0);
                let native = native_run(&published, &input, 2, None, &stop, None);
                let interpreter = interpreter_run(&input, 2, None, false, None);
                assert_matches(seed, case_id * 2 + replay, &input, &native, &interpreter);
                assert_eq!(native.status, "Complete", "seed={seed:#x} case={case_id}");
                assert_eq!(native.retired, 2, "seed={seed:#x} case={case_id}");
                assert_eq!(native.callbacks.len(), 2, "seed={seed:#x} case={case_id}");
                assert_eq!(native.callbacks, expected_order(input.sequence));
            }
        }
        drop(published);
    }
    assert_eq!(total, 2400);
    assert_eq!(unique.len(), total);
    assert!(address_relations.iter().all(|count| *count > 0));
}

fn expected_order(sequence: Sequence) -> Vec<&'static str> {
    match sequence {
        Sequence::StoreThenLoad => vec!["write", "read"],
        Sequence::LoadThenStore => vec!["read", "write"],
    }
}

#[test]
fn named_fault_stop_budget_and_continuation_cases_are_separate() {
    let _guard = JIT_TEST_LOCK.lock().unwrap();
    let seed = 0x5343_3441_ffff_0001;
    for sequence in [Sequence::StoreThenLoad, Sequence::LoadThenStore] {
        let case_id = sequence as usize;
        let input = generated_input(seed, case_id, Some(sequence));
        let fault = MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(2),
            linear_address: Some(input.first_address),
        });
        let source = sequence.bytes();
        let block = od_jit::memory_load_v2::compile_ir_memory_two_site_block_v2(
            od_jit::block::CodeView::new(ENTRY_RIP, &source).unwrap(),
            ENTRY_RIP,
        )
        .unwrap();
        let published = PublishedCode::publish_ir_memory_two_site_v2(&block).unwrap();

        let stop = AtomicU8::new(0);
        let native_fault = native_run(&published, &input, 2, None, &stop, Some(fault.clone()));
        let interp_fault = interpreter_run(&input, 2, None, false, Some(fault.clone()));
        assert_matches_named("fault", seed, case_id, &input, &native_fault, &interp_fault);
        assert_named_condition(
            "fault",
            seed,
            case_id,
            &input,
            &native_fault,
            &interp_fault,
            native_fault.status.contains("GuestFault")
                && interp_fault.status.contains("GuestFault"),
            "expected an architectural guest fault",
        );

        let requested = AtomicU8::new(1);
        let native_stop = native_run(&published, &input, 2, None, &requested, None);
        let interp_stop = interpreter_run(&input, 2, None, true, None);
        assert_matches_named("stop", seed, case_id, &input, &native_stop, &interp_stop);
        assert_named_condition(
            "stop",
            seed,
            case_id,
            &input,
            &native_stop,
            &interp_stop,
            native_stop.status == "StopRequested" && interp_stop.status == "StopRequested",
            "expected a pre-entry STOP exit",
        );

        let no_stop = AtomicU8::new(0);
        let native_budget = native_run(&published, &input, 1, None, &no_stop, None);
        let interp_budget = interpreter_run(&input, 1, None, false, None);
        assert_matches_named(
            "budget_before_continuation",
            seed,
            case_id,
            &input,
            &native_budget,
            &interp_budget,
        );
        assert_named_condition(
            "budget_before_continuation",
            seed,
            case_id,
            &input,
            &native_budget,
            &interp_budget,
            native_budget.status == "BudgetExhausted"
                && interp_budget.status == "BudgetExhausted"
                && native_budget.retired == 1
                && interp_budget.retired == 1
                && native_budget.state.rip == ENTRY_RIP + 3
                && interp_budget.state.rip == ENTRY_RIP + 3,
            "expected one committed instruction before budget exhaustion",
        );

        let native_continuation = native_continue(&published, &input, &native_budget);
        let interp_continuation = interpreter_continue(&input, &interp_budget);
        assert_matches_named(
            "continuation_from_committed_snapshot",
            seed,
            case_id,
            &input,
            &native_continuation,
            &interp_continuation,
        );
        assert_named_condition(
            "continuation_from_committed_snapshot",
            seed,
            case_id,
            &input,
            &native_continuation,
            &interp_continuation,
            native_continuation.status == "Complete"
                && interp_continuation.status == "Complete"
                && native_continuation.retired == 1
                && interp_continuation.retired == 1,
            "expected continuation to retire the second instruction",
        );
        drop(published);
    }
}

#[test]
fn comparator_rejects_corruption_and_serializes_named_diagnostics() {
    let input = generated_input(SEEDS[0], 0, Some(Sequence::StoreThenLoad));
    let base = Observation {
        state: initial_state(&input, ENTRY_RIP + 6),
        ram: input.ram,
        callbacks: vec!["write", "read"],
        status: "Complete".into(),
        remaining_budget: 0,
        retired: 2,
    };
    let mut wrong = base.clone();
    wrong.state.gpr[0] ^= 1;
    assert!(compare_observations(&base, &wrong).is_err(), "wrong GPR");
    let mut wrong = base.clone();
    wrong.state.rip += 1;
    assert!(compare_observations(&base, &wrong).is_err(), "wrong RIP");
    let mut wrong = base.clone();
    wrong.ram[17] ^= 1;
    assert!(
        compare_observations(&base, &wrong).is_err(),
        "wrong memory byte"
    );
    let mut wrong = base.clone();
    wrong.retired -= 1;
    assert!(
        compare_observations(&base, &wrong).is_err(),
        "wrong retirement"
    );
    let mut missing_fault = base.clone();
    missing_fault.status = "GuestFault(ArchitecturalFault { vector: 14 })".into();
    assert!(
        compare_observations(&base, &missing_fault).is_err(),
        "missing fault"
    );

    let seed = 0x5343_3441_ffff_0001;
    let case_id = 1;
    let input = generated_input(seed, case_id, Some(Sequence::LoadThenStore));
    let native = Observation {
        state: initial_state(&input, ENTRY_RIP + 3),
        ram: input.ram,
        callbacks: vec!["read"],
        status: "BudgetExhausted".into(),
        remaining_budget: 0,
        retired: 1,
    };
    let mut interpreter = native.clone();
    interpreter.status = "Complete".into();

    assert!(compare_observations(&native, &interpreter).is_err());
    let report = mismatch_report(
        "continuation_from_committed_snapshot",
        seed,
        case_id,
        &input,
        &native,
        &interpreter,
        "controlled named mismatch",
    );
    assert!(report.starts_with("{\"case\":\"continuation_from_committed_snapshot\""));
    assert!(report.contains(&format!("\"seed\":{seed}")));
    assert!(report.contains("\"case_id\":1"));
    assert!(report.contains("\"input\":{\"sequence\":\"load_then_store\""));
    assert!(report.contains("\"native\":{\"state_debug\":"));
    assert!(report.contains("\"interpreter\":{\"state_debug\":"));
    assert!(report.contains("\"status\":\"BudgetExhausted\""));
    assert!(report.contains("\"status\":\"Complete\""));
}
