//! Opt-in, bounded trace and replay for the integer fixture profile.

use std::fs;
use std::io::Read;
use std::process::ExitCode;

use od_isa::execute::{RunConfig, RunExit, run_with_rflags_defined};
use od_isa::memory::AccessContext;
use od_isa::{INTEGER_FLAGS, LongModeState};
use od_testkit::memory::FlatMemory;
use od_testkit::v2::{self, Profile};
use serde_json::{Value, json};

const MAX_RECORDS: usize = 64;
const MAX_EVENT_BYTES: usize = 2048;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_TRACE_MEMORY_BYTES: usize = 64 * 1024;
const DEFAULT_EVENT_BYTES: usize = 1024;

pub fn run(args: &[String]) -> ExitCode {
    match make_trace(args) {
        Ok(result) => write_result(result),
        Err(error) => fail(&error),
    }
}

pub fn replay(args: &[String]) -> ExitCode {
    if args.len() != 1 {
        return fail("usage: od-runner --diagnostic-replay TRACE.json");
    }
    let bytes = match read_bounded(&args[0], MAX_OUTPUT_BYTES) {
        Ok(bytes) => bytes,
        Err(error) => return fail(&error),
    };
    let replay: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(error) => return fail(&format!("invalid replay JSON: {error}")),
    };
    let Some(request) = replay.get("request") else {
        return fail("invalid replay: missing request");
    };
    let Some(fixture_json) = request.get("fixture_json").and_then(Value::as_str) else {
        return fail("invalid replay: missing fixture_json");
    };
    let Some(max_steps) = request.get("max_steps").and_then(Value::as_u64) else {
        return fail("invalid replay: missing max_steps");
    };
    let Some(max_event_bytes) = request.get("max_event_bytes").and_then(Value::as_u64) else {
        return fail("invalid replay: missing max_event_bytes");
    };
    let max_steps = match usize::try_from(max_steps) {
        Ok(value) if value <= MAX_RECORDS => value,
        _ => return fail("invalid replay: max_steps is outside the supported range"),
    };
    let max_event_bytes = match usize::try_from(max_event_bytes) {
        Ok(value) if value <= MAX_EVENT_BYTES => value,
        _ => return fail("invalid replay: max_event_bytes is outside the supported range"),
    };
    if fixture_json.len() > v2::MAX_FIXTURE_JSON_BYTES {
        return fail("invalid replay: fixture JSON exceeds the size limit");
    }
    if replay.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return fail("invalid replay schema version");
    }
    let recomputed = match trace_bytes(fixture_json.as_bytes(), max_steps, max_event_bytes) {
        Ok(value) => value,
        Err(error) => return fail(&error),
    };
    if replay.get("result") != Some(&recomputed) {
        return fail("replay mismatch: serialized trace does not match recomputed execution");
    }
    write_result(json!({"schema_version": 1, "replayed": true, "result": recomputed}))
}

fn make_trace(args: &[String]) -> Result<Value, String> {
    let mut path = None;
    let mut max_steps = 1usize;
    let mut max_event_bytes = DEFAULT_EVENT_BYTES;
    let mut engine = "interpreter";
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--steps" => {
                index += 1;
                max_steps = parse_limit(args.get(index), "--steps", MAX_RECORDS)?;
            }
            "--max-event-bytes" => {
                index += 1;
                max_event_bytes =
                    parse_limit(args.get(index), "--max-event-bytes", MAX_EVENT_BYTES)?;
            }
            "--engine" => {
                index += 1;
                engine = args.get(index).ok_or("missing value for --engine")?;
            }
            value if path.is_none() => path = Some(value.to_owned()),
            value => return Err(format!("unexpected diagnostic argument '{value}'")),
        }
        index += 1;
    }
    if engine != "interpreter" {
        return Err("diagnostic execution supports only --engine interpreter; JIT/native diagnostics are unsupported".into());
    }
    let path = path.ok_or("usage: od-runner --diagnostic CASE.json [--steps N] [--max-event-bytes N] [--engine interpreter]")?;
    let bytes = read_bounded(&path, v2::MAX_FIXTURE_JSON_BYTES)?;
    let result = trace_bytes(&bytes, max_steps, max_event_bytes)?;
    let fixture_json = std::str::from_utf8(&bytes).map_err(|_| "fixture JSON must be UTF-8")?;
    Ok(json!({
        "schema_version": 1,
        "request": {
            "fixture_json": fixture_json,
            "max_steps": max_steps,
            "max_event_bytes": max_event_bytes
        },
        "result": result
    }))
}

