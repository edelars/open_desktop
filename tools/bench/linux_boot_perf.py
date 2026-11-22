#!/usr/bin/env python3
"""PERF-01: reproducible Linux boot measurement and golden run for the CPU core performance line.

Subcommands (plan: docs/plans/CPU_CORE_PERF.md, methodology: docs/testing/BENCHMARKS.md):

  build    build the runner, the initramfs, the ROM, the ESP and the NVRAM seed once; write inputs.json
  record   one runner run on the unified loop; save work/cpu-core-perf/<label>-<budget>.json
  compare  compare two records field by field: GOLDEN: MATCH (exit 0), GOLDEN: MISMATCH (1), INPUTS DIFFER (2)
  measure  R sequential runs under /usr/bin/time -l; wall, host instructions, cycles, peak RSS

Runs are always sequential: never start two runner processes at once. Everything lands in
work/cpu-core-perf/, which is not in Git (the /work/** rule of .gitignore).
"""
from __future__ import annotations

import argparse
import fcntl
import json
import os
import re
import statistics
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools" / "bench"))
sys.path.insert(0, str(ROOT / "tests" / "system"))
import firmware_boot_bench as bench  # noqa: E402  (sha256, command, host_fingerprint)

WORK = ROOT / "work" / "cpu-core-perf"
INPUTS = WORK / "inputs"
INPUTS_JSON = WORK / "inputs.json"
# Fixed output paths of every run: the runner prints them in its JSON, so they must not depend on the label.
RUN_DIR = WORK / "run"
COM1 = RUN_DIR / "com1.bin"
NVRAM_OUT = RUN_DIR / "nvram.out"
STOP_SNAPSHOT = RUN_DIR / "stop-snapshot.json"
DEFAULT_STAGE4 = ROOT / "work" / "stage4"
KERNEL_REL = Path("linux-7.0.0-38.38") / "vmlinuz-7.0.0-38-generic"
INITRAMFS_BUILDER = ROOT / "tools" / "build" / "build_s7_31n_initramfs.py"
FIRMWARE_BUILDER = ROOT / "tools" / "build" / "build_firmware.py"
DISK_BUILDER = ROOT / "tools" / "build" / "make_test_disk.py"
INIT_LINE = b"Run /init as init process"
PRIORITY = -5
EXIT_MATCH, EXIT_MISMATCH, EXIT_INPUTS = 0, 1, 2


def runner_path() -> Path:
    return ROOT / Path(os.environ.get("CARGO_TARGET_DIR", "target")) / "release" / "od-runner"


def sha256_file(path: Path) -> str:
    return bench.sha256(path.read_bytes())


def git_state() -> dict:
    head = bench.command(["git", "rev-parse", "HEAD"])
    diff = subprocess.check_output(["git", "diff", "--binary", "HEAD", "--", "crates", "tools/runner", "Cargo.toml",
                                    "Cargo.lock"], cwd=ROOT)
    return {"commit": head, "source_diff_sha256": bench.sha256(diff)}


def build_runner() -> None:
    subprocess.run(["cargo", "build", "--release", "-p", "od-runner", "--offline"], check=True, cwd=ROOT)


def cmd_build(args: argparse.Namespace) -> int:
    WORK.mkdir(parents=True, exist_ok=True)
    build_runner()
    if args.runner_only:
        if not INPUTS_JSON.exists():
            print("build --runner-only needs inputs.json from an earlier full build", file=sys.stderr)
            return 1
        data = json.loads(INPUTS_JSON.read_text())
        data["runner_sha256"] = sha256_file(runner_path())
        data.update(git_state())
        INPUTS_JSON.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n")
        print(f"runner rebuilt: {data['runner_sha256']}")
        return 0
    kernel = args.stage4_dir / KERNEL_REL
    if not kernel.is_file():
        print(f"BLOCKER: kernel not found: {kernel}", file=sys.stderr)
        return 1
    import firmware_linux_uefi_efivars_acpi as efi  # noqa: E402  (NVRAM seed; reused, not copied)

    INPUTS.mkdir(parents=True, exist_ok=True)
    initramfs_dir = INPUTS / "initramfs"
    subprocess.run([sys.executable, INITRAMFS_BUILDER, "--out-dir", initramfs_dir], check=True, cwd=ROOT,
                   stdout=subprocess.DEVNULL)
    initramfs = initramfs_dir / "initramfs.cpio"
    rom = INPUTS / "firmware.bin"
    subprocess.run([sys.executable, FIRMWARE_BUILDER, "--out", rom, "--target-dir", INPUTS / "firmware-target"],
                   check=True, cwd=ROOT, stdout=subprocess.DEVNULL)
    esp = INPUTS / "esp.raw"
    subprocess.run([sys.executable, DISK_BUILDER, "--esp-fat32", "--bootx64", kernel, "--boot-file",
                    f"INITRD.IMG={initramfs}", "--out", esp], check=True, cwd=ROOT)
    seed = INPUTS / "seed.nv"
    efi.write_nvram_seed(seed)
    data = {
        "schema": "od-cpu-core-perf-inputs-v1",
        "inputs": {  # the data inputs of a run: records are comparable only when these are equal
            "vmlinuz_sha256": sha256_file(kernel),
            "initramfs_sha256": sha256_file(initramfs),
            "firmware_rom_sha256": sha256_file(rom),
            "esp_sha256": sha256_file(esp),
            "nvram_seed_sha256": sha256_file(seed),
            "cmdline": efi.stop.CMDLINE,
        },
        "paths": {"rom": str(rom), "esp": str(esp), "seed": str(seed), "kernel": str(kernel),
                  "initramfs": str(initramfs)},
        "runner_sha256": sha256_file(runner_path()),
        "build_profile": "release (cargo build --release -p od-runner --offline)",
        **git_state(),
    }
    INPUTS_JSON.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n")
    for name, value in data["inputs"].items():
        print(f"{name}: {value}")
    print(f"runner_sha256: {data['runner_sha256']}")
    print(f"wrote {INPUTS_JSON}")
    return 0


