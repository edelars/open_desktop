//! S4-42a: reproducible micro-benchmark of Long64 4 KiB translations.
//!
//! Compares the uncached pure walker (`probe_long64_4k`) with the experimental
//! capacity-one `TranslationProbeCache` over deterministic working sets,
//! sequential and random order, rights paths, fault paths and cross-page
//! accesses. No Linux boot, no JIT, no CPU interpreter: only the translation
//! API. Every timed batch must reproduce a checksum computed by an independent
//! cache-free table walk written in this file (`oracle`); a mismatching batch
//! fails the whole run, so no number is produced for a wrong result.

use od_isa::memory::{AccessContext, AccessKind, PrivilegeLevel};
use od_machine::{
    GuestLinearAddress, GuestPhysicalAddress, GuestPhysicalRange, Long64ProbeConfig,
    MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, PhysicalMemory, ProbeOutcome,
    ProbeReport, RamLimits, TranslationProbeCache, TranslationProbeCacheOutcome, probe_long64_4k,
};
use serde::Serialize;
use std::hint::black_box;
use std::time::{Duration, Instant};

type BoxError = Box<dyn std::error::Error>;

const RAM_BYTES: u64 = 32 * 1024 * 1024;
const ROOT: u64 = 0x1000;
const PDPT: u64 = 0x2000;
const PD_BASE: u64 = 0x3000;
const PT_BASE: u64 = 0x7000;
const LINEAR_BASE: u64 = 0x0040_0000;
const PHYSICAL_BASE: u64 = 0x0100_0000;
const REGION_PAGES: usize = 4096;
const REGION_BYTES: u64 = (REGION_PAGES as u64) * 0x1000;
const PT_PER_REGION: usize = REGION_PAGES / 512;
const OFFSETS: [u64; 4] = [0, 8, 64, 4088];
const CROSS_OFFSET: u64 = 0xffc;
const CROSS_LEN: u64 = 8;
const WORKING_SETS: [usize; 3] = [16, 256, 4096];
const REPETITIONS: usize = 15;
const BATCH_FLOOR: Duration = Duration::from_millis(50);
const CR0: u64 = 0x8001_0011; // PG | WP | ET | PE
const CR4: u64 = 0x20; // PAE
const EFER: u64 = 0xd00; // LME | LMA | NXE
const NX_BIT: u64 = 1 << 63;
const FAULT_TAG: u64 = 1 << 63;
const BAD_OUTCOME: u64 = 0xdead_beef_dead_beef;
const SHUFFLE_SEED: u64 = 0x5334_3432_6100_0001;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
enum Region {
    UserRwx = 0,
    UserReadOnly = 1,
    SupervisorOnly = 2,
    UserNoExecute = 3,
    NotPresent = 4,
}

impl Region {
    fn base(self) -> u64 {
        LINEAR_BASE + (self as u64) * REGION_BYTES
    }

