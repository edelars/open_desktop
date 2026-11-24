//! PERF-31 (ADR-0018, variant (a), prototype): the predecoded block cache against the run without
//! it.
//!
//! Every case runs the same scenario twice over fresh boards through batched calls of the set with
//! the fast path and the decoded-instruction cache, once with the block cache and once without, and
//! requires the same CPU state, the same report of every call (exit, fault record, context changes),
//! the same committed RAM writes in order and the same bytes of the frames. The cases also say what
//! the block cache did, because a case in which no instruction ran from a block proves nothing.

use super::*;
use crate::decoded_block_cache::{DecodedBlockCache, DecodedBlockCacheStats};
use crate::decoded_instruction_cache::DecodedInstructionCache;
use crate::fetch_translation_cache::{FetchCacheInvalidation, FetchCacheTag};
use crate::paged_memory_adapter::run_cpl0_set_paged_2m_ram_observed_with_sweep;

const fn va(page: u64) -> u64 {
    SPLIT_VA + page * 0x1000
}

const FRAME_A: u64 = SPLIT_PAGE0;
const FRAME_B: u64 = SPLIT_PAGE1;
const FRAME_C: u64 = SPLIT_PAGE0 + 0x2000;
const HLT: u8 = 0xF4;

fn pte(frame: u64) -> u64 {
    frame | A_PRESENT_WRITABLE
}

/// `MOV r32, imm32`.
fn mov32(register: u8, value: u32) -> Vec<u8> {
    let mut code = vec![0xB8 + register];
    code.extend(value.to_le_bytes());
    code
}

fn lay_code(boot: &mut LinuxDirectBoot, frame: u64, offset: u64, code: &[u8]) {
    write_phys(boot, frame + offset, code);
}

enum Step<'a> {
    /// One batched call of the set loop with this budget.
    Run(u64),
    /// The host does something between two calls.
    Host(&'a dyn Fn(&mut LinuxDirectBoot, &mut LongModeState)),
}

/// How the block cache of a scenario behaves.
#[derive(Clone, Copy, PartialEq)]
enum Blocks {
    Off,
    On,
    /// The mutation: the invalidation is ignored (page generations, fetch cache drops).
    WithoutInvalidation,
}

struct Observed {
    state: LongModeState,
    reports: Vec<PagedCpl0SetReport>,
    writes: Vec<(u64, u64)>,
    memory: Vec<u8>,
    stats: DecodedBlockCacheStats,
}

fn run_scenario(
    build: &dyn Fn(&mut LinuxDirectBoot) -> LongModeState,
    steps: &[Step<'_>],
    blocks: Blocks,
) -> Observed {
    let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
    let set = switch_set(&cli, &halt).with_fast_path();
    let set = if blocks == Blocks::Off {
        set
    } else {
        set.with_block_cache_prototype()
    };
    let (mut boot, _) = boot_with_entry(&[HLT]);
    let mut state = build(&mut boot);
    let mut efer = boot.board.efer();
    let decoded = DecodedInstructionCache::new(1 << 10);
    let cache = DecodedBlockCache::new(1 << 8);
    if blocks == Blocks::WithoutInvalidation {
        cache.ignore_invalidation_for_test();
    }
    let (mut reports, mut writes) = (Vec::new(), Vec::new());
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
                    &set,
                    None,
                    || false,
                    |range| writes.push((range.start().get(), range.len())),
                    // No CPUID boundary: a batched run has none.
                    false,
                    Some(FetchCacheInvalidation::WriteWatch),
                    None,
                    None,
                    None,
                    None,
                    FetchCacheTag::Page4K,
                    Some(&decoded),
                    false,
                    true,
                    (blocks != Blocks::Off).then_some(&cache),
                );
                efer = report.efer;
                state.cr3 = Some(report.cr3);
                // Measurement, not behaviour: the walk counters and the fetch cache counters (a
                // block serves instructions without a peek of the fetch cache).
                report.walk_counts = Default::default();
                report.fetch_cache_stats = None;
                reports.push(report);
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
        stats: cache.stats(),
    }
}

