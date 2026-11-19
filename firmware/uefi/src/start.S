/*
 * Startup stub of the Open Desktop firmware image (own; assembled by the system clang with
 * `--target=x86_64-unknown-none-elf`, see build.rs).
 *
 * Entry contract (docs/specs/FIRMWARE_BOOT.md, section 5): Long64, CPL0, interrupts off, DF = 0,
 * RSP = 0x0015_0000 (16-byte aligned, no return address), RDI = GPA of the handoff block. The stub
 * does not parse the handoff block. It first checks RDI = 0x0010_0000 (FIRMWARE_BOOT.md, section 5:
 * the check is in this stub, before any Rust code); on a mismatch it prints the stop line
 * `OD-FW STOP code=0x01 detail=0x<RDI>` to COM1 and halts, using no stack and no .bss. Otherwise it
 * zeroes .bss, keeps RDI and calls the Rust entry point (which repeats the check, defence in depth).
 */

    .section .image_header, "a", @progbits
    .globl __image_header
__image_header:
    /* Reserved: 0x40 zero bytes. build_firmware.py writes the ODFWIMG1 header here. */
    .fill 0x40, 1, 0

    .section .text.start, "ax", @progbits
    .code64
    .globl _start
    .type _start, @function
_start:
    cld
    movq    $0x100000, %rax             /* HANDOFF_GPA */
    cmpq    %rax, %rdi
    jne     .Lbad_handoff
    movq    %rdi, %r8                   /* keep the handoff GPA across BSS clearing */
    leaq    __bss_start(%rip), %rdi
    leaq    __bss_end(%rip), %r10
    subq    %rdi, %r10
    shrq    $3, %r10                    /* the linker aligns both ends */
    xorl    %eax, %eax
.Lzero_bss:
    testq   %r10, %r10
    jz      .Lbss_done
    movl    $512, %ecx                  /* at most 4 KiB per REP */
    cmpq    %rcx, %r10
    cmovbq  %r10, %rcx
    subq    %rcx, %r10
    rep stosq
    jmp     .Lzero_bss
.Lbss_done:
    movq    %r8, %rdi                   /* first argument of od_fw_main (System V, internal) */
    call    od_fw_main
    /* od_fw_main never returns; stop if it does. */
1:  cli
    hlt
    jmp     1b
    .size _start, . - _start

/*
 * Stop path for RDI != 0x0010_0000: same line as stop.rs (code 0x01, detail = RDI). No stack, no
 * memory writes: subroutines return through registers (r9 = putc return, r12 = puts return).
 * COM1 setup as in FIRMWARE_BOOT.md, section 11: 8N1, divisor 1; polls THRE at most 65536 times
 * per byte, then drops the byte.
 */
.Lbad_handoff:
    movq    %rdi, %r8                   /* detail = the RDI that was received */
    movw    $0x3FB, %dx                 /* LCR: DLAB = 1 */
    movb    $0x80, %al
    outb    %al, %dx
    movw    $0x3F8, %dx                 /* divisor latch low = 1 */
    movb    $0x01, %al
    outb    %al, %dx
    movw    $0x3F9, %dx                 /* divisor latch high = 0 */
    xorl    %eax, %eax
    outb    %al, %dx
    movw    $0x3FB, %dx                 /* LCR: 8N1, DLAB = 0 */
    movb    $0x03, %al
    outb    %al, %dx
    leaq    .Lstop_prefix(%rip), %rsi
    leaq    10f(%rip), %r12
    jmp     .Lputs
10: leaq    .Lhex_digits(%rip), %rdi
    movl    $16, %r13d
11: rolq    $4, %r8                     /* next nibble, most significant first */
    movl    %r8d, %eax
    andl    $0xF, %eax
    movb    (%rdi,%rax), %bl
    leaq    12f(%rip), %r9
    jmp     .Lputc
12: decl    %r13d
    jnz     11b
    leaq    .Lstop_suffix(%rip), %rsi
    leaq    13f(%rip), %r12
    jmp     .Lputs
13: cli
    hlt
    jmp     13b

/* putc: byte in %bl, return address in %r9; clobbers %al, %dx, %r11d. */
.Lputc:
    movl    $65536, %r11d
20: movw    $0x3FD, %dx                 /* LSR */
    inb     %dx, %al
    testb   $0x20, %al                  /* THRE */
    jnz     21f
    decl    %r11d
    jnz     20b
    jmp     *%r9                        /* timeout: drop the byte, never hang */
21: movw    $0x3F8, %dx
    movb    %bl, %al
    outb    %al, %dx
    jmp     *%r9

/* puts: NUL-terminated string at %rsi, return address in %r12; clobbers %bl, %r9 and putc's. */
.Lputs:
30: movb    (%rsi), %bl
    testb   %bl, %bl
    jz      31f
    leaq    32f(%rip), %r9
    jmp     .Lputc
32: incq    %rsi
    jmp     30b
31: jmp     *%r12

.Lstop_prefix:
    .asciz  "OD-FW STOP code=0x01 detail=0x"
.Lstop_suffix:
    .asciz  "\r\n"
.Lhex_digits:
    .ascii  "0123456789ABCDEF"


    .section .data,"aw",@progbits
    .p2align 3
    .globl od_fw_gdt
od_fw_gdt:
    .quad 0
    .quad 0x00af9a000000ffff           /* 08: present ring-0 long-mode code */
    .quad 0x00cf92000000ffff           /* 10: present ring-0 writable data */
.Lgdt_end:

    .section .rodata,"a",@progbits
    .p2align 3
.Lgdtr:
    .word .Lgdt_end - od_fw_gdt - 1
    .quad od_fw_gdt

    .section .text,"ax",@progbits
    .globl od_fw_install_gdt
    .type od_fw_install_gdt, @function
od_fw_install_gdt:
    lgdt    .Lgdtr(%rip)
    movl    $0x08, %eax
    pushq   %rax
    leaq    .Lgdt_reload(%rip), %rax
    pushq   %rax
    lretq
.Lgdt_reload:
    movw    $0x10, %ax
    movw    %ax, %ds
    movw    %ax, %es
    movw    %ax, %fs
    movw    %ax, %gs
    movw    %ax, %ss
    nop                               /* consumes the MOV SS interrupt shadow */
    retq
    .size od_fw_install_gdt, .-od_fw_install_gdt


    .p2align 4
    .globl od_fw_exception_stubs
    .type od_fw_exception_stubs, @function
od_fw_exception_stubs:
    /* Each entry is ten bytes: MOV EDI,vector and JMP rel32. Using fixed
       encodings keeps the Rust IDT encoder's vector stride invariant. */
    .set .Lvector, 0
    .rept 256
    .byte 0xbf
    .long .Lvector
    .byte 0xe9
    .long .Lexception_stop - . - 4
    .set .Lvector, .Lvector + 1
    .endr
    .size od_fw_exception_stubs, .-od_fw_exception_stubs
.Lexception_stop:
    cld                               /* System V requires DF = 0 at Rust entry */
    andq    $-16, %rsp
    callq   od_fw_exception_stop
    ud2
