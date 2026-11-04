use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use od_isa::memory::{AccessContext, GuestMemory, MemoryError};
use od_isa::{LongModeState, OperandSize};
use od_testkit::case::MemoryPermissions;
use od_testkit::memory::{FlatMemory, encode_bytes};
use od_testkit::v2::{ExpectedExit, Fixture, IntegerState};
use od_testkit::{HexU64, sha256_hex};
use serde::{Deserialize, Serialize};
use serde_json::json;

const FIXTURE_DIR: &str = "../../tests/fixtures/x86/stage2/s2_42s_rep_checkpoint";
const CASE_IDS: [&str; 8] = [
    "s2_42s_count1024_df0",
    "s2_42s_count1024_df1",
    "s2_42s_count1025_df0",
    "s2_42s_count1025_df1",
    "s2_42s_count2048_df0",
    "s2_42s_count2048_df1",
    "s2_42s_count2049_df0",
    "s2_42s_count2049_df1",
];
const CODE: [u8; 5] = [0x48, 0x85, 0xc0, 0xf3, 0xaa];
const CODE_BASE: u64 = 0x10_0000;
const DATA_BASE: u64 = 0x20_0000;
const DATA_LEN: usize = 4096;
const REPEAT_RCX: usize = 1;
const REPEAT_RDI: usize = 7;
const RFLAGS_DEFINED_AFTER_TEST: u64 = 0x0cc7;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct MachineSnapshot {
    gpr: [HexU64; 16],
    rip: HexU64,
    rflags: HexU64,
    rflags_defined: HexU64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct RegionSnapshot {
    region_id: String,
    guest_address: HexU64,
    bytes_hex: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Store {
    address: HexU64,
    length: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct PhaseRecord {
    phase: usize,
    exit: String,
    instructions_executed: u64,
    machine: MachineSnapshot,
    memory: Vec<RegionSnapshot>,
    write_journal: Vec<Store>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct CaseRecord {
    case_id: String,
    fixture_sha256: String,
    phases: Vec<PhaseRecord>,
}

struct JournalMemory {
    inner: FlatMemory,
    writes: Vec<Store>,
}

impl GuestMemory for JournalMemory {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        self.inner.fetch(address, context)
    }

    fn read(
        &mut self,
        address: u64,
        context: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        self.inner.read(address, context, destination)
    }

    fn write(
        &mut self,
        address: u64,
        context: AccessContext,
        source: &[u8],
    ) -> Result<(), MemoryError> {
        self.inner.write(address, context, source)?;
        self.writes.push(Store {
            address: HexU64(address),
            length: source.len(),
        });
        Ok(())
    }

    fn atomic_exchange_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
        replacement: u64,
    ) -> Result<u64, MemoryError> {
        self.inner
            .atomic_exchange_integer(address, context, size, replacement)
    }

    fn atomic_fetch_add_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
        addend: u64,
    ) -> Result<u64, MemoryError> {
        self.inner
            .atomic_fetch_add_integer(address, context, size, addend)
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
        expected: u64,
        replacement: u64,
    ) -> Result<u64, MemoryError> {
        self.inner
            .atomic_compare_exchange_integer(address, context, size, expected, replacement)
    }

    fn mfence(&mut self, context: AccessContext) -> Result<(), MemoryError> {
        self.inner.mfence(context)
    }
}

fn data_bytes() -> Vec<u8> {
    (0..DATA_LEN)
        .map(|index| [0xa5, 0x5a, 0x3c, 0xc3][index % 4])
        .collect()
}

fn fixture_path(case_id: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(FIXTURE_DIR)
        .join(format!("{case_id}.case.json"))
}

fn read_fixture(case_id: &str) -> (Vec<u8>, Fixture) {
    let bytes = fs::read(fixture_path(case_id)).expect("fixture file");
    let fixture = Fixture::from_json(&bytes)
        .expect("valid schema-v2 fixture")
        .fixture;
    (bytes, fixture)
}

fn validate_fixture(case_id: &str, bytes: &[u8], fixture: &Fixture) {
    assert_eq!(fixture.schema_version, 2, "{case_id}");
    assert_eq!(fixture.case_id, case_id);
    assert_eq!(fixture.profile, od_testkit::v2::Profile::LongIntegerV1);
    assert_eq!(fixture.initial_state.rip.get(), CODE_BASE);
    assert_eq!(fixture.initial_state.gpr[10].get(), 0x8877_6655_4433_2211);
    assert_eq!(fixture.initial_state.gpr[0].get() & 0xff, 0x11);
    assert_eq!(
        fixture.stop_rip.map(HexU64::get),
        Some(CODE_BASE + CODE.len() as u64)
    );
    assert_eq!(fixture.instruction_budget, 2);
    assert_eq!(fixture.rflags_mask.get(), 0x0cc7);
    assert_eq!(fixture.memory.len(), 2);
    let code = &fixture.memory[0];
    assert_eq!(code.region_id, "code");
    assert_eq!(code.guest_address.get(), CODE_BASE);
    assert_eq!(code.initial_bytes_hex, encode_bytes(&CODE));
    assert_eq!(
        code.permissions,
        MemoryPermissions {
            read: false,
            write: false,
            execute: true
        }
    );
    let data = &fixture.memory[1];
    assert_eq!(data.region_id, "data");
    assert_eq!(data.guest_address.get(), DATA_BASE);
    assert_eq!(data.length as usize, DATA_LEN);
    assert_eq!(data.initial_bytes_hex, encode_bytes(&data_bytes()));
    assert_eq!(
        data.permissions,
        MemoryPermissions {
            read: true,
            write: true,
            execute: false
        }
    );
    assert_eq!(data.comparison_mask_hex, "ff".repeat(DATA_LEN));
    let df = case_id.ends_with("df1");
    assert_eq!(fixture.initial_state.rflags.get() & (1 << 10) != 0, df);
    let offset = if df { 3072 } else { 16 };
    assert_eq!(
        fixture.initial_state.gpr[REPEAT_RDI].get(),
        DATA_BASE + offset
    );
    let count = case_id.split("count").nth(1).unwrap()[..4]
        .parse::<u64>()
        .unwrap();
    assert_eq!(fixture.initial_state.gpr[REPEAT_RCX].get(), count);
    assert_eq!(sha256_hex(bytes).len(), 64);
}

fn machine_snapshot(state: &LongModeState, defined: u64) -> MachineSnapshot {
    let state = IntegerState::try_from(state).expect("selected integer state is serializable");
    MachineSnapshot {
        gpr: state.gpr,
        rip: state.rip,
        rflags: state.rflags,
        rflags_defined: HexU64(defined),
    }
}

fn memory_snapshot(memory: &JournalMemory) -> Vec<RegionSnapshot> {
    memory
        .inner
        .snapshot()
        .into_iter()
        .map(|region| RegionSnapshot {
            region_id: region.region_id,
            guest_address: region.guest_address,
            bytes_hex: region.bytes_hex,
        })
        .collect()
}

fn restore_state(snapshot: &MachineSnapshot) -> LongModeState {
    let state = IntegerState {
        mode: od_testkit::v2::Mode::Long64,
        gpr: snapshot.gpr,
        rip: snapshot.rip,
        rflags: snapshot.rflags,
        cr0: None,
        cr4: None,
        fs_base: None,
        gs_base: None,
        kernel_gs_base: None,
        fs_selector: None,
        gs_selector: None,
        ds_selector: None,
        es_selector: None,
        ds_unusable: None,
        es_unusable: None,
        gdtr_base: None,
        gdtr_limit: None,
        cs_selector: None,
        cs_cache_base: None,
        cs_cache_limit: None,
        cs_cache_access: None,
        cs_cache_flags: None,
        ss_selector: None,
        ss_cache_base: None,
        ss_cache_limit: None,
        ss_cache_access: None,
        ss_cache_flags: None,
        fs_cache_limit: None,
        fs_cache_access: None,
        fs_cache_flags: None,
        gs_cache_limit: None,
        gs_cache_access: None,
        gs_cache_flags: None,
        x87: None,
        sse: None,
        sti_shadow: None,
        activity: None,
    };
    state.to_cpu().expect("serialized CPU checkpoint restores")
}

fn restore_memory(
    fixture: &Fixture,
    snapshot: &[RegionSnapshot],
    writes: Vec<Store>,
) -> JournalMemory {
    let mut restored = fixture.clone();
    assert_eq!(restored.memory.len(), snapshot.len());
    for (region, saved) in restored.memory.iter_mut().zip(snapshot) {
        assert_eq!(region.region_id, saved.region_id);
        assert_eq!(region.guest_address, saved.guest_address);
        region.initial_bytes_hex.clone_from(&saved.bytes_hex);
    }
    JournalMemory {
        inner: FlatMemory::from_fixture(&restored).expect("checkpoint RAM restores"),
        writes,
    }
}

fn exit_name(exit: &od_isa::execute::RunExit) -> &'static str {
    match exit {
        od_isa::execute::RunExit::StopAddress => "stop_address",
        od_isa::execute::RunExit::RepIterationBudgetExhausted => "rep_iteration_budget_exhausted",
        other => panic!("unexpected exit: {other:?}"),
    }
}

fn fixture_expected_exit_name(exit: ExpectedExit) -> &'static str {
    match exit {
        ExpectedExit::StopAddress => "stop_address",
        ExpectedExit::RepIterationBudgetExhausted => "rep_iteration_budget_exhausted",
        other => panic!("unexpected fixture expected_exit: {other:?}"),
    }
}

fn expected_write_addresses(case_id: &str, count: usize) -> Vec<Store> {
    let start = if case_id.ends_with("df1") {
        3072i64
    } else {
        16i64
    };
    let step = if case_id.ends_with("df1") { -1 } else { 1 };
    (0..count)
        .map(|index| Store {
            address: HexU64((DATA_BASE as i64 + start + step * index as i64) as u64),
            length: 1,
        })
        .collect()
}

fn run_case(case_id: &str, fixture_bytes: &[u8], fixture: &Fixture) -> CaseRecord {
    let mut state = fixture.initial_state.to_cpu().expect("fixture CPU state");
    let mut memory = JournalMemory {
        inner: FlatMemory::from_fixture(fixture).expect("fixture memory"),
        writes: Vec::new(),
    };
    let total = fixture.initial_state.gpr[REPEAT_RCX].get() as usize;
    let expected_first_exit = if total <= 1024 {
        ExpectedExit::StopAddress
    } else {
        ExpectedExit::RepIterationBudgetExhausted
    };
    assert_eq!(
        fixture.expected_exit, expected_first_exit,
        "fixture expected_exit must match its first bounded call"
    );
    let mut defined = od_isa::INTEGER_FLAGS;
    let mut phases = Vec::new();
    let mut cumulative = 0usize;
    loop {
        let phase = phases.len();
        let budget = if phase == 0 { 2 } else { 1 };
        let result = od_isa::execute::run_with_rflags_defined(
            &mut state,
            &mut memory,
            od_isa::execute::RunConfig {
                instruction_budget: budget,
                stop_rip: fixture.stop_rip.map(HexU64::get),
                context: AccessContext::default(),
            },
            defined,
            || false,
        )
        .expect("definedness mask remains valid");
        if phase == 0 {
            assert_eq!(
                exit_name(&result.exit),
                fixture_expected_exit_name(fixture.expected_exit)
            );
        }
        defined = result.rflags_defined;
        if phase == 0 {
            assert_eq!(defined, RFLAGS_DEFINED_AFTER_TEST);
        }
        cumulative = (cumulative + 1024).min(total);
        let machine = machine_snapshot(&state, defined);
        let memory_state = memory_snapshot(&memory);
        let expected = PhaseRecord {
            phase: phase + 1,
            exit: if cumulative == total {
                "stop_address"
            } else {
                "rep_iteration_budget_exhausted"
            }
            .into(),
            instructions_executed: if cumulative == total {
                if phase == 0 { 2 } else { 1 }
            } else if phase == 0 {
                1
            } else {
                0
            },
            machine: machine.clone(),
            memory: memory_state.clone(),
            write_journal: memory.writes.clone(),
        };
        assert_eq!(exit_name(&result.exit), expected.exit);
        assert_eq!(result.instructions_executed, expected.instructions_executed);
        let expected_rcx = (total - cumulative) as u64;
        assert_eq!(machine.gpr[REPEAT_RCX].get(), expected_rcx);
        let start = if case_id.ends_with("df1") {
            3072i64
        } else {
            16i64
        };
        let step = if case_id.ends_with("df1") {
            -1i64
        } else {
            1i64
        };
        assert_eq!(
            machine.gpr[REPEAT_RDI].get(),
            (DATA_BASE as i64 + start + step * cumulative as i64) as u64
        );
        assert_eq!(
            machine.rip.get(),
            if cumulative == total {
                CODE_BASE + 5
            } else {
                CODE_BASE + 3
            }
        );
        assert_eq!(machine.rflags_defined.get(), RFLAGS_DEFINED_AFTER_TEST);
        assert_eq!(
            machine.rflags.get() & RFLAGS_DEFINED_AFTER_TEST,
            if case_id.ends_with("df1") {
                0x486
            } else {
                0x86
            }
        );
        assert_eq!(memory.writes, expected_write_addresses(case_id, cumulative));
        phases.push(expected);
        if cumulative == total {
            break;
        }

        // The next phase starts only from serialized state, memory, definedness, and the journal.
        let serialized = serde_json::to_vec(&phases.last().unwrap()).unwrap();
        let checkpoint: PhaseRecord = serde_json::from_slice(&serialized).unwrap();
        state = restore_state(&checkpoint.machine);
        memory = restore_memory(fixture, &checkpoint.memory, checkpoint.write_journal);
        defined = checkpoint.machine.rflags_defined.get();
    }
    CaseRecord {
        case_id: case_id.into(),
        fixture_sha256: sha256_hex(fixture_bytes),
        phases,
    }
}

