#!/usr/bin/env python3
"""Build and measure the native stage 7 FV and ESP firmware paths on macOS."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import statistics
import struct
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/build"))

import build_firmware as firmware
import make_test_disk as disk

MIB = 1024 * 1024
PE_BYTES = MIB
READ_BYTES = 64 * MIB
DATA_OFFSET = 40 * MIB
DEFAULT_BUDGET = 1_200_000_000
MAX_JSON = 100 * 1024
SERVICES = ("empty", "read_blocks_64_mib", "load_image_1_mib", "hash_log_extend_1_mib")
METRICS = ("host_ns", "retired", "quanta")
MARKERS = ("empty complete", "ReadBlocks verified", "LoadImage verified", "HashLogExtendEvent verified")
BENCHMARK_SOURCES = (
    "crates/od-machine/src/firmware_board.rs",
    "crates/od-machine/src/paged_probe.rs",
    "firmware/uefi/tests/apps/Cargo.toml",
    "firmware/uefi/tests/apps/bench/main.rs",
    "tools/bench/firmware_boot_bench.py",
    "tools/build/build_firmware.py",
    "tools/runner/src/firmware_boot.rs",
)


def command(argv, *, cwd=ROOT, env=None):
    return subprocess.check_output(argv, cwd=cwd, env=env, text=True).strip()


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def payload_pe():
    """A PE32+ with 1 MiB on disk and a mapped, initialized read-only section."""
    data_bytes = PE_BYTES - 0x600
    section_align = 0x1000
    image_base = 0x1_4000_0000
    image_size = (0x3000 + data_bytes + section_align - 1) & -section_align
    pe = bytearray(PE_BYTES)
    pe[:2] = b"MZ"
    header = 0x80
    struct.pack_into("<I", pe, 0x3C, header)
    pe[header:header + 4] = b"PE\0\0"
    optional_bytes = 112 + 16 * 8
    struct.pack_into("<HHIIIHH", pe, header + 4, 0x8664, 3, 0, 0, 0, optional_bytes, 0x0022)
    opt = header + 24
    struct.pack_into("<H", pe, opt, 0x20B)
    struct.pack_into("<IIIII", pe, opt + 4, 0x200, 0x200 + data_bytes, 0, 0x1000, 0x1000)
    struct.pack_into("<QII", pe, opt + 24, image_base, section_align, 0x200)
    struct.pack_into("<II", pe, opt + 56, image_size, 0x200)
    struct.pack_into("<HH", pe, opt + 68, 10, 0x40)
    struct.pack_into("<I", pe, opt + 108, 16)
    struct.pack_into("<II", pe, opt + 112 + 5 * 8, 0x2000, 12)
    rows = (
        (b".text", 0x200, 0x1000, 0x200, 0x200, 0x6000_0020),
        (b".reloc", 12, 0x2000, 0x200, 0x400, 0x4200_0040),
        (b".rdata", data_bytes, 0x3000, data_bytes, 0x600, 0x4000_0040),
    )
    for index, (name, vsize, rva, size, offset, flags) in enumerate(rows):
        struct.pack_into("<8sIIIIIIHHI", pe, opt + optional_bytes + index * 40,
                         name, vsize, rva, size, offset, 0, 0, 0, 0, flags)
    pe[0x200:0x203] = b"\x31\xc0\xc3"  # xor eax,eax; ret (not started)
    struct.pack_into("<Q", pe, 0x218, image_base + 0x1000)
    struct.pack_into("<IIHH", pe, 0x400, 0x1000, 12, 0xA018, 0)
    pe[0x600:] = b"\xa5" * data_bytes
    firmware.inspect_pe(pe)
    return bytes(pe)


def write_disk(path, app, payload):
    """Keep the production-sized FAT32 ESP and place the read area beyond it."""
    start = DATA_OFFSET // disk.BLOCK
    end = (DATA_OFFSET + READ_BYTES) // disk.BLOCK
    if start < disk.FAT32_BLOCKS:
        raise ValueError("raw read area overlaps ESP")
    blocks = end + disk.ARRAY_BLOCKS + 1
    regions = disk.gpt_regions(blocks, disk.FAT32_PARTITIONS, "none")
    esp = disk.fat32_blocks(disk.FAT32_START, disk.FAT32_END - disk.FAT32_START + 1,
                            ((b"BOOTX64 EFI", app),))
    with path.open("wb") as file:
        file.truncate(blocks * disk.BLOCK)
        for stream in (regions, esp):
            for lba, contents in stream:
                file.seek(lba * disk.BLOCK)
                file.write(contents)
        file.seek(DATA_OFFSET)
        file.write(payload)
        file.seek(DATA_OFFSET + READ_BYTES - 1)
        file.write(b"\xa6")


def build_fixtures(work):
    tools = firmware.resolve_tools()
    pe_tools = firmware.resolve_probe_tools()
    payload = payload_pe()
    digest = hashlib.sha256(payload).digest()
    (work / "digests.bin").write_bytes(digest + hashlib.sha256(bytes(32) + digest).digest())
    env = dict(pe_tools.tool_env)
    env.pop("RUSTFLAGS", None)
    env.pop("CARGO_ENCODED_RUSTFLAGS", None)
    env["OD_FW_BENCH_DIR"] = str(work)
    target_dir = work / "app-target"
    subprocess.run([pe_tools.cargo, "build", "--release", "--locked", "--offline",
                    "--target", firmware.PROBE_TARGET, "--target-dir", target_dir,
                    "--features", "firmware-bench", "--bin", "firmware-bench"],
                   cwd=firmware.FIRMWARE_DIR / "tests/apps", env=env, check=True)
    app = (target_dir / firmware.PROBE_TARGET / "release/firmware-bench.efi").read_bytes()
    app_info = firmware.inspect_pe(app)
    target_dir = work / "firmware-target"
    runtime = firmware.build_runtime_driver(pe_tools, target_dir / "runtime")
    archive = firmware.write_fv_archive(target_dir, firmware.pack_fv({"hello": app, "runtime": runtime}))
    image, image_info = firmware.build_image(tools, archive.parent, work / "firmware.bin", fv_archive=archive)
    write_disk(work / "disk.raw", app, payload)
    return {
        "sha256": {"firmware": sha256(image), "efi_app": sha256(app), "load_image_payload": digest.hex(),
                   "disk": sha256((work / "disk.raw").read_bytes())},
        "firmware_rip": firmware.LINK_BASE + image_info["entry_offset"],
        "entry_rva": app_info["entry_rva"],
        "tool_versions": firmware.tool_versions(tools),
    }


def boot(runner, work, budget, route, suffix, targets=()):
    com1 = work / f"{route}-{suffix}.com1"
    argv = [runner, "boot-firmware", "--image", work / "firmware.bin", "--budget", str(budget),
            "--com1-out", com1, "--nvme-image", work / "disk.raw", "--tpm",
            "--nvram-out", work / f"{route}.nvram"]
    if route == "FV":
        argv.extend(("--fv-app", "hello"))
    for address in targets:
        argv.extend(("--sample-rip", hex(address)))
    proc = subprocess.run(argv, cwd=ROOT, text=True, capture_output=True)
    trace = work / f"{route}-{suffix}.json"
    trace.write_text(proc.stdout, encoding="utf-8")
    # Both apps return to firmware. Match the distinct, documented end of
    # each route and require the app's success markers as well.
    detail = 0 if route == "FV" else 1
    expected_stop = {"code": "0x21", "detail": f"0x{detail:016x}"}
    if proc.returncode != 6:
        raise ValueError(f"{route} {suffix}: runner exit {proc.returncode}, see {trace} and {com1} {proc.stderr}")
    result = json.loads(proc.stdout)
    transcript = com1.read_bytes()
    expected = [b"route " + route.encode(), b"verified"]
    if route == "FV":
        expected[1:1] = [m.encode() for m in MARKERS]
        if transcript.count(b"OD-FW image hello: EFI_SUCCESS\r\n") != 1:
            raise ValueError(f"{route} {suffix}: EFI image did not return success in {com1}")
    if any(transcript.count(b"OD-S7-27 " + line + b"\r\n") != 1 for line in expected):
        raise ValueError(f"{route} {suffix}: missing or duplicate EFI success markers in {com1}")
    if (
        (result["status"], result["fw_stop"]) != ("firmware_stop", expected_stop)
        or result["exit"] != "halted"
        or any(result[key] is not None for key in ("page_fault", "memory_error"))
    ):
        raise ValueError(f"{route} {suffix}: unexpected board result in {trace}")
    addresses = []
    for label in ("base", "entry", "begin", "end"):
        matches = re.findall(rb"OD-S7-27 " + label.encode() + rb" ([0-9a-f]{16})\r\n", transcript)
        if len(matches) != 1:
            raise ValueError(f"{route} {suffix}: missing or duplicate {label} address in {com1}")
        addresses.append(int(matches[0], 16))
    return tuple(addresses), result


def run_series(runner, work, budget, route, fixtures, repeats):
    addresses, _ = boot(runner, work, budget, route, "discover")
    base, entry, begin, end = addresses
    if base <= 0 or base % 4096 or entry != base + fixtures["entry_rva"] or end != begin + 2 or begin <= base:
        raise ValueError(f"{route}: inconsistent PE entry or call addresses {addresses}")
    targets = [fixtures["firmware_rip"], entry]
    windows = [f"handoff_to_{route.lower()}_entry"]
    if route == "FV":
        targets.extend([begin, end] * len(SERVICES))
        windows.extend(SERVICES)
    measurements = {name: [] for name in windows}
    fingerprint = None
    for index in range(repeats + 1):
        found, report = boot(runner, work, budget, route, str(index), targets)
        samples = report["rip_samples"]
        if found != addresses or [int(sample["rip"], 16) for sample in samples] != targets:
            raise ValueError(f"{route} repeat {index}: incomplete or changed RIP sequence")
        if tuple(samples[0][name] for name in ("retired", "quanta")) != (0, 0):
            raise ValueError(f"{route} repeat {index}: firmware entry was not the first instruction")
        current = {key: report[key] for key in ("budget", "retired", "quanta", "handoff_sha256", "tsc_hz")}
        if fingerprint is None:
            fingerprint = current
        if current != fingerprint or report["image_sha256"] != fixtures["sha256"]["firmware"]:
            raise ValueError(f"{route} repeat {index}: changed machine configuration or execution counts")
        for name, start, end in zip(windows, samples[::2], samples[1::2], strict=True):
            delta = {metric: end[metric] - start[metric] for metric in METRICS}
            if delta["host_ns"] <= 0 or delta["retired"] <= 0 or delta["quanta"] < 0:
                raise ValueError(f"{route} repeat {index}: invalid {name} interval {delta}")
            measurements[name].append(delta)
        print(f"{route} {index}/{repeats}: {current['retired']} retired, {current['quanta']} quanta", flush=True)
    return measurements, {"rip_sequence": [hex(value) for value in targets], **fingerprint}


def summarize(raw):
    def summarize_metric(metric):
        values = sorted(row[metric] for row in raw[1:])
        return {
            "mean": round(statistics.fmean(values), 2),
            "p50": statistics.median(values),
            "p95": values[math.ceil(0.95 * len(values)) - 1],
            "min": values[0],
            "max": values[-1],
            "spread": values[-1] - values[0],
        }

    return {"discarded_warmup": raw[0], "raw": raw[1:],
            "summary": {metric: summarize_metric(metric) for metric in METRICS}}


def host_fingerprint():
    hardware = json.loads(command(["system_profiler", "SPHardwareDataType", "-json", "-detailLevel", "mini"]))
    model = hardware["SPHardwareDataType"][0]
    return {
        "model": model["machine_model"],
        "cpu": model["chip_type"],
        "memory": model["physical_memory"],
        "architecture": command(["uname", "-m"]),
        "macos_version": command(["sw_vers", "-productVersion"]),
        "macos_build": command(["sw_vers", "-buildVersion"]),
        "python": sys.version.split()[0],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--budget", type=int, default=DEFAULT_BUDGET)
    parser.add_argument("--repeats", type=int, default=10, help="valid runs after one warmup, at least 10")
    args = parser.parse_args()
    if args.repeats < 10 or args.budget <= 0:
        parser.error("need at least 10 valid repeats and a positive budget")
    work = args.work_dir.resolve()
    work.mkdir(parents=True, exist_ok=True)
    host = host_fingerprint()
    build = {
        "profile": "release",
        "engine": "paged firmware interpreter",
        "git_head": command(["git", "rev-parse", "HEAD"]),
        "source_diff_sha256": sha256(subprocess.check_output(["git", "diff", "--binary", "HEAD"], cwd=ROOT)),
        "benchmark_sources_sha256": {path: sha256((ROOT / path).read_bytes()) for path in BENCHMARK_SOURCES},
    }
    runner_target = work / "runner-target"
    subprocess.run(["cargo", "build", "-p", "od-runner", "--release", "--locked", "--offline",
                    "--target-dir", runner_target], cwd=ROOT, check=True)
    runner = runner_target / "release/od-runner"
    fixtures = build_fixtures(work)
    build.update(runner_sha256=sha256(runner.read_bytes()), asset_sha256=fixtures["sha256"],
                 tools=fixtures["tool_versions"])
    case = {}
    series = {}
    for route in ("FV", "ESP"):
        raw, case[route] = run_series(runner, work, args.budget, route, fixtures, args.repeats)
        series.update({name: summarize(samples) for name, samples in raw.items()})
    result = {
        "schema_version": 1,
        "host": host,
        "build": build,
        "board": {"ram_bytes": 128 * MIB, "vcpu_count": 1, "nvme": "fixed controller, 512-byte sectors, in-memory backend",
                  "nvram": "fresh legacy bank", "tpm": "fresh CRB, active SHA-256",
                  "quantum_limit": "remaining global instruction budget, ended early by device events",
                  "read_offset": DATA_OFFSET, "read_bytes": READ_BYTES, "pe_bytes": PE_BYTES, "hash_bytes": PE_BYTES},
        "case": case,
        "repeat_count": args.repeats,
        "series": series,
    }
    encoded = (json.dumps(result, indent=2) + "\n").encode()
    if len(encoded) > MAX_JSON:
        raise ValueError(f"benchmark JSON is {len(encoded)} bytes, limit is {MAX_JSON}")
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_bytes(encoded)
    print(f"Wrote {len(encoded)} bytes to {args.out}")


if __name__ == "__main__":
    main()
