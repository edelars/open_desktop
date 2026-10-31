//! Bounded comparison harness for typed JIT translation-key lookup prototypes.
//!
//! Translation keys and traces are prepared before timing. Both Vec and
//! HashMap candidates perform identical full-key equality and dependency
//! snapshot validation.

use std::collections::{HashMap, HashSet};
use std::hint::black_box;
use std::io::Read;
use std::num::NonZeroU64;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use od_isa::memory::AccessContext;
use od_jit::block::{
    AdmittedInstructionBoundaries, BlockLimits, BuildOutcome, CodeView, build_block,
};
use od_jit::translation::{
    AddressSpaceId, BackingId, BackingVersion, CodeIdentity, CompileFlavor, ExecutionContextId,
    ExecutionContextSnapshot, MappingId, MappingVersion,
};
use od_jit::translation::{CodeDependencySpan, TranslationKey};
use serde::{Deserialize, Serialize};

pub const MAX_INPUTS: usize = 1_010;
pub const MAX_CODE_BYTES_PER_INPUT: usize = 4 * 1024;
pub const MAX_INPUT_JSON_BYTES: usize = 64 * 1024;
pub const MAX_VISITS_PER_INPUT: usize = 4_096;
pub const MAX_TOTAL_TRACE_VISITS: usize = 65_536;
pub const MAX_KEYS: usize = 4_096;
pub const MAX_LOGICAL_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_SCENARIOS: usize = 24;
pub const MAX_TRACE_TIME: Duration = Duration::from_secs(10 * 60);
pub const MAX_RUN_TIME: Duration = Duration::from_secs(30 * 60);
pub const DEFAULT_SAMPLE_PAIRS: usize = 20;
pub const MAX_SAMPLE_PAIRS: usize = 100;
pub const MIN_BATCH_TIME: Duration = Duration::from_millis(200);

/// Stable synthetic execution-context identity used only by this fixture
/// benchmark harness; it is not a host pointer or production VM identity.
pub fn benchmark_execution_context() -> ExecutionContextSnapshot {
    ExecutionContextSnapshot::s0_09_fixture(
        ExecutionContextId::from_u64(1).expect("benchmark context ID is nonzero"),
    )
}

#[derive(Clone, Debug)]
pub struct LookupQuery {
    pub input_index: usize,
    pub key: TranslationKey,
    /// Current memory-owner snapshot. It may be stale or non-executable; that
    /// result must be reported as Stale after finding the cached key.
    pub current_dependencies: Vec<CodeDependencySpan>,
}

