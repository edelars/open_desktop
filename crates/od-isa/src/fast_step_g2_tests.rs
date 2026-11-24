//! PERF-23, group G2 (ADR-0017): register ALU `Arithmetic`, `Logic` and `Shift` in
//! `execute::try_fast_step`. The fast path is called directly on a copy of the state and compared
//! with the general path (`run` of one instruction): same CPU state, same `rflags_defined`, same
//! outcome, no call to the memory. On inadmissible conditions it must answer "not handled" and
//! change nothing.

use std::collections::BTreeSet;

use crate::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, RunConfig, RunExit, StepOutcome,
    run_with_cpl0_profile_set, try_fast_step,
};
use crate::long_decode::{self, Instruction, Operand, Operation};
use crate::memory::AccessKind;
use crate::{
    AccessContext, AccessOrigin, GuestMemory, INTEGER_FLAGS, InstructionMode, LongModeState,
    MemoryError, PrivilegeLevel, StiShadow,
};

pub(super) const CODE: u64 = 0x1000;
const CF: u64 = 1;
const PF: u64 = 1 << 2;
const AF: u64 = 1 << 4;
const ZF: u64 = 1 << 6;
const SF: u64 = 1 << 7;
const OF: u64 = 1 << 11;
const DF: u64 = 1 << 10;
const IF: u64 = 1 << 9;
const TF: u64 = 1 << 8;
const RF: u64 = 1 << 16;

/// Code bytes for the fetch; every data access is counted and fails (a register form has none).
struct Code {
    bytes: Vec<u8>,
    data_calls: u32,
}

impl Code {
    fn new(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.to_vec(),
            data_calls: 0,
        }
    }
}

impl GuestMemory for Code {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        address
            .checked_sub(CODE)
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

pub(super) fn context(privilege: PrivilegeLevel) -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege,
        origin: AccessOrigin::Explicit,
    }
}

