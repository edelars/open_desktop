//! PERF-24, groups G1 and G3 (ADR-0017): register `Mov`/`MovImmediate` and relative `Branch`
//! (Jcc) and `Jump` (JMP) in `execute::try_fast_step`. As for G2 (fast_step_g2_tests.rs), the fast
//! path is called directly on a copy of the state and compared with the general path (`run` of one
//! instruction under the CPL0 profile set): same CPU state, same `rflags_defined`, the same outcome
//! (`Advanced`, `Branched` or the same fault) and no call to the memory. On inadmissible conditions
//! it must answer "not handled" and change nothing.

use std::collections::BTreeSet;

use crate::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, RunConfig, RunExit, StepOutcome,
    run_with_cpl0_profile_set, try_fast_step,
};
use crate::fast_step_g2_tests::{CODE, assert_refused, context, state};
use crate::long_decode::{self, Instruction, OpcodeMap, Operand, Operation};
use crate::memory::AccessKind;
use crate::{
    AccessContext, GuestMemory, INTEGER_FLAGS, LongModeState, MemoryError, PrivilegeLevel,
    StiShadow,
};

const CF: u64 = 1;
const PF: u64 = 1 << 2;
const AF: u64 = 1 << 4;
const ZF: u64 = 1 << 6;
const SF: u64 = 1 << 7;
const OF: u64 = 1 << 11;
const TF: u64 = 1 << 8;
const RF: u64 = 1 << 16;

/// Code bytes at `base` for the fetch; every data access is counted and fails.
struct CodeAt {
    base: u64,
    bytes: Vec<u8>,
    data_calls: u32,
}

impl CodeAt {
    fn new(base: u64, bytes: &[u8]) -> Self {
        Self {
            base,
            bytes: bytes.to_vec(),
            data_calls: 0,
        }
    }
}

impl GuestMemory for CodeAt {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        address
            .checked_sub(self.base)
            .and_then(|offset| self.bytes.get(offset as usize))
            .copied()
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }

    fn read(&mut self, address: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        self.data_calls += 1;
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })
    }

    fn write(&mut self, address: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        self.data_calls += 1;
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Write,
        })
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        self.data_calls += 1;
        Ok(())
    }
}

fn decode_at(base: u64, bytes: &[u8]) -> (Instruction, CodeAt) {
    let mut memory = CodeAt::new(base, bytes);
    let instruction = long_decode::decode(&mut memory, base, context(PrivilegeLevel::Ring0))
        .unwrap_or_else(|error| panic!("{bytes:02x?}: {error:?}"));
    assert_eq!(usize::from(instruction.length), bytes.len(), "{bytes:02x?}");
    (instruction, memory)
}

/// What the general path did with one instruction.
#[derive(Debug, PartialEq)]
enum General {
    /// Retired: the instruction budget of one ran out.
    Retired,
    Fault(crate::ArchitecturalFault),
}

/// The general path: one instruction (budget 1, no stop address) through `run` of the CPL0
/// profile set, at `start.rip`.
fn general(bytes: &[u8], start: &LongModeState, defined: u64) -> (LongModeState, u64, General) {
    let plain = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = start.clone();
    let mut memory = CodeAt::new(start.rip, bytes);
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: context(PrivilegeLevel::Ring0),
        },
        defined,
        &plain,
        None,
        || false,
    )
    .unwrap();
    assert_eq!(memory.data_calls, 0, "{bytes:02x?}");
    let general = match result.exit {
        Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted) => {
            assert_eq!(result.instructions_executed, 1, "{bytes:02x?}");
            General::Retired
        }
        Cpl0ProfileSetExit::Run(RunExit::Fault(fault)) => {
            assert_eq!(result.instructions_executed, 0, "{bytes:02x?}");
            General::Fault(fault)
        }
        other => panic!("{bytes:02x?}: {other:?}"),
    };
    (state, result.rflags_defined, general)
}

/// The fast path on a copy of `start`; it must take the instruction.
fn fast(
    bytes: &[u8],
    start: &LongModeState,
    defined: u64,
    policy: bool,
) -> (LongModeState, u64, StepOutcome) {
    let (instruction, mut memory) = decode_at(start.rip, bytes);
    let mut state = start.clone();
    let mut defined = defined;
    let outcome = try_fast_step(
        &mut state,
        &mut memory,
        context(PrivilegeLevel::Ring0),
        &mut defined,
        &instruction,
        policy,
    );
    assert_eq!(memory.data_calls, 0, "{bytes:02x?}");
    let Some(Ok(outcome)) = outcome else {
        panic!("{bytes:02x?}: not taken: {outcome:?}");
    };
    (state, defined, outcome)
}

