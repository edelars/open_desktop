#!/usr/bin/env python3
"""Build and validate the freestanding Stage 4 x86-64 flat probe fixture."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shutil
import struct
import subprocess
import sys
from pathlib import Path
from typing import Any


IMAGE_BASE = 0x1000
IMAGE_SIZE = 0x9000
PAGE_SIZE = 0x1000
PAYLOAD_GPA = 0x9000
PAYLOAD_GVA = 0x800000
CODE_GPA = 0x7000
CODE_GVA = 0x400000
STACK_GPA = 0xA000
STACK_GVA = 0xA00000
NX = 1 << 63


class BuildError(Exception):
    pass


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def run(command: list[str], *, env: dict[str, str], cwd: Path | None = None) -> str:
    result = subprocess.run(
        command,
        cwd=cwd,
        env=env,
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise BuildError(f"command failed ({result.returncode}): {command!r}\n{detail}")
    return (result.stdout + result.stderr).strip()


def resolve_executable(value: str | None, name: str) -> Path:
    candidate = Path(value).expanduser() if value else None
    if candidate is None:
        found = shutil.which(name)
        if not found:
            raise BuildError(f"could not find {name!r} on PATH")
        candidate = Path(found)
    if not candidate.is_absolute():
        found = shutil.which(str(candidate))
        if not found:
            raise BuildError(f"{name} must resolve to an executable path: {candidate}")
        candidate = Path(found)
    candidate = candidate.resolve()
    if not candidate.is_file() or not os.access(candidate, os.X_OK):
        raise BuildError(f"{name} is not an executable file: {candidate}")
    return candidate


def rust_toolchain() -> tuple[Path, str, str, Path, Path, dict[str, str]]:
    rustc_value = os.environ.get("RUSTC") or shutil.which("rustc")
    if not rustc_value:
        raise BuildError("could not find rustc on PATH (or via RUSTC)")
    # Keep rustup's rustc shim path intact: resolving the symlink invokes the
    # rustup binary directly, whose CLI does not accept rustc arguments.
    rustc = Path(rustc_value).expanduser()
    if not rustc.is_absolute():
        rustc = Path(shutil.which(str(rustc)) or "").absolute()
    else:
        rustc = rustc.absolute()
    if not rustc.is_file() or not os.access(rustc, os.X_OK):
        raise BuildError(f"rustc is not an executable file: {rustc}")
    rustc_version = run([str(rustc), "-vV"], env=os.environ.copy())
    sysroot = Path(run([str(rustc), "--print", "sysroot"], env=os.environ.copy()).strip())
    host = next(
        (line.split(":", 1)[1].strip() for line in rustc_version.splitlines() if line.startswith("host:")),
        None,
    )
    if host is None:
        raise BuildError("rustc -vV did not report a host triple")

    ld_env = os.environ.get("STAGE4_LD_LLD")
    objcopy_env = os.environ.get("STAGE4_RUST_OBJCOPY")
    ld_candidates = [
        sysroot / "lib/rustlib" / host / "bin/gcc-ld/ld.lld",
        sysroot / "lib/rustlib" / host / "bin/ld.lld",
    ]
    objcopy_candidates = [sysroot / "lib/rustlib" / host / "bin/rust-objcopy"]

    def select_tool(explicit: str | None, candidates: list[Path], name: str) -> Path:
        if explicit:
            return resolve_executable(explicit, name)
        for path in candidates:
            if path.is_file() and os.access(path, os.X_OK):
                return path.resolve()
        raise BuildError(f"could not find {name} in the active Rust sysroot {sysroot}")

    ld_lld = select_tool(ld_env, ld_candidates, "ld.lld")
    objcopy = select_tool(objcopy_env, objcopy_candidates, "rust-objcopy")

    tool_env = os.environ.copy()
    if platform.system() == "Darwin":
        llvm_libraries = sorted((sysroot / "lib").glob("libLLVM*.dylib"))
        if not llvm_libraries:
            raise BuildError(f"could not discover libLLVM.dylib under {sysroot / 'lib'}")
        library_dir = str(llvm_libraries[0].parent.resolve())
        inherited = tool_env.get("DYLD_LIBRARY_PATH", "")
        search = [library_dir, *(part for part in inherited.split(os.pathsep) if part)]
        tool_env["DYLD_LIBRARY_PATH"] = os.pathsep.join(dict.fromkeys(search))
    return rustc, host, rustc_version, ld_lld, objcopy, tool_env


def parse_elf_load_segments(elf: bytes) -> tuple[int, list[dict[str, int]]]:
    if len(elf) < 64 or elf[:4] != b"\x7fELF" or elf[4] != 2 or elf[5] != 1:
        raise BuildError("linker output is not a little-endian ELF64 image")
    entry = struct.unpack_from("<Q", elf, 24)[0]
    phoff = struct.unpack_from("<Q", elf, 32)[0]
    phentsize, phnum = struct.unpack_from("<HH", elf, 54)
    if phentsize < 56 or phoff + phentsize * phnum > len(elf):
        raise BuildError("ELF program header table is malformed")
    segments: list[dict[str, int]] = []
    for index in range(phnum):
        offset = phoff + index * phentsize
        p_type, flags, file_offset, vaddr, paddr, filesz, memsz, align = struct.unpack_from(
            "<IIQQQQQQ", elf, offset
        )
        if p_type == 1:
            if file_offset + filesz > len(elf):
                raise BuildError("ELF PT_LOAD extends past the file")
            segments.append(
                {
                    "flags": flags,
                    "offset": file_offset,
                    "vaddr": vaddr,
                    "paddr": paddr,
                    "filesz": filesz,
                    "memsz": memsz,
                    "align": align,
                }
            )
    return entry, segments


def elf_symbol(elf: bytes, wanted: str) -> tuple[int, int]:
    shoff = struct.unpack_from("<Q", elf, 40)[0]
    shentsize, shnum = struct.unpack_from("<HH", elf, 58)
    if shentsize < 64 or shoff + shentsize * shnum > len(elf):
        raise BuildError("ELF section header table is malformed")
    for index in range(shnum):
        section_offset = shoff + index * shentsize
        section = struct.unpack_from("<IIQQQQIIQQ", elf, section_offset)
        section_type, section_offset_file, section_size, link, entry_size = (
            section[1], section[4], section[5], section[6], section[9]
        )
        if section_type != 2:
            continue
        if link >= shnum or entry_size < 24 or section_size % entry_size:
            raise BuildError("ELF symbol table metadata is malformed")
        strings_header = struct.unpack_from("<IIQQQQIIQQ", elf, shoff + link * shentsize)
        strings = elf[strings_header[4] : strings_header[4] + strings_header[5]]
        symbols = elf[section_offset_file : section_offset_file + section_size]
        for symbol_offset in range(0, len(symbols), entry_size):
            name_index, _, _, _, value, size = struct.unpack_from("<IBBHQQ", symbols, symbol_offset)
            if name_index >= len(strings):
                continue
            end = strings.find(b"\0", name_index)
            if end >= 0 and strings[name_index:end].decode("ascii", errors="replace") == wanted:
                return value, size
    raise BuildError(f"ELF symbol {wanted!r} is missing")


def validate_image(elf: bytes, raw: bytes, profile: str, payload: int, initial: int, store_value: int) -> dict[str, Any]:
    entry, segments = parse_elf_load_segments(elf)
    if entry != CODE_GVA:
        raise BuildError(f"ELF entry is 0x{entry:x}, expected 0x{CODE_GVA:x}")
    code_symbol, code_size = elf_symbol(elf, "_start")
    code_end, _ = elf_symbol(elf, "stage4_code_end")
    code = {
        "flat-probe": bytes.fromhex("488b03eef4"),
        "store-load-probe": bytes.fromhex("48890b488b03eef4"),
        "stack-probe": bytes.fromhex("5158eef4"),
    }[profile]
    data_size = 16 if profile == "store-load-probe" else 8
    if (code_symbol, code_size, code_end - code_symbol) != (CODE_GVA, len(code), len(code)):
        raise BuildError(
            "assembled entry must match the selected profile at its linked address; "
            f"got address=0x{code_symbol:x}, symbol_size={code_size}, end_delta={code_end - code_symbol}"
        )
    code_segment_size = 4 if profile == "stack-probe" else 8
    expected_segments = [
        (0x1000, 0x1000, 0x6000, 6),
        (CODE_GVA, CODE_GPA, code_segment_size, 5),
        (PAYLOAD_GVA, PAYLOAD_GPA, data_size, 6),
    ]
    if len(segments) != len(expected_segments):
        raise BuildError(f"expected 3 PT_LOAD segments, found {len(segments)}")
    for segment, (vaddr, paddr, size, flags) in zip(segments, expected_segments, strict=True):
        actual = (segment["vaddr"], segment["paddr"], segment["filesz"], segment["flags"])
        if actual != (vaddr, paddr, size, flags) or segment["memsz"] != size:
            raise BuildError(
                "unexpected PT_LOAD layout: "
                f"got {actual} memsz={segment['memsz']}, expected "
                f"{(vaddr, paddr, size, flags)}"
            )

    if len(raw) != IMAGE_SIZE:
        raise BuildError(f"raw image is {len(raw):#x} bytes, expected exactly {IMAGE_SIZE:#x}")
    for segment in segments:
        image_offset = segment["paddr"] - IMAGE_BASE
        file_bytes = elf[segment["offset"] : segment["offset"] + segment["filesz"]]
        if raw[image_offset : image_offset + segment["filesz"]] != file_bytes:
            raise BuildError(f"raw bytes do not match ELF PT_LOAD at GPA 0x{segment['paddr']:x}")

    entries = [
        (0x0000, 0x2003),
        (0x1000, 0x3003),
        (0x2010, 0x4003),
        (0x2020, 0x5003),
        (0x2028, 0x6003),
        (0x3000, 0x7001),
        (0x4000, NX | 0x9003),
        (0x5000, NX | 0xA003),
    ]
    for offset, expected in entries:
        actual = struct.unpack_from("<Q", raw, offset)[0]
        if actual != expected:
            raise BuildError(f"page-table entry at raw offset 0x{offset:x} is 0x{actual:x}, expected 0x{expected:x}")
    expected_data = (
        struct.pack("<QQ", initial, store_value)
        if profile == "store-load-probe"
        else struct.pack("<Q", payload)
    )
    if raw[0x8000 : 0x8000 + len(expected_data)] != expected_data:
        raise BuildError("guest data bytes do not match selected profile inputs")
    if raw[0x6000 : 0x6000 + len(code)] != code:
        raise BuildError(
            f"guest code bytes do not match {profile} profile: "
            f"{raw[0x6000 : 0x6000 + len(code)].hex()} != {code.hex()}"
        )
    if raw[0x6000 : 0x6000 + len(code)] != elf[
        segments[1]["offset"] : segments[1]["offset"] + code_size
    ]:
        raise BuildError("assembled guest instruction bytes were not preserved in the raw image")
    if raw[0x6000 + len(code) : 0x6008] != bytes(8 - len(code)):
        raise BuildError("the three code-segment alignment bytes must be zero")
    if raw[0x7000:0x8000] != bytes(PAGE_SIZE):
        raise BuildError("the physical gap between code and data is not zero-filled")

    return {
        "entry_gva": f"0x{entry:x}",
        "load_gpa": f"0x{IMAGE_BASE:x}",
        "image_size": f"0x{IMAGE_SIZE:x}",
        "segments": [
            {
                "vaddr": f"0x{s['vaddr']:x}",
                "paddr": f"0x{s['paddr']:x}",
                "filesz": f"0x{s['filesz']:x}",
                "memsz": f"0x{s['memsz']:x}",
                "flags": f"0x{s['flags']:x}",
            }
            for s in segments
        ],
        "page_table_entries_checked": len(entries),
        "payload_gva": f"0x{PAYLOAD_GVA:x}",
        "stack": {
            "gva_start": f"0x{STACK_GVA:x}",
            "gva_end": f"0x{STACK_GVA + PAGE_SIZE:x}",
            "gpa_start": f"0x{STACK_GPA:x}",
            "gpa_end": f"0x{STACK_GPA + PAGE_SIZE:x}",
            "in_raw_image": False,
        },
    }


def output_directory(repo_root: Path, requested: str | None) -> Path:
    work_root = (repo_root / "work/stage4").resolve()
    target = Path(requested).expanduser() if requested else work_root / "flat_probe"
    if not target.is_absolute():
        target = repo_root / target
    target = target.resolve()
    try:
        target.relative_to(work_root)
    except ValueError as exc:
        raise BuildError(f"output directory must be inside ignored {work_root}") from exc
    if target.exists() and any(target.iterdir()):
        raise BuildError(f"output directory must be absent or empty: {target}")
    target.mkdir(parents=True, exist_ok=True)
    return target


def build(args: argparse.Namespace) -> int:
    script = Path(__file__).resolve()
    repo_root = script.parents[2]
    profile = args.profile
    fixture_name = {
        "flat-probe": "flat_probe",
        "store-load-probe": "store_load_probe",
        "stack-probe": "stack_probe",
    }[profile]
    fixture_dir = repo_root / "tests/fixtures/guest/stage4" / fixture_name
    source = fixture_dir / "entry.S"
    linker_script = fixture_dir / "layout.ld"
    payload, initial, store_value = args.payload, args.initial_value, args.store_value
    for name, value in (("--payload", payload), ("--initial-value", initial), ("--store-value", store_value)):
        if value is not None and not 0 <= value <= 0xFFFFFFFFFFFFFFFF:
            raise BuildError(f"{name} must fit in an unsigned 64-bit integer")
    if profile == "store-load-probe" and (initial is None or store_value is None):
        raise BuildError("--profile store-load-probe requires --initial-value and --store-value")
    if profile != "store-load-probe" and (initial is not None or store_value is not None):
        raise BuildError("--initial-value and --store-value require --profile store-load-probe")
    payload = payload if profile != "store-load-probe" else 0
    initial = initial or 0
    store_value = store_value or 0

    out_dir = output_directory(repo_root, args.output_dir)
    clang = resolve_executable(os.environ.get("STAGE4_CLANG"), "clang")
    rustc, host, rustc_version, ld_lld, objcopy, tool_env = rust_toolchain()
    build_env = tool_env.copy()
    build_env.update({"LC_ALL": "C", "SOURCE_DATE_EPOCH": "0"})

    clang_version = run([str(clang), "--version"], env=build_env).splitlines()[0]
    lld_version = run([str(ld_lld), "--version"], env=build_env).splitlines()[0]
    objcopy_version = run([str(objcopy), "--version"], env=build_env)
    commands = [
        [
            str(clang),
            "--target=x86_64-unknown-none-elf",
            "-x",
            "assembler-with-cpp",
            "-ffreestanding",
            "-fno-pic",
            "-fno-pie",
            "-mno-red-zone",
            f"-DPROBE_PAYLOAD=0x{payload:016x}",
            f"-DPROBE_INITIAL=0x{initial:016x}",
            f"-DPROBE_STORE_VALUE=0x{store_value:016x}",
            "-c",
            str(source),
            "-o",
            str(out_dir / "entry.o"),
        ],
        [
            str(ld_lld),
            "-m",
            "elf_x86_64",
            "--build-id=none",
            "-T",
            str(linker_script),
            "-o",
            str(out_dir / "flat_probe.elf"),
            str(out_dir / "entry.o"),
        ],
        [
            str(objcopy),
            "-O",
            "binary",
            "--gap-fill=0x00",
            str(out_dir / "flat_probe.elf"),
            str(out_dir / "flat_probe.unpadded.bin"),
        ],
    ]
    for command in commands:
        run(command, env=build_env, cwd=repo_root)

    elf_path = out_dir / "flat_probe.elf"
    unpadded_path = out_dir / "flat_probe.unpadded.bin"
    elf = elf_path.read_bytes()
    raw = unpadded_path.read_bytes()
    if len(raw) > IMAGE_SIZE:
        raise BuildError(f"objcopy image exceeds fixed size: {len(raw):#x} > {IMAGE_SIZE:#x}")
    raw = raw + bytes(IMAGE_SIZE - len(raw))
    layout = validate_image(elf, raw, profile, payload, initial, store_value)
    raw_path = out_dir / "flat_probe.bin"
    raw_path.write_bytes(raw)
    unpadded_path.unlink()

    source_revision = run(["git", "rev-parse", "HEAD"], env=build_env, cwd=repo_root).strip()
    manifest = {
        "schema": "stage4-flat-probe-build-v1",
        "profile": {
            "flat-probe": "Stage4FlatProbeV1",
            "store-load-probe": "Stage4StoreLoadProbeV1",
            "stack-probe": "Stage4StackProbeV1",
        }[profile],
        "source_revision": source_revision,
        "source": {
            "entry.S": sha256(source.read_bytes()),
            "layout.ld": sha256(linker_script.read_bytes()),
        },
        "payload_u64": f"0x{payload:016x}",
        "initial_u64": f"0x{initial:016x}" if profile == "store-load-probe" else None,
        "store_value_u64": f"0x{store_value:016x}" if profile == "store-load-probe" else None,
        "target": "x86_64-unknown-none-elf",
        "toolchain": {
            "clang": {"path": str(clang), "version": clang_version},
            "rustc": {"path": str(rustc), "host": host, "version": rustc_version},
            "ld.lld": {"path": str(ld_lld), "version": lld_version},
            "rust-objcopy": {"path": str(objcopy), "version": objcopy_version},
            "DYLD_LIBRARY_PATH": build_env.get("DYLD_LIBRARY_PATH"),
        },
        "commands": commands,
        "layout": layout,
        "artifacts": {
            "flat_probe.elf": {"size": len(elf), "sha256": sha256(elf)},
            "flat_probe.bin": {"size": len(raw), "sha256": sha256(raw)},
        },
    }
    (out_dir / "build_manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(f"output: {out_dir}")
    print(f"ELF sha256: {manifest['artifacts']['flat_probe.elf']['sha256']}")
    print(f"raw sha256: {manifest['artifacts']['flat_probe.bin']['sha256']}")
    print(f"raw size: {len(raw):#x}; entry: {layout['entry_gva']}; payload: 0x{payload:016x}")
    print(f"clang: {clang_version}")
    print(f"ld.lld: {lld_version}")
    print(f"rust-objcopy: {objcopy_version}")
    print(f"manifest: {out_dir / 'build_manifest.json'}")
    return 0


def parse_payload(value: str) -> int:
    try:
        return int(value, 0)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("payload must be an integer such as 0x1122334455667788") from exc


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", choices=("flat-probe", "store-load-probe", "stack-probe"), default="flat-probe")
    parser.add_argument("--payload", type=parse_payload, default=0x5354473441424331)
    parser.add_argument("--initial-value", type=parse_payload)
    parser.add_argument("--store-value", type=parse_payload)
    parser.add_argument("--output-dir")
    args = parser.parse_args()
    try:
        return build(args)
    except (BuildError, OSError, subprocess.SubprocessError) as exc:
        print(f"build_stage4_flat_probe: error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
