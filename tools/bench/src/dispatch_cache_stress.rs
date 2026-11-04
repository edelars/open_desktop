//! Sustained FIFO16 lifecycle workload for the S3-37d resource-correctness slice.
//! This deliberately reuses the existing cache/arena implementation and records
//! bounded aggregates rather than a row for every dispatch.

use super::*;
use serde::{Deserialize, Serialize};

const PROFILE: &str = "s3-37d-fifo16-mixed-lifecycle-v5";
const CALIBRATION_WARMUP_EPOCHS: usize = 512;
const CALIBRATION_MEASUREMENT_BATCH_EPOCHS: usize = 65_536;
const CALIBRATION_MEASUREMENT_BATCHES: usize = 8;
const CALIBRATION_EPOCHS: usize = CALIBRATION_WARMUP_EPOCHS
    + CALIBRATION_MEASUREMENT_BATCH_EPOCHS * CALIBRATION_MEASUREMENT_BATCHES;
const TARGET_STRESS_NS: u128 = 11 * 60 * 1_000_000_000;
const MAX_EPOCHS: usize = 50_000_000;
const CLEAR_INTERVAL: usize = 128;
const REOPEN_INTERVAL: usize = 512;
const MAX_CHECKPOINTS: usize = 128;
const RUN_TIME_MARGIN: Duration = Duration::from_secs(60);
const CALIBRATION_SCHEDULE: &str = "same mixed hit/miss/invalidate/recompile/hit sequence as scored profile; one retained runtime; 512 warm-up epochs followed by eight 65,536-epoch measurement batches; 128 bounded checkpoints across all calibration epochs; 128 epoch clear and 512 epoch reopen cadence; use the fastest measured batch rate for target sizing; reject an epoch cap that cannot meet the target duration";
const DETERMINISTIC_SCHEDULE: &str = "per epoch: hit active key; compile unique version of pressure key and verify one FIFO eviction; invalidate active backing between calls and verify one removal; execute alternate admitted bytes under advanced backing version and verify one successful compile/publication without eviction; repeat and verify hit; alternate original/changed bytes on successive epochs. Every 128 epochs clear and assert zero current resident entries/code/metadata plus all 16 arena slots, then refill. Every 512 epochs clear, drop the sole runtime/arena, reopen one runtime, execute after reopen with a bumped execution context, and refill. No more than one MAP_JIT arena is live.";

const STRESS_SOURCE: &[u8] = include_bytes!("dispatch_cache_stress.rs");
const DISPATCH_CACHE_SOURCE: &[u8] = include_bytes!("dispatch_cache.rs");
const BENCH_LIBRARY_SOURCE: &[u8] = include_bytes!("lib.rs");

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
struct Totals {
    hits: u64,
    misses: u64,
    successful_compiles: u64,
    successful_publications: u64,
    evictions: u64,
    dispatches: u64,
    blocks_executed: u64,
    retired_guest_instructions: u64,
    epoch_body_elapsed_ns: u128,
    checkpoint_elapsed_ns: u128,
    checkpoint_count: usize,
    pressure_miss_dispatch_elapsed_ns: u128,
    version_recompile_dispatch_elapsed_ns: u128,
    repeated_hit_dispatch_elapsed_ns: u128,
    invalidations: u64,
    invalidated_entries: u64,
    clears: u64,
    reopens: u64,
    dispatch_elapsed_ns: u128,
    invalidation_elapsed_ns: u128,
    clear_elapsed_ns: u128,
    teardown_elapsed_ns: u128,
    reopen_elapsed_ns: u128,
}

#[derive(Clone, Debug, Serialize)]
struct Checkpoint {
    completed_epochs: usize,
    action: &'static str,
    totals: Totals,
    cache: LifecycleCacheSnapshot,
    rss: ProcessRssSnapshot,
    elapsed_ns: u128,
}