/// The starting definedness masks: all flags, none, and all but AF and OF.
const START_MASKS: [u64; 3] = [INTEGER_FLAGS, 0, INTEGER_FLAGS & !(AF | OF)];

/// The register-to-register and immediate MOV encodings of every size.
fn mov_encodings() -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    let sizes: [(usize, &[u8], bool); 4] = [
        (1, &[], false),
        (2, &[0x66], false),
        (4, &[], false),
        (8, &[], true),
    ];
    let pairs: [(u8, u8); 8] = [
        (0, 1),
        (3, 2),
        (6, 7),
        (0, 0),
        (4, 5),
        (9, 10),
        (15, 8),
        (7, 4),
    ];
    for &(size, legacy, wide) in &sizes {
        let wordsize = u8::from(size != 1);
        for &(dst, src) in &pairs {
            // 4..=7 as bytes are AH..BH without a REX prefix and SPL..DIL with one.
            let rex_variants: &[bool] =
                if size == 1 && ((4..8).contains(&dst) || (4..8).contains(&src)) {
                    &[false, true]
                } else {
                    &[false]
                };
            for &force_rex in rex_variants {
                let tag = format!("size {size} dst {dst} src {src} rex {force_rex}");
                let head = |reg: u8, rm: u8| {
                    let bits = (u8::from(wide) << 3) | ((reg >> 3) << 2) | (rm >> 3);
                    let mut head = legacy.to_vec();
                    head.extend((bits != 0 || force_rex).then_some(0x40 | bits));
                    head
                };
                // MOV r/m, r (88/89) and MOV r, r/m (8A/8B), register forms.
                let mut bytes = head(src, dst);
                bytes.extend([0x88 + wordsize, 0xc0 | (dst & 7) | ((src & 7) << 3)]);
                out.push((format!("mov rm,reg {tag}"), bytes));
                let mut bytes = head(dst, src);
                bytes.extend([0x8a + wordsize, 0xc0 | (src & 7) | ((dst & 7) << 3)]);
                out.push((format!("mov reg,rm {tag}"), bytes));
                // MOV r/m, imm (C6/C7 /0) and MOV r, imm (B0+r / B8+r).
                for immediate in [0u64, 1, 0x7f, 0x80, 0xff, 0x8000, 0x8000_0000, 0xffff_ffff] {
                    let mut bytes = head(0, dst);
                    bytes.extend([0xc6 + wordsize, 0xc0 | (dst & 7)]);
                    match size {
                        1 => bytes.push(immediate as u8),
                        2 => bytes.extend((immediate as u16).to_le_bytes()),
                        _ => bytes.extend((immediate as u32).to_le_bytes()),
                    }
                    out.push((format!("mov rm,imm {immediate:#x} {tag}"), bytes));
                }
                let short: &[u64] = if size == 8 {
                    &[
                        0,
                        1,
                        0x8000_0000,
                        0x8000_0000_0000_0000,
                        u64::MAX,
                        0x1234_5678_9abc_def0,
                    ]
                } else {
                    &[0, 1, 0x7f, 0x80, 0xff, 0x8000, 0xffff_ffff]
                };
                for &immediate in short {
                    let mut bytes = head(0, dst);
                    bytes.push(if size == 1 { 0xb0 } else { 0xb8 } + (dst & 7));
                    match size {
                        1 => bytes.push(immediate as u8),
                        2 => bytes.extend((immediate as u16).to_le_bytes()),
                        4 => bytes.extend((immediate as u32).to_le_bytes()),
                        _ => bytes.extend(immediate.to_le_bytes()),
                    }
                    out.push((format!("mov reg,imm {immediate:#x} {tag}"), bytes));
                }
            }
        }
    }
    out
}

