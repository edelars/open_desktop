#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use od_host::{MemoryOperationExecutionExitV2, PublishedCode};
use od_isa::memory::{AccessContext, InstructionMode, PrivilegeLevel};
use od_isa::{LongModeState, Register};
use od_machine::{
    AccessWidth, GuestPhysicalAddress, GuestPhysicalRange, MachineBus, MachineMemoryMap,
    MemoryRegionKind, MemoryRegionSpec, RamLimits,
};
use od_testkit::memory_fault::{
    Backend, DeviceEvent, ExecutionStatus, MemoryDelta, MemoryFaultObservation, Outcome,
    PhysicalWriteRange, compare,
};
use od_testkit::result::Observation;
use od_testkit::{HexU64, sha256_hex};
use std::sync::Mutex;

const CODE_RIP: u64 = 0x0040_1000;
const DATA_LINEAR: u64 = 0x0000_0080_0000_0120;
const CR0: u64 = 0x8001_0033;
const CR3: u64 = 0x1000;
const CR4: u64 = 1 << 5;
const EFER: u64 = (1 << 8) | (1 << 10) | (1 << 11);
const RFLAGS_DEFINED: u64 = od_isa::INTEGER_FLAGS & !(1 << 7);
const RAM_BYTES: u64 = 0xa000;
const CODE_PHYSICAL: u64 = 0x8000;
const DATA_PHYSICAL: u64 = 0x9000;
const INITIAL_RAX: u64 = 0xfeed_face_cafe_beef;
const LOADED_VALUE: u64 = 0x0123_4567_89ab_cdef;
const CODE: [u8; 3] = [0x48, 0x8b, 0x03];
const RAM_OBSERVATION_WORDS: [u64; 8] = [
    0x1000, 0x1008, 0x2000, 0x3000, 0x4010, 0x5000, 0x6008, 0x7000,
];
static JIT_TEST_LOCK: Mutex<()> = Mutex::new(());

fn context() -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: od_isa::memory::AccessOrigin::Explicit,
    }
}

fn put_qword(bus: &mut MachineBus, address: u64, value: u64) {
    bus.write_physical(
        GuestPhysicalAddress::new(address),
        AccessWidth::Qword,
        value,
        |_| {},
    )
    .unwrap();
}

fn page_table_entries(data_leaf_present: bool) -> [(u64, u64); 8] {
    [
        (0x1000, 0x2027),
        (0x1008, 0x3027),
        (0x2000, 0x4027),
        (0x3000, 0x5027),
        (0x4010, 0x6027),
        (0x5000, 0x7007),
        (0x6008, CODE_PHYSICAL | 0x27),
        (
            0x7000,
            DATA_PHYSICAL | if data_leaf_present { 0x7 } else { 0x6 },
        ),
    ]
}

fn make_bus(data_leaf_present: bool) -> MachineBus {
    let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), RAM_BYTES).unwrap();
    let map =
        MachineMemoryMap::new(36, vec![MemoryRegionSpec::new(ram, MemoryRegionKind::Ram)]).unwrap();
    let mut bus = MachineBus::new(map, RamLimits::new(RAM_BYTES), vec![], vec![], vec![]).unwrap();

    // Separate 4 KiB-aligned code/data branches keep fetch A-bits pre-set and
    // data A-bits observable in both backends.
    for (address, value) in page_table_entries(data_leaf_present) {
        put_qword(&mut bus, address, value);
    }
    bus.patch_physical_ram(GuestPhysicalAddress::new(CODE_PHYSICAL), &CODE)
        .unwrap();
    put_qword(&mut bus, DATA_PHYSICAL + 0x120, LOADED_VALUE);
    bus
}

fn initial_state(include_cr3: bool) -> LongModeState {
    let mut state = LongModeState {
        rip: CODE_RIP,
        cr0: Some(CR0),
        cr3: include_cr3.then_some(CR3),
        cr4: Some(CR4),
        rflags: 0x202,
        activity: Some(od_isa::state::CpuActivity::Running),
        sti_shadow: Some(od_isa::state::StiShadow::Clear),
        ..Default::default()
    };
    state.gpr[Register::Rax as usize] = INITIAL_RAX;
    state.gpr[Register::Rbx as usize] = DATA_LINEAR;
    state
}

