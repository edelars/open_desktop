#!/usr/bin/env python3
"""Build and validate the freestanding Stage4PitCalibrateProbeV1 kernel image (S4-29d).

The image is a flat identity-mapped Long64/CPL0 guest: a GDT and one code page
that repeats the Linux 8254 channel-2 TSC calibrations (`pit_calibrate_tsc`
and the MSB form of `quick_pit_calibrate`) on virtual time and writes the
results to COM1 before HLT. The raw image is loaded at GPA 0x1000 and is exactly
0x4000 bytes. The builder also holds the reference model that derives every
expected value from the PIT counting formula and the port-tick rule; it does
not read the output of the machine code.
"""

from __future__ import annotations

import argparse
import json
import os
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from build_stage4_flat_probe import (  # noqa: E402
    BuildError,
    elf_symbol,
    output_directory,
    parse_elf_load_segments,
    resolve_executable,
    run,
    rust_toolchain,
    sha256,
)

IMAGE_BASE = 0x1000
IMAGE_SIZE = 0x4000
GDT_GPA = 0x2000
CODE_GPA = 0x4000
GDT = struct.pack("<QQQ", 0, 0x00AF9B000000FFFF, 0x00CF93000000FFFF)

# --- the PIT model and the port-tick rule (inputs of the reference model) ---
PIT_INPUT_HZ = 1_193_182
PORT_TICK_NS = 1_000  # every read of port 0x61 advances virtual time by 1 us
TSC_HZ = 1_000_000_000  # one TSC tick per nanosecond of virtual time
CAL_LATCH = 11_931  # PIT_TICK_RATE / (1000 / CAL_MS) with CAL_MS = 10
CAL_MS = 10
CAL_LOOPMIN = 1_000
QUICK_COUNT = 0xFFFF
QUICK_MIN_PASSES = 5  # a stage needs more than this many passes
FAILED_FREQUENCY = 0xFFFF_FFFF_FFFF_FFFF


def pit_ticks(now_ns: int) -> int:
    """floor(t_ns * 1193182 / 1e9), the tick index of the PIT input clock."""
    return now_ns * PIT_INPUT_HZ // 1_000_000_000


# --- independent byte encoding of the guest program -------------------------
def _imm32(value: int) -> bytes:
    return struct.pack("<I", value & 0xFFFFFFFF)


def out8(port: int, value: int) -> bytes:
    return b"\xba" + _imm32(port) + bytes([0xB0, value, 0xEE])  # mov edx; mov al; out


def rdtsc64() -> bytes:
    return bytes.fromhex("0f31" "48c1e220" "4809d0")  # rdtsc; shl rdx,32; or rax,rdx


def gate_high() -> bytes:
    return b"\xba" + _imm32(0x61) + bytes.fromhex("ec" "24fd" "0c01" "ee")


def probe_msb() -> bytes:
    return (
        b"\xba" + _imm32(0x61) + b"\xec"  # time driver: in al,dx
        + out8(0x43, 0x80)  # latch channel 2
        + b"\xba" + _imm32(0x42) + b"\xec\xec"  # LSB then MSB
    )


class Code:
    """Byte stream with rel8 branches and rel32 calls resolved by label."""

    def __init__(self) -> None:
        self.data = bytearray()
        self.labels: dict[str, int] = {}
        self.fixups: list[tuple[int, str, int]] = []  # (position, label, width)

    def raw(self, data: bytes | str) -> None:
        self.data += bytes.fromhex(data) if isinstance(data, str) else data

    def label(self, name: str) -> None:
        self.labels[name] = len(self.data)

    def branch(self, opcode: int, label: str) -> None:
        self.data += bytes([opcode, 0])
        self.fixups.append((len(self.data) - 1, label, 1))

    def call(self, label: str) -> None:
        self.data += b"\xe8\0\0\0\0"
        self.fixups.append((len(self.data) - 4, label, 4))

    def finish(self) -> bytes:
        for position, label, width in self.fixups:
            relative = self.labels[label] - (position + width)
            if width == 1:
                self.data[position] = struct.pack("<b", relative)[0]
            else:
                self.data[position : position + 4] = struct.pack("<i", relative)
        return bytes(self.data)


