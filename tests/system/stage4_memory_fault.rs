#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use od_host::{MemoryOperationExecutionExitV2, PublishedCode};
use od_isa::memory::{AccessContext, InstructionMode, PrivilegeLevel};
use od_isa::{LongModeState, Register};
use od_machine::{
    AccessWidth, BusDevice, DeviceAccess, DeviceError, GuestPhysicalAddress, GuestPhysicalRange,
    IoSpace, MachineBus, MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, MmioBinding,
    RamLimits,
};
use od_testkit::memory_fault::{
    Backend, DeviceEvent, ExecutionStatus, MemoryDelta, MemoryFaultObservation, Outcome,
    PhysicalWriteRange, compare,
};
use od_testkit::result::Observation;
use od_testkit::{HexU64, sha256_hex};
use std::sync::{Arc, Mutex};

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
/// `mov [rbx], rcx` (the interpreter implements it), placed right after `CODE`
/// in the fetch fixtures.
const FOLLOWING_CODE: [u8; 3] = [0x48, 0x89, 0x0b];
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
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

// ---------------------------------------------------------------------------
// S4-15b: the JIT block is admitted from bytes fetched out of guest RAM
// through the tracked instruction-fetch path, not from a compile-time constant.
// ---------------------------------------------------------------------------

const FETCH_RAM_BYTES: u64 = 0xb000;
/// Second code page: the same `48 8B 03` at offsets 0x0 and 0x80.
const CODE_PHYSICAL_2: u64 = 0xa000;
const CODE_RIP_2: u64 = 0x0040_2080;
const NX: u64 = 1 << 63;
const ACCESSED: u64 = 1 << 5;
const DIRTY: u64 = 1 << 6;
/// Code-branch table entries in walk order: PML4, PDPT, PD, PT.
const CODE_WALK_ENTRIES: [u64; 4] = [0x1000, 0x2000, 0x4010, 0x6008];
const CODE_WALK_RANGES: [(u64, usize); 4] = [(0x1000, 8), (0x2000, 8), (0x4010, 8), (0x6008, 8)];
/// Data-branch table entries (separate PML4 slot) and the data leaf.
const DATA_WALK_ENTRIES: [u64; 4] = [0x1008, 0x3000, 0x5000, 0x7000];

#[derive(Clone, Copy)]
struct FetchLayout {
    /// Accessed bit preset on the three upper code-branch entries.
    code_upper_ad: bool,
    /// Full flag word (low bits and NX) of the code PT leaf entries.
    code_leaf_flags: u64,
    data_leaf_present: bool,
}

impl FetchLayout {
    /// Code page user-accessible and executable, no Accessed bits preset.
    const CLEAN: Self = Self {
        code_upper_ad: false,
        code_leaf_flags: 0x7,
        data_leaf_present: true,
    };
}

fn fetch_context(privilege: PrivilegeLevel) -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege,
        origin: od_isa::memory::AccessOrigin::Explicit,
    }
}

fn make_fetch_bus(layout: FetchLayout) -> MachineBus {
    let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), FETCH_RAM_BYTES).unwrap();
    let map =
        MachineMemoryMap::new(36, vec![MemoryRegionSpec::new(ram, MemoryRegionKind::Ram)]).unwrap();
    let mut bus =
        MachineBus::new(map, RamLimits::new(FETCH_RAM_BYTES), vec![], vec![], vec![]).unwrap();
    populate_fetch_memory(&mut bus, layout);
    bus
}

/// Page tables, code bytes and the data qword shared by the RAM-only and the
/// MMIO-backed fetch buses.
fn populate_fetch_memory(bus: &mut MachineBus, layout: FetchLayout) {
    let upper = if layout.code_upper_ad { 0x27 } else { 0x7 };
    for (address, value) in [
        (0x1000, 0x2000 | upper),
        (0x2000, 0x4000 | upper),
        (0x4010, 0x6000 | upper),
        (0x6008, CODE_PHYSICAL | layout.code_leaf_flags),
        (0x6010, CODE_PHYSICAL_2 | layout.code_leaf_flags),
        // Data branch: every Accessed bit clear.
        (0x1008, 0x3007),
        (0x3000, 0x5007),
        (0x5000, 0x7007),
        (
            0x7000,
            DATA_PHYSICAL | if layout.data_leaf_present { 0x7 } else { 0x6 },
        ),
    ] {
        put_qword(bus, address, value);
    }
    for address in [CODE_PHYSICAL, CODE_PHYSICAL_2, CODE_PHYSICAL_2 + 0x80] {
        bus.patch_physical_ram(GuestPhysicalAddress::new(address), &CODE)
            .unwrap();
    }
    // A second instruction the interpreter executes (`mov [rbx], rcx`) right
    // after the first one. One step must retire only the instruction at RIP: an
    // extra interpreter step after the JIT would move RIP, overwrite the data
    // qword with RCX and set its Dirty bit, all visible against the oracle.
    bus.patch_physical_ram(
        GuestPhysicalAddress::new(CODE_PHYSICAL + CODE.len() as u64),
        &FOLLOWING_CODE,
    )
    .unwrap();
    put_qword(bus, DATA_PHYSICAL + 0x120, LOADED_VALUE);
}

fn fetch_state(rip: u64) -> LongModeState {
    let mut state = initial_state(false);
    state.rip = rip;
    state
}

fn ram_image(bus: &mut MachineBus) -> Vec<u64> {
    (0..FETCH_RAM_BYTES)
        .step_by(8)
        .map(|address| {
            bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                .unwrap()
        })
        .collect()
}

fn entry(bus: &mut MachineBus, address: u64) -> u64 {
    bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
        .unwrap()
}

fn tracked_patch(bus: &mut MachineBus, linear: u64, physical_page: u64, value: u8) {
    let paging = od_machine::Long64ProbeConfig::new(CR0, CR3, CR4, EFER).unwrap();
    bus.patch_tracked_code_byte(
        &paging,
        linear,
        GuestPhysicalAddress::new(physical_page),
        value,
        fetch_context(PrivilegeLevel::Ring0),
    )
    .unwrap();
}

/// Everything compared between the interpreter and the fetched-JIT path.
#[derive(Debug, PartialEq)]
struct FetchObservation {
    gprs: [u64; 16],
    rip: u64,
    rflags: u64,
    page_fault: Option<od_isa::memory::PageFaultRecord>,
    writes: Vec<(u64, u64)>,
    ram: Vec<u64>,
}

fn range_pairs(ranges: &[GuestPhysicalRange]) -> Vec<(u64, u64)> {
    ranges
        .iter()
        .map(|range| (range.start().get(), range.len()))
        .collect()
}

fn observe_interpreter(
    bus: &mut MachineBus,
    state: &mut LongModeState,
    privilege: PrivilegeLevel,
) -> FetchObservation {
    let mut writes = Vec::new();
    let report = od_machine::step_paged_mov(
        state,
        bus,
        CR3,
        EFER,
        fetch_context(privilege),
        RFLAGS_DEFINED,
        |range| writes.push(range),
    )
    .unwrap();
    let page_fault = match &report.cpu.exit {
        od_isa::execute::RunExit::StopAddress => None,
        od_isa::execute::RunExit::Fault(fault) => {
            let record = report.page_fault.clone().unwrap();
            assert_eq!(fault, record.fault());
            Some(record)
        }
        other => panic!("unexpected interpreter exit: {other:?}"),
    };
    FetchObservation {
        gprs: state.gpr,
        rip: state.rip,
        rflags: state.rflags & RFLAGS_DEFINED,
        page_fault,
        writes: range_pairs(&writes),
        ram: ram_image(bus),
    }
}

/// Runs the session once; returns the observation and the admission outcome,
/// or the typed refusal (with the state/RAM left for the caller to inspect).
fn observe_fetched_jit(
    session: &mut od_host::PagedFetchedLoadSession,
    bus: &mut MachineBus,
    state: &mut LongModeState,
    privilege: PrivilegeLevel,
) -> Result<(FetchObservation, od_host::PagedFetchedLoadReport), od_host::PagedFetchedLoadError> {
    let report = session.execute(state, bus, CR3, EFER, fetch_context(privilege))?;
    let execution = report.execution.callback_result.as_ref().unwrap();
    let page_fault = match &execution.exit {
        MemoryOperationExecutionExitV2::Complete => None,
        MemoryOperationExecutionExitV2::GuestFault(fault) => {
            let record = report.execution.page_fault.clone().unwrap();
            assert_eq!(fault, record.fault());
            Some(record)
        }
        other => panic!("unexpected JIT exit: {other:?}"),
    };
    let mut writes = range_pairs(&report.fetch_committed_writes);
    writes.extend(range_pairs(&report.execution.committed_writes));
    Ok((
        FetchObservation {
            gprs: state.gpr,
            rip: state.rip,
            rflags: state.rflags & RFLAGS_DEFINED,
            page_fault,
            writes,
            ram: ram_image(bus),
        },
        report,
    ))
}

fn fetch_page_fault(
    error: &od_host::PagedFetchedLoadError,
) -> (
    &od_isa::memory::PageFaultRecord,
    &od_machine::PageFaultReason,
) {
    match &error.kind {
        od_host::PagedFetchedLoadErrorKind::Fetch(
            od_machine::TrackedCodeSnapshotError::Access(
                od_machine::TrackedAccessError::PageFault { record, reason },
            ),
        ) => (record, reason),
        other => panic!("expected a fetch #PF, got {other:?}"),
    }
}

#[test]
fn s4_15b_block_is_admitted_from_guest_ram_bytes_and_matches_the_interpreter() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut session = od_host::PagedFetchedLoadSession::new();

    // Interpreter oracle and JIT run over independent identical RAM.
    let mut interp_bus = make_fetch_bus(FetchLayout::CLEAN);
    let mut interp_state = fetch_state(CODE_RIP);
    let expected = observe_interpreter(&mut interp_bus, &mut interp_state, PrivilegeLevel::Ring0);
    assert_eq!(expected.gprs[Register::Rax as usize], LOADED_VALUE);

    let mut bus = make_fetch_bus(FetchLayout::CLEAN);
    let mut state = fetch_state(CODE_RIP);
    let (observed, report) =
        observe_fetched_jit(&mut session, &mut bus, &mut state, PrivilegeLevel::Ring0).unwrap();
    assert_eq!(observed, expected, "JIT must equal the interpreter");
    assert_eq!(report.admission, od_host::PagedFetchAdmission::Admitted);
    assert_eq!(report.fetched_bytes, CODE);
    assert_eq!(report.fetched_linear_base, CODE_RIP);
    assert_eq!(report.fetched_physical_page.get(), CODE_PHYSICAL);
    assert_eq!(state.rip, CODE_RIP + 3);
    assert_eq!(session.admissions(), 1);

    // Same bytes, same page, same generation: the admitted block is reused.
    state.rip = CODE_RIP;
    state.gpr[Register::Rax as usize] = INITIAL_RAX;
    let (_, again) =
        observe_fetched_jit(&mut session, &mut bus, &mut state, PrivilegeLevel::Ring0).unwrap();
    assert_eq!(again.admission, od_host::PagedFetchAdmission::Reused);
    assert_eq!(session.admissions(), 1);
    assert_eq!(state.gpr[Register::Rax as usize], LOADED_VALUE);

    // The block follows the fetched RIP: the same form at another linear
    // address, another physical page and another page offset is admitted
    // anew. A block compiled from a constant CODE_RIP would refuse this entry
    // with EntryRipMismatch instead of executing.
    let mut alias_bus = make_fetch_bus(FetchLayout::CLEAN);
    let mut alias_interp_bus = make_fetch_bus(FetchLayout::CLEAN);
    let mut alias_interp_state = fetch_state(CODE_RIP_2);
    let alias_expected = observe_interpreter(
        &mut alias_interp_bus,
        &mut alias_interp_state,
        PrivilegeLevel::Ring0,
    );
    let mut alias_state = fetch_state(CODE_RIP_2);
    let (alias_observed, alias_report) = observe_fetched_jit(
        &mut session,
        &mut alias_bus,
        &mut alias_state,
        PrivilegeLevel::Ring0,
    )
    .unwrap();
    assert_eq!(alias_observed, alias_expected);
    assert_eq!(
        alias_report.admission,
        od_host::PagedFetchAdmission::Admitted
    );
    assert_eq!(alias_report.fetched_physical_page.get(), CODE_PHYSICAL_2);
    assert_eq!(alias_state.rip, CODE_RIP_2 + 3);
    assert_eq!(session.admissions(), 2);
}

#[test]
fn s4_15b_unsupported_bytes_in_ram_are_refused_not_replaced_by_a_constant() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut session = od_host::PagedFetchedLoadSession::new();
    let mut bus = make_fetch_bus(FetchLayout::CLEAN);

    // Admit the supported form first so a stale block exists to be dropped.
    let mut state = fetch_state(CODE_RIP);
    observe_fetched_jit(&mut session, &mut bus, &mut state, PrivilegeLevel::Ring0).unwrap();
    assert!(session.has_admitted_block());

    // 48 8B 0B is MOV RCX,[RBX]: a different (compilable) memory form.
    tracked_patch(&mut bus, CODE_RIP + 2, CODE_PHYSICAL, 0x0b);
    let data_before: Vec<u64> = DATA_WALK_ENTRIES.map(|a| entry(&mut bus, a)).to_vec();
    let mut state = fetch_state(CODE_RIP);
    let before = state.clone();
    let error = session
        .execute(
            &mut state,
            &mut bus,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
        )
        .unwrap_err();
    assert_eq!(
        error.kind,
        od_host::PagedFetchedLoadErrorKind::UnsupportedForm {
            bytes: vec![0x48, 0x8b, 0x0b]
        }
    );
    // The constant `48 8B 03` block was NOT run: RAX/RCX/RIP untouched, and
    // the operand walk never happened.
    assert_eq!(state, before);
    assert_eq!(state.gpr[Register::Rax as usize], INITIAL_RAX);
    let data_after: Vec<u64> = DATA_WALK_ENTRIES.map(|a| entry(&mut bus, a)).to_vec();
    assert_eq!(data_after, data_before);
    assert!(!session.has_admitted_block());

    // Not a memory load at all (NOP NOP NOP).
    for byte in [0x90u8, 0x90, 0x90].iter().enumerate() {
        tracked_patch(&mut bus, CODE_RIP + byte.0 as u64, CODE_PHYSICAL, *byte.1);
    }
    let mut state = fetch_state(CODE_RIP);
    let error = session
        .execute(
            &mut state,
            &mut bus,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
        )
        .unwrap_err();
    assert_eq!(
        error.kind,
        od_host::PagedFetchedLoadErrorKind::UnsupportedForm {
            bytes: vec![0x90, 0x90, 0x90]
        }
    );
    assert_eq!(state.gpr[Register::Rax as usize], INITIAL_RAX);
    assert_eq!(session.admissions(), 1);
}

#[test]
fn s4_15b_fetch_faults_precede_any_data_access_and_match_the_interpreter() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // (label, layout, CPL, expected PFEC literal)
    // PFEC bits: P=0x1, U/S=0x4, I/D=0x10 (EFER.NXE=1).
    let cases = [
        (
            "NX leaf at CPL0",
            FetchLayout {
                code_leaf_flags: 0x27 | NX,
                ..FetchLayout::CLEAN
            },
            PrivilegeLevel::Ring0,
            0x11u32,
        ),
        (
            "supervisor-only leaf at CPL3",
            FetchLayout {
                code_leaf_flags: 0x3,
                ..FetchLayout::CLEAN
            },
            PrivilegeLevel::Ring3,
            0x15,
        ),
        (
            "not-present leaf at CPL0",
            FetchLayout {
                code_leaf_flags: 0x6,
                ..FetchLayout::CLEAN
            },
            PrivilegeLevel::Ring0,
            0x10,
        ),
    ];
    for (label, layout, privilege, pfec) in cases {
        let mut interp_bus = make_fetch_bus(layout);
        let mut interp_state = fetch_state(CODE_RIP);
        let expected = observe_interpreter(&mut interp_bus, &mut interp_state, privilege);
        let interp_record = expected.page_fault.clone().expect(label);
        assert_eq!(interp_record.fault().vector, 14, "{label}");
        assert_eq!(interp_record.fault().error_code, Some(pfec), "{label}");
        assert_eq!(interp_record.cr2_value(), CODE_RIP, "{label}");

        let mut session = od_host::PagedFetchedLoadSession::new();
        let mut bus = make_fetch_bus(layout);
        let data_before: Vec<u64> = DATA_WALK_ENTRIES.map(|a| entry(&mut bus, a)).to_vec();
        let mut state = fetch_state(CODE_RIP);
        let before = state.clone();
        let error = session
            .execute(&mut state, &mut bus, CR3, EFER, fetch_context(privilege))
            .unwrap_err();
        let (record, _reason) = fetch_page_fault(&error);
        // Exact #PF: vector, instruction-fetch PFEC, CR2 = RIP, saved RIP.
        assert_eq!(record, &interp_record, "{label}");
        assert_eq!(record.fault().error_code, Some(pfec), "{label}");
        assert_eq!(record.cr2_value(), CODE_RIP, "{label}");
        assert_eq!(record.saved_rip(), CODE_RIP, "{label}");
        // No RAX write, no retirement, nothing admitted or executed.
        assert_eq!(state, before, "{label}");
        assert_eq!(state.gpr[Register::Rax as usize], INITIAL_RAX, "{label}");
        assert!(!session.has_admitted_block(), "{label}");
        assert_eq!(session.admissions(), 0, "{label}");
        // The operand walk (which would succeed) never ran: data-branch
        // entries keep their Accessed bits clear.
        let data_after: Vec<u64> = DATA_WALK_ENTRIES.map(|a| entry(&mut bus, a)).to_vec();
        assert_eq!(data_after, data_before, "{label}");
        assert!(data_after.iter().all(|v| v & ACCESSED == 0), "{label}");
        // Same committed fetch-walk effects and final RAM as the interpreter.
        assert_eq!(
            range_pairs(&error.fetch_committed_writes),
            expected.writes,
            "{label}"
        );
        assert_eq!(ram_image(&mut bus), expected.ram, "{label}");
        assert_eq!(state.rip, expected.rip, "{label}");
        assert_eq!(state.gpr, expected.gprs, "{label}");
    }
}

#[test]
fn s4_15b_code_accessed_bits_are_committed_without_preset_and_data_fault_matches() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for (label, layout, privilege) in [
        ("success CPL0", FetchLayout::CLEAN, PrivilegeLevel::Ring0),
        ("success CPL3", FetchLayout::CLEAN, PrivilegeLevel::Ring3),
        (
            "missing data leaf CPL0",
            FetchLayout {
                data_leaf_present: false,
                ..FetchLayout::CLEAN
            },
            PrivilegeLevel::Ring0,
        ),
    ] {
        let mut interp_bus = make_fetch_bus(layout);
        let mut interp_state = fetch_state(CODE_RIP);
        let expected = observe_interpreter(&mut interp_bus, &mut interp_state, privilege);

        let mut session = od_host::PagedFetchedLoadSession::new();
        let mut bus = make_fetch_bus(layout);
        // No Accessed bit on the code branch before execution (S4-17a preset them).
        for address in CODE_WALK_ENTRIES {
            assert_eq!(entry(&mut bus, address) & ACCESSED, 0, "{label}");
        }
        let mut state = fetch_state(CODE_RIP);
        let (observed, report) =
            observe_fetched_jit(&mut session, &mut bus, &mut state, privilege).unwrap();
        assert_eq!(observed, expected, "{label}: JIT == interpreter");
        // Fetch A commits are an effect: four ordered walk writes, A set, D clear.
        assert_eq!(
            range_pairs(&report.fetch_committed_writes),
            CODE_WALK_RANGES.map(|(a, l)| (a, l as u64)).to_vec(),
            "{label}"
        );
        for address in CODE_WALK_ENTRIES {
            let value = entry(&mut bus, address);
            assert_ne!(value & ACCESSED, 0, "{label}: A set at {address:#x}");
            assert_eq!(value & DIRTY, 0, "{label}: no D at {address:#x}");
        }
        if layout.data_leaf_present {
            assert_eq!(state.gpr[Register::Rax as usize], LOADED_VALUE, "{label}");
        } else {
            // Data #PF after a successful fetch: PFEC 0 (not-present read),
            // RAX untouched, yet the code A bits above stay committed.
            let record = observed.page_fault.as_ref().unwrap();
            assert_eq!(record.fault().error_code, Some(0));
            assert_eq!(record.cr2_value(), DATA_LINEAR);
            assert_eq!(record.saved_rip(), CODE_RIP);
            assert_eq!(state.gpr[Register::Rax as usize], INITIAL_RAX);
            assert_eq!(state.rip, CODE_RIP);
        }

        // A second execution finds every code A bit already set: the walk
        // commits nothing new for the fetch, and the block is reused.
        state.rip = CODE_RIP;
        let (_, second) =
            observe_fetched_jit(&mut session, &mut bus, &mut state, privilege).unwrap();
        assert!(second.fetch_committed_writes.is_empty(), "{label}");
        assert_eq!(second.admission, od_host::PagedFetchAdmission::Reused);
    }
}

#[test]
fn s4_15b_patched_code_byte_forces_readmission_and_never_runs_the_old_block() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut session = od_host::PagedFetchedLoadSession::new();
    let mut bus = make_fetch_bus(FetchLayout::CLEAN);
    let run = |session: &mut od_host::PagedFetchedLoadSession, bus: &mut MachineBus| {
        let mut state = fetch_state(CODE_RIP);
        let result = session.execute(
            &mut state,
            bus,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
        );
        (state, result)
    };

    let (state, first) = run(&mut session, &mut bus);
    let first = first.unwrap();
    assert_eq!(state.gpr[Register::Rax as usize], LOADED_VALUE);
    assert_eq!(first.admission, od_host::PagedFetchAdmission::Admitted);

    // Patch through a tracked write to a different byte: the old block would
    // still load LOADED_VALUE, so RAX staying INITIAL_RAX proves it did not run.
    tracked_patch(&mut bus, CODE_RIP + 2, CODE_PHYSICAL, 0x0b);
    let (state, patched) = run(&mut session, &mut bus);
    let error = patched.unwrap_err();
    assert!(matches!(
        error.kind,
        od_host::PagedFetchedLoadErrorKind::UnsupportedForm { .. }
    ));
    assert_eq!(state.gpr[Register::Rax as usize], INITIAL_RAX);
    assert_eq!(state.rip, CODE_RIP);
    assert!(!session.has_admitted_block());

    // Restoring the original byte is a new write: new generation, new admission.
    tracked_patch(&mut bus, CODE_RIP + 2, CODE_PHYSICAL, 0x03);
    let (state, restored) = run(&mut session, &mut bus);
    let restored = restored.unwrap();
    assert_eq!(restored.admission, od_host::PagedFetchAdmission::Admitted);
    assert!(restored.fetched_generation > first.fetched_generation);
    assert_eq!(state.gpr[Register::Rax as usize], LOADED_VALUE);
    assert_eq!(session.admissions(), 2);

    // Rewriting the same value still moves the generation: conservative
    // re-admission rather than trusting that the bytes are unchanged.
    tracked_patch(&mut bus, CODE_RIP + 2, CODE_PHYSICAL, 0x03);
    let (_, rewritten) = run(&mut session, &mut bus);
    let rewritten = rewritten.unwrap();
    assert_eq!(rewritten.admission, od_host::PagedFetchAdmission::Admitted);
    assert!(rewritten.fetched_generation > restored.fetched_generation);
    assert_eq!(session.admissions(), 3);

    // Remapping the code PTE to another physical page holding the same bytes
    // changes the page identity: admitted again, bytes unchanged.
    put_qword(&mut bus, 0x6008, CODE_PHYSICAL_2 | 0x7);
    let (state, remapped) = run(&mut session, &mut bus);
    let remapped = remapped.unwrap();
    assert_eq!(remapped.admission, od_host::PagedFetchAdmission::Admitted);
    assert_eq!(remapped.fetched_physical_page.get(), CODE_PHYSICAL_2);
    assert_eq!(state.gpr[Register::Rax as usize], LOADED_VALUE);
    assert_eq!(session.admissions(), 4);

    // Rights are re-checked on every call even with an admitted block: making
    // the live code page NX makes the next execution fault without running it.
    put_qword(&mut bus, 0x6008, CODE_PHYSICAL_2 | 0x27 | NX);
    let (state, nx) = run(&mut session, &mut bus);
    let error = nx.unwrap_err();
    assert_eq!(fetch_page_fault(&error).0.fault().error_code, Some(0x11));
    assert_eq!(state.gpr[Register::Rax as usize], INITIAL_RAX);
    assert_eq!(session.admissions(), 4);
}

#[test]
fn s4_15b_reuse_key_compares_bytes_when_page_and_generation_are_equal() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // The session key is (linear RIP, physical page, generation, bytes) and
    // is not bound to a particular bus, so a second bus can present the same
    // RIP, the same physical page and the same generation with other bytes.
    // Both buses get exactly one tracked write to the code page, so their
    // generations are equal; only the bytes differ.
    let mut session = od_host::PagedFetchedLoadSession::new();
    let mut bus_a = make_fetch_bus(FetchLayout::CLEAN);
    tracked_patch(&mut bus_a, CODE_RIP + 2, CODE_PHYSICAL, 0x03);
    let mut bus_b = make_fetch_bus(FetchLayout::CLEAN);
    tracked_patch(&mut bus_b, CODE_RIP + 2, CODE_PHYSICAL, 0x0b);

    let page = GuestPhysicalAddress::new(CODE_PHYSICAL);
    let generation = bus_a.code_page_generation(page).unwrap();
    assert_eq!(bus_b.code_page_generation(page).unwrap(), generation);

    let mut state = fetch_state(CODE_RIP);
    let (_, first) =
        observe_fetched_jit(&mut session, &mut bus_a, &mut state, PrivilegeLevel::Ring0).unwrap();
    assert_eq!(first.admission, od_host::PagedFetchAdmission::Admitted);
    assert_eq!(first.fetched_generation, generation);
    assert_eq!(first.fetched_physical_page.get(), CODE_PHYSICAL);
    assert_eq!(state.gpr[Register::Rax as usize], LOADED_VALUE);
    assert!(session.has_admitted_block());

    // Same RIP, page and generation, different bytes (48 8B 0B): the old
    // block must be refused, not run and not reused.
    let mut state = fetch_state(CODE_RIP);
    let before = state.clone();
    let error = session
        .execute(
            &mut state,
            &mut bus_b,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
        )
        .unwrap_err();
    assert_eq!(
        error.kind,
        od_host::PagedFetchedLoadErrorKind::UnsupportedForm {
            bytes: vec![0x48, 0x8b, 0x0b]
        }
    );
    assert_eq!(state, before);
    assert_eq!(state.gpr[Register::Rax as usize], INITIAL_RAX);
    assert!(!session.has_admitted_block());
    assert_eq!(session.admissions(), 1);
}

// ---------------------------------------------------------------------------
// S4-15c: one step through the JIT with an interpreter fallback. A JIT refusal
// that executed nothing runs `step_paged_mov` exactly once on the same state
// and bus; an executed JIT step (success or guest #PF) is never repeated.
// ---------------------------------------------------------------------------

/// Device window directly above RAM; the data leaf maps to it in the MMIO cases.
const MMIO_BASE: u64 = FETCH_RAM_BYTES;
const MMIO_VALUE: u64 = 0x5a5a_a5a5_0f0f_f0f0;
/// Data-walk entries in walk order, as the page walk commits them.
const DATA_WALK_RANGES: [(u64, u64); 4] = [(0x1008, 8), (0x3000, 8), (0x5000, 8), (0x7000, 8)];

#[derive(Clone, Debug, Eq, PartialEq)]
enum DeviceCall {
    Read(DeviceAccess),
    Write(DeviceAccess, u64),
}

type DeviceJournal = Arc<Mutex<Vec<DeviceCall>>>;

struct JournalMmio(DeviceJournal);

impl BusDevice for JournalMmio {
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
        self.0.lock().unwrap().push(DeviceCall::Read(access));
        Ok(MMIO_VALUE)
    }

    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        self.0
            .lock()
            .unwrap()
            .push(DeviceCall::Write(access, value));
        Ok(())
    }
}

/// The fetch layout with the data leaf mapped onto a counting MMIO device.
fn make_mmio_fetch_bus(journal: &DeviceJournal) -> MachineBus {
    let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), FETCH_RAM_BYTES).unwrap();
    let mmio = GuestPhysicalRange::new(GuestPhysicalAddress::new(MMIO_BASE), 0x1000).unwrap();
    let map = MachineMemoryMap::new(
        36,
        vec![
            MemoryRegionSpec::new(ram, MemoryRegionKind::Ram),
            MemoryRegionSpec::new(mmio, MemoryRegionKind::Mmio),
        ],
    )
    .unwrap();
    let mut bus = MachineBus::new(
        map,
        RamLimits::new(FETCH_RAM_BYTES),
        vec![Box::new(JournalMmio(Arc::clone(journal)))],
        vec![MmioBinding {
            region_start: GuestPhysicalAddress::new(MMIO_BASE),
            device_index: 0,
        }],
        vec![],
    )
    .unwrap();
    populate_fetch_memory(&mut bus, FetchLayout::CLEAN);
    put_qword(&mut bus, 0x7000, MMIO_BASE | 0x7);
    bus
}

fn mmio_access(offset: u64) -> DeviceAccess {
    DeviceAccess {
        space: IoSpace::Mmio,
        offset,
        width: AccessWidth::Qword,
    }
}

/// State and RAM compared between the dispatcher and a pure interpreter step.
#[derive(Debug, PartialEq)]
struct StepSnapshot {
    gprs: [u64; 16],
    rip: u64,
    rflags: u64,
    writes: Vec<(u64, u64)>,
    ram: Vec<u64>,
}

fn snapshot(
    state: &LongModeState,
    writes: &[GuestPhysicalRange],
    bus: &mut MachineBus,
) -> StepSnapshot {
    StepSnapshot {
        gprs: state.gpr,
        rip: state.rip,
        rflags: state.rflags & RFLAGS_DEFINED,
        writes: range_pairs(writes),
        ram: ram_image(bus),
    }
}

fn pure_interpreter(
    bus: &mut MachineBus,
    state: &mut LongModeState,
    privilege: PrivilegeLevel,
) -> (od_machine::PagedMovStepReport, StepSnapshot) {
    let mut writes = Vec::new();
    let report = od_machine::step_paged_mov(
        state,
        bus,
        CR3,
        EFER,
        fetch_context(privilege),
        RFLAGS_DEFINED,
        |range| writes.push(range),
    )
    .unwrap();
    let snapshot = snapshot(state, &writes, bus);
    (report, snapshot)
}

/// Runs the dispatcher on `bus` and a pure interpreter step on the identical
/// `oracle_bus`, asserts they agree on registers, RIP, RFLAGS, PageFaultRecord,
/// the full ordered committed-write list and the whole RAM image, and returns
/// the dispatcher outcome for path-specific assertions.
fn dispatch_equals_interpreter(
    label: &str,
    session: &mut od_host::PagedFetchedLoadSession,
    mut oracle_bus: MachineBus,
    bus: &mut MachineBus,
    privilege: PrivilegeLevel,
    prepare: impl Fn(&mut LongModeState),
) -> (od_host::PagedStepOutcome, LongModeState) {
    let mut oracle_state = fetch_state(CODE_RIP);
    prepare(&mut oracle_state);
    let (oracle_report, oracle_snapshot) =
        pure_interpreter(&mut oracle_bus, &mut oracle_state, privilege);

    let mut state = fetch_state(CODE_RIP);
    prepare(&mut state);
    let outcome = session
        .execute_or_interpret(
            &mut state,
            bus,
            CR3,
            EFER,
            fetch_context(privilege),
            RFLAGS_DEFINED,
        )
        .unwrap_or_else(|error| panic!("{label}: dispatcher error {error:?}"));
    let writes = match &outcome {
        od_host::PagedStepOutcome::Jit(report) => {
            let execution = report.execution.callback_result.as_ref().unwrap();
            // The JIT result must mean exactly what the interpreter reported.
            match &execution.exit {
                MemoryOperationExecutionExitV2::Complete => {
                    assert_eq!(
                        oracle_report.cpu.exit,
                        od_isa::execute::RunExit::StopAddress,
                        "{label}"
                    );
                    assert_eq!(oracle_report.page_fault, None, "{label}");
                }
                MemoryOperationExecutionExitV2::GuestFault(fault) => {
                    assert_eq!(
                        oracle_report.cpu.exit,
                        od_isa::execute::RunExit::Fault(fault.clone()),
                        "{label}"
                    );
                    assert_eq!(
                        report.execution.page_fault, oracle_report.page_fault,
                        "{label}"
                    );
                }
                other => panic!("{label}: unexpected JIT exit {other:?}"),
            }
            assert_eq!(
                execution.retired_instructions, oracle_report.cpu.instructions_executed,
                "{label}"
            );
            let mut writes = report.fetch_committed_writes.clone();
            writes.extend(report.execution.committed_writes.iter().copied());
            writes
        }
        od_host::PagedStepOutcome::Interpreter {
            report,
            committed_writes,
            ..
        } => {
            // Whole report: CPU result, retained PageFaultRecord, memory error.
            assert_eq!(report, &oracle_report, "{label}");
            committed_writes.clone()
        }
    };
    assert_eq!(
        snapshot(&state, &writes, bus),
        oracle_snapshot,
        "{label}: dispatcher == pure interpreter"
    );
    // The whole CPU state, not only the masked RFLAGS inside the snapshot.
    assert_eq!(state, oracle_state, "{label}: full LongModeState");
    (outcome, state)
}