fn publish_load() -> PublishedCode {
    let view = od_jit::block::CodeView::new(CODE_RIP, &CODE).unwrap();
    let block = od_jit::memory_load_v2::compile_memory_load_block_v2(view, CODE_RIP).unwrap();
    PublishedCode::publish_memory_load_v2(&block).unwrap()
}

fn table_snapshot(bus: &mut MachineBus) -> Vec<(u64, [u8; 8])> {
    RAM_OBSERVATION_WORDS
        .into_iter()
        .map(|address| {
            let value = bus
                .read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                .unwrap();
            (address, value.to_le_bytes())
        })
        .collect()
}

fn memory_deltas(before: &[(u64, [u8; 8])], after: &[(u64, [u8; 8])]) -> Vec<MemoryDelta> {
    before
        .iter()
        .zip(after)
        .filter_map(|((address, before), (_, after))| {
            (before != after).then(|| MemoryDelta {
                physical_address: HexU64(*address),
                before: before.to_vec(),
                after: after.to_vec(),
            })
        })
        .collect()
}

fn normalized(
    case_id: &str,
    input_sha256: &str,
    backend: Backend,
    state: &LongModeState,
    rflags_defined: u64,
    retired: u64,
    outcome: Outcome,
    before: &[(u64, [u8; 8])],
    bus: &mut MachineBus,
    committed_writes: Vec<GuestPhysicalRange>,
) -> MemoryFaultObservation {
    let after = table_snapshot(bus);
    let registers = [
        Register::Rax,
        Register::Rcx,
        Register::Rdx,
        Register::Rbx,
        Register::Rsp,
        Register::Rbp,
        Register::Rsi,
        Register::Rdi,
        Register::R8,
        Register::R9,
        Register::R10,
        Register::R11,
        Register::R12,
        Register::R13,
        Register::R14,
        Register::R15,
    ];
    MemoryFaultObservation {
        schema_version: od_testkit::memory_fault::MEMORY_FAULT_SCHEMA_VERSION,
        case_id: case_id.to_owned(),
        input_sha256: input_sha256.to_owned(),
        backend,
        status: ExecutionStatus::Completed {},
        gprs: Observation::Available {
            value: registers.map(|register| HexU64(state.gpr[register as usize])),
        },
        rip: Observation::Available {
            value: HexU64(state.rip),
        },
        rflags: Observation::Available {
            value: HexU64(state.rflags),
        },
        rflags_defined: HexU64(rflags_defined),
        retired: Observation::Available { value: retired },
        outcome: Observation::Available { value: outcome },
        architectural_cr2: Observation::Unavailable {
            reason: "paged step reports PageFaultRecord::cr2_value but does not commit CR2".into(),
        },
        memory_deltas: Observation::Available {
            value: memory_deltas(before, &after),
        },
        committed_writes: Observation::Available {
            value: committed_writes
                .into_iter()
                .map(|range| PhysicalWriteRange {
                    physical_address: HexU64(range.start().get()),
                    length: u32::try_from(range.len()).unwrap(),
                })
                .collect(),
        },
        device_events: Observation::Available {
            value: Vec::<DeviceEvent>::new(),
        },
    }
}

fn page_fault_outcome(record: &od_isa::memory::PageFaultRecord) -> Outcome {
    let fault = record.fault();
    Outcome::PageFault {
        vector: fault.vector,
        pfec: HexU64(u64::from(fault.error_code.unwrap_or(0))),
        faulting_rip: HexU64(record.saved_rip()),
        cr2_value: HexU64(record.cr2_value()),
    }
}

fn canonical_input(data_leaf_present: bool) -> Vec<u8> {
    let case_id = if data_leaf_present {
        "s4-17a-paged-mov-success"
    } else {
        "s4-17a-paged-mov-missing-pte"
    };
    let page_tables: Vec<_> = page_table_entries(data_leaf_present)
        .into_iter()
        .map(|(address, value)| (format!("{address:#x}"), format!("{value:#x}")))
        .collect();
    serde_json::to_vec(&serde_json::json!({
        "case_id": case_id,
        "instruction_hex": "488b03",
        "code_rip": format!("{CODE_RIP:#x}"),
        "code_physical": format!("{CODE_PHYSICAL:#x}"),
        "operand_linear": format!("{DATA_LINEAR:#x}"),
        "data_physical": format!("{DATA_PHYSICAL:#x}"),
        "ram_bytes": format!("{RAM_BYTES:#x}"),
        "cr0": format!("{CR0:#x}"),
        "cr3": format!("{CR3:#x}"),
        "cr4": format!("{CR4:#x}"),
        "efer": format!("{EFER:#x}"),
        "initial_rax": format!("{INITIAL_RAX:#x}"),
        "initial_rbx": format!("{DATA_LINEAR:#x}"),
        "initial_rflags": "0x202",
        "rflags_defined": format!("{RFLAGS_DEFINED:#x}"),
        "data_qword": format!("{LOADED_VALUE:#x}"),
        "page_table_entries": page_tables,
    }))
    .unwrap()
}

