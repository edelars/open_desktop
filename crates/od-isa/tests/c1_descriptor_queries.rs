//! S7-32, ADR-0023 slice C1, plan step C1-5c: LAR, LSL, VERR and VERW in the set at CPL0 and CPL3
//! against a model written from the Intel SDM pages (Vol. 2A "LAR—Load Access Rights Byte", Vol. 2B
//! "LSL—Load Segment Limit", "VERR/VERW—Verify a Segment for Reading or Writing"; read from the
//! felixcloutier.com copy of the SDM text):
//!
//! - ZF = 0 for a null selector, a descriptor beyond the table limit, a type the instruction does not
//!   accept (LAR: code, data, LDT, 64-bit TSS available/busy, 64-bit call gate; LSL: code, data, LDT,
//!   64-bit TSS; VERR: data or readable code; VERW: writable data), or a descriptor that is not
//!   visible (not conforming code and CPL > DPL or RPL > DPL); P is not checked;
//! - on ZF = 0 the destination is not changed; only ZF changes;
//! - LAR: `high dword & 0x00F0_FF00` (bits 19:16 undefined, the model gives 0; 16-bit: low 16 bits);
//!   LSL: the 20-bit limit, scaled by G (`(limit << 12) | 0xFFF`), 16-bit: the low 16 bits; a 32-bit
//!   destination is zero-extended.
//! - The IA-32e system descriptors are 16 bytes; the footnote of the LSL table ("bits 12:8 of the upper
//!   4 bytes must be 0") is applied by the model to LAR too.

mod cpl3_sweep;

use cpl3_sweep::{
    CR4, Cells, DATA, GDT, GS_BASE, PROCESSOR_NUMBER, RIP, machine, page_fault, run_exit, run_on,
};
use od_isa::Register;
use od_isa::execute::RunExit;

const ZF: u64 = 1 << 6;
const SLOT: u64 = 0x38;
const RAX: u64 = 0xdead_beef_cafe_f00d;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Lar,
    Lsl,
    Verr,
    Verw,
}

fn bytes(kind: Kind, size: u8) -> Vec<u8> {
    let prefix: &[u8] = match size {
        16 => &[0x66],
        64 => &[0x48],
        _ => &[],
    };
    let body: &[u8] = match kind {
        Kind::Lar => &[0x0f, 0x02, 0xc1],
        Kind::Lsl => &[0x0f, 0x03, 0xc1],
        Kind::Verr => &[0x0f, 0x00, 0xe1],
        Kind::Verw => &[0x0f, 0x00, 0xe9],
    };
    [prefix, body].concat()
}

/// The descriptor of a case: S, type, DPL, P, G over a fixed base, limit 0xa_bcde and AVL, L, D.
fn descriptor(segment: bool, kind_bits: u8, dpl: u8, present: bool, granular: bool) -> u64 {
    let access = u64::from(kind_bits)
        | (u64::from(segment) << 4)
        | (u64::from(dpl) << 5)
        | (u64::from(present) << 7);
    let flags = 0x7 | (u64::from(granular) << 3);
    0xbcde
        | (0x5678 << 16)
        | (0x34 << 32)
        | (access << 40)
        | (0xa << 48)
        | (flags << 52)
        | (0x12 << 56)
}

/// The model of the pages: `Some(value)` with ZF = 1.
fn model(kind: Kind, desc: u64, high: u64, selector: u16, cpl: u8) -> Option<u64> {
    if selector & !3 == 0 || selector & 4 != 0 {
        return None;
    }
    let access = (desc >> 40) as u8;
    let segment = access & 0x10 != 0;
    let kind_bits = access & 0xf;
    let code = segment && kind_bits & 8 != 0;
    let conforming = code && kind_bits & 4 != 0;
    let valid = match kind {
        Kind::Lar => segment || [2, 9, 0xb, 0xc].contains(&kind_bits),
        Kind::Lsl => segment || [2, 9, 0xb].contains(&kind_bits),
        Kind::Verr => segment && (!code || kind_bits & 2 != 0),
        Kind::Verw => segment && !code && kind_bits & 2 != 0,
    };
    let dpl = access >> 5 & 3;
    let rpl = (selector & 3) as u8;
    let visible = conforming || (cpl <= dpl && rpl <= dpl);
    if !valid || !visible {
        return None;
    }
    if !segment && (high >> 40) & 0x1f != 0 {
        return None;
    }
    let limit = (desc & 0xffff) | ((desc >> 48 & 0xf) << 16);
    Some(match kind {
        Kind::Lar => (desc >> 32) & 0x00f0_ff00,
        Kind::Lsl if desc >> 55 & 1 != 0 => (limit << 12) | 0xfff,
        Kind::Lsl => limit,
        Kind::Verr | Kind::Verw => 0,
    })
}

