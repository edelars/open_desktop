/*
 * Stage4InterruptPumpProbeV1 guest kernel (S4-27i1).
 *
 * Flat identity-mapped Long64/CPL0 image for the event pump. The loader
 * installs IDTR; the guest only software-enables the Local APIC (SVR = 0x1ff)
 * and then runs one scenario selected at build time. Interrupt requests come
 * from the caller (through the APIC handle) or from guest MMIO (self-IPI).
 *
 * Vectors: 0x41 (priority class 4) is handled by stage4_handler_b (EOI, IRETQ).
 * 0x51 (class 5) is handled by stage4_handler_a, which raises a self-IPI for
 * 0x41, writes the EOI and returns with IRETQ.
 *
 * Build variants (assembler macros; the default is the accepted profile):
 *   PROBE_SCENARIO=<n>  0 sti_hlt:  STI; HLT; HLT
 *                       1 if1_hlt:  STI; NOP; HLT
 *                       2 cli_hlt:  CLI; HLT
 *                       3 rep:      STI; NOP; MOV ECX,2000; MOV EDI,0x5000; REP STOSB; HLT
 *   PROBE_OMIT_EOI=1    injected regression: both handlers skip the LAPIC EOI
 */
#ifndef PROBE_SCENARIO
#define PROBE_SCENARIO 0
#endif
#ifndef PROBE_OMIT_EOI
#define PROBE_OMIT_EOI 0
#endif

.section .gdt,"aw",@progbits
.global stage4_gdt
stage4_gdt:
    .quad 0x0000000000000000
    .quad 0x00af9b000000ffff
    .quad 0x00cf93000000ffff
.global stage4_gdt_end
stage4_gdt_end:

.macro gate handler
    .word \handler
    .word 0x0008
    .byte 0x00
    .byte 0x8e
    .word 0x0000
    .long 0x00000000
    .long 0x00000000
.endm

.section .idt,"aw",@progbits
.global stage4_idt
stage4_idt:
    .zero 0x41 * 16
    gate stage4_handler_b
    .zero (0x51 - 0x42) * 16
    gate stage4_handler_a
    .zero 0x1000 - 0x52 * 16

.macro eoi
#if !PROBE_OMIT_EOI
    movl $0, %eax
    movl %eax, 0xb0(%rdi)
#endif
.endm

.section .text.entry,"ax",@progbits
.global _start
.type _start,@function
_start:
    /* Local APIC software enable (SVR = 0x1ff). */
    movl $0xfee00000, %edi
    movl $0x1ff, %eax
    movl %eax, 0xf0(%rdi)
#if PROBE_SCENARIO == 0
    sti
    hlt
    hlt
#elif PROBE_SCENARIO == 1
    sti
    nop
    hlt
#elif PROBE_SCENARIO == 2
    cli
    hlt
#elif PROBE_SCENARIO == 3
    sti
    nop
    movl $2000, %ecx
    movl $0x5000, %edi
    rep stosb
    hlt
#else
#error "unknown PROBE_SCENARIO"
#endif
.Lspin:
    jmp .Lspin
.size _start, .-_start

.section .text.handler,"ax",@progbits
.global stage4_handler_a
.type stage4_handler_a,@function
stage4_handler_a:
    movl $0xfee00000, %edi
    movl $0x44041, %eax
    movl %eax, 0x300(%rdi)
    eoi
    iretq
.size stage4_handler_a, .-stage4_handler_a

.global stage4_handler_b
.type stage4_handler_b,@function
stage4_handler_b:
    movl $0xfee00000, %edi
    eoi
    iretq
.global stage4_code_end
stage4_code_end:
.size stage4_handler_b, .-stage4_handler_b
