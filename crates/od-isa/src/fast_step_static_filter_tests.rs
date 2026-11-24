//! PERF-33 (E3, adapted to HEAD after E3 was rolled back): the static rule of the predecoded
//! blocks (`predecoded_block::fast_block_role`) against what the fast path
//! (`execute::try_fast_step`) actually runs, on every decode of a generated set of encodings: every
//! one- and two-byte (`0F xx`) opcode under the group 1 prefixes (none, F0, F2, F3), the segment
//! prefixes (none, CS, FS, GS), the operand- and address-size prefixes and REX (REX.W and a REX
//! without W; see `encodings`), register and memory ModRM forms of every `reg` field and two
//! immediate/displacement patterns. The state is that of the G2 tests (CPL0, RF = TF = 0, a clear
//! shadow), so the dynamic part of the admission of `try_fast_step` passes and only its static part
//! can refuse.
//!
//! For every successful decode:
//! - `fast_block_role` is `None` => `try_fast_step` answers "not handled" (nothing changed, no
//!   data access): a group added to the fast path but not to the block rule fails here;
//! - `fast_block_role` is `Some(Straight)` => `try_fast_step` handles it, and never as `Branched`;
//! - `fast_block_role` is `Some(Terminator)` => `try_fast_step` handles it, except a control
//!   transfer outside the form of `executable` (a prefix other than the default ones: `3E 75`,
//!   `2E E8`, `66 C3`, `67 E8`, ...), which the rule admits by design (its documentation: "Only a
//!   filter ... a form this admits but the fast path refuses ends the run of the block there") and
//!   the fast path refuses before any effect.
//!
//! A handled admitted decode is refused again under each dynamic refusal (CPL3, TF, RF, a shadow).

use std::collections::HashSet;
use std::mem::{Discriminant, discriminant};

use crate::execute::{StepOutcome, try_fast_step};
use crate::fast_step_g2_tests::{CODE, context, state};
use crate::fast_step_g4_tests::Ram;
use crate::long_decode::{self, Instruction, OpcodeMap, Operation, Prefixes};
use crate::predecoded_block::{FastBlockRole, fast_block_role};
use crate::{
    AccessContext, GuestMemory, INTEGER_FLAGS, LongModeState, MemoryError, PrivilegeLevel,
    StiShadow,
};

const TF: u64 = 1 << 8;
const RF: u64 = 1 << 16;

/// The memory of the G4 tests (code at `CODE`, a sparse RAM elsewhere) with every data access
/// counted.
struct Counted {
    ram: Ram,
    data_calls: u32,
}

impl GuestMemory for Counted {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        self.ram.fetch(address, context)
    }

    fn read(
        &mut self,
        address: u64,
        context: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        self.data_calls += 1;
        self.ram.read(address, context, destination)
    }

    fn write(
        &mut self,
        address: u64,
        context: AccessContext,
        source: &[u8],
    ) -> Result<(), MemoryError> {
        self.data_calls += 1;
        self.ram.write(address, context, source)
    }

    fn mfence(&mut self, context: AccessContext) -> Result<(), MemoryError> {
        self.data_calls += 1;
        self.ram.mfence(context)
    }
}

/// The generated encodings (see the module documentation), each followed by enough bytes for any
/// displacement and immediate.
fn encodings() -> Vec<Vec<u8>> {
    const TAILS: [[u8; 12]; 2] = [
        [
            0x05, 0x00, 0x00, 0x00, 0x10, 0x20, 0x30, 0x40, 0x01, 0x02, 0x03, 0x04,
        ],
        [
            0xf0, 0xff, 0xff, 0xff, 0x80, 0x7f, 0xfe, 0x00, 0x11, 0x22, 0x33, 0x44,
        ],
    ];
    let mut all = Vec::new();
    for group1 in [None, Some(0xf0), Some(0xf2), Some(0xf3)] {
        for segment in [None, Some(0x2e), Some(0x64), Some(0x65)] {
            // The operand- and address-size prefixes and REX (REX.W and a REX without W), each
            // alone, and REX.W after each size prefix: the full product of the three (12 forms)
            // made the debug run 3.1 times as long as without 67 and 40; these 7 make it 1.8.
            for size_and_rex in [
                &[][..],
                &[0x66],
                &[0x67],
                &[0x40],
                &[0x48],
                &[0x66, 0x48],
                &[0x67, 0x48],
            ] {
                let mut prefixes = Vec::new();
                prefixes.extend(group1);
                prefixes.extend(segment);
                prefixes.extend_from_slice(size_and_rex);
                for opcode in 0..512u16 {
                    let opcode_bytes: &[u8] = if opcode < 256 {
                        &[opcode as u8]
                    } else {
                        &[0x0f, opcode as u8]
                    };
                    for reg in 0..8u8 {
                        // ModRM: `reg`, RCX (register form); `reg`, [RAX] (memory form).
                        for modrm in [0xc1 | reg << 3, reg << 3] {
                            for tail in &TAILS {
                                let mut bytes = prefixes.clone();
                                bytes.extend_from_slice(opcode_bytes);
                                bytes.push(modrm);
                                bytes.extend_from_slice(tail);
                                all.push(bytes);
                            }
                        }
                    }
                }
            }
        }
    }
    all
}

