/*
 * S4-34c: bounded CPL0 reads and writes of CR0, CR3, and CR4.
 * Build with layout.ld. Control-register forms follow Intel SDM Vol. 2
 * MOV-to/from-control-register instruction entry; CR semantics are Vol. 3A.
 */

#define RESULT_GPA 0x18000
#define RESULT_MAGIC 0x314d564b57483453
#define RESULT_COMPLETE 0x434f4d50
#define CASE_ID 201
#define CR0_WP 0x10000
#define CR4_PGE 0x80

.section .text,"ax",@progbits
.code64
.global _start
_start:
    cli
    cld
    movabs $RESULT_GPA, %r15
    movabs $RESULT_MAGIC, %rax
    movq %rax, 0(%r15)
    movl $1, 8(%r15)
    movl $CASE_ID, 12(%r15)
    movl $0, 16(%r15)
    movl $13, 20(%r15)
    movl $0, 24(%r15)
    movl $0, 28(%r15)

    /* Words 0..3: original CR0, after setting WP, after clearing WP,
     * and restored CR0. Record values read back from the control register.
     */
    movq %cr0, %r12
    movq %r12, 32(%r15)
    movq %r12, %rax
    orq $CR0_WP, %rax
    movq %rax, %cr0
    movq %cr0, %rax
    movq %rax, 40(%r15)
    andq $~CR0_WP, %rax
    movq %rax, %cr0
    movq %cr0, %rax
    movq %rax, 48(%r15)
    movq %r12, %cr0
    movq %cr0, %rax
    movq %rax, 56(%r15)

    /* Words 4..6: original CR3, readback after same-value CR3 write,
     * and final CR3 readback. The write exercises the architectural
     * translation-lookaside-buffer invalidation path for this root.
     */
    movq %cr3, %r13
    movq %r13, 64(%r15)
    movq %r13, %cr3
    movq %cr3, %rax
    movq %rax, 72(%r15)
    movq %cr3, %rax
    movq %rax, 80(%r15)

    /* Words 7..10: original CR4, after setting PGE, after clearing PGE,
     * and restored CR4. CPUID.1:EDX is word 11 and PGE support is word 12.
     * Unsupported PGE is reported with unchanged raw CR4 readbacks.
     */
    movq %cr4, %r14
    movq %r14, 88(%r15)
    movl $0, 128(%r15)
    movl $1, %eax
    cpuid
    movl %edx, 120(%r15)
    testl $0x00002000, %edx
    jz pge_unsupported
    movl $1, 128(%r15)

    movq %r14, %rax
    orq $CR4_PGE, %rax
    movq %rax, %cr4
    movq %cr4, %rax
    movq %rax, 96(%r15)
    andq $~CR4_PGE, %rax
    movq %rax, %cr4
    movq %cr4, %rax
    movq %rax, 104(%r15)
    movq %r14, %cr4
    movq %cr4, %rax
    movq %rax, 112(%r15)
    jmp control_complete

pge_unsupported:
    movq %r14, 96(%r15)
    movq %r14, 104(%r15)
    movq %r14, %cr4
    movq %cr4, %rax
    movq %rax, 112(%r15)

control_complete:
    movl $RESULT_COMPLETE, 16(%r15)
    cli
    hlt
    jmp .