fn evidence_dir() -> PathBuf {
    let path = std::env::var_os("OD_S2_42S_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("s2_42s-rep-checkpoint-{}", std::process::id()))
        });
    fs::create_dir(&path).expect("evidence directory must be fresh");
    path
}

#[test]
fn s2_42s_rep_stosb_checkpoint_replay_matches_literal_evidence() {
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE_DIR);
    let expected_bytes =
        fs::read(fixture_root.join("expected.json")).expect("literal expected snapshots");
    let expected: BTreeMap<String, Vec<PhaseRecord>> =
        serde_json::from_slice(&expected_bytes).expect("expected snapshots parse");
    let manifest_bytes = fs::read(fixture_root.join("manifest.json")).expect("fixture manifest");
    let manifest: BTreeMap<String, String> =
        serde_json::from_slice(&manifest_bytes).expect("manifest parse");
    let expected_files: BTreeSet<String> = CASE_IDS
        .iter()
        .map(|id| format!("{id}.case.json"))
        .chain(["expected.json".into()])
        .collect();
    let actual_files: BTreeSet<String> = fs::read_dir(&fixture_root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name != "manifest.json")
        .collect();
    assert_eq!(
        actual_files, expected_files,
        "fixture directory file set drifted"
    );
    assert_eq!(
        manifest.keys().cloned().collect::<BTreeSet<_>>(),
        expected_files
    );
    assert_eq!(sha256_hex(&expected_bytes), manifest["expected.json"]);

    let out = evidence_dir();
    let mut records = Vec::new();
    for case_id in CASE_IDS {
        let (bytes, fixture) = read_fixture(case_id);
        validate_fixture(case_id, &bytes, &fixture);
        let filename = format!("{case_id}.case.json");
        assert_eq!(
            sha256_hex(&bytes),
            manifest[&filename],
            "fixture digest mismatch: {case_id}"
        );
        assert_eq!(
            expected.get(case_id).map(Vec::len),
            Some(expected_phase_count(case_id))
        );
        fs::write(out.join(&filename), bytes).unwrap();
        for repeat in 1..=2 {
            let record = run_case(case_id, &read_fixture(case_id).0, &fixture);
            assert!(
                case_phases_match(
                    &record.phases,
                    &expected[case_id],
                    expected_phase_count(case_id)
                ),
                "literal checkpoint mismatch: {case_id}, repeat {repeat}"
            );
            fs::write(
                out.join(format!("{case_id}.repeat-{repeat}.replay.json")),
                serde_json::to_vec_pretty(&record).unwrap(),
            )
            .unwrap();
            records.push(record);
        }
    }
    assert!(repeats_complete(&records));
    assert!(
        !repeats_complete(&records[..records.len() - 1]),
        "a missing clean repeat is rejected"
    );
    fs::write(out.join("expected.json"), expected_bytes).unwrap();
    fs::write(out.join("manifest.json"), manifest_bytes).unwrap();
    let summary = json!({
        "status": "pass",
        "cases": CASE_IDS.len(),
        "phases": records.iter().map(|record| record.phases.len()).sum::<usize>(),
        "repeats_per_case": 2,
        "native": "not_run",
        "physical": "not_run",
        "backend": "local od-isa interpreter with FlatMemory",
        "profile": "s2-long-integer-v1",
        "rflags_mask": "0x0000000000000cc7",
        "source_revision": command_output("git", &["rev-parse", "HEAD"]),
        "rustc": command_output("rustc", &["--version"]),
        "host": format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
        "code_hex": encode_bytes(&CODE),
        "command": "OD_S2_42S_EVIDENCE_DIR=<fresh-run-dir> cargo test -p od-testkit --test s2_42s_rep_checkpoint -- --exact s2_42s_rep_stosb_checkpoint_replay_matches_literal_evidence --nocapture"
    });
    fs::write(
        out.join("summary.json"),
        serde_json::to_vec_pretty(&summary).unwrap(),
    )
    .unwrap();
    println!(
        "S2-42s fail-closed summary: {}",
        serde_json::to_string(&summary).unwrap()
    );
}

