//! S7-32, ADR-0023 slice C1: the privilege layer of the CPL0 set at CPL3 (plan steps C1-2, C1-3,
//! C1-5b) against Intel SDM 325462-093US:
//!
//! - Vol. 3A §6.9 and §2.8 Table 2-3, Vol. 2B "SWAPGS", "SYSRET", "SYSEXIT": the privileged
//!   instructions are #GP(0) at CPL > 0, before any memory access of the instruction (ADR-0023
//!   "Порядок проверок", item 1: `LGDT m` at CPL3 is #GP(0) even with an unmapped operand).
//! - Vol. 3A §7.9 Table 7-2: the decode faults (#UD) precede the #GP(0) of privilege: LOCK, CR1,
//!   CR5-CR7, CR9-CR15, DR8-DR15, DR4/DR5 with CR4.DE = 1, SYSRET with EFER.SCE = 0.
//! - Vol. 2B "SGDT", "SIDT", "SLDT", "STR" (UMIP = 0), "RDTSC" (TSD = 0), "RDPMC" (PCE = 0).
//! - Vol. 2A "INT n/INTO/INT3/INT1": INT3 from CPL3 through its gate (DPL >= CPL, else
//!   #GP(vector·8 + 2)).

mod cpl3_sweep;

use cpl3_sweep::{
    CR4, CR4_DE, Cells, GP0, IDT, RIP, SweepMemory, UD, digest_fields, gate, machine,
    outcome_fields, page_fault, run_cloned, run_cpl3_with, run_exit, run_one, set,
};
use od_isa::execute::{ControlRegisterPolicy, Cpl0ProfileSet, RunExit};
use od_isa::{LongModeState, Register};

/// CR4 without DE and MCE: a set without a CPUID member admits it (no undeclared feature).
const CR4_BARE: u64 = CR4 & !((1 << 3) | (1 << 6));
/// An unmapped address (on-demand mapping off in these tests).
const UNMAPPED: u64 = 0x7000_0000;

/// The CPL3 machine without on-demand mapping, CR3 unmodeled (a set without `with_cpu_owned_cr3`),
/// CR4 `cr4`, `bytes` at `RIP`, RAX and RDI pointing to an unmapped page.
fn bare_cpl3(bytes: &[u8], cr4: u64) -> (LongModeState, SweepMemory) {
    let (mut state, mut memory) = machine(3, 0, cr4);
    memory.on_demand = false;
    state.cr3 = None;
    state.gpr[Register::Rax as usize] = UNMAPPED;
    state.gpr[Register::Rdi as usize] = UNMAPPED;
    memory.put(RIP, bytes);
    (state, memory)
}

/// The privileged forms (name, bytes) the layer must stop with #GP(0) at CPL3.
const PRIVILEGED: &[(&str, &[u8])] = &[
    ("hlt", &[0xf4]),
    ("wbinvd", &[0x0f, 0x09]),
    ("rdmsr", &[0x0f, 0x32]),
    ("wrmsr", &[0x0f, 0x30]),
    ("swapgs", &[0x0f, 0x01, 0xf8]),
    ("lgdt [rax] (unmapped)", &[0x0f, 0x01, 0x10]),
    ("lidt [rax] (unmapped)", &[0x0f, 0x01, 0x18]),
    ("lldt ax", &[0x0f, 0x00, 0xd0]),
    ("ltr ax", &[0x0f, 0x00, 0xd8]),
    ("invlpg [rax] (unmapped)", &[0x0f, 0x01, 0x38]),
    ("mov rax, cr0", &[0x0f, 0x20, 0xc0]),
    ("mov rax, cr2", &[0x0f, 0x20, 0xd0]),
    ("mov rax, cr3", &[0x0f, 0x20, 0xd8]),
    ("mov rax, cr4", &[0x0f, 0x20, 0xe0]),
    ("mov rax, cr8", &[0x44, 0x0f, 0x20, 0xc0]),
    ("mov rdi, cr0", &[0x0f, 0x20, 0xc7]),
    ("mov cr0, rax", &[0x0f, 0x22, 0xc0]),
    ("mov cr0, rdi", &[0x0f, 0x22, 0xc7]),
    ("mov cr2, rax", &[0x0f, 0x22, 0xd0]),
    ("mov cr3, rax", &[0x0f, 0x22, 0xd8]),
    ("mov cr4, rax", &[0x0f, 0x22, 0xe0]),
    ("mov cr8, rax", &[0x44, 0x0f, 0x22, 0xc0]),
    ("mov rax, dr0", &[0x0f, 0x21, 0xc0]),
    ("mov rax, dr7", &[0x0f, 0x21, 0xf8]),
    ("mov dr0, rax", &[0x0f, 0x23, 0xc0]),
    ("mov dr7, rax", &[0x0f, 0x23, 0xf8]),
    ("sysexit", &[0x0f, 0x35]),
];

