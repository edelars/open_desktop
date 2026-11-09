/*
 * Stage4InterruptQueueProbeV1 guest kernel (S4-27i3).
 *
 * Flat identity-mapped Long64/CPL0 image for the IRQ event queue of the pump.
 * The loader installs IDTR; the guest software-enables the Local APIC
 * (SVR = 0x1ff) and runs `STI; loop: HLT; JMP loop`. It never programs a timer:
 * every interrupt comes from the pump's event queue (the caller schedules the
 * events).
 *
 * Handler (vector 0x62): increments the dword counter at 0x5000, writes the
 * LAPIC EOI and returns with IRETQ. The handler clobbers EAX/ESI/EDI; the main
 * loop uses no register.
 */
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
    .zero 0x62 * 16
    gate stage4_queue_handler
    .zero 0x1000 - 0x63 * 16

.section .text.entry,"ax",@progbits
.global _start
.type _start,@function
_start:
    movl $0xfee00000, %edi
    /* Local APIC software enable (SVR = 0x1ff). */
    movl $0x1ff, %eax
    movl %eax, 0xf0(%rdi)
    sti
.Lloop:
    hlt
    jmp .Lloop
.size _start, .-_start

.section .text.handler,"ax",@progbits
.global stage4_queue_handler
.type stage4_queue_handler,@function
stage4_queue_handler:
    movl $0xfee00000, %edi
    movl $0x5000, %esi
    movl (%rsi), %eax
    addl $1, %eax
    movl %eax, (%rsi)
    movl $0, %eax
    movl %eax, 0xb0(%rdi)
    iretq
.global stage4_code_end
stage4_code_end:
.size stage4_queue_handler, .-stage4_queue_handler