JNZ, JAE, JBE, JB, JA, JNE, JMP = 0x75, 0x73, 0x76, 0x72, 0x77, 0x75, 0xEB


def expected_code(omit_gate: bool) -> tuple[bytes, dict[str, int]]:
    c = Code()
    for port, value in ((0x3FB, 0x83), (0x3F8, 0x01), (0x3F9, 0x00), (0x3FB, 0x03)):
        c.raw(out8(port, value))  # COM1: 8N1, divisor 1
    if not omit_gate:
        c.raw(gate_high())
    c.raw(out8(0x43, 0xB0) + out8(0x42, CAL_LATCH & 0xFF) + out8(0x42, CAL_LATCH >> 8))
    c.raw(rdtsc64())
    c.raw("4889c3" "4889c6")  # mov rbx,rax (t1); mov rsi,rax (tsc)
    c.raw("48c7c7ffffffff" "31ed" "4531e4" "4531ed")  # rdi=-1; ebp=0; r12d=0; r13d=0
    c.label("a_loop")
    c.raw(b"\xba" + _imm32(0x61) + bytes.fromhex("eca820"))  # in al,dx; test al,0x20
    c.branch(JNZ, "a_done")
    c.raw(rdtsc64())
    c.raw("4889c2" "4829f2" "4889c6" "4839fa")  # rdx=rax; rdx-=rsi; rsi=rax; cmp rdx,rdi
    c.branch(JAE, "a_min")
    c.raw("4889d7")  # rdi=rdx
    c.label("a_min")
    c.raw("4839ea")  # cmp rdx,rbp
    c.branch(JBE, "a_max")
    c.raw("4889d5")  # rbp=rdx
    c.label("a_max")
    c.raw("49ffc4")  # inc r12
    c.branch(JMP, "a_loop")
    c.label("a_done")
    c.raw("4989f6" "4929de" "4981fce8030000")  # r14=rsi; r14-=rbx; cmp r12,1000
    c.branch(JB, "a_fail")
    c.raw("488d04bf" "4801c0" "4839c5")  # rax=rdi*5; rax+=rax; cmp rbp,rax
    c.branch(JA, "a_fail")
    c.raw("4c89f0" "31d2" "b90a000000" "48f7f1" "4989c7")  # rax=r14; rdx=0; rcx=10; div; r15=rax
    c.branch(JMP, "a_emit")
    c.label("a_fail")
    c.raw("41bd01000000" "49c7c7ffffffff")  # r13d=1; r15=-1
    c.label("a_emit")
    c.raw(out8(0x3F8, 0x41))
    c.raw(b"\xba" + _imm32(0x3F8) + bytes.fromhex("4488e8" "ee"))  # al=r13b; out
    for move in ("4c89f8", "4c89f0", "4c89e0", "4889f8", "4889e8"):  # r15 r14 r12 rdi rbp -> rax
        c.raw(move)
        c.call("emit64")
    # B: quick_pit_calibrate (MSB form)
    c.raw(gate_high())
    c.raw(out8(0x43, 0xB0) + out8(0x42, QUICK_COUNT & 0xFF) + out8(0x42, QUICK_COUNT >> 8))
    c.raw("4531ed" "31c9")  # r13d=0; ecx=0
    for stage, msb in (("b1", 0xFF), ("b2", 0xFE)):
        if stage == "b2":
            c.raw("31c9")  # ecx = 0
        c.label(f"{stage}_loop")
        c.raw(probe_msb())
        c.raw(bytes([0x3C, msb]))  # cmp al,msb
        c.branch(JNE, f"{stage}_done")
        c.raw(rdtsc64())
        c.raw("4889c6" "48ffc1")  # rsi=rax; inc rcx
        c.branch(JMP, f"{stage}_loop")
        c.label(f"{stage}_done")
        if stage == "b1":
            c.raw("4989ce" "4889f3" "4883f905")  # r14=rcx; rbx=rsi; cmp rcx,5
            c.branch(JA, "b1_ok")
            c.raw("41bd02000000")  # r13d=2
            c.label("b1_ok")
        else:
            c.raw("4989cc" "4883f905")  # r12=rcx; cmp rcx,5
            c.branch(JA, "b2_ok")
            c.raw("41bd03000000")  # r13d=3
            c.label("b2_ok")
    c.raw("4989f7" "4929df" "4585ed")  # r15=rsi; r15-=rbx; test r13d,r13d
    c.branch(JNZ, "b_fail")
    c.raw("4c89f8" "b9de341200" "48f7e1" "b900e80300" "48f7f1" "4889c7")  # delta*1193182/256000
    c.branch(JMP, "b_emit")
    c.label("b_fail")
    c.raw("48c7c7ffffffff")
    c.label("b_emit")
    c.raw(out8(0x3F8, 0x42))
    c.raw(b"\xba" + _imm32(0x3F8) + bytes.fromhex("4488e8" "ee"))
    for move in ("4889f8", "4c89f8", "4c89f0", "4c89e0"):  # rdi r15 r14 r12 -> rax
        c.raw(move)
        c.call("emit64")
    c.raw(out8(0x3F8, 0x5A))
    c.label("done")
    c.raw("f4")
    c.branch(JMP, "done")
    c.label("emit64")
    c.raw("4989c2" "41bb08000000" + "ba" + _imm32(0x3F8).hex())
    c.label("emit_byte")
    c.raw("4c89d0" "ee" "49c1ea08" "49ffcb")
    c.branch(JNZ, "emit_byte")
    c.raw("c3")
    c.label("code_end")
    return c.finish(), c.labels


