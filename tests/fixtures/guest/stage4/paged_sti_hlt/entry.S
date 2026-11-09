# Stage4PagedStiHltProbeV1 (S4-27i4a): the Stage4PagedIrqProbeV1 layout (S4-27h) with
# `STI; HLT; HLT` as main code: STI -> HLT (CPU halts) -> wake by APIC vector 0x41 -> IDT gate ->
# handler (EOI store, IRETQ) -> the second HLT. The CPU handoff (CS=0x08/SS=0x10,
# CR0/CR3/CR4/EFER, GDTR/IDTR, RAX=0, RBX=0x6020b0, RSP, RFLAGS) and the Local APIC software
# enable are supplied by the loader (RFLAGS.IF=0 in the wake scenario, IF=1 in the boundary one);
# this image owns page tables, code, IDT/GDT and the stack page. All leaf PTEs start with A and
# D clear.
#define HANDLER_RIP 0x402000
#define VECTOR 0x41
.section .page_tables,"aw",@progbits
.balign 4096
stage4_pml4: .quad 0x2023; .zero 0xff8
stage4_pdpt: .quad 0x3023; .zero 0xff8
stage4_pd:
    .zero 16
    .quad 0x4023            # GVA 0x400000: code table
    .quad 0x5023            # GVA 0x600000: IDT/GDT/APIC/stack table
    .zero 0xfe0
stage4_code_pt:
    .zero 8
    .quad 0x6003            # GVA 0x401000: main code
    .quad 0x7003            # GVA 0x402000: interrupt handler
    .zero 0xfe8
stage4_sys_pt:
    .quad 0x8003            # GVA 0x600000: IDT
    .quad 0x9003            # GVA 0x601000: GDT
    .quad 0xfee00003        # GVA 0x602000: Local APIC page (GPA 0xfee00000)
    .zero 0x7e8
    .quad 0xa003            # GVA 0x700000: stack page
    .zero 0x7f8
.section .text.main,"ax",@progbits
.global _start
.type _start,@function
_start:
    sti                      # fb: IF=1, interrupts inhibited until after the next instruction
    hlt                      # f4: retires inside the shadow, the CPU halts until the vector
    hlt                      # f4: runs after the handler returned (RIP after the first HLT)
.size _start, .-_start
.section .text.handler,"ax",@progbits
.global stage4_irq_handler
.type stage4_irq_handler,@function
stage4_irq_handler:
    movl %eax, (%rbx)        # 89 03, EOI: dword 0 to the Local APIC register at 0xfee000b0
    iretq                    # 48 cf
.size stage4_irq_handler, .-stage4_irq_handler
.section .idt,"aw",@progbits
    .zero VECTOR*16
    .word HANDLER_RIP & 0xffff       # vector 0x41: interrupt gate, CS 0x08, IST 0
    .word 0x08
    .byte 0
    .byte 0x8e
    .word (HANDLER_RIP >> 16) & 0xffff
    .long HANDLER_RIP >> 32
    .long 0
.section .gdt,"aw",@progbits
    .quad 0
    .quad 0x00af9b000000ffff         # 0x08: 64-bit CPL0 code, accessed
    .quad 0x00cf93000000ffff         # 0x10: CPL0 data, accessed
