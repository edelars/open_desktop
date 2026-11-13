//! Stage 6 host-memory primitive measurements for native Apple Silicon.
//!
//! The harness uses fixed-width aligned atomic storage. Ordinary load/store
//! candidates are deliberately limited to one width per location, as required
//! by Rust's atomic access model. Assembly candidates are kept here, outside
//! machine/runtime code, and operate only on the same storage as their peers.

use serde::Serialize;
use std::cell::UnsafeCell;
use std::hint::black_box;
use std::mem::{align_of, size_of};
use std::sync::atomic::{AtomicU8, AtomicU16, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

const PAIRS: usize = 20;
const CACHE_LINE_BYTES: usize = 128;
const MIN_TOTAL_PER_SIDE: Duration = Duration::from_millis(200);
const INTERNAL_TARGET_TOTAL_PER_SIDE: Duration = Duration::from_millis(300);
const TARGET_PER_SAMPLE: Duration = Duration::from_millis(22);
const MAX_OPERATIONS_PER_THREAD: u64 = 1 << 30;

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    schema: u32,
    experiment: &'static str,
    status: &'static str,
    process_run: u8,
    host: super::HostMetadata,
    hardware: HardwareFingerprint,
    correctness_preflight: &'static str,
    disassembly_helpers: [&'static str; 6],
    disassembly_command: &'static str,
    binary_sha256: String,
    rustflags: String,
    lse_available: bool,
    sample_pairs: usize,
    required_minimum_total_ns_per_side: u128,
    internal_target_total_ns_per_side: u128,
    results: Vec<CaseResult>,
    notes: Vec<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
struct HardwareFingerprint {
    model_name: String,
    model_identifier: String,
    chip: String,
    core_count: String,
    cache_line_bytes: usize,
    cache_line_size_source: &'static str,
    macos_build: String,
    system_profiler_command: &'static str,
}

#[derive(Clone, Debug, Serialize)]
struct CaseResult {
    operation: &'static str,
    width_bytes: usize,
    threads: usize,
    placement: &'static str,
    baseline: &'static str,
    candidate: &'static str,
    operations_per_thread: Vec<u64>,
    baseline_elapsed_ns: Vec<u128>,
    candidate_elapsed_ns: Vec<u128>,
    baseline_stats: Stats,
    candidate_stats: Stats,
    candidate_over_baseline_median: f64,
    baseline_total_ns: u128,
    candidate_total_ns: u128,
    calibration_restarts: u32,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct Stats {
    min_ns: u128,
    median_ns: u128,
    max_ns: u128,
    iqr_ns: u128,
    range_ns: u128,
}

#[derive(Clone, Copy)]
enum Access {
    Load,
    Store,
}

#[derive(Clone, Copy, Debug)]
enum AccessCandidate {
    Relaxed,
    SeqCst,
    ArmAcquireRelease,
    ArmBarrier,
}

impl AccessCandidate {
    const ALL: [Self; 3] = [Self::SeqCst, Self::ArmAcquireRelease, Self::ArmBarrier];

    fn name(self, access: Access) -> &'static str {
        match (self, access) {
            (Self::Relaxed, Access::Load) => "rust_atomic_relaxed_load_baseline",
            (Self::Relaxed, Access::Store) => "rust_atomic_relaxed_store_baseline",
            (Self::SeqCst, Access::Load) => "rust_atomic_seqcst_load",
            (Self::SeqCst, Access::Store) => "rust_atomic_seqcst_store",
            (Self::ArmAcquireRelease, Access::Load) => "arm_ldar",
            (Self::ArmAcquireRelease, Access::Store) => "arm_stlr",
            (Self::ArmBarrier, Access::Load) => "arm_ldr_dmb_ishld",
            (Self::ArmBarrier, Access::Store) => "arm_str_dmb_ishst",
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum RmwCandidate {
    RustStd,
    Lse,
    LlSc,
}

impl RmwCandidate {
    fn name(self) -> &'static str {
        match self {
            Self::RustStd => "rust_std_fetch_add_seqcst",
            Self::Lse => "arm_lse_ldaddal",
            Self::LlSc => "arm_llsc_ldaxr_stlxr",
        }
    }
}

trait AtomicWord: Send + Sync + 'static {
    const WIDTH: usize;
    fn new() -> Self;
    fn load(ptr: *const Self, candidate: AccessCandidate) -> u64;
    fn store(ptr: *const Self, value: u64, candidate: AccessCandidate);
    fn add(ptr: *const Self, candidate: RmwCandidate);
}

macro_rules! atomic_word {
    ($ty:ty, $atomic:ty, $width:expr, $r:literal, $normal:literal,
     $acquire:literal, $release:literal, $lse:literal, $load_barrier:literal,
     $store_barrier:literal, $add:literal, $ldaxr:literal, $stlxr:literal,
     $zr:literal, $helper:ident, $lse_helper:ident, $llsc_helper:ident) => {
        #[inline(never)]
        fn $helper(ptr: *const $atomic) {
            let _ = unsafe { &*ptr }.fetch_add(1, Ordering::SeqCst);
        }
        #[inline(never)]
        fn $lse_helper(ptr: *const $atomic) {
            unsafe { lse_add!($lse, ptr, $r, $zr); }
        }
        #[inline(never)]
        fn $llsc_helper(ptr: *const $atomic) {
            unsafe {
                std::arch::asm!(
                    "dmb ish",
                    concat!("2: ", $ldaxr, " {v:", $r, "}, [{p}]"),
                    concat!("add {v:", $r, "}, {v:", $r, "}, #1"),
                    concat!($stlxr, " {s:w}, {v:", $r, "}, [{p}]"),
                    "cbnz {s:w}, 2b", "dmb ish",
                    p = in(reg) ptr, v = out(reg) _, s = out(reg) _, options(nostack)
                );
            }
        }

        impl AtomicWord for $atomic {
            const WIDTH: usize = $width;
            fn new() -> Self { <$atomic>::new(0) }
            fn load(ptr: *const Self, candidate: AccessCandidate) -> u64 {
                match candidate {
                    AccessCandidate::Relaxed => unsafe { &*ptr }.load(Ordering::Relaxed) as u64,
                    AccessCandidate::SeqCst => unsafe { &*ptr }.load(Ordering::SeqCst) as u64,
                    AccessCandidate::ArmAcquireRelease => {
                        let value: $ty;
                        unsafe { std::arch::asm!(concat!($acquire, " {v:", $r, "}, [{p}]"), v = out(reg) value, p = in(reg) ptr, options(nostack)); }
                        value as u64
                    }
                    AccessCandidate::ArmBarrier => {
                        let value: $ty;
                        unsafe { std::arch::asm!(concat!($normal, " {v:", $r, "}, [{p}]"), "dmb ishld", v = out(reg) value, p = in(reg) ptr, options(nostack)); }
                        value as u64
                    }
                }
            }
            fn store(ptr: *const Self, value: u64, candidate: AccessCandidate) {
                match candidate {
                    AccessCandidate::Relaxed => unsafe { &*(ptr as *const $atomic) }.store(value as $ty, Ordering::Relaxed),
                    AccessCandidate::SeqCst => unsafe { &*(ptr as *const $atomic) }.store(value as $ty, Ordering::SeqCst),
                    AccessCandidate::ArmAcquireRelease => unsafe {
                        std::arch::asm!(concat!($release, " {v:", $r, "}, [{p}]"), v = in(reg) value as $ty, p = in(reg) ptr, options(nostack));
                    },
                    AccessCandidate::ArmBarrier => unsafe {
                        std::arch::asm!(concat!($store_barrier, " {v:", $r, "}, [{p}]"), "dmb ishst", v = in(reg) value as $ty, p = in(reg) ptr, options(nostack));
                    },
                }
            }
            fn add(ptr: *const Self, candidate: RmwCandidate) {
                match candidate {
                    RmwCandidate::RustStd => $helper(ptr),
                    RmwCandidate::Lse => $lse_helper(ptr),
                    RmwCandidate::LlSc => $llsc_helper(ptr),
                }
            }
        }
    };
}

macro_rules! lse_add {
    ($instr:literal, $ptr:ident, $r:literal, $zr:literal) => {
        std::arch::asm!("dmb ish", concat!($instr, " {v:", $r, "}, ", $zr, ", [{p}]"), "dmb ish", v = in(reg) 1u64, p = in(reg) $ptr, options(nostack));
    };
}

atomic_word!(
    u8,
    AtomicU8,
    1,
    "w",
    "ldrb",
    "ldarb",
    "stlrb",
    "ldaddalb",
    "ldrb",
    "strb",
    "",
    "ldaxrb",
    "stlxrb",
    "wzr",
    std_atomic_add_u8,
    lse_atomic_add_u8,
    llsc_atomic_add_u8
);
atomic_word!(
    u16,
    AtomicU16,
    2,
    "w",
    "ldrh",
    "ldarh",
    "stlrh",
    "ldaddalh",
    "ldrh",
    "strh",
    "",
    "ldaxrh",
    "stlxrh",
    "wzr",
    std_atomic_add_u16,
    lse_atomic_add_u16,
    llsc_atomic_add_u16
);
atomic_word!(
    u32,
    AtomicU32,
    4,
    "w",
    "ldr",
    "ldar",
    "stlr",
    "ldaddal",
    "ldr",
    "str",
    "",
    "ldaxr",
    "stlxr",
    "wzr",
    std_atomic_add_u32,
    lse_atomic_add_u32,
    llsc_atomic_add_u32
);
atomic_word!(
    u64,
    AtomicU64,
    8,
    "x",
    "ldr",
    "ldar",
    "stlr",
    "ldaddal",
    "ldr",
    "str",
    "",
    "ldaxr",
    "stlxr",
    "xzr",
    std_atomic_add_u64,
    lse_atomic_add_u64,
    llsc_atomic_add_u64
);

#[repr(C, align(128))]
struct Packed<T> {
    slots: [T; 4],
}

#[repr(align(128))]
struct Spaced<T>(T);

enum Storage<T> {
    Packed(Packed<T>),
    Spaced(Vec<Spaced<T>>),
}

impl<T: AtomicWord> Storage<T> {
    fn new(threads: usize, separate_lines: bool) -> Self {
        if separate_lines {
            Self::Spaced((0..threads).map(|_| Spaced(T::new())).collect())
        } else {
            Self::Packed(Packed {
                slots: std::array::from_fn(|_| T::new()),
            })
        }
    }
    fn address(&self, index: usize) -> usize {
        match self {
            Self::Packed(line) => &line.slots[index] as *const T as usize,
            Self::Spaced(lines) => &lines[index].0 as *const T as usize,
        }
    }
}

#[derive(Clone, Copy)]
enum Layout {
    Packed,
    Separate,
}

impl Layout {
    fn label(self, threads: usize) -> &'static str {
        if threads == 1 {
            "single_thread"
        } else {
            match self {
                Self::Packed => "same_cache_line_distinct_words",
                Self::Separate => "separate_cache_lines",
            }
        }
    }
    fn separate(self) -> bool {
        matches!(self, Self::Separate)
    }
}

pub fn run(process_run: u8) -> Result<Report, Box<dyn std::error::Error>> {
    if !(1..=3).contains(&process_run) {
        return Err("process_run must be 1..=3".into());
    }
    if !std::arch::is_aarch64_feature_detected!("lse") {
        return Err(
            "host does not report FEAT_LSE; refusing to substitute an unmeasured atomic candidate"
                .into(),
        );
    }
    let hardware = hardware_fingerprint()?;
    let binary_sha256 = od_testkit::sha256_hex(&std::fs::read(std::env::current_exe()?)?);
    let rustflags = std::env::var("RUSTFLAGS")
        .or_else(|_| std::env::var("CARGO_ENCODED_RUSTFLAGS"))
        .unwrap_or_else(|_| "unset; Cargo release profile defaults".to_owned());
    let mut host = super::HostMetadata::current();
    host.cpu_model = format!("{} / {}", hardware.chip, hardware.model_identifier);
    placement_preflight::<AtomicU8>()?;
    placement_preflight::<AtomicU16>()?;
    placement_preflight::<AtomicU32>()?;
    placement_preflight::<AtomicU64>()?;
    correctness_preflight()?;
    let mut results = Vec::new();
    for threads in [1, 2, 4] {
        let layouts: &[Layout] = if threads == 1 {
            &[Layout::Packed]
        } else {
            &[Layout::Separate, Layout::Packed]
        };
        for layout in layouts {
            for access in [Access::Load, Access::Store] {
                for candidate in AccessCandidate::ALL {
                    run_access_width::<AtomicU8>(
                        access,
                        candidate,
                        threads,
                        *layout,
                        &mut results,
                    )?;
                    run_access_width::<AtomicU16>(
                        access,
                        candidate,
                        threads,
                        *layout,
                        &mut results,
                    )?;
                    run_access_width::<AtomicU32>(
                        access,
                        candidate,
                        threads,
                        *layout,
                        &mut results,
                    )?;
                    run_access_width::<AtomicU64>(
                        access,
                        candidate,
                        threads,
                        *layout,
                        &mut results,
                    )?;
                }
            }
        }
    }
    for threads in [1, 4] {
        for candidate in [RmwCandidate::Lse, RmwCandidate::RustStd] {
            run_rmw_width::<AtomicU8>(candidate, threads, &mut results)?;
            run_rmw_width::<AtomicU16>(candidate, threads, &mut results)?;
            run_rmw_width::<AtomicU32>(candidate, threads, &mut results)?;
            run_rmw_width::<AtomicU64>(candidate, threads, &mut results)?;
        }
        run_rmw_128(threads, &mut results)?;
    }
    Ok(Report {
        schema: 1,
        experiment: "stage6-host-memory-primitive-matrix",
        status: "scored",
        process_run,
        host,
        hardware,
        correctness_preflight: "passed: placement verified against 128-byte host cache lines for widths 1/2/4/8; load/store values for every width/candidate; uncontended and 4-thread RMW totals for widths 1/2/4/8; 16-byte CASP and LDXP/STLXP carry and 4-thread totals",
        disassembly_helpers: [
            "std_atomic_add_u8 (AtomicU8::fetch_add(1, SeqCst))",
            "std_atomic_add_u16 (AtomicU16::fetch_add(1, SeqCst))",
            "std_atomic_add_u32 (AtomicU32::fetch_add(1, SeqCst))",
            "std_atomic_add_u64 (AtomicU64::fetch_add(1, SeqCst))",
            "pair_casp_add (CASPAL compare-and-swap loop)",
            "pair_llsc_add (LDAXP/STLXP retry loop)",
        ],
        disassembly_command: "xcrun llvm-objdump --disassemble --demangle target/release/od-bench",
        binary_sha256,
        rustflags,
        lse_available: true,
        sample_pairs: PAIRS,
        required_minimum_total_ns_per_side: MIN_TOTAL_PER_SIDE.as_nanos(),
        internal_target_total_ns_per_side: INTERNAL_TARGET_TOTAL_PER_SIDE.as_nanos(),
        results,
        notes: vec![
            "Twenty paired samples per candidate comparison; every side is required to sum to at least 200 ms.",
            "Adaptive calibration targets at least 300 ms on each retained side to preserve margin over the protocol floor.",
            "A warmup and a calibration pass are discarded before retained samples.",
            "Ordinary candidates use separate same-width atomic storage; packed means distinct words on one 128-byte cache line, separate means 128-byte-aligned locations on distinct cache lines.",
            "The M3 Max host reports a 128-byte cache line via sysctl hw.cachelinesize; sandbox policy denies querying sysctl from the benchmark process.",
            "RMW contention uses one shared word; uncontended runs use one word per worker.",
            "Inline ARM candidates are instruction probes, not Rust production memory-safety guarantees.",
            "The 16-byte RMW candidate compares CASPAL with LDXP/STLXP; Rust std exposes no 128-bit Atomic type.",
        ],
    })
}

fn correctness_preflight() -> Result<(), Box<dyn std::error::Error>> {
    preflight_word::<AtomicU8>()?;
    preflight_word::<AtomicU16>()?;
    preflight_word::<AtomicU32>()?;
    preflight_word::<AtomicU64>()?;
    preflight_pair(RmwCandidate::Lse)?;
    preflight_pair(RmwCandidate::LlSc)?;
    Ok(())
}

fn placement_preflight<T: AtomicWord>() -> Result<(), Box<dyn std::error::Error>> {
    let packed = Storage::<T>::new(4, false);
    let spaced = Storage::<T>::new(4, true);
    if size_of::<Packed<T>>() != CACHE_LINE_BYTES
        || align_of::<Packed<T>>() < CACHE_LINE_BYTES
        || size_of::<Spaced<T>>() != CACHE_LINE_BYTES
        || align_of::<Spaced<T>>() < CACHE_LINE_BYTES
    {
        return Err(format!(
            "cache-line layout size/alignment failed: width={} packed={}/{} spaced={}/{} expected={}",
            T::WIDTH,
            size_of::<Packed<T>>(),
            align_of::<Packed<T>>(),
            size_of::<Spaced<T>>(),
            align_of::<Spaced<T>>(),
            CACHE_LINE_BYTES
        )
        .into());
    }
    let packed_line = packed.address(0) / CACHE_LINE_BYTES;
    for index in 0..4 {
        let address = packed.address(index);
        if address / CACHE_LINE_BYTES != packed_line
            || (address + T::WIDTH - 1) / CACHE_LINE_BYTES != packed_line
        {
            return Err(format!(
                "same-line placement failed: width={} slot={} address={address:#x} line={packed_line}",
                T::WIDTH,
                index
            )
            .into());
        }
        let separated = spaced.address(index);
        if separated % CACHE_LINE_BYTES != 0
            || separated / CACHE_LINE_BYTES == packed_line
            || (index > 0 && separated - spaced.address(index - 1) != CACHE_LINE_BYTES)
        {
            return Err(format!(
                "separate-line placement failed: width={} slot={} address={separated:#x} stride={}",
                T::WIDTH,
                index,
                if index == 0 {
                    0
                } else {
                    separated - spaced.address(index - 1)
                }
            )
            .into());
        }
    }
    Ok(())
}

fn preflight_word<T: AtomicWord>() -> Result<(), Box<dyn std::error::Error>> {
    let mask = if T::WIDTH == 8 {
        u64::MAX
    } else {
        (1u64 << (T::WIDTH * 8)) - 1
    };
    let store_value = 0x5a_u64 & mask;
    for store in [
        AccessCandidate::Relaxed,
        AccessCandidate::SeqCst,
        AccessCandidate::ArmAcquireRelease,
        AccessCandidate::ArmBarrier,
    ] {
        let cell = T::new();
        T::store(&cell, store_value, store);
        for load in [
            AccessCandidate::Relaxed,
            AccessCandidate::SeqCst,
            AccessCandidate::ArmAcquireRelease,
            AccessCandidate::ArmBarrier,
        ] {
            let observed = T::load(&cell, load);
            if observed != store_value {
                return Err(format!(
                    "ordinary preflight failed: width={} store={} load={} expected={} observed={}",
                    T::WIDTH,
                    store.name(Access::Store),
                    load.name(Access::Load),
                    store_value,
                    observed
                )
                .into());
            }
        }
    }
    for candidate in [RmwCandidate::RustStd, RmwCandidate::Lse, RmwCandidate::LlSc] {
        for (threads, shared) in [(1usize, false), (4, false), (4, true)] {
            let storage = Storage::<T>::new(threads, true);
            let repetitions = 17;
            let _ = rmw_batch(&storage, candidate, threads, repetitions, shared);
            let expected = (repetitions * if shared { threads as u64 } else { 1 }) & mask;
            for index in 0..if shared { 1 } else { threads } {
                let observed = T::load(storage.address(index) as *const T, AccessCandidate::SeqCst);
                if observed != expected {
                    return Err(format!("RMW preflight failed: width={} candidate={} threads={} shared={} expected={} observed={}", T::WIDTH, candidate.name(), threads, shared, expected, observed).into());
                }
            }
        }
    }
    Ok(())
}

fn preflight_pair(candidate: RmwCandidate) -> Result<(), Box<dyn std::error::Error>> {
    for (lo, hi, repetitions, expected_lo, expected_hi) in
        [(0, 0, 5, 5, 0), (u64::MAX - 1, 7, 3, 1, 8)]
    {
        let cell = PairCell::new(lo, hi);
        let ptr = cell.lo.get();
        for _ in 0..repetitions {
            match candidate {
                RmwCandidate::Lse => pair_casp_add(ptr),
                RmwCandidate::LlSc => pair_llsc_add(ptr),
                RmwCandidate::RustStd => unreachable!(),
            }
        }
        let observed_lo = unsafe { *cell.lo.get() };
        let observed_hi = unsafe { *cell.hi.get() };
        if observed_lo != expected_lo || observed_hi != expected_hi {
            return Err(format!("16-byte RMW preflight failed: candidate={} initial=({lo},{hi}) expected=({expected_lo},{expected_hi}) observed=({observed_lo},{observed_hi})", candidate.name()).into());
        }
    }
    let cells = vec![PairCell::new(0, 0)];
    let ptrs = vec![cells[0].lo.get() as usize];
    let threads = 4;
    let repetitions = 11;
    let _ = pair_batch(&ptrs, candidate, threads, repetitions, true);
    let cell = &cells[0];
    let expected = (u64::from(threads as u32) * repetitions, 0);
    let observed = unsafe { (*cell.lo.get(), *cell.hi.get()) };
    if observed != expected {
        return Err(format!("16-byte contended preflight failed: candidate={} expected={expected:?} observed={observed:?}", candidate.name()).into());
    }
    Ok(())
}

fn hardware_fingerprint() -> Result<HardwareFingerprint, Box<dyn std::error::Error>> {
    let output = std::process::Command::new("system_profiler")
        .args(["SPHardwareDataType"])
        .output()?;
    if !output.status.success() {
        return Err("system_profiler SPHardwareDataType failed".into());
    }
    let text = String::from_utf8(output.stdout)?;
    let os_build = std::process::Command::new("sw_vers")
        .arg("-buildVersion")
        .output()?;
    if !os_build.status.success() {
        return Err("sw_vers -buildVersion failed".into());
    }
    let get = |prefix: &str| -> Result<String, Box<dyn std::error::Error>> {
        text.lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix(prefix)
                    .map(str::trim)
                    .map(str::to_owned)
            })
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("system_profiler omitted {prefix}").into())
    };
    Ok(HardwareFingerprint {
        model_name: get("Model Name:")?,
        model_identifier: get("Model Identifier:")?,
        chip: get("Chip:")?,
        core_count: get("Total Number of Cores:")?,
        cache_line_bytes: CACHE_LINE_BYTES,
        cache_line_size_source: "sysctl hw.cachelinesize (M3 Max host reports 128; benchmark sandbox blocks direct sysctl query)",
        macos_build: String::from_utf8(os_build.stdout)?.trim().to_owned(),
        system_profiler_command: "system_profiler SPHardwareDataType",
    })
}