pub(super) fn state(seed: u64, rflags: u64) -> LongModeState {
    let mut state = LongModeState {
        rip: CODE,
        rflags,
        // The general path (profile set) needs the control registers and the activity; the fast
        // path needs a modelled, clear interrupt shadow (ADR-0017 section 4).
        cr0: Some(0x8001_0033),
        cr4: Some(0x20),
        activity: Some(crate::CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ..LongModeState::default()
    };
    let mut x = seed;
    for (index, register) in state.gpr.iter_mut().enumerate() {
        x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        *register = match seed {
            0 => 0,
            1 => u64::MAX,
            2 => 0x8000_0000_0000_0000 + index as u64,
            3 => 0x7fff_ffff_ffff_ffff - index as u64,
            4 => 0x0000_0000_8000_0000 | (index as u64) << 8,
            _ => z ^ (z >> 31),
        };
    }
    state
}

fn states() -> Vec<LongModeState> {
    let mut all = Vec::new();
    for (seed, rflags) in [
        (0, 2),
        (1, 2 | CF),
        (2, 2 | CF | PF | AF | ZF | SF | OF),
        (3, 2 | DF | IF),
        (4, 2 | ZF),
        (5, 2),
        (6, 2 | CF | OF),
        (7, 2 | PF | SF),
        (8, 2 | AF),
    ] {
        all.push(state(seed, rflags));
    }
    all.push(state(9, 2 | CF));
    all
}

/// The register-and-immediate G2 encodings of one operand size, and a label of what they are.
fn encodings() -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    let mut add = |label: String, bytes: Vec<u8>| out.push((label, bytes));
    // (size in bytes, legacy prefixes, REX.W)
    let sizes: [(usize, &[u8], bool); 4] = [
        (1, &[], false),
        (2, &[0x66], false),
        (4, &[], false),
        (8, &[], true),
    ];
    // (destination, source) register numbers; 4..=7 as bytes are AH..BH without a REX prefix.
    let pairs: [(u8, u8); 7] = [(0, 1), (3, 2), (6, 7), (0, 0), (4, 5), (9, 10), (15, 8)];
    for &(size, legacy, wide) in &sizes {
        for &(dst, src) in &pairs {
            let rex_variants: &[bool] = if size == 1 && (4..8).contains(&dst) {
                &[false, true]
            } else {
                &[false]
            };
            for &force_rex in rex_variants {
                let rex_bits = (u8::from(wide) << 3) | ((src >> 3) << 2) | (dst >> 3);
                let rex = (rex_bits != 0 || force_rex).then_some(0x40 | rex_bits);
                let mut head = legacy.to_vec();
                head.extend(rex);
                let low = (dst & 7) | ((src & 7) << 3);
                let reverse = (src & 7) | ((dst & 7) << 3);
                let wordsize = u8::from(size != 1);
                let tag = format!("size {size} dst {dst} src {src} rex {force_rex}");
                for kind in 0u8..8 {
                    let base = kind * 8;
                    let mut bytes = head.clone();
                    bytes.extend([base + wordsize, 0xc0 | low]);
                    add(format!("alu{kind} rm,reg {tag}"), bytes);
                    let mut bytes = head.clone();
                    bytes.extend([base + 2 + wordsize, 0xc0 | reverse]);
                    add(format!("alu{kind} reg,rm {tag}"), bytes);
                }
                // TEST rm, reg
                let mut bytes = head.clone();
                bytes.extend([0x84 + wordsize, 0xc0 | low]);
                add(format!("test rm,reg {tag}"), bytes);
                // INC, DEC, NOT, NEG on the destination (rm)
                let rm_only = 0xc0 | (dst & 7);
                let (unary, unary_reg): (u8, &[u8]) = if size == 1 {
                    (0xfe, &[0, 1])
                } else {
                    (0xff, &[0, 1])
                };
                for &digit in unary_reg {
                    let mut bytes = head.clone();
                    bytes.extend([unary, rm_only | (digit << 3)]);
                    add(format!("incdec{digit} {tag}"), bytes);
                }
                for digit in [2u8, 3] {
                    let mut bytes = head.clone();
                    bytes.extend([0xf6 + wordsize, rm_only | (digit << 3)]);
                    add(format!("notneg{digit} {tag}"), bytes);
                }
                // group 1 with an immediate
                for kind in 0u8..8 {
                    for immediate in [0u64, 1, 0x7f, 0x80, 0xff, 0x1234, 0x8000, 0xffff_ffff] {
                        let mut bytes = head.clone();
                        let group = rm_only | (kind << 3);
                        match size {
                            1 => {
                                bytes.extend([0x80, group, immediate as u8]);
                            }
                            2 => {
                                bytes.extend([0x81, group]);
                                bytes.extend((immediate as u16).to_le_bytes());
                            }
                            _ => {
                                bytes.extend([0x81, group]);
                                bytes.extend((immediate as u32).to_le_bytes());
                            }
                        }
                        add(format!("group1 {kind} imm {immediate:#x} {tag}"), bytes);
                        if size != 1 && immediate <= 0xff {
                            let mut bytes = head.clone();
                            bytes.extend([0x83, group, immediate as u8]);
                            add(format!("group1 {kind} imm8 {immediate:#x} {tag}"), bytes);
                        }
                    }
                }
                // TEST rm, imm
                for immediate in [0u64, 0x80, 0xffff_ffff] {
                    let mut bytes = head.clone();
                    bytes.extend([0xf6 + wordsize, rm_only]);
                    match size {
                        1 => bytes.push(immediate as u8),
                        2 => bytes.extend((immediate as u16).to_le_bytes()),
                        _ => bytes.extend((immediate as u32).to_le_bytes()),
                    }
                    add(format!("test imm {immediate:#x} {tag}"), bytes);
                }
                // shifts and rotates: by an immediate, by one, by CL (digit 6 is not taken)
                for digit in [0u8, 1, 2, 3, 4, 5, 7] {
                    let group = rm_only | (digit << 3);
                    for count in [0u8, 1, 2, 3, 7, 8, 15, 16, 31, 32, 33, 63, 64, 255] {
                        let mut bytes = head.clone();
                        bytes.extend([0xc0 + wordsize, group, count]);
                        add(format!("shift{digit} imm {count} {tag}"), bytes);
                    }
                    let mut bytes = head.clone();
                    bytes.extend([0xd0 + wordsize, group]);
                    add(format!("shift{digit} by 1 {tag}"), bytes);
                    let mut bytes = head.clone();
                    bytes.extend([0xd2 + wordsize, group]);
                    add(format!("shift{digit} by cl {tag}"), bytes);
                }
                // SHLD, SHRD (word, dword, qword)
                if size != 1 {
                    for (opcode, name) in [(0xa4u8, "shld imm"), (0xac, "shrd imm")] {
                        for count in [0u8, 1, 5, 16, 31, 32, 63, 64] {
                            let mut bytes = head.clone();
                            bytes.extend([0x0f, opcode, 0xc0 | low, count]);
                            add(format!("{name} {count} {tag}"), bytes);
                        }
                    }
                    for (opcode, name) in [(0xa5u8, "shld cl"), (0xad, "shrd cl")] {
                        let mut bytes = head.clone();
                        bytes.extend([0x0f, opcode, 0xc0 | low]);
                        add(format!("{name} {tag}"), bytes);
                    }
                }
            }
        }
    }
    // accumulator forms: TEST AL/AX/EAX/RAX, imm
    add("test al,imm".into(), vec![0xa8, 0x81]);
    add("test ax,imm".into(), vec![0x66, 0xa9, 0x34, 0x12]);
    add("test eax,imm".into(), vec![0xa9, 0x00, 0x00, 0x00, 0x80]);
    add(
        "test rax,imm".into(),
        vec![0x48, 0xa9, 0xff, 0xff, 0xff, 0xff],
    );
    out
}

