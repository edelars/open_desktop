#ifndef PROBE_PAYLOAD
#define PROBE_PAYLOAD 0x5354473453544b31
#endif

.section .page_tables,"aw",@progbits
.balign 4096
stage4_pml4: .quad 0x2003; .zero 0xff8
stage4_pdpt: .quad 0x3003; .zero 0xff8
stage4_pd:
    .zero 16
    .quad 0x4003
    .zero 8
    .quad 0x5003
    .quad 0x6003
    .zero 0xfd0
stage4_code_pt: .quad 0x7001; .zero 0xff8
stage4_data_pt: .quad (0x9003 | (1 << 63)); .zero 0xff8
stage4_stack_pt: .quad (0xa003 | (1 << 63)); .zero 0xff8

.section .text.entry,"ax",@progbits
.global _start
.type _start,@function
_start:
    pushq %rcx
    popq %rax
    outb %al, %dx
    hlt
.global stage4_code_end
stage4_code_end:
.size _start, .-_start

.section .data.payload,"aw",@progbits
stage4_payload: .quad PROBE_PAYLOAD