#[test]
fn s4_15c_supported_form_stays_on_the_jit_and_equals_the_interpreter() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for (label, layout, privilege) in [
        ("success CPL0", FetchLayout::CLEAN, PrivilegeLevel::Ring0),
        ("success CPL3", FetchLayout::CLEAN, PrivilegeLevel::Ring3),
        (
            // A data #PF is an executed JIT result: it is delivered as is and
            // the interpreter must not re-run it.
            "data #PF CPL0",
            FetchLayout {
                data_leaf_present: false,
                ..FetchLayout::CLEAN
            },
            PrivilegeLevel::Ring0,
        ),
    ] {
        let mut session = od_host::PagedFetchedLoadSession::new();
        let mut bus = make_fetch_bus(layout);
        let (outcome, state) = dispatch_equals_interpreter(
            label,
            &mut session,
            make_fetch_bus(layout),
            &mut bus,
            privilege,
            |_| {},
        );
        assert_eq!(outcome.path(), od_host::PagedStepPath::Jit, "{label}");
        assert_eq!(session.admissions(), 1, "{label}");
        if layout.data_leaf_present {
            assert_eq!(state.gpr[Register::Rax as usize], LOADED_VALUE, "{label}");
            assert_eq!(state.rip, CODE_RIP + 3, "{label}");
        } else {
            assert_eq!(state.gpr[Register::Rax as usize], INITIAL_RAX, "{label}");
            assert_eq!(state.rip, CODE_RIP, "{label}");
            let od_host::PagedStepOutcome::Jit(report) = &outcome else {
                unreachable!();
            };
            let record = report.execution.page_fault.as_ref().unwrap();
            assert_eq!(record.fault().error_code, Some(0), "{label}");
            assert_eq!(record.cr2_value(), DATA_LINEAR, "{label}");
        }
        // Code and data walk Accessed bits are committed once each.
        for address in CODE_WALK_ENTRIES.into_iter().chain(DATA_WALK_ENTRIES) {
            let present_chain = layout.data_leaf_present || address != 0x7000;
            if present_chain {
                assert_ne!(
                    entry(&mut bus, address) & ACCESSED,
                    0,
                    "{label}: {address:#x}"
                );
            }
        }
    }
}

#[test]
fn s4_15c_fetch_fault_is_the_interpreters_page_fault_with_the_fetch_writes_kept() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // NX code leaf at CPL0: the JIT fetch walk commits the code A bits, then
    // faults. The interpreter re-walks (idempotent) and reports the same #PF.
    let layout = FetchLayout {
        code_leaf_flags: 0x27 | NX,
        ..FetchLayout::CLEAN
    };
    let mut session = od_host::PagedFetchedLoadSession::new();
    let mut bus = make_fetch_bus(layout);
    let (outcome, state) = dispatch_equals_interpreter(
        "NX fetch",
        &mut session,
        make_fetch_bus(layout),
        &mut bus,
        PrivilegeLevel::Ring0,
        |_| {},
    );
    let od_host::PagedStepOutcome::Interpreter {
        reason,
        report,
        committed_writes,
    } = &outcome
    else {
        panic!("a fetch #PF falls back to the interpreter, got {outcome:?}");
    };
    // The JIT-side fetch refusal carries the architectural record itself; it
    // must be the very record the interpreter reports for the same walk.
    let od_host::PagedStepFallbackReason::Refused(od_host::PagedFetchedLoadErrorKind::Fetch(
        od_machine::TrackedCodeSnapshotError::Access(od_machine::TrackedAccessError::PageFault {
            record: jit_record,
            ..
        }),
    )) = reason
    else {
        panic!("expected a JIT fetch page-fault refusal, got {reason:?}");
    };
    let record = report.page_fault.as_ref().unwrap();
    assert_eq!(jit_record, record, "JIT fetch #PF == interpreter #PF");
    assert_eq!(jit_record.fault().vector, 14);
    assert_eq!(jit_record.fault().error_code, Some(0x11));
    assert_eq!(jit_record.cr2_value(), CODE_RIP);
    assert_eq!(jit_record.saved_rip(), CODE_RIP);
    assert_eq!(jit_record.access(), od_isa::memory::AccessKind::Fetch);
    assert_eq!(jit_record.context(), fetch_context(PrivilegeLevel::Ring0));
    assert_eq!(record.fault().error_code, Some(0x11));
    assert_eq!(record.cr2_value(), CODE_RIP);
    assert_eq!(state.gpr[Register::Rax as usize], INITIAL_RAX);
    assert_eq!(state.rip, CODE_RIP);
    // Each fetch-walk entry is written once even though two walks happened.
    // The NX violation is found at the leaf before its Accessed bit is
    // written: only the three upper entries are committed, once each.
    assert_eq!(
        range_pairs(committed_writes),
        CODE_WALK_RANGES[..3]
            .iter()
            .map(|&(a, l)| (a, l as u64))
            .collect::<Vec<_>>()
    );
    // The operand walk never ran.
    for address in DATA_WALK_ENTRIES {
        assert_eq!(entry(&mut bus, address) & ACCESSED, 0);
    }
}

#[test]
fn s4_15c_mmio_operand_reaches_the_device_exactly_once_via_the_interpreter() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let offset = DATA_LINEAR & 0xfff;

    // The JIT alone refuses the MMIO operand: its helper reports a memory
    // error, the CPU state is untouched and the device saw nothing.
    let attempt_journal = DeviceJournal::default();
    let mut attempt_bus = make_mmio_fetch_bus(&attempt_journal);
    let mut attempt_session = od_host::PagedFetchedLoadSession::new();
    let mut attempt_state = fetch_state(CODE_RIP);
    let before = attempt_state.clone();
    let attempt = attempt_session
        .execute(
            &mut attempt_state,
            &mut attempt_bus,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
        )
        .unwrap();
    assert!(
        matches!(
            attempt.execution.callback_result,
            Err(od_host::MemoryOperationExecutionErrorV2::MemoryError(_))
        ),
        "{:?}",
        attempt.execution.callback_result
    );
    assert_eq!(attempt_state, before);
    assert!(
        attempt_journal.lock().unwrap().is_empty(),
        "JIT: 0 device calls"
    );

    // A re-walk by the interpreter is idempotent for Accessed bits: whatever
    // the refused JIT data walk committed is not committed again, RAM does not
    // change, and the device is read exactly once.
    let jit_data_writes = range_pairs(&attempt.execution.committed_writes);
    // The RAM-only data walk commits its four Accessed bits before the adapter
    // refuses the non-RAM leaf target.
    assert_eq!(jit_data_writes, DATA_WALK_RANGES.to_vec());
    let ram_after_attempt = ram_image(&mut attempt_bus);
    let mut interpreter_writes = Vec::new();
    let report = od_machine::step_paged_mov(
        &mut attempt_state,
        &mut attempt_bus,
        CR3,
        EFER,
        fetch_context(PrivilegeLevel::Ring0),
        RFLAGS_DEFINED,
        |range| interpreter_writes.push(range),
    )
    .unwrap();
    assert_eq!(report.cpu.exit, od_isa::execute::RunExit::StopAddress);
    // Fetch and data entries already carry A: the interpreter commits nothing.
    assert!(
        interpreter_writes.is_empty(),
        "re-walk committed {interpreter_writes:?} a second time"
    );
    assert_eq!(
        *attempt_journal.lock().unwrap(),
        [DeviceCall::Read(mmio_access(offset))]
    );
    assert_eq!(attempt_state.gpr[Register::Rax as usize], MMIO_VALUE);
    assert_eq!(ram_image(&mut attempt_bus), ram_after_attempt);
    // The host has one executable region: release the attempt's block first.
    drop(attempt_session);

    // The dispatcher on a fresh bus: one device call in total, path
    // Interpreter, everything equal to a pure interpreter step.
    let journal = DeviceJournal::default();
    let oracle_journal = DeviceJournal::default();
    let mut bus = make_mmio_fetch_bus(&journal);
    let mut session = od_host::PagedFetchedLoadSession::new();
    let (outcome, state) = dispatch_equals_interpreter(
        "MMIO operand",
        &mut session,
        make_mmio_fetch_bus(&oracle_journal),
        &mut bus,
        PrivilegeLevel::Ring0,
        |_| {},
    );
    assert_eq!(outcome.path(), od_host::PagedStepPath::Interpreter);
    let od_host::PagedStepOutcome::Interpreter {
        reason,
        committed_writes,
        ..
    } = &outcome
    else {
        unreachable!();
    };
    assert!(
        matches!(
            reason,
            od_host::PagedStepFallbackReason::OperandMemoryError(_)
        ),
        "{reason:?}"
    );
    assert_eq!(
        *journal.lock().unwrap(),
        [DeviceCall::Read(mmio_access(offset))],
        "dispatcher: exactly 1 device call"
    );
    assert_eq!(*oracle_journal.lock().unwrap(), *journal.lock().unwrap());
    assert_eq!(state.gpr[Register::Rax as usize], MMIO_VALUE);
    assert_eq!(state.rip, CODE_RIP + 3);
    // Code walk then data walk, each entry once (no duplicate from the re-walk).
    let expected_writes: Vec<(u64, u64)> = CODE_WALK_RANGES
        .into_iter()
        .map(|(a, l)| (a, l as u64))
        .chain(DATA_WALK_RANGES)
        .collect();
    assert_eq!(range_pairs(committed_writes), expected_writes);
    for address in CODE_WALK_ENTRIES.into_iter().chain(DATA_WALK_ENTRIES) {
        let value = entry(&mut bus, address);
        assert_ne!(value & ACCESSED, 0, "A set at {address:#x}");
        assert_eq!(value & DIRTY, 0, "read must not set D at {address:#x}");
    }
}

#[test]
fn s4_15c_cross_page_operand_is_refused_by_the_jit_before_the_walk_and_interpreted() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Qword at page offset 0xffc: 4 bytes in the data page, 4 in the next one.
    let cross_linear = (DATA_LINEAR & !0xfff) + 0xffc;
    let prepare_memory = |bus: &mut MachineBus| {
        put_qword(bus, 0x7008, CODE_PHYSICAL_2 | 0x7);
        bus.write_physical(
            GuestPhysicalAddress::new(DATA_PHYSICAL + 0xffc),
            AccessWidth::Dword,
            0x1122_3344,
            |_| {},
        )
        .unwrap();
    };
    let mut oracle_bus = make_fetch_bus(FetchLayout::CLEAN);
    prepare_memory(&mut oracle_bus);
    let mut bus = make_fetch_bus(FetchLayout::CLEAN);
    prepare_memory(&mut bus);
    let mut session = od_host::PagedFetchedLoadSession::new();
    let (outcome, state) = dispatch_equals_interpreter(
        "cross-page operand",
        &mut session,
        oracle_bus,
        &mut bus,
        PrivilegeLevel::Ring0,
        |state| state.gpr[Register::Rbx as usize] = cross_linear,
    );
    let od_host::PagedStepOutcome::Interpreter {
        reason,
        report,
        committed_writes,
    } = &outcome
    else {
        panic!("cross-page operand must be interpreted, got {outcome:?}");
    };
    assert_eq!(
        *reason,
        od_host::PagedStepFallbackReason::Refused(od_host::PagedFetchedLoadErrorKind::Load(
            od_host::PagedMemoryLoadV2Error::UnsupportedOperandRange {
                address: cross_linear,
                length: 8
            }
        ))
    );
    assert_eq!(report.cpu.exit, od_isa::execute::RunExit::StopAddress);
    // Low half from the data page, high half from the first code bytes
    // `48 8B 03 00` of the second page.
    assert_eq!(state.gpr[Register::Rax as usize], 0x0003_8b48_1122_3344);
    assert_eq!(state.rip, CODE_RIP + 3);
    // The JIT refused before any fetch: nothing admitted, and the whole walk
    // (code fetch plus both data pages) is the interpreter's single execution.
    assert!(!session.has_admitted_block());
    assert_eq!(session.admissions(), 0);
    let writes = range_pairs(committed_writes);
    assert!(writes.starts_with(&CODE_WALK_RANGES.map(|(a, l)| (a, l as u64))));
    let mut unique = writes.clone();
    unique.sort_unstable();
    unique.dedup();
    // The shared upper data entries appear once per walk that changes them.
    assert!(unique.len() >= 8, "{writes:?}");
    assert_eq!(entry(&mut bus, 0x7008) & ACCESSED, ACCESSED);
}

#[test]
fn s4_15c_unsupported_forms_are_interpreted_once_or_reported_unsupported() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let patch = |bus: &mut MachineBus, bytes: &[u8]| {
        bus.patch_physical_ram(GuestPhysicalAddress::new(CODE_PHYSICAL), bytes)
            .unwrap();
    };

    // NOP NOP NOP: the interpreter's allowlist is only `48 8B 03` / `48 89 0B`,
    // so the fallback ends in its explicit Unsupported exit, nothing retires.
    let mut session = od_host::PagedFetchedLoadSession::new();
    let mut oracle_bus = make_fetch_bus(FetchLayout::CLEAN);
    patch(&mut oracle_bus, &[0x90, 0x90, 0x90]);
    let mut bus = make_fetch_bus(FetchLayout::CLEAN);
    patch(&mut bus, &[0x90, 0x90, 0x90]);
    let (outcome, state) = dispatch_equals_interpreter(
        "NOP",
        &mut session,
        oracle_bus,
        &mut bus,
        PrivilegeLevel::Ring0,
        |_| {},
    );
    let od_host::PagedStepOutcome::Interpreter { reason, report, .. } = &outcome else {
        panic!("NOP must be interpreted, got {outcome:?}");
    };
    assert_eq!(
        *reason,
        od_host::PagedStepFallbackReason::Refused(
            od_host::PagedFetchedLoadErrorKind::UnsupportedForm {
                bytes: vec![0x90, 0x90, 0x90]
            }
        )
    );
    assert!(
        matches!(
            report.cpu.exit,
            od_isa::execute::RunExit::Unsupported { .. }
        ),
        "{:?}",
        report.cpu.exit
    );
    assert_eq!(report.cpu.instructions_executed, 0);
    assert_eq!(state.rip, CODE_RIP);
    assert_eq!(state.gpr[Register::Rax as usize], INITIAL_RAX);
    assert_eq!(session.admissions(), 0);

    // MOV RCX,[RBX] (`48 8B 0B`) is a JIT-compilable form the interpreter
    // profile does not allow: still one interpreter run, Unsupported, no effects.
    let mut session = od_host::PagedFetchedLoadSession::new();
    let mut oracle_bus = make_fetch_bus(FetchLayout::CLEAN);
    patch(&mut oracle_bus, &[0x48, 0x8b, 0x0b]);
    let mut bus = make_fetch_bus(FetchLayout::CLEAN);
    patch(&mut bus, &[0x48, 0x8b, 0x0b]);
    let (outcome, state) = dispatch_equals_interpreter(
        "MOV RCX,[RBX]",
        &mut session,
        oracle_bus,
        &mut bus,
        PrivilegeLevel::Ring0,
        |_| {},
    );
    let od_host::PagedStepOutcome::Interpreter { report, .. } = &outcome else {
        panic!("unsupported form must be interpreted, got {outcome:?}");
    };
    assert!(matches!(
        report.cpu.exit,
        od_isa::execute::RunExit::Unsupported { .. }
    ));
    assert_eq!(state.gpr[Register::Rcx as usize], 0);
    assert_eq!(state.rip, CODE_RIP);

    // MOV [RBX],RCX (`48 89 0B`): not a JIT form here (the store is S4-15d),
    // but the interpreter implements it, so the dispatcher executes the store
    // once through it: the data qword changes and the dirty bit is set.
    let store_value = 0x1357_9bdf_0246_8ace;
    let mut session = od_host::PagedFetchedLoadSession::new();
    let mut oracle_bus = make_fetch_bus(FetchLayout::CLEAN);
    patch(&mut oracle_bus, &[0x48, 0x89, 0x0b]);
    let mut bus = make_fetch_bus(FetchLayout::CLEAN);
    patch(&mut bus, &[0x48, 0x89, 0x0b]);
    let (outcome, state) = dispatch_equals_interpreter(
        "MOV [RBX],RCX",
        &mut session,
        oracle_bus,
        &mut bus,
        PrivilegeLevel::Ring0,
        |state| state.gpr[Register::Rcx as usize] = store_value,
    );
    assert_eq!(outcome.path(), od_host::PagedStepPath::Interpreter);
    assert_eq!(state.rip, CODE_RIP + 3);
    assert_eq!(entry(&mut bus, DATA_PHYSICAL + 0x120), store_value);
    assert_ne!(
        entry(&mut bus, 0x7000) & DIRTY,
        0,
        "store sets D on the leaf"
    );
    assert_eq!(state.gpr[Register::Rax as usize], INITIAL_RAX);

    // The same store aimed at the MMIO window: the device sees exactly one
    // write, so the step was not executed twice (a RAM store alone is
    // idempotent and could not show that).
    let journal = DeviceJournal::default();
    let oracle_journal = DeviceJournal::default();
    let mut session = od_host::PagedFetchedLoadSession::new();
    let mut oracle_bus = make_mmio_fetch_bus(&oracle_journal);
    patch(&mut oracle_bus, &[0x48, 0x89, 0x0b]);
    let mut bus = make_mmio_fetch_bus(&journal);
    patch(&mut bus, &[0x48, 0x89, 0x0b]);
    let (outcome, _) = dispatch_equals_interpreter(
        "MOV [RBX],RCX to MMIO",
        &mut session,
        oracle_bus,
        &mut bus,
        PrivilegeLevel::Ring0,
        |state| state.gpr[Register::Rcx as usize] = store_value,
    );
    assert_eq!(outcome.path(), od_host::PagedStepPath::Interpreter);
    assert_eq!(
        *journal.lock().unwrap(),
        [DeviceCall::Write(
            mmio_access(DATA_LINEAR & 0xfff),
            store_value
        )]
    );
    assert_eq!(*oracle_journal.lock().unwrap(), *journal.lock().unwrap());
}

