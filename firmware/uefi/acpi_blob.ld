/*
 * Linker script of the ACPI table blob carrier (S4-31d). Not a guest image: the only payload is
 * the `.acpi_blob` section, which tools/build/build_stage4_acpi_blob.py reads from the ELF.
 * The section is placed at address 0 because it is never loaded as it stands; the blob header
 * carries the guest load address of its payload.
 */
ENTRY(_start)

SECTIONS
{
    . = 0;
    .acpi_blob : ALIGN(16) { KEEP(*(.acpi_blob)) }
    .text : ALIGN(16) { *(.text .text.*) }
    .rodata : ALIGN(16) { *(.rodata .rodata.*) }

    /DISCARD/ : { *(.comment) *(.eh_frame .eh_frame_hdr) *(.note .note.*) *(.gnu.hash) *(.hash) *(.dynsym .dynstr .dynamic) *(.data .data.*) *(.bss .bss.*) }
}