/// One CPL3 instruction of `bytes` in a set without any member; asserts the exit, nothing retired,
/// nothing changed, no data access.
fn assert_stops_without_access(name: &str, bytes: &[u8], cr4: u64, expected: &RunExit) {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let (mut state, mut memory) = bare_cpl3(bytes, cr4);
    let before = state.clone();
    let result = run_cpl3_with(&mut state, &mut memory, &set, 1, None).unwrap();
    assert_eq!(run_exit(&result), expected, "{name}");
    assert_eq!(result.instructions_executed, 0, "{name}");
    assert_eq!(state, before, "{name}");
    assert!(memory.accesses.is_empty(), "{name}: {:?}", memory.accesses);
    assert!(memory.writes.is_empty(), "{name}");
}

#[test]
fn privileged_forms_at_cpl3_are_gp0_without_any_member_and_without_memory_access() {
    for (name, bytes) in PRIVILEGED {
        assert_stops_without_access(name, bytes, CR4_BARE, &GP0);
    }
}

#[test]
fn mov_dr4_and_dr5_with_cr4_de_at_cpl3_are_ud_before_gp() {
    // The Windows CPUID member declares DE, so CR4.DE = 1 is admitted.
    for bytes in [
        &[0x0f, 0x21, 0xe0][..],
        &[0x0f, 0x21, 0xe8],
        &[0x0f, 0x23, 0xe0],
        &[0x0f, 0x23, 0xe8],
    ] {
        let run = run_one(bytes, 3, 0, CR4);
        let result = run.result.as_ref().unwrap();
        assert_eq!(run_exit(result), &UD, "{bytes:02x?}");
        assert_eq!(result.instructions_executed, 0, "{bytes:02x?}");
    }
}

#[test]
fn mov_dr4_and_dr5_with_cr4_de_at_cpl3_without_de_are_gp0() {
    for bytes in [
        &[0x0f, 0x21, 0xe0][..],
        &[0x0f, 0x21, 0xe8],
        &[0x0f, 0x23, 0xe0],
        &[0x0f, 0x23, 0xe8],
    ] {
        let run = run_one(bytes, 3, 0, CR4 & !CR4_DE);
        let result = run.result.as_ref().unwrap();
        assert_eq!(run_exit(result), &GP0, "{bytes:02x?}");
        assert_eq!(result.instructions_executed, 0, "{bytes:02x?}");
    }
}

#[test]
fn sysret_at_cpl3_is_ud_with_sce_clear_and_gp0_with_sce_set() {
    for bytes in [&[0x48, 0x0f, 0x07][..], &[0x0f, 0x07]] {
        for (efer, expected) in [(0xd01, GP0), (0xd00, UD)] {
            let (state, memory) = machine(3, 0, CR4);
            let cells = Cells::new();
            cells.efer.set(efer);
            let run = cpl3_sweep::run_on(state, memory, cells, bytes, 1);
            let result = run.result.as_ref().unwrap();
            assert_eq!(run_exit(result), &expected, "{bytes:02x?} EFER {efer:#x}");
            assert_eq!(result.instructions_executed, 0);
        }
    }
}

#[test]
fn lock_before_a_privileged_form_at_cpl3_is_ud() {
    for bytes in [&[0xf0, 0x0f, 0x20, 0xc0][..], &[0xf0, 0x0f, 0x01, 0x38]] {
        assert_stops_without_access("lock", bytes, CR4_BARE, &UD);
    }
}

#[test]
fn invalid_control_and_debug_registers_at_cpl3_are_ud_before_gp() {
    for bytes in [
        &[0x0f, 0x20, 0xc8][..],
        &[0x44, 0x0f, 0x20, 0xc8],
        &[0x44, 0x0f, 0x21, 0xc0],
        &[0x44, 0x0f, 0x23, 0xc0],
    ] {
        assert_stops_without_access("reserved register", bytes, CR4_BARE, &UD);
    }
    assert_stops_without_access("mov rax, cr8", &[0x44, 0x0f, 0x20, 0xc0], CR4_BARE, &GP0);
}

