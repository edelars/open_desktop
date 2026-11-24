#!/usr/bin/env python3
"""PERF-01: reproducible Linux boot measurement and golden run for the CPU core performance line.

Subcommands (plan: docs/plans/CPU_CORE_PERF.md, methodology: docs/testing/BENCHMARKS.md):

  build    build the runner, the initramfs, the ROM, the ESP and the NVRAM seed once; write inputs.json
  record   one runner run on the unified loop; save work/cpu-core-perf/<label>-<budget>.json
  compare  compare two records field by field: GOLDEN: MATCH (exit 0), GOLDEN: MISMATCH (1), INPUTS DIFFER (2)
  measure  R sequential runs under /usr/bin/time -l; wall, host instructions, cycles, peak RSS
  profile  one run under /usr/bin/sample; self and inclusive tables by function (PERF-02, K6)

Runs are always sequential: never start two runner processes at once. Everything lands in
work/cpu-core-perf/, which is not in Git (the /work/** rule of .gitignore).
"""
from __future__ import annotations

import argparse
import fcntl
import json
import os
import re
import signal
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


def runner_argv(inputs: dict, budget: int, *, snapshot: bool, fast_path: bool = False,
                data_cache: bool = False, block_cache: bool = False) -> list:
    paths = inputs["paths"]
    argv = [str(runner_path()), "boot-firmware", "--image", paths["rom"], "--nvme-image", paths["esp"],
            "--budget", str(budget), "--com1-out", str(COM1), "--nvram-in", paths["seed"],
            "--nvram-out", str(NVRAM_OUT), "--cpu-loop", "unified"]
    if snapshot:
        argv += ["--stop-snapshot", str(STOP_SNAPSHOT)]
    if fast_path:
        argv += ["--fast-path"]
    if data_cache:
        # PERF-11: the data translation cache prototype (ADR-0016); the golden record must not change.
        argv += ["--data-cache-prototype"]
    if block_cache:
        # PERF-31 variant (a): the block cache prototype (ADR-0018); needs --fast-path, unified only.
        argv += ["--block-cache-prototype"]
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
    argv = runner_argv(inputs, args.budget, snapshot=not args.no_snapshot, fast_path=args.fast_path,
                       data_cache=args.data_cache_prototype,
                       block_cache=args.block_cache_prototype)
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
        "fast_path": bool(args.fast_path),
        "data_cache_prototype": bool(args.data_cache_prototype), "block_cache_prototype": bool(args.block_cache_prototype),
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
    if not args.allow_fast_path_diff and bool(first.get("fast_path", False)) != bool(second.get("fast_path", False)):
        print("INPUTS DIFFER")
        print(f"  fast_path: {bool(first.get('fast_path', False))} != {bool(second.get('fast_path', False))} "
              "(use --allow-fast-path-diff to compare a fast-path run with the base)")
        return EXIT_INPUTS
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
        argv = ["/usr/bin/time", "-l", "-o", str(time_out)] + runner_argv(inputs, args.budget, snapshot=False,
                                                                       fast_path=args.fast_path,
                                                                       data_cache=args.data_cache_prototype,
                       block_cache=args.block_cache_prototype)
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
        "priority": priority, "preliminary": preliminary, "fast_path": bool(args.fast_path),
        "data_cache_prototype": bool(args.data_cache_prototype), "block_cache_prototype": bool(args.block_cache_prototype),
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


# ---------------------------------------------------------------------------------------------------------------
# profile: /usr/bin/sample of a running runner and the "Call graph" analysis (PERF-02, K6)
# ---------------------------------------------------------------------------------------------------------------

SAMPLE = Path("/usr/bin/sample")
PROFILE_TOP = 40
NAME_LIMIT = 110
# A line of the "Call graph" section: a prefix of ' ', '+', '!', ':', '|' (its length is the depth), the sample
# count, then the node text:  name  (in image) + offset  [0xaddress]  file:line
GRAPH_LINE = re.compile(r"^([ +!:|]*)(\d+) (.*)$")
NODE_END = re.compile(r"\s+\(in [^)]*\)|\s+\[0x[0-9a-fA-F]+\]|\s+\+\s+\d+\s+\[0x")
NODE_IMAGE = re.compile(r"\(in ([^)]*)\)")
GRAPH_END = ("Total number in stack", "Sort by top of stack", "Binary Images:")


