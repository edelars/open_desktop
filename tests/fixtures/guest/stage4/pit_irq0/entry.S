/*
 * Stage4PitIrq0ProbeV1 guest kernel (S4-29f).
 *
 * Flat identity-mapped Long64/CPL0 image for PIT channel 0 -> OUT0 -> IOAPIC
 * INTIN2 (GSI 2) -> Local APIC. The loader installs IDTR. The guest
 *   1. software-enables the Local APIC (SVR = 0x1ff);
 *   2. initialises the master 8259 (ICW1..ICW4, vector base 0x30) and masks it;
 *   3. writes IOAPIC redirection entry 2 (IOREGSEL 0x14, IOWIN): vector 0x60,
 *      edge, unmasked, fixed, physical destination 0;
 *   4. programs PIT channel 0 (control 0x34: LSB+MSB, mode 2, binary) with the
 *      count 1790, then reprograms it with the count 1193 (a second control word
 *      and count: the first program's pending edge must be dropped);
 *   5. runs `STI; loop: HLT; JMP loop`.
 *
 * Handler (vector 0x60): increments the dword counter at 0x5000; after the
 * PROBE_INTERRUPTS-th interrupt it selects the PIC IRR (OCW3 0x0A), reads it
 * (IN 0x20) and shuts channel 0 down (control word 0x30); then it writes the
 * Local APIC EOI and returns with IRETQ. The handler clobbers EAX/EDX/ESI/EDI;
 * the main loop uses no register.
 *
 * Build variants (assembler macros; the default is the accepted profile):
 *   PROBE_INTERRUPTS=<n>  interrupts before the shutdown (1..127, default 3)
 *   PROBE_CONTROL=<n>     channel 0 control word: 0x34 mode 2 (default), 0x30 mode 0
 */
#ifndef PROBE_INTERRUPTS
#define PROBE_INTERRUPTS 3
#endif
#ifndef PROBE_CONTROL
#define PROBE_CONTROL 0x34
#endif
#define PROBE_COUNT_FIRST 1790
#define PROBE_COUNT_SECOND 1193

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
    .zero 0x60 * 16
    gate stage4_pit_handler
    .zero 0x1000 - 0x61 * 16

.macro out8 port, value
    movl $\port, %edx
    movb $\value, %al
    outb %al, %dx
.endm

.macro program_channel0 control, count
    out8 0x43, \control
    out8 0x40, (\count)&0xff
    out8 0x40, ((\count)>>8)&0xff
.endm

.section .text.entry,"ax",@progbits
.global _start
.type _start,@function
_start:
    movl $0xfee00000, %edi
    /* Local APIC software enable (SVR = 0x1ff). */
    movl $0x1ff, %eax
    movl %eax, 0xf0(%rdi)
    /* Master 8259: ICW1, ICW2 = 0x30, ICW3 = 0x04, ICW4 = 0x01, IMR = 0xff. */
    out8 0x20, 0x11
    out8 0x21, 0x30
    out8 0x21, 0x04
    out8 0x21, 0x01
    out8 0x21, 0xff
    /* IOAPIC entry 2 low dword (IOREGSEL 0x14): vector 0x60, edge, unmasked. */
    movl $0xfec00000, %esi
    movl $0x14, %eax
    movl %eax, (%rsi)
    movl $0x60, %eax
    movl %eax, 0x10(%rsi)
    /* PIT channel 0: first program, then the reprogram that replaces it. */
    program_channel0 PROBE_CONTROL, PROBE_COUNT_FIRST
    program_channel0 PROBE_CONTROL, PROBE_COUNT_SECOND
    sti
.Lloop:
    hlt
    jmp .Lloop
.size _start, .-_start

.section .text.handler,"ax",@progbits
.global stage4_pit_handler
.type stage4_pit_handler,@function
stage4_pit_handler:
    movl $0xfee00000, %edi
    movl $0x5000, %esi
    movl (%rsi), %eax
    addl $1, %eax
    movl %eax, (%rsi)
    cmpl $PROBE_INTERRUPTS, %eax
    jne .Lack
    /* PIC IRR (OCW3 0x0A, then IN 0x20), then channel 0 shutdown. */
    out8 0x20, 0x0a
    movl $0x20, %edx
    inb %dx, %al
    out8 0x43, 0x30
.Lack:
    movl $0, %eax
    movl %eax, 0xb0(%rdi)
    iretq
.global stage4_code_end
stage4_code_end:
.size stage4_pit_handler, .-stage4_pit_handler
