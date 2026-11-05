//! Deterministic uncached Long64 page-walk versus one-entry probe-cache study.

use od_isa::memory::{AccessContext, AccessKind};
use od_machine::{
    GuestLinearAddress, GuestPhysicalAddress, GuestPhysicalRange, Long64ProbeConfig,
    MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, PhysicalMemory, ProbeOutcome, RamLimits,
    TranslationProbeCache, TranslationProbeCacheOutcome, probe_long64_4k,
};
use serde::Serialize;
use std::hint::black_box;
use std::time::{Duration, Instant};

const RAM_BYTES: u64 = 32 * 1024 * 1024;
const ROOT: u64 = 0x1000;
const PDPT: u64 = 0x2000;
const PD_BASE: u64 = 0x3000;
const PT_BASE: u64 = 0x7000;
const LINEAR_BASE: u64 = 0x0040_0000;
const PHYSICAL_BASE: u64 = 0x0100_0000;
const OFFSETS: [u64; 4] = [0, 8, 64, 4088];
const TARGET_BATCH: Duration = Duration::from_millis(200);
const SAMPLE_PAIRS: usize = 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Request {
    linear: GuestLinearAddress,
    expected_physical: u64,
}

#[derive(Clone, Debug)]
struct Profile {
    name: &'static str,
    pages: usize,
    requests: Vec<Request>,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct PairSample {
    baseline_ns: u64,
    candidate_ns: u64,
    candidate_over_baseline: f64,
    episodes: usize,
}

#[derive(Clone, Debug, Serialize)]
struct ModeReport {
    mode: &'static str,
    episodes_per_sample: usize,
    calibration_attempts: Vec<CalibrationAttempt>,
    discarded_series: Vec<DiscardedSeries>,
    hit_count_per_episode: usize,
    walk_count_per_episode: usize,
    raw_pairs: Vec<PairSample>,
    median_candidate_over_baseline: f64,
    min_candidate_over_baseline: f64,
    max_candidate_over_baseline: f64,
    iqr_candidate_over_baseline: f64,
    baseline_median_ns_per_request: f64,
    candidate_median_ns_per_request: f64,
    baseline_median_requests_per_second: f64,
    candidate_median_requests_per_second: f64,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct CalibrationAttempt {
    episodes: usize,
    baseline_ns: u64,
    candidate_ns: u64,
    accepted: bool,
}

#[derive(Clone, Debug, Serialize)]
struct DiscardedSeries {
    episodes: usize,
    pair_count: usize,
    reason: &'static str,
    samples: Vec<PairSample>,
}

#[derive(Clone, Debug, Serialize)]
struct ProfileReport {
    name: &'static str,
    pages: usize,
    requests_per_episode: usize,
    empty_cache: ModeReport,
    repeated: ModeReport,
}

#[derive(Clone, Debug, Serialize)]
pub struct PageWalkCompareReport {
    schema: &'static str,
    process_run: u8,
    fixture: &'static str,
    fixture_sha256: String,
    request_profile_sha256: String,
    memory_bytes: u64,
    request_storage_bytes: usize,
    cache_object_bytes: usize,
    sample_pairs: usize,
    minimum_variant_batch_ns: u64,
    source_metadata: crate::HostMetadata,
    profiles: Vec<ProfileReport>,
}

pub fn run(process_run: u8) -> Result<PageWalkCompareReport, Box<dyn std::error::Error>> {
    if !(1..=3).contains(&process_run) {
        return Err("--process-run must be in 1..=3".into());
    }
    let memory = build_memory(4096)?;
    let config = Long64ProbeConfig::new(0x8001_0011, ROOT, 0x20, 0xd00)
        .map_err(|error| format!("invalid fixed paging fixture: {error:?}"))?;
    let context = AccessContext::default();
    let profiles = build_profiles();
    let fixture_sha256 = hash_memory(&memory)?;
    let request_profile_sha256 = hash_profiles(&profiles);

    // Validate every request before any timed work. Also require exact literal
    // physical addresses and equal pure-probe/cache outcomes.
    for profile in &profiles {
        validate_profile(&memory, &config, context, profile)?;
    }
    if hash_memory(&memory)? != fixture_sha256 {
        return Err("pure translation preflight changed fixture RAM".into());
    }

    let mut reports = Vec::with_capacity(profiles.len());
    for profile in &profiles {
        let empty_cache =
            measure_mode(&memory, &config, context, profile, CacheMode::EmptyEpisode)?;
        let repeated = measure_mode(&memory, &config, context, profile, CacheMode::Repeated)?;
        reports.push(ProfileReport {
            name: profile.name,
            pages: profile.pages,
            requests_per_episode: profile.requests.len(),
            empty_cache,
            repeated,
        });
    }
    if hash_memory(&memory)? != fixture_sha256 {
        return Err("timed pure translations changed fixture RAM".into());
    }
    Ok(PageWalkCompareReport {
        schema: "od-bench-page-walk-compare-v1",
        process_run,
        fixture: "synthetic-long64-4k-32m-v1",
        fixture_sha256,
        request_profile_sha256,
        memory_bytes: RAM_BYTES,
        request_storage_bytes: profiles
            .iter()
            .map(|profile| profile.requests.len() * std::mem::size_of::<Request>())
            .sum(),
        cache_object_bytes: std::mem::size_of::<TranslationProbeCache<'static>>(),
        sample_pairs: SAMPLE_PAIRS,
        minimum_variant_batch_ns: TARGET_BATCH.as_nanos() as u64,
        source_metadata: crate::HostMetadata::current(),
        profiles: reports,
    })
}

#[derive(Clone, Copy)]
enum CacheMode {
    EmptyEpisode,
    Repeated,
}

impl CacheMode {
    fn name(self) -> &'static str {
        match self {
            Self::EmptyEpisode => "empty-cache episode",
            Self::Repeated => "repeated episodes",
        }
    }
}

fn build_memory(pages: usize) -> Result<PhysicalMemory, Box<dyn std::error::Error>> {
    let range = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), RAM_BYTES)?;
    let map = MachineMemoryMap::new(
        36,
        vec![MemoryRegionSpec::new(range, MemoryRegionKind::Ram)],
    )?;
    let mut memory = PhysicalMemory::new(&map, RamLimits::new(RAM_BYTES))?;
    memory.write_u64(GuestPhysicalAddress::new(ROOT), PDPT | 7, |_| {})?;
    memory.write_u64(GuestPhysicalAddress::new(PDPT), PD_BASE | 7, |_| {})?;
    for page in 0..pages {
        let group = page / 512;
        let pt = PT_BASE + (group as u64) * 0x1000;
        let pd_index = 2 + group as u64;
        memory.write_u64(
            GuestPhysicalAddress::new(PD_BASE + pd_index * 8),
            pt | 7,
            |_| {},
        )?;
        let pt_index = page % 512;
        let leaf = PHYSICAL_BASE + (page as u64) * 0x1000;
        memory.write_u64(
            GuestPhysicalAddress::new(pt + (pt_index as u64) * 8),
            leaf | 7,
            |_| {},
        )?;
    }
    Ok(memory)
}

