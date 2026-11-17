#!/usr/bin/env python3
"""Build the Stage6SmpProbeV1 guest image (S6-16e): INIT/SIPI start of an application processor.

The image is encoded by hand in this file. No assembler, compiler or linker is involved, so two
builds are byte for byte the same. The raw image covers GPA 0x2000..0xD000 (0xB000 bytes) and is
placed there by the loader (`crates/od-machine/src/stage6_smp_probe.rs`). The nonce of the run is
NOT in the image: the loader writes a 64-byte info block at GPA 0x7000
(`magic u32, version u32, vcpu_count u32, nonce_len u32, nonce[16..64]` zero padded) and the guest
reads it, so one image serves every nonce and every processor count.

Protocol (docs/plans/STAGE_6.md, S6-16, items 1-3; SMP_CONTRACT.md 5; Intel SDM Vol. 3A 11.4.4
"MP initialization protocol" and 10.6 "Issuing interprocessor interrupts"):

* BSP (64-bit, CPL0, IF=0, own tables): checks the info block, enables its Local APIC (SVR bit 8),
  copies the nonce into its marker, prints `BSP<id> UP <nonce>`. With `vcpu_count == 1` it prints
  `SMP PROBE AP ABSENT <nonce>` and halts: the probe never reports success without an AP.
* Otherwise it sends INIT assert and INIT deassert (ICR, destination APIC ID 1), waits W iterations
  while the cell "AP entered" (the trampoline's `LOCK BTS` cell) stays 0 (an AP that ran before SIPI
  is `FAIL EARLY`), sends SIPI with the vector of the trampoline page and waits for the flag "AP
  up" for at most T iterations (timeout: `FAIL TIMEOUT`). Limit of T = 0x10000: the BSP executes
  about 327 000 steps while it waits, so with a per-turn quantum of about 327 000 steps or more the
  BSP can finish the wait before the AP prints and the probe reports `FAIL TIMEOUT` (never a false
  OK). The order of UART lines and the OK are deterministic for quanta up to about 300 000 steps;
  raising T needs a bound from the maximum quantum of the policy (S6-16f).
* AP: trampoline (real mode -> protected mode -> IA-32e, forms of S6-14a only), then in 64-bit
  mode: enables its Local APIC, reads its APIC ID (`0xFEE00020`, `1 << 24` for AP 1), stores the
  marker with the nonce in RAM, increments the start counter, prints `AP<id> UP <nonce>`, sets the
  flag, `STI; HLT`. The AP prints BEFORE the flag and the BSP prints the result AFTER it, so the
  order of UART lines does not depend on the schedule.
* BSP after the flag: counter == 1 and the marker of the AP are checked; `SMP PROBE OK <nonce>`.

Variants exist only to prove that broken guests fail the checks (`--variant`):
`no_long_mode` (the trampoline never enters long mode: the model refuses, `ApLegacyRefused`) and
`no_flag` (the AP never raises the flag: the BSP times out).

The manifest records the expected UART transcript and expected memory cells, derived from the
protocol above and never from a run of the machine.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import struct
import sys
from pathlib import Path

IMAGE_BASE = 0x2000
IMAGE_SIZE = 0xB000

PML4, PDPT, PD, PD_L, PT_L = 0x2000, 0x3000, 0x4000, 0x5000, 0x6000
INFO = 0x7000
REAL_AT = 0x8000  # SIPI vector 0x08: the AP starts at 0800:0000
SIPI_VECTOR = 0x08
LOCK = 0x8100  # "AP entered": LOCK BTS operand of the trampoline
GDTR_AT = 0x8130
IDTR_AT = 0x8140
PROTECTED_AT = 0x8200
COMPAT_AT = 0x8300
CELLS_AT = 0x8400  # EFER low, EFER high, JUMP_CELL
JUMP_CELL = 0x8408
GDT_AT = 0x9000
STR_AT = 0x9100
BSP_CODE = 0xA000
AP_CODE = 0xB000
DATA = 0xC000
FLAG = DATA + 0x08  # "AP up"
COUNT = DATA + 0x10  # start counter
MARKER_BSP = DATA + 0x40
MARKER_AP = DATA + 0x80
BSP_STACK_TOP = 0xE000
AP_STACK_TOP = 0xF000

LAPIC = 0xFEE00000
UART = 0x3F8
INFO_MAGIC = 0x3650_4D53  # "SMP6"
INFO_VERSION = 1
MARKER_MAGIC = 0x4B52_414D  # "MARK"
NONCE_MAX = 47  # bytes 16..64 of the info block; one NUL terminates the nonce
WAIT_INIT = 64  # W: iterations between INIT and SIPI
WAIT_UP = 0x1_0000  # T: iterations waiting for the flag (~327 000 BSP steps; OK only for quanta <= ~300 000)
ICR_INIT_ASSERT = (5 << 8) | (3 << 14)  # delivery INIT, level assert, level triggered
ICR_INIT_DEASSERT = (5 << 8) | (1 << 15)  # delivery INIT, level 0, level triggered
ICR_SIPI = (6 << 8) | SIPI_VECTOR

VARIANTS = ("normal", "no_long_mode", "no_flag")

# Segment descriptors (Intel SDM Vol. 3A 3.4.5): null, flat 32-bit code, 64-bit code (L=1), flat
# data with the accessed bit clear (the load into SS sets it through the tracked path).
GDT = bytes(8) + bytes([0xFF, 0xFF, 0, 0, 0, 0x9B, 0xCF, 0]) + bytes([0xFF, 0xFF, 0, 0, 0, 0x9B, 0xAF, 0]) \
    + bytes([0xFF, 0xFF, 0, 0, 0, 0x92, 0xCF, 0])

MESSAGES = {
    "bsp": "BSP",
    "ap": "AP",
    "up": " UP ",
    "nl": "\n",
    "ok": "SMP PROBE OK ",
    "absent": "SMP PROBE AP ABSENT ",
    "early": "SMP PROBE FAIL EARLY ",
    "timeout": "SMP PROBE FAIL TIMEOUT ",
    "count": "SMP PROBE FAIL COUNT ",
    "marker": "SMP PROBE FAIL MARKER ",
    "info": "SMP PROBE FAIL INFO\n",
}


class BuildError(Exception):
    pass


def le32(value: int) -> bytes:
    return struct.pack("<I", value & 0xFFFF_FFFF)


class Asm:
    """A one-pass assembler of hand-encoded instructions with 8-bit jump labels."""

    def __init__(self, base: int) -> None:
        self.base = base
        self.items: list[tuple] = []  # ("op", text, bytes) | ("label", name) | ("jump", text, opcode, label)

    def op(self, text: str, data: bytes) -> None:
        self.items.append(("op", text, data))

    def label(self, name: str) -> None:
        self.items.append(("label", name))

    def jump(self, text: str, opcode: int, label: str) -> None:
        self.items.append(("jump", text, opcode, label))

    def jump32(self, text: str, opcode: bytes, label: str) -> None:
        self.items.append(("jump32", text, opcode, label))

    def assemble(self) -> tuple[bytes, list[dict], dict[str, int]]:
        offsets: dict[str, int] = {}
        position = 0
        for item in self.items:
            if item[0] == "label":
                if item[1] in offsets:
                    raise BuildError(f"duplicate label {item[1]}")
                offsets[item[1]] = position
            else:
                position += {"op": lambda: len(item[2]), "jump": lambda: 2, "jump32": lambda: len(item[2]) + 4}[item[0]]()
        code = bytearray()
        listing = []
        for item in self.items:
            if item[0] == "label":
                continue
            at = self.base + len(code)
            if item[0] == "op":
                data = item[2]
            elif item[0] == "jump32":
                if item[3] not in offsets:
                    raise BuildError(f"unknown label {item[3]}")
                size = len(item[2]) + 4
                data = item[2] + struct.pack("<i", offsets[item[3]] - (len(code) + size))
            else:
                if item[3] not in offsets:
                    raise BuildError(f"unknown label {item[3]}")
                rel = offsets[item[3]] - (len(code) + 2)
                if not -128 <= rel <= 127:
                    raise BuildError(f"jump to {item[3]} out of rel8 range: {rel}")
                data = bytes([item[2], rel & 0xFF])
            listing.append({"gpa": f"0x{at:x}", "bytes": data.hex(), "asm": item[1]})
            code += data
        return bytes(code), listing, {name: self.base + off for name, off in offsets.items()}


def strings_blob() -> tuple[bytes, dict[str, int]]:
    blob = bytearray()
    where = {}
    for key, text in MESSAGES.items():
        where[key] = STR_AT + len(blob)
        blob += text.encode("ascii") + b"\0"
    return bytes(blob), where


# ---- long-mode code: only forms of the CPL0 set (CPU_CONTRACT.md, S6-16e) -------------------------


def mov_r32(asm: Asm, reg: str, value: int) -> None:
    opcode = {"eax": 0xB8, "ecx": 0xB9, "edx": 0xBA, "ebx": 0xBB, "ebp": 0xBD, "esi": 0xBE, "edi": 0xBF}[reg]
    asm.op(f"mov {reg}, 0x{value:x}", bytes([opcode]) + le32(value))


def store_ebx(asm: Asm, disp: int) -> None:
    asm.op(f"mov [rbx+0x{disp:x}], eax", bytes([0x89, 0x83]) + le32(disp))


def load_ebx(asm: Asm, disp: int) -> None:
    asm.op(f"mov eax, [rbx+0x{disp:x}]", bytes([0x8B, 0x83]) + le32(disp))


def cmp_eax(asm: Asm, value: int) -> None:
    asm.op(f"cmp eax, 0x{value:x}", bytes([0x83, 0xF8, value]) if value < 0x80 else bytes([0x3D]) + le32(value))


def print_string(asm: Asm, where: int, tag: str) -> None:
    """Print the NUL-terminated string at `where` to COM1 (EDX = 0x3F8)."""
    mov_r32(asm, "esi", where)
    asm.label(f"{tag}_loop")
    asm.op("mov al, [rsi]", bytes([0x8A, 0x06]))
    asm.op("test al, al", bytes([0x84, 0xC0]))
    asm.jump("jz done", 0x74, f"{tag}_done")
    asm.op("out dx, al", bytes([0xEE]))
    asm.op("inc esi", bytes([0xFF, 0xC6]))
    asm.jump("jmp loop", 0xEB, f"{tag}_loop")
    asm.label(f"{tag}_done")


def print_id_digit(asm: Asm) -> None:
    """Print the APIC ID of this processor as one decimal digit (ID < 10)."""
    load_ebx(asm, 0x20)
    asm.op("shr eax, 24", bytes([0xC1, 0xE8, 0x18]))
    asm.op("add eax, 0x30", bytes([0x83, 0xC0, 0x30]))
    asm.op("out dx, al", bytes([0xEE]))


def copy_nonce(asm: Asm, marker: int, tag: str) -> None:
    mov_r32(asm, "esi", INFO + 16)
    mov_r32(asm, "edi", marker + 16)
    asm.label(f"{tag}_copy")
    asm.op("mov eax, [rsi]", bytes([0x8B, 0x06]))
    asm.op("mov [rdi], eax", bytes([0x89, 0x07]))
    asm.op("add esi, 4", bytes([0x83, 0xC6, 0x04]))
    asm.op("add edi, 4", bytes([0x83, 0xC7, 0x04]))
    asm.op(f"cmp esi, 0x{INFO + 64:x}", bytes([0x81, 0xFE]) + le32(INFO + 64))
    asm.jump("jb copy", 0x72, f"{tag}_copy")


def store_marker_head(asm: Asm, marker: int) -> None:
    mov_r32(asm, "edi", marker)
    mov_r32(asm, "eax", MARKER_MAGIC)
    asm.op("mov [rdi], eax", bytes([0x89, 0x07]))
    load_ebx(asm, 0x20)
    asm.op("mov [rdi+4], eax", bytes([0x89, 0x47, 0x04]))


def halt_loop(asm: Asm, tag: str) -> None:
    asm.op("cli", bytes([0xFA]))
    asm.label(f"{tag}_hlt")
    asm.op("hlt", bytes([0xF4]))
    asm.jump("jmp hlt", 0xEB, f"{tag}_hlt")


def fail(asm: Asm, tag: str, key: str, where: dict[str, int]) -> None:
    asm.label(tag)
    print_string(asm, where[key], f"{tag}_msg")
    print_string(asm, INFO + 16, f"{tag}_nonce")
    print_string(asm, where["nl"], f"{tag}_nl")
    halt_loop(asm, f"{tag}_halt")


def bsp_code(where: dict[str, int]) -> tuple[bytes, list[dict], dict[str, int]]:
    a = Asm(BSP_CODE)
    mov_r32(a, "ebx", LAPIC)
    mov_r32(a, "edx", UART)
    mov_r32(a, "ebp", INFO)
    # The info block: magic and version.
    a.op("mov eax, [rbp+0]", bytes([0x8B, 0x45, 0x00]))
    a.op(f"cmp eax, 0x{INFO_MAGIC:x}", bytes([0x3D]) + le32(INFO_MAGIC))
    a.jump("jne bad_info", 0x75, "bad_info")
    a.op("mov eax, [rbp+4]", bytes([0x8B, 0x45, 0x04]))
    cmp_eax(a, INFO_VERSION)
    a.jump("jne bad_info", 0x75, "bad_info")
    a.jump("jmp main", 0xEB, "main")
    a.label("bad_info")
    print_string(a, where["info"], "m_info")
    halt_loop(a, "info_halt")
    a.label("main")
    # Local APIC enable, own marker, own line.
    mov_r32(a, "eax", 0x1FF)
    store_ebx(a, 0xF0)
    store_marker_head(a, MARKER_BSP)
    copy_nonce(a, MARKER_BSP, "bsp")
    print_string(a, where["bsp"], "p_bsp")
    print_id_digit(a)
    print_string(a, where["up"], "p_up")
    print_string(a, INFO + 16, "p_nonce")
    print_string(a, where["nl"], "p_nl")
    a.op("mov eax, [rbp+8]", bytes([0x8B, 0x45, 0x08]))
    cmp_eax(a, 1)
    a.jump("ja smp", 0x77, "smp")
    # One vCPU: no AP exists, the probe says so.
    print_string(a, where["absent"], "m_absent")
    print_string(a, INFO + 16, "m_absent_nonce")
    print_string(a, where["nl"], "m_absent_nl")
    halt_loop(a, "absent_halt")
    # Two or more: INIT assert, INIT deassert, wait, SIPI.
    a.label("smp")
    mov_r32(a, "eax", 1 << 24)
    store_ebx(a, 0x310)
    mov_r32(a, "eax", ICR_INIT_ASSERT)
    store_ebx(a, 0x300)
    mov_r32(a, "eax", ICR_INIT_DEASSERT)
    store_ebx(a, 0x300)
    mov_r32(a, "ecx", WAIT_INIT)
    mov_r32(a, "edi", LOCK)
    a.label("wait_init")
    a.op("mov eax, [rdi]", bytes([0x8B, 0x07]))
    a.op("test eax, eax", bytes([0x85, 0xC0]))
    a.jump("jnz fail_early", 0x75, "to_early")
    a.op("dec ecx", bytes([0xFF, 0xC9]))
    a.jump("jnz wait_init", 0x75, "wait_init")
    mov_r32(a, "eax", ICR_SIPI)
    store_ebx(a, 0x300)
    mov_r32(a, "ecx", WAIT_UP)
    mov_r32(a, "edi", FLAG)
    a.label("wait_up")
    a.op("mov eax, [rdi]", bytes([0x8B, 0x07]))
    a.op("test eax, eax", bytes([0x85, 0xC0]))
    a.jump("jnz up", 0x75, "up")
    a.op("dec ecx", bytes([0xFF, 0xC9]))
    a.jump("jnz wait_up", 0x75, "wait_up")
    a.jump32("jmp timeout", bytes([0xE9]), "timeout")
    a.label("to_early")
    a.jump32("jmp early", bytes([0xE9]), "early")
    # The AP is up: counter and marker, then the result.
    a.label("up")
    mov_r32(a, "edi", COUNT)
    a.op("mov eax, [rdi]", bytes([0x8B, 0x07]))
    cmp_eax(a, 1)
    a.jump("jne count", 0x75, "to_count")
    mov_r32(a, "edi", MARKER_AP)
    a.op("mov eax, [rdi]", bytes([0x8B, 0x07]))
    a.op(f"cmp eax, 0x{MARKER_MAGIC:x}", bytes([0x3D]) + le32(MARKER_MAGIC))
    a.jump("jne marker", 0x75, "to_marker")
    a.op("mov eax, [rdi+4]", bytes([0x8B, 0x47, 0x04]))
    a.op(f"cmp eax, 0x{1 << 24:x}", bytes([0x3D]) + le32(1 << 24))
    a.jump("jne marker", 0x75, "to_marker")
    print_string(a, where["ok"], "m_ok")
    print_string(a, INFO + 16, "m_ok_nonce")
    print_string(a, where["nl"], "m_ok_nl")
    halt_loop(a, "ok_halt")
    a.label("to_count")
    a.jump32("jmp count", bytes([0xE9]), "count")
    a.label("to_marker")
    a.jump32("jmp marker", bytes([0xE9]), "marker")
    fail(a, "early", "early", where)
    fail(a, "timeout", "timeout", where)
    fail(a, "count", "count", where)
    fail(a, "marker", "marker", where)
    return a.assemble()


def ap_code(where: dict[str, int], variant: str) -> tuple[bytes, list[dict], dict[str, int]]:
    a = Asm(AP_CODE)
    mov_r32(a, "ebx", LAPIC)
    mov_r32(a, "edx", UART)
    mov_r32(a, "eax", 0x1FF)
    store_ebx(a, 0xF0)
    store_marker_head(a, MARKER_AP)
    copy_nonce(a, MARKER_AP, "ap")
    mov_r32(a, "edi", COUNT)
    a.op("mov eax, [rdi]", bytes([0x8B, 0x07]))
    a.op("inc eax", bytes([0xFF, 0xC0]))
    a.op("mov [rdi], eax", bytes([0x89, 0x07]))
    print_string(a, where["ap"], "p_ap")
    print_id_digit(a)
    print_string(a, where["up"], "p_up")
    print_string(a, INFO + 16, "p_nonce")
    print_string(a, where["nl"], "p_nl")
    if variant != "no_flag":
        mov_r32(a, "edi", FLAG)
        mov_r32(a, "eax", 1)
        a.op("mov [rdi], eax", bytes([0x89, 0x07]))
    a.op("sti", bytes([0xFB]))
    a.label("hlt")
    a.op("hlt", bytes([0xF4]))
    a.jump("jmp hlt", 0xEB, "hlt")
    return a.assemble()


# ---- trampoline: forms of S6-14a only (the real/protected/64-bit models of od-isa) -----------------


def trampoline(variant: str) -> dict[int, tuple[bytes, list[tuple[str, bytes]]]]:
    """Blocks of the trampoline: {gpa: (bytes, [(asm, bytes)])}."""
    real = [
        ("cli", bytes([0xFA])),
        ("mov ax, cs", bytes([0x8C, 0xC8])),
        ("mov ds, ax", bytes([0x8E, 0xD8])),
        ("mov es, ax", bytes([0x8E, 0xC0])),
        ("mov ss, ax", bytes([0x8E, 0xD0])),
        ("lock bts dword ds:LOCK, 0", bytes([0x66, 0xF0, 0x0F, 0xBA, 0x2E]) + struct.pack("<H", LOCK - REAL_AT) + bytes([0x00])),
        ("jae +4", bytes([0x73, 0x04])),
        ("pause", bytes([0xF3, 0x90])),
        ("jmp lock bts", bytes([0xEB, 0xF2])),
        (f"mov esp, 0x{AP_STACK_TOP:x}", bytes([0x66, 0xBC]) + le32(AP_STACK_TOP)),
        ("lidt ds:IDTR", bytes([0x66, 0x0F, 0x01, 0x1E]) + struct.pack("<H", IDTR_AT - REAL_AT)),
        ("lgdt ds:GDTR", bytes([0x66, 0x0F, 0x01, 0x16]) + struct.pack("<H", GDTR_AT - REAL_AT)),
        ("mov dx, 0x18", bytes([0xBA, 0x18, 0x00])),
        ("mov eax, 0x50033", bytes([0x66, 0xB8]) + le32(0x50033)),
        ("mov cr0, eax", bytes([0x0F, 0x22, 0xC0])),
        (f"jmp 0x08:0x{PROTECTED_AT:x}", bytes([0x66, 0xEA]) + le32(PROTECTED_AT) + struct.pack("<H", 0x08)),
    ]
    segments = [
        ("mov ss, edx", bytes([0x8E, 0xD2])),
        ("mov ds, edx", bytes([0x8E, 0xDA])),
        ("mov es, edx", bytes([0x8E, 0xC2])),
    ]
    if variant == "no_long_mode":
        # The AP stays in protected mode: nothing enables PAE, EFER.LME or paging, nothing jumps to
        # a 64-bit code segment. By the SDM it would halt here; the model of the trampoline only
        # runs the forms of S6-14a, so the executor refuses.
        protected = segments + [("hlt", bytes([0xF4]))]
    else:
        protected = segments + [
            ("mov eax, 0x6b0", bytes([0xB8]) + le32(0x6B0)),
            ("mov cr4, eax", bytes([0x0F, 0x22, 0xE0])),
            (f"mov eax, 0x{PML4:x}", bytes([0xB8]) + le32(PML4)),
            ("mov cr3, eax", bytes([0x0F, 0x22, 0xD8])),
            ("mov ecx, 0xc0000080", bytes([0xB9, 0x80, 0x00, 0x00, 0xC0])),
            ("mov eax, ds:EFER_LO", bytes([0xA1]) + le32(CELLS_AT)),
            ("mov edx, ds:EFER_HI", bytes([0x8B, 0x15]) + le32(CELLS_AT + 4)),
            ("wrmsr", bytes([0x0F, 0x30])),
            ("mov eax, 0x80050033", bytes([0xB8]) + le32(0x8005_0033)),
            ("mov cr0, eax", bytes([0x0F, 0x22, 0xC0])),
            (f"jmp 0x10:0x{COMPAT_AT:x}", bytes([0xEA]) + le32(COMPAT_AT) + struct.pack("<H", 0x10)),
        ]
    compat = [("jmp qword [rip+JUMP_CELL]", bytes([0xFF, 0x25]) + struct.pack("<i", JUMP_CELL - (COMPAT_AT + 6)))]

    def join(items: list[tuple[str, bytes]]) -> tuple[bytes, list[tuple[str, bytes]]]:
        return b"".join(data for _, data in items), items

    return {REAL_AT: join(real), PROTECTED_AT: join(protected), COMPAT_AT: join(compat)}


def build_image(variant: str) -> tuple[bytes, dict]:
    if variant not in VARIANTS:
        raise BuildError(f"--variant must be one of {VARIANTS}")
    blob, where = strings_blob()
    bsp, bsp_listing, bsp_labels = bsp_code(where)
    ap, ap_listing, ap_labels = ap_code(where, variant)
    memory: dict[int, bytes] = {
        PML4: struct.pack("<Q", PDPT | 3),
        PDPT: struct.pack("<Q", PD | 3) + bytes(16) + struct.pack("<Q", PD_L | 3),
        PD: struct.pack("<Q", 0x83),
        PD_L + 0x1F7 * 8: struct.pack("<Q", PT_L | 3),
        PT_L: struct.pack("<Q", LAPIC | 0x13),
        GDTR_AT: struct.pack("<HI", len(GDT) - 1, GDT_AT),
        IDTR_AT: struct.pack("<HI", 0, 0),
        CELLS_AT: le32(0x901) + le32(0) + struct.pack("<Q", AP_CODE),  # EFER: SCE | LME | NXE
        GDT_AT: GDT,
        STR_AT: blob,
        BSP_CODE: bsp,
        AP_CODE: ap,
    }
    tramp = trampoline(variant)
    for gpa, (data, _) in tramp.items():
        memory[gpa] = data
    raw = bytearray(IMAGE_SIZE)
    for gpa, data in memory.items():
        if gpa < IMAGE_BASE or gpa + len(data) > IMAGE_BASE + IMAGE_SIZE:
            raise BuildError(f"block at 0x{gpa:x} is outside the image")
        start = gpa - IMAGE_BASE
        if any(raw[start : start + len(data)]):
            raise BuildError(f"block at 0x{gpa:x} overlaps another block")
        raw[start : start + len(data)] = data
    listing = {
        "trampoline_real": [{"asm": a, "bytes": d.hex()} for a, d in tramp[REAL_AT][1]],
        "trampoline_protected": [{"asm": a, "bytes": d.hex()} for a, d in tramp[PROTECTED_AT][1]],
        "trampoline_long_entry": [{"asm": a, "bytes": d.hex()} for a, d in tramp[COMPAT_AT][1]],
        "bsp": bsp_listing,
        "ap": ap_listing,
    }
    return bytes(raw), {"listing": listing, "bsp_labels": bsp_labels, "ap_labels": ap_labels, "strings": where}


# ---- expectations: derived from the protocol, not from a run --------------------------------------


def expected_uart(variant: str, vcpus: int) -> str | None:
    """The UART transcript with `{nonce}` for the nonce of the run; None when the run is refused."""
    head = "BSP0 UP {nonce}\n"
    if vcpus == 1:
        return head + "SMP PROBE AP ABSENT {nonce}\n"
    if variant == "normal":
        return head + "AP1 UP {nonce}\nSMP PROBE OK {nonce}\n"
    if variant == "no_flag":
        return head + "AP1 UP {nonce}\nSMP PROBE FAIL TIMEOUT {nonce}\n"
    return None  # no_long_mode: the AP is refused by the executor, the run ends with an error


def expected_memory(variant: str, vcpus: int) -> dict:
    """Cells after a run to the end (values; `nonce` cells hold the zero padded nonce)."""
    cells = {
        "lock_entered": {"gpa": f"0x{LOCK:x}", "value": "0x1" if vcpus > 1 else "0x0"},
        "flag_ap_up": {"gpa": f"0x{FLAG:x}", "value": "0x1" if vcpus > 1 and variant == "normal" else "0x0"},
        "start_count": {"gpa": f"0x{COUNT:x}", "value": "0x1" if vcpus > 1 and variant != "no_long_mode" else "0x0"},
        "marker_bsp": {"gpa": f"0x{MARKER_BSP:x}", "magic": f"0x{MARKER_MAGIC:x}", "apic_id_raw": "0x0", "nonce_at": f"0x{MARKER_BSP + 16:x}"},
    }
    if vcpus > 1 and variant != "no_long_mode":
        cells["marker_ap"] = {
            "gpa": f"0x{MARKER_AP:x}",
            "magic": f"0x{MARKER_MAGIC:x}",
            "apic_id_raw": f"0x{1 << 24:x}",
            "nonce_at": f"0x{MARKER_AP + 16:x}",
        }
    return cells


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def build(output: Path, variant: str) -> dict:
    raw, details = build_image(variant)
    output.mkdir(parents=True, exist_ok=True)
    (output / "probe.bin").write_bytes(raw)
    expectations = {}
    for vcpus in (1, 2):
        text = expected_uart(variant, vcpus)
        name = f"n{vcpus}"
        expectations[name] = {
            "uart_template": text,
            "memory": expected_memory(variant, vcpus),
            "run_error": None if text is not None else "ApLegacyRefused",
        }
        if text is not None:
            (output / f"expected_uart_{name}.txt").write_text(text)
    manifest = {
        "schema": "stage6-smp-probe-build-v1",
        "profile": "Stage6SmpProbeV1",
        "variant": variant,
        "method": "hand-encoded bytes; no assembler, compiler or linker",
        "layout": {
            "image_base": f"0x{IMAGE_BASE:x}",
            "image_size": f"0x{IMAGE_SIZE:x}",
            "page_tables": f"0x{PML4:x}",
            "info": f"0x{INFO:x}",
            "trampoline": f"0x{REAL_AT:x}",
            "sipi_vector": f"0x{SIPI_VECTOR:x}",
            "gdt": f"0x{GDT_AT:x}",
            "bsp_code": f"0x{BSP_CODE:x}",
            "ap_code": f"0x{AP_CODE:x}",
            "data": f"0x{DATA:x}",
            "bsp_stack_top": f"0x{BSP_STACK_TOP:x}",
        },
        "info_block": {
            "size": 64,
            "fields": "magic u32 @0 (SMP6 = 0x36504d53), version u32 @4 (1), vcpu_count u32 @8, nonce_len u32 @12, nonce @16..64 (<= 47 bytes, NUL terminated)",
            "written_by": "the loader, never in the image",
        },
        "protocol": {
            "wait_init_iterations": WAIT_INIT,
            "wait_up_iterations": WAIT_UP,
            "icr_init_assert": f"0x{ICR_INIT_ASSERT:x}",
            "icr_init_deassert": f"0x{ICR_INIT_DEASSERT:x}",
            "icr_sipi": f"0x{ICR_SIPI:x}",
        },
        "expected": expectations,
        "code": details["listing"],
        "artifacts": {"probe.bin": {"size": len(raw), "sha256": sha256(raw)}},
    }
    (output / "build_manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    return manifest


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--variant", choices=VARIANTS, default="normal")
    args = parser.parse_args()
    try:
        manifest = build(Path(args.output_dir), args.variant)
    except (BuildError, OSError) as error:
        print(f"build_stage6_smp_probe: error: {error}", file=sys.stderr)
        return 1
    print(f"output: {args.output_dir}")
    print(f"raw sha256: {manifest['artifacts']['probe.bin']['sha256']}; variant: {args.variant}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
