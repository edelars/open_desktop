/* Linux/x86-64 PID 1 using the stage4a CPL3 instruction profile. */

#define SYS_READ       0
#define SYS_WRITE      1
#define SYS_CLOSE      3
#define SYS_NANOSLEEP 35
#define SYS_MKDIR     83
#define SYS_MOUNT    165
#define SYS_OPENAT   257
#define AT_FDCWD    -100
#define EINTR          4
#define EBUSY         16
#define EEXIST        17
#define MAX_CPU_ID  4095
#define MIN_NONCE     16
#define MAX_NONCE    128

/* The Linux entry stack is nonzero. Jcc rel32 is available for distant labels. */
.macro JUMP target
    testq %rsp, %rsp
    jne \target
.endm

.macro MOUNT_FS path, filesystem
    leaq \path(%rip), %rdi
    mov $0555, %esi
    mov $SYS_MKDIR, %eax
    syscall
    testq %rax, %rax
    jns .Lmount\@
    movq $-EEXIST, %r10
    cmpq %r10, %rax
    jne .Lmount_error
.Lmount\@:
    leaq \filesystem(%rip), %rdi
    leaq \path(%rip), %rsi
    leaq \filesystem(%rip), %rdx
    xor %r10d, %r10d
    xor %r8d, %r8d
    mov $SYS_MOUNT, %eax
    syscall
    testq %rax, %rax
    jz .Lmounted\@
    movq $-EBUSY, %r10
    cmpq %r10, %rax
    jne .Lmount_error
.Lmounted\@:
.endm

/* Read the complete file into input_buffer. Return its end in R13. */
.macro READ_FILE path, capacity
    movq $AT_FDCWD, %rdi
    leaq \path(%rip), %rsi
    xor %edx, %edx
    xor %r10d, %r10d
    mov $SYS_OPENAT, %eax
    syscall
    testq %rax, %rax
    js .Linput_error
    movq %rax, %r12
    leaq input_buffer(%rip), %r13
    mov $\capacity, %r14d
.Lread\@:
    movq %r12, %rdi
    movq %r13, %rsi
    movq %r14, %rdx
    testq %rdx, %rdx
    jnz .Lread_syscall\@
    /* Verify EOF if the file filled the buffer exactly. */
    leaq overflow_byte(%rip), %rsi
    mov $1, %edx
.Lread_syscall\@:
    mov $SYS_READ, %eax
    syscall
    movq $-EINTR, %r10
    cmpq %r10, %rax
    je .Lread\@
    testq %rax, %rax
    jz .Lclose\@
    js .Lread_error\@
    testq %r14, %r14
    jz .Lread_error\@
    leaq (%r13,%rax), %r13
    subq %rax, %r14
    jmp .Lread\@
.Lread_error\@:
    movq %r12, %rdi
    mov $SYS_CLOSE, %eax
    syscall
    JUMP .Linput_error
.Lclose\@:
    movq %r12, %rdi
    mov $SYS_CLOSE, %eax
    syscall
.endm

/* Append a literal to the output buffer at RDI. */
.macro APPEND label
    leaq \label(%rip), %rsi
    mov $(\label\()_end - \label), %ecx
    rep movsb
.endm

/* Parse a decimal CPU index at RBX, with an exclusive limit at R13. */
.macro CPU_INDEX value, value32
    movq %rbx, %rsi
    xor \value32, \value32
.Lcpu_digit\@:
    cmpq %r13, %rbx
    jae .Lcpu_end\@
    movzbl (%rbx), %eax
    cmpb $'0', %al
    jb .Lcpu_end\@
    cmpb $'9', %al
    ja .Lcpu_end\@
    subq %r11, %rax
    leaq (\value,\value,4), \value
    leaq (%rax,\value,2), \value
    cmpq $MAX_CPU_ID, \value
    ja .Linput_error
    incq %rbx
    jmp .Lcpu_digit\@
.Lcpu_end\@:
    cmpq %rsi, %rbx
    je .Linput_error
.endm

/* Emit a decimal column from R9, suppressing leading zeros with R8. */
.macro COUNT_DIGIT place
    mov $\place, %r10d
    xor %eax, %eax