fn build_profiles() -> Vec<Profile> {
    [1usize, 2, 16, 256, 4096]
        .into_iter()
        .map(|pages| {
            let name = match pages {
                1 => "repeated-page",
                2 => "alternating-pages",
                16 => "small-locality-16-pages",
                256 => "medium-locality-256-pages",
                _ => "large-locality-4096-pages",
            };
            let mut requests = Vec::with_capacity(pages * OFFSETS.len());
            if pages == 2 {
                for offset in OFFSETS {
                    for page in 0..pages {
                        requests.push(request(page, offset));
                    }
                }
            } else {
                for page in 0..pages {
                    for offset in OFFSETS {
                        requests.push(request(page, offset));
                    }
                }
            }
            Profile {
                name,
                pages,
                requests,
            }
        })
        .collect()
}

fn request(page: usize, offset: u64) -> Request {
    Request {
        linear: GuestLinearAddress::new(LINEAR_BASE + (page as u64) * 0x1000 + offset),
        expected_physical: PHYSICAL_BASE + (page as u64) * 0x1000 + offset,
    }
}

fn validate_profile(
    memory: &PhysicalMemory,
    config: &Long64ProbeConfig,
    context: AccessContext,
    profile: &Profile,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = TranslationProbeCache::new(memory);
    for request in &profile.requests {
        let baseline = probe_long64_4k(memory, config, request.linear, AccessKind::Read, context);
        let expected = match baseline.outcome {
            Ok(ProbeOutcome::Translated(translation)) => translation,
            other => return Err(format!("{} baseline failed: {other:?}", profile.name).into()),
        };
        if expected.physical.get() != request.expected_physical {
            return Err(format!(
                "{} wrong literal translation: linear={:#x}, expected={:#x}, actual={:#x}",
                profile.name,
                request.linear.get(),
                request.expected_physical,
                expected.physical.get()
            )
            .into());
        }
        match cache.probe(config, request.linear, AccessKind::Read, context) {
            TranslationProbeCacheOutcome::Hit(actual) => {
                if actual != expected {
                    return Err(format!("{} cache hit diverged from baseline", profile.name).into());
                }
            }
            TranslationProbeCacheOutcome::Walk(report) => match report.outcome {
                Ok(ProbeOutcome::Translated(actual)) if actual == expected => {}
                other => {
                    return Err(format!("{} cache walk diverged: {other:?}", profile.name).into());
                }
            },
        }
    }
    Ok(())
}