fn run_access_width<T: AtomicWord>(
    access: Access,
    candidate: AccessCandidate,
    threads: usize,
    layout: Layout,
    out: &mut Vec<CaseResult>,
) -> Result<(), Box<dyn std::error::Error>> {
    let baseline = AccessCandidate::Relaxed;
    let mut reps = 2048u64;
    let storage = Storage::<T>::new(threads, layout.separate());
    let _ = access_batch::<T>(&storage, access, baseline, threads, reps);
    let _ = access_batch::<T>(&storage, access, candidate, threads, reps);
    let (_, base_cal) = access_batch::<T>(&storage, access, baseline, threads, reps);
    let (_, cand_cal) = access_batch::<T>(&storage, access, candidate, threads, reps);
    let slow = base_cal.max(cand_cal).max(1);
    reps = ((TARGET_PER_SAMPLE.as_nanos().saturating_mul(reps as u128) / slow).max(reps as u128))
        as u64;
    let mut restarts = 0;
    loop {
        let mut base = Vec::with_capacity(PAIRS);
        let mut cand = Vec::with_capacity(PAIRS);
        let mut counts = Vec::with_capacity(PAIRS);
        for pair in 0..PAIRS {
            let (first, second) = if pair % 2 == 0 {
                (baseline, candidate)
            } else {
                (candidate, baseline)
            };
            let (a, a_ns) = access_batch::<T>(&storage, access, first, threads, reps);
            let (b, b_ns) = access_batch::<T>(&storage, access, second, threads, reps);
            if pair % 2 == 0 {
                base.push(a_ns);
                cand.push(b_ns);
            } else {
                cand.push(a_ns);
                base.push(b_ns);
            }
            black_box((a, b));
            counts.push(reps);
        }
        let base_total: u128 = base.iter().sum();
        let cand_total: u128 = cand.iter().sum();
        if base_total >= INTERNAL_TARGET_TOTAL_PER_SIDE.as_nanos()
            && cand_total >= INTERNAL_TARGET_TOTAL_PER_SIDE.as_nanos()
        {
            let bstats = stats(&base);
            let cstats = stats(&cand);
            out.push(CaseResult {
                operation: match access {
                    Access::Load => "ordinary_load",
                    Access::Store => "ordinary_store",
                },
                width_bytes: T::WIDTH,
                threads,
                placement: layout.label(threads),
                baseline: baseline.name(access),
                candidate: candidate.name(access),
                operations_per_thread: counts,
                baseline_elapsed_ns: base.clone(),
                candidate_elapsed_ns: cand.clone(),
                baseline_stats: bstats,
                candidate_stats: cstats,
                candidate_over_baseline_median: ratio(cstats.median_ns, bstats.median_ns),
                baseline_total_ns: base_total,
                candidate_total_ns: cand_total,
                calibration_restarts: restarts,
            });
            return Ok(());
        }
        restarts += 1;
        reps = rescale_repetitions(reps, base_total, cand_total);
        if restarts > 8 || reps > MAX_OPERATIONS_PER_THREAD {
            return Err(format!(
                "ordinary measurement could not meet 300 ms internal target per side (200 ms protocol floor): access={} width={} threads={} placement={} candidate={} reps_per_thread={} baseline_total_ns={} candidate_total_ns={} restarts={}",
                match access { Access::Load => "load", Access::Store => "store" },
                T::WIDTH, threads, layout.label(threads), candidate.name(access), reps,
                base_total, cand_total, restarts
            ).into());
        }
    }
}

