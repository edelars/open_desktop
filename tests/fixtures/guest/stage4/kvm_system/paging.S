/*
 * S4-34c: independent CPL0 page-walk and permission observations.
 *
 * Build with -DCASE_ID=101..108 and layout.ld. The test VA lives under PML4[1]
 * (0x0000008000000000); code, handler, result block and page-table storage use
 * the runner's separate identity-mapped PML4[0] region.
 */
#ifndef CASE_ID
#error "CASE_ID must select one paging fixture"
#endif

#define RESULT_GPA 0x18000
#define RESULT_MAGIC 0x314d564b57483453
#define RESULT_COMPLETE 0x434f4d50
#define IDT_GPA 0x9000
#define PT_L3 0x10000
#define PT_L2 0x11000
#define PT_L1 0x12000
#define ROOT_B 0x13000
#define DATA_A 0x14000
#define PT_B_L3 0x15000
#define PT_B_L2 0x16000
#define PT_B_L1 0x17000
#define DATA_B 0x19000
#define FAULT_RESUME 0x1b000
#define TEST_VA 0x8000000000
#define PAGE_PRESENT 1
#define PAGE_WRITE 2
#define PAGE_ACCESSED 0x20
#define PAGE_DIRTY 0x40
#define PAGE_NX 0x8000000000000000

.section .text,"ax",@progbits
.code64
.global _start
_start:
    cli
    cld
    movabs $0x1f000, %rsp
    movabs $RESULT_GPA, %r15
    movabs $RESULT_MAGIC, %rax
    movq %rax, 0(%r15)
    movl $1, 8(%r15)
    movl $CASE_ID, 12(%r15)
    movl $0, 16(%r15)
    movl $0, 20(%r15)
    movl $0, 24(%r15)
    movl $0, 28(%r15)

    call initialize_test_mapping

.if CASE_ID == 101
    /* read4k observations: [0] read value; [1..4] raw PML4E/PDPTE/PDE/PTE;
     * [5] direct-physical sentinel; [6] final CR3. A=1 and leaf D=0 are
     * checked by the host from the raw entries.
     */
    movl $7, 20(%r15)
    movabs $DATA_A, %rdi
    movl $0x11223344, (%rdi)
    movabs $TEST_VA, %rax
    movq (%rax), %rbx
    movq %rbx, 32(%r15)
    call record_test_entries
    movabs $DATA_A, %rdi
    movq (%rdi), %rax
    movq %rax, 72(%r15)
    movq %cr3, %rax
    movq %rax, 80(%r15)
.elseif CASE_ID == 102
    /* write4k observations: [0] data read by the test VA; [1..4] raw entries;
     * [5] direct-physical data; [6] final CR3. The host checks A=1 throughout
     * the path and leaf D=1 after the successful write.
     */
    movl $7, 20(%r15)
    movabs $DATA_A, %rdi
    movl $0, (%rdi)
    movabs $TEST_VA, %rax
    movabs $0x55667788, %rbx
    movq %rbx, (%rax)
    movq (%rax), %rbx
    movq %rbx, 32(%r15)
    call record_test_entries
    movabs $DATA_A, %rdi
    movq (%rdi), %rax
    movq %rax, 72(%r15)
    movq %cr3, %rax
    movq %rax, 80(%r15)
.elseif CASE_ID == 103
    /* pf_not_present observations: [0] PFEC; [1] CR2; [2] saved fault RIP;
     * [3..6] raw paging entries after the fault; [7] untouched DATA_A. The
     * PF handler records the frame and resumes at `pf_not_present_resumed`.
     */
    movl $8, 20(%r15)
    call install_page_fault_gate
    movabs $FAULT_RESUME, %rdi
    movabs $pf_not_present_resumed, %rax
    movq %rax, (%rdi)
    movabs $PT_L1, %rdi
    movq $(DATA_A | PAGE_WRITE), (%rdi)
    movabs $TEST_VA, %rax
    .global s4_34c_fault_not_present
s4_34c_fault_not_present:
    movq (%rax), %rcx
pf_not_present_resumed:
    call record_test_entries
    movabs $DATA_A, %rdi
    movq (%rdi), %rax
    movq %rax, 88(%r15)