#[test]
fn s4_15c_invalid_rflags_definedness_is_refused_before_any_effect() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut session = od_host::PagedFetchedLoadSession::new();
    let mut bus = make_fetch_bus(FetchLayout::CLEAN);
    let ram_before = ram_image(&mut bus);
    let mut state = fetch_state(CODE_RIP);
    let before = state.clone();
    let invalid = !od_isa::INTEGER_FLAGS | 1;
    let error = session
        .execute_or_interpret(
            &mut state,
            &mut bus,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
            invalid,
        )
        .unwrap_err();
    assert_eq!(
        error,
        od_host::PagedStepError::Interpreter(od_machine::PagedMovStepError::CpuInput(
            od_isa::execute::RunInputError::InvalidRflagsDefined { value: invalid }
        ))
    );
    // Neither the fetch walk's Accessed bits nor the registers changed.
    assert_eq!(state, before);
    assert_eq!(ram_image(&mut bus), ram_before);
    assert_eq!(session.admissions(), 0);
}

#[test]
fn s4_15c_publish_refusal_falls_back_to_the_interpreter() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // The host owns one executable region. While another session's block is
    // live the publish is refused after the fetch walk committed its Accessed
    // bits; the step still completes, once, on the interpreter.
    let mut holder = od_host::PagedFetchedLoadSession::new();
    let mut holder_bus = make_fetch_bus(FetchLayout::CLEAN);
    let mut holder_state = fetch_state(CODE_RIP);
    holder
        .execute(
            &mut holder_state,
            &mut holder_bus,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
        )
        .unwrap();
    assert!(holder.has_admitted_block());

    let mut session = od_host::PagedFetchedLoadSession::new();
    let mut bus = make_fetch_bus(FetchLayout::CLEAN);
    let (outcome, state) = dispatch_equals_interpreter(
        "publish refused",
        &mut session,
        make_fetch_bus(FetchLayout::CLEAN),
        &mut bus,
        PrivilegeLevel::Ring0,
        |_| {},
    );
    let od_host::PagedStepOutcome::Interpreter {
        reason,
        committed_writes,
        ..
    } = &outcome
    else {
        panic!("publish refusal must fall back, got {outcome:?}");
    };
    assert_eq!(
        reason,
        &od_host::PagedStepFallbackReason::Refused(od_host::PagedFetchedLoadErrorKind::Publish(
            od_host::ExecutableMemoryError::MappingAlreadyLive
        ))
    );
    assert_eq!(state.gpr[Register::Rax as usize], LOADED_VALUE);
    assert_eq!(state.rip, CODE_RIP + 3);
    // Code walk (committed by the refused attempt) then data walk, each once.
    let expected: Vec<(u64, u64)> = CODE_WALK_RANGES
        .into_iter()
        .map(|(a, l)| (a, l as u64))
        .chain(DATA_WALK_RANGES)
        .collect();
    assert_eq!(range_pairs(committed_writes), expected);
    assert!(!session.has_admitted_block());
}

// ---------------------------------------------------------------------------
// S4-15d: `48 89 03` (MOV [RBX],RAX) on the JIT path of the dispatcher, through
// the tracked store adapter. A guest #PF is a final JIT result (like S4-15c);
// a refusal that changed nothing (non-RAM target, the block's own code page)
// falls back to the interpreter exactly once. The interpreter's store
// allowlist is `48 89 0B` (MOV [RBX],RCX), not `48 89 03`, so that fallback
// reports Unsupported: the refused store is not executed by either engine.
// ---------------------------------------------------------------------------

const STORE_CODE: [u8; 3] = [0x48, 0x89, 0x03];
/// The same store with RCX as the source: what the interpreter implements.
const STORE_RCX_CODE: [u8; 3] = [0x48, 0x89, 0x0b];
/// Data qword the store targets: leaf 0x7000 maps `DATA_LINEAR` to this address.
const DATA_QWORD: u64 = DATA_PHYSICAL + 0x120;
/// Second data page (PT entry 0x7008): `DATA_LINEAR`'s page plus 4 KiB.
const DATA2_LINEAR: u64 = 0x0000_0080_0000_1120;
const DATA_LEAF_ADDRESS: u64 = 0x7000;
const DATA2_LEAF_ADDRESS: u64 = 0x7008;

/// A fetch-layout bus whose code at `CODE_RIP` is `MOV [RBX],RAX` and whose
/// data leaf carries exactly `data_leaf_flags` (0x7 = present, writable, user).
fn make_store_bus(data_leaf_flags: u64) -> MachineBus {
    let mut bus = make_fetch_bus(FetchLayout::CLEAN);
    prepare_store_bus(&mut bus, data_leaf_flags);
    bus
}

fn prepare_store_bus(bus: &mut MachineBus, data_leaf_flags: u64) {
    bus.patch_physical_ram(GuestPhysicalAddress::new(CODE_PHYSICAL), &STORE_CODE)
        .unwrap();
    put_qword(bus, DATA_LEAF_ADDRESS, DATA_PHYSICAL | data_leaf_flags);
}

/// Runs the dispatcher on `bus` for an executed JIT store of `MOV [RBX],RAX` and
/// asserts it equals the interpreter's `MOV [RBX],RCX` (same store, source
/// register RCX = RAX) on identical RAM: registers, RIP, full CPU state,
/// PageFaultRecord, the ordered committed writes and the whole RAM image (the
/// oracle's code byte is restored first). The JIT result must be final.
fn store_equals_rcx_interpreter(
    label: &str,
    session: &mut od_host::PagedFetchedLoadSession,
    make_bus: &dyn Fn() -> MachineBus,
    bus: &mut MachineBus,
    privilege: PrivilegeLevel,
    prepare: impl Fn(&mut LongModeState),
) -> (od_host::PagedStepOutcome, LongModeState) {
    let mut oracle_bus = make_bus();
    oracle_bus
        .patch_physical_ram(GuestPhysicalAddress::new(CODE_PHYSICAL), &STORE_RCX_CODE)
        .unwrap();
    let init = |state: &mut LongModeState| {
        prepare(state);
        state.gpr[Register::Rcx as usize] = state.gpr[Register::Rax as usize];
    };
    let mut oracle_state = fetch_state(CODE_RIP);
    init(&mut oracle_state);
    let (oracle_report, _) = pure_interpreter(&mut oracle_bus, &mut oracle_state, privilege);
    oracle_bus
        .patch_physical_ram(GuestPhysicalAddress::new(CODE_PHYSICAL), &STORE_CODE)
        .unwrap();

    let mut state = fetch_state(CODE_RIP);
    init(&mut state);
    let outcome = session
        .execute_or_interpret(
            &mut state,
            bus,
            CR3,
            EFER,
            fetch_context(privilege),
            RFLAGS_DEFINED,
        )
        .unwrap_or_else(|error| panic!("{label}: dispatcher error {error:?}"));
    let od_host::PagedStepOutcome::Jit(report) = &outcome else {
        panic!("{label}: expected the JIT to execute the store, got {outcome:?}");
    };
    let execution = report.execution.callback_result.as_ref().unwrap();
    match &execution.exit {
        MemoryOperationExecutionExitV2::Complete => {
            assert_eq!(
                oracle_report.cpu.exit,
                od_isa::execute::RunExit::StopAddress,
                "{label}"
            );
            assert_eq!(oracle_report.page_fault, None, "{label}");
        }
        MemoryOperationExecutionExitV2::GuestFault(fault) => {
            assert_eq!(
                oracle_report.cpu.exit,
                od_isa::execute::RunExit::Fault(fault.clone()),
                "{label}"
            );
            assert_eq!(
                report.execution.page_fault, oracle_report.page_fault,
                "{label}"
            );
        }
        other => panic!("{label}: unexpected JIT exit {other:?}"),
    }
    assert_eq!(
        execution.retired_instructions, oracle_report.cpu.instructions_executed,
        "{label}"
    );
    assert_eq!(report.store_refusal, None, "{label}");
    let writes = jit_writes(&outcome);
    // The oracle's own write list, recomputed on the same oracle step.
    let mut oracle_bus = make_bus();
    oracle_bus
        .patch_physical_ram(GuestPhysicalAddress::new(CODE_PHYSICAL), &STORE_RCX_CODE)
        .unwrap();
    let mut replay_state = fetch_state(CODE_RIP);
    init(&mut replay_state);
    let mut oracle_writes = Vec::new();
    od_machine::step_paged_mov(
        &mut replay_state,
        &mut oracle_bus,
        CR3,
        EFER,
        fetch_context(privilege),
        RFLAGS_DEFINED,
        |range| oracle_writes.push(range),
    )
    .unwrap();
    oracle_bus
        .patch_physical_ram(GuestPhysicalAddress::new(CODE_PHYSICAL), &STORE_CODE)
        .unwrap();
    assert_eq!(
        writes,
        range_pairs(&oracle_writes),
        "{label}: committed writes"
    );
    assert_eq!(
        ram_image(bus),
        ram_image(&mut oracle_bus),
        "{label}: RAM image"
    );
    assert_eq!(state, oracle_state, "{label}: full LongModeState");
    (outcome, state)
}

fn store_page_fault(outcome: &od_host::PagedStepOutcome) -> &od_isa::memory::PageFaultRecord {
    let od_host::PagedStepOutcome::Jit(report) = outcome else {
        panic!("a store #PF is a final JIT result, got {outcome:?}");
    };
    let execution = report.execution.callback_result.as_ref().unwrap();
    assert!(
        matches!(
            execution.exit,
            MemoryOperationExecutionExitV2::GuestFault(_)
        ),
        "{:?}",
        execution.exit
    );
    report.execution.page_fault.as_ref().unwrap()
}

fn jit_writes(outcome: &od_host::PagedStepOutcome) -> Vec<(u64, u64)> {
    let od_host::PagedStepOutcome::Jit(report) = outcome else {
        panic!("expected a JIT outcome, got {outcome:?}");
    };
    let mut writes = range_pairs(&report.fetch_committed_writes);
    writes.extend(range_pairs(&report.execution.committed_writes));
    writes
}

#[test]
fn s4_15d_store_on_ram_equals_the_interpreter_and_sets_the_dirty_bit() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for (label, leaf_flags, privilege) in [
        ("CPL0 user leaf", 0x7, PrivilegeLevel::Ring0),
        ("CPL3 user leaf", 0x7, PrivilegeLevel::Ring3),
        // Supervisor pages are writable at CPL0: U/S only restricts CPL3.
        ("CPL0 supervisor leaf", 0x3, PrivilegeLevel::Ring0),
        // NX is an instruction-fetch right: it must not affect a data write.
        ("CPL0 NX data leaf", 0x7 | NX, PrivilegeLevel::Ring0),
        // A Dirty bit that is already set is not written again.
        ("CPL0 leaf with D preset", 0x47, PrivilegeLevel::Ring0),
    ] {
        let mut session = od_host::PagedFetchedLoadSession::new();
        let mut bus = make_store_bus(leaf_flags);
        let (outcome, state) = store_equals_rcx_interpreter(
            label,
            &mut session,
            &|| make_store_bus(leaf_flags),
            &mut bus,
            privilege,
            |_| {},
        );
        let od_host::PagedStepOutcome::Jit(report) = &outcome else {
            unreachable!();
        };
        assert_eq!(report.fetched_bytes, STORE_CODE, "{label}");
        assert_eq!(report.admission, od_host::PagedFetchAdmission::Admitted);
        assert_eq!(session.admissions(), 1, "{label}");
        // The operand qword is RAX; RAX, RIP and the data are as the oracle
        // has them, and the store retired once: the following
        // `mov [rbx],rcx` (RCX = RAX here, so also distinguishable by RIP)
        // did not run.
        assert_eq!(entry(&mut bus, DATA_QWORD), INITIAL_RAX, "{label}");
        assert_eq!(state.gpr[Register::Rax as usize], INITIAL_RAX, "{label}");
        assert_eq!(state.rip, CODE_RIP + 3, "{label}");
        // Order: code walk, data walk (Accessed), Dirty on the leaf, operand.
        let writes = jit_writes(&outcome);
        assert_eq!(writes.last(), Some(&(DATA_QWORD, 8)), "{label}");
        let leaf_writes = writes
            .iter()
            .filter(|&&(address, _)| address == DATA_LEAF_ADDRESS)
            .count();
        assert_eq!(
            leaf_writes,
            if leaf_flags & DIRTY != 0 { 1 } else { 2 },
            "{label}"
        );
        // Dirty is set on the data leaf by the write, never on the code leaf
        // (the fetch is a read) and never on an upper level.
        assert_ne!(entry(&mut bus, DATA_LEAF_ADDRESS) & DIRTY, 0, "{label}");
        for address in CODE_WALK_ENTRIES
            .into_iter()
            .chain(DATA_WALK_ENTRIES[..3].iter().copied())
        {
            assert_eq!(entry(&mut bus, address) & DIRTY, 0, "{label}: {address:#x}");
        }
        for address in CODE_WALK_ENTRIES.into_iter().chain(DATA_WALK_ENTRIES) {
            assert_ne!(
                entry(&mut bus, address) & ACCESSED,
                0,
                "{label}: {address:#x}"
            );
        }

        // The same bytes at the same generation reuse the block: the store
        // wrote no code page, so nothing invalidated it.
        let mut again = fetch_state(CODE_RIP);
        again.gpr[Register::Rax as usize] = 0x1111_2222_3333_4444;
        let second = session
            .execute_or_interpret(
                &mut again,
                &mut bus,
                CR3,
                EFER,
                fetch_context(privilege),
                RFLAGS_DEFINED,
            )
            .unwrap();
        let od_host::PagedStepOutcome::Jit(second) = second else {
            panic!("{label}: second store must stay on the JIT");
        };
        assert_eq!(
            second.admission,
            od_host::PagedFetchAdmission::Reused,
            "{label}"
        );
        assert_eq!(
            entry(&mut bus, DATA_QWORD),
            0x1111_2222_3333_4444,
            "{label}"
        );
        assert_eq!(session.admissions(), 1, "{label}");
    }
}

#[test]
fn s4_15d_store_faults_precede_the_write_and_equal_the_interpreter() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // PFEC bits: P=0x1, W=0x2, U/S=0x4. The leaf is the only level that denies.
    for (label, leaf_flags, privilege, pfec) in [
        ("not-present leaf CPL0", 0x6, PrivilegeLevel::Ring0, 0x2u32),
        // CR0.WP is set: a read-only page faults even at CPL0.
        ("read-only leaf CPL0", 0x5, PrivilegeLevel::Ring0, 0x3),
        ("read-only leaf CPL3", 0x5, PrivilegeLevel::Ring3, 0x7),
        ("not-present leaf CPL3", 0x6, PrivilegeLevel::Ring3, 0x6),
    ] {
        let mut session = od_host::PagedFetchedLoadSession::new();
        let mut bus = make_store_bus(leaf_flags);
        let ram_before = ram_image(&mut bus);
        let (outcome, state) = store_equals_rcx_interpreter(
            label,
            &mut session,
            &|| make_store_bus(leaf_flags),
            &mut bus,
            privilege,
            |_| {},
        );
        // Not re-run on the interpreter: the fault is the JIT's executed result.
        assert_eq!(outcome.path(), od_host::PagedStepPath::Jit, "{label}");
        let record = store_page_fault(&outcome);
        assert_eq!(record.fault().vector, 14, "{label}");
        assert_eq!(record.fault().error_code, Some(pfec), "{label}");
        assert_eq!(record.cr2_value(), DATA_LINEAR, "{label}");
        assert_eq!(record.saved_rip(), CODE_RIP, "{label}");
        assert_eq!(
            record.access(),
            od_isa::memory::AccessKind::Write,
            "{label}"
        );
        // Nothing was stored, nothing retired, no Dirty bit anywhere.
        assert_eq!(entry(&mut bus, DATA_QWORD), LOADED_VALUE, "{label}");
        assert_eq!(state.rip, CODE_RIP, "{label}");
        assert_eq!(state.gpr[Register::Rax as usize], INITIAL_RAX, "{label}");
        for address in CODE_WALK_ENTRIES.into_iter().chain(DATA_WALK_ENTRIES) {
            assert_eq!(entry(&mut bus, address) & DIRTY, 0, "{label}: {address:#x}");
        }
        // No committed write is the operand, and the only RAM words that
        // changed against the start are page-table Accessed bits.
        let writes = jit_writes(&outcome);
        assert!(!writes.contains(&(DATA_QWORD, 8)), "{label}: {writes:?}");
        let ram_after = ram_image(&mut bus);
        for (index, (before, after)) in ram_before.iter().zip(&ram_after).enumerate() {
            if before != after {
                let address = index as u64 * 8;
                assert!(
                    CODE_WALK_ENTRIES
                        .into_iter()
                        .chain(DATA_WALK_ENTRIES)
                        .any(|entry| entry == address)
                        && before | ACCESSED == *after,
                    "{label}: unexpected change at {address:#x}"
                );
            }
        }
    }
}

