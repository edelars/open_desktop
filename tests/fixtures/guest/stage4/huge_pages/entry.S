# Stage4HugePagesProbeV1: a 2 MiB RAM page and a 1 GiB (PDPTE.PS=1) negative, on the own kernel (S4-33e).
#
# Code lives on one 4 KiB page (GVA 0x400000, frame 0x7000). The data page is one 2 MiB leaf: the PDE
# at GVA 0x800000 has PS=1 and maps the 2 MiB frame at GPA 0x200000. Three different qwords sit at
# the start (offset 0), the middle (offset 0x100000) and the last qword (offset 0x1ffff8) of that
# frame; a fourth qword at offset 0x1fff00 holds a decoy that the case overwrites and reads back.
# The next 2 MiB region (GVA 0xa00000) maps frame 0x400000 and holds a sentinel qword, so an access
# one byte past the first page lands on a visibly different physical address.
# The negative route is a separate PML4E (GVA 0x8000000000) whose PDPT entry has PS=1: a 1 GiB
# mapping is not declared (CPUID Page1GB=0), so PS=1 in a PDPTE is a reserved-bit #PF.
# The CPU handoff (CPL0, RFLAGS.AC=0, CR0/CR4/EFER with NXE=1, RAX, RCX, RDX) is supplied by the
# loader; the sequencer supplies RBX per step. This image owns page tables, code and data.
#ifndef START_VALUE
#define START_VALUE 0x1122334455667788
#endif
#ifndef MIDDLE_VALUE
#define MIDDLE_VALUE 0x99aabbccddeeff01
#endif
#ifndef END_VALUE
#define END_VALUE 0x0f1e2d3c4b5a6978
#endif
.section .page_tables,"aw",@progbits
.balign 4096
stage4_pml4:
    .quad 0x2003                    # PML4E[0]: PDPT 0 (code and 2 MiB data)
    .quad 0x5003                    # PML4E[1]: PDPT 1 (1 GiB negative route)
    .zero 0xff0
stage4_pdpt0: .quad 0x3003; .zero 0xff8
stage4_pd:
    .zero 16
    .quad 0x4003                    # PDE[2], GVA 0x400000: code page table
    .zero 8
    .quad 0x200083 | (1 << 63)      # PDE[4], GVA 0x800000: 2 MiB leaf, P|RW|PS, NX, A and D clear
    .quad 0x400083 | (1 << 63)      # PDE[5], GVA 0xa00000: next 2 MiB leaf, frame 0x400000
    .zero 0xfd0
stage4_code_pt:
    .quad 0x7001                    # GVA 0x400000: code frame, supervisor read/execute
    .zero 0xff8
stage4_pdpt1:
    .quad 0x40000083                # PDPTE[0], GVA 0x8000000000: P|RW|PS=1, a 1 GiB leaf (reserved)
    .zero 0xff8
stage4_spare_pt: .zero 0x1000       # empty 4 KiB table, filled only by the PDE.PS injection
.section .text.code,"ax",@progbits
.global _start
.type _start,@function
_start:
    movq (%rbx), %rax               # 48 8b 03, GVA 0x400000, offset 0 of the 2 MiB page
    movq (%rbx), %rax               # 48 8b 03, GVA 0x400003, offset 0x100000
    movq (%rbx), %rax               # 48 8b 03, GVA 0x400006, offset 0x1ffff8
    movq %rcx, (%rbx)               # 48 89 0b, GVA 0x400009, offset 0x1fff00: sets D on the PDE
    movq (%rbx), %rax               # 48 8b 03, GVA 0x40000c, reads the stored value back
    outb %al, %dx                   # ee, GVA 0x40000f, result byte to the UART
    hlt                             # f4, GVA 0x400010
.section .decoy.frame,"aw",@progbits
    .quad 0xa5a5a5a5a5a5a5a5        # frame 0x8000, mapped only by the PDE.PS injection
.section .huge.start,"aw",@progbits
    .quad START_VALUE
.section .huge.middle,"aw",@progbits
    .quad MIDDLE_VALUE
.section .huge.store,"aw",@progbits
    .quad 0x5a5a5a5a5a5a5a5a        # overwritten by the case's store
.section .huge.end,"aw",@progbits
    .quad END_VALUE
.section .huge.sentinel,"aw",@progbits
    .quad 0xfeedface0badf00d        # frame 0x400000, the page after the first 2 MiB page