.elseif CASE_ID == 104
    /* wp_enabled observations: [0..2] PFEC/CR2/fault RIP; [3..6] raw paging
     * entries; [7] unchanged backing value; [8] final CR0. A/D values after
     * a protection fault are recorded, not asserted.
     */
    movl $9, 20(%r15)
    call install_page_fault_gate
    movabs $DATA_A, %rdi
    movl $0x11223344, (%rdi)
    movabs $PT_L1, %rdi
    movq $(DATA_A | PAGE_PRESENT), (%rdi)
    movabs $FAULT_RESUME, %rdi
    movabs $wp_enabled_resumed, %rax
    movq %rax, (%rdi)
    movq %cr0, %rax
    movq %rax, %rbx
    orq $0x10000, %rax
    movq %rax, %cr0
    movabs $TEST_VA, %rax
    movabs $0xaabbccdd, %rcx
    .global s4_34c_fault_wp_enabled
s4_34c_fault_wp_enabled:
    movq %rcx, (%rax)
wp_enabled_resumed:
    movq %rbx, %cr0
    call record_test_entries
    movabs $DATA_A, %rdi
    movq (%rdi), %rax
    movq %rax, 88(%r15)
    movq %cr0, %rax
    movq %rax, 96(%r15)
.elseif CASE_ID == 105
    /* wp_disabled observations: [0] backing value after successful write;
     * [1..4] raw paging entries; [5] final CR0; [6] fault-event count. The
     * PTE is read-only, but CPL0 writes are allowed with CR0.WP=0.
     */
    movl $7, 20(%r15)
    movabs $DATA_A, %rdi
    movl $0x11223344, (%rdi)
    movabs $PT_L1, %rdi
    movq $(DATA_A | PAGE_PRESENT), (%rdi)
    movq %cr0, %rbx
    andq $~0x10000, %rbx
    movq %rbx, %cr0
    movabs $TEST_VA, %rax
    movabs $0xaabbccdd, %rcx
    movq %rcx, (%rax)
    call record_test_entries
    movabs $DATA_A, %rdi
    movq (%rdi), %rax
    movq %rax, 32(%r15)
    movq %cr0, %rax
    movq %rax, 72(%r15)
    movl 24(%r15), %eax
    movq %rax, 80(%r15)
.elseif CASE_ID == 106
    /* nx_fetch observations: [0] NX capability (0=unsupported, 1=supported);
     * [1] CPUID.80000001:EDX; [2..4] PFEC/CR2/fault RIP if supported; [5..8]
     * raw PML4E/PDPTE/PDE/PTE; [9] final EFER. Only execute when NX is advertised.
     */
    movl $10, 20(%r15)
    movl $0x80000000, %eax
    cpuid
    cmpl $0x80000001, %eax
    jae nx_cpuid_leaf_available
    movq $0, 32(%r15)
    movq $0, 40(%r15)
    jmp nx_unsupported
nx_cpuid_leaf_available:
    movl $0x80000001, %eax
    cpuid
    movl %edx, 40(%r15)
    testl $0x00100000, %edx
    jnz nx_supported
    movq $0, 32(%r15)
    jmp nx_unsupported
nx_supported:
    movq $1, 32(%r15)

    call install_page_fault_gate
    movabs $DATA_A, %rdi
    movb $0xf4, (%rdi)
    movabs $PT_L1, %rdi
    movabs $(DATA_A | PAGE_PRESENT | PAGE_WRITE | PAGE_NX), %rax
    movq %rax, (%rdi)
    movabs $FAULT_RESUME, %rdi
    movabs $nx_fetch_resumed, %rax
    movq %rax, (%rdi)
    movl $0xc0000080, %ecx
    rdmsr
    movl %eax, saved_efer_low(%rip)
    movl %edx, saved_efer_high(%rip)
    orl $0x800, %eax
    wrmsr
    movabs $TEST_VA, %rax
    .global s4_34c_fault_nx_fetch
s4_34c_fault_nx_fetch:
    jmp *%rax
