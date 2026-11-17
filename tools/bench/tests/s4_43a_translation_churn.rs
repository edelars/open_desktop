//! S4-43a: bounded churn, reset and resource checks for the experimental
//! translation cache (`MutableTranslationProbeSession`, `TranslationProbeCache`).
//!
//! Scope: translation and cache only. No Linux boot, no IRQ/timer
//! reprogramming, no SMP, no long run. The test is a deterministic, bounded
//! stress (explicit constants below), not evidence of long-term reliability.
//!
//! Every cache answer is compared with `oracle`, an independent cache-free
//! table walk over a plain-Rust mirror of the written table words (same idea as
//! `translation_micro::oracle`, extended with the CR0.WP switch). The whole workload runs on a worker thread under a watchdog.
//!
//! `S4_43A_JSON_OUT=<path>` additionally writes the summary JSON.

use od_isa::memory::{AccessContext, AccessKind, AccessOrigin, InstructionMode, PrivilegeLevel};
use od_isa::{
    CpuActivity, INTEGER_FLAGS, InvlpgExit, LongModeState, MovCr3Exit, Register, StiShadow,
};
use od_machine::{
    GuestLinearAddress, GuestPhysicalAddress, GuestPhysicalRange, Long64ProbeConfig,
    MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, MutableTranslationProbeSession,
    PhysicalMemory, ProbeOutcome, RamLimits, TranslationProbeCache, TranslationProbeCacheOutcome,
};
use serde::Serialize;
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

// ---- explicit limits (constants, not tunables) ------------------------------
/// Measured churn iterations (the 2000-cycle run).
const MAIN_ITERATIONS: usize = 2000;
/// Baseline run used to compare release after repetitions.
const BASELINE_ITERATIONS: usize = 200;
/// Warm-up before the first measurement (allocator arenas, thread startup).
const WARMUP_ITERATIONS: usize = 100;
/// Iterations per epoch; each epoch starts with freshly allocated guest RAM.
const EPOCH_ITERATIONS: usize = 100;
/// Iterations per session; each segment starts with a new session (cache reset).
const SEGMENT_ITERATIONS: usize = 25;
/// Every CODE_EVERY-th iteration also performs code writes and a code remap.
const CODE_EVERY: usize = 10;
/// Churned data pages (plus one code page and one sentinel page).
const CHURN_PAGES: usize = 16;
/// Guest RAM per epoch.
const RAM_BYTES: u64 = 1024 * 1024;
/// Frames available for remapping (bounded host and guest working set).
const POOL_FRAMES: u64 = 64;
/// Upper bound of RAM writes (the committed-write trace) per iteration.
const MAX_WRITES_PER_ITERATION: usize = 8;
/// Whole-run watchdog.
const WATCHDOG: Duration = Duration::from_secs(120);
/// One epoch must not take longer than this.
const EPOCH_BUDGET: Duration = Duration::from_secs(20);
/// Process RSS growth limit over the 2000-cycle run, bytes (5 MiB).
const RSS_GROWTH_LIMIT: u64 = 5 * 1024 * 1024;
/// Live-heap difference allowed between the end of the baseline and the end
/// of the 2000-cycle run (equal-sized steady states), bytes.
const LIVE_HEAP_GROWTH_LIMIT: i64 = 64 * 1024;
/// Peak live heap above the pre-run level allowed during any phase.
const PEAK_HEAP_LIMIT: u64 = 2 * RAM_BYTES + 512 * 1024;
/// Minimum share of iterations in which the pre-invalidation lookup is a
/// resident hit whose answer differs from the post-write oracle (truly stale).
/// A guard that the workload exercises such cases at all, not a proof of
/// sensitivity (the control runs in the report provide that).
const MIN_STALE_HIT_PERCENT: usize = 20;

const ROOT: u64 = 0x1000;
const PDPT: u64 = 0x2000;
const PD: u64 = 0x3000;
const PT: u64 = 0x4000;
const LINEAR_BASE: u64 = 0x0040_0000;
const POOL_BASE: u64 = 0x1_0000;
const CODE_A: u64 = POOL_BASE;
const CODE_B: u64 = POOL_BASE + (POOL_FRAMES - 1) * 0x1000;
const REMAP_FIRST: u64 = 20;
const REMAP_COUNT: u64 = 40; // pool indices 20..59
const INVLPG_OFFSET: u64 = 0x20;
const MOV_CR3_OFFSET: u64 = 0x40;
const INVLPG_QWORD: u64 = 0x003b_010f; // invlpg [rbx]
const MOV_CR3_QWORD: u64 = 0x00d8_220f; // mov cr3, rax
const CODE_PAGE: usize = 0;
const SENTINEL_PAGE: usize = 1;
const FIRST_CHURN_PAGE: usize = 2;
const TOTAL_PAGES: usize = FIRST_CHURN_PAGE + CHURN_PAGES;
const CR0_WP: u64 = 0x8001_0033;
const CR0_NO_WP: u64 = 0x8000_0033;
const CR4: u64 = 0x20;
const EFER: u64 = 0xd00;
const NX_BIT: u64 = 1 << 63;
const FAULT_TAG: u64 = 1 << 63;
const BAD_OUTCOME: u64 = 0xdead_beef_dead_beef;