fn command_output(command: &str, args: &[&str]) -> String {
    std::process::Command::new(command)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|| "unavailable".into())
}

fn expected_phase_count(case_id: &str) -> usize {
    let total = case_id.split("count").nth(1).unwrap()[..4]
        .parse::<usize>()
        .unwrap();
    total.div_ceil(1024)
}

#[test]
fn s2_42s_literal_verifier_rejects_checkpoint_and_manifest_tampering() {
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE_DIR);
    let expected_bytes = fs::read(fixture_root.join("expected.json")).unwrap();
    let mut expected: BTreeMap<String, Vec<PhaseRecord>> =
        serde_json::from_slice(&expected_bytes).unwrap();
    let baseline = expected.get(CASE_IDS[3]).unwrap()[0].clone();
    let mut undefined_af_changed = baseline.clone();
    undefined_af_changed.machine.rflags.0 ^= 1 << 4;
    assert!(
        replay_matches_literal(&undefined_af_changed, &baseline),
        "any concrete AF value is accepted"
    );
    let mut defined_flag_changed = baseline.clone();
    defined_flag_changed.machine.rflags.0 ^= 1 << 2;
    assert!(
        !replay_matches_literal(&defined_flag_changed, &baseline),
        "a defined flag mismatch is rejected"
    );
    let mut mask_changed = baseline.clone();
    mask_changed.machine.rflags_defined.0 ^= 1 << 2;
    assert!(
        !replay_matches_literal(&mask_changed, &baseline),
        "returned definedness must match exactly"
    );
    for corrupt in [
        {
            let mut value = baseline.clone();
            value.machine.gpr[REPEAT_RCX].0 ^= 1;
            value
        },
        {
            let mut value = baseline.clone();
            value.machine.gpr[REPEAT_RDI].0 ^= 1;
            value
        },
        {
            let mut value = baseline.clone();
            value.machine.rflags_defined.0 = od_isa::INTEGER_FLAGS;
            value
        },
        {
            let mut value = baseline.clone();
            value.write_journal.pop();
            value
        },
        {
            let mut value = baseline.clone();
            value.write_journal.push(value.write_journal[0].clone());
            value
        },
        {
            let mut value = baseline.clone();
            value.memory[1].bytes_hex.replace_range(..2, "00");
            value
        },
    ] {
        assert!(
            !replay_matches_literal(&corrupt, &baseline),
            "each replay corruption must be rejected"
        );
    }
    let full = expected.get(CASE_IDS[3]).unwrap();
    assert_eq!(full.len(), expected_phase_count(CASE_IDS[3]));
    let mut missing_phase = full.clone();
    missing_phase.pop();
    assert!(!case_phases_match(
        &missing_phase,
        full,
        expected_phase_count(CASE_IDS[3])
    ));
    let mut duplicate = full[0].clone();
    duplicate
        .write_journal
        .push(duplicate.write_journal[0].clone());
    assert!(
        !replay_matches_literal(&duplicate, &full[0]),
        "duplicate store rejected"
    );
    let fixture_bytes = fs::read(fixture_path(CASE_IDS[0])).unwrap();
    let manifest: BTreeMap<String, String> =
        serde_json::from_slice(&fs::read(fixture_root.join("manifest.json")).unwrap()).unwrap();
    for mutated in [
        {
            let mut v: serde_json::Value = serde_json::from_slice(&fixture_bytes).unwrap();
            v["initial_state"]["gpr"][1] = json!("0x0000000000000400");
            serde_json::to_vec(&v).unwrap()
        },
        {
            let mut v: serde_json::Value = serde_json::from_slice(&fixture_bytes).unwrap();
            v["initial_state"]["gpr"][7] = json!("0x000000000020000f");
            serde_json::to_vec(&v).unwrap()
        },
        {
            let mut v: serde_json::Value = serde_json::from_slice(&fixture_bytes).unwrap();
            v["memory"][1]["guest_address"] = json!("0x0000000000200001");
            serde_json::to_vec(&v).unwrap()
        },
        {
            let mut v: serde_json::Value = serde_json::from_slice(&fixture_bytes).unwrap();
            v["profile"] = json!("s2-long-arithmetic-v1");
            serde_json::to_vec(&v).unwrap()
        },
        {
            let mut v: serde_json::Value = serde_json::from_slice(&fixture_bytes).unwrap();
            v["memory"][0]["initial_bytes_hex"] = json!("4885c0f3ab");
            serde_json::to_vec(&v).unwrap()
        },
    ] {
        assert_ne!(
            sha256_hex(&mutated),
            manifest[&format!("{}.case.json", CASE_IDS[0])],
            "fixture digest rejects altered input"
        );
    }
    expected.get_mut(CASE_IDS[3]).unwrap().pop();
    assert_ne!(
        serde_json::to_vec(&expected).unwrap(),
        expected_bytes,
        "missing phase changes literal evidence digest"
    );
    let mut manifest: BTreeMap<String, String> =
        serde_json::from_slice(&fs::read(fixture_root.join("manifest.json")).unwrap()).unwrap();
    manifest
        .get_mut(&format!("{}.case.json", CASE_IDS[0]))
        .unwrap()
        .replace_range(..1, "0");
    assert_ne!(
        manifest,
        serde_json::from_slice::<BTreeMap<String, String>>(
            &fs::read(fixture_root.join("manifest.json")).unwrap()
        )
        .unwrap()
    );
}

fn repeats_complete(records: &[CaseRecord]) -> bool {
    records.len() == CASE_IDS.len() * 2
        && CASE_IDS.iter().all(|case_id| {
            records
                .iter()
                .filter(|record| record.case_id == *case_id)
                .count()
                == 2
        })
}

fn case_phases_match(
    actual: &[PhaseRecord],
    expected: &[PhaseRecord],
    expected_count: usize,
) -> bool {
    actual.len() == expected_count
        && actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| replay_matches_literal(actual, expected))
}

fn replay_matches_literal(actual: &PhaseRecord, expected: &PhaseRecord) -> bool {
    actual.phase == expected.phase
        && actual.exit == expected.exit
        && actual.instructions_executed == expected.instructions_executed
        && actual.machine.gpr == expected.machine.gpr
        && actual.machine.rip == expected.machine.rip
        && actual.machine.rflags.0 & RFLAGS_DEFINED_AFTER_TEST
            == expected.machine.rflags.0 & RFLAGS_DEFINED_AFTER_TEST
        && actual.machine.rflags_defined == HexU64(RFLAGS_DEFINED_AFTER_TEST)
        && actual.memory == expected.memory
        && actual.write_journal == expected.write_journal
}
