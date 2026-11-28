//! S7-32, ADR-0023 slice C1, plan step C1-6 (decision 4): the alignment-check exception `#AC` at
//! CPL3, against Intel SDM 325462-093US Vol. 3A §7.15 "Interrupt 17—Alignment Check Exception (#AC)"
//! and Table 7-7 ("Alignment Requirements by Data Type"), Vol. 2A §2.5 (SSE exception classes):
//!
//! - #AC (vector 17, error code 0, a fault: nothing changes) when CPL = 3, CR0.AM = 1 and RFLAGS.AC =
//!   1, for a data or stack operand whose start address is not aligned to the requirement of the
//!   instruction; implicit accesses to the descriptor tables are never checked;
//! - the order of ADR-0023 "Порядок проверок": canonical (#GP(0), #SS(0) for an SS address) before
//!   alignment, alignment before paging (#PF);
//! - requirements (the executor passes them): the operand size for the integer, stack, MMX, x87
//!   m16/m32/m64 and the 2/4/8-byte SSE accesses; none for the 16-byte SSE forms (aligned ones have
//!   their #GP(0)); SGDT/SIDT 4; SLDT/STR m16 2; x87 m80 8, FBLD/FBSTP 8; FNSTENV/FLDENV/FNSAVE/FRSTOR 4
//!   (2 with `66`); MASKMOVDQU/MASKMOVQ 8 (the physical oracle of `s2_39_rest.rs`); FXSAVE/FXRSTOR
//!   none (a misaligned area is #GP(0)); CMPXCHG8B 8; CMPXCHG16B none (its own #GP(0)).
//!
//! Reachability in C1 (plan C1-6 item 5): (a) the CPL3 entry of the set — the forms of a class other
//! than `Neutral` and the `Neutral` forms the C1 allow-list admits; (b) the bare Ring3 step (no set,
//! `execute::step` with a Ring3 context) — the shared SSE/MMX/x87/POPCNT/CRC32/MASKMOV*/FISTTP
//! executors; (c) after C2a-4 — the remaining forms (CMPXCHG8B, XADD m, CMPXCHG m, the x87 forms of
//! the FP member, a misaligned LOCK), see `REACHABILITY`.

mod cpl3_sweep;

use cpl3_sweep::{
    CR4, Cells, DataAccess, RIP, SweepMemory, encodings, machine_cached, run_exit, run_on,
};
use od_isa::execute::{self, RunExit, StepOutcome};
use od_isa::state::{SseState, X87State};
use od_isa::{AccessOrigin, ArchitecturalFault, LongModeState, PrivilegeLevel, Register};

const AC: u64 = 1 << 18;
const AM: u64 = 1 << 18;
const ALIGNMENT_CHECK: ArchitecturalFault = ArchitecturalFault {
    vector: 17,
    error_code: Some(0),
    linear_address: None,
};

/// Plan C1-6 item 5: form → where it is reachable in C1 → the test that checks it.
pub const REACHABILITY: &[(&str, &str, &str)] = &[
    (
        "mov rax, [rdi] (48 8B 07)",
        "(a) CPL3 entry, allow-list",
        "alignment_check_matches_table_7_7",
    ),
    (
        "mov [rdi], rax (48 89 07)",
        "(a) CPL3 entry, allow-list",
        "alignment_check_matches_table_7_7",
    ),
    (
        "mov qword [rdi], -2 (48 C7 07 imm32)",
        "(a) CPL3 entry, allow-list",
        "alignment_check_matches_table_7_7",
    ),
    (
        "sgdt/sidt [rdi]",
        "(a) CPL3 entry, Cr4Gated",
        "alignment_check_matches_table_7_7",
    ),
    (
        "sldt/str [rdi]",
        "(a) CPL3 entry, Cr4Gated",
        "alignment_check_matches_table_7_7",
    ),
    (
        "lar/lsl/verr/verw m16",
        "(a) CPL3 entry, CplSpecific",
        "alignment_check_matches_table_7_7",
    ),
    (
        "mov sreg, m16",
        "(a) CPL3 entry, CplSpecific",
        "alignment_check_matches_table_7_7",
    ),
    (
        "popf (stack 8/2)",
        "(a) CPL3 entry, IoplSensitive",
        "alignment_check_matches_table_7_7",
    ),
    (
        "iretq (stack frame)",
        "(a) CPL3 entry, CplSpecific",
        "descriptor_reads_are_not_alignment_checked",
    ),
    (
        "int n (descriptor reads)",
        "(a) CPL3 entry, CplSpecific",
        "descriptor_reads_are_not_alignment_checked",
    ),
    (
        "push/pop/call/ret (stack)",
        "(b) bare Ring3 step",
        "alignment_check_matches_table_7_7",
    ),
    (
        "movd/movq mm, m32/m64; paddw mm, m64",
        "(b) bare Ring3 step",
        "alignment_check_matches_table_7_7",
    ),
    (
        "movss/movsd/addsd m32/m64; movups/movdqu m128",
        "(b) bare Ring3 step",
        "alignment_check_matches_table_7_7",
    ),
    (
        "popcnt/crc32 m16/m32/m64",
        "(b) bare Ring3 step",
        "alignment_check_is_complete_on_the_ring3_bare_step",
    ),
    (
        "maskmovdqu (8)",
        "(b) bare Ring3 step",
        "s2_39_rest.rs maskmovdqu_alignment_check_is_an_eight_byte_rule",
    ),
    (
        "fisttp m16/m32/m64",
        "(b) bare Ring3 step",
        "alignment_check_is_complete_on_the_ring3_bare_step",
    ),
    (
        "fnstenv/fnsave [rax] (4)",
        "(b) bare Ring3 step",
        "alignment_check_matches_table_7_7",
    ),
    (
        "cmpxchg8b [rdi] (8)",
        "(b) bare Ring3 step",
        "alignment_check_matches_table_7_7",
    ),
    (
        "fs/gs forms of `transfer` (60 executors)",
        "(a) CPL3 entry (glibc TLS forms) and (b) bare Ring3 step",
        "fs_and_gs_forms_raise_alignment_check",
    ),
    (
        "fld/fstp m80, fbld/fbstp, fldenv/frstor [rax] (not covered: refused or state-dependent at CPL3)",
        "not covered — C2a (ISSUE-722)",
        "-",
    ),
    (
        "xadd m, cmpxchg m, lock forms, x87 of the FP member (checks inserted, not covered by a test)",
        "(c) after C2a-4 (ISSUE-722)",
        "C2a-3 split_lock_with_ac_at_cpl3_is_alignment_check",
    ),
];

