//! PERF-10: the decoded-instruction cache against the byte-per-fetch decoder.
//!
//! Every case runs the same scenario twice over fresh boards, once with the cache and once
//! without, and requires the same CPU state after every quantum, the same report of every quantum
//! (exit, fault record, CR2, error code, context changes), the same committed RAM writes in order and
//! the same bytes of the frames. The cases that must not be served from the cache also say what the
//! cache did (hits, inserts), because a case in which the cache never hit proves nothing.

use super::*;
use crate::decoded_instruction_cache::test_support::insert_probe;
use crate::decoded_instruction_cache::{DecodedInstructionCache, DecodedInstructionCacheStats};
use crate::fetch_cache_sweep::SweepConfig;
use crate::fetch_translation_cache::{FetchCacheInvalidation, FetchCacheTag};
use crate::paged_memory_adapter::run_cpl0_set_paged_2m_ram_observed_with_sweep;
use od_isa::memory::AccessKind;

/// Linear page `k` of the test window (PT index `k` of the tree).
const fn va(page: u64) -> u64 {
    SPLIT_VA + page * 0x1000
}

const FRAME_A: u64 = SPLIT_PAGE0;
const FRAME_B: u64 = SPLIT_PAGE1;
const FRAME_C: u64 = SPLIT_PAGE0 + 0x2000;
const XD: u64 = 1 << 63;

fn pte(frame: u64) -> u64 {
    frame | A_PRESENT_WRITABLE
}

/// `E9 rel32` placed so that the byte after it is at `next`.
fn jmp_rel32(next: u64, target: u64) -> Vec<u8> {
    let mut code = vec![0xE9];
    code.extend((target.wrapping_sub(next) as i32).to_le_bytes());
    code
}

/// `MOV r32, imm32`.
fn mov32(register: u8, value: u32) -> Vec<u8> {
    let mut code = vec![0xB8 + register];
    code.extend(value.to_le_bytes());
    code
}

const DEC_ECX: [u8; 2] = [0xFF, 0xC9];
const DEC_EDI: [u8; 2] = [0xFF, 0xCF];
const HLT: u8 = 0xF4;

enum Step<'a> {
    /// One call of the set loop with this budget (a fresh fetch cache; the decoded-instruction
    /// cache of the scenario lives on).
    Run(u64),
    /// The host does something between two calls.
    Host(&'a dyn Fn(&mut LinuxDirectBoot, &mut LongModeState)),
}

struct Observed {
    state: LongModeState,
    reports: Vec<PagedCpl0SetReport>,
    writes: Vec<(u64, u64)>,
    memory: Vec<u8>,
    /// The counters of the decoded-instruction cache after each `Run` (all zero without it).
    stats: Vec<DecodedInstructionCacheStats>,
}

