/*
 * S4-34d CPL3 delivery and privilege fault fixtures.
 * Build with -DKVM_SYSTEM_CASE=0x23 (cpl3_roundtrip) or 0x24
 * (cpl3_privileged_gp). Images are loaded at GPA/RIP 0x1000.
 *
 * Intel SDM rev093: Vol. 3A §§7.14.2–7.14.4, §7.15; Vol. 2A/2B
 * entries for IRETQ, INT, LTR, LGDT/LIDT, and MOV to/from CR0.
 */

#define RESULT 0x18000
#define GDT_BASE 0x9000
#define TSS_BASE 0xa000
#define IDT_BASE 0xc000
#define RESULT_MAGIC 0x314d564b57483453
#define RESULT_COMPLETE 0x434f4d50
#define CASE_CPL3_ROUNDTRIP 0x23
#define CASE_CPL3_PRIVILEGED_GP 0x24
#define USER_STACK 0x16000
#define KERNEL_STACK 0x17000
#define PML4_BASE 0x2000
#define PDPT_BASE 0x3000
#define PD_BASE 0x4000

.set GDT_LIMIT, 0x37
.set IDT_LIMIT, 0x41f
.set IDT_VECTOR_40, IDT_BASE + (0x40 * 16)
.set IDT_VECTOR_41, IDT_BASE + (0x41 * 16)
.set IDT_VECTOR_GP, IDT_BASE + (13 * 16)
.set TSS_DESC_LOW, 0x00008900a0000067

.macro RESULT_HEADER case_id, word_count
    movabsq $RESULT_MAGIC, %rax
    movq %rax, RESULT+0
    movl $1, RESULT+8
    movl $\case_id, RESULT+12
    movl $0, RESULT+16
    movl $\word_count, RESULT+20
    movl $0, RESULT+24
    movl $0, RESULT+28
.endm

.macro RESULT_FINISH
    movl $RESULT_COMPLETE, RESULT+16
    hlt
1:  jmp 1b
.endm

.macro BUILD_GDT
    movq $0, GDT_BASE+0
    movabsq $0x00af9b000000ffff, %rax
    movq %rax, GDT_BASE+8
    movabsq $0x00cf93000000ffff, %rax
    movq %rax, GDT_BASE+16
    movabsq $0x00affb000000ffff, %rax
    movq %rax, GDT_BASE+24
    movabsq $0x00cff3000000ffff, %rax
    movq %rax, GDT_BASE+32
    movabsq $TSS_DESC_LOW, %rax
    movq %rax, GDT_BASE+40
    movq $0, GDT_BASE+48
    /* 64-bit TSS: RSP0=KERNEL_STACK, I/O bitmap offset=104 (past limit). */
    movq $0, TSS_BASE+0
    movq $0, TSS_BASE+8
    movq $0, TSS_BASE+16
    movq $0, TSS_BASE+24
    movq $0, TSS_BASE+32
    movq $0, TSS_BASE+40
    movq $0, TSS_BASE+48
    movq $0, TSS_BASE+56
    movq $0, TSS_BASE+64
    movq $0, TSS_BASE+72
    movq $0, TSS_BASE+80
    movq $0, TSS_BASE+88
    movq $0, TSS_BASE+96
    movq $KERNEL_STACK, TSS_BASE+4
    movw $104, TSS_BASE+102
    lgdt gdt_ptr(%rip)
    movw $0x10, %ax
    movw %ax, %ds
    movw %ax, %es
    movw %ax, %ss
    movw $0x28, %ax
    ltr %ax
.endm

.macro SET_GATE gate_addr, handler, attr
    movabsq $\handler, %rax
    movw %ax, \gate_addr+0
    movw $0x08, \gate_addr+2
    movb $0, \gate_addr+4
    movb $\attr, \gate_addr+5
    shrq $16, %rax
    movw %ax, \gate_addr+6
    shrq $16, %rax
    movl %eax, \gate_addr+8
    movl $0, \gate_addr+12
.endm

.macro ENTER_USER entry
    pushq $0x23                 /* user SS */
    pushq $USER_STACK           /* user RSP */
    pushfq
    popq %rax
    orq $0x200, %rax            /* user RFLAGS.IF=1 */
    pushq %rax
    pushq $0x1b                 /* user CS */
    leaq \entry(%rip), %rax
    pushq %rax
    iretq
.endm

.macro ENABLE_USER_IDENTITY_MAP
    /*
     * The KVM runner maps 0..2 MiB with one 2 MiB leaf and initially clears
     * U/S in all three paging levels. Set U/S at PML4, PDPT, and PDE so CPL3
     * can fetch and execute this fixture. This deliberately exposes the whole
     * 2 MiB identity map to user mode; these cases test privilege transitions,
     * not user/kernel memory isolation.
     */
    orq $0x4, PML4_BASE
    orq $0x4, PDPT_BASE
    orq $0x4, PD_BASE
    movq %cr3, %rax
    movq %rax, %cr3            /* flush cached supervisor-only translation */