// ---- the bare Ring3 step -----------------------------------------------------------------------

/// The state of the bare step: no set, x87 and SSE carried in the state (as the S2 profile tests),
/// CR0 not carried (AM counts as 1), the GPRs of the sweep.
fn bare_state() -> LongModeState {
    let (machine_state, _) = machine_cached(3, 0, CR4);
    let mut x87 = X87State::fixture_seed();
    x87.instruction_selector = Some(0x33);
    x87.data_selector = Some(0x2b);
    LongModeState {
        rip: RIP,
        rflags: 2,
        gpr: machine_state.gpr,
        fs_base: machine_state.fs_base,
        gs_base: machine_state.gs_base,
        x87: Some(x87),
        sse: Some(SseState {
            xmm: cpl3_sweep::fp_seed().sse.xmm,
            mxcsr: 0x1f80,
        }),
        ..LongModeState::default()
    }
}

fn bare_run(
    mut state: LongModeState,
    mut memory: SweepMemory,
    bytes: &[u8],
) -> (
    Result<StepOutcome, od_isa::MemoryError>,
    LongModeState,
    SweepMemory,
) {
    memory.put(RIP, bytes);
    let outcome = execute::step(
        &mut state,
        &mut memory,
        cpl3_sweep::context(PrivilegeLevel::Ring3),
    );
    (outcome, state, memory)
}

/// Shift every data address of a state by `shift`: the GPRs (RSP included) and the FS/GS bases.
fn shifted(mut state: LongModeState, shift: u64) -> LongModeState {
    for value in state.gpr.iter_mut() {
        *value = value.wrapping_add(shift);
    }
    state.fs_base += shift;
    state.gs_base += shift;
    state
}

/// The misaligned explicit or stack data accesses of 2, 4 or 8 bytes in the log of a run.
fn misaligned_data_accesses(memory: &SweepMemory) -> Vec<cpl3_sweep::Access> {
    memory
        .accesses
        .iter()
        .filter(|access| {
            access.origin != AccessOrigin::DescriptorTable
                && matches!(access.len, 2 | 4 | 8)
                && access.address % access.len as u64 != 0
        })
        .copied()
        .collect()
}

