#!/usr/bin/env python3
"""Build the Stage6SmpProbeV2 guest image (S6-16e start of an AP, S6-16f IPI exchange and negative cases).

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
  up" for at most T iterations (timeout: `FAIL TIMEOUT`). T = 0x100000 iterations is about 5.2 million
  BSP steps, so with a quantum of the policy of at most ~4 million steps the BSP cannot run the
  whole wait before the AP prints (S6-16e had T = 0x10000 and failed from a quantum of ~327 000).
* AP: trampoline (real mode -> protected mode -> IA-32e, forms of S6-14a only), then in 64-bit
  mode: enables its Local APIC, reads its APIC ID (`0xFEE00020`, `1 << 24` for AP 1), stores the
  marker with the nonce in RAM, increments the start counter, prints `AP<id> UP <nonce>`, sets the
  flag, `STI; HLT`. The AP prints BEFORE the flag and the BSP prints the result AFTER it, so the
  order of UART lines does not depend on the schedule.
* BSP after the flag: counter == 1 and the marker of the AP are checked; then the scenario of the
  build (`--scenario`, `--rounds`):
  - `normal` (S6-16e): `SMP PROBE OK <nonce>`.
  - `exchange` (S6-16f): the BSP enables interrupts and does K rounds. A round is: Fixed IPI of
    vector 0x30 to APIC ID 1, wait (at most R iterations) until the reply counter of the BSP
    reaches the round number. The AP handler of vector 0x30 increments the AP counter, writes EOI
    (`0xFEE000B0`), sends Fixed vector 0x31 to APIC ID 0 and returns with IRETQ; the BSP handler
    of 0x31 increments the BSP counter, writes EOI and returns. After K rounds both counters must
    be K and the start counter 1; `SMP PROBE OK K=<K> <nonce>`.
  - `resipi`: as `exchange`, with a second SIPI to the running AP before the first round; the
    start counter must stay 1 (the AP is not restarted, SMP_CONTRACT 5).
  - `bad-ipi`: the BSP prints `SMP PROBE BAD-IPI <nonce>` and writes a Fixed ICR to APIC ID
    `vcpu_count` (no such processor): the machine must end the run with a device error and the
    guest prints nothing after the marker (a guest that gets control back prints
    `SMP PROBE FAIL ACCEPTED`).
  - `nmi`: the BSP sends an NMI (ICR delivery mode 100b) to APIC ID 1 and waits for the AP handler
    of vector 2 to increment its cell; `SMP PROBE NMI OK <nonce>` (SDM Vol. 3A 6.7.1).
* Interrupt tables: the IDT (GPA 0x9800, vectors 0..0x3F, 64-bit interrupt gates with selector
  0x10, DPL 0) is in the image for every scenario. The AP loads its IDTR in real mode in the
  trampoline (`LIDT` with a 32-bit base, forms of S6-14a). The BSP loads its IDTR in long mode with
  the only form the CPL0 set loop executes, `LIDT [RSP+disp8]` (`0F 01 5C 24 00`): RSP is pointed at
  the 10-byte descriptor (limit u16, base u64) at GPA 0x8150 for that one instruction.

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
IDTR_BSP_AT = 0x8150  # 10-byte descriptor read by the BSP's `LIDT [RSP+0]`
PROTECTED_AT = 0x8200
COMPAT_AT = 0x8300
CELLS_AT = 0x8400  # EFER low, EFER high, JUMP_CELL
JUMP_CELL = 0x8408
GDT_AT = 0x9000
STR_AT = 0x9100
BSP_CODE = 0xA000
AP_CODE = 0xB000
IDT_AT = 0x9800  # 0x400 bytes: vectors 0..0x3F
IDT_LIMIT = 0x3FF
HANDLER_AP_AT = 0x9C00  # vector 0x30 on the AP
HANDLER_BSP_AT = 0x9D00  # vector 0x31 on the BSP
HANDLER_NMI_AT = 0x9E00  # vector 2 on the AP
DATA = 0xC000
FLAG = DATA + 0x08  # "AP up"
COUNT = DATA + 0x10  # start counter
AP_RX = DATA + 0x18  # incremented by the AP handler of vector 0x30
BSP_RX = DATA + 0x20  # incremented by the BSP handler of vector 0x31
NMI_CELL = DATA + 0x28  # incremented by the AP handler of vector 2
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
# T and R: iterations of the wait loops (`mov eax,[rdi]; test; jnz; dec ecx; jnz` = 5 BSP steps each).
# 0x10_0000 iterations are about 5.2 million BSP steps: a turn of the boot processor cannot run out
# of the wait before the AP gets its turn when the quantum of the policy is at most ~4 million
# steps (S6-16e had 0x1_0000 and failed from ~327 000). `Fixed(Q)` with Q above that is outside the
# range of the probe; the seeded policy gives at most 256.
WAIT_UP = 0x10_0000  # T: iterations waiting for the flag "AP up"
WAIT_RX = 0x10_0000  # R: iterations waiting for the reply of one round (and for the NMI cell)
VECTOR_AP = 0x30
VECTOR_BSP = 0x31
NMI_VECTOR = 2
ICR_FIXED_AP = VECTOR_AP  # delivery Fixed, physical, level 0, edge
ICR_FIXED_BSP = VECTOR_BSP
ICR_NMI = 4 << 8
ROUNDS_DEFAULT = 16
ROUNDS_MAX = 100
MAX_FIXED_QUANTUM = 4_000_000  # largest `Fixed` quantum the waits above are sized for
ICR_INIT_ASSERT = (5 << 8) | (3 << 14)  # delivery INIT, level assert, level triggered
ICR_INIT_DEASSERT = (5 << 8) | (1 << 15)  # delivery INIT, level 0, level triggered
ICR_SIPI = (6 << 8) | SIPI_VECTOR

VARIANTS = ("normal", "no_long_mode", "no_flag")
SCENARIOS = ("normal", "exchange", "resipi", "bad-ipi", "nmi")

# Segment descriptors (Intel SDM Vol. 3A 3.4.5): null, flat 32-bit code, 64-bit code (L=1), flat
# data with the accessed bit clear (the load into SS sets it through the tracked path).
GDT = bytes(8) + bytes([0xFF, 0xFF, 0, 0, 0, 0x9B, 0xCF, 0]) + bytes([0xFF, 0xFF, 0, 0, 0, 0x9B, 0xAF, 0]) \
    + bytes([0xFF, 0xFF, 0, 0, 0, 0x92, 0xCF, 0])

def messages(scenario: str = "normal", rounds: int = ROUNDS_DEFAULT) -> dict[str, str]:
    ok = "SMP PROBE OK " if scenario in ("normal", "nmi") else f"SMP PROBE OK K={rounds} "
    if scenario == "nmi":
        ok = "SMP PROBE NMI OK "
    return {
        "bsp": "BSP",
        "ap": "AP",
        "up": " UP ",
        "nl": "\n",
        "ok": ok,
        "absent": "SMP PROBE AP ABSENT ",
        "early": "SMP PROBE FAIL EARLY ",
        "timeout": "SMP PROBE FAIL TIMEOUT ",
        "count": "SMP PROBE FAIL COUNT ",
        "marker": "SMP PROBE FAIL MARKER ",
        "info": "SMP PROBE FAIL INFO\n",
        "exchange": "SMP PROBE FAIL EXCHANGE ",
        "accepted": "SMP PROBE FAIL ACCEPTED ",
        "bad_ipi": "SMP PROBE BAD-IPI ",
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


def strings_blob(scenario: str = "normal", rounds: int = ROUNDS_DEFAULT) -> tuple[bytes, dict[str, int]]:
    blob = bytearray()
    where = {}
    for key, text in messages(scenario, rounds).items():
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


def print_result(a: Asm, where: dict[str, int], tag: str) -> None:
    print_string(a, where["ok"], f"{tag}_ok")
    print_string(a, INFO + 16, f"{tag}_nonce")
    print_string(a, where["nl"], f"{tag}_nl")
    halt_loop(a, f"{tag}_halt")


def send_fixed(a: Asm, destination: int, icr_low: int) -> None:
    mov_r32(a, "eax", destination << 24)
    store_ebx(a, 0x310)
    mov_r32(a, "eax", icr_low)
    store_ebx(a, 0x300)


def scenario_normal(a: Asm, where: dict[str, int]) -> None:
    print_result(a, where, "res")


def scenario_exchange(a: Asm, where: dict[str, int], rounds: int, resipi: bool) -> None:
    if not 1 <= rounds <= ROUNDS_MAX:
        raise BuildError(f"--rounds must be 1..={ROUNDS_MAX}")
    if resipi:
        # A second start-up IPI to the AP that already runs: it must be ignored.
        send_fixed(a, 1, ICR_SIPI)
    mov_r32(a, "edi", BSP_RX)
    mov_r32(a, "ebp", 0)
    a.op("sti", bytes([0xFB]))
    a.label("round")
    send_fixed(a, 1, ICR_FIXED_AP)
    a.op("inc ebp", bytes([0xFF, 0xC5]))
    mov_r32(a, "ecx", WAIT_RX)
    a.label("wait_rx")
    a.op("mov eax, [rdi]", bytes([0x8B, 0x07]))
    a.op("cmp eax, ebp", bytes([0x39, 0xE8]))
    a.jump("je got", 0x74, "got")
    a.op("dec ecx", bytes([0xFF, 0xC9]))
    a.jump("jnz wait_rx", 0x75, "wait_rx")
    a.jump32("jmp exchange", bytes([0xE9]), "exchange")
    a.label("got")
    a.op(f"cmp ebp, {rounds}", bytes([0x83, 0xFD, rounds]))
    a.jump("jb round", 0x72, "round")
    a.op("cli", bytes([0xFA]))
    # Both counters are K, the AP started once.
    a.op("mov eax, [rdi]", bytes([0x8B, 0x07]))
    cmp_eax(a, rounds)
    a.jump32("jne exchange", bytes([0x0F, 0x85]), "exchange")
    mov_r32(a, "edi", AP_RX)
    a.op("mov eax, [rdi]", bytes([0x8B, 0x07]))
    cmp_eax(a, rounds)
    a.jump32("jne exchange", bytes([0x0F, 0x85]), "exchange")
    mov_r32(a, "edi", COUNT)
    a.op("mov eax, [rdi]", bytes([0x8B, 0x07]))
    cmp_eax(a, 1)
    a.jump32("jne count", bytes([0x0F, 0x85]), "count")
    print_result(a, where, "res")


def scenario_bad_ipi(a: Asm, where: dict[str, int]) -> None:
    print_string(a, where["bad_ipi"], "m_bad")
    print_string(a, INFO + 16, "m_bad_nonce")
    print_string(a, where["nl"], "m_bad_nl")
    # ICR_HIGH = vcpu_count << 24: the first APIC ID that does not exist.
    a.op("mov eax, [rbp+8]", bytes([0x8B, 0x45, 0x08]))
    a.op("shl eax, 24", bytes([0xC1, 0xE0, 0x18]))
    store_ebx(a, 0x310)
    mov_r32(a, "eax", ICR_FIXED_AP)
    store_ebx(a, 0x300)
    # Only reached when the machine accepted the write.
    a.jump32("jmp accepted", bytes([0xE9]), "accepted")


def scenario_nmi(a: Asm, where: dict[str, int]) -> None:
    send_fixed(a, 1, ICR_NMI)
    mov_r32(a, "ecx", WAIT_RX)
    mov_r32(a, "edi", NMI_CELL)
    a.label("wait_nmi")
    a.op("mov eax, [rdi]", bytes([0x8B, 0x07]))
    a.op("test eax, eax", bytes([0x85, 0xC0]))
    a.jump("jnz done", 0x75, "nmi_done")
    a.op("dec ecx", bytes([0xFF, 0xC9]))
    a.jump("jnz wait_nmi", 0x75, "wait_nmi")
    a.jump32("jmp timeout", bytes([0xE9]), "timeout")
    a.label("nmi_done")
    print_result(a, where, "res")


def bsp_code(where: dict[str, int], scenario: str = "normal", rounds: int = ROUNDS_DEFAULT) -> tuple[bytes, list[dict], dict[str, int]]:
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
    # IDTR of the BSP: RSP at the descriptor for one `LIDT [RSP+0]`, then back to the stack.
    a.op(f"mov esp, 0x{IDTR_BSP_AT:x}", bytes([0xBC]) + le32(IDTR_BSP_AT))
    a.op("lidt [rsp+0]", bytes([0x0F, 0x01, 0x5C, 0x24, 0x00]))
    a.op(f"mov esp, 0x{BSP_STACK_TOP:x}", bytes([0xBC]) + le32(BSP_STACK_TOP))
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
    # The AP is up: counter and marker, then the scenario.
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
    a.jump32("jmp scenario", bytes([0xE9]), "scenario")
    a.label("to_count")
    a.jump32("jmp count", bytes([0xE9]), "count")
    a.label("to_marker")
    a.jump32("jmp marker", bytes([0xE9]), "marker")
    a.label("scenario")
    {
        "normal": lambda: scenario_normal(a, where),
        "exchange": lambda: scenario_exchange(a, where, rounds, resipi=False),
        "resipi": lambda: scenario_exchange(a, where, rounds, resipi=True),
        "bad-ipi": lambda: scenario_bad_ipi(a, where),
        "nmi": lambda: scenario_nmi(a, where),
    }[scenario]()
    fail(a, "early", "early", where)
    fail(a, "timeout", "timeout", where)
    fail(a, "count", "count", where)
    fail(a, "marker", "marker", where)
    fail(a, "exchange", "exchange", where)
    fail(a, "accepted", "accepted", where)
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


# ---- interrupt handlers and the IDT: forms of the CPL0 set (the same as S6-16d tests) -------------


def count_and_eoi(counter: int) -> bytes:
    """counter += 1 (R9D), then EOI: only R8..R10, the programs do not use them."""
    return (
        bytes([0x41, 0xB8]) + le32(counter)  # mov r8d, counter
        + bytes([0x45, 0x8B, 0x08])  # mov r9d, [r8]
        + bytes([0x41, 0xFF, 0xC1])  # inc r9d
        + bytes([0x45, 0x89, 0x08])  # mov [r8], r9d
        + bytes([0x41, 0xB8]) + le32(LAPIC)  # mov r8d, LAPIC
        + bytes([0x45, 0x31, 0xD2])  # xor r10d, r10d
        + bytes([0x45, 0x89, 0x90]) + le32(0xB0)  # mov [r8+0xB0], r10d  (EOI)
    )


def send_from_r8(vector: int, destination: int) -> bytes:
    return (
        bytes([0x41, 0xBA]) + le32(destination << 24)  # mov r10d, destination << 24
        + bytes([0x45, 0x89, 0x90]) + le32(0x310)  # mov [r8+0x310], r10d
        + bytes([0x41, 0xBA]) + le32(vector)  # mov r10d, Fixed | vector
        + bytes([0x45, 0x89, 0x90]) + le32(0x300)  # mov [r8+0x300], r10d
    )


IRETQ = bytes([0x48, 0xCF])


def handlers_blob() -> dict[int, tuple[bytes, str]]:
    ap = count_and_eoi(AP_RX) + send_from_r8(ICR_FIXED_BSP, 0) + IRETQ
    bsp = count_and_eoi(BSP_RX) + IRETQ
    nmi = (
        bytes([0x41, 0xB8]) + le32(NMI_CELL)
        + bytes([0x45, 0x8B, 0x08, 0x41, 0xFF, 0xC1, 0x45, 0x89, 0x08])
        + IRETQ
    )
    return {
        HANDLER_AP_AT: (ap, "vector 0x30 on the AP: count, EOI, Fixed 0x31 to APIC ID 0, IRETQ"),
        HANDLER_BSP_AT: (bsp, "vector 0x31 on the BSP: count, EOI, IRETQ"),
        HANDLER_NMI_AT: (nmi, "vector 2 on the AP: count, IRETQ (no EOI for an NMI)"),
    }


def idt_bytes() -> bytes:
    """64-bit interrupt gates (SDM Vol. 3A 6.14.1): offset 15:0, selector 0x10, IST 0, type 0x8E
    (present, DPL 0, interrupt gate), offset 31:16, offset 63:32, reserved."""
    table = bytearray(IDT_LIMIT + 1)
    for vector, handler in ((NMI_VECTOR, HANDLER_NMI_AT), (VECTOR_AP, HANDLER_AP_AT), (VECTOR_BSP, HANDLER_BSP_AT)):
        table[vector * 16 : vector * 16 + 16] = struct.pack("<HHBBHII", handler & 0xFFFF, 0x10, 0, 0x8E, handler >> 16, 0, 0)
    return bytes(table)


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


def build_image(variant: str, scenario: str = "normal", rounds: int = ROUNDS_DEFAULT) -> tuple[bytes, dict]:
    if variant not in VARIANTS:
        raise BuildError(f"--variant must be one of {VARIANTS}")
    if scenario not in SCENARIOS:
        raise BuildError(f"--scenario must be one of {SCENARIOS}")
    blob, where = strings_blob(scenario, rounds)
    bsp, bsp_listing, bsp_labels = bsp_code(where, scenario, rounds)
    ap, ap_listing, ap_labels = ap_code(where, variant)
    handlers = handlers_blob()
    memory: dict[int, bytes] = {
        PML4: struct.pack("<Q", PDPT | 3),
        PDPT: struct.pack("<Q", PD | 3) + bytes(16) + struct.pack("<Q", PD_L | 3),
        PD: struct.pack("<Q", 0x83),
        PD_L + 0x1F7 * 8: struct.pack("<Q", PT_L | 3),
        PT_L: struct.pack("<Q", LAPIC | 0x13),
        GDTR_AT: struct.pack("<HI", len(GDT) - 1, GDT_AT),
        IDTR_AT: struct.pack("<HI", IDT_LIMIT, IDT_AT),
        IDTR_BSP_AT: struct.pack("<HQ", IDT_LIMIT, IDT_AT),
        CELLS_AT: le32(0x901) + le32(0) + struct.pack("<Q", AP_CODE),  # EFER: SCE | LME | NXE
        GDT_AT: GDT,
        STR_AT: blob,
        IDT_AT: idt_bytes(),
        BSP_CODE: bsp,
        AP_CODE: ap,
    }
    for gpa, (data, _) in handlers.items():
        memory[gpa] = data
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
    if len(bsp) > AP_CODE - BSP_CODE:
        raise BuildError("the BSP code overlaps the AP code")
    listing = {
        "trampoline_real": [{"asm": a, "bytes": d.hex()} for a, d in tramp[REAL_AT][1]],
        "trampoline_protected": [{"asm": a, "bytes": d.hex()} for a, d in tramp[PROTECTED_AT][1]],
        "trampoline_long_entry": [{"asm": a, "bytes": d.hex()} for a, d in tramp[COMPAT_AT][1]],
        "bsp": bsp_listing,
        "ap": ap_listing,
        "handlers": {f"0x{gpa:x}": {"bytes": data.hex(), "role": role} for gpa, (data, role) in handlers.items()},
    }
    return bytes(raw), {"listing": listing, "bsp_labels": bsp_labels, "ap_labels": ap_labels, "strings": where}


# ---- expectations: derived from the protocol, not from a run --------------------------------------


def expected_uart(variant: str, vcpus: int, scenario: str = "normal", rounds: int = ROUNDS_DEFAULT) -> str | None:
    """The UART transcript with `{nonce}` for the nonce of the run; None when the run is refused.

    For `bad-ipi` the run ends with a device error after the marker line; the text is what the
    guest printed up to the error. For `nmi` it is the transcript by the SDM (the AP takes the NMI
    through vector 2); the run completes, `expected_run_error` returns None for it."""
    head = "BSP0 UP {nonce}\n"
    if vcpus == 1:
        return head + "SMP PROBE AP ABSENT {nonce}\n"
    if variant == "no_flag":
        return head + "AP1 UP {nonce}\nSMP PROBE FAIL TIMEOUT {nonce}\n"
    if variant == "no_long_mode":
        return None  # the AP is refused by the executor, the run ends with an error
    body = head + "AP1 UP {nonce}\n"
    return body + {
        "normal": "SMP PROBE OK {nonce}\n",
        "exchange": f"SMP PROBE OK K={rounds} {{nonce}}\n",
        "resipi": f"SMP PROBE OK K={rounds} {{nonce}}\n",
        "bad-ipi": "SMP PROBE BAD-IPI {nonce}\n",
        "nmi": "SMP PROBE NMI OK {nonce}\n",
    }[scenario]


def expected_run_error(variant: str, vcpus: int, scenario: str) -> str | None:
    if vcpus > 1 and variant == "no_long_mode":
        return "ApLegacyRefused"
    if vcpus > 1 and variant == "normal" and scenario == "bad-ipi":
        # SMP_CONTRACT 5: an ICR destination outside 0..N-1 is an explicit LAPIC device error.
        return "LapicDeviceError"
    return None


def expected_memory(variant: str, vcpus: int, scenario: str = "normal", rounds: int = ROUNDS_DEFAULT) -> dict:
    """Cells after a run to the end (values; `nonce` cells hold the zero padded nonce)."""
    up = vcpus > 1 and variant == "normal"
    exchanged = up and scenario in ("exchange", "resipi")
    cells = {
        "lock_entered": {"gpa": f"0x{LOCK:x}", "value": "0x1" if vcpus > 1 else "0x0"},
        "flag_ap_up": {"gpa": f"0x{FLAG:x}", "value": "0x1" if up else "0x0"},
        "start_count": {"gpa": f"0x{COUNT:x}", "value": "0x1" if vcpus > 1 and variant != "no_long_mode" else "0x0"},
        "ap_rx": {"gpa": f"0x{AP_RX:x}", "value": f"0x{rounds if exchanged else 0:x}"},
        "bsp_rx": {"gpa": f"0x{BSP_RX:x}", "value": f"0x{rounds if exchanged else 0:x}"},
        "nmi_cell": {"gpa": f"0x{NMI_CELL:x}", "value": "0x1" if up and scenario == "nmi" else "0x0"},
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


def build(output: Path, variant: str, scenario: str = "normal", rounds: int = ROUNDS_DEFAULT) -> dict:
    raw, details = build_image(variant, scenario, rounds)
    output.mkdir(parents=True, exist_ok=True)
    (output / "probe.bin").write_bytes(raw)
    expectations = {}
    for vcpus in (1, 2):
        text = expected_uart(variant, vcpus, scenario, rounds)
        name = f"n{vcpus}"
        expectations[name] = {
            "uart_template": text,
            "memory": expected_memory(variant, vcpus, scenario, rounds),
            "run_error": expected_run_error(variant, vcpus, scenario),
        }
        if vcpus > 1 and variant == "normal":
            # Scheduler counters of the AP: INIT applied once, started once (a second SIPI to a
            # running AP does not start it again).
            expectations[name]["ap_init_applied"] = 1
            expectations[name]["ap_started"] = 1
        if text is not None:
            (output / f"expected_uart_{name}.txt").write_text(text)
    manifest = {
        "schema": "stage6-smp-probe-build-v2",
        "profile": "Stage6SmpProbeV2",
        "variant": variant,
        "scenario": scenario,
        "rounds": rounds if scenario in ("exchange", "resipi") else 0,
        "method": "hand-encoded bytes; no assembler, compiler or linker",
        "layout": {
            "image_base": f"0x{IMAGE_BASE:x}",
            "image_size": f"0x{IMAGE_SIZE:x}",
            "page_tables": f"0x{PML4:x}",
            "info": f"0x{INFO:x}",
            "trampoline": f"0x{REAL_AT:x}",
            "sipi_vector": f"0x{SIPI_VECTOR:x}",
            "gdt": f"0x{GDT_AT:x}",
            "idt": f"0x{IDT_AT:x}",
            "idt_limit": f"0x{IDT_LIMIT:x}",
            "idtr_bsp_descriptor": f"0x{IDTR_BSP_AT:x}",
            "bsp_code": f"0x{BSP_CODE:x}",
            "ap_code": f"0x{AP_CODE:x}",
            "data": f"0x{DATA:x}",
            "bsp_stack_top": f"0x{BSP_STACK_TOP:x}",
            "ap_stack_top": f"0x{AP_STACK_TOP:x}",
        },
        "info_block": {
            "size": 64,
            "fields": "magic u32 @0 (SMP6 = 0x36504d53), version u32 @4 (1), vcpu_count u32 @8, nonce_len u32 @12, nonce @16..64 (<= 47 bytes, NUL terminated)",
            "written_by": "the loader, never in the image",
        },
        "protocol": {
            "wait_init_iterations": WAIT_INIT,
            "wait_up_iterations": WAIT_UP,
            "wait_reply_iterations": WAIT_RX,
            "icr_init_assert": f"0x{ICR_INIT_ASSERT:x}",
            "icr_init_deassert": f"0x{ICR_INIT_DEASSERT:x}",
            "icr_sipi": f"0x{ICR_SIPI:x}",
            "vector_to_ap": f"0x{VECTOR_AP:x}",
            "vector_to_bsp": f"0x{VECTOR_BSP:x}",
            "icr_nmi": f"0x{ICR_NMI:x}",
            "bad_ipi_destination": "vcpu_count (the first APIC ID that does not exist)",
            "max_fixed_quantum_steps": MAX_FIXED_QUANTUM,
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
    parser.add_argument("--scenario", choices=SCENARIOS, default="normal")
    parser.add_argument("--rounds", type=int, default=ROUNDS_DEFAULT, help=f"K of exchange/resipi, 1..={ROUNDS_MAX}")
    args = parser.parse_args()
    try:
        manifest = build(Path(args.output_dir), args.variant, args.scenario, args.rounds)
    except (BuildError, OSError) as error:
        print(f"build_stage6_smp_probe: error: {error}", file=sys.stderr)
        return 1
    print(f"output: {args.output_dir}")
    print(f"raw sha256: {manifest['artifacts']['probe.bin']['sha256']}; variant: {args.variant}; scenario: {args.scenario}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
