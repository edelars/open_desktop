#!/usr/bin/env python3
"""S6-14l oracle images (run from this directory). All images are entered at 0000:0000 (GPA 0).

Layout, as in s6-14k: `jmp far 0:0x1027` at 0, GDT pointer at 0x30 (limit 0x1f, base 0x38), GDT at
0x38 (selector 8 = flat code 0x00CF9A000000FFFF with A clear, selector 0x18 = flat data with A set),
the verbatim linked bytes 0x1027..0x1044 of 16bit-objdump.txt, the block 5 far jump `66 ea 00 20
00 00 08 00` at 0x1044, HLT at 0x104c (the jne failure target).

Block 6: the verbatim linked bytes 0x2000..0x2062 of 32bit-objdump.txt, i.e. everything up to the
`MOV CR0, EAX` that enables paging (0x2062): segment loads, BT/JAE, MOV EAX/CR4/CR3/ECX, RDMSR (EFER),
CMP EAX/EDX with the header, JNE/JE, the conditional MOV EAX/MOV EDX/WRMSR, `mov eax,0x80050033`.
The `MOV CR0, EAX` row is NOT copied: its place (0x2062) holds the "tail" of the image:
  * tail "hlt":    HLT at 0x2062 (the model stops there, as it would stop at the refused MOV CR0).
  * tail "verify": a read-back of EFER and a comparison with the header, then HLT:
        0x2062 0f 32                 rdmsr
        0x2064 3b 05 08 50 00 00     cmp eax, ds:0x5008
        0x206a 75 08                 jne 0x2074
        0x206c 3b 15 0c 50 00 00     cmp edx, ds:0x500c
        0x2072 74 01                 je 0x2075
        0x2074 f4                    hlt   (EFER read back differs from the header)
        0x2075 f4                    hlt   (EFER read back equals the header)
    Only forms of the closed set of S6-14k/S6-14l; the rel8 values are re-computed for the new addresses.
Trampoline header (0x5000 + DS.base): efer at +0x08 (low dword) and +0x0c (high dword), cr4 at +0x10,
flags at +0x14 (bit 0 = the SME branch, kept 0 here). EFER at entry is 0 (kvm_runner sets it to 0).
"""
import struct
import os

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
assert all(a in BLOB for a in range(0x2000, 0x2062))
assert (BLOB[0x2062], BLOB[0x2063], BLOB[0x2064]) == (0x0F, 0x22, 0xC0)  # MOV CR0, EAX

FLAT_CODE = bytes.fromhex("ffff0000009acf00")
FLAT_DATA = bytes.fromhex("ffff00000092cf00")
FLAT_DATA_A = bytes.fromhex("ffff00000093cf00")

TAIL_VERIFY = bytes.fromhex("0f32" "3b0508500000" "7508" "3b150c500000" "7401" "f4" "f4")
assert len(TAIL_VERIFY) == 0x14


def image(name, efer_lo, efer_hi, tail, cr4=0x6B0, flags=0):
    img = bytearray(0x6100)
    img[0:5] = bytes([0xEA, 0x27, 0x10, 0x00, 0x00])
    img[0x30:0x36] = struct.pack("<HI", 0x1F, 0x38)
    img[0x40:0x48] = FLAT_CODE
    img[0x48:0x50] = FLAT_DATA
    img[0x50:0x58] = FLAT_DATA_A
    block = bytearray(BLOCK_1027)
    block[DX_IMM_AT : DX_IMM_AT + 2] = struct.pack("<H", 0x18)
    img[0x1027:0x1044] = block
    img[0x1044:0x104C] = bytes([0x66, 0xEA, 0x00, 0x20, 0x00, 0x00]) + struct.pack("<H", 0x08)
    img[0x104C] = 0xF4
    for a in range(0x2000, 0x2062):
        img[a] = BLOB[a]
    if tail == "hlt":
        img[0x2062] = 0xF4
    else:
        img[0x2062 : 0x2062 + len(TAIL_VERIFY)] = TAIL_VERIFY
    img[0x5008:0x500C] = struct.pack("<I", efer_lo)
    img[0x500C:0x5010] = struct.pack("<I", efer_hi)
    img[0x5010:0x5014] = struct.pack("<I", cr4)
    img[0x5014:0x5018] = struct.pack("<I", flags)
    open(name + "-0000-0000.bin", "wb").write(img)


# a: header EFER = SCE|LME|NXE (0x901, what Linux writes for the BSP with LMA masked off): EFER at
#    entry is 0, so the compare fails, the header is written, the read-back equals it.
image("a-efer-901-written-and-read-back", 0x901, 0, "verify")
# b: header EFER = 0: both compares succeed, JE jumps over the WRMSR; EFER stays 0.
image("b-efer-0-equal-skips-wrmsr", 0, 0, "hlt")
# c: header EFER = LME only.
image("c-efer-lme-only", 0x100, 0, "verify")
# d: low dword equals EFER (0), the high dword differs: JNE not taken, CMP EDX not equal, JE not
#    taken, the WRMSR gets EDX:EAX = 1:0 (bit 32 reserved).
image("d-efer-high-dword-bit32-reserved", 0, 1, "verify")
# e: header EFER = SCE|LMA (0x401): a write of LMA from the program.
image("e-efer-sce-lma-written", 0x401, 0, "verify")
# f: reserved bit 1.
image("f-efer-bit1-reserved", 0x2, 0, "verify")
# g: bit 12 (SVME, AMD only).
image("g-efer-bit12-svme", 0x1000, 0, "verify")
# h: NXE only.
image("h-efer-nxe-only", 0x800, 0, "verify")
# i: reserved bit 9.
image("i-efer-bit9-reserved", 0x200, 0, "verify")
# j: LMA only (0x400).
image("j-efer-lma-only", 0x400, 0, "verify")
# k: SCE|LME|LMA|NXE (0xd01): the LMA bit is dropped by the write, the read-back differs.
image("k-efer-d01-lma-dropped", 0xD01, 0, "verify")