class ProfileNode:
    __slots__ = ("name", "image", "count", "children", "line")

    def __init__(self, name: str, image: str | None, count: int, line: int):
        self.name, self.image, self.count, self.line = name, image, count, line
        self.children: list[ProfileNode] = []


BASE62 = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"
RAW_MARK = "[raw] "          # a name that could not be shortened: kept as it is and marked (never passed off as short)
# Leaf functions of libsystem_kernel in which a thread waits (does not run on a CPU)
WAIT_NAME = re.compile(r"^(?:\w*_trap|__psynch_\w+|__semwait\w*|mach_msg\w*|kevent\w*|__workq_kernreturn|swtch_pri)$")
WAIT_IMAGE = "libsystem_kernel.dylib"
# A thread counts as running on a CPU when its CPU samples are at least this share of the busiest root's samples
CPU_THREAD_MIN_PERCENT = 5.0
THREAD_LINE = re.compile(r"^Thread_(\d+):?\s*(.*)$")


class _V0Error(ValueError):
    pass


class _V0Parser:
    """Recursive parser of the path of a Rust v0 symbol (the text after '_R'), grammar of the Rust mangling RFC 2603.
    path() returns 'crate::module::function' without generic arguments; types are only skipped. impl paths (M, X)
    give the path of the impl (the self type is skipped), Y gives the trait path. Whatever follows the top-level
    path (the instantiating crate, a vendor suffix) is ignored. Raises _V0Error on anything that does not fit."""

    def __init__(self, symbol: str):
        self.sym, self.pos, self.steps = symbol, 0, 0

    def peek(self) -> str:
        return self.sym[self.pos] if self.pos < len(self.sym) else ""

    def take(self) -> str:
        char = self.peek()
        if not char:
            raise _V0Error("end of symbol")
        self.pos += 1
        return char

    def expect(self, char: str) -> None:
        if self.take() != char:
            raise _V0Error(f"expected {char!r} at {self.pos - 1}")

    def count(self) -> None:
        self.steps += 1
        if self.steps > 20000:
            raise _V0Error("symbol too complex")

    def base62(self) -> int:
        """{0-9a-zA-Z} '_': '_' is 0, anything else is its value plus one."""
        value, digits = 0, 0
        while True:
            char = self.take()
            if char == "_":
                return value + 1 if digits else 0
            digit = BASE62.find(char)
            if digit < 0:
                raise _V0Error(f"bad base-62 digit {char!r} at {self.pos - 1}")
            value, digits = value * 62 + digit, digits + 1

    def ident(self) -> str:
        """[disambiguator 's' base62] ['u'] length ['_'] bytes; an empty identifier (closures) is returned as ''."""
        if self.peek() == "s":
            self.pos += 1
            self.base62()
        if self.peek() == "u":
            self.pos += 1
        start = self.pos
        if self.peek() == "0":                            # decimal number: '0' alone, else no leading zero
            self.pos += 1
        else:
            while self.peek().isascii() and self.peek().isdigit():
                self.pos += 1
        if start == self.pos:
            raise _V0Error(f"identifier length expected at {start}")
        length = int(self.sym[start:self.pos])
        if self.peek() == "_":
            self.pos += 1
        end = self.pos + length
        if end > len(self.sym):
            raise _V0Error("identifier past the end of the symbol")
        text, self.pos = self.sym[self.pos:end], end
        return text

    def backref(self, parse):
        """'B' base62: the item at that earlier offset of the symbol; parse continues from the end of the reference."""
        at = self.pos - 1
        target = self.base62()
        if target >= at:
            raise _V0Error(f"back reference {target} is not backwards at {at}")
        after, self.pos = self.pos, target
        try:
            return parse()
        finally:
            self.pos = after

    def type_path(self):
        """The self type of an impl: its path when it is a path type, otherwise skipped (None)."""
        c = self.peek()
        if c and c in "CMXYNI":
            return self.path()
        if c == "B":
            self.pos += 1
            return self.backref(self.type_path)
        self.skip_type()
        return None

    def path(self) -> str:
        self.count()
        char = self.take()
        if char == "C":
            return self.ident()
        if char == "N":
            self.take()                                   # namespace: C closure, S shim, upper-case others
            base = self.path()
            return f"{base}::{self.ident() or '{closure}'}"
        if char in "MX":
            if self.peek() == "s":                        # impl disambiguator
                self.pos += 1
                self.base62()
            base = self.path()
            owner = self.type_path()                      # the self type: keeps Type::method apart
            if char == "X":
                self.path()                               # the trait
            return owner or base
        if char == "Y":
            self.skip_type()
            return self.path()
        if char == "I":
            base = self.path()
            while self.peek() != "E":
                self.skip_generic_arg()
            self.pos += 1
            return base
        if char == "B":
            return self.backref(self.path)
        raise _V0Error(f"path expected, got {char!r} at {self.pos - 1}")

    def skip_lifetime(self) -> None:
        self.expect("L")
        self.base62()

    def skip_generic_arg(self) -> None:
        char = self.peek()
        if char == "L":
            self.skip_lifetime()
        elif char == "K":
            self.pos += 1
            self.skip_const()
        else:
            self.skip_type()

    def skip_const(self) -> None:
        self.count()
        char = self.peek()
        if char == "p":
            self.pos += 1
        elif char == "B":
            self.pos += 1
            self.backref(self.skip_const)
        else:
            self.skip_type()
            if self.peek() == "n":
                self.pos += 1
            while self.peek() and self.peek() in "0123456789abcdef":
                self.pos += 1
            self.expect("_")

    def skip_type(self) -> None:
        self.count()
        char = self.take()
        if char.islower():                                # basic type
            return
        if char in "CMXYNI":
            self.pos -= 1
            self.path()
        elif char == "B":
            self.backref(self.skip_type)
        elif char == "A":
            self.skip_type()
            self.skip_const()
        elif char in "SPO":
            self.skip_type()
        elif char in "RQ":
            if self.peek() == "L":
                self.skip_lifetime()
            self.skip_type()
        elif char == "T":
            while self.peek() != "E":
                self.skip_type()
            self.pos += 1
        elif char == "F":
            if self.peek() == "G":
                self.pos += 1
                self.base62()
            if self.peek() == "U":
                self.pos += 1
            if self.peek() == "K":
                self.pos += 1
                if self.peek() == "C":
                    self.pos += 1
                else:
                    self.ident()
            while self.peek() != "E":
                self.skip_type()
            self.pos += 1
            self.skip_type()
        elif char == "D":
            if self.peek() == "G":
                self.pos += 1
                self.base62()
            while self.peek() != "E":
                self.path()
                while self.peek() == "p":                 # associated type binding
                    self.pos += 1
                    self.ident()
                    self.skip_type()
            self.pos += 1
            self.skip_lifetime()
        else:
            raise _V0Error(f"type expected, got {char!r} at {self.pos - 1}")