// ---- host allocation counter --------------------------------------------------
struct CountingAllocator;
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static PEAK_BYTES: AtomicU64 = AtomicU64::new(0);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

// SAFETY: every method forwards to `System` unchanged; the atomics only count.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record_alloc(layout.size() as u64);
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record_alloc(layout.size() as u64);
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(pointer, layout) };
        LIVE_BYTES.fetch_sub(layout.size() as u64, Ordering::Relaxed);
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        let new_pointer = unsafe { System.realloc(pointer, layout, new_size) };
        if !new_pointer.is_null() {
            LIVE_BYTES.fetch_sub(layout.size() as u64, Ordering::Relaxed);
            record_alloc(new_size as u64);
        }
        new_pointer
    }
}

fn record_alloc(size: u64) {
    ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    let live = LIVE_BYTES.fetch_add(size, Ordering::Relaxed) + size;
    PEAK_BYTES.fetch_max(live, Ordering::Relaxed);
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn live_bytes() -> u64 {
    LIVE_BYTES.load(Ordering::Relaxed)
}

fn reset_peak() {
    PEAK_BYTES.store(live_bytes(), Ordering::Relaxed);
}

fn process_rss_bytes() -> Option<u64> {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p"])
        .arg(std::process::id().to_string())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let kib: u64 = String::from_utf8(output.stdout).ok()?.trim().parse().ok()?;
    Some(kib * 1024)
}

// ---- deterministic helpers ----------------------------------------------------
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
}

fn ring3() -> AccessContext {
    AccessContext::default()
}

fn ring0() -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: AccessOrigin::Explicit,
    }
}

fn combos() -> [(AccessKind, AccessContext); 5] {
    [
        (AccessKind::Read, ring3()),
        (AccessKind::Write, ring3()),
        (AccessKind::Read, ring0()),
        (AccessKind::Write, ring0()),
        (AccessKind::Fetch, ring0()),
    ]
}

fn page_linear(page: usize) -> u64 {
    LINEAR_BASE + (page as u64) * 0x1000
}

fn initial_frame(page: usize) -> u64 {
    POOL_BASE + (page as u64) * 0x1000
}

/// Plain-Rust model of every table word the test ever wrote. The oracle reads
/// this model, so it shares neither code nor memory with the cache under test;
/// after each segment the model is also compared with the real guest RAM.
#[derive(Default)]
struct Mirror(BTreeMap<u64, u64>);

impl Mirror {
    fn get(&self, address: u64) -> u64 {
        self.0.get(&address).copied().unwrap_or(0)
    }
    fn set(&mut self, address: u64, value: u64) {
        self.0.insert(address, value);
    }
}

/// Independent cache-free walk (plain table reads, explicit rights logic).
fn oracle(memory: &Mirror, cr0: u64, linear: u64, kind: AccessKind, context: AccessContext) -> u64 {
    let user_access = matches!(context.page_privilege(), PrivilegeLevel::Ring3);
    let pfec = |present: bool| -> u64 {
        FAULT_TAG
            | u64::from(present)
            | (u64::from(matches!(kind, AccessKind::Write)) << 1)
            | (u64::from(user_access) << 2)
            | (u64::from(matches!(kind, AccessKind::Fetch)) << 4)
    };
    let indexes = [
        (linear >> 39) & 0x1ff,
        (linear >> 30) & 0x1ff,
        (linear >> 21) & 0x1ff,
        (linear >> 12) & 0x1ff,
    ];
    let mut table = ROOT;
    let (mut user, mut writable, mut no_execute) = (true, true, false);
    for index in indexes {
        let entry = memory.get(table + index * 8);
        if entry & 1 == 0 {
            return pfec(false);
        }
        user &= entry & 4 != 0;
        writable &= entry & 2 != 0;
        no_execute |= entry & NX_BIT != 0;
        table = entry & 0x000f_ffff_ffff_f000;
    }
    let wp = cr0 & (1 << 16) != 0;
    let nxe = EFER & (1 << 11) != 0;
    let denied = (user_access && !user)
        || (matches!(kind, AccessKind::Write) && !writable && (user_access || wp))
        || (matches!(kind, AccessKind::Fetch) && no_execute && nxe);
    if denied {
        return pfec(true);
    }
    table | (linear & 0xfff)
}

fn encode(outcome: &TranslationProbeCacheOutcome) -> (bool, u64) {
    match outcome {
        TranslationProbeCacheOutcome::Hit(translation) => (true, translation.physical.get()),
        TranslationProbeCacheOutcome::Walk(report) => (
            false,
            match &report.outcome {
                Ok(ProbeOutcome::Translated(translation)) => translation.physical.get(),
                Ok(ProbeOutcome::PageFault(fault)) => FAULT_TAG | u64::from(fault.error_code),
                _ => BAD_OUTCOME,
            },
        ),
    }
}

