#!/usr/bin/env python3
"""Build and validate the Stage 4 minimal initramfs with a real /init (S4-36).

The archive is a cpio "newc" (magic 070701) stream holding exactly one regular
file, /init (mode 0100755), followed by the TRAILER!!! entry. /init is a static
freestanding x86-64 ELF (tests/fixtures/guest/stage4/initramfs/init.S). The run
nonce is never baked into the archive: /init reads "od.nonce=<value>" from
/proc/cmdline, and the host runner passes the value on the kernel command line.

The builder repacks deterministically (mtime 0, uid/gid 0, fixed inode numbers),
validates structure, executable ABI, host-path/secret absence and the kernel
config, proves a repeatable build, runs its own negative variants and writes a
timestamp-free evidence JSON. Nothing here boots Linux.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import secrets
import struct
import sys
import tempfile
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
from build_stage4_flat_probe import (  # noqa: E402
    BuildError,
    resolve_executable,
    run,
    rust_toolchain,
    sha256,
)

REPO_ROOT = Path(__file__).resolve().parents[2]
FIXTURE_DIR = REPO_ROOT / "tests/fixtures/guest/stage4/initramfs"
WORK_ROOT = REPO_ROOT / "work/stage4"
DEFAULT_OUTPUT_DIR = WORK_ROOT / "initramfs"
EVIDENCE_DIR = REPO_ROOT / "docs/reports/data/stage4"
DEFAULT_EVIDENCE = EVIDENCE_DIR / "s4-36-initramfs.json"
S435_EVIDENCE = REPO_ROOT / "docs/reports/data/stage4/s4-35-linux-artifact.json"
DEFAULT_CONFIG = REPO_ROOT / "work/vms/s2-00-ubuntu-26.04.1/source/extracted/config"

CPIO_MAGIC = b"070701"
CPIO_HEADER_LEN = 110
CPIO_TRAILER = "TRAILER!!!"
INIT_NAME = "init"
INIT_MODE = 0o100755
S_IFMT = 0o170000
S_IFREG = 0o100000

NONCE_RE = re.compile(r"[A-Za-z0-9]{16,128}")
NONCE_MIN = 16
NONCE_MAX = 128
BAKED_NONCE_RE = re.compile(rb"(?:od\.nonce=|OD_NONCE=)[A-Za-z0-9]{16,}")

ET_EXEC = 2
EM_X86_64 = 62
PT_LOAD, PT_DYNAMIC, PT_INTERP = 1, 2, 3
PF_X, PF_W = 1, 2

# Syscall numbers from arch/x86/entry/syscalls/syscall_64.tbl of
# linux-source-7.0.0 7.0.0-38.38.
ALLOWED_SYSCALLS = {
    0: "read",
    1: "write",
    3: "close",
    35: "nanosleep",
    39: "getpid",
    60: "exit",
    83: "mkdir",
    165: "mount",
    257: "openat",
}
HOST_PATH_MARKERS = (b"/Users", b"/home", b"/private", b"/opt", b"/var/folders", b"/Volumes", b"/nix")
SECRET_MARKERS = (b"PRIVATE KEY", b"password", b"passwd", b"secret", b"token", b"AKIA", b"ghp_", b"BEGIN ")

REQUIRED_CONFIG = {
    "CONFIG_BLK_DEV_INITRD": "y",
    "CONFIG_X86_64": "y",
    "CONFIG_BINFMT_ELF": "y",
    "CONFIG_PROC_FS": "y",
    "CONFIG_TMPFS": "y",
}
INFORMATIONAL_CONFIG = ("CONFIG_RD_GZIP", "CONFIG_DEVTMPFS", "CONFIG_INITRAMFS_SOURCE")


# ---------------------------------------------------------------------------
# nonce
# ---------------------------------------------------------------------------


def validate_nonce(value: str) -> str:
    if not isinstance(value, str) or value == "":
        raise BuildError("nonce is empty")
    if len(value) < NONCE_MIN:
        raise BuildError(f"nonce is too short: {len(value)} characters, need at least {NONCE_MIN}")
    if len(value) > NONCE_MAX:
        raise BuildError(f"nonce is too long: {len(value)} characters, at most {NONCE_MAX}")
    if not NONCE_RE.fullmatch(value):
        raise BuildError("nonce must consist of [A-Za-z0-9] only (no spaces or other characters)")
    return value


# ---------------------------------------------------------------------------
# cpio newc
# ---------------------------------------------------------------------------


def _align4(length: int) -> int:
    return (length + 3) & ~3


def _cpio_record(name: str, mode: int, data: bytes, ino: int, nlink: int) -> bytes:
    encoded = name.encode("ascii") + b"\0"
    fields = [
        ino,
        mode,
        0,  # uid
        0,  # gid
        nlink,
        0,  # mtime
        len(data),
        0,  # devmajor
        0,  # devminor
        0,  # rdevmajor
        0,  # rdevminor
        len(encoded),
        0,  # check
    ]
    header = CPIO_MAGIC + b"".join(f"{value:08X}".encode("ascii") for value in fields)
    record = header + encoded
    record += b"\0" * (_align4(len(record)) - len(record))
    record += data
    record += b"\0" * (_align4(len(record)) - len(record))
    return record


def pack_cpio(entries: list[tuple[str, int, bytes]]) -> bytes:
    """Deterministic newc archive: fixed order, ino = 1..n, mtime/uid/gid 0."""
    out = b""
    for index, (name, mode, data) in enumerate(entries, start=1):
        nlink = 2 if (mode & S_IFMT) == 0o040000 else 1
        out += _cpio_record(name, mode, data, index, nlink)
    return out + _cpio_record(CPIO_TRAILER, 0, b"", 0, 1)


def parse_cpio(archive: bytes) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    offset = 0
    seen: set[str] = set()
    while True:
        if offset + CPIO_HEADER_LEN > len(archive):
            raise BuildError("cpio archive is truncated: no TRAILER!!! entry")
        header = archive[offset : offset + CPIO_HEADER_LEN]
        if header[:6] != CPIO_MAGIC:
            raise BuildError(f"cpio entry at offset {offset} has bad magic {header[:6]!r}, expected 070701")
        try:
            values = [int(header[6 + 8 * i : 14 + 8 * i], 16) for i in range(13)]
        except ValueError as exc:
            raise BuildError(f"cpio entry at offset {offset} has a non-hex header field") from exc
        ino, mode, uid, gid, nlink, mtime, filesize, _dmaj, _dmin, _rmaj, _rmin, namesize, _check = values
        name_start = offset + CPIO_HEADER_LEN
        name_end = name_start + namesize
        if name_end > len(archive):
            raise BuildError(f"cpio archive is truncated inside the entry at offset {offset}")
        if namesize < 1 or archive[name_end - 1] != 0:
            raise BuildError(f"cpio entry at offset {offset} has a bad name field")
        name = archive[name_start : name_end - 1].decode("ascii", errors="replace")
        data_start = _align4(name_end)
        data_end = data_start + filesize
        if data_end > len(archive):
            raise BuildError(f"cpio entry {name!r} data extends past the end of the archive")
        data = archive[data_start:data_end]
        offset = _align4(data_end)
        if name == CPIO_TRAILER:
            if archive[offset:].strip(b"\0"):
                raise BuildError("cpio archive has non-zero data after TRAILER!!!")
            return entries
        if name in seen:
            raise BuildError(f"cpio archive has duplicate entry {name!r}")
        seen.add(name)
        entries.append(
            {
                "name": name,
                "ino": ino,
                "mode": mode,
                "uid": uid,
                "gid": gid,
                "nlink": nlink,
                "mtime": mtime,
                "data": data,
            }
        )


# ---------------------------------------------------------------------------
# ELF /init
# ---------------------------------------------------------------------------


def check_elf(elf: bytes) -> dict[str, Any]:
    """Validate the static x86-64 executable ABI of /init; raise BuildError if not."""
    if len(elf) < 64 or elf[:4] != b"\x7fELF":
        raise BuildError("/init is not an ELF file")
    if elf[4] != 2:
        raise BuildError("/init is not ELF64 (EI_CLASS)")
    if elf[5] != 1:
        raise BuildError("/init is not little-endian (EI_DATA)")
    e_type, e_machine = struct.unpack_from("<HH", elf, 16)
    if e_machine != EM_X86_64:
        raise BuildError(f"/init has e_machine {e_machine}, expected {EM_X86_64} (x86-64)")
    if e_type != ET_EXEC:
        raise BuildError(f"/init has e_type {e_type}, expected {ET_EXEC} (ET_EXEC, static non-PIE)")
    entry = struct.unpack_from("<Q", elf, 24)[0]
    phoff = struct.unpack_from("<Q", elf, 32)[0]
    phentsize, phnum = struct.unpack_from("<HH", elf, 54)
    if phentsize != 56 or phnum == 0 or phoff + phentsize * phnum > len(elf):
        raise BuildError("/init has a malformed program header table")
    segments = []
    for index in range(phnum):
        p_type, flags, p_offset, vaddr, _paddr, filesz, memsz, align = struct.unpack_from(
            "<IIQQQQQQ", elf, phoff + index * phentsize
        )
        if p_type == PT_INTERP:
            raise BuildError("/init has a PT_INTERP segment (dynamic loader); it must be static")
        if p_type == PT_DYNAMIC:
            raise BuildError("/init has a PT_DYNAMIC segment; it must be static")
        if p_type == PT_LOAD:
            if p_offset + filesz > len(elf):
                raise BuildError("/init PT_LOAD extends past the end of the file")
            if (flags & PF_X) and (flags & PF_W):
                raise BuildError("/init has a writable and executable PT_LOAD segment")
            segments.append(
                {"flags": flags, "offset": p_offset, "vaddr": vaddr, "filesz": filesz, "memsz": memsz, "align": align}
            )
    code_segments = [s for s in segments if s["flags"] & PF_X]
    if not code_segments:
        raise BuildError("/init has no executable PT_LOAD segment")
    if not any(s["vaddr"] <= entry < s["vaddr"] + s["filesz"] for s in code_segments):
        raise BuildError(f"/init entry 0x{entry:x} is not inside an executable PT_LOAD segment")

    for marker in HOST_PATH_MARKERS:
        if marker in elf:
            raise BuildError(f"/init contains a host path string ({marker.decode()})")
    for marker in SECRET_MARKERS:
        if marker in elf:
            raise BuildError(f"/init contains a secret-like string ({marker.decode()})")

    syscalls_used: dict[int, int] = {}
    for segment in code_segments:
        code = elf[segment["offset"] : segment["offset"] + segment["filesz"]]
        if b"\xcd\x80" in code or b"\x0f\x34" in code:
            raise BuildError("/init uses int 0x80 or sysenter; only the 64-bit syscall instruction is allowed")
        position = code.find(b"\x0f\x05")
        while position != -1:
            if position < 5 or code[position - 5] != 0xB8:
                raise BuildError(f"syscall at code offset 0x{position:x} is not preceded by mov imm32, %eax")
            number = int.from_bytes(code[position - 4 : position], "little")
            if number not in ALLOWED_SYSCALLS:
                raise BuildError(f"/init uses syscall number {number}, not in the allowed set")
            syscalls_used[number] = syscalls_used.get(number, 0) + 1
            position = code.find(b"\x0f\x05", position + 2)
    if not syscalls_used:
        raise BuildError("/init contains no syscall instruction")
    return {
        "class": "ELF64",
        "data": "little-endian",
        "e_type": "ET_EXEC",
        "e_machine": "EM_X86_64",
        "entry": f"0x{entry:x}",
        "has_pt_interp": False,
        "has_pt_dynamic": False,
        "load_segments": [
            {
                "vaddr": f"0x{s['vaddr']:x}",
                "filesz": s["filesz"],
                "memsz": s["memsz"],
                "flags": ("R" if s["flags"] & 4 else "-") + ("W" if s["flags"] & PF_W else "-") + ("X" if s["flags"] & PF_X else "-"),
            }
            for s in segments
        ],
        "host_path_markers_checked": [m.decode() for m in HOST_PATH_MARKERS],
        "secret_markers_checked": [m.decode() for m in SECRET_MARKERS],
        "syscalls_used": {ALLOWED_SYSCALLS[n]: {"number": n, "sites": c} for n, c in sorted(syscalls_used.items())},
        "size_bytes": len(elf),
        "sha256": sha256(elf),
    }


# ---------------------------------------------------------------------------
# archive verification
# ---------------------------------------------------------------------------


def verify_archive(archive: bytes, nonce: str | None = None) -> dict[str, Any]:
    """Check content, mode, ABI of /init and the absence of a baked nonce."""
    entries = parse_cpio(archive)
    by_name = {entry["name"]: entry for entry in entries}
    if INIT_NAME not in by_name:
        raise BuildError("archive has no /init entry")
    extra = [name for name in by_name if name != INIT_NAME]
    if extra:
        raise BuildError(f"archive has unexpected entries besides /init: {extra}")
    init = by_name[INIT_NAME]
    if (init["mode"] & S_IFMT) != S_IFREG:
        raise BuildError(f"/init is not a regular file (mode {init['mode']:o})")
    if init["mode"] & 0o111 == 0:
        raise BuildError(f"/init is not executable (mode {init['mode']:o})")
    if init["mode"] != INIT_MODE:
        raise BuildError(f"/init has mode {init['mode']:o}, expected {INIT_MODE:o}")
    if (init["uid"], init["gid"], init["mtime"], init["nlink"]) != (0, 0, 0, 1):
        raise BuildError("/init must have uid 0, gid 0, mtime 0, nlink 1")
    baked = BAKED_NONCE_RE.search(archive)
    if baked:
        raise BuildError("archive contains a baked nonce-like value after od.nonce=/OD_NONCE=")
    if nonce is not None:
        validate_nonce(nonce)
        if nonce.encode("ascii") in archive:
            raise BuildError("the nonce is baked into the archive")
    elf_report = check_elf(init["data"])
    return {
        "entries": [
            {
                "name": f"/{entry['name']}",
                "mode": f"0{entry['mode']:o}",
                "uid": entry["uid"],
                "gid": entry["gid"],
                "mtime": entry["mtime"],
                "nlink": entry["nlink"],
                "ino": entry["ino"],
                "size_bytes": len(entry["data"]),
                "sha256": sha256(entry["data"]),
            }
            for entry in entries
        ],
        "trailer": CPIO_TRAILER,
        "elf": elf_report,
    }


# ---------------------------------------------------------------------------
# kernel config / S4-35 consistency
# ---------------------------------------------------------------------------


def parse_config(text: str) -> dict[str, str]:
    values: dict[str, str] = {}
    for line in text.splitlines():
        match = re.fullmatch(r"(CONFIG_[A-Za-z0-9_]+)=(.*)", line.strip())
        if match:
            values[match.group(1)] = match.group(2)
        else:
            match = re.fullmatch(r"# (CONFIG_[A-Za-z0-9_]+) is not set", line.strip())
            if match:
                values[match.group(1)] = "n"
    return values


def check_config(config_path: Path) -> dict[str, Any]:
    if not config_path.is_file():
        raise BuildError(f"kernel config is missing: {config_path}")
    raw = config_path.read_bytes()
    values = parse_config(raw.decode("utf-8", errors="replace"))
    for key, wanted in REQUIRED_CONFIG.items():
        if values.get(key) != wanted:
            raise BuildError(f"kernel config {key} is {values.get(key, 'absent')!r}, expected {wanted!r}")
    if values.get("CONFIG_INITRAMFS_FORCE", "n") != "n":
        raise BuildError("kernel config sets CONFIG_INITRAMFS_FORCE: an external initrd would be ignored")
    return {
        "sha256": sha256(raw),
        "required": {key: values[key] for key in REQUIRED_CONFIG},
        "initramfs_force": "not set",
        "informational": {key: values.get(key, "absent") for key in INFORMATIONAL_CONFIG},
    }


def kernel_identity(config_path: Path) -> dict[str, Any]:
    if not S435_EVIDENCE.is_file():
        raise BuildError(f"S4-35 evidence is missing: {S435_EVIDENCE}")
    evidence = json.loads(S435_EVIDENCE.read_text())
    config_entries = [f for f in evidence["files"] if f["role"] == "config"]
    if len(config_entries) != 1:
        raise BuildError("S4-35 evidence must list exactly one config file")
    config_sha = sha256(config_path.read_bytes())
    if config_entries[0]["sha256"] != config_sha:
        raise BuildError("kernel config hash differs from the S4-35 evidence")
    vmlinuz = [f for f in evidence["files"] if f["name"].startswith("vmlinuz-")]
    return {
        "kernel_source_package_version": evidence["kernel_source_package_version"],
        "architecture": evidence["architecture"],
        "s4_35_evidence": str(S435_EVIDENCE.relative_to(REPO_ROOT)),
        "vmlinuz_sha256": vmlinuz[0]["sha256"] if vmlinuz else None,
        "vmlinuz_sha256_source": "s4-35 manifest, not verified in S4-36 (no kernel is used here)",
        "config_sha256_matches_s4_35": True,
    }


# ---------------------------------------------------------------------------
# compile /init
# ---------------------------------------------------------------------------


def compile_init(build_dir: Path) -> tuple[bytes, dict[str, str]]:
    clang = resolve_executable(os.environ.get("STAGE4_CLANG"), "clang")
    _rustc, _host, _rustc_version, ld_lld, _objcopy, tool_env = rust_toolchain()
    env = tool_env.copy()
    env.update({"LC_ALL": "C", "SOURCE_DATE_EPOCH": "0"})
    versions = {
        "clang": run([str(clang), "--version"], env=env).splitlines()[0],
        "ld.lld": run([str(ld_lld), "--version"], env=env).splitlines()[0],
    }
    obj = build_dir / "init.o"
    elf = build_dir / "init"
    run(
        [
            str(clang),
            "--target=x86_64-unknown-none-elf",
            "-x",
            "assembler-with-cpp",
            "-ffreestanding",
            "-fno-pic",
            "-fno-pie",
            "-c",
            "init.S",
            "-o",
            str(obj),
        ],
        env=env,
        cwd=FIXTURE_DIR,
    )
    run(
        [
            str(ld_lld),
            "-m",
            "elf_x86_64",
            "-static",
            "--build-id=none",
            "--strip-all",
            "-z",
            "max-page-size=4096",
            "-e",
            "_start",
            "-T",
            "layout.ld",
            "-o",
            str(elf),
            str(obj),
        ],
        env=env,
        cwd=FIXTURE_DIR,
    )
    return elf.read_bytes(), versions


# ---------------------------------------------------------------------------
# negative variants (run by the builder itself; no kernel needed)
# ---------------------------------------------------------------------------


def _expect_reject(name: str, action, expected_fragment: str) -> dict[str, Any]:
    """Run a negative variant; it must raise BuildError whose message contains expected_fragment."""
    try:
        action()
    except BuildError as exc:
        message = str(exc).splitlines()[0]
        if expected_fragment not in message:
            raise BuildError(
                f"negative variant {name!r} was rejected for another reason: "
                f"expected {expected_fragment!r} in {message!r}"
            ) from exc
        return {"name": name, "rejected": True, "expected_fragment": expected_fragment, "message": message}
    raise BuildError(f"negative variant {name!r} was accepted")


def _patch(data: bytes, offset: int, value: bytes) -> bytes:
    return data[:offset] + value + data[offset + len(value) :]


def run_negatives(init_elf: bytes) -> list[dict[str, Any]]:
    def archive(entries: list[tuple[str, int, bytes]]) -> bytes:
        return pack_cpio(entries)

    results = [
        _expect_reject("missing_init", lambda: verify_archive(archive([])), "no /init entry"),
        _expect_reject(
            "non_executable_init",
            lambda: verify_archive(archive([(INIT_NAME, 0o100644, init_elf)])),
            "not executable",
        ),
        _expect_reject(
            "extra_entry",
            lambda: verify_archive(archive([(INIT_NAME, INIT_MODE, init_elf), ("etc", 0o040755, b"")])),
            "unexpected entries",
        ),
    ]
    for label, bad, fragment in (
        ("empty", "", "nonce is empty"),
        ("too_short", "abc123", "too short"),
        ("with_space", "abcd efgh ijkl mnop", "[A-Za-z0-9]"),
        ("non_alnum", "abcdefgh-ijklmnop!", "[A-Za-z0-9]"),
    ):
        results.append(_expect_reject(f"invalid_nonce_{label}", lambda bad=bad: validate_nonce(bad), fragment))
    probe = secrets.token_hex(16)
    results.append(
        _expect_reject(
            "nonce_baked_into_archive",
            lambda: verify_archive(archive([(INIT_NAME, INIT_MODE, init_elf + probe.encode())]), probe),
            "the nonce is baked into the archive",
        )
    )
    results.append(
        _expect_reject(
            "baked_od_nonce_value",
            lambda: verify_archive(archive([(INIT_NAME, INIT_MODE, init_elf + b"od.nonce=0123456789abcdef")])),
            "baked nonce-like value",
        )
    )
    results.append(
        _expect_reject(
            "host_path_in_binary",
            lambda: verify_archive(archive([(INIT_NAME, INIT_MODE, init_elf + b"/Users/someone/build\0")])),
            "host path string",
        )
    )
    results.append(
        _expect_reject(
            "not_elf",
            lambda: verify_archive(archive([(INIT_NAME, INIT_MODE, b"#!/bin/sh\necho hello\n" * 8)])),
            "not an ELF",
        )
    )
    results.append(
        _expect_reject(
            "wrong_e_machine",
            lambda: verify_archive(archive([(INIT_NAME, INIT_MODE, _patch(init_elf, 18, struct.pack("<H", 183)))])),
            "e_machine 183",
        )
    )
    return results


# ---------------------------------------------------------------------------
# orchestration
# ---------------------------------------------------------------------------


def build_archive(init_elf: bytes) -> bytes:
    return pack_cpio([(INIT_NAME, INIT_MODE, init_elf)])


def run_build(
    out_dir: Path,
    *,
    nonce: str | None = None,
    init_binary: Path | None = None,
    config_path: Path = DEFAULT_CONFIG,
    evidence_path: Path | None = None,
    check_kernel_identity: bool = True,
) -> dict[str, Any]:
    if nonce is not None:
        validate_nonce(nonce)
    config_report = check_config(config_path)
    kernel = kernel_identity(config_path) if check_kernel_identity else None
    versions: dict[str, str] = {}
    if init_binary is not None:
        elf_a = elf_b = init_binary.read_bytes()
        build_kind = "prebuilt /init supplied with --init-binary (one input; no independent recompilation)"
        build_runs = 1
    else:
        with tempfile.TemporaryDirectory(prefix="stage4-initramfs-a-") as dir_a, tempfile.TemporaryDirectory(
            prefix="stage4-initramfs-b-"
        ) as dir_b:
            elf_a, versions = compile_init(Path(dir_a))
            elf_b, _ = compile_init(Path(dir_b))
        build_kind = "compiled from tests/fixtures/guest/stage4/initramfs (two independent builds)"
        build_runs = 2
    cpio_a = build_archive(elf_a)
    cpio_b = build_archive(elf_b)
    if cpio_a != cpio_b or sha256(cpio_a) != sha256(cpio_b):
        raise BuildError("build is not repeatable: two runs produced different archives")
    probe = nonce if nonce is not None else secrets.token_hex(16)
    report = verify_archive(cpio_a, probe)
    negatives = run_negatives(elf_a)

    out_dir.mkdir(parents=True, exist_ok=True)
    cpio_path = out_dir / "initramfs.cpio"
    elf_path = out_dir / "init.elf"
    cpio_path.write_bytes(cpio_a)
    elf_path.write_bytes(elf_a)
    try:
        shown = str(cpio_path.resolve().relative_to(REPO_ROOT))
    except ValueError:
        shown = cpio_path.name

    evidence = {
        "slice": "S4-36",
        "status": "passed",
        "scope": "static build and structure checks; Linux was not booted",
        "kernel": kernel,
        "kernel_config": config_report,
        "init_source": {
            "path": "tests/fixtures/guest/stage4/initramfs/init.S",
            "sha256": sha256((FIXTURE_DIR / "init.S").read_bytes()),
            "linker_script": "tests/fixtures/guest/stage4/initramfs/layout.ld",
            "linker_script_sha256": sha256((FIXTURE_DIR / "layout.ld").read_bytes()),
            "build": build_kind,
            "tools": versions,
        },
        "cpio": {
            "path": shown,
            "format": "newc 070701, 4-byte aligned, no trailing padding, uncompressed",
            "size_bytes": len(cpio_a),
            "sha256": sha256(cpio_a),
            "entries": report["entries"],
            "trailer": report["trailer"],
        },
        "init_elf": report["elf"],
        "checks": {
            "archive_contains_only_init_and_trailer": True,
            "init_mode_0100755": True,
            "executable_abi": "ok",
            "no_host_paths_in_binary": True,
            "nonce_not_baked": True,
            "repeatable_build": {"runs": build_runs, "cpio_bytes_equal": True, "sha256_equal": True},
            "kernel_config_compatible": True,
        },
        "negatives": negatives,
        "limitations": [
            "Linux was not booted: execution of /init inside the kernel is not confirmed (S4-37/S4-38).",
            "The nonce reaching /init through the kernel command line and /proc/cmdline is not confirmed without a kernel.",
            "/init mounts proc itself (mkdir 83, mount 165): the kernel does not mount /proc in an initramfs.",
            "stdout depends on /dev/console, which the kernel built-in default cpio list creates (CONFIG_INITRAMFS_SOURCE is empty, CONFIG_INITRAMFS_FORCE is not set); the archive itself carries no /dev.",
            "Exit of PID 1 makes the kernel panic: that is the expected end of the run, not a fault of the archive.",
            "Syscall numbers come from the kernel source tree table, not from executing /init.",
            "The nonce parsing in /init is not covered by repository tests; it was only exercised outside the repository on a modified copy.",
            "/init cuts a nonce value longer than 128 characters to 128 and prints it with exit 0; the host compares the whole value and validate_nonce rejects longer ones.",
            "/init splits cmdline tokens by space (0x20) only; tabs and quotes are not handled (the host builds the command line).",
            "Failures of openat/read/mkdir/mount all end as OD_NONCE=MISSING with exit 1, without a separate code (deferred to S4-38).",
            "The kernel config is the one extracted from the Ubuntu buildinfo; provenance is not authenticated (see S4-35).",
        ],
    }
    if evidence_path is not None:
        evidence_path.parent.mkdir(parents=True, exist_ok=True)
        evidence_path.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n")
    return evidence


def output_directory(requested: str | None) -> Path:
    target = Path(requested).expanduser() if requested else DEFAULT_OUTPUT_DIR
    if not target.is_absolute():
        target = REPO_ROOT / target
    target = target.resolve()
    try:
        target.relative_to(WORK_ROOT.resolve())
    except ValueError as exc:
        raise BuildError(f"output directory must be inside ignored {WORK_ROOT}") from exc
    return target


def evidence_file(requested: str) -> Path:
    target = Path(requested).expanduser()
    if not target.is_absolute():
        target = REPO_ROOT / target
    target = target.resolve()
    allowed = EVIDENCE_DIR.resolve()
    try:
        target.relative_to(allowed)
    except ValueError as exc:
        raise BuildError(f"evidence path must be inside {allowed}") from exc
    if target == allowed:
        raise BuildError(f"evidence path must be a file inside {allowed}")
    return target


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--output-dir", help="inside work/stage4; default work/stage4/initramfs")
    parser.add_argument("--nonce", help="validate this nonce and prove it is not baked into the archive (never stored)")
    parser.add_argument("--init-binary", help="use a prebuilt /init instead of compiling the fixture")
    parser.add_argument("--config", default=str(DEFAULT_CONFIG), help="kernel config to check")
    parser.add_argument("--evidence", default=str(DEFAULT_EVIDENCE), help="evidence JSON path, inside docs/reports/data/stage4")
    parser.add_argument("--no-evidence", action="store_true", help="do not write the evidence JSON")
    parser.add_argument("--verify-archive", help="only verify an existing cpio archive and exit")
    args = parser.parse_args(argv)
    try:
        if args.verify_archive:
            report = verify_archive(Path(args.verify_archive).read_bytes(), args.nonce)
            print(f"archive ok: {len(report['entries'])} entry, sha256 {sha256(Path(args.verify_archive).read_bytes())}")
            return 0
        if args.nonce is not None:
            validate_nonce(args.nonce)
        out_dir = output_directory(args.output_dir)
        evidence_path = None if args.no_evidence else evidence_file(args.evidence)
        evidence = run_build(
            out_dir,
            nonce=args.nonce,
            init_binary=Path(args.init_binary) if args.init_binary else None,
            config_path=Path(args.config),
            evidence_path=evidence_path,
        )
    except BuildError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    print(f"status: {evidence['status']}")
    print(f"cpio: {evidence['cpio']['path']} {evidence['cpio']['size_bytes']} bytes sha256 {evidence['cpio']['sha256']}")
    print(f"/init: {evidence['init_elf']['size_bytes']} bytes sha256 {evidence['init_elf']['sha256']}")
    print(f"negatives rejected: {sum(1 for n in evidence['negatives'] if n['rejected'])}/{len(evidence['negatives'])}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