fn measure_mode(
    memory: &PhysicalMemory,
    config: &Long64ProbeConfig,
    context: AccessContext,
    profile: &Profile,
    mode: CacheMode,
) -> Result<ModeReport, Box<dyn std::error::Error>> {
    let expected_counts = outcome_counts(profile, mode);
    let observed_counts = observed_outcome_counts(memory, config, context, profile, mode)?;
    if observed_counts != expected_counts {
        return Err(format!(
            "{} {} cache counts differ: expected {expected_counts:?}, observed {observed_counts:?}",
            profile.name,
            mode.name()
        )
        .into());
    }
    let (episodes, calibration_attempts) = calibrate(memory, config, context, profile, mode)?;
    let mut discarded_series = Vec::new();
    let raw_pairs = loop {
        let series = collect_pairs(memory, config, context, profile, mode, episodes)?;
        if series.iter().all(|sample| {
            sample.baseline_ns >= TARGET_BATCH.as_nanos() as u64
                && sample.candidate_ns >= TARGET_BATCH.as_nanos() as u64
        }) {
            break series;
        }
        discarded_series.push(DiscardedSeries {
            episodes,
            pair_count: series.len(),
            reason: "at least one side fell below the 200 ms batch floor; the full paired series was discarded",
            samples: series.clone(),
        });
        let next = episodes
            .checked_mul(2)
            .ok_or("episode count overflow during timing calibration")?;
        return measure_mode_at_count(
            memory,
            config,
            context,
            profile,
            mode,
            next,
            calibration_attempts,
            discarded_series,
        );
    };
    finish_mode(
        profile,
        mode,
        episodes,
        calibration_attempts,
        discarded_series,
        raw_pairs,
    )
}

fn measure_mode_at_count(
    memory: &PhysicalMemory,
    config: &Long64ProbeConfig,
    context: AccessContext,
    profile: &Profile,
    mode: CacheMode,
    episodes: usize,
    mut calibration_attempts: Vec<CalibrationAttempt>,
    mut discarded_series: Vec<DiscardedSeries>,
) -> Result<ModeReport, Box<dyn std::error::Error>> {
    let mut count = episodes;
    loop {
        let (base_ns, candidate_ns) = measure_once(memory, config, context, profile, mode, count)?;
        let accepted = base_ns >= TARGET_BATCH.as_nanos() as u64
            && candidate_ns >= TARGET_BATCH.as_nanos() as u64;
        calibration_attempts.push(CalibrationAttempt {
            episodes: count,
            baseline_ns: base_ns,
            candidate_ns,
            accepted,
        });
        if accepted {
            break;
        }
        count = count.checked_mul(2).ok_or("episode count overflow")?;
    }
    loop {
        let series = collect_pairs(memory, config, context, profile, mode, count)?;
        if series.iter().all(|sample| {
            sample.baseline_ns >= TARGET_BATCH.as_nanos() as u64
                && sample.candidate_ns >= TARGET_BATCH.as_nanos() as u64
        }) {
            return finish_mode(
                profile,
                mode,
                count,
                calibration_attempts,
                discarded_series,
                series,
            );
        }
        discarded_series.push(DiscardedSeries {
            episodes: count,
            pair_count: series.len(),
            reason: "at least one side fell below the 200 ms batch floor; the full paired series was discarded",
            samples: series.clone(),
        });
        count = count.checked_mul(2).ok_or("episode count overflow")?;
        loop {
            let (base_ns, candidate_ns) =
                measure_once(memory, config, context, profile, mode, count)?;
            let accepted = base_ns >= TARGET_BATCH.as_nanos() as u64
                && candidate_ns >= TARGET_BATCH.as_nanos() as u64;
            calibration_attempts.push(CalibrationAttempt {
                episodes: count,
                baseline_ns: base_ns,
                candidate_ns,
                accepted,
            });
            if accepted {
                break;
            }
            count = count.checked_mul(2).ok_or("episode count overflow")?;
        }
    }
}

