//! S7-32, ADR-0023 slice C1, plan step C1-4: the `IoplSensitive` class at CPL3 of the set against an
//! independent model of Intel SDM 325462-093US:
//!
//! - Vol. 2A "CLI", Vol. 2B "STI" (protected mode, PVI = 0): #GP(0) if IOPL < CPL; otherwise IF is
//!   cleared/set (STI with IF = 0 opens the one-instruction shadow).
//! - Vol. 1 §20.5.1-20.5.2, Vol. 2A "IN", Vol. 2B "OUT": at CPL > IOPL the I/O permission bit map of
//!   the TSS (16-bit base at offset 0x66) decides: the two bytes at `base + port / 8` are read (an
//!   implicit supervisor access, its #PF first); a set bit of `port .. port + width - 1` or a byte
//!   beyond the TSS limit is #GP(0). The CPL3 entry has no port: an allowed access is `Unsupported`.
//! - Vol. 2B "POPF/POPFD/POPFQ", Operation, CPL > 0: the status flags, DF, NT (and AC, ID for the
//!   64-bit form) load; IF only if CPL <= IOPL; IOPL, VIP, VIF, VM and reserved bits never; the 64-bit
//!   form and, by footnote 1, the 16-bit form clear RF; the 16-bit form pops FLAGS[15:0] only.

mod cpl3_sweep;

use cpl3_sweep::{
    CR4, Cells, DataAccess, GP0, IO_MAP_BASE, RIP, STACK_PAGE, SweepMemory, TSS, TSS_LIMIT,
    machine, run_exit, run_on,
};
use od_isa::execute::RunExit;
use od_isa::{AccessOrigin, LongModeState, Register, StiShadow};

const IF: u64 = 1 << 9;
const TF: u64 = 1 << 8;
const RF: u64 = 1 << 16;
const STATUS: u64 = 0x8d5;
const DF: u64 = 1 << 10;
const NT: u64 = 1 << 14;
const AC: u64 = 1 << 18;
const ID: u64 = 1 << 21;
const VM: u64 = 1 << 17;
const VIF: u64 = 1 << 19;
const VIP: u64 = 1 << 20;
const IOPL: u64 = 3 << 12;

// ---- CLI / STI ----------------------------------------------------------------------------------

#[test]
fn cli_and_sti_at_cpl3_follow_iopl() {
    for iopl in 0..4u8 {
        for (bytes, sti) in [(&[0xfa][..], false), (&[0xfb], true)] {
            for interrupt_flag in [0, IF] {
                let (mut state, memory) = machine(3, iopl, CR4);
                state.rflags = (state.rflags & !IF) | interrupt_flag;
                let before = state.clone();
                let run = run_on(state, memory, Cells::new(), bytes, 1);
                let result = run.result.as_ref().unwrap();
                let name = format!("{bytes:02x?} IOPL {iopl} IF {interrupt_flag:#x}");
                // The model: IOPL < CPL (3) is #GP(0) with nothing changed.
                if iopl < 3 {
                    assert_eq!(run_exit(result), &GP0, "{name}");
                    assert_eq!(run.state, before, "{name}");
                    continue;
                }
                assert_eq!(
                    run_exit(result),
                    &RunExit::InstructionBudgetExhausted,
                    "{name}"
                );
                assert_eq!(run.state.rip, RIP + 1, "{name}");
                assert_eq!(run.state.rflags & IF, if sti { IF } else { 0 }, "{name}");
                let shadow = if sti && interrupt_flag == 0 {
                    StiShadow::Blocking
                } else {
                    StiShadow::Clear
                };
                assert_eq!(run.state.sti_shadow, Some(shadow), "{name}");
                assert_eq!(run.state.rflags & !IF, before.rflags & !IF, "{name}");
            }
        }
    }
}

// ---- IN / OUT ------------------------------------------------------------------------------------

/// An I/O form: bytes (the imm8 forms take their port byte from the case), width, DX or imm8.
struct PortForm {
    name: &'static str,
    opcode: &'static [u8],
    width: u8,
    imm8: bool,
}