    fn leaf_flags(self) -> Option<u64> {
        match self {
            Self::UserRwx => Some(0x7),
            Self::UserReadOnly => Some(0x5),
            Self::SupervisorOnly => Some(0x3),
            Self::UserNoExecute => Some(0x7 | NX_BIT),
            Self::NotPresent => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
enum Order {
    Sequential,
    Random,
}

#[derive(Clone, Copy, Debug)]
struct Request {
    linear: u64,
    len: u64,
}

impl Request {
    fn second_probe(self) -> Option<u64> {
        let last = self.linear + self.len - 1;
        (last >> 12 != self.linear >> 12).then_some(last)
    }
}

#[derive(Clone, Debug)]
struct Scenario {
    name: &'static str,
    group: &'static str,
    kind: AccessKind,
    privilege: PrivilegeLevel,
    region: Region,
    order: Order,
    cross_page: bool,
}

impl Scenario {
    fn context(&self) -> AccessContext {
        AccessContext {
            privilege: self.privilege,
            ..AccessContext::default()
        }
    }
}

fn scenarios() -> Vec<Scenario> {
    use AccessKind::{Fetch, Read, Write};
    use PrivilegeLevel::{Ring0, Ring3};
    let mut list = Vec::new();
    let ok = [
        ("read-user", Read, Ring3, Region::UserRwx),
        ("write-user", Write, Ring3, Region::UserRwx),
        ("fetch-user", Fetch, Ring3, Region::UserRwx),
        ("read-supervisor-ring0", Read, Ring0, Region::SupervisorOnly),
    ];
    for (name, kind, privilege, region) in ok {
        for order in [Order::Sequential, Order::Random] {
            list.push(Scenario {
                name,
                group: "rights-ok",
                kind,
                privilege,
                region,
                order,
                cross_page: false,
            });
        }
    }
    let faults = [
        ("fault-not-present", Read, Ring3, Region::NotPresent),
        ("fault-user-denied", Read, Ring3, Region::SupervisorOnly),
        (
            "fault-write-readonly-ring3",
            Write,
            Ring3,
            Region::UserReadOnly,
        ),
        (
            "fault-write-readonly-ring0-wp",
            Write,
            Ring0,
            Region::UserReadOnly,
        ),
        ("fault-fetch-nx", Fetch, Ring3, Region::UserNoExecute),
    ];
    for (name, kind, privilege, region) in faults {
        list.push(Scenario {
            name,
            group: "fault",
            kind,
            privilege,
            region,
            order: Order::Sequential,
            cross_page: false,
        });
    }
    for order in [Order::Sequential, Order::Random] {
        list.push(Scenario {
            name: "cross-page-read-user",
            group: "cross-page",
            kind: Read,
            privilege: Ring3,
            region: Region::UserRwx,
            order,
            cross_page: true,
        });
    }
    list
}

fn build_requests(scenario: &Scenario, pages: usize) -> Vec<Request> {
    let base = scenario.region.base();
    let mut requests = Vec::new();
    if scenario.cross_page {
        // Request i spans pages i and i+1; the last page of the set is only a
        // second page, so the region never leaks into its neighbour.
        for page in 0..pages - 1 {
            requests.push(Request {
                linear: base + (page as u64) * 0x1000 + CROSS_OFFSET,
                len: CROSS_LEN,
            });
        }
    } else {
        for page in 0..pages {
            for offset in OFFSETS {
                requests.push(Request {
                    linear: base + (page as u64) * 0x1000 + offset,
                    len: 8,
                });
            }
        }
    }
    if scenario.order == Order::Random {
        let mut state = SHUFFLE_SEED ^ (pages as u64);
        for index in (1..requests.len()).rev() {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            let draw = state.wrapping_mul(0x2545_f491_4f6c_dd1d);
            requests.swap(index, (draw % (index as u64 + 1)) as usize);
        }
    }
    requests
}

fn build_memory() -> Result<PhysicalMemory, BoxError> {
    let range = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), RAM_BYTES)?;
    let map = MachineMemoryMap::new(
        36,
        vec![MemoryRegionSpec::new(range, MemoryRegionKind::Ram)],
    )?;
    let mut memory = PhysicalMemory::new(&map, RamLimits::new(RAM_BYTES))?;
    memory.write_u64(GuestPhysicalAddress::new(ROOT), PDPT | 7, |_| {})?;
    memory.write_u64(GuestPhysicalAddress::new(PDPT), PD_BASE | 7, |_| {})?;
    for region in [
        Region::UserRwx,
        Region::UserReadOnly,
        Region::SupervisorOnly,
        Region::UserNoExecute,
        Region::NotPresent,
    ] {
        for group in 0..PT_PER_REGION {
            let table_index = (region as usize) * PT_PER_REGION + group;
            let pt = PT_BASE + (table_index as u64) * 0x1000;
            let pd_index = (region.base() - LINEAR_BASE) / 0x20_0000 + 2 + group as u64;
            memory.write_u64(
                GuestPhysicalAddress::new(PD_BASE + pd_index * 8),
                pt | 7,
                |_| {},
            )?;
            let Some(flags) = region.leaf_flags() else {
                continue;
            };
            for slot in 0..512usize {
                let page = group * 512 + slot;
                let leaf = PHYSICAL_BASE + (page as u64) * 0x1000;
                memory.write_u64(
                    GuestPhysicalAddress::new(pt + (slot as u64) * 8),
                    leaf | flags,
                    |_| {},
                )?;
            }
        }
    }
    Ok(memory)
}

/// Independent cache-free walk: plain table reads and explicit rights logic,
/// sharing no code with `probe_long64_4k`. Encoded like `encode_outcome`.
fn oracle(
    memory: &PhysicalMemory,
    linear: u64,
    kind: AccessKind,
    context: AccessContext,
) -> Result<u64, BoxError> {
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
        let entry = memory.read_u64(GuestPhysicalAddress::new(table + index * 8))?;
        if entry & 1 == 0 {
            return Ok(pfec(false));
        }
        user &= entry & 4 != 0;
        writable &= entry & 2 != 0;
        no_execute |= entry & NX_BIT != 0;
        table = entry & 0x000f_ffff_ffff_f000;
    }
    let wp = CR0 & (1 << 16) != 0;
    let nxe = EFER & (1 << 11) != 0;
    let denied = (user_access && !user)
        || (matches!(kind, AccessKind::Write) && !writable && (user_access || wp))
        || (matches!(kind, AccessKind::Fetch) && no_execute && nxe);
    if denied {
        return Ok(pfec(true));
    }
    Ok(table | (linear & 0xfff))
}