fn trace_bytes(bytes: &[u8], max_steps: usize, max_event_bytes: usize) -> Result<Value, String> {
    if max_steps > MAX_RECORDS || max_event_bytes > MAX_EVENT_BYTES {
        return Err("diagnostic limits exceed the supported maximum".into());
    }
    let loaded =
        v2::Fixture::from_json(bytes).map_err(|error| format!("invalid v2 fixture: {error}"))?;
    if loaded.fixture.profile != Profile::LongIntegerV1 {
        return Err("diagnostic supports only profile s2-long-integer-v1".into());
    }
    let memory_size = loaded
        .fixture
        .memory
        .iter()
        .try_fold(0usize, |total, region| {
            total.checked_add(region.length as usize)
        })
        .ok_or("fixture memory size overflow")?;
    if memory_size > MAX_TRACE_MEMORY_BYTES {
        return Err(format!(
            "diagnostic fixture memory exceeds the {MAX_TRACE_MEMORY_BYTES}-byte limit"
        ));
    }
    let mut state = loaded
        .fixture
        .initial_state
        .to_cpu_for(Profile::LongIntegerV1)
        .map_err(|error| format!("invalid integer state: {error}"))?;
    let mut memory = FlatMemory::from_fixture(&loaded.fixture)
        .map_err(|error| format!("invalid fixture memory: {error}"))?;
    let mut rflags_defined = INTEGER_FLAGS;
    let mut used_event_bytes = 0usize;
    let mut records = Vec::with_capacity(max_steps);
    for _ in 0..max_steps {
        let before_state = integer_state(&state)?;
        let before_memory = memory.snapshot();
        let before_rip = state.rip;
        let rflags_defined_before = rflags_defined;
        let execution = run_with_rflags_defined(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: loaded.fixture.stop_rip.map(|value| value.get()),
                context: AccessContext::default(),
            },
            rflags_defined,
            || false,
        )
        .map_err(|error| format!("diagnostic run rejected input: {error:?}"))?;
        rflags_defined = execution.rflags_defined;
        let (memory_effects, bytes_used, memory_effects_truncated) = memory_effects(
            &before_memory,
            &memory.snapshot(),
            max_event_bytes.saturating_sub(used_event_bytes),
        )?;
        used_event_bytes += bytes_used;
        let faulting_rip = matches!(&execution.exit, RunExit::Fault(_) | RunExit::MemoryError(_))
            .then_some(format!("0x{before_rip:016x}"));
        let outcome = format!("{:?}", execution.exit);
        let retired = execution.instructions_executed;
        records.push(json!({
            "before": before_state,
            "after": integer_state(&state)?,
            "rflags_defined_before": format!("0x{rflags_defined_before:016x}"),
            "rflags_defined_after": format!("0x{rflags_defined:016x}"),
            "retired": retired,
            "faulting_rip": faulting_rip,
            "outcome": outcome,
            "memory_effects": memory_effects,
            "memory_effects_truncated": memory_effects_truncated
        }));
        if retired == 0 || !matches!(&execution.exit, RunExit::InstructionBudgetExhausted) {
            break;
        }
    }
    Ok(json!({
        "case_id": loaded.fixture.case_id,
        "profile": "s2-long-integer-v1",
        "input_sha256": loaded.input_sha256,
        "records": records,
        "event_bytes": used_event_bytes,
        "limits": {"max_records": max_steps, "max_event_bytes": max_event_bytes}
    }))
}

