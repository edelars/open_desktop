/*
 * S4-34d hardware oracle fixtures for descriptor tables and same-CPL delivery.
 * Build with -DKVM_SYSTEM_CASE=0x21 (tables_roundtrip) or 0x22
 * (int_iret_cpl0). Images are loaded at GPA/RIP 0x1000.
 * 0x21 payload words 0..4 are GDTR limit/base, IDTR limit/base, and STR;
 * words 5..11 are GDT entries 0..6 as raw qwords after LTR; words 12..24
 * are all 13 raw qwords of the 104-byte 64-bit TSS after initialization.
 * 0x22 payload words are pre-INT RFLAGS/RSP/SS, frame RIP/CS/RFLAGS/RSP/SS,
 * handler RFLAGS, post-IRET RSP/SS, and the post-IRET marker. In 64-bit mode
 * the same-CPL interrupt frame includes saved RSP/SS; separate pre/post
 * machine observations let the host verify that IRETQ restores them.
 * The host judges these raw observations.
 */

#define RESULT 0x18000
#define GDT_BASE 0x9000
#define TSS_BASE 0xa000
#define IDT_BASE 0xc000
#define RESULT_MAGIC 0x314d564b57483453
#define RESULT_COMPLETE 0x434f4d50
#define CASE_TABLES_ROUNDTRIP 0x21
#define CASE_INT_IRET_CPL0 0x22

.set GDT_LIMIT_TABLES, 0x37
.set GDT_LIMIT_INT, 0x27
.set IDT_LIMIT, 0x40f
.set IDT_VECTOR_40, IDT_BASE + (0x40 * 16)
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

.macro BUILD_GDT limit_value, with_tss, gdt_ptr_symbol
    movq $0, GDT_BASE+0
    movabsq $0x00af9b000000ffff, %rax
    movq %rax, GDT_BASE+8
    movabsq $0x00cf93000000ffff, %rax
    movq %rax, GDT_BASE+16
    movabsq $0x00affb000000ffff, %rax
    movq %rax, GDT_BASE+24
    movabsq $0x00cff3000000ffff, %rax
    movq %rax, GDT_BASE+32
    .if \with_tss
        movabsq $TSS_DESC_LOW, %rax
        movq %rax, GDT_BASE+40
        movq $0, GDT_BASE+48
        /* A 64-bit TSS occupies 104 bytes; initialize it before LTR. */
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
        movq $0xb000, TSS_BASE+4
        movw $104, TSS_BASE+102
    .endif
    lgdt \gdt_ptr_symbol(%rip)
    movw $0x10, %ax
    movw %ax, %ds
    movw %ax, %es
    movw %ax, %ss
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

