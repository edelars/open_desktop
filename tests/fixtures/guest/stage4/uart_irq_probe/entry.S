/*
 * Stage4UartIrqProbeV1 guest kernel (S4-24e).
 *
 * Flat identity-mapped Long64/CPL0 image. The guest installs its own IDT with
 * LIDT, enables the Local APIC, unmasks IOAPIC INTIN4 (vector 0x51, edge),
 * programs COM1 (8N1, OUT2, IER) and then idles with STI; HLT. The handler
 * reads IIR and RBR, echoes the byte to THR, writes the Local APIC EOI and
 * returns with IRETQ.
 *
 * Build variants (assembler macros, all default to the accepted profile):
 *   PROBE_IER=<n>            IER value (default 1 = RDA)
 *   PROBE_LATE_UNMASK=1      program IER before unmasking IOAPIC INTIN4
 *   PROBE_OMIT_EOI=1         injected regression: handler skips the LAPIC EOI
 *   PROBE_OMIT_RBR_READ=1    injected regression: handler skips the RBR read
 */
#ifndef PROBE_IER
#define PROBE_IER 1
#endif
#ifndef PROBE_LATE_UNMASK
#define PROBE_LATE_UNMASK 0
#endif
#ifndef PROBE_OMIT_EOI
#define PROBE_OMIT_EOI 0
#endif
#ifndef PROBE_OMIT_RBR_READ
#define PROBE_OMIT_RBR_READ 0
#endif

.section .gdt,"aw",@progbits
.global stage4_gdt
stage4_gdt:
    .quad 0x0000000000000000
    .quad 0x00af9b000000ffff
    .quad 0x00cf93000000ffff
.global stage4_gdt_end
stage4_gdt_end:

.section .idt,"aw",@progbits
.global stage4_idt
stage4_idt:
    .zero 0x51 * 16
    .word stage4_irq_handler
    .word 0x0008
    .byte 0x00
    .byte 0x8e
    .word 0x0000
    .long 0x00000000
    .long 0x00000000
    .zero 0x1000 - 0x52 * 16

.section .idtr,"aw",@progbits
.global stage4_idtr
stage4_idtr:
    .word 0x0fff
    .quad stage4_idt

.macro unmask_intin4
    movl $0xfec00000, %edi
    movl $0x18, %eax
    movl %eax, (%rdi)
    movl $0x51, %eax
    movl %eax, 0x10(%rdi)
.endm

.macro out8 port, value
    movl $\port, %edx
    movb $\value, %al
    outb %al, %dx
.endm

.section .text.entry,"ax",@progbits
.global _start
.type _start,@function
_start:
    /* LIDT [RSP+0]; the loader points RSP at stage4_idtr. */
    .byte 0x0f, 0x01, 0x5c, 0x24, 0x00
    /* Local APIC software enable (SVR = 0x1ff). */
    movl $0xfee00000, %edi
    movl $0x1ff, %eax
    movl %eax, 0xf0(%rdi)
#if !PROBE_LATE_UNMASK
    unmask_intin4
#endif
    out8 0x3fb, 0x83
    out8 0x3f8, 0x01
    out8 0x3f9, 0x00
    out8 0x3fb, 0x03
    out8 0x3fc, 0x08
    out8 0x3f9, PROBE_IER
#if PROBE_LATE_UNMASK
    unmask_intin4
#endif
    sti
.global stage4_idle
stage4_idle:
.Lidle:
    hlt
    jmp .Lidle
.size _start, .-_start

.section .text.handler,"ax",@progbits
.global stage4_irq_handler
.type stage4_irq_handler,@function
stage4_irq_handler:
    movl $0x3fa, %edx
    inb %dx, %al
    movl $0x3f8, %edx
#if !PROBE_OMIT_RBR_READ
    inb %dx, %al
#endif
    outb %al, %dx
#if !PROBE_OMIT_EOI
    movl $0xfee00000, %edi
    movl $0, %eax
    movl %eax, 0xb0(%rdi)
#endif
    iretq
.global stage4_code_end
stage4_code_end:
.size stage4_irq_handler, .-stage4_irq_handler