def load_inputs() -> dict:
    if not INPUTS_JSON.exists():
        raise SystemExit("inputs.json is missing: run 'build' first")
    data = json.loads(INPUTS_JSON.read_text())
    if sha256_file(runner_path()) != data.get("runner_sha256"):
        raise SystemExit("runner binary differs from inputs.json: run 'build --runner-only'")
    for key in ("rom", "esp", "seed"):
        path = Path(data["paths"][key])
        if sha256_file(path) != data["inputs"][{"rom": "firmware_rom_sha256", "esp": "esp_sha256",
                                                   "seed": "nvram_seed_sha256"}[key]]:
            raise SystemExit(f"{path} changed after build: rebuild the inputs")
    return data


def runner_argv(inputs: dict, budget: int, *, snapshot: bool) -> list:
    paths = inputs["paths"]
    argv = [str(runner_path()), "boot-firmware", "--image", paths["rom"], "--nvme-image", paths["esp"],
            "--budget", str(budget), "--com1-out", str(COM1), "--nvram-in", paths["seed"],
            "--nvram-out", str(NVRAM_OUT), "--cpu-loop", "unified"]
    if snapshot:
        argv += ["--stop-snapshot", str(STOP_SNAPSHOT)]
    return argv


def clean_run_dir() -> None:
    RUN_DIR.mkdir(parents=True, exist_ok=True)
    for path in (COM1, NVRAM_OUT, STOP_SNAPSHOT):
        path.unlink(missing_ok=True)


def digest_if_exists(path: Path) -> str | None:
    return sha256_file(path) if path.exists() else None


def dig(value, *keys):
    for key in keys:
        if not isinstance(value, dict) or key not in value:
            return None
        value = value[key]
    return value


def cmd_record(args: argparse.Namespace) -> int:
    inputs = load_inputs()
    clean_run_dir()
    argv = runner_argv(inputs, args.budget, snapshot=not args.no_snapshot)
    started = time.monotonic()
    proc = subprocess.run(argv, cwd=ROOT, capture_output=True, text=True)
    wall = time.monotonic() - started
    try:
        report = json.loads(proc.stdout)
    except json.JSONDecodeError:
        print(f"runner printed no JSON (exit {proc.returncode}):\n{proc.stdout[-2000:]}\n{proc.stderr[-2000:]}",
              file=sys.stderr)
        return 1
    stem = f"{args.label}-{args.budget}"
    copies = {}
    for kind, source in (("com1", COM1), ("nvram", NVRAM_OUT), ("stop-snapshot", STOP_SNAPSHOT)):
        if source.exists():
            target = WORK / f"{stem}.{kind}"
            target.write_bytes(source.read_bytes())
            copies[kind] = str(target)
    com1 = COM1.read_bytes() if COM1.exists() else b""
    record = {
        "schema": "od-cpu-core-perf-record-v1",
        "label": args.label,
        "budget": args.budget,
        "runner_exit_code": proc.returncode,
        "runner_stdout": report,
        "com1_sha256": digest_if_exists(COM1),
        "nvram_sha256": digest_if_exists(NVRAM_OUT),
        "stop_snapshot_sha256": digest_if_exists(STOP_SNAPSHOT),
        "inputs": inputs["inputs"],
        "runner_sha256": sha256_file(runner_path()),
        "wall_s": round(wall, 3),
        "files": copies,
        "facts": {
            "init_line_in_com1": INIT_LINE in com1,
            "irqs_delivered": dig(report, "unified", "interrupts", "irqs_delivered"),
            "cpl3_retired": dig(report, "unified", "cpl3_retired"),
            "retired": report.get("retired"),
            "stop_snapshot": report.get("stop_snapshot"),
        },
        **git_state(),
    }
    out = WORK / f"{stem}.json"
    out.write_text(json.dumps(record, indent=2, sort_keys=True) + "\n")
    print(f"runner exit {proc.returncode}, wall {wall:.1f} s, retired {report.get('retired')}")
    print(f"facts: {json.dumps(record['facts'], sort_keys=True)}")
    print(f"wrote {out}")
    return 0