fn calibrate(
    memory: &PhysicalMemory,
    config: &Long64ProbeConfig,
    context: AccessContext,
    profile: &Profile,
    mode: CacheMode,
) -> Result<(usize, Vec<CalibrationAttempt>), Box<dyn std::error::Error>> {
    let mut episodes = 1usize;
    let mut attempts = Vec::new();
    loop {
        let (baseline_ns, candidate_ns) =
            measure_once(memory, config, context, profile, mode, episodes)?;
        let accepted = baseline_ns >= TARGET_BATCH.as_nanos() as u64
            && candidate_ns >= TARGET_BATCH.as_nanos() as u64;
        attempts.push(CalibrationAttempt {
            episodes,
            baseline_ns,
            candidate_ns,
            accepted,
        });
        if accepted {
            return Ok((episodes, attempts));
        }
        episodes = episodes.checked_mul(2).ok_or("episode count overflow")?;
    }
}

fn measure_once(
    memory: &PhysicalMemory,
    config: &Long64ProbeConfig,
    context: AccessContext,
    profile: &Profile,
    mode: CacheMode,
    episodes: usize,
) -> Result<(u64, u64), Box<dyn std::error::Error>> {
    let mut cache = TranslationProbeCache::new(memory);
    if matches!(mode, CacheMode::Repeated) {
        let _ = run_candidate_episode(&mut cache, profile, config, context, false)?;
    }
    let base_start = Instant::now();
    let _ = run_baseline(memory, profile, config, context, episodes)?;
    let baseline_ns = nanos(base_start.elapsed());
    if matches!(mode, CacheMode::EmptyEpisode) {
        cache.clear();
    }
    let candidate_start = Instant::now();
    let _ = run_candidate(&mut cache, profile, config, context, mode, episodes)?;
    let candidate_ns = nanos(candidate_start.elapsed());
    Ok((baseline_ns, candidate_ns))
}

fn collect_pairs(
    memory: &PhysicalMemory,
    config: &Long64ProbeConfig,
    context: AccessContext,
    profile: &Profile,
    mode: CacheMode,
    episodes: usize,
) -> Result<Vec<PairSample>, Box<dyn std::error::Error>> {
    let mut samples = Vec::with_capacity(SAMPLE_PAIRS);
    for pair in 0..SAMPLE_PAIRS {
        let (baseline_ns, candidate_ns) = if pair % 2 == 0 {
            measure_pair_side(memory, config, context, profile, mode, episodes, true)?
        } else {
            measure_pair_side(memory, config, context, profile, mode, episodes, false)?
        };
        samples.push(PairSample {
            baseline_ns,
            candidate_ns,
            candidate_over_baseline: candidate_ns as f64 / baseline_ns as f64,
            episodes,
        });
    }
    Ok(samples)
}

fn measure_pair_side(
    memory: &PhysicalMemory,
    config: &Long64ProbeConfig,
    context: AccessContext,
    profile: &Profile,
    mode: CacheMode,
    episodes: usize,
    baseline_first: bool,
) -> Result<(u64, u64), Box<dyn std::error::Error>> {
    let mut cache = TranslationProbeCache::new(memory);
    if matches!(mode, CacheMode::Repeated) {
        let _ = run_candidate_episode(&mut cache, profile, config, context, false)?;
    }
    let (baseline_ns, candidate_ns) = if baseline_first {
        let start = Instant::now();
        let _ = run_baseline(memory, profile, config, context, episodes)?;
        let baseline = nanos(start.elapsed());
        if matches!(mode, CacheMode::EmptyEpisode) {
            cache.clear();
        }
        let start = Instant::now();
        let _ = run_candidate(&mut cache, profile, config, context, mode, episodes)?;
        (baseline, nanos(start.elapsed()))
    } else {
        if matches!(mode, CacheMode::EmptyEpisode) {
            cache.clear();
        }
        let start = Instant::now();
        let _ = run_candidate(&mut cache, profile, config, context, mode, episodes)?;
        let candidate = nanos(start.elapsed());
        let start = Instant::now();
        let _ = run_baseline(memory, profile, config, context, episodes)?;
        (nanos(start.elapsed()), candidate)
    };
    Ok((baseline_ns, candidate_ns))
}

