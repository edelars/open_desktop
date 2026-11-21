#!/usr/bin/env python3
"""S7-31a: record where the stock firmware stops on the Ubuntu 7.0.0-38 EFI stub.

Diagnostic, not an acceptance test: it exits 0 when every variant produced two identical
stop records, whatever the stop is. The vmlinuz and initramfs are inputs from work/stage4 and
are never copied into Git. Variants (the first is the unmodified artifact):

  stock        vmlinuz as shipped (PE ImageBase 0, no .reloc)
  rebased      PE ImageBase patched to 0x1000000 so a non-relocatable image fits
  rebased-hdr  rebased, plus SizeOfHeaders patched from 0x1000 to 0x400

Patched variants are diagnostic copies; their signature is void and they prove nothing about
the stock image. Bytes near RIP come from the firmware ROM or from the image file, not from
guest memory (the runner has no memory dump).
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tests" / "system"))
import firmware_esp_boot_smoke as esp_smoke  # noqa: E402  (reuses the NVRAM and load-option encoders)

DISK = ROOT / "tools" / "build" / "make_test_disk.py"
BUILDER = ROOT / "tools" / "build" / "build_firmware.py"
LINK_BASE = 0x200000
NONCE = "od7s31adiagnostic0001"
CMDLINE = "console=ttyS0 initrd=\\EFI\\BOOT\\INITRD.IMG od.nonce=" + NONCE
REBASED_IMAGE_BASE = 0x1000000
VARIANTS = ("stock", "rebased", "rebased-hdr")
# LoadImage plans the placement (plan_image) and then maps the PE (load_into). Two images are loaded
# before the kernel stub runs, and the stock kernel stops at the second plan_image. Visits are matched
# in order, so a run that fails in plan_image never reaches the matching load_into. The firmware calls
# plan_image once more after the second load_into; that third call is not explained (S7-31a report).
PLAN_IMAGE = "<od_firmware::mem::pages::PageAllocator>::plan_image"
LOAD_INTO = "od_firmware::pe::loader::load_into"
EXIT_BOOT_SERVICES = "od_firmware::bs::exit::exit_boot_services"
LOADS = 2
WINDOW_BEFORE, WINDOW_AFTER = 16, 32


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def pe_layout(image: bytes) -> dict:
    pe = struct.unpack_from("<I", image, 0x3C)[0]
    section_count = struct.unpack_from("<H", image, pe + 6)[0]
    optional_size = struct.unpack_from("<H", image, pe + 20)[0]
    optional = pe + 24
    first = optional + optional_size
    sections = [struct.unpack_from("<IIII", image, first + 40 * i + 8) for i in range(section_count)]
    return {
        "pe": pe,
        "optional": optional,
        "entry_rva": struct.unpack_from("<I", image, optional + 16)[0],
        "image_base": struct.unpack_from("<Q", image, optional + 24)[0],
        "image_size": struct.unpack_from("<I", image, optional + 56)[0],
        "headers_size": struct.unpack_from("<I", image, optional + 60)[0],
        "sections": sections,  # (virtual_size, virtual_address, raw_size, raw_offset)
    }


def make_variant(kernel: Path, variant: str, out: Path) -> Path:
    data = bytearray(kernel.read_bytes())
    layout = pe_layout(data)
    if variant != "stock":
        struct.pack_into("<Q", data, layout["optional"] + 24, REBASED_IMAGE_BASE)
    if variant == "rebased-hdr":
        struct.pack_into("<I", data, layout["optional"] + 60, 0x400)
    out.write_bytes(data)
    return out


def symbols(elf: Path) -> list[tuple[int, str]]:
    sysroot = subprocess.run(["rustc", "--print", "sysroot"], capture_output=True, text=True, check=True).stdout.strip()
    nm = next(Path(sysroot).glob("lib/rustlib/*/bin/llvm-nm"))
    text = subprocess.run([nm, "-C", elf], capture_output=True, text=True, check=True).stdout
    result = []
    for line in text.splitlines():
        parts = line.split(" ", 2)
        if len(parts) == 3 and parts[1] in "Tt":
            result.append((int(parts[0], 16), parts[2]))
    return sorted(result)


def symbol_at(table: list[tuple[int, str]], address: int) -> str | None:
    best = None
    for start, name in table:
        if start <= address:
            best = (start, name)
        else:
            break
    return None if best is None else f"{best[1]}+0x{address - best[0]:x}"


def window(rip: int, rom: bytes, kernel: bytes, layout: dict, base: int | None) -> dict:
    """Return the bytes around RIP from the ROM image or from the loaded PE file."""
    if LINK_BASE <= rip < LINK_BASE + len(rom):
        source, blob, offset = "firmware-rom", rom, rip - LINK_BASE
    elif base is not None and base <= rip < base + layout["image_size"]:
        rva = rip - base
        for _, va, raw_size, raw_offset in layout["sections"]:
            if va <= rva < va + raw_size:
                source, blob, offset = "kernel-pe-file", kernel, raw_offset + rva - va
                break
        else:
            return {"source": "kernel-image-bss-or-header", "rva": hex(rip - base)}
    else:
        return {"source": "unmapped-to-a-known-image"}
    start = max(0, offset - WINDOW_BEFORE)
    return {
        "source": source,
        "before": blob[start:offset].hex(),
        "at_rip": blob[offset:offset + WINDOW_AFTER].hex(),
    }


def boot(runner: Path, rom: Path, esp: Path, nvram: Path, scratch: Path, name: str,
         budget: int, samples: list[int], cpu_loop: str = "firmware",
         mark_rip: int | None = None) -> tuple[dict, bytes]:
    trace = scratch / f"{name}.com1"
    args = [runner, "boot-firmware", "--image", rom, "--nvme-image", esp, "--budget", str(budget),
            "--com1-out", trace, "--nvram-in", nvram, "--nvram-out", scratch / f"{name}-after.nv"]
    if cpu_loop != "firmware":
        args += ["--cpu-loop", cpu_loop]
    if mark_rip is not None:  # unified loop only: the first boundary at this RIP is reported
        args += ["--mark-rip", f"0x{mark_rip:x}"]
    for address in samples:
        args += ["--sample-rip", f"0x{address:x}"]
    result = subprocess.run(args, capture_output=True, text=True, check=False, cwd=ROOT)
    report = json.loads(result.stdout)
    report["runner_exit_code"] = result.returncode
    return report, trace.read_bytes()


def last_line(com1: bytes) -> str:
    text = re.sub(rb"\x1b\[[0-9;?]*[A-Za-z]", b"", com1).replace(b"\r", b"")
    lines = [line for line in text.split(b"\n") if line.strip()]
    return lines[-1].decode("ascii", "replace") if lines else ""


def stop_record(variant: str, report: dict, com1: bytes, rom: bytes, kernel: bytes, layout: dict,
                base: int | None, table: list[tuple[int, str]], sampled: list[str]) -> dict:
    rip = int(report["rip"], 16)
    visited = [sample["rip"] for sample in report.get("rip_samples", [])]
    return {
        "variant": variant,
        "status": report["status"],
        "exit": report["exit"],
        "exit_detail": report["exit_detail"],
        "runner_exit_code": report["runner_exit_code"],
        "activity": report["activity"],
        "rip": report["rip"],
        "rip_symbol": symbol_at(table, rip) if LINK_BASE <= rip < LINK_BASE + len(rom) else None,
        "bytes": window(rip, rom, kernel, layout, base),
        "retired": report["retired"],
        "quanta": report["quanta"],
        "page_fault": report["page_fault"],
        "memory_error": report["memory_error"],
        "stop_reason": report.get("stop_reason"),
        "fw_stop": report["fw_stop"],
        "com1_bytes": report["com1_bytes"],
        "com1_sha256": hashlib.sha256(com1).hexdigest(),
        "com1_last_line": last_line(com1),
        "milestones_requested": sampled,
        "milestones_visited_in_order": sampled[:len(visited)],
        "nvram_snapshot_sha256": report["nvram_snapshot_sha256"],
        "cpu_loop": report.get("cpu_loop", "firmware"),
        # S7-31c: the unified loop reads the bytes at RIP from guest memory (a read-only walk).
        "unified": report.get("unified"),
        # S7-31d: whether the ROM's exit_boot_services was entered (the marked RIP was reached);
        # None where the loop has no marker (the firmware profile loop samples RIPs instead).
        "exit_boot_services_entered": (
            bool(report["unified"].get("marker")) if report.get("unified") else None
        ),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--stage4-dir", type=Path, default=ROOT / "work" / "stage4",
                        help="directory with linux-7.0.0-38.38/ and initramfs/")
    parser.add_argument("--work-dir", type=Path, default=None, help="scratch directory (default: temporary)")
    parser.add_argument("--out", type=Path, required=True, help="JSON record to write")
    parser.add_argument("--runs", type=int, default=2)
    parser.add_argument("--budget", type=int, default=2_000_000_000)
    parser.add_argument("--variants", default=",".join(VARIANTS))
    parser.add_argument("--cpu-loop", choices=("firmware", "unified"), default="firmware",
                        help="firmware: the firmware profile loop (S7-31a record); unified: the ROM and the guest in the "
                             "CPL0 profile set loop (S7-31c, ADR-0013 variant (b)); no RIP milestones there")
    args = parser.parse_args()
    variants = args.variants.split(",")
    if any(name not in VARIANTS for name in variants) or args.runs < 1:
        parser.error(f"variants must come from {VARIANTS}")
    kernel_path = args.stage4_dir / "linux-7.0.0-38.38" / "vmlinuz-7.0.0-38-generic"
    initrd_path = args.stage4_dir / "initramfs" / "initramfs.cpio"
    target = ROOT / Path(os.environ.get("CARGO_TARGET_DIR", "target"))
    runner = target / "release" / "od-runner"
    subprocess.run(["cargo", "build", "--release", "-p", "od-runner", "--offline"], check=True, cwd=ROOT)
    with tempfile.TemporaryDirectory(prefix="od-linux-uefi-") as temporary:
        scratch = args.work_dir or Path(temporary)
        scratch.mkdir(parents=True, exist_ok=True)
        rom = scratch / "stock.bin"
        subprocess.run([sys.executable, BUILDER, "--out", rom, "--target-dir", scratch / "rom"],
                       check=True, cwd=ROOT, stdout=subprocess.DEVNULL)
        elf = next((scratch / "rom").glob("fv/*/x86_64-unknown-none/release/od-firmware"))
        table = symbols(elf)
        named = {name: start for start, name in table}
        rom_bytes = rom.read_bytes()
        nvram = scratch / "seed.nv"
        option = esp_smoke.load_option(esp_smoke.file_node("BOOTX64.EFI"), (CMDLINE + "\0").encode("utf-16le"))
        nvram.write_bytes(esp_smoke.encode_snapshot([("Boot0001", option), ("BootOrder", struct.pack("<H", 1))]))
        record = {
            "schema": "od-s7-31a-stop-record-v1" if args.cpu_loop == "firmware" else "od-s7-31f-stop-record-v1",
            "inputs": {
                "vmlinuz_sha256": sha256(kernel_path),
                "initramfs_sha256": sha256(initrd_path),
                "firmware_rom_sha256": sha256(rom),
                "nvram_seed_sha256": sha256(nvram),
                "cmdline": CMDLINE,
                "boot_option": "Boot0001 = \\EFI\\BOOT\\BOOTX64.EFI + OptionalData (UTF-16LE cmdline), BootOrder = [1]",
                "nonce_channel": "kernel command line od.nonce= (S5-07 path); no trailer in the archive",
            },
            "variants": {},
        }
        all_equal = True
        for variant in variants:
            patched = make_variant(kernel_path, variant, scratch / f"vmlinuz-{variant}.efi")
            layout = pe_layout(patched.read_bytes())
            esp = scratch / f"esp-{variant}.raw"
            subprocess.run([sys.executable, DISK, "--esp-fat32", "--bootx64", patched,
                            "--boot-file", f"INITRD.IMG={initrd_path}", "--out", esp], check=True, cwd=ROOT)
            base = layout["image_base"] if variant != "stock" else None
            samples, labels = [], []
            if args.cpu_loop == "firmware":  # the unified loop has no per-instruction RIP observer
                for number in range(1, LOADS + 1):
                    samples += [named[PLAN_IMAGE], named[LOAD_INTO]]
                    labels += [f"plan_image #{number}", f"load_into #{number}"]
                if base is not None:
                    samples.append(base + layout["entry_rva"])
                    labels.append("kernel EFI stub entry (efi_pe_entry, first instruction)")
                samples.append(named[EXIT_BOOT_SERVICES])
                labels.append("ExitBootServices")
            records = []
            for run in range(args.runs):
                report, com1 = boot(runner, rom, esp, nvram, scratch, f"{variant}-{run}", args.budget, samples,
                                   args.cpu_loop,
                                   named[EXIT_BOOT_SERVICES] if args.cpu_loop == "unified" else None)
                records.append(stop_record(variant, report, com1, rom_bytes, patched.read_bytes(), layout,
                                           base, table, labels))
                print(f"{variant} run {run}: {records[-1]['exit']} {records[-1]['exit_detail']} "
                      f"rip={records[-1]['rip']} retired={records[-1]['retired']}", flush=True)
            identical = all(item == records[0] for item in records[1:])
            all_equal &= identical
            record["variants"][variant] = {
                "variant_file_sha256": sha256(patched),
                "esp_sha256": sha256(esp),
                "pe": {"image_base": hex(layout["image_base"]), "image_size": hex(layout["image_size"]),
                       "entry_rva": hex(layout["entry_rva"]), "headers_size": hex(layout["headers_size"])},
                "runs": records,
                "records_identical": identical,
            }
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps(record, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        print(f"records identical across runs: {all_equal}; wrote {args.out}")
        if not all_equal:
            raise SystemExit(2)


if __name__ == "__main__":
    try:
        main()
    except (KeyError, OSError, StopIteration, ValueError, subprocess.CalledProcessError) as error:
        print(f"stop record: {error!r}", file=sys.stderr)
        raise SystemExit(1) from error
