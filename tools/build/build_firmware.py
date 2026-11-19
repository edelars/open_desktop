#!/usr/bin/env python3
"""Build the Open Desktop firmware ROM image (S7-01).

Pipeline: `cargo build --target x86_64-unknown-none` in firmware/uefi (build.rs assembles
start.S with the system clang and links with firmware/uefi/linker.ld) -> ELF -> `rust-objcopy -O
binary` -> flat raw bytes -> ODFWIMG1 header + zero padding to exactly 0x400000 bytes.

The image format is docs/specs/FIRMWARE_BOOT.md, section 3.2. The result of S7-01 is a build
skeleton that proves the toolchain works; it is not a firmware implementation.

`--efi-probe <path>` is a separate mode: it builds firmware/uefi/probe/efi_probe for
`x86_64-unknown-uefi`, checks the PE32+ headers (Machine, magic, Subsystem, `.reloc`, base
relocation directory) and writes the .efi to <path>. It is a build proof only: not a test
application (S7-06) and not the runtime image (S7-11).

`--smoke` is a third, toolchain-free mode (S7-03): it writes a minimal 0x400000-byte `ODFWIMG1` image
whose entry code is `mov al,'X'; mov dx,0x3F8; out dx,al; hlt` (64-bit). It is a machine-loader test
input, not firmware.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import platform
import shutil
import struct
import subprocess
import sys
import tempfile
import zlib
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping


ROOT = Path(__file__).resolve().parents[2]
FIRMWARE_DIR = ROOT / "firmware/uefi"
DEFAULT_OUT = ROOT / "work/stage7/firmware.bin"
DEFAULT_SMOKE_OUT = ROOT / "work/stage7/mini.bin"
DEFAULT_TARGET_DIR = ROOT / "work/stage7/firmware-target"

TARGET = "x86_64-unknown-none"
BIN_NAME = "od-firmware"

PROBE_DIR = FIRMWARE_DIR / "probe/efi_probe"
PROBE_TARGET = "x86_64-unknown-uefi"
PROBE_ARTIFACT = "efi_probe.efi"
TEST_APPS = ("ebs", "hello")
FV_MAGIC = b"ODFVARC\0"
FV_VERSION = 1
FV_HEADER_SIZE = 24
PE_MACHINE_X64 = 0x8664
PE32_PLUS_MAGIC = 0x20B
PE_SUBSYSTEM_EFI_APPLICATION = 10
PE_DIRECTORY_BASERELOC = 5

IMAGE_MAGIC = b"ODFWIMG1"
FORMAT_VERSION = 1
HEADER_SIZE = 0x40
IMAGE_SIZE = 0x0040_0000
LINK_BASE = 0x0020_0000
HANDOFF_MAJOR = 1
HANDOFF_MINOR_MAX = 4
CRC_OFFSET = 0x30

# --smoke entry code (64-bit): mov al,'X' ; mov dx,0x3F8 ; out dx,al ; hlt
SMOKE_CODE = bytes([0xB0, 0x58, 0x66, 0xBA, 0xF8, 0x03, 0xEE, 0xF4])

ET_EXEC = 2
PT_LOAD = 1


class BuildError(Exception):
    pass


# --- Image format (FIRMWARE_BOOT.md, section 3.2) --------------------------------------------


def crc32_ieee(data: bytes) -> int:
    """CRC-32 IEEE 802.3, reflected (FIRMWARE_BOOT.md, section 6.6); check('123456789') = 0xCBF43926."""
    return zlib.crc32(data) & 0xFFFF_FFFF


def build_header(entry_offset: int, crc: int = 0) -> bytes:
    header = bytearray(HEADER_SIZE)
    struct.pack_into(
        "<8sIIQQQHHII",
        header,
        0,
        IMAGE_MAGIC,
        FORMAT_VERSION,
        HEADER_SIZE,
        IMAGE_SIZE,
        LINK_BASE,
        entry_offset,
        HANDOFF_MAJOR,
        HANDOFF_MINOR_MAX,
        0,
        crc,
    )
    return bytes(header)


def image_crc(image: bytes) -> int:
    """CRC-32 of the whole image with the image_crc32 field treated as zero."""
    return crc32_ieee(image[:CRC_OFFSET] + b"\0\0\0\0" + image[CRC_OFFSET + 4 :])


def assemble_image(raw: bytes, entry_offset: int) -> bytes:
    """Put the header in front of the flat bytes and pad to exactly IMAGE_SIZE."""
    if len(raw) <= HEADER_SIZE:
        raise BuildError(f"flat image is only {len(raw):#x} bytes, no code after the {HEADER_SIZE:#x}-byte header")
    if len(raw) > IMAGE_SIZE:
        raise BuildError(f"flat image is {len(raw):#x} bytes, larger than the fixed image size {IMAGE_SIZE:#x}")
    if any(raw[:HEADER_SIZE]):
        raise BuildError("the header area of the flat image is not zero: the linker script must only reserve it")
    if not HEADER_SIZE <= entry_offset < IMAGE_SIZE:
        raise BuildError(f"entry offset {entry_offset:#x} is outside [{HEADER_SIZE:#x}, {IMAGE_SIZE:#x})")
    image = bytearray(IMAGE_SIZE)
    image[: len(raw)] = raw
    image[:HEADER_SIZE] = build_header(entry_offset)
    struct.pack_into("<I", image, CRC_OFFSET, image_crc(bytes(image)))
    return bytes(image)


def validate_image(image: bytes) -> dict[str, int]:
    """Check every field of the v1 header; return the decoded fields."""
    if len(image) != IMAGE_SIZE:
        raise BuildError(f"image is {len(image):#x} bytes, expected exactly {IMAGE_SIZE:#x}")
    (magic, version, header_size, image_size, link_base, entry_offset, handoff_major, handoff_minor_max, flags, crc) = (
        struct.unpack_from("<8sIIQQQHHII", image, 0)
    )
    fields = {
        "format_version": version,
        "header_size": header_size,
        "image_size": image_size,
        "link_base": link_base,
        "entry_offset": entry_offset,
        "handoff_major": handoff_major,
        "handoff_minor_max": handoff_minor_max,
        "flags": flags,
        "image_crc32": crc,
    }
    if magic != IMAGE_MAGIC:
        raise BuildError(f"bad image magic {magic!r}, expected {IMAGE_MAGIC!r}")
    expected = {
        "format_version": FORMAT_VERSION,
        "header_size": HEADER_SIZE,
        "image_size": IMAGE_SIZE,
        "link_base": LINK_BASE,
        "handoff_major": HANDOFF_MAJOR,
        "flags": 0,
    }
    for name, value in expected.items():
        if fields[name] != value:
            raise BuildError(f"header field {name} is {fields[name]:#x}, expected {value:#x}")
    if not HEADER_SIZE <= entry_offset < image_size:
        raise BuildError(f"entry_offset {entry_offset:#x} is outside [{HEADER_SIZE:#x}, {image_size:#x})")
    if any(image[0x34:HEADER_SIZE]):
        raise BuildError("reserved header bytes are not zero")
    computed = image_crc(image)
    if computed != crc:
        raise BuildError(f"image_crc32 is {crc:#010x}, computed {computed:#010x}")
    return fields


# --- ELF checks -------------------------------------------------------------------------------


def inspect_elf(elf: bytes) -> tuple[int, int, int]:
    """Return (entry, file offset, file size) of the single flat PT_LOAD segment of the firmware ELF."""
    if len(elf) < 64 or elf[:4] != b"\x7fELF" or elf[4] != 2 or elf[5] != 1:
        raise BuildError("linker output is not a little-endian ELF64 image")
    e_type = struct.unpack_from("<H", elf, 16)[0]
    if e_type != ET_EXEC:
        raise BuildError(
            f"ELF type is {e_type}, expected ET_EXEC (2): the image must be linked statically at a fixed "
            "address (is RUSTFLAGS overriding firmware/uefi/.cargo/config.toml?)"
        )
    entry = struct.unpack_from("<Q", elf, 24)[0]
    phoff = struct.unpack_from("<Q", elf, 32)[0]
    phentsize, phnum = struct.unpack_from("<HH", elf, 54)
    if phentsize < 56 or phoff + phentsize * phnum > len(elf):
        raise BuildError("ELF program header table is malformed")
    loads = []
    for index in range(phnum):
        p_type, _flags, offset, vaddr, paddr, filesz, memsz, _align = struct.unpack_from(
            "<IIQQQQQQ", elf, phoff + index * phentsize
        )
        if p_type == PT_LOAD:
            loads.append((offset, vaddr, paddr, filesz, memsz))
    if len(loads) != 1:
        raise BuildError(f"expected exactly one PT_LOAD segment, found {len(loads)}")
    offset, vaddr, paddr, filesz, memsz = loads[0]
    if vaddr != LINK_BASE or paddr != LINK_BASE:
        raise BuildError(f"PT_LOAD is at {vaddr:#x}/{paddr:#x}, expected link base {LINK_BASE:#x}")
    if memsz > IMAGE_SIZE or offset + filesz > len(elf):
        raise BuildError("PT_LOAD does not fit the image size or the file")
    if not LINK_BASE + HEADER_SIZE <= entry < LINK_BASE + filesz:
        raise BuildError(f"ELF entry {entry:#x} is outside the loaded code")
    return entry, offset, filesz


# --- PE32+ checks (EFI probe) ------------------------------------------------------------------


def inspect_pe(pe: bytes) -> dict[str, int]:
    """Check that `pe` is an x86-64 PE32+ EFI application with a base relocation table.

    Checked: MZ/PE signatures, Machine = 0x8664, optional header magic = 0x20B, Subsystem = 10
    (EFI_APPLICATION), a section named `.reloc` and DataDirectory[5] (Base Relocation Table)
    pointing inside that section. Returns the decoded fields.
    """
    if len(pe) < 0x40 or pe[:2] != b"MZ":
        raise BuildError("not a PE image: no MZ signature")
    pe_offset = struct.unpack_from("<I", pe, 0x3C)[0]
    if pe_offset + 24 > len(pe) or pe[pe_offset : pe_offset + 4] != b"PE\0\0":
        raise BuildError("not a PE image: no PE signature at e_lfanew")
    machine, section_count, _stamp, _symtab, _symbols, optional_size, _characteristics = struct.unpack_from(
        "<HHIIIHH", pe, pe_offset + 4
    )
    optional = pe_offset + 24
    if machine != PE_MACHINE_X64:
        raise BuildError(f"PE Machine is {machine:#06x}, expected {PE_MACHINE_X64:#06x} (x86-64)")
    if optional_size < 2 or optional + optional_size > len(pe):
        raise BuildError("PE optional header is truncated")
    magic = struct.unpack_from("<H", pe, optional)[0]
    if magic != PE32_PLUS_MAGIC:
        raise BuildError(f"PE optional header magic is {magic:#06x}, expected {PE32_PLUS_MAGIC:#06x} (PE32+)")
    if optional_size < 112:
        raise BuildError("PE32+ optional header is shorter than the fixed part")
    subsystem = struct.unpack_from("<H", pe, optional + 68)[0]
    if subsystem != PE_SUBSYSTEM_EFI_APPLICATION:
        raise BuildError(f"PE Subsystem is {subsystem}, expected {PE_SUBSYSTEM_EFI_APPLICATION} (EFI_APPLICATION)")
    directory_count = struct.unpack_from("<I", pe, optional + 108)[0]
    directory_base = optional + 112
    if directory_count <= PE_DIRECTORY_BASERELOC or directory_base + 8 * directory_count > optional + optional_size:
        raise BuildError("PE has no Base Relocation directory entry (DataDirectory[5])")
    reloc_rva, reloc_size = struct.unpack_from("<II", pe, directory_base + 8 * PE_DIRECTORY_BASERELOC)
    if reloc_rva == 0 or reloc_size == 0:
        raise BuildError("PE DataDirectory[5] (Base Relocation Table) is empty")
    table = optional + optional_size
    if table + 40 * section_count > len(pe):
        raise BuildError("PE section table is truncated")
    reloc_section = None
    for index in range(section_count):
        name, virtual_size, virtual_address, raw_size = struct.unpack_from("<8sIII", pe, table + 40 * index)
        if name.rstrip(b"\0") == b".reloc":
            reloc_section = (virtual_size, virtual_address, raw_size)
    if reloc_section is None:
        raise BuildError("PE has no .reloc section")
    virtual_size, virtual_address, raw_size = reloc_section
    if not (
        virtual_address <= reloc_rva and reloc_rva + reloc_size <= virtual_address + max(virtual_size, raw_size)
    ):
        raise BuildError("PE DataDirectory[5] does not point inside the .reloc section")
    return {
        "machine": machine,
        "magic": magic,
        "subsystem": subsystem,
        "sections": section_count,
        "reloc_rva": reloc_rva,
        "reloc_size": reloc_size,
    }


# --- Tools ------------------------------------------------------------------------------------


@dataclass(frozen=True)
class Tools:
    clang: Path
    cargo: Path
    rustc: Path
    objcopy: Path
    sysroot: Path
    tool_env: dict[str, str]


def _find(env: Mapping[str, str], variable: str, name: str, hint: str) -> Path:
    explicit = env.get(variable)
    if explicit:
        path = Path(explicit).expanduser()
        if not path.is_file() or not os.access(path, os.X_OK):
            raise BuildError(f"{variable}={explicit} is not an executable file ({name} is required: {hint})")
        return path.absolute()
    found = shutil.which(name, path=env.get("PATH", ""))
    if not found:
        raise BuildError(f"could not find {name!r} on PATH ({hint}); alternatively set {variable}")
    return Path(found).absolute()


def _run(command: list[str], *, env: Mapping[str, str], cwd: Path | None = None) -> str:
    result = subprocess.run(command, cwd=cwd, env=dict(env), check=False, capture_output=True, text=True)
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise BuildError(f"command failed ({result.returncode}): {command!r}\n{detail}")
    return (result.stdout + result.stderr).strip()


def resolve_tools(env: Mapping[str, str] | None = None) -> Tools:
    """Find clang, cargo, rustc, the x86_64-unknown-none target and rust-objcopy, or fail with a clear message."""
    env = dict(os.environ if env is None else env)
    clang = _find(env, "OD_FW_CLANG", "clang", "install the Xcode command line tools or LLVM clang")
    cargo = _find(env, "OD_FW_CARGO", "cargo", "install Rust with rustup")
    rustc = _find(env, "OD_FW_RUSTC", "rustc", "install Rust with rustup")
    version = _run([str(rustc), "-vV"], env=env)
    sysroot = Path(_run([str(rustc), "--print", "sysroot"], env=env))
    host = next((l.split(":", 1)[1].strip() for l in version.splitlines() if l.startswith("host:")), None)
    if host is None:
        raise BuildError("rustc -vV did not report a host triple")
    if not (sysroot / "lib/rustlib" / TARGET / "lib").is_dir():
        raise BuildError(f"the Rust target {TARGET} is not installed in {sysroot}: run `rustup target add {TARGET}`")
    objcopy_explicit = env.get("OD_FW_RUST_OBJCOPY")
    if objcopy_explicit:
        objcopy = _find(env, "OD_FW_RUST_OBJCOPY", "rust-objcopy", "ships with the Rust sysroot")
    else:
        objcopy = sysroot / "lib/rustlib" / host / "bin/rust-objcopy"
        if not objcopy.is_file() or not os.access(objcopy, os.X_OK):
            raise BuildError(
                f"could not find rust-objcopy at {objcopy}: it ships with the Rust sysroot "
                "(`rustup component add llvm-tools` provides it; or set OD_FW_RUST_OBJCOPY)"
            )
    tool_env = dict(env)
    if platform.system() == "Darwin":
        libraries = sorted((sysroot / "lib").glob("libLLVM*.dylib"))
        if libraries:
            library_dir = str(libraries[0].parent.resolve())
            inherited = tool_env.get("DYLD_LIBRARY_PATH", "")
            search = [library_dir, *(part for part in inherited.split(os.pathsep) if part)]
            tool_env["DYLD_LIBRARY_PATH"] = os.pathsep.join(dict.fromkeys(search))
    return Tools(clang, cargo, rustc, objcopy.absolute(), sysroot, tool_env)


# --- Build ------------------------------------------------------------------------------------


def pack_fv(files: Mapping[str, bytes]) -> bytes:
    """Pack the built-in firmware volume in sorted name order."""
    archive = bytearray(FV_HEADER_SIZE)
    for name, data in sorted(files.items()):
        if (
            not 1 <= len(name) <= 63
            or name in (".", "..")
            or any(char not in "abcdefghijklmnopqrstuvwxyz0123456789._-" for char in name)
        ):
            raise BuildError(f"invalid FV name: {name!r}")
        if not data:
            raise BuildError(f"empty FV file: {name}")
        if len(archive) + 8 + len(name) + len(data) > IMAGE_SIZE:
            raise BuildError("firmware volume exceeds the ROM capacity")
        archive.extend(struct.pack("<HHI", len(name), 0, len(data)))
        archive.extend(name.encode("ascii"))
        archive.extend(data)
    struct.pack_into(
        "<8sHHIII", archive, 0, FV_MAGIC, FV_VERSION, FV_HEADER_SIZE, len(archive), len(files), 0,
    )
    struct.pack_into("<I", archive, 20, crc32_ieee(archive))
    return bytes(archive)


def build_elf(tools: Tools, target_dir: Path, *, fv_archive: Path | None = None) -> Path:
    env = dict(tools.tool_env)
    # RUSTFLAGS would replace firmware/uefi/.cargo/config.toml (relocation-model=static).
    env.pop("RUSTFLAGS", None)
    env.pop("CARGO_ENCODED_RUSTFLAGS", None)
    env["CARGO_TARGET_DIR"] = str(target_dir)
    env["OD_FW_CLANG"] = str(tools.clang)
    env.pop("OD_FW_FV_ARCHIVE", None)
    if fv_archive is not None:
        env["OD_FW_FV_ARCHIVE"] = str(fv_archive)
    # cwd must be inside firmware/uefi: cargo reads .cargo/config.toml relative to the cwd.
    _run(
        [str(tools.cargo), "build", "--release", "--target", TARGET, "--offline"],
        env=env,
        cwd=FIRMWARE_DIR,
    )
    elf = target_dir / TARGET / "release" / BIN_NAME
    if not elf.is_file():
        raise BuildError(f"cargo did not produce {elf}")
    return elf


def build_image(
    tools: Tools, target_dir: Path, out: Path, *, fv_archive: Path | None = None,
) -> tuple[bytes, dict[str, int]]:
    elf_path = build_elf(tools, target_dir, fv_archive=fv_archive)
    elf = elf_path.read_bytes()
    entry, offset, filesz = inspect_elf(elf)
    raw_path = elf_path.with_suffix(".raw")
    _run([str(tools.objcopy), "-O", "binary", str(elf_path), str(raw_path)], env=tools.tool_env)
    raw = raw_path.read_bytes()
    if raw != elf[offset : offset + filesz]:
        raise BuildError("rust-objcopy output does not match the ELF PT_LOAD bytes")
    image = assemble_image(raw, entry - LINK_BASE)
    fields = validate_image(image)
    out.parent.mkdir(parents=True, exist_ok=True)
    temporary = out.with_name(out.name + ".tmp")
    temporary.write_bytes(image)
    temporary.replace(out)
    return image, fields


@dataclass(frozen=True)
class ProbeTools:
    cargo: Path
    rustc: Path
    sysroot: Path
    lld: Path | None
    lld_link: Path | None
    tool_env: dict[str, str]


def resolve_probe_tools(env: Mapping[str, str] | None = None) -> ProbeTools:
    """Find cargo, rustc and the x86_64-unknown-uefi target; locate rust-lld / lld-link for the report.

    rustc itself invokes the linker of the target (`rust-lld` in `lld-link` mode, shipped in the
    sysroot); the paths found here are only reported, cargo is not told to use them.
    """
    env = dict(os.environ if env is None else env)
    cargo = _find(env, "OD_FW_CARGO", "cargo", "install Rust with rustup")
    rustc = _find(env, "OD_FW_RUSTC", "rustc", "install Rust with rustup")
    version = _run([str(rustc), "-vV"], env=env)
    sysroot = Path(_run([str(rustc), "--print", "sysroot"], env=env))
    host = next((l.split(":", 1)[1].strip() for l in version.splitlines() if l.startswith("host:")), None)
    if host is None:
        raise BuildError("rustc -vV did not report a host triple")
    if not (sysroot / "lib/rustlib" / PROBE_TARGET / "lib").is_dir():
        raise BuildError(
            f"the Rust target {PROBE_TARGET} is not installed in {sysroot}: run `rustup target add {PROBE_TARGET}`"
        )
    bin_dir = sysroot / "lib/rustlib" / host / "bin"
    lld = bin_dir / "rust-lld"
    lld_link = bin_dir / "gcc-ld/lld-link"
    return ProbeTools(
        cargo,
        rustc,
        sysroot,
        lld if lld.is_file() else None,
        lld_link if lld_link.is_file() else None,
        dict(env),
    )


def build_probe(tools: ProbeTools, target_dir: Path, out: Path) -> tuple[bytes, dict[str, int]]:
    """Build the EFI probe, check its PE32+ headers and write the .efi to `out`."""
    env = dict(tools.tool_env)
    env.pop("RUSTFLAGS", None)
    env.pop("CARGO_ENCODED_RUSTFLAGS", None)
    env["CARGO_TARGET_DIR"] = str(target_dir)
    _run(
        [str(tools.cargo), "build", "--release", "--target", PROBE_TARGET, "--offline"],
        env=env,
        cwd=PROBE_DIR,
    )
    artifact = target_dir / PROBE_TARGET / "release" / PROBE_ARTIFACT
    if not artifact.is_file():
        raise BuildError(f"cargo did not produce {artifact}")
    pe = artifact.read_bytes()
    fields = inspect_pe(pe)
    out.parent.mkdir(parents=True, exist_ok=True)
    temporary = out.with_name(out.name + ".tmp")
    temporary.write_bytes(pe)
    temporary.replace(out)
    return pe, fields


def build_test_apps(tools: ProbeTools, target_dir: Path) -> bytes:
    env = dict(tools.tool_env)
    env.pop("RUSTFLAGS", None)
    env.pop("CARGO_ENCODED_RUSTFLAGS", None)
    env["CARGO_TARGET_DIR"] = str(target_dir)
    _run(
        [str(tools.cargo), "build", "--release", "--target", PROBE_TARGET, "--locked", "--offline"],
        env=env,
        cwd=FIRMWARE_DIR / "tests/apps",
    )
    files = {}
    for name in TEST_APPS:
        artifact = target_dir / PROBE_TARGET / "release" / f"{name}.efi"
        if not artifact.is_file():
            raise BuildError(f"cargo did not produce {artifact}")
        files[name] = artifact.read_bytes()
    return pack_fv(files)


def write_test_app_archive(target_dir: Path, archive: bytes) -> Path:
    archive_dir = target_dir / "fv" / hashlib.sha256(archive).hexdigest()
    archive_dir.mkdir(parents=True, exist_ok=True)
    path = archive_dir / "archive.bin"
    if path.is_file() and path.read_bytes() == archive:
        return path
    with tempfile.TemporaryDirectory(dir=archive_dir) as directory:
        temporary = Path(directory) / path.name
        temporary.write_bytes(archive)
        temporary.replace(path)
    return path


def tool_versions(tools: Tools) -> list[str]:
    lines = [
        _run([str(tools.rustc), "--version"], env=tools.tool_env),
        _run([str(tools.cargo), "--version"], env=tools.tool_env),
        _run([str(tools.clang), "--version"], env=tools.tool_env).splitlines()[0],
    ]
    try:
        lines.append("rust-objcopy: " + _run([str(tools.objcopy), "--version"], env=tools.tool_env).splitlines()[0])
    except BuildError as error:
        lines.append(f"rust-objcopy: version unavailable ({error})")
    return lines


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument(
        "--out",
        type=Path,
        default=None,
        help=f"output image (default: {DEFAULT_OUT.relative_to(ROOT)}; with --smoke: {DEFAULT_SMOKE_OUT.relative_to(ROOT)})",
    )
    parser.add_argument(
        "--efi-probe",
        type=Path,
        metavar="PATH",
        help="instead of the ROM image: build the PE32+ EFI build proof (firmware/uefi/probe/efi_probe), "
        "check its headers and write the .efi to PATH",
    )
    parser.add_argument(
        "--smoke",
        action="store_true",
        help="instead of the ROM image: write the minimal smoke image (entry code writes 'X' to COM1) to --out (default: work/stage7/mini.bin, never the real firmware.bin); "
        "needs no toolchain",
    )
    parser.add_argument(
        "--target-dir",
        type=Path,
        default=DEFAULT_TARGET_DIR,
        help=f"cargo target directory (default: {DEFAULT_TARGET_DIR.relative_to(ROOT)})",
    )
    parser.add_argument(
        "--with-test-apps",
        action="store_true",
        help="build the hello and ebs EFI applications and embed them in the firmware volume",
    )
    args = parser.parse_args(argv)
    if args.with_test_apps and (args.smoke or args.efi_probe is not None):
        parser.error("--with-test-apps requires a ROM build")
    if args.out is None:
        args.out = DEFAULT_SMOKE_OUT if args.smoke else DEFAULT_OUT
    return args


def build_smoke_image() -> bytes:
    """The --smoke image: the v1 header followed directly by SMOKE_CODE (entry_offset = header size)."""
    image = assemble_image(bytes(HEADER_SIZE) + SMOKE_CODE, HEADER_SIZE)
    validate_image(image)
    return image


def main_smoke(args: argparse.Namespace) -> int:
    image = build_smoke_image()
    out = args.out.absolute()
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_bytes(image)
    print(f"smoke image: {out}")
    print(f"size: {len(image):#x} ({len(image)} bytes)")
    print(f"sha256: {hashlib.sha256(image).hexdigest()}")
    print(f"entry_offset: {HEADER_SIZE:#x} (RIP = {LINK_BASE + HEADER_SIZE:#x}), code: {SMOKE_CODE.hex()}")
    return 0


def main_efi_probe(args: argparse.Namespace) -> int:
    try:
        tools = resolve_probe_tools()
        target_dir = args.target_dir.absolute().with_name(args.target_dir.absolute().name + "-efi-probe")
        pe, fields = build_probe(tools, target_dir, args.efi_probe.absolute())
        versions = [
            _run([str(tools.rustc), "--version"], env=tools.tool_env),
            _run([str(tools.cargo), "--version"], env=tools.tool_env),
        ]
    except BuildError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    print(f"efi probe: {args.efi_probe}")
    print(f"size: {len(pe):#x} ({len(pe)} bytes)")
    print(f"sha256: {hashlib.sha256(pe).hexdigest()}")
    print(
        f"pe: Machine={fields['machine']:#06x} magic={fields['magic']:#06x} Subsystem={fields['subsystem']} "
        f"sections={fields['sections']} .reloc present, DataDirectory[5] rva={fields['reloc_rva']:#x} "
        f"size={fields['reloc_size']:#x}"
    )
    print(f"linker: rust-lld (lld-link flavor) {tools.lld or 'not found in sysroot'}; lld-link wrapper {tools.lld_link or 'not found'}")
    for line in versions:
        print(line)
    return 0


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    if args.smoke:
        if args.efi_probe is not None:
            print("error: --smoke and --efi-probe are mutually exclusive", file=sys.stderr)
            return 2
        return main_smoke(args)
    if args.efi_probe is not None:
        return main_efi_probe(args)
    try:
        tools = resolve_tools()
        target_dir = args.target_dir.absolute()
        fv_archive = None
        if args.with_test_apps:
            archive = build_test_apps(resolve_probe_tools(), target_dir / "test-apps")
            fv_archive = write_test_app_archive(target_dir, archive)
            target_dir = fv_archive.parent
        image, fields = build_image(tools, target_dir, args.out.absolute(), fv_archive=fv_archive)
        versions = tool_versions(tools)
    except BuildError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    print(f"image: {args.out}")
    print(f"size: {len(image):#x} ({len(image)} bytes)")
    print(f"sha256: {hashlib.sha256(image).hexdigest()}")
    print(f"entry_offset: {fields['entry_offset']:#x} (RIP = {LINK_BASE + fields['entry_offset']:#x})")
    print(f"image_crc32: {fields['image_crc32']:#010x}")
    for line in versions:
        print(line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