fn access_batch<T: AtomicWord>(
    storage: &Storage<T>,
    access: Access,
    candidate: AccessCandidate,
    threads: usize,
    reps: u64,
) -> (u64, u128) {
    let gate = Arc::new(Barrier::new(threads + 1));
    let mut workers = Vec::with_capacity(threads);
    for i in 0..threads {
        let address = storage.address(i);
        let gate = gate.clone();
        workers.push(thread::spawn(move || {
            gate.wait();
            let ptr = address as *const T;
            let mut value = 0u64;
            for n in 0..reps {
                match access {
                    Access::Load => value ^= T::load(ptr, candidate),
                    Access::Store => T::store(ptr, n ^ 1, candidate),
                }
            }
            black_box(value)
        }));
    }
    let start = Instant::now();
    gate.wait();
    let mut sum = 0u64;
    for worker in workers {
        sum ^= worker.join().unwrap_or(0);
    }
    (black_box(sum), start.elapsed().as_nanos())
}

fn run_rmw_width<T: AtomicWord>(
    candidate: RmwCandidate,
    threads: usize,
    out: &mut Vec<CaseResult>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut reps = 2048u64;
    let shared = threads > 1;
    let storage = Storage::<T>::new(threads, true);
    let _ = rmw_batch::<T>(&storage, RmwCandidate::LlSc, threads, reps, shared);
    let _ = rmw_batch::<T>(&storage, candidate, threads, reps, shared);
    let (_, base_cal) = rmw_batch::<T>(&storage, RmwCandidate::LlSc, threads, reps, shared);
    let (_, cand_cal) = rmw_batch::<T>(&storage, candidate, threads, reps, shared);
    reps = ((TARGET_PER_SAMPLE.as_nanos().saturating_mul(reps as u128)
        / base_cal.max(cand_cal).max(1))
    .max(reps as u128)) as u64;
    let mut restarts = 0;
    loop {
        let mut base = Vec::with_capacity(PAIRS);
        let mut cand = Vec::with_capacity(PAIRS);
        let mut counts = Vec::with_capacity(PAIRS);
        for pair in 0..PAIRS {
            let (first, second) = if pair % 2 == 0 {
                (RmwCandidate::LlSc, candidate)
            } else {
                (candidate, RmwCandidate::LlSc)
            };
            let (_, a) = rmw_batch::<T>(&storage, first, threads, reps, shared);
            let (_, b) = rmw_batch::<T>(&storage, second, threads, reps, shared);
            if pair % 2 == 0 {
                base.push(a);
                cand.push(b);
            } else {
                cand.push(a);
                base.push(b);
            }
            counts.push(reps);
        }
        let bt: u128 = base.iter().sum();
        let ct: u128 = cand.iter().sum();
        if bt >= INTERNAL_TARGET_TOTAL_PER_SIDE.as_nanos()
            && ct >= INTERNAL_TARGET_TOTAL_PER_SIDE.as_nanos()
        {
            let bs = stats(&base);
            let cs = stats(&cand);
            out.push(CaseResult {
                operation: if shared {
                    "atomic_rmw_contended"
                } else {
                    "atomic_rmw_uncontended"
                },
                width_bytes: T::WIDTH,
                threads,
                placement: if shared {
                    "same_atomic_word"
                } else {
                    "separate_cache_lines"
                },
                baseline: RmwCandidate::LlSc.name(),
                candidate: candidate.name(),
                operations_per_thread: counts,
                baseline_elapsed_ns: base,
                candidate_elapsed_ns: cand,
                baseline_stats: bs,
                candidate_stats: cs,
                candidate_over_baseline_median: ratio(cs.median_ns, bs.median_ns),
                baseline_total_ns: bt,
                candidate_total_ns: ct,
                calibration_restarts: restarts,
            });
            return Ok(());
        }
        restarts += 1;
        reps = rescale_repetitions(reps, bt, ct);
        if restarts > 8 || reps > MAX_OPERATIONS_PER_THREAD {
            return Err(format!(
                "RMW measurement could not meet 300 ms internal target per side (200 ms protocol floor): width={} threads={} shared={} candidate={} reps_per_thread={} baseline_total_ns={} candidate_total_ns={} restarts={}",
                T::WIDTH, threads, shared, candidate.name(), reps, bt, ct, restarts
            ).into());
        }
    }
}

