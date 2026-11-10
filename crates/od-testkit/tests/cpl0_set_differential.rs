//! Differential check for the combined CPL0 profile set (S2-45a): on the
//! integer, branch, memory and stack inputs of the existing corpora, a state
//! without control registers run through `run` must match the same state with
//! CR0/CR4 modeled run through the set, up to the fields the set requires.
//!
//! Limits: no corpus contains CLI (0xfa), so the plain set and the set with
//! CLI execute the same instructions here and CLI is not covered
//! differentially; it is covered only by the targeted tests in
//! `crates/od-isa/tests/cpl0_profile_set.rs`. Only the BASE CR0/CR4 values are
//! used because `run` takes a state without control registers; values inside
//! the policy but not BASE (EM/TS, OSFXSR/OSXMMEXCPT) have no `run` baseline
//! and are covered by the targeted tests as well.

use od_isa::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, RunConfig, run,
    run_with_cpl0_profile_set,
};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, CliProfile, CpuActivity, GuestMemory, InstructionMode,
    LongModeState, MemoryError, OperandSize, PrivilegeLevel, StiShadow,
};
use od_testkit::generate::generate_fixture;
use od_testkit::memory_corpus::{
    MEMORY_CORPUS_RAM_LEN, MEMORY_CORPUS_SEQUENCES, S3_34A_SEEDS, generate_memory_corpus_input,
};

#[derive(Clone, Debug, Eq, PartialEq)]
struct Memory {
    regions: Vec<(u64, Vec<u8>)>,
}

impl Memory {
    fn slot(&self, address: u64, len: usize) -> Option<(usize, usize)> {
        self.regions
            .iter()
            .enumerate()
            .find_map(|(index, (base, bytes))| {
                let offset = address.checked_sub(*base)? as usize;
                (offset.checked_add(len)? <= bytes.len()).then_some((index, offset))
            })
    }
}

impl GuestMemory for Memory {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        let (index, offset) = self.slot(address, 1).ok_or(MemoryError::Unmapped {
            address,
            access: AccessKind::Fetch,
        })?;
        Ok(self.regions[index].1[offset])
    }

    fn read(&mut self, address: u64, _: AccessContext, out: &mut [u8]) -> Result<(), MemoryError> {
        let (index, offset) = self.slot(address, out.len()).ok_or(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })?;
        out.copy_from_slice(&self.regions[index].1[offset..offset + out.len()]);
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, data: &[u8]) -> Result<(), MemoryError> {
        let (index, offset) = self
            .slot(address, data.len())
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Write,
            })?;
        self.regions[index].1[offset..offset + data.len()].copy_from_slice(data);
        Ok(())
    }

    fn atomic_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!()
    }

    fn atomic_fetch_add_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!()
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!()
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        unreachable!()
    }
}

struct Case {
    state: LongModeState,
    memory: Memory,
    stop_rip: u64,
}

fn config(case: &Case) -> RunConfig {
    RunConfig {
        instruction_budget: 64,
        stop_rip: Some(case.stop_rip),
        context: AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        },
    }
}

#[derive(Default)]
struct Tally {
    cases: usize,
    stopped_at_end: usize,
    other_exits: usize,
}

/// Run the case through `run` and through the set (with and without CLI) and
/// require identical observable results.
fn check(case: Case, tally: &mut Tally, label: &str) {
    let cli = CliProfile::NonPviLong64V1;
    let plain = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let with_cli = plain.with_cli(&cli);

    let mut base_state = case.state.clone();
    let mut base_memory = case.memory.clone();
    let base = run(&mut base_state, &mut base_memory, config(&case), || false);

    for set in [&plain, &with_cli] {
        let mut state = case.state.clone();
        state.cr0 = Some(0x8001_0033);
        state.cr4 = Some(0x20);
        state.activity = Some(CpuActivity::Running);
        state.sti_shadow = Some(StiShadow::Clear);
        let mut memory = case.memory.clone();
        let result = run_with_cpl0_profile_set(
            &mut state,
            &mut memory,
            config(&case),
            od_isa::INTEGER_FLAGS,
            set,
            None,
            || false,
        )
        .unwrap_or_else(|error| panic!("{label}: admission failed: {error:?}"));
        assert_eq!(
            result.exit,
            Cpl0ProfileSetExit::Run(base.exit.clone()),
            "{label}"
        );
        assert_eq!(
            result.instructions_executed, base.instructions_executed,
            "{label}"
        );
        assert_eq!(result.rflags_defined, base.rflags_defined, "{label}");
        // The set requires CR0/CR4, activity and the STI shadow; they must
        // come back unchanged and everything else must match `run`.
        assert_eq!(state.cr0, Some(0x8001_0033), "{label}");
        assert_eq!(state.cr4, Some(0x20), "{label}");
        assert_eq!(state.activity, Some(CpuActivity::Running), "{label}");
        assert_eq!(state.sti_shadow, Some(StiShadow::Clear), "{label}");
        state.cr0 = None;
        state.cr4 = None;
        state.activity = None;
        state.sti_shadow = None;
        assert_eq!(state, base_state, "{label}");
        assert_eq!(memory, base_memory, "{label}");
    }
    tally.cases += 1;
    if base.exit == od_isa::execute::RunExit::StopAddress {
        tally.stopped_at_end += 1;
    } else {
        tally.other_exits += 1;
    }
}