.Lcount_digit\@:
    cmpq %r10, %r9
    jb .Lcount_emit\@
    subq %r10, %r9
    incq %rax
    jmp .Lcount_digit\@
.Lcount_emit\@:
    .if \place != 1
        testq %r8, %r8
        jnz .Lappend_digit\@
        testq %rax, %rax
        jz .Lcount_done\@
    .endif
.Lappend_digit\@:
    mov $1, %r8d
    leaq (%rbx,%rax), %rsi
    mov $1, %ecx
    rep movsb
.Lcount_done\@:
.endm

.text
.globl _start
_start:
    MOUNT_FS proc_path, proc_name
    MOUNT_FS sys_path, sys_name

    READ_FILE cmdline_path, 4096
    leaq input_buffer(%rip), %rbx
    xor %r9d, %r9d           /* Length of the unique nonce when found. */
.Lscan:
    cmpq %r13, %rbx
    jae .Lcmdline_done
    movb (%rbx), %al
    cmpb $' ', %al
    je .Lseparator
    cmpb $'\t', %al
    je .Lseparator
    cmpb $'\n', %al
    je .Lseparator
    movq %rbx, %r12
    leaq nonce_key(%rip), %rsi
    mov $(nonce_key_end - nonce_key), %ecx
.Lkey:
    cmpq %r13, %rbx
    jae .Lkey_mismatch
    movb (%rsi), %al
    cmpb %al, (%rbx)
    jne .Lkey_mismatch
    incq %rsi
    incq %rbx
    decl %ecx
    jnz .Lkey
    testq %r9, %r9
    jnz .Linput_error
    movq %rbx, %r8
.Lnonce:
    cmpq %r13, %rbx
    jae .Lnonce_done
    movb (%rbx), %al
    cmpb $' ', %al
    je .Lnonce_done
    cmpb $'\t', %al
    je .Lnonce_done
    cmpb $'\n', %al
    je .Lnonce_done
    cmpb $'0', %al
    jb .Linput_error
    cmpb $'9', %al
    jbe .Lnonce_byte
    cmpb $'A', %al
    jb .Linput_error
    cmpb $'Z', %al
    jbe .Lnonce_byte
    cmpb $'a', %al
    jb .Linput_error
    cmpb $'z', %al
    ja .Linput_error
.Lnonce_byte:
    incq %rbx
    jmp .Lnonce
.Lnonce_done:
    movq %rbx, %r9
    subq %r8, %r9
    mov $MIN_NONCE, %eax
    cmpq %rax, %r9
    jb .Linput_error
    cmpq $MAX_NONCE, %r9
    ja .Linput_error
    JUMP .Lscan
.Lkey_mismatch:
    movq %r12, %rbx
.Lskip_token:
    cmpq %r13, %rbx
    jae .Lcmdline_done
    movb (%rbx), %al
    cmpb $' ', %al
    je .Lseparator
    cmpb $'\t', %al
    je .Lseparator
    cmpb $'\n', %al
    je .Lseparator
    incq %rbx
    jmp .Lskip_token
.Lseparator:
    incq %rbx
    JUMP .Lscan
.Lcmdline_done:
    testq %r9, %r9
    jz .Linput_error
    leaq output_buffer(%rip), %rdi
    APPEND observation_prefix
    movq %r8, %rsi
    movq %r9, %rcx
    rep movsb
    APPEND online_prefix
    movq %rdi, %rbp

    READ_FILE online_path, 4096
    leaq input_buffer(%rip), %rbx
    cmpq %rbx, %r13
    je .Linput_error
    cmpb $'\n', -1(%r13)
    jne .Linput_error
    mov $1, %eax
    subq %rax, %r13
    mov $'0', %r11d
    movq $-1, %r8           /* End of the preceding range. */
    xor %r9d, %r9d          /* Number of online CPUs. */
.Lcpu_group:
    CPU_INDEX %r10, %r10d
    cmpq %r8, %r10
    jle .Linput_error
    movq %r10, %r12
    cmpq %r13, %rbx
    jae .Lrange_done
    cmpb $'-', (%rbx)
    jne .Lrange_done
    incq %rbx
    CPU_INDEX %r12, %r12d
    cmpq %r10, %r12
    jb .Linput_error
