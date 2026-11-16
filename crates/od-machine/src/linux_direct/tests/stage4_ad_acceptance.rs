//! S4-09f: system acceptance of the accessed/dirty (A/D) bits through the production set loop.
//!
//! The scenarios run `run_cpl0_set_paged_2m_ram` (the loop of the Linux board set facade) over a
//! mixed page-table tree whose entries all start with A = D = 0 (nothing is preset: the CPU has to
//! set the bits itself). The loop is stepped one instruction at a time (the `stop` poll ends the
//! call after one retired instruction or after one delivered `#PF`), and after every step the
//! test compares
//!
//! - the page-table frames with the frames before the step: the exact set of changed entries and
//!   their new values (literals below, not computed by the code under test), and
//! - the committed-write log of the step, in order (literal `(physical address, length)` pairs).
//!
//! The loop and its helpers are `cfg(test)` and `pub(crate)` in `od-machine`, so this lives next
//! to the other set-loop board tests (`tests/system` cannot reach it). A/D bits of the guest
//! descriptor tables (GDT/IDT/TSS, byte 5 of a descriptor) are a different class of side effect
//! and not part of this scenario.

use super::*;
use od_isa::state::{Gdtr, Idtr};

const A: u64 = 1 << 5;
const D: u64 = 1 << 6;

// ---- physical layout ----

/// The page-table frames (PML4 at `P0`), 14 consecutive frames.
const P0: u64 = 0x0400_0000;
const PML4: u64 = P0;
const PDPT0: u64 = P0 + 0x1000;
const PD0: u64 = P0 + 0x2000;
const PT0: u64 = P0 + 0x3000;
const PDPT1: u64 = P0 + 0x4000;
const PD1: u64 = P0 + 0x5000;
const FAR_PT: u64 = P0 + 0x6000;
const PDPT2: u64 = P0 + 0x7000;
const PD2: u64 = P0 + 0x8000;
const NP_PT: u64 = P0 + 0x9000;
const RO_PT: u64 = P0 + 0xA000;
const PDPT3: u64 = P0 + 0xB000;
const PD3: u64 = P0 + 0xC000;
const PT3: u64 = P0 + 0xD000;
const TABLES_END: u64 = P0 + 0xE000;

/// Data frames of root 0 (PT0 index `k` maps `F0 + 0x1000 * k`).
const F0: u64 = 0x0500_0000;
/// Data frames of the far 4 KiB tree (FAR_PT index `k` maps `FF + 0x1000 * k`).
const FF: u64 = 0x0510_0000;
/// The frame of the single page of the write-first 4 KiB tree.
const WR_FRAME: u64 = 0x0520_0000;
/// The 2 MiB frames: PD0[1] (code), PD2[0], PD2[1], PD2[2].
const M_CODE: u64 = 0x0640_0000;
const M0: u64 = 0x0600_0000;
const M1: u64 = 0x0620_0000;
const M2_FRAME: u64 = 0x0660_0000;

// ---- linear layout ----

const CODE: u64 = 0x1000;
const STACK_TOP: u64 = 0x8000;
const IDT: u64 = 0x8000;
const GDT: u64 = 0x8800;
const HANDLER: u64 = 0x9000;
/// Window onto the frame of `NP_PT` (so the handler can repair its entry).
const WINDOW: u64 = 0xB000;
const RO_PAGE: u64 = 0xC000;
/// PD0[1]: the 2 MiB code page (a `HLT`).
const M_CODE_LINEAR: u64 = 0x20_0000;
/// PD0[4]: the subtree with the nonpresent PTE.
const NP_LINEAR: u64 = 0x80_0000;
/// PD0[6]: the PDE that denies writes over a writable PTE.
const RO_PDE_LINEAR: u64 = 0xC0_0000;
/// PML4[1]: the far 4 KiB tree.
const FAR: u64 = 0x80_0000_0000;
/// PML4[2]: the 2 MiB tree.
const MTREE: u64 = 0x100_0000_0000;
/// PML4[3]: the 4 KiB tree whose first access is a write.
const WR: u64 = 0x180_0000_0000;

const RCX_VALUE: u64 = 0x0123_4567_89AB_CDEF;
const DATA0: u64 = 0x1111_2222_3333_4444;
const NP_DATA: u64 = 0xCAFE_F00D_0BAD_BEEF;
const M2_DATA: u64 = 0x5555_6666_7777_8888;

fn w8(address: u64) -> (u64, u64) {
    (address, 8)
}