fn run_interpreter(data_leaf_present: bool) -> MemoryFaultObservation {
    let mut bus = make_bus(data_leaf_present);
    assert_eq!(
        bus.read_physical(GuestPhysicalAddress::new(CODE_PHYSICAL), AccessWidth::Byte)
            .unwrap(),
        u64::from(CODE[0])
    );
    let paging = od_machine::Long64ProbeConfig::new(CR0, CR3, CR4, EFER).unwrap();
    assert_eq!(
        bus.tracked_code_snapshot(&paging, CODE_RIP, CODE.len(), context())
            .unwrap()
            .bytes,
        CODE
    );
    let before = table_snapshot(&mut bus);
    let mut state = initial_state(false);
    let mut writes = Vec::new();
    let report = od_machine::step_paged_mov(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        RFLAGS_DEFINED,
        |range| writes.push(range),
    )
    .unwrap();
    assert_eq!(report.cpu.rflags_defined, RFLAGS_DEFINED);
    let outcome = match &report.cpu.exit {
        od_isa::execute::RunExit::StopAddress => Outcome::Completed {},
        od_isa::execute::RunExit::Fault(fault) => {
            assert_eq!(fault.vector, 14);
            let page_fault = report.page_fault.as_ref().unwrap();
            assert_eq!(fault, page_fault.fault());
            page_fault_outcome(page_fault)
        }
        other => panic!(
            "unexpected uncached interpreter exit: {other:?}; cpu={:?}; page_fault={:?}; memory_error={:?}; writes={writes:?}",
            report.cpu, report.page_fault, report.memory_error
        ),
    };
    let input = canonical_input(data_leaf_present);
    normalized(
        if data_leaf_present {
            "s4-17a-paged-mov-success"
        } else {
            "s4-17a-paged-mov-missing-pte"
        },
        &sha256_hex(&input),
        Backend::UncachedInterpreter,
        &state,
        report.cpu.rflags_defined,
        report.cpu.instructions_executed,
        outcome,
        &before,
        &mut bus,
        writes,
    )
}

fn run_jit(published: &PublishedCode, data_leaf_present: bool) -> MemoryFaultObservation {
    let mut bus = make_bus(data_leaf_present);
    let before = table_snapshot(&mut bus);
    let mut state = initial_state(false);
    let scope = published
        .execute_paged_mov_rax_rbx_v2(&mut state, &mut bus, CR3, EFER, context())
        .unwrap();
    let execution = scope.callback_result.unwrap();
    let outcome = match &execution.exit {
        MemoryOperationExecutionExitV2::Complete => Outcome::Completed {},
        MemoryOperationExecutionExitV2::GuestFault(fault) => {
            assert_eq!(fault.vector, 14);
            let page_fault = scope.page_fault.as_ref().unwrap();
            assert_eq!(fault, page_fault.fault());
            page_fault_outcome(page_fault)
        }
        other => panic!("unexpected JIT exit: {other:?}"),
    };
    let input = canonical_input(data_leaf_present);
    normalized(
        if data_leaf_present {
            "s4-17a-paged-mov-success"
        } else {
            "s4-17a-paged-mov-missing-pte"
        },
        &sha256_hex(&input),
        Backend::Jit,
        &state,
        RFLAGS_DEFINED,
        execution.retired_instructions,
        outcome,
        &before,
        &mut bus,
        scope.committed_writes,
    )
}

