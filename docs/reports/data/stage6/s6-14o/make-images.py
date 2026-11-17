#!/usr/bin/env python3
"""S6-14o oracle images (run from anywhere; writes next to this file).

Two images, both loaded by kvm_runner at GPA 0x8000 and entered at `0800:0000` (the state an AP has
after SIPI vector 0x08; nothing below 0x8000 is used by the image):

  a-full-s6-14a-path-0800-0000.bin   the complete connected path of the S6-14a table, rows 1..8,
                                     written here from the instruction forms of crates/od-isa
                                     (real_mode_exec.rs, protected_exec.rs, long_mode_entry.rs,
                                     paging.rs). Run with `--cpuid-profile od-x86-64-v1-neutral`.
  b-cpuid-probe-0800-0000.bin        a diagnostic: stores the registers CPUID returns for a list of
                                     leaves, then HLT (what the guest really sees through KVM).

The path of image `a` (offsets are CS-relative inside the real-mode block, 0x8000 + offset):

  row 1  0x00 CLI | WBINVD | JMP FAR 0800:0008 | MOV AX,CS | MOV DS/ES/SS,AX | LOCK BTS ds:LOCK,0 |
         JAE | (PAUSE; JMP back) | MOV ESP,0x2000 | CALL verify_cpu
  row 2  0x80 verify_cpu: PUSHF/PUSH 0/POPF, PUSHFD/POP/MOV EBX,EAX/XOR/PUSH/POPFD/PUSHFD/POP/CMP/JE,
         CPUID leaf 0, CMP/JB, XOR DI,DI, CMP EBX/JNE x2 (vendor dispatcher), CPUID 1 + AND/XOR EDX + JNE, CPUID 0x80000000 + CMP + JB,
         CPUID 0x80000001 + AND/XOR EDX + JNE, CPUID 1 + AND/CMP EDX + JE, POPF, XOR EAX,EAX, RET.
         The vendor dispatcher IS written: CMP EBX,'Auth' + JNE (taken) and CMP EBX,'Genu' + JNE
         (taken) run on the path of the profile OdX8664V1Neutral (the OpenDesktop vendor is
         neither AMD nor Intel, so each comparison is executed and the branch is bypassed). Only
         the bodies of the AMD and Intel branches (the EDX/ECX comparisons, RDMSR, WRMSR, BTR)
         are not written: they are not executed and are INT3.
  row 3  0x27 TEST EAX,EAX | JNE halt (not taken)
  row 4  0x2c LIDT/LGDT (66 0F 01) | MOV DX,0x18 | MOV EAX,0x50033 | MOV CR0,EAX
  row 5  0x44 66 EA far jump to 0x08:0x8200
  row 6  0x8200 MOV SS/DS/ES/FS/GS,EDX | ADD ESP,0 | BT ds:SME,0 | JAE (SME branch skipped; its 15
         bytes are INT3) | MOV EAX,ds:CR4 ; MOV CR4,EAX | MOV EAX,PML4 ; MOV CR3,EAX | RDMSR EFER |
         CMP/JNE/CMP/JE | MOV EAX,ds:EFER_LO ; MOV EDX,ds:EFER_HI ; WRMSR | MOV EAX,0x80050033 ;
         MOV CR0,EAX (PG)
  row 7  EA far jump to 0x10:0x8300 (64-bit code segment)
  row 8  0x8300 FF 25 JMP [rip+disp] through the cell at 0x8400 to the target 0xB000; the target is
         HLT (the oracle stops there; the model stops before it: `Mode64TargetEntered`).

Header cells (the layout of the linked blob header 0x5000.. moved to 0x8400..): +0 jump cell, +8
EFER low, +0xc EFER high, +0x10 CR4, +0x14 SME flag (0), +0x18 LOCK cell. GDTR descriptor at 0x8430,
IDTR descriptor at 0x8460, GDT at 0x9000 (null, code32 0x08, code64 0x10, data 0x18), stack top
SS:SP = 0800:2000 (linear 0xA000). Page tables: PML4 0x10000, PDPT 0x11000, PD 0x12000, PT 0x13000
(identity 4 KiB pages 0..0x1F; the target page 0xB, the code and cell page 0x8 and the GDT page 0x9
(the far jump reads the GDT with paging already on) get A).
"""
import hashlib
import os
import struct

HERE = os.path.dirname(os.path.abspath(__file__))

ENTRY_GPA = 0x8000
IMAGE_END = 0x14000
CS = 0x0800  # SIPI vector 0x08