fn build_memory() -> PhysicalMemory {
    let range = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), RAM_BYTES).unwrap();
    let map = MachineMemoryMap::new(
        36,
        vec![MemoryRegionSpec::new(range, MemoryRegionKind::Ram)],
    )
    .unwrap();
    PhysicalMemory::new(&map, RamLimits::new(RAM_BYTES)).unwrap()
}

/// Counts committed writes per 4 KiB page; the memory's code-page generation
/// of a page must equal `1 + writes` (generation starts at 1).
#[derive(Default)]
struct WriteLog {
    per_page: BTreeMap<u64, u64>,
    total: u64,
}

impl WriteLog {
    fn record(&mut self, range: GuestPhysicalRange) {
        *self.per_page.entry(range.start().get() >> 12).or_default() += 1;
        self.total += 1;
    }
}

fn init_memory(memory: &PhysicalMemory, mirror: &mut Mirror, log: &mut WriteLog) {
    let mut words: Vec<(u64, u64)> = vec![(ROOT, PDPT | 7), (PDPT, PD | 7), (PD + 16, PT | 7)];
    for page in 0..TOTAL_PAGES {
        words.push((PT + (page as u64) * 8, initial_frame(page) | 7));
    }
    words.push((CODE_A + INVLPG_OFFSET, INVLPG_QWORD));
    words.push((CODE_A + MOV_CR3_OFFSET, MOV_CR3_QWORD));
    for (address, value) in words {
        memory
            .store_u64(GuestPhysicalAddress::new(address), value, |r| log.record(r))
            .unwrap();
        mirror.set(address, value);
    }
}

fn cpu_for(cr0: u64, rip: u64, target: u64) -> LongModeState {
    let mut cpu = LongModeState::default();
    cpu.rip = rip;
    cpu.gpr[Register::Rbx as usize] = target; // INVLPG [rbx]
    cpu.gpr[Register::Rax as usize] = ROOT; // MOV CR3, RAX (same root)
    cpu.cr0 = Some(cr0);
    cpu.cr3 = Some(ROOT);
    cpu.cr4 = Some(CR4);
    cpu.activity = Some(CpuActivity::Running);
    cpu.sti_shadow = Some(StiShadow::Clear);
    cpu
}

#[derive(Clone, Copy)]
enum Action {
    Unmap,
    Map,
    Remap,
    ReadOnly,
    Supervisor,
    NoExecute,
}

const ACTIONS: [Action; 6] = [
    Action::Unmap,
    Action::Map,
    Action::Remap,
    Action::ReadOnly,
    Action::Supervisor,
    Action::NoExecute,
];

#[derive(Default, Serialize, Clone)]
struct Counters {
    iterations: u64,
    epochs: u64,
    segments: u64,
    probes_compared: u64,
    /// Hits on the resident entry before invalidation (any hit).
    resident_hits_before_invalidation: u64,
    /// Resident hits whose answer differs from the post-write oracle (truly stale).
    stale_hits_before_invalidation: u64,
    invlpg_invalidations: u64,
    mov_cr3_invalidations: u64,
    /// Unmap, Map, Remap, ReadOnly, Supervisor, NoExecute.
    action_counts: [u64; 6],
    code_garbage_writes: u64,
    code_rewrites: u64,
    code_remaps: u64,
    session_resets_checked: u64,
    full_resets_checked: u64,
    cache_type_clear_checks: u64,
    generation_checks: u64,
    max_writes_in_one_iteration: u64,
    total_committed_writes: u64,
}

struct Harness {
    rng: Rng,
    epoch: usize,
    baseline: Option<Vec<u64>>,
}

fn check(
    counters: &mut Counters,
    what: &str,
    mirror: &Mirror,
    cr0: u64,
    linear: u64,
    kind: AccessKind,
    context: AccessContext,
    got: u64,
) {
    let expected = oracle(mirror, cr0, linear, kind, context);
    counters.probes_compared += 1;
    assert!(
        got == expected,
        "{what}: {kind:?}/{:?} at {linear:#x}: cache {got:#x} != oracle {expected:#x} (stale or wrong translation)",
        context.privilege
    );
}

fn put(
    session: &mut MutableTranslationProbeSession<'_>,
    mirror: &mut Mirror,
    log: &mut WriteLog,
    address: u64,
    value: u64,
) {
    session
        .write_pte(GuestPhysicalAddress::new(address), value, |r| log.record(r))
        .unwrap();
    mirror.set(address, value);
}

fn probe(
    session: &mut MutableTranslationProbeSession<'_>,
    linear: u64,
    kind: AccessKind,
    context: AccessContext,
) -> (bool, u64) {
    encode(
        &session
            .probe(GuestLinearAddress::new(linear), kind, context)
            .unwrap(),
    )
}

