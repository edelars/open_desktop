#!/usr/bin/env python3
"""S6-14j oracle images (run from this directory). All images are entered at 0000:0000 (GPA 0).

Layout, as in s6-14i image a: `jmp far 0:0x1027` at 0, GDT pointer at 0x30 (limit 0x1f, base 0x38),
GDT at 0x38 (selector 8 = linear 0x40), the verbatim linked bytes 0x1027..0x1044 of
16bit-objdump.txt (test eax,eax; jne 0x104c; lidtd ds:0x60; lgdtd ds:0x30; mov dx,0x18;
mov eax,0x50033; mov cr0,eax), the block 5 far jump `66 ea 00 20 00 00 SS SS` (jmp SS:0x2000)
at 0x1044, HLT at 0x104c (the jne failure target, as in the linked blob) and HLT at CS.base+0x2000.
"""
import struct

BLOCK_1027 = bytes.fromhex(
    "668 5c0 7520 660f011e6000 660f01163000 ba1800 66b833000500 0f22c0".replace(" ", "")
)
assert len(BLOCK_1027) == 0x1044 - 0x1027


def image(sel, desc8, halt_at, name):
    img = bytearray(halt_at + 1)
    img[0:5] = bytes([0xEA, 0x27, 0x10, 0x00, 0x00])  # jmp far 0:0x1027
    img[0x30:0x36] = struct.pack("<HI", 0x1F, 0x38)  # GDT pointer: limit 0x1f, base 0x38
    # GDT: null, [selector 8] = desc8, data 0x00CF92000000FFFF (A clear), second code 0x00CF9A.
    img[0x38:0x40] = bytes(8)
    img[0x40:0x48] = desc8
    img[0x48:0x50] = bytes.fromhex("ffff00000092cf00")
    img[0x50:0x58] = bytes.fromhex("ffff00000093cf00")
    img[0x1027:0x1044] = BLOCK_1027
    img[0x1044:0x104C] = bytes([0x66, 0xEA, 0x00, 0x20, 0x00, 0x00]) + struct.pack("<H", sel)
    img[0x104C] = 0xF4
    img[halt_at] = 0xF4
    open(name + "-0000-0000.bin", "wb").write(img)


flat_code = lambda access: bytes.fromhex("ffff0000") + bytes([0x00, access, 0xCF, 0x00])
# a: flat code, accessed clear (0x9A): hardware sets A in memory (0x9B) and in the cache.
image(0x08, flat_code(0x9A), 0x2000, "a-flat-code-accessed-clear")
# b: the same with the accessed bit already set (0x9B): memory unchanged.
image(0x08, flat_code(0x9B), 0x2000, "b-flat-code-accessed-set")
# c: 16-bit byte-granular code: base 0x1000, limit 0x2fff, D = 0, G = 0: HLT at linear 0x3000.
image(0x08, bytes.fromhex("ff2f0010009a0000"), 0x3000, "c-small-16bit-code-base-1000")
# d: G = 1, D = 1, raw limit 2 (pages): limit 0x2fff, base 0x1000: HLT at linear 0x3000.
image(0x08, bytes.fromhex("0200001000" "9a" "c0" "00"), 0x3000, "d-page-granular-d1-base-1000")
# e: conforming code (0x9e, DPL 0) entered with RPL 2 (selector 0x0a): CS.selector becomes 0x08.
image(0x0A, flat_code(0x9E), 0x2000, "e-conforming-rpl2")
# f: G = 1, D = 1, AVL = 1, raw limit 0x0ffff (byte 6 = 0xd0; limit (0xffff << 12) | 0xfff = 0x0fffffff), base 0x8000: HLT at linear 0xa000.
image(0x08, bytes.fromhex("ffff0080009ad000"), 0xA000, "f-avl-g-d-base-8000-limit-0fffffff")