fn run_baseline(
    memory: &PhysicalMemory,
    profile: &Profile,
    config: &Long64ProbeConfig,
    context: AccessContext,
    episodes: usize,
) -> Result<u64, Box<dyn std::error::Error>> {
    let mut runner = BaselineRunner {
        memory,
        config,
        context,
        profile_name: profile.name,
    };
    run_timed_episodes(&mut runner, &profile.requests, episodes)
}

fn run_candidate(
    cache: &mut TranslationProbeCache<'_>,
    profile: &Profile,
    config: &Long64ProbeConfig,
    context: AccessContext,
    mode: CacheMode,
    episodes: usize,
) -> Result<u64, Box<dyn std::error::Error>> {
    let mut runner = CandidateRunner {
        cache,
        config,
        context,
        profile_name: profile.name,
        mode,
    };
    run_timed_episodes(&mut runner, &profile.requests, episodes)
}

trait TranslationRunner {
    fn begin_episode(&mut self);
    fn translate(&mut self, request: Request) -> Result<u64, Box<dyn std::error::Error>>;
}

struct BaselineRunner<'a> {
    memory: &'a PhysicalMemory,
    config: &'a Long64ProbeConfig,
    context: AccessContext,
    profile_name: &'static str,
}

impl TranslationRunner for BaselineRunner<'_> {
    fn begin_episode(&mut self) {}

    fn translate(&mut self, request: Request) -> Result<u64, Box<dyn std::error::Error>> {
        let report = probe_long64_4k(
            self.memory,
            self.config,
            request.linear,
            AccessKind::Read,
            self.context,
        );
        match report.outcome {
            Ok(ProbeOutcome::Translated(translation)) => Ok(translation.physical.get()),
            other => Err(format!("{} baseline failed: {other:?}", self.profile_name).into()),
        }
    }
}

struct CandidateRunner<'a, 'memory> {
    cache: &'a mut TranslationProbeCache<'memory>,
    config: &'a Long64ProbeConfig,
    context: AccessContext,
    profile_name: &'static str,
    mode: CacheMode,
}

impl TranslationRunner for CandidateRunner<'_, '_> {
    fn begin_episode(&mut self) {
        if matches!(self.mode, CacheMode::EmptyEpisode) {
            self.cache.clear();
        }
    }

    fn translate(&mut self, request: Request) -> Result<u64, Box<dyn std::error::Error>> {
        match self
            .cache
            .probe(self.config, request.linear, AccessKind::Read, self.context)
        {
            TranslationProbeCacheOutcome::Hit(translation) => Ok(translation.physical.get()),
            TranslationProbeCacheOutcome::Walk(report) => match report.outcome {
                Ok(ProbeOutcome::Translated(translation)) => Ok(translation.physical.get()),
                other => Err(format!("{} candidate failed: {other:?}", self.profile_name).into()),
            },
        }
    }
}

fn run_timed_episodes<R: TranslationRunner>(
    runner: &mut R,
    requests: &[Request],
    episodes: usize,
) -> Result<u64, Box<dyn std::error::Error>> {
    let mut checksum = 0u64;
    for _ in 0..episodes {
        runner.begin_episode();
        for &request in requests {
            checksum = accumulate_translation(checksum, runner.translate(request)?);
        }
        checksum = complete_episode(checksum);
    }
    Ok(black_box(checksum))
}

fn accumulate_translation(checksum: u64, physical: u64) -> u64 {
    checksum.wrapping_add(black_box(physical))
}

fn complete_episode(checksum: u64) -> u64 {
    black_box(checksum)
}

fn observed_outcome_counts(
    memory: &PhysicalMemory,
    config: &Long64ProbeConfig,
    context: AccessContext,
    profile: &Profile,
    mode: CacheMode,
) -> Result<(usize, usize), Box<dyn std::error::Error>> {
    let mut cache = TranslationProbeCache::new(memory);
    if matches!(mode, CacheMode::Repeated) {
        let _ = run_candidate_episode(&mut cache, profile, config, context, false)?;
    }
    if matches!(mode, CacheMode::EmptyEpisode) {
        cache.clear();
    }
    let (_, hits, walks) = run_candidate_episode(&mut cache, profile, config, context, true)?;
    Ok((hits, walks))
}

