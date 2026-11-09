OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)

SECTIONS
{
    /* Raw image is linked at its guest physical load address 0x1000;
     * the runner maps it identity, so GVA == GPA. */
    .text 0x1000 :
    {
        KEEP(*(.text.entry))
    }
    ASSERT(SIZEOF(.text) <= 0x1000, "image must stay inside 0x1000..0x1fff")
    ASSERT(_start == 0x1000, "entry must be 0x1000")

    /DISCARD/ :
    {
        *(.comment)
        *(.note*)
        *(.eh_frame*)
        *(.data*)
        *(.bss*)
    }
}