def flatten(value, prefix="") -> dict:
    if isinstance(value, dict):
        result = {}
        for key, item in value.items():
            result.update(flatten(item, f"{prefix}.{key}" if prefix else str(key)))
        return result
    if isinstance(value, list):
        result = {f"{prefix}.#len": len(value)}
        for index, item in enumerate(value):
            result.update(flatten(item, f"{prefix}[{index}]"))
        return result
    return {prefix: value}


COMPARED = ("runner_exit_code", "com1_sha256", "nvram_sha256", "stop_snapshot_sha256")


def cmd_compare(args: argparse.Namespace) -> int:
    records = []
    for path in (args.a, args.b):
        try:
            record = json.loads(path.read_text())
        except (OSError, json.JSONDecodeError) as exc:
            print(f"cannot read {path}: {exc}", file=sys.stderr)
            return EXIT_INPUTS
        if not isinstance(record, dict) or record.get("schema") != "od-cpu-core-perf-record-v1":
            print(f"not a record file (schema od-cpu-core-perf-record-v1 expected): {path}", file=sys.stderr)
            return EXIT_INPUTS
        records.append(record)
    first, second = records
    if first["inputs"] != second["inputs"]:
        print("INPUTS DIFFER")
        for key in sorted(set(first["inputs"]) | set(second["inputs"])):
            if first["inputs"].get(key) != second["inputs"].get(key):
                print(f"  {key}: {first['inputs'].get(key)} != {second['inputs'].get(key)}")
        return EXIT_INPUTS
    left = {key: first.get(key) for key in COMPARED}
    right = {key: second.get(key) for key in COMPARED}
    left.update({f"runner_stdout.{k}": v for k, v in flatten(first["runner_stdout"]).items()})
    right.update({f"runner_stdout.{k}": v for k, v in flatten(second["runner_stdout"]).items()})
    differing = [key for key in sorted(set(left) | set(right)) if left.get(key) != right.get(key)]
    if not differing:
        print("GOLDEN: MATCH")
        return EXIT_MATCH
    print("GOLDEN: MISMATCH")
    for key in differing:
        print(f"  {key}: {left.get(key)!r} != {right.get(key)!r}")
    return EXIT_MISMATCH


TIME_FIELDS = {
    "real": re.compile(r"^\s*([0-9.]+)\s+real", re.M),
    "max_rss_bytes": re.compile(r"^\s*(\d+)\s+maximum resident set size", re.M),
    "host_instructions": re.compile(r"^\s*(\d+)\s+instructions retired", re.M),
    "host_cycles": re.compile(r"^\s*(\d+)\s+cycles elapsed", re.M),
    "peak_footprint_bytes": re.compile(r"^\s*(\d+)\s+peak memory footprint", re.M),
}


def parse_time(text: str) -> dict:
    result = {}
    for name, pattern in TIME_FIELDS.items():
        match = pattern.search(text)
        result[name] = float(match.group(1)) if match and name == "real" else (int(match.group(1)) if match else None)
    return result


def spread(values: list[float]) -> dict:
    return {"median": statistics.median(values), "min": min(values), "max": max(values)}