def v0_path(symbol: str) -> str:
    """Path of a v0 symbol without the '_R' prefix; raises _V0Error when it is not a well-formed v0 symbol."""
    parser = _V0Parser(symbol)
    while parser.peek().isascii() and parser.peek().isdigit():     # optional encoding version
        parser.pos += 1
    path = parser.path()
    if not path:
        raise _V0Error("empty path")
    return path


def _strip_generics(name: str) -> str:
    """Drop generic argument lists (after an identifier or ':'); keep '<T as Trait>' qualified paths."""
    out, skip, previous = [], 0, ""
    for char in name:
        if skip:
            if char == "<":
                skip += 1
            elif char == ">" and previous != "-":
                skip -= 1
        elif char == "<" and previous and (previous.isalnum() or previous in "_:"):
            skip = 1
            while out and out[-1] == ":" and len(out) >= 2 and out[-2] == ":":   # 'name::<T>' loses its '::' too
                del out[-2:]
        else:
            out.append(char)
        previous = char
    return "".join(out)


def _limit(name: str) -> str:
    return name if len(name) <= NAME_LIMIT else name[:NAME_LIMIT - 3] + "..."


def shorten_name(raw: str) -> str:
    """Rust symbol to 'module::path::function'; v0 (mangled or demangled), legacy hash and generic arguments are dropped.
    A mangled v0 name that does not parse is returned as it is with the prefix RAW_MARK. Names of other languages
    (libsystem and so on) stay as they are."""
    name = raw.strip()
    name = re.sub(r"\.llvm\.\d+$", "", name)
    if re.match(r"^_{1,2}R[A-Z]", name):
        try:
            name = v0_path(name.lstrip("_")[1:])
        except _V0Error:
            return _limit(RAW_MARK + name)
    name = re.sub(r"\[[0-9a-f]{4,16}\]", "", name)     # crate disambiguator of demangled v0: od_isa[1a2b3c]::f
    name = re.sub(r"::h[0-9a-f]{16}$", "", name)       # legacy hash
    name = _strip_generics(name)
    name = name.replace("{{closure}}", "{closure}")
    return _limit(name)