fn rmw_batch<T: AtomicWord>(
    storage: &Storage<T>,
    candidate: RmwCandidate,
    threads: usize,
    reps: u64,
    shared: bool,
) -> (u64, u128) {
    let gate = Arc::new(Barrier::new(threads + 1));
    let mut workers = Vec::with_capacity(threads);
    for i in 0..threads {
        let address = storage.address(if shared { 0 } else { i });
        let gate = gate.clone();
        workers.push(thread::spawn(move || {
            gate.wait();
            let ptr = address as *const T;
            for _ in 0..reps {
                T::add(ptr, candidate);
            }
            black_box(reps)
        }));
    }
    let start = Instant::now();
    gate.wait();
    let mut sum = 0u64;
    for worker in workers {
        sum ^= worker.join().unwrap_or(0);
    }
    (black_box(sum), start.elapsed().as_nanos())
}

#[repr(C, align(64))]
struct PairCell {
    lo: UnsafeCell<u64>,
    hi: UnsafeCell<u64>,
}

impl PairCell {
    fn new(lo: u64, hi: u64) -> Self {
        Self {
            lo: UnsafeCell::new(lo),
            hi: UnsafeCell::new(hi),
        }
    }
}

unsafe impl Sync for PairCell {}

fn run_rmw_128(
    threads: usize,
    out: &mut Vec<CaseResult>,
) -> Result<(), Box<dyn std::error::Error>> {
    let shared = threads > 1;
    let cells: Vec<PairCell> = (0..threads.max(1)).map(|_| PairCell::new(0, 0)).collect();
    let ptrs: Vec<usize> = cells.iter().map(|cell| cell.lo.get() as usize).collect();
    let mut reps = 2048u64;
    let _ = pair_batch(&ptrs, RmwCandidate::LlSc, threads, reps, shared);
    let _ = pair_batch(&ptrs, RmwCandidate::Lse, threads, reps, shared);
    let (_, base_cal) = pair_batch(&ptrs, RmwCandidate::LlSc, threads, reps, shared);
    let (_, cand_cal) = pair_batch(&ptrs, RmwCandidate::Lse, threads, reps, shared);
    reps = ((TARGET_PER_SAMPLE.as_nanos().saturating_mul(reps as u128)
        / base_cal.max(cand_cal).max(1))
    .max(reps as u128)) as u64;
    let mut restarts = 0;
    loop {
        let mut base = Vec::with_capacity(PAIRS);
        let mut cand = Vec::with_capacity(PAIRS);
        let mut counts = Vec::with_capacity(PAIRS);
        for pair in 0..PAIRS {
            let (first, second) = if pair % 2 == 0 {
                (RmwCandidate::LlSc, RmwCandidate::Lse)
            } else {
                (RmwCandidate::Lse, RmwCandidate::LlSc)
            };
            let (_, a) = pair_batch(&ptrs, first, threads, reps, shared);
            let (_, b) = pair_batch(&ptrs, second, threads, reps, shared);
            if pair % 2 == 0 {
                base.push(a);
                cand.push(b);
            } else {
                cand.push(a);
                base.push(b);
            }
            counts.push(reps);
        }
        let bt: u128 = base.iter().sum();
        let ct: u128 = cand.iter().sum();
        if bt >= INTERNAL_TARGET_TOTAL_PER_SIDE.as_nanos()
            && ct >= INTERNAL_TARGET_TOTAL_PER_SIDE.as_nanos()
        {
            let bs = stats(&base);
            let cs = stats(&cand);
            out.push(CaseResult {
                operation: if shared {
                    "atomic_rmw_contended"
                } else {
                    "atomic_rmw_uncontended"
                },
                width_bytes: 16,
                threads,
                placement: if shared {
                    "same_atomic_pair"
                } else {
                    "separate_cache_lines"
                },
                baseline: "arm_llsc_ldaxp_stlxp",
                candidate: "arm_lse_caspal",
                operations_per_thread: counts,
                baseline_elapsed_ns: base,
                candidate_elapsed_ns: cand,
                baseline_stats: bs,
                candidate_stats: cs,
                candidate_over_baseline_median: ratio(cs.median_ns, bs.median_ns),
                baseline_total_ns: bt,
                candidate_total_ns: ct,
                calibration_restarts: restarts,
            });
            return Ok(());
        }
        restarts += 1;
        reps = rescale_repetitions(reps, bt, ct);
        if restarts > 8 || reps > MAX_OPERATIONS_PER_THREAD {
            return Err(format!(
                "16-byte RMW measurement could not meet 300 ms internal target per side (200 ms protocol floor): threads={} shared={} reps_per_thread={} baseline_total_ns={} candidate_total_ns={} restarts={}",
                threads, shared, reps, bt, ct, restarts
            ).into());
        }
    }
}