#[test]
fn s4_15d_cpl3_store_to_a_supervisor_page_faults_before_the_write() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Present and writable but U/S = 0: CPL3 may not touch it.
    let leaf_flags = 0x3;
    let mut session = od_host::PagedFetchedLoadSession::new();
    let mut bus = make_store_bus(leaf_flags);
    let (outcome, state) = store_equals_rcx_interpreter(
        "CPL3 supervisor leaf",
        &mut session,
        &|| make_store_bus(leaf_flags),
        &mut bus,
        PrivilegeLevel::Ring3,
        |_| {},
    );
    assert_eq!(outcome.path(), od_host::PagedStepPath::Jit);
    let record = store_page_fault(&outcome);
    // Protection violation of a user write: P | W | U.
    assert_eq!(record.fault().error_code, Some(0x7));
    assert_eq!(record.cr2_value(), DATA_LINEAR);
    assert_eq!(record.saved_rip(), CODE_RIP);
    assert_eq!(entry(&mut bus, DATA_QWORD), LOADED_VALUE);
    assert_eq!(entry(&mut bus, DATA_LEAF_ADDRESS) & DIRTY, 0);
    assert_eq!(state.rip, CODE_RIP);
    assert!(!jit_writes(&outcome).contains(&(DATA_QWORD, 8)));

    // The JIT attempt alone, without the dispatcher: same fault, store never
    // committed, and the block stays admitted for a later retry. (The host has
    // one executable region: release the dispatcher session's block first.)
    drop(session);
    let mut jit_only = od_host::PagedFetchedLoadSession::new();
    let mut bus = make_store_bus(leaf_flags);
    let mut state = fetch_state(CODE_RIP);
    let report = jit_only
        .execute_with_store(
            &mut state,
            &mut bus,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring3),
        )
        .unwrap();
    assert!(matches!(
        report.execution.callback_result.as_ref().unwrap().exit,
        MemoryOperationExecutionExitV2::GuestFault(_)
    ));
    assert_eq!(entry(&mut bus, DATA_QWORD), LOADED_VALUE);
    assert!(jit_only.has_admitted_block());
    // The S4-15b entry still refuses the store form: it was not widened.
    drop(jit_only);
    let mut legacy = od_host::PagedFetchedLoadSession::new();
    let mut bus = make_store_bus(0x7);
    let mut state = fetch_state(CODE_RIP);
    let error = legacy
        .execute(
            &mut state,
            &mut bus,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
        )
        .unwrap_err();
    assert_eq!(
        error.kind,
        od_host::PagedFetchedLoadErrorKind::UnsupportedForm {
            bytes: STORE_CODE.to_vec()
        }
    );
    assert_eq!(entry(&mut bus, DATA_QWORD), LOADED_VALUE);
}

#[test]
fn s4_15d_mmio_store_is_refused_before_any_effect_and_not_executed() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let offset = DATA_LINEAR & 0xfff;
    let make = |journal: &DeviceJournal| {
        let mut bus = make_mmio_fetch_bus(journal);
        bus.patch_physical_ram(GuestPhysicalAddress::new(CODE_PHYSICAL), &STORE_CODE)
            .unwrap();
        bus
    };

    // JIT attempt alone: the adapter refuses the non-RAM operand before it
    // walks, so not one data-walk bit changed and the device saw nothing.
    let attempt_journal = DeviceJournal::default();
    let mut attempt_bus = make(&attempt_journal);
    let mut attempt_session = od_host::PagedFetchedLoadSession::new();
    let mut attempt_state = fetch_state(CODE_RIP);
    let before = attempt_state.clone();
    let attempt = attempt_session
        .execute_with_store(
            &mut attempt_state,
            &mut attempt_bus,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
        )
        .unwrap();
    assert!(matches!(
        attempt.execution.callback_result,
        Err(od_host::MemoryOperationExecutionErrorV2::MemoryError(_))
    ));
    assert!(matches!(
        attempt.store_refusal,
        Some(od_machine::PagedStoreRefusal::NonRamTarget { physical })
            if physical.get() == MMIO_BASE + offset
    ));
    assert_eq!(attempt_state, before);
    assert!(attempt.execution.committed_writes.is_empty());
    assert!(
        attempt_journal.lock().unwrap().is_empty(),
        "JIT: 0 device calls"
    );
    for address in DATA_WALK_ENTRIES {
        assert_eq!(
            entry(&mut attempt_bus, address) & ACCESSED,
            0,
            "{address:#x}"
        );
    }
    drop(attempt_session);

    // The dispatcher equals a pure interpreter step on the same `48 89 03`
    // (Unsupported, nothing retired): a leaked JIT effect (an Accessed or
    // Dirty bit of the data walk, a device write) would differ from it.
    let journal = DeviceJournal::default();
    let oracle_journal = DeviceJournal::default();
    let mut bus = make(&journal);
    let mut session = od_host::PagedFetchedLoadSession::new();
    let (outcome, state) = dispatch_equals_interpreter(
        "MMIO store",
        &mut session,
        make(&oracle_journal),
        &mut bus,
        PrivilegeLevel::Ring0,
        |_| {},
    );
    let od_host::PagedStepOutcome::Interpreter {
        reason,
        report,
        committed_writes,
    } = &outcome
    else {
        panic!("an MMIO store must be interpreted, got {outcome:?}");
    };
    assert!(matches!(
        reason,
        od_host::PagedStepFallbackReason::StoreTargetRefused(
            od_machine::PagedStoreRefusal::NonRamTarget { .. }
        )
    ));
    assert!(matches!(
        report.cpu.exit,
        od_isa::execute::RunExit::Unsupported { .. }
    ));
    assert!(journal.lock().unwrap().is_empty(), "0 device calls");
    assert!(oracle_journal.lock().unwrap().is_empty());
    assert_eq!(state.rip, CODE_RIP);
    assert_eq!(state.gpr[Register::Rax as usize], INITIAL_RAX);
    // Only the code-walk Accessed bits (the fetch) were committed, once.
    assert_eq!(
        range_pairs(committed_writes),
        CODE_WALK_RANGES
            .into_iter()
            .map(|(a, l)| (a, l as u64))
            .collect::<Vec<_>>()
    );
    for address in DATA_WALK_ENTRIES {
        assert_eq!(entry(&mut bus, address) & ACCESSED, 0, "{address:#x}");
    }
}

#[test]
fn s4_15d_store_into_another_code_page_makes_the_old_bytes_unreachable() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Page 0xa000 holds `48 8B 03` (a load) at offset 0x80, reached at
    // CODE_RIP_2. The store block at CODE_RIP (page 0x8000) rewrites those
    // bytes to `48 89 03` through the data leaf, which now maps that page.
    let rewritten = u64::from_le_bytes([0x48, 0x89, 0x03, 0, 0, 0, 0, 0]);
    let code2_linear = 0x0000_0080_0000_0080;
    let marker = 0x0bad_f00d_dead_beef;
    let mut session = od_host::PagedFetchedLoadSession::new();
    let mut bus = make_fetch_bus(FetchLayout::CLEAN);
    prepare_store_bus(&mut bus, 0x7);
    put_qword(&mut bus, DATA_LEAF_ADDRESS, CODE_PHYSICAL_2 | 0x7);
    put_qword(&mut bus, DATA2_LEAF_ADDRESS, DATA_PHYSICAL | 0x7);
    let code2 = GuestPhysicalAddress::new(CODE_PHYSICAL_2);
    let step = |session: &mut od_host::PagedFetchedLoadSession,
                bus: &mut MachineBus,
                state: &mut LongModeState| {
        let outcome = session
            .execute_or_interpret(
                state,
                bus,
                CR3,
                EFER,
                fetch_context(PrivilegeLevel::Ring0),
                RFLAGS_DEFINED,
            )
            .unwrap();
        let od_host::PagedStepOutcome::Jit(report) = outcome else {
            panic!("every step of this scenario is a JIT step: {outcome:?}");
        };
        report
    };
    let store_state = || {
        let mut state = fetch_state(CODE_RIP);
        state.gpr[Register::Rax as usize] = rewritten;
        state.gpr[Register::Rbx as usize] = code2_linear;
        state
    };

    // 1. The load at CODE_RIP_2 runs first and is admitted. The data leaf maps
    // the same page, so it loads the (zero) qword at 0xa120.
    let generation_before = bus.code_page_generation(code2).unwrap();
    let mut state = fetch_state(CODE_RIP_2);
    let load = step(&mut session, &mut bus, &mut state);
    assert_eq!(load.admission, od_host::PagedFetchAdmission::Admitted);
    assert_eq!(load.fetched_bytes, CODE);
    assert_eq!(state.gpr[Register::Rax as usize], 0);

    // 2. The store at CODE_RIP rewrites the load's bytes in page 0xa000.
    let mut state = store_state();
    let first_store = step(&mut session, &mut bus, &mut state);
    assert_eq!(
        first_store.admission,
        od_host::PagedFetchAdmission::Admitted
    );
    assert_eq!(first_store.fetched_physical_page.get(), CODE_PHYSICAL);
    assert_eq!(state.rip, CODE_RIP + 3);
    assert_eq!(
        entry(&mut bus, CODE_PHYSICAL_2 + 0x80) & 0xff_ffff,
        0x03_8948,
        "the bytes in the other code page changed"
    );
    let generation_after = bus.code_page_generation(code2).unwrap();
    assert!(generation_after > generation_before);

    // 3. The same store again: writing another code page did not invalidate
    // the block that is running, so it is reused, not compiled again.
    let mut state = store_state();
    let second_store = step(&mut session, &mut bus, &mut state);
    assert_eq!(second_store.admission, od_host::PagedFetchAdmission::Reused);
    assert_eq!(
        second_store.fetched_generation,
        first_store.fetched_generation
    );
    assert!(bus.code_page_generation(code2).unwrap() > generation_after);

    // 4. CODE_RIP_2 again. The old load, if it were run or reused, would load
    // RAX from the data qword. What is fetched now is a store: it is compiled
    // from the new bytes and writes RAX (the marker) to the second data page.
    let mut state = fetch_state(CODE_RIP_2);
    state.gpr[Register::Rax as usize] = marker;
    state.gpr[Register::Rbx as usize] = DATA2_LINEAR;
    let rewritten_run = step(&mut session, &mut bus, &mut state);
    assert_eq!(
        rewritten_run.admission,
        od_host::PagedFetchAdmission::Admitted
    );
    assert_eq!(rewritten_run.fetched_bytes, STORE_CODE);
    assert!(rewritten_run.fetched_generation > load.fetched_generation);
    assert_eq!(state.gpr[Register::Rax as usize], marker);
    assert_eq!(state.rip, CODE_RIP_2 + 3);
    assert_eq!(entry(&mut bus, DATA_QWORD), marker);
    // Load, store, rewritten store: three compilations, one reuse.
    assert_eq!(session.admissions(), 3);
}

#[test]
fn s4_15d_store_into_the_executing_code_page_is_refused_before_commit() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // The data leaf maps the page the store block itself is fetched from.
    let make = || {
        let mut bus = make_fetch_bus(FetchLayout::CLEAN);
        prepare_store_bus(&mut bus, 0x7);
        put_qword(&mut bus, DATA_LEAF_ADDRESS, CODE_PHYSICAL | 0x7);
        bus
    };
    let code_page = |bus: &mut MachineBus| -> Vec<u64> {
        (0..0x1000u64)
            .step_by(8)
            .map(|offset| entry(bus, CODE_PHYSICAL + offset))
            .collect()
    };
    let refusal = od_machine::PagedStoreRefusal::ActiveCodePage {
        page: GuestPhysicalAddress::new(CODE_PHYSICAL),
    };

    // Two operands on the page: beside the block's bytes (offset 0x120, the
    // DATA_LINEAR default) and over them (offset 0, RAX = NOPs).
    let nops = 0x9090_9090_9090_9090;
    for (label, operand, value) in [
        ("beside the block", DATA_LINEAR, INITIAL_RAX),
        ("over the block", 0x0000_0080_0000_0000, nops),
    ] {
        let prepare = |state: &mut LongModeState| {
            state.gpr[Register::Rax as usize] = value;
            state.gpr[Register::Rbx as usize] = operand;
        };
        // JIT attempt alone: refused before the operand walk and any write.
        let mut bus = make();
        let mut session = od_host::PagedFetchedLoadSession::new();
        let mut state = fetch_state(CODE_RIP);
        prepare(&mut state);
        let before = state.clone();
        let code_before = code_page(&mut bus);
        let attempt = session
            .execute_with_store(
                &mut state,
                &mut bus,
                CR3,
                EFER,
                fetch_context(PrivilegeLevel::Ring0),
            )
            .unwrap();
        assert_eq!(attempt.store_refusal, Some(refusal), "{label}");
        assert!(
            matches!(
                attempt.execution.callback_result,
                Err(od_host::MemoryOperationExecutionErrorV2::MemoryError(_))
            ),
            "{label}"
        );
        assert_eq!(state, before, "{label}");
        assert!(attempt.execution.committed_writes.is_empty(), "{label}");
        for address in DATA_WALK_ENTRIES {
            assert_eq!(entry(&mut bus, address) & ACCESSED, 0, "{label}");
        }
        assert_eq!(code_page(&mut bus), code_before, "{label}: page unchanged");
        drop(session);

        // The dispatcher equals a pure interpreter step on the same bytes
        // (Unsupported, nothing retired): the block is not corrupted, the
        // refused store is not executed, and no JIT effect leaked.
        let mut session = od_host::PagedFetchedLoadSession::new();
        let mut bus = make();
        let (outcome, state) = dispatch_equals_interpreter(
            label,
            &mut session,
            make(),
            &mut bus,
            PrivilegeLevel::Ring0,
            prepare,
        );
        let od_host::PagedStepOutcome::Interpreter { reason, report, .. } = &outcome else {
            panic!("{label}: a store into the executing page is not a JIT step: {outcome:?}");
        };
        assert_eq!(
            *reason,
            od_host::PagedStepFallbackReason::StoreTargetRefused(refusal),
            "{label}"
        );
        assert!(matches!(
            report.cpu.exit,
            od_isa::execute::RunExit::Unsupported { .. }
        ));
        assert_eq!(state.rip, CODE_RIP, "{label}");
        assert_eq!(code_page(&mut bus), code_before, "{label}");

        // Entering again: nothing changed, so the same block is reused and
        // refuses the same store again, still without any effect.
        let ram = ram_image(&mut bus);
        let mut state = fetch_state(CODE_RIP);
        prepare(&mut state);
        let again = session
            .execute_with_store(
                &mut state,
                &mut bus,
                CR3,
                EFER,
                fetch_context(PrivilegeLevel::Ring0),
            )
            .unwrap();
        assert_eq!(
            again.admission,
            od_host::PagedFetchAdmission::Reused,
            "{label}"
        );
        assert_eq!(again.store_refusal, Some(refusal), "{label}");
        assert_eq!(ram_image(&mut bus), ram, "{label}");
        assert_eq!(session.admissions(), 1, "{label}");
    }
}

#[test]
fn s4_15d_load_only_entry_never_runs_a_store_block_admitted_by_the_store_entry() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut session = od_host::PagedFetchedLoadSession::new();
    let mut bus = make_store_bus(0x7);

    // The store entry admits `48 89 03` and executes it.
    let mut state = fetch_state(CODE_RIP);
    let first = session
        .execute_with_store(
            &mut state,
            &mut bus,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
        )
        .unwrap();
    assert_eq!(first.admission, od_host::PagedFetchAdmission::Admitted);
    assert!(matches!(
        first.execution.callback_result.as_ref().unwrap().exit,
        MemoryOperationExecutionExitV2::Complete
    ));
    assert_eq!(entry(&mut bus, DATA_QWORD), INITIAL_RAX);
    assert!(session.has_admitted_block());

    // Same session, same bytes, same generation: the load-only entry must
    // refuse the form, not reuse the store block. Nothing is stored and RIP and
    // RAX stay as they were (the fetch's Accessed bits are already set).
    let ram_before = ram_image(&mut bus);
    let other_rax = 0x1111_2222_3333_4444;
    let mut state = fetch_state(CODE_RIP);
    state.gpr[Register::Rax as usize] = other_rax;
    let before = state.clone();
    let error = session
        .execute(
            &mut state,
            &mut bus,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
        )
        .unwrap_err();
    assert_eq!(
        error.kind,
        od_host::PagedFetchedLoadErrorKind::UnsupportedForm {
            bytes: STORE_CODE.to_vec()
        }
    );
    assert_eq!(state, before);
    assert_eq!(state.rip, CODE_RIP);
    assert_eq!(state.gpr[Register::Rax as usize], other_rax);
    assert_eq!(entry(&mut bus, DATA_QWORD), INITIAL_RAX);
    assert_eq!(ram_image(&mut bus), ram_before);
    assert_eq!(session.admissions(), 1);

    // The refusal is not sticky: the store entry still reuses the block.
    let mut state = fetch_state(CODE_RIP);
    state.gpr[Register::Rax as usize] = other_rax;
    let again = session
        .execute_with_store(
            &mut state,
            &mut bus,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
        )
        .unwrap();
    assert_eq!(again.admission, od_host::PagedFetchAdmission::Reused);
    assert_eq!(entry(&mut bus, DATA_QWORD), other_rax);
    assert_eq!(session.admissions(), 1);
}

