use std::num::NonZeroU64;
use std::path::PathBuf;
use std::time::Instant;

use od_bench::{
    BenchmarkConfig, BenchmarkRunMetadata, BenchmarkSuite, HostMetadata, LookupQuery,
    RecordedTrace, ScenarioWorkload, benchmark_execution_context, run_suite_with_metadata,
    suite_from_recorded_trace,
};
use od_isa::memory::AccessContext;
use od_jit::block::{BlockLimits, BuildOutcome, CodeView, build_block};
use od_jit::translation::{
    AddressSpaceId, BackingId, BackingVersion, CodeDependencySpan, CodeIdentity, CompileFlavor,
    MappingId, MappingVersion, TranslationKey,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(mode) = args.next() else {
        print_usage();
        return Err("missing command".into());
    };
    let mut input = None::<PathBuf>;
    let mut corpus_path = None::<PathBuf>;
    let mut case_list = None::<PathBuf>;
    let mut trace_path = None::<PathBuf>;
    let mut output = None::<PathBuf>;
    let mut calibration = None::<PathBuf>;
    let mut epochs = None::<usize>;
    let mut process_run = 1u8;
    let mut preflight_only = false;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--case-list" => {
                let path: PathBuf = args.next().ok_or("--case-list requires a path")?.into();
                case_list = Some(path.clone());
                input = Some(path);
            }
            "--trace" => {
                let path: PathBuf = args.next().ok_or("--trace requires a path")?.into();
                trace_path = Some(path.clone());
                input = Some(path);
            }
            "--corpus" => {
                corpus_path = Some(args.next().ok_or("--corpus requires a path")?.into());
            }
            "--output" => {
                output = Some(args.next().ok_or("--output requires a path")?.into());
            }
            "--calibration" => {
                calibration = Some(args.next().ok_or("--calibration requires a path")?.into());
            }
            "--epochs" => {
                epochs = Some(args.next().ok_or("--epochs requires an integer")?.parse()?);
            }
            "--process-run" => {
                process_run = args
                    .next()
                    .ok_or("--process-run requires an integer")?
                    .parse()?;
            }
            "--preflight-only" => preflight_only = true,
            "--help" | "-h" => {
                print_usage();
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    match mode.as_str() {
        "record" => {
            let case_list = input.ok_or("record requires --case-list PATH")?;
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            let trace = od_bench::record::record_case_list(&case_list)
                .map_err(|error| format!("trace capture failed: {error:?}"))?;
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            let trace: RecordedTrace = {
                let _ = case_list;
                return Err("record requires native Apple Silicon macOS JIT execution".into());
            };
            write_json(output.as_ref(), &trace)?;
        }
        "measure" => {
            let preparation_started = Instant::now();
            let trace_path = input.ok_or("measure requires --trace PATH")?;
            let metadata = std::fs::metadata(&trace_path)?;
            if metadata.len() > od_bench::MAX_LOGICAL_BYTES as u64 {
                return Err("trace JSON exceeds the 128 MiB bounded input limit".into());
            }
            let bytes = std::fs::read(&trace_path)?;
            let trace_sha256 = od_testkit::sha256_hex(&bytes);
            let trace: RecordedTrace = serde_json::from_slice(&bytes)?;
            let capture_host = trace.capture_host.clone();
            let suite = suite_from_recorded_trace(&trace)
                .map_err(|error| format!("trace validation failed: {error:?}"))?;
            drop(trace);
            drop(bytes);
            let measure_host = HostMetadata::current();
            let report = run_suite_with_metadata(
                &suite,
                BenchmarkConfig {
                    process_run,
                    preparation_elapsed: preparation_started.elapsed(),
                    ..BenchmarkConfig::default()
                },
                BenchmarkRunMetadata {
                    trace_sha256: Some(trace_sha256),
                    capture_host: Some(capture_host),
                    measure_host,
                },
            )
            .map_err(|error| format!("benchmark failed: {error:?}"))?;
            write_json(output.as_ref(), &report)?;
        }
        "synthetic" => {
            let preparation_started = Instant::now();
            let suite = synthetic_smoke_suite()?;
            let measure_host = HostMetadata::current();
            let report = run_suite_with_metadata(
                &suite,
                BenchmarkConfig {
                    process_run,
                    preparation_elapsed: preparation_started.elapsed(),
                    ..BenchmarkConfig::default()
                },
                BenchmarkRunMetadata {
                    measure_host,
                    ..BenchmarkRunMetadata::default()
                },
            )
            .map_err(|error| format!("benchmark failed: {error:?}"))?;
            write_json(output.as_ref(), &report)?;
        }
        "dispatch-cache" => {
            let case_list = case_list.ok_or("dispatch-cache requires --case-list PATH")?;
            let trace = trace_path.ok_or("dispatch-cache requires --trace PATH")?;
            let output = output.ok_or("dispatch-cache requires --output PATH")?;
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            {
                let value = od_bench::dispatch_cache::run(&case_list, &trace, process_run)?;
                od_bench::dispatch_cache::write_atomic(&output, &value)?;
            }
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            {
                let _ = (case_list, trace, output);
                return Err("dispatch-cache requires native Apple Silicon macOS execution".into());
            }
        }
        "cold-first-run" => {
            let case_list = case_list.ok_or("cold-first-run requires --case-list PATH")?;
            let trace = trace_path.ok_or("cold-first-run requires --trace PATH")?;
            let output = output.ok_or("cold-first-run requires --output PATH")?;
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            {
                let value = od_bench::dispatch_cache::run_cold_first_execution(
                    &case_list,
                    &trace,
                    process_run,
                )?;
                od_bench::dispatch_cache::write_atomic(&output, &value)?;
            }
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            {
                let _ = (case_list, trace, output);
                return Err("cold-first-run requires native Apple Silicon macOS execution".into());
            }
        }
        "cold-phases" => {
            let case_list = case_list.ok_or("cold-phases requires --case-list PATH")?;
            let trace = trace_path.ok_or("cold-phases requires --trace PATH")?;
            let output = output.ok_or("cold-phases requires --output PATH")?;
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            {
                let value = od_bench::dispatch_cache::run_cold_pipeline_attribution(
                    &case_list,
                    &trace,
                    process_run,
                )?;
                od_bench::dispatch_cache::write_atomic(&output, &value)?;
            }
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            {
                let _ = (case_list, trace, output);
                return Err("cold-phases requires native Apple Silicon macOS execution".into());
            }
        }
        "warm-first-run" => {
            let case_list = case_list.ok_or("warm-first-run requires --case-list PATH")?;
            let trace = trace_path.ok_or("warm-first-run requires --trace PATH")?;
            let output = output.ok_or("warm-first-run requires --output PATH")?;
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            {
                let value = od_bench::dispatch_cache::run_warm_first_execution(
                    &case_list,
                    &trace,
                    process_run,
                )?;
                od_bench::dispatch_cache::write_atomic(&output, &value)?;
            }
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            {
                let _ = (case_list, trace, output);
                return Err("warm-first-run requires native Apple Silicon macOS execution".into());
            }
        }
        "warm-branch-helper" => {
            let case_list = case_list.ok_or("warm-branch-helper requires --case-list PATH")?;
            let trace = trace_path.ok_or("warm-branch-helper requires --trace PATH")?;
            let output = output.ok_or("warm-branch-helper requires --output PATH")?;
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            {
                let value = od_bench::dispatch_cache::run_warm_branch_helper(
                    &case_list,
                    &trace,
                    process_run,
                )?;
                od_bench::dispatch_cache::write_atomic(&output, &value)?;
            }
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            {
                let _ = (case_list, trace, output);
                return Err(
                    "warm-branch-helper requires native Apple Silicon macOS execution".into(),
                );
            }
        }
        "cache-churn" => {
            let case_list = case_list.ok_or("cache-churn requires --case-list PATH")?;
            let trace = trace_path.ok_or("cache-churn requires --trace PATH")?;
            let output = output.ok_or("cache-churn requires --output PATH")?;
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            {
                let value =
                    od_bench::dispatch_cache::run_cache_churn(&case_list, &trace, process_run)?;
                od_bench::dispatch_cache::write_atomic(&output, &value)?;
            }
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            {
                let _ = (case_list, trace, output);
                return Err("cache-churn requires native Apple Silicon macOS execution".into());
            }
        }
        "cache-lifecycle" => {
            let case_list = case_list.ok_or("cache-lifecycle requires --case-list PATH")?;
            let trace = trace_path.ok_or("cache-lifecycle requires --trace PATH")?;
            let output = output.ok_or("cache-lifecycle requires --output PATH")?;
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            {
                let value =
                    od_bench::dispatch_cache::run_cache_lifecycle(&case_list, &trace, process_run)?;
                od_bench::dispatch_cache::write_atomic(&output, &value)?;
            }
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            {
                let _ = (case_list, trace, output, process_run);
                return Err("cache-lifecycle requires native Apple Silicon macOS execution".into());
            }
        }
        "cache-lifecycle-stress-calibrate" => {
            let case_list =
                case_list.ok_or("cache-lifecycle-stress-calibrate requires --case-list PATH")?;
            let trace =
                trace_path.ok_or("cache-lifecycle-stress-calibrate requires --trace PATH")?;
            let output = output.ok_or("cache-lifecycle-stress-calibrate requires --output PATH")?;
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            {
                let _ = od_bench::dispatch_cache::calibrate_cache_lifecycle_stress(
                    &case_list, &trace, &output,
                )?;
            }
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            {
                let _ = (case_list, trace, output);
                return Err("cache-lifecycle-stress-calibrate requires native Apple Silicon macOS execution".into());
            }
        }
        "cache-lifecycle-stress" => {
            let case_list = case_list.ok_or("cache-lifecycle-stress requires --case-list PATH")?;
            let trace = trace_path.ok_or("cache-lifecycle-stress requires --trace PATH")?;
            let calibration =
                calibration.ok_or("cache-lifecycle-stress requires --calibration PATH")?;
            let epochs = epochs.ok_or("cache-lifecycle-stress requires --epochs N")?;
            let output = output.ok_or("cache-lifecycle-stress requires --output PATH")?;
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            {
                let value = od_bench::dispatch_cache::run_cache_lifecycle_stress(
                    &case_list,
                    &trace,
                    &calibration,
                    process_run,
                    epochs,
                    &output,
                )?;
                let _ = value;
            }
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            {
                let _ = (case_list, trace, calibration, epochs, output, process_run);
                return Err(
                    "cache-lifecycle-stress requires native Apple Silicon macOS execution".into(),
                );
            }
        }
        "flags-compare" => {
            let output = output.ok_or("flags-compare requires --output PATH")?;
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            {
                let value = serde_json::to_value(od_bench::flags_compare::run(process_run)?)?;
                od_bench::dispatch_cache::write_atomic(&output, &value)?;
            }
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            {
                let _ = (output, process_run);
                return Err("flags-compare requires native Apple Silicon macOS execution".into());
            }
        }
        "cached-flags-compare" => {
            let output = output.ok_or("cached-flags-compare requires --output PATH")?;
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            {
                let value = if preflight_only {
                    serde_json::to_value(od_bench::cached_flags_compare::preflight()?)?
                } else {
                    serde_json::to_value(od_bench::cached_flags_compare::run(process_run)?)?
                };
                od_bench::dispatch_cache::write_atomic(&output, &value)?;
            }
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            {
                let _ = (output, process_run, preflight_only);
                return Err(
                    "cached-flags-compare requires native Apple Silicon macOS execution".into(),
                );
            }
        }
        "cached-native-flags-compare" => {
            let corpus = corpus_path.ok_or("cached-native-flags-compare requires --corpus PATH")?;
            let output = output.ok_or("cached-native-flags-compare requires --output PATH")?;
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            {
                let value = if preflight_only {
                    serde_json::to_value(od_bench::cached_native_flags_compare::preflight(
                        &corpus,
                    )?)?
                } else {
                    serde_json::to_value(od_bench::cached_native_flags_compare::run(
                        &corpus,
                        process_run,
                    )?)?
                };
                od_bench::dispatch_cache::write_atomic(&output, &value)?;
            }
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            {
                let _ = (corpus, output, process_run, preflight_only);
                return Err(
                    "cached-native-flags-compare requires native Apple Silicon macOS execution"
                        .into(),
                );
            }
        }
        "allocator-cost" => {
            let output = output.ok_or("allocator-cost requires --output PATH")?;
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            {
                let value = if preflight_only {
                    serde_json::to_value(od_bench::allocator_cost::preflight(process_run)?)?
                } else {
                    serde_json::to_value(od_bench::allocator_cost::run(process_run)?)?
                };
                od_bench::dispatch_cache::write_atomic(&output, &value)?;
            }
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            {
                let _ = (output, process_run, preflight_only);
                return Err("allocator-cost requires native Apple Silicon macOS execution".into());
            }
        }
        "warm-native-fallback" => {
            let corpus = corpus_path.ok_or("warm-native-fallback requires --corpus PATH")?;
            let output = output.ok_or("warm-native-fallback requires --output PATH")?;
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            {
                let value = if preflight_only {
                    serde_json::to_value(
                        od_bench::cached_native_flags_compare::warm_fallback::preflight(&corpus)?,
                    )?
                } else {
                    serde_json::to_value(
                        od_bench::cached_native_flags_compare::warm_fallback::run(
                            &corpus,
                            process_run,
                        )?,
                    )?
                };
                od_bench::dispatch_cache::write_atomic(&output, &value)?;
            }
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            {
                let _ = (corpus, output, process_run, preflight_only);
                return Err(
                    "warm-native-fallback requires native Apple Silicon macOS execution".into(),
                );
            }
        }
        _ => {
            print_usage();
            return Err(
                "expected record, measure, synthetic, dispatch-cache, cold-first-run, cold-phases, warm-first-run, warm-branch-helper, warm-native-fallback, cache-churn, cache-lifecycle, cache-lifecycle-stress-calibrate, cache-lifecycle-stress, flags-compare, cached-flags-compare, cached-native-flags-compare, or allocator-cost".into(),
            );
        }
    }
    Ok(())
}