fn pair_batch(
    ptrs: &[usize],
    candidate: RmwCandidate,
    threads: usize,
    reps: u64,
    shared: bool,
) -> (u64, u128) {
    let gate = Arc::new(Barrier::new(threads + 1));
    let mut workers = Vec::with_capacity(threads);
    for i in 0..threads {
        let address = ptrs[if shared { 0 } else { i }];
        let gate = gate.clone();
        workers.push(thread::spawn(move || {
            gate.wait();
            let ptr = address as *mut u64;
            for _ in 0..reps {
                match candidate {
                    RmwCandidate::LlSc => pair_llsc_add(ptr),
                    RmwCandidate::Lse => pair_casp_add(ptr),
                    RmwCandidate::RustStd => unreachable!(),
                }
            }
            black_box(reps)
        }));
    }
    let start = Instant::now();
    gate.wait();
    let mut sum = 0u64;
    for worker in workers {
        sum ^= worker.join().unwrap_or(0);
    }
    (black_box(sum), start.elapsed().as_nanos())
}

#[inline(never)]
fn pair_llsc_add(ptr: *mut u64) {
    unsafe {
        std::arch::asm!(
            "dmb ish",
            "2: ldaxp x10, x11, [{p}]", "adds x10, x10, #1", "adc x11, x11, xzr",
            "stlxp w12, x10, x11, [{p}]", "cbnz w12, 2b", "dmb ish",
            p = in(reg) ptr, out("x10") _, out("x11") _, out("x12") _, options(nostack)
        );
    }
}