fn run_scenario(
    build: &dyn Fn(&mut LinuxDirectBoot) -> LongModeState,
    steps: &[Step<'_>],
    set: &Cpl0ProfileSet<'_>,
    cached: bool,
) -> Observed {
    let (mut boot, _) = boot_with_entry(&[HLT]);
    let mut state = build(&mut boot);
    let mut efer = boot.board.efer();
    let decoded = DecodedInstructionCache::new(1 << 10);
    let (mut reports, mut writes, mut stats) = (Vec::new(), Vec::new(), Vec::new());
    for step in steps {
        match step {
            Step::Host(act) => act(&mut boot, &mut state),
            Step::Run(budget) => {
                let cr3 = state.cr3.expect("the case runs with a CPU-owned CR3");
                let mut report = run_cpl0_set_paged_2m_ram_observed_with_sweep(
                    &mut state,
                    boot.board.bus_mut(),
                    cr3,
                    efer,
                    long64(),
                    *budget,
                    INTEGER_FLAGS,
                    set,
                    None,
                    || false,
                    |range| writes.push((range.start().get(), range.len())),
                    true,
                    Some(FetchCacheInvalidation::WriteWatch),
                    None,
                    None,
                    None,
                    None,
                    FetchCacheTag::Page4K,
                    cached.then_some(&decoded),
                    crate::data_translation_cache::DataCacheUse::Off,
                    false,
                    None,
                    None,
                );
                efer = report.efer;
                state.cr3 = Some(report.cr3);
                // Measurement, not behaviour: the walk counters and the counters of the fetch
                // cache (the decoded-instruction cache serves the bytes of a hit).
                report.walk_counts = Default::default();
                report.fetch_cache_stats = None;
                reports.push(report);
                stats.push(decoded.stats());
            }
        }
    }
    let mut memory = Vec::new();
    for (start, len) in [
        (TREE_PML4, 0x5000usize),
        (TREE2_PML4, 0x4000),
        (SPLIT_PAGE0, 0x6000),
        (SPLIT_PAGE1, 0x1000),
    ] {
        let mut bytes = vec![0u8; len];
        read(&mut boot.board, start, &mut bytes).expect("frames are RAM");
        memory.extend(bytes);
    }
    Observed {
        state,
        reports,
        writes,
        memory,
        stats,
    }
}

/// The differential check. Returns the observation of the run WITH the cache.
fn assert_cache_is_invisible(
    build: &dyn Fn(&mut LinuxDirectBoot) -> LongModeState,
    steps: &[Step<'_>],
) -> Observed {
    let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
    let set = switch_set(&cli, &halt);
    let plain = run_scenario(build, steps, &set, false);
    let cached = run_scenario(build, steps, &set, true);
    assert_eq!(
        plain.stats.last().unwrap().hits,
        0,
        "the plain run has no cache"
    );
    assert_eq!(cached.reports, plain.reports);
    assert_eq!(cached.state, plain.state);
    assert_eq!(cached.writes, plain.writes);
    assert!(cached.memory == plain.memory, "frame bytes differ");
    cached
}

fn halted(report: &PagedCpl0SetReport) -> bool {
    report.exit == PagedCpl0SetExit::Run(RunExit::Halted)
}

fn lay_code(boot: &mut LinuxDirectBoot, frame: u64, offset: u64, code: &[u8]) {
    write_phys(boot, frame + offset, code);
}

// ---- (a) self-modifying code on the page of the instruction, in the same call ----

/// Two passes over a loop whose first instruction the guest patches between them. The patch is a
/// store into the immediate of an instruction that has been executed (and decoded) before, on the
/// page that is being executed.
#[test]
fn self_modifying_code_on_the_same_page_in_the_same_call_runs_the_new_bytes() {
    let build = |boot: &mut LinuxDirectBoot| {
        write_tree(boot, TREE_PML4, &[(0, pte(FRAME_A))], None);
        let base = va(0) + 0x100;
        let patch = base + 0x1B;
        let mut code = mov_imm32(3, patch as u32); // 0x00 MOV RBX, patch
        code.extend(mov_imm32(0, 0x5555)); // 0x07 MOV RAX, 0x5555
        code.extend(mov32(7, 2)); // 0x0E MOV EDI, 2
        code.extend(mov32(1, 3)); // 0x13 P: MOV ECX, 3
        code.extend(mov_imm32(2, 0x1111)); // 0x18 L: MOV RDX, 0x1111 (the immediate is at 0x1B)
        code.extend(DEC_ECX); // 0x1F
        code.extend([0x75, 0xF5]); // 0x21 JNZ L
        code.extend([0x89, 0x03]); // 0x23 MOV [RBX], EAX
        code.extend(DEC_EDI); // 0x25
        code.extend([0x75, 0xEA]); // 0x27 JNZ P
        code.push(HLT); // 0x29
        assert_eq!(code.len(), 0x2A);
        lay_code(boot, FRAME_A, 0x100, &code);
        tree_state(boot, TREE_PML4, base)
    };
    let observed = assert_cache_is_invisible(&build, &[Step::Run(200)]);
    assert!(halted(&observed.reports[0]), "{:?}", observed.reports[0]);
    // The patched immediate ran; the old one (0x1111) did too, in the first pass.
    assert_eq!(observed.state.gpr[Register::Rdx as usize], 0x5555);
    let stats = observed.stats[0];
    assert!(stats.hits >= 4, "the loop must have been served: {stats:?}");
    assert!(stats.inserts >= 2, "{stats:?}");
}

// ---- (b) a device write into a code page between two calls ----

/// Two pages that jump to each other. Page A: `MOV RDX,3; MOV RSI,0x1111; JMP B`; page B:
/// `MOV RAX,5; MOV RCX,6; JMP A`. Six instructions per cycle, so 30 end a call on the first of A.
fn two_page_loop(boot: &mut LinuxDirectBoot) -> LongModeState {
    write_tree(
        boot,
        TREE_PML4,
        &[(0, pte(FRAME_A)), (1, pte(FRAME_B)), (2, pte(FRAME_C))],
        None,
    );
    let mut a = mov_imm32(2, 3);
    a.extend(mov_imm32(6, 0x1111)); // the immediate is at 0x10A
    a.extend(jmp_rel32(va(0) + 0x113, va(1) + 0x100));
    lay_code(boot, FRAME_A, 0x100, &a);
    let mut b = mov_imm32(0, 5);
    b.extend(mov_imm32(1, 6));
    b.extend(jmp_rel32(va(1) + 0x113, va(0) + 0x100));
    lay_code(boot, FRAME_B, 0x100, &b);
    tree_state(boot, TREE_PML4, va(0) + 0x100)
}

#[test]
fn a_device_write_into_a_code_page_between_calls_runs_the_new_bytes_and_keeps_the_other_page() {
    let device_write = |boot: &mut LinuxDirectBoot, _: &mut LongModeState| {
        write_phys(boot, FRAME_A + 0x10A, &0x2222u32.to_le_bytes());
    };
    let observed = assert_cache_is_invisible(
        &two_page_loop,
        &[Step::Run(30), Step::Host(&device_write), Step::Run(30)],
    );
    assert_eq!(observed.state.gpr[Register::Rsi as usize], 0x2222);
    let (first, second) = (observed.stats[0], observed.stats[1]);
    assert!(first.hits >= 8, "{first:?}");
    // Page A was written: its three instructions are decoded and stored again (the first of
    // them in the second cycle: the first instruction of a page in a call is never stored, the
    // fetch cache has not got the translation of its page yet). Page B was not: its entries
    // still serve, nothing of it is stored again.
    assert_eq!(second.inserts - first.inserts, 3, "{first:?} {second:?}");
    assert!(second.hits - first.hits >= 8, "{first:?} {second:?}");
}

#[test]
fn a_device_write_into_another_page_keeps_every_entry() {
    let control = |boot: &mut LinuxDirectBoot, _: &mut LongModeState| {
        write_phys(boot, FRAME_C + 0x10, &0xDEAD_BEEFu32.to_le_bytes());
    };
    let observed = assert_cache_is_invisible(
        &two_page_loop,
        &[Step::Run(30), Step::Host(&control), Step::Run(30)],
    );
    assert_eq!(observed.state.gpr[Register::Rsi as usize], 0x1111);
    let (first, second) = (observed.stats[0], observed.stats[1]);
    assert_eq!(second.inserts, first.inserts, "nothing is decoded again");
    assert!(second.hits - first.hits >= 16, "{first:?} {second:?}");
}

// ---- (c) an instruction across the page boundary ----

#[test]
fn an_instruction_across_a_page_boundary_is_never_stored_and_runs_right_every_time() {
    let build = |boot: &mut LinuxDirectBoot| {
        write_tree(
            boot,
            TREE_PML4,
            &[(0, pte(FRAME_A)), (1, pte(FRAME_B))],
            None,
        );
        // MOV RDX, 0x1234 at 0xFFC..0x1002: four bytes on page A, three on page B; then, on page
        // B, a JMP back to it.
        let crossing = mov_imm32(2, 0x1234);
        assert_eq!(crossing.len(), 7);
        lay_code(boot, FRAME_A, 0xFFC, &crossing[..4]);
        lay_code(boot, FRAME_B, 0, &crossing[4..]);
        lay_code(boot, FRAME_B, 3, &jmp_rel32(va(1) + 8, va(0) + 0xFFC));
        tree_state(boot, TREE_PML4, va(0) + 0xFFC)
    };
    let observed = assert_cache_is_invisible(&build, &[Step::Run(40)]);
    assert_eq!(observed.state.gpr[Register::Rdx as usize], 0x1234);
    let stats = observed.stats[0];
    // Only the JMP lies wholly on one page: it is stored once and served after that. The
    // crossing instruction (20 times) is decoded from its bytes every time.
    assert_eq!(stats.inserts, 1, "{stats:?}");
    assert!(stats.hits >= 15, "{stats:?}");
}

// ---- (d) one frame at two linear addresses ----

#[test]
fn one_frame_at_two_linear_addresses_gives_each_address_its_own_result() {
    let build = |boot: &mut LinuxDirectBoot| {
        // Frame A is the code of both linear pages 0 and 3; page 2 is the caller.
        write_tree(
            boot,
            TREE_PML4,
            &[(0, pte(FRAME_A)), (2, pte(FRAME_C)), (3, pte(FRAME_A))],
            None,
        );
        // LEA RAX, [RIP+0]; JMP RCX
        let mut shared = vec![0x48, 0x8D, 0x05, 0, 0, 0, 0];
        shared.extend([0xFF, 0xE1]);
        lay_code(boot, FRAME_A, 0x100, &shared);
        let caller = va(2) + 0x100;
        let mut code = mov32(7, 4); // 0x00 MOV EDI, 4
        code.extend(mov_imm32(3, (va(0) + 0x100) as u32)); // 0x05 L: MOV RBX, alias 0
        code.extend(mov_imm32(1, (caller + 0x15) as u32)); // 0x0C MOV RCX, return 1
        code.extend([0xFF, 0xE3]); // 0x13 JMP RBX
        code.extend([0x48, 0x89, 0xC2]); // 0x15 MOV RDX, RAX
        code.extend(mov_imm32(3, (va(3) + 0x100) as u32)); // 0x18 MOV RBX, alias 3
        code.extend(mov_imm32(1, (caller + 0x28) as u32)); // 0x1F MOV RCX, return 2
        code.extend([0xFF, 0xE3]); // 0x26 JMP RBX
        code.extend([0x48, 0x89, 0xC6]); // 0x28 MOV RSI, RAX
        code.extend(DEC_EDI); // 0x2B
        code.extend([0x75, 0xD6]); // 0x2D JNZ L
        code.push(HLT); // 0x2F
        assert_eq!(code.len(), 0x30);
        lay_code(boot, FRAME_C, 0x100, &code);
        tree_state(boot, TREE_PML4, caller)
    };
    let observed = assert_cache_is_invisible(&build, &[Step::Run(200)]);
    assert!(halted(&observed.reports[0]), "{:?}", observed.reports[0]);
    let (first, second) = (
        observed.state.gpr[Register::Rdx as usize],
        observed.state.gpr[Register::Rsi as usize],
    );
    // The result of `LEA RAX,[RIP+0]` is the next RIP of the alias it ran at.
    assert_eq!(first, va(0) + 0x107);
    assert_eq!(second, va(3) + 0x107);
    assert_eq!(second - first, va(3) - va(0));
    // The counts follow from the loop. The first instruction a call fetches from a page is never
    // stored (the fetch cache has no translation of the page yet). Four passes; per pass the
    // caller runs 10 instructions after its `MOV EDI,4`, and each alias runs `LEA` and `JMP RCX`.
    // The slot of an entry is chosen by the physical place (region and offset), not by the RIP,
    // so the two aliases of one frame share their slots and every visit of an alias replaces the
    // entry the other one left: no alias instruction is ever served, and none is ever served
    // with the other alias's result.
    //  - Inserts: pass 1: the 3 + 4 + 3 caller instructions and the two `JMP RCX` (12); passes 2,
    //    3 and 4: the four alias instructions each (12); the `HLT` after the loop: 25.
    //  - Hits: the 10 caller instructions in each of passes 2, 3 and 4: 30.
    let stats = observed.stats[0];
    assert_eq!((stats.inserts, stats.hits), (25, 30), "{stats:?}");
}

// ---- (e) CR3 changes ----

const IMM_OFFSET: u64 = 0x05 + 3;

/// The same code at the same linear page in two trees, over two frames that differ in the
/// immediate of the loop and in one byte after it: tree 1 -> frame A (the guest switches to tree
/// 2 at the end of the loop), tree 2 -> frame C (the guest halts at the same place).
fn cr3_case(boot: &mut LinuxDirectBoot) -> LongModeState {
    write_tree(boot, TREE_PML4, &[(0, pte(FRAME_A))], None);
    write_tree(boot, TREE2_PML4, &[(0, pte(FRAME_C))], None);
    let build = |immediate: u32, tail_is_halt: bool| {
        let mut code = mov32(1, 5); // 0x00 MOV ECX, 5
        code.extend(mov_imm32(2, immediate)); // 0x05 L: MOV RDX, imm
        code.extend(DEC_ECX); // 0x0C
        code.extend([0x75, 0xF5]); // 0x0E JNZ L
        if tail_is_halt {
            code.push(HLT); // 0x10
            code.extend([0u8; 6]);
        } else {
            code.extend(mov_imm32(0, TREE2_PML4 as u32)); // 0x10 MOV RAX, tree 2
        }
        code.extend([0x0F, 0x22, 0xD8]); // 0x17 MOV CR3, RAX
        code.extend([0xEB, 0xE4]); // 0x1A JMP 0x00
        assert_eq!(code.len(), 0x1C);
        code
    };
    assert_eq!(IMM_OFFSET, 0x08);
    lay_code(boot, FRAME_A, 0x100, &build(0x1111, false));
    lay_code(boot, FRAME_C, 0x100, &build(0x2222, true));
    tree_state(boot, TREE_PML4, va(0) + 0x100)
}

#[test]
fn a_mov_cr3_of_the_guest_makes_the_same_rip_run_the_code_of_the_new_tree() {
    let observed = assert_cache_is_invisible(&cr3_case, &[Step::Run(300)]);
    assert!(halted(&observed.reports[0]), "{:?}", observed.reports[0]);
    assert_eq!(observed.state.cr3, Some(TREE2_PML4));
    assert_eq!(observed.state.gpr[Register::Rdx as usize], 0x2222);
    assert!(observed.stats[0].hits >= 6, "{:?}", observed.stats[0]);
}

#[test]
fn a_cr3_change_of_the_host_between_calls_makes_the_same_rip_run_the_code_of_the_new_tree() {
    let switch = |_: &mut LinuxDirectBoot, state: &mut LongModeState| {
        state.cr3 = Some(TREE2_PML4);
    };
    let observed = assert_cache_is_invisible(
        &cr3_case,
        &[Step::Run(12), Step::Host(&switch), Step::Run(300)],
    );
    assert!(halted(&observed.reports[1]), "{:?}", observed.reports[1]);
    assert_eq!(observed.state.gpr[Register::Rdx as usize], 0x2222);
    assert!(observed.stats[0].hits >= 2, "{:?}", observed.stats[0]);
}

// ---- (f) rights that change after hits ----

fn assert_fetch_fault(report: &PagedCpl0SetReport, address: u64) {
    let record = report
        .page_fault
        .as_ref()
        .expect("a page fault is recorded");
    let fault = record.fault();
    assert_eq!(fault.vector, 14, "{report:?}");
    assert_eq!(fault.linear_address, Some(address), "{report:?}");
    // The instruction-fetch bit (bit 4) is set when NX is enabled: the fault is of a fetch.
    assert_eq!(record.access(), AccessKind::Fetch, "{report:?}");
}

#[test]
fn xd_set_in_the_pte_of_the_running_page_without_invlpg_faults_as_without_the_cache() {
    let build = |boot: &mut LinuxDirectBoot| {
        write_tree(
            boot,
            TREE_PML4,
            &[(0, pte(FRAME_A))],
            Some(TREE_PML4 + 0x3000),
        );
        let base = va(0) + 0x100;
        let mut code = mov_imm32(3, WINDOW_VA as u32); // 0x00 MOV RBX, window of the PT
        code.extend([0x48, 0xB8]); // 0x07 MOV RAX, PTE | XD
        code.extend((pte(FRAME_A) | XD).to_le_bytes());
        code.extend(mov32(1, 4)); // 0x11 MOV ECX, 4
        code.extend(DEC_ECX); // 0x16 L: DEC ECX
        code.extend([0x75, 0xFC]); // 0x18 JNZ L
        code.extend([0x48, 0x89, 0x03]); // 0x1A MOV [RBX], RAX
        let faulting = base + code.len() as u64;
        code.extend(mov_imm32(6, 7)); // 0x1D MOV RSI, 7 (the next fetch faults)
        code.push(HLT);
        lay_code(boot, FRAME_A, 0x100, &code);
        let mut state = tree_state(boot, TREE_PML4, base);
        state.gpr[Register::Rax as usize] = faulting;
        state
    };
    let observed = assert_cache_is_invisible(&build, &[Step::Run(100)]);
    let report = &observed.reports[0];
    // MOV RAX,... overwrote the scratch above; the faulting address is the instruction after the store.
    assert_fetch_fault(report, va(0) + 0x100 + 0x1D);
    assert_ne!(
        observed.state.gpr[Register::Rsi as usize],
        7,
        "MOV RSI did not run"
    );
    assert!(observed.stats[0].hits >= 2, "{:?}", observed.stats[0]);
}

#[test]
fn a_jump_to_a_page_that_is_not_executable_faults_as_without_the_cache() {
    let build = |boot: &mut LinuxDirectBoot| {
        write_tree(
            boot,
            TREE_PML4,
            &[(0, pte(FRAME_A)), (1, pte(FRAME_B) | XD)],
            None,
        );
        let mut code = mov32(1, 5); // 0x00 MOV ECX, 5
        code.extend(DEC_ECX); // 0x05 L: DEC ECX
        code.extend([0x75, 0xFC]); // 0x07 JNZ L
        code.extend(jmp_rel32(va(0) + 0x100 + 0x0E, va(1) + 0x100)); // 0x09 JMP page 1
        lay_code(boot, FRAME_A, 0x100, &code);
        lay_code(boot, FRAME_B, 0x100, &[HLT]);
        tree_state(boot, TREE_PML4, va(0) + 0x100)
    };
    let observed = assert_cache_is_invisible(&build, &[Step::Run(100)]);
    assert_fetch_fault(&observed.reports[0], va(1) + 0x100);
    assert!(observed.stats[0].hits >= 3, "{:?}", observed.stats[0]);
}

// ---- (g) the next page is missing ----

#[test]
fn an_instruction_at_the_end_of_a_page_with_no_next_page_faults_as_without_the_cache() {
    let build = |boot: &mut LinuxDirectBoot| {
        // Page 1 is not present.
        write_tree(boot, TREE_PML4, &[(0, pte(FRAME_A))], None);
        let mut code = mov32(1, 4); // 0xFD0 MOV ECX, 4
        code.extend(DEC_ECX); // 0xFD5 L: DEC ECX
        code.extend([0x75, 0xFC]); // 0xFD7 JNZ L
        code.extend(jmp_rel32(va(0) + 0xFDE, va(0) + 0xFFC)); // 0xFD9 JMP 0xFFC
        assert_eq!(code.len(), 0xE);
        lay_code(boot, FRAME_A, 0xFD0, &code);
        // MOV RDX, imm32 from 0xFFC: three of its bytes would be on the missing page.
        lay_code(boot, FRAME_A, 0xFFC, &mov_imm32(2, 0x1234)[..4]);
        tree_state(boot, TREE_PML4, va(0) + 0xFD0)
    };
    let observed = assert_cache_is_invisible(&build, &[Step::Run(100)]);
    // The first byte on the missing page, as without the cache; nothing is stored for the
    // instruction (the stored ones are DEC, JNZ and the JMP).
    assert_fetch_fault(&observed.reports[0], va(1));
    assert_eq!(observed.reports[0].instructions_executed, 1 + 4 * 2 + 1);
    let stats = observed.stats[0];
    assert_eq!(stats.inserts, 3, "{stats:?}");
    assert!(stats.hits >= 4, "{stats:?}");
    assert_eq!(observed.state.gpr[Register::Rdx as usize], 0);
}

/// The one look-ahead of the decoder: after a bare `9B` it fetches two more bytes to tell the
/// spelling `9B D9 30` / `9B DD 30` from FWAIT. It is held inside the 4 KiB page of the `9B`: with
/// `9B` on the last byte of a page no byte of the next page is fetched (page 1 is missing here, and
/// no page fault comes). The set profile does not run a bare FWAIT, so the run ends there as
/// unsupported, the same with and without the cache. No other
/// encoding of the decoder reads past the bytes it uses, so a page-crossing instruction (cases c
/// and g) is the only way to leave the page of the RIP.
#[test]
fn fwait_on_the_last_byte_of_a_page_does_not_look_ahead_to_the_missing_page() {
    let build = |boot: &mut LinuxDirectBoot| {
        write_tree(boot, TREE_PML4, &[(0, pte(FRAME_A))], None);
        let mut code = mov32(1, 4); // 0xFF0 MOV ECX, 4
        code.extend(DEC_ECX); // 0xFF5 L: DEC ECX
        code.extend([0x75, 0xFC]); // 0xFF7 JNZ L
        code.extend([0xEB, 0x04]); // 0xFF9 JMP 0xFFF
        assert_eq!(code.len(), 0xB);
        lay_code(boot, FRAME_A, 0xFF0, &code);
        lay_code(boot, FRAME_A, 0xFFF, &[0x9B]); // FWAIT, the last byte of the page
        tree_state(boot, TREE_PML4, va(0) + 0xFF0)
    };
    let observed = assert_cache_is_invisible(&build, &[Step::Run(100)]);
    let report = &observed.reports[0];
    assert_eq!(
        report.exit,
        PagedCpl0SetExit::Run(RunExit::Unsupported { opcode: 0x9B }),
        "{report:?}"
    );
    assert!(report.page_fault.is_none(), "{report:?}");
    assert_eq!(report.instructions_executed, 1 + 4 * 2 + 1);
    let stats = observed.stats[0];
    // Stored: DEC, JNZ, the JMP (the MOV is the first of the page in the call) and the FWAIT
    // itself: the decoder accepts a bare `9B` and the set refuses it when it runs. Had the look-ahead
    // fetched a byte of page 1, the decode would have been kept out of the cache. Served: DEC and
    // JNZ three times each.
    assert_eq!((stats.inserts, stats.hits), (4, 6), "{stats:?}");
}

// ---- the board: CPL3 on a supervisor page, and the runs that must not use the cache ----

/// `MOV ECX, 5; L: DEC ECX; JNZ L`: a short loop of CPL0 code at the entry of the board.
const KERNEL_LOOP: [u8; 9] = [0xB9, 5, 0, 0, 0, 0xFF, 0xC9, 0x75, 0xFC];

#[test]
fn cpl3_on_a_supervisor_page_faults_as_without_the_cache_after_the_kernel_code_was_served() {
    let run = |fetch_cache: bool| {
        // The kernel loop, then `IRETQ` to user code on a supervisor page (`user_page = false`).
        let (mut boot, file) =
            user_mode_board_with(&KERNEL_LOOP, &[0xB8, 0x53, 0, 0, 0, 0xF4], false, 0x202);
        let members = LinuxCpl0SetMembers {
            fetch_cache,
            ..LinuxCpl0SetMembers::minimal()
        };
        let record = run_linux_direct_board_set_user_mode(
            &mut boot,
            &input(&file, &[], BLOB, board_hash()),
            64,
            members,
        )
        .expect("runs");
        (record, boot.board.decoded_instruction_cache().stats())
    };
    let (plain, plain_stats) = run(false);
    let (cached, cached_stats) = run(true);
    assert_eq!(plain_stats, DecodedInstructionCacheStats::default());
    assert!(cached_stats.hits >= 3, "{cached_stats:?}");
    // `LinuxStopRecord` has no `PartialEq`: its Debug text carries every field.
    assert_eq!(format!("{cached:?}"), format!("{plain:?}"));
    assert_eq!(cached.cpl, 3, "{cached:?}");
    assert_eq!(
        cached.cpl3_retired, 0,
        "the user page is not fetchable: {cached:?}"
    );
    assert!(cached.exit_kind.contains("fault"), "{cached:?}");
}

#[test]
fn the_board_cache_serves_an_ordinary_run_and_not_a_diagnostic_one_and_reset_empties_it() {
    let mut code = KERNEL_LOOP.to_vec();
    code.push(0xF4);
    let run = |fetch_cache: bool, report: bool| {
        let (mut boot, file) = boot_with_entry(&code);
        if report {
            boot.board.enable_fetch_cache_report();
        }
        let members = LinuxCpl0SetMembers {
            fetch_cache,
            ..halting_default()
        };
        let record = run_set_facade(&mut boot, &file, 100, members);
        (record, boot)
    };
    let (plain, plain_boot) = run(false, false);
    let (cached, cached_boot) = run(true, false);
    let (reported, reported_boot) = run(true, true);
    assert_eq!(plain.exit_kind, "halted");
    assert_eq!(format!("{cached:?}"), format!("{plain:?}"));
    assert_eq!(format!("{reported:?}"), format!("{plain:?}"));
    // No fetch cache, no decoded-instruction cache; the fetch cache report is a diagnostic.
    assert_eq!(
        plain_boot.board.decoded_instruction_cache().stats(),
        DecodedInstructionCacheStats::default()
    );
    assert_eq!(
        reported_boot.board.decoded_instruction_cache().stats(),
        DecodedInstructionCacheStats::default()
    );
    let stats = cached_boot.board.decoded_instruction_cache().stats();
    assert!(stats.hits >= 3 && stats.inserts >= 2, "{stats:?}");
    // The reset empties the table: a key that was in it is not served after it (the counter of
    // resets alone would rise on any call).
    let cache = cached_boot.board.decoded_instruction_cache();
    assert!(cache.entries() >= 2, "the run stored entries");
    let (rip, ring0, probe, nop) = insert_probe(cache);
    assert_eq!(cache.lookup(rip, ring0, probe), Some(nop));
    cached_boot.board.reset_decoded_instruction_cache();
    let cache = cached_boot.board.decoded_instruction_cache();
    assert_eq!(cache.stats().resets, 1);
    assert_eq!(cache.entries(), 0, "the table is empty after the reset");
    assert_eq!(cache.lookup(rip, ring0, probe), None, "the probe is gone");
}

/// A sweep, an execution observation (the instruction path trace is a part of it) and the fetch
/// cache report keep the byte-per-fetch decoding: the cache is not asked and not filled. The
/// control run, with none of them, is served, so the zero counters mean something.
#[test]
fn the_board_cache_is_not_used_with_a_sweep_execution_observations_or_a_trace() {
    #[derive(Clone, Copy, Debug)]
    enum Diagnostic {
        Control,
        Sweep,
        Observations,
        Trace,
        SweepAndObservations,
    }
    let mut code = KERNEL_LOOP.to_vec();
    code.push(0xF4);
    let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
    let run = |diagnostic: Diagnostic| {
        let (mut boot, _) = boot_with_entry(&code);
        let sweep = matches!(
            diagnostic,
            Diagnostic::Sweep | Diagnostic::SweepAndObservations
        );
        if sweep {
            boot.board
                .enable_fetch_cache_sweep(vec![SweepConfig::REFERENCE]);
        }
        let observations = !matches!(diagnostic, Diagnostic::Control | Diagnostic::Sweep);
        let trace = matches!(diagnostic, Diagnostic::Trace);
        let report = boot
            .board
            .run_linux_boot_cpl0_set_with_post_iret_index(
                100,
                &switch_set(&cli, &halt),
                false,
                true,
                false,
                observations,
                None,
                None,
                1,
                None,
                None,
                None,
                trace,
                false,
                false,
                false,
            )
            .unwrap_or_else(|failure| panic!("{diagnostic:?}: {failure:?}"));
        // The loop ran to its HLT: MOV, five times DEC and JNZ, HLT.
        assert_eq!(report.run.retired, 1 + 5 * 2 + 1, "{diagnostic:?}");
        boot.board.decoded_instruction_cache().stats()
    };
    let control = run(Diagnostic::Control);
    assert!(control.hits >= 3 && control.inserts >= 2, "{control:?}");
    for diagnostic in [
        Diagnostic::Sweep,
        Diagnostic::Observations,
        Diagnostic::Trace,
        Diagnostic::SweepAndObservations,
    ] {
        assert_eq!(
            run(diagnostic),
            DecodedInstructionCacheStats::default(),
            "{diagnostic:?}"
        );
    }
}
