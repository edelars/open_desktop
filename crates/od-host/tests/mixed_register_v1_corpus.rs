#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::collections::HashSet;
use std::num::NonZeroU64;
use std::sync::atomic::AtomicU8;

use od_host::PublishedArenaBackend;
use od_isa::execute::{StepOutcome, step};
use od_isa::memory::{AccessContext, AccessKind, GuestMemory, MemoryError};
use od_isa::{LongModeState, OperandSize};
use od_jit::block::{CodeView, MixedRegisterV1Admission, NativeRegisterV1Admission};
use od_jit::dispatch::{
    DispatchControl, DispatchExit, MAX_MULTI_ENTRY_CACHE_CODE_BYTES,
    MAX_MULTI_ENTRY_CACHE_METADATA_BYTES, MultiEntryBlockCache, RegisterFlagsMode,
    run_multi_cached_mixed_register_v1_controlled,
};
use od_jit::runtime::RuntimeExitTag;
use od_jit::translation::{
    AddressSpaceId, BackingId, BackingVersion, CodeDependencySpan, CodeIdentity,
    ExecutionContextId, ExecutionContextSnapshot, MappingId, MappingVersion,
};
use od_testkit::{generate::generate_fixture, sha256_hex};

const BASE: u64 = 0x3345_0000;
const DF: u64 = 1 << 10;
const NOP: u8 = 0x90;
const CLD: u8 = 0xfc;
const STD: u8 = 0xfd;
const SEEDS: [u64; 3] = [
    0x5333_3435_0000_0001,
    0x5333_3435_0000_0002,
    0x5333_3435_0000_0003,
];
const CASES_PER_SEED: u32 = 400;

struct CodeOnly<'a>(&'a [u8]);

impl GuestMemory for CodeOnly<'_> {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        address
            .checked_sub(BASE)
            .and_then(|offset| usize::try_from(offset).ok())
            .and_then(|offset| self.0.get(offset))
            .copied()
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }

    fn read(&mut self, address: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        panic!("unexpected data read at {address:#x}")
    }

    fn write(&mut self, address: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        panic!("unexpected data write at {address:#x}")
    }

    fn atomic_exchange_integer(
        &mut self,
        address: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        panic!("unexpected atomic exchange at {address:#x}")
    }

    fn atomic_fetch_add_integer(
        &mut self,
        address: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        panic!("unexpected atomic fetch-add at {address:#x}")
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        address: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        panic!("unexpected atomic compare-exchange at {address:#x}")
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        panic!("unexpected fence")
    }
}

#[derive(Debug)]
struct Input {
    code: Vec<u8>,
    initial: LongModeState,
    stop_rip: Option<u64>,
    stop_requested: bool,
    budget: u64,
}