/// All entries of the tree as `(physical address, value, leaf)`. Every value has A = D = 0.
/// `preset` ORs A (and D on a leaf) into the present entries: the differential run.
fn tree(preset: bool) -> Vec<(u64, u64)> {
    // (address, value, is a leaf)
    let raw: &[(u64, u64, bool)] = &[
        (PML4, PDPT0 | 3, false),
        (PML4 + 8, PDPT1 | 3, false),
        (PML4 + 16, PDPT2 | 3, false),
        (PML4 + 24, PDPT3 | 3, false),
        (PDPT0, PD0 | 3, false),
        (PDPT1, PD1 | 3, false),
        (PDPT2, PD2 | 3, false),
        (PDPT3, PD3 | 3, false),
        (PD0, PT0 | 3, false),
        (PD0 + 8, M_CODE | 0x83, true),
        (PD0 + 4 * 8, NP_PT | 3, false),
        // The PDE denies writes (R/W = 0); the PTE under it is writable.
        (PD0 + 6 * 8, RO_PT | 1, false),
        (PD1, FAR_PT | 3, false),
        (PD2, M0 | 0x83, true),
        (PD2 + 8, M1 | 0x83, true),
        (PD2 + 16, M2_FRAME | 0x83, true),
        (PD3, PT3 | 3, false),
        (PT3, WR_FRAME | 3, true),
        // Root 0, 4 KiB pages: code, stack, IDT and GDT, handler, window onto NP_PT, read-only.
        (PT0 + 8, (F0 + 0x1000) | 3, true),
        (PT0 + 7 * 8, (F0 + 0x7000) | 3, true),
        (PT0 + 8 * 8, (F0 + 0x8000) | 3, true),
        (PT0 + 9 * 8, (F0 + 0x9000) | 3, true),
        (PT0 + 0xB * 8, NP_PT | 3, true),
        (PT0 + 0xC * 8, (F0 + 0xC000) | 1, true),
        // The nonpresent PTE (P = 0; the repair is the guest handler's job) and the writable
        // PTE under the write-denying PDE.
        (NP_PT, (F0 + 0xE000) | 2, true),
        (RO_PT, (F0 + 0xF000) | 3, true),
        // The far 4 KiB tree: read/write page, write-first page, RMW page, LOCK page, the pair of
        // a successful cross-page store, the ECAM page, a pair whose second page is nonpresent
        // and a pair whose second page is read-only.
        (FAR_PT, FF | 3, true),
        (FAR_PT + 8, (FF + 0x1000) | 3, true),
        (FAR_PT + 2 * 8, (FF + 0x2000) | 3, true),
        (FAR_PT + 3 * 8, (FF + 0x3000) | 3, true),
        (FAR_PT + 4 * 8, (FF + 0x4000) | 3, true),
        (FAR_PT + 5 * 8, (FF + 0x5000) | 3, true),
        (FAR_PT + 6 * 8, (crate::PCI_ECAM_DESCRIPTOR.base) | 3, true),
        (FAR_PT + 7 * 8, (FF + 0x7000) | 3, true),
        (FAR_PT + 8 * 8, (FF + 0x8000) | 2, true),
        (FAR_PT + 9 * 8, (FF + 0x9000) | 3, true),
        (FAR_PT + 10 * 8, (FF + 0xA000) | 1, true),
    ];
    raw.iter()
        .map(|&(address, value, leaf)| {
            let preset_bits = if value & 1 == 0 {
                0
            } else if leaf {
                A | D
            } else {
                A
            };
            (address, value | if preset { preset_bits } else { 0 })
        })
        .collect()
}

/// `MOV r64, imm64` for RBX (`0xBB`) or RDX (`0xBA`).
fn mov_imm64(opcode: u8, value: u64) -> Vec<u8> {
    let mut code = vec![0x48, opcode];
    code.extend(value.to_le_bytes());
    code
}

struct Out {
    report: PagedCpl0SetReport,
    writes: Vec<(u64, u64)>,
    /// `(entry address, new value)` of every qword of the page-table frames that changed.
    delta: Vec<(u64, u64)>,
}

struct Run {
    boot: LinuxDirectBoot,
    state: LongModeState,
    tables: Vec<u64>,
}

impl Run {
    fn new(code: &[u8], preset: bool, regs: &[(Register, u64)], idt: bool) -> Self {
        let (mut boot, _) = boot_with_entry(&[0xF4]);
        for zone in &boot.zones {
            assert!(
                zone.end <= 0x0400_0000 || zone.start >= 0x0700_0000,
                "the scenario frames overlap the board zone {zone:?}"
            );
        }
        // The frames start empty: no other table entry, no stale byte.
        write_phys(&mut boot, P0, &vec![0u8; (TABLES_END - P0) as usize]);
        for (address, value) in tree(preset) {
            write_phys(&mut boot, address, &value.to_le_bytes());
        }
        write_phys(&mut boot, F0 + 0x1000, code);
        // IDT gate 14: an interrupt gate (type 0xE, present) to the handler through selector 0x08.
        let mut gate = [0u8; 16];
        gate[0..2].copy_from_slice(&(HANDLER as u16).to_le_bytes());
        gate[2..4].copy_from_slice(&0x08u16.to_le_bytes());
        gate[5] = 0x8E;
        write_phys(&mut boot, F0 + 0x8000 + 14 * 16, &gate);
        // GDT[1] (selector 0x08): a 64-bit ring 0 code descriptor with A already set.
        write_phys(
            &mut boot,
            F0 + 0x8000 + (GDT - IDT) + 8,
            &0x00af_9b00_0000_ffff_u64.to_le_bytes(),
        );
        // The selectors the board's CPU state holds (the frame's CS and SS, which `IRETQ` loads
        // again): a ring 0 code and data descriptor with A already set.
        let (cs, ss) = (
            boot.board.state().cs_selector.unwrap_or(0),
            boot.board.state().ss_selector.unwrap_or(0),
        );
        for (selector, descriptor) in [(cs, 0x00af_9b00_0000_ffff_u64), (ss, 0x00cf_9300_0000_ffff)]
        {
            if selector >> 3 != 0 {
                let at = F0 + 0x8000 + (GDT - IDT) + u64::from(selector & !7);
                write_phys(&mut boot, at, &descriptor.to_le_bytes());
            }
        }
        // The handler: drop the error code, store `0x0500_E003` (the PTE repaired as present, A and
        // D clear) through the window onto `NP_PT`, return.
        let mut handler = vec![0x48, 0x83, 0xC4, 0x08]; // ADD RSP, 8
        handler.extend([0x48, 0xC7, 0xC2]); // MOV RDX, imm32
        handler.extend((WINDOW as u32).to_le_bytes());
        handler.extend([0x48, 0xC7, 0xC6]); // MOV RSI, imm32
        handler.extend(((F0 + 0xE000) as u32 | 3).to_le_bytes());
        handler.extend([0x48, 0x89, 0x32]); // MOV [RDX], RSI
        handler.extend([0x48, 0xCF]); // IRETQ
        write_phys(&mut boot, F0 + 0x9000, &handler);
        // Data.
        write_phys(&mut boot, FF + 0x10, &DATA0.to_le_bytes());
        write_phys(&mut boot, FF + 0x2000 + 0x20, &0x1000u64.to_le_bytes());
        write_phys(&mut boot, FF + 0x3000 + 0x30, &0x2000u64.to_le_bytes());
        write_phys(&mut boot, F0 + 0xE000 + 0x18, &NP_DATA.to_le_bytes());
        write_phys(&mut boot, M1 + 0x20, &0x3000u64.to_le_bytes());
        write_phys(&mut boot, M2_FRAME + 0x10, &M2_DATA.to_le_bytes());
        // The 2 MiB code page: a `HLT`.
        write_phys(&mut boot, M_CODE, &[0xF4]);

        let mut state = boot.board.state().clone();
        state.cr3 = Some(PML4);
        state.rip = CODE;
        state.cr0 = Some(state.cr0.unwrap_or(0) | (1 << 16));
        state.idtr = idt.then_some(Idtr {
            base: IDT,
            limit: 0xFF,
        });
        state.gdtr = Some(Gdtr {
            base: GDT,
            limit: 0xFF,
        });
        state.gpr[Register::Rax as usize] = 0;
        state.gpr[Register::Rsp as usize] = STACK_TOP;
        for &(register, value) in regs {
            state.gpr[register as usize] = value;
        }
        let mut run = Self {
            boot,
            state,
            tables: Vec::new(),
        };
        run.tables = run.snapshot();
        run
    }