VERIFY_AT = 0x8080
PROTECTED_AT = 0x8200
COMPAT_AT = 0x8300
CELLS_AT = 0x8400
JUMP_CELL = CELLS_AT + 0x00
EFER_LO_AT = CELLS_AT + 0x08
EFER_HI_AT = CELLS_AT + 0x0C
CR4_AT = CELLS_AT + 0x10
SME_AT = CELLS_AT + 0x14
LOCK_AT = CELLS_AT + 0x18
GDTR_AT = CELLS_AT + 0x30
IDTR_AT = CELLS_AT + 0x60
GDT_AT = 0x9000
IDT_AT = 0x9800
IDT_LIMIT = 0x3FF
STACK_SP = 0x2000  # SS:SP, linear 0x8000 + 0x2000
TARGET = 0xB000
PML4, PDPT, PD, PT = 0x10000, 0x11000, 0x12000, 0x13000
PT_PAGES = 0x20

CR4_VALUE = 0x6B0  # OSXMMEXCPT | OSFXSR | PGE | PAE | PSE
EFER_VALUE = 0x901  # SCE | LME | NXE (the neutral table declares SYSCALL, NX, LM)
CR0_PE = 0x50033
CR0_PG = 0x80050033

# Flat code32, code64 (L=1), flat data with the accessed bit clear.
GDT = (
    bytes(8)
    + bytes([0xFF, 0xFF, 0, 0, 0, 0x9B, 0xCF, 0])
    + bytes([0xFF, 0xFF, 0, 0, 0, 0x9B, 0xAF, 0])
    + bytes([0xFF, 0xFF, 0, 0, 0, 0x92, 0xCF, 0])
)


def le32(v: int) -> bytes:
    return struct.pack("<I", v & 0xFFFFFFFF)


def le16(v: int) -> bytes:
    return struct.pack("<H", v & 0xFFFF)


class Asm:
    """Straight-line code with forward/backward short and near jumps, resolved at `assemble`."""

    def __init__(self, base: int) -> None:
        self.base = base
        self.items: list = []  # bytes | ("j8", opcode_bytes, label) | ("j16", opcode_bytes, label)
        self.labels: dict[str, int] = {}
        self.pos = 0

    def raw(self, data: bytes) -> None:
        self.items.append(data)
        self.pos += len(data)

    def label(self, name: str) -> None:
        assert name not in self.labels
        self.labels[name] = self.pos

    def j8(self, opcode: int, label: str) -> None:
        self.items.append(("j8", bytes([opcode]), label, self.pos))
        self.pos += 2

    def j16(self, opcode: bytes, label: str) -> None:
        self.items.append(("j16", opcode, label, self.pos))
        self.pos += len(opcode) + 2

    def assemble(self) -> bytes:
        out = bytearray()
        for item in self.items:
            if isinstance(item, bytes):
                out += item
                continue
            kind, opcode, label, at = item
            target = self.labels[label]
            if kind == "j8":
                rel = target - (at + 2)
                assert -128 <= rel <= 127, (label, rel)
                out += opcode + struct.pack("<b", rel)
            else:
                rel = target - (at + len(opcode) + 2)
                assert -32768 <= rel <= 32767
                out += opcode + struct.pack("<h", rel)
        assert len(out) == self.pos
        return bytes(out)


