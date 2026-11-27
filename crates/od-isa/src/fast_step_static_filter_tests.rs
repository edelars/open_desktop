//! The static rule of the former interpreter fast path (`execute::fast_handler`, ADR-0018,
//! «Дополнение E10»), which the turbo engine of `od-machine` asks (`fast_handler(..).is_some()`)
//! before it admits an instruction to generated code. The rule is pinned on every decode of a
//! generated set of encodings: every one- and two-byte (`0F xx`) opcode under the group 1 prefixes
//! (none, F0, F2, F3), the segment prefixes (none, CS, FS, GS), the operand- and address-size
//! prefixes and REX (REX.W and a REX without W; see `encodings`), register and memory ModRM forms of
//! every `reg` field and two immediate/displacement patterns.
//!
//! For every successful decode the rule gives a class only to an instruction without a LOCK/REP
//! prefix and without an FS/GS segment prefix, and a control transfer only in the form of
//! `executable` (`3E 75`, `2E E8`, `66 C3`, `67 E8`, ... have none). The admitted set is the one of
//! HEAD `1500400` (its count, the counts of straight-line instructions and control transfers, and an
//! FNV-1a-64 hash of the sorted encodings, taken there with this generator). A change of the decoder
//! or of the generator that changes these numbers needs them taken again on the code before the
//! change, with a proof that the rule itself did not change.

use std::collections::{BTreeSet, HashSet};
use std::mem::{Discriminant, discriminant};

use crate::execute::fast_handler;
use crate::long_decode::{self, Instruction, OpcodeMap, Operation, Prefixes};
use crate::memory::AccessKind;
use crate::{
    AccessContext, AccessOrigin, GuestMemory, InstructionMode, MemoryError, PrivilegeLevel,
};

const CODE: u64 = 0x1000;

/// Code bytes at `CODE` for the fetch; the decoder makes no data access.
struct Code(Vec<u8>);

impl GuestMemory for Code {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        address
            .checked_sub(CODE)
            .and_then(|offset| self.0.get(offset as usize))
            .copied()
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }

    fn read(&mut self, address: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })
    }

    fn write(&mut self, address: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Write,
        })
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
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
            // alone, and REX.W after each size prefix.
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

/// A control transfer (the end of a straight-line run of code).
fn is_transfer(instruction: &Instruction) -> bool {
    matches!(
        instruction.operation,
        Operation::Branch { .. }
            | Operation::Jump { .. }
            | Operation::Call { .. }
            | Operation::Return { .. }
    )
}

/// Whether a control transfer is in the form of `executable` (ADR-0017, 2.2): default prefixes,
/// and the opcode map and byte of its arm.
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

/// The decode of `bytes` at `CODE` at CPL0, if it succeeds (with only the bytes it consumed).
fn decode(bytes: &[u8]) -> Option<(Instruction, Vec<u8>)> {
    let context = AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: AccessOrigin::Explicit,
    };
    let instruction = long_decode::decode(&mut Code(bytes.to_vec()), CODE, context).ok()?;
    Some((
        instruction,
        bytes[..usize::from(instruction.length)].to_vec(),
    ))
}

#[test]
fn the_static_rule_admits_the_pinned_set_of_every_generated_decode() {
    let mut seen = HashSet::new();
    let mut admitted: BTreeSet<Vec<u8>> = BTreeSet::new();
    let mut admitted_operations: HashSet<Discriminant<Operation>> = HashSet::new();
    let (mut decodes, mut straight, mut transfers) = (0usize, 0usize, 0usize);
    for encoding in encodings() {
        let Some((instruction, bytes)) = decode(&encoding) else {
            continue;
        };
        if !seen.insert(bytes.clone()) {
            continue;
        }
        decodes += 1;
        if fast_handler(&instruction).is_none() {
            continue;
        }
        assert!(
            instruction.prefixes.group1.is_none()
                && !matches!(instruction.prefixes.segment, Some(0x64 | 0x65)),
            "{bytes:02x?}: a LOCK/REP or FS/GS prefix admitted: {instruction:?}"
        );
        if is_transfer(&instruction) {
            assert!(
                in_executable_form(&instruction),
                "{bytes:02x?}: a control transfer outside the form of `executable`: \
                 {instruction:?}"
            );
            transfers += 1;
        } else {
            straight += 1;
        }
        admitted.insert(bytes);
        admitted_operations.insert(discriminant(&instruction.operation));
    }
    // The generated set reaches every group: thousands of decodes, the fifteen operation classes
    // of G1-G6.
    assert!(decodes > 10_000, "{decodes}");
    assert_eq!(admitted_operations.len(), 15, "{decodes} decodes");
    // The admitted set of HEAD `1500400`: 126 403 decodes, 33 009 admitted (32 128 straight-line,
    // 881 control transfers), and the same encodings; S7-32 changes it as noted below.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for encoding in &admitted {
        for &byte in encoding.iter().chain(std::iter::once(&0xffu8)) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    // S7-32: 385 more decodes (the system-form fallback of `decode`) and 80 more admitted, all
    // MOVNTI (`0F C3` with a memory destination), decoded as the plain `Mov` store it is for the
    // model. S7-32 W2: 4 028 more decodes (the locked read-modify-write forms, `LockedRmw`, and
    // the scalar MOVSD/MOVSS memory forms) and none admitted: the rule gives no class to a LOCK
    // form or to these SSE operations, so the admitted set and its hash are unchanged. S7-32 W3:
    // 19 251 more decodes (the general FS/GS and null segment overrides of memory forms, through the
    // segment-override fallback) and none admitted: `fast_handler` refuses an FS/GS prefix, and a
    // null override on a form the profile decoder refused gives the same refused shape, so the
    // admitted set and its hash are unchanged. The W3 add-on: 20 more decodes (`66 8E /r`, MOV
    // Sreg with the operand-size prefix, as `MovToSreg`) and none admitted (the rule gives
    // `MovToSreg` no class). S7-32 W4: 44 more decodes (FXSAVE/FXRSTOR, FILD m32int and FNSAVE
    // with memory operands of the W4 fallbacks: `Fxsave`/`Fxrstor`, `FildM32Int`, `FnsaveMemory`)
    // and none admitted (the rule gives these operations no class). S7-32 slice S: 123 more decodes
    // (REPNE SCAS without `67`, `RepneScas`, and LOOPNE/LOOPE/JRCXZ and the `67` LOOP forms) and
    // none admitted (the rule gives these operations no class; they run on the general step).
    assert_eq!(
        (decodes, straight, transfers, admitted.len()),
        (150_254, 32_208, 881, 33_089)
    );
    assert_eq!(hash, 0x9173_c634_efcd_f113, "the admitted set changed");
}