fn integer_state(state: &LongModeState) -> Result<Value, String> {
    let snapshot = v2::IntegerState::from_cpu_for(state, Profile::LongIntegerV1)
        .map_err(|error| format!("could not materialize integer state: {error}"))?;
    serde_json::to_value(snapshot).map_err(|error| format!("could not serialize state: {error}"))
}

fn memory_effects(
    before: &[od_testkit::result::MemoryRegionSnapshot],
    after: &[od_testkit::result::MemoryRegionSnapshot],
    remaining: usize,
) -> Result<(Vec<Value>, usize, bool), String> {
    let mut effects = Vec::new();
    let mut used = 0usize;
    let mut truncated = false;
    for (old_region, new_region) in before.iter().zip(after) {
        let old = v2::decode_bytes(&old_region.bytes_hex).map_err(|e| e.to_string())?;
        let new = v2::decode_bytes(&new_region.bytes_hex).map_err(|e| e.to_string())?;
        for (offset, (old_byte, new_byte)) in old.iter().zip(&new).enumerate() {
            if old_byte == new_byte {
                continue;
            }
            if remaining.saturating_sub(used) < 2 {
                truncated = true;
                continue;
            }
            effects.push(json!({
                "region_id": old_region.region_id,
                "address": format!("0x{:016x}", old_region.guest_address.get() + offset as u64),
                "before": format!("{old_byte:02x}"),
                "after": format!("{new_byte:02x}")
            }));
            used += 2;
        }
    }
    Ok((effects, used, truncated))
}

fn parse_limit(value: Option<&String>, option: &str, maximum: usize) -> Result<usize, String> {
    let value = value.ok_or_else(|| format!("missing value for {option}"))?;
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("invalid value for {option}"))?;
    if parsed > maximum {
        return Err(format!("{option} must be at most {maximum}"));
    }
    Ok(parsed)
}

fn read_bounded(path: &str, limit: usize) -> Result<Vec<u8>, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("could not read '{path}': {error}"))?;
    if metadata.len() > limit as u64 {
        return Err(format!("'{path}' exceeds the {limit}-byte input limit"));
    }
    let file = fs::File::open(path).map_err(|error| format!("could not read '{path}': {error}"))?;
    let read_limit = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    let mut bytes = Vec::with_capacity(limit.min(8192));
    file.take(read_limit)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read '{path}': {error}"))?;
    if bytes.len() > limit {
        return Err(format!("'{path}' exceeds the {limit}-byte input limit"));
    }
    Ok(bytes)
}

fn write_result(result: Value) -> ExitCode {
    if estimated_json_size(&result) > MAX_OUTPUT_BYTES {
        return fail("serialized diagnostic output exceeds the size limit");
    }
    match serde_json::to_vec(&result) {
        Ok(bytes) if bytes.len() <= MAX_OUTPUT_BYTES => {
            println!("{}", String::from_utf8_lossy(&bytes));
            ExitCode::SUCCESS
        }
        Ok(_) => fail("serialized diagnostic output exceeds the size limit"),
        Err(error) => fail(&format!("could not serialize diagnostic output: {error}")),
    }
}

fn estimated_json_size(value: &Value) -> usize {
    match value {
        Value::Null => 4,
        Value::Bool(true) => 4,
        Value::Bool(false) => 5,
        Value::Number(number) => number.to_string().len(),
        Value::String(string) => string.len().saturating_mul(6).saturating_add(2),
        Value::Array(values) => values.iter().fold(2usize, |total, item| {
            total
                .saturating_add(estimated_json_size(item))
                .saturating_add(1)
        }),
        Value::Object(values) => values.iter().fold(2usize, |total, (key, item)| {
            total
                .saturating_add(key.len().saturating_mul(6).saturating_add(2))
                .saturating_add(estimated_json_size(item))
                .saturating_add(2)
        }),
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("error: {message}");
    ExitCode::FAILURE
}