fn decode(bytes: &[u8], context: AccessContext) -> (Instruction, Code) {
    let mut memory = Code::new(bytes);
    let instruction = long_decode::decode(&mut memory, CODE, context)
        .unwrap_or_else(|error| panic!("{bytes:02x?}: {error:?}"));
    assert_eq!(usize::from(instruction.length), bytes.len(), "{bytes:02x?}");
    (instruction, memory)
}

/// The general path: one instruction through `run`.
/// `defined` is the explicit starting `rflags_defined` mask.
fn general(bytes: &[u8], start: &LongModeState, defined: u64) -> (LongModeState, u64) {
    let plain = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let mut state = start.clone();
    let mut memory = Code::new(bytes);
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 4,
            stop_rip: Some(CODE + bytes.len() as u64),
            context: context(PrivilegeLevel::Ring0),
        },
        defined,
        &plain,
        None,
        || false,
    )
    .unwrap();
    assert_eq!(
        result.exit,
        Cpl0ProfileSetExit::Run(RunExit::StopAddress),
        "{bytes:02x?}: {result:?}"
    );
    assert_eq!(result.instructions_executed, 1, "{bytes:02x?}");
    (state, result.rflags_defined)
}

#[test]
fn every_g2_register_form_equals_the_general_path() {
    let ctx = context(PrivilegeLevel::Ring0);
    let mut kinds = BTreeSet::new();
    let mut sizes = BTreeSet::new();
    let mut immediate_forms = 0u64;
    let mut cases = 0u64;
    let all_states = states();
    // The starting definedness mask: all flags, none, and all but AF and OF.
    let start_masks = [INTEGER_FLAGS, 0, INTEGER_FLAGS & !(AF | OF)];
    for (label, bytes) in encodings() {
        let (instruction, _) = decode(&bytes, ctx);
        match instruction.operation {
            Operation::Arithmetic { kind, src, .. } => {
                kinds.insert(format!("{kind:?}"));
                immediate_forms += u64::from(matches!(src, Operand::Immediate(_)));
            }
            Operation::Logic { kind, src, .. } => {
                kinds.insert(format!("{kind:?}"));
                immediate_forms += u64::from(matches!(src, Operand::Immediate(_)));
            }
            Operation::Shift { kind, .. } => {
                kinds.insert(format!("{kind:?}"));
            }
            other => panic!("{label}: {bytes:02x?} decodes to {other:?}, not a G2 operation"),
        }
        sizes.insert(instruction.operand_size.bytes());
        for (start, start_defined) in all_states
            .iter()
            .flat_map(|s| start_masks.iter().map(move |m| (s, *m)))
        {
            let (expected_state, expected_defined) = general(&bytes, start, start_defined);
            for policy in [false, true] {
                let (instruction, mut memory) = decode(&bytes, ctx);
                let mut fast_state = start.clone();
                let mut defined = start_defined;
                let outcome = try_fast_step(
                    &mut fast_state,
                    &mut memory,
                    ctx,
                    &mut defined,
                    &instruction,
                    policy,
                );
                assert!(
                    matches!(outcome, Some(Ok(StepOutcome::Advanced))),
                    "{label}: {bytes:02x?}: {outcome:?}"
                );
                assert_eq!(fast_state, expected_state, "{label}: {bytes:02x?}");
                assert_eq!(defined, expected_defined, "{label}: {bytes:02x?}");
                assert_eq!(memory.data_calls, 0, "{label}: {bytes:02x?}");
            }
            cases += 1;
        }
    }
    for expected in [
        "Add", "Adc", "Sub", "Sbb", "Cmp", "Inc", "Dec", "Neg", "And", "Or", "Xor", "Test", "Not",
        "Shl", "Shr", "Sar", "Rol", "Ror", "Rcl", "Rcr", "Shld", "Shrd",
    ] {
        assert!(
            kinds.contains(expected),
            "{expected} is not covered: {kinds:?}"
        );
    }
    assert_eq!(sizes, BTreeSet::from([1, 2, 4, 8]));
    assert!(immediate_forms > 100, "{immediate_forms}");
    assert!(cases > 10_000, "{cases}");
}

