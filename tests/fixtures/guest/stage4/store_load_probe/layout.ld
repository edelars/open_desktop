OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)
PHDRS
{
    page_tables PT_LOAD FLAGS(6);
    guest_code  PT_LOAD FLAGS(5);
    guest_data  PT_LOAD FLAGS(6);
}
SECTIONS
{
    .page_tables 0x1000 : AT(0x1000) { KEEP(*(.page_tables)) } :page_tables
    ASSERT(SIZEOF(.page_tables) == 0x6000, "page tables must occupy 0x1000..0x6fff")
    .text 0x400000 : AT(0x7000)
    {
        FILL(0x00)
        KEEP(*(.text.entry))
    } :guest_code
    ASSERT(SIZEOF(.text) <= 0x1000, "probe entry must fit in its mapped page")
    ASSERT(stage4_code_end - _start == 8, "store-load probe must occupy exactly eight bytes")
    .data 0x800000 : AT(0x9000) { KEEP(*(.data.payload)) } :guest_data
    ASSERT(SIZEOF(.data) == 16, "probe data must contain initial and store-value qwords")
    /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }
}
