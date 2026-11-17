#!/usr/bin/env python3
"""Unit tests of make-images.py (run: python3 -m unittest test_make_images, from this directory)."""
import hashlib
import importlib.util
import os
import struct
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("make_images", os.path.join(HERE, "make-images.py"))
mi = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mi)

SHA_A = "a35c1a827c00a7d0a7c89817153b7d5fb588d2955ad84722f5ab99947db8417a"
SHA_B = "bbf86bf5455ae3ad0a955c307fa1a2a1dbe9f625bc8ec87a5072752bce71434d"


def at(image: bytes, gpa: int, n: int) -> bytes:
    return image[gpa - mi.ENTRY_GPA : gpa - mi.ENTRY_GPA + n]


class MakeImages(unittest.TestCase):
    def test_images_are_reproducible_and_match_the_committed_files(self):
        for name, sha in (("a-full-s6-14a-path-0800-0000.bin", SHA_A), ("b-cpuid-probe-0800-0000.bin", SHA_B)):
            built = mi.IMAGES[name]()
            self.assertEqual(built, mi.IMAGES[name]())
            self.assertEqual(hashlib.sha256(built).hexdigest(), sha)
            with open(os.path.join(HERE, name), "rb") as f:
                self.assertEqual(f.read(), built)

    def test_row_forms_and_offsets(self):
        img = mi.build_full()
        self.assertEqual(len(img), 0x14000 - 0x8000)
        real = at(img, 0x8000, 0x4F)
        self.assertEqual(real[0:3], bytes([0xFA, 0x0F, 0x09]))  # CLI, WBINVD
        self.assertEqual(real[3:8], bytes([0xEA, 0x08, 0x00, 0x00, 0x08]))  # JMP FAR 0800:0008
        self.assertEqual(real[0x10:0x15], bytes([0x66, 0xF0, 0x0F, 0xBA, 0x2E]))  # LOCK BTS
        self.assertEqual(struct.unpack("<H", real[0x15:0x17])[0], mi.LOCK_AT - 0x8000)
        self.assertEqual(real[0x18:0x1A], bytes([0x73, 0x04]))  # JAE
        self.assertEqual(real[0x24:0x27], bytes([0xE8, 0x59, 0x00]))  # CALL verify_cpu at 0x80
        self.assertEqual(real[0x27:0x2C], bytes([0x66, 0x85, 0xC0, 0x75, 0x20]))  # TEST, JNE 0x4c
        self.assertEqual(real[0x4C], 0xF4)
        self.assertEqual(real[0x44:0x4C], bytes([0x66, 0xEA]) + struct.pack("<I", 0x8200) + bytes([0x08, 0x00]))
        # Row 7 and row 8.
        protected = mi.protected_block()
        self.assertEqual(at(img, mi.PROTECTED_AT, len(protected)), protected)
        self.assertEqual(protected[-7:], bytes([0xEA]) + struct.pack("<I", 0x8300) + bytes([0x10, 0x00]))
        self.assertEqual(at(img, mi.COMPAT_AT, 6), bytes([0xFF, 0x25]) + struct.pack("<i", 0x8400 - 0x8306))
        self.assertEqual(struct.unpack("<Q", at(img, mi.JUMP_CELL, 8))[0], mi.TARGET)
        self.assertEqual(at(img, mi.TARGET, 1), b"\xf4")

    def test_verify_cpu_branches_resolve_to_the_success_path(self):
        v = mi.verify_cpu()
        ok = len(v) - 5  # POPF; XOR EAX,EAX; RET
        self.assertEqual(v[ok : ok + 5], bytes([0x9D, 0x66, 0x31, 0xC0, 0xC3]))
        # The last conditional jump (JE) lands on `ok`; the first two near jumps on the failure tail.
        je = v.rfind(bytes([0x74]), 0, ok)
        self.assertEqual(je + 2 + v[je + 1], ok)
        fail = ok - 9  # POPF; MOV EAX,1; RET; INT3
        self.assertEqual(v[fail : fail + 9], bytes([0x9D, 0x66, 0xB8, 1, 0, 0, 0, 0xC3, 0xCC]))
        # The first near JE (0F 84, after CMP EBX,EAX) lands on `fail`.
        je = v.find(bytes([0x0F, 0x84]))
        self.assertEqual(je + 4 + struct.unpack("<h", v[je + 2 : je + 4])[0], fail)

    def test_verify_cpu_has_the_vendor_dispatcher(self):
        v = mi.verify_cpu()
        self.assertEqual(len(v), 0x112)  # the last RET at 0x8191
        d = 0x80B2 - mi.VERIFY_AT  # the blob 0x10b2, the image 0x80b2
        self.assertEqual(v[d - 2 : d], bytes([0x31, 0xFF]))  # XOR DI,DI just before
        self.assertEqual(v[d : d + 9], bytes([0x66, 0x81, 0xFB]) + struct.pack("<I", 0x68747541) + bytes([0x75, 0x17]))
        self.assertEqual(v[d + 9 : d + 32], bytes([0xCC]) * 23)  # AMD branch, not executed
        g = d + 32  # 0x80d2
        self.assertEqual(v[g : g + 9], bytes([0x66, 0x81, 0xFB]) + struct.pack("<I", 0x756E6547) + bytes([0x75, 0x51]))
        self.assertEqual(v[g + 9 : g + 90], bytes([0xCC]) * 81)  # Intel branch, not executed
        # Both JNE land on `mov eax,1` (0x812c) that precedes the CPUID leaf 1 of the common tail.
        self.assertEqual(v[g + 90 : g + 96], bytes([0x66, 0xB8, 1, 0, 0, 0]))
        self.assertEqual(mi.VERIFY_AT + g + 90, 0x812C)

    def test_tables_and_header(self):
        img = mi.build_full()
        self.assertEqual(struct.unpack("<Q", at(img, mi.PML4, 8))[0], mi.PDPT | 3)
        self.assertEqual(struct.unpack("<Q", at(img, mi.PDPT, 8))[0], mi.PD | 3)
        self.assertEqual(struct.unpack("<Q", at(img, mi.PD, 8))[0], mi.PT | 3)
        for i in range(mi.PT_PAGES):
            self.assertEqual(struct.unpack("<Q", at(img, mi.PT + 8 * i, 8))[0], (i << 12) | 3)
        self.assertEqual(struct.unpack("<II", at(img, mi.EFER_LO_AT, 8)), (0x901, 0))
        self.assertEqual(struct.unpack("<I", at(img, mi.CR4_AT, 4))[0], 0x6B0)
        self.assertEqual(struct.unpack("<I", at(img, mi.SME_AT, 4))[0], 0)  # SME flag clear: JAE taken
        self.assertEqual(at(img, mi.GDT_AT, 32), mi.GDT)
        self.assertEqual(struct.unpack("<HI", at(img, mi.GDTR_AT, 6)), (31, mi.GDT_AT))
        self.assertEqual(struct.unpack("<HI", at(img, mi.IDTR_AT, 6)), (0x3FF, mi.IDT_AT))


if __name__ == "__main__":
    unittest.main()
