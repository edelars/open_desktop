#!/usr/bin/env python3
"""S6-14m oracle images (run from this directory). All images are entered at 0000:0000 (GPA 0).

Layout, as in s6-14l: `jmp far 0:0x1027` at 0, GDT pointer at 0x30 (limit 0x1f, base 0x38), GDT at
0x38 (selector 8 = flat code 0x00CF9A000000FFFF with A clear, selector 0x10 = the descriptor under
test, selector 0x18 = flat data with A set), the verbatim linked bytes 0x1027..0x1044 of
16bit-objdump.txt, the block 5 far jump `66 ea 00 20 00 00 08 00` at 0x1044, HLT at 0x104c.

Block 6: the verbatim linked bytes 0x2000..0x2062 of 32bit-objdump.txt (segment loads, CR4, CR3 =
0x4000, EFER read/compare/write from the header, `mov eax, 0x80050033`), then the verbatim bytes
0x2062..0x206c of the same file: `mov cr0, eax` (PG, LME and PAE: IA-32e mode) and the far jump
`jmp 0x10:0x20f0` (`ea f0 20 00 00 10 00`). A HLT stands at 0x20f0 in place of the 64-bit code (the
S6-14n slice), so the run ends after the jump with the new CS (or with the refusal of the jump).

Page tables (4 KiB each; CR3 = 0x4000 is fixed by the linked `mov eax, 0x4000`):
  PML4 at 0x4000, PDPT at 0x7000, PD at 0x8000, PT at 0x9000 (only in the 4 KiB variants).
  PML4[0] -> PDPT, PDPT[0] -> PD, PD[0] = 2 MiB page at 0 (PS) or -> PT with PT[i] = page i
  (i < 10, identity). All entries are P|RW (0x03, 2 MiB: 0x83) with A and D clear, unless the image
  says otherwise. Page 0 holds the GDT and page 2 the code, so the 2 MiB (or PT) mapping covers
  everything the path touches.
Trampoline header (0x5000): efer at +0x08/+0x0c, cr4 at +0x10 (0x6b0 = PSE|PAE|PGE|OSFXSR|OSXMMEXCPT),
flags at +0x14 (0). EFER at entry is 0 (kvm_runner sets it to 0).
"""
import os
import struct

HERE = os.path.dirname(os.path.abspath(__file__))
OBJDUMP32 = HERE + "/../s6-02-x86/32bit-objdump.txt"

BLOCK_1027 = bytes.fromhex(
    "6685c0 7520 660f011e6000 660f01163000 ba1800 66b833000500 0f22c0".replace(" ", "")
)
assert len(BLOCK_1027) == 0x1044 - 0x1027
DX_IMM_AT = 18
assert BLOCK_1027[DX_IMM_AT - 1] == 0xBA


def blob32():
    ram = {}
    for line in open(OBJDUMP32):
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


BLOB = blob32()
assert all(a in BLOB for a in range(0x2000, 0x206C))
assert bytes(BLOB[a] for a in range(0x2062, 0x206C)) == bytes.fromhex("0f22c0" "eaf0200000" "1000")

FLAT_CODE32_A_CLEAR = bytes.fromhex("ffff0000009acf00")
FLAT_DATA = bytes.fromhex("ffff00000092cf00")
FLAT_DATA_A = bytes.fromhex("ffff00000093cf00")

CODE64_A = bytes.fromhex("ffff0000009baf00")  # L=1 D=0 G=1, 9b: P DPL0 code R/A
CODE64_NO_A = bytes.fromhex("ffff0000009aaf00")
CODE32_COMPAT = bytes.fromhex("ffff0000009bcf00")  # L=0 D=1
CODE_L_AND_D = bytes.fromhex("ffff0000009bef00")  # L=1 D=1
CODE64_NOT_PRESENT = bytes.fromhex("ffff0000001baf00")
CODE64_DPL3 = bytes.fromhex("ffff000000fbaf00")  # access 0xfb: P DPL3 code

PML4, PDPT, PD, PT = 0x4000, 0x7000, 0x8000, 0x9000
IMAGE_SIZE = 0xA000