    fn snapshot(&mut self) -> Vec<u64> {
        let mut bytes = vec![0u8; (TABLES_END - P0) as usize];
        read(&mut self.boot.board, P0, &mut bytes).expect("the table frames are RAM");
        bytes
            .chunks_exact(8)
            .map(|chunk| u64::from_le_bytes(chunk.try_into().unwrap()))
            .collect()
    }

    fn qword(&mut self, address: u64) -> u64 {
        guest_qword(&mut self.boot, address)
    }

    /// One instruction (or one delivery) of the production loop. The loop polls `stop` at the top
    /// of each iteration: the first poll lets the instruction run, the second one ends the call.
    fn step(&mut self) -> Out {
        self.run(1_000, true)
    }

    fn run(&mut self, budget: u64, single: bool) -> Out {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let set = switch_set(&cli, &halt);
        let efer = self.boot.board.efer();
        let polls = std::cell::Cell::new(0u32);
        let mut writes = Vec::new();
        let report = run_cpl0_set_paged_2m_ram(
            &mut self.state,
            self.boot.board.bus_mut(),
            PML4,
            efer,
            long64(),
            budget,
            INTEGER_FLAGS,
            &set,
            None,
            || {
                polls.set(polls.get() + 1);
                single && polls.get() > 1
            },
            |range| writes.push((range.start().get(), range.len())),
        );
        let after = self.snapshot();
        let delta = after
            .iter()
            .zip(&self.tables)
            .enumerate()
            .filter(|(_, (after, before))| after != before)
            .map(|(index, (after, _))| (P0 + 8 * index as u64, *after))
            .collect();
        self.tables = after;
        Out {
            report,
            writes,
            delta,
        }
    }
}

#[track_caller]
fn check(out: &Out, name: &str, delta: &[(u64, u64)], writes: &[(u64, u64)]) {
    assert_eq!(
        out.delta.as_slice(),
        delta,
        "{name}: changed table entries (actual {:x?})",
        out.delta
    );
    assert_eq!(
        out.writes.as_slice(),
        writes,
        "{name}: committed writes in order (actual {:x?})",
        out.writes
    );
}

#[track_caller]
fn retired(out: &Out, name: &str, count: u64, delivered: u64) {
    assert_eq!(
        (
            out.report.instructions_executed,
            out.report.delivered_page_faults
        ),
        (count, delivered),
        "{name}: retired, delivered #PF ({:?})",
        out.report
    );
}

fn stopped() -> PagedCpl0SetExit {
    PagedCpl0SetExit::Run(RunExit::StopRequested)
}

/// The sequence of the scenario. `RBX` and the other registers are loaded by the state or by an
/// `MOV imm` of the program.
struct Program {
    code: Vec<u8>,
    call_return: u64,
}

