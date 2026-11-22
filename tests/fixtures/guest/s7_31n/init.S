/*
 * S7-31n /init: efivarfs and the kernel ring buffer to COM1.
 *
 * Freestanding static x86-64 ELF, no libc. It uses only the instruction forms that the CPL3
 * allow-list of the CPU model already admits (the forms of tests/fixtures/guest/stage4/initramfs/init.S:
 * register moves, RIP-relative LEA, MOV imm, XOR/TEST register forms, short and near Jcc,
 * JMP, SYSCALL). It adds no ISA form.
 *
 * Flow: mkdir /sys; mount sysfs on /sys; mount efivarfs on /sys/firmware/efi/efivars; write
 * "OD_S7_31N_EFIVARFS_MOUNT_OK" or "_FAIL"; open the directory and write every getdents64 buffer
 * between the markers OD_S7_31N_EFIVARS_BEGIN and OD_S7_31N_EFIVARS_END (raw linux_dirent64
 * records: the host reads the names out of them); read the kernel ring buffer with
 * syslog(SYSLOG_ACTION_READ_ALL) and write it between OD_S7_31N_DMESG_BEGIN and
 * OD_S7_31N_DMESG_END (what `dmesg` prints; the host filters it); write OD_S7_31N_END; exit(0).
 * S7-31o: after the listing, open and read BootOrder, Boot0001 and ODBootCount through efivarfs
 * (openat/read) and write each file (4 attribute bytes, then the value) raw between
 * OD_S7_31O_<NAME>_BEGIN and OD_S7_31O_<NAME>_END. The console tty turns every 0x0A into 0D 0A
 * (ONLCR); the host undoes that by replacing each CR LF with LF.
 * System calls (syscall_64.tbl of linux-source-7.0.0 7.0.0-38.38): 0 read, 1 write, 60 exit, 83 mkdir, 103 syslog, 165 mount, 217 getdents64, 257 openat.
 */

#define SYS_read       0
#define SYS_write      1
#define SYS_exit       60
#define SYS_mkdir      83
#define SYS_syslog     103
#define SYS_mount      165
#define SYS_getdents64 217
#define SYS_openat     257
#define AT_FDCWD       (-100)
#define O_DIRECTORY    0x10000
#define DENT_SIZE      4096
#define VAL_SIZE       4096
#define LOG_SIZE       262144

/* S7-31o: dump one efivarfs file (4 attribute bytes, then the value) between its own markers. */
.macro dumpvar path, begin, end
	say	\begin
	movq	$AT_FDCWD, %rdi
	lea	\path(%rip), %rsi
	xor	%edx, %edx
	xor	%r10d, %r10d
	mov	$SYS_openat, %eax
	syscall
	test	%rax, %rax
	js	.Lskip\@
	mov	%rax, %rdi
	lea	valbuf(%rip), %rsi
	mov	$VAL_SIZE, %edx
	mov	$SYS_read, %eax
	syscall
	test	%rax, %rax
	jle	.Lskip\@
	mov	%rax, %rdx
	mov	$1, %edi
	lea	valbuf(%rip), %rsi
	mov	$SYS_write, %eax
	syscall
.Lskip\@:
	say	\end
.endm

.macro say label
	lea	\label(%rip), %rsi
	mov	$(\label\()_end - \label), %edx
	mov	$1, %edi
	mov	$SYS_write, %eax
	syscall
.endm

	.text
	.globl _start
_start:
	say	m_begin

	/* mkdir("/sys", 0555); mount("sysfs", "/sys", "sysfs", 0, NULL); results ignored. */
	lea	sys_dir(%rip), %rdi
	mov	$0555, %esi
	mov	$SYS_mkdir, %eax
	syscall
	lea	sysfs_name(%rip), %rdi
	lea	sys_dir(%rip), %rsi
	lea	sysfs_name(%rip), %rdx
	xor	%r10d, %r10d
	xor	%r8d, %r8d
	mov	$SYS_mount, %eax
	syscall

	/* mount("efivarfs", "/sys/firmware/efi/efivars", "efivarfs", 0, NULL) */
	lea	efivarfs_name(%rip), %rdi
	lea	efivars_dir(%rip), %rsi
	lea	efivarfs_name(%rip), %rdx
	xor	%r10d, %r10d
	xor	%r8d, %r8d
	mov	$SYS_mount, %eax
	syscall
	test	%rax, %rax
	js	mount_failed
	say	m_mount_ok
	jmp	list
mount_failed:
	say	m_mount_fail