# --- reference model: expected values from the formula and the port tick ---
def reference_model(port_tick_ns: int = PORT_TICK_NS) -> dict:
    """Simulate the guest at the port level on virtual time.

    Only reads of port 0x61 advance time (by `port_tick_ns`, after the value
    is sampled); instructions, writes and reads of 0x42 take no virtual time.
    TSC = virtual nanoseconds (1 GHz). Counter: mode 0, count N loaded at
    t_load completes at T0 = pit_ticks(t_load); with e = pit_ticks(t) - T0
    OUT2 is high when e >= N and the counter reads (N - e) & 0xffff.
    """
    state = {"t": 0, "reads": 0}

    def read_port_b() -> int:
        sampled = state["t"]
        state["reads"] += 1
        state["t"] += port_tick_ns
        return sampled

    # A: pit_calibrate_tsc
    read_port_b()  # inb(0x61) of the gate read-modify-write
    t_load = state["t"]
    start_tick = pit_ticks(t_load)
    t1 = state["t"]
    tsc = t1
    tsc_min, tsc_max, pitcnt = FAILED_FREQUENCY, 0, 0
    t2 = t1
    while True:
        sampled = read_port_b()
        if pit_ticks(sampled) - start_tick >= CAL_LATCH:  # OUT2 high
            break
        t2 = state["t"]
        delta = t2 - tsc
        tsc = t2
        tsc_min = min(tsc_min, delta)
        tsc_max = max(tsc_max, delta)
        pitcnt += 1
    delta_a = t2 - t1
    ok_a = pitcnt >= CAL_LOOPMIN and tsc_max <= 10 * tsc_min
    freq_a = delta_a // CAL_MS if ok_a else FAILED_FREQUENCY
    reads_a = state["reads"]

    # B: quick_pit_calibrate (MSB form)
    read_port_b()  # inb(0x61) of the gate read-modify-write
    start_tick_b = pit_ticks(state["t"])

    def msb_pass() -> int:
        read_port_b()  # time driver; the latch happens after its tick
        count = (QUICK_COUNT - (pit_ticks(state["t"]) - start_tick_b)) & 0xFFFF
        return count >> 8

    stage_counts = []
    stage_tsc = []
    for expected_msb in (0xFF, 0xFE):
        passes, last_tsc = 0, 0
        while msb_pass() == expected_msb:
            last_tsc = state["t"]
            passes += 1
        stage_counts.append(passes)
        stage_tsc.append(last_tsc)
    ok_b = all(count > QUICK_MIN_PASSES for count in stage_counts)
    delta_b = stage_tsc[1] - stage_tsc[0]
    freq_b = delta_b * PIT_INPUT_HZ // (256 * 1000) if ok_b else FAILED_FREQUENCY

    # Stage 2 runs last and overwrites the status of a failed stage 1.
    status_b = 3 if stage_counts[1] <= QUICK_MIN_PASSES else (2 if stage_counts[0] <= QUICK_MIN_PASSES else 0)
    serial = bytes([0x41, 0 if ok_a else 1])
    for value in (freq_a, delta_a, pitcnt, tsc_min, tsc_max):
        serial += struct.pack("<Q", value)
    serial += bytes([0x42, status_b])
    for value in (freq_b, delta_b, stage_counts[0], stage_counts[1]):
        serial += struct.pack("<Q", value)
    serial += bytes([0x5A])
    return {
        "port_tick_ns": port_tick_ns,
        "a": {
            "latch": CAL_LATCH,
            "ms": CAL_MS,
            "load_ns": t_load,
            "start_pit_tick": start_tick,
            "freq_khz": freq_a,
            "delta_tsc": delta_a,
            "pitcnt": pitcnt,
            "tscmin": tsc_min,
            "tscmax": tsc_max,
            "port_b_reads": reads_a,
        },
        "b": {
            "freq_khz": freq_b,
            "delta_tsc": delta_b,
            "stage1_passes": stage_counts[0],
            "stage2_passes": stage_counts[1],
            "port_b_reads": state["reads"] - reads_a,
        },
        "port_b_reads_total": state["reads"],
        "clock_ns": state["t"],
        "serial_bytes": list(serial),
        "serial_hex": serial.hex(),
    }