const FORMS: [PortForm; 6] = [
    PortForm {
        name: "in al, dx",
        opcode: &[0xec],
        width: 1,
        imm8: false,
    },
    PortForm {
        name: "in eax, dx",
        opcode: &[0xed],
        width: 4,
        imm8: false,
    },
    PortForm {
        name: "in ax, dx",
        opcode: &[0x66, 0xed],
        width: 2,
        imm8: false,
    },
    PortForm {
        name: "out dx, al",
        opcode: &[0xee],
        width: 1,
        imm8: false,
    },
    PortForm {
        name: "in al, imm8",
        opcode: &[0xe4],
        width: 1,
        imm8: true,
    },
    PortForm {
        name: "out imm8, al",
        opcode: &[0xe6],
        width: 1,
        imm8: true,
    },
];

/// Which bit of the range the case sets in the map (the others of the range are clear).
#[derive(Clone, Copy, Debug)]
enum MapBit {
    None,
    First,
    Last,
}

/// The machine at CPL3 with `iopl`, DX = `port`, the map bytes of the range written for `bit`, and
/// the I/O map base moved beyond the TSS limit when `beyond`.
fn port_machine(
    port: u16,
    width: u8,
    bit: MapBit,
    beyond: bool,
    iopl: u8,
) -> (LongModeState, SweepMemory) {
    let (mut state, mut memory) = machine(3, iopl, CR4);
    memory.on_demand = false;
    state.gpr[Register::Rdx as usize] = 0x5555_0000 | u64::from(port);
    let map = TSS + u64::from(IO_MAP_BASE);
    let first = u64::from(port / 8);
    for byte in first..=first + 1 {
        if byte < 0x2000 {
            memory.put(map + byte, &[0]);
        }
    }
    let set = match bit {
        MapBit::None => None,
        MapBit::First => Some(u32::from(port)),
        MapBit::Last => Some(u32::from(port) + u32::from(width) - 1),
    };
    if let Some(bit) = set {
        let address = map + u64::from(bit / 8);
        let value = memory.peek(address, 1).unwrap()[0] | (1 << (bit % 8));
        memory.put(address, &[value]);
    }
    if beyond {
        memory.put_u16(TSS + 0x66, TSS_LIMIT as u16);
    }
    (state, memory)
}

/// The model of the outcome: `Some(true)` allowed (`Unsupported`, no port), `Some(false)` #GP(0),
/// and whether the map is read at all.
fn port_model(port: u16, width: u8, bit: MapBit, beyond: bool, iopl: u8) -> (bool, bool) {
    if 3 <= iopl {
        // CPL <= IOPL: no bit map check; the CPL3 entry has no port.
        return (true, false);
    }
    let base = if beyond {
        u64::from(TSS_LIMIT)
    } else {
        u64::from(IO_MAP_BASE)
    };
    if base + u64::from(port / 8) + 1 > u64::from(TSS_LIMIT) {
        return (false, true);
    }
    // Bits of ports above 0xffff are in the byte after the map, which the TSS sets to 0xff.
    if u32::from(port) + u32::from(width) > 0x1_0000 {
        return (false, true);
    }
    (matches!(bit, MapBit::None), true)
}

