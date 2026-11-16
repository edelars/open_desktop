/*
 * Stage 4 initramfs /init (S4-36).
 *
 * Freestanding static x86-64 ELF, no libc, no dynamic loader. Only Linux x86-64
 * system calls are used (numbers from arch/x86/entry/syscalls/syscall_64.tbl of
 * linux-source-7.0.0 7.0.0-38.38):
 *   0 read, 1 write, 3 close, 35 nanosleep, 39 getpid, 60 exit,
 *   83 mkdir, 165 mount, 257 openat.
 *
 * Flow: mkdir /proc and mount proc on it (the kernel does not mount /proc in an
 * initramfs), read /proc/cmdline, find the token "od.nonce=<value>", nanosleep
 * 1 ms, write "OD_NONCE=<value>\n" to fd 1, getpid, exit(0).
 * Without a usable od.nonce token: write "OD_NONCE=MISSING\n", exit(1).
 * The nonce is never part of the archive; the host passes it on the command line.
 *
 * Exit codes: 0 ok, 1 nonce missing, 3 write failed, 4 getpid failed,
 * 5 nanosleep failed.
 */

#define SYS_read       0
#define SYS_write      1
#define SYS_close      3
#define SYS_nanosleep  35
#define SYS_getpid     39
#define SYS_exit       60
#define SYS_mkdir      83
#define SYS_mount      165
#define SYS_openat     257

#define AT_FDCWD       (-100)
#define CMDBUF_SIZE    4096
#define NONCE_MAX      128

	.text
	.globl _start
_start:
	/* mkdir("/proc", 0555); the result is ignored (EEXIST is fine). */
	lea	proc_dir(%rip), %rdi
	mov	$0555, %esi
	mov	$SYS_mkdir, %eax
	syscall

	/* mount("proc", "/proc", "proc", 0, NULL); the result is ignored. */
	lea	fs_name(%rip), %rdi
	lea	proc_dir(%rip), %rsi
	lea	fs_name(%rip), %rdx
	xor	%r10d, %r10d
	xor	%r8d, %r8d
	mov	$SYS_mount, %eax
	syscall

	/* fd = openat(AT_FDCWD, "/proc/cmdline", O_RDONLY, 0) */
	movq	$AT_FDCWD, %rdi
	lea	cmdline_path(%rip), %rsi
	xor	%edx, %edx
	xor	%r10d, %r10d
	mov	$SYS_openat, %eax
	syscall
	test	%rax, %rax
	js	missing
	mov	%rax, %r12

	/* len = read(fd, cmdbuf, CMDBUF_SIZE - 1) */
	mov	%r12, %rdi
	lea	cmdbuf(%rip), %rsi
	mov	$(CMDBUF_SIZE - 1), %edx
	mov	$SYS_read, %eax
	syscall
	test	%rax, %rax
	js	missing
	mov	%rax, %r13

	/* close(fd) */
	mov	%r12, %rdi
	mov	$SYS_close, %eax
	syscall

	lea	cmdbuf(%rip), %r14
	movb	$0, (%r14,%r13)
	xor	%ebx, %ebx

/* Scan for "od.nonce=" at the start of the buffer or after a space. */
scan:
	cmp	%r13, %rbx
	jae	missing
	test	%rbx, %rbx
	jz	compare
	cmpb	$0x20, -1(%r14,%rbx)
	jne	next
compare:
	lea	needle(%rip), %rsi
	lea	(%r14,%rbx), %rdi
	mov	$(needle_end - needle), %ecx
cmp_loop:
	test	%ecx, %ecx
	jz	found
	movb	(%rsi), %al
	cmpb	%al, (%rdi)
	jne	next
	inc	%rsi
	inc	%rdi
	dec	%ecx
	jmp	cmp_loop
next:
	inc	%rbx
	jmp	scan

/* %rdi points to the first byte of the value. */
found:
	mov	%rdi, %r8
	xor	%ecx, %ecx
val_loop:
	movzbl	(%r8,%rcx), %eax
	test	%al, %al
	jz	val_done
	cmp	$0x20, %al
	je	val_done
	cmp	$0x0a, %al
	je	val_done
	inc	%rcx
	cmp	$NONCE_MAX, %rcx
	jb	val_loop
val_done:
	test	%rcx, %rcx
	jz	missing
	mov	%rcx, %r9
	/* nanosleep(&{0 s, 1000000 ns}, NULL): must return 0 */
	lea	sleep_ts(%rip), %rdi
	xor	%esi, %esi
	mov	$SYS_nanosleep, %eax
	syscall
	test	%rax, %rax
	jnz	fail_sleep

	/* outbuf = "OD_NONCE=" + value + "\n" */
	lea	outbuf(%rip), %rdi
	lea	prefix(%rip), %rsi
	mov	$(prefix_end - prefix), %ecx
	rep movsb
	mov	%r8, %rsi
	mov	%r9, %rcx
	rep movsb
	movb	$0x0a, (%rdi)
	inc	%rdi
	lea	outbuf(%rip), %rsi
	mov	%rdi, %rdx
	sub	%rsi, %rdx
	mov	$1, %edi
	mov	$SYS_write, %eax
	syscall
	test	%rax, %rax
	js	fail_write

	/* getpid(): must be positive */
	mov	$SYS_getpid, %eax
	syscall
	test	%rax, %rax
	jle	fail_getpid

	xor	%edi, %edi
	jmp	do_exit

missing:
	mov	$1, %edi
	lea	missing_msg(%rip), %rsi
	mov	$(missing_msg_end - missing_msg), %edx
	mov	$SYS_write, %eax
	syscall
	mov	$1, %edi
	jmp	do_exit

fail_write:
	mov	$3, %edi
	jmp	do_exit
fail_getpid:
	mov	$4, %edi
	jmp	do_exit
fail_sleep:
	mov	$5, %edi
do_exit:
	mov	$SYS_exit, %eax
	syscall
1:	jmp	1b

	.section .rodata
proc_dir:
	.asciz	"/proc"
fs_name:
	.asciz	"proc"
cmdline_path:
	.asciz	"/proc/cmdline"
needle:
	.ascii	"od.nonce="
needle_end:
prefix:
	.ascii	"OD_NONCE="
prefix_end:
missing_msg:
	.ascii	"OD_NONCE=MISSING\n"
missing_msg_end:
	.balign	8
sleep_ts:
	.quad	0
	.quad	1000000

	.bss
	.balign	16
cmdbuf:
	.skip	CMDBUF_SIZE
outbuf:
	.skip	NONCE_MAX + 16

	.section .note.GNU-stack,"",@progbits