/// The operations whose operand is checked at its start while the executor moves it in pieces
/// (plan C1-6 item 6: the literal list of exceptions of the log check).
fn piecewise_operand(bytes: &[u8]) -> bool {
    let Ok(instruction) = cpl3_sweep::decode(bytes) else {
        return false;
    };
    let name = format!("{:?}", instruction.operation);
    [
        "Fxsave",
        "Fxrstor",
        "Cmpxchg16b",
        "StoreGdtr",
        "StoreIdtr",
        "FldM80",
        "FstpM80",
        "Fld80",
        "Fstp80",
        "Fbld",
        "Fbstp",
        "Fnstenv",
        "Fldenv",
        "Fnsave",
        "Frstor",
        "Fstenv",
        "Fsave",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}

/// Review B1: an #AC is justified only when the same form with AC = 0 makes an explicit or stack
/// access of 2 to 8 bytes that is misaligned for its width (16 bytes: MASKMOVDQU only, checked on
/// 8), or the form is one of `piecewise_operand`. A false #AC (say a byte access
/// checked as a word) has no such access.
fn justifies_alignment_check(bytes: &[u8], plain: &SweepMemory) -> bool {
    // Review R2: a 16-byte access justifies #AC only for MASKMOVDQU (checked on 8 bytes).
    let maskmovdqu = cpl3_sweep::decode(bytes)
        .is_ok_and(|instruction| format!("{:?}", instruction.operation).starts_with("Sse2Maskmov"));
    piecewise_operand(bytes)
        || plain.accesses.iter().any(|access| {
            access.origin != AccessOrigin::DescriptorTable
                && match access.len {
                    2..=8 => access.address % access.len as u64 != 0,
                    16 => maskmovdqu && access.address % 8 != 0,
                    _ => false,
                }
        })
}

/// A defect of the completeness test (the encoding, the path, why).
fn defect(path: &str, bytes: &[u8], why: String) -> String {
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{path} {hex}: {why}")
}

/// Plan C1-6 item 6 (the completeness test of the coordinator): the reduced sweep of C1-0 on the bare
/// Ring3 step and on the CPL3 entry with AC = AM = 1 and every data address shifted by 1: each
/// outcome equals the outcome with AC = 0, or is #AC(0) with nothing changed; an `Unsupported` with
/// AC = 1 where AC = 0 executes is a defect; an executed instruction whose log holds a misaligned
/// explicit/stack access of 2, 4 or 8 bytes is a defect (except `piecewise_operand`).
#[test]
fn alignment_checking_is_complete_on_the_ring3_bare_step() {
    let encodings = encodings();
    let defects = cpl3_sweep::parallel_map_with(
        &encodings,
        || (),
        |_, bytes| {
            let mut found = Vec::new();
            // (b) the bare Ring3 step.
            let mut memory = SweepMemory::default();
            memory.on_demand = true;
            memory.map(RIP, true, true);
            let base = shifted(bare_state(), 1);
            let (plain, _, plain_memory) = bare_run(base.clone(), memory.clone(), bytes);
            let mut armed_state = base.clone();
            armed_state.rflags |= AC;
            let (armed, after, armed_memory) = bare_run(armed_state.clone(), memory, bytes);
            let executed = matches!(plain, Ok(StepOutcome::Advanced | StepOutcome::Branched));
            if armed == Ok(StepOutcome::Fault(ALIGNMENT_CHECK)) {
                if after != armed_state || !armed_memory.writes.is_empty() {
                    found.push(defect("bare", bytes, "#AC changed the state".into()));
                }
                if !justifies_alignment_check(bytes, &plain_memory) {
                    found.push(defect(
                        "bare",
                        bytes,
                        "#AC without a misaligned access".into(),
                    ));
                }
            } else if armed != plain {
                found.push(defect(
                    "bare",
                    bytes,
                    format!("AC=0 {plain:?}, AC=1 {armed:?}"),
                ));
            } else if executed && !piecewise_operand(bytes) {
                let misaligned = misaligned_data_accesses(&armed_memory);
                if !misaligned.is_empty() {
                    found.push(defect(
                        "bare",
                        bytes,
                        format!("executed with {misaligned:?}"),
                    ));
                }
            }
            // (a) the CPL3 entry of the set.
            let (state, memory) = machine_cached(3, 0, CR4);
            let base = shifted(state, 1);
            let plain = run_on(base.clone(), memory.clone(), Cells::new(), bytes, 1);
            let mut armed_state = base;
            armed_state.rflags |= AC;
            let armed = run_on(armed_state.clone(), memory, Cells::new(), bytes, 1);
            let armed_exit = run_exit(armed.result.as_ref().unwrap()).clone();
            let executed =
                run_exit(plain.result.as_ref().unwrap()) == &RunExit::InstructionBudgetExhausted;
            if armed_exit == RunExit::Fault(ALIGNMENT_CHECK) {
                if armed.state != armed_state || !armed.memory.writes.is_empty() {
                    found.push(defect("cpl3", bytes, "#AC changed the state".into()));
                }
                if !justifies_alignment_check(bytes, &plain.memory) {
                    found.push(defect(
                        "cpl3",
                        bytes,
                        "#AC without a misaligned access".into(),
                    ));
                }
            } else if armed.result != plain.result {
                found.push(defect(
                    "cpl3",
                    bytes,
                    format!(
                        "AC=0 {:?}, AC=1 {armed_exit:?}",
                        run_exit(plain.result.as_ref().unwrap())
                    ),
                ));
            } else if executed && !piecewise_operand(bytes) {
                let misaligned = misaligned_data_accesses(&armed.memory);
                if !misaligned.is_empty() {
                    found.push(defect(
                        "cpl3",
                        bytes,
                        format!("executed with {misaligned:?}"),
                    ));
                }
            }
            found
        },
    );
    let defects: Vec<String> = defects.into_iter().flatten().collect();
    assert!(
        defects.is_empty(),
        "{} defects, first 60:\n{}",
        defects.len(),
        defects
            .iter()
            .take(60)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    let _ = (AM, DataAccess::Read, Register::Rax);
}

// ---- the model of Table 7-7 ----------------------------------------------------------------------

/// Which register holds the operand address of a form.
#[derive(Clone, Copy, Debug)]
enum Base {
    Rdi,
    Rax,
    Rsp,
}

/// A form of the model test: name, bytes, the register of its operand, the requirement of Table 7-7
/// (or the choice of the model, see the module doc), the path.
struct Form {
    name: &'static str,
    bytes: &'static [u8],
    base: Base,
    requirement: u64,
    cpl3_entry: bool,
}

const FORMS: &[Form] = &[
    // (a) the CPL3 entry of the set.
    Form {
        name: "mov rax, [rdi]",
        bytes: &[0x48, 0x8b, 0x07],
        base: Base::Rdi,
        requirement: 8,
        cpl3_entry: true,
    },
    Form {
        name: "mov [rdi], rax",
        bytes: &[0x48, 0x89, 0x07],
        base: Base::Rdi,
        requirement: 8,
        cpl3_entry: true,
    },
    Form {
        name: "mov qword [rdi], -2",
        bytes: &[0x48, 0xc7, 0x07, 0xfe, 0xff, 0xff, 0xff],
        base: Base::Rdi,
        requirement: 8,
        cpl3_entry: true,
    },
    Form {
        name: "sgdt [rdi]",
        bytes: &[0x0f, 0x01, 0x07],
        base: Base::Rdi,
        requirement: 4,
        cpl3_entry: true,
    },
    Form {
        name: "sidt [rdi]",
        bytes: &[0x0f, 0x01, 0x0f],
        base: Base::Rdi,
        requirement: 4,
        cpl3_entry: true,
    },
    Form {
        name: "sldt [rdi]",
        bytes: &[0x0f, 0x00, 0x07],
        base: Base::Rdi,
        requirement: 2,
        cpl3_entry: true,
    },
    Form {
        name: "str [rdi]",
        bytes: &[0x0f, 0x00, 0x0f],
        base: Base::Rdi,
        requirement: 2,
        cpl3_entry: true,
    },
    Form {
        name: "lar eax, [rdi]",
        bytes: &[0x0f, 0x02, 0x07],
        base: Base::Rdi,
        requirement: 2,
        cpl3_entry: true,
    },
    Form {
        name: "verw [rdi]",
        bytes: &[0x0f, 0x00, 0x2f],
        base: Base::Rdi,
        requirement: 2,
        cpl3_entry: true,
    },
    Form {
        name: "mov ds, [rdi]",
        bytes: &[0x8e, 0x1f],
        base: Base::Rdi,
        requirement: 2,
        cpl3_entry: true,
    },
    Form {
        name: "popfq",
        bytes: &[0x9d],
        base: Base::Rsp,
        requirement: 8,
        cpl3_entry: true,
    },
    Form {
        name: "popfw",
        bytes: &[0x66, 0x9d],
        base: Base::Rsp,
        requirement: 2,
        cpl3_entry: true,
    },
    // (b) the bare Ring3 step.
    Form {
        name: "push rax",
        bytes: &[0x50],
        base: Base::Rsp,
        requirement: 8,
        cpl3_entry: false,
    },
    Form {
        name: "pop rax",
        bytes: &[0x58],
        base: Base::Rsp,
        requirement: 8,
        cpl3_entry: false,
    },
    Form {
        name: "call rel32",
        bytes: &[0xe8, 0, 0, 0, 0],
        base: Base::Rsp,
        requirement: 8,
        cpl3_entry: false,
    },
    Form {
        name: "ret",
        bytes: &[0xc3],
        base: Base::Rsp,
        requirement: 8,
        cpl3_entry: false,
    },
    Form {
        name: "mov rax, [rdi] (bare)",
        bytes: &[0x48, 0x8b, 0x07],
        base: Base::Rdi,
        requirement: 8,
        cpl3_entry: false,
    },
    Form {
        name: "bt [rdi], rax (REX.W, model choice 8)",
        bytes: &[0x48, 0x0f, 0xa3, 0x07],
        base: Base::Rdi,
        requirement: 8,
        cpl3_entry: false,
    },
    Form {
        name: "movd mm0, [rdi]",
        bytes: &[0x0f, 0x6e, 0x07],
        base: Base::Rdi,
        requirement: 4,
        cpl3_entry: false,
    },
    Form {
        name: "movq mm0, [rdi]",
        bytes: &[0x0f, 0x6f, 0x07],
        base: Base::Rdi,
        requirement: 8,
        cpl3_entry: false,
    },
    Form {
        name: "paddw mm0, [rdi]",
        bytes: &[0x0f, 0xfd, 0x07],
        base: Base::Rdi,
        requirement: 8,
        cpl3_entry: false,
    },
    Form {
        name: "movss xmm0, [rdi]",
        bytes: &[0xf3, 0x0f, 0x10, 0x07],
        base: Base::Rdi,
        requirement: 4,
        cpl3_entry: false,
    },
    Form {
        name: "addsd xmm0, [rdi]",
        bytes: &[0xf2, 0x0f, 0x58, 0x07],
        base: Base::Rdi,
        requirement: 8,
        cpl3_entry: false,
    },
    Form {
        name: "movups xmm0, [rdi] (Type 4: none)",
        bytes: &[0x0f, 0x10, 0x07],
        base: Base::Rdi,
        requirement: 1,
        cpl3_entry: false,
    },
    Form {
        name: "movdqu xmm0, [rdi] (Type 4: none)",
        bytes: &[0xf3, 0x0f, 0x6f, 0x07],
        base: Base::Rdi,
        requirement: 1,
        cpl3_entry: false,
    },
    Form {
        name: "popcnt eax, [rdi]",
        bytes: &[0xf3, 0x0f, 0xb8, 0x07],
        base: Base::Rdi,
        requirement: 4,
        cpl3_entry: false,
    },
    Form {
        name: "maskmovdqu (model choice 8)",
        bytes: &[0x66, 0x0f, 0xf7, 0xc1],
        base: Base::Rdi,
        requirement: 8,
        cpl3_entry: false,
    },
    Form {
        name: "fnstenv [rax] (4)",
        bytes: &[0xd9, 0x30],
        base: Base::Rax,
        requirement: 4,
        cpl3_entry: false,
    },
    Form {
        name: "fnsave [rax] (4)",
        bytes: &[0xdd, 0x30],
        base: Base::Rax,
        requirement: 4,
        cpl3_entry: false,
    },
    Form {
        name: "cmpxchg8b [rdi] (8)",
        bytes: &[0x0f, 0xc7, 0x0f],
        base: Base::Rdi,
        requirement: 8,
        cpl3_entry: false,
    },
    // Review B1: byte forms and forms without a data access never raise #AC.
    Form {
        name: "mov al, [rdi] (byte)",
        bytes: &[0x8a, 0x07],
        base: Base::Rdi,
        requirement: 1,
        cpl3_entry: false,
    },
    Form {
        name: "movzx eax, byte [rdi]",
        bytes: &[0x0f, 0xb6, 0x07],
        base: Base::Rdi,
        requirement: 1,
        cpl3_entry: false,
    },
    Form {
        name: "cmp byte [rdi], 0",
        bytes: &[0x80, 0x3f, 0x00],
        base: Base::Rdi,
        requirement: 1,
        cpl3_entry: false,
    },
    Form {
        name: "sete byte [rdi]",
        bytes: &[0x0f, 0x94, 0x07],
        base: Base::Rdi,
        requirement: 1,
        cpl3_entry: false,
    },
    Form {
        name: "lea rax, [rdi] (no access)",
        bytes: &[0x48, 0x8d, 0x07],
        base: Base::Rdi,
        requirement: 1,
        cpl3_entry: false,
    },
    Form {
        name: "nop dword [rdi] (no access)",
        bytes: &[0x0f, 0x1f, 0x07],
        base: Base::Rdi,
        requirement: 1,
        cpl3_entry: true,
    },
];

/// The operand address of a form at shift 0 (8-byte aligned for every base).
const OPERAND: u64 = cpl3_sweep::DATA + 0x400;
const STACK: u64 = cpl3_sweep::STACK_PAGE + 0x800;

fn place(state: &mut LongModeState, base: Base, shift: u64) {
    match base {
        Base::Rdi => state.gpr[Register::Rdi as usize] = OPERAND + shift,
        Base::Rax => state.gpr[Register::Rax as usize] = OPERAND + shift,
        Base::Rsp => state.gpr[Register::Rsp as usize] = STACK + shift,
    }
}

/// One run of a form: the exit (`None` for any non-fault result), nothing changed on #AC.
fn run_form(form: &Form, cpl: u8, am: bool, ac: bool, shift: u64) -> Option<ArchitecturalFault> {
    if form.cpl3_entry {
        let (mut state, memory) = machine_cached(cpl, 0, CR4);
        state.cr0 = Some(if am {
            cpl3_sweep::CR0
        } else {
            cpl3_sweep::CR0 & !AM
        });
        if ac {
            state.rflags |= AC;
        }
        place(&mut state, form.base, shift);
        let before = state.clone();
        let run = run_on(state, memory, Cells::new(), form.bytes, 1);
        match &run.result.as_ref().unwrap().exit {
            od_isa::Cpl0ProfileSetExit::Run(RunExit::Fault(fault)) => {
                if *fault == ALIGNMENT_CHECK {
                    assert_eq!(run.state, before, "{}", form.name);
                    assert!(run.memory.writes.is_empty(), "{}", form.name);
                }
                Some(fault.clone())
            }
            _ => None,
        }
    } else {
        // The bare step carries no CR0: AM counts as 1 (the AM = 0 rows are the entry's).
        assert!(am);
        let mut state = bare_state();
        if ac {
            state.rflags |= AC;
        }
        place(&mut state, form.base, shift);
        let before = state.clone();
        let mut memory = SweepMemory::default();
        memory.on_demand = true;
        memory.map(RIP, true, true);
        let privilege = if cpl == 3 {
            PrivilegeLevel::Ring3
        } else {
            PrivilegeLevel::Ring0
        };
        memory.put(RIP, form.bytes);
        let outcome = execute::step(&mut state, &mut memory, cpl3_sweep::context(privilege));
        match outcome {
            Ok(StepOutcome::Fault(fault)) => {
                if fault == ALIGNMENT_CHECK {
                    assert_eq!(state, before, "{}", form.name);
                    assert!(memory.writes.is_empty(), "{}", form.name);
                }
                Some(fault)
            }
            Ok(StepOutcome::Unsupported { .. }) => {
                panic!("{} at CPL{cpl} shift {shift} is not executed", form.name)
            }
            _ => None,
        }
    }
}

#[test]
fn alignment_check_matches_table_7_7() {
    for form in FORMS {
        for cpl in [0u8, 3] {
            for am in [false, true] {
                if !am && !form.cpl3_entry {
                    continue;
                }
                for ac in [false, true] {
                    for shift in [0u64, 1, 2, 4, 8] {
                        let expected = cpl == 3 && am && ac && shift % form.requirement != 0;
                        let fault = run_form(form, cpl, am, ac, shift);
                        assert_eq!(
                            fault.as_ref() == Some(&ALIGNMENT_CHECK),
                            expected,
                            "{} CPL{cpl} AM {am} AC {ac} shift {shift}: {fault:?}",
                            form.name
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn noncanonical_before_alignment_and_alignment_before_paging() {
    let armed = |mut state: LongModeState| {
        state.rflags |= AC;
        state
    };
    // A non-canonical misaligned operand: #GP(0); a non-canonical misaligned stack: #SS(0).
    let (mut state, memory) = machine_cached(3, 0, CR4);
    state.gpr[Register::Rdi as usize] = 0x0000_8000_0000_0001;
    let run = run_on(
        armed(state),
        memory.clone(),
        Cells::new(),
        &[0x48, 0x8b, 0x07],
        1,
    );
    assert_eq!(run_exit(run.result.as_ref().unwrap()), &cpl3_sweep::GP0);
    let mut state = bare_state();
    state.gpr[Register::Rsp as usize] = 0x0000_8000_0000_0009;
    let (outcome, _, _) = bare_run(armed(state), memory_with_code(), &[0x50]);
    assert_eq!(
        outcome,
        Ok(StepOutcome::Fault(ArchitecturalFault {
            vector: 12,
            error_code: Some(0),
            linear_address: None
        }))
    );
    // A misaligned operand on an unmapped page and across two pages: #AC, not #PF.
    for address in [0x7700_0001u64, cpl3_sweep::DATA + 0xffd] {
        let (mut state, mut memory) = machine_cached(3, 0, CR4);
        memory.on_demand = false;
        memory.pages.remove(&(cpl3_sweep::DATA + 0x1000));
        state.gpr[Register::Rdi as usize] = address;
        let run = run_on(armed(state), memory, Cells::new(), &[0x48, 0x8b, 0x07], 1);
        assert_eq!(
            run_exit(run.result.as_ref().unwrap()),
            &RunExit::Fault(ALIGNMENT_CHECK),
            "{address:#x}"
        );
        assert!(run.memory.accesses.is_empty(), "{address:#x}");
    }
}

fn memory_with_code() -> SweepMemory {
    let mut memory = SweepMemory::default();
    memory.on_demand = true;
    memory.map(RIP, true, true);
    memory
}

/// The descriptor-table reads of INT n and IRETQ are implicit supervisor accesses: never checked,
/// even at misaligned table bases; the IRETQ frame is a stack access and is checked.
#[test]
fn descriptor_reads_are_not_alignment_checked() {
    // INT 2E (DPL 3 gate) with the IDT moved to a base that is 1 mod 8.
    let (mut state, mut memory) = machine_cached(3, 0, CR4);
    let idt = cpl3_sweep::IDT + 0x10001;
    memory.map(idt, false, true);
    memory.map(idt + 0x1000, false, true);
    let gate = memory.peek(cpl3_sweep::IDT + 16 * 0x2e, 16).unwrap();
    memory.put(idt + 16 * 0x2e, &gate);
    state.idtr = Some(od_isa::state::Idtr {
        base: idt,
        limit: 0xfff,
    });
    let run = run_on(
        {
            state.rflags |= AC;
            state
        },
        memory,
        Cells::new(),
        &[0xcd, 0x2e],
        1,
    );
    assert_eq!(
        run_exit(run.result.as_ref().unwrap()),
        &RunExit::InstructionBudgetExhausted
    );
    assert_eq!(run.state.rip, cpl3_sweep::handler(0x2e));
    assert_eq!(run.state.cs_selector, Some(0x08));
    // IRETQ to CPL3 with the GDT at a base that is 1 mod 8.
    let frame = |memory: &mut SweepMemory, rsp: u64| {
        for (index, value) in [
            0x0000_7ff6_0000_1000u64,
            0x33,
            0x202,
            0x0000_00e1_0000_0000,
            0x2b,
        ]
        .iter()
        .enumerate()
        {
            memory.put_u64(rsp + 8 * index as u64, *value);
        }
    };
    let (mut state, mut memory) = machine_cached(3, 0, CR4);
    let gdt = cpl3_sweep::GDT + 0x20001;
    memory.map(gdt, false, true);
    let table = memory.peek(cpl3_sweep::GDT, 0x58).unwrap();
    memory.put(gdt, &table);
    state.gdtr = Some(od_isa::state::Gdtr {
        base: gdt,
        limit: 0x57,
    });
    let rsp = cpl3_sweep::STACK_PAGE + 0x800;
    state.gpr[Register::Rsp as usize] = rsp;
    frame(&mut memory, rsp);
    state.rflags |= AC;
    let run = run_on(
        state.clone(),
        memory.clone(),
        Cells::new(),
        &[0x48, 0xcf],
        1,
    );
    assert_eq!(
        run_exit(run.result.as_ref().unwrap()),
        &RunExit::InstructionBudgetExhausted
    );
    // The same frame at RSP + 4 (misaligned for its qwords): #AC.
    state.gpr[Register::Rsp as usize] = rsp + 4;
    frame(&mut memory, rsp + 4);
    let before = state.clone();
    let run = run_on(state, memory, Cells::new(), &[0x48, 0xcf], 1);
    assert_eq!(
        run_exit(run.result.as_ref().unwrap()),
        &RunExit::Fault(ALIGNMENT_CHECK)
    );
    assert_eq!(run.state, before);
}

/// Review B4: the bounded x87 `[RAX]` forms check the whole operand canonical before alignment:
/// `FNSTENV [RAX]` with a canonical start and a non-canonical last byte is #GP(0), not #AC.
#[test]
fn x87_rax_forms_check_the_canonical_range_before_alignment() {
    for (rax, expected) in [
        (0x7fff_ffff_fff1u64, cpl3_sweep::GP0),
        (0x7fff_ffff_fff4, cpl3_sweep::GP0),
        (cpl3_sweep::DATA + 0x401, RunExit::Fault(ALIGNMENT_CHECK)),
    ] {
        let mut state = bare_state();
        state.x87.as_mut().unwrap().tag_word = 0x3fff;
        state.x87.as_mut().unwrap().status_word = 0x3800;
        state.gpr[Register::Rax as usize] = rax;
        state.rflags |= AC;
        let (outcome, _, _) = bare_run(state, memory_with_code(), &[0xd9, 0x30]);
        let expected = match expected {
            RunExit::Fault(fault) => Ok(StepOutcome::Fault(fault)),
            _ => unreachable!(),
        };
        assert_eq!(outcome, expected, "RAX {rax:#x}");
    }
}

/// Review B5: the #GP(0) of privilege precedes the #AC: `LGDT [rax]` at CPL3 with a misaligned RAX
/// and AC = AM = 1.
#[test]
fn the_gp0_of_privilege_precedes_alignment_check() {
    let (mut state, memory) = machine_cached(3, 0, CR4);
    state.gpr[Register::Rax as usize] = cpl3_sweep::DATA + 0x401;
    state.rflags |= AC;
    let run = run_on(state, memory, Cells::new(), &[0x0f, 0x01, 0x10], 1);
    assert_eq!(run_exit(run.result.as_ref().unwrap()), &cpl3_sweep::GP0);
    assert!(run.memory.accesses.is_empty());
}

/// Review B5: the descriptor reads of LAR, LSL, VERR and VERW are implicit supervisor accesses:
/// no #AC at a GDT base that is 1 mod 8 with AC = AM = 1 at CPL3.
#[test]
fn descriptor_query_reads_are_not_alignment_checked() {
    for code in [
        &[0x0f, 0x02, 0xc1][..],
        &[0x0f, 0x03, 0xc1],
        &[0x0f, 0x00, 0xe1],
        &[0x0f, 0x00, 0xe9],
    ] {
        let (mut state, mut memory) = machine_cached(3, 0, CR4);
        let gdt = cpl3_sweep::GDT + 0x20001;
        memory.map(gdt, false, true);
        let table = memory.peek(cpl3_sweep::GDT, 0x58).unwrap();
        memory.put(gdt, &table);
        state.gdtr = Some(od_isa::state::Gdtr {
            base: gdt,
            limit: 0x57,
        });
        state.gpr[Register::Rcx as usize] = 0x2b;
        state.rflags |= AC;
        let run = run_on(state, memory, Cells::new(), code, 1);
        assert_eq!(
            run_exit(run.result.as_ref().unwrap()),
            &RunExit::InstructionBudgetExhausted,
            "{code:02x?}"
        );
        assert_ne!(run.state.rflags & (1 << 6), 0, "{code:02x?}: ZF");
    }
}

/// Review B5: the FS/GS forms of `transfer` (the 60 executors with their own address computation)
/// raise #AC: every decoded FS/GS encoding of the reduced sweep and of the `64 48`/`65 48` sets that
/// executes with AC = 0 with a misaligned access of 2 or more bytes is #AC(0) with AC = 1, on the
/// bare Ring3 step and on the CPL3 entry.
#[test]
fn fs_and_gs_forms_raise_alignment_check() {
    // Every ModRM byte (a SIB of `25` — no base, no index — for rm = 4) after the FS/GS prefix sets,
    // displacements and immediates fixed at 0x11.
    let units: Vec<(&[u8], u8)> = cpl3_sweep::MAPS[..2]
        .iter()
        .flat_map(|(_, map)| (0..=255u8).map(move |opcode| (*map, opcode)))
        .collect();
    let mut candidates: Vec<Vec<u8>> = cpl3_sweep::parallel_map_with(
        &units,
        || (),
        |_, (map_bytes, opcode)| {
            let mut found = Vec::new();
            for prefixes in [
                &[0x64u8][..],
                &[0x65],
                &[0x64, 0x48],
                &[0x65, 0x48],
                &[0x66, 0x64],
                &[0x66, 0x65],
                &[0xf0, 0x64],
                &[0xf0, 0x65],
                &[0xf0, 0x64, 0x48],
                &[0xf0, 0x65, 0x48],
            ] {
                for modrm in 0..=255u8 {
                    let mut candidate = prefixes.to_vec();
                    candidate.extend_from_slice(map_bytes);
                    candidate.push(*opcode);
                    candidate.push(modrm);
                    if modrm >> 6 != 3 && modrm & 7 == 4 {
                        candidate.push(0x25);
                    }
                    candidate.extend_from_slice(&[0x11; 12]);
                    if let Some(length) = cpl3_sweep::decoded_length(&candidate) {
                        candidate.truncate(length);
                        found.push(candidate);
                    }
                }
            }
            found
        },
    )
    .into_iter()
    .flatten()
    .collect();
    candidates.extend(encodings());
    candidates.sort();
    candidates.dedup();
    let fs_gs: Vec<(Vec<u8>, String)> = candidates
        .into_iter()
        .filter_map(|bytes| {
            let name = format!("{:?}", cpl3_sweep::decode(&bytes).ok()?.operation);
            let variant = name.split([' ', '(']).next().unwrap().to_string();
            (variant.contains("Fs") || variant.contains("Gs")).then_some((bytes, variant))
        })
        .collect();
    let results = cpl3_sweep::parallel_map_with(
        &fs_gs,
        || (),
        |_, (bytes, variant)| {
            let mut covered = Vec::new();
            let mut failures = Vec::new();
            // The bare Ring3 step.
            let base = shifted(bare_state(), 1);
            let (plain, _, plain_memory) = bare_run(base.clone(), memory_with_code(), bytes);
            let mut armed = base;
            armed.rflags |= AC;
            let (armed, _, _) = bare_run(armed, memory_with_code(), bytes);
            if matches!(plain, Ok(StepOutcome::Advanced | StepOutcome::Branched))
                && justifies_alignment_check(bytes, &plain_memory)
            {
                covered.push(variant.clone());
                if armed != Ok(StepOutcome::Fault(ALIGNMENT_CHECK)) {
                    failures.push(format!("bare {bytes:02x?} {variant}: {armed:?}"));
                }
            }
            // The CPL3 entry.
            let (state, memory) = machine_cached(3, 0, CR4);
            let base = shifted(state, 1);
            let plain = run_on(base.clone(), memory.clone(), Cells::new(), bytes, 1);
            let mut armed = base;
            armed.rflags |= AC;
            let armed = run_on(armed, memory, Cells::new(), bytes, 1);
            if run_exit(plain.result.as_ref().unwrap()) == &RunExit::InstructionBudgetExhausted
                && justifies_alignment_check(bytes, &plain.memory)
            {
                covered.push(variant.clone());
                let exit = run_exit(armed.result.as_ref().unwrap()).clone();
                if exit != RunExit::Fault(ALIGNMENT_CHECK) {
                    failures.push(format!("cpl3 {bytes:02x?} {variant}: {exit:?}"));
                }
            }
            (covered, failures)
        },
    );
    let mut covered = std::collections::BTreeSet::new();
    let mut failures = Vec::new();
    for (names, found) in results {
        covered.extend(names);
        failures.extend(found);
    }
    let decoded: std::collections::BTreeSet<&String> =
        fs_gs.iter().map(|(_, variant)| variant).collect();
    println!(
        "FS/GS variants decoded: {}, executed with a misaligned access (covered): {} {covered:?}",
        decoded.len(),
        covered.len()
    );
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
    // In C1 the other FS/GS variants do not execute at CPL3 (the step admits them at Ring0 only, plan
    // appendix B, or only in narrow bare profiles): covered after C2a-4 (ISSUE-722).
    for variant in [
        "MovFsLoadD",
        "MovFsLoadQ",
        "MovFsLoad",
        "MovGsLoad",
        "MovFsStore",
        "MovFsMemImm",
        "AddFsLoad",
        "CmpFsMemImm",
    ] {
        assert!(covered.contains(variant), "{variant}: {covered:?}");
    }
}