def verify_cpu() -> bytes:
    """Row 2: the success path of verify_cpu for the vendor `OpenDesktop ` (forms of S6-14g)."""
    a = Asm(VERIFY_AT)
    a.raw(bytes([0x9C]))  # pushf
    a.raw(bytes([0x6A, 0x00]))  # push 0
    a.raw(bytes([0x9D]))  # popf
    a.raw(bytes([0x66, 0x9C]))  # pushfd
    a.raw(bytes([0x66, 0x58]))  # pop eax
    a.raw(bytes([0x66, 0x89, 0xC3]))  # mov ebx,eax
    a.raw(bytes([0x66, 0x35]) + le32(0x200000))  # xor eax,0x200000
    a.raw(bytes([0x66, 0x50]))  # push eax
    a.raw(bytes([0x66, 0x9D]))  # popfd
    a.raw(bytes([0x66, 0x9C]))  # pushfd
    a.raw(bytes([0x66, 0x58]))  # pop eax
    a.raw(bytes([0x66, 0x39, 0xC3]))  # cmp ebx,eax
    a.j16(bytes([0x0F, 0x84]), "fail")  # je fail
    a.raw(bytes([0x66, 0xB8]) + le32(0))  # mov eax,0
    a.raw(bytes([0x0F, 0xA2]))  # cpuid (leaf 0)
    a.raw(bytes([0x66, 0x83, 0xF8, 0x01]))  # cmp eax,1
    a.j16(bytes([0x0F, 0x82]), "fail")  # jb fail
    a.raw(bytes([0x31, 0xFF]))  # xor di,di
    # Vendor dispatcher (blob 0x10b2..0x10d9): the OpenDesktop vendor is neither AMD nor Intel, so
    # both comparisons are executed and both JNE are taken; the not-taken continuations (the AMD
    # and Intel branches, 23 and 81 bytes in the blob) are never executed and are INT3 here.
    a.raw(bytes([0x66, 0x81, 0xFB]) + le32(0x68747541))  # cmp ebx,'Auth'
    a.j8(0x75, "not_amd")  # jne not_amd (taken)
    a.raw(bytes([0xCC]) * 23)  # AMD branch (not executed)
    a.label("not_amd")
    a.raw(bytes([0x66, 0x81, 0xFB]) + le32(0x756E6547))  # cmp ebx,'Genu'
    a.j8(0x75, "common")  # jne common (taken)
    a.raw(bytes([0xCC]) * 81)  # Intel branch (not executed)
    a.label("common")
    a.raw(bytes([0x66, 0xB8]) + le32(1))  # mov eax,1
    a.raw(bytes([0x0F, 0xA2]))  # cpuid (leaf 1)
    a.raw(bytes([0x66, 0x81, 0xE2]) + le32(0x07008161))  # and edx,0x7008161
    a.raw(bytes([0x66, 0x81, 0xF2]) + le32(0x07008161))  # xor edx,0x7008161
    a.j8(0x75, "fail")  # jne fail (not taken)
    a.raw(bytes([0x66, 0xB8]) + le32(0x80000000))  # mov eax,0x80000000
    a.raw(bytes([0x0F, 0xA2]))  # cpuid
    a.raw(bytes([0x66, 0x3D]) + le32(0x80000001))  # cmp eax,0x80000001
    a.j8(0x72, "fail")  # jb fail
    a.raw(bytes([0x66, 0xB8]) + le32(0x80000001))  # mov eax,0x80000001
    a.raw(bytes([0x0F, 0xA2]))  # cpuid
    a.raw(bytes([0x66, 0x81, 0xE2]) + le32(0x20000000))  # and edx,0x20000000
    a.raw(bytes([0x66, 0x81, 0xF2]) + le32(0x20000000))  # xor edx,0x20000000
    a.j8(0x75, "fail")  # jne fail (not taken)
    a.raw(bytes([0x66, 0xB8]) + le32(1))  # mov eax,1
    a.raw(bytes([0x0F, 0xA2]))  # cpuid
    a.raw(bytes([0x66, 0x81, 0xE2]) + le32(0x06000000))  # and edx,0x6000000
    a.raw(bytes([0x66, 0x81, 0xFA]) + le32(0x06000000))  # cmp edx,0x6000000
    a.j8(0x74, "ok")  # je ok (taken)
    a.label("fail")
    a.raw(bytes([0x9D]))  # popf
    a.raw(bytes([0x66, 0xB8]) + le32(1))  # mov eax,1
    a.raw(bytes([0xC3]))  # ret
    a.raw(bytes([0xCC]))  # int3 (alignment, never executed)
    a.label("ok")
    a.raw(bytes([0x9D]))  # popf
    a.raw(bytes([0x66, 0x31, 0xC0]))  # xor eax,eax
    a.raw(bytes([0xC3]))  # ret
    return a.assemble()