fn write_json(
    path: Option<&PathBuf>,
    serializable: &impl serde::Serialize,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut bytes = serde_json::to_vec_pretty(serializable)?;
    bytes.push(b'\n');
    match path {
        Some(path) => std::fs::write(path, bytes)?,
        None => print!("{}", String::from_utf8(bytes)?),
    }
    Ok(())
}

fn print_usage() {
    eprintln!(
        "Usage:\n  od-bench record --case-list PATH [--output trace.json]\n  od-bench measure --trace trace.json [--process-run 1..3] [--output report.json]\n  od-bench synthetic [--process-run 1..3] [--output report.json]\n  od-bench dispatch-cache --case-list PATH --trace trace.json --process-run 1..3 --output report.json\n  od-bench cold-first-run --case-list PATH --trace trace.json --process-run 1..3 --output report.json\n\
  od-bench cold-phases --case-list PATH --trace PATH --process-run 1..3 --output report.json\n\
  od-bench warm-first-run --case-list PATH --trace PATH --process-run 1..3 --output report.json\n\
  od-bench warm-branch-helper --case-list PATH --trace PATH --process-run 1..3 --output report.json\n\
  od-bench warm-native-fallback --corpus observations.json [--preflight-only] --process-run 1..3 --output report.json\n\
  od-bench cache-churn --case-list PATH --trace PATH --process-run 1..3 --output report.json\n\
  od-bench cache-lifecycle --case-list PATH --trace PATH --process-run 1..3 --output report.json\n\
  od-bench cache-lifecycle-stress-calibrate --case-list PATH --trace PATH --output calibration.json\n\
  od-bench cache-lifecycle-stress --case-list PATH --trace PATH --calibration calibration.json --epochs N --process-run 1..3 --output report.json\n\
  od-bench flags-compare --process-run 1..3 --output report.json\n\
  od-bench cached-flags-compare [--preflight-only] --process-run 1..3 --output report.json\n\
  od-bench cached-native-flags-compare --corpus observations.json [--preflight-only] --process-run 1..3 --output report.json\n\
  od-bench allocator-cost [--preflight-only] --process-run 1..3 --output report.json\n\
         `record` requires macOS ARM64. `measure` rebuilds all keys before timing."
    );
}