def parse_call_graph(text: str) -> list[ProfileNode]:
    """Roots (one per thread) of the 'Call graph' section; the parent of a node is the nearest earlier node with a
    shorter prefix. Raises ValueError when there is no section, when a line of the section (before the 'Total number
    in stack' line) is not a node, or when the section is empty."""
    lines = text.splitlines()
    try:
        start = next(i for i, line in enumerate(lines) if line.strip() == "Call graph:") + 1
    except StopIteration:
        raise ValueError("no 'Call graph:' section in the sample output") from None
    roots: list[ProfileNode] = []
    stack: list[tuple[int, ProfileNode]] = []
    for number in range(start, len(lines)):
        line = lines[number]
        if not line.strip():
            continue
        if line.startswith(GRAPH_END):
            break
        match = GRAPH_LINE.match(line)
        if not match:
            raise ValueError(f"line {number + 1}: not a call graph node: {line[:80]!r}")
        depth, count, rest = len(match.group(1)), int(match.group(2)), match.group(3)
        end = NODE_END.search(rest)
        name = (rest[:end.start()] if end else rest).strip()
        image = NODE_IMAGE.search(rest)
        thread = THREAD_LINE.match(name)
        if thread:        # 'Thread_123: od-vcpu-0' or 'Thread_123   DispatchQueue_1: com.apple.main-thread  (serial)'
            label = " ".join(thread.group(2).split()) or f"Thread_{thread.group(1)}"
            node = ProfileNode(_limit(label), None, count, number + 1)
        else:
            node = ProfileNode(shorten_name(name), image.group(1) if image else None, count, number + 1)
        while stack and stack[-1][0] >= depth:
            stack.pop()
        if stack:
            stack[-1][1].children.append(node)
        else:
            roots.append(node)
        stack.append((depth, node))
    if not roots:
        raise ValueError("the 'Call graph:' section is empty")
    return roots


def analyse_tree(root: ProfileNode) -> tuple[dict, dict, dict, int]:
    """(self, inclusive, image, waiting) per function name for one tree. self = count minus the children's counts
    (a negative value raises ValueError); inclusive = count of every node whose name is not already on the path from
    the root (recursion counted once); waiting = self of the libsystem_kernel wait leaves (swtch_pri, *_trap, ...)."""
    own: dict[str, int] = {}
    inclusive: dict[str, int] = {}
    images: dict[str, str | None] = {}
    active: dict[str, int] = {}
    waiting = 0
    work: list[tuple[ProfileNode, bool]] = [(root, False)]
    while work:
        node, leaving = work.pop()
        if leaving:
            active[node.name] -= 1
            continue
        own_count = node.count - sum(child.count for child in node.children)
        if own_count < 0:
            raise ValueError(f"line {node.line}: {node.name} counts {node.count}, its children {node.count - own_count}")
        own[node.name] = own.get(node.name, 0) + own_count
        if node.image == WAIT_IMAGE and WAIT_NAME.match(node.name):
            waiting += own_count
        images.setdefault(node.name, node.image)
        if not active.get(node.name):
            inclusive[node.name] = inclusive.get(node.name, 0) + node.count
        active[node.name] = active.get(node.name, 0) + 1
        work.append((node, True))
        work.extend((child, False) for child in reversed(node.children))
    return own, inclusive, images, waiting