def validate_image(elf: bytes, raw: bytes, omit_gate: bool) -> dict:
    entry, segments = parse_elf_load_segments(elf)
    code_start, _ = elf_symbol(elf, "_start")
    code_end, _ = elf_symbol(elf, "stage4_code_end")
    if entry != CODE_GPA or code_start != CODE_GPA:
        raise BuildError(f"entry is 0x{entry:x}, expected 0x{CODE_GPA:x}")
    code, _labels = expected_code(omit_gate)
    if code_end != CODE_GPA + len(code):
        raise BuildError(f"code ends at 0x{code_end:x}, expected 0x{CODE_GPA + len(code):x}")
    expected = [
        (GDT_GPA, GDT_GPA, len(GDT), 6),
        # The linker rounds the code output section up to a multiple of four bytes.
        (CODE_GPA, CODE_GPA, (len(code) + 3) & ~3, 5),
    ]
    if len(segments) != len(expected):
        raise BuildError(f"expected {len(expected)} PT_LOAD segments, found {len(segments)}")
    for segment, (vaddr, paddr, size, flags) in zip(segments, expected, strict=True):
        actual = (segment["vaddr"], segment["paddr"], segment["filesz"], segment["flags"])
        if actual != (vaddr, paddr, size, flags) or segment["memsz"] != size:
            raise BuildError(f"unexpected PT_LOAD {actual}, expected {(vaddr, paddr, size, flags)}")
    if len(raw) != IMAGE_SIZE:
        raise BuildError(f"raw image is 0x{len(raw):x} bytes, expected 0x{IMAGE_SIZE:x}")
    for segment in segments:
        start = segment["paddr"] - IMAGE_BASE
        if raw[start : start + segment["filesz"]] != elf[segment["offset"] : segment["offset"] + segment["filesz"]]:
            raise BuildError(f"raw bytes differ from ELF at GPA 0x{segment['paddr']:x}")

    def at(gpa: int, length: int) -> bytes:
        return raw[gpa - IMAGE_BASE : gpa - IMAGE_BASE + length]

    if at(GDT_GPA, len(GDT)) != GDT:
        raise BuildError("GDT descriptors do not match the fixed profile")
    if at(CODE_GPA, len(code)) != code:
        raise BuildError("code does not match the independently encoded guest program")
    covered = [(GDT_GPA, len(GDT)), (CODE_GPA, len(code))]
    for gpa in range(IMAGE_BASE, IMAGE_BASE + IMAGE_SIZE):
        if raw[gpa - IMAGE_BASE] and not any(start <= gpa < start + size for start, size in covered):
            raise BuildError(f"non-zero byte outside the fixed layout at GPA 0x{gpa:x}")
    return {
        "entry_gpa": f"0x{entry:x}",
        "code_end_gpa": f"0x{code_end:x}",
        "code_bytes": len(code),
        "gdt_gpa": f"0x{GDT_GPA:x}",
        "load_gpa": f"0x{IMAGE_BASE:x}",
        "image_size": f"0x{IMAGE_SIZE:x}",
        "segments": [
            {"paddr": f"0x{p:x}", "filesz": f"0x{s:x}", "flags": f"0x{f:x}"} for _, p, s, f in expected
        ],
    }