struct SegmentArgs<'a> {
    cr0: u64,
    rng: &'a mut Rng,
    mirror: &'a mut Mirror,
    log: &'a mut WriteLog,
    counters: &'a mut Counters,
    code_frame: &'a mut u64,
    first_iteration: usize,
    iterations: usize,
    /// `Some(baseline)` for the first segment of an epoch (fresh guest RAM).
    fresh_baseline: Option<&'a [u64]>,
}

/// One session lifetime: new session (cache reset), then `iterations` churn steps.
fn run_segment(memory: &PhysicalMemory, args: SegmentArgs<'_>) {
    let SegmentArgs {
        cr0,
        rng,
        mirror,
        log,
        counters,
        code_frame,
        first_iteration,
        iterations,
        fresh_baseline,
    } = args;
    let paging = Long64ProbeConfig::new(cr0, ROOT, CR4, EFER).unwrap();
    let mut session = MutableTranslationProbeSession::new(memory, paging).unwrap();
    assert!(
        session.is_cache_empty(),
        "new session must start with an empty cache"
    );
    counters.segments += 1;

    // Reset check: after construction every distinct first probe walks (no
    // entry carried over) and matches the oracle; on fresh RAM it also
    // matches the initial baseline (no pre-reset mapping survives).
    let all = combos();
    for page in 0..TOTAL_PAGES {
        for (index, &(kind, context)) in all.iter().enumerate() {
            let linear = page_linear(page) + 8;
            let (hit, value) = probe(&mut session, linear, kind, context);
            assert!(!hit, "first probe of page {page} in a new session hit");
            check(
                counters,
                "session reset",
                mirror,
                cr0,
                linear,
                kind,
                context,
                value,
            );
            if let Some(baseline) = fresh_baseline {
                assert_eq!(
                    value,
                    baseline[page * all.len() + index],
                    "fresh RAM translation differs from the initial baseline"
                );
            }
        }
    }
    counters.session_resets_checked += 1;
    if fresh_baseline.is_some() {
        counters.full_resets_checked += 1;
    }

    for local in 0..iterations {
        let iteration = first_iteration + local;
        let writes_before = log.total;
        let target_page = FIRST_CHURN_PAGE + (rng.next() as usize) % CHURN_PAGES;
        let target_linear = page_linear(target_page) + 0x18;
        let warm_combo = all[(rng.next() as usize) % all.len()];

        // Warm every combination, then leave `warm_combo` resident.
        for &(kind, context) in all.iter().chain(std::iter::once(&warm_combo)) {
            let (_, value) = probe(&mut session, target_linear, kind, context);
            check(
                counters,
                "warm probe",
                mirror,
                cr0,
                target_linear,
                kind,
                context,
                value,
            );
        }
        let warm_before = probe(&mut session, target_linear, warm_combo.0, warm_combo.1);

        // Mutate one PTE; no implicit flush.
        let slot = iteration % ACTIONS.len();
        counters.action_counts[slot] += 1;
        let pte = PT + (target_page as u64) * 8;
        let current_leaf = mirror.get(pte);
        let current_frame = current_leaf & 0x000f_ffff_ffff_f000;
        let keep_frame = if current_frame == 0 {
            initial_frame(target_page)
        } else {
            current_frame
        };
        let mut fresh_frame = || loop {
            let candidate = POOL_BASE + (REMAP_FIRST + rng.next() % REMAP_COUNT) * 0x1000;
            if candidate != current_frame {
                break candidate;
            }
        };
        let new_leaf = match ACTIONS[slot] {
            Action::Unmap => 0,
            Action::Map | Action::Remap => fresh_frame() | 7,
            Action::ReadOnly => keep_frame | 5,
            Action::Supervisor => keep_frame | 3,
            Action::NoExecute => keep_frame | 7 | NX_BIT,
        };
        put(&mut session, mirror, log, pte, new_leaf);

        // Contract: before invalidation a matching resident entry may stay
        // in the cache. Every such hit must be exactly the pre-write answer,
        // never another. A hit is only `stale` when the post-write oracle
        // answer differs from it (the write really changed this key's result).
        let (resident_hit, resident_value) =
            probe(&mut session, target_linear, warm_combo.0, warm_combo.1);
        if resident_hit {
            counters.resident_hits_before_invalidation += 1;
            assert_eq!(
                resident_value, warm_before.1,
                "resident entry before invalidation must be the pre-write answer"
            );
            let after_write = oracle(mirror, cr0, target_linear, warm_combo.0, warm_combo.1);
            if resident_value != after_write {
                counters.stale_hits_before_invalidation += 1;
            }
        }

        // Invalidate: even iterations INVLPG, odd iterations MOV CR3 (same root).
        let code_rip = page_linear(CODE_PAGE);
        if iteration % 2 == 0 {
            let mut cpu = cpu_for(cr0, code_rip + INVLPG_OFFSET, target_linear);
            let result = session
                .execute_invlpg_wp(&mut cpu, ring0(), INTEGER_FLAGS)
                .unwrap();
            assert!(
                matches!(result.exit, InvlpgExit::TranslationInvalidation(_)),
                "INVLPG must retire with an invalidation effect, got {:?}",
                result.exit
            );
            counters.invlpg_invalidations += 1;
        } else {
            let mut cpu = cpu_for(cr0, code_rip + MOV_CR3_OFFSET, 0);
            let result = session
                .execute_mov_cr3_wp(&mut cpu, ring0(), INTEGER_FLAGS)
                .unwrap();
            assert!(
                matches!(result.exit, MovCr3Exit::TranslationContextChange(_)),
                "MOV CR3 must retire with a context-change effect, got {:?}",
                result.exit
            );
            assert_eq!(cpu.cr3, Some(ROOT));
            counters.mov_cr3_invalidations += 1;
        }
        assert!(
            session.is_cache_empty(),
            "invalidation must empty the cache"
        );

        // After invalidation every answer equals the oracle. Only the first
        // lookup is a real check that the cache was emptied; the later Walk
        // results follow from the capacity-one cache and distinct keys.
        let mut last = (false, 0);
        for &(kind, context) in &all {
            let (hit, value) = probe(&mut session, target_linear, kind, context);
            assert!(
                !hit,
                "lookup of a distinct key after invalidation must walk"
            );
            check(
                counters,
                "post-invalidation target",
                mirror,
                cr0,
                target_linear,
                kind,
                context,
                value,
            );
            last = (hit, value);
        }
        let last_combo = all[all.len() - 1];
        let again = probe(&mut session, target_linear, last_combo.0, last_combo.1);
        if last.1 & FAULT_TAG == 0 {
            assert_eq!(again, (true, last.1), "repeat of a translated key must hit");
        } else {
            assert_eq!(again, (false, last.1), "faults are never cached");
        }
        // A random other page (code page included) must agree too.
        let other_page = (rng.next() as usize) % TOTAL_PAGES;
        let other_combo = all[(rng.next() as usize) % all.len()];
        let other_linear = page_linear(other_page) + 0x30;
        let (_, value) = probe(&mut session, other_linear, other_combo.0, other_combo.1);
        check(
            counters,
            "post-invalidation other page",
            mirror,
            cr0,
            other_linear,
            other_combo.0,
            other_combo.1,
            value,
        );

        // Code writes and code remap: fetch must see the new bytes and must
        // not use a stale translation of the code page.
        if iteration % CODE_EVERY == 3 {
            let invlpg_at = |frame: u64| frame + INVLPG_OFFSET;
            let mov_at = |frame: u64| frame + MOV_CR3_OFFSET;
            let sentinel = page_linear(SENTINEL_PAGE) + 0x40;
            let (_, warm) = probe(&mut session, sentinel, AccessKind::Read, ring3());
            assert_eq!(warm, initial_frame(SENTINEL_PAGE) + 0x40);

            // 1. Overwrite the INVLPG bytes with zeros: the fetch must see them.
            put(&mut session, mirror, log, invlpg_at(*code_frame), 0);
            counters.code_garbage_writes += 1;
            let mut cpu = cpu_for(cr0, code_rip + INVLPG_OFFSET, sentinel);
            let result = session
                .execute_invlpg_wp(&mut cpu, ring0(), INTEGER_FLAGS)
                .unwrap();
            assert!(
                !matches!(result.exit, InvlpgExit::TranslationInvalidation(_)),
                "fetch must observe the overwritten instruction bytes"
            );
            assert!(
                !session.is_cache_empty(),
                "a non-retiring INVLPG must not flush"
            );

            // 2. Restore the bytes: the next fetch must retire the INVLPG.
            put(
                &mut session,
                mirror,
                log,
                invlpg_at(*code_frame),
                INVLPG_QWORD,
            );
            counters.code_rewrites += 1;
            let mut cpu = cpu_for(cr0, code_rip + INVLPG_OFFSET, sentinel);
            let result = session
                .execute_invlpg_wp(&mut cpu, ring0(), INTEGER_FLAGS)
                .unwrap();
            assert!(
                matches!(result.exit, InvlpgExit::TranslationInvalidation(_)),
                "fetch must observe the rewritten instruction bytes"
            );
            assert!(session.is_cache_empty());

            // 3. Remap the code page. Keep a fetch entry for the old frame
            // resident; only the new frame holds valid code afterwards.
            let (_, old_fetch) = probe(
                &mut session,
                code_rip + INVLPG_OFFSET,
                AccessKind::Fetch,
                ring0(),
            );
            assert_eq!(old_fetch, *code_frame + INVLPG_OFFSET);
            let other = if *code_frame == CODE_A {
                CODE_B
            } else {
                CODE_A
            };
            put(&mut session, mirror, log, invlpg_at(other), INVLPG_QWORD);
            put(&mut session, mirror, log, mov_at(other), MOV_CR3_QWORD);
            put(&mut session, mirror, log, invlpg_at(*code_frame), 0);
            put(&mut session, mirror, log, mov_at(*code_frame), 0);
            put(&mut session, mirror, log, PT, other | 7);
            let mut cpu = cpu_for(cr0, code_rip + INVLPG_OFFSET, sentinel);
            let result = session
                .execute_invlpg_wp(&mut cpu, ring0(), INTEGER_FLAGS)
                .unwrap();
            assert!(
                matches!(result.exit, InvlpgExit::TranslationInvalidation(_)),
                "instruction fetch after code remap must use the new frame"
            );
            assert!(session.is_cache_empty());
            let (hit, fetched) = probe(
                &mut session,
                code_rip + INVLPG_OFFSET,
                AccessKind::Fetch,
                ring0(),
            );
            assert!(!hit);
            assert_eq!(
                fetched,
                other + INVLPG_OFFSET,
                "fetch must follow the remapped code page"
            );
            check(
                counters,
                "code page after remap",
                mirror,
                cr0,
                code_rip + INVLPG_OFFSET,
                AccessKind::Fetch,
                ring0(),
                fetched,
            );
            *code_frame = other;
            counters.code_remaps += 1;
        }

        let writes_here = log.total - writes_before;
        counters.max_writes_in_one_iteration =
            counters.max_writes_in_one_iteration.max(writes_here);
        assert!(
            writes_here as usize <= MAX_WRITES_PER_ITERATION,
            "write trace of one iteration exceeds the limit"
        );
        counters.iterations += 1;
    }
}