fn synthetic_smoke_suite() -> Result<BenchmarkSuite, Box<dyn std::error::Error>> {
    let mut keys = Vec::new();
    for index in 0..8u8 {
        keys.push(make_key(
            index,
            0x10_0000,
            u64::from(index) + 1,
            u64::from(index) + 100,
        )?);
    }
    let recorded = scenario_from_sequence("synthetic-recorded-sequence", &keys, 8);
    let hot_one = scenario_from_repeated("synthetic-hot-loop-1", &keys[..1], 4_096);
    let hot_eight = scenario_from_repeated("synthetic-hot-loop-8", &keys, 4_096);
    let stale = make_key(0, 0x10_0000, 1, 100)?;
    let stale_backing = make_key(0, 0x10_0000, 1, 101)?;
    let absent = make_key(0xfe, 0x20_0000, 900, 901)?;
    let mixed = ScenarioWorkload {
        name: "synthetic-stale-and-miss-smoke".into(),
        synthetic: true,
        cache_entries: vec![stale.clone()],
        queries: vec![query(0, &stale, &stale_backing), query(1, &absent, &absent)],
        input_count: 2,
        visits_per_input: vec![1, 1],
    };
    Ok(BenchmarkSuite {
        max_input_json_bytes: 0,
        trace_elapsed: std::time::Duration::ZERO,
        recorded_trace_visits: 0,
        scenarios: vec![recorded, hot_one, hot_eight, mixed],
    })
}

