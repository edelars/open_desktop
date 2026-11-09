ENTRY(_start)

PHDRS {
    text PT_LOAD FLAGS(5);   /* R+X: code and read-only data */
    data PT_LOAD FLAGS(6);   /* R+W: .bss */
}

SECTIONS {
    . = 0x400000 + SIZEOF_HEADERS;
    .text   : { *(.text .text.*) } :text
    .rodata : { *(.rodata .rodata.*) } :text
    . = ALIGN(0x1000);
    .bss    : { *(.bss .bss.*) *(COMMON) } :data
    /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }
}