/// The `Run` exit of a run, or `Unsupported { opcode: 0 }` as a stand-in for any other exit of the
/// CPL0 entry (a MOV CR3 ends with a translation-context exit), which is never #UD/#GP(0).
fn any_exit(run: &cpl3_sweep::Run) -> RunExit {
    match &run.result.as_ref().unwrap().exit {
        od_isa::Cpl0ProfileSetExit::Run(exit) => exit.clone(),
        _ => RunExit::Unsupported { opcode: 0 },
    }
}

/// Vol. 2B "MOV—Move to/from Control Registers" and "MOV—Move to/from Debug Registers": a reserved
/// CRn (CR1, CR5-CR7, CR9-CR15) or DRn (DR8-DR15, REX.R) is #UD at any CPL; the other registers keep
/// their outcome (CPL0: the set's executors; CPL3: #GP(0) of the layer, DR4/DR5 with DE: #UD).
#[test]
fn reserved_control_and_debug_registers_are_ud_at_cpl0_and_cpl3() {
    for opcode in [0x20u8, 0x22] {
        for cr in 0..16u8 {
            let rex = 0x40 | ((cr >> 3) << 2);
            let bytes = [rex, 0x0f, opcode, 0xc0 | ((cr & 7) << 3)];
            let reserved = !matches!(cr, 0 | 2 | 3 | 4 | 8);
            for cpl in [0u8, 3] {
                let run = run_one(&bytes, cpl, 0, CR4);
                let exit = any_exit(&run);
                if reserved {
                    assert_eq!(exit, UD, "CPL{cpl} {bytes:02x?}");
                } else if cpl == 3 {
                    assert_eq!(exit, GP0, "CPL3 {bytes:02x?}");
                } else {
                    assert_ne!(exit, UD, "CPL0 {bytes:02x?}");
                }
            }
        }
    }
    for opcode in [0x21u8, 0x23] {
        for dr in 0..16u8 {
            let rex = 0x40 | ((dr >> 3) << 2);
            let bytes = [rex, 0x0f, opcode, 0xc0 | ((dr & 7) << 3)];
            for cpl in [0u8, 3] {
                for cr4 in [CR4, CR4 & !CR4_DE] {
                    let run = run_one(&bytes, cpl, 0, cr4);
                    let exit = any_exit(&run);
                    let alias = matches!(dr, 4 | 5) && cr4 & CR4_DE != 0;
                    if dr >= 8 || alias {
                        assert_eq!(exit, UD, "CPL{cpl} CR4 {cr4:#x} {bytes:02x?}");
                    } else if cpl == 3 {
                        assert_eq!(exit, GP0, "CPL3 CR4 {cr4:#x} {bytes:02x?}");
                    } else {
                        assert_ne!(exit, UD, "CPL0 CR4 {cr4:#x} {bytes:02x?}");
                    }
                }
            }
        }
    }
}