fn assert_mirror_matches_memory(memory: &PhysicalMemory, mirror: &Mirror) {
    for (&address, &value) in &mirror.0 {
        assert_eq!(
            memory.load_u64(GuestPhysicalAddress::new(address)).unwrap(),
            value,
            "mirror and guest RAM disagree at {address:#x}"
        );
    }
}

/// Snapshot-cache checks over the immutable borrow: reset via `clear`.
fn check_snapshot_cache(
    memory: &PhysicalMemory,
    mirror: &Mirror,
    cr0: u64,
    counters: &mut Counters,
) {
    let paging = Long64ProbeConfig::new(cr0, ROOT, CR4, EFER).unwrap();
    let mut cache = TranslationProbeCache::new(memory);
    assert!(cache.is_empty());
    for page in 0..TOTAL_PAGES {
        for &(kind, context) in &combos() {
            let linear = page_linear(page) + 0x10;
            let out = cache.probe(&paging, GuestLinearAddress::new(linear), kind, context);
            let (hit, value) = encode(&out);
            assert!(!hit, "distinct key must walk in the snapshot cache");
            check(
                counters,
                "snapshot cache",
                mirror,
                cr0,
                linear,
                kind,
                context,
                value,
            );
            assert_eq!(
                cache.is_empty(),
                value & FAULT_TAG != 0,
                "only translated results may be resident"
            );
        }
    }
    let sentinel = GuestLinearAddress::new(page_linear(SENTINEL_PAGE) + 0x10);
    let first = cache.probe(&paging, sentinel, AccessKind::Read, ring3());
    let second = cache.probe(&paging, sentinel, AccessKind::Read, ring3());
    assert!(!encode(&first).0 && encode(&second).0);
    assert_eq!(encode(&first).1, encode(&second).1);
    cache.clear();
    assert!(cache.is_empty(), "clear must reset the cache");
    let third = cache.probe(&paging, sentinel, AccessKind::Read, ring3());
    assert!(!encode(&third).0, "probe after clear must walk");
    assert_eq!(encode(&third).1, encode(&first).1);
    counters.cache_type_clear_checks += 1;
}

