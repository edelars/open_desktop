# Fixed freestanding image for the mixed 4 KiB / 2 MiB CPU-owned CR3 quantum.
# Root A (GPA 0x1000) maps the code page at GVA 0x400000 with a 4 KiB leaf.
# Root B (GPA 0x5000) maps GVA 0x400000.. with one 2 MiB leaf at GPA 0x200000.
# Program: A: MOV CR3,RAX (RAX = root B) -> B: MOV RAX,[RBX] (loads root A from
# the 2 MiB page) -> B: MOV CR3,RAX -> A. With a budget of 3 the quantum ends
# there; a larger budget reaches the trailing UD2 sentinel at A+9 (precise #UD).
.section .page_tables,"aw",@progbits
.balign 4096
.quad 0x2003
.zero 0xff8
.quad 0x3003
.zero 0xff8
.zero 16
.quad 0x4003
.zero 0xfe8
.quad 0xb001
.zero 0xff8
.quad 0x6003
.zero 0xff8
.quad 0x7003
.zero 0xff8
.zero 16
.quad 0x200083
.zero 0xfe8
.section .text.a,"ax",@progbits
.global _start
.type _start,@function
_start:
 mov %rax, %cr3
 ud2
 .zero 4
 ud2
.size _start, .-_start
.section .text.b,"ax",@progbits
.zero 3
 mov (%rbx), %rax
 mov %rax, %cr3
 ud2
.section .data.b,"aw",@progbits
.quad 0x1000