#[derive(Clone, Debug)]
pub struct ScenarioWorkload {
    pub name: String,
    pub synthetic: bool,
    pub cache_entries: Vec<TranslationKey>,
    pub queries: Vec<LookupQuery>,
    pub input_count: usize,
    pub visits_per_input: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct BenchmarkSuite {
    pub max_input_json_bytes: usize,
    pub trace_elapsed: Duration,
    pub recorded_trace_visits: usize,
    pub scenarios: Vec<ScenarioWorkload>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BenchmarkConfig {
    pub sample_pairs: usize,
    pub minimum_batch: Duration,
    pub total_time_limit: Duration,
    /// Caller supplies 1..=3 when collecting process-level repeats.
    pub process_run: u8,
    /// Time spent loading/validating the trace and building the suite. It is
    /// excluded from measured series, but included in the total-run bound.
    pub preparation_elapsed: Duration,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostMetadata {
    pub os: String,
    pub os_version: String,
    pub architecture: String,
    pub cpu_model: String,
    pub build: BuildMetadata,
    pub execution: ExecutionContext,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildMetadata {
    pub rustc: String,
    pub target: String,
    pub profile: String,
    pub source_revision: String,
    pub source_sha256: String,
    pub source_tree_dirty: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionContext {
    pub working_directory: String,
    pub repository_revision: String,
    pub working_tree_dirty: Option<bool>,
}

impl Default for HostMetadata {
    fn default() -> Self {
        Self {
            os: "not-recorded".into(),
            os_version: "not-recorded".into(),
            architecture: "not-recorded".into(),
            cpu_model: "not-recorded".into(),
            build: BuildMetadata {
                rustc: "not-recorded".into(),
                target: "not-recorded".into(),
                profile: "not-recorded".into(),
                source_revision: "not-recorded".into(),
                source_sha256: "not-recorded".into(),
                source_tree_dirty: None,
            },
            execution: ExecutionContext {
                working_directory: "not-recorded".into(),
                repository_revision: "not-recorded".into(),
                working_tree_dirty: None,
            },
        }
    }
}

impl HostMetadata {
    pub fn current() -> Self {
        let mut execution = ExecutionContext {
            working_directory: std::env::current_dir()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| "unavailable".into()),
            repository_revision: command_text("git", &["rev-parse", "--verify", "HEAD"]),
            working_tree_dirty: None,
        };
        execution.working_tree_dirty =
            match command_stdout_bounded("git", &["status", "--porcelain"]) {
                Ok(output) => Some(!output.trim().is_empty()),
                Err(reason) => {
                    eprintln!("host metadata git status unavailable: {reason}");
                    None
                }
            };
        Self {
            os: std::env::consts::OS.into(),
            os_version: host_os_version(),
            architecture: std::env::consts::ARCH.into(),
            cpu_model: host_cpu_model(),
            build: BuildMetadata {
                rustc: env!("OD_BENCH_BUILD_RUSTC").into(),
                target: env!("OD_BENCH_BUILD_TARGET").into(),
                profile: env!("OD_BENCH_BUILD_PROFILE").into(),
                source_revision: env!("OD_BENCH_BUILD_SOURCE_REVISION").into(),
                source_sha256: env!("OD_BENCH_BUILD_SOURCE_SHA256").into(),
                source_tree_dirty: match env!("OD_BENCH_BUILD_TREE_DIRTY") {
                    "true" => Some(true),
                    "false" => Some(false),
                    _ => None,
                },
            },
            execution,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BenchmarkRunMetadata {
    pub trace_sha256: Option<String>,
    pub capture_host: Option<HostMetadata>,
    pub measure_host: HostMetadata,
}

fn command_text(program: &str, args: &[&str]) -> String {
    match command_stdout_bounded(program, args) {
        Ok(text) if !text.trim().is_empty() => text.trim().to_owned(),
        Ok(_) => "unavailable: command returned empty output".into(),
        Err(reason) => format!("unavailable: {reason}"),
    }
}

fn command_stdout_bounded(program: &str, args: &[&str]) -> Result<String, String> {
    const TIMEOUT: Duration = Duration::from_secs(2);
    const OUTPUT_LIMIT: u64 = 1024 * 1024;
    let started = Instant::now();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to spawn {program}: {error}"))?;
    let Some(stdout) = child.stdout.take() else {
        terminate_child_group(&mut child);
        return Err(format!("{program} stdout pipe unavailable"));
    };
    let output_label = program.to_owned();
    let reader = thread::Builder::new()
        .name("bench-host-metadata-reader".into())
        .spawn(move || {
            let mut output = Vec::new();
            let result = stdout
                .take(OUTPUT_LIMIT + 1)
                .read_to_end(&mut output)
                .map_err(|error| format!("failed reading {output_label} output: {error}"))
                .and_then(|_| {
                    if output.len() as u64 > OUTPUT_LIMIT {
                        Err(format!(
                            "{output_label} output exceeds {OUTPUT_LIMIT} bytes"
                        ))
                    } else {
                        Ok(String::from_utf8_lossy(&output).into_owned())
                    }
                });
            let _ = sender.send(result);
        });
    let _reader = match reader {
        Ok(reader) => reader,
        Err(error) => {
            terminate_child_group(&mut child);
            return Err(format!("failed starting {program} output reader: {error}"));
        }
    };
    let mut status = None;
    let mut output = None;
    loop {
        if status.is_none() {
            match child.try_wait() {
                Ok(Some(exit)) => status = Some(exit),
                Ok(None) => {}
                Err(error) => {
                    terminate_child_group(&mut child);
                    return Err(format!("failed waiting for {program}: {error}"));
                }
            }
        }
        if output.is_none() {
            match receiver.try_recv() {
                Ok(result) => output = Some(result),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    terminate_child_group(&mut child);
                    return Err(format!("{program} output reader disconnected"));
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if status.is_some() && output.is_some() {
            break;
        }
        if started.elapsed() >= TIMEOUT {
            terminate_child_group(&mut child);
            let _ = receiver.recv_timeout(Duration::from_millis(100));
            return Err(format!(
                "{program} timed out after {} ms",
                TIMEOUT.as_millis()
            ));
        }
        thread::sleep(Duration::from_millis(5));
    }
    let exit = status.expect("checked above");
    if !exit.success() {
        return Err(format!("{program} exited with status {exit}"));
    }
    Ok(output.expect("checked above")?.trim_end().to_owned())
}

fn terminate_child_group(child: &mut Child) {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        if let Ok(pid) = i32::try_from(child.id()) {
            // The command has its own process group, so descendants that
            // inherited stdout cannot keep the reader open after timeout.
            unsafe {
                let _ = kill(-pid, 9);
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn host_os_version() -> String {
    #[cfg(target_os = "macos")]
    {
        return command_text("sw_vers", &["-productVersion"]);
    }
    #[cfg(target_os = "linux")]
    {
        return command_text("uname", &["-sr"]);
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        "unavailable".into()
    }
}

fn host_cpu_model() -> String {
    #[cfg(target_os = "macos")]
    {
        return command_text("sysctl", &["-n", "machdep.cpu.brand_string"]);
    }
    #[cfg(target_os = "linux")]
    {
        return std::fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|contents| {
                contents.lines().find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    matches!(key.trim(), "model name" | "Hardware").then(|| value.trim().to_owned())
                })
            })
            .filter(|model| !model.is_empty())
            .unwrap_or_else(|| "unavailable".into());
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "unavailable".into())
    }
}

impl Default for BenchmarkConfig {
    fn default() -> Self {
        Self {
            sample_pairs: DEFAULT_SAMPLE_PAIRS,
            minimum_batch: MIN_BATCH_TIME,
            total_time_limit: MAX_RUN_TIME,
            process_run: 1,
            preparation_elapsed: Duration::ZERO,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LookupResult {
    FreshHit,
    StaleHit,
    Miss,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct LookupTotals {
    pub fresh_hits: u64,
    pub stale_hits: u64,
    pub misses: u64,
}

impl LookupTotals {
    fn record(&mut self, result: LookupResult) {
        match result {
            LookupResult::FreshHit => self.fresh_hits += 1,
            LookupResult::StaleHit => self.stale_hits += 1,
            LookupResult::Miss => self.misses += 1,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BenchmarkError {
    TooManyInputs {
        actual: usize,
        max: usize,
    },
    InputJsonTooLarge {
        actual: usize,
        max: usize,
    },
    TraceTooLong {
        actual: Duration,
        max: Duration,
    },
    TooManyTraceVisits {
        actual: usize,
        max: usize,
    },
    TooManyScenarios {
        actual: usize,
        max: usize,
    },
    InvalidScenarioName,
    ScenarioTooManyInputs {
        actual: usize,
        max: usize,
    },
    VisitCountMismatch {
        queries: usize,
        counted: usize,
    },
    TooManyVisitsForInput {
        input_index: usize,
        actual: usize,
        max: usize,
    },
    TooManyVisits {
        actual: usize,
        max: usize,
    },
    CodeTooLarge {
        input_index: usize,
        actual: usize,
        max: usize,
    },
    TooManyKeys {
        actual: usize,
        max: usize,
    },
    DuplicateCacheKey {
        scenario: String,
    },
    LogicalMemoryTooLarge {
        actual: usize,
        max: usize,
    },
    InvalidBenchmarkConfig,
    ColdBatchMemoryLimit {
        requested: u64,
        max: u64,
    },
    TotalTimeExceeded {
        elapsed: Duration,
        max: Duration,
    },
    EmptyQueries {
        scenario: String,
    },
    CounterOverflow,
    MalformedTrace(String),
}

impl BenchmarkSuite {
    fn logical_memory_lower_bound(&self) -> Result<usize, BenchmarkError> {
        self.scenarios.iter().try_fold(0usize, |total, scenario| {
            total
                .checked_add(scenario.logical_memory_lower_bound()?)
                .ok_or(BenchmarkError::CounterOverflow)
        })
    }

    pub fn validate(&self) -> Result<(), BenchmarkError> {
        if self.max_input_json_bytes > MAX_INPUT_JSON_BYTES {
            return Err(BenchmarkError::InputJsonTooLarge {
                actual: self.max_input_json_bytes,
                max: MAX_INPUT_JSON_BYTES,
            });
        }
        if self.trace_elapsed > MAX_TRACE_TIME {
            return Err(BenchmarkError::TraceTooLong {
                actual: self.trace_elapsed,
                max: MAX_TRACE_TIME,
            });
        }
        if self.recorded_trace_visits > MAX_TOTAL_TRACE_VISITS {
            return Err(BenchmarkError::TooManyTraceVisits {
                actual: self.recorded_trace_visits,
                max: MAX_TOTAL_TRACE_VISITS,
            });
        }
        if self.scenarios.len() > MAX_SCENARIOS {
            return Err(BenchmarkError::TooManyScenarios {
                actual: self.scenarios.len(),
                max: MAX_SCENARIOS,
            });
        }
        let mut total_logical_bytes = 0usize;
        for scenario in &self.scenarios {
            scenario.validate()?;
            total_logical_bytes = total_logical_bytes
                .checked_add(scenario.logical_memory_lower_bound()?)
                .ok_or(BenchmarkError::CounterOverflow)?;
            if total_logical_bytes > MAX_LOGICAL_BYTES {
                return Err(BenchmarkError::LogicalMemoryTooLarge {
                    actual: total_logical_bytes,
                    max: MAX_LOGICAL_BYTES,
                });
            }
        }
        Ok(())
    }
}

/// Rebuild typed keys from a trace recorded at actual dispatch entry points.
/// Decode, key construction, and trace preparation happen before any timing.
pub fn suite_from_recorded_trace(trace: &RecordedTrace) -> Result<BenchmarkSuite, BenchmarkError> {
    if trace.schema != 2 {
        return Err(BenchmarkError::MalformedTrace(format!(
            "unsupported trace schema {}",
            trace.schema
        )));
    }
    if trace.inputs.len() > MAX_INPUTS {
        return Err(BenchmarkError::TooManyInputs {
            actual: trace.inputs.len(),
            max: MAX_INPUTS,
        });
    }
    if trace.visits.len() > MAX_TOTAL_TRACE_VISITS {
        return Err(BenchmarkError::TooManyTraceVisits {
            actual: trace.visits.len(),
            max: MAX_TOTAL_TRACE_VISITS,
        });
    }

    let mut decoded_inputs = Vec::with_capacity(trace.inputs.len());
    let mut max_fixture_json_bytes = 0usize;
    let mut total_fixture_json_bytes = 0usize;
    for (index, input) in trace.inputs.iter().enumerate() {
        if input.fixture_json_bytes > MAX_INPUT_JSON_BYTES {
            return Err(BenchmarkError::InputJsonTooLarge {
                actual: input.fixture_json_bytes,
                max: MAX_INPUT_JSON_BYTES,
            });
        }
        max_fixture_json_bytes = max_fixture_json_bytes.max(input.fixture_json_bytes);
        total_fixture_json_bytes = total_fixture_json_bytes
            .checked_add(input.fixture_json_bytes)
            .ok_or(BenchmarkError::CounterOverflow)?;
        if total_fixture_json_bytes > MAX_LOGICAL_BYTES {
            return Err(BenchmarkError::LogicalMemoryTooLarge {
                actual: total_fixture_json_bytes,
                max: MAX_LOGICAL_BYTES,
            });
        }
        let code_bytes = decode_hex(&input.code_hex)
            .map_err(|reason| BenchmarkError::MalformedTrace(format!("input {index}: {reason}")))?;
        if code_bytes.len() > MAX_CODE_BYTES_PER_INPUT {
            return Err(BenchmarkError::CodeTooLarge {
                input_index: index,
                actual: code_bytes.len(),
                max: MAX_CODE_BYTES_PER_INPUT,
            });
        }
        validate_digest(&input.fixture_sha256).map_err(|reason| {
            BenchmarkError::MalformedTrace(format!("input {index} fixture digest: {reason}"))
        })?;
        validate_digest(&input.program_sha256).map_err(|reason| {
            BenchmarkError::MalformedTrace(format!("input {index} program digest: {reason}"))
        })?;
        if od_testkit::sha256_hex(&code_bytes) != input.program_sha256 {
            return Err(BenchmarkError::MalformedTrace(format!(
                "input {index} program digest does not match code bytes"
            )));
        }
        let code = CodeView::new(input.base_rip, &code_bytes)
            .map_err(|error| BenchmarkError::MalformedTrace(format!("input {index}: {error:?}")))?;
        let identity = input_identity(input)
            .map_err(|reason| BenchmarkError::MalformedTrace(format!("input {index}: {reason}")))?;
        let fixture_decode = od_isa::decode(&code_bytes).map_err(|error| {
            BenchmarkError::MalformedTrace(format!("input {index} fixture decode: {error:?}"))
        })?;
        AdmittedInstructionBoundaries::from_fixture_decode(code, &fixture_decode).map_err(
            |error| {
                BenchmarkError::MalformedTrace(format!(
                    "input {index} fixture admission: {error:?}"
                ))
            },
        )?;
        let mut boundaries = HashSet::with_capacity(fixture_decode.len() + 1);
        for (instruction_index, instruction) in fixture_decode.iter().enumerate() {
            let offset = u64::try_from(instruction.offset).map_err(|_| {
                BenchmarkError::MalformedTrace(format!(
                    "input {index} fixture instruction {instruction_index} offset overflow"
                ))
            })?;
            let rip = code.base_rip().checked_add(offset).ok_or_else(|| {
                BenchmarkError::MalformedTrace(format!(
                    "input {index} fixture instruction {instruction_index} RIP overflow"
                ))
            })?;
            boundaries.insert(rip);
        }
        boundaries.insert(code.code_end());
        decoded_inputs.push((code_bytes, identity, boundaries));
    }

    let mut visits_per_input = vec![0usize; trace.inputs.len()];
    let mut cache_entries = Vec::<TranslationKey>::new();
    let mut cache_input_indices = Vec::<usize>::new();
    let mut known_keys = HashMap::<TranslationKey, usize>::new();
    let mut queries = Vec::with_capacity(trace.visits.len());
    let mut recorded_logical_bytes = 0usize;
    for (visit_index, visit) in trace.visits.iter().enumerate() {
        let Some((bytes, identity, boundaries)) = decoded_inputs.get(visit.input_index) else {
            return Err(BenchmarkError::MalformedTrace(format!(
                "visit {visit_index} refers to missing input {}",
                visit.input_index
            )));
        };
        if !boundaries.contains(&visit.entry_rip) {
            return Err(BenchmarkError::MalformedTrace(format!(
                "visit {visit_index} entry RIP {:#x} is not a complete fixture instruction boundary",
                visit.entry_rip
            )));
        }
        visits_per_input[visit.input_index] = visits_per_input[visit.input_index]
            .checked_add(1)
            .ok_or(BenchmarkError::CounterOverflow)?;
        if visits_per_input[visit.input_index] > MAX_VISITS_PER_INPUT {
            return Err(BenchmarkError::TooManyVisitsForInput {
                input_index: visit.input_index,
                actual: visits_per_input[visit.input_index],
                max: MAX_VISITS_PER_INPUT,
            });
        }

        let input = &trace.inputs[visit.input_index];
        let code = CodeView::new(input.base_rip, bytes).map_err(|error| {
            BenchmarkError::MalformedTrace(format!("visit {visit_index}: {error:?}"))
        })?;
        let BuildOutcome::Block(block) = build_block(
            code,
            visit.entry_rip,
            AccessContext::default(),
            BlockLimits::default(),
        )
        .map_err(|error| {
            BenchmarkError::MalformedTrace(format!("visit {visit_index}: {error:?}"))
        })?
        else {
            return Err(BenchmarkError::MalformedTrace(format!(
                "visit {visit_index} produced no basic block"
            )));
        };
        for instruction in &block.instructions {
            if !boundaries.contains(&instruction.guest_rip)
                || !boundaries.contains(&instruction.next_rip)
            {
                return Err(BenchmarkError::MalformedTrace(format!(
                    "visit {visit_index} block record {:#x}..{:#x} is not covered by fixture boundaries",
                    instruction.guest_rip, instruction.next_rip
                )));
            }
        }
        let dependency = CodeDependencySpan {
            start_rip: code.base_rip(),
            end_rip: code.code_end(),
            identity: *identity,
        };
        let key = TranslationKey::for_block(
            code,
            &block,
            CompileFlavor::S0_09FixtureDispatch,
            benchmark_execution_context(),
            &[dependency],
        )
        .map_err(|error| {
            BenchmarkError::MalformedTrace(format!("visit {visit_index}: {error:?}"))
        })?;
        let key_index = match known_keys.get(&key).copied() {
            Some(index) => index,
            None => {
                if cache_entries.len() >= MAX_KEYS {
                    return Err(BenchmarkError::TooManyKeys {
                        actual: cache_entries.len() + 1,
                        max: MAX_KEYS,
                    });
                }
                let index = cache_entries.len();
                recorded_logical_bytes = recorded_logical_bytes
                    .checked_add(key_payload_lower_bound(&key))
                    .ok_or(BenchmarkError::CounterOverflow)?;
                cache_entries.push(key.clone());
                cache_input_indices.push(visit.input_index);
                known_keys.insert(key.clone(), index);
                index
            }
        };
        recorded_logical_bytes = recorded_logical_bytes
            .checked_add(key_payload_lower_bound(&cache_entries[key_index]))
            .and_then(|total| {
                total.checked_add(
                    cache_entries[key_index]
                        .dependencies()
                        .len()
                        .checked_mul(std::mem::size_of::<CodeDependencySpan>())?,
                )
            })
            .ok_or(BenchmarkError::CounterOverflow)?;
        if recorded_logical_bytes > MAX_LOGICAL_BYTES {
            return Err(BenchmarkError::LogicalMemoryTooLarge {
                actual: recorded_logical_bytes,
                max: MAX_LOGICAL_BYTES,
            });
        }
        queries.push(LookupQuery {
            input_index: visit.input_index,
            key: cache_entries[key_index].clone(),
            current_dependencies: cache_entries[key_index].dependencies().to_vec(),
        });
    }

    let recorded = ScenarioWorkload {
        name: "recorded-corpus-trace".into(),
        synthetic: false,
        cache_entries: cache_entries.clone(),
        queries,
        input_count: trace.inputs.len(),
        visits_per_input,
    };
    let mut scenarios = vec![recorded];
    let mut observed_visits_per_input = vec![0usize; trace.inputs.len()];
    let observed_queries = cache_entries
        .iter()
        .zip(&cache_input_indices)
        .map(|(key, input_index)| {
            observed_visits_per_input[*input_index] += 1;
            query_for(*input_index, key, key)
        })
        .collect();
    append_synthetic_scenarios(&mut scenarios, &cache_entries)?;
    let observed = cache_entries.len();
    // Insert beside the real trace before synthetic scaling workloads.
    scenarios.insert(
        1,
        ScenarioWorkload {
            name: format!("observed-unique-count-{observed}"),
            synthetic: true,
            cache_entries: cache_entries.clone(),
            queries: observed_queries,
            input_count: trace.inputs.len(),
            visits_per_input: observed_visits_per_input,
        },
    );
    let suite = BenchmarkSuite {
        max_input_json_bytes: max_fixture_json_bytes,
        trace_elapsed: Duration::from_nanos(trace.trace_elapsed_ns),
        recorded_trace_visits: trace.visits.len(),
        scenarios,
    };
    suite.validate()?;
    Ok(suite)
}

fn decode_hex(text: &str) -> Result<Vec<u8>, &'static str> {
    if text.len() % 2 != 0 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("code_hex must contain pairs of hexadecimal digits");
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16).ok_or("invalid hex digit")?;
            let low = (pair[1] as char).to_digit(16).ok_or("invalid hex digit")?;
            Ok(((high << 4) | low) as u8)
        })
        .collect()
}

fn validate_digest(text: &str) -> Result<(), &'static str> {
    if text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("expected 64 hexadecimal characters")
    }
}

fn input_identity(input: &RecordedInput) -> Result<CodeIdentity, &'static str> {
    Ok(CodeIdentity {
        address_space: AddressSpaceId::new(
            NonZeroU64::new(input.address_space_id).ok_or("zero address-space ID")?,
        ),
        mapping: MappingId::new(NonZeroU64::new(input.mapping_id).ok_or("zero mapping ID")?),
        mapping_version: MappingVersion::new(
            NonZeroU64::new(input.mapping_version).ok_or("zero mapping version")?,
        ),
        backing: BackingId::new(NonZeroU64::new(input.backing_id).ok_or("zero backing ID")?),
        backing_version: BackingVersion::new(
            NonZeroU64::new(input.backing_version).ok_or("zero backing version")?,
        ),
        executable: true,
    })
}

fn append_synthetic_scenarios(
    scenarios: &mut Vec<ScenarioWorkload>,
    observed_keys: &[TranslationKey],
) -> Result<(), BenchmarkError> {
    if observed_keys.is_empty() {
        return Err(BenchmarkError::MalformedTrace(
            "recorded trace has no dispatched blocks".into(),
        ));
    }
    let key_for_id = |id: u64, flavor, spans| {
        synthetic_key(
            0xa5,
            0x20_0000,
            SYNTHETIC_ID_BASE + id,
            SYNTHETIC_ID_BASE + id,
            flavor,
            spans,
            true,
        )
    };
    let synthetic_keys: Vec<_> = (0..MAX_KEYS as u64)
        .map(|id| key_for_id(id + 1, CompileFlavor::S0_09NativeBlock, 1))
        .collect::<Result<_, _>>()?;

    for count in [1usize, 8, 64, 512, 4096] {
        let keys = synthetic_keys[..count].to_vec();
        let queries = keys
            .iter()
            .enumerate()
            .rev()
            .map(|(_, key)| query_for(0, key, key))
            .collect::<Vec<_>>();
        scenarios.push(synthetic_workload(
            format!("synthetic-size-{count}"),
            keys,
            queries,
            1,
        ));
    }

    for (name, key_count) in [("hot-loop-1", 1usize), ("hot-loop-8", 8usize)] {
        let keys = synthetic_keys[..key_count].to_vec();
        let queries = (0..MAX_VISITS_PER_INPUT)
            .map(|index| {
                let key = &keys[index % keys.len()];
                query_for(0, key, key)
            })
            .collect();
        scenarios.push(synthetic_workload(
            format!("synthetic-{name}"),
            keys,
            queries,
            1,
        ));
    }

    let no_locality = synthetic_keys[..MAX_KEYS].to_vec();
    let no_locality_queries = no_locality
        .iter()
        .rev()
        .map(|key| query_for(0, key, key))
        .collect();
    scenarios.push(synthetic_workload(
        "synthetic-no-locality-unique".into(),
        no_locality,
        no_locality_queries,
        1,
    ));

    let hit_count = MAX_VISITS_PER_INPUT / 2;
    let mixed_entries = synthetic_keys[..hit_count].to_vec();
    let absent_keys = (0..hit_count)
        .map(|id| key_for_id(10_000 + id as u64, CompileFlavor::S0_09FixtureDispatch, 1))
        .collect::<Result<Vec<_>, _>>()?;
    let mut mixed_queries = Vec::with_capacity(MAX_VISITS_PER_INPUT);
    for index in 0..hit_count {
        mixed_queries.push(query_for(0, &mixed_entries[index], &mixed_entries[index]));
        mixed_queries.push(query_for(0, &absent_keys[index], &absent_keys[index]));
    }
    scenarios.push(synthetic_workload(
        "synthetic-mixed-hit-miss".into(),
        mixed_entries,
        mixed_queries,
        1,
    ));

    let stale_entries = synthetic_keys[..8].to_vec();
    for stale_kind in ["mapping", "backing", "permission"] {
        let queries = stale_entries
            .iter()
            .map(|key| {
                let mut current = key.dependencies().to_vec();
                for dependency in &mut current {
                    match stale_kind {
                        "mapping" => {
                            dependency.identity.mapping_version = dependency
                                .identity
                                .mapping_version
                                .checked_next()
                                .ok_or(BenchmarkError::CounterOverflow)?;
                        }
                        "backing" => {
                            dependency.identity.backing_version = dependency
                                .identity
                                .backing_version
                                .checked_next()
                                .ok_or(BenchmarkError::CounterOverflow)?;
                        }
                        _ => dependency.identity.executable = false,
                    }
                }
                Ok(LookupQuery {
                    input_index: 0,
                    key: key.clone(),
                    current_dependencies: current,
                })
            })
            .collect::<Result<Vec<_>, BenchmarkError>>()?;
        scenarios.push(synthetic_workload(
            format!("synthetic-stale-{stale_kind}"),
            stale_entries.clone(),
            queries,
            1,
        ));
    }

    let alias_a = synthetic_key(
        7,
        0x30_0000,
        SYNTHETIC_ID_BASE + 30_001,
        SYNTHETIC_ID_BASE + 40_001,
        CompileFlavor::S0_09NativeBlock,
        1,
        true,
    )?;
    let alias_b = synthetic_key(
        7,
        0x40_0000,
        SYNTHETIC_ID_BASE + 30_002,
        SYNTHETIC_ID_BASE + 40_001,
        CompileFlavor::S0_09NativeBlock,
        1,
        true,
    )?;
    scenarios.push(ScenarioWorkload {
        name: "synthetic-alias-shared-backing".into(),
        synthetic: true,
        cache_entries: vec![alias_a.clone(), alias_b.clone()],
        queries: vec![
            query_for(0, &alias_a, &alias_a),
            query_for(1, &alias_b, &alias_b),
        ],
        input_count: 2,
        visits_per_input: vec![1, 1],
    });

    for span_count in [1usize, 2, 4] {
        let key = synthetic_key(
            0x5a,
            0x50_0000,
            SYNTHETIC_ID_BASE + 50_000 + span_count as u64,
            SYNTHETIC_ID_BASE + 60_000,
            CompileFlavor::S0_09NativeBlock,
            span_count,
            true,
        )?;
        scenarios.push(synthetic_workload(
            format!("synthetic-dependency-spans-{span_count}"),
            vec![key.clone()],
            vec![query_for(0, &key, &key)],
            1,
        ));
    }

    if scenarios.len() > MAX_SCENARIOS {
        return Err(BenchmarkError::TooManyScenarios {
            actual: scenarios.len(),
            max: MAX_SCENARIOS,
        });
    }
    Ok(())
}

const SYNTHETIC_ID_BASE: u64 = 1 << 60;

fn synthetic_key(
    immediate: u8,
    base_rip: u64,
    mapping_id: u64,
    backing_id: u64,
    flavor: CompileFlavor,
    span_count: usize,
    executable: bool,
) -> Result<TranslationKey, BenchmarkError> {
    let bytes = [0x48, 0xb8, immediate, 0, 0, 0, 0, 0, 0, 0];
    let code = CodeView::new(base_rip, &bytes)
        .map_err(|error| BenchmarkError::MalformedTrace(format!("synthetic code: {error:?}")))?;
    let BuildOutcome::Block(block) = build_block(
        code,
        base_rip,
        AccessContext::default(),
        BlockLimits::default(),
    )
    .map_err(|error| BenchmarkError::MalformedTrace(format!("synthetic block: {error:?}")))?
    else {
        return Err(BenchmarkError::MalformedTrace(
            "synthetic instruction produced no block".into(),
        ));
    };
    if span_count == 0 || span_count > bytes.len() {
        return Err(BenchmarkError::MalformedTrace(
            "synthetic dependency span count is invalid".into(),
        ));
    }
    let identity = CodeIdentity {
        address_space: AddressSpaceId::new(
            NonZeroU64::new(SYNTHETIC_ID_BASE).expect("constant is nonzero"),
        ),
        mapping: MappingId::new(
            NonZeroU64::new(mapping_id).ok_or(BenchmarkError::CounterOverflow)?,
        ),
        mapping_version: MappingVersion::new(NonZeroU64::new(1).unwrap()),
        backing: BackingId::new(
            NonZeroU64::new(backing_id).ok_or(BenchmarkError::CounterOverflow)?,
        ),
        backing_version: BackingVersion::new(NonZeroU64::new(1).unwrap()),
        executable,
    };
    let dependencies = (0..span_count)
        .map(|index| {
            let start_offset = bytes.len() * index / span_count;
            let end_offset = bytes.len() * (index + 1) / span_count;
            CodeDependencySpan {
                start_rip: base_rip + start_offset as u64,
                end_rip: base_rip + end_offset as u64,
                identity,
            }
        })
        .collect::<Vec<_>>();
    TranslationKey::for_block(
        code,
        &block,
        flavor,
        benchmark_execution_context(),
        &dependencies,
    )
    .map_err(|error| BenchmarkError::MalformedTrace(format!("synthetic key: {error:?}")))
}

fn query_for(input_index: usize, key: &TranslationKey, current: &TranslationKey) -> LookupQuery {
    LookupQuery {
        input_index,
        key: key.clone(),
        current_dependencies: current.dependencies().to_vec(),
    }
}

fn synthetic_workload(
    name: String,
    cache_entries: Vec<TranslationKey>,
    queries: Vec<LookupQuery>,
    input_count: usize,
) -> ScenarioWorkload {
    let mut visits_per_input = vec![0usize; input_count];
    for query in &queries {
        if let Some(visits) = visits_per_input.get_mut(query.input_index) {
            *visits += 1;
        }
    }
    ScenarioWorkload {
        name,
        synthetic: true,
        cache_entries,
        queries,
        input_count,
        visits_per_input,
    }
}

impl ScenarioWorkload {
    fn logical_memory_lower_bound(&self) -> Result<usize, BenchmarkError> {
        let mut total = std::mem::size_of::<ScenarioWorkload>()
            .checked_add(self.name.capacity())
            .and_then(|value| {
                value.checked_add(
                    self.cache_entries
                        .capacity()
                        .checked_mul(std::mem::size_of::<TranslationKey>())?,
                )
            })
            .and_then(|value| {
                value.checked_add(
                    self.queries
                        .capacity()
                        .checked_mul(std::mem::size_of::<LookupQuery>())?,
                )
            })
            .and_then(|value| {
                value.checked_add(
                    self.visits_per_input
                        .capacity()
                        .checked_mul(std::mem::size_of::<usize>())?,
                )
            })
            .ok_or(BenchmarkError::CounterOverflow)?;
        for key in &self.cache_entries {
            total = total
                .checked_add(key_owned_buffer_bytes(key))
                .ok_or(BenchmarkError::CounterOverflow)?;
        }
        for query in &self.queries {
            total = total
                .checked_add(key_owned_buffer_bytes(&query.key))
                .and_then(|value| {
                    value.checked_add(
                        query
                            .current_dependencies
                            .capacity()
                            .checked_mul(std::mem::size_of::<CodeDependencySpan>())?,
                    )
                })
                .ok_or(BenchmarkError::CounterOverflow)?;
        }
        Ok(total)
    }

    pub fn validate(&self) -> Result<(), BenchmarkError> {
        if self.name.trim().is_empty() {
            return Err(BenchmarkError::InvalidScenarioName);
        }
        if self.input_count > MAX_INPUTS {
            return Err(BenchmarkError::ScenarioTooManyInputs {
                actual: self.input_count,
                max: MAX_INPUTS,
            });
        }
        if self.visits_per_input.len() != self.input_count {
            return Err(BenchmarkError::VisitCountMismatch {
                queries: self.queries.len(),
                counted: self.visits_per_input.iter().sum(),
            });
        }
        let mut counted_visits = 0usize;
        for (input_index, visits) in self.visits_per_input.iter().copied().enumerate() {
            if visits > MAX_VISITS_PER_INPUT {
                return Err(BenchmarkError::TooManyVisitsForInput {
                    input_index,
                    actual: visits,
                    max: MAX_VISITS_PER_INPUT,
                });
            }
            counted_visits = counted_visits
                .checked_add(visits)
                .ok_or(BenchmarkError::CounterOverflow)?;
        }
        if counted_visits != self.queries.len() {
            return Err(BenchmarkError::VisitCountMismatch {
                queries: self.queries.len(),
                counted: counted_visits,
            });
        }
        if self.queries.len() > MAX_TOTAL_TRACE_VISITS {
            return Err(BenchmarkError::TooManyVisits {
                actual: self.queries.len(),
                max: MAX_TOTAL_TRACE_VISITS,
            });
        }
        if self.queries.is_empty() {
            return Err(BenchmarkError::EmptyQueries {
                scenario: self.name.clone(),
            });
        }
        if self.cache_entries.len() > MAX_KEYS {
            return Err(BenchmarkError::TooManyKeys {
                actual: self.cache_entries.len(),
                max: MAX_KEYS,
            });
        }
        let mut unique = HashSet::with_capacity(self.cache_entries.len());
        let mut logical_bytes = 0usize;
        for key in &self.cache_entries {
            if key.code_bytes().len() > MAX_CODE_BYTES_PER_INPUT {
                return Err(BenchmarkError::CodeTooLarge {
                    input_index: 0,
                    actual: key.code_bytes().len(),
                    max: MAX_CODE_BYTES_PER_INPUT,
                });
            }
            if !unique.insert(key) {
                return Err(BenchmarkError::DuplicateCacheKey {
                    scenario: self.name.clone(),
                });
            }
            logical_bytes = logical_bytes
                .checked_add(key_payload_lower_bound(key))
                .ok_or(BenchmarkError::CounterOverflow)?;
        }
        let mut input_code = HashMap::<usize, &TranslationKey>::new();
        for query in &self.queries {
            if query.input_index >= self.input_count {
                return Err(BenchmarkError::ScenarioTooManyInputs {
                    actual: query.input_index.saturating_add(1),
                    max: self.input_count,
                });
            }
            if query.key.code_bytes().len() > MAX_CODE_BYTES_PER_INPUT {
                return Err(BenchmarkError::CodeTooLarge {
                    input_index: query.input_index,
                    actual: query.key.code_bytes().len(),
                    max: MAX_CODE_BYTES_PER_INPUT,
                });
            }
            if input_code
                .insert(query.input_index, &query.key)
                .is_some_and(|previous| {
                    previous.code_base() != query.key.code_base()
                        || previous.code_end() != query.key.code_end()
                        || previous.code_bytes() != query.key.code_bytes()
                })
            {
                return Err(BenchmarkError::MalformedTrace(format!(
                    "scenario {} assigns different bytes to input {}",
                    self.name, query.input_index
                )));
            }
            logical_bytes = logical_bytes
                .checked_add(key_payload_lower_bound(&query.key))
                .and_then(|value| {
                    value.checked_add(
                        query
                            .current_dependencies
                            .len()
                            .checked_mul(std::mem::size_of::<CodeDependencySpan>())?,
                    )
                })
                .ok_or(BenchmarkError::CounterOverflow)?;
        }
        if logical_bytes > MAX_LOGICAL_BYTES {
            return Err(BenchmarkError::LogicalMemoryTooLarge {
                actual: logical_bytes,
                max: MAX_LOGICAL_BYTES,
            });
        }
        Ok(())
    }
}

fn key_payload_lower_bound(key: &TranslationKey) -> usize {
    key_owned_buffer_bytes(key)
}

fn key_owned_buffer_bytes(key: &TranslationKey) -> usize {
    key.code_bytes().len()
        + key
            .dependencies()
            .len()
            .saturating_mul(std::mem::size_of::<CodeDependencySpan>())
        + key
            .fetch_ranges()
            .len()
            .saturating_mul(std::mem::size_of::<od_jit::block::GuestByteRange>())
}

/// Conservative element-capacity bound for the standard library's current
/// power-of-two HashMap bucket growth, including its minimum four-bucket
/// allocation for nonempty maps. It is used for admission/accounting; the
/// measured map reports its actual public capacity separately.
fn logical_hash_map_capacity_bound(entries: usize) -> usize {
    if entries == 0 {
        0
    } else {
        entries
            .checked_next_power_of_two()
            .unwrap_or(usize::MAX)
            .saturating_mul(2)
            .max(4)
    }
}

fn lookup_in_cached_key(key: &TranslationKey, query: &LookupQuery) -> LookupResult {
    match key.dependencies_match_snapshot(&query.current_dependencies) {
        Ok(true) => LookupResult::FreshHit,
        Ok(false) | Err(_) => LookupResult::StaleHit,
    }
}

pub fn lookup_vec(entries: &[TranslationKey], query: &LookupQuery) -> LookupResult {
    entries
        .iter()
        .find(|candidate| *candidate == &query.key)
        .map_or(LookupResult::Miss, |cached| {
            lookup_in_cached_key(cached, query)
        })
}

pub fn lookup_hash_map(entries: &HashMap<TranslationKey, ()>, query: &LookupQuery) -> LookupResult {
    entries
        .get_key_value(&query.key)
        .map_or(LookupResult::Miss, |(cached, ())| {
            lookup_in_cached_key(cached, query)
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct RawSample {
    /// Sum of construction-only timer intervals. Destruction and inter-batch
    /// pauses are excluded; the report's total elapsed time includes them.
    pub elapsed_ns: u64,
    pub operations: u64,
    /// Number of bounded construction batches used by this sample. Warm
    /// lookup samples use one batch.
    pub batches: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Distribution {
    pub median_ns_per_operation: u64,
    pub min_ns_per_operation: u64,
    pub max_ns_per_operation: u64,
    pub iqr_ns_per_operation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TimedSeries {
    pub samples: Vec<RawSample>,
    pub distribution: Distribution,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct StorageCapacity {
    pub entry_capacity: usize,
    /// Payload size lower bound; allocator metadata/control bytes are omitted.
    pub logical_payload_lower_bound_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ScenarioReport {
    pub name: String,
    pub synthetic: bool,
    pub process_run: u8,
    pub calibration_repetitions: CalibrationRepetitions,
    pub cold_vec: TimedSeries,
    pub cold_hash_map: TimedSeries,
    pub warm_vec: TimedSeries,
    pub warm_hash_map: TimedSeries,
    pub vec_capacity: StorageCapacity,
    pub hash_capacity: StorageCapacity,
    pub expected_totals: LookupTotals,
    pub rss_note: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct CalibrationRepetitions {
    pub cold_vec: u64,
    pub cold_hash_map: u64,
    pub warm_vec: u64,
    pub warm_hash_map: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BenchmarkReport {
    pub schema: u32,
    pub evidence_scope: String,
    pub trace_sha256: Option<String>,
    pub capture_host: Option<HostMetadata>,
    pub measure_host: HostMetadata,
    pub process_run: u8,
    pub recorded_trace_visits: usize,
    pub trace_elapsed_ns: u64,
    pub preparation_elapsed_ns: u64,
    pub scenarios: Vec<ScenarioReport>,
    pub total_elapsed_ns: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedTrace {
    pub schema: u32,
    pub capture_host: HostMetadata,
    pub trace_elapsed_ns: u64,
    pub inputs: Vec<RecordedInput>,
    pub visits: Vec<RecordedVisit>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedInput {
    pub case_id: String,
    pub fixture_sha256: String,
    pub fixture_json_bytes: usize,
    pub code_hex: String,
    pub program_sha256: String,
    pub base_rip: u64,
    pub address_space_id: u64,
    pub mapping_id: u64,
    pub mapping_version: u64,
    pub backing_id: u64,
    pub backing_version: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedVisit {
    pub input_index: usize,
    pub entry_rip: u64,
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod record;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod cached_flags_compare;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod cached_native_flags_compare;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod dispatch_cache;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod flags_compare;

pub fn run_suite(
    suite: &BenchmarkSuite,
    config: BenchmarkConfig,
) -> Result<BenchmarkReport, BenchmarkError> {
    run_suite_with_metadata(suite, config, BenchmarkRunMetadata::default())
}

pub fn run_suite_with_metadata(
    suite: &BenchmarkSuite,
    config: BenchmarkConfig,
    metadata: BenchmarkRunMetadata,
) -> Result<BenchmarkReport, BenchmarkError> {
    if !(DEFAULT_SAMPLE_PAIRS..=MAX_SAMPLE_PAIRS).contains(&config.sample_pairs)
        || config.minimum_batch < MIN_BATCH_TIME
        || config.total_time_limit > MAX_RUN_TIME
        || config.total_time_limit.is_zero()
        || !(1..=3).contains(&config.process_run)
        || config.preparation_elapsed > config.total_time_limit
    {
        return Err(BenchmarkError::InvalidBenchmarkConfig);
    }

    let started = Instant::now();
    check_duration(config.preparation_elapsed, config.total_time_limit)?;
    suite.validate()?;
    let suite_resident = suite.logical_memory_lower_bound()?;
    check_deadline(started, config.preparation_elapsed, config.total_time_limit)?;
    let mut reports = Vec::with_capacity(suite.scenarios.len());
    for scenario in &suite.scenarios {
        check_deadline(started, config.preparation_elapsed, config.total_time_limit)?;
        reports.push(run_scenario(scenario, suite_resident, config, started)?);
    }
    let total_elapsed = started.elapsed();
    let total_elapsed = config.preparation_elapsed.saturating_add(total_elapsed);
    check_duration(total_elapsed, config.total_time_limit)?;
    Ok(BenchmarkReport {
        schema: 3,
        evidence_scope: if suite.recorded_trace_visits == 0 {
            "synthetic_prototype".into()
        } else {
            "recorded_fixture_trace_plus_synthetic_scenarios".into()
        },
        trace_sha256: metadata.trace_sha256,
        capture_host: metadata.capture_host,
        measure_host: metadata.measure_host,
        process_run: config.process_run,
        recorded_trace_visits: suite.recorded_trace_visits,
        trace_elapsed_ns: duration_ns(suite.trace_elapsed),
        preparation_elapsed_ns: duration_ns(config.preparation_elapsed),
        scenarios: reports,
        total_elapsed_ns: duration_ns(total_elapsed),
    })
}

fn run_scenario(
    scenario: &ScenarioWorkload,
    suite_resident_bytes: usize,
    config: BenchmarkConfig,
    started: Instant,
) -> Result<ScenarioReport, BenchmarkError> {
    let map_capacity_bound = logical_hash_map_capacity_bound(scenario.cache_entries.len());
    let key_buffers_bytes = scenario
        .cache_entries
        .iter()
        .map(key_owned_buffer_bytes)
        .sum::<usize>();
    let execution_tables_bytes = std::mem::size_of::<Vec<TranslationKey>>()
        .saturating_add(
            scenario
                .cache_entries
                .len()
                .saturating_mul(std::mem::size_of::<TranslationKey>()),
        )
        .saturating_add(key_buffers_bytes)
        .saturating_add(std::mem::size_of::<HashMap<TranslationKey, ()>>())
        .saturating_add(
            map_capacity_bound.saturating_mul(std::mem::size_of::<(TranslationKey, ())>()),
        )
        .saturating_add(key_buffers_bytes);
    let cold_memory_budget = MAX_LOGICAL_BYTES
        .saturating_sub(suite_resident_bytes)
        .saturating_sub(execution_tables_bytes);
    let max_vec_batch =
        cold_batch_repetition_limit(&scenario.cache_entries, false, cold_memory_budget, 0);
    let max_map_batch = cold_batch_repetition_limit(
        &scenario.cache_entries,
        true,
        cold_memory_budget,
        map_capacity_bound,
    );
    if max_vec_batch == 0 || max_map_batch == 0 {
        return Err(BenchmarkError::ColdBatchMemoryLimit {
            requested: 1,
            max: max_vec_batch.min(max_map_batch),
        });
    }

    // Check the live-memory budget before allocating the execution tables.
    let mut entries_vec = scenario.cache_entries.clone();
    let mut entries_map: HashMap<TranslationKey, ()> = scenario
        .cache_entries
        .iter()
        .cloned()
        .map(|key| (key, ()))
        .collect();
    let vec_capacity = StorageCapacity {
        entry_capacity: entries_vec.capacity(),
        logical_payload_lower_bound_bytes: scenario
            .cache_entries
            .iter()
            .map(key_payload_lower_bound)
            .sum(),
    };
    let hash_capacity = StorageCapacity {
        entry_capacity: entries_map.capacity(),
        logical_payload_lower_bound_bytes: vec_capacity.logical_payload_lower_bound_bytes,
    };
    let totals_vec = run_queries_vec(&entries_vec, &scenario.queries, 1);
    let totals_map = run_queries_map(&entries_map, &scenario.queries, 1);
    if totals_vec != totals_map {
        return Err(BenchmarkError::InvalidScenarioName);
    }
    let expected_totals = totals_vec;

    let repetitions = CalibrationRepetitions {
        cold_vec: calibrate_repetitions(
            started,
            config,
            |repetitions| {
                time_cold_build_vec(&scenario.cache_entries, repetitions, cold_memory_budget)
                    .elapsed
            },
            cold_batch_repetition_limit(&scenario.cache_entries, false, cold_memory_budget, 0),
        )?,
        cold_hash_map: calibrate_repetitions(
            started,
            config,
            |repetitions| {
                time_cold_build_map(
                    &scenario.cache_entries,
                    repetitions,
                    cold_memory_budget,
                    map_capacity_bound,
                )
                .elapsed
            },
            cold_batch_repetition_limit(
                &scenario.cache_entries,
                true,
                cold_memory_budget,
                map_capacity_bound,
            ),
        )?,
        warm_vec: calibrate_repetitions(
            started,
            config,
            |repetitions| time_warm_vec(&entries_vec, &scenario.queries, repetitions).elapsed,
            u64::MAX,
        )?,
        warm_hash_map: calibrate_repetitions(
            started,
            config,
            |repetitions| time_warm_map(&entries_map, &scenario.queries, repetitions).elapsed,
            u64::MAX,
        )?,
    };

    let mut cold_vec_samples = Vec::with_capacity(config.sample_pairs);
    let mut cold_map_samples = Vec::with_capacity(config.sample_pairs);
    let mut warm_vec_samples = Vec::with_capacity(config.sample_pairs);
    let mut warm_map_samples = Vec::with_capacity(config.sample_pairs);
    for pair in 0..config.sample_pairs {
        check_deadline(started, config.preparation_elapsed, config.total_time_limit)?;
        if pair % 2 == 0 {
            cold_vec_samples.push(sample(time_cold_build_vec(
                &scenario.cache_entries,
                repetitions.cold_vec,
                cold_memory_budget,
            ))?);
            cold_map_samples.push(sample(time_cold_build_map(
                &scenario.cache_entries,
                repetitions.cold_hash_map,
                cold_memory_budget,
                map_capacity_bound,
            ))?);
            warm_vec_samples.push(sample(time_warm_vec(
                &entries_vec,
                &scenario.queries,
                repetitions.warm_vec,
            ))?);
            warm_map_samples.push(sample(time_warm_map(
                &entries_map,
                &scenario.queries,
                repetitions.warm_hash_map,
            ))?);
        } else {
            warm_map_samples.push(sample(time_warm_map(
                &entries_map,
                &scenario.queries,
                repetitions.warm_hash_map,
            ))?);
            warm_vec_samples.push(sample(time_warm_vec(
                &entries_vec,
                &scenario.queries,
                repetitions.warm_vec,
            ))?);
            cold_map_samples.push(sample(time_cold_build_map(
                &scenario.cache_entries,
                repetitions.cold_hash_map,
                cold_memory_budget,
                map_capacity_bound,
            ))?);
            cold_vec_samples.push(sample(time_cold_build_vec(
                &scenario.cache_entries,
                repetitions.cold_vec,
                cold_memory_budget,
            ))?);
        }
    }
    // Keep the warmed structures live through measurements and make sure the
    // optimizer cannot discard their contents.
    black_box(&mut entries_vec);
    black_box(&mut entries_map);
    Ok(ScenarioReport {
        name: scenario.name.clone(),
        synthetic: scenario.synthetic,
        process_run: config.process_run,
        calibration_repetitions: repetitions,
        cold_vec: summarize(cold_vec_samples)?,
        cold_hash_map: summarize(cold_map_samples)?,
        warm_vec: summarize(warm_vec_samples)?,
        warm_hash_map: summarize(warm_map_samples)?,
        vec_capacity,
        hash_capacity,
        expected_totals,
        rss_note: "the 128 MiB logical budget includes inline key/table slots and owned key buffers across resident suite state, execution tables, and one cold micro-batch; allocator metadata, HashMap control bytes, runtime overhead, and process RSS are not measured".into(),
    })
}

fn calibrate_repetitions(
    started: Instant,
    config: BenchmarkConfig,
    mut measure: impl FnMut(u64) -> Duration,
    max_batch_repetitions: u64,
) -> Result<u64, BenchmarkError> {
    check_deadline(started, config.preparation_elapsed, config.total_time_limit)?;
    if max_batch_repetitions == 0 {
        return Err(BenchmarkError::ColdBatchMemoryLimit {
            requested: 1,
            max: 0,
        });
    }
    let mut repetitions = 1u64;
    loop {
        check_deadline(started, config.preparation_elapsed, config.total_time_limit)?;
        let elapsed = measure(repetitions);
        check_deadline(started, config.preparation_elapsed, config.total_time_limit)?;
        if elapsed >= config.minimum_batch {
            return Ok(repetitions);
        }
        repetitions = repetitions
            .checked_mul(2)
            .ok_or(BenchmarkError::CounterOverflow)?;
        // Cold construction partitions repetitions into memory-bounded
        // micro-batches. The cap bounds peak retained tables, not total work.
    }
}

fn cold_batch_repetition_limit(
    keys: &[TranslationKey],
    hash_map: bool,
    logical_budget: usize,
    map_capacity: usize,
) -> u64 {
    let key_buffers = keys.iter().fold(0usize, |total, key| {
        total.saturating_add(key_owned_buffer_bytes(key))
    });
    let per_table = if hash_map {
        std::mem::size_of::<HashMap<TranslationKey, ()>>()
            .saturating_add(
                map_capacity.saturating_mul(std::mem::size_of::<(TranslationKey, ())>()),
            )
            .saturating_add(key_buffers)
    } else {
        std::mem::size_of::<Vec<TranslationKey>>()
            .saturating_add(
                keys.len()
                    .saturating_mul(std::mem::size_of::<TranslationKey>()),
            )
            .saturating_add(key_buffers)
    };
    if per_table == 0 {
        0
    } else {
        u64::try_from(logical_budget / per_table).unwrap_or(u64::MAX)
    }
}

struct TimedBatch {
    elapsed: Duration,
    operations: u64,
    batches: u64,
}

fn time_cold_build_vec(
    keys: &[TranslationKey],
    repetitions: u64,
    logical_budget: usize,
) -> TimedBatch {
    let mut operations = 0u64;
    let batch_limit = cold_batch_repetition_limit(keys, false, logical_budget, 0).max(1);
    let mut elapsed = Duration::ZERO;
    let mut completed = 0u64;
    let mut batches = 0u64;
    while completed < repetitions {
        let count = (repetitions - completed).min(batch_limit);
        let mut tables = Vec::with_capacity(usize::try_from(count).unwrap_or(usize::MAX));
        let started = Instant::now();
        for _ in 0..count {
            let constructed = keys.to_vec();
            operations = operations.saturating_add(constructed.len() as u64);
            tables.push(constructed);
        }
        black_box(&tables);
        elapsed = elapsed.saturating_add(started.elapsed());
        drop(tables);
        completed += count;
        batches = batches.saturating_add(1);
    }
    TimedBatch {
        elapsed,
        operations,
        batches,
    }
}

fn time_cold_build_map(
    keys: &[TranslationKey],
    repetitions: u64,
    logical_budget: usize,
    map_capacity: usize,
) -> TimedBatch {
    let mut operations = 0u64;
    let batch_limit = cold_batch_repetition_limit(keys, true, logical_budget, map_capacity).max(1);
    let mut elapsed = Duration::ZERO;
    let mut completed = 0u64;
    let mut batches = 0u64;
    while completed < repetitions {
        let count = (repetitions - completed).min(batch_limit);
        let mut tables = Vec::with_capacity(usize::try_from(count).unwrap_or(usize::MAX));
        let started = Instant::now();
        for _ in 0..count {
            let mut constructed = HashMap::with_capacity(keys.len());
            for key in keys {
                constructed.insert(key.clone(), ());
            }
            operations = operations.saturating_add(constructed.len() as u64);
            tables.push(constructed);
        }
        black_box(&tables);
        elapsed = elapsed.saturating_add(started.elapsed());
        drop(tables);
        completed += count;
        batches = batches.saturating_add(1);
    }
    TimedBatch {
        elapsed,
        operations,
        batches,
    }
}

fn time_warm_vec(keys: &[TranslationKey], queries: &[LookupQuery], repetitions: u64) -> TimedBatch {
    let started = Instant::now();
    let totals = run_queries_vec(keys, queries, repetitions);
    black_box(totals);
    TimedBatch {
        elapsed: started.elapsed(),
        operations: (queries.len() as u64).saturating_mul(repetitions),
        batches: 1,
    }
}

fn time_warm_map(
    keys: &HashMap<TranslationKey, ()>,
    queries: &[LookupQuery],
    repetitions: u64,
) -> TimedBatch {
    let started = Instant::now();
    let totals = run_queries_map(keys, queries, repetitions);
    black_box(totals);
    TimedBatch {
        elapsed: started.elapsed(),
        operations: (queries.len() as u64).saturating_mul(repetitions),
        batches: 1,
    }
}

fn run_queries_vec(
    keys: &[TranslationKey],
    queries: &[LookupQuery],
    repetitions: u64,
) -> LookupTotals {
    let mut totals = LookupTotals::default();
    for _ in 0..repetitions {
        for query in queries {
            totals.record(lookup_vec(keys, query));
        }
    }
    totals
}

fn run_queries_map(
    keys: &HashMap<TranslationKey, ()>,
    queries: &[LookupQuery],
    repetitions: u64,
) -> LookupTotals {
    let mut totals = LookupTotals::default();
    for _ in 0..repetitions {
        for query in queries {
            totals.record(lookup_hash_map(keys, query));
        }
    }
    totals
}

fn sample(batch: TimedBatch) -> Result<RawSample, BenchmarkError> {
    if batch.operations == 0 {
        return Err(BenchmarkError::CounterOverflow);
    }
    Ok(RawSample {
        elapsed_ns: duration_ns(batch.elapsed),
        operations: batch.operations,
        batches: batch.batches,
    })
}

fn summarize(samples: Vec<RawSample>) -> Result<TimedSeries, BenchmarkError> {
    let mut per_operation = Vec::with_capacity(samples.len());
    for sample in &samples {
        if sample.operations == 0 {
            return Err(BenchmarkError::CounterOverflow);
        }
        per_operation.push(sample.elapsed_ns / sample.operations);
    }
    per_operation.sort_unstable();
    let len = per_operation.len();
    if len == 0 {
        return Err(BenchmarkError::CounterOverflow);
    }
    let median = if len.is_multiple_of(2) {
        let lower = per_operation[len / 2 - 1];
        let upper = per_operation[len / 2];
        lower + (upper - lower) / 2
    } else {
        per_operation[len / 2]
    };
    let q1 = per_operation[len / 4];
    let q3 = per_operation[(len * 3 / 4).min(len - 1)];
    Ok(TimedSeries {
        samples,
        distribution: Distribution {
            median_ns_per_operation: median,
            min_ns_per_operation: per_operation[0],
            max_ns_per_operation: per_operation[len - 1],
            iqr_ns_per_operation: q3.saturating_sub(q1),
        },
    })
}

fn check_deadline(
    started: Instant,
    prior_elapsed: Duration,
    limit: Duration,
) -> Result<(), BenchmarkError> {
    check_duration(prior_elapsed.saturating_add(started.elapsed()), limit)
}

fn check_duration(elapsed: Duration, limit: Duration) -> Result<(), BenchmarkError> {
    if elapsed > limit {
        return Err(BenchmarkError::TotalTimeExceeded {
            elapsed,
            max: limit,
        });
    }
    Ok(())
}

fn duration_ns(duration: Duration) -> u64 {
    duration.as_nanos().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use od_isa::memory::AccessContext;
    use od_jit::block::{BlockLimits, BuildOutcome, CodeView, build_block};
    use od_jit::translation::{
        AddressSpaceId, BackingId, BackingVersion, CodeIdentity, CompileFlavor, MappingId,
        MappingVersion,
    };
    use std::num::NonZeroU64;

    #[cfg(unix)]
    #[test]
    fn host_metadata_timeout_covers_descendants_holding_stdout() {
        let started = Instant::now();
        let result = command_stdout_bounded("/bin/sh", &["-c", "(sleep 30) & exit 0"]);
        assert!(matches!(result, Err(message) if message.contains("timed out")));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    fn key(value: u8, mapping_version: u64, backing_version: u64) -> TranslationKey {
        let bytes = [0x48, 0xb8, value, 0, 0, 0, 0, 0, 0, 0];
        let code = CodeView::new(0x1000, &bytes).unwrap();
        let BuildOutcome::Block(block) = build_block(
            code,
            0x1000,
            AccessContext::default(),
            BlockLimits::default(),
        )
        .unwrap() else {
            panic!("expected compiled prefix");
        };
        let identity = CodeIdentity {
            address_space: AddressSpaceId::new(NonZeroU64::new(1).unwrap()),
            mapping: MappingId::new(NonZeroU64::new(1).unwrap()),
            mapping_version: MappingVersion::new(NonZeroU64::new(mapping_version).unwrap()),
            backing: BackingId::new(NonZeroU64::new(2).unwrap()),
            backing_version: BackingVersion::new(NonZeroU64::new(backing_version).unwrap()),
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
        .unwrap()
    }

    fn query(input_index: usize, key: &TranslationKey, current: &TranslationKey) -> LookupQuery {
        LookupQuery {
            input_index,
            key: key.clone(),
            current_dependencies: current.dependencies().to_vec(),
        }
    }

    fn one_visit_trace() -> RecordedTrace {
        let code = [0x48, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0];
        RecordedTrace {
            schema: 2,
            capture_host: HostMetadata::default(),
            trace_elapsed_ns: 1,
            inputs: vec![RecordedInput {
                case_id: "one".into(),
                fixture_sha256: "00".repeat(32),
                fixture_json_bytes: 128,
                code_hex: code.iter().map(|byte| format!("{byte:02x}")).collect(),
                program_sha256: od_testkit::sha256_hex(&code),
                base_rip: 0x1000,
                address_space_id: 1,
                mapping_id: 1,
                mapping_version: 1,
                backing_id: 1,
                backing_version: 1,
            }],
            visits: vec![RecordedVisit {
                input_index: 0,
                entry_rip: 0x1000,
            }],
        }
    }

    #[test]
    fn vec_and_hashmap_match_for_hit_miss_and_stale_snapshot() {
        let cached = key(1, 1, 1);
        let changed_backing = key(1, 1, 2);
        let absent = key(2, 1, 1);
        let vec = vec![cached.clone()];
        let map = HashMap::from([(cached.clone(), ())]);

        for (query, expected) in [
            (query(0, &cached, &cached), LookupResult::FreshHit),
            (query(0, &cached, &changed_backing), LookupResult::StaleHit),
            (query(0, &absent, &absent), LookupResult::Miss),
        ] {
            assert_eq!(lookup_vec(&vec, &query), expected);
            assert_eq!(lookup_hash_map(&map, &query), expected);
        }
    }

    #[test]
    fn bounded_limits_reject_oversize_inputs_and_invalid_sample_config() {
        let key = key(1, 1, 1);
        let query = query(0, &key, &key);
        let scenario = ScenarioWorkload {
            name: "one".into(),
            synthetic: true,
            cache_entries: vec![key],
            queries: vec![query],
            input_count: 1,
            visits_per_input: vec![1],
        };
        assert!(scenario.validate().is_ok());
        let too_many = ScenarioWorkload {
            visits_per_input: vec![MAX_VISITS_PER_INPUT + 1],
            ..scenario.clone()
        };
        assert!(matches!(
            too_many.validate(),
            Err(BenchmarkError::TooManyVisitsForInput { .. })
        ));
        let suite = BenchmarkSuite {
            max_input_json_bytes: MAX_INPUT_JSON_BYTES + 1,
            trace_elapsed: Duration::ZERO,
            recorded_trace_visits: 0,
            scenarios: vec![scenario],
        };
        assert!(matches!(
            suite.validate(),
            Err(BenchmarkError::InputJsonTooLarge { .. })
        ));
        assert!(matches!(
            run_suite(
                &BenchmarkSuite {
                    max_input_json_bytes: 0,
                    trace_elapsed: Duration::ZERO,
                    recorded_trace_visits: 0,
                    scenarios: vec![],
                },
                BenchmarkConfig {
                    sample_pairs: 19,
                    ..BenchmarkConfig::default()
                }
            ),
            Err(BenchmarkError::InvalidBenchmarkConfig)
        ));
        assert!(matches!(
            run_suite(
                &BenchmarkSuite {
                    max_input_json_bytes: 0,
                    trace_elapsed: Duration::ZERO,
                    recorded_trace_visits: 0,
                    scenarios: vec![],
                },
                BenchmarkConfig {
                    sample_pairs: usize::MAX,
                    ..BenchmarkConfig::default()
                }
            ),
            Err(BenchmarkError::InvalidBenchmarkConfig)
        ));
        assert!(matches!(
            run_suite(
                &BenchmarkSuite {
                    max_input_json_bytes: 0,
                    trace_elapsed: Duration::ZERO,
                    recorded_trace_visits: 0,
                    scenarios: vec![],
                },
                BenchmarkConfig {
                    preparation_elapsed: MAX_RUN_TIME + Duration::from_nanos(1),
                    ..BenchmarkConfig::default()
                }
            ),
            Err(BenchmarkError::InvalidBenchmarkConfig)
        ));
    }

    #[test]
    fn recorded_trace_builds_all_required_scenario_families() {
        let bytes = serde_json::to_vec(&one_visit_trace()).unwrap();
        let replayed: RecordedTrace = serde_json::from_slice(&bytes).unwrap();
        let suite = suite_from_recorded_trace(&replayed).unwrap();
        assert_eq!(suite.recorded_trace_visits, 1);
        assert_eq!(suite.scenarios.len(), 18);
        assert_eq!(suite.scenarios[0].name, "recorded-corpus-trace");
        assert!(!suite.scenarios[0].synthetic);
        for required in [
            "synthetic-size-1",
            "synthetic-size-8",
            "synthetic-size-64",
            "synthetic-size-512",
            "synthetic-size-4096",
            "observed-unique-count-1",
            "synthetic-hot-loop-1",
            "synthetic-hot-loop-8",
            "synthetic-no-locality-unique",
            "synthetic-mixed-hit-miss",
            "synthetic-stale-mapping",
            "synthetic-stale-backing",
            "synthetic-stale-permission",
            "synthetic-alias-shared-backing",
            "synthetic-dependency-spans-1",
            "synthetic-dependency-spans-2",
            "synthetic-dependency-spans-4",
        ] {
            assert!(
                suite
                    .scenarios
                    .iter()
                    .any(|scenario| scenario.name == required)
            );
        }
        suite.validate().unwrap();
    }

    #[test]
    fn observed_unique_keys_keep_their_fixture_input_associations() {
        let mut trace = one_visit_trace();
        let second_code = [0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0];
        trace.inputs.push(RecordedInput {
            case_id: "two".into(),
            fixture_sha256: "11".repeat(32),
            fixture_json_bytes: 128,
            code_hex: second_code
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            program_sha256: od_testkit::sha256_hex(&second_code),
            base_rip: 0x2000,
            address_space_id: 1,
            mapping_id: 2,
            mapping_version: 1,
            backing_id: 2,
            backing_version: 1,
        });
        trace.visits.push(RecordedVisit {
            input_index: 1,
            entry_rip: 0x2000,
        });

        let suite = suite_from_recorded_trace(&trace).unwrap();
        let observed = suite
            .scenarios
            .iter()
            .find(|scenario| scenario.name == "observed-unique-count-2")
            .unwrap();
        assert_eq!(observed.input_count, 2);
        assert_eq!(observed.visits_per_input, [1, 1]);
        assert_eq!(
            observed
                .queries
                .iter()
                .map(|query| query.input_index)
                .collect::<Vec<_>>(),
            [0, 1]
        );
        observed.validate().unwrap();
    }

    #[test]
    fn observed_keys_group_multiple_blocks_under_one_fixture_input() {
        let mut trace = one_visit_trace();
        let code = [
            0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0, 0x48, 0xb8, 2, 0, 0, 0, 0, 0, 0, 0,
        ];
        trace.inputs[0].code_hex = code.iter().map(|byte| format!("{byte:02x}")).collect();
        trace.inputs[0].program_sha256 = od_testkit::sha256_hex(&code);
        trace.visits.push(RecordedVisit {
            input_index: 0,
            entry_rip: 0x100a,
        });

        let suite = suite_from_recorded_trace(&trace).unwrap();
        let observed = suite
            .scenarios
            .iter()
            .find(|scenario| scenario.name == "observed-unique-count-2")
            .unwrap();
        assert_eq!(observed.input_count, 1);
        assert_eq!(observed.visits_per_input, [2]);
        assert_eq!(
            observed
                .queries
                .iter()
                .map(|query| query.input_index)
                .collect::<Vec<_>>(),
            [0, 0]
        );
        observed.validate().unwrap();
    }

    #[test]
    fn stale_query_finds_old_cached_key_before_dependency_validation() {
        let cached = key(1, 1, 1);
        let changed = key(1, 2, 1);
        let query = query(0, &cached, &changed);
        let entries = vec![cached.clone()];
        let map = HashMap::from([(cached, ())]);
        assert_eq!(lookup_vec(&entries, &query), LookupResult::StaleHit);
        assert_eq!(lookup_hash_map(&map, &query), LookupResult::StaleHit);
    }

    #[test]
    fn each_kernel_calibrates_independent_repetition_count() {
        let started = Instant::now();
        let config = BenchmarkConfig::default();
        let minimum = config.minimum_batch;
        let cold_vec = calibrate_repetitions(
            started,
            config,
            |repetitions| {
                if repetitions >= 2 {
                    minimum
                } else {
                    Duration::ZERO
                }
            },
            u64::MAX,
        )
        .unwrap();
        let cold_map = calibrate_repetitions(
            started,
            config,
            |repetitions| {
                if repetitions >= 8 {
                    minimum
                } else {
                    Duration::ZERO
                }
            },
            u64::MAX,
        )
        .unwrap();
        assert_eq!(cold_vec, 2);
        assert_eq!(cold_map, 8);
        let micro_batched = calibrate_repetitions(
            started,
            config,
            |repetitions| {
                if repetitions >= 4 {
                    minimum
                } else {
                    Duration::ZERO
                }
            },
            1,
        )
        .unwrap();
        assert_eq!(micro_batched, 4);
        assert!(matches!(
            calibrate_repetitions(started, config, |_| Duration::ZERO, 0),
            Err(BenchmarkError::ColdBatchMemoryLimit {
                requested: 1,
                max: 0
            })
        ));
    }

    #[test]
    fn even_sample_median_preserves_equal_odd_values() {
        let series = summarize(vec![
            RawSample {
                elapsed_ns: 3,
                operations: 1,
                batches: 1,
            },
            RawSample {
                elapsed_ns: 3,
                operations: 1,
                batches: 1,
            },
        ])
        .unwrap();
        assert_eq!(series.distribution.median_ns_per_operation, 3);
        assert_eq!(series.distribution.min_ns_per_operation, 3);
        assert_eq!(series.distribution.max_ns_per_operation, 3);
    }

    #[test]
    fn cold_micro_batches_fit_logical_budget_and_count_operations() {
        let keys = vec![key(1, 1, 1), key(2, 2, 2)];
        let key_buffers = keys.iter().map(key_owned_buffer_bytes).sum::<usize>();
        let vec_per_table = std::mem::size_of::<Vec<TranslationKey>>()
            + keys.len() * std::mem::size_of::<TranslationKey>()
            + key_buffers;
        let outer_batch_header = std::mem::size_of::<Vec<HashMap<TranslationKey, ()>>>();
        let vec_budget = outer_batch_header + vec_per_table * 2;
        assert_eq!(cold_batch_repetition_limit(&keys, false, vec_budget, 0), 2);
        let vec_batch = time_cold_build_vec(&keys, 5, vec_budget);
        assert_eq!(vec_batch.operations, 10);
        assert_eq!(vec_batch.batches, 3);

        let map: HashMap<_, _> = keys.iter().cloned().map(|key| (key, ())).collect();
        let map_per_table = std::mem::size_of::<HashMap<TranslationKey, ()>>()
            + map.capacity() * std::mem::size_of::<(TranslationKey, ())>()
            + key_buffers;
        let map_budget = outer_batch_header + map_per_table * 2;
        assert_eq!(
            cold_batch_repetition_limit(&keys, true, map_budget, map.capacity()),
            2
        );
        let map_batch = time_cold_build_map(&keys, 5, map_budget, map.capacity());
        assert_eq!(map_batch.operations, 10);
        assert_eq!(map_batch.batches, 3);

        // This matches the original 256 -> 512 calibration failure shape:
        // the batch cap is 416, but the 512-work batch is measured as two
        // bounded sub-batches instead of being rejected.
        let vec_budget = outer_batch_header + vec_per_table * 416;
        let cap = cold_batch_repetition_limit(&keys, false, vec_budget, 0);
        assert_eq!(cap, 416);
        let over_cap = time_cold_build_vec(&keys, 512, vec_budget);
        assert_eq!(over_cap.operations, 1_024);
        assert_eq!(over_cap.batches, 2);
    }

    #[test]
    fn hash_map_capacity_bound_covers_standard_capacity_for_supported_sizes() {
        for entries in 0..=MAX_KEYS {
            let map = HashMap::<u64, ()>::with_capacity(entries);
            assert!(
                map.capacity() <= logical_hash_map_capacity_bound(entries),
                "entry count {entries}: actual capacity {}, bound {}",
                map.capacity(),
                logical_hash_map_capacity_bound(entries)
            );
        }
    }

    #[test]
    fn host_metadata_separates_build_provenance_from_runtime_context() {
        let metadata = HostMetadata::current();
        assert!(!metadata.build.rustc.is_empty());
        assert!(!metadata.build.source_revision.is_empty());
        assert!(!metadata.build.source_sha256.is_empty());
        assert!(!metadata.execution.working_directory.is_empty());
        assert!(!metadata.execution.repository_revision.is_empty());
        if metadata.build.source_sha256 != "unavailable" {
            assert_eq!(metadata.build.source_sha256.len(), 64);
            assert!(
                metadata
                    .build
                    .source_sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
            );
        }
    }

    #[test]
    fn malformed_trace_limits_and_unbounded_scenario_count_are_rejected() {
        let mut trace = one_visit_trace();
        trace.visits.push(RecordedVisit {
            input_index: 1,
            entry_rip: 0x1000,
        });
        assert!(matches!(
            suite_from_recorded_trace(&trace),
            Err(BenchmarkError::MalformedTrace(_))
        ));

        let mut wrong_digest = one_visit_trace();
        wrong_digest.inputs[0].program_sha256 = "ff".repeat(32);
        assert!(matches!(
            suite_from_recorded_trace(&wrong_digest),
            Err(BenchmarkError::MalformedTrace(_))
        ));

        let mut mid_instruction = one_visit_trace();
        let overlapping_code = [0x48, 0xb8, 0x48, 0xb8, 0, 0, 0, 0, 0, 0, 0x74, 0];
        mid_instruction.inputs[0].code_hex = overlapping_code
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        mid_instruction.inputs[0].program_sha256 = od_testkit::sha256_hex(&overlapping_code);
        mid_instruction.inputs[0].base_rip = 0x1000;
        mid_instruction.visits[0].entry_rip = 0x1002;
        assert!(matches!(
            suite_from_recorded_trace(&mid_instruction),
            Err(BenchmarkError::MalformedTrace(_))
        ));

        let mut malformed_tail = one_visit_trace();
        let mut tail_code = [0x48, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0].to_vec();
        tail_code.push(0x0f);
        malformed_tail.inputs[0].code_hex =
            tail_code.iter().map(|byte| format!("{byte:02x}")).collect();
        malformed_tail.inputs[0].program_sha256 = od_testkit::sha256_hex(&tail_code);
        assert!(matches!(
            suite_from_recorded_trace(&malformed_tail),
            Err(BenchmarkError::MalformedTrace(_))
        ));

        let mut suite = suite_from_recorded_trace(&one_visit_trace()).unwrap();
        let duplicate = suite.scenarios[0].clone();
        suite.scenarios.resize(MAX_SCENARIOS + 1, duplicate);
        assert!(matches!(
            suite.validate(),
            Err(BenchmarkError::TooManyScenarios { .. })
        ));
    }
}