def image(name, desc=CODE64_A, efer=0x901, cr4=0x6B0, pt=False, pml4e=None, pdpte=None,
          pde=None, pte=None, jmp_selector=None, preset_ad=False):
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
        img[a] = BLOB[a]
    if jmp_selector is not None:
        img[0x206A:0x206C] = struct.pack("<H", jmp_selector)
    img[0x20F0] = 0xF4  # HLT in place of the 64-bit code at startup_64
    img[0x5008:0x500C] = struct.pack("<I", efer & 0xFFFFFFFF)
    img[0x500C:0x5010] = struct.pack("<I", efer >> 32)
    img[0x5010:0x5014] = struct.pack("<I", cr4)
    img[0x5014:0x5018] = struct.pack("<I", 0)
    ad = 0x60 if preset_ad else 0
    q = lambda at, v: img.__setitem__(slice(at, at + 8), struct.pack("<Q", v))
    q(PML4, (PDPT | 0x03 | ad) if pml4e is None else pml4e)
    q(PDPT, (PD | 0x03 | ad) if pdpte is None else pdpte)
    if pt:
        q(PD, (PT | 0x03 | ad) if pde is None else pde)
        for page in range(10):
            q(PT + 8 * page, (page << 12 | 0x03 | ad) if pte is None or page not in pte else pte[page])
    else:
        q(PD, (0x83 | ad) if pde is None else pde)
    open(name + "-0000-0000.bin", "wb").write(img)


# a: the path as Linux has it: 2 MiB identity page, code64 descriptor with A set, A/D clear in the
#    tables (the walks set A in PML4E, PDPTE, PDE; no write, so D stays clear).
image("a-ia32e-2mib-code64-accessed-set")
# b: the descriptor's A bit is clear: the jump writes it, through the walk (D set in the PDE).
image("b-ia32e-2mib-code64-accessed-clear", desc=CODE64_NO_A)
# c: 4 KiB pages, A clear in the descriptor: A in PML4E/PDPTE/PDE/PTE(2) for the fetch, D in PTE(0).
image("c-ia32e-4k-code64-accessed-clear", desc=CODE64_NO_A, pt=True)
# d: the target is a 32-bit compatibility code segment (L = 0, D = 1): the HLT runs in compat mode.
image("d-compat-target-code32", desc=CODE32_COMPAT)
# e: L = 1 and D = 1: #GP at the far jump.
image("e-long-and-d-set-gp", desc=CODE_L_AND_D)
# f: code64 with P = 0: #NP.
image("f-code64-not-present", desc=CODE64_NOT_PRESENT)
# g: selector 0x28 beyond the GDT limit (0x1f).
image("g-selector-beyond-gdt-limit", jmp_selector=0x28)
# h: selector 0x10 is a data segment.
image("h-selector-is-data", desc=FLAT_DATA_A)
# i: code64 with DPL 3 (non-conforming, CPL 0).
image("i-code64-dpl3", desc=CODE64_DPL3)
# j: CR4.PAE clear (header CR4 0x690): `mov cr0` with PG and LME is #GP.
image("j-cr4-pae-clear-gp-at-mov-cr0", cr4=0x690)
# k: 4 KiB pages, page 2 (the code) not present: #PF fetching the far jump (CR2 = 0x2065).
image("k-pte-code-page-not-present", pt=True, pte={2: 0})
# l: 4 KiB pages, page 0 (the GDT) not present: #PF reading the descriptor.
image("l-pte-gdt-page-not-present", pt=True, pte={0: 0})
# m: the 2 MiB PDE has XD, EFER.NXE = 1: instruction fetch #PF (CR2 = 0x2065).
image("m-pde-xd-nxe-on", pde=0x83 | (1 << 63))
# n: the same PDE with NXE = 0 (header EFER = SCE|LME): XD is reserved, #PF.
image("n-pde-xd-nxe-off", efer=0x101, pde=0x83 | (1 << 63))
# o: 2 MiB PDE with bit 13 set (not 2 MiB aligned): reserved bit #PF.
image("o-pde-2mib-misaligned-bit13", pde=0x83 | (1 << 13))
# p: PML4E with PS set: reserved.
image("p-pml4e-ps-reserved", pml4e=PDPT | 0x83)
# q: PDPTE with PS (1 GiB page).
image("q-pdpte-1gib-page", pdpte=0x83)
# r: 2 MiB page with R/W = 0 and a descriptor with A clear: the write of A under CR0.WP = 1.
image("r-pde-readonly-accessed-clear", desc=CODE64_NO_A, pde=0x81)
# s: PML4E not present.
image("s-pml4e-not-present", pml4e=0)
# t: A and D preset in every entry and in the descriptor: nothing is written.
image("t-tables-accessed-dirty-preset", preset_ad=True)