.section .text.entry,"ax",@progbits
.code64
.global _start
_start:
#if !defined(KVM_SYSTEM_CASE)
#error "KVM_SYSTEM_CASE must select a descriptor fixture"
#elif KVM_SYSTEM_CASE == CASE_TABLES_ROUNDTRIP
    RESULT_HEADER CASE_TABLES_ROUNDTRIP, 25
    BUILD_GDT GDT_LIMIT_TABLES, 1, gdt_ptr_tables
    lidt idt_ptr(%rip)
    movw $0x28, %ax
    ltr %ax

    /* Capture the architectural table registers and the post-LTR descriptor. */
    sgdt observed_gdtr(%rip)
    sidt observed_idtr(%rip)
    str observed_tr(%rip)
    movzwq observed_gdtr(%rip), %rax
    movq %rax, RESULT+32
    movq observed_gdtr+2(%rip), %rax
    movq %rax, RESULT+40
    movzwq observed_idtr(%rip), %rax
    movq %rax, RESULT+48
    movq observed_idtr+2(%rip), %rax
    movq %rax, RESULT+56
    movzwq observed_tr(%rip), %rax
    movq %rax, RESULT+64
    /* Payload slots 5..11: exact raw qwords for GDT entries 0..6. */
    movq GDT_BASE+0, %rax
    movq %rax, RESULT+72
    movq GDT_BASE+8, %rax
    movq %rax, RESULT+80
    movq GDT_BASE+16, %rax
    movq %rax, RESULT+88
    movq GDT_BASE+24, %rax
    movq %rax, RESULT+96
    movq GDT_BASE+32, %rax
    movq %rax, RESULT+104
    movq GDT_BASE+40, %rax
    movq %rax, RESULT+112
    movq GDT_BASE+48, %rax
    movq %rax, RESULT+120
    /* Payload slots 12..24: all 104 architecturally defined TSS bytes. */
    movq TSS_BASE+0, %rax
    movq %rax, RESULT+128
    movq TSS_BASE+8, %rax
    movq %rax, RESULT+136
    movq TSS_BASE+16, %rax
    movq %rax, RESULT+144
    movq TSS_BASE+24, %rax
    movq %rax, RESULT+152
    movq TSS_BASE+32, %rax
    movq %rax, RESULT+160
    movq TSS_BASE+40, %rax
    movq %rax, RESULT+168
    movq TSS_BASE+48, %rax
    movq %rax, RESULT+176
    movq TSS_BASE+56, %rax
    movq %rax, RESULT+184
    movq TSS_BASE+64, %rax
    movq %rax, RESULT+192
    movq TSS_BASE+72, %rax
    movq %rax, RESULT+200
    movq TSS_BASE+80, %rax
    movq %rax, RESULT+208
    movq TSS_BASE+88, %rax
    movq %rax, RESULT+216
    movq TSS_BASE+96, %rax
    movq %rax, RESULT+224
    RESULT_FINISH

#elif KVM_SYSTEM_CASE == CASE_INT_IRET_CPL0
    RESULT_HEADER CASE_INT_IRET_CPL0, 12
    BUILD_GDT GDT_LIMIT_INT, 0, gdt_ptr_int
    SET_GATE IDT_VECTOR_40, int40_handler, 0x8e
    lidt idt_ptr(%rip)

    pushfq
    popq %rax
    movq %rax, RESULT+32
    movq %rsp, %rax
    movq %rax, RESULT+40
    movw %ss, %ax
    movzwq %ax, %rax
    movq %rax, RESULT+48
    sti
    int $0x40
int40_returned:
    movq %rsp, %rax
    movq %rax, RESULT+104
    movw %ss, %ax
    movzwq %ax, %rax
    movq %rax, RESULT+112
    movq $1, RESULT+120
    RESULT_FINISH

int40_handler:
    pushq %rax
    pushq %rbx
    leaq 16(%rsp), %rsi
    movq 0(%rsi), %rax
    movq %rax, RESULT+56          /* frame RIP */
    movq 8(%rsi), %rax
    movq %rax, RESULT+64          /* frame CS */
    movq 16(%rsi), %rax
    movq %rax, RESULT+72          /* saved frame RFLAGS */
    movq 24(%rsi), %rax
    movq %rax, RESULT+80          /* saved frame RSP */
    movq 32(%rsi), %rax
    movq %rax, RESULT+88          /* saved frame SS */
    pushfq
    popq %rax
    movq %rax, RESULT+96          /* interrupt-gate handler RFLAGS */
    movl RESULT+24, %ebx
    incl %ebx
    movl %ebx, RESULT+24
    popq %rbx
    popq %rax
    iretq
#else
#error "unsupported KVM_SYSTEM_CASE for descriptors.S"
#endif

.section .rodata,"a",@progbits
.align 8
gdt_ptr_tables:
    .word GDT_LIMIT_TABLES
    .quad GDT_BASE
gdt_ptr_int:
    .word GDT_LIMIT_INT
    .quad GDT_BASE
idt_ptr:
    .word IDT_LIMIT
    .quad IDT_BASE

.section .bss,"aw",@nobits
.align 8
observed_gdtr:
    .space 10
observed_idtr:
    .space 10
observed_tr:
    .space 2