/// Whether a control transfer is in the form of `executable` that `try_fast_step` admits
/// (ADR-0017, 2.2): default prefixes, and the opcode map and byte of its arm.
fn in_executable_form(instruction: &Instruction) -> bool {
    let opcode = instruction.opcode;
    instruction.prefixes == Prefixes::default()
        && match instruction.operation {
            Operation::Branch { .. } => {
                matches!(opcode.map, OpcodeMap::Primary | OpcodeMap::Escape0f)
            }
            Operation::Jump { .. } => opcode.map == OpcodeMap::Primary,
            Operation::Call { .. } => opcode.map == OpcodeMap::Primary && opcode.byte == 0xe8,
            Operation::Return { .. } => {
                opcode.map == OpcodeMap::Primary && matches!(opcode.byte, 0xc3 | 0xc2)
            }
            _ => false,
        }
}

/// The decode of `bytes` at `CODE`, if it succeeds (with only the bytes it consumed).
fn decode(bytes: &[u8]) -> Option<(Instruction, Vec<u8>)> {
    let mut ram = Ram::new(CODE, bytes);
    let instruction = long_decode::decode(&mut ram, CODE, context(PrivilegeLevel::Ring0)).ok()?;
    Some((
        instruction,
        bytes[..usize::from(instruction.length)].to_vec(),
    ))
}

/// What a fast path left: its outcome, the state, the definedness and the data calls.
struct Fast {
    outcome: Option<Result<StepOutcome, MemoryError>>,
    state: LongModeState,
    defined: u64,
    data_calls: u32,
}

fn run(bytes: &[u8], instruction: &Instruction, start: &LongModeState, ctx: AccessContext) -> Fast {
    let mut state = start.clone();
    let mut defined = INTEGER_FLAGS;
    let mut memory = Counted {
        ram: Ram::new(CODE, bytes),
        data_calls: 0,
    };
    let outcome = try_fast_step(
        &mut state,
        &mut memory,
        ctx,
        &mut defined,
        instruction,
        true,
    );
    Fast {
        outcome,
        state,
        defined,
        data_calls: memory.data_calls,
    }
}

fn assert_not_handled(label: &str, bytes: &[u8], fast: &Fast, start: &LongModeState) {
    assert!(
        fast.outcome.is_none(),
        "{label}: {bytes:02x?}: {:?}",
        fast.outcome
    );
    assert_eq!(&fast.state, start, "{label}: {bytes:02x?}");
    assert_eq!(fast.defined, INTEGER_FLAGS, "{label}: {bytes:02x?}");
    assert_eq!(fast.data_calls, 0, "{label}: {bytes:02x?}");
}

#[test]
fn the_block_role_matches_the_fast_path_on_every_generated_decode() {
    let ring0 = context(PrivilegeLevel::Ring0);
    let start = state(5, 2);
    // The dynamic refusals of the admission: each alone refuses every instruction.
    let mut tf = start.clone();
    tf.rflags |= TF;
    let mut rf = start.clone();
    rf.rflags |= RF;
    let mut shadow = start.clone();
    shadow.sti_shadow = Some(StiShadow::Blocking);
    let mut unmodelled = start.clone();
    unmodelled.sti_shadow = None;

    let mut seen = HashSet::new();
    let mut handled_operations: HashSet<Discriminant<Operation>> = HashSet::new();
    let (mut decodes, mut straight, mut terminator, mut role_only) =
        (0usize, 0usize, 0usize, 0usize);
    for encoding in encodings() {
        let Some((instruction, bytes)) = decode(&encoding) else {
            continue;
        };
        if !seen.insert(bytes.clone()) {
            continue;
        }
        decodes += 1;
        let role = fast_block_role(&instruction);
        let fast = run(&bytes, &instruction, &start, ring0);
        let Some(role) = role else {
            assert_not_handled("refused by fast_block_role", &bytes, &fast, &start);
            continue;
        };
        if fast.outcome.is_none() {
            // The documented superset of the rule: only a control transfer outside the form of
            // `executable`, refused before any effect.
            assert_not_handled("admitted by the role only", &bytes, &fast, &start);
            assert_eq!(
                role,
                FastBlockRole::Terminator,
                "{bytes:02x?}: the role admits {instruction:?} as straight, the fast path does \
                 not handle it"
            );
            assert!(
                !in_executable_form(&instruction),
                "{bytes:02x?}: the fast path refuses {instruction:?} in the form of `executable`"
            );
            role_only += 1;
            continue;
        }
        if matches!(fast.outcome, Some(Ok(StepOutcome::Branched))) {
            assert_eq!(
                role,
                FastBlockRole::Terminator,
                "{bytes:02x?}: {instruction:?}"
            );
        }
        match role {
            FastBlockRole::Straight => straight += 1,
            FastBlockRole::Terminator => terminator += 1,
        }
        handled_operations.insert(discriminant(&instruction.operation));
        for (label, refused, ctx) in [
            ("CPL3", &start, context(PrivilegeLevel::Ring3)),
            ("TF", &tf, ring0),
            ("RF", &rf, ring0),
            ("STI/MOV SS shadow", &shadow, ring0),
            ("unmodelled shadow", &unmodelled, ring0),
        ] {
            let fast = run(&bytes, &instruction, refused, ctx);
            assert_not_handled(label, &bytes, &fast, refused);
        }
    }
    // The generated set reaches every group: thousands of decodes, both roles, the twelve
    // operation classes of G1-G5, and the prefixed control transfers the rule admits by design.
    assert!(decodes > 10_000, "{decodes}");
    assert!(
        straight > 1_000 && terminator > 10,
        "{straight} {terminator}"
    );
    assert!(role_only > 0, "{decodes} decodes");
    assert_eq!(handled_operations.len(), 12, "{decodes} decodes");
}