#[test]
fn not_at_cpl3_forms_stop_as_unsupported_without_change() {
    for (bytes, opcode) in [(&[0x48, 0xcb][..], 0xcb), (&[0x0f, 0x34], 0x34)] {
        let run = run_one(bytes, 3, 0, CR4);
        let result = run.result.as_ref().unwrap();
        assert_eq!(
            run_exit(result),
            &RunExit::Unsupported { opcode },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        let (before, _) = machine(3, 0, CR4);
        assert_eq!(run.state, before, "{bytes:02x?}");
        assert!(run.memory.writes.is_empty());
    }
}

/// The CPL0 outcome (variant v0 of the C1-0 sweep: IOPL 0, CR4 0x6f8) of the privileged encodings
/// of the sweep, as recorded on G0 (`work/s7-32/c1/base/cpl0-outcomes.txt`, the same digests the
/// group digests of `c1_cpl0_baseline.txt` fold): the layer does not run at CPL0.
#[test]
fn privileged_forms_at_cpl0_are_unchanged() {
    const G0: &[(&[u8], u64)] = &[
        (&[0xf4], 0xe56ce003a60dc1f2),
        (&[0x0f, 0x09], 0x4f2284bdd119ba4c),
        (&[0x0f, 0x32], 0x6a40161239fc46d1),
        (&[0x0f, 0x30], 0xd2f21b7c582f0846),
        (&[0x0f, 0x01, 0xf8], 0xbdb06ec4deaf614b),
        (&[0x0f, 0x01, 0x17], 0x36478936b1a81a2d),
        (&[0x0f, 0x01, 0x1f], 0x36478936b1a81a2d),
        (&[0x0f, 0x00, 0xd0], 0x1c8eb9746cbb9674),
        (&[0x0f, 0x00, 0xd8], 0x440c6152bbb42263),
        (&[0x0f, 0x01, 0x3f], 0xea56962395f78533),
        (&[0x0f, 0x20, 0xc0], 0xa418aef114ce535a),
        (&[0x0f, 0x20, 0xd0], 0xbd60d01179215349),
        (&[0x0f, 0x20, 0xd8], 0x41c931000549cb90),
        (&[0x0f, 0x20, 0xe0], 0x72eee01847e0f5ff),
        (&[0x47, 0x0f, 0x20, 0xc0], 0xbf6fcfba586b14dd),
        (&[0x0f, 0x22, 0xc0], 0x95e6678c95c35d7b),
        (&[0x0f, 0x22, 0xd0], 0xcf413920b61721f0),
        (&[0x0f, 0x22, 0xd8], 0x1b766aae64f199e8),
        (&[0x0f, 0x22, 0xe0], 0x36478936b1a81a2d),
        (&[0x47, 0x0f, 0x22, 0xc0], 0x36478936b1a81a2d),
        (&[0x0f, 0x21, 0xc0], 0x10fd25f09142d6eb),
        (&[0x0f, 0x21, 0xf8], 0x70dccfb65bb4672f),
        (&[0x0f, 0x23, 0xc0], 0xb09be547ae7d6efd),
        (&[0x0f, 0x23, 0xf8], 0xac354190ef36752d),
        (&[0x0f, 0x35], 0x7e1f32edba3ec54e),
        (&[0x48, 0x0f, 0x07], 0x674ff278f2941c87),
        (&[0x0f, 0x07], 0x674ff278f2941c87),
    ];
    let base = machine(0, 0, CR4);
    for (bytes, digest) in G0 {
        let run = run_cloned(&base, bytes, 1);
        assert_eq!(
            digest_fields(&outcome_fields(&run)),
            *digest,
            "CPL0 {bytes:02x?}: {:?}",
            run.result
        );
    }
}

// ---- C1-3: Cr4Gated ----------------------------------------------------------------------------------

/// `SGDT [rdi]`, `SIDT [rdi]`, `SLDT [rdi]`, `STR [rdi]` and their register forms at CPL3 store what
/// CPL0 stores (UMIP = 0); the store is a user access: a supervisor page is the #PF of a user write
/// (P = 1, W = 1, U = 1), and so is a read-only user page; nothing is written then.
#[test]
fn sgdt_sidt_sldt_str_at_cpl3_store_what_cpl0_stores_with_user_rights() {
    let forms: &[(&str, &[u8])] = &[
        ("sgdt [rdi]", &[0x0f, 0x01, 0x07]),
        ("sidt [rdi]", &[0x0f, 0x01, 0x0f]),
        ("sldt [rdi]", &[0x0f, 0x00, 0x07]),
        ("str [rdi]", &[0x0f, 0x00, 0x0f]),
        ("sldt eax", &[0x0f, 0x00, 0xc0]),
        ("str eax", &[0x0f, 0x00, 0xc8]),
    ];
    for (name, bytes) in forms {
        let memory_form = bytes[2] >> 6 != 3;
        let cpl0 = run_one(bytes, 0, 0, CR4);
        let cpl3 = run_one(bytes, 3, 0, CR4);
        let exit0 = run_exit(cpl0.result.as_ref().unwrap()).clone();
        assert_eq!(exit0, RunExit::InstructionBudgetExhausted, "{name} at CPL0");
        assert_eq!(
            run_exit(cpl3.result.as_ref().unwrap()),
            &exit0,
            "{name} at CPL3"
        );
        assert_eq!(cpl3.memory.writes, cpl0.memory.writes, "{name}");
        assert_eq!(cpl3.state.gpr, cpl0.state.gpr, "{name}");
        assert_eq!(cpl3.state.rip, cpl0.state.rip, "{name}");
        if !memory_form {
            continue;
        }
        let address = cpl3.memory.writes[0].0;
        for (user, writable) in [(false, true), (true, false)] {
            let (state, mut memory) = machine(3, 0, CR4);
            memory.map(address, user, writable);
            let run = cpl3_sweep::run_on(state, memory, Cells::new(), bytes, 1);
            assert_eq!(
                run_exit(run.result.as_ref().unwrap()),
                &page_fault(0b111, address),
                "{name} user {user} writable {writable}"
            );
            assert!(run.memory.writes.is_empty(), "{name}");
        }
    }
}

#[test]
fn rdtsc_at_cpl3_with_tsd_clear_reads_the_source() {
    // `48 0F 31` (REX.W) is not decoded by the set (an `Unsupported` decode at any CPL).
    for bytes in [&[0x0f, 0x31][..]] {
        let run = run_one(bytes, 3, 0, CR4);
        assert_eq!(
            run_exit(run.result.as_ref().unwrap()),
            &RunExit::InstructionBudgetExhausted,
            "{bytes:02x?}"
        );
        assert_eq!(
            run.state.gpr[Register::Rax as usize],
            cpl3_sweep::TSC & 0xffff_ffff
        );
        assert_eq!(run.state.gpr[Register::Rdx as usize], cpl3_sweep::TSC >> 32);
        assert_eq!(run.state.rip, RIP + bytes.len() as u64);
    }
}

#[test]
fn rdpmc_at_cpl3_is_gp0() {
    // CR4.PCE is outside the policy (= 0): #GP(0) at CPL > 0 (Vol. 2B "RDPMC").
    let run = run_one(&[0x0f, 0x33], 3, 0, CR4);
    assert_eq!(run_exit(run.result.as_ref().unwrap()), &GP0);
    assert_eq!(run.result.as_ref().unwrap().instructions_executed, 0);
}

// ---- C1-5b: INT3 ------------------------------------------------------------------------------------

/// The 64-bit frame of a delivery from CPL3 on the RSP0 stack: SS, RSP, RFLAGS, CS, RIP.
fn frame(memory: &SweepMemory, rsp: u64) -> [u64; 5] {
    let mut frame = [0u64; 5];
    for (index, slot) in frame.iter_mut().enumerate() {
        *slot = memory.peek_u64(rsp + 8 * index as u64).unwrap();
    }
    frame
}

#[test]
fn int3_at_cpl3_is_delivered_through_a_dpl3_gate() {
    // The machine's gate of vector 3 has DPL 3 (Windows); the set has `with_int3`, which must not
    // matter at CPL3, so run it also without the member.
    for with_member in [true, false] {
        let (mut state, mut memory) = machine(3, 0, CR4);
        memory.put(RIP, &[0xcc]);
        let before = state.clone();
        let cells = Cells::new();
        let full = set(&cells);
        let bare = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
            .with_cpuid(&cpl3_sweep::CPUID)
            .with_cpu_owned_cr3(od_isa::MovCr3Long64V1);
        let set = if with_member { full } else { bare };
        let result = run_cpl3_with(&mut state, &mut memory, &set, 1, None).unwrap();
        assert_eq!(result.instructions_executed, 1, "member {with_member}");
        assert_eq!(state.rip, cpl3_sweep::handler(3));
        assert_eq!(state.cs_selector, Some(0x08));
        let rsp0 = cpl3_sweep::KERNEL_STACK + 0x1000;
        assert_eq!(state.gpr[Register::Rsp as usize], rsp0 - 40);
        assert_eq!(
            frame(&memory, rsp0 - 40),
            [
                RIP + 1,
                0x33,
                before.rflags,
                before.gpr[Register::Rsp as usize],
                0x2b
            ],
            "member {with_member}"
        );
        assert_eq!(state.rflags & (1 << 9), 0, "an interrupt gate clears IF");
    }
}

#[test]
fn int3_at_cpl3_through_a_dpl0_gate_is_gp_with_the_idt_selector() {
    let (mut state, mut memory) = machine(3, 0, CR4);
    memory.put(IDT + 16 * 3, &gate(cpl3_sweep::handler(3), 0x08, 0, 0x8e));
    memory.put(RIP, &[0xcc]);
    let before = state.clone();
    let cells = Cells::new();
    let result = run_cpl3_with(&mut state, &mut memory, &set(&cells), 1, None).unwrap();
    assert_eq!(
        run_exit(&result),
        &RunExit::Fault(od_isa::ArchitecturalFault {
            vector: 13,
            error_code: Some(3 * 8 + 2),
            linear_address: None,
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

#[test]
fn int3_at_cpl3_through_an_ist_gate_is_unsupported_ist() {
    let (mut state, mut memory) = machine(3, 0, CR4);
    memory.put(IDT + 16 * 3, &gate(cpl3_sweep::handler(3), 0x08, 1, 0xee));
    memory.put(RIP, &[0xcc]);
    let before = state.clone();
    let cells = Cells::new();
    let result = run_cpl3_with(&mut state, &mut memory, &set(&cells), 1, None).unwrap();
    assert_eq!(run_exit(&result), &RunExit::Unsupported { opcode: 0xcc });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}