def analyse_profile(text: str, *, all_threads: bool = False) -> dict:
    """Parse sample output into the profile record. By default only the thread with the most CPU samples (the
    emulation loop; CPU samples = root count minus the waiting leaves) is analysed, so that idle and waiting threads do
    not dilute the shares; all_threads adds every thread. A warning is recorded when more than one thread runs."""
    roots = parse_call_graph(text)
    trees = [analyse_tree(root) for root in roots]
    threads = [{"name": root.name, "samples": root.count, "waiting_samples": tree[3], "cpu_samples": root.count - tree[3]}
               for root, tree in zip(roots, trees)]
    if all_threads:
        chosen = list(range(len(roots)))
    else:
        chosen = [max(range(len(roots)), key=lambda index: threads[index]["cpu_samples"])]
    warnings: list[str] = []
    window = max(root.count for root in roots)
    busy = [thread["name"] for thread in threads if thread["cpu_samples"] * 100.0 >= CPU_THREAD_MIN_PERCENT * window]
    if len(busy) > 1 and not all_threads:
        warnings.append(f"{len(busy)} threads run on a CPU ({', '.join(busy)}) but only "
                        f"{threads[chosen[0]]['name']} is analysed; use --all-threads for all of them")
    own: dict[str, int] = {}
    inclusive: dict[str, int] = {}
    images: dict[str, str | None] = {}
    for index in chosen:
        tree_own, tree_inclusive, tree_images, _ = trees[index]
        for name, value in tree_own.items():
            own[name] = own.get(name, 0) + value
        for name, value in tree_inclusive.items():
            inclusive[name] = inclusive.get(name, 0) + value
        for name, image in tree_images.items():
            images.setdefault(name, image)
    total = sum(roots[index].count for index in chosen)
    if sum(own.values()) != total:
        raise ValueError(f"sum of self counts {sum(own.values())} != samples {total}")
    functions = [{"name": name, "image": images.get(name), "self": own.get(name, 0), "inclusive": inclusive.get(name, 0)}
                 for name in set(own) | set(inclusive)]
    functions.sort(key=lambda row: (-row["inclusive"], row["name"]))
    raw = [row for row in functions if row["name"].startswith(RAW_MARK)]
    raw_names = {"count": len(raw), "of": len(functions),
                 "percent": round(100.0 * len(raw) / len(functions), 2) if functions else 0.0,
                 "self_samples": sum(row["self"] for row in raw),
                 "self_percent": round(100.0 * sum(row["self"] for row in raw) / total, 2) if total else 0.0}
    return {"samples": total, "threads": threads, "analysed_threads": [roots[index].name for index in chosen],
            "self_sum": sum(own.values()), "warnings": warnings, "raw_names": raw_names, "functions": functions}


def top_table(profile: dict, key: str, limit: int = PROFILE_TOP) -> list[dict]:
    rows = sorted(profile["functions"], key=lambda row: (-row[key], row["name"]))[:limit]
    total = profile["samples"]
    return [{"rank": index + 1, "name": row["name"], "image": row["image"], "samples": row[key],
             "share_percent": round(100.0 * row[key] / total, 2) if total else 0.0}
            for index, row in enumerate(rows) if row[key] > 0]


def print_table(title: str, rows: list[dict], total: int) -> None:
    print(f"\n{title} (samples {total})")
    print(f"{'#':>3} {'share':>7} {'samples':>8}  function")
    for row in rows:
        print(f"{row['rank']:>3} {row['share_percent']:>6.2f}% {row['samples']:>8}  {row['name']}")


def print_profile(profile: dict, tables: dict) -> None:
    for thread in profile["threads"]:
        print(f"thread {thread['name']}: samples {thread['samples']}, waiting {thread['waiting_samples']}, "
              f"cpu {thread['cpu_samples']}")
    print(f"analysed: {profile['analysed_threads']}")
    for warning in profile["warnings"]:
        print(f"WARNING: {warning}", file=sys.stderr)
    print_table(f"Top {PROFILE_TOP} by self", tables["self"], profile["samples"])
    print_table(f"Top {PROFILE_TOP} by inclusive", tables["inclusive"], profile["samples"])
    raw = profile["raw_names"]
    print(f"\nraw (not shortened) names: {raw['count']} of {raw['of']} functions ({raw['percent']}%), "
          f"self samples {raw['self_samples']} ({raw['self_percent']}%)")
    print(f"self sum {profile['self_sum']} == samples {profile['samples']}")


def cmd_profile_from_sample(args: argparse.Namespace) -> int:
    """Analyse a saved sample file again: nothing is run, no lock, no JSON."""
    try:
        profile = analyse_profile(args.from_sample.read_text(errors="replace"), all_threads=args.all_threads)
    except (OSError, ValueError) as error:
        print(f"cannot parse {args.from_sample}: {error}", file=sys.stderr)
        return 1
    print_profile(profile, {"self": top_table(profile, "self"), "inclusive": top_table(profile, "inclusive")})
    return 0


