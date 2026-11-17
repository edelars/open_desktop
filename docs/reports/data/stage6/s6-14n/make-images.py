#!/usr/bin/env python3
"""S6-14n oracle images (run from this directory). All images are entered at 0000:0000 (GPA 0).

Everything up to the far jump is the S6-14m path (make-images.py of s6-14m): `jmp far 0:0x1027` at
0, GDT pointer at 0x30 (limit 0x1f, base 0x38), GDT at 0x38 (selector 8 = flat 32-bit code with A
clear, 0x10 = flat code64 L=1 D=0 with A set, 0x18 = flat data with A set), the verbatim linked
bytes 0x1027..0x1044 of 16bit-objdump.txt, the block 5 far jump at 0x1044, the verbatim linked
bytes 0x2000..0x206c of 32bit-objdump.txt (block 6, `mov cr0, eax` with PG|LME|PAE and the far
jump `jmp 0x10:0x20f0`).

Block 8 (S6-14n): the verbatim linked row 0x20f0 of 64bit-objdump.txt, `ff 25 0a 2f 00 00`
(`jmp QWORD PTR [rip+0x2f0a]`, cell at 0x20f6 + 0x2f0a = 0x5000). The cell at 0x5000 (tr_start,
the first quadword of the trampoline header) holds the jump target; the trampoline header keeps
efer at +0x08, cr4 at +0x10 and flags at +0x14 as in S6-14m. The run ends with the HLT at the
target (the oracle executes it; the model stops before it, `Mode64TargetEntered`), or with the
fault of the jump (`#GP`) or of the fetch of the target (`#PF`), which, with an empty IDT, the
runner reports as triple_fault_or_shutdown.

Page tables (4 KiB each, CR3 = 0x4000 as in the linked `mov eax, 0x4000`):
  PML4 at 0x4000, PDPT at 0x7000, PD at 0x8000, PT at 0x9000 (4 KiB variants only), as in S6-14m.
  For the kernel-virtual target 0xffffffff8139c090 (secondary_startup_64 in the staged vmlinux):
  PML4[511] -> PDPT2 at 0xa000, PDPT2[510] -> PD2 at 0xb000, PD2[9] -> PT2 at 0xc000,
  PT2[0x19c] -> page 0xd000, which holds the HLT at offset 0x90 (the model never walks these).
"""
import os
import struct

HERE = os.path.dirname(os.path.abspath(__file__))
OBJDUMP32 = HERE + "/../s6-02-x86/32bit-objdump.txt"
OBJDUMP64 = HERE + "/../s6-02-x86/64bit-objdump.txt"

BLOCK_1027 = bytes.fromhex(
    "6685c0 7520 660f011e6000 660f01163000 ba1800 66b833000500 0f22c0".replace(" ", "")
)
assert len(BLOCK_1027) == 0x1044 - 0x1027
DX_IMM_AT = 18
assert BLOCK_1027[DX_IMM_AT - 1] == 0xBA


def blob(path):
    ram = {}
    for line in open(path):
        cols = line.split("\t")
        if len(cols) < 2:
            continue
        try:
            addr = int(cols[0].strip().rstrip(":"), 16)
        except ValueError:
            continue
        for i, tok in enumerate(cols[1].split()):
            ram[addr + i] = int(tok, 16)
    return ram


BLOB32 = blob(OBJDUMP32)
BLOB64 = blob(OBJDUMP64)
assert all(a in BLOB32 for a in range(0x2000, 0x206C))
assert bytes(BLOB32[a] for a in range(0x2062, 0x206C)) == bytes.fromhex("0f22c0" "eaf0200000" "1000")
assert bytes(BLOB64[a] for a in range(0x20F0, 0x20F6)) == bytes.fromhex("ff250a2f0000")

FLAT_CODE32_A_CLEAR = bytes.fromhex("ffff0000009acf00")
FLAT_DATA_A = bytes.fromhex("ffff00000093cf00")
CODE64_A = bytes.fromhex("ffff0000009baf00")  # L=1 D=0 G=1, 9b: P DPL0 code R/A
CODE64_A_BASE_1000 = bytes.fromhex("ffff0010009baf00")  # the same with a base of 0x1000

PML4, PDPT, PD, PT = 0x4000, 0x7000, 0x8000, 0x9000
PDPT2, PD2, PT2, HLT_PAGE = 0xA000, 0xB000, 0xC000, 0xD000
IMAGE_SIZE = 0xE000
SECONDARY_STARTUP_64 = 0xFFFFFFFF8139C090


def image(name, cell=SECONDARY_STARTUP_64, desc=CODE64_A, pt=False, pde=None, pte=None,
          insn=None, cell_at=0x5000, low_hlt=None, high=True):
    img = bytearray(IMAGE_SIZE)
    img[0:5] = bytes([0xEA, 0x27, 0x10, 0x00, 0x00])
    img[0x30:0x36] = struct.pack("<HI", 0x1F, 0x38)
    img[0x40:0x48] = FLAT_CODE32_A_CLEAR
    img[0x48:0x50] = desc
    img[0x50:0x58] = FLAT_DATA_A
    block = bytearray(BLOCK_1027)
    block[DX_IMM_AT : DX_IMM_AT + 2] = struct.pack("<H", 0x18)
    img[0x1027:0x1044] = block
    img[0x1044:0x104C] = bytes([0x66, 0xEA, 0x00, 0x20, 0x00, 0x00]) + struct.pack("<H", 0x08)
    img[0x104C] = 0xF4
    for a in range(0x2000, 0x206C):
        img[a] = BLOB32[a]
    for a in range(0x20F0, 0x20F6):
        img[a] = BLOB64[a]
    if insn is not None:
        img[0x20F0 : 0x20F0 + len(insn)] = insn
    img[0x5008:0x500C] = struct.pack("<I", 0x901)
    img[0x500C:0x5010] = struct.pack("<I", 0)
    img[0x5010:0x5014] = struct.pack("<I", 0x6B0)
    img[0x5014:0x5018] = struct.pack("<I", 0)
    img[cell_at : cell_at + 8] = struct.pack("<Q", cell)
    q = lambda at, v: img.__setitem__(slice(at, at + 8), struct.pack("<Q", v))
    q(PML4, PDPT | 0x03)
    q(PDPT, PD | 0x03)
    if pt:
        q(PD, (PT | 0x03) if pde is None else pde)
        for page in range(10):
            q(PT + 8 * page, (page << 12 | 0x03) if pte is None or page not in pte else pte[page])
    else:
        q(PD, 0x83 if pde is None else pde)
    if high:
        # kernel-virtual mapping of 0xffffffff8139c090 with a HLT at its physical page offset 0x90
        q(PML4 + 8 * 511, PDPT2 | 0x03)
        q(PDPT2 + 8 * 510, PD2 | 0x03)
        q(PD2 + 8 * 9, PT2 | 0x03)
        q(PT2 + 8 * 0x19C, HLT_PAGE | 0x03)
        img[HLT_PAGE + 0x90] = 0xF4
    if low_hlt is not None:
        img[low_hlt] = 0xF4
    open(name + "-0000-0000.bin", "wb").write(img)


# a: the linked path: 2 MiB identity page, cell 0xffffffff8139c090 (secondary_startup_64), the
#    target mapped through PML4[511]; the oracle fetches (and executes) the HLT there.
image("a-2mib-secondary-startup-64")
# b: 4 KiB pages: the cell read sets A in PT[5] (page 0x5000); PT[2] was set by the fetches.
image("b-4k-secondary-startup-64", pt=True)
# c: low canonical target 0x2100 with a HLT: the model stops at the target without fetching it.
image("c-low-target-0x2100", cell=0x2100, low_hlt=0x2100, high=False)
# d: 0x0000800000000000, the first non-canonical address above the low half: #GP at the jump.
image("d-noncanonical-low-gp", cell=0x0000800000000000, high=False)
# e: 0xffff7fffffffffff, the last non-canonical address below the high half: #GP at the jump.
image("e-noncanonical-high-gp", cell=0xFFFF7FFFFFFFFFFF, high=False)
# f: 0x00007fffffffffff, the last canonical low address (unmapped): #PF at the fetch of the target.
image("f-canonical-low-max-unmapped-pf", cell=0x00007FFFFFFFFFFF, high=False)
# g: 0xffff800000000000, the first canonical high address (unmapped): #PF at the fetch of the target.
image("g-canonical-high-min-unmapped-pf", cell=0xFFFF800000000000, high=False)
# h: 4 KiB pages, PT[5] not present: #PF at the read of the cell (CR2 = 0x5000).
image("h-pte-cell-page-not-present-pf", pt=True, pte={5: 0}, high=False)
# i: 2 MiB read-only PDE (R/W = 0): a read is allowed; A is set, D stays clear.
image("i-pde-readonly-cell-read", pde=0x81)
# j: negative displacement: `ff 25 ea ff ff ff` (disp -0x16) reads the cell at 0x20e0.
image("j-negative-disp-cell-at-0x20e0", insn=bytes.fromhex("ff25eaffffff"), cell_at=0x20E0)
# k: the cell straddles two 4 KiB pages: `ff 25 06 3f 00 00` reads 0x5ffc..0x6003 (PT[5], PT[6]).
image("k-4k-cell-crosses-page", pt=True, insn=bytes.fromhex("ff25063f0000"), cell_at=0x5FFC)
# l: CS descriptor with a base of 0x1000: in 64-bit mode the base is ignored.
image("l-code64-base-1000-ignored", desc=CODE64_A_BASE_1000)
