OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)
PHDRS { page_tables PT_LOAD FLAGS(6); code_head PT_LOAD FLAGS(5); code_tail PT_LOAD FLAGS(5); data0_decoy PT_LOAD FLAGS(6); data0_control PT_LOAD FLAGS(6); data0_value PT_LOAD FLAGS(6); data1_value PT_LOAD FLAGS(6); data1_decoy PT_LOAD FLAGS(6); }
SECTIONS {
 .page_tables 0x1000 : AT(0x1000) { KEEP(*(.page_tables)) } :page_tables
 ASSERT(SIZEOF(.page_tables) == 0x5000, "page tables must occupy 0x1000..0x5fff")
 .text_head 0x400ffe : AT(0x7ffe) { KEEP(*(.text.head)) } :code_head
 ASSERT(SIZEOF(.text_head) == 2, "the first code frame holds only the 66 8B prefix and opcode of the crossing MOV AX,[RBX]")
 ASSERT(ADDR(.text_head) == 0x400ffe, "the crossing instruction starts two bytes before the 4 KiB boundary")
 .text_tail 0x401000 : AT(0x9000) { KEEP(*(.text.tail)) } :code_tail
 ASSERT(SIZEOF(.text_tail) == 11, "second code frame: ModRM 03; qword, dword, word loads; OUT DX,AL; HLT")
 .data0_decoy 0x800000 : AT(0xb000) { KEEP(*(.data0.decoy)) } :data0_decoy
 ASSERT(SIZEOF(.data0_decoy) == 8, "first data frame decoy")
 .data0_control 0x800800 : AT(0xb800) { KEEP(*(.data0.control)) } :data0_control
 ASSERT(SIZEOF(.data0_control) == 8, "same-page control copy of the value")
 .data0_value 0x800ffd : AT(0xbffd) { KEEP(*(.data0.value)) } :data0_value
 ASSERT(SIZEOF(.data0_value) == 3, "three value bytes end the first data page")
 .data1_value 0x801000 : AT(0xd000) { KEEP(*(.data1.value)) } :data1_value
 ASSERT(SIZEOF(.data1_value) == 5, "five value bytes start the second data page")
 .data1_decoy 0x801ff8 : AT(0xdff8) { KEEP(*(.data1.decoy)) } :data1_decoy
 ASSERT(SIZEOF(.data1_decoy) == 8, "second data frame decoy")
 /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }
}