.Lrange_done:
    movq %r12, %r8
    subq %r10, %r12
    incq %r12
    leaq (%r9,%r12), %r9
    cmpq %r13, %rbx
    je .Lonline_done
    cmpb $',', (%rbx)
    jne .Linput_error
    incq %rbx
    JUMP .Lcpu_group
.Lonline_done:
    movq %rbp, %rdi
    leaq input_buffer(%rip), %rsi
    movq %r13, %rcx
    subq %rsi, %rcx
    incq %rcx               /* Include the original newline. */
    rep movsb
    APPEND count_prefix
    leaq decimal_digits(%rip), %rbx
    xor %r8d, %r8d
    COUNT_DIGIT 1000
    COUNT_DIGIT 100
    COUNT_DIGIT 10
    COUNT_DIGIT 1
    APPEND irq_prefix
    movq %rdi, %rbp

    READ_FILE interrupts_path, 16384
    leaq input_buffer(%rip), %rsi
    cmpq %rsi, %r13
    je .Linput_error
    cmpb $'\n', -1(%r13)
    jne .Linput_error
    movq %rbp, %rdi
    movq %r13, %rcx
    subq %rsi, %rcx
    rep movsb
    APPEND observation_suffix
    leaq output_buffer(%rip), %rsi
    movq %rdi, %rdx
    subq %rsi, %rdx
    xor %r15d, %r15d
.Lwrite:
    mov $1, %edi
    mov $SYS_WRITE, %eax
    syscall
    movq $-EINTR, %r10
    cmpq %r10, %rax
    je .Lwrite
    testq %rax, %rax
    jle .Lwrite_error
    leaq (%rsi,%rax), %rsi
    subq %rax, %rdx
    jnz .Lwrite
    JUMP .Lidle

.Lmount_error:
    leaq mount_error(%rip), %rsi
    mov $(mount_error_end - mount_error), %edx
    jmp .Lprint_error
.Linput_error:
    leaq input_error(%rip), %rsi
    mov $(input_error_end - input_error), %edx
    jmp .Lprint_error
.Lwrite_error:
    testq %r15, %r15
    jnz .Lidle
    leaq write_error(%rip), %rsi
    mov $(write_error_end - write_error), %edx
.Lprint_error:
    mov $1, %r15d
    JUMP .Lwrite
.Lidle:
    leaq sleep_time(%rip), %rdi
    xor %esi, %esi
    mov $SYS_NANOSLEEP, %eax
    syscall
    jmp .Lidle

.section .rodata
proc_path:         .asciz "/proc"
proc_name:         .asciz "proc"
sys_path:          .asciz "/sys"
sys_name:          .asciz "sysfs"
cmdline_path:      .asciz "/proc/cmdline"
online_path:       .asciz "/sys/devices/system/cpu/online"
interrupts_path:   .asciz "/proc/interrupts"
nonce_key:         .ascii "od.nonce="
nonce_key_end:
decimal_digits:    .ascii "0123456789"
observation_prefix: .ascii "OD_SMP_V1 "
observation_prefix_end:
online_prefix:     .ascii "\nOD_CPU_ONLINE "
online_prefix_end:
count_prefix:      .ascii "OD_CPU_COUNT "
count_prefix_end:
irq_prefix:        .ascii "\nOD_IRQ_BEGIN\n"
irq_prefix_end:
observation_suffix: .ascii "OD_IRQ_END\nOD_SMP_END\n"
observation_suffix_end:
mount_error:       .ascii "OD_SMP_ERROR mount\n"
mount_error_end:
input_error:       .ascii "OD_SMP_ERROR input\n"
input_error_end:
write_error:       .ascii "OD_SMP_ERROR write\n"
write_error_end:
.balign 8
sleep_time:        .quad 1, 0

.bss
input_buffer:      .skip 16384
overflow_byte:     .skip 1
/* 128 bytes of nonce, 4096 of cpulist and 16384 of interrupts plus markers. */
output_buffer:     .skip 32768