fn main_program() -> Program {
    let mut code = Vec::new();
    code.push(0x90); // s0   NOP
    code.extend([0x48, 0x8B, 0x03]); // s1   MOV RAX, [RBX]        (far page 0, read)
    code.extend([0x48, 0x89, 0x0B]); // s2   MOV [RBX], RCX        (far page 0, write)
    code.extend([0x48, 0x89, 0x4D, 0x00]); // s3   MOV [RBP], RCX  (far page 1, first access a write)
    code.extend([0x48, 0x01, 0x0A]); // s4   ADD [RDX], RCX        (far page 2, RMW)
    code.extend([0xF0, 0x48, 0x01, 0x0E]); // s5   LOCK ADD [RSI], RCX (far page 3)
    code.push(0x51); // s6   PUSH RCX
    code.extend([0xE8, 0, 0, 0, 0]); // s7   CALL +0
    let call_return = CODE + code.len() as u64;
    code.extend([0x48, 0x89, 0x0F]); // s8   MOV [RDI], RCX        (store across 4 KiB pages 4 and 5)
    code.extend(mov_imm64(0xBB, WR + 0x40)); // s9   MOV RBX, WR + 0x40
    code.extend([0x48, 0x89, 0x0B]); // s10  MOV [RBX], RCX        (write-first 4 KiB tree)
    code.extend(mov_imm64(
        0xBB,
        FAR + 6 * 0x1000 + (crate::PCI_ECAM_DESCRIPTOR.base & 0xfff) + 0x10,
    )); // s11
    code.extend([0x89, 0x0B]); // s12  MOV [RBX], ECX        (ECAM dword store)
    code.extend(mov_imm64(0xBB, MTREE + 0x10)); // s13
    code.extend([0x48, 0x89, 0x0B]); // s14  MOV [RBX], RCX        (2 MiB page 0, write first)
    code.extend(mov_imm64(0xBB, MTREE + 2 * 0x20_0000 + 0x10)); // s15
    code.extend([0x48, 0x8B, 0x03]); // s16  MOV RAX, [RBX]        (2 MiB page 2, read)
    code.extend(mov_imm64(0xBA, MTREE + 0x20_0000 + 0x20)); // s17
    code.extend([0x48, 0x01, 0x0A]); // s18  ADD [RDX], RCX        (2 MiB page 1, RMW)
    code.extend([0x48, 0xC7, 0xC3]); // s19  MOV RBX, imm32 (NP_LINEAR + 0x18)
    code.extend(((NP_LINEAR + 0x18) as u32).to_le_bytes());
    let fault_rip = CODE + code.len() as u64;
    assert_eq!(
        fault_rip, FAULT_RIP,
        "the faulting load is where the literals say"
    );
    code.extend([0x48, 0x8B, 0x03]); // s20  MOV RAX, [RBX]        (#PF: nonpresent; s26 retries it)
    let jmp_at = CODE + code.len() as u64;
    code.push(0xE9); // s27  JMP rel32 to the 2 MiB code page
    code.extend(((M_CODE_LINEAR as i64 - (jmp_at as i64 + 5)) as i32).to_le_bytes());
    Program { code, call_return }
}

/// The RIP of the faulting `MOV RAX, [RBX]` of `main_program`, as a literal: the code before it
/// is 1 + 3 + 3 + 4 + 3 + 4 + 1 + 5 + 3 + 10 + 3 + 10 + 2 + 10 + 3 + 10 + 3 + 10 + 3 + 7 bytes.
const FAULT_RIP: u64 = CODE + 98;

fn main_registers() -> [(Register, u64); 6] {
    [
        (Register::Rbx, FAR + 0x10),
        (Register::Rbp, FAR + 0x1000 + 0x20),
        (Register::Rdx, FAR + 0x2000 + 0x20),
        (Register::Rsi, FAR + 0x3000 + 0x30),
        (Register::Rdi, FAR + 0x4000 + 0xFFC),
        (Register::Rcx, RCX_VALUE),
    ]
}