#[test]
fn s4_15d_store_with_an_untranslatable_operand_is_refused_without_any_data_walk_effect() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // A 2 MiB data PDE (present, writable, user, PS = 0x87 at 0x5000) is not a
    // 4 KiB leaf: the probe reports UnsupportedPageSize.
    for label in ["2 MiB data PDE"] {
        let make = || {
            let mut bus = make_store_bus(0x7);
            put_qword(&mut bus, 0x5000, 0x87);
            bus
        };
        let prepare = |_: &mut LongModeState| {};
        let refusal = od_machine::PagedStoreRefusal::ProbeUnsupported;

        // JIT attempt alone: refused before the data walk, not one write.
        let mut bus = make();
        let ram_before = ram_image(&mut bus);
        let mut session = od_host::PagedFetchedLoadSession::new();
        let mut state = fetch_state(CODE_RIP);
        prepare(&mut state);
        let before = state.clone();
        let attempt = session
            .execute_with_store(
                &mut state,
                &mut bus,
                CR3,
                EFER,
                fetch_context(PrivilegeLevel::Ring0),
            )
            .unwrap();
        assert_eq!(attempt.store_refusal, Some(refusal), "{label}");
        assert!(
            matches!(
                attempt.execution.callback_result,
                Err(od_host::MemoryOperationExecutionErrorV2::MemoryError(_))
            ),
            "{label}"
        );
        assert_eq!(state, before, "{label}");
        assert!(attempt.execution.committed_writes.is_empty(), "{label}");
        assert!(attempt.execution.committed_ad_updates.is_empty(), "{label}");
        for address in DATA_WALK_ENTRIES {
            assert_eq!(
                entry(&mut bus, address) & (ACCESSED | DIRTY),
                0,
                "{label}: {address:#x}"
            );
        }
        // Only the fetch's Accessed bits changed in RAM.
        let ram_after = ram_image(&mut bus);
        for (index, (before, after)) in ram_before.iter().zip(&ram_after).enumerate() {
            if before != after {
                let address = index as u64 * 8;
                assert!(
                    CODE_WALK_ENTRIES.contains(&address) && before | ACCESSED == *after,
                    "{label}: unexpected change at {address:#x}"
                );
            }
        }
        drop(session);

        // The dispatcher equals a pure interpreter step (Unsupported, nothing
        // retired): the same writes (the fetch walk only) and the same RAM.
        let mut session = od_host::PagedFetchedLoadSession::new();
        let mut bus = make();
        let (outcome, state) = dispatch_equals_interpreter(
            label,
            &mut session,
            make(),
            &mut bus,
            PrivilegeLevel::Ring0,
            prepare,
        );
        let od_host::PagedStepOutcome::Interpreter {
            reason,
            report,
            committed_writes,
        } = &outcome
        else {
            panic!("{label}: an untranslatable store must be interpreted, got {outcome:?}");
        };
        assert_eq!(
            *reason,
            od_host::PagedStepFallbackReason::StoreTargetRefused(refusal),
            "{label}"
        );
        assert!(
            matches!(
                report.cpu.exit,
                od_isa::execute::RunExit::Unsupported { .. }
            ),
            "{label}"
        );
        assert_eq!(state.rip, CODE_RIP, "{label}");
        assert_eq!(state.gpr[Register::Rax as usize], INITIAL_RAX, "{label}");
        assert_eq!(
            range_pairs(committed_writes),
            CODE_WALK_RANGES
                .into_iter()
                .map(|(a, l)| (a, l as u64))
                .collect::<Vec<_>>(),
            "{label}"
        );
        for address in DATA_WALK_ENTRIES {
            assert_eq!(
                entry(&mut bus, address) & (ACCESSED | DIRTY),
                0,
                "{label}: {address:#x}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// S4-15e: more operand forms through `execute_forms`. The interpreter's paged
// allowlist is `48 8B 03` / `48 89 0B` plus the separate dword profile
// `8B 03`; it has no displacement, RIP-relative or `67` form. The dword load is
// compared with `step_paged_mov_dword_read` itself. A displacement or
// RIP-relative load is compared with the interpreter's `48 8B 03` run on
// identical RAM with RBX set to the effective address the test computes
// independently from RIP, RBX and the displacement (the oracle's RBX and RIP
// are normalised afterwards, its code bytes restored before the RAM image is
// compared), the same equivalence S4-15d uses for the store.
// ---------------------------------------------------------------------------

const DWORD_CODE: [u8; 2] = [0x8b, 0x03];
/// Second code page used as readable data for the RIP-relative load: leaf 0x6010
/// maps linear 0x0040_2000 to `CODE_PHYSICAL_2`.
const RIP_DATA_LINEAR: u64 = 0x0040_2200;
const RIP_DATA_PHYSICAL: u64 = CODE_PHYSICAL_2 + 0x200;
const RIP_VALUE: u64 = 0x1122_3344_5566_7788;
const RIP_VALUE_AT_SECOND_RIP: u64 = 0x99aa_bbcc_ddee_ff01;
const RIP_LEAF_ADDRESS: u64 = 0x6010;
/// `mov rax, [rip + 0x11f9]` at `CODE_RIP`: next RIP 0x40_1007, address 0x40_2200.
const RIP_REL_CODE: [u8; 7] = [0x48, 0x8b, 0x05, 0xf9, 0x11, 0x00, 0x00];
const SUPERVISOR_LEAF: u64 = 0x3;

#[derive(Clone, Copy)]
enum Equivalent {
    Qword,
    Dword,
}

impl Equivalent {
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Qword => &CODE,
            Self::Dword => &DWORD_CODE,
        }
    }
}

fn forms_bus(code: &[u8], tweak: &dyn Fn(&mut MachineBus)) -> MachineBus {
    let mut bus = make_fetch_bus(FetchLayout::CLEAN);
    bus.patch_physical_ram(GuestPhysicalAddress::new(CODE_PHYSICAL), code)
        .unwrap();
    tweak(&mut bus);
    bus
}

fn no_tweak(_: &mut MachineBus) {}

fn data_leaf(flags: u64) -> impl Fn(&mut MachineBus) {
    move |bus| put_qword(bus, DATA_LEAF_ADDRESS, DATA_PHYSICAL | flags)
}

fn rip_data(leaf_flags: u64) -> impl Fn(&mut MachineBus) {
    move |bus| {
        put_qword(bus, RIP_DATA_PHYSICAL, RIP_VALUE);
        put_qword(bus, RIP_DATA_PHYSICAL + 0x40, RIP_VALUE_AT_SECOND_RIP);
        put_qword(bus, RIP_LEAF_ADDRESS, CODE_PHYSICAL_2 | leaf_flags);
    }
}

/// One `execute_forms` step of `code` against the interpreter's equivalent
/// instruction on identical RAM: exit, retired count, PageFaultRecord, the
/// ordered committed writes, the whole RAM image and the full CPU state.
fn forms_equal_interpreter(
    label: &str,
    code: &[u8],
    equivalent: Equivalent,
    expected_address: u64,
    privilege: PrivilegeLevel,
    tweak: &dyn Fn(&mut MachineBus),
    prepare: &dyn Fn(&mut LongModeState),
) -> (od_host::PagedFetchedFormReport, LongModeState, MachineBus) {
    let mut oracle_bus = forms_bus(equivalent.bytes(), tweak);
    let mut oracle_state = fetch_state(CODE_RIP);
    prepare(&mut oracle_state);
    let original_rbx = oracle_state.gpr[Register::Rbx as usize];
    oracle_state.gpr[Register::Rbx as usize] = expected_address;
    let mut oracle_writes = Vec::new();
    let oracle_report = match equivalent {
        Equivalent::Qword => od_machine::step_paged_mov(
            &mut oracle_state,
            &mut oracle_bus,
            CR3,
            EFER,
            fetch_context(privilege),
            RFLAGS_DEFINED,
            |range| oracle_writes.push(range),
        ),
        Equivalent::Dword => od_machine::step_paged_mov_dword_read(
            &mut oracle_state,
            &mut oracle_bus,
            CR3,
            EFER,
            fetch_context(privilege),
            RFLAGS_DEFINED,
            |range| oracle_writes.push(range),
        ),
    }
    .unwrap();
    oracle_state.gpr[Register::Rbx as usize] = original_rbx;
    if oracle_report.cpu.instructions_executed == 1 {
        oracle_state.rip = CODE_RIP + code.len() as u64;
    }
    oracle_bus
        .patch_physical_ram(GuestPhysicalAddress::new(CODE_PHYSICAL), code)
        .unwrap();

    let mut bus = forms_bus(code, tweak);
    let mut state = fetch_state(CODE_RIP);
    prepare(&mut state);
    let mut session = od_host::PagedFetchedLoadSession::new();
    let report = session
        .execute_forms(&mut state, &mut bus, CR3, EFER, fetch_context(privilege))
        .unwrap_or_else(|error| panic!("{label}: refused {error:?}"));
    assert_eq!(report.fetched_code, code, "{label}");
    assert_eq!(report.admission, od_host::PagedFetchAdmission::Admitted);
    assert_eq!(report.store_refusal, None, "{label}");
    let execution = report.execution.callback_result.as_ref().unwrap();
    match &execution.exit {
        MemoryOperationExecutionExitV2::Complete => {
            assert_eq!(
                oracle_report.cpu.exit,
                od_isa::execute::RunExit::StopAddress,
                "{label}"
            );
            assert_eq!(oracle_report.page_fault, None, "{label}");
        }
        MemoryOperationExecutionExitV2::GuestFault(fault) => {
            assert_eq!(
                oracle_report.cpu.exit,
                od_isa::execute::RunExit::Fault(fault.clone()),
                "{label}"
            );
            assert_eq!(
                report.execution.page_fault, oracle_report.page_fault,
                "{label}"
            );
        }
        other => panic!("{label}: unexpected JIT exit {other:?}"),
    }
    assert_eq!(
        execution.retired_instructions, oracle_report.cpu.instructions_executed,
        "{label}"
    );
    let mut writes = range_pairs(&report.fetch_committed_writes);
    writes.extend(range_pairs(&report.execution.committed_writes));
    assert_eq!(writes, range_pairs(&oracle_writes), "{label}: writes");
    assert_eq!(
        ram_image(&mut bus),
        ram_image(&mut oracle_bus),
        "{label}: RAM image"
    );
    assert_eq!(state, oracle_state, "{label}: full LongModeState");
    (report, state, bus)
}

fn forms_page_fault(report: &od_host::PagedFetchedFormReport) -> &od_isa::memory::PageFaultRecord {
    assert!(matches!(
        report.execution.callback_result.as_ref().unwrap().exit,
        MemoryOperationExecutionExitV2::GuestFault(_)
    ));
    report.execution.page_fault.as_ref().unwrap()
}

fn forms_state_with_rbx(rbx: u64) -> impl Fn(&mut LongModeState) {
    move |state| state.gpr[Register::Rbx as usize] = rbx
}

fn forms_refusal(
    code: &[u8],
    rbx: u64,
    tweak: &dyn Fn(&mut MachineBus),
) -> (
    od_host::PagedFetchedLoadError,
    LongModeState,
    MachineBus,
    bool,
) {
    let mut bus = forms_bus(code, tweak);
    let mut state = fetch_state(CODE_RIP);
    state.gpr[Register::Rbx as usize] = rbx;
    let mut session = od_host::PagedFetchedLoadSession::new();
    let error = session
        .execute_forms(
            &mut state,
            &mut bus,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
        )
        .unwrap_err();
    (error, state, bus, session.has_admitted_block())
}

#[test]
fn s4_15e_dword_load_zero_extends_into_rax_and_equals_the_dword_interpreter() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dword = LOADED_VALUE as u32;
    assert_ne!(INITIAL_RAX >> 32, 0, "the high half must be observable");
    for (label, tweak, privilege, fault) in [
        (
            "CPL0",
            &no_tweak as &dyn Fn(&mut MachineBus),
            PrivilegeLevel::Ring0,
            None,
        ),
        ("CPL3", &no_tweak, PrivilegeLevel::Ring3, None),
        (
            "not-present data leaf CPL0",
            &data_leaf(0x6),
            PrivilegeLevel::Ring0,
            Some(0x0u32),
        ),
        (
            "supervisor data leaf CPL3",
            &data_leaf(SUPERVISOR_LEAF),
            PrivilegeLevel::Ring3,
            Some(0x5),
        ),
    ] {
        let (report, state, _bus) = forms_equal_interpreter(
            label,
            &DWORD_CODE,
            Equivalent::Dword,
            DATA_LINEAR,
            privilege,
            tweak,
            &|_| {},
        );
        assert_eq!(report.form, od_host::PagedFetchedForm::DwordLoadRbx);
        match fault {
            None => {
                // 32-bit destination write: bits 63:32 of RAX are cleared, and
                // only four bytes were read (the next dword is not in RAX).
                assert_eq!(
                    state.gpr[Register::Rax as usize],
                    u64::from(dword),
                    "{label}"
                );
                assert_eq!(state.rip, CODE_RIP + 2, "{label}");
            }
            Some(pfec) => {
                let record = forms_page_fault(&report);
                assert_eq!(record.fault().error_code, Some(pfec), "{label}");
                assert_eq!(record.cr2_value(), DATA_LINEAR, "{label}");
                assert_eq!(record.saved_rip(), CODE_RIP, "{label}");
                assert_eq!(state.gpr[Register::Rax as usize], INITIAL_RAX, "{label}");
                assert_eq!(state.rip, CODE_RIP, "{label}");
            }
        }
    }
}

#[test]
fn s4_15e_dword_load_may_end_exactly_at_a_page_boundary_but_not_cross_it() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let page = DATA_LINEAR & !0xfff;
    let put_dword = |bus: &mut MachineBus| {
        bus.write_physical(
            GuestPhysicalAddress::new(DATA_PHYSICAL + 0xffc),
            AccessWidth::Dword,
            0xdead_beef,
            |_| {},
        )
        .unwrap();
    };
    // Offset 0xffc: the last four bytes of the page. A qword here would be
    // refused (S4-15c); a dword is wholly inside the page.
    let (report, state, _bus) = forms_equal_interpreter(
        "dword at 0xffc",
        &DWORD_CODE,
        Equivalent::Dword,
        page + 0xffc,
        PrivilegeLevel::Ring0,
        &put_dword,
        &forms_state_with_rbx(page + 0xffc),
    );
    assert_eq!(report.form, od_host::PagedFetchedForm::DwordLoadRbx);
    assert_eq!(state.gpr[Register::Rax as usize], 0xdead_beef);

    // Offset 0xffd: one byte would be on the next page: refused after the
    // fetch, before any data walk.
    let (error, state, mut bus, admitted) = forms_refusal(&DWORD_CODE, page + 0xffd, &put_dword);
    assert_eq!(
        error.kind,
        od_host::PagedFetchedLoadErrorKind::Load(
            od_host::PagedMemoryLoadV2Error::UnsupportedOperandRange {
                address: page + 0xffd,
                length: 4
            }
        )
    );
    assert_eq!(state.gpr[Register::Rax as usize], INITIAL_RAX);
    assert_eq!(state.rip, CODE_RIP);
    assert!(!admitted);
    for address in DATA_WALK_ENTRIES {
        assert_eq!(entry(&mut bus, address) & ACCESSED, 0, "{address:#x}");
    }
}

#[test]
fn s4_15e_dword_load_at_the_end_of_the_code_page_needs_only_its_two_bytes() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // `8B 03` at 0x40_1ffe: a third fetch byte would be on the next page, which
    // the legacy fetch of three bytes would refuse. The next page is not even
    // mapped executable here (NX), so reading a third byte would be a fetch #PF.
    let rip = CODE_RIP + 0xffe;
    let mut bus = make_fetch_bus(FetchLayout::CLEAN);
    bus.patch_physical_ram(
        GuestPhysicalAddress::new(CODE_PHYSICAL + 0xffe),
        &DWORD_CODE,
    )
    .unwrap();
    put_qword(&mut bus, 0x6010, CODE_PHYSICAL_2 | 0x7 | NX);
    let mut state = fetch_state(rip);
    let mut session = od_host::PagedFetchedLoadSession::new();
    let report = session
        .execute_forms(
            &mut state,
            &mut bus,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
        )
        .unwrap();
    assert_eq!(report.fetched_code, DWORD_CODE);
    assert_eq!(
        state.gpr[Register::Rax as usize],
        u64::from(LOADED_VALUE as u32)
    );
    assert_eq!(state.rip, rip + 2);
}