fn encode_outcome(report: &ProbeReport) -> u64 {
    match &report.outcome {
        Ok(ProbeOutcome::Translated(translation)) => translation.physical.get(),
        Ok(ProbeOutcome::PageFault(fault)) => FAULT_TAG | u64::from(fault.error_code),
        Err(_) => BAD_OUTCOME,
    }
}

fn fold(checksum: u64, value: u64) -> u64 {
    (checksum.rotate_left(5) ^ value).wrapping_mul(0x9e37_79b9_7f4a_7c15)
}

fn expected_episode_checksum(
    memory: &PhysicalMemory,
    scenario: &Scenario,
    requests: &[Request],
) -> Result<u64, BoxError> {
    let context = scenario.context();
    let mut checksum = 0;
    for request in requests {
        checksum = fold(
            checksum,
            oracle(memory, request.linear, scenario.kind, context)?,
        );
        if let Some(second) = request.second_probe() {
            checksum = fold(checksum, oracle(memory, second, scenario.kind, context)?);
        }
    }
    Ok(checksum)
}

fn run_baseline(
    memory: &PhysicalMemory,
    config: &Long64ProbeConfig,
    scenario: &Scenario,
    requests: &[Request],
    episodes: usize,
) -> u64 {
    let context = scenario.context();
    let kind = scenario.kind;
    let mut total = 0u64;
    for _ in 0..episodes {
        let mut checksum = 0;
        for request in requests {
            let report = probe_long64_4k(
                memory,
                config,
                GuestLinearAddress::new(request.linear),
                kind,
                context,
            );
            checksum = fold(checksum, encode_outcome(&report));
            if let Some(second) = request.second_probe() {
                let report = probe_long64_4k(
                    memory,
                    config,
                    GuestLinearAddress::new(second),
                    kind,
                    context,
                );
                checksum = fold(checksum, encode_outcome(&report));
            }
        }
        total = total.wrapping_add(checksum);
    }
    black_box(total)
}

fn cached_value(outcome: &TranslationProbeCacheOutcome) -> u64 {
    match outcome {
        TranslationProbeCacheOutcome::Hit(translation) => translation.physical.get(),
        TranslationProbeCacheOutcome::Walk(report) => encode_outcome(report),
    }
}

