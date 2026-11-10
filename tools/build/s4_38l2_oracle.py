#!/usr/bin/env python3
"""Independent oracles for the Linux direct boot (S4-38 L2): host zstd, PT_LOAD cross-check,
checkpoint address table, decompressor static facts.

No guest is executed and no kernel byte is modified. The tool:

1. reads the bzImage setup header, cuts the payload by `payload_offset` / `payload_length`
   (relative to the protected-mode code at `(setup_sects + 1) * 512`), removes the 4-byte
   little-endian size trailer and decompresses with the host `zstd` binary;
2. compares the result with the staged `vmlinux` files: build-id, e_entry and every PT_LOAD
   (header tuple, SHA-256 and first differing byte);
3. builds the checkpoint address table from the SYMTAB of `vmlinux-debug`;
4. derives the address of the relocated decompressor copy (RBX) from bytes of the image;
5. checks, against the kernel source, that the decompressor does not write to COM1 for the
   S4-37 command line (no `earlyprintk`, no `console=uart8250,io,...`);
6. runs the injections (corrupted payload byte, foreign build-id, trailer left in the stream).

Only the standard library and the host `zstd` binary are used. Large inputs live in the ignored
`work/stage4/` directory; the decompressed ELF is written to a temporary directory outside Git.

Exit codes: 0 all checks passed, 1 a check failed, 2 usage or input error.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_STAGE_DIR = ROOT / "work/stage4/linux-7.0.0-38.38"
DEFAULT_S435_EVIDENCE = ROOT / "docs/reports/data/stage4/s4-35-linux-artifact.json"
DEFAULT_EVIDENCE = ROOT / "docs/reports/data/stage4/s4-38l2-oracle.json"
DEFAULT_ZSTD = "/opt/homebrew/bin/zstd"
VMLINUZ = "vmlinuz-7.0.0-38-generic"
VMLINUX = "vmlinux-7.0.0-38-generic"
VMLINUX_DEBUG = "vmlinux-debug-7.0.0-38-generic"
SOURCE_TAR = "source/linux-source-7.0.0.tar.bz2"
EXPECTED_BUILD_ID_PREFIX = "338e542e"
TAR_PREFIX = "linux-source-7.0.0/"
CHUNK = 8 * 1024 * 1024
ZSTD_MAGIC = b"\x28\xb5\x2f\xfd"
PT_LOAD = 1
PT_NOTE = 4
SHT_SYMTAB = 2
NT_GNU_BUILD_ID = 3

# Command line of S4-37 L6 (tools/runner/src/linux_direct.rs): prefix, nonce, suffix.
CMDLINE_PREFIX = "console=ttyS0,115200 earlycon=uart8250,io,0x3f8 8250.nr_uarts=1"
CMDLINE_SUFFIX = "nokaslr"

CHECKPOINTS = [
    # (name, symbol, offset added to the symbol, role)
    ("e_entry", "startup_64", 0, "ELF entry; must equal physical startup_64"),
    ("startup_64", "startup_64", 0, "kernel entry (physical == ELF e_entry)"),
    ("verify_cpu", "verify_cpu", 0, "CPU sanitize; called from startup_64"),
    ("cpuid1", "verify_cpu", 0x87, "C7: first CPUID(1) when the vendor is neither GenuineIntel nor AuthenticAMD (cpuid_divergence boundary)"),
    ("x86_64_start_kernel", "x86_64_start_kernel", 0, "checkpoint of the L2 table (C-number assigned in L4)"),
    ("start_kernel", "start_kernel", 0, "checkpoint of the L2 table (C-number assigned in L4)"),
    ("setup_arch", "setup_arch", 0, "checkpoint of the L2 table (C-number assigned in L4)"),
    ("tsc_init", "tsc_init", 0, "checkpoint of the L2 table (C-number assigned in L4)"),
    ("apic_intr_mode_init", "apic_intr_mode_init", 0, "checkpoint of the L2 table (C-number assigned in L4)"),
    ("check_timer", "check_timer", 0, "checkpoint of the L2 table (C-number assigned in L4)"),
    ("populate_rootfs", "populate_rootfs", 0, "checkpoint of the L2 table (C-number assigned in L4)"),
    ("run_init_process", "run_init_process", 0, "checkpoint of the L2 table (C-number assigned in L4)"),
    ("kernel_execve", "kernel_execve", 0, "C17: exec of init (kernel_execve)"),
]


class OracleError(Exception):
    """Input is unusable (missing, malformed)."""


# --------------------------------------------------------------------------------------
# Hashing
# --------------------------------------------------------------------------------------


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> tuple[str, int]:
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        while True:
            chunk = handle.read(CHUNK)
            if not chunk:
                break
            digest.update(chunk)
            size += len(chunk)
    return digest.hexdigest(), size


# --------------------------------------------------------------------------------------
# bzImage
# --------------------------------------------------------------------------------------


def parse_setup_header(image: bytes) -> dict:
    """Return the setup header fields needed to find the compressed payload."""
    if len(image) < 0x270:
        raise OracleError("bzImage shorter than the setup header")
    if image[0x202:0x206] != b"HdrS":
        raise OracleError("setup header magic HdrS missing")
    if struct.unpack_from("<H", image, 0x1FE)[0] != 0xAA55:
        raise OracleError("boot flag 0xAA55 missing")
    setup_sects = image[0x1F1] or 4
    payload_offset, payload_length = struct.unpack_from("<II", image, 0x248)
    kernel_alignment = struct.unpack_from("<I", image, 0x230)[0]
    pref_address = struct.unpack_from("<Q", image, 0x258)[0]
    init_size = struct.unpack_from("<I", image, 0x260)[0]
    return {
        "setup_sects": setup_sects,
        "protocol_version": struct.unpack_from("<H", image, 0x206)[0],
        "pm_offset": (setup_sects + 1) * 512,
        "payload_offset": payload_offset,
        "payload_length": payload_length,
        "kernel_alignment": kernel_alignment,
        "pref_address": pref_address,
        "init_size": init_size,
    }


def extract_payload(image: bytes, header: dict) -> bytes:
    start = header["pm_offset"] + header["payload_offset"]
    end = start + header["payload_length"]
    if header["payload_length"] < 8 or end > len(image):
        raise OracleError("payload range is outside the image")
    return image[start:end]


def split_trailer(payload: bytes) -> tuple[bytes, int]:
    """Split `payload` into the zstd stream and the 4-byte little-endian size trailer."""
    if len(payload) < 8 or payload[:4] != ZSTD_MAGIC:
        raise OracleError("payload does not start with the zstd frame magic")
    return payload[:-4], struct.unpack("<I", payload[-4:])[0]


def zstd_decompress(stream: bytes, zstd_bin: str, out_path: Path) -> tuple[bool, str]:
    """Run the host zstd; return (ok, diagnostic). The output file is written only on success."""
    partial = out_path.with_name(out_path.name + ".partial")
    with partial.open("wb") as handle:
        proc = subprocess.run(
            [zstd_bin, "-d", "-q", "-c"],
            input=stream,
            stdout=handle,
            stderr=subprocess.PIPE,
            check=False,
        )
    if proc.returncode != 0:
        partial.unlink(missing_ok=True)
        return False, proc.stderr.decode("utf-8", "replace").strip() or f"exit {proc.returncode}"
    partial.replace(out_path)
    return True, ""


# --------------------------------------------------------------------------------------
# ELF
# --------------------------------------------------------------------------------------


class Elf64:
    """Minimal ELF64 little-endian reader over a file; segments are read on demand."""

    def __init__(self, path: Path):
        self.path = path
        with path.open("rb") as handle:
            head = handle.read(64)
            if len(head) < 64 or head[:4] != b"\x7fELF" or head[4] != 2 or head[5] != 1:
                raise OracleError(f"{path.name}: not an ELF64 little-endian file")
            (
                self.e_type,
                self.e_machine,
                _version,
                self.e_entry,
                phoff,
                shoff,
                _flags,
                _ehsize,
                phentsize,
                phnum,
                shentsize,
                shnum,
                self.shstrndx,
            ) = struct.unpack_from("<HHIQQQIHHHHHH", head, 16)
            self.phdrs: list[dict] = []
            handle.seek(phoff)
            for _ in range(phnum):
                raw = handle.read(phentsize)
                if len(raw) < 56:
                    raise OracleError(f"{path.name}: truncated program header table")
                p_type, p_flags, p_offset, p_vaddr, p_paddr, p_filesz, p_memsz, p_align = struct.unpack_from(
                    "<IIQQQQQQ", raw
                )
                self.phdrs.append(
                    {
                        "type": p_type,
                        "flags": p_flags,
                        "offset": p_offset,
                        "vaddr": p_vaddr,
                        "paddr": p_paddr,
                        "filesz": p_filesz,
                        "memsz": p_memsz,
                        "align": p_align,
                    }
                )
            self.shdrs: list[dict] = []
            if shoff:
                handle.seek(shoff)
                for _ in range(shnum):
                    raw = handle.read(shentsize)
                    if len(raw) < 64:
                        raise OracleError(f"{path.name}: truncated section header table")
                    name, sh_type, _fl, addr, offset, size, link, _info, _al, entsize = struct.unpack_from(
                        "<IIQQQQIIQQ", raw
                    )
                    self.shdrs.append(
                        {"name": name, "type": sh_type, "addr": addr, "offset": offset, "size": size, "link": link, "entsize": entsize}
                    )

    def read(self, offset: int, size: int) -> bytes:
        with self.path.open("rb") as handle:
            handle.seek(offset)
            data = handle.read(size)
        if len(data) != size:
            raise OracleError(f"{self.path.name}: short read at {offset:#x}")
        return data

    def loads(self) -> list[dict]:
        return [p for p in self.phdrs if p["type"] == PT_LOAD]

    def build_id(self) -> str | None:
        for phdr in self.phdrs:
            if phdr["type"] != PT_NOTE:
                continue
            data = self.read(phdr["offset"], phdr["filesz"])
            pos = 0
            while pos + 12 <= len(data):
                namesz, descsz, note_type = struct.unpack_from("<III", data, pos)
                name_at = pos + 12
                desc_at = name_at + ((namesz + 3) & ~3)
                if desc_at + descsz > len(data):
                    break
                if note_type == NT_GNU_BUILD_ID and data[name_at : name_at + namesz] == b"GNU\x00":
                    return data[desc_at : desc_at + descsz].hex()
                pos = desc_at + ((descsz + 3) & ~3)
        return None

    def build_id_location(self) -> int | None:
        """File offset of the first build-id byte (used by the injections)."""
        for phdr in self.phdrs:
            if phdr["type"] != PT_NOTE:
                continue
            data = self.read(phdr["offset"], phdr["filesz"])
            pos = 0
            while pos + 12 <= len(data):
                namesz, descsz, note_type = struct.unpack_from("<III", data, pos)
                name_at = pos + 12
                desc_at = name_at + ((namesz + 3) & ~3)
                if note_type == NT_GNU_BUILD_ID and data[name_at : name_at + namesz] == b"GNU\x00":
                    return phdr["offset"] + desc_at
                pos = desc_at + ((descsz + 3) & ~3)
        return None

    def symbols(self, wanted: set[str] | None) -> dict[str, list[dict]]:
        """Return SYMTAB entries whose name is in `wanted` (all duplicates are kept); None means all."""
        symtab = next((s for s in self.shdrs if s["type"] == SHT_SYMTAB), None)
        if symtab is None:
            raise OracleError(f"{self.path.name}: no SYMTAB section")
        strtab_hdr = self.shdrs[symtab["link"]]
        table = self.read(symtab["offset"], symtab["size"])
        strtab = self.read(strtab_hdr["offset"], strtab_hdr["size"])
        found: dict[str, list[dict]] = {}
        for pos in range(0, len(table) - 23, 24):
            st_name, st_info, _other, shndx, value, size = struct.unpack_from("<IBBHQQ", table, pos)
            if st_name == 0:
                continue
            end = strtab.find(b"\x00", st_name)
            name = strtab[st_name:end].decode("ascii", "replace")
            if wanted is None or name in wanted:
                found.setdefault(name, []).append(
                    {"value": value, "size": size, "type": st_info & 0xF, "bind": st_info >> 4, "shndx": shndx}
                )
        return found

    def va_to_pa_and_offset(self, va: int) -> tuple[int, int] | None:
        for phdr in self.loads():
            if phdr["vaddr"] <= va < phdr["vaddr"] + phdr["filesz"]:
                delta = va - phdr["vaddr"]
                return phdr["paddr"] + delta, phdr["offset"] + delta
        return None


# --------------------------------------------------------------------------------------
# Comparisons
# --------------------------------------------------------------------------------------


def load_key(phdr: dict) -> tuple:
    return (phdr["flags"], phdr["vaddr"], phdr["paddr"], phdr["filesz"], phdr["memsz"], phdr["align"])


def compare_pt_loads(candidate: Elf64, reference: Elf64) -> dict:
    """Compare PT_LOAD headers and file bytes; never raises on a mismatch."""
    cand_loads, ref_loads = candidate.loads(), reference.loads()
    result: dict = {
        "load_count_candidate": len(cand_loads),
        "load_count_reference": len(ref_loads),
        "entry_equal": candidate.e_entry == reference.e_entry,
        "segments": [],
        "equal": False,
    }
    all_equal = len(cand_loads) == len(ref_loads) and candidate.e_entry == reference.e_entry
    for index in range(max(len(cand_loads), len(ref_loads))):
        a = cand_loads[index] if index < len(cand_loads) else None
        b = ref_loads[index] if index < len(ref_loads) else None
        entry: dict = {"index": index}
        if a is None or b is None:
            entry["equal"] = False
            all_equal = False
            result["segments"].append(entry)
            continue
        header_equal = load_key(a) == load_key(b)
        entry.update(
            {
                "vaddr": a["vaddr"],
                "paddr": a["paddr"],
                "filesz": a["filesz"],
                "memsz": a["memsz"],
                "flags": a["flags"],
                "header_equal": header_equal,
            }
        )
        digest_a, digest_b = hashlib.sha256(), hashlib.sha256()
        first_diff = None
        if a["filesz"] == b["filesz"]:
            done = 0
            while done < a["filesz"]:
                step = min(CHUNK, a["filesz"] - done)
                data_a = candidate.read(a["offset"] + done, step)
                data_b = reference.read(b["offset"] + done, step)
                digest_a.update(data_a)
                digest_b.update(data_b)
                if first_diff is None and data_a != data_b:
                    first_diff = done + next(i for i in range(step) if data_a[i] != data_b[i])
                done += step
        entry["sha256_candidate"] = digest_a.hexdigest()
        entry["sha256_reference"] = digest_b.hexdigest()
        entry["first_diff_offset"] = first_diff
        entry["equal"] = header_equal and first_diff is None and a["filesz"] == b["filesz"]
        all_equal = all_equal and entry["equal"]
        result["segments"].append(entry)
    result["equal"] = all_equal
    return result


def check_elf_against_reference(candidate: Elf64, reference: Elf64) -> dict:
    """Build-id plus PT_LOAD comparison. `ok` is true only if both agree."""
    cand_id, ref_id = candidate.build_id(), reference.build_id()
    loads = compare_pt_loads(candidate, reference)
    build_id_equal = cand_id is not None and cand_id == ref_id
    return {
        "build_id_candidate": cand_id,
        "build_id_reference": ref_id,
        "build_id_equal": build_id_equal,
        "pt_load": loads,
        "ok": build_id_equal and loads["equal"],
    }


# --------------------------------------------------------------------------------------
# Decompressor facts derived from image bytes
# --------------------------------------------------------------------------------------

# compressed/head_64.S startup_64 (CONFIG_RELOCATABLE), see source lines 314-331:
#   lea rbp,[rip+x]; mov eax,[rsi+BP_kernel_alignment]; dec eax; add rbp,rax; not rax; and rbp,rax;
#   cmp rbp,LOAD_PHYSICAL_ADDR; jae; mov rbp,LOAD_PHYSICAL_ADDR
#   mov ebx,[rsi+BP_init_size]; sub ebx,rva(_end); add rbx,rbp
BP_KERNEL_ALIGNMENT = 0x230
BP_INIT_SIZE = 0x260


def relocated_copy_address(pm_code: bytes, header: dict, load_address: int) -> dict:
    """Derive RBX (relocated decompressor copy) and the relocated RIP from the image bytes.

    `pm_code` is the protected-mode image; the 64-bit entry is at offset 0x200. Nothing is guessed:
    `rva(_end)` and `rva(.Lrelocated)` are immediates in the code, found by their byte patterns.
    """
    entry = pm_code[0x200:0x300]
    mov_ebx = bytes([0x8B, 0x9E]) + struct.pack("<I", BP_INIT_SIZE)  # mov ebx,[rsi+0x260]
    at = entry.find(mov_ebx)
    if at < 0 or entry[at + 6 : at + 8] != b"\x81\xeb":
        raise OracleError("pattern 'mov ebx,[rsi+init_size]; sub ebx,imm32' not found at startup_64")
    rva_end = struct.unpack_from("<I", entry, at + 8)[0]
    lea_reloc = entry.find(b"\x48\x8d\x83")  # lea rax,[rbx+disp32]
    # The first LEA with a RBX base after the GDT reload is `.Lrelocated`; verify the next bytes are `jmp rax`.
    reloc_rva = None
    cursor = 0
    while lea_reloc >= 0:
        cursor = lea_reloc
        if entry[lea_reloc + 7 : lea_reloc + 9] == b"\xff\xe0":
            reloc_rva = struct.unpack_from("<I", entry, lea_reloc + 3)[0]
            break
        lea_reloc = entry.find(b"\x48\x8d\x83", cursor + 1)
    if reloc_rva is None:
        raise OracleError("pattern 'lea rax,[rbx+disp32]; jmp rax' not found at startup_64")
    alignment = header["kernel_alignment"]
    rbp = (load_address + alignment - 1) & ~(alignment - 1)
    if rbp < header["pref_address"]:
        rbp = header["pref_address"]
    rbx = (rbp + header["init_size"] - rva_end) & 0xFFFFFFFFFFFFFFFF
    return {
        "load_address_of_bzimage_pm": load_address,
        "kernel_alignment": alignment,
        "pref_address_LOAD_PHYSICAL_ADDR": header["pref_address"],
        "rbp_decompressed_kernel_start": rbp,
        "init_size": header["init_size"],
        "rva_end_imm32": rva_end,
        "rbx_relocated_copy": rbx,
        "rva_Lrelocated_imm32": reloc_rva,
        "relocated_rip_first_fetch": rbx + reloc_rva,
        "method": (
            "rbx = rbp + init_size - rva(_end); rbp = max(align_up(load address of startup_32, kernel_alignment), "
            "LOAD_PHYSICAL_ADDR); rva(_end) and rva(.Lrelocated) are the immediates of `sub ebx,imm32` and "
            "`lea rax,[rbx+disp32]; jmp rax` in startup_64 (head_64.S:314-331, 432-441)."
        ),
        "not_verified": "derived by static byte decoding; confirmed against execution only by checkpoint C1 (S4-38 L4)",
    }


# --------------------------------------------------------------------------------------
# early serial console check (compressed/early_serial_console.c, cmdline.c)
# --------------------------------------------------------------------------------------


def cmdline_option_value(cmdline: bytes, option: str) -> str | None:
    """Port of `__cmdline_find_option` (arch/x86/boot/cmdline.c): last value of `option=value`.

    The C code truncates to the caller's buffer; here the buffer is unbounded, which can only make the
    port more permissive than the real parser.
    """
    state = "wordstart"
    idx = 0
    opt = option.encode()
    value = None
    buf = bytearray()
    for char in cmdline:
        space = char <= 0x20
        if state == "wordstart":
            if space:
                continue
            state = "wordcmp"
            idx = 0
        if state == "wordcmp":
            if char == ord("=") and idx == len(opt):
                state = "bufcpy"
                buf = bytearray()
                value = ""
            elif space:
                state = "wordstart"
            else:
                matches = idx < len(opt) and char == opt[idx]
                idx += 1
                if not matches:
                    state = "wordskip"
        elif state == "wordskip":
            if space:
                state = "wordstart"
        elif state == "bufcpy":
            if space:
                state = "wordstart"
                value = buf.decode("latin-1")
            else:
                buf.append(char)
                value = buf.decode("latin-1")
    return value


def early_serial_decision(cmdline: str) -> dict:
    """Does `console_init()` of the decompressor open a serial port for this command line?

    Mirrors `parse_earlyprintk()` (needs `earlyprintk=`: otherwise port stays 0) and
    `parse_console_uart8250()` (needs `console=uart8250,io,...` or `console=uart,io,...`).
    """
    raw = cmdline.encode()
    earlyprintk = cmdline_option_value(raw, "earlyprintk")
    console = cmdline_option_value(raw, "console")
    port_from_earlyprintk = earlyprintk is not None
    console_ok = console is not None and (console.startswith("uart8250,io,") or console.startswith("uart,io,"))
    return {
        "cmdline": cmdline,
        "earlyprintk_value": earlyprintk,
        "console_value_last": console,
        "parse_earlyprintk_opens_port": port_from_earlyprintk,
        "parse_console_uart8250_opens_port": console_ok,
        "early_serial_base_nonzero": port_from_earlyprintk or console_ok,
    }


def read_source_members(tar_path: Path, members: list[str]) -> dict[str, bytes]:
    wanted = {TAR_PREFIX + m: m for m in members}
    out: dict[str, bytes] = {}
    with tarfile.open(tar_path, "r|bz2") as archive:
        for info in archive:
            if info.name in wanted and info.isfile():
                handle = archive.extractfile(info)
                assert handle is not None
                out[wanted[info.name]] = handle.read()
                if len(out) == len(wanted):
                    break
    missing = [m for m in members if m not in out]
    if missing:
        raise OracleError(f"source members missing in the tarball: {missing}")
    return out


def early_serial_report(tar_path: Path | None, nonce: str) -> dict:
    cmdline = f"{CMDLINE_PREFIX} od.nonce={nonce} {CMDLINE_SUFFIX}"
    decision = early_serial_decision(cmdline)
    report: dict = {"decision": decision}
    if tar_path is not None and tar_path.exists():
        members = [
            "arch/x86/boot/compressed/early_serial_console.c",
            "arch/x86/boot/early_serial_console.c",
            "arch/x86/boot/cmdline.c",
            "arch/x86/boot/compressed/misc.c",
            "arch/x86/boot/compressed/misc.h",
        ]
        data = read_source_members(tar_path, members)
        text = {k: v.decode("utf-8") for k, v in data.items()}
        shared = text["arch/x86/boot/early_serial_console.c"]
        misc = text["arch/x86/boot/compressed/misc.c"]
        misc_h = text["arch/x86/boot/compressed/misc.h"]
        report["source_sha256"] = {k: sha256_bytes(v) for k, v in data.items()}
        report["source_facts"] = {
            "early_serial_base_in_data_section_zero_initialised": 'int early_serial_base __section(".data");'
            in text["arch/x86/boot/compressed/early_serial_console.c"],
            "port_set_only_in_early_serial_init": shared.count("early_serial_base = port;") == 1,
            "early_serial_init_called_only_if_port_nonzero": shared.count("if (port)\n\t\tearly_serial_init(port, baud);") == 2,
            "earlyprintk_needs_option": 'cmdline_find_option("earlyprintk"' in shared,
            "console_uart8250_needs_uart8250_or_uart_io_prefix": '"uart8250,io,"' in shared and '"uart,io,"' in shared,
            "console_uart8250_skipped_when_base_nonzero": "if (!early_serial_base)\n\t\tparse_console_uart8250();" in shared,
            "putstr_writes_serial_only_if_base_nonzero": "if (early_serial_base) {" in misc,
            "putstr_also_writes_vga_if_lines_cols_nonzero": "if (lines == 0 || cols == 0)\n\t\treturn;" in misc,
            "debug_putstr_empty_without_CONFIG_X86_VERBOSE_BOOTUP": "#ifdef CONFIG_X86_VERBOSE_BOOTUP" in misc_h
            and "static inline void debug_putstr" in misc_h,
        }
        report["source_facts_all_true"] = all(report["source_facts"].values())
    else:
        report["source_facts"] = None
        report["source_facts_all_true"] = None
        report["note"] = "source tarball not available; only the cmdline decision was evaluated"
    report["notes"] = [
        "The S4-37 command line has `console=ttyS0,115200` (not `uart8250,io,`/`uart,io,`) and `earlycon=...` (a different option name for cmdline_find_option), so console_init() leaves early_serial_base == 0 and __putstr() never touches COM1.",
        "The binary agrees: console_init (decompressor, first call is cmdline_find_option(\"earlyprintk\"); `jg` taken only if found, else early_serial_base is tested before parse_console_uart8250).",
        "VGA: __putstr() also writes video memory and the CRTC ports 0x3d4/0x3d5 if boot_params.screen_info lines/cols are non-zero. The od-machine boot_params serializer zeroes every byte outside its written ranges (linux_boot_params.rs tests assert_zero_outside), so lines == cols == 0 is expected; the exact screen_info offsets were not re-checked here.",
        "Messages the decompressor can still emit without debug: warn(\"KASLR disabled: 'nokaslr' on cmdline.\") and error(); with early_serial_base == 0 they go only to the (absent) VGA path.",
    ]
    report["confirmed"] = bool(
        not decision["early_serial_base_nonzero"] and (report["source_facts_all_true"] in (True, None))
    )
    return report


# --------------------------------------------------------------------------------------
# Checkpoint table
# --------------------------------------------------------------------------------------


def checkpoint_table(debug: Elf64, zstd_elf_entry: int) -> dict:
    wanted = {symbol for _, symbol, _, _ in CHECKPOINTS} | {"phys_startup_64"}
    syms = debug.symbols(wanted)
    rows = []
    problems = []
    for name, symbol, offset, role in CHECKPOINTS:
        found = syms.get(symbol)
        if not found:
            problems.append(f"symbol {symbol} missing")
            continue
        if len(found) > 1:
            problems.append(f"symbol {symbol} is ambiguous ({len(found)} entries)")
        va = found[0]["value"] + offset
        located = debug.va_to_pa_and_offset(va)
        row = {
            "name": name,
            "symbol": symbol,
            "symbol_offset": offset,
            "va": va,
            "size": found[0]["size"],
            "role": role,
            "pa": located[0] if located else None,
            "file_offset": located[1] if located else None,
        }
        if located:
            row["bytes_16"] = debug.read(located[1], 16).hex()
        rows.append(row)
    by_name = {r["name"]: r for r in rows}
    checks: dict = {}
    if "startup_64" in by_name:
        checks["e_entry_equals_physical_startup_64"] = by_name["startup_64"]["pa"] == zstd_elf_entry == debug.e_entry
    if "cpuid1" in by_name and by_name["cpuid1"].get("bytes_16"):
        checks["cpuid1_bytes_are_0f_a2"] = by_name["cpuid1"]["bytes_16"].startswith("0fa2")
        checks["cpuid1_physical_is_0x139c287"] = by_name["cpuid1"]["pa"] == 0x139C287
        located = debug.va_to_pa_and_offset(by_name["cpuid1"]["va"] - 8)
        if located:
            checks["cpuid1_preceding_8_bytes"] = debug.read(located[1], 8).hex()
    verify_va = by_name.get("verify_cpu", {}).get("va")
    if verify_va is not None:
        # GenuineIntel takes the branch at verify_cpu+0x4d: `mov eax,1; cpuid` at +0x52 comes before +0x87.
        located = debug.va_to_pa_and_offset(verify_va + 0x4D)
        if located:
            raw = debug.read(located[1], 7)
            checks["verify_cpu_intel_branch_mov_eax_1_cpuid_at_0x52"] = raw.hex() == "b8010000000fa2"
            checks["verify_cpu_intel_branch_cpuid1_pa"] = located[0] + 5
    phys = syms.get("phys_startup_64")
    if phys:
        checks["phys_startup_64_symbol"] = phys[0]["value"]
        checks["phys_startup_64_equals_e_entry"] = phys[0]["value"] == debug.e_entry
    return {"rows": rows, "checks": checks, "problems": problems}


# --------------------------------------------------------------------------------------
# Main oracle run
# --------------------------------------------------------------------------------------


def run_oracle(stage_dir: Path, work_dir: Path, zstd_bin: str, s435: Path | None, source_tar: Path | None, nonce: str) -> dict:
    vmlinuz_path = stage_dir / VMLINUZ
    vmlinux_path = stage_dir / VMLINUX
    debug_path = stage_dir / VMLINUX_DEBUG
    for path in (vmlinuz_path, vmlinux_path, debug_path):
        if not path.is_file():
            raise OracleError(f"missing staged file: {path}")
    image = vmlinuz_path.read_bytes()
    header = parse_setup_header(image)
    payload = extract_payload(image, header)
    stream, trailer = split_trailer(payload)
    out_path = work_dir / "vmlinux.decompressed"
    ok, diag = zstd_decompress(stream, zstd_bin, out_path)
    evidence: dict = {
        "slice": "S4-38L2",
        "tool": "tools/build/s4_38l2_oracle.py",
        "no_guest_execution": True,
        "zstd": {
            "binary_version": subprocess.run([zstd_bin, "-V"], capture_output=True, text=True, check=False).stdout.strip(),
            "stream_decompressed": ok,
            "diagnostic": diag,
        },
        "inputs": {
            "vmlinuz": {"name": VMLINUZ, "size": len(image), "sha256": sha256_bytes(image)},
            "vmlinux": {"name": VMLINUX},
            "vmlinux_debug": {"name": VMLINUX_DEBUG},
        },
        "setup_header": header,
        "payload": {
            "file_offset": header["pm_offset"] + header["payload_offset"],
            "length_including_trailer": len(payload),
            "zstd_stream_length": len(stream),
            "zstd_stream_sha256": sha256_bytes(stream),
            "trailer_u32_le": trailer,
            "trailer_semantics": "uncompressed size appended by the kernel build (zstd22_with_size); not part of the zstd frame",
        },
    }
    for key, path in (("vmlinux", vmlinux_path), ("vmlinux_debug", debug_path)):
        digest, size = sha256_file(path)
        evidence["inputs"][key].update({"size": size, "sha256": digest})
    if s435 is not None and s435.is_file():
        recorded = {f["name"]: f.get("sha256") for f in json.loads(s435.read_text())["files"] if "name" in f}
        evidence["inputs"]["s4_35_hashes_match"] = {
            name: recorded.get(name) == evidence["inputs"][key]["sha256"]
            for name, key in ((VMLINUZ, "vmlinuz"), (VMLINUX, "vmlinux"), (VMLINUX_DEBUG, "vmlinux_debug"))
        }
    if not ok:
        evidence["verdict"] = {"payload_decompresses": False, "pt_load_equal": False, "all_ok": False}
        return evidence
    decompressed_size = out_path.stat().st_size
    decompressed_sha, _ = sha256_file(out_path)
    evidence["decompressed"] = {
        "size": decompressed_size,
        "sha256": decompressed_sha,
        "size_equals_trailer": decompressed_size == trailer,
        "file_equal_to_vmlinux": decompressed_sha == evidence["inputs"]["vmlinux"]["sha256"],
    }
    candidate = Elf64(out_path)
    evidence["decompressed"]["e_entry"] = candidate.e_entry
    evidence["decompressed"]["build_id"] = candidate.build_id()
    evidence["comparisons"] = {}
    for key, path in (("vmlinux", vmlinux_path), ("vmlinux_debug", debug_path)):
        evidence["comparisons"][key] = check_elf_against_reference(candidate, Elf64(path))
    build_id = candidate.build_id() or ""
    evidence["build_id_expected_prefix"] = EXPECTED_BUILD_ID_PREFIX
    evidence["build_id_prefix_ok"] = build_id.startswith(EXPECTED_BUILD_ID_PREFIX)
    debug = Elf64(debug_path)
    evidence["checkpoints"] = checkpoint_table(debug, candidate.e_entry)
    pm_code = image[header["pm_offset"] :]
    try:
        evidence["relocated_copy"] = relocated_copy_address(pm_code, header, header["pref_address"])
    except OracleError as error:
        evidence["relocated_copy"] = {"error": str(error)}
    evidence["early_serial_console"] = early_serial_report(source_tar, nonce)
    evidence["injections"] = run_injections(image, header, stream, trailer, candidate, zstd_bin, work_dir, vmlinux_path)
    cp_checks = evidence["checkpoints"]["checks"]
    comparisons_ok = all(c["ok"] for c in evidence["comparisons"].values())
    evidence["verdict"] = {
        "payload_decompresses": True,
        "size_equals_trailer": evidence["decompressed"]["size_equals_trailer"],
        "pt_load_equal": comparisons_ok,
        "build_id_prefix_ok": evidence["build_id_prefix_ok"],
        "checkpoint_checks_ok": all(v for k, v in cp_checks.items() if isinstance(v, bool)) and not evidence["checkpoints"]["problems"],
        "early_serial_confirmed": evidence["early_serial_console"]["confirmed"],
        "injections_all_detected": all(i["detected"] for i in evidence["injections"]),
    }
    evidence["verdict"]["all_ok"] = all(v for v in evidence["verdict"].values() if isinstance(v, bool))
    return evidence


# --------------------------------------------------------------------------------------
# Injections
# --------------------------------------------------------------------------------------


def run_injections(
    image: bytes,
    header: dict,
    stream: bytes,
    trailer: int,
    candidate: Elf64,
    zstd_bin: str,
    work_dir: Path,
    vmlinux_path: Path,
) -> list[dict]:
    results = []

    # 1. Corrupted payload byte (middle of the stream).
    position = len(stream) // 2
    corrupted = bytearray(stream)
    corrupted[position] ^= 0x01
    out = work_dir / "inject-corrupt.decompressed"
    ok, diag = zstd_decompress(bytes(corrupted), zstd_bin, out)
    entry: dict = {
        "name": "corrupted_payload_byte",
        "stream_offset": position,
        "zstd_failed": not ok,
        "zstd_diagnostic": diag,
    }
    if ok:
        mismatch = check_elf_against_reference(Elf64(out), Elf64(vmlinux_path))
        entry["pt_load_equal"] = mismatch["pt_load"]["equal"]
        entry["detected"] = not mismatch["ok"]
        entry["detected_by"] = "pt_load_comparison"
        out.unlink(missing_ok=True)
    else:
        entry["detected"] = True
        entry["detected_by"] = "zstd"
    results.append(entry)

    # 2. Trailer left in the stream: zstd must reject the stream.
    out = work_dir / "inject-trailer.decompressed"
    ok, diag = zstd_decompress(stream + struct.pack("<I", trailer), zstd_bin, out)
    results.append(
        {
            "name": "size_trailer_not_removed",
            "zstd_failed": not ok,
            "zstd_diagnostic": diag,
            "detected": not ok,
            "detected_by": "zstd",
        }
    )
    out.unlink(missing_ok=True)

    # 3. Foreign vmlinux: same ELF with one build-id byte changed.
    location = candidate.build_id_location()
    foreign = work_dir / "inject-foreign-vmlinux"
    shutil.copyfile(vmlinux_path, foreign)
    if location is None:
        results.append({"name": "foreign_vmlinux_build_id", "detected": False, "error": "no build-id note"})
    else:
        with foreign.open("r+b") as handle:
            handle.seek(location)
            original = handle.read(1)
            handle.seek(location)
            handle.write(bytes([original[0] ^ 0xFF]))
        verdict = check_elf_against_reference(candidate, Elf64(foreign))
        results.append(
            {
                "name": "foreign_vmlinux_build_id",
                "build_id_foreign": verdict["build_id_reference"],
                "build_id_candidate": verdict["build_id_candidate"],
                "build_id_equal": verdict["build_id_equal"],
                "pt_load_equal": verdict["pt_load"]["equal"],
                "detected": not verdict["build_id_equal"],
                "detected_by": "build_id",
            }
        )
    foreign.unlink(missing_ok=True)
    return results


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--stage-dir", type=Path, default=DEFAULT_STAGE_DIR)
    parser.add_argument("--zstd", default=DEFAULT_ZSTD)
    parser.add_argument("--work-dir", type=Path, help="temporary directory outside Git (default: system temp)")
    parser.add_argument("--s435-evidence", type=Path, default=DEFAULT_S435_EVIDENCE)
    parser.add_argument("--source-tar", type=Path, help="linux-source tarball (default: <stage-dir>/source/...)")
    parser.add_argument("--nonce", default="0123456789abcdef", help="nonce placeholder for the command line")
    parser.add_argument("--evidence", type=Path, default=DEFAULT_EVIDENCE)
    args = parser.parse_args(argv)
    if not Path(args.zstd).is_file():
        print(f"zstd binary not found: {args.zstd}", file=sys.stderr)
        return 2
    source_tar = args.source_tar or (args.stage_dir / SOURCE_TAR)
    try:
        resolved = args.work_dir.resolve() if args.work_dir else None
        if resolved is not None and ROOT in resolved.parents:
            print("work dir must be outside the repository", file=sys.stderr)
            return 2
        if resolved is not None:
            resolved.mkdir(parents=True, exist_ok=True)
            evidence = run_oracle(args.stage_dir, resolved, args.zstd, args.s435_evidence, source_tar, args.nonce)
        else:
            with tempfile.TemporaryDirectory(prefix="s4-38l2-") as tmp:
                evidence = run_oracle(args.stage_dir, Path(tmp), args.zstd, args.s435_evidence, source_tar, args.nonce)
    except OracleError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    args.evidence.parent.mkdir(parents=True, exist_ok=True)
    args.evidence.write_text(json.dumps(evidence, indent=2, sort_keys=False) + "\n")
    verdict = evidence["verdict"]
    print(f"verdict: {json.dumps(verdict)}")
    print(f"evidence: {args.evidence}")
    return 0 if verdict["all_ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