fn assert_same(with: &Observed, without: &Observed) {
    assert_eq!(with.reports, without.reports);
    assert_eq!(with.state, without.state);
    assert_eq!(with.writes, without.writes);
    assert!(with.memory == without.memory, "frame bytes differ");
}

/// The differential check. Returns the observation of the run WITH the block cache.
fn assert_blocks_are_invisible(
    build: &dyn Fn(&mut LinuxDirectBoot) -> LongModeState,
    steps: &[Step<'_>],
) -> Observed {
    let plain = run_scenario(build, steps, Blocks::Off);
    let blocks = run_scenario(build, steps, Blocks::On);
    assert_eq!(plain.stats, DecodedBlockCacheStats::default());
    assert_same(&blocks, &plain);
    blocks
}

fn halted(report: &PagedCpl0SetReport) -> bool {
    report.exit == PagedCpl0SetExit::Run(RunExit::Halted)
}

/// A loop whose first instruction patches the immediate of the next one with the loop counter:
/// the store and the patched instruction are in one block. RSI sums what the patched instruction
/// loaded.
fn self_patching_loop(boot: &mut LinuxDirectBoot) -> LongModeState {
    write_tree(boot, TREE_PML4, &[(0, pte(FRAME_A))], None);
    let base = va(0) + 0x100;
    let mut code = mov_imm32(3, (base + 0x18) as u32); // 0x00 MOV RBX, the immediate at 0x18
    code.extend(mov_imm32(0, 0x5555)); // 0x07 MOV RAX, 0x5555
    code.extend(mov32(1, 3)); // 0x0E MOV ECX, 3
    code.extend([0x89, 0x03]); // 0x13 L: MOV [RBX], EAX
    code.extend(mov_imm32(2, 0x1111)); // 0x15 MOV RDX, 0x1111 (immediate at 0x18)
    code.extend([0x48, 0x01, 0xD6]); // 0x1C ADD RSI, RDX
    code.extend([0x83, 0xC0, 0x01]); // 0x1F ADD EAX, 1
    code.extend([0xFF, 0xC9]); // 0x22 DEC ECX
    code.extend([0x75, 0xED]); // 0x24 JNZ L
    code.push(HLT); // 0x26
    assert_eq!(code.len(), 0x27);
    lay_code(boot, FRAME_A, 0x100, &code);
    let mut state = tree_state(boot, TREE_PML4, base);
    state.gpr[Register::Rsi as usize] = 0;
    state
}

#[test]
fn a_store_into_the_running_block_ends_it_and_the_new_bytes_run() {
    let observed = assert_blocks_are_invisible(&self_patching_loop, &[Step::Run(200)]);
    assert!(halted(&observed.reports[0]), "{:?}", observed.reports[0]);
    // Every pass ran the immediate its own store had just written.
    assert_eq!(
        observed.state.gpr[Register::Rsi as usize],
        0x5555 + 0x5556 + 0x5557
    );
    let stats = observed.stats;
    // Each store ended the run of the block it was in, before the patched instruction (the first one
    // also set the dirty bit of the code page's PTE, a write to a table the fetch cache watches,
    // which drops its entries: that alone ends the run too); the block of the loop head was then
    // stale at every later pass.
    assert_eq!(stats.holds_failed, 3, "{stats:?}");
    assert!(stats.stale >= 2, "{stats:?}");
    assert!(stats.retired >= 12, "{stats:?}");
}

/// The mutation of the invalidation: a cache that ignores the page generations runs the stale
/// decode of the patched instruction, and the differential check sees it.
#[test]
fn without_the_invalidation_the_stale_block_runs_and_the_check_sees_it() {
    let steps = [Step::Run(200)];
    let plain = run_scenario(&self_patching_loop, &steps, Blocks::Off);
    let mutated = run_scenario(&self_patching_loop, &steps, Blocks::WithoutInvalidation);
    assert_ne!(mutated.state, plain.state, "the mutation must be visible");
    assert_ne!(
        mutated.state.gpr[Register::Rsi as usize],
        plain.state.gpr[Register::Rsi as usize]
    );
}

/// A loop over the end of a page: `DEC ECX; MOV EAX` end at 0xFFC, the `MOV EDX` there needs a
/// byte of the next page, and the `JNZ` back is on the next page. The block of the loop head stops
/// before the crossing instruction, which runs on the general path every time; the `JNZ` is a block
/// of its own page.
#[test]
fn a_block_never_crosses_a_page_boundary() {
    let build = |boot: &mut LinuxDirectBoot| {
        write_tree(
            boot,
            TREE_PML4,
            &[(0, pte(FRAME_A)), (1, pte(FRAME_B))],
            None,
        );
        let mut code = mov32(1, 4); // 0xFF0 MOV ECX, 4
        code.extend([0xFF, 0xC9]); // 0xFF5 L: DEC ECX
        code.extend(mov32(0, 0x11)); // 0xFF7 MOV EAX, 0x11
        let crossing = mov32(2, 0x22); // 0xFFC MOV EDX, 0x22: four bytes here, one on page B
        code.extend(&crossing[..4]);
        assert_eq!(code.len(), 0x10);
        lay_code(boot, FRAME_A, 0xFF0, &code);
        let mut tail = crossing[4..].to_vec(); // B+0x000
        tail.extend([0x75, 0xF2]); // B+0x001 JNZ L
        tail.push(HLT); // B+0x003
        lay_code(boot, FRAME_B, 0, &tail);
        tree_state(boot, TREE_PML4, va(0) + 0xFF0)
    };
    let observed = assert_blocks_are_invisible(&build, &[Step::Run(100)]);
    assert!(halted(&observed.reports[0]), "{:?}", observed.reports[0]);
    assert_eq!(observed.state.gpr[Register::Rdx as usize], 0x22);
    let stats = observed.stats;
    // Built: `DEC; MOV EAX` (two) and `JNZ` (one); the crossing instruction and the HLT have no
    // block. Four passes: 2 + 1 instructions from blocks each, in two runs.
    assert_eq!(
        (stats.built_blocks, stats.built_instructions),
        (2, 3),
        "{stats:?}"
    );
    assert_eq!((stats.runs, stats.retired), (8, 12), "{stats:?}");
    assert_eq!(stats.retired_in_runs_of_two_or_more, 8, "{stats:?}");
}

/// Two pages that jump to each other; the host writes into page A between two calls.
fn two_page_loop(boot: &mut LinuxDirectBoot) -> LongModeState {
    write_tree(
        boot,
        TREE_PML4,
        &[(0, pte(FRAME_A)), (1, pte(FRAME_B)), (2, pte(FRAME_C))],
        None,
    );
    let jmp_rel32 = |next: u64, target: u64| {
        let mut code = vec![0xE9];
        code.extend((target.wrapping_sub(next) as i32).to_le_bytes());
        code
    };
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
fn a_device_write_into_a_code_page_between_calls_makes_its_blocks_stale() {
    let device_write = |boot: &mut LinuxDirectBoot, _: &mut LongModeState| {
        write_phys(boot, FRAME_A + 0x10A, &0x2222u32.to_le_bytes());
    };
    let observed = assert_blocks_are_invisible(
        &two_page_loop,
        &[Step::Run(30), Step::Host(&device_write), Step::Run(30)],
    );
    assert_eq!(observed.state.gpr[Register::Rsi as usize], 0x2222);
    let stats = observed.stats;
    assert!(stats.stale >= 1, "{stats:?}");
    assert!(stats.retired >= 40, "{stats:?}");
}

/// The same code at the same linear page in two trees over two frames; the guest switches to the
/// second tree with `MOV CR3` (a translation context change: the cache is flushed).
#[test]
fn a_mov_cr3_of_the_guest_flushes_the_blocks_and_the_new_tree_runs() {
    let build = |boot: &mut LinuxDirectBoot| {
        write_tree(boot, TREE_PML4, &[(0, pte(FRAME_A))], None);
        write_tree(boot, TREE2_PML4, &[(0, pte(FRAME_C))], None);
        let code = |immediate: u32, tail_is_halt: bool| {
            let mut code = mov32(1, 5); // 0x00 MOV ECX, 5
            code.extend(mov_imm32(2, immediate)); // 0x05 L: MOV RDX, imm
            code.extend([0xFF, 0xC9]); // 0x0C DEC ECX
            code.extend([0x75, 0xF5]); // 0x0E JNZ L
            if tail_is_halt {
                code.push(HLT); // 0x10
                code.extend([0u8; 6]);
            } else {
                code.extend(mov_imm32(0, TREE2_PML4 as u32)); // 0x10 MOV RAX, tree 2
            }
            code.extend([0x0F, 0x22, 0xD8]); // 0x17 MOV CR3, RAX
            code.extend([0xEB, 0xE4]); // 0x1A JMP 0x00
            code
        };
        lay_code(boot, FRAME_A, 0x100, &code(0x1111, false));
        lay_code(boot, FRAME_C, 0x100, &code(0x2222, true));
        tree_state(boot, TREE_PML4, va(0) + 0x100)
    };
    let observed = assert_blocks_are_invisible(&build, &[Step::Run(300)]);
    assert!(halted(&observed.reports[0]), "{:?}", observed.reports[0]);
    assert_eq!(observed.state.cr3, Some(TREE2_PML4));
    assert_eq!(observed.state.gpr[Register::Rdx as usize], 0x2222);
    let stats = observed.stats;
    assert_eq!(stats.flushes, 1, "{stats:?}");
    assert!(stats.retired >= 10, "{stats:?}");
}

/// The guest rewrites the PTE of the page it runs on, through a window onto its page table, to a
/// frame with other code, and the next instruction is in the same block. The store drops the fetch
/// cache entries (a write to a watched table frame) without touching the code page: the block must
/// end there, and the next fetch walks to the new frame, as without blocks. The mutation (the
/// invalidation ignored) runs the old frame's decode and is seen.
#[test]
fn a_store_into_the_pte_of_the_running_page_ends_the_block() {
    let build = |boot: &mut LinuxDirectBoot| {
        write_tree(
            boot,
            TREE_PML4,
            &[(0, pte(FRAME_A))],
            Some(TREE_PML4 + 0x3000),
        );
        let code = |immediate: u32| {
            let mut code = mov_imm32(3, WINDOW_VA as u32); // 0x00 MOV RBX, PT[0] through the window
            code.extend(mov_imm32(0, pte(FRAME_C) as u32)); // 0x07 MOV RAX, PTE -> frame C
            code.extend([0x48, 0x89, 0x03]); // 0x0E MOV [RBX], RAX
            code.extend(mov_imm32(2, immediate)); // 0x11 MOV RDX, imm
            code.push(HLT); // 0x18
            code
        };
        lay_code(boot, FRAME_A, 0x100, &code(0x1111));
        lay_code(boot, FRAME_C, 0x100, &code(0x2222));
        tree_state(boot, TREE_PML4, va(0) + 0x100)
    };
    let steps = [Step::Run(100)];
    let observed = assert_blocks_are_invisible(&build, &steps);
    assert!(halted(&observed.reports[0]), "{:?}", observed.reports[0]);
    assert_eq!(observed.state.gpr[Register::Rdx as usize], 0x2222);
    let stats = observed.stats;
    // MOV RAX and the store ran from the block; it no longer held before MOV RDX.
    assert_eq!((stats.holds_failed, stats.retired), (1, 2), "{stats:?}");
    let plain = run_scenario(&build, &steps, Blocks::Off);
    let mutated = run_scenario(&build, &steps, Blocks::WithoutInvalidation);
    assert_eq!(plain.state.gpr[Register::Rdx as usize], 0x2222);
    assert_eq!(
        mutated.state.gpr[Register::Rdx as usize],
        0x1111,
        "the mutation must be visible"
    );
}