def real_block() -> bytes:
    """Rows 1, 3, 4, 5 at CS:0 (blob offsets 0x1000.. moved to 0x00..)."""
    code = bytearray()

    def at(offset: int) -> None:
        assert len(code) == offset, (hex(len(code)), hex(offset))

    code += bytes([0xFA])  # 0x00 cli
    code += bytes([0x0F, 0x09])  # 0x01 wbinvd
    code += bytes([0xEA]) + le16(0x0008) + le16(CS)  # 0x03 jmp 0x0800:0x0008
    at(0x08)
    code += bytes([0x8C, 0xC8])  # mov ax,cs
    code += bytes([0x8E, 0xD8])  # mov ds,ax
    code += bytes([0x8E, 0xC0])  # mov es,ax
    code += bytes([0x8E, 0xD0])  # mov ss,ax
    at(0x10)
    code += bytes([0x66, 0xF0, 0x0F, 0xBA, 0x2E]) + le16(LOCK_AT - ENTRY_GPA) + bytes([0x00])
    at(0x18)
    code += bytes([0x73, 0x04])  # jae 0x1e
    code += bytes([0xF3, 0x90])  # pause
    code += bytes([0xEB, 0xF2])  # jmp 0x10
    at(0x1E)
    code += bytes([0x66, 0xBC]) + le32(STACK_SP)  # mov esp,0x2000
    at(0x24)
    rel = VERIFY_AT - ENTRY_GPA - (0x24 + 3)
    code += bytes([0xE8]) + struct.pack("<h", rel)  # call verify_cpu
    at(0x27)
    code += bytes([0x66, 0x85, 0xC0])  # test eax,eax
    code += bytes([0x75, 0x20])  # jne 0x4c
    at(0x2C)
    code += bytes([0x66, 0x0F, 0x01, 0x1E]) + le16(IDTR_AT - ENTRY_GPA)  # lidt ds:IDTR
    code += bytes([0x66, 0x0F, 0x01, 0x16]) + le16(GDTR_AT - ENTRY_GPA)  # lgdt ds:GDTR
    at(0x38)
    code += bytes([0xBA, 0x18, 0x00])  # mov dx,0x18
    code += bytes([0x66, 0xB8]) + le32(CR0_PE)  # mov eax,0x50033
    code += bytes([0x0F, 0x22, 0xC0])  # mov cr0,eax
    at(0x44)
    code += bytes([0x66, 0xEA]) + le32(PROTECTED_AT) + le16(0x08)  # jmp 0x08:0x8200
    at(0x4C)
    code += bytes([0xF4])  # hlt
    code += bytes([0xEB, 0xFD])  # jmp 0x4c
    return bytes(code)


def protected_block() -> bytes:
    """Rows 6 and 7 at 0x8200 (flat 32-bit code)."""
    a = Asm(PROTECTED_AT)
    a.raw(bytes([0x8E, 0xD2]))  # mov ss,edx
    a.raw(bytes([0x81, 0xC4]) + le32(0))  # add esp,0
    a.raw(bytes([0x8E, 0xDA]))  # mov ds,edx
    a.raw(bytes([0x8E, 0xC2]))  # mov es,edx
    a.raw(bytes([0x8E, 0xE2]))  # mov fs,edx
    a.raw(bytes([0x8E, 0xEA]))  # mov gs,edx
    a.raw(bytes([0x0F, 0xBA, 0x25]) + le32(SME_AT) + bytes([0x00]))  # bt ds:SME,0
    a.raw(bytes([0x73, 0x0F]))  # jae over the SME branch (15 bytes, INT3 here)
    a.raw(bytes([0xCC]) * 15)
    a.raw(bytes([0xA1]) + le32(CR4_AT))  # mov eax,ds:CR4
    a.raw(bytes([0x0F, 0x22, 0xE0]))  # mov cr4,eax
    a.raw(bytes([0xB8]) + le32(PML4))  # mov eax,PML4
    a.raw(bytes([0x0F, 0x22, 0xD8]))  # mov cr3,eax
    a.raw(bytes([0xB9]) + le32(0xC0000080))  # mov ecx,EFER
    a.raw(bytes([0x0F, 0x32]))  # rdmsr
    a.raw(bytes([0x3B, 0x05]) + le32(EFER_LO_AT))  # cmp eax,ds:EFER_LO
    a.raw(bytes([0x75, 0x08]))  # jne write
    a.raw(bytes([0x3B, 0x15]) + le32(EFER_HI_AT))  # cmp edx,ds:EFER_HI
    a.raw(bytes([0x74, 0x0D]))  # je skip
    a.raw(bytes([0xA1]) + le32(EFER_LO_AT))  # write: mov eax,ds:EFER_LO
    a.raw(bytes([0x8B, 0x15]) + le32(EFER_HI_AT))  # mov edx,ds:EFER_HI
    a.raw(bytes([0x0F, 0x30]))  # wrmsr
    a.raw(bytes([0xB8]) + le32(CR0_PG))  # skip: mov eax,0x80050033
    a.raw(bytes([0x0F, 0x22, 0xC0]))  # mov cr0,eax
    a.raw(bytes([0xEA]) + le32(COMPAT_AT) + le16(0x10))  # jmp 0x10:0x8300
    return a.assemble()