/// Known encodings: their role (`None`: the block ends before them) and whether the fast path
/// handles them on the state of the G2 tests.
#[test]
fn the_block_role_and_the_fast_path_on_known_forms() {
    use FastBlockRole::{Straight, Terminator};
    let cases: &[(&[u8], Option<FastBlockRole>, bool)] = &[
        (&[0x01, 0xd8], Some(Straight), true),       // add eax, ebx
        (&[0x48, 0x01, 0x18], Some(Straight), true), // add [rax], rbx
        (&[0xf0, 0x01, 0x18], None, false),          // lock add [rax], ebx
        (&[0x64, 0x8b, 0x00], None, false),          // mov eax, fs:[rax]
        (&[0x65, 0x01, 0xd8], None, false),          // gs: add eax, ebx
        (&[0x2e, 0x8b, 0x00], Some(Straight), true), // mov eax, cs:[rax]
        (&[0xd1, 0xe0], Some(Straight), true),       // shl eax, 1
        (&[0xd1, 0x20], None, false),                // shl dword [rax], 1
        (&[0x48, 0xb8, 1, 2, 3, 4, 5, 6, 7, 8], Some(Straight), true), // mov rax, imm64
        (&[0x0f, 0xb6, 0xc0], Some(Straight), true), // movzx eax, al
        (&[0x8d, 0x04, 0x24], Some(Straight), true), // lea eax, [rsp]
        (&[0x50], Some(Straight), true),             // push rax
        (&[0x75, 0xfe], Some(Terminator), true),     // jnz -2
        (&[0x0f, 0x85, 0, 0, 0, 0], Some(Terminator), true), // jnz rel32
        (&[0x3e, 0x75, 0xfe], Some(Terminator), false), // ds: jnz (a branch hint)
        (&[0xeb, 0xfe], Some(Terminator), true),     // jmp -2
        (&[0xe8, 0, 0, 0, 0], Some(Terminator), true), // call rel32
        (&[0x2e, 0xe8, 0, 0, 0, 0], Some(Terminator), false), // cs: call rel32
        (&[0xc3], Some(Terminator), true),           // ret
        (&[0xc2, 8, 0], Some(Terminator), true),     // ret 8
        (&[0x66, 0xc3], Some(Terminator), false),    // o16 ret
        (&[0xee], None, false),                      // out dx, al
        (&[0x0f, 0x31], None, false),                // rdtsc
        (&[0xfa], None, false),                      // cli
        (&[0xf3, 0xaa], None, false),                // rep stosb
        (&[0x67, 0xe8, 0, 0, 0, 0], Some(Terminator), false), // a32 call rel32
        (&[0x66, 0xeb, 0xfe], Some(Terminator), false), // o16 jmp -2
        (&[0x67, 0x75, 0xfe], Some(Terminator), false), // a32 jnz -2
    ];
    // Prefixed near branches the decoder itself refuses (`Unsupported`): they never reach the
    // rule. A decoder that learns one of them must add it to the table above.
    let undecoded: &[&[u8]] = &[
        &[0x66, 0xe8, 0, 0, 0, 0], // o16 call rel32
        &[0x48, 0xe8, 0, 0, 0, 0], // rex.w call rel32
        &[0x40, 0xc3],             // rex ret
        &[0x66, 0xc2, 8, 0],       // o16 ret 8
    ];
    for bytes in undecoded {
        assert!(decode(bytes).is_none(), "{bytes:02x?}");
    }
    let ring0 = context(PrivilegeLevel::Ring0);
    let start = state(5, 2);
    for &(bytes, role, handled) in cases {
        let (instruction, consumed) = decode(bytes).unwrap_or_else(|| panic!("{bytes:02x?}"));
        assert_eq!(consumed.len(), bytes.len(), "{bytes:02x?}");
        assert_eq!(
            fast_block_role(&instruction),
            role,
            "{bytes:02x?}: {instruction:?}"
        );
        let fast = run(bytes, &instruction, &start, ring0);
        if handled {
            assert!(fast.outcome.is_some(), "{bytes:02x?}: {instruction:?}");
        } else {
            assert_not_handled("known form", bytes, &fast, &start);
        }
    }
}