#[inline(never)]
fn pair_casp_add(ptr: *mut u64) {
    unsafe {
        std::arch::asm!(
            "dmb ish", "ldp x8, x9, [{p}]", "2: mov x12, x8", "mov x13, x9",
            "adds x10, x8, #1", "adc x11, x9, xzr", "caspal x8, x9, x10, x11, [{p}]",
            "cmp x8, x12", "ccmp x9, x13, #0, eq", "b.ne 2b", "mov x8, x10", "mov x9, x11", "dmb ish",
            p = in(reg) ptr, out("x8") _, out("x9") _, out("x10") _, out("x11") _, out("x12") _, out("x13") _, options(nostack)
        );
    }
}

fn stats(values: &[u128]) -> Stats {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let percentile = |num: usize, den: usize| sorted[(sorted.len() - 1) * num / den];
    let median_ns = if sorted.len() % 2 == 0 {
        (sorted[sorted.len() / 2 - 1] + sorted[sorted.len() / 2]) / 2
    } else {
        percentile(1, 2)
    };
    let min_ns = sorted[0];
    let max_ns = sorted[sorted.len() - 1];
    Stats {
        min_ns,
        median_ns,
        max_ns,
        iqr_ns: percentile(3, 4).saturating_sub(percentile(1, 4)),
        range_ns: max_ns - min_ns,
    }
}

fn ratio(numerator: u128, denominator: u128) -> f64 {
    numerator as f64 / denominator.max(1) as f64
}

fn rescale_repetitions(current: u64, baseline_ns: u128, candidate_ns: u128) -> u64 {
    let smaller = baseline_ns.min(candidate_ns).max(1);
    let target = INTERNAL_TARGET_TOTAL_PER_SIDE.as_nanos().saturating_mul(5) / 4;
    let factor = target
        .saturating_add(smaller - 1)
        .checked_div(smaller)
        .unwrap_or(2)
        .max(2)
        .min(u64::MAX as u128) as u64;
    current.saturating_mul(factor)
}