fn assert_success_literals(result: &MemoryFaultObservation) {
    assert_eq!(result.backend, Backend::UncachedInterpreter);
    assert_eq!(
        result.rip,
        Observation::Available {
            value: HexU64(CODE_RIP + 3)
        }
    );
    assert_eq!(result.retired, Observation::Available { value: 1 });
    assert_eq!(result.rflags_defined, HexU64(RFLAGS_DEFINED));
    assert_eq!(
        result.outcome,
        Observation::Available {
            value: Outcome::Completed {}
        }
    );
    let Observation::Available { value: gprs } = &result.gprs else {
        panic!("GPR snapshot missing");
    };
    assert_eq!(gprs[Register::Rax as usize], HexU64(LOADED_VALUE));
    assert!(matches!(
        result.memory_deltas,
        Observation::Available { ref value }
            if value.iter().map(|d| d.physical_address.get()).collect::<Vec<_>>() == [0x5000, 0x7000]
                && value.iter().all(|d| {
                    d.after[0] & 0x20 != 0
                        && d.before.iter().zip(&d.after).map(|(a, b)| a ^ b).collect::<Vec<_>>()
                            == [0x20, 0, 0, 0, 0, 0, 0, 0]
                })
    ));
    assert_eq!(
        result.committed_writes,
        Observation::Available {
            value: vec![
                PhysicalWriteRange {
                    physical_address: HexU64(0x5000),
                    length: 8
                },
                PhysicalWriteRange {
                    physical_address: HexU64(0x7000),
                    length: 8
                },
            ]
        }
    );
    assert_eq!(
        result.device_events,
        Observation::Available { value: vec![] }
    );
}

#[test]
fn s4_17a_compares_paged_interpreter_and_jit_memory_fault_results() {
    let _guard = JIT_TEST_LOCK.lock().unwrap();
    let published = publish_load();

    for data_leaf_present in [true, false] {
        let interpreter = run_interpreter(data_leaf_present);
        let jit = run_jit(&published, data_leaf_present);
        let differences = compare(&interpreter, &jit).unwrap();
        assert!(
            differences.is_empty(),
            "differential mismatch: {differences:#?}"
        );
        assert_eq!(interpreter.case_id, jit.case_id);
        assert_eq!(interpreter.input_sha256, jit.input_sha256);
        assert_eq!(interpreter.architectural_cr2, jit.architectural_cr2);
        assert_eq!(
            interpreter.device_events,
            Observation::Available { value: vec![] }
        );

        if data_leaf_present {
            assert_success_literals(&interpreter);
        } else {
            assert_eq!(
                interpreter.outcome,
                Observation::Available {
                    value: Outcome::PageFault {
                        vector: 14,
                        pfec: HexU64(0),
                        faulting_rip: HexU64(CODE_RIP),
                        cr2_value: HexU64(DATA_LINEAR),
                    }
                }
            );
            assert_eq!(interpreter.retired, Observation::Available { value: 0 });
            assert_eq!(
                interpreter.rip,
                Observation::Available {
                    value: HexU64(CODE_RIP)
                }
            );
            let Observation::Available { value: gprs } = &interpreter.gprs else {
                panic!("GPR snapshot missing");
            };
            assert_eq!(gprs[Register::Rax as usize], HexU64(INITIAL_RAX));
            assert!(matches!(
                interpreter.memory_deltas,
                Observation::Available { ref value }
                    if value.iter().map(|d| d.physical_address.get()).collect::<Vec<_>>() == [0x5000]
                        && value[0].after[0] & 0x20 != 0
                        && value[0].before.iter().zip(&value[0].after)
                            .map(|(a, b)| a ^ b).collect::<Vec<_>>() == [0x20, 0, 0, 0, 0, 0, 0, 0]
            ));
            assert_eq!(
                interpreter.committed_writes,
                Observation::Available {
                    value: vec![PhysicalWriteRange {
                        physical_address: HexU64(0x5000),
                        length: 8,
                    }]
                }
            );
        }

        // Inject a reporting fault after both backends ran; the comparator
        // must locate the missing Accessed-bit change at its physical entry.
        let mut injected = jit.clone();
        let Observation::Available { value } = &mut injected.memory_deltas else {
            unreachable!();
        };
        let data_pde = value
            .iter_mut()
            .find(|d| d.physical_address == HexU64(0x5000))
            .unwrap();
        data_pde.after[0] &= !0x20;
        let differences = compare(&interpreter, &injected).unwrap();
        assert!(differences.iter().any(|difference| {
            difference.field == "memory_deltas"
                && difference.physical_address == Some(HexU64(0x5000))
                && difference.differing_bits == Some(0x20)
        }));
    }
}