/// RAX after a destination write of `size` bits of `value` (16: the low word; 32/64: zero-extended).
fn written(size: u8, value: u64) -> u64 {
    if size == 16 {
        (RAX & !0xffff) | (value & 0xffff)
    } else {
        value
    }
}

struct Case {
    kind: Kind,
    desc: u64,
    high: u64,
    selector: u16,
    cpl: u8,
    size: u8,
}

fn check(case: &Case) {
    let (mut state, mut memory) = cpl3_sweep::machine_cached(case.cpl, 0, CR4);
    memory.put_u64(GDT + SLOT, case.desc);
    memory.put_u64(GDT + SLOT + 8, case.high);
    state.gpr[Register::Rcx as usize] = 0xffff_0000 | u64::from(case.selector);
    state.gpr[Register::Rax as usize] = RAX;
    state.rflags |= 0x8d5 & !ZF;
    let before = state.clone();
    let code = bytes(case.kind, case.size);
    let run = run_on(state, memory, Cells::new(), &code, 1);
    let name = format!(
        "{:?} desc {:#018x} selector {:#x} CPL{} size {}",
        case.kind, case.desc, case.selector, case.cpl, case.size
    );
    assert_eq!(
        run_exit(run.result.as_ref().unwrap()),
        &RunExit::InstructionBudgetExhausted,
        "{name}"
    );
    let expected = model(case.kind, case.desc, case.high, case.selector, case.cpl);
    assert_eq!(run.state.rflags & ZF != 0, expected.is_some(), "{name}");
    assert_eq!(
        run.state.rflags & !ZF,
        before.rflags & !ZF,
        "{name}: only ZF"
    );
    let rax = match (expected, case.kind) {
        (Some(value), Kind::Lar | Kind::Lsl) => written(case.size, value),
        _ => RAX,
    };
    assert_eq!(run.state.gpr[Register::Rax as usize], rax, "{name}");
    assert_eq!(run.state.rip, RIP + code.len() as u64, "{name}");
    assert!(run.memory.writes.is_empty(), "{name}: no Accessed bit");
}

