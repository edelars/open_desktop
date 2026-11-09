# Stage4CrossPageProbeV1: reads that cross 4 KiB pages, on the own kernel (S4-33d).
#
# GVA 0x800ffd holds three value bytes at the end of data page 0; the other five bytes of the
# little-endian qword are at the start of data page 1 (GVA 0x801000). The two data pages map to
# non-contiguous RAM frames. The first instruction (66 8B 03) straddles the boundary of two
# non-contiguous code frames: 66 8B end the first frame, 03 starts the second. Its word operand
# (0x800ffd, 0x800ffe) stays on data page 0. The CPU handoff (CPL0, RFLAGS.AC=0, CR0/CR4/EFER
# with NXE=1, RBX, RDX, RAX) is supplied by the loader; this image owns page tables, code and data.
#ifndef PROBE_VALUE
#define PROBE_VALUE 0x8877665544332211
#endif
.section .page_tables,"aw",@progbits
.balign 4096
stage4_pml4: .quad 0x2003; .zero 0xff8
stage4_pdpt: .quad 0x3003; .zero 0xff8
stage4_pd:
    .zero 16
    .quad 0x4003            # GVA 0x400000: code table
    .zero 8
    .quad 0x5003            # GVA 0x800000: data table
    .zero 0xfd8
stage4_code_pt:
    .quad 0x7001            # GVA 0x400000: first code frame, supervisor read/execute
    .quad 0x9001            # GVA 0x401000: second code frame, not adjacent to the first
    .zero 0xff0
stage4_data_pt:
    .quad 0xb003 | (1 << 63)  # GVA 0x800000: first data frame, supervisor RW, NX
    .quad 0xd003 | (1 << 63)  # GVA 0x801000: second data frame, not adjacent to the first
    .zero 0xff0
.section .text.head,"ax",@progbits
.global _start
.type _start,@function
_start:
    .byte 0x66, 0x8b         # first two bytes of MOV AX,[RBX] (66 8B 03), end of the first frame
.section .text.tail,"ax",@progbits
    .byte 0x03               # ModRM of the crossing MOV AX,[RBX], first byte of the second frame
    movq (%rbx), %rax        # 48 8b 03, qword read across the data page boundary
    movl (%rbx), %eax        # 8b 03, dword read across the data page boundary
    movw (%rbx), %ax         # 66 8b 03, word read inside data page 0
    outb %al, %dx            # ee, result byte to the UART
    hlt                      # f4
.section .data0.decoy,"aw",@progbits
    .quad 0xa5a5a5a5a5a5a5a5
.section .data0.control,"aw",@progbits
    .quad PROBE_VALUE        # same-page control copy of the value
.section .data0.value,"aw",@progbits
    .byte (PROBE_VALUE) & 0xff, (PROBE_VALUE >> 8) & 0xff, (PROBE_VALUE >> 16) & 0xff
.section .data1.value,"aw",@progbits
    .byte (PROBE_VALUE >> 24) & 0xff, (PROBE_VALUE >> 32) & 0xff, (PROBE_VALUE >> 40) & 0xff
    .byte (PROBE_VALUE >> 48) & 0xff, (PROBE_VALUE >> 56) & 0xff
.section .data1.decoy,"aw",@progbits
    .quad 0x5a5a5a5a5a5a5a5a
