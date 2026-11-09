/* S4-34a: freestanding Long64/CPL0 guest for the KVM method probe.
 * Loaded at GPA 0x1000 (entry RIP 0x1000). Writes "OK\n" to COM1 (0x3f8)
 * and halts. No interrupts, no memory access. Injection builds:
 *   -DINJECT_NO_HLT    ends in `jmp .` instead of HLT (host sees budget exit)
 *   -DINJECT_WRONG_STR emits "NO\n" instead of "OK\n"
 */
#ifdef INJECT_WRONG_STR
#define BYTE0 'N'
#define BYTE1 'O'
#else
#define BYTE0 'O'
#define BYTE1 'K'
#endif

.section .text.entry,"ax",@progbits
.code64
.global _start
_start:
    movw $0x3f8, %dx
    movb $BYTE0, %al
    outb %al, %dx
    movb $BYTE1, %al
    outb %al, %dx
    movb $0x0a, %al
    outb %al, %dx
#ifdef INJECT_NO_HLT
1:  jmp 1b
#else
    hlt
#endif
.global kvm_method_end
kvm_method_end:
