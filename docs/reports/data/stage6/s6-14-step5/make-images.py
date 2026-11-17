#!/usr/bin/env python3
"""S6-14 step 5 oracle image (run from anywhere; writes next to this file).

The image is the trampoline of the S6-16e/S6-16f probe (tools/build/build_stage6_smp_probe.py,
variant `normal`, scenario `normal`), built by that builder, not copied from the kernel block.
It is entered like an AP after SIPI vector 0x08: `--real-mode 0x0800:0x0000`, GPA 0x8000.

Three deliberate differences from the image the builder writes for the machine (all of them
checked below against a second build with the builder's own constants):

1. kvm_runner loads ONE contiguous image at the entry GPA (`(CS << 4) + IP`, here 0x8000), so
   nothing below 0x8000 can be placed. The builder keeps its page tables at 0x2000..0x6FFF (PML4
   0x2000, PDPT 0x3000, PD 0x4000, PD_L 0x5000, PT_L 0x6000). Here the five tables are at
   0x10000..0x14FFF (PML4 0x10000 ...). The only instruction that changes is the immediate of
   `mov eax, PML4` before `mov cr3, eax` (`b8 00 20 00 00` -> `b8 00 00 01 00`). The 64-byte info
   block at 0x7000 is written by the loader, not by the builder, and is not read before the 64-bit
   code: it is absent.
2. The first byte of the AP's 64-bit code (GPA 0xB000, `bb` of `mov ebx, 0xFEE00000`) is replaced
   by `hlt` (`f4`). The model stops before the first 64-bit instruction (`Mode64TargetEntered`);
   the oracle executes this HLT, so its RIP is one past the target.
3. The image ends at 0x15000, the end of the relocated tables (the builder's image ends at 0xD000;
   its data region 0xC000.. is zero in both).

Everything else, including the trampoline at 0x8000/0x8200/0x8300, the GDT at 0x9000, the GDTR
and IDTR descriptors at 0x8130/0x8140, the EFER and jump cells at 0x8400/0x8408, the IDT and the
code of the BSP and the AP, is byte for byte what the builder produces.
"""
import hashlib
import importlib.util
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
BUILDER = os.path.join(HERE, "../../../../../tools/build/build_stage6_smp_probe.py")

ENTRY_GPA = 0x8000
TABLES_AT = 0x10000  # PML4, PDPT, PD, PD_L, PT_L: one 4 KiB page each
IMAGE_END = 0x15000


def load_builder():
    spec = importlib.util.spec_from_file_location("probe_builder", BUILDER)
    module = importlib.util.module_from_spec(spec)
    sys.modules["probe_builder"] = module
    spec.loader.exec_module(module)
    return module


def build(relocate: bool):
    b = load_builder()
    if relocate:
        b.PML4, b.PDPT, b.PD, b.PD_L, b.PT_L = (TABLES_AT + 0x1000 * i for i in range(5))
        b.IMAGE_BASE = ENTRY_GPA
        b.IMAGE_SIZE = IMAGE_END - ENTRY_GPA
    raw, _ = b.build_image("normal", "normal")
    return b, raw


def main():
    stock_b, stock = build(False)
    b, raw = build(True)
    # Builder image: GPA 0x2000.., ours: GPA 0x8000.. . Compare the common range 0x8000..0xD000.
    stock_view = stock[ENTRY_GPA - stock_b.IMAGE_BASE : 0xD000 - stock_b.IMAGE_BASE]
    ours_view = raw[: 0xD000 - ENTRY_GPA]
    diff = [i for i in range(len(stock_view)) if stock_view[i] != ours_view[i]]
    # The only difference in 0x8000..0xD000 is the 32-bit immediate of `mov eax, PML4`.
    old_imm = (0x2000).to_bytes(4, "little")
    new_imm = (TABLES_AT).to_bytes(4, "little")
    at = stock_view.find(b"\xb8" + old_imm + b"\x0f\x22\xd8")
    assert at >= 0 and stock_view.count(b"\xb8" + old_imm + b"\x0f\x22\xd8") == 1
    assert diff == [at + 1 + i for i in range(4) if old_imm[i] != new_imm[i]], diff
    assert ours_view[at : at + 4] == b"\xb8" + new_imm[:3]
    print(f"mov eax, PML4 at GPA {ENTRY_GPA + at:#x}: only immediate differs ({len(diff)} bytes)")

    # The five tables: the range check above does not cover them (0x2000..0x6FFF vs
    # 0x10000..0x14FFF). Each page of ours must equal the builder's page with every 8-byte entry
    # that points at a table moved by the same shift (0x3003 -> 0x11003, 0x4003 -> 0x12003,
    # 0x5003 -> 0x13003, 0x6003 -> 0x14003); PD[0] (0x83) and PT_L[0] (LAPIC | 0x13) are kept.
    shift = TABLES_AT - stock_b.PML4
    old_tables = [stock_b.PML4, stock_b.PDPT, stock_b.PD, stock_b.PD_L, stock_b.PT_L]
    expected_entries = {  # (table index, entry index): (builder value, ours value)
        (0, 0): (0x3003, 0x11003),
        (1, 0): (0x4003, 0x12003),
        (1, 3): (0x5003, 0x13003),
        (2, 0): (0x83, 0x83),
        (3, 0x1F7): (0x6003, 0x14003),
        (4, 0): (stock_b.LAPIC | 0x13, stock_b.LAPIC | 0x13),
    }
    found = {}
    for t, old_base in enumerate(old_tables):
        s = stock[old_base - stock_b.IMAGE_BASE : old_base - stock_b.IMAGE_BASE + 0x1000]
        o_at = TABLES_AT + 0x1000 * t - ENTRY_GPA
        o = raw[o_at : o_at + 0x1000]
        for e in range(512):
            sv = int.from_bytes(s[e * 8 : e * 8 + 8], "little")
            ov = int.from_bytes(o[e * 8 : e * 8 + 8], "little")
            if sv == 0 and ov == 0:
                continue
            found[(t, e)] = (sv, ov)
            # A table pointer is moved by `shift`; any other entry is identical.
            moved = sv + shift if (sv & ~0xFFF) in old_tables else sv
            assert ov == moved, (t, e, hex(sv), hex(ov))
    assert found == expected_entries, found
    print("page tables: 6 non-zero entries, pointers moved by", hex(shift), ", the rest identical")

    # The AP's 64-bit code starts at AP_CODE; the oracle stops at an HLT put there.
    assert b.AP_CODE == 0xB000
    off = b.AP_CODE - ENTRY_GPA
    assert raw[off] == 0xBB, "first AP instruction is mov ebx, imm32"
    image = bytearray(raw)
    image[off] = 0xF4
    # The block of the cell: EFER, jump cell = AP_CODE.
    assert int.from_bytes(image[0x8408 - ENTRY_GPA : 0x8410 - ENTRY_GPA], "little") == b.AP_CODE
    name = "a-probe-v2-normal-sipi-hlt-at-0xb000-0800-0000.bin"
    path = os.path.join(HERE, name)
    with open(path, "wb") as f:
        f.write(bytes(image))
    print(name, len(image), hashlib.sha256(bytes(image)).hexdigest())
    # Also record the unmodified (HLT-free) image sha256 for the record.
    print("without HLT:", hashlib.sha256(raw).hexdigest())


if __name__ == "__main__":
    main()