#[test]
fn port_io_at_cpl3_checks_the_tss_bitmap() {
    let mut cases = 0;
    for form in &FORMS {
        let ports: &[u16] = if form.imm8 {
            &[0x60, 0xf8, 0xcf, 0xff]
        } else {
            // 0x3fe with width 4 crosses a map byte boundary.
            &[0x60, 0x3f8, 0x3fe, 0xcf8, 0xffff]
        };
        for &port in ports {
            for bit in [MapBit::None, MapBit::First, MapBit::Last] {
                for beyond in [false, true] {
                    for iopl in [0u8, 3] {
                        let (state, memory) = port_machine(port, form.width, bit, beyond, iopl);
                        let mut bytes = form.opcode.to_vec();
                        if form.imm8 {
                            bytes.push(port as u8);
                        }
                        let before = state.clone();
                        let run = run_on(state, memory, Cells::new(), &bytes, 1);
                        let result = run.result.as_ref().unwrap();
                        let (allowed, reads) = port_model(port, form.width, bit, beyond, iopl);
                        let name = format!(
                            "{} port {port:#x} {bit:?} beyond {beyond} IOPL {iopl}",
                            form.name
                        );
                        let opcode = *form.opcode.last().unwrap();
                        let expected = if allowed {
                            RunExit::Unsupported { opcode }
                        } else {
                            GP0
                        };
                        assert_eq!(run_exit(result), &expected, "{name}");
                        assert_eq!(result.instructions_executed, 0, "{name}");
                        assert_eq!(run.state, before, "{name}");
                        assert!(run.memory.writes.is_empty(), "{name}");
                        let map_reads = run
                            .memory
                            .accesses
                            .iter()
                            .filter(|access| {
                                access.kind == DataAccess::Read
                                    && access.origin == AccessOrigin::DescriptorTable
                            })
                            .count();
                        assert_eq!(map_reads > 0, reads, "{name}: {:?}", run.memory.accesses);
                        assert_eq!(
                            run.memory.accesses.len(),
                            map_reads,
                            "{name}: only implicit reads"
                        );
                        cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cases, 4 * 5 * 3 * 2 * 2 + 2 * 4 * 3 * 2 * 2);
}

#[test]
fn a_tss_bitmap_on_an_unmapped_page_is_a_supervisor_page_fault_before_gp() {
    // Port 0x8000: the map byte at TSS + 0x68 + 0x1000 = 0x4068 (the second TSS page).
    for bit in [MapBit::None, MapBit::First] {
        let (mut state, mut memory) = port_machine(0x8000, 1, bit, false, 0);
        memory.pages.remove(&0x4000);
        state.gpr[Register::Rdx as usize] = 0x8000;
        let before = state.clone();
        let run = run_on(state, memory, Cells::new(), &[0xec], 1);
        // A supervisor read of a not-present page: P = 0, W = 0, U = 0.
        assert_eq!(
            run_exit(run.result.as_ref().unwrap()),
            &cpl3_sweep::page_fault(0, 0x4068),
            "{bit:?}"
        );
        assert_eq!(run.state, before);
    }
}

// ---- POPF ---------------------------------------------------------------------------------------

/// 64 images over the bits POPF may or may not load (TF always 0; bit 1 set).
fn images() -> Vec<u64> {
    const BITS: u64 = STATUS | DF | IF | IOPL | NT | AC | ID | VM | VIF | VIP | RF;
    let mut x = 0x9e37_79b9_7f4a_7c15u64;
    (0..64)
        .map(|index| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let value = match index {
                0 => 0,
                1 => BITS,
                _ => x & BITS,
            };
            value | 2
        })
        .collect()
}

/// The model of the RFLAGS after a retired POPF: CPL3 by the SDM text, CPL0 by the recorded S2-50
/// rule of the set (`with_popf_cpl0`: status, DF, IF, IOPL, NT, AC, ID; RF, VM, VIF, VIP kept).
fn popf_model(cpl: u8, iopl: u8, before: u64, image: u64, word: bool) -> u64 {
    if cpl == 0 {
        let loaded = STATUS | DF | IF | IOPL | NT | AC | ID;
        return (before & !loaded) | (image & loaded);
    }
    let mut loaded = STATUS | DF | NT | AC | ID;
    if cpl <= iopl {
        loaded |= IF;
    }
    if word {
        loaded &= 0xffff;
    }
    // RF: "RF is always zero after the execution of POPF" (footnote 1), both widths at CPL3.
    ((before & !loaded) | (image & loaded)) & !RF
}

#[test]
fn popf_at_cpl3_follows_if_and_iopl_rules() {
    for cpl in [0u8, 3] {
        for iopl in 0..4u8 {
            for image in images() {
                for word in [false, true] {
                    let (mut state, mut memory) = machine(cpl, iopl, CR4);
                    // A recognisable state: IF set, RF set (as after an IRETQ that loaded it).
                    state.rflags |= IF | RF | STATUS;
                    let rsp = STACK_PAGE + 0xff0;
                    state.gpr[Register::Rsp as usize] = rsp;
                    let width = if word { 2 } else { 8 };
                    memory.put(rsp, &image.to_le_bytes()[..width]);
                    let before = state.clone();
                    let bytes: &[u8] = if word { &[0x66, 0x9d] } else { &[0x9d] };
                    let run = run_on(state, memory, Cells::new(), bytes, 1);
                    let result = run.result.as_ref().unwrap();
                    let name = format!("CPL{cpl} IOPL {iopl} image {image:#x} word {word}");
                    if cpl == 0 && word {
                        // The 16-bit form stays outside the CPL0 member (unchanged at CPL0).
                        assert_eq!(
                            run_exit(result),
                            &RunExit::Unsupported { opcode: 0x9d },
                            "{name}"
                        );
                        continue;
                    }
                    assert_eq!(
                        run_exit(result),
                        &RunExit::InstructionBudgetExhausted,
                        "{name}"
                    );
                    let image = if word { image & 0xffff } else { image };
                    assert_eq!(
                        run.state.rflags,
                        popf_model(cpl, iopl, before.rflags, image, word),
                        "{name}"
                    );
                    assert_eq!(
                        run.state.gpr[Register::Rsp as usize],
                        rsp + width as u64,
                        "{name}"
                    );
                    assert_eq!(run.state.rip, RIP + bytes.len() as u64, "{name}");
                }
            }
        }
    }
}

#[test]
fn popf_with_tf_in_the_image_stays_unsupported_without_change() {
    for (cpl, bytes) in [(0u8, &[0x9d][..]), (3, &[0x9d]), (3, &[0x66, 0x9d])] {
        let (mut state, mut memory) = machine(cpl, 0, CR4);
        let rsp = STACK_PAGE + 0xff0;
        state.gpr[Register::Rsp as usize] = rsp;
        memory.put(rsp, &(2 | TF).to_le_bytes());
        let before = state.clone();
        let run = run_on(state, memory, Cells::new(), bytes, 1);
        assert_eq!(
            run_exit(run.result.as_ref().unwrap()),
            &RunExit::Unsupported { opcode: 0x9d },
            "CPL{cpl} {bytes:02x?}"
        );
        assert_eq!(run.state, before);
    }
}

/// Review A1: the TSS limit of the bit map read. The two map bytes at `IoMapBase + port / 8` must both
/// be within the limit: with the I/O map base at the limit (the second byte beyond it), or one below
/// with port 8, the access is #GP(0) even though the bytes there are 0; one below with port 0 (both
/// bytes within the limit and 0) is allowed (`Unsupported`, the entry has no port).
#[test]
fn the_two_map_bytes_must_be_within_the_tss_limit() {
    for (base, port, allowed) in [
        (TSS_LIMIT as u16, 0u16, false),
        (TSS_LIMIT as u16 - 1, 8, false),
        (TSS_LIMIT as u16 - 1, 0, true),
    ] {
        let (mut state, mut memory) = machine(3, 0, CR4);
        memory.on_demand = false;
        memory.put_u16(TSS + 0x66, base);
        for offset in u64::from(TSS_LIMIT) - 1..=u64::from(TSS_LIMIT) + 1 {
            memory.put(TSS + offset, &[0]);
        }
        state.gpr[Register::Rdx as usize] = u64::from(port);
        let before = state.clone();
        let run = run_on(state, memory, Cells::new(), &[0xec], 1);
        let expected = if allowed {
            RunExit::Unsupported { opcode: 0xec }
        } else {
            GP0
        };
        assert_eq!(
            run_exit(run.result.as_ref().unwrap()),
            &expected,
            "base {base:#x} port {port:#x}"
        );
        assert_eq!(run.state, before);
    }
}