.endm

.section .text.entry,"ax",@progbits
.code64
.global _start
_start:
#if !defined(KVM_SYSTEM_CASE)
#error "KVM_SYSTEM_CASE must select a privilege fixture"
#elif KVM_SYSTEM_CASE == CASE_CPL3_ROUNDTRIP
    /*
     * Payload words: 0 user INT RIP, 1 user CS, 2 user RFLAGS, 3 user RSP,
     * 4 user SS, 5 second INT marker RIP, 6 terminal marker. event_count=2.
     */
    RESULT_HEADER CASE_CPL3_ROUNDTRIP, 7
    BUILD_GDT
    SET_GATE IDT_VECTOR_40, user_int40_handler, 0xee
    SET_GATE IDT_VECTOR_41, user_int41_handler, 0xee
    lidt idt_ptr(%rip)
    ENABLE_USER_IDENTITY_MAP
    ENTER_USER cpl3_roundtrip_user

cpl3_roundtrip_user:
    int $0x40
cpl3_roundtrip_after_int:
    int $0x41
    ud2

user_int40_handler:
    /* CPL3->CPL0 frame is RIP, CS, RFLAGS, RSP, SS at the current RSP. */
    movq 0(%rsp), %rax
    movq %rax, RESULT+32
    movq 8(%rsp), %rax
    movq %rax, RESULT+40
    movq 16(%rsp), %rax
    movq %rax, RESULT+48
    movq 24(%rsp), %rax
    movq %rax, RESULT+56
    movq 32(%rsp), %rax
    movq %rax, RESULT+64
    movl $1, RESULT+24
    iretq

user_int41_handler:
    movq 0(%rsp), %rax
    movq %rax, RESULT+72
    movl $2, RESULT+24
    movabsq $cpl3_roundtrip_done, %rax
    movq %rax, 0(%rsp)
    movq $0x08, 8(%rsp)
    movq $KERNEL_STACK, 24(%rsp)
    movq $0x10, 32(%rsp)
    iretq

cpl3_roundtrip_done:
    movq $1, RESULT+80
    RESULT_FINISH

#elif KVM_SYSTEM_CASE == CASE_CPL3_PRIVILEGED_GP
    /*
     * Payload words: 0 CR0 before fault, 1 #GP error code, 2 fault RIP,
     * 3 fault CS, 4 fault RFLAGS, 5 CR0 in handler, 6 return marker RIP,
     * 7 terminal marker. event_count=2 (#GP and user marker INT).
     */
    RESULT_HEADER CASE_CPL3_PRIVILEGED_GP, 8
    BUILD_GDT
    SET_GATE IDT_VECTOR_40, user_marker_handler, 0xee
    SET_GATE IDT_VECTOR_GP, general_protection_handler, 0x8e
    lidt idt_ptr(%rip)
    movq %cr0, %rax
    movq %rax, RESULT+32
    ENABLE_USER_IDENTITY_MAP
    ENTER_USER cpl3_privileged_user

cpl3_privileged_user:
    movq %cr0, %rax
cpl3_privileged_returned:
    int $0x40
cpl3_privileged_after_int:
    ud2

general_protection_handler:
    /* #GP pushes error code before RIP, CS, and RFLAGS. */
    movq %cr0, %rax
    movq %rax, RESULT+72
    movq 0(%rsp), %rax
    movq %rax, RESULT+40
    movq 8(%rsp), %rax
    movq %rax, RESULT+48
    movq 16(%rsp), %rax
    movq %rax, RESULT+56
    movq 24(%rsp), %rax
    movq %rax, RESULT+64
    movl $1, RESULT+24
    addq $3, 8(%rsp)            /* skip 3-byte MOV CR0, then IRETQ */
    addq $8, %rsp                /* discard the #GP(0) error code */
    iretq

user_marker_handler:
    movq 0(%rsp), %rax
    movq %rax, RESULT+80
    movl $2, RESULT+24
    movabsq $cpl3_privileged_done, %rax
    movq %rax, 0(%rsp)
    movq $0x08, 8(%rsp)
    movq $KERNEL_STACK, 24(%rsp)
    movq $0x10, 32(%rsp)
    iretq

cpl3_privileged_done:
    movq $1, RESULT+88
    RESULT_FINISH
#else
#error "unsupported KVM_SYSTEM_CASE for privilege.S"
#endif

.section .rodata,"a",@progbits
.align 8
gdt_ptr:
    .word GDT_LIMIT
    .quad GDT_BASE
idt_ptr:
    .word IDT_LIMIT
    .quad IDT_BASE