def build(args: argparse.Namespace) -> int:
    root = Path(__file__).resolve().parents[2]
    fixture = root / "tests/fixtures/guest/stage4/pit_calibrate"
    out = output_directory(root, args.output_dir)
    clang = resolve_executable(os.environ.get("STAGE4_CLANG"), "clang")
    rustc, host, rustc_version, linker, objcopy, env = rust_toolchain()
    env.update({"LC_ALL": "C", "SOURCE_DATE_EPOCH": "0"})
    clang_version = run([str(clang), "--version"], env=env).splitlines()[0]
    linker_version = run([str(linker), "--version"], env=env).splitlines()[0]
    objcopy_version = run([str(objcopy), "--version"], env=env)
    defines = [f"-DPROBE_OMIT_GATE={int(args.omit_gate)}"]
    commands = [
        [str(clang), "--target=x86_64-unknown-none-elf", "-x", "assembler-with-cpp", "-ffreestanding",
         "-fno-pic", "-fno-pie", "-mno-red-zone", *defines, "-c", str(fixture / "entry.S"),
         "-o", str(out / "entry.o")],
        [str(linker), "-m", "elf_x86_64", "--build-id=none", "-T", str(fixture / "layout.ld"),
         "-o", str(out / "probe.elf"), str(out / "entry.o")],
        [str(objcopy), "-O", "binary", "--gap-fill=0x00", str(out / "probe.elf"),
         str(out / "probe.unpadded.bin")],
    ]
    for command in commands:
        run(command, env=env, cwd=root)
    elf = (out / "probe.elf").read_bytes()
    # objcopy starts at the lowest load address (the GDT at 0x2000).
    body = (out / "probe.unpadded.bin").read_bytes()
    raw = bytes(GDT_GPA - IMAGE_BASE) + body
    if len(raw) > IMAGE_SIZE:
        raise BuildError(f"image exceeds its fixed size: 0x{len(raw):x}")
    raw = raw.ljust(IMAGE_SIZE, b"\0")
    layout = validate_image(elf, raw, args.omit_gate)
    (out / "probe.bin").write_bytes(raw)
    (out / "probe.unpadded.bin").unlink()
    revision = run(["git", "rev-parse", "HEAD"], env=env, cwd=root).strip()
    variant = {"omit_gate": args.omit_gate}
    code, _ = expected_code(args.omit_gate)
    manifest = {
        "schema": "stage4-pit-calibrate-probe-build-v1",
        "profile": "Stage4PitCalibrateProbeV1",
        "variant": variant,
        "accepted_profile": variant == {"omit_gate": False},
        "base_revision": revision,
        "source": {name: sha256((fixture / name).read_bytes()) for name in ("entry.S", "layout.ld")},
        "target": "x86_64-unknown-none-elf",
        "toolchain": {
            "clang": clang_version,
            "ld.lld": linker_version,
            "rust-objcopy": objcopy_version.splitlines()[0] if objcopy_version else "",
            "rustc": rustc_version.splitlines()[0],
            "host": host,
        },
        "commands": commands,
        "layout": layout,
        "code_hex": code.hex(),
        "model": {
            "pit_input_hz": PIT_INPUT_HZ,
            "port_tick_ns": PORT_TICK_NS,
            "tsc_hz": TSC_HZ,
            "formula": "floor(t_ns * 1193182 / 1e9); mode 0 OUT2 high at elapsed >= N",
        },
        "expected": reference_model(),
        "artifacts": {
            "probe.elf": {"size": len(elf), "sha256": sha256(elf)},
            "probe.bin": {"size": len(raw), "sha256": sha256(raw)},
        },
    }
    (out / "build_manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(f"output: {out}")
    print(f"raw sha256: {manifest['artifacts']['probe.bin']['sha256']}; variant: {variant}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--omit-gate", action="store_true")
    args = parser.parse_args()
    try:
        return build(args)
    except (BuildError, OSError) as error:
        print(f"build_stage4_pit_calibrate_probe: error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