fn run_epoch(harness: &mut Harness, iterations: usize, counters: &mut Counters) {
    let started = Instant::now();
    let cr0 = if harness.epoch % 2 == 0 {
        CR0_WP
    } else {
        CR0_NO_WP
    };
    harness.epoch += 1;
    counters.epochs += 1;

    // Full reset: freshly allocated guest RAM, fresh mirror, fresh write log.
    let mut memory = build_memory();
    let mut log = WriteLog::default();
    let mut mirror = Mirror::default();
    init_memory(&mut memory, &mut mirror, &mut log);
    for (&page, &writes) in &log.per_page {
        assert_eq!(
            memory
                .code_page_generation(GuestPhysicalAddress::new(page << 12))
                .unwrap(),
            1 + writes,
            "fresh RAM generation counter must start at 1"
        );
    }
    assert_eq!(
        memory
            .code_page_generation(GuestPhysicalAddress::new(CODE_B))
            .unwrap(),
        1,
        "untouched page must be at generation 1 after reset"
    );
    let baseline_now: Vec<u64> = (0..TOTAL_PAGES)
        .flat_map(|page| {
            let mirror = &mirror;
            combos().into_iter().map(move |(kind, context)| {
                oracle(mirror, cr0, page_linear(page) + 8, kind, context)
            })
        })
        .collect();
    if harness.baseline.is_none() {
        harness.baseline = Some(baseline_now.clone());
    }
    assert_eq!(
        harness.baseline.as_ref().unwrap(),
        &baseline_now,
        "initial state must be identical after every reset"
    );
    let baseline = harness.baseline.clone().unwrap();

    let mut code_frame = CODE_A;
    let mut done = 0;
    let mut first = true;
    while done < iterations {
        let take = SEGMENT_ITERATIONS.min(iterations - done);
        let first_iteration = counters.iterations as usize;
        run_segment(
            &mut memory,
            SegmentArgs {
                cr0,
                rng: &mut harness.rng,
                mirror: &mut mirror,
                log: &mut log,
                counters,
                code_frame: &mut code_frame,
                first_iteration,
                iterations: take,
                fresh_baseline: first.then_some(&baseline[..]),
            },
        );
        first = false;
        done += take;
        assert_mirror_matches_memory(&memory, &mirror);
    }

    for (&page, &writes) in &log.per_page {
        assert_eq!(
            memory
                .code_page_generation(GuestPhysicalAddress::new(page << 12))
                .unwrap(),
            1 + writes,
            "page {page:#x}: write generation must equal 1 + committed writes"
        );
        counters.generation_checks += 1;
    }
    counters.total_committed_writes += log.total;
    check_snapshot_cache(&memory, &mirror, cr0, counters);
    assert!(
        started.elapsed() < EPOCH_BUDGET,
        "epoch exceeded its time budget"
    );
}