nx_fetch_resumed:
    movl $0xc0000080, %ecx
    movl saved_efer_low(%rip), %eax
    movl saved_efer_high(%rip), %edx
    wrmsr
    call record_test_entries
    movl $0xc0000080, %ecx
    rdmsr
    shlq $32, %rdx
    orq %rdx, %rax
    movq %rax, 104(%r15)
    jmp paging_complete
nx_unsupported:
    movq $0, 48(%r15)
    movq $0, 56(%r15)
    movq $0, 64(%r15)
    movq $0, 72(%r15)
    movq $0, 80(%r15)
    movq $0, 88(%r15)
    movq $0, 96(%r15)
    movq $0, 104(%r15)
.elseif CASE_ID == 107
    /* cr3_switch observations: [0] read via root A; [1] read via root B;
     * [2] CR3 after switch; [3..4] leaf entries through each root; [5] rootA;
     * [6] rootB. Both roots retain the separate PML4[0] identity path.
     */
    movl $7, 20(%r15)
    movabs $DATA_A, %rdi
    movl $0x11223344, (%rdi)
    movabs $DATA_B, %rdi
    movl $0x55667788, (%rdi)
    call build_root_b
    movabs $TEST_VA, %rax
    movq (%rax), %rbx
    movq %rbx, 32(%r15)
    movabs $ROOT_B, %rax
    movq %rax, %cr3
    movq %cr3, %rax
    movq %rax, 48(%r15)
    movabs $TEST_VA, %rax
    movq (%rax), %rbx
    movq %rbx, 40(%r15)
    movabs $PT_L1, %rdi
    movq (%rdi), %rax
    movq %rax, 56(%r15)
    movabs $PT_B_L1, %rdi
    movq (%rdi), %rax
    movq %rax, 64(%r15)
    movabs $0x2000, %rax
    movq %rax, 72(%r15)
    movabs $ROOT_B, %rax
    movq %rax, 80(%r15)
.elseif CASE_ID == 108
    /* invlpg_remap observations: [0] value through mapping A; [1] new value
     * after PTE remap+INVLPG; [2] B PTE before access, [3] B PTE after access;
     * [4] old backing page; [5] new backing page.
     */
    movl $6, 20(%r15)
    movabs $DATA_A, %rdi
    movl $0x11223344, (%rdi)
    movabs $DATA_B, %rdi
    movl $0x55667788, (%rdi)
    movabs $TEST_VA, %rax
    movq (%rax), %rbx
    movq %rbx, 32(%r15)
    movabs $PT_L1, %rdi
    movabs $(DATA_B | PAGE_PRESENT | PAGE_WRITE), %rbx
    movq %rbx, (%rdi)
    movq %rbx, 48(%r15)
    movabs $TEST_VA, %rax
    invlpg (%rax)
    movq (%rax), %rbx
    movq %rbx, 40(%r15)
    movabs $PT_L1, %rdi
    movq (%rdi), %rax
    movq %rax, 56(%r15)
    movabs $DATA_A, %rdi
    movq (%rdi), %rax
    movq %rax, 64(%r15)
    movabs $DATA_B, %rdi
    movq (%rdi), %rax
    movq %rax, 72(%r15)
.else
    .error "unsupported CASE_ID for paging.S"
.endif

paging_complete:
    movabs $RESULT_GPA, %r15
    movl $RESULT_COMPLETE, 16(%r15)
    cli
    hlt
    jmp .

/* Build the independent 4 KiB mapping at TEST_VA -> DATA_A. */
initialize_test_mapping:
    movabs $0x2000, %rdi
    movabs $(PT_L3 | 3), %rax
    movq %rax, 8(%rdi)          /* PML4[1] */
    movabs $PT_L3, %rdi
    movabs $(PT_L2 | 3), %rax
    movq %rax, (%rdi)           /* PDPTE[0] */
    movabs $PT_L2, %rdi
    movabs $(PT_L1 | 3), %rax
    movq %rax, (%rdi)           /* PDE[0] */
    movabs $PT_L1, %rdi
    movabs $(DATA_A | PAGE_PRESENT | PAGE_WRITE), %rax
    movq %rax, (%rdi)           /* PTE[0], A/D initially clear */
    ret

