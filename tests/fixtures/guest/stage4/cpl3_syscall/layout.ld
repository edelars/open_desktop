OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)
PHDRS { page_tables PT_LOAD FLAGS(6); user_code PT_LOAD FLAGS(5); kernel_code PT_LOAD FLAGS(5); guest_data PT_LOAD FLAGS(6); }
SECTIONS {
 .page_tables 0x1000 : AT(0x1000) { KEEP(*(.page_tables)) } :page_tables
 ASSERT(SIZEOF(.page_tables) == 0x6000, "page tables must occupy 0x1000..0x6fff")
 .text_user 0x400000 : AT(0x7000) { FILL(0x00) KEEP(*(.text.user)) } :user_code
 ASSERT(SIZEOF(.text_user) == 5, "user code is exactly SYSCALL; MOV RAX,[RBX]")
 .text_kernel 0x600000 : AT(0x8000) { FILL(0x00) KEEP(*(.text.kernel)) } :kernel_code
 ASSERT(SIZEOF(.text_kernel) == 6, "kernel code is exactly MOV [RDI],R10; SYSRETQ")
 .data 0x800000 : AT(0x9000) { KEEP(*(.data.payload)) } :guest_data
 ASSERT(SIZEOF(.data) == 16, "image must contain initial and store qwords")
 /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }
}