#[test]
fn s4_15e_qword_displacement_loads_use_the_sign_extended_displacement_and_equal_the_interpreter() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // (label, code, RBX such that RBX + displacement = DATA_LINEAR, form)
    let forms: [(&str, &[u8], u64, od_host::PagedFetchedForm); 4] = [
        (
            "disp8 +0x20",
            &[0x48, 0x8b, 0x43, 0x20],
            DATA_LINEAR - 0x20,
            od_host::PagedFetchedForm::QwordLoadRbxDisp8,
        ),
        (
            "disp8 -0x10",
            &[0x48, 0x8b, 0x43, 0xf0],
            DATA_LINEAR + 0x10,
            od_host::PagedFetchedForm::QwordLoadRbxDisp8,
        ),
        (
            "disp32 +0x1234",
            &[0x48, 0x8b, 0x83, 0x34, 0x12, 0x00, 0x00],
            DATA_LINEAR - 0x1234,
            od_host::PagedFetchedForm::QwordLoadRbxDisp32,
        ),
        (
            "disp32 -0x2000",
            &[0x48, 0x8b, 0x83, 0x00, 0xe0, 0xff, 0xff],
            DATA_LINEAR + 0x2000,
            od_host::PagedFetchedForm::QwordLoadRbxDisp32,
        ),
    ];
    for (name, code, rbx, form) in forms {
        for (case, tweak, privilege, fault) in [
            (
                "CPL0",
                &no_tweak as &dyn Fn(&mut MachineBus),
                PrivilegeLevel::Ring0,
                None,
            ),
            ("CPL3", &no_tweak, PrivilegeLevel::Ring3, None),
            (
                "not-present leaf CPL0",
                &data_leaf(0x6),
                PrivilegeLevel::Ring0,
                Some(0x0u32),
            ),
            (
                "supervisor leaf CPL3",
                &data_leaf(SUPERVISOR_LEAF),
                PrivilegeLevel::Ring3,
                Some(0x5),
            ),
        ] {
            let label = format!("{name} {case}");
            let (report, state, _bus) = forms_equal_interpreter(
                &label,
                code,
                Equivalent::Qword,
                DATA_LINEAR,
                privilege,
                tweak,
                &forms_state_with_rbx(rbx),
            );
            assert_eq!(report.form, form, "{label}");
            // RBX is only a base: it is never modified.
            assert_eq!(state.gpr[Register::Rbx as usize], rbx, "{label}");
            match fault {
                None => {
                    assert_eq!(state.gpr[Register::Rax as usize], LOADED_VALUE, "{label}");
                    assert_eq!(state.rip, CODE_RIP + code.len() as u64, "{label}");
                }
                Some(pfec) => {
                    let record = forms_page_fault(&report);
                    assert_eq!(record.fault().error_code, Some(pfec), "{label}");
                    // CR2 is the effective address, not RBX.
                    assert_eq!(record.cr2_value(), DATA_LINEAR, "{label}");
                    assert_eq!(record.saved_rip(), CODE_RIP, "{label}");
                    assert_eq!(state.gpr[Register::Rax as usize], INITIAL_RAX, "{label}");
                    assert_eq!(state.rip, CODE_RIP, "{label}");
                }
            }
        }
    }
}

#[test]
fn s4_15e_rip_relative_load_is_relative_to_the_next_instruction_not_rbx_or_the_start() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(
        CODE_RIP + RIP_REL_CODE.len() as u64 + 0x11f9,
        RIP_DATA_LINEAR
    );
    for (label, tweak, privilege, fault) in [
        (
            "CPL0",
            &rip_data(0x7) as &dyn Fn(&mut MachineBus),
            PrivilegeLevel::Ring0,
            None,
        ),
        ("CPL3", &rip_data(0x7), PrivilegeLevel::Ring3, None),
        (
            "not-present leaf CPL0",
            &rip_data(0x6),
            PrivilegeLevel::Ring0,
            Some(0x0u32),
        ),
        (
            "supervisor leaf CPL3",
            &rip_data(SUPERVISOR_LEAF),
            PrivilegeLevel::Ring3,
            Some(0x5),
        ),
    ] {
        // RBX deliberately points at other readable data (LOADED_VALUE): the
        // RBX-relative reading is distinguishable from the RIP-relative one.
        let (report, state, mut bus) = forms_equal_interpreter(
            label,
            &RIP_REL_CODE,
            Equivalent::Qword,
            RIP_DATA_LINEAR,
            privilege,
            tweak,
            &|_| {},
        );
        assert_eq!(report.form, od_host::PagedFetchedForm::QwordLoadRipRel32);
        assert_eq!(state.gpr[Register::Rbx as usize], DATA_LINEAR, "{label}");
        match fault {
            None => {
                assert_eq!(state.gpr[Register::Rax as usize], RIP_VALUE, "{label}");
                assert_ne!(RIP_VALUE, LOADED_VALUE);
                assert_eq!(state.rip, CODE_RIP + 7, "{label}");
                // The data walk touched only the second code page's leaf, never
                // the RBX data branch.
                assert_ne!(entry(&mut bus, RIP_LEAF_ADDRESS) & ACCESSED, 0, "{label}");
                for address in DATA_WALK_ENTRIES {
                    assert_eq!(entry(&mut bus, address) & ACCESSED, 0, "{label}");
                }
            }
            Some(pfec) => {
                let record = forms_page_fault(&report);
                assert_eq!(record.fault().error_code, Some(pfec), "{label}");
                assert_eq!(record.cr2_value(), RIP_DATA_LINEAR, "{label}");
                assert_eq!(record.saved_rip(), CODE_RIP, "{label}");
                assert_eq!(state.gpr[Register::Rax as usize], INITIAL_RAX, "{label}");
                assert_eq!(state.rip, CODE_RIP, "{label}");
            }
        }
    }

    // Reading at the start of the instruction instead of the next RIP would
    // land 7 bytes lower: bytes `00..00 88`, a different value.
    let mut bus = forms_bus(&RIP_REL_CODE, &rip_data(0x7));
    let wrong = bus
        .read_physical(
            GuestPhysicalAddress::new(RIP_DATA_PHYSICAL - RIP_REL_CODE.len() as u64),
            AccessWidth::Qword,
        )
        .unwrap();
    assert_ne!(wrong, RIP_VALUE);
}

#[test]
fn s4_15e_reuse_key_keeps_rip_so_a_rip_relative_block_is_never_reused_at_another_rip() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let second_rip = CODE_RIP + 0x40;
    let mut bus = forms_bus(&RIP_REL_CODE, &rip_data(0x7));
    // The same instruction bytes at the same physical page, 0x40 bytes on.
    bus.patch_physical_ram(
        GuestPhysicalAddress::new(CODE_PHYSICAL + 0x40),
        &RIP_REL_CODE,
    )
    .unwrap();
    let mut session = od_host::PagedFetchedLoadSession::new();
    let run = |session: &mut od_host::PagedFetchedLoadSession,
               bus: &mut MachineBus,
               rip: u64|
     -> (od_host::PagedFetchAdmission, u64, u64) {
        let mut state = fetch_state(rip);
        let report = session
            .execute_forms(
                &mut state,
                bus,
                CR3,
                EFER,
                fetch_context(PrivilegeLevel::Ring0),
            )
            .unwrap_or_else(|error| panic!("{rip:#x}: {error:?}"));
        (
            report.admission,
            state.gpr[Register::Rax as usize],
            state.rip,
        )
    };
    assert_eq!(
        run(&mut session, &mut bus, CODE_RIP),
        (
            od_host::PagedFetchAdmission::Admitted,
            RIP_VALUE,
            CODE_RIP + 7
        )
    );
    // Same RIP again: reused, address recomputed from RIP (same data).
    assert_eq!(
        run(&mut session, &mut bus, CODE_RIP).0,
        od_host::PagedFetchAdmission::Reused
    );
    // Same bytes, same page, same generation, other RIP: the block is
    // recompiled, and the address is relative to this instruction's next RIP
    // (0x40 higher), reading the second value.
    assert_eq!(
        run(&mut session, &mut bus, second_rip),
        (
            od_host::PagedFetchAdmission::Admitted,
            RIP_VALUE_AT_SECOND_RIP,
            second_rip + 7
        )
    );
    assert_eq!(session.admissions(), 2);
    // Data that changed between two calls of the reused block is read afresh.
    put_qword(&mut bus, RIP_DATA_PHYSICAL + 0x40, 0x0bad_f00d);
    let mut state = fetch_state(second_rip);
    let report = session
        .execute_forms(
            &mut state,
            &mut bus,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
        )
        .unwrap();
    assert_eq!(report.admission, od_host::PagedFetchAdmission::Reused);
    assert_eq!(state.gpr[Register::Rax as usize], 0x0bad_f00d);
}

#[test]
fn s4_15e_reused_displacement_block_recomputes_the_address_from_rbx() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let code = [0x48, 0x8b, 0x43, 0x08];
    let mut bus = forms_bus(&code, &no_tweak);
    put_qword(&mut bus, DATA_PHYSICAL + 0x128, 0x5555_6666_7777_8888);
    let mut session = od_host::PagedFetchedLoadSession::new();
    let mut first = fetch_state(CODE_RIP);
    first.gpr[Register::Rbx as usize] = DATA_LINEAR - 0x08;
    let report = session
        .execute_forms(
            &mut first,
            &mut bus,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
        )
        .unwrap();
    assert_eq!(report.admission, od_host::PagedFetchAdmission::Admitted);
    assert_eq!(first.gpr[Register::Rax as usize], LOADED_VALUE);
    let mut second = fetch_state(CODE_RIP);
    second.gpr[Register::Rbx as usize] = DATA_LINEAR;
    let report = session
        .execute_forms(
            &mut second,
            &mut bus,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
        )
        .unwrap();
    assert_eq!(report.admission, od_host::PagedFetchAdmission::Reused);
    assert_eq!(second.gpr[Register::Rax as usize], 0x5555_6666_7777_8888);
}

#[test]
fn s4_15e_effective_address_crossing_a_page_is_refused_before_the_data_walk() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let page = DATA_LINEAR & !0xfff;
    // RBX alone passes the range check (offset 0xf80); base plus displacement
    // does not: the qword at offset 0xffc has four bytes on the next page.
    for (label, code, rbx, address) in [
        (
            "disp8",
            &[0x48u8, 0x8b, 0x43, 0x7c][..],
            page + 0xf80,
            page + 0xffc,
        ),
        (
            "disp32",
            &[0x48, 0x8b, 0x83, 0x7c, 0x00, 0x00, 0x00][..],
            page + 0xf80,
            page + 0xffc,
        ),
        (
            // 0x40_1007 + 0x1ff9 = 0x40_3000: not even the page of the data
            // branch; the offset 0xffc form below is the interesting one.
            "rip-relative",
            &[0x48, 0x8b, 0x05, 0xf5, 0x1f, 0x00, 0x00][..],
            DATA_LINEAR,
            CODE_RIP + 7 + 0x1ff5,
        ),
    ] {
        assert_ne!(rbx & 0xfff, 0xffc, "{label}: RBX alone would be accepted");
        let (error, state, mut bus, admitted) = forms_refusal(code, rbx, &rip_data(0x7));
        assert_eq!(
            error.kind,
            od_host::PagedFetchedLoadErrorKind::Load(
                od_host::PagedMemoryLoadV2Error::UnsupportedOperandRange { address, length: 8 }
            ),
            "{label}"
        );
        assert_eq!(address & 0xfff, 0xffc, "{label}");
        // The fetch ran (its Accessed bits are reported); nothing else did.
        assert_eq!(
            range_pairs(&error.fetch_committed_writes),
            CODE_WALK_RANGES.map(|(a, l)| (a, l as u64)).to_vec(),
            "{label}"
        );
        assert_eq!(state.gpr[Register::Rax as usize], INITIAL_RAX, "{label}");
        assert_eq!(state.rip, CODE_RIP, "{label}");
        assert!(!admitted, "{label}");
        for address in DATA_WALK_ENTRIES {
            assert_eq!(entry(&mut bus, address) & ACCESSED, 0, "{label}");
        }
        assert_eq!(entry(&mut bus, RIP_LEAF_ADDRESS) & ACCESSED, 0, "{label}");
    }
}

#[test]
fn s4_15e_fetch_faults_of_the_longer_forms_precede_any_data_access_and_equal_the_interpreter() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let cases = [
        (
            "NX leaf CPL0",
            FetchLayout {
                code_leaf_flags: 0x27 | NX,
                ..FetchLayout::CLEAN
            },
            PrivilegeLevel::Ring0,
            0x11u32,
        ),
        (
            "supervisor leaf CPL3",
            FetchLayout {
                code_leaf_flags: 0x3,
                ..FetchLayout::CLEAN
            },
            PrivilegeLevel::Ring3,
            0x15,
        ),
        (
            "not-present leaf CPL0",
            FetchLayout {
                code_leaf_flags: 0x6,
                ..FetchLayout::CLEAN
            },
            PrivilegeLevel::Ring0,
            0x10,
        ),
    ];
    for (form, code) in [
        ("dword", &DWORD_CODE[..]),
        ("disp8", &[0x48, 0x8b, 0x43, 0x20][..]),
        ("disp32", &[0x48, 0x8b, 0x83, 0x34, 0x12, 0x00, 0x00][..]),
        ("rip-relative", &RIP_REL_CODE[..]),
    ] {
        for (label, layout, privilege, pfec) in cases {
            let label = format!("{form} {label}");
            let mut oracle_bus = make_fetch_bus(layout);
            let mut oracle_state = fetch_state(CODE_RIP);
            let expected = observe_interpreter(&mut oracle_bus, &mut oracle_state, privilege);
            let oracle_record = expected.page_fault.clone().expect(&label);
            assert_eq!(oracle_record.fault().error_code, Some(pfec), "{label}");

            let mut bus = make_fetch_bus(layout);
            bus.patch_physical_ram(GuestPhysicalAddress::new(CODE_PHYSICAL), code)
                .unwrap();
            let mut state = fetch_state(CODE_RIP);
            let before = state.clone();
            let mut session = od_host::PagedFetchedLoadSession::new();
            let error = session
                .execute_forms(&mut state, &mut bus, CR3, EFER, fetch_context(privilege))
                .unwrap_err();
            let (record, _) = fetch_page_fault(&error);
            assert_eq!(record, &oracle_record, "{label}");
            assert_eq!(record.cr2_value(), CODE_RIP, "{label}");
            assert_eq!(state, before, "{label}");
            assert!(!session.has_admitted_block(), "{label}");
            assert_eq!(
                range_pairs(&error.fetch_committed_writes),
                expected.writes,
                "{label}"
            );
            for address in DATA_WALK_ENTRIES {
                assert_eq!(entry(&mut bus, address) & ACCESSED, 0, "{label}");
            }
        }
    }
}

#[test]
fn s4_15e_code_that_does_not_fit_the_page_is_refused_not_faulted() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // `48 8B 83 ...` needs seven bytes; at offset 0xffd only three are on the
    // page. Not a fault of the guest: the 2 MiB / cross-page code case stays
    // with the interpreter.
    let rip = CODE_RIP + 0xffd;
    let mut bus = make_fetch_bus(FetchLayout::CLEAN);
    bus.patch_physical_ram(
        GuestPhysicalAddress::new(CODE_PHYSICAL + 0xffd),
        &[0x48, 0x8b, 0x83],
    )
    .unwrap();
    let mut state = fetch_state(rip);
    let before = state.clone();
    let mut session = od_host::PagedFetchedLoadSession::new();
    let error = session
        .execute_forms(
            &mut state,
            &mut bus,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
        )
        .unwrap_err();
    assert_eq!(
        error.kind,
        od_host::PagedFetchedLoadErrorKind::Fetch(
            od_machine::TrackedCodeSnapshotError::InvalidLength
        )
    );
    assert_eq!(state, before);
    assert!(!session.has_admitted_block());
}