#[derive(Serialize)]
struct StressReport {
    schema: u32,
    profile: &'static str,
    status: &'static str,
    process_run: u8,
    case_list_sha256: String,
    trace_sha256: String,
    build_source_sha256: String,
    calibration_sha256: Option<String>,
    capture_host: HostMetadata,
    measure_host: HostMetadata,
    cache_capacity_entries: usize,
    epochs_requested: usize,
    epochs_completed: usize,
    clear_interval_epochs: usize,
    reopen_interval_epochs: usize,
    deterministic_schedule: &'static str,
    parity_contract: &'static str,
    limitations: Vec<String>,
    cumulative_totals: Totals,
    current_cache: LifecycleCacheSnapshot,
    peak_resident_entries: usize,
    peak_resident_code_bytes: usize,
    peak_resident_logical_metadata_bytes: usize,
    minimum_available_slots_at_full_occupancy: usize,
    arena_reserved_mapping_bytes_high_water: usize,
    process_rss_before_runtime: ProcessRssSnapshot,
    process_rss_after_stress_before_drop: ProcessRssSnapshot,
    process_rss_after_drop: ProcessRssSnapshot,
    checkpoints: Vec<Checkpoint>,
    target_elapsed_ns: Option<u128>,
    duration_target_met: Option<bool>,
    elapsed_ns: u128,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Calibration {
    schema: u32,
    profile: String,
    case_list_sha256: String,
    trace_sha256: String,
    calibration_epochs: usize,
    calibration_warmup_epochs: usize,
    calibration_measurement_batch_epochs: usize,
    calibration_measurement_batches: usize,
    calibration_batch_body_elapsed_ns: Vec<u128>,
    calibration_warmup_body_elapsed_ns: u128,
    calibration_elapsed_ns: u128,
    calibration_epoch_body_elapsed_ns: u128,
    calibration_checkpoint_elapsed_ns: u128,
    calibration_checkpoint_count: usize,
    calibration_fixed_elapsed_ns: u128,
    estimated_epoch_ns: u128,
    target_stress_ns: u128,
    recommended_epochs: usize,
    upper_bound_elapsed_ns: u128,
    max_run_ns: u128,
    build_source_sha256: String,
    calibration_integrity_sha256: String,
    capture_host: HostMetadata,
    measure_host: HostMetadata,
    schedule: String,
}

struct Prepared {
    list: Vec<u8>,
    trace_raw: Vec<u8>,
    trace: RecordedTrace,
    cases: Vec<Case>,
    fillers: Vec<usize>,
    target: usize,
    pressure: usize,
    alternate_bytes: Vec<u8>,
    alternate_admission: AdmittedInstructionBoundaries,
    measure_host: HostMetadata,
}

fn add(left: u64, right: u64) -> Result<u64, BenchError> {
    left.checked_add(right)
        .ok_or_else(|| err("stress counter overflow"))
}

fn add_time(left: u128, right: u128) -> Result<u128, BenchError> {
    left.checked_add(right)
        .ok_or_else(|| err("stress elapsed-time counter overflow"))
}

fn check_deadline(started: Instant, stage: &str) -> Result<(), BenchError> {
    if started.elapsed() > MAX_RUN_TIME {
        return Err(err(format!(
            "30 minute process deadline exceeded during {stage}"
        )));
    }
    Ok(())
}

fn counter_delta(after: u64, before: u64, label: &str) -> Result<u64, BenchError> {
    after
        .checked_sub(before)
        .ok_or_else(|| err(format!("FIFO counter regressed during {label}")))
}

fn recommend_epochs(
    target_ns: u128,
    checkpoint_ns: u128,
    fixed_ns: u128,
    epoch_ns: u128,
) -> Result<(usize, u128), BenchError> {
    if epoch_ns == 0 {
        return Err(err("calibration estimated zero time per epoch"));
    }
    let available = target_ns
        .checked_sub(checkpoint_ns)
        .and_then(|value| value.checked_sub(fixed_ns))
        .ok_or_else(|| err("calibration fixed costs exceed target duration"))?;
    let uncapped = available.div_ceil(epoch_ns);
    let recommended = usize::try_from(uncapped)
        .unwrap_or(MAX_EPOCHS)
        .clamp(1, MAX_EPOCHS);
    let projected = fixed_ns
        .checked_add(checkpoint_ns)
        .and_then(|value| value.checked_add(epoch_ns.checked_mul(recommended as u128)?))
        .ok_or_else(|| err("calibration projection overflow"))?;
    if projected < target_ns {
        return Err(err(
            "epoch cap cannot meet target duration; increase the cap and recalibrate",
        ));
    }
    if projected > (MAX_RUN_TIME - RUN_TIME_MARGIN).as_nanos() {
        return Err(err(
            "calibrated stress exceeds the 30 minute process deadline",
        ));
    }
    Ok((recommended, projected))
}

fn fastest_batch_epoch_ns(
    batch_body_elapsed_ns: &[u128],
    batch_epochs: usize,
) -> Result<u128, BenchError> {
    if batch_epochs == 0 || batch_body_elapsed_ns.len() != CALIBRATION_MEASUREMENT_BATCHES {
        return Err(err("calibration measurement batch shape is invalid"));
    }
    let fastest_batch = batch_body_elapsed_ns
        .iter()
        .copied()
        .min()
        .ok_or_else(|| err("calibration has no measurement batches"))?;
    let per_epoch = fastest_batch / batch_epochs as u128;
    if per_epoch == 0 {
        return Err(err("calibration fastest batch has zero time per epoch"));
    }
    Ok(per_epoch)
}

fn duration_target_met(target_elapsed_ns: Option<u128>, elapsed_ns: u128) -> Option<bool> {
    target_elapsed_ns.map(|target| elapsed_ns >= target)
}

fn implementation_sha256() -> String {
    let mut sources = Vec::with_capacity(
        STRESS_SOURCE.len() + DISPATCH_CACHE_SOURCE.len() + BENCH_LIBRARY_SOURCE.len(),
    );
    sources.extend_from_slice(STRESS_SOURCE);
    sources.extend_from_slice(DISPATCH_CACHE_SOURCE);
    sources.extend_from_slice(BENCH_LIBRARY_SOURCE);
    od_testkit::sha256_hex(&sources)
}

fn prepare(case_list: &Path, trace_path: &Path, started: Instant) -> Result<Prepared, BenchError> {
    check_deadline(started, "input preparation")?;
    let list = read_bounded(case_list, MAX_CASE_LIST_BYTES, "case list")?;
    check_deadline(started, "case-list read")?;
    let trace_raw = read_bounded(trace_path, crate::MAX_LOGICAL_BYTES, "trace")?;
    check_deadline(started, "trace read")?;
    if od_testkit::sha256_hex(&list) != FROZEN_CASE_LIST_SHA256
        || od_testkit::sha256_hex(&trace_raw) != FROZEN_TRACE_SHA256
    {
        return Err(err(
            "case list or trace does not match the frozen S3-17 corpus",
        ));
    }
    let trace: RecordedTrace =
        serde_json::from_slice(&trace_raw).map_err(|e| err(format!("trace JSON: {e}")))?;
    if trace.inputs.len() != MAX_INPUTS || trace.visits.len() != 1_322 {
        return Err(err("frozen S3-17 trace count mismatch"));
    }
    let cases = prepare_cases(&list, &trace, started)?;
    check_deadline(started, "fixture admission and interpreter preparation")?;
    let eligible: Vec<usize> = cases
        .iter()
        .enumerate()
        .filter_map(|(index, case)| {
            (case.entry_count == 1 && case.unique_entries == 1 && case.initial.rip == 0)
                .then_some(index)
        })
        .collect();
    if eligible.len() < ARENA_SLOTS + 1 {
        return Err(err(
            "stress profile needs 17 admitted one-entry cases at RIP zero",
        ));
    }
    let target = eligible[ARENA_SLOTS - 1];
    let pressure = eligible[ARENA_SLOTS];
    let target_case = &cases[target];
    let mut alternate = None;
    for candidate in cases.iter().filter(|case| {
        case.entry_count == 1
            && case.unique_entries == 1
            && case.initial.rip == 0
            && case.bytes != target_case.bytes
    }) {
        let changed = lifecycle_case_with_program(
            candidate,
            &target_case.initial,
            target_case.budget,
            target_case.identity,
            "s3-37d-alternate-oracle".into(),
        )?;
        if lifecycle_results_differ(target_case, &changed) {
            alternate = Some((candidate, changed));
            break;
        }
    }
    let (alternate, _changed) = alternate
        .ok_or_else(|| err("no distinct admitted alternate program in frozen S3-17 corpus"))?;
    let code = CodeView::new(0, &alternate.bytes).map_err(|e| err(format!("code view: {e:?}")))?;
    let decoded = od_isa::decode(&alternate.bytes).map_err(|e| err(format!("decode: {e:?}")))?;
    let alternate_admission = AdmittedInstructionBoundaries::from_fixture_decode(code, &decoded)
        .map_err(|e| err(format!("admission: {e:?}")))?;
    let alternate_bytes = alternate.bytes.clone();
    check_deadline(started, "host metadata")?;
    let measure_host = HostMetadata::current();
    check_deadline(started, "host metadata subprocesses")?;
    Ok(Prepared {
        list,
        trace_raw,
        trace,
        cases,
        fillers: eligible[..ARENA_SLOTS - 1].to_vec(),
        target,
        pressure,
        alternate_bytes,
        alternate_admission,
        measure_host,
    })
}

fn fill(
    prepared: &Prepared,
    engine: &mut Variant,
    active: &Case,
    context: ExecutionContextSnapshot,
    totals: &mut Totals,
    peak_entries: &mut usize,
    peak_code_bytes: &mut usize,
    peak_metadata_bytes: &mut usize,
    min_free_slots: &mut usize,
    arena_bytes: &mut usize,
) -> Result<(), BenchError> {
    let before = engine.fifo_lifecycle_snapshot()?;
    for &index in &prepared.fillers {
        let _ = dispatch(
            engine,
            &prepared.cases[index],
            prepared.cases[index].identity,
            context,
            totals,
        )?;
    }
    let _ = dispatch(engine, active, active.identity, context, totals)?;
    let snapshot = engine.fifo_lifecycle_snapshot()?;
    let fill_misses = counter_delta(snapshot.misses, before.misses, "FIFO refill")?;
    let fill_compiles = counter_delta(
        snapshot.successful_compiles,
        before.successful_compiles,
        "FIFO refill",
    )?;
    let fill_publications = counter_delta(
        snapshot.successful_publications,
        before.successful_publications,
        "FIFO refill",
    )?;
    let fill_hits = counter_delta(snapshot.hits, before.hits, "FIFO refill")?;
    let fill_evictions = counter_delta(snapshot.evictions, before.evictions, "FIFO refill")?;
    account_peak(
        snapshot,
        peak_entries,
        peak_code_bytes,
        peak_metadata_bytes,
        min_free_slots,
        arena_bytes,
    );
    if snapshot.resident_entries_current != ARENA_SLOTS
        || snapshot.arena_available_slots_actual != 0
        || fill_misses != ARENA_SLOTS as u64
        || fill_compiles != ARENA_SLOTS as u64
        || fill_publications != ARENA_SLOTS as u64
        || fill_hits != 0
        || fill_evictions != 0
    {
        return Err(err("stress refill did not occupy all 16 FIFO slots"));
    }
    Ok(())
}

fn account_peak(
    snapshot: LifecycleCacheSnapshot,
    entries: &mut usize,
    code: &mut usize,
    metadata: &mut usize,
    free: &mut usize,
    arena: &mut usize,
) {
    *entries = (*entries).max(snapshot.resident_entries_high_water);
    *code = (*code).max(snapshot.resident_code_bytes_high_water);
    *metadata = (*metadata).max(snapshot.resident_logical_metadata_bytes_high_water);
    if snapshot.resident_entries_current == ARENA_SLOTS {
        *free = (*free).min(snapshot.arena_available_slots_actual);
    }
    *arena = (*arena).max(snapshot.arena_reserved_mapping_bytes);
}

fn dispatch(
    engine: &mut Variant,
    case: &Case,
    identity: CodeIdentity,
    context: ExecutionContextSnapshot,
    totals: &mut Totals,
) -> Result<Duration, BenchError> {
    let before = engine.fifo_lifecycle_snapshot()?;
    let mut state = case.initial.clone();
    let started = Instant::now();
    let result = engine.call_fifo_with_identity_and_context(case, &mut state, identity, context)?;
    let elapsed = started.elapsed();
    totals.dispatch_elapsed_ns = add_time(totals.dispatch_elapsed_ns, elapsed.as_nanos())?;
    validate_full_dispatch_state(case, &state)?;
    validate_dispatch_result(case, &state, &result)?;
    let after = engine.fifo_lifecycle_snapshot()?;
    let hits = counter_delta(after.hits, before.hits, "dispatch")?;
    let misses = counter_delta(after.misses, before.misses, "dispatch")?;
    let compiles = counter_delta(
        after.successful_compiles,
        before.successful_compiles,
        "dispatch",
    )?;
    let publications = counter_delta(
        after.successful_publications,
        before.successful_publications,
        "dispatch",
    )?;
    let evictions = counter_delta(after.evictions, before.evictions, "dispatch")?;
    if (hits, misses) == (1, 0) {
        if compiles != 0 || publications != 0 || evictions != 0 {
            return Err(err(
                "FIFO hit changed compile/publication/eviction counters",
            ));
        }
    } else if (hits, misses) == (0, 1) {
        if compiles != 1 || publications != 1 || evictions > 1 {
            return Err(err("FIFO miss did not compile and publish exactly once"));
        }
    } else {
        return Err(err("FIFO dispatch did not record exactly one hit or miss"));
    }
    totals.hits = add(totals.hits, hits)?;
    totals.misses = add(totals.misses, misses)?;
    totals.successful_compiles = add(totals.successful_compiles, compiles)?;
    totals.successful_publications = add(totals.successful_publications, publications)?;
    totals.evictions = add(totals.evictions, evictions)?;
    totals.dispatches = add(totals.dispatches, 1)?;
    totals.blocks_executed = add(totals.blocks_executed, result.blocks_executed)?;
    totals.retired_guest_instructions = add(
        totals.retired_guest_instructions,
        result.retired_instructions,
    )?;
    Ok(elapsed)
}

fn versioned_case(
    prepared: &Prepared,
    alternate: bool,
    identity: CodeIdentity,
) -> Result<Case, BenchError> {
    let template = &prepared.cases[prepared.target];
    let bytes = if alternate {
        &prepared.alternate_bytes
    } else {
        &template.bytes
    };
    let admission = if alternate {
        prepared.alternate_admission.clone()
    } else {
        template.admission.clone()
    };
    let mut case = lifecycle_case_with_program(
        template,
        &template.initial,
        template.budget,
        identity,
        format!("s3-37d-epoch-{}", identity.backing_version.get()),
    )?;
    case.bytes = bytes.clone();
    case.admission = admission;
    // lifecycle_case_with_program computes the interpreter oracle for the
    // target bytes; recompute for the selected alternate before dispatch.
    if alternate {
        let view = CodeView::new(0, &case.bytes).map_err(|e| err(format!("code view: {e:?}")))?;
        let mut expected = case.initial.clone();
        let mut memory = FetchMemory(&case.bytes);
        let result = interpreter_run(
            &mut expected,
            &mut memory,
            RunConfig {
                instruction_budget: case.budget,
                stop_rip: Some(view.code_end()),
                context: AccessContext::default(),
            },
            || false,
        );
        if result.instructions_executed > case.budget {
            return Err(err("stress interpreter exceeded instruction budget"));
        }
        case.expected = expected;
        case.exit = match result.exit {
            RunExit::StopAddress => DispatchExit::CodeEnd,
            RunExit::InstructionBudgetExhausted => DispatchExit::BudgetExhausted,
            _ => return Err(err("stress alternate interpreter exit was unexpected")),
        };
        case.retired = result.instructions_executed;
        case.remaining = case.budget.saturating_sub(result.instructions_executed);
    }
    Ok(case)
}

fn empty_cache(snapshot: LifecycleCacheSnapshot) -> bool {
    snapshot.resident_entries_current == 0
        && snapshot.resident_code_bytes_current == 0
        && snapshot.resident_logical_metadata_bytes_current == 0
        && snapshot.arena_available_slots_actual == ARENA_SLOTS
}

fn reconcile_totals(epochs: usize, totals: Totals) -> Result<(), BenchError> {
    let epoch_count = u64::try_from(epochs).map_err(|_| err("epoch count overflow"))?;
    let clear_count =
        u64::try_from(epochs / CLEAR_INTERVAL).map_err(|_| err("clear count overflow"))?;
    let fill_count = clear_count
        .checked_add(1)
        .ok_or_else(|| err("fill count overflow"))?;
    let fill_dispatches = fill_count
        .checked_mul(ARENA_SLOTS as u64)
        .ok_or_else(|| err("fill dispatch count overflow"))?;
    let expected_hits = epoch_count
        .checked_mul(2)
        .ok_or_else(|| err("expected hit count overflow"))?;
    let epoch_misses = epoch_count
        .checked_mul(2)
        .ok_or_else(|| err("epoch miss count overflow"))?;
    let expected_misses = fill_dispatches
        .checked_add(epoch_misses)
        .ok_or_else(|| err("expected miss count overflow"))?;
    let expected_dispatches = fill_dispatches
        .checked_add(
            epoch_count
                .checked_mul(4)
                .ok_or_else(|| err("expected dispatch count overflow"))?,
        )
        .ok_or_else(|| err("expected dispatch count overflow"))?;
    if totals.hits != expected_hits
        || totals.misses != expected_misses
        || totals.successful_compiles != expected_misses
        || totals.successful_publications != expected_misses
        || totals.evictions != epoch_count
        || totals.dispatches != expected_dispatches
        || totals.invalidations != epoch_count
        || totals.invalidated_entries != epoch_count
        || totals.clears != clear_count
        || totals.reopens
            != u64::try_from(epochs / REOPEN_INTERVAL).map_err(|_| err("reopen count overflow"))?
    {
        return Err(err(
            "final mixed-lifecycle totals do not match the deterministic epoch schedule",
        ));
    }
    Ok(())
}

fn run_epochs(
    prepared: &Prepared,
    epochs: usize,
    process_run: u8,
    calibration_sha256: Option<String>,
    output_path: Option<&Path>,
    process_started: Instant,
    target_elapsed_ns: Option<u128>,
    calibration_batch_body_elapsed_ns: Option<&mut Vec<u128>>,
) -> Result<serde_json::Value, BenchError> {
    if cfg!(debug_assertions) {
        return Err(err("cache-lifecycle-stress requires a release build"));
    }
    if !(1..=3).contains(&process_run) || epochs == 0 || epochs > MAX_EPOCHS {
        return Err(err(
            "stress requires process-run 1..=3 and epochs 1..=50,000,000",
        ));
    }
    check_deadline(process_started, "runtime setup")?;
    let rss_before = process_rss_snapshot();
    check_deadline(process_started, "initial RSS snapshot")?;
    let mut engine = Variant::new(VariantName::MultiFifo16)?;
    check_deadline(process_started, "runtime creation")?;
    let mut context = crate::benchmark_execution_context();
    let target = &prepared.cases[prepared.target];
    let mut totals = Totals::default();
    let mut peak_entries = 0;
    let mut peak_code_bytes = 0;
    let mut peak_metadata_bytes = 0;
    let mut min_free_slots = ARENA_SLOTS;
    let mut arena_bytes = 0;
    let mut checkpoints = Vec::with_capacity(MAX_CHECKPOINTS);
    let mut calibration_batch_body_elapsed_ns = calibration_batch_body_elapsed_ns;
    let mut previous_calibration_batch_body_ns = 0u128;
    let checkpoint_count = epochs.min(MAX_CHECKPOINTS);
    let checkpoint_epochs: Vec<usize> = (1..=checkpoint_count)
        .map(|index| index.saturating_mul(epochs).div_ceil(checkpoint_count))
        .collect();
    let mut next_checkpoint = 0usize;
    let mut active_identity = target.identity;
    let mut active_alternate = false;
    let mut active_case = versioned_case(prepared, false, active_identity)?;
    fill(
        prepared,
        &mut engine,
        &active_case,
        context,
        &mut totals,
        &mut peak_entries,
        &mut peak_code_bytes,
        &mut peak_metadata_bytes,
        &mut min_free_slots,
        &mut arena_bytes,
    )?;
    check_deadline(process_started, "initial FIFO refill")?;
    for epoch in 1..=epochs {
        check_deadline(process_started, "epoch execution")?;
        let epoch_started = Instant::now();
        // A repeated active key is a real cache hit.
        let before_epoch_hit = engine.fifo_lifecycle_snapshot()?;
        let epoch_hit_elapsed = dispatch(
            &mut engine,
            &active_case,
            active_identity,
            context,
            &mut totals,
        )?;
        check_deadline(process_started, "active hit dispatch")?;
        totals.repeated_hit_dispatch_elapsed_ns = add_time(
            totals.repeated_hit_dispatch_elapsed_ns,
            epoch_hit_elapsed.as_nanos(),
        )?;
        let after_epoch_hit = engine.fifo_lifecycle_snapshot()?;
        if counter_delta(
            after_epoch_hit.hits,
            before_epoch_hit.hits,
            "epoch active hit",
        )? != 1
            || counter_delta(
                after_epoch_hit.misses,
                before_epoch_hit.misses,
                "epoch active hit",
            )? != 0
            || counter_delta(
                after_epoch_hit.successful_compiles,
                before_epoch_hit.successful_compiles,
                "epoch active hit",
            )? != 0
            || counter_delta(
                after_epoch_hit.successful_publications,
                before_epoch_hit.successful_publications,
                "epoch active hit",
            )? != 0
            || counter_delta(
                after_epoch_hit.evictions,
                before_epoch_hit.evictions,
                "epoch active hit",
            )? != 0
        {
            return Err(err(format!(
                "epoch {epoch}: active key call was not hit-only"
            )));
        }

        // Every epoch advances the pressure key version, forcing a miss and a
        // single FIFO eviction while keeping one arena alive across epochs.
        let pressure_base = &prepared.cases[prepared.pressure];
        let mut pressure_identity = pressure_base.identity;
        let pressure_version = pressure_base
            .identity
            .backing_version
            .get()
            .checked_add(u64::try_from(epoch).map_err(|_| err("epoch overflow"))?)
            .and_then(NonZeroU64::new)
            .ok_or_else(|| err("pressure backing version exhausted"))?;
        pressure_identity.backing_version = BackingVersion::new(pressure_version);
        let before_pressure = engine.fifo_lifecycle_snapshot()?;
        let pressure_elapsed = dispatch(
            &mut engine,
            pressure_base,
            pressure_identity,
            context,
            &mut totals,
        )?;
        check_deadline(process_started, "pressure miss dispatch")?;
        totals.pressure_miss_dispatch_elapsed_ns = add_time(
            totals.pressure_miss_dispatch_elapsed_ns,
            pressure_elapsed.as_nanos(),
        )?;
        let after_pressure = engine.fifo_lifecycle_snapshot()?;
        if after_pressure.misses != before_pressure.misses + 1
            || after_pressure.successful_compiles != before_pressure.successful_compiles + 1
            || after_pressure.successful_publications != before_pressure.successful_publications + 1
            || after_pressure.evictions != before_pressure.evictions + 1
            || after_pressure.resident_entries_current != ARENA_SLOTS
        {
            return Err(err(format!(
                "epoch {epoch}: pressure miss did not evict exactly one FIFO entry"
            )));
        }
        account_peak(
            after_pressure,
            &mut peak_entries,
            &mut peak_code_bytes,
            &mut peak_metadata_bytes,
            &mut min_free_slots,
            &mut arena_bytes,
        );

        let next_version = active_identity
            .backing_version
            .checked_next()
            .ok_or_else(|| err("target backing version exhausted"))?;
        let before_invalidation = engine.fifo_lifecycle_snapshot()?;
        let invalidate_started = Instant::now();
        let removed = engine.fifo_invalidate_backing(active_identity.backing, next_version)?;
        check_deadline(process_started, "backing invalidation")?;
        totals.invalidation_elapsed_ns = add_time(
            totals.invalidation_elapsed_ns,
            invalidate_started.elapsed().as_nanos(),
        )?;
        totals.invalidations = add(totals.invalidations, 1)?;
        totals.invalidated_entries = add(totals.invalidated_entries, removed as u64)?;
        let after_invalidation = engine.fifo_lifecycle_snapshot()?;
        if removed != 1
            || after_invalidation.resident_entries_current != ARENA_SLOTS - 1
            || counter_delta(
                after_invalidation.hits,
                before_invalidation.hits,
                "invalidation",
            )? != 0
            || counter_delta(
                after_invalidation.misses,
                before_invalidation.misses,
                "invalidation",
            )? != 0
            || counter_delta(
                after_invalidation.successful_compiles,
                before_invalidation.successful_compiles,
                "invalidation",
            )? != 0
            || counter_delta(
                after_invalidation.successful_publications,
                before_invalidation.successful_publications,
                "invalidation",
            )? != 0
            || counter_delta(
                after_invalidation.evictions,
                before_invalidation.evictions,
                "invalidation",
            )? != 0
        {
            return Err(err(format!(
                "epoch {epoch}: between-call invalidation did not remove one active entry"
            )));
        }
        active_identity.backing_version = next_version;
        active_alternate = !active_alternate;
        active_case = versioned_case(prepared, active_alternate, active_identity)?;
        if active_alternate != lifecycle_results_differ(target, &active_case) {
            return Err(err(
                "versioned code did not preserve the expected distinct interpreter result",
            ));
        }
        let before_compile = engine.fifo_lifecycle_snapshot()?;
        let compile_elapsed = dispatch(
            &mut engine,
            &active_case,
            active_identity,
            context,
            &mut totals,
        )?;
        check_deadline(process_started, "versioned code dispatch")?;
        totals.version_recompile_dispatch_elapsed_ns = add_time(
            totals.version_recompile_dispatch_elapsed_ns,
            compile_elapsed.as_nanos(),
        )?;
        let after_compile = engine.fifo_lifecycle_snapshot()?;
        if after_compile.misses != before_compile.misses + 1
            || after_compile.successful_compiles != before_compile.successful_compiles + 1
            || after_compile.successful_publications != before_compile.successful_publications + 1
            || after_compile.evictions != before_compile.evictions
            || after_compile.resident_entries_current != ARENA_SLOTS
        {
            return Err(err(format!(
                "epoch {epoch}: new backing/code version did not recompile without eviction"
            )));
        }
        let repeated_version_hit_elapsed = dispatch(
            &mut engine,
            &active_case,
            active_identity,
            context,
            &mut totals,
        )?;
        totals.repeated_hit_dispatch_elapsed_ns = add_time(
            totals.repeated_hit_dispatch_elapsed_ns,
            repeated_version_hit_elapsed.as_nanos(),
        )?;
        let after_hit = engine.fifo_lifecycle_snapshot()?;
        if after_hit.hits != after_compile.hits + 1
            || after_hit.misses != after_compile.misses
            || after_hit.successful_compiles != after_compile.successful_compiles
            || after_hit.successful_publications != after_compile.successful_publications
            || after_hit.evictions != after_compile.evictions
        {
            return Err(err(format!(
                "epoch {epoch}: recompiled code was not hit on the next dispatch"
            )));
        }
        account_peak(
            after_hit,
            &mut peak_entries,
            &mut peak_code_bytes,
            &mut peak_metadata_bytes,
            &mut min_free_slots,
            &mut arena_bytes,
        );

        let is_reopen = epoch % REOPEN_INTERVAL == 0;
        let is_clear = epoch % CLEAR_INTERVAL == 0;
        if is_clear {
            let before_clear = engine.fifo_lifecycle_snapshot()?;
            let clear_started = Instant::now();
            engine.fifo_clear()?;
            check_deadline(process_started, "cache clear")?;
            totals.clear_elapsed_ns =
                add_time(totals.clear_elapsed_ns, clear_started.elapsed().as_nanos())?;
            totals.clears = add(totals.clears, 1)?;
            let after_clear = engine.fifo_lifecycle_snapshot()?;
            if !empty_cache(after_clear) || after_clear.resident_entries_high_water != ARENA_SLOTS {
                return Err(err(format!(
                    "epoch {epoch}: periodic clear did not return zero resident usage and 16 slots"
                )));
            }
            if counter_delta(after_clear.hits, before_clear.hits, "clear")? != 0
                || counter_delta(after_clear.misses, before_clear.misses, "clear")? != 0
                || counter_delta(
                    after_clear.successful_compiles,
                    before_clear.successful_compiles,
                    "clear",
                )? != 0
                || counter_delta(
                    after_clear.successful_publications,
                    before_clear.successful_publications,
                    "clear",
                )? != 0
                || counter_delta(after_clear.evictions, before_clear.evictions, "clear")? != 0
            {
                return Err(err(format!(
                    "epoch {epoch}: FIFO clear changed lifetime cache counters"
                )));
            }
            if is_reopen {
                let drop_started = Instant::now();
                drop(engine);
                check_deadline(process_started, "runtime drop")?;
                totals.teardown_elapsed_ns = add_time(
                    totals.teardown_elapsed_ns,
                    drop_started.elapsed().as_nanos(),
                )?;
                let reopen_started = Instant::now();
                engine = Variant::new(VariantName::MultiFifo16)?;
                check_deadline(process_started, "runtime reopen")?;
                totals.reopen_elapsed_ns = add_time(
                    totals.reopen_elapsed_ns,
                    reopen_started.elapsed().as_nanos(),
                )?;
                totals.reopens = add(totals.reopens, 1)?;
                context = ExecutionContextSnapshot::s0_09_fixture(
                    context
                        .id
                        .checked_next()
                        .ok_or_else(|| err("context generation exhausted"))?,
                );
                let fresh = engine.fifo_lifecycle_snapshot()?;
                if !empty_cache(fresh) {
                    return Err(err("reopened runtime was not empty"));
                }
                fill(
                    prepared,
                    &mut engine,
                    &active_case,
                    context,
                    &mut totals,
                    &mut peak_entries,
                    &mut peak_code_bytes,
                    &mut peak_metadata_bytes,
                    &mut min_free_slots,
                    &mut arena_bytes,
                )?;
            } else {
                fill(
                    prepared,
                    &mut engine,
                    &active_case,
                    context,
                    &mut totals,
                    &mut peak_entries,
                    &mut peak_code_bytes,
                    &mut peak_metadata_bytes,
                    &mut min_free_slots,
                    &mut arena_bytes,
                )?;
            }
        }

        check_deadline(process_started, "versioned hit dispatch")?;
        check_deadline(process_started, "epoch completion")?;
        let checkpoint_due = checkpoint_epochs.get(next_checkpoint) == Some(&epoch);
        totals.epoch_body_elapsed_ns = add_time(
            totals.epoch_body_elapsed_ns,
            epoch_started.elapsed().as_nanos(),
        )?;
        if let Some(batch_timings) = calibration_batch_body_elapsed_ns.as_mut() {
            if epoch == CALIBRATION_WARMUP_EPOCHS {
                previous_calibration_batch_body_ns = totals.epoch_body_elapsed_ns;
            } else if epoch > CALIBRATION_WARMUP_EPOCHS
                && (epoch - CALIBRATION_WARMUP_EPOCHS) % CALIBRATION_MEASUREMENT_BATCH_EPOCHS == 0
            {
                let batch_body_ns = totals
                    .epoch_body_elapsed_ns
                    .checked_sub(previous_calibration_batch_body_ns)
                    .ok_or_else(|| err("calibration epoch-body counter regressed"))?;
                if batch_body_ns == 0 {
                    return Err(err("calibration measurement batch has zero elapsed time"));
                }
                batch_timings.push(batch_body_ns);
                previous_calibration_batch_body_ns = totals.epoch_body_elapsed_ns;
            }
        }
        if checkpoint_due {
            let checkpoint_started = Instant::now();
            check_deadline(process_started, "checkpoint start")?;
            let snapshot = engine.fifo_lifecycle_snapshot()?;
            checkpoints.push(Checkpoint {
                completed_epochs: epoch,
                action: if is_reopen {
                    "drop_reopen_execute_refill"
                } else if is_clear {
                    "clear_refill"
                } else {
                    "mixed_epoch"
                },
                totals,
                cache: snapshot,
                rss: process_rss_snapshot(),
                elapsed_ns: process_started.elapsed().as_nanos(),
            });
            let partial = report(
                prepared,
                process_run,
                epochs,
                epoch,
                calibration_sha256.clone(),
                "running",
                totals,
                snapshot,
                peak_entries,
                peak_code_bytes,
                peak_metadata_bytes,
                min_free_slots,
                arena_bytes,
                rss_before.clone(),
                ProcessRssSnapshot {
                    rss_bytes: None,
                    method: "not-final",
                    unavailable_reason: Some("run is active".into()),
                },
                ProcessRssSnapshot {
                    rss_bytes: None,
                    method: "not-final",
                    unavailable_reason: Some("run is active".into()),
                },
                checkpoints.clone(),
                &prepared.measure_host,
                process_started.elapsed().as_nanos(),
                target_elapsed_ns,
            )?;
            if let Some(path) = output_path {
                write_atomic(path, &partial)?;
            }
            totals.checkpoint_elapsed_ns = add_time(
                totals.checkpoint_elapsed_ns,
                checkpoint_started.elapsed().as_nanos(),
            )?;
            totals.checkpoint_count = totals
                .checkpoint_count
                .checked_add(1)
                .ok_or_else(|| err("checkpoint count overflow"))?;
            next_checkpoint = next_checkpoint
                .checked_add(1)
                .ok_or_else(|| err("checkpoint index overflow"))?;
            check_deadline(process_started, "checkpoint/report persistence")?;
        }
    }
    if next_checkpoint != checkpoint_count {
        return Err(err(
            "stress checkpoint schedule did not reach its bounded count",
        ));
    }
    reconcile_totals(epochs, totals)?;
    check_deadline(process_started, "pre-final snapshot")?;
    let final_snapshot = engine.fifo_lifecycle_snapshot()?;
    if final_snapshot.resident_entries_current != ARENA_SLOTS
        || final_snapshot.arena_available_slots_actual != 0
    {
        return Err(err("stress completed outside full FIFO16 occupancy"));
    }
    let rss_before_drop = process_rss_snapshot();
    check_deadline(process_started, "pre-teardown RSS snapshot")?;
    drop(engine);
    check_deadline(process_started, "runtime teardown")?;
    let rss_after_drop = process_rss_snapshot();
    check_deadline(process_started, "post-teardown RSS snapshot")?;
    let mut elapsed_ns = process_started.elapsed().as_nanos();
    let mut target_met = duration_target_met(target_elapsed_ns, elapsed_ns);
    let mut status = if target_met == Some(false) {
        "under-target"
    } else {
        "completed"
    };
    let mut value = report(
        prepared,
        process_run,
        epochs,
        epochs,
        calibration_sha256,
        status,
        totals,
        final_snapshot,
        peak_entries,
        peak_code_bytes,
        peak_metadata_bytes,
        min_free_slots,
        arena_bytes,
        rss_before,
        rss_before_drop,
        rss_after_drop,
        checkpoints,
        &prepared.measure_host,
        elapsed_ns,
        target_elapsed_ns,
    )?;
    if let Some(path) = output_path {
        write_atomic(path, &value)?;
        if target_elapsed_ns.is_some() {
            elapsed_ns = process_started.elapsed().as_nanos();
            target_met = duration_target_met(target_elapsed_ns, elapsed_ns);
            status = if target_met == Some(false) {
                "under-target"
            } else {
                "completed"
            };
            let object = value
                .as_object_mut()
                .ok_or_else(|| err("final stress report is not a JSON object"))?;
            object.insert("status".into(), status.into());
            object.insert(
                "elapsed_ns".into(),
                serde_json::to_value(elapsed_ns).map_err(|e| err(e.to_string()))?,
            );
            object.insert(
                "duration_target_met".into(),
                serde_json::to_value(target_met).map_err(|e| err(e.to_string()))?,
            );
            write_atomic(path, &value)?;
        }
    }
    check_deadline(process_started, "final report persistence")?;
    if target_met == Some(false) {
        return Err(err(format!(
            "completed stress run was under target duration; report persisted with status under-target ({elapsed_ns} ns < {} ns)",
            target_elapsed_ns.unwrap_or_default()
        )));
    }
    Ok(value)
}

#[allow(clippy::too_many_arguments)]
fn report(
    prepared: &Prepared,
    process_run: u8,
    epochs_requested: usize,
    epochs_completed: usize,
    calibration_sha256: Option<String>,
    status: &'static str,
    cumulative_totals: Totals,
    current_cache: LifecycleCacheSnapshot,
    peak_resident_entries: usize,
    peak_resident_code_bytes: usize,
    peak_resident_logical_metadata_bytes: usize,
    minimum_available_slots_at_full_occupancy: usize,
    arena_reserved_mapping_bytes_high_water: usize,
    process_rss_before_runtime: ProcessRssSnapshot,
    process_rss_after_stress_before_drop: ProcessRssSnapshot,
    process_rss_after_drop: ProcessRssSnapshot,
    checkpoints: Vec<Checkpoint>,
    measure_host: &HostMetadata,
    elapsed_ns: u128,
    target_elapsed_ns: Option<u128>,
) -> Result<serde_json::Value, BenchError> {
    serde_json::to_value(StressReport {
        schema: 3,
        profile: PROFILE,
        status,
        process_run,
        case_list_sha256: od_testkit::sha256_hex(&prepared.list),
        trace_sha256: od_testkit::sha256_hex(&prepared.trace_raw),
        build_source_sha256: implementation_sha256(),
        calibration_sha256,
        capture_host: prepared.trace.capture_host.clone(),
        measure_host: measure_host.clone(),
        cache_capacity_entries: ARENA_SLOTS,
        epochs_requested,
        epochs_completed,
        clear_interval_epochs: CLEAR_INTERVAL,
        reopen_interval_epochs: REOPEN_INTERVAL,
        deterministic_schedule: DETERMINISTIC_SCHEDULE.into(),
        parity_contract: "Every dispatched case is checked against its interpreter oracle for all GPR state, RIP, masked flags, exit, instruction budget and retired instruction count. The two code versions have distinct interpreter results; a version is only dispatched after the previous backing was invalidated, and its first dispatch must compile/publish before the required next-call hit.".into(),
        limitations: vec![
            "Invalidation is a direct between-call cache API exercise; production owner notification, guest-write SMC, and active-code modification are not tested.".into(),
            "RSS is an auxiliary process-wide snapshot and cannot establish cache attribution or a leak-free result.".into(),
            "No guest faults, simultaneous arenas, SMP, or concurrent independent runtime is exercised.".into(),
        ],
        cumulative_totals,
        current_cache,
        peak_resident_entries,
        peak_resident_code_bytes,
        peak_resident_logical_metadata_bytes,
        minimum_available_slots_at_full_occupancy,
        arena_reserved_mapping_bytes_high_water,
        process_rss_before_runtime,
        process_rss_after_stress_before_drop,
        process_rss_after_drop,
        checkpoints,
        target_elapsed_ns,
        duration_target_met: if status == "running" {
            None
        } else {
            duration_target_met(target_elapsed_ns, elapsed_ns)
        },
        elapsed_ns,
    }).map_err(|e| err(e.to_string()))
}

fn load_inputs(
    case_list: &Path,
    trace_path: &Path,
    process_started: Instant,
) -> Result<Prepared, BenchError> {
    prepare(case_list, trace_path, process_started)
}

fn calibration_payload_hash(value: &serde_json::Value) -> Result<String, BenchError> {
    let mut payload = value.clone();
    let object = payload
        .as_object_mut()
        .ok_or_else(|| err("calibration artifact must be a JSON object"))?;
    object.remove("calibration_integrity_sha256");
    let canonical = serde_json::to_vec(&payload).map_err(|e| err(e.to_string()))?;
    Ok(od_testkit::sha256_hex(&canonical))
}

fn expected_calibration_epochs(calibration: &Calibration) -> Result<usize, BenchError> {
    if calibration.schema != 3
        || calibration.profile != PROFILE
        || calibration.calibration_epochs != CALIBRATION_EPOCHS
        || calibration.calibration_warmup_epochs != CALIBRATION_WARMUP_EPOCHS
        || calibration.calibration_measurement_batch_epochs != CALIBRATION_MEASUREMENT_BATCH_EPOCHS
        || calibration.calibration_measurement_batches != CALIBRATION_MEASUREMENT_BATCHES
        || calibration.calibration_elapsed_ns == 0
        || calibration.calibration_epoch_body_elapsed_ns == 0
        || calibration.calibration_warmup_body_elapsed_ns == 0
        || calibration.calibration_checkpoint_count != MAX_CHECKPOINTS
        || calibration.target_stress_ns != TARGET_STRESS_NS
        || calibration.max_run_ns != MAX_RUN_TIME.as_nanos()
        || calibration.build_source_sha256 != implementation_sha256()
        || calibration.schedule != CALIBRATION_SCHEDULE
    {
        return Err(err("calibration fields do not match this build/profile"));
    }
    let batch_body_elapsed_ns = calibration
        .calibration_batch_body_elapsed_ns
        .iter()
        .try_fold(0u128, |sum, elapsed| sum.checked_add(*elapsed))
        .ok_or_else(|| err("calibration batch timing sum overflow"))?;
    if batch_body_elapsed_ns != calibration.calibration_epoch_body_elapsed_ns {
        return Err(err(
            "calibration batch timings do not match measured body total",
        ));
    }
    let composed = calibration
        .calibration_epoch_body_elapsed_ns
        .checked_add(calibration.calibration_warmup_body_elapsed_ns)
        .and_then(|value| value.checked_add(calibration.calibration_checkpoint_elapsed_ns))
        .and_then(|value| value.checked_add(calibration.calibration_fixed_elapsed_ns))
        .ok_or_else(|| err("calibration elapsed-time arithmetic overflow"))?;
    if composed != calibration.calibration_elapsed_ns {
        return Err(err("calibration elapsed-time components do not reconcile"));
    }
    let expected_epoch = fastest_batch_epoch_ns(
        &calibration.calibration_batch_body_elapsed_ns,
        calibration.calibration_measurement_batch_epochs,
    )?;
    if expected_epoch != calibration.estimated_epoch_ns {
        return Err(err("calibration fastest-batch rate is inconsistent"));
    }
    let checkpoint_budget = calibration.calibration_checkpoint_elapsed_ns;
    let fixed_budget = calibration.calibration_fixed_elapsed_ns;
    let (recommended, projected) = recommend_epochs(
        calibration.target_stress_ns,
        checkpoint_budget,
        fixed_budget,
        expected_epoch,
    )?;
    if recommended != calibration.recommended_epochs || recommended < REOPEN_INTERVAL {
        return Err(err(
            "calibration recommended epoch arithmetic is inconsistent or too short",
        ));
    }
    if projected != calibration.upper_bound_elapsed_ns {
        return Err(err(
            "calibration projection exceeds process budget or does not reconcile",
        ));
    }
    Ok(recommended)
}

pub(super) fn calibrate(
    case_list: &Path,
    trace_path: &Path,
    output_path: &Path,
) -> Result<serde_json::Value, BenchError> {
    if cfg!(debug_assertions) {
        return Err(err("calibration requires a release build"));
    }
    let process_started = Instant::now();
    let prepared = load_inputs(case_list, trace_path, process_started)?;
    check_deadline(process_started, "calibration setup")?;
    let progress_path =
        output_path.with_extension(format!("calibration-progress.{}.json", std::process::id()));
    let mut batch_body_elapsed_ns = Vec::with_capacity(CALIBRATION_MEASUREMENT_BATCHES);
    let stress_report = run_epochs(
        &prepared,
        CALIBRATION_EPOCHS,
        1,
        None,
        Some(&progress_path),
        process_started,
        None,
        Some(&mut batch_body_elapsed_ns),
    )?;
    let elapsed = process_started.elapsed().as_nanos();
    let totals: Totals = serde_json::from_value(
        stress_report
            .get("cumulative_totals")
            .cloned()
            .ok_or_else(|| err("calibration stress report has no aggregate totals"))?,
    )
    .map_err(|e| err(format!("calibration totals: {e}")))?;
    if batch_body_elapsed_ns.len() != CALIBRATION_MEASUREMENT_BATCHES {
        return Err(err("calibration did not complete all measurement batches"));
    }
    let measured_body_elapsed_ns = batch_body_elapsed_ns
        .iter()
        .try_fold(0u128, |sum, batch_ns| sum.checked_add(*batch_ns))
        .ok_or_else(|| err("calibration batch elapsed-time sum overflow"))?;
    let warmup_body_elapsed_ns = totals
        .epoch_body_elapsed_ns
        .checked_sub(measured_body_elapsed_ns)
        .ok_or_else(|| err("calibration warm-up exceeds total epoch-body time"))?;
    let fixed_elapsed = elapsed
        .checked_sub(measured_body_elapsed_ns)
        .and_then(|value| value.checked_sub(warmup_body_elapsed_ns))
        .and_then(|value| value.checked_sub(totals.checkpoint_elapsed_ns))
        .ok_or_else(|| err("calibration timing components exceed total elapsed time"))?;
    let estimated_epoch_ns =
        fastest_batch_epoch_ns(&batch_body_elapsed_ns, CALIBRATION_MEASUREMENT_BATCH_EPOCHS)?;
    let (recommended_epochs, projected) = recommend_epochs(
        TARGET_STRESS_NS,
        totals.checkpoint_elapsed_ns,
        fixed_elapsed,
        estimated_epoch_ns,
    )?;
    if recommended_epochs < REOPEN_INTERVAL {
        return Err(err(
            "calibration does not provide enough epochs for repeated reopen stress",
        ));
    }
    let mut value = serde_json::to_value(Calibration {
        schema: 3,
        profile: PROFILE.into(),
        case_list_sha256: od_testkit::sha256_hex(&prepared.list),
        trace_sha256: od_testkit::sha256_hex(&prepared.trace_raw),
        calibration_epochs: CALIBRATION_EPOCHS,
        calibration_warmup_epochs: CALIBRATION_WARMUP_EPOCHS,
        calibration_measurement_batch_epochs: CALIBRATION_MEASUREMENT_BATCH_EPOCHS,
        calibration_measurement_batches: CALIBRATION_MEASUREMENT_BATCHES,
        calibration_batch_body_elapsed_ns: batch_body_elapsed_ns,
        calibration_warmup_body_elapsed_ns: warmup_body_elapsed_ns,
        calibration_elapsed_ns: elapsed,
        calibration_epoch_body_elapsed_ns: measured_body_elapsed_ns,
        calibration_checkpoint_elapsed_ns: totals.checkpoint_elapsed_ns,
        calibration_checkpoint_count: totals.checkpoint_count,
        calibration_fixed_elapsed_ns: fixed_elapsed,
        estimated_epoch_ns,
        target_stress_ns: TARGET_STRESS_NS,
        recommended_epochs,
        upper_bound_elapsed_ns: projected,
        max_run_ns: MAX_RUN_TIME.as_nanos(),
        build_source_sha256: implementation_sha256(),
        calibration_integrity_sha256: String::new(),
        capture_host: prepared.trace.capture_host.clone(),
        measure_host: prepared.measure_host.clone(),
        schedule: CALIBRATION_SCHEDULE.into(),
    })
    .map_err(|e| err(e.to_string()))?;
    let integrity = calibration_payload_hash(&value)?;
    value["calibration_integrity_sha256"] = integrity.into();
    if let Err(error) = std::fs::remove_file(&progress_path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(err(format!(
                "remove calibration progress artifact: {error}"
            )));
        }
    }
    check_deadline(process_started, "calibration finalization")?;
    write_atomic(output_path, &value)?;
    check_deadline(process_started, "calibration artifact persistence")?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::{
        CALIBRATION_MEASUREMENT_BATCHES, MAX_EPOCHS, duration_target_met, fastest_batch_epoch_ns,
        recommend_epochs,
    };

