OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)
PHDRS { page_tables PT_LOAD FLAGS(6); main_code PT_LOAD FLAGS(5); handler_code PT_LOAD FLAGS(5); idt PT_LOAD FLAGS(6); gdt PT_LOAD FLAGS(6); guest_data PT_LOAD FLAGS(6); }
SECTIONS {
 .page_tables 0x1000 : AT(0x1000) { KEEP(*(.page_tables)) } :page_tables
 ASSERT(SIZEOF(.page_tables) == 0x6000, "page tables must occupy 0x1000..0x6fff")
 .text_main 0x401000 : AT(0x7000) { FILL(0x00) KEEP(*(.text.main)) } :main_code
 ASSERT(SIZEOF(.text_main) == 6, "main code is exactly MOV [RBX],RCX; MOV RAX,[RBX]")
 .text_handler 0x402000 : AT(0x8000) { FILL(0x00) KEEP(*(.text.handler)) } :handler_code
 ASSERT(SIZEOF(.text_handler) == 13, "handler is exactly MOV [RDI],RSI; INVLPG [RBX]; LEA RSP,[RSP+8]; IRETQ")
 ASSERT(ADDR(.text_handler) == 0x402000, "the IDT gate targets 0x402000")
 .idt 0x600000 : AT(0x9000) { KEEP(*(.idt)) } :idt
 ASSERT(SIZEOF(.idt) == 0xf0, "IDT holds vectors 0..14")
 .gdt 0x601000 : AT(0xa000) { KEEP(*(.gdt)) } :gdt
 ASSERT(SIZEOF(.gdt) == 0x18, "GDT holds null, CS and SS")
 .data 0x800000 : AT(0xd000) { KEEP(*(.data.payload)) } :guest_data
 ASSERT(SIZEOF(.data) == 24, "image must contain initial, canary and store qwords")
 /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }
}