#[test]
fn every_g1_register_mov_equals_the_general_path() {
    let mut forms = BTreeSet::new();
    let mut sizes = BTreeSet::new();
    let mut high_bytes = 0u64;
    let mut cases = 0u64;
    let starts: Vec<LongModeState> = (0..10)
        .map(|seed| state(seed, 2 | if seed % 2 == 0 { CF | ZF } else { SF | OF }))
        .collect();
    for (label, bytes) in mov_encodings() {
        let (instruction, _) = decode_at(CODE, &bytes);
        match instruction.operation {
            Operation::Mov {
                dst: Operand::Register(dst),
                src,
            } => {
                assert!(!matches!(src, Operand::Memory(_)), "{label}");
                forms.insert(if matches!(src, Operand::Immediate(_)) {
                    "Mov imm"
                } else {
                    "Mov reg"
                });
                high_bytes += u64::from(format!("{dst:?}").contains("shift: 8"));
                if let Operand::Register(src) = src {
                    high_bytes += u64::from(format!("{src:?}").contains("shift: 8"));
                }
            }
            Operation::MovImmediate {
                dst: Operand::Register(dst),
                ..
            } => {
                forms.insert("MovImmediate");
                high_bytes += u64::from(format!("{dst:?}").contains("shift: 8"));
            }
            other => panic!("{label}: {bytes:02x?} decodes to {other:?}, not a G1 operation"),
        }
        sizes.insert(instruction.operand_size.bytes());
        for start in &starts {
            for defined in START_MASKS {
                let (expected, expected_defined, general) = general(&bytes, start, defined);
                assert_eq!(general, General::Retired, "{label}");
                for policy in [false, true] {
                    let (state, fast_defined, outcome) = fast(&bytes, start, defined, policy);
                    assert_eq!(outcome, StepOutcome::Advanced, "{label}: {bytes:02x?}");
                    assert_eq!(state, expected, "{label}: {bytes:02x?}");
                    assert_eq!(fast_defined, expected_defined, "{label}: {bytes:02x?}");
                }
                cases += 1;
            }
        }
    }
    assert!(forms.contains("Mov reg"), "{forms:?}");
    assert!(
        forms.contains("MovImmediate") || forms.contains("Mov imm"),
        "{forms:?}"
    );
    assert_eq!(sizes, BTreeSet::from([1, 2, 4, 8]));
    assert!(high_bytes > 10, "{high_bytes}");
    assert!(cases > 10_000, "{cases}");
}

/// Relative Jcc (rel8, rel32) and JMP (rel8, rel32) with a spread of displacements.
fn branch_encodings() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let short: [i8; 6] = [0, 2, -2, 0x7f, -0x80, 0x10];
    let near: [i32; 6] = [0, 0x10, -0x10, 0x7fff_0000, -0x2000, i32::MIN];
    for cc in 0u8..16 {
        for displacement in short {
            out.push((
                format!("jcc{cc} rel8 {displacement}"),
                vec![0x70 + cc, displacement as u8],
            ));
        }
        for displacement in near {
            let mut bytes = vec![0x0f, 0x80 + cc];
            bytes.extend(displacement.to_le_bytes());
            out.push((format!("jcc{cc} rel32 {displacement}"), bytes));
        }
    }
    for displacement in short {
        out.push((
            format!("jmp rel8 {displacement}"),
            vec![0xeb, displacement as u8],
        ));
    }
    for displacement in near {
        let mut bytes = vec![0xe9];
        bytes.extend(displacement.to_le_bytes());
        out.push((format!("jmp rel32 {displacement}"), bytes));
    }
    out
}

/// Every combination of the five status flags the conditions read.
fn condition_flags() -> impl Iterator<Item = u64> {
    (0u64..32).map(|bits| {
        2 | [CF, PF, ZF, SF, OF]
            .iter()
            .enumerate()
            .filter(|(index, _)| bits & (1 << index) != 0)
            .fold(0, |acc, (_, flag)| acc | flag)
    })
}

/// Whether the decoded branch is taken under `rflags` (a JMP always is).
fn taken(instruction: &Instruction, rflags: u64) -> bool {
    match instruction.operation {
        Operation::Branch { condition, .. } => condition.evaluate(rflags),
        Operation::Jump { .. } => true,
        other => panic!("{other:?} is not a G3 operation"),
    }
}