/// Snapshot of everything a refusal must leave untouched.
pub(super) fn assert_refused(
    label: &str,
    bytes: &[u8],
    mut state: LongModeState,
    ctx: AccessContext,
    edit: impl FnOnce(&mut LongModeState, &mut Instruction),
) {
    let (mut instruction, mut memory) = decode(bytes, context(PrivilegeLevel::Ring0));
    edit(&mut state, &mut instruction);
    let before = state.clone();
    let mut defined = INTEGER_FLAGS;
    memory.data_calls = 0;
    let outcome = try_fast_step(
        &mut state,
        &mut memory,
        ctx,
        &mut defined,
        &instruction,
        true,
    );
    assert!(outcome.is_none(), "{label}: {bytes:02x?}: {outcome:?}");
    assert_eq!(state, before, "{label}: {bytes:02x?}");
    assert_eq!(defined, INTEGER_FLAGS, "{label}: {bytes:02x?}");
    assert_eq!(memory.data_calls, 0, "{label}: {bytes:02x?}");
}

/// A sample of G2 register forms of every family.
const SAMPLE: &[&[u8]] = &[
    &[0x48, 0x01, 0xc8],             // add rax, rcx
    &[0x48, 0x83, 0xc0, 0x05],       // add rax, 5
    &[0x48, 0x39, 0xd0],             // cmp rax, rdx
    &[0x48, 0xff, 0xc9],             // dec rcx
    &[0x21, 0xd8],                   // and eax, ebx
    &[0x48, 0x85, 0xc0],             // test rax, rax
    &[0x48, 0xf7, 0xd0],             // not rax
    &[0x48, 0xc1, 0xe0, 0x03],       // shl rax, 3
    &[0x48, 0xd3, 0xe8],             // shr rax, cl
    &[0x48, 0x0f, 0xa4, 0xc8, 0x03], // shld rax, rcx, 3
];