#[test]
fn descriptor_queries_match_the_sdm_model() {
    let mut cases = 0;
    for kind in [Kind::Lar, Kind::Lsl, Kind::Verr, Kind::Verw] {
        let sizes: &[u8] = if matches!(kind, Kind::Lar | Kind::Lsl) {
            &[16, 32, 64]
        } else {
            &[32]
        };
        for segment in [false, true] {
            for kind_bits in 0..16u8 {
                for dpl in 0..4u8 {
                    for rpl in 0..4u16 {
                        for present in [false, true] {
                            for granular in [false, true] {
                                for cpl in [0u8, 3] {
                                    for &size in sizes {
                                        check(&Case {
                                            kind,
                                            desc: descriptor(
                                                segment, kind_bits, dpl, present, granular,
                                            ),
                                            high: 0,
                                            selector: SLOT as u16 | rpl,
                                            cpl,
                                            size,
                                        });
                                        cases += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(
        cases,
        2 * (2 * 16 * 4 * 4 * 2 * 2 * 2 * 3) + 2 * (2 * 16 * 4 * 4 * 2 * 2 * 2)
    );
}

#[test]
fn a_system_descriptor_with_a_type_in_its_upper_half_clears_zf() {
    for kind in [Kind::Lar, Kind::Lsl] {
        for high in [0u64, 1 << 40, 0x1f << 40, 1 << 45] {
            check(&Case {
                kind,
                desc: descriptor(false, 0x9, 3, true, false),
                high,
                selector: SLOT as u16 | 3,
                cpl: 3,
                size: 32,
            });
        }
    }
}

#[test]
fn lsl_of_the_windows_processor_selector_returns_the_scaled_limit() {
    // `lsl eax, ecx` with ECX = 0x53 (GDT 0x50 of the machine: user data, the processor number in
    // the limit), then `shr eax, 14` (done here: the step of the CPL3 entry runs one form).
    for cpl in [0u8, 3] {
        let (mut state, memory) = machine(cpl, 0, CR4);
        state.gpr[Register::Rcx as usize] = 0x53;
        let run = run_on(state, memory, Cells::new(), &[0x0f, 0x03, 0xc1], 1);
        assert_eq!(
            run_exit(run.result.as_ref().unwrap()),
            &RunExit::InstructionBudgetExhausted
        );
        assert_ne!(run.state.rflags & ZF, 0, "CPL{cpl}");
        assert_eq!(
            run.state.gpr[Register::Rax as usize] >> 14,
            PROCESSOR_NUMBER,
            "CPL{cpl}"
        );
    }
}

#[test]
fn verw_with_a_gs_override_reads_the_selector_through_the_gs_base() {
    // ISSUE-718: `verw word gs:[0xb02a]` (ntdll).
    let code = [0x65, 0x0f, 0x00, 0x2c, 0x25, 0x2a, 0xb0, 0x00, 0x00];
    for (selector, writable) in [(0x2bu16, true), (0x33, false)] {
        for cpl in [0u8, 3] {
            let (state, mut memory) = machine(cpl, 0, CR4);
            memory.map(GS_BASE + 0xb02a, true, true);
            memory.put(GS_BASE + 0xb02a, &selector.to_le_bytes());
            let run = run_on(state, memory, Cells::new(), &code, 1);
            assert_eq!(
                run_exit(run.result.as_ref().unwrap()),
                &RunExit::InstructionBudgetExhausted
            );
            assert_eq!(
                run.state.rflags & ZF != 0,
                writable,
                "CPL{cpl} {selector:#x}"
            );
            let read = run
                .memory
                .accesses
                .iter()
                .find(|access| access.len == 2)
                .unwrap();
            assert_eq!(read.address, GS_BASE + 0xb02a);
        }
    }
}

#[test]
fn a_selector_beyond_the_gdt_limit_clears_zf_and_keeps_the_destination() {
    for selector in [0x58u16, 0x5b, 0x63, 0xfffb] {
        for kind in [Kind::Lar, Kind::Lsl, Kind::Verr, Kind::Verw] {
            let (mut state, memory) = machine(3, 0, CR4);
            state.gpr[Register::Rcx as usize] = u64::from(selector);
            state.gpr[Register::Rax as usize] = RAX;
            state.rflags |= ZF;
            let run = run_on(state, memory, Cells::new(), &bytes(kind, 32), 1);
            assert_eq!(
                run_exit(run.result.as_ref().unwrap()),
                &RunExit::InstructionBudgetExhausted
            );
            assert_eq!(run.state.rflags & ZF, 0, "{kind:?} {selector:#x}");
            assert_eq!(run.state.gpr[Register::Rax as usize], RAX);
            assert!(
                run.memory.accesses.is_empty(),
                "{kind:?} {selector:#x}: no descriptor read"
            );
        }
    }
}

#[test]
fn the_memory_source_at_cpl3_is_a_user_access() {
    // `verw [rdi]` and `lar eax, [rdi]` with RDI on a supervisor page: the #PF of a user read.
    for code in [&[0x0f, 0x00, 0x2f][..], &[0x0f, 0x02, 0x07]] {
        let (state, mut memory) = machine(3, 0, CR4);
        let rdi = state.gpr[Register::Rdi as usize];
        memory.map(DATA, false, true);
        let before = state.clone();
        let run = run_on(state, memory, Cells::new(), code, 1);
        assert_eq!(
            run_exit(run.result.as_ref().unwrap()),
            &page_fault(0b101, rdi),
            "{code:02x?}"
        );
        assert_eq!(run.state, before);
    }
}