def cmd_measure(args: argparse.Namespace) -> int:
    inputs = load_inputs()
    priority = os.getpriority(os.PRIO_PROCESS, 0)
    preliminary = priority != PRIORITY
    if preliminary:
        print(f"WARNING: process priority is {priority}, not {PRIORITY}: result is preliminary. Run via\n"
              f"  sudo nice -n {PRIORITY} sudo -u \"$USER\" python3 tools/bench/linux_boot_perf.py measure ...",
              file=sys.stderr)
    runs, fingerprint = [], None
    for index in range(args.runs):
        clean_run_dir()
        time_out = RUN_DIR / "time.txt"
        time_out.unlink(missing_ok=True)
        argv = ["/usr/bin/time", "-l", "-o", str(time_out)] + runner_argv(inputs, args.budget, snapshot=False)
        started = time.monotonic()
        proc = subprocess.run(argv, cwd=ROOT, capture_output=True, text=True)
        wall = time.monotonic() - started
        try:
            report = json.loads(proc.stdout)
        except json.JSONDecodeError:
            print(f"run {index}: runner printed no JSON (exit {proc.returncode})", file=sys.stderr)
            return 1
        current = {"retired": report.get("retired"), "com1_sha256": digest_if_exists(COM1),
                   "runner_exit_code": proc.returncode}
        if fingerprint is None:
            fingerprint = current
        elif current != fingerprint:
            print(f"run {index}: result changed between runs: {current} != {fingerprint}", file=sys.stderr)
            return 1
        timing = parse_time(time_out.read_text()) if time_out.exists() else {}
        retired = report["retired"]
        row = {"wall_s": round(wall, 3), "time_real_s": timing.get("real"), "retired": retired, **timing,
               "minst_per_s": round(retired / wall / 1e6, 4)}
        if timing.get("host_instructions"):
            row["host_instr_per_guest"] = round(timing["host_instructions"] / retired, 2)
        runs.append(row)
        print(f"run {index + 1}/{args.runs}: wall {wall:.1f} s, {row['minst_per_s']} Minst/s"
              f"{', ' + str(row['host_instr_per_guest']) + ' host/guest' if 'host_instr_per_guest' in row else ''}",
              flush=True)
    summary = {"minst_per_s": spread([row["minst_per_s"] for row in runs]),
               "wall_s": spread([row["wall_s"] for row in runs])}
    ratios = [row["host_instr_per_guest"] for row in runs if "host_instr_per_guest" in row]
    if ratios:
        summary["host_instr_per_guest"] = spread(ratios)
    rss = [row["max_rss_bytes"] for row in runs if row.get("max_rss_bytes")]
    if rss:
        summary["max_rss_bytes"] = spread(rss)
    result = {
        "schema": "od-cpu-core-perf-measure-v1",
        "label": args.label, "budget": args.budget, "runs_requested": args.runs,
        "priority": priority, "preliminary": preliminary,
        "inputs": inputs["inputs"], "runner_sha256": sha256_file(runner_path()),
        "build_profile": inputs.get("build_profile"), "host": bench.host_fingerprint(),
        "fingerprint": fingerprint, "runs": runs, "summary": summary, **git_state(),
    }
    out = WORK / f"measure-{args.label}-{args.budget}.json"
    out.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    for name, stats in summary.items():
        print(f"{name}: median {stats['median']} min {stats['min']} max {stats['max']}")
    print(f"preliminary: {preliminary}; wrote {out}")
    return 0


_RUN_LOCK = None  # held for the life of the process


def acquire_run_lock() -> bool:
    """Only one record or measure may run at a time: a second one would delete the first one's files."""
    global _RUN_LOCK
    RUN_DIR.parent.mkdir(parents=True, exist_ok=True)
    handle = open(RUN_DIR.parent / ".run.lock", "w")
    try:
        fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        handle.close()
        return False
    _RUN_LOCK = handle
    return True


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    build = sub.add_parser("build", help="build the runner and all inputs")
    build.add_argument("--runner-only", action="store_true", help="rebuild only the runner")
    build.add_argument("--stage4-dir", type=Path, default=DEFAULT_STAGE4,
                       help="directory with linux-7.0.0-38.38/ (default: work/stage4 of this checkout)")
    build.set_defaults(func=cmd_build)
    record = sub.add_parser("record", help="one golden run")
    record.add_argument("--budget", type=int, required=True)
    record.add_argument("--label", required=True)
    record.add_argument("--no-snapshot", action="store_true", help="do not pass --stop-snapshot")
    record.set_defaults(func=cmd_record)
    compare = sub.add_parser("compare", help="compare two records")
    compare.add_argument("a", type=Path)
    compare.add_argument("b", type=Path)
    compare.set_defaults(func=cmd_compare)
    measure = sub.add_parser("measure", help="timed sequential runs (priority -5 expected)")
    measure.add_argument("--budget", type=int, required=True)
    measure.add_argument("--runs", type=int, required=True)
    measure.add_argument("--label", required=True)
    measure.set_defaults(func=cmd_measure)
    args = parser.parse_args()
    if args.command in ("record", "measure") and (args.budget <= 0 or (args.command == "measure" and args.runs < 1)):
        parser.error("budget and runs must be positive")
    if args.command in ("record", "measure") and not acquire_run_lock():
        print("another record or measure is running (lock: work/cpu-core-perf/.run.lock)", file=sys.stderr)
        return 1
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