    #[test]
    fn recommendation_rejects_epoch_cap_below_target() {
        let target_ns = 660_000_000_000;
        let result = recommend_epochs(target_ns, 2_000_000_000, 2_000_000_000, 1_000);
        assert!(result.is_err());

        let enough_per_epoch = target_ns / (MAX_EPOCHS as u128 / 2);
        let (recommended, projected) =
            recommend_epochs(target_ns, 2_000_000_000, 2_000_000_000, enough_per_epoch)
                .expect("cap can reach the target");
        assert!(recommended < MAX_EPOCHS);
        assert!(projected >= target_ns);
    }

    #[test]
    fn fastest_batch_sets_conservative_minimum_duration_rate() {
        let batch_ns = vec![140, 151, 146, 143, 158, 149, 145, 152];
        assert_eq!(batch_ns.len(), CALIBRATION_MEASUREMENT_BATCHES);
        assert_eq!(fastest_batch_epoch_ns(&batch_ns, 10).unwrap(), 14);
    }

    #[test]
    fn calibration_batch_rate_rejects_missing_or_zero_batches() {
        assert!(fastest_batch_epoch_ns(&[1, 2], 1).is_err());
        assert!(fastest_batch_epoch_ns(&[0; CALIBRATION_MEASUREMENT_BATCHES], 1).is_err());
        assert!(fastest_batch_epoch_ns(&[1; CALIBRATION_MEASUREMENT_BATCHES], 0).is_err());
    }