#[test]
fn every_g3_relative_branch_equals_the_general_path() {
    let mut taken_cases = 0u64;
    let mut not_taken_cases = 0u64;
    let mut conditions = BTreeSet::new();
    for (label, bytes) in branch_encodings() {
        let (instruction, _) = decode_at(CODE, &bytes);
        if let Operation::Branch { condition, .. } = instruction.operation {
            conditions.insert(format!("{condition:?}"));
        }
        for rflags in condition_flags() {
            let start = state(5, rflags);
            let is_taken = taken(&instruction, rflags);
            for defined in START_MASKS {
                let (expected, expected_defined, general) = general(&bytes, &start, defined);
                assert_eq!(general, General::Retired, "{label}");
                let (state, fast_defined, outcome) = fast(&bytes, &start, defined, true);
                let expected_outcome = if is_taken {
                    StepOutcome::Branched
                } else {
                    StepOutcome::Advanced
                };
                assert_eq!(outcome, expected_outcome, "{label}: rflags {rflags:#x}");
                assert_eq!(state, expected, "{label}: rflags {rflags:#x}");
                assert_eq!(
                    fast_defined, expected_defined,
                    "{label}: rflags {rflags:#x}"
                );
            }
            if is_taken {
                taken_cases += 1;
            } else {
                not_taken_cases += 1;
            }
        }
    }
    assert_eq!(conditions.len(), 16, "{conditions:?}");
    assert!(
        taken_cases > 500 && not_taken_cases > 500,
        "{taken_cases} {not_taken_cases}"
    );
}

#[test]
fn a_non_canonical_target_is_the_same_fault_and_changes_nothing() {
    // (where the instruction lies, the instruction): each target leaves the canonical range
    // (upward past 0x0000_7fff_ffff_ffff, downward below 0xffff_8000_0000_0000).
    let cases: Vec<(u64, Vec<u8>)> = vec![
        (0x0000_7fff_ffff_ff90, vec![0xeb, 0x7f]),
        (0x0000_7fff_ffff_ff00, vec![0xe9, 0x00, 0x02, 0x00, 0x00]),
        (0x0000_7fff_ffff_ff90, vec![0x74, 0x7f]),
        (
            0x0000_7fff_ffff_ff00,
            vec![0x0f, 0x85, 0x00, 0x02, 0x00, 0x00],
        ),
        (0xffff_8000_0000_0010, vec![0xeb, 0x80]),
        (0xffff_8000_0000_0010, vec![0xe9, 0x00, 0xff, 0xff, 0xff]),
        (0xffff_8000_0000_0010, vec![0x75, 0x80]),
        (
            0xffff_8000_0000_0010,
            vec![0x0f, 0x84, 0x00, 0xff, 0xff, 0xff],
        ),
        (
            0x0000_7fff_ffff_ff00,
            vec![0x0f, 0x8c, 0x00, 0x00, 0x00, 0x7f],
        ),
    ];
    let mut faults = 0;
    let mut not_taken = 0;
    for (base, bytes) in &cases {
        let (instruction, _) = decode_at(*base, bytes);
        for rflags in condition_flags() {
            let mut start = state(5, rflags);
            start.rip = *base;
            for defined in START_MASKS {
                let (expected, expected_defined, general) = general(bytes, &start, defined);
                let (state, fast_defined, outcome) = fast(bytes, &start, defined, true);
                if taken(&instruction, rflags) {
                    let StepOutcome::Fault(fault) = outcome else {
                        panic!("{bytes:02x?} at {base:#x}: {outcome:?}");
                    };
                    assert_eq!(fault.vector, 13, "{bytes:02x?}");
                    assert_eq!(general, General::Fault(fault), "{bytes:02x?} at {base:#x}");
                    assert_eq!(state, start, "{bytes:02x?} at {base:#x}");
                    assert_eq!(fast_defined, defined, "{bytes:02x?} at {base:#x}");
                    faults += 1;
                } else {
                    assert_eq!(general, General::Retired, "{bytes:02x?} at {base:#x}");
                    assert_eq!(outcome, StepOutcome::Advanced, "{bytes:02x?}");
                    not_taken += 1;
                }
                assert_eq!(state, expected, "{bytes:02x?} at {base:#x}");
                assert_eq!(fast_defined, expected_defined, "{bytes:02x?} at {base:#x}");
            }
        }
    }
    assert!(faults > 100 && not_taken > 100, "{faults} {not_taken}");
}