fn run_candidate_episode(
    cache: &mut TranslationProbeCache<'_>,
    profile: &Profile,
    config: &Long64ProbeConfig,
    context: AccessContext,
    count_outcomes: bool,
) -> Result<(u64, usize, usize), Box<dyn std::error::Error>> {
    let mut checksum = 0u64;
    let mut hits = 0usize;
    let mut walks = 0usize;
    for request in &profile.requests {
        let (translation, hit) =
            match cache.probe(config, request.linear, AccessKind::Read, context) {
                TranslationProbeCacheOutcome::Hit(translation) => (translation, true),
                TranslationProbeCacheOutcome::Walk(report) => match report.outcome {
                    Ok(ProbeOutcome::Translated(translation)) => (translation, false),
                    other => {
                        return Err(format!("{} candidate failed: {other:?}", profile.name).into());
                    }
                },
            };
        if translation.physical.get() != request.expected_physical {
            return Err(
                format!("{} candidate returned wrong physical address", profile.name).into(),
            );
        }
        checksum = accumulate_translation(checksum, translation.physical.get());
        if count_outcomes {
            if hit {
                hits += 1;
            } else {
                walks += 1;
            }
        }
    }
    checksum = complete_episode(checksum);
    Ok((checksum, hits, walks))
}

fn finish_mode(
    profile: &Profile,
    mode: CacheMode,
    episodes: usize,
    calibration_attempts: Vec<CalibrationAttempt>,
    discarded_series: Vec<DiscardedSeries>,
    raw_pairs: Vec<PairSample>,
) -> Result<ModeReport, Box<dyn std::error::Error>> {
    let ratios: Vec<f64> = raw_pairs
        .iter()
        .map(|sample| sample.candidate_over_baseline)
        .collect();
    let (median, min, max, iqr) = quartiles(&ratios);
    let base_samples: Vec<f64> = raw_pairs
        .iter()
        .map(|sample| sample.baseline_ns as f64)
        .collect();
    let candidate_samples: Vec<f64> = raw_pairs
        .iter()
        .map(|sample| sample.candidate_ns as f64)
        .collect();
    let base_median = quartiles(&base_samples).0;
    let candidate_median = quartiles(&candidate_samples).0;
    let requests = profile.requests.len() * episodes;
    let (hit_count_per_episode, walk_count_per_episode) = outcome_counts(profile, mode);
    Ok(ModeReport {
        mode: mode.name(),
        episodes_per_sample: episodes,
        calibration_attempts,
        discarded_series,
        hit_count_per_episode,
        walk_count_per_episode,
        raw_pairs,
        median_candidate_over_baseline: median,
        min_candidate_over_baseline: min,
        max_candidate_over_baseline: max,
        iqr_candidate_over_baseline: iqr,
        baseline_median_ns_per_request: base_median / requests as f64,
        candidate_median_ns_per_request: candidate_median / requests as f64,
        baseline_median_requests_per_second: requests as f64 * 1e9 / base_median,
        candidate_median_requests_per_second: requests as f64 * 1e9 / candidate_median,
    })
}

fn outcome_counts(profile: &Profile, mode: CacheMode) -> (usize, usize) {
    let (hits, walks) = if profile.pages == 2 {
        (0, profile.requests.len())
    } else if profile.pages == 1 && matches!(mode, CacheMode::Repeated) {
        (profile.requests.len(), 0)
    } else {
        (profile.pages * (OFFSETS.len() - 1), profile.pages)
    };
    match mode {
        CacheMode::EmptyEpisode | CacheMode::Repeated => (hits, walks),
    }
}

fn quartiles(values: &[f64]) -> (f64, f64, f64, f64) {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let median = percentile(&sorted, 0.5);
    let q1 = percentile(&sorted, 0.25);
    let q3 = percentile(&sorted, 0.75);
    (median, sorted[0], sorted[sorted.len() - 1], q3 - q1)
}

fn percentile(sorted: &[f64], quantile: f64) -> f64 {
    let index = ((sorted.len() - 1) as f64 * quantile).round() as usize;
    sorted[index]
}