#[test]
fn a_d_acceptance_mixed_tree_sequence_through_the_set_loop_matches_the_literal_bits_and_write_order()
 {
    let program = main_program();
    let mut run = Run::new(&program.code, false, &main_registers(), true);
    // Nothing is preset: every entry of the tree has A = D = 0.
    assert!(
        run.tables
            .iter()
            .all(|entry| entry & (A | D) == 0 || entry & 1 == 0),
        "the tree must start without A and D"
    );
    let mut all_writes: Vec<(u64, u64)> = Vec::new();
    let mut retired_total = 0;
    let mut step = |run: &mut Run, name: &str, count: u64, delivered: u64| {
        let out = run.step();
        retired(&out, name, count, delivered);
        assert_eq!(out.report.exit, stopped(), "{name}: {:?}", out.report);
        retired_total += count;
        all_writes.extend(out.writes.iter().copied());
        out
    };

    // s0: instruction fetch alone. A on the whole path of the code page (PML4E, PDPTE, PDE,
    // PTE), no D (case c).
    let out = step(&mut run, "s0 fetch", 1, 0);
    check(
        &out,
        "s0 fetch",
        &[
            (PML4, PDPT0 | 3 | A),
            (PDPT0, PD0 | 3 | A),
            (PD0, PT0 | 3 | A),
            (PT0 + 8, (F0 + 0x1000) | 3 | A),
        ],
        &[w8(PML4), w8(PDPT0), w8(PD0), w8(PT0 + 8)],
    );

    // s1: read of a fresh page of another PML4 entry. A on all four levels, no D (case a).
    let out = step(&mut run, "s1 read", 1, 0);
    check(
        &out,
        "s1 read",
        &[
            (PML4 + 8, PDPT1 | 3 | A),
            (PDPT1, PD1 | 3 | A),
            (PD1, FAR_PT | 3 | A),
            (FAR_PT, FF | 3 | A),
        ],
        &[w8(PML4 + 8), w8(PDPT1), w8(PD1), w8(FAR_PT)],
    );
    assert_eq!(run.state.gpr[Register::Rax as usize], DATA0);

    // s2: write to the page the read touched. D on the leaf only, then the operand (case b).
    let out = step(&mut run, "s2 write", 1, 0);
    check(
        &out,
        "s2 write",
        &[(FAR_PT, FF | 3 | A | D)],
        &[w8(FAR_PT), w8(FF + 0x10)],
    );
    assert_eq!(run.qword(FF + 0x10), RCX_VALUE);

    // s3: write to a page whose first access is the write: A, then D, then the operand.
    let out = step(&mut run, "s3 write first", 1, 0);
    check(
        &out,
        "s3 write first",
        &[(FAR_PT + 8, (FF + 0x1000) | 3 | A | D)],
        &[w8(FAR_PT + 8), w8(FAR_PT + 8), w8(FF + 0x1020)],
    );
    assert_eq!(run.qword(FF + 0x1020), RCX_VALUE);

    // s4: RMW (`ADD [mem], reg`): the read walk sets A, the write walk sets D (one write of the
    // entry each, nothing more), then the operand (case f).
    let out = step(&mut run, "s4 RMW", 1, 0);
    check(
        &out,
        "s4 RMW",
        &[(FAR_PT + 2 * 8, (FF + 0x2000) | 3 | A | D)],
        &[w8(FAR_PT + 2 * 8), w8(FAR_PT + 2 * 8), w8(FF + 0x2020)],
    );
    assert_eq!(run.qword(FF + 0x2020), 0x1000 + RCX_VALUE);

    // s5: the same for the `LOCK`-prefixed form (case g).
    let out = step(&mut run, "s5 LOCK", 1, 0);
    check(
        &out,
        "s5 LOCK",
        &[(FAR_PT + 3 * 8, (FF + 0x3000) | 3 | A | D)],
        &[w8(FAR_PT + 3 * 8), w8(FAR_PT + 3 * 8), w8(FF + 0x3030)],
    );
    assert_eq!(run.qword(FF + 0x3030), 0x2000 + RCX_VALUE);

    // s6: `PUSH`: the stack page gets A and D, then the slot is written.
    let out = step(&mut run, "s6 PUSH", 1, 0);
    check(
        &out,
        "s6 PUSH",
        &[(PT0 + 7 * 8, (F0 + 0x7000) | 3 | A | D)],
        &[w8(PT0 + 7 * 8), w8(PT0 + 7 * 8), w8(F0 + 0x7FF8)],
    );
    assert_eq!(run.state.gpr[Register::Rsp as usize], 0x7FF8);
    assert_eq!(run.qword(F0 + 0x7FF8), RCX_VALUE);

    // s7: `CALL`: the stack page already has A and D, so only the operand is committed.
    let out = step(&mut run, "s7 CALL", 1, 0);
    check(&out, "s7 CALL", &[], &[w8(F0 + 0x7FF0)]);
    assert_eq!(run.state.gpr[Register::Rsp as usize], 0x7FF0);
    assert_eq!(run.qword(F0 + 0x7FF0), program.call_return);

    // s8: a qword store across the boundary of far pages 4 and 5. The preflight walks both pages
    // (A on both, no D), then the bytes go one by one and each page gets its D with its first
    // byte (case e).
    let out = step(&mut run, "s8 cross-page store", 1, 0);
    let byte = |address: u64| (address, 1);
    check(
        &out,
        "s8 cross-page store",
        &[
            (FAR_PT + 4 * 8, (FF + 0x4000) | 3 | A | D),
            (FAR_PT + 5 * 8, (FF + 0x5000) | 3 | A | D),
        ],
        &[
            w8(FAR_PT + 4 * 8),
            w8(FAR_PT + 5 * 8),
            w8(FAR_PT + 4 * 8),
            byte(FF + 0x4FFC),
            byte(FF + 0x4FFD),
            byte(FF + 0x4FFE),
            byte(FF + 0x4FFF),
            w8(FAR_PT + 5 * 8),
            byte(FF + 0x5000),
            byte(FF + 0x5001),
            byte(FF + 0x5002),
            byte(FF + 0x5003),
        ],
    );
    assert_eq!(
        (
            run.qword(FF + 0x4FF8) >> 32,
            run.qword(FF + 0x5000) & 0xFFFF_FFFF
        ),
        (RCX_VALUE & 0xFFFF_FFFF, RCX_VALUE >> 32),
        "the store is split little-endian between the two frames"
    );

    // s9: `MOV RBX, imm64` retires without touching the tables.
    let out = step(&mut run, "s9 MOV", 1, 0);
    check(&out, "s9 MOV", &[], &[]);

    // s10: a write-first access of a tree that no earlier access touched: A on all four levels,
    // then D on the leaf, then the operand (case b).
    let out = step(&mut run, "s10 write-first tree", 1, 0);
    check(
        &out,
        "s10 write-first tree",
        &[
            (PML4 + 24, PDPT3 | 3 | A),
            (PDPT3, PD3 | 3 | A),
            (PD3, PT3 | 3 | A),
            (PT3, WR_FRAME | 3 | A | D),
        ],
        &[
            w8(PML4 + 24),
            w8(PDPT3),
            w8(PD3),
            w8(PT3),
            w8(PT3),
            w8(WR_FRAME + 0x40),
        ],
    );
    assert_eq!(run.qword(WR_FRAME + 0x40), RCX_VALUE);

    // s11, s12: a dword store to the ECAM window through a 4 KiB leaf. A, then D before the device
    // is reached; the device drops the data, so no operand write is committed.
    let out = step(&mut run, "s11 MOV", 1, 0);
    check(&out, "s11 MOV", &[], &[]);
    let out = step(&mut run, "s12 ECAM store", 1, 0);
    check(
        &out,
        "s12 ECAM store",
        &[(FAR_PT + 6 * 8, crate::PCI_ECAM_DESCRIPTOR.base | 3 | A | D)],
        &[w8(FAR_PT + 6 * 8), w8(FAR_PT + 6 * 8)],
    );

    // s13, s14: write-first access of a 2 MiB page: A on PML4E, PDPTE and the PDE, D on the PDE.
    let out = step(&mut run, "s13 MOV", 1, 0);
    check(&out, "s13 MOV", &[], &[]);
    let out = step(&mut run, "s14 2 MiB write", 1, 0);
    check(
        &out,
        "s14 2 MiB write",
        &[
            (PML4 + 16, PDPT2 | 3 | A),
            (PDPT2, PD2 | 3 | A),
            (PD2, M0 | 0x83 | A | D),
        ],
        &[w8(PML4 + 16), w8(PDPT2), w8(PD2), w8(PD2), w8(M0 + 0x10)],
    );
    assert_eq!(run.qword(M0 + 0x10), RCX_VALUE);

    // s15, s16: read of another 2 MiB page: A on its PDE only (the others are set), no D.
    let out = step(&mut run, "s15 MOV", 1, 0);
    check(&out, "s15 MOV", &[], &[]);
    let out = step(&mut run, "s16 2 MiB read", 1, 0);
    check(
        &out,
        "s16 2 MiB read",
        &[(PD2 + 16, M2_FRAME | 0x83 | A)],
        &[w8(PD2 + 16)],
    );
    assert_eq!(run.state.gpr[Register::Rax as usize], M2_DATA);

    // s17, s18: RMW of a third 2 MiB page: A (read walk) and D (write walk) on its PDE.
    let out = step(&mut run, "s17 MOV", 1, 0);
    check(&out, "s17 MOV", &[], &[]);
    let out = step(&mut run, "s18 2 MiB RMW", 1, 0);
    check(
        &out,
        "s18 2 MiB RMW",
        &[(PD2 + 8, M1 | 0x83 | A | D)],
        &[w8(PD2 + 8), w8(PD2 + 8), w8(M1 + 0x20)],
    );
    assert_eq!(run.qword(M1 + 0x20), 0x3000 + RCX_VALUE);

    // s19: load RBX with the address under the nonpresent PTE.
    let out = step(&mut run, "s19 MOV", 1, 0);
    check(&out, "s19 MOV", &[], &[]);

    // s20: the load faults, nothing retires, the loop delivers the `#PF` through the IDT.
    // Nonpresent PTE (case d): A only on the predecessor (PDE of the subtree), the PTE itself is
    // unchanged. The delivery reads the IDT (A on its page), then writes the six slots of the
    // frame to the stack page (already A and D).
    assert_eq!(run.state.rip, FAULT_RIP);
    let before_cs = run.state.cs_selector;
    let before_ss = run.state.ss_selector;
    let out = step(&mut run, "s20 #PF nonpresent", 0, 1);
    check(
        &out,
        "s20 #PF nonpresent",
        &[
            (PD0 + 4 * 8, NP_PT | 3 | A),
            (PT0 + 8 * 8, (F0 + 0x8000) | 3 | A),
        ],
        &[
            w8(PD0 + 4 * 8),
            w8(PT0 + 8 * 8),
            w8(F0 + 0x7FE8),
            w8(F0 + 0x7FE0),
            w8(F0 + 0x7FD8),
            w8(F0 + 0x7FD0),
            w8(F0 + 0x7FC8),
            w8(F0 + 0x7FC0),
        ],
    );
    assert_eq!(run.tables[((NP_PT - P0) / 8) as usize], (F0 + 0xE000) | 2);
    assert_eq!(run.state.cr2, Some(NP_LINEAR + 0x18));
    assert_eq!(
        (run.state.rip, run.state.gpr[Register::Rsp as usize]),
        (HANDLER, 0x7FC0)
    );
    assert_eq!(
        run.qword(F0 + 0x7FC0),
        0,
        "error code: not present, read, supervisor"
    );
    assert_eq!(run.qword(F0 + 0x7FC8), FAULT_RIP);
    assert_eq!(Some(run.qword(F0 + 0x7FD0)), before_cs.map(u64::from));
    assert_eq!(run.qword(F0 + 0x7FE0), 0x7FF0);
    assert_eq!(Some(run.qword(F0 + 0x7FE8)), before_ss.map(u64::from));

    // s21: the handler's first instruction is fetched: A on the handler page. `ADD RSP, 8`.
    let out = step(&mut run, "s21 handler ADD RSP", 1, 0);
    check(
        &out,
        "s21 handler ADD RSP",
        &[(PT0 + 9 * 8, (F0 + 0x9000) | 3 | A)],
        &[w8(PT0 + 9 * 8)],
    );
    // s22, s23: MOV RDX, MOV RSI.
    for name in ["s22 handler MOV RDX", "s23 handler MOV RSI"] {
        let out = step(&mut run, name, 1, 0);
        check(&out, name, &[], &[]);
    }
    // s24: `MOV [RDX], RSI` stores through the window page (A, D on the window's PTE) into the
    // frame of the nonpresent PTE and makes it present, with A and D still clear.
    let out = step(&mut run, "s24 handler repair store", 1, 0);
    check(
        &out,
        "s24 handler repair store",
        &[
            (PT0 + 0xB * 8, NP_PT | 3 | A | D),
            (NP_PT, (F0 + 0xE000) | 3),
        ],
        &[w8(PT0 + 0xB * 8), w8(PT0 + 0xB * 8), w8(NP_PT)],
    );
    // s25: `IRETQ` reads the frame (stack page already A and D) and returns to the load.
    let out = step(&mut run, "s25 handler IRETQ", 1, 0);
    check(&out, "s25 handler IRETQ", &[], &[]);
    assert_eq!(
        (run.state.rip, run.state.gpr[Register::Rsp as usize]),
        (FAULT_RIP, 0x7FF0)
    );

    // s26: the retried load now walks the repaired PTE: A on it, no D.
    let out = step(&mut run, "s26 retry", 1, 0);
    check(
        &out,
        "s26 retry",
        &[(NP_PT, (F0 + 0xE000) | 3 | A)],
        &[w8(NP_PT)],
    );
    assert_eq!(run.state.gpr[Register::Rax as usize], NP_DATA);

    // s27: `JMP rel32` to the 2 MiB code page.
    let out = step(&mut run, "s27 JMP", 1, 0);
    check(&out, "s27 JMP", &[], &[]);

    // s28: fetch from a 2 MiB page: A on its PDE (the two upper levels are set), no D; `HLT`.
    let out = run.step();
    retired(&out, "s28 fetch 2 MiB + HLT", 1, 0);
    assert_eq!(
        out.report.exit,
        PagedCpl0SetExit::Run(RunExit::Halted),
        "{:?}",
        out.report
    );
    check(
        &out,
        "s28 fetch 2 MiB + HLT",
        &[(PD0 + 8, M_CODE | 0x83 | A)],
        &[w8(PD0 + 8)],
    );
    retired_total += 1;
    all_writes.extend(out.writes.iter().copied());
    assert_eq!(retired_total, 28);

    // The final tables, by literal: every entry the scenario touched, and every other entry as
    // the layout wrote it (the delta checks above already prove that nothing else moved).
    let entry = |run: &Run, address: u64| run.tables[((address - P0) / 8) as usize];
    for (address, value) in [
        (PML4, PDPT0 | 3 | A),
        (PML4 + 8, PDPT1 | 3 | A),
        (PML4 + 16, PDPT2 | 3 | A),
        (PML4 + 24, PDPT3 | 3 | A),
        (PD0 + 6 * 8, RO_PT | 1),
        (RO_PT, (F0 + 0xF000) | 3),
        (PT0 + 0xC * 8, (F0 + 0xC000) | 1),
        (FAR_PT + 7 * 8, (FF + 0x7000) | 3),
        (FAR_PT + 8 * 8, (FF + 0x8000) | 2),
        (FAR_PT + 9 * 8, (FF + 0x9000) | 3),
        (FAR_PT + 10 * 8, (FF + 0xA000) | 1),
    ] {
        assert_eq!(entry(&run, address), value, "final entry {address:#x}");
    }

    // The differential run: the same program on an identical tree whose present entries have A
    // (and D on a leaf) preset. A and D never change what the program computes, and with them
    // preset the only table-frame writes are the handler's own store and the retry's A.
    let mut preset = Run::new(&program.code, true, &main_registers(), true);
    let out = preset.run(100, false);
    assert_eq!(out.report.exit, PagedCpl0SetExit::Run(RunExit::Halted));
    assert_eq!(
        (
            out.report.instructions_executed,
            out.report.delivered_page_faults
        ),
        (28, 1)
    );
    assert_eq!(preset.state.gpr, run.state.gpr);
    assert_eq!(
        (preset.state.rip, preset.state.rflags),
        (run.state.rip, run.state.rflags)
    );
    let in_tables = |address: u64| (P0..TABLES_END).contains(&address);
    let table_writes: Vec<_> = out.writes.iter().filter(|w| in_tables(w.0)).collect();
    assert_eq!(
        table_writes,
        [&w8(NP_PT), &w8(NP_PT)],
        "preset: only the handler store and the retry's A reach the table frames"
    );
    let data = |writes: &[(u64, u64)]| -> Vec<(u64, u64)> {
        writes.iter().copied().filter(|w| !in_tables(w.0)).collect()
    };
    assert_eq!(
        data(&out.writes),
        data(&all_writes),
        "the data and stack writes are the same with and without preset A/D"
    );
    for (start, len) in [
        (F0, 0x10000usize),
        (FF, 0xB000),
        (WR_FRAME, 0x1000),
        (M0, 0x40),
        (M1, 0x40),
        (M2_FRAME, 0x40),
    ] {
        let mut left = vec![0u8; len];
        let mut right = vec![0u8; len];
        read(&mut run.boot.board, start, &mut left).unwrap();
        read(&mut preset.boot.board, start, &mut right).unwrap();
        assert!(left == right, "RAM at {start:#x} differs with preset A/D");
    }
}

