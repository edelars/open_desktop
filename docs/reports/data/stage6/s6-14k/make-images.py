#!/usr/bin/env python3
"""S6-14k oracle images (run from this directory). All images are entered at 0000:0000 (GPA 0).

Layout, as in s6-14j: `jmp far 0:0x1027` at 0, GDT pointer at 0x30 (limit 0x1f, base 0x38), GDT at
0x38 (selector 8 = flat code 0x00CF9A000000FFFF with A clear, selectors 0x10 and 0x18 = data
descriptors that differ per image), the verbatim linked bytes 0x1027..0x1044 of 16bit-objdump.txt
(test eax,eax; jne 0x104c; lidtd ds:0x60; lgdtd ds:0x30; mov dx,SEL; mov eax,0x50033; mov cr0,eax),
the block 5 far jump `66 ea 00 20 00 00 08 00` at 0x1044, HLT at 0x104c (the jne failure target).

Block 6: the verbatim linked bytes 0x2000..0x203e of 32bit-objdump.txt (rows 0x2000..0x2039: mov
ss/ds/es/fs/gs,edx; add esp,0; bt ds:0x5014,0; jae 0x2029; the optional SME rows; mov eax,ds:0x5010;
mov cr4,eax; mov eax,0x4000; mov cr3,eax; mov ecx,0xc0000080). The slice stops before the RDMSR at
0x203e: that address holds HLT (0xf4) in these images (image f: HLT at 0x201f instead, the SME
RDMSR). The trampoline header (0x5000 + DS.base): efer at +0x08 (not read in this slice), cr4 at
+0x10, flags at +0x14 (bit 0 = the SME branch).
"""
import struct

HERE = __import__("os").path.dirname(__import__("os").path.abspath(__file__))
OBJDUMP32 = HERE + "/../s6-02-x86/32bit-objdump.txt"

BLOCK_1027 = bytes.fromhex(
    "6685c0 7520 660f011e6000 660f01163000 ba1800 66b833000500 0f22c0".replace(" ", "")
)
assert len(BLOCK_1027) == 0x1044 - 0x1027
DX_IMM_AT = 18  # offset of the imm16 of `mov dx` inside BLOCK_1027
assert BLOCK_1027[DX_IMM_AT - 1] == 0xBA, hex(BLOCK_1027[DX_IMM_AT - 1])


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
# The verbatim rows 0x2000..0x203d (the last row kept is `mov ecx,0xc0000080` at 0x2039..0x203d).
assert all(a in BLOB for a in range(0x2000, 0x203E))
assert BLOB[0x203E] == 0x0F and BLOB[0x203F] == 0x32  # the RDMSR the slice stops before

FLAT_CODE = bytes.fromhex("ffff0000009acf00".replace(" ", ""))


def image(name, dx_sel, data10, data18, cr4, flags, ds_base=0, hlt_at=0x203E):
    img = bytearray(0x6100)
    img[0:5] = bytes([0xEA, 0x27, 0x10, 0x00, 0x00])  # jmp far 0:0x1027
    img[0x30:0x36] = struct.pack("<HI", 0x1F, 0x38)
    img[0x38:0x40] = bytes(8)
    img[0x40:0x48] = FLAT_CODE
    img[0x48:0x50] = data10
    img[0x50:0x58] = data18
    block = bytearray(BLOCK_1027)
    block[DX_IMM_AT : DX_IMM_AT + 2] = struct.pack("<H", dx_sel)
    img[0x1027:0x1044] = block
    img[0x1044:0x104C] = bytes([0x66, 0xEA, 0x00, 0x20, 0x00, 0x00]) + struct.pack("<H", 0x08)
    img[0x104C] = 0xF4
    for a in range(0x2000, 0x203E):
        img[a] = BLOB[a]
    img[hlt_at] = 0xF4
    hdr = 0x5000 + ds_base
    img[hdr + 0x08 : hdr + 0x10] = struct.pack("<Q", 0x100)  # EFER.LME (not read here)
    img[hdr + 0x10 : hdr + 0x14] = struct.pack("<I", cr4)
    img[hdr + 0x14 : hdr + 0x18] = struct.pack("<I", flags)
    open(name + "-0000-0000.bin", "wb").write(img)


flat = lambda access, flg=0xCF, base=0: (
    bytes.fromhex("ffff") + struct.pack("<I", base)[0:3] + bytes([access, flg, base >> 24])
)

# a: selector 0x18 = flat data, A already set (0x93); CR4 = PSE|PAE|MCE|PGE|OSFXSR|OSXMMEXCPT.
image("a-flat-data-accessed-set-cr4-06b0", 0x18, flat(0x92), flat(0x93), 0x6B0, 0)
# b: selector 0x10 = flat data with A clear (0x92): the five loads set A in memory (0x93); the SME
#    flag word is 0xfffffffe: only bit 0 selects the SME branch; CR4 = PAE only.
image("b-flat-data-accessed-clear-flags-fffffffe", 0x10, flat(0x92), flat(0x93), 0x20, 0xFFFFFFFE)
# c: data base 0x1000, G = 1, D/B = 1, AVL = 1 (byte 6 = 0xdf): the header moves to DS.base + 0x5010
#    (linear 0x6010, 0x6014); a decoy cr4 0x7ff at linear 0x5010 must not be read.
img_c = "c-base-1000-avl-g-d-header-at-ds-base"
image(img_c, 0x10, flat(0x92, 0xDF, 0x1000), flat(0x93), 0x20, 0, ds_base=0x1000)
# the decoy (written after, outside of `image`)
raw = bytearray(open(img_c + "-0000-0000.bin", "rb").read())
raw[0x5010:0x5014] = struct.pack("<I", 0x7FF)
raw[0x5014:0x5018] = struct.pack("<I", 1)  # the decoy would take the SME branch
open(img_c + "-0000-0000.bin", "wb").write(raw)
# d: byte-granular data limit 0x5fff (G = 0, D/B = 1, byte 6 = 0x40), still covers 0x5014..0x5017.
image("d-byte-limit-5fff", 0x18, flat(0x92), bytes.fromhex("ff5f00000093 4000".replace(" ", "")), 0x7FF, 0xFFFFFFFE)
# e: D/B = 0, G = 1 (byte 6 = 0x8f): a 16-bit stack-size SS; CR4 = 0x7ff (bits 0..10).
image("e-data-db0-g1-cr4-07ff", 0x10, flat(0x92, 0x8F), flat(0x93), 0x7FF, 0)
# f: the SME flag bit set: the jae is not taken, `mov ecx,0xc0010010` runs, the slice stops at the
#    RDMSR at 0x201f (HLT in the image). CR4 and CR3 are not reached.
image("f-sme-flag-stop-at-rdmsr-201f", 0x18, flat(0x92), flat(0x93), 0x6B0, 1, hlt_at=0x201F)