/// A sample of G1 and G3 forms.
const MOV_SAMPLE: &[&[u8]] = &[
    &[0x48, 0x89, 0xc8],                         // mov rax, rcx
    &[0x8a, 0xe3],                               // mov ah, bl
    &[0x66, 0x8b, 0xc1],                         // mov ax, cx
    &[0xb8, 0x07, 0x00, 0x00, 0x00],             // mov eax, 7
    &[0x48, 0xc7, 0xc0, 0xff, 0xff, 0xff, 0xff], // mov rax, -1
    &[0xb0, 0x12],                               // mov al, 0x12
];
const BRANCH_SAMPLE: &[&[u8]] = &[
    &[0xeb, 0x05],                         // jmp +5
    &[0xe9, 0x00, 0x01, 0x00, 0x00],       // jmp +0x100
    &[0x74, 0x05],                         // jz +5
    &[0x75, 0x05],                         // jnz +5
    &[0x0f, 0x8c, 0x10, 0x00, 0x00, 0x00], // jl +0x10
];

#[test]
fn inadmissible_conditions_are_refused_without_side_effects() {
    let ring0 = context(PrivilegeLevel::Ring0);
    let base = || state(5, 2 | CF);
    for &bytes in MOV_SAMPLE.iter().chain(BRANCH_SAMPLE) {
        // Control: the plain case is taken, so each refusal below comes from its condition.
        let (_, _, _) = fast(bytes, &base(), INTEGER_FLAGS, true);
        for privilege in [
            PrivilegeLevel::Ring1,
            PrivilegeLevel::Ring2,
            PrivilegeLevel::Ring3,
        ] {
            assert_refused("cpl", bytes, base(), context(privilege), |_, _| {});
        }
        assert_refused("rf", bytes, base(), ring0, |s, _| s.rflags |= RF);
        assert_refused("tf", bytes, base(), ring0, |s, _| s.rflags |= TF);
        assert_refused("shadow", bytes, base(), ring0, |s, _| {
            s.sti_shadow = Some(StiShadow::Blocking);
        });
        assert_refused("no shadow model", bytes, base(), ring0, |s, _| {
            s.sti_shadow = None;
        });
        for group in [0xf0u8, 0xf2, 0xf3] {
            assert_refused("group1", bytes, base(), ring0, |_, i| {
                i.prefixes.group1 = Some(group);
            });
        }
        for segment in [0x64u8, 0x65] {
            assert_refused("fs/gs", bytes, base(), ring0, |_, i| {
                i.prefixes.segment = Some(segment);
            });
        }
    }
    // MOV: the other segment prefixes (zero base, no memory operand) are taken, as for G2.
    for &bytes in MOV_SAMPLE {
        for segment in [0x26u8, 0x2e, 0x36, 0x3e] {
            let (mut instruction, mut memory) = decode_at(CODE, bytes);
            instruction.prefixes.segment = Some(segment);
            let mut state = base();
            let mut defined = INTEGER_FLAGS;
            assert!(
                try_fast_step(
                    &mut state,
                    &mut memory,
                    ring0,
                    &mut defined,
                    &instruction,
                    true
                )
                .is_some(),
                "{bytes:02x?} segment {segment:#x}"
            );
        }
    }
    // Jcc and JMP: any prefix at all (the admission of `executable`), and a foreign opcode map.
    for &bytes in BRANCH_SAMPLE {
        for segment in [0x26u8, 0x2e, 0x36, 0x3e] {
            assert_refused("segment", bytes, base(), ring0, |_, i| {
                i.prefixes.segment = Some(segment);
            });
        }
        assert_refused("66", bytes, base(), ring0, |_, i| {
            i.prefixes.operand_override = true;
        });
        assert_refused("67", bytes, base(), ring0, |_, i| {
            i.prefixes.address_override = true;
        });
        for rex in [0x40u8, 0x48, 0x41] {
            assert_refused("rex", bytes, base(), ring0, |_, i| {
                i.prefixes.rex = Some(rex);
            });
        }
        assert_refused("map", bytes, base(), ring0, |_, i| {
            i.opcode.map = OpcodeMap::Escape0f38;
        });
    }
    // JMP rel is a primary-map opcode only.
    assert_refused("jmp in 0f", &[0xeb, 0x05], base(), ring0, |_, i| {
        i.opcode.map = OpcodeMap::Escape0f;
    });
}

