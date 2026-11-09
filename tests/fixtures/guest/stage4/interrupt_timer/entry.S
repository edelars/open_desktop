/*
 * Stage4InterruptTimerProbeV1 guest kernel (S4-27i2).
 *
 * Flat identity-mapped Long64/CPL0 image for the owner-driven timer pump. The
 * loader installs IDTR; the guest software-enables the Local APIC (SVR = 0x1ff),
 * programs the periodic APIC timer and then runs `STI; loop: HLT; JMP loop`.
 *
 * Timer: Divide Configuration 0x3 (divide by 16), LVT Timer = periodic, vector
 * 0x61, unmasked; Initial Count 625 (SDM Vol. 3A 10.5.4: a periodic timer
 * reloads the initial count every expiry; the count runs at the bus frequency
 * divided by the divisor). With the host timer frequency of 100 MHz the period
 * is 625 * 16 / 100 MHz = 100 us (100000 ns).
 *
 * Handler (vector 0x61): increments the dword counter at 0x5000, cancels the
 * timer (Initial Count = 0) once the counter reaches PROBE_WAKEUPS, writes the
 * LAPIC EOI and returns with IRETQ. The handler clobbers EAX/ESI/EDI; the main
 * loop uses no register.
 *
 * Build variants (assembler macros; the default is the accepted profile):
 *   PROBE_WAKEUPS=<n>   wake-ups before the handler cancels the timer (1..127)
 *   PROBE_SCENARIO=<n>  0 periodic (default); 1 unarmed: the timer is never
 *                       programmed (the halted guest has no deadline)
 */
#ifndef PROBE_WAKEUPS
#define PROBE_WAKEUPS 5
#endif
#ifndef PROBE_SCENARIO
#define PROBE_SCENARIO 0
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
    .zero 0x61 * 16
    gate stage4_timer_handler
    .zero 0x1000 - 0x62 * 16

.section .text.entry,"ax",@progbits
.global _start
.type _start,@function
_start:
    movl $0xfee00000, %edi
    /* Local APIC software enable (SVR = 0x1ff). */
    movl $0x1ff, %eax
    movl %eax, 0xf0(%rdi)
#if PROBE_SCENARIO == 0
    /* Divide Configuration = 0x3 (divide by 16). */
    movl $0x3, %eax
    movl %eax, 0x3e0(%rdi)
    /* LVT Timer: periodic (bit 17), unmasked, vector 0x61. */
    movl $0x20061, %eax
    movl %eax, 0x320(%rdi)
    /* Initial Count = 625: arms the timer. */
    movl $625, %eax
    movl %eax, 0x380(%rdi)
#endif
    sti
.Lloop:
    hlt
    jmp .Lloop
.size _start, .-_start

.section .text.handler,"ax",@progbits
.global stage4_timer_handler
.type stage4_timer_handler,@function
stage4_timer_handler:
    movl $0xfee00000, %edi
    movl $0x5000, %esi
    movl (%rsi), %eax
    addl $1, %eax
    movl %eax, (%rsi)
    cmpl $PROBE_WAKEUPS, %eax
    jne .Lack
    /* Cancel the periodic timer: Initial Count = 0. */
    movl $0, %eax
    movl %eax, 0x380(%rdi)
.Lack:
    movl $0, %eax
    movl %eax, 0xb0(%rdi)
    iretq
.global stage4_code_end
stage4_code_end:
.size stage4_timer_handler, .-stage4_timer_handler