build_root_b:
    movabs $ROOT_B, %rdi
    movabs $(0x3000 | 3), %rax
    movq %rax, (%rdi)           /* PML4[0] retains runner identity map */
    movabs $(PT_B_L3 | 3), %rax
    movq %rax, 8(%rdi)          /* PML4[1] */
    movabs $PT_B_L3, %rdi
    movabs $(PT_B_L2 | 3), %rax
    movq %rax, (%rdi)
    movabs $PT_B_L2, %rdi
    movabs $(PT_B_L1 | 3), %rax
    movq %rax, (%rdi)
    movabs $PT_B_L1, %rdi
    movabs $(DATA_B | PAGE_PRESENT | PAGE_WRITE), %rax
    movq %rax, (%rdi)
    ret

record_test_entries:
#if CASE_ID == 103 || CASE_ID == 104
    movabs $0x2000, %rdi
    movq 8(%rdi), %rax
    movq %rax, 56(%r15)
    movabs $PT_L3, %rdi
    movq (%rdi), %rax
    movq %rax, 64(%r15)
    movabs $PT_L2, %rdi
    movq (%rdi), %rax
    movq %rax, 72(%r15)
    movabs $PT_L1, %rdi
    movq (%rdi), %rax
    movq %rax, 80(%r15)
#elif CASE_ID == 106
    /* nx_fetch reserves words 0..1 for support and raw CPUID. */
    movabs $0x2000, %rdi
    movq 8(%rdi), %rax
    movq %rax, 72(%r15)
    movabs $PT_L3, %rdi
    movq (%rdi), %rax
    movq %rax, 80(%r15)
    movabs $PT_L2, %rdi
    movq (%rdi), %rax
    movq %rax, 88(%r15)
    movabs $PT_L1, %rdi
    movq (%rdi), %rax
    movq %rax, 96(%r15)
#else
    movabs $0x2000, %rdi
    movq 8(%rdi), %rax
    movq %rax, 40(%r15)
    movabs $PT_L3, %rdi
    movq (%rdi), %rax
    movq %rax, 48(%r15)
    movabs $PT_L2, %rdi
    movq (%rdi), %rax
    movq %rax, 56(%r15)
    movabs $PT_L1, %rdi
    movq (%rdi), %rax
    movq %rax, 64(%r15)
#endif
    ret

install_page_fault_gate:
    movabs $IDT_GPA, %rdi
    movabs $page_fault_handler, %rax
    movw %ax, 224(%rdi)         /* vector 14, offset low */
    movw $0x8, 226(%rdi)        /* current 64-bit code selector */
    movb $0, 228(%rdi)          /* IST = 0 */
    movb $0x8e, 229(%rdi)       /* present, DPL0, 64-bit interrupt gate */
    shrq $16, %rax
    movw %ax, 230(%rdi)         /* offset middle */
    shrq $16, %rax
    movl %eax, 232(%rdi)        /* offset high */
    movl $0, 236(%rdi)
    lidt page_fault_idtr(%rip)
    ret

/* CPL0 #PF frame starts with error code, then RIP, CS and RFLAGS. */
page_fault_handler:
    pushq %rax
    pushq %rdx
    movq 16(%rsp), %rax         /* PFEC */
#if CASE_ID == 106
    movq %rax, 48(%r15)
    movq %cr2, %rax
    movq %rax, 56(%r15)
    movq 24(%rsp), %rdx         /* saved fault RIP */
    movq %rdx, 64(%r15)
#else
    movq %rax, 32(%r15)
    movq %cr2, %rax
    movq %rax, 40(%r15)
    movq 24(%rsp), %rdx         /* saved fault RIP */
    movq %rdx, 48(%r15)
#endif
    incl 24(%r15)
    movabs $FAULT_RESUME, %rax
    movq (%rax), %rax
    movq %rax, 24(%rsp)         /* redirect saved RIP to fixture recovery */
    popq %rdx
    popq %rax
    addq $8, %rsp               /* discard error code */
    iretq

.section .rodata,"a",@progbits
.align 8
page_fault_idtr:
    .word 239
    .quad IDT_GPA

.section .bss,"aw",@nobits
.align 4
saved_efer_low:
    .long 0
saved_efer_high:
    .long 0
