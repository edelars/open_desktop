OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)
PHDRS {
 tables PT_LOAD FLAGS(6);
 code_a PT_LOAD FLAGS(5);
 code_b PT_LOAD FLAGS(5);
 data_b PT_LOAD FLAGS(6);
}
/* Virtual addresses of .text.b/.data.b only keep the load segments disjoint. Under
   root B the code runs at GVA 0x400003 (inside the 2 MiB leaf at GPA 0x200000) and the
   data is read at GVA 0x401000 through RBX; only the physical addresses (AT) matter. */
SECTIONS {
 .page_tables 0x1000 : AT(0x1000) { KEEP(*(.page_tables)) } :tables
 ASSERT(SIZEOF(.page_tables) == 0x7000, "seven table pages required")
 .text.a 0x400000 : AT(0xb000) { KEEP(*(.text.a)) } :code_a
 .text.b 0x401000 : AT(0x200000) { KEEP(*(.text.b)) } :code_b
 .data.b 0x402000 : AT(0x201000) { KEEP(*(.data.b)) } :data_b
 ASSERT(SIZEOF(.text.a) == 11 && SIZEOF(.text.b) == 11, "unexpected code")
 ASSERT(SIZEOF(.data.b) == 8, "unexpected data")
 /DISCARD/ : { *(.text) *(.comment) *(.note*) *(.eh_frame*) }
}
