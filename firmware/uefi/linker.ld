/*
 * Linker script of the Open Desktop firmware image (own, minimal).
 *
 * Layout contract: docs/specs/FIRMWARE_BOOT.md, section 3.2. The flat image is exactly
 * 0x0040_0000 bytes and is linked at 0x0020_0000 (FIRMWARE_SHADOW). The first 0x40 bytes are the
 * `ODFWIMG1` header: this script only reserves them; tools/build/build_firmware.py fills in the
 * fields (entry offset, CRC-32) after the link.
 */
ENTRY(_start)

IMAGE_BASE = 0x00200000;
IMAGE_SIZE = 0x00400000;

PHDRS
{
    /* One flat segment for the whole image (flags RWX = 7): the machine copies the image as is. */
    image PT_LOAD FLAGS(7);
}

SECTIONS
{
    . = IMAGE_BASE;
    __image_start = .;

    .image_header : ALIGN(16) { KEEP(*(.image_header)) } :image

    .text : ALIGN(16)
    {
        KEEP(*(.text.start))
        *(.text .text.*)
    } :image

    .rodata : ALIGN(16) { *(.rodata .rodata.*) } :image
    .data   : ALIGN(16) { *(.data .data.*) } :image

    /*
     * Zeroed by the startup code. The flat image is zero-padded to IMAGE_SIZE, so .bss also lies
     * inside the image.
     */
    .bss : ALIGN(16)
    {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    } :image

    /*
     * RESERVED (not allocated in S7-01): the embedded firmware volume with the test EFI
     * applications and the relocatable RT image (FIRMWARE_BOOT.md, section 9). Its offset and size
     * are chosen by S7-06; nothing is placed here yet.
     */

    __image_end = .;
    ASSERT(__image_end <= IMAGE_BASE + IMAGE_SIZE, "firmware image does not fit in 0x400000 bytes")
    ASSERT(ADDR(.image_header) == IMAGE_BASE, ".image_header must be at the link base")
    ASSERT(SIZEOF(.image_header) == 0x40, ".image_header must be exactly 0x40 bytes")

    /DISCARD/ : { *(.comment) *(.eh_frame .eh_frame_hdr) *(.note .note.*) *(.gnu.hash) *(.hash) *(.dynsym .dynstr .dynamic) }
}