    #[test]
    fn under_target_duration_is_explicit() {
        assert_eq!(duration_target_met(Some(660), 659), Some(false));
        assert_eq!(duration_target_met(Some(660), 660), Some(true));
        assert_eq!(duration_target_met(None, 1), None);
    }
}

pub(super) fn run(
    case_list: &Path,
    trace_path: &Path,
    calibration_path: &Path,
    process_run: u8,
    epochs: usize,
    output_path: &Path,
) -> Result<serde_json::Value, BenchError> {
    let process_started = Instant::now();
    if !(1..=3).contains(&process_run) {
        return Err(err("process-run must be 1..=3"));
    }
    let calibration_raw = read_bounded(calibration_path, 1024 * 1024, "calibration")?;
    check_deadline(process_started, "calibration read")?;
    let parsed: serde_json::Value = serde_json::from_slice(&calibration_raw)
        .map_err(|e| err(format!("calibration JSON: {e}")))?;
    let calibration: Calibration = serde_json::from_value(parsed.clone())
        .map_err(|e| err(format!("calibration JSON: {e}")))?;
    if calibration_payload_hash(&parsed)? != calibration.calibration_integrity_sha256 {
        return Err(err("calibration integrity digest mismatch"));
    }
    let prepared = load_inputs(case_list, trace_path, process_started)?;
    check_deadline(process_started, "scored input preparation")?;
    let recommended = expected_calibration_epochs(&calibration)?;
    if calibration.case_list_sha256 != od_testkit::sha256_hex(&prepared.list)
        || calibration.trace_sha256 != od_testkit::sha256_hex(&prepared.trace_raw)
        || calibration.capture_host != prepared.trace.capture_host
        || calibration.measure_host != prepared.measure_host
        || epochs != recommended
    {
        return Err(err(
            "calibration corpus, host, source, or fixed epoch recommendation does not match this run",
        ));
    }
    check_deadline(process_started, "calibration validation")?;
    let calibration_sha256 = od_testkit::sha256_hex(&calibration_raw);
    run_epochs(
        &prepared,
        epochs,
        process_run,
        Some(calibration_sha256),
        Some(output_path),
        process_started,
        Some(calibration.target_stress_ns),
        None,
    )
}