#[test]
fn inadmissible_conditions_are_refused_without_side_effects() {
    let ring0 = context(PrivilegeLevel::Ring0);
    let base = || state(5, 2 | CF);
    for &bytes in SAMPLE {
        // Control: the plain case is taken, so each refusal below comes from its condition.
        let (instruction, mut memory) = decode(bytes, ring0);
        let mut taken = base();
        let mut defined = INTEGER_FLAGS;
        assert!(
            try_fast_step(
                &mut taken,
                &mut memory,
                ring0,
                &mut defined,
                &instruction,
                true
            )
            .is_some(),
            "{bytes:02x?}"
        );
        for privilege in [
            PrivilegeLevel::Ring1,
            PrivilegeLevel::Ring2,
            PrivilegeLevel::Ring3,
        ] {
            assert_refused("cpl", bytes, base(), context(privilege), |_, _| {});
        }
        assert_refused("rf", bytes, base(), ring0, |s, _| s.rflags |= RF);
        assert_refused("tf", bytes, base(), ring0, |s, _| s.rflags |= TF);
        assert_refused("rf+tf", bytes, base(), ring0, |s, _| s.rflags |= RF | TF);
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
        // The other segment prefixes have a zero base for the general forms: taken.
        for segment in [0x26u8, 0x2e, 0x36, 0x3e] {
            let (mut instruction, mut memory) = decode(bytes, ring0);
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
    // A cleared shadow is not a shadow: taken. A state without the shadow model is refused
    // (checked above, "no shadow model").
    let mut cleared = base();
    let (instruction, mut memory) = decode(SAMPLE[0], ring0);
    let mut defined = INTEGER_FLAGS;
    assert!(
        try_fast_step(
            &mut cleared,
            &mut memory,
            ring0,
            &mut defined,
            &instruction,
            true
        )
        .is_some()
    );
}

#[test]
fn memory_operands_and_other_operations_are_refused_without_side_effects() {
    let ring0 = context(PrivilegeLevel::Ring0);
    let refused: &[(&str, &[u8])] = &[
        ("add rax, [rsi]", &[0x48, 0x03, 0x06]),
        ("add [rsi], rax", &[0x48, 0x01, 0x06]),
        ("add [rsi], 5", &[0x48, 0x83, 0x06, 0x05]),
        ("cmp [rsi], rax", &[0x48, 0x39, 0x06]),
        ("inc qword [rsi]", &[0x48, 0xff, 0x06]),
        ("neg dword [rsi]", &[0xf7, 0x1e]),
        ("and [rsi], rax", &[0x48, 0x21, 0x06]),
        ("or rax, [rsi]", &[0x48, 0x0b, 0x06]),
        ("test [rsi], rax", &[0x48, 0x85, 0x06]),
        ("not qword [rsi]", &[0x48, 0xf7, 0x16]),
        ("shl qword [rsi], 1", &[0x48, 0xd1, 0x26]),
        ("shl qword [rsi], cl", &[0x48, 0xd3, 0x26]),
        ("shr dword [rsi], 3", &[0xc1, 0x2e, 0x03]),
        ("shld [rsi], rax, 3", &[0x48, 0x0f, 0xa4, 0x06, 0x03]),
        ("shrd [rsi], rax, cl", &[0x48, 0x0f, 0xad, 0x06]),
        // PERF-24: the register MOV forms and the relative JMP/Jcc are groups G1 and G3 now
        // (fast_step_g1_g3_tests.rs), the memory MOV forms and LEA group G4 (PERF-24b,
        // fast_step_g4_tests.rs); the other operations stay refused.
        ("jmp [rsi]", &[0xff, 0x26]),
        ("push rax", &[0x50]),
        ("nop", &[0x90]),
        ("imul rax, rcx", &[0x48, 0x0f, 0xaf, 0xc1]),
    ];
    for &(label, bytes) in refused {
        let mut start = state(5, 2 | CF);
        start.gpr[6] = 0x2000;
        assert_refused(label, bytes, start, ring0, |_, _| {});
    }
}

/// PERF-23: the step takes the G2 forms through the fast path only when the set was built with
/// `with_fast_path`, and the run is the same either way (retired, exit, definedness, state).
#[test]
fn the_step_takes_g2_forms_with_the_flag_and_the_run_is_unchanged() {
    use crate::CpuActivity;
    use crate::execute::{
        ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, run_with_cpl0_profile_set,
    };
    use crate::fast_path_check;
    // add rax, rcx; xor edx, edx; shl rax, 3; mov rbx, rax; cmp rax, rdx; nop
    let program = [
        0x48, 0x01, 0xC8, 0x31, 0xD2, 0x48, 0xC1, 0xE0, 0x03, 0x48, 0x89, 0xC3, 0x48, 0x39, 0xD0,
        0x90,
    ];
    let plain = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let run_set = |set: &Cpl0ProfileSet<'_>, seed: u64| {
        let mut state = state(seed, 0x202);
        state.cr0 = Some(0x8001_0033);
        state.cr4 = Some(0x20);
        state.activity = Some(CpuActivity::Running);
        state.sti_shadow = Some(StiShadow::Clear);
        let mut memory = Code::new(&program);
        fast_path_check::reset_fast_steps_taken();
        let result = run_with_cpl0_profile_set(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 16,
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
        assert_eq!(off.instructions_executed, 6);
        assert_eq!(on, off, "seed {seed}");
        assert_eq!(on_state, off_state, "seed {seed}");
        assert_eq!(taken_off, 0);
        if fast_path_check::FAST_STEPS_COUNTED {
            // add, xor, shl, cmp (G2) and, since PERF-24, the register MOV (G1); the NOP is not
            // taken.
            assert_eq!(taken_on, 5, "seed {seed}");
        }
    }
}