def compat_block() -> bytes:
    """Row 8 at 0x8300: JMP [rip+disp] through the jump cell."""
    return bytes([0xFF, 0x25]) + struct.pack("<i", JUMP_CELL - (COMPAT_AT + 6))


def place(image: bytearray, gpa: int, data: bytes) -> None:
    start = gpa - ENTRY_GPA
    assert start >= 0 and start + len(data) <= len(image), hex(gpa)
    assert not any(image[start : start + len(data)]), f"overlap at {gpa:#x}"
    image[start : start + len(data)] = data


def build_full() -> bytes:
    image = bytearray(IMAGE_END - ENTRY_GPA)
    real = real_block()
    assert len(real) == 0x4F and ENTRY_GPA + len(real) <= VERIFY_AT
    place(image, ENTRY_GPA, real)
    verify = verify_cpu()
    assert VERIFY_AT + len(verify) <= PROTECTED_AT
    place(image, VERIFY_AT, verify)
    protected = protected_block()
    assert PROTECTED_AT + len(protected) <= COMPAT_AT
    place(image, PROTECTED_AT, protected)
    place(image, COMPAT_AT, compat_block())
    place(image, JUMP_CELL, struct.pack("<Q", TARGET))
    place(image, EFER_LO_AT, le32(EFER_VALUE) + le32(0))
    place(image, CR4_AT, le32(CR4_VALUE))
    place(image, GDTR_AT, struct.pack("<HI", len(GDT) - 1, GDT_AT))
    place(image, IDTR_AT, struct.pack("<HI", IDT_LIMIT, IDT_AT))
    place(image, GDT_AT, GDT)
    place(image, TARGET, bytes([0xF4]))  # hlt: the oracle stops at the first 64-bit instruction
    place(image, PML4, struct.pack("<Q", PDPT | 3))
    place(image, PDPT, struct.pack("<Q", PD | 3))
    place(image, PD, struct.pack("<Q", PT | 3))
    place(image, PT, b"".join(struct.pack("<Q", (i << 12) | 3) for i in range(PT_PAGES)))
    return bytes(image)


PROBE_LEAVES = [0, 1, 2, 4, 7, 0xB, 0x40000000, 0x80000000, 0x80000001, 0x80000002, 0x80000008,
                0x80000009]
PROBE_OUT = 0x8200  #16 bytes per leaf: EAX, EBX, ECX, EDX


def build_probe() -> bytes:
    """CPUID diagnostic in real mode: results at PROBE_OUT = 0x8200 + 16 * index, then HLT."""
    code = bytearray()
    code += bytes([0xFA])  # cli
    code += bytes([0x8C, 0xC8, 0x8E, 0xD8])  # mov ax,cs ; mov ds,ax
    for index, leaf in enumerate(PROBE_LEAVES):
        out = PROBE_OUT - ENTRY_GPA + 16 * index
        code += bytes([0x66, 0xB8]) + le32(leaf)  # mov eax,leaf
        code += bytes([0x66, 0x31, 0xC9])  # xor ecx,ecx
        code += bytes([0x0F, 0xA2])  # cpuid
        code += bytes([0x66, 0xA3]) + le16(out)  # mov ds:out,eax
        code += bytes([0x66, 0x89, 0x1E]) + le16(out + 4)  # mov ds:out+4,ebx
        code += bytes([0x66, 0x89, 0x0E]) + le16(out + 8)  # mov ds:out+8,ecx
        code += bytes([0x66, 0x89, 0x16]) + le16(out + 12)  # mov ds:out+12,edx
    code += bytes([0xF4])  # hlt
    assert len(code) < PROBE_OUT - ENTRY_GPA
    image = bytearray(PROBE_OUT - ENTRY_GPA + 16 * len(PROBE_LEAVES))
    image[: len(code)] = code
    return bytes(image)


IMAGES = {
    "a-full-s6-14a-path-0800-0000.bin": build_full,
    "b-cpuid-probe-0800-0000.bin": build_probe,
}


def main() -> None:
    for name, build in IMAGES.items():
        data = build()
        again = build()
        assert data == again, "the builder is not deterministic"
        with open(os.path.join(HERE, name), "wb") as f:
            f.write(data)
        print(name, len(data), hashlib.sha256(data).hexdigest())


if __name__ == "__main__":
    main()
