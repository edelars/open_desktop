#!/usr/bin/env python3
"""Audit the three raw reports from the bounded S3-31c profile.

This is deliberately profile-specific. It validates the saved samples and
recomputes their ratios; it does not run benchmarks or infer production policy.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import statistics
import sys
from pathlib import Path
from typing import Any, NoReturn


PROFILE_ID = "s3-31c-native-register-v1-run-local-prepared-key-reuse-v1"
PREFLIGHT_PARTIALS = {
    "later_iteration_after_lea",
    "later_iteration_after_xchg",
    "later_iteration_before_jnz",
    "stop_address_before_xchg",
    "stop_address_at_jnz",
    "wrapping_operand_later_iteration",
}
STRATA = ("a_1", "a_64", "a_4096", "b_1", "b_64", "b_4096")
BASE_RIP = 0x1000
U64_MASK = (1 << 64) - 1
INITIAL_CALIBRATION_CALLS = 64
MAX_CALLS = 1 << 27
WARMUP_CALLS = 32
CODE_A = "488d040b4887d875f7"
CODE_B = "488d040b4887d8488d040b4887d875f0"
ITERATIONS_BY_CASE = {"a_1": 1, "a_64": 64, "a_4096": 4096, "b_1": 1, "b_64": 64, "b_4096": 4096}
SEED_BY_ITERATIONS = {1: 0x36_0001, 64: 0x36_0040, 4096: 0x36_1000}
EXPECTED_NATIVE_BLOCKS = {
    "later_iteration_after_lea": 2,
    "later_iteration_after_xchg": 2,
    "later_iteration_before_jnz": 2,
    "stop_address_before_xchg": 1,
    "stop_address_at_jnz": 1,
    "wrapping_operand_later_iteration": 3,
}
EXPECTED_FINGERPRINT_NAMES = {
    "profile",
    "cli",
    "lib",
    "dispatch",
    "block",
    "host",
    "frozen_fixture",
}
ORDERS = ("BCI", "BIC", "CBI", "CIB", "IBC", "ICB")
PROCESS_RUNS = (1, 2, 3)
PAIRS_PER_STRATUM = 20
SAMPLE_FLOOR_NS = 200_000_000
FINGERPRINT_RE = re.compile(r"^[0-9a-f]{64}$")
CODE_HEX_RE = re.compile(r"^(?:[0-9a-f]{2})+$")


class ValidationError(ValueError):
    """A report failed a profile-specific validation rule."""


def _fail(path: str, message: str) -> NoReturn:
    raise ValidationError(f"{path}: {message}")


def _object(value: Any, path: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        _fail(path, "must be a JSON object")
    return value


def _array(value: Any, path: str) -> list[Any]:
    if not isinstance(value, list):
        _fail(path, "must be a JSON array")
    return value


def _string(value: Any, path: str) -> str:
    if not isinstance(value, str) or not value:
        _fail(path, "must be a non-empty string")
    return value


def _integer(value: Any, path: str, *, minimum: int | None = None) -> int:
    if type(value) is not int:
        _fail(path, "must be an integer")
    if minimum is not None and value < minimum:
        _fail(path, f"must be >= {minimum}")
    return value


def _same(left: Any, right: Any, path: str) -> None:
    if left != right:
        _fail(path, f"inconsistent values: {left!r} != {right!r}")


def _object_without_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValidationError(f"duplicate JSON key {key!r}")
        result[key] = value
    return result


def _reject_nonfinite(token: str) -> NoReturn:
    raise ValidationError(f"non-finite JSON number {token!r} is not allowed")


def load_report(path: Path) -> dict[str, Any]:
    """Load a report without accepting duplicate keys or non-standard numbers."""
    try:
        with path.open("r", encoding="utf-8") as source:
            value = json.load(
                source,
                object_pairs_hook=_object_without_duplicate_keys,
                parse_constant=_reject_nonfinite,
            )
    except ValidationError as error:
        _fail(str(path), str(error))
    except (OSError, json.JSONDecodeError, UnicodeDecodeError) as error:
        _fail(str(path), f"cannot read JSON: {error}")
    return _object(value, str(path))


def _fingerprints(value: Any, path: str) -> dict[str, str]:
    result: dict[str, str] = {}
    for index, item in enumerate(_array(value, path)):
        pair = _array(item, f"{path}[{index}]")
        if len(pair) != 2:
            _fail(f"{path}[{index}]", "must contain [name, sha256]")
        name = _string(pair[0], f"{path}[{index}][0]")
        digest = _string(pair[1], f"{path}[{index}][1]")
        if not FINGERPRINT_RE.fullmatch(digest):
            _fail(f"{path}[{index}][1]", "must be a lowercase SHA-256 hex digest")
        if name in result:
            _fail(path, f"duplicate fingerprint name {name!r}")
        result[name] = digest
    if not result:
        _fail(path, "must contain at least one source fingerprint")
    return result


def _expected_cache_delta(value: Any, path: str, expected_hits: int) -> None:
    cache = _object(value, path)
    for key in ("hits", "misses", "compiles", "publications", "replacements"):
        _integer(cache.get(key), f"{path}.{key}", minimum=0)
    _same(cache["hits"], expected_hits, f"{path}.hits")
    for key in ("misses", "compiles", "publications", "replacements"):
        _same(cache[key], 0, f"{path}.{key}")


def _initial_state(seed: int, *, wrapping: bool = False) -> dict[str, Any]:
    gpr = [0] * 16
    gpr[0] = seed ^ 0xA55A_1234_9876_0001
    gpr[3] = (((seed << 17) | (seed >> (64 - 17))) & U64_MASK) ^ 0x55AA_FEDC_0123_4567
    gpr[1] = (seed * 0x9E37_79B9_7F4A_7C15) & U64_MASK
    if wrapping:
        gpr[0] = U64_MASK - 1
        gpr[3] = U64_MASK
        gpr[1] = 1
    return {"gpr": gpr, "rip": BASE_RIP, "rflags": 0x202}


def _validate_state(value: Any, path: str) -> dict[str, Any]:
    state = _object(value, path)
    if set(state) != {"gpr", "rip", "rflags"}:
        _fail(path, "must contain exactly gpr, rip, and rflags")
    gpr = _array(state.get("gpr"), f"{path}.gpr")
    if len(gpr) != 16:
        _fail(f"{path}.gpr", f"expected 16 general registers; got {len(gpr)}")
    for index, register in enumerate(gpr):
        _integer(register, f"{path}.gpr[{index}]", minimum=0)
        if register > U64_MASK:
            _fail(f"{path}.gpr[{index}]", "must fit in u64")
    for key in ("rip", "rflags"):
        value = _integer(state.get(key), f"{path}.{key}", minimum=0)
        if value > U64_MASK:
            _fail(f"{path}.{key}", "must fit in u64")
    return state


def _validate_observation(value: Any, path: str) -> dict[str, Any]:
    observation = _object(value, path)
    _validate_state(observation.get("state"), f"{path}.state")
    _string(observation.get("exit"), f"{path}.exit")
    for key in ("retired", "remaining_budget", "blocks_executed"):
        _integer(observation.get(key), f"{path}.{key}", minimum=0)
    return observation


def _validate_observation_group(
    baseline_value: Any,
    candidate_value: Any,
    interpreter_value: Any,
    *,
    path: str,
    budget: int,
    retired: int,
    native_exit: str,
    interpreter_exit: str,
    native_blocks: int,
    expected_rip: int,
) -> None:
    baseline = _validate_observation(baseline_value, f"{path}.baseline")
    candidate = _validate_observation(candidate_value, f"{path}.candidate")
    interpreter = _validate_observation(interpreter_value, f"{path}.interpreter")
    _same(candidate, baseline, f"{path}.candidate")
    _same(candidate["state"], interpreter["state"], f"{path}.interpreter.state")
    _same(candidate["retired"], interpreter["retired"], f"{path}.interpreter.retired")
    _same(candidate["remaining_budget"], interpreter["remaining_budget"], f"{path}.interpreter.remaining_budget")
    for mode, observation in (("baseline", baseline), ("candidate", candidate)):
        _same(observation["retired"], retired, f"{path}.{mode}.retired")
        _same(observation["remaining_budget"], budget - retired, f"{path}.{mode}.remaining_budget")
        _same(observation["retired"] + observation["remaining_budget"], budget, f"{path}.{mode}.budget_accounting")
        _same(observation["exit"], native_exit, f"{path}.{mode}.exit")
        _same(observation["blocks_executed"], native_blocks, f"{path}.{mode}.blocks_executed")
        _same(observation["state"]["rip"], expected_rip, f"{path}.{mode}.state.rip")
    _same(interpreter["retired"], retired, f"{path}.interpreter.retired")
    _same(interpreter["remaining_budget"], budget - retired, f"{path}.interpreter.remaining_budget")
    _same(interpreter["retired"] + interpreter["remaining_budget"], budget, f"{path}.interpreter.budget_accounting")
    _same(interpreter["exit"], interpreter_exit, f"{path}.interpreter.exit")
    _same(interpreter["blocks_executed"], 0, f"{path}.interpreter.blocks_executed")
    _same(interpreter["state"]["rip"], expected_rip, f"{path}.interpreter.state.rip")


def _ratio(pair: dict[str, Any], numerator: str, denominator: str, path: str) -> float:
    num = pair[numerator]
    den = pair[denominator]
    # Counts are validated equal before this calculation. Normalizing by guest
    # work makes the audited formula explicit while preserving the writer's ratio.
    return (num["elapsed_ns"] / num["guest_instructions"]) / (
        den["elapsed_ns"] / den["guest_instructions"]
    )


def _validate_sample(
    raw: Any,
    *,
    path: str,
    pair_id: int,
    order: str,
    seed: int,
    iterations: int,
    budget_per_call: int,
    side: str,
    require_floor: bool,
) -> dict[str, Any]:
    sample = _object(raw, path)
    _same(_integer(sample.get("pair"), f"{path}.pair", minimum=0), pair_id, f"{path}.pair")
    _same(_string(sample.get("order"), f"{path}.order"), order, f"{path}.order")
    _same(_integer(sample.get("seed"), f"{path}.seed", minimum=0), seed, f"{path}.seed")
    calls = _integer(sample.get("calls"), f"{path}.calls", minimum=1)
    guest_instructions = _integer(
        sample.get("guest_instructions"), f"{path}.guest_instructions", minimum=1
    )
    expected_work = calls * budget_per_call
    _same(guest_instructions, expected_work, f"{path}.guest_instructions")
    retired = _integer(sample.get("retired"), f"{path}.retired", minimum=1)
    _same(retired, guest_instructions, f"{path}.retired")
    elapsed_ns = _integer(sample.get("elapsed_ns"), f"{path}.elapsed_ns", minimum=1)
    if require_floor and elapsed_ns < SAMPLE_FLOOR_NS:
        _fail(path, f"elapsed_ns {elapsed_ns} is below the {SAMPLE_FLOOR_NS} ns floor")

    cache_delta = sample.get("cache_delta")
    if side == "interpreter":
        if cache_delta is not None:
            _fail(f"{path}.cache_delta", "interpreter sample must not contain cache counters")
    else:
        _expected_cache_delta(
            cache_delta,
            f"{path}.cache_delta",
            calls * iterations,
        )
    return sample


def _validate_pair(
    raw: Any,
    *,
    path: str,
    seed: int,
    iterations: int,
    budget_per_call: int,
    require_floor: bool,
) -> tuple[int, dict[str, Any]]:
    pair = _object(raw, path)
    pair_id = _integer(pair.get("pair"), f"{path}.pair", minimum=0)
    order = _string(pair.get("order"), f"{path}.order")
    expected_order = ORDERS[pair_id % len(ORDERS)]
    _same(order, expected_order, f"{path}.order")
    _same(_integer(pair.get("seed"), f"{path}.seed", minimum=0), seed, f"{path}.seed")

    for side in ("baseline", "candidate", "interpreter"):
        _validate_sample(
            pair.get(side),
            path=f"{path}.{side}",
            pair_id=pair_id,
            order=order,
            seed=seed,
            iterations=iterations,
            budget_per_call=budget_per_call,
            side=side,
            require_floor=require_floor,
        )

    samples = [pair[name] for name in ("baseline", "candidate", "interpreter")]
    calls = samples[0]["calls"]
    guest_instructions = samples[0]["guest_instructions"]
    retired = samples[0]["retired"]
    for side, sample in zip(("candidate", "interpreter"), samples[1:]):
        _same(sample["calls"], calls, f"{path}.{side}.calls")
        _same(sample["guest_instructions"], guest_instructions, f"{path}.{side}.guest_instructions")
        _same(sample["retired"], retired, f"{path}.{side}.retired")

    for field, numerator, denominator in (
        ("candidate_over_baseline_ratio", "candidate", "baseline"),
        ("candidate_over_interpreter_ratio", "candidate", "interpreter"),
    ):
        stored = pair.get(field)
        if isinstance(stored, bool) or not isinstance(stored, (int, float)):
            _fail(f"{path}.{field}", "must be a finite number")
        if not math.isfinite(stored) or stored <= 0:
            _fail(f"{path}.{field}", "must be finite and positive")
        recomputed = _ratio(pair, numerator, denominator, path)
        if not math.isclose(float(stored), recomputed, rel_tol=1e-12, abs_tol=1e-15):
            _fail(
                f"{path}.{field}",
                f"stored ratio {stored!r} does not match raw samples {recomputed!r}",
            )
    return pair_id, pair


def _validate_host(report: dict[str, Any], path: str) -> dict[str, Any]:
    host = _object(report.get("measure_host"), f"{path}.measure_host")
    for key in ("architecture", "cpu_model", "os", "os_version"):
        _string(host.get(key), f"{path}.measure_host.{key}")
    build = _object(host.get("build"), f"{path}.measure_host.build")
    for key in ("profile", "rustc", "source_revision", "source_sha256", "target"):
        _string(build.get(key), f"{path}.measure_host.build.{key}")
    if not FINGERPRINT_RE.fullmatch(build["source_sha256"]):
        _fail(f"{path}.measure_host.build.source_sha256", "must be a lowercase SHA-256 hex digest")
    if build.get("source_tree_dirty") is not False:
        _fail(f"{path}.measure_host.build.source_tree_dirty", "must be false for scored evidence")
    execution = _object(host.get("execution"), f"{path}.measure_host.execution")
    for key in ("repository_revision", "working_directory"):
        _string(execution.get(key), f"{path}.measure_host.execution.{key}")
    _same(
        execution["repository_revision"],
        build["source_revision"],
        f"{path}.measure_host.execution.repository_revision",
    )
    return host


def _validate_strata(report: dict[str, Any], run_path: str) -> dict[str, dict[str, Any]]:
    strata: dict[str, dict[str, Any]] = {}
    for index, raw in enumerate(_array(report.get("strata"), f"{run_path}.strata")):
        path = f"{run_path}.strata[{index}]"
        stratum = _object(raw, path)
        stratum_id = _string(stratum.get("id"), f"{path}.id")
        if stratum_id not in STRATA:
            _fail(f"{path}.id", f"unexpected stratum {stratum_id!r}")
        if stratum_id in strata:
            _fail(f"{path}.id", f"duplicate stratum {stratum_id!r}")
        strata[stratum_id] = stratum
    if set(strata) != set(STRATA):
        missing = sorted(set(STRATA) - set(strata))
        _fail(f"{run_path}.strata", f"missing strata: {', '.join(missing)}")
    return strata


def _profile_case(stratum_id: str) -> dict[str, Any]:
    iterations = ITERATIONS_BY_CASE[stratum_id]
    instructions_per_iteration = 3 if stratum_id.startswith("a_") else 5
    code_hex = CODE_A if stratum_id.startswith("a_") else CODE_B
    return {
        "iterations": iterations,
        "seed": SEED_BY_ITERATIONS[iterations],
        "instructions_per_iteration": instructions_per_iteration,
        "guest_work_per_call": iterations * instructions_per_iteration,
        "code_hex": code_hex,
        "code_sha256": hashlib.sha256(bytes.fromhex(code_hex)).hexdigest(),
    }


def _partial_control_spec(stratum_id: str, control_id: str) -> dict[str, Any]:
    case = _profile_case(stratum_id)
    instructions_per_iteration = case["instructions_per_iteration"]
    code_hex = case["code_hex"]
    branch_rip = BASE_RIP + len(bytes.fromhex(code_hex)) - 2
    budget = case["guest_work_per_call"]
    stop_rip: int | None = None
    wrapping = False
    expected_rip = BASE_RIP
    retired = budget
    if control_id == "later_iteration_after_lea":
        budget = instructions_per_iteration + 1
        expected_rip = BASE_RIP + 4
    elif control_id == "later_iteration_after_xchg":
        budget = instructions_per_iteration + 2
        expected_rip = BASE_RIP + 7
    elif control_id == "later_iteration_before_jnz":
        budget = instructions_per_iteration * 2 - 1
        expected_rip = branch_rip
    elif control_id == "stop_address_before_xchg":
        stop_rip = BASE_RIP + 4
        expected_rip = stop_rip
        retired = 1
    elif control_id == "stop_address_at_jnz":
        stop_rip = branch_rip
        expected_rip = stop_rip
        retired = instructions_per_iteration - 1
    elif control_id == "wrapping_operand_later_iteration":
        budget = instructions_per_iteration * 2 + 1
        wrapping = True
        expected_rip = BASE_RIP + 4
    else:
        _fail(f"preflight.strata.{stratum_id}.partial_controls", f"unknown control {control_id!r}")
    if stop_rip is None:
        retired = budget
    return {
        "budget": budget,
        "stop_rip": stop_rip,
        "initial_state": _initial_state(case["seed"], wrapping=wrapping),
        "blocks_executed": EXPECTED_NATIVE_BLOCKS[control_id],
        "retired": retired,
        "native_exit": "Runtime(StopAddress)" if stop_rip is not None else "BudgetExhausted",
        "interpreter_exit": "StopAddress" if stop_rip is not None else "InstructionBudgetExhausted",
        "expected_rip": expected_rip,
    }


def _validate_preflight(
    preflight: dict[str, Any],
    reports: list[dict[str, Any]],
    fingerprints: dict[str, str],
) -> dict[str, Any]:
    path = "preflight"
    _same(_integer(preflight.get("schema"), f"{path}.schema"), 1, f"{path}.schema")
    _same(_string(preflight.get("profile_id"), f"{path}.profile_id"), PROFILE_ID, f"{path}.profile_id")
    if preflight.get("timing_started") is not False:
        _fail(f"{path}.timing_started", "preflight must not include timing")
    _same(_fingerprints(preflight.get("source_fingerprints"), f"{path}.source_fingerprints"), fingerprints, f"{path}.source_fingerprints")
    host = _validate_host(preflight, path)
    timed_host = reports[0]["measure_host"]
    for key in ("architecture", "cpu_model", "os", "os_version"):
        _same(host[key], timed_host[key], f"{path}.measure_host.{key}")
    preflight_build = host["build"]
    timed_build = timed_host["build"]
    for key in (
        "profile", "rustc", "source_revision", "source_sha256", "source_tree_dirty", "target"
    ):
        _same(preflight_build[key], timed_build[key], f"{path}.measure_host.build.{key}")
    strata = _validate_strata(preflight, path)
    report_strata = _validate_strata(reports[0], "reports[0]")
    for stratum_id in STRATA:
        item = strata[stratum_id]
        item_path = f"{path}.strata.{stratum_id}"
        report_item = report_strata[stratum_id]
        expected_case = _profile_case(stratum_id)
        iterations = _integer(item.get("iterations"), f"{item_path}.iterations", minimum=1)
        budget = _integer(item.get("budget"), f"{item_path}.budget", minimum=1)
        _same(iterations, expected_case["iterations"], f"{item_path}.iterations")
        _same(budget, expected_case["guest_work_per_call"], f"{item_path}.budget")
        for key in ("code_hex", "code_sha256", "seed"):
            _same(item.get(key), expected_case[key], f"{item_path}.{key}")
            _same(report_item.get(key), expected_case[key], f"reports[0].strata.{stratum_id}.{key}")
        _same(iterations, report_item.get("iterations"), f"reports[0].strata.{stratum_id}.iterations")
        _same(budget, report_item.get("guest_work_per_call"), f"reports[0].strata.{stratum_id}.guest_work_per_call")
        counts = _object(item.get("reuse_counts"), f"{item_path}.reuse_counts")
        _same(_integer(counts.get("prepared_keys"), f"{item_path}.reuse_counts.prepared_keys"), 1, f"{item_path}.reuse_counts.prepared_keys")
        _same(_integer(counts.get("reused_keys"), f"{item_path}.reuse_counts.reused_keys"), iterations - 1, f"{item_path}.reuse_counts.reused_keys")
        for flag in (
            "hit_only",
            "zero_work_checked",
            "entry_stop_checked",
            "entry_event_checked",
            "partial_controls_checked",
            "dependency_replacement_checked",
        ):
            if item.get(flag) is not True:
                _fail(f"{item_path}.{flag}", "must be true")
        _validate_observation_group(
            item.get("baseline"), item.get("candidate"), item.get("interpreter"),
            path=item_path, budget=budget, retired=budget, native_exit="BudgetExhausted",
            interpreter_exit="InstructionBudgetExhausted", native_blocks=iterations,
            expected_rip=BASE_RIP,
        )
        controls = _array(item.get("partial_controls"), f"{item_path}.partial_controls")
        control_map: dict[str, dict[str, Any]] = {}
        for index, raw_control in enumerate(controls):
            cpath = f"{item_path}.partial_controls[{index}]"
            control = _object(raw_control, cpath)
            control_id = _string(control.get("id"), f"{cpath}.id")
            if control_id in control_map:
                _fail(f"{cpath}.id", f"duplicate partial control {control_id!r}")
            control_map[control_id] = control
            spec = _partial_control_spec(stratum_id, control_id)
            _integer(control.get("budget"), f"{cpath}.budget", minimum=1)
            _same(control["budget"], spec["budget"], f"{cpath}.budget")
            if "stop_rip" not in control:
                _fail(f"{cpath}.stop_rip", "field is required")
            stop_rip = control["stop_rip"]
            if stop_rip is not None:
                _integer(stop_rip, f"{cpath}.stop_rip", minimum=0)
            _same(stop_rip, spec["stop_rip"], f"{cpath}.stop_rip")
            initial_state = _validate_state(control.get("initial_state"), f"{cpath}.initial_state")
            _same(initial_state, spec["initial_state"], f"{cpath}.initial_state")
            _validate_observation_group(
                control.get("baseline"), control.get("candidate"), control.get("interpreter"),
                path=cpath, budget=spec["budget"], retired=spec["retired"],
                native_exit=spec["native_exit"], interpreter_exit=spec["interpreter_exit"],
                native_blocks=spec["blocks_executed"], expected_rip=spec["expected_rip"],
            )
            reuse = _object(control.get("candidate_reuse_counts"), f"{cpath}.candidate_reuse_counts")
            _same(_integer(reuse.get("prepared_keys"), f"{cpath}.candidate_reuse_counts.prepared_keys"), 1, f"{cpath}.candidate_reuse_counts.prepared_keys")
            _same(_integer(reuse.get("reused_keys"), f"{cpath}.candidate_reuse_counts.reused_keys"), spec["blocks_executed"] - 1, f"{cpath}.candidate_reuse_counts.reused_keys")
        if set(control_map) != PREFLIGHT_PARTIALS:
            missing = sorted(PREFLIGHT_PARTIALS - set(control_map))
            extra = sorted(set(control_map) - PREFLIGHT_PARTIALS)
            _fail(item_path + ".partial_controls", f"partial control mismatch; missing={missing}, extra={extra}")
    return {
        "timing_started": False,
        "measure_host": host,
        "checked_strata": list(STRATA),
        "partial_controls": sorted(PREFLIGHT_PARTIALS),
    }


def validate_reports(
    reports: list[dict[str, Any]], preflight: dict[str, Any] | None = None
) -> dict[str, Any]:
    """Validate and summarize exactly three scored S3-31c process reports."""
    if len(reports) != 3:
        _fail("reports", f"expected exactly 3 process reports; got {len(reports)}")

    indexed: dict[int, dict[str, Any]] = {}
    hosts: dict[int, dict[str, Any]] = {}
    fingerprints_by_run: dict[int, dict[str, str]] = {}
    strata_by_run: dict[int, dict[str, dict[str, Any]]] = {}
    compatibility_fields: dict[int, dict[str, Any]] = {}

    for position, report_value in enumerate(reports):
        path = f"reports[{position}]"
        report = _object(report_value, path)
        _same(_integer(report.get("schema"), f"{path}.schema"), 1, f"{path}.schema")
        _same(_string(report.get("profile_id"), f"{path}.profile_id"), PROFILE_ID, f"{path}.profile_id")
        _integer(report.get("total_elapsed_ns"), f"{path}.total_elapsed_ns", minimum=1)
        process_run = _integer(report.get("process_run"), f"{path}.process_run", minimum=1)
        if process_run not in PROCESS_RUNS:
            _fail(f"{path}.process_run", f"expected one of {PROCESS_RUNS}; got {process_run}")
        if process_run in indexed:
            _fail(f"{path}.process_run", f"duplicate process run {process_run}")
        indexed[process_run] = report
        hosts[process_run] = _validate_host(report, path)
        fingerprints_by_run[process_run] = _fingerprints(
            report.get("source_fingerprints"), f"{path}.source_fingerprints"
        )
        if set(fingerprints_by_run[process_run]) != EXPECTED_FINGERPRINT_NAMES:
            _fail(
                f"{path}.source_fingerprints",
                "fingerprint names must be exactly " + ", ".join(sorted(EXPECTED_FINGERPRINT_NAMES)),
            )
        strata_by_run[process_run] = _validate_strata(report, path)
        for key in ("timed_scope", "sampling_protocol", "ratio_contract"):
            _string(report.get(key), f"{path}.{key}")
        if "elapsed candidate / elapsed comparator" not in report["ratio_contract"]:
            _fail(f"{path}.ratio_contract", "does not describe the saved ratio convention")
        compatibility_fields[process_run] = {
            key: report[key]
            for key in ("timed_scope", "sampling_protocol", "ratio_contract")
        }

    if set(indexed) != set(PROCESS_RUNS):
        _fail("reports", f"expected process runs {PROCESS_RUNS}; got {tuple(sorted(indexed))}")
    ordered = [indexed[run] for run in PROCESS_RUNS]
    first_fingerprints = fingerprints_by_run[1]
    first_host = hosts[1]
    first_build = _object(first_host.get("build"), "reports[0].measure_host.build")
    first_execution = _object(first_host.get("execution"), "reports[0].measure_host.execution")
    first_compatibility = compatibility_fields[1]
    for run in PROCESS_RUNS[1:]:
        path = f"process_run[{run}]"
        _same(fingerprints_by_run[run], first_fingerprints, f"{path}.source_fingerprints")
        _same(compatibility_fields[run], first_compatibility, f"{path}.sampling_contract")
        host = hosts[run]
        for key in ("architecture", "cpu_model", "os", "os_version"):
            _same(host.get(key), first_host.get(key), f"{path}.measure_host.{key}")
        build = _object(host.get("build"), f"{path}.measure_host.build")
        execution = _object(host.get("execution"), f"{path}.measure_host.execution")
        for key in ("profile", "rustc", "source_revision", "source_sha256", "source_tree_dirty", "target"):
            _same(build.get(key), first_build.get(key), f"{path}.measure_host.build.{key}")
        for key in ("repository_revision", "working_directory"):
            _same(execution.get(key), first_execution.get(key), f"{path}.measure_host.execution.{key}")

    workloads: dict[str, dict[str, Any]] = {}
    summaries: dict[str, dict[str, Any]] = {}
    for stratum_id in STRATA:
        samples_for_stratum: list[dict[str, Any]] = []
        workload: dict[str, Any] | None = None
        all_elapsed: list[int] = []
        for run in PROCESS_RUNS:
            report = indexed[run]
            stratum = strata_by_run[run][stratum_id]
            path = f"process_run[{run}].strata.{stratum_id}"
            expected_case = _profile_case(stratum_id)
            seed = _integer(stratum.get("seed"), f"{path}.seed", minimum=0)
            iterations = _integer(stratum.get("iterations"), f"{path}.iterations", minimum=1)
            guest_work = _integer(stratum.get("guest_work_per_call"), f"{path}.guest_work_per_call", minimum=1)
            code_hex = _string(stratum.get("code_hex"), f"{path}.code_hex")
            if not CODE_HEX_RE.fullmatch(code_hex):
                _fail(f"{path}.code_hex", "must contain lowercase byte-aligned hex")
            digest = _string(stratum.get("code_sha256"), f"{path}.code_sha256")
            if not FINGERPRINT_RE.fullmatch(digest):
                _fail(f"{path}.code_sha256", "must be a lowercase SHA-256 hex digest")
            actual_digest = hashlib.sha256(bytes.fromhex(code_hex)).hexdigest()
            _same(digest, actual_digest, f"{path}.code_sha256")
            for field, actual in (
                ("seed", seed),
                ("iterations", iterations),
                ("guest_work_per_call", guest_work),
                ("code_hex", code_hex),
                ("code_sha256", digest),
            ):
                _same(actual, expected_case[field], f"{path}.{field}.profile_fixture")
            resident_entries = _integer(stratum.get("resident_entries"), f"{path}.resident_entries", minimum=1)
            resident_code_bytes = _integer(stratum.get("resident_code_bytes"), f"{path}.resident_code_bytes", minimum=1)
            resident_logical_bytes = _integer(stratum.get("resident_logical_bytes"), f"{path}.resident_logical_bytes", minimum=1)
            _same(resident_entries, 1, f"{path}.resident_entries")
            warmup_calls = _integer(stratum.get("warmup_calls_per_side"), f"{path}.warmup_calls_per_side", minimum=1)
            calls = _integer(stratum.get("calibrated_calls_per_side"), f"{path}.calibrated_calls_per_side", minimum=1)
            _same(warmup_calls, WARMUP_CALLS, f"{path}.warmup_calls_per_side")
            if calls < INITIAL_CALIBRATION_CALLS or calls > MAX_CALLS or calls & (calls - 1):
                _fail(f"{path}.calibrated_calls_per_side", "must be a power of two within the profile call bounds")
            current_workload = {
                "seed": seed,
                "iterations": iterations,
                "guest_work_per_call": guest_work,
                "code_hex": code_hex,
                "code_sha256": digest,
                "resident_entries": resident_entries,
                "resident_code_bytes": resident_code_bytes,
                "resident_logical_bytes": resident_logical_bytes,
                "warmup_calls_per_side": warmup_calls,
            }
            if workload is None:
                workload = current_workload
            else:
                _same(current_workload, workload, f"{path}.workload")

            calibration_id, calibration = _validate_pair(
                stratum.get("calibration"),
                path=f"{path}.calibration",
                seed=seed,
                iterations=iterations,
                budget_per_call=guest_work,
                require_floor=True,
            )
            _same(calibration_id, 0, f"{path}.calibration.pair")
            calibration_calls = _integer(
                calibration["baseline"]["calls"], f"{path}.calibration.baseline.calls", minimum=1
            )
            if (
                calibration_calls < INITIAL_CALIBRATION_CALLS
                or calibration_calls > MAX_CALLS
                or calibration_calls & (calibration_calls - 1)
            ):
                _fail(f"{path}.calibration", "calls must be a power of two within the profile call bounds")
            for side in ("baseline", "candidate", "interpreter"):
                _same(calibration[side]["calls"], calibration_calls, f"{path}.calibration.{side}.calls")

            retained = _array(stratum.get("retained_pairs"), f"{path}.retained_pairs")
            if len(retained) != PAIRS_PER_STRATUM:
                _fail(
                    f"{path}.retained_pairs",
                    f"expected {PAIRS_PER_STRATUM} retained pairs; got {len(retained)}",
                )
            pair_ids: list[int] = []
            for index, raw_pair in enumerate(retained):
                pair_path = f"{path}.retained_pairs[{index}]"
                pair_id, pair = _validate_pair(
                    raw_pair,
                    path=pair_path,
                    seed=seed,
                    iterations=iterations,
                    budget_per_call=guest_work,
                    require_floor=True,
                )
                if pair_id in pair_ids:
                    _fail(f"{pair_path}.pair", f"duplicate retained pair id {pair_id}")
                pair_ids.append(pair_id)
                for side in ("baseline", "candidate", "interpreter"):
                    _same(pair[side]["calls"], calls, f"{pair_path}.{side}.calls")
                    all_elapsed.append(pair[side]["elapsed_ns"])
                samples_for_stratum.append(pair)

            if len(set(pair_ids)) != PAIRS_PER_STRATUM:
                _fail(f"{path}.retained_pairs", "retained pair ids are not unique")
            restarts = _array(stratum.get("discarded_restarts"), f"{path}.discarded_restarts")
            if len(restarts) > 2:
                _fail(f"{path}.discarded_restarts", "successful report may contain at most two discarded attempts")
            expected_calls = calibration_calls
            for restart_index, raw_restart in enumerate(restarts):
                restart_path = f"{path}.discarded_restarts[{restart_index}]"
                restart = _object(raw_restart, restart_path)
                attempt = _integer(restart.get("attempt"), f"{restart_path}.attempt", minimum=1)
                _same(attempt, restart_index + 1, f"{restart_path}.attempt")
                prev_calls = _integer(restart.get("previous_calls"), f"{restart_path}.previous_calls", minimum=1)
                _same(prev_calls, expected_calls, f"{restart_path}.previous_calls")
                next_calls = _integer(
                    restart.get("next_calls"), f"{restart_path}.next_calls", minimum=prev_calls + 1
                )
                _same(next_calls, min(prev_calls * 2, MAX_CALLS), f"{restart_path}.next_calls")
                discarded = _array(restart.get("discarded_pairs"), f"{restart_path}.discarded_pairs")
                if len(discarded) != PAIRS_PER_STRATUM:
                    _fail(f"{restart_path}.discarded_pairs", f"expected {PAIRS_PER_STRATUM} pairs")
                floor_missed = False
                for pair_index, raw_pair in enumerate(discarded):
                    pair_path = f"{restart_path}.discarded_pairs[{pair_index}]"
                    pair_id, pair = _validate_pair(
                        raw_pair,
                        path=pair_path,
                        seed=seed,
                        iterations=iterations,
                        budget_per_call=guest_work,
                        require_floor=False,
                    )
                    _same(pair_id, pair_index + restart_index * PAIRS_PER_STRATUM, f"{pair_path}.pair")
                    for side in ("baseline", "candidate", "interpreter"):
                        _same(pair[side]["calls"], prev_calls, f"{pair_path}.{side}.calls")
                        floor_missed |= pair[side]["elapsed_ns"] < SAMPLE_FLOOR_NS
                if not floor_missed:
                    _fail(restart_path, "discarded restart has no sample below the minimum floor")
                expected_calls = next_calls
            _same(calls, expected_calls, f"{path}.calibrated_calls_per_side")
            expected_pair_ids = list(
                range(len(restarts) * PAIRS_PER_STRATUM, (len(restarts) + 1) * PAIRS_PER_STRATUM)
            )
            _same(pair_ids, expected_pair_ids, f"{path}.retained_pairs.ids")

        assert workload is not None
        workloads[stratum_id] = workload
        base_ratios = [
            _ratio(pair, "candidate", "baseline", f"{stratum_id}.pair[{pair['pair']}]")
            for pair in samples_for_stratum
        ]
        interpreter_ratios = [
            _ratio(pair, "candidate", "interpreter", f"{stratum_id}.pair[{pair['pair']}]")
            for pair in samples_for_stratum
        ]
        summaries[stratum_id] = {
            "seed": workload["seed"],
            "iterations": workload["iterations"],
            "guest_work_per_call": workload["guest_work_per_call"],
            "code_sha256": workload["code_sha256"],
            "retained_pair_count": len(samples_for_stratum),
            "retained_pair_ids_by_run": {
                str(run): [pair["pair"] for pair in strata_by_run[run][stratum_id]["retained_pairs"]]
                for run in PROCESS_RUNS
            },
            "sample_floor_ns": SAMPLE_FLOOR_NS,
            "minimum_retained_elapsed_ns": min(all_elapsed),
            "candidate_over_baseline_ratio": _ratio_summary(base_ratios),
            "candidate_over_interpreter_ratio": _ratio_summary(interpreter_ratios),
        }

    reference_fingerprints = _fingerprints(
        indexed[1]["source_fingerprints"], "process_run[1].source_fingerprints"
    )
    preflight_summary = None
    if preflight is not None:
        preflight_summary = _validate_preflight(preflight, ordered, reference_fingerprints)

    return {
        "schema": 1,
        "status": "passed",
        "profile_id": PROFILE_ID,
        "process_runs": list(PROCESS_RUNS),
        "source_fingerprints": [[name, value] for name, value in sorted(reference_fingerprints.items())],
        "build": {
            key: first_build[key]
            for key in ("profile", "rustc", "source_revision", "source_sha256", "source_tree_dirty", "target")
        },
        "host_runs": [
            {"process_run": run, "measure_host": hosts[run]}
            for run in PROCESS_RUNS
        ],
        "sampling_protocol": first_compatibility["sampling_protocol"],
        "timed_scope": first_compatibility["timed_scope"],
        "ratio_contract": first_compatibility["ratio_contract"],
        "workloads": workloads,
        "preflight": preflight_summary,
        "strata": summaries,
    }


def _ratio_summary(values: list[float]) -> dict[str, float]:
    if not values or any(not math.isfinite(value) or value <= 0 for value in values):
        _fail("ratios", "must contain finite positive values")
    return {
        "count": len(values),
        "median": statistics.median(values),
        "min": min(values),
        "max": max(values),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reports", nargs=3, type=Path, metavar=("RUN1.json", "RUN2.json", "RUN3.json"))
    parser.add_argument("--preflight", type=Path, help="optional untimed preflight report")
    args = parser.parse_args(argv)
    try:
        reports = [load_report(path) for path in args.reports]
        preflight = load_report(args.preflight) if args.preflight else None
        summary = validate_reports(reports, preflight)
    except ValidationError as error:
        print(f"validation failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps(summary, allow_nan=False, sort_keys=True, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