fn scenario_from_sequence(
    name: &str,
    keys: &[TranslationKey],
    input_count: usize,
) -> ScenarioWorkload {
    let visits_per_input = (0..input_count)
        .map(|index| usize::from(index < keys.len()))
        .collect();
    let queries = keys
        .iter()
        .enumerate()
        .map(|(index, key)| query(index, key, key))
        .collect();
    ScenarioWorkload {
        name: name.into(),
        synthetic: true,
        cache_entries: keys.to_vec(),
        queries,
        input_count,
        visits_per_input,
    }
}

fn scenario_from_repeated(name: &str, keys: &[TranslationKey], visits: usize) -> ScenarioWorkload {
    let mut counts = vec![0usize; keys.len()];
    let queries = (0..visits)
        .map(|index| {
            let input_index = index % keys.len();
            counts[input_index] += 1;
            query(input_index, &keys[input_index], &keys[input_index])
        })
        .collect();
    ScenarioWorkload {
        name: name.into(),
        synthetic: true,
        cache_entries: keys.to_vec(),
        queries,
        input_count: keys.len(),
        visits_per_input: counts,
    }
}

fn query(input_index: usize, key: &TranslationKey, current: &TranslationKey) -> LookupQuery {
    LookupQuery {
        input_index,
        key: key.clone(),
        current_dependencies: current.dependencies().to_vec(),
    }
}