fn run_cached(
    cache: &mut TranslationProbeCache<'_>,
    config: &Long64ProbeConfig,
    scenario: &Scenario,
    requests: &[Request],
    episodes: usize,
) -> u64 {
    let context = scenario.context();
    let kind = scenario.kind;
    let mut total = 0u64;
    for _ in 0..episodes {
        let mut checksum = 0;
        for request in requests {
            let outcome = cache.probe(
                config,
                GuestLinearAddress::new(request.linear),
                kind,
                context,
            );
            checksum = fold(checksum, cached_value(&outcome));
            if let Some(second) = request.second_probe() {
                let outcome = cache.probe(config, GuestLinearAddress::new(second), kind, context);
                checksum = fold(checksum, cached_value(&outcome));
            }
        }
        total = total.wrapping_add(checksum);
    }
    black_box(total)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
struct EpisodeCounts {
    accesses: usize,
    probes: usize,
    faults: usize,
    cache_hits: usize,
    cache_walks: usize,
    uncached_table_entry_reads: usize,
    cached_table_entry_reads: usize,
}

/// Untimed per-request preflight: baseline, cached path (two episodes, counts
/// taken from the second one) and the independent oracle must all agree.
fn validate_and_count(
    memory: &PhysicalMemory,
    config: &Long64ProbeConfig,
    scenario: &Scenario,
    requests: &[Request],
) -> Result<EpisodeCounts, BoxError> {
    let context = scenario.context();
    let kind = scenario.kind;
    let mut cache = TranslationProbeCache::new(memory);
    let mut counts = EpisodeCounts::default();
    for _ in 0..2 {
        counts = EpisodeCounts {
            accesses: requests.len(),
            ..EpisodeCounts::default()
        };
        for request in requests {
            let mut probes = vec![request.linear];
            probes.extend(request.second_probe());
            for linear in probes {
                let expected = oracle(memory, linear, kind, context)?;
                let baseline = probe_long64_4k(
                    memory,
                    config,
                    GuestLinearAddress::new(linear),
                    kind,
                    context,
                );
                if encode_outcome(&baseline) != expected {
                    return Err(format!(
                        "{} {linear:#x}: uncached probe {:#x} != oracle {expected:#x}",
                        scenario.name,
                        encode_outcome(&baseline)
                    )
                    .into());
                }
                let outcome = cache.probe(config, GuestLinearAddress::new(linear), kind, context);
                let cached = cached_value(&outcome);
                if cached != expected {
                    return Err(format!(
                        "{} {linear:#x}: cached probe {cached:#x} != oracle {expected:#x}",
                        scenario.name
                    )
                    .into());
                }
                counts.probes += 1;
                counts.faults += usize::from(expected & FAULT_TAG != 0);
                counts.uncached_table_entry_reads += baseline.trace.len();
                match outcome {
                    TranslationProbeCacheOutcome::Hit(translation) => {
                        match baseline.outcome {
                            Ok(ProbeOutcome::Translated(reference)) if reference == translation => {
                            }
                            _ => {
                                return Err(format!(
                                    "{} {linear:#x}: cache hit != walk",
                                    scenario.name
                                )
                                .into());
                            }
                        }
                        counts.cache_hits += 1;
                    }
                    TranslationProbeCacheOutcome::Walk(report) => {
                        counts.cache_walks += 1;
                        counts.cached_table_entry_reads += report.trace.len();
                    }
                }
            }
        }
    }
    Ok(counts)
}

#[derive(Clone, Copy, Debug, Serialize)]
struct Repetition {
    episodes: usize,
    baseline_ns: u64,
    cached_ns: u64,
    baseline_ns_per_access: f64,
    cached_ns_per_access: f64,
    cached_over_baseline: f64,
    baseline_first: bool,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct Stats {
    median: f64,
    min: f64,
    max: f64,
    q1: f64,
    q3: f64,
    /// (max - min) / median.
    relative_range: f64,
}

#[derive(Clone, Debug, Serialize)]
struct CellReport {
    scenario: &'static str,
    group: &'static str,
    access_kind: String,
    privilege: String,
    region: Region,
    order: Order,
    cross_page: bool,
    working_set_pages: usize,
    working_set_bytes: u64,
    counts: EpisodeCounts,
    episodes_per_batch: usize,
    calibration_batches_ns: Vec<(u64, u64)>,
    expected_episode_checksum: String,
    correctness: &'static str,
    repetitions: Vec<Repetition>,
    baseline_ns_per_access: Stats,
    cached_ns_per_access: Stats,
    cached_over_baseline: Stats,
}

#[derive(Clone, Debug, Serialize)]
pub struct TranslationMicroReport {
    schema: &'static str,
    process_run: u8,
    fixture: &'static str,
    fixture_sha256: String,
    request_profile_sha256: String,
    guest_page_bytes: u64,
    host_page_bytes: String,
    cache_model: &'static str,
    repetitions: usize,
    batch_floor_ns: u64,
    config: ConfigReport,
    environment: Environment,
    cells: Vec<CellReport>,
}

#[derive(Clone, Debug, Serialize)]
struct ConfigReport {
    cr0: String,
    cr3: String,
    cr4: String,
    efer: String,
}

#[derive(Clone, Debug, Serialize)]
struct Environment {
    host: crate::HostMetadata,
    binary_path: String,
    binary_sha256: String,
    logical_cpus: usize,
    rustflags_env: String,
    power_source: String,
}

pub fn run(process_run: u8) -> Result<TranslationMicroReport, BoxError> {
    if !(1..=3).contains(&process_run) {
        return Err("--process-run must be in 1..=3".into());
    }
    let memory = build_memory()?;
    let config = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER)
        .map_err(|error| format!("invalid paging fixture: {error:?}"))?;
    let fixture_sha256 = hash_memory(&memory)?;
    let list = scenarios();

    // Preflight for every cell before any timing starts.
    let mut prepared = Vec::new();
    let mut profile_bytes = Vec::new();
    for scenario in &list {
        for pages in WORKING_SETS {
            let requests = build_requests(scenario, pages);
            let counts = validate_and_count(&memory, &config, scenario, &requests)?;
            let expected = expected_episode_checksum(&memory, scenario, &requests)?;
            profile_bytes.extend_from_slice(scenario.name.as_bytes());
            profile_bytes.extend_from_slice(&(pages as u64).to_le_bytes());
            for request in &requests {
                profile_bytes.extend_from_slice(&request.linear.to_le_bytes());
            }
            prepared.push((scenario, pages, requests, counts, expected));
        }
    }
    if hash_memory(&memory)? != fixture_sha256 {
        return Err("preflight changed fixture RAM".into());
    }

    let mut cells = Vec::new();
    for (scenario, pages, requests, counts, expected) in prepared {
        eprintln!("translation-micro: {} / {pages} pages", scenario.name);
        cells.push(measure_cell(
            &memory, &config, scenario, pages, &requests, counts, expected,
        )?);
    }
    if hash_memory(&memory)? != fixture_sha256 {
        return Err("timed translations changed fixture RAM".into());
    }
    let binary_path = std::env::current_exe()?;
    Ok(TranslationMicroReport {
        schema: "od-bench-translation-micro-v1",
        process_run,
        fixture: "synthetic-long64-4k-5-regions-32m-v1",
        fixture_sha256,
        request_profile_sha256: od_testkit::sha256_hex(&profile_bytes),
        guest_page_bytes: 4096,
        host_page_bytes: command_line("getconf", &["PAGESIZE"]),
        cache_model: "TranslationProbeCache: capacity one, fresh per batch, carried across episodes within a batch",
        repetitions: REPETITIONS,
        batch_floor_ns: BATCH_FLOOR.as_nanos() as u64,
        config: ConfigReport {
            cr0: format!("{CR0:#x}"),
            cr3: format!("{ROOT:#x}"),
            cr4: format!("{CR4:#x}"),
            efer: format!("{EFER:#x}"),
        },
        environment: Environment {
            host: crate::HostMetadata::current(),
            binary_path: binary_path.display().to_string(),
            binary_sha256: od_testkit::sha256_hex(&std::fs::read(&binary_path)?),
            logical_cpus: std::thread::available_parallelism().map_or(0, usize::from),
            rustflags_env: std::env::var("RUSTFLAGS").unwrap_or_else(|_| "unset".into()),
            power_source: command_line("pmset", &["-g", "batt"])
                .lines()
                .next()
                .unwrap_or("unavailable")
                .to_owned(),
        },
        cells,
    })
}

fn measure_cell(
    memory: &PhysicalMemory,
    config: &Long64ProbeConfig,
    scenario: &Scenario,
    pages: usize,
    requests: &[Request],
    counts: EpisodeCounts,
    expected: u64,
) -> Result<CellReport, BoxError> {
    let floor = BATCH_FLOOR.as_nanos() as u64;
    let mut episodes = 1usize;
    let mut calibration = Vec::new();
    loop {
        let (baseline_ns, cached_ns) =
            time_pair(memory, config, scenario, requests, episodes, expected, true)?;
        calibration.push((baseline_ns, cached_ns));
        if baseline_ns >= floor && cached_ns >= floor {
            break;
        }
        episodes = episodes.checked_mul(2).ok_or("episode count overflow")?;
    }
    let mut repetitions = Vec::with_capacity(REPETITIONS);
    for repetition in 0..REPETITIONS {
        let baseline_first = repetition % 2 == 0;
        let (baseline_ns, cached_ns) = time_pair(
            memory,
            config,
            scenario,
            requests,
            episodes,
            expected,
            baseline_first,
        )?;
        let accesses = (episodes * requests.len()) as f64;
        repetitions.push(Repetition {
            episodes,
            baseline_ns,
            cached_ns,
            baseline_ns_per_access: baseline_ns as f64 / accesses,
            cached_ns_per_access: cached_ns as f64 / accesses,
            cached_over_baseline: cached_ns as f64 / baseline_ns as f64,
            baseline_first,
        });
    }
    let stats = |select: fn(&Repetition) -> f64| {
        stats_of(repetitions.iter().map(select).collect::<Vec<_>>())
    };
    Ok(CellReport {
        scenario: scenario.name,
        group: scenario.group,
        access_kind: format!("{:?}", scenario.kind),
        privilege: format!("{:?}", scenario.privilege),
        region: scenario.region,
        order: scenario.order,
        cross_page: scenario.cross_page,
        working_set_pages: pages,
        working_set_bytes: (pages as u64) * 4096,
        counts,
        episodes_per_batch: episodes,
        calibration_batches_ns: calibration,
        expected_episode_checksum: format!("{expected:#018x}"),
        correctness: "every batch checksum equals the independent-oracle checksum",
        baseline_ns_per_access: stats(|r| r.baseline_ns_per_access),
        cached_ns_per_access: stats(|r| r.cached_ns_per_access),
        cached_over_baseline: stats(|r| r.cached_over_baseline),
        repetitions,
    })
}

fn time_pair(
    memory: &PhysicalMemory,
    config: &Long64ProbeConfig,
    scenario: &Scenario,
    requests: &[Request],
    episodes: usize,
    expected_episode: u64,
    baseline_first: bool,
) -> Result<(u64, u64), BoxError> {
    let expected = expected_episode.wrapping_mul(episodes as u64);
    let mut cache = TranslationProbeCache::new(memory);
    let mut baseline_ns = 0;
    let mut cached_ns = 0;
    let mut baseline_sum = 0;
    let mut cached_sum = 0;
    for step in 0..2 {
        if (step == 0) == baseline_first {
            let start = Instant::now();
            baseline_sum = run_baseline(memory, config, scenario, requests, episodes);
            baseline_ns = nanos(start.elapsed());
        } else {
            let start = Instant::now();
            cached_sum = run_cached(&mut cache, config, scenario, requests, episodes);
            cached_ns = nanos(start.elapsed());
        }
    }
    if baseline_sum != expected || cached_sum != expected {
        return Err(format!(
            "{}: batch checksum mismatch (expected {expected:#x}, uncached {baseline_sum:#x}, cached {cached_sum:#x}); measurement rejected",
            scenario.name
        )
        .into());
    }
    Ok((baseline_ns, cached_ns))
}

fn stats_of(mut values: Vec<f64>) -> Stats {
    values.sort_by(|left, right| left.total_cmp(right));
    let median = percentile(&values, 0.5);
    let min = values[0];
    let max = values[values.len() - 1];
    Stats {
        median,
        min,
        max,
        q1: percentile(&values, 0.25),
        q3: percentile(&values, 0.75),
        relative_range: (max - min) / median,
    }
}

fn percentile(sorted: &[f64], quantile: f64) -> f64 {
    let position = quantile * (sorted.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    sorted[lower] + (sorted[upper] - sorted[lower]) * (position - lower as f64)
}

fn nanos(duration: Duration) -> u64 {
    duration.as_nanos().min(u128::from(u64::MAX)) as u64
}

fn hash_memory(memory: &PhysicalMemory) -> Result<String, BoxError> {
    let mut bytes = vec![0; RAM_BYTES as usize];
    memory.read_bytes(GuestPhysicalAddress::new(0), &mut bytes)?;
    Ok(od_testkit::sha256_hex(&bytes))
}

fn command_line(program: &str, args: &[&str]) -> String {
    std::process::Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_else(|| "unavailable".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scenario_matches_the_independent_oracle_on_small_working_sets() {
        let memory = build_memory().unwrap();
        let config = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap();
        for scenario in scenarios() {
            for pages in [2usize, 16] {
                let requests = build_requests(&scenario, pages);
                let counts = validate_and_count(&memory, &config, &scenario, &requests).unwrap();
                assert_eq!(counts.cache_hits + counts.cache_walks, counts.probes);
                if scenario.group == "fault" {
                    assert_eq!(counts.faults, counts.probes, "{}", scenario.name);
                    assert_eq!(counts.cache_hits, 0, "faults must never be cached");
                } else {
                    assert_eq!(counts.faults, 0, "{}", scenario.name);
                }
            }
        }
    }

    #[test]
    fn literal_translations_and_page_fault_error_codes() {
        let memory = build_memory().unwrap();
        let ctx = AccessContext::default();
        let read = |region: Region, kind, context| {
            oracle(&memory, region.base() + 0x123, kind, context).unwrap()
        };
        assert_eq!(
            read(Region::UserRwx, AccessKind::Read, ctx),
            PHYSICAL_BASE + 0x123
        );
        assert_eq!(
            read(Region::NotPresent, AccessKind::Read, ctx),
            FAULT_TAG | 4
        );
        assert_eq!(
            read(Region::SupervisorOnly, AccessKind::Read, ctx),
            FAULT_TAG | 5
        );
        assert_eq!(
            read(Region::UserReadOnly, AccessKind::Write, ctx),
            FAULT_TAG | 7
        );
        assert_eq!(
            read(Region::UserNoExecute, AccessKind::Fetch, ctx),
            FAULT_TAG | 21
        );
        let ring0 = AccessContext {
            privilege: PrivilegeLevel::Ring0,
            ..ctx
        };
        assert_eq!(
            read(Region::UserReadOnly, AccessKind::Write, ring0),
            FAULT_TAG | 3
        );
    }

    #[test]
    fn requests_are_deterministic_and_random_order_is_a_permutation() {
        let all = scenarios();
        let sequential = all
            .iter()
            .find(|s| s.name == "read-user" && s.order == Order::Sequential)
            .unwrap();
        let random = all
            .iter()
            .find(|s| s.name == "read-user" && s.order == Order::Random)
            .unwrap();
        let a = build_requests(random, 256);
        let b = build_requests(random, 256);
        assert_eq!(
            a.iter().map(|r| r.linear).collect::<Vec<_>>(),
            b.iter().map(|r| r.linear).collect::<Vec<_>>()
        );
        let sequential_order: Vec<u64> = build_requests(sequential, 256)
            .iter()
            .map(|r| r.linear)
            .collect();
        assert_ne!(
            a.iter().map(|r| r.linear).collect::<Vec<_>>(),
            sequential_order,
            "random order must differ from sequential"
        );
        let mut sorted: Vec<u64> = a.iter().map(|r| r.linear).collect();
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            build_requests(sequential, 256)
                .iter()
                .map(|r| r.linear)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn mismatching_checksum_is_rejected() {
        let memory = build_memory().unwrap();
        let config = Long64ProbeConfig::new(CR0, ROOT, CR4, EFER).unwrap();
        let scenario = &scenarios()[0];
        let requests = build_requests(scenario, 16);
        let expected = expected_episode_checksum(&memory, scenario, &requests).unwrap();
        assert!(time_pair(&memory, &config, scenario, &requests, 1, expected, true).is_ok());
        assert!(time_pair(&memory, &config, scenario, &requests, 1, expected ^ 1, true).is_err());
    }
}
