OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)

PHDRS
{
    gdt  PT_LOAD FLAGS(6);
    idt  PT_LOAD FLAGS(6);
    code PT_LOAD FLAGS(5);
}

SECTIONS
{
    .gdt 0x2000 : AT(0x2000)
    {
        KEEP(*(.gdt))
    } :gdt
    ASSERT(SIZEOF(.gdt) == 0x18, "GDT must hold the null, code and data descriptors")

    .idt 0x3000 : AT(0x3000)
    {
        KEEP(*(.idt))
    } :idt
    ASSERT(SIZEOF(.idt) == 0x1000, "IDT must occupy exactly one page")

    .text 0x4000 : AT(0x4000)
    {
        FILL(0x00)
        KEEP(*(.text.entry))
        KEEP(*(.text.handler))
    } :code
    ASSERT(SIZEOF(.text) <= 0x1000, "kernel code must fit in its page")
    ASSERT(_start == 0x4000, "entry must be the first code byte")

    /DISCARD/ :
    {
        *(.comment)
        *(.note*)
        *(.eh_frame*)
    }
}