struct SplitMix(u64);

impl SplitMix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

#[test]
fn integer_and_branch_corpus_matches_run() {
    let mut tally = Tally::default();
    for seed in [
        0x5332_3435_0000_0001u64,
        0x5332_3435_0000_0002,
        0x5332_3435_0000_0003,
    ] {
        for sequence in 0..400u32 {
            let fixture = generate_fixture(seed, sequence);
            let program = fixture.program.bytes().expect("generated program decodes");
            let mut state = LongModeState {
                rip: 0x1000,
                rflags: 2,
                ..LongModeState::default()
            };
            state.gpr[0] = fixture.initial_state.rax.0;
            state.gpr[1] = fixture.initial_state.rcx.0;
            state.gpr[2] = fixture.initial_state.rdx.0;
            state.gpr[3] = fixture.initial_state.rbx.0;
            let stop_rip = 0x1000 + program.len() as u64;
            check(
                Case {
                    state,
                    memory: Memory {
                        regions: vec![(0x1000, program)],
                    },
                    stop_rip,
                },
                &mut tally,
                &format!("integer seed={seed:#x} sequence={sequence}"),
            );
        }
    }
    assert_eq!(tally.cases, 1200);
    assert!(tally.stopped_at_end > 0);
    println!(
        "integer/branch: {} cases, {} stopped at code end, {} other exits",
        tally.cases, tally.stopped_at_end, tally.other_exits
    );
}

#[test]
fn memory_corpus_matches_run() {
    let mut tally = Tally::default();
    for seed in S3_34A_SEEDS {
        for sequence in MEMORY_CORPUS_SEQUENCES {
            for index in 0..120 {
                let input = generate_memory_corpus_input(seed, index, Some(sequence));
                let program = sequence.bytes().to_vec();
                let stop_rip = 0x9e00 + program.len() as u64;
                let mut state = LongModeState {
                    rip: 0x9e00,
                    rflags: 2 | input.flags,
                    gpr: input.gpr,
                    ..LongModeState::default()
                };
                state.rflags &= !(1 << 9);
                check(
                    Case {
                        state,
                        memory: Memory {
                            regions: vec![
                                (0, input.ram[..MEMORY_CORPUS_RAM_LEN].to_vec()),
                                (0x9e00, program),
                            ],
                        },
                        stop_rip,
                    },
                    &mut tally,
                    &format!("memory seed={seed:#x} {sequence:?} index={index}"),
                );
            }
        }
    }
    assert_eq!(tally.cases, 3 * 3 * 120);
    assert!(tally.stopped_at_end > 0);
    println!(
        "memory: {} cases, {} stopped at code end, {} other exits",
        tally.cases, tally.stopped_at_end, tally.other_exits
    );
}

#[test]
fn stack_forms_match_run() {
    // PUSH r64, POP r64, PUSH imm8, CALL rel32 + POP, PUSH r8, POP r9.
    let programs: [&[u8]; 6] = [
        &[0x50, 0x59],
        &[0x6a, 0x05, 0x5a],
        &[0xe8, 0x00, 0x00, 0x00, 0x00, 0x58],
        &[0x41, 0x50, 0x41, 0x59],
        &[0x53, 0x53, 0x5e, 0x5f],
        &[0x50, 0xc3],
    ];
    let mut tally = Tally::default();
    let mut random = SplitMix(0x5332_3435_5354_4b01);
    for (index, program) in programs.iter().enumerate() {
        for round in 0..200 {
            let mut state = LongModeState {
                rip: 0x1000,
                rflags: 2 | (random.next() & 0x8d5),
                ..LongModeState::default()
            };
            state.rflags &= !(1 << 9);
            for value in &mut state.gpr {
                *value = random.next();
            }
            // Stack inside, at both edges of, and outside the 64-byte region.
            let rsp = match round % 4 {
                0 => 0x2000 + 8 * (random.next() % 8),
                1 => 0x2000 + 64,
                2 => 0x2000 + (random.next() % 64),
                _ => 0x2008,
            };
            state.gpr[4] = rsp;
            let mut stack = vec![0u8; 64];
            for chunk in stack.chunks_mut(8) {
                chunk.copy_from_slice(&random.next().to_le_bytes());
            }
            let stop_rip = 0x1000 + program.len() as u64;
            check(
                Case {
                    state,
                    memory: Memory {
                        regions: vec![(0x1000, program.to_vec()), (0x2000, stack)],
                    },
                    stop_rip,
                },
                &mut tally,
                &format!("stack program={index} round={round}"),
            );
        }
    }
    assert_eq!(tally.cases, programs.len() * 200);
    // The generator is seeded, so the split is exact: 884 cases reach the end
    // of the code and 316 end on a memory error from the 64-byte stack region.
    assert_eq!((tally.stopped_at_end, tally.other_exits), (884, 316));
    println!(
        "stack: {} cases, {} stopped at code end, {} other exits",
        tally.cases, tally.stopped_at_end, tally.other_exits
    );
}