impl Input {
    fn canonical_bytes(&self) -> Vec<u8> {
        // The fixture base and every unlisted auxiliary field are fixed.
        let mut bytes = (self.code.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(&self.code);
        for value in self.initial.gpr {
            bytes.extend(value.to_le_bytes());
        }
        for value in [
            BASE,
            self.initial.rip,
            self.initial.rflags,
            self.budget,
            self.stop_rip.unwrap_or(0),
        ] {
            bytes.extend(value.to_le_bytes());
        }
        bytes.push(u8::from(self.stop_requested));
        bytes
    }
}

fn generate_case(seed: u64, index: u32) -> Input {
    let fixture = generate_fixture(seed, index);
    let original = fixture.program.bytes().unwrap();
    // The v1 generator starts MOV imm64 then ADD/SUB/CMP r64. The optional
    // JZ/JNZ and its target move together, so the branch now targets helper 2.
    let (prefix, branch_tail) = original.split_at(13);
    assert_eq!(prefix[0], 0x48);
    assert!((0xb8..=0xbb).contains(&prefix[1]));
    assert_eq!(prefix[10], 0x48);
    assert!(matches!(prefix[11], 0x03 | 0x2b | 0x3b));
    assert!(
        branch_tail.is_empty()
            || (branch_tail.len() == 12
                && matches!(branch_tail[0], 0x74 | 0x75)
                && branch_tail[1] == 10)
    );
    let (first, second) = if (u64::from(index / 12) ^ seed) & 1 == 0 {
        (CLD, STD)
    } else {
        (STD, CLD)
    };
    let mut code = prefix.to_vec();
    let first_rip = BASE + code.len() as u64;
    code.extend_from_slice(&[first, NOP]);
    code.extend_from_slice(branch_tail);
    let second_rip = BASE + code.len() as u64;
    // Partial flag writer after the second helper preserves CF and DF.
    code.extend_from_slice(&[second, 0x48, 0xff, 0xc0]);
    let code_end = BASE + code.len() as u64;
    // Keep more than 1,000 cases that actually run the full mixed route.
    let (budget, stop_rip, stop_requested) = match index % 64 {
        0 => (0, None, false),
        1 => (2, None, false),
        2 => (3, None, false),
        3 => (16, Some(first_rip), false),
        4 => (16, Some(first_rip + 1), false),
        5 => (16, Some(second_rip), false),
        6 => (16, Some(second_rip + 1), false),
        7 => (16, None, true),
        8 => (16, Some(code_end), false),
        _ => (16, None, false),
    };
    let mut initial = LongModeState {
        rip: BASE,
        ..LongModeState::default()
    };
    let source = fixture.initial_state;
    let generated = [
        source.rax.get(),
        source.rcx.get(),
        source.rdx.get(),
        source.rbx.get(),
    ];
    initial.gpr[..4].copy_from_slice(&generated);
    for (index, register) in initial.gpr[4..].iter_mut().enumerate() {
        *register = generated[index % 4].rotate_left((index + 3) as u32);
    }
    // Distinguish both transition directions and watch preserved CF/IF.
    initial.rflags =
        0x202 | ((generated[0] >> 13) & 0x8d5) | if (index / 24) & 1 == 0 { DF } else { 0 };
    Input {
        code,
        initial,
        stop_rip,
        stop_requested,
        budget,
    }
}

#[derive(Debug, Eq, PartialEq)]
struct Oracle {
    state: LongModeState,
    exit: DispatchExit,
    remaining: u64,
    native: u64,
    helpers: [u64; 3],
    branches: [u64; 2],
    df_changes: [u64; 2],
}

fn interpret(input: &Input, label: &str) -> Oracle {
    let code_end = BASE + input.code.len() as u64;
    let mut memory = CodeOnly(&input.code);
    let mut state = input.initial.clone();
    let mut remaining = input.budget;
    let mut native = 0;
    let mut helpers = [0; 3];
    let mut branches = [0; 2];
    let mut df_changes = [0; 2];
    let exit = loop {
        if input.stop_rip == Some(state.rip) {
            break DispatchExit::Runtime(RuntimeExitTag::StopAddress);
        }
        if input.stop_requested {
            break DispatchExit::Runtime(RuntimeExitTag::StopRequested);
        }
        if state.rip == code_end {
            break DispatchExit::CodeEnd;
        }
        if remaining == 0 {
            break DispatchExit::BudgetExhausted;
        }
        let opcode = input.code[(state.rip - BASE) as usize];
        let flags_before = state.rflags;
        let outcome = step(&mut state, &mut memory, AccessContext::default())
            .unwrap_or_else(|error| panic!("{label} interpreter: {error:?}"));
        assert!(
            matches!(outcome, StepOutcome::Advanced | StepOutcome::Branched),
            "{label}: {outcome:?}"
        );
        match opcode {
            NOP => {
                helpers[0] += 1;
                assert_eq!(state.rflags, flags_before, "{label} NOP flags");
            }
            CLD | STD => {
                let set = opcode == STD;
                helpers[1 + usize::from(set)] += 1;
                assert_eq!(
                    state.rflags,
                    (flags_before & !DF) | if set { DF } else { 0 },
                    "{label} direction flags"
                );
                if (state.rflags ^ flags_before) & DF != 0 {
                    df_changes[usize::from(set)] += 1;
                }
            }
            _ => {
                native += 1;
                if matches!(opcode, 0x74 | 0x75) {
                    branches[usize::from(outcome == StepOutcome::Branched)] += 1;
                }
            }
        }
        remaining -= 1;
    };
    Oracle {
        state,
        exit,
        remaining,
        native,
        helpers,
        branches,
        df_changes,
    }
}

#[test]
fn generated_mixed_register_v1_corpus_runs_twice_on_published_arena() {
    let mut arena = PublishedArenaBackend::new(8).expect("real ARM64 MAP_JIT arena");
    let mut canonical_inputs = HashSet::new();
    let mut unique_programs = HashSet::new();
    let mut corpus_bytes = Vec::new();
    let mut exit_counts = [0; 4];
    let mut helper_counts = [0; 3];
    let mut branch_counts = [0; 2];
    let mut direction_changes = [0; 2];
    let mut native_steps = 0;
    let mut warm_hits = 0;
    let context = ExecutionContextSnapshot::native_register_v1_fixture(
        ExecutionContextId::from_u64(1).unwrap(),
    );
    let identity = NonZeroU64::new(1).unwrap();
    for seed in SEEDS {
        for index in 0..CASES_PER_SEED {
            let input = generate_case(seed, index);
            let label = format!("seed={seed:#018x} case={index}, bytes={:02x?}", input.code);
            let canonical = input.canonical_bytes();
            corpus_bytes.extend_from_slice(&canonical);
            assert!(
                canonical_inputs.insert(canonical),
                "{label} duplicate complete input"
            );
            unique_programs.insert(input.code.clone());
            let view = CodeView::new(BASE, &input.code).unwrap();
            let admission = MixedRegisterV1Admission::from_code(view)
                .unwrap_or_else(|error| panic!("{label} mixed admission: {error:?}"));
            assert!(
                NativeRegisterV1Admission::from_code(view).is_err(),
                "{label} admitted by strict profile"
            );
            let dependencies = [CodeDependencySpan {
                start_rip: BASE,
                end_rip: view.code_end(),
                identity: CodeIdentity {
                    address_space: AddressSpaceId::new(identity),
                    mapping: MappingId::new(identity),
                    mapping_version: MappingVersion::new(identity),
                    backing: BackingId::new(identity),
                    backing_version: BackingVersion::new(identity),
                    executable: true,
                },
            }];
            let stop = AtomicU8::new(u8::from(input.stop_requested));
            let control = DispatchControl {
                stop_rip: input.stop_rip,
                stop_requested: &stop,
                event_requested: None,
            };
            let expected = interpret(&input, &label);
            assert_eq!(
                expected,
                interpret(&input, &label),
                "{label} nondeterministic oracle"
            );
            let helper_steps: u64 = expected.helpers.iter().sum();
            let exit_index = match expected.exit {
                DispatchExit::CodeEnd => 0,
                DispatchExit::BudgetExhausted => 1,
                DispatchExit::Runtime(RuntimeExitTag::StopAddress) => 2,
                DispatchExit::Runtime(RuntimeExitTag::StopRequested) => 3,
                exit => panic!("{label} unexpected expected exit: {exit:?}"),
            };
            exit_counts[exit_index] += 1;
            native_steps += expected.native;
            for (total, count) in helper_counts.iter_mut().zip(expected.helpers) {
                *total += count;
            }
            for (total, count) in branch_counts.iter_mut().zip(expected.branches) {
                *total += count;
            }
            for (total, count) in direction_changes.iter_mut().zip(expected.df_changes) {
                *total += count;
            }
            for (flags_label, flags) in [
                ("eager", RegisterFlagsMode::Eager),
                ("demand", RegisterFlagsMode::DemandFlagsCandidate),
            ] {
                let mut cache = MultiEntryBlockCache::new(
                    8,
                    MAX_MULTI_ENTRY_CACHE_CODE_BYTES,
                    MAX_MULTI_ENTRY_CACHE_METADATA_BYTES,
                )
                .unwrap();
                for repeat in 0..2 {
                    let mut state = input.initial.clone();
                    let previous = cache.counters();
                    let result = run_multi_cached_mixed_register_v1_controlled(
                        view,
                        &admission,
                        context,
                        &dependencies,
                        &mut state,
                        input.budget,
                        control,
                        &mut cache,
                        &mut arena,
                        flags,
                    )
                    .unwrap_or_else(|error| {
                        panic!("{label} {flags_label} repeat={repeat}: {error:?}")
                    });
                    let actual = result.dispatch;
                    assert_eq!(
                        state, expected.state,
                        "{label} {flags_label} repeat={repeat} state"
                    );
                    assert_eq!(
                        (
                            actual.exit,
                            actual.remaining_budget,
                            actual.retired_native,
                            actual.retired_interpreter,
                            result.counters.helper_calls,
                            result.counters.interpreter_steps
                        ),
                        (
                            expected.exit,
                            expected.remaining,
                            expected.native,
                            helper_steps,
                            helper_steps,
                            helper_steps
                        ),
                        "{label} {flags_label} repeat={repeat} retirement/control",
                    );
                    let counters = cache.counters();
                    assert_eq!(counters.evictions, 0, "{label}");
                    if repeat == 0 {
                        assert_eq!(counters.hits, 0, "{label}");
                        assert_eq!(
                            counters.successful_publications, actual.blocks_executed,
                            "{label}"
                        );
                        assert!(
                            result.counters.compiled_instructions >= expected.native,
                            "{label}"
                        );
                    } else {
                        assert_eq!(result.counters.compiled_instructions, 0, "{label}");
                        assert_eq!(counters.misses, previous.misses, "{label}");
                        assert_eq!(
                            counters.successful_publications, previous.successful_publications,
                            "{label}"
                        );
                        assert_eq!(
                            counters.hits - previous.hits,
                            actual.blocks_executed,
                            "{label}"
                        );
                        warm_hits += actual.blocks_executed;
                    }
                }
            }
        }
    }
    assert_eq!(canonical_inputs.len(), 1_200);
    assert!(
        unique_programs.len() >= 1_000,
        "only {} distinct programs",
        unique_programs.len()
    );
    assert_eq!(exit_counts, [1_011, 63, 105, 21]);
    assert!(helper_counts.into_iter().all(|count| count > 0));
    assert!(branch_counts.into_iter().all(|count| count > 0));
    assert!(direction_changes.into_iter().all(|count| count > 0));
    assert!(native_steps > 0 && warm_hits > 0);
    println!(
        "mixed inputs={} programs={} seeds={SEEDS:016x?} sha256={} \
         exits={exit_counts:?} helpers={helper_counts:?} branches={branch_counts:?} \
         df_changes={direction_changes:?} native_steps={native_steps} warm_hits={warm_hits}",
        canonical_inputs.len(),
        unique_programs.len(),
        sha256_hex(&corpus_bytes),
    );
}