fn nanos(duration: Duration) -> u64 {
    duration.as_nanos().min(u128::from(u64::MAX)) as u64
}

fn hash_memory(memory: &PhysicalMemory) -> Result<String, Box<dyn std::error::Error>> {
    let mut bytes = vec![0; RAM_BYTES as usize];
    memory.read_bytes(GuestPhysicalAddress::new(0), &mut bytes)?;
    Ok(sha256(&bytes))
}

fn hash_profiles(profiles: &[Profile]) -> String {
    let mut bytes = Vec::new();
    for profile in profiles {
        bytes.extend_from_slice(profile.name.as_bytes());
        bytes.push(0);
        for request in &profile.requests {
            bytes.extend_from_slice(&request.linear.get().to_le_bytes());
            bytes.extend_from_slice(&request.expected_physical.to_le_bytes());
        }
    }
    sha256(&bytes)
}

fn sha256(bytes: &[u8]) -> String {
    od_testkit::sha256_hex(bytes)
}

#[cfg(test)]
mod tests {
    use super::{
        BaselineRunner, CacheMode, CandidateRunner, build_memory, build_profiles, outcome_counts,
        request, run_timed_episodes, validate_profile,
    };
    use od_isa::memory::AccessContext;
    use od_machine::Long64ProbeConfig;

    #[test]
    fn fixed_workload_profiles_and_order_are_stable() {
        let profiles = build_profiles();
        assert_eq!(
            profiles
                .iter()
                .map(|profile| profile.pages)
                .collect::<Vec<_>>(),
            [1, 2, 16, 256, 4096]
        );
        assert_eq!(
            profiles[0].requests,
            [
                request(0, 0),
                request(0, 8),
                request(0, 64),
                request(0, 4088)
            ]
        );
        assert_eq!(
            profiles[1].requests,
            [
                request(0, 0),
                request(1, 0),
                request(0, 8),
                request(1, 8),
                request(0, 64),
                request(1, 64),
                request(0, 4088),
                request(1, 4088),
            ]
        );
        assert_eq!(profiles[4].requests.len(), 16_384);
        assert_eq!(profiles[4].requests[16_383], request(4095, 4088));
        assert_eq!(
            outcome_counts(&profiles[0], CacheMode::EmptyEpisode),
            (3, 1)
        );
        assert_eq!(outcome_counts(&profiles[0], CacheMode::Repeated), (4, 0));
        assert_eq!(outcome_counts(&profiles[1], CacheMode::Repeated), (0, 8));
        assert_eq!(
            outcome_counts(&profiles[2], CacheMode::EmptyEpisode),
            (48, 16)
        );
    }

    #[test]
    fn synthetic_fixture_matches_literal_addresses_for_all_profiles() {
        let memory = build_memory(4096).unwrap();
        let config = Long64ProbeConfig::new(0x8001_0011, 0x1000, 0x20, 0xd00).unwrap();
        let profiles = build_profiles();
        for profile in &profiles {
            validate_profile(&memory, &config, AccessContext::default(), profile).unwrap();
        }
    }

    #[test]
    fn timed_baseline_and_candidate_share_the_same_checksum_sequence() {
        let memory = build_memory(1).unwrap();
        let config = Long64ProbeConfig::new(0x8001_0011, 0x1000, 0x20, 0xd00).unwrap();
        let context = AccessContext::default();
        let profile = &build_profiles()[0];
        validate_profile(&memory, &config, context, profile).unwrap();

        let mut baseline = BaselineRunner {
            memory: &memory,
            config: &config,
            context,
            profile_name: profile.name,
        };
        let mut baseline_cache = od_machine::TranslationProbeCache::new(&memory);
        let mut candidate = CandidateRunner {
            cache: &mut baseline_cache,
            config: &config,
            context,
            profile_name: profile.name,
            mode: CacheMode::EmptyEpisode,
        };
        let baseline_checksum = run_timed_episodes(&mut baseline, &profile.requests, 3).unwrap();
        let candidate_checksum = run_timed_episodes(&mut candidate, &profile.requests, 3).unwrap();

        assert_eq!(
            baseline_checksum,
            3 * (0x0100_0000 + 0x0100_0008 + 0x0100_0040 + 0x0100_0ff8)
        );
        assert_eq!(candidate_checksum, baseline_checksum);
    }
}
