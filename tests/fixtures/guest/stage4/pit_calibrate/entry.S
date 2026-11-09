/*
 * Stage4PitCalibrateProbeV1 guest kernel (S4-29d).
 *
 * Flat identity-mapped Long64/CPL0 image. It repeats, in guest code and on
 * virtual time, the two Linux TSC calibrations that use the 8254 channel 2:
 *
 *   A. pit_calibrate_tsc(latch = 11931, ms = 10, loopmin = 1000): set gate 2
 *      high (outb((inb(0x61) & ~2) | 1, 0x61)), control word 0xb0 (channel 2,
 *      LSB+MSB, mode 0, binary), load the count, RDTSC, then poll port 0x61
 *      until bit 5 (OUT2) is set, tracking min/max of the TSC delta per poll;
 *      frequency in kHz = (t2 - t1) / ms, sanity: pitcnt >= loopmin and
 *      tscmax <= 10 * tscmin.
 *   B. quick_pit_calibrate (MSB form): gate 2 high, count 0xffff; every pass
 *      polls port 0x61, latches channel 2 (control word 0x80) and reads
 *      LSB and MSB from 0x42. Stage 1 waits while MSB == 0xff, stage 2 while
 *      MSB == 0xfe; each stage must see more than 5 passes (the control
 *      threshold). Frequency in kHz = (tsc_end - tsc_start) * 1193182 /
 *      (256 * 1000).
 *
 * Results go to COM1 THR with OUT DX,AL; the guest then executes HLT.
 * Serial record (all integers little endian):
 *   'A' status_a freq_khz delta pitcnt tscmin tscmax   (status 0 ok, 1 sanity)
 *   'B' status_b freq_khz delta count1 count2          (0 ok, 2 stage 1 failed,
 *                                                       3 stage 2 failed; 3 wins)
 *   'Z'
 * freq is 0xffffffffffffffff when the sanity check or threshold failed.
 *
 * Build variants (assembler macros, default to the accepted profile):
 *   PROBE_OMIT_GATE=1   injected regression: phase A does not raise gate 2
 *                       (the model refuses loading channel 2 with gate 2 low)
 */
#ifndef PROBE_OMIT_GATE
#define PROBE_OMIT_GATE 0
#endif

.section .gdt,"aw",@progbits
.global stage4_gdt
stage4_gdt:
    .quad 0x0000000000000000
    .quad 0x00af9b000000ffff
    .quad 0x00cf93000000ffff
.global stage4_gdt_end
stage4_gdt_end:

.macro OUT8 port, value
    movl $\port, %edx
    movb $\value, %al
    outb %al, %dx
.endm

.macro RDTSC64
    rdtsc
    shlq $32, %rdx
    orq %rdx, %rax
.endm

/* Raise gate 2 and clear the speaker bit: outb((inb(0x61) & ~2) | 1, 0x61). */
.macro GATE_HIGH
    movl $0x61, %edx
    inb %dx, %al
    andb $0xfd, %al
    orb $0x01, %al
    outb %al, %dx
.endm

/* One quick_pit_calibrate pass: time driver, latch channel 2, read LSB, MSB. */
.macro PROBE_MSB
    movl $0x61, %edx
    inb %dx, %al
    OUT8 0x43, 0x80
    movl $0x42, %edx
    inb %dx, %al
    inb %dx, %al
.endm

.section .text.entry,"ax",@progbits
.global _start
_start:
    /* COM1: 8N1, divisor 1 (the polling transmitter refuses anything else). */
    OUT8 0x3fb, 0x83
    OUT8 0x3f8, 0x01
    OUT8 0x3f9, 0x00
    OUT8 0x3fb, 0x03
#if !PROBE_OMIT_GATE
    GATE_HIGH
#endif
    OUT8 0x43, 0xb0
    OUT8 0x42, 0x9b
    OUT8 0x42, 0x2e
    RDTSC64
    movq %rax, %rbx
    movq %rax, %rsi
    movq $-1, %rdi
    xorl %ebp, %ebp
    xorl %r12d, %r12d
    xorl %r13d, %r13d
a_loop:
    movl $0x61, %edx
    inb %dx, %al
    testb $0x20, %al
    jnz a_done
    RDTSC64
    movq %rax, %rdx
    subq %rsi, %rdx
    movq %rax, %rsi
    cmpq %rdi, %rdx
    jae 1f
    movq %rdx, %rdi
1:
    cmpq %rbp, %rdx
    jbe 2f
    movq %rdx, %rbp
2:
    incq %r12
    jmp a_loop
a_done:
    movq %rsi, %r14
    subq %rbx, %r14
    cmpq $1000, %r12
    jb a_fail
    leaq (%rdi,%rdi,4), %rax
    addq %rax, %rax
    cmpq %rax, %rbp
    ja a_fail
    movq %r14, %rax
    xorl %edx, %edx
    movl $10, %ecx
    divq %rcx
    movq %rax, %r15
    jmp a_emit
a_fail:
    movl $1, %r13d
    movq $-1, %r15
a_emit:
    OUT8 0x3f8, 0x41
    movl $0x3f8, %edx
    movb %r13b, %al
    outb %al, %dx
    movq %r15, %rax
    call emit64
    movq %r14, %rax
    call emit64
    movq %r12, %rax
    call emit64
    movq %rdi, %rax
    call emit64
    movq %rbp, %rax
    call emit64

    GATE_HIGH
    OUT8 0x43, 0xb0
    OUT8 0x42, 0xff
    OUT8 0x42, 0xff
    xorl %r13d, %r13d
    xorl %ecx, %ecx
b1_loop:
    PROBE_MSB
    cmpb $0xff, %al
    jne b1_done
    RDTSC64
    movq %rax, %rsi
    incq %rcx
    jmp b1_loop
b1_done:
    movq %rcx, %r14
    movq %rsi, %rbx
    cmpq $5, %rcx
    ja 1f
    movl $2, %r13d
1:
    xorl %ecx, %ecx
b2_loop:
    PROBE_MSB
    cmpb $0xfe, %al
    jne b2_done
    RDTSC64
    movq %rax, %rsi
    incq %rcx
    jmp b2_loop
b2_done:
    movq %rcx, %r12
    cmpq $5, %rcx
    ja 2f
    movl $3, %r13d
2:
    movq %rsi, %r15
    subq %rbx, %r15
    testl %r13d, %r13d
    jnz b_fail
    movq %r15, %rax
    movl $1193182, %ecx
    mulq %rcx
    movl $256000, %ecx
    divq %rcx
    movq %rax, %rdi
    jmp b_emit
b_fail:
    movq $-1, %rdi
b_emit:
    OUT8 0x3f8, 0x42
    movl $0x3f8, %edx
    movb %r13b, %al
    outb %al, %dx
    movq %rdi, %rax
    call emit64
    movq %r15, %rax
    call emit64
    movq %r14, %rax
    call emit64
    movq %r12, %rax
    call emit64
    OUT8 0x3f8, 0x5a
done:
    hlt
    jmp done

/* Write RAX to COM1 THR, least significant byte first. Clobbers RAX, RDX, R10, R11. */
emit64:
    movq %rax, %r10
    movl $8, %r11d
    movl $0x3f8, %edx
3:
    movq %r10, %rax
    outb %al, %dx
    shrq $8, %r10
    decq %r11
    jnz 3b
    ret

.global stage4_code_end
stage4_code_end:
