#!/usr/bin/env python3
"""Package the stage-4 ACPI table blob (S4-31d).

The blob is NOT serialized here. `firmware/uefi` builds the freestanding binary `od-acpi-blob`
for `x86_64-unknown-none`; the compiler evaluates the `const fn` serializers (build_dsdt,
build_mcfg, the blob header) into the `.acpi_blob` section. This script only

  1. runs `cargo build` for that binary (or takes `--elf`),
  2. cuts the `.acpi_blob` section out of the ELF by its section header,
  3. reads the header (version, manifest hash, load ranges of the payload and of the FACS
     segment, RSDP address, table directory) and checks that it is consistent with the section,
     that every table and both RSDP checksums add up to zero and that the cross-references
     (RSDP -> XSDT -> tables, FADT -> DSDT/FACS) hit real tables,
  4. writes the raw blob and a metadata JSON.

Format: firmware/uefi/src/acpi/blob.rs. The machine-side validator (od-machine, `acpi_blob.rs`)
is the authority on whether a blob fits a board manifest; the checks here are a sanity net for
the extraction, not a second serializer or a second validator of the manifest.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import struct
import subprocess
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
FIRMWARE_DIR = ROOT / "firmware/uefi"
DEFAULT_OUT = ROOT / "work/stage4/acpi_blob"
TARGET = "x86_64-unknown-none"
BIN_NAME = "od-acpi-blob"
SECTION = ".acpi_blob"
OUT_NAME = "stage4_acpi_blob"

MAGIC = b"ODACPIB1"
VERSION = 2
HEADER_LEN = 0x60
DIR_ENTRY_LEN = 16
MAX_TABLES = 8
RSDP_LEN = 36
FACS_LEN = 64
ACPI_REGION_BASE = 0xE0000
FACS_ADDRESS = 0xEF000
ACPI_REGION_END = 0xF0000


class BuildError(Exception):
    pass


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def load_build_firmware():
    """Reuse the toolchain lookup of build_firmware.py (clang, cargo, rustc)."""
    import importlib.util

    path = ROOT / "tools/build/build_firmware.py"
    spec = importlib.util.spec_from_file_location("od_build_firmware", path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def build_elf(
    target_dir: Path, *, linux_nvme: bool = False, handoff: Path | None = None
) -> Path:
    bf = load_build_firmware()
    try:
        tools = bf.resolve_tools()
        env = dict(tools.tool_env)
        # RUSTFLAGS would replace firmware/uefi/.cargo/config.toml (relocation-model=static).
        env.pop("RUSTFLAGS", None)
        env.pop("CARGO_ENCODED_RUSTFLAGS", None)
        env["CARGO_TARGET_DIR"] = str(target_dir)
        env["OD_FW_CLANG"] = str(tools.clang)
        # cwd inside firmware/uefi: cargo reads .cargo/config.toml relative to the cwd.
        command = [str(tools.cargo), "build", "--release", "--target", TARGET, "--offline", "--bin", BIN_NAME]
        if linux_nvme:
            command.extend(["--features", "linux-nvme-acpi"])
        if handoff is not None:
            command.extend(["--features", "handoff-acpi"])
            env["OD_FW_ACPI_HANDOFF"] = str(handoff.resolve())
        result = subprocess.run(
            command,
            cwd=FIRMWARE_DIR,
            env=env,
            check=False,
            capture_output=True,
            text=True,
        )
    except bf.BuildError as exc:
        raise BuildError(str(exc)) from exc
    if result.returncode != 0:
        raise BuildError(f"cargo build failed ({result.returncode}):\n{(result.stderr or result.stdout).strip()}")
    elf = target_dir / TARGET / "release" / BIN_NAME
    if not elf.is_file():
        raise BuildError(f"cargo did not produce {elf}")
    return elf


def extract_section(elf: bytes, wanted: str = SECTION) -> bytes:
    """Bytes of the named section of an ELF64 little-endian x86-64 object, by section header."""
    if len(elf) < 0x40 or elf[:4] != b"\x7fELF":
        raise BuildError("not an ELF file")
    if elf[4] != 2 or elf[5] != 1:
        raise BuildError("expected ELF64 little-endian")
    (e_machine,) = struct.unpack_from("<H", elf, 0x12)
    if e_machine != 0x3E:
        raise BuildError(f"expected EM_X86_64, got {e_machine:#x}")
    (e_shoff,) = struct.unpack_from("<Q", elf, 0x28)
    e_shentsize, e_shnum, e_shstrndx = struct.unpack_from("<HHH", elf, 0x3A)
    if e_shoff == 0 or e_shentsize != 64 or e_shnum == 0 or e_shstrndx >= e_shnum:
        raise BuildError("ELF has no usable section header table")
    if e_shoff + e_shnum * e_shentsize > len(elf):
        raise BuildError("ELF section header table is out of file")

    def header(index: int) -> tuple[int, int, int, int, int]:
        name, sh_type, _flags, _addr, offset, size = struct.unpack_from("<IIQQQQ", elf, e_shoff + index * 64)
        return name, sh_type, offset, size, _addr

    _, _, str_offset, str_size, _ = header(e_shstrndx)
    if str_offset + str_size > len(elf):
        raise BuildError("ELF section name table is out of file")
    strings = elf[str_offset : str_offset + str_size]
    found: list[tuple[int, int]] = []
    for index in range(e_shnum):
        name, sh_type, offset, size, _ = header(index)
        end = strings.find(b"\0", name)
        if end < 0:
            raise BuildError("unterminated section name")
        if strings[name:end].decode("ascii", "replace") == wanted:
            if sh_type == 8:  # SHT_NOBITS
                raise BuildError(f"section {wanted} has no file bytes")
            found.append((offset, size))
    if len(found) != 1:
        raise BuildError(f"expected exactly one {wanted} section, found {len(found)}")
    offset, size = found[0]
    if offset + size > len(elf):
        raise BuildError(f"section {wanted} is out of file")
    return elf[offset : offset + size]


def parse_blob(blob: bytes) -> dict[str, Any]:
    """Reads the header and directory and checks they fit the bytes. Nothing is rebuilt."""
    if len(blob) < HEADER_LEN:
        raise BuildError("blob is shorter than its header")
    if blob[:8] != MAGIC:
        raise BuildError("bad blob magic")
    version, header_len = struct.unpack_from("<II", blob, 0x08)
    if version != VERSION:
        raise BuildError(f"unsupported blob version {version}")
    if header_len != HEADER_LEN:
        raise BuildError(f"unexpected header length {header_len:#x}")
    manifest_hash = blob[0x10:0x30]
    (load_base,) = struct.unpack_from("<Q", blob, 0x30)
    payload_offset, payload_len = struct.unpack_from("<II", blob, 0x38)
    (rsdp_address,) = struct.unpack_from("<Q", blob, 0x40)
    table_count, total_len = struct.unpack_from("<II", blob, 0x48)
    facs_offset, facs_len = struct.unpack_from("<II", blob, 0x50)
    (facs_address,) = struct.unpack_from("<Q", blob, 0x58)
    if total_len != len(blob):
        raise BuildError(f"total_len {total_len} differs from the section size {len(blob)}")
    if table_count == 0 or table_count > MAX_TABLES:
        raise BuildError(f"table count {table_count} out of range")
    dir_end = HEADER_LEN + table_count * DIR_ENTRY_LEN
    if payload_offset < dir_end or payload_offset + payload_len != facs_offset:
        raise BuildError("the FACS segment does not follow the payload")
    if facs_len != FACS_LEN or facs_offset + facs_len != len(blob):
        raise BuildError("the FACS segment does not end the blob")
    if load_base != ACPI_REGION_BASE or facs_address != FACS_ADDRESS or rsdp_address != load_base:
        raise BuildError("load base, FACS address or RSDP address is not the contract's")
    if payload_len < RSDP_LEN or load_base + payload_len > facs_address:
        raise BuildError("ACPI payload lies outside its reclaim reservation")
    if facs_address + facs_len > ACPI_REGION_END:
        raise BuildError("FACS lies outside its NVS reservation")
    facs = blob[facs_offset : facs_offset + facs_len]
    if facs[:4] != b"FACS" or struct.unpack_from("<I", facs, 4)[0] != FACS_LEN:
        raise BuildError("bad FACS signature or length")
    payload = blob[payload_offset : payload_offset + payload_len]
    rsdp = payload[:RSDP_LEN]
    if (rsdp[:8] != b"RSD PTR " or rsdp[15] != 2
            or struct.unpack_from("<II", rsdp, 16) != (0, RSDP_LEN)
            or sum(rsdp[:20]) & 0xFF or sum(rsdp) & 0xFF):
        raise BuildError("RSDP signature, revision or checksum is wrong")
    tables = []
    by_signature: dict[bytes, dict[str, Any]] = {}
    ranges = []
    for index in range(table_count):
        at = HEADER_LEN + index * DIR_ENTRY_LEN
        signature = blob[at : at + 4]
        offset, length, reserved = struct.unpack_from("<III", blob, at + 4)
        if reserved:
            raise BuildError("directory reserved word is not zero")
        if signature in by_signature:
            raise BuildError(f"duplicate ACPI table {signature!r}")
        if offset < 0x40 or offset % 16 or length < 36 or offset + length > payload_len:
            raise BuildError(f"table {signature!r} lies outside the payload")
        end = offset + length
        if any(offset < previous_end and previous_start < end for previous_start, previous_end in ranges):
            raise BuildError(f"table {signature!r} overlaps another ACPI table")
        ranges.append((offset, end))
        table = payload[offset : offset + length]
        if table[:4] != signature:
            raise BuildError(f"table at payload offset {offset:#x} has another signature than its directory entry")
        (declared,) = struct.unpack_from("<I", table, 4)
        if declared != length:
            raise BuildError(f"table {signature!r}: Length field {declared} differs from the directory ({length})")
        if sum(table) & 0xFF:
            raise BuildError(f"table {signature!r}: bytes do not add to zero")
        entry = {
            "signature": signature.decode("ascii"),
            "payload_offset": offset,
            "length": length,
            "address": load_base + offset,
            "sha256": sha256(table),
        }
        tables.append(entry)
        by_signature[signature] = entry
    # RSDP -> XSDT -> FADT/MADT/MCFG; FADT -> DSDT and FACS.
    (xsdt_address,) = struct.unpack_from("<Q", rsdp, 24)
    if b"XSDT" not in by_signature or by_signature[b"XSDT"]["address"] != xsdt_address:
        raise BuildError("RSDP XsdtAddress is not the XSDT of the blob")
    xsdt = payload[by_signature[b"XSDT"]["payload_offset"] :][: by_signature[b"XSDT"]["length"]]
    if (len(xsdt) - 36) % 8:
        raise BuildError("XSDT length cuts off an address")
    listed = [struct.unpack_from("<Q", xsdt, 36 + 8 * i)[0] for i in range((len(xsdt) - 36) // 8)]
    wanted = sorted(t["address"] for t in tables if t["signature"] in ("FACP", "APIC", "MCFG"))
    if sorted(listed) != wanted:
        raise BuildError("XSDT does not list exactly FADT, MADT and MCFG")
    if not {b"FACP", b"DSDT", b"APIC"} <= by_signature.keys():
        raise BuildError("FADT, DSDT or MADT is missing")
    fadt = payload[by_signature[b"FACP"]["payload_offset"] :][: by_signature[b"FACP"]["length"]]
    dsdt_address = by_signature[b"DSDT"]["address"]
    if (len(fadt) < 148
            or struct.unpack_from("<II", fadt, 36) != (facs_address, dsdt_address)
            or struct.unpack_from("<QQ", fadt, 132) != (facs_address, dsdt_address)):
        raise BuildError("FADT FACS or DSDT addresses do not match the blob")
    return {
        "version": version,
        "manifest_hash": manifest_hash.hex(),
        "load_range": {"start": load_base, "end": load_base + payload_len},
        "payload_offset": payload_offset,
        "payload_len": payload_len,
        "rsdp_address": rsdp_address,
        "tables": tables,
        "facs": {
            "blob_offset": facs_offset,
            "length": facs_len,
            "address": facs_address,
            "load_range": {"start": facs_address, "end": facs_address + facs_len},
            "sha256": sha256(facs),
        },
        "xsdt_entries": listed,
    }


def package(elf_path: Path, out_dir: Path) -> dict[str, Any]:
    blob = extract_section(elf_path.read_bytes())
    metadata = parse_blob(blob)
    metadata["blob_bytes"] = len(blob)
    metadata["blob_sha256"] = sha256(blob)
    metadata["section"] = SECTION
    metadata["source"] = f"firmware/uefi:{BIN_NAME} ({TARGET})"
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / f"{OUT_NAME}.bin").write_bytes(blob)
    (out_dir / f"{OUT_NAME}.json").write_text(json.dumps(metadata, indent=2) + "\n")
    return metadata


def compare_guest_memory(blob: bytes, guest: bytes, metadata: dict[str, Any]) -> None:
    """Compare both reservations and the unused zero-filled space between them."""
    expected = bytearray(ACPI_REGION_END - ACPI_REGION_BASE)
    payload_offset = metadata["payload_offset"]
    payload_len = metadata["payload_len"]
    expected[:payload_len] = blob[payload_offset : payload_offset + payload_len]
    facs = metadata["facs"]
    offset = facs["address"] - ACPI_REGION_BASE
    start = facs["blob_offset"]
    expected[offset : offset + facs["length"]] = blob[start : start + facs["length"]]
    if guest != expected:
        if len(guest) != len(expected):
            raise BuildError(f"guest ACPI range has {len(guest)} bytes, expected {len(expected)}")
        mismatch = next(index for index, pair in enumerate(zip(guest, expected)) if pair[0] != pair[1])
        raise BuildError(f"guest ACPI differs at physical address {ACPI_REGION_BASE + mismatch:#x}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir")
    parser.add_argument("--target-dir", help="cargo target directory (default: <output-dir>/cargo-target)")
    parser.add_argument("--elf", help="package this ELF instead of building one")
    parser.add_argument("--compare", help="a blob file the extracted bytes must equal (the committed fixture)")
    parser.add_argument("--linux-nvme", action="store_true", help="build for the direct Linux PCI NVMe board")
    args = parser.parse_args()
    if args.elf and args.linux_nvme:
        parser.error("--linux-nvme selects a build and cannot be combined with --elf")
    default_out = DEFAULT_OUT.with_name("linux_nvme_acpi_blob") if args.linux_nvme else DEFAULT_OUT
    out_dir = Path(args.output_dir) if args.output_dir else default_out
    if not out_dir.is_absolute():
        out_dir = ROOT / out_dir
    try:
        if args.elf:
            elf = Path(args.elf)
        else:
            target = Path(args.target_dir) if args.target_dir else out_dir / "cargo-target"
            elf = build_elf(target if target.is_absolute() else ROOT / target, linux_nvme=args.linux_nvme)
        metadata = package(elf, out_dir)
        if args.compare:
            expected = Path(args.compare).read_bytes()
            if (out_dir / f"{OUT_NAME}.bin").read_bytes() != expected:
                raise BuildError(f"extracted blob differs from {args.compare}")
    except BuildError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    print(json.dumps({k: metadata[k] for k in ("version", "manifest_hash", "load_range", "rsdp_address", "blob_bytes", "blob_sha256")}, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
