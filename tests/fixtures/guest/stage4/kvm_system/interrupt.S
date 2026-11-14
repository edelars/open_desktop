/*
 * S4-34e: user-space injected external vector wakes a Long64/CPL0 guest from
 * HLT, reaches one IDT interrupt gate, and returns through IRETQ.
 *
 * This is a kvm-injected CPU delivery case, not an APIC/IOAPIC/device test.
 * The runner queues vector 0x41 with KVM_INTERRUPT after the first HLT exit.
 * Architectural frame/IRET behavior follows Intel SDM 253668-093US, Vol. 3A
 * §§7.14.2-7.14.3; the instruction behavior follows the Vol. 2 HLT/STI/IRETQ
 * entries.  The host-side builder compares the raw result block and snapshots.
 */

.section .text.entry,"ax",@progbits
.code64
.global _start
_start:
    cld

    /* Common S4HWKVM1 result block at GPA 0x18000 (512-byte bounded snapshot). */
    movabs $0x314d564b57483453, %rax
    movq %rax, 0x18000
    movl $1, 0x18008
    movl $0x31, 0x1800c
    movl $0, 0x18010              /* completion set only after handler + IRETQ */
    movl $7, 0x18014              /* seven u64 observations follow the header */
    movl $0, 0x18018              /* first HLT checkpoint: no interrupt yet */
    movl $0, 0x1801c
    movq $0, 0x18020
    movq $0, 0x18028
    movq $0, 0x18030
    movq $0, 0x18038
    movq $0, 0x18040
    movq $0, 0x18048
    movq $0, 0x18050

    /* Install only vector 0x41 in an otherwise zero IDT at 0x9000. */
    leaq vector_41_handler(%rip), %rax
    movw %ax, 0x9410
    movw $0x08, 0x9412
    movb $0, 0x9414              /* IST = 0 */
    movb $0x8e, 0x9415           /* present, DPL0, 64-bit interrupt gate */
    shrq $16, %rax
    movw %ax, 0x9416
    shrq $16, %rax
    movl %eax, 0x9418
    movl $0, 0x941c
    lidt idtr(%rip)

    sti
first_hlt:
    hlt
after_first_hlt:
    /* IRETQ must return to the instruction immediately after the first HLT. */
    movabs $0x53434b504f535431, %rax /* raw little-endian resume sentinel */
    movq %rax, 0x18050
    movl $0x434f4d50, 0x18010     /* completion marker; bytes are PMOC */
terminal_hlt:
    hlt
1:
    jmp 1b

/* KVM_INTERRUPT vector 0x41 enters here with RIP, CS, RFLAGS at [RSP..+16]. */
vector_41_handler:
    pushq %rax
    pushq %rbx
    pushq %rcx
    leaq 24(%rsp), %rbx
    movq $0x41, 0x18020          /* observation[0]: vector */
    movq 0(%rbx), %rax
    movq %rax, 0x18028           /* observation[1]: saved RIP */
    movq 8(%rbx), %rax
    movq %rax, 0x18030           /* observation[2]: saved CS */
    movq 16(%rbx), %rax
    movq %rax, 0x18038           /* observation[3]: saved RFLAGS */
    pushfq
    popq %rax
    movq %rax, 0x18040           /* observation[4]: handler RFLAGS (IF must be 0) */
    movabs $0x48414e444c455231, %rax /* observation[5]: handler marker */
    movq %rax, 0x18048
    incl 0x18018                /* exactly one delivered event */
    popq %rcx
    popq %rbx
    popq %rax
    iretq

.section .data,"aw",@progbits
.align 8
idtr:
    .word 0x041f                 /* includes vector 0x41: 65 * 16 + 15 */
    .quad 0x9000