list:
	say	m_efivars_begin
	/* fd = openat(AT_FDCWD, "/sys/firmware/efi/efivars", O_RDONLY | O_DIRECTORY, 0) */
	movq	$AT_FDCWD, %rdi
	lea	efivars_dir(%rip), %rsi
	mov	$O_DIRECTORY, %edx
	xor	%r10d, %r10d
	mov	$SYS_openat, %eax
	syscall
	test	%rax, %rax
	js	listed
	mov	%rax, %r12
dents:
	mov	%r12, %rdi
	lea	dentbuf(%rip), %rsi
	mov	$DENT_SIZE, %edx
	mov	$SYS_getdents64, %eax
	syscall
	test	%rax, %rax
	jle	listed
	mov	%rax, %rdx
	mov	$1, %edi
	lea	dentbuf(%rip), %rsi
	mov	$SYS_write, %eax
	syscall
	jmp	dents
listed:
	say	m_efivars_end

	dumpvar	p_bootorder, m_v1_begin, m_v1_end
	dumpvar	p_boot0001, m_v2_begin, m_v2_end
	dumpvar	p_odbootcount, m_v3_begin, m_v3_end

	say	m_dmesg_begin
	/* n = syslog(SYSLOG_ACTION_READ_ALL = 3, logbuf, LOG_SIZE) */
	mov	$3, %edi
	lea	logbuf(%rip), %rsi
	mov	$LOG_SIZE, %edx
	mov	$SYS_syslog, %eax
	syscall
	test	%rax, %rax
	jle	logged
	mov	%rax, %rdx
	mov	$1, %edi
	lea	logbuf(%rip), %rsi
	mov	$SYS_write, %eax
	syscall
logged:
	say	m_dmesg_end

	say	m_end
	xor	%edi, %edi
	mov	$SYS_exit, %eax
	syscall
1:	jmp	1b

	.section .rodata
sys_dir:
	.asciz	"/sys"
sysfs_name:
	.asciz	"sysfs"
efivarfs_name:
	.asciz	"efivarfs"
efivars_dir:
	.asciz	"/sys/firmware/efi/efivars"
p_bootorder:
	.asciz	"/sys/firmware/efi/efivars/BootOrder-8be4df61-93ca-11d2-aa0d-00e098032b8c"
p_boot0001:
	.asciz	"/sys/firmware/efi/efivars/Boot0001-8be4df61-93ca-11d2-aa0d-00e098032b8c"
p_odbootcount:
	.asciz	"/sys/firmware/efi/efivars/ODBootCount-6f1d52a4-26b0-4f0c-9a37-4e0b61c82d15"
m_begin:
	.ascii	"OD_S7_31N_BEGIN\n"
m_begin_end:
m_mount_ok:
	.ascii	"OD_S7_31N_EFIVARFS_MOUNT_OK\n"
m_mount_ok_end:
m_mount_fail:
	.ascii	"OD_S7_31N_EFIVARFS_MOUNT_FAIL\n"
m_mount_fail_end:
m_efivars_begin:
	.ascii	"\nOD_S7_31N_EFIVARS_BEGIN\n"
m_efivars_begin_end:
m_efivars_end:
	.ascii	"\nOD_S7_31N_EFIVARS_END\n"
m_efivars_end_end:
m_v1_begin:
	.ascii	"OD_S7_31O_BOOTORDER_BEGIN\n"
m_v1_begin_end:
m_v1_end:
	.ascii	"OD_S7_31O_BOOTORDER_END\n"
m_v1_end_end:
m_v2_begin:
	.ascii	"OD_S7_31O_BOOT0001_BEGIN\n"
m_v2_begin_end:
m_v2_end:
	.ascii	"OD_S7_31O_BOOT0001_END\n"
m_v2_end_end:
m_v3_begin:
	.ascii	"OD_S7_31O_ODBOOTCOUNT_BEGIN\n"
m_v3_begin_end:
m_v3_end:
	.ascii	"OD_S7_31O_ODBOOTCOUNT_END\n"
m_v3_end_end:
m_dmesg_begin:
	.ascii	"OD_S7_31N_DMESG_BEGIN\n"
m_dmesg_begin_end:
m_dmesg_end:
	.ascii	"\nOD_S7_31N_DMESG_END\n"
m_dmesg_end_end:
m_end:
	.ascii	"OD_S7_31N_END\n"
m_end_end:

	.bss
	.balign	16
dentbuf:
	.skip	DENT_SIZE
valbuf:
	.skip	VAL_SIZE
logbuf:
	.skip	LOG_SIZE

	.section .note.GNU-stack,"",@progbits