#[derive(Serialize)]
struct PhaseReport {
    name: &'static str,
    iterations: usize,
    epochs: u64,
    elapsed_ms: u128,
    live_heap_before: u64,
    live_heap_after: u64,
    peak_heap_above_before: u64,
    allocations: u64,
    rss_before_bytes: Option<u64>,
    rss_after_bytes: Option<u64>,
    counters: Counters,
}

fn run_phase(harness: &mut Harness, name: &'static str, iterations: usize) -> PhaseReport {
    let mut counters = Counters::default();
    let live_before = live_bytes();
    let allocations_before = ALLOCATIONS.load(Ordering::Relaxed);
    reset_peak();
    let rss_before = process_rss_bytes();
    let started = Instant::now();
    let mut remaining = iterations;
    while remaining > 0 {
        let take = EPOCH_ITERATIONS.min(remaining);
        run_epoch(harness, take, &mut counters);
        remaining -= take;
    }
    let elapsed = started.elapsed();
    let peak = PEAK_BYTES.load(Ordering::Relaxed);
    PhaseReport {
        name,
        iterations,
        epochs: counters.epochs,
        elapsed_ms: elapsed.as_millis(),
        live_heap_before: live_before,
        live_heap_after: live_bytes(),
        peak_heap_above_before: peak.saturating_sub(live_before),
        allocations: ALLOCATIONS.load(Ordering::Relaxed) - allocations_before,
        rss_before_bytes: rss_before,
        rss_after_bytes: process_rss_bytes(),
        counters,
    }
}

enum Watchdog<T> {
    Done(T),
    Panicked(Box<dyn std::any::Any + Send>),
    TimedOut,
}

fn run_with_watchdog<T: Send + 'static>(
    timeout: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Watchdog<T> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)));
    });
    match receiver.recv_timeout(timeout) {
        Ok(Ok(value)) => Watchdog::Done(value),
        Ok(Err(payload)) => Watchdog::Panicked(payload),
        Err(_) => Watchdog::TimedOut,
    }
}

#[derive(Serialize)]
struct Limits {
    main_iterations: usize,
    baseline_iterations: usize,
    warmup_iterations: usize,
    epoch_iterations: usize,
    segment_iterations: usize,
    code_every: usize,
    churn_pages: usize,
    ram_bytes: u64,
    pool_frames: u64,
    max_writes_per_iteration: usize,
    watchdog_seconds: u64,
    epoch_budget_seconds: u64,
    rss_growth_limit_bytes: u64,
    live_heap_growth_limit_bytes: i64,
    peak_heap_limit_bytes: u64,
    min_stale_hit_percent: usize,
}