def cmd_profile(args: argparse.Namespace) -> int:
    inputs = load_inputs()
    if not SAMPLE.exists():
        print(f"{SAMPLE} not found: profile needs macOS sample", file=sys.stderr)
        return 1
    clean_run_dir()
    sample_file = WORK / f"{args.label}-profile.sample.txt"
    out_path = WORK / f"{args.label}-profile.json"
    sample_file.unlink(missing_ok=True)
    out_path.unlink(missing_ok=True)
    stdout_file, stderr_file = RUN_DIR / "profile-runner.stdout", RUN_DIR / "profile-runner.stderr"
    argv = runner_argv(inputs, args.budget, snapshot=False, fast_path=args.fast_path,
                       data_cache=args.data_cache_prototype,
                       block_cache=args.block_cache_prototype)
    started = time.monotonic()
    sample_rc, sample_message, sampled_from, runner_alive_after = None, "", None, None
    with open(stdout_file, "wb") as out, open(stderr_file, "wb") as err:
        proc = subprocess.Popen(argv, cwd=ROOT, stdout=out, stderr=err)
        try:
            while proc.poll() is None and time.monotonic() - started < args.at:
                time.sleep(0.2)
            if proc.poll() is not None:
                print(f"runner ended after {time.monotonic() - started:.1f} s, before --at {args.at}: "
                      "no window to sample (lower --at or raise --budget)", file=sys.stderr)
            else:
                sampled_from = time.monotonic() - started
                print(f"sampling pid {proc.pid} for {args.seconds} s from {sampled_from:.1f} s", flush=True)
                done = subprocess.run([str(SAMPLE), str(proc.pid), str(args.seconds), "1", "-mayDie", "-file",
                                       str(sample_file)], capture_output=True, text=True)
                sample_rc, sample_message = done.returncode, (done.stderr + done.stdout).strip()
                runner_alive_after = proc.poll() is None
                print(f"sample exit {sample_rc}; runner {'still running' if runner_alive_after else 'ended'}; "
                      "waiting for the run to finish", flush=True)
            proc.wait()
        except BaseException:      # Ctrl-C, SIGTERM (main turns it into SystemExit) and errors: the runner must not outlive us
            proc.kill()
            proc.wait()
            raise
    wall = time.monotonic() - started
    if sample_rc != 0:
        print(f"sample failed (exit {sample_rc}), no JSON written" if sample_rc is not None
              else "no sample taken, no JSON written", file=sys.stderr)
        if sample_message:
            print(sample_message, file=sys.stderr)
        return 1
    if not sample_file.exists():
        print(f"no sample file {sample_file} (sample exit 0), no JSON written", file=sys.stderr)
        return 1
    runner_json = True
    try:
        report = json.loads(stdout_file.read_text())
        if not isinstance(report, dict):
            raise ValueError("not an object")
    except (OSError, ValueError):      # JSONDecodeError is a ValueError
        report, runner_json = {}, False
        print(f"WARNING: the runner printed no JSON ({stdout_file}): retired and the exit code are not recorded",
              file=sys.stderr)
    try:
        profile = analyse_profile(sample_file.read_text(errors="replace"), all_threads=args.all_threads)
    except ValueError as error:
        print(f"cannot parse {sample_file}: {error}", file=sys.stderr)
        return 1
    tables = {"self": top_table(profile, "self"), "inclusive": top_table(profile, "inclusive")}
    result = {
        "schema": "od-cpu-core-perf-profile-v1",
        "label": args.label, "budget": args.budget, "fast_path": bool(args.fast_path),
        "data_cache_prototype": bool(args.data_cache_prototype), "block_cache_prototype": bool(args.block_cache_prototype), "at_s": args.at, "seconds": args.seconds,
        "sampled_from_s": round(sampled_from, 3) if sampled_from is not None else None,
        "sample_exit_code": sample_rc, "runner_alive_after_sample": runner_alive_after,
        "runner_exit_code": proc.returncode, "runner_json": runner_json, "wall_s": round(wall, 3),
        "retired": report.get("retired"),
        "sample_file": str(sample_file),
        "samples": profile["samples"], "self_sum": profile["self_sum"],
        "threads": profile["threads"], "analysed_threads": profile["analysed_threads"],
        "warnings": profile["warnings"], "raw_names": profile["raw_names"],
        "top_self": tables["self"], "top_inclusive": tables["inclusive"], "functions": profile["functions"],
        "inputs": inputs["inputs"], "runner_sha256": sha256_file(runner_path()),
        "build_profile": inputs.get("build_profile"), "host": bench.host_fingerprint(), **git_state(),
    }
    out_path.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print_profile(profile, tables)
    if runner_alive_after is False:
        print("WARNING: the runner ended inside the sampling window: the window is shorter than --seconds",
              file=sys.stderr)
    print(f"runner exit {proc.returncode}, wall {wall:.1f} s, retired {report.get('retired')}; wrote {out_path}")
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
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))   # the runner of profile is then killed, not orphaned
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
    record.add_argument("--fast-path", action="store_true", help="pass --fast-path to the runner (unified loop only)")
    record.add_argument("--data-cache-prototype", action="store_true",
                        help="pass --data-cache-prototype to the runner (PERF-11, unified loop only)")
    record.add_argument("--block-cache-prototype", action="store_true",
        help="pass --block-cache-prototype to the runner (PERF-31, unified loop, needs --fast-path)")
    record.set_defaults(func=cmd_record)
    compare = sub.add_parser("compare", help="compare two records")
    compare.add_argument("a", type=Path)
    compare.add_argument("b", type=Path)
    compare.add_argument("--allow-fast-path-diff", action="store_true",
                         help="records may differ in the fast_path flag (fast path vs base); everything else is compared")
    compare.set_defaults(func=cmd_compare)
    measure = sub.add_parser("measure", help="timed sequential runs (priority -5 expected)")
    measure.add_argument("--budget", type=int, required=True)
    measure.add_argument("--runs", type=int, required=True)
    measure.add_argument("--label", required=True)
    measure.add_argument("--fast-path", action="store_true", help="pass --fast-path to the runner (unified loop only)")
    measure.add_argument("--data-cache-prototype", action="store_true",
                         help="pass --data-cache-prototype to the runner (PERF-11, unified loop only)")
    measure.add_argument("--block-cache-prototype", action="store_true",
        help="pass --block-cache-prototype to the runner (PERF-31, unified loop, needs --fast-path)")
    measure.set_defaults(func=cmd_measure)
    profile = sub.add_parser("profile", help="one run under /usr/bin/sample; self/inclusive tables (K6)")
    profile.add_argument("--budget", type=int, help="instruction budget (required unless --from-sample)")
    profile.add_argument("--label", help="output label (required unless --from-sample)")
    profile.add_argument("--at", type=float, default=30.0, help="seconds after the start of the run (default 30)")
    profile.add_argument("--seconds", type=int, default=20, help="length of the sampling window (default 20)")
    profile.add_argument("--all-threads", action="store_true",
                         help="count every thread (default: only the thread with the most CPU samples)")
    profile.add_argument("--from-sample", type=Path, metavar="FILE",
                         help="analyse a saved sample file again: no run, no lock, no JSON")
    profile.add_argument("--fast-path", action="store_true", help="pass --fast-path to the runner (unified loop only)")
    profile.add_argument("--data-cache-prototype", action="store_true",
                         help="pass --data-cache-prototype to the runner (PERF-11, unified loop only)")
    profile.add_argument("--block-cache-prototype", action="store_true",
        help="pass --block-cache-prototype to the runner (PERF-31, unified loop, needs --fast-path)")
    profile.set_defaults(func=cmd_profile)
    args = parser.parse_args()
    if args.command == "profile" and args.from_sample is not None:
        return cmd_profile_from_sample(args)
    if args.command == "profile" and (args.budget is None or args.label is None):
        parser.error("profile needs --budget and --label (or --from-sample FILE)")
    if args.command in ("record", "measure", "profile") and (
            args.budget <= 0 or (args.command == "measure" and args.runs < 1)):
        parser.error("budget and runs must be positive")
    if args.command == "profile" and (args.at < 0 or args.seconds < 1):
        parser.error("--at must be >= 0 and --seconds >= 1")
    if args.command in ("record", "measure", "profile") and not acquire_run_lock():
        print("another record, measure or profile is running (lock: work/cpu-core-perf/.run.lock)", file=sys.stderr)
        return 1
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