#[test]
fn memory_movs_and_other_control_transfers_are_refused_without_side_effects() {
    let ring0 = context(PrivilegeLevel::Ring0);
    let refused: &[(&str, &[u8])] = &[
        ("mov rax, [rsi]", &[0x48, 0x8b, 0x06]),
        ("mov [rsi], rax", &[0x48, 0x89, 0x06]),
        ("mov al, [rsi]", &[0x8a, 0x06]),
        ("mov [rsi], ax", &[0x66, 0x89, 0x06]),
        ("mov byte [rsi], 5", &[0xc6, 0x06, 0x05]),
        (
            "mov qword [rsi], 5",
            &[0x48, 0xc7, 0x06, 0x05, 0x00, 0x00, 0x00],
        ),
        ("mov eax, [rip]", &[0x8b, 0x05, 0x00, 0x00, 0x00, 0x00]),
        ("mov rax, [rsi+rcx*8+8]", &[0x48, 0x8b, 0x44, 0xce, 0x08]),
        ("jmp rax", &[0xff, 0xe0]),
        ("jmp [rsi]", &[0xff, 0x26]),
        ("call +0", &[0xe8, 0x00, 0x00, 0x00, 0x00]),
        ("ret", &[0xc3]),
        ("push rax", &[0x50]),
        ("lea rax, [rsi]", &[0x48, 0x8d, 0x06]),
        ("movzx eax, cl", &[0x0f, 0xb6, 0xc1]),
        ("cmovz rax, rcx", &[0x48, 0x0f, 0x44, 0xc1]),
        ("nop", &[0x90]),
    ];
    for &(label, bytes) in refused {
        let mut start = state(5, 2 | CF | ZF);
        start.gpr[6] = 0x2000;
        assert_refused(label, bytes, start, ring0, |_, _| {});
    }
}

/// PERF-24: the step takes the G1 and G3 forms through the fast path only when the set was built
/// with `with_fast_path`, and the run is the same either way.
#[test]
fn the_step_takes_g1_and_g3_forms_with_the_flag_and_the_run_is_unchanged() {
    use crate::fast_path_check;
    // 0x00 mov rbx, rax      48 89 c3
    // 0x03 mov ecx, 5        b9 05 00 00 00
    // 0x08 dec ecx           ff c9
    // 0x0a jnz 0x08          75 fc
    // 0x0c jmp 0x10          eb 02
    // 0x0e nop; nop          90 90 (skipped)
    // 0x10 mov ah, 0x12      b4 12
    // 0x12 mov rdx, rbx      48 89 da
    // 0x15 nop               90
    let program = [
        0x48, 0x89, 0xc3, 0xb9, 0x05, 0x00, 0x00, 0x00, 0xff, 0xc9, 0x75, 0xfc, 0xeb, 0x02, 0x90,
        0x90, 0xb4, 0x12, 0x48, 0x89, 0xda, 0x90,
    ];
    let plain = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let run_set = |set: &Cpl0ProfileSet<'_>, seed: u64| {
        let mut state = state(seed, 0x202);
        let mut memory = CodeAt::new(CODE, &program);
        fast_path_check::reset_fast_steps_taken();
        let result = run_with_cpl0_profile_set(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 64,
                stop_rip: Some(CODE + program.len() as u64),
                context: context(PrivilegeLevel::Ring0),
            },
            INTEGER_FLAGS,
            set,
            None,
            || false,
        )
        .unwrap();
        assert_eq!(memory.data_calls, 0);
        (result, state, fast_path_check::fast_steps_taken())
    };
    for seed in 0..8 {
        let (off, off_state, taken_off) = run_set(&plain, seed);
        let (on, on_state, taken_on) = run_set(&plain.with_fast_path(), seed);
        assert_eq!(off.exit, Cpl0ProfileSetExit::Run(RunExit::StopAddress));
        // 2 MOV, 5 x (DEC, JNZ), JMP, 2 MOV, NOP.
        assert_eq!(off.instructions_executed, 16);
        assert_eq!(on, off, "seed {seed}");
        assert_eq!(on_state, off_state, "seed {seed}");
        assert_eq!(off_state.gpr[1] & 0xffff_ffff, 0);
        assert_eq!(taken_off, 0);
        if fast_path_check::FAST_STEPS_COUNTED {
            // Everything but the final NOP.
            assert_eq!(taken_on, 15, "seed {seed}");
        }
    }
}