fn make_key(
    immediate: u8,
    base: u64,
    mapping_id: u64,
    backing_id: u64,
) -> Result<TranslationKey, Box<dyn std::error::Error>> {
    let bytes = [0x48, 0xb8, immediate, 0, 0, 0, 0, 0, 0, 0];
    let code = CodeView::new(base, &bytes).map_err(|error| format!("{error:?}"))?;
    let BuildOutcome::Block(block) =
        build_block(code, base, AccessContext::default(), BlockLimits::default())
            .map_err(|error| format!("{error:?}"))?
    else {
        return Err("synthetic MOV must build a block".into());
    };
    let identity = CodeIdentity {
        address_space: AddressSpaceId::new(nz(1)),
        mapping: MappingId::new(nz(mapping_id)),
        mapping_version: MappingVersion::new(nz(1)),
        backing: BackingId::new(nz(backing_id)),
        backing_version: BackingVersion::new(nz(1)),
        executable: true,
    };
    TranslationKey::for_block(
        code,
        &block,
        CompileFlavor::S0_09NativeBlock,
        benchmark_execution_context(),
        &[CodeDependencySpan {
            start_rip: code.base_rip(),
            end_rip: code.code_end(),
            identity,
        }],
    )
    .map_err(|error| format!("{error:?}").into())
}

fn nz(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).expect("synthetic stable IDs are nonzero")
}