fn protection_program() -> Vec<u8> {
    // NOP; MOV [RBX], RCX
    vec![0x90, 0x48, 0x89, 0x0B]
}

#[test]
fn a_d_acceptance_write_protection_fault_after_earlier_accesses_keeps_a_on_every_level_and_no_d() {
    let fetch_a = [
        (PML4, PDPT0 | 3 | A),
        (PDPT0, PD0 | 3 | A),
        (PD0, PT0 | 3 | A),
        (PT0 + 8, (F0 + 0x1000) | 3 | A),
    ];
    // The fault is a stop of the loop (no IDT here): the `#PF` record stays the report's.
    let fault = |linear: u64| {
        PagedCpl0SetExit::Run(RunExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(3),
            linear_address: Some(linear),
        }))
    };

    // (1) A read-only PTE (R/W = 0, CR0.WP = 1) under entries that earlier accesses already
    // marked accessed: the fault sets A on the denying PTE itself, never D.
    let mut run = Run::new(
        &protection_program(),
        false,
        &[(Register::Rbx, RO_PAGE + 0x20), (Register::Rcx, RCX_VALUE)],
        false,
    );
    let out = run.step();
    check(
        &out,
        "ro leaf: fetch",
        &fetch_a,
        &[w8(PML4), w8(PDPT0), w8(PD0), w8(PT0 + 8)],
    );
    let out = run.step();
    assert_eq!(out.report.exit, fault(RO_PAGE + 0x20), "{:?}", out.report);
    assert_eq!(out.report.instructions_executed, 0);
    check(
        &out,
        "ro leaf: protection fault",
        &[(PT0 + 0xC * 8, (F0 + 0xC000) | 1 | A)],
        &[w8(PT0 + 0xC * 8)],
    );
    assert_eq!(run.qword(F0 + 0xC020), 0, "the operand is not written");

    // (2) A writable PTE under a PDE with R/W = 0: the walk reaches the PTE before the rights
    // are checked, so A is set on the denying PDE and on the PTE below it, never D.
    let mut run = Run::new(
        &protection_program(),
        false,
        &[
            (Register::Rbx, RO_PDE_LINEAR + 0x20),
            (Register::Rcx, RCX_VALUE),
        ],
        false,
    );
    let out = run.step();
    check(
        &out,
        "ro pde: fetch",
        &fetch_a,
        &[w8(PML4), w8(PDPT0), w8(PD0), w8(PT0 + 8)],
    );
    let out = run.step();
    assert_eq!(
        out.report.exit,
        fault(RO_PDE_LINEAR + 0x20),
        "{:?}",
        out.report
    );
    check(
        &out,
        "ro pde: protection fault",
        &[(PD0 + 6 * 8, RO_PT | 1 | A), (RO_PT, (F0 + 0xF000) | 3 | A)],
        &[w8(PD0 + 6 * 8), w8(RO_PT)],
    );
    assert_eq!(run.qword(F0 + 0xF020), 0, "the operand is not written");
}

