OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)
PHDRS { page_tables PT_LOAD FLAGS(6); code PT_LOAD FLAGS(5); decoy PT_LOAD FLAGS(6); huge_start PT_LOAD FLAGS(6); huge_middle PT_LOAD FLAGS(6); huge_store PT_LOAD FLAGS(6); huge_end PT_LOAD FLAGS(6); huge_sentinel PT_LOAD FLAGS(6); }
SECTIONS {
 .page_tables 0x1000 : AT(0x1000) { KEEP(*(.page_tables)) } :page_tables
 ASSERT(SIZEOF(.page_tables) == 0x6000, "page tables must occupy 0x1000..0x6fff")
 .text_code 0x400000 : AT(0x7000) { KEEP(*(.text.code)) } :code
 ASSERT(SIZEOF(.text_code) == 17, "three loads, one store, one load, OUT DX,AL, HLT")
 .decoy_frame 0x600000 : AT(0x8000) { KEEP(*(.decoy.frame)) } :decoy
 ASSERT(SIZEOF(.decoy_frame) == 8, "decoy qword of the 4 KiB frame used by the PDE.PS injection")
 .huge_start 0x800000 : AT(0x200000) { KEEP(*(.huge.start)) } :huge_start
 ASSERT(SIZEOF(.huge_start) == 8, "start qword of the 2 MiB frame")
 .huge_middle 0x900000 : AT(0x300000) { KEEP(*(.huge.middle)) } :huge_middle
 ASSERT(SIZEOF(.huge_middle) == 8, "middle qword of the 2 MiB frame")
 .huge_store 0x9fff00 : AT(0x3fff00) { KEEP(*(.huge.store)) } :huge_store
 ASSERT(SIZEOF(.huge_store) == 8, "store target qword")
 .huge_end 0x9ffff8 : AT(0x3ffff8) { KEEP(*(.huge.end)) } :huge_end
 ASSERT(SIZEOF(.huge_end) == 8, "last qword of the 2 MiB frame")
 .huge_sentinel 0xa00000 : AT(0x400000) { KEEP(*(.huge.sentinel)) } :huge_sentinel
 ASSERT(SIZEOF(.huge_sentinel) == 8, "sentinel qword of the next 2 MiB frame")
 /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }
}
