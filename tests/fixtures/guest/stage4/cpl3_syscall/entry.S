# Stage4Cpl3SyscallProbeV1: CPL3 SYSCALL -> CPL0 kernel store -> SYSRETQ -> CPL3 load.
# The CPU handoff (CS=0x23, STAR/LSTAR/FMASK, RDI/RBX/R10) is supplied by the loader;
# this image owns only page tables, user code, kernel code and the data qwords.
#ifndef PROBE_INITIAL
#define PROBE_INITIAL 0x1122334455667788
#endif
#ifndef PROBE_STORE_VALUE
#define PROBE_STORE_VALUE 0x8877665544332211
#endif
.section .page_tables,"aw",@progbits
.balign 4096
stage4_pml4: .quad 0x2007; .zero 0xff8
stage4_pdpt: .quad 0x3007; .zero 0xff8
stage4_pd:
    .zero 16
    .quad 0x4007            # GVA 0x400000: user code table
    .quad 0x6003            # GVA 0x600000: kernel code table (supervisor)
    .quad 0x5007            # GVA 0x800000: data table
    .zero 0xfd8
stage4_user_pt: .quad 0x7005; .zero 0xff8
stage4_data_pt:
    .quad (0x9003 | (1 << 63))   # supervisor RW alias
    .quad (0x9005 | (1 << 63))   # user RO alias of the same GPA
    .zero 0xff0
stage4_kernel_pt: .quad 0x8001; .zero 0xff8
.section .text.user,"ax",@progbits
.global _start
.type _start,@function
_start:
    syscall                  # 0f 05
    movq (%rbx), %rax        # 48 8b 03, user load through the RO alias
.size _start, .-_start
.section .text.kernel,"ax",@progbits
.global stage4_syscall_entry
.type stage4_syscall_entry,@function
stage4_syscall_entry:
    movq %r10, (%rdi)        # 4c 89 17, kernel store through the RW alias
    sysretq                  # 48 0f 07
.size stage4_syscall_entry, .-stage4_syscall_entry
.section .data.payload,"aw",@progbits
stage4_initial: .quad PROBE_INITIAL
stage4_store: .quad PROBE_STORE_VALUE