#[derive(Serialize)]
struct Summary {
    schema: &'static str,
    scope: &'static str,
    not_covered: [&'static str; 6],
    os: &'static str,
    arch: &'static str,
    limits: Limits,
    phases: Vec<PhaseReport>,
    rss_growth_main_vs_warmup_bytes: Option<i64>,
    live_heap_growth_main_vs_baseline_bytes: i64,
    watchdog_self_test_timed_out: bool,
    total_elapsed_ms: u128,
}

fn workload() -> Summary {
    let started = Instant::now();
    let mut harness = Harness {
        rng: Rng(0x5334_3433_6100_0001),
        epoch: 0,
        baseline: None,
    };
    let warmup = run_phase(&mut harness, "warmup", WARMUP_ITERATIONS);
    let baseline = run_phase(&mut harness, "baseline", BASELINE_ITERATIONS);
    let main = run_phase(&mut harness, "main", MAIN_ITERATIONS);

    let rss_growth = match (warmup.rss_after_bytes, main.rss_after_bytes) {
        (Some(before), Some(after)) => Some(after as i64 - before as i64),
        _ => None,
    };
    let heap_growth = main.live_heap_after as i64 - baseline.live_heap_after as i64;
    Summary {
        schema: "s4-43a-churn-v1",
        scope: "translation cache only; bounded stress, not long-run reliability",
        not_covered: [
            "Linux boot",
            "IRQ and timer reprogramming",
            "event queue and pending events",
            "executable (JIT) cache",
            "long-run reliability",
            "SMP",
        ],
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        limits: Limits {
            main_iterations: MAIN_ITERATIONS,
            baseline_iterations: BASELINE_ITERATIONS,
            warmup_iterations: WARMUP_ITERATIONS,
            epoch_iterations: EPOCH_ITERATIONS,
            segment_iterations: SEGMENT_ITERATIONS,
            code_every: CODE_EVERY,
            churn_pages: CHURN_PAGES,
            ram_bytes: RAM_BYTES,
            pool_frames: POOL_FRAMES,
            max_writes_per_iteration: MAX_WRITES_PER_ITERATION,
            watchdog_seconds: WATCHDOG.as_secs(),
            epoch_budget_seconds: EPOCH_BUDGET.as_secs(),
            rss_growth_limit_bytes: RSS_GROWTH_LIMIT,
            live_heap_growth_limit_bytes: LIVE_HEAP_GROWTH_LIMIT,
            peak_heap_limit_bytes: PEAK_HEAP_LIMIT,
            min_stale_hit_percent: MIN_STALE_HIT_PERCENT,
        },
        phases: vec![warmup, baseline, main],
        rss_growth_main_vs_warmup_bytes: rss_growth,
        live_heap_growth_main_vs_baseline_bytes: heap_growth,
        watchdog_self_test_timed_out: false,
        total_elapsed_ms: started.elapsed().as_millis(),
    }
}

#[test]
fn translation_churn_reset_and_bounds() {
    // Watchdog self-test: a stuck closure must be reported, not waited for.
    let stuck = run_with_watchdog(Duration::from_millis(200), || {
        std::thread::sleep(Duration::from_secs(2));
    });
    assert!(matches!(stuck, Watchdog::TimedOut));
    let quick = run_with_watchdog(Duration::from_secs(5), || 7u32);
    assert!(matches!(quick, Watchdog::Done(7)));

    let mut summary = match run_with_watchdog(WATCHDOG, workload) {
        Watchdog::Done(summary) => summary,
        Watchdog::Panicked(payload) => std::panic::resume_unwind(payload),
        Watchdog::TimedOut => panic!("churn did not finish within {WATCHDOG:?}: hang"),
    };
    summary.watchdog_self_test_timed_out = true;

    let main = summary.phases.last().unwrap();
    let c = &main.counters;
    assert_eq!(c.iterations as usize, MAIN_ITERATIONS);
    assert!(
        c.action_counts.iter().all(|&n| n > 0),
        "every action must run"
    );
    assert!(c.code_remaps > 0 && c.code_garbage_writes > 0 && c.code_rewrites > 0);
    assert!(c.invlpg_invalidations > 0 && c.mov_cr3_invalidations > 0);
    assert!(
        c.stale_hits_before_invalidation as usize * 100 >= MIN_STALE_HIT_PERCENT * MAIN_ITERATIONS,
        "too few truly stale hits observed before invalidation"
    );
    assert_eq!(c.full_resets_checked, c.epochs);
    assert!(c.max_writes_in_one_iteration as usize <= MAX_WRITES_PER_ITERATION);
    assert!(
        main.peak_heap_above_before <= PEAK_HEAP_LIMIT,
        "peak host heap {} above the limit",
        main.peak_heap_above_before
    );
    assert!(
        summary.live_heap_growth_main_vs_baseline_bytes.abs() <= LIVE_HEAP_GROWTH_LIMIT,
        "live heap after 2000 cycles differs from baseline by {}",
        summary.live_heap_growth_main_vs_baseline_bytes
    );
    let rss_growth = summary
        .rss_growth_main_vs_warmup_bytes
        .expect("process RSS must be measurable (ps)");
    assert!(
        rss_growth < RSS_GROWTH_LIMIT as i64,
        "RSS grew by {rss_growth} bytes over the main run"
    );

    if let Ok(path) = std::env::var("S4_43A_JSON_OUT") {
        let json = serde_json::to_string_pretty(&summary).unwrap();
        std::fs::write(path, json + "\n").unwrap();
    }
}