/// Known encodings and whether the rule gives them a class.
#[test]
fn the_static_rule_on_known_forms() {
    let cases: &[(&[u8], bool)] = &[
        (&[0x01, 0xd8], true),                         // add eax, ebx
        (&[0x48, 0x01, 0x18], true),                   // add [rax], rbx
        (&[0xf0, 0x01, 0x18], false),                  // lock add [rax], ebx
        (&[0x64, 0x8b, 0x00], false),                  // mov eax, fs:[rax]
        (&[0x65, 0x01, 0xd8], false),                  // gs: add eax, ebx
        (&[0x2e, 0x8b, 0x00], true),                   // mov eax, cs:[rax]
        (&[0xd1, 0xe0], true),                         // shl eax, 1
        (&[0xd1, 0x20], false),                        // shl dword [rax], 1
        (&[0x48, 0xb8, 1, 2, 3, 4, 5, 6, 7, 8], true), // mov rax, imm64
        (&[0x0f, 0xb6, 0xc0], true),                   // movzx eax, al
        (&[0x8d, 0x04, 0x24], true),                   // lea eax, [rsp]
        (&[0x50], true),                               // push rax
        (&[0x75, 0xfe], true),                         // jnz -2
        (&[0x0f, 0x85, 0, 0, 0, 0], true),             // jnz rel32
        (&[0x3e, 0x75, 0xfe], false),                  // ds: jnz (a branch hint)
        (&[0xeb, 0xfe], true),                         // jmp -2
        (&[0xe8, 0, 0, 0, 0], true),                   // call rel32
        (&[0x2e, 0xe8, 0, 0, 0, 0], false),            // cs: call rel32
        (&[0xc3], true),                               // ret
        (&[0xc2, 8, 0], true),                         // ret 8
        (&[0x66, 0xc3], false),                        // o16 ret
        (&[0xee], false),                              // out dx, al
        (&[0x0f, 0x31], false),                        // rdtsc
        (&[0xfa], false),                              // cli
        (&[0xf3, 0xaa], false),                        // rep stosb
        (&[0x67, 0xe8, 0, 0, 0, 0], false),            // a32 call rel32
        (&[0x66, 0xeb, 0xfe], false),                  // o16 jmp -2
        (&[0x67, 0x75, 0xfe], false),                  // a32 jnz -2
        (&[0x48, 0x0f, 0xaf, 0xc1], true),             // imul rax, rcx
        (&[0x0f, 0xaf, 0x00], true),                   // imul eax, [rax]
        (&[0x90], true),                               // nop
        (&[0x66, 0x90], true),                         // o16 nop
        (&[0x0f, 0x1f, 0x44, 0x00, 0x00], true),       // nop dword [rax+rax]
        (&[0x66, 0x0f, 0x1f, 0x44, 0x00, 0x00], true), // nop word [rax+rax]
        (&[0x0f, 0x18, 0x08], true),                   // prefetcht0 [rax]
        (&[0x64, 0x0f, 0x1f, 0x00], false),            // nop dword fs:[rax]
        (&[0xf3, 0x90], false),                        // pause
        (&[0x6b, 0xc1, 0x05], false),                  // imul eax, ecx, 5
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
    for &(bytes, admitted) in cases {
        let (instruction, consumed) = decode(bytes).unwrap_or_else(|| panic!("{bytes:02x?}"));
        assert_eq!(consumed.len(), bytes.len(), "{bytes:02x?}");
        assert_eq!(
            fast_handler(&instruction).is_some(),
            admitted,
            "{bytes:02x?}: {instruction:?}"
        );
    }
}