#[test]
fn a_d_acceptance_cross_page_store_faulting_on_the_second_page_leaves_no_d_and_no_byte() {
    let cross_store = [0x90, 0x48, 0x89, 0x0F]; // NOP; MOV [RDI], RCX

    // (1) The second page is nonpresent: the first page gets A (the preflight walks it), the
    // second one nothing (its PTE is the nonpresent entry), neither gets D, no byte is written.
    let mut run = Run::new(
        &cross_store,
        false,
        &[
            (Register::Rdi, FAR + 7 * 0x1000 + 0xFFC),
            (Register::Rcx, RCX_VALUE),
        ],
        false,
    );
    let out = run.step();
    assert_eq!(out.report.exit, stopped());
    let out = run.step();
    assert_eq!(
        out.report.exit,
        PagedCpl0SetExit::Run(RunExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(2),
            linear_address: Some(FAR + 8 * 0x1000),
        })),
        "{:?}",
        out.report
    );
    check(
        &out,
        "second page nonpresent",
        &[
            (PML4 + 8, PDPT1 | 3 | A),
            (PDPT1, PD1 | 3 | A),
            (PD1, FAR_PT | 3 | A),
            (FAR_PT + 7 * 8, (FF + 0x7000) | 3 | A),
        ],
        &[w8(PML4 + 8), w8(PDPT1), w8(PD1), w8(FAR_PT + 7 * 8)],
    );
    assert_eq!(
        run.tables[((FAR_PT + 8 * 8 - P0) / 8) as usize],
        (FF + 0x8000) | 2
    );
    assert_eq!(run.qword(FF + 0x7FF8), 0, "no byte on the first page");
    assert_eq!(run.qword(FF + 0x8000), 0);

    // (2) The second page is read-only (WP = 1): a protection fault there leaves A on the first
    // page and on every level of the second one (the denying PTE included), no D, no byte.
    let mut run = Run::new(
        &cross_store,
        false,
        &[
            (Register::Rdi, FAR + 9 * 0x1000 + 0xFFC),
            (Register::Rcx, RCX_VALUE),
        ],
        false,
    );
    let out = run.step();
    assert_eq!(out.report.exit, stopped());
    let out = run.step();
    assert_eq!(
        out.report.exit,
        PagedCpl0SetExit::Run(RunExit::Fault(od_isa::ArchitecturalFault {
            vector: 14,
            error_code: Some(3),
            linear_address: Some(FAR + 10 * 0x1000),
        })),
        "{:?}",
        out.report
    );
    check(
        &out,
        "second page read-only",
        &[
            (PML4 + 8, PDPT1 | 3 | A),
            (PDPT1, PD1 | 3 | A),
            (PD1, FAR_PT | 3 | A),
            (FAR_PT + 9 * 8, (FF + 0x9000) | 3 | A),
            (FAR_PT + 10 * 8, (FF + 0xA000) | 1 | A),
        ],
        &[
            w8(PML4 + 8),
            w8(PDPT1),
            w8(PD1),
            w8(FAR_PT + 9 * 8),
            w8(FAR_PT + 10 * 8),
        ],
    );
    assert_eq!(run.qword(FF + 0x9FF8), 0, "no byte on the first page");
    assert_eq!(run.qword(FF + 0xA000), 0);
}