#[test]
fn s4_15e_forms_outside_the_allowlist_are_unsupported_forms_with_no_effect() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // None of these is admitted: the interpreter has no such form (store with a
    // displacement, `67` address prefix), or the JIT does not (other registers,
    // dword with a displacement). The reported bytes are the three the fetch
    // classified them from.
    let unsupported: [(&str, &[u8], [u8; 3]); 5] = [
        ("store disp8", &[0x48, 0x89, 0x43, 0x08], [0x48, 0x89, 0x43]),
        ("addr32 dword load", &[0x67, 0x8b, 0x03], [0x67, 0x8b, 0x03]),
        ("dword disp8", &[0x8b, 0x43, 0x08], [0x8b, 0x43, 0x08]),
        ("MOV RCX,[RBX]", &[0x48, 0x8b, 0x0b], [0x48, 0x8b, 0x0b]),
        ("nop", &[0x90, 0x90, 0x90], [0x90, 0x90, 0x90]),
    ];
    let mut bus = forms_bus(&[0x48, 0x8b, 0x43, 0x20], &no_tweak);
    let mut session = od_host::PagedFetchedLoadSession::new();
    let mut admit = fetch_state(CODE_RIP);
    admit.gpr[Register::Rbx as usize] = DATA_LINEAR - 0x20;
    session
        .execute_forms(
            &mut admit,
            &mut bus,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
        )
        .unwrap();
    assert!(session.has_admitted_block());
    for (label, code, reported) in unsupported {
        // Re-admit a block so the refusal's drop of the stale block is visible.
        bus.patch_physical_ram(
            GuestPhysicalAddress::new(CODE_PHYSICAL),
            &[0x48, 0x8b, 0x43, 0x20],
        )
        .unwrap();
        let mut state = fetch_state(CODE_RIP);
        state.gpr[Register::Rbx as usize] = DATA_LINEAR - 0x20;
        session
            .execute_forms(
                &mut state,
                &mut bus,
                CR3,
                EFER,
                fetch_context(PrivilegeLevel::Ring0),
            )
            .unwrap();
        assert!(session.has_admitted_block(), "{label}");

        bus.patch_physical_ram(GuestPhysicalAddress::new(CODE_PHYSICAL), code)
            .unwrap();
        let data_before = DATA_WALK_ENTRIES.map(|a| entry(&mut bus, a));
        let mut state = fetch_state(CODE_RIP);
        let before = state.clone();
        let error = session
            .execute_forms(
                &mut state,
                &mut bus,
                CR3,
                EFER,
                fetch_context(PrivilegeLevel::Ring0),
            )
            .unwrap_err();
        assert_eq!(
            error.kind,
            od_host::PagedFetchedLoadErrorKind::UnsupportedForm {
                bytes: reported.to_vec()
            },
            "{label}"
        );
        assert_eq!(state, before, "{label}");
        assert!(!session.has_admitted_block(), "{label}");
        assert_eq!(DATA_WALK_ENTRIES.map(|a| entry(&mut bus, a)), data_before);
    }
}

#[test]
fn s4_15e_legacy_entries_do_not_admit_the_new_forms() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for (code, reported) in [
        (&DWORD_CODE[..], vec![0x8b, 0x03, 0x03]),
        (&[0x48u8, 0x8b, 0x43, 0x20][..], vec![0x48, 0x8b, 0x43]),
        (&RIP_REL_CODE[..], vec![0x48, 0x8b, 0x05]),
    ] {
        let mut bus = forms_bus(code, &no_tweak);
        let mut state = fetch_state(CODE_RIP);
        let before = state.clone();
        let mut session = od_host::PagedFetchedLoadSession::new();
        for with_store in [false, true] {
            let result = if with_store {
                session.execute_with_store(
                    &mut state,
                    &mut bus,
                    CR3,
                    EFER,
                    fetch_context(PrivilegeLevel::Ring0),
                )
            } else {
                session.execute(
                    &mut state,
                    &mut bus,
                    CR3,
                    EFER,
                    fetch_context(PrivilegeLevel::Ring0),
                )
            };
            assert_eq!(
                result.unwrap_err().kind,
                od_host::PagedFetchedLoadErrorKind::UnsupportedForm {
                    bytes: reported.clone()
                }
            );
            assert_eq!(state, before);
        }
    }
}

#[test]
fn s4_15e_the_existing_forms_through_execute_forms_equal_the_interpreter() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // `48 8B 03` against its own interpreter step.
    let (report, state, _bus) = forms_equal_interpreter(
        "qword load",
        &CODE,
        Equivalent::Qword,
        DATA_LINEAR,
        PrivilegeLevel::Ring0,
        &no_tweak,
        &|_| {},
    );
    assert_eq!(report.form, od_host::PagedFetchedForm::QwordLoadRbx);
    assert_eq!(state.gpr[Register::Rax as usize], LOADED_VALUE);

    // `48 89 03` against the interpreter's `48 89 0B` with RCX = RAX.
    let mut oracle_bus = forms_bus(&STORE_RCX_CODE, &no_tweak);
    let mut oracle_state = fetch_state(CODE_RIP);
    oracle_state.gpr[Register::Rcx as usize] = INITIAL_RAX;
    let mut oracle_writes = Vec::new();
    let oracle = od_machine::step_paged_mov(
        &mut oracle_state,
        &mut oracle_bus,
        CR3,
        EFER,
        fetch_context(PrivilegeLevel::Ring0),
        RFLAGS_DEFINED,
        |range| oracle_writes.push(range),
    )
    .unwrap();
    assert_eq!(oracle.cpu.exit, od_isa::execute::RunExit::StopAddress);
    oracle_bus
        .patch_physical_ram(GuestPhysicalAddress::new(CODE_PHYSICAL), &STORE_CODE)
        .unwrap();
    let mut bus = forms_bus(&STORE_CODE, &no_tweak);
    let mut state = fetch_state(CODE_RIP);
    let mut session = od_host::PagedFetchedLoadSession::new();
    let report = session
        .execute_forms(
            &mut state,
            &mut bus,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
        )
        .unwrap();
    assert_eq!(report.form, od_host::PagedFetchedForm::QwordStoreRbx);
    oracle_state.gpr[Register::Rcx as usize] = state.gpr[Register::Rcx as usize];
    assert_eq!(state, oracle_state);
    let mut writes = range_pairs(&report.fetch_committed_writes);
    writes.extend(range_pairs(&report.execution.committed_writes));
    assert_eq!(writes, range_pairs(&oracle_writes));
    assert_eq!(ram_image(&mut bus), ram_image(&mut oracle_bus));
    assert_eq!(entry(&mut bus, DATA_QWORD), INITIAL_RAX);
}

// ---------------------------------------------------------------------------
// S4-15f: reset and release of the JIT region on the production owner
//
// The session owns its block through `PublishedCode`, i.e. the process's single
// MAP_JIT region. The number of free "slots" is observed in two independent
// ways: `map_jit_region_live()` and a probe arena (capacity 16) that can only be
// created when the region is free and then reports all its slots available.
// ---------------------------------------------------------------------------

/// Free MAP_JIT slots of the process: 0 while a session holds the region, the
/// full probe capacity while it is free. Cross-checked with the lease flag.
fn jit_free_slots() -> usize {
    let live = od_host::map_jit_region_live();
    let slots = match od_host::PublishedArenaBackend::new(od_host::MAX_PUBLISHED_ARENA_BLOCKS) {
        Ok(arena) => {
            let free = arena.available_slots();
            assert_eq!(free, arena.capacity(), "a fresh probe arena is all free");
            free
        }
        Err(od_host::PublishedArenaError::MappingAlreadyLive) => 0,
        Err(other) => panic!("unexpected probe failure: {other:?}"),
    };
    assert_eq!(slots == 0, live, "probe and lease flag must agree");
    slots
}

const ALL_FREE: usize = od_host::MAX_PUBLISHED_ARENA_BLOCKS;

fn run_load(
    session: &mut od_host::PagedFetchedLoadSession,
    bus: &mut MachineBus,
) -> (LongModeState, od_host::PagedFetchedLoadReport) {
    let mut state = fetch_state(CODE_RIP);
    let report = session
        .execute(
            &mut state,
            bus,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
        )
        .unwrap();
    (state, report)
}

#[test]
fn s4_15f_reset_between_two_entries_readmits_and_equals_the_interpreter() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(jit_free_slots(), ALL_FREE);

    // Interpreter oracle: the same two entries on its own identical RAM.
    let mut oracle_bus = make_fetch_bus(FetchLayout::CLEAN);
    let mut oracle = Vec::new();
    for _ in 0..2 {
        let mut state = fetch_state(CODE_RIP);
        oracle.push(observe_interpreter(
            &mut oracle_bus,
            &mut state,
            PrivilegeLevel::Ring0,
        ));
    }

    let mut bus = make_fetch_bus(FetchLayout::CLEAN);
    let mut session = od_host::PagedFetchedLoadSession::new();
    let mut observed = Vec::new();
    let mut admissions = Vec::new();
    for entry_index in 0..2 {
        let mut state = fetch_state(CODE_RIP);
        let (jit, report) =
            observe_fetched_jit(&mut session, &mut bus, &mut state, PrivilegeLevel::Ring0).unwrap();
        observed.push(jit);
        admissions.push(report.admission);
        if entry_index == 0 {
            assert!(session.has_admitted_block());
            assert!(session.reset(), "the admitted block is released");
            assert!(!session.has_admitted_block());
            assert_eq!(jit_free_slots(), ALL_FREE);
        }
    }
    assert_eq!(
        admissions,
        vec![
            od_host::PagedFetchAdmission::Admitted,
            od_host::PagedFetchAdmission::Admitted
        ],
        "after a reset the next entry admits again, it never reuses"
    );
    assert_eq!(observed, oracle, "RAX, flags, RIP, writes and RAM equal");
    assert_eq!(observed[1].gprs[Register::Rax as usize], LOADED_VALUE);
    assert_eq!(session.admissions(), 2);
    assert_eq!(session.resets(), 1);

    // Control: without a reset the same second entry reuses the block, so the
    // `Admitted` above is caused by the reset and not by the fixture.
    let (_, reused) = run_load(&mut session, &mut bus);
    assert_eq!(reused.admission, od_host::PagedFetchAdmission::Reused);
    assert_eq!(session.admissions(), 2);
}

#[test]
fn s4_15f_free_slots_return_to_the_initial_count_after_reset_and_after_drop() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let initial = jit_free_slots();
    assert_eq!(initial, ALL_FREE);

    let mut bus = make_fetch_bus(FetchLayout::CLEAN);
    let mut session = od_host::PagedFetchedLoadSession::new();
    assert!(!session.reset(), "nothing admitted, nothing to release");
    assert_eq!(jit_free_slots(), initial);

    let _ = run_load(&mut session, &mut bus);
    assert!(od_host::map_jit_region_live());
    assert_eq!(jit_free_slots(), 0, "one slot is owned after admission");

    assert!(session.reset());
    assert_eq!(jit_free_slots(), initial, "reset returns the slot");

    let _ = run_load(&mut session, &mut bus);
    assert_eq!(jit_free_slots(), 0);
    drop(session);
    assert_eq!(jit_free_slots(), initial, "dropping the session returns it");

    // The store form owns its block the same way.
    let mut store_bus = forms_bus(&STORE_CODE, &no_tweak);
    let mut store_state = fetch_state(CODE_RIP);
    let mut store_session = od_host::PagedFetchedLoadSession::new();
    store_session
        .execute_with_store(
            &mut store_state,
            &mut store_bus,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
        )
        .unwrap();
    assert_eq!(jit_free_slots(), 0);
    assert!(store_session.reset());
    assert_eq!(jit_free_slots(), initial);
    drop(store_session);
    assert_eq!(jit_free_slots(), initial);
}

#[test]
fn s4_15f_the_block_stays_owned_after_its_execution_until_reset_or_drop() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut bus = make_fetch_bus(FetchLayout::CLEAN);
    let mut session = od_host::PagedFetchedLoadSession::new();

    // A completed execution does not release the block on its way out.
    let (state, first) = run_load(&mut session, &mut bus);
    assert_eq!(state.gpr[Register::Rax as usize], LOADED_VALUE);
    assert_eq!(first.admission, od_host::PagedFetchAdmission::Admitted);
    assert!(session.has_admitted_block());
    assert_eq!(jit_free_slots(), 0);
    let (state, second) = run_load(&mut session, &mut bus);
    assert_eq!(state.gpr[Register::Rax as usize], LOADED_VALUE);
    assert_eq!(second.admission, od_host::PagedFetchAdmission::Reused);
    assert_eq!(jit_free_slots(), 0);

    // A guest #PF raised by the executing block does not release it either.
    let mut fault_bus = make_fetch_bus(FetchLayout {
        data_leaf_present: false,
        ..FetchLayout::CLEAN
    });
    let mut fault_session = od_host::PagedFetchedLoadSession::new();
    assert!(session.reset());
    assert_eq!(jit_free_slots(), ALL_FREE);
    let mut fault_state = fetch_state(CODE_RIP);
    let fault_report = fault_session
        .execute(
            &mut fault_state,
            &mut fault_bus,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
        )
        .unwrap();
    assert!(matches!(
        fault_report
            .execution
            .callback_result
            .as_ref()
            .unwrap()
            .exit,
        MemoryOperationExecutionExitV2::GuestFault(_)
    ));
    assert_eq!(fault_state.gpr[Register::Rax as usize], INITIAL_RAX);
    assert!(fault_session.has_admitted_block());
    assert_eq!(jit_free_slots(), 0, "still owned after the faulting call");
    assert!(fault_session.reset());
    assert_eq!(jit_free_slots(), ALL_FREE);

    // `reset(&mut self)` and every execute entry take `&mut self`: a reset can
    // only run between calls, never from inside a block. The inside-callback
    // case of the arena owner is covered by the od-host lib tests.
}

#[test]
fn s4_15f_repeated_reset_and_late_drop_never_release_another_sessions_region() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut bus_a = make_fetch_bus(FetchLayout::CLEAN);
    let mut bus_b = make_fetch_bus(FetchLayout::CLEAN);
    let mut a = od_host::PagedFetchedLoadSession::new();
    let mut b = od_host::PagedFetchedLoadSession::new();

    let _ = run_load(&mut a, &mut bus_a);
    // One region per process: B cannot publish while A owns it.
    let mut state_b = fetch_state(CODE_RIP);
    let refused = b
        .execute(
            &mut state_b,
            &mut bus_b,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
        )
        .unwrap_err();
    assert!(matches!(
        refused.kind,
        od_host::PagedFetchedLoadErrorKind::Publish(
            od_host::ExecutableMemoryError::MappingAlreadyLive
        )
    ));
    assert_eq!(state_b.gpr[Register::Rax as usize], INITIAL_RAX);
    assert!(!b.has_admitted_block());

    assert!(a.reset());
    let (state, report) = run_load(&mut b, &mut bus_b);
    assert_eq!(report.admission, od_host::PagedFetchAdmission::Admitted);
    assert_eq!(state.gpr[Register::Rax as usize], LOADED_VALUE);
    assert_eq!(jit_free_slots(), 0);

    // A second reset of A, and A's drop, release nothing: the region now
    // belongs to B and stays live and usable.
    assert!(!a.reset(), "a second reset is a no-op");
    assert_eq!(a.resets(), 1);
    assert!(od_host::map_jit_region_live());
    drop(a);
    assert!(
        od_host::map_jit_region_live(),
        "A's drop must not free B's region"
    );
    assert_eq!(jit_free_slots(), 0);
    let (state, again) = run_load(&mut b, &mut bus_b);
    assert_eq!(again.admission, od_host::PagedFetchAdmission::Reused);
    assert_eq!(state.gpr[Register::Rax as usize], LOADED_VALUE);

    assert!(b.reset());
    assert!(!b.reset());
    assert_eq!(jit_free_slots(), ALL_FREE);
    drop(b);
    assert_eq!(jit_free_slots(), ALL_FREE);
}

#[test]
fn s4_15f_generation_invalidation_replaces_the_block_without_leaking_the_region() {
    let _guard = JIT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut bus = make_fetch_bus(FetchLayout::CLEAN);
    let mut session = od_host::PagedFetchedLoadSession::new();
    let (_, first) = run_load(&mut session, &mut bus);
    let mut generation = first.fetched_generation;
    assert_eq!(jit_free_slots(), 0);

    // Rewriting the same byte moves the generation: a fresh block replaces
    // the stale one and the single region is never held twice or lost.
    for round in 1..=5_u64 {
        tracked_patch(&mut bus, CODE_RIP + 2, CODE_PHYSICAL, 0x03);
        let (state, report) = run_load(&mut session, &mut bus);
        assert_eq!(report.admission, od_host::PagedFetchAdmission::Admitted);
        assert!(report.fetched_generation > generation);
        generation = report.fetched_generation;
        assert_eq!(state.gpr[Register::Rax as usize], LOADED_VALUE);
        assert_eq!(session.admissions(), 1 + round);
        assert_eq!(
            jit_free_slots(),
            0,
            "exactly the new block's region is live"
        );
    }

    // Patching to bytes outside the allowlist drops the stale block and admits
    // nothing: the region is free again without any reset.
    tracked_patch(&mut bus, CODE_RIP + 2, CODE_PHYSICAL, 0x0b);
    let mut state = fetch_state(CODE_RIP);
    let error = session
        .execute(
            &mut state,
            &mut bus,
            CR3,
            EFER,
            fetch_context(PrivilegeLevel::Ring0),
        )
        .unwrap_err();
    assert!(matches!(
        error.kind,
        od_host::PagedFetchedLoadErrorKind::UnsupportedForm { .. }
    ));
    assert_eq!(state.gpr[Register::Rax as usize], INITIAL_RAX);
    assert!(!session.has_admitted_block());
    assert_eq!(jit_free_slots(), ALL_FREE, "invalidation freed the region");
    assert!(!session.reset(), "nothing left to reset");

    tracked_patch(&mut bus, CODE_RIP + 2, CODE_PHYSICAL, 0x03);
    let (state, restored) = run_load(&mut session, &mut bus);
    assert_eq!(restored.admission, od_host::PagedFetchAdmission::Admitted);
    assert_eq!(state.gpr[Register::Rax as usize], LOADED_VALUE);
    assert_eq!(jit_free_slots(), 0);
    assert!(session.reset());
    assert_eq!(jit_free_slots(), ALL_FREE);
}
