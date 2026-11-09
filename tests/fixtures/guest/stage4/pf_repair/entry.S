# Stage4PfRepairProbeV1: CR0.WP write to a supervisor RO page -> #PF -> handler repairs the
# PTE through an RW alias of the page table -> INVLPG -> LEA RSP,[RSP+8] -> IRETQ -> retry.
# The CPU handoff (CS=0x08/SS=0x10, CR0/CR3/CR4/EFER, GDTR/IDTR, RBX/RCX/RDI/RSI/RSP) is
# supplied by the loader; this image owns page tables, code, IDT/GDT, stack pages and data.
#ifndef PROBE_INITIAL
#define PROBE_INITIAL 0x1122334455667788
#endif
#ifndef PROBE_CANARY
#define PROBE_CANARY 0x6a3591c7d248b0ef
#endif
#ifndef PROBE_STORE_VALUE
#define PROBE_STORE_VALUE 0x8877665544332211
#endif
#define HANDLER_RIP 0x402000
.section .page_tables,"aw",@progbits
.balign 4096
stage4_pml4: .quad 0x2027; .zero 0xff8
stage4_pdpt: .quad 0x3027; .zero 0xff8
stage4_pd:
    .zero 16
    .quad 0x4027            # GVA 0x400000: code table
    .quad 0x5027            # GVA 0x600000: IDT/GDT/stack table
    .quad 0x6007            # GVA 0x800000: data table (A clear)
    .zero 0xfd8
stage4_code_pt:
    .zero 8
    .quad 0x7027            # GVA 0x401000: main code
    .quad 0x8027            # GVA 0x402000: #PF handler
    .quad 0x6027            # GVA 0x403000: RW alias of the data page table
    .zero 0xfe0
stage4_sys_pt:
    .quad 0x9027            # GVA 0x600000: IDT
    .quad 0xa027            # GVA 0x601000: GDT
    .zero 0x7f8
    .quad 0xb027            # GVA 0x701000: stack, lower page
    .quad 0xc027            # GVA 0x702000: stack
    .zero 0x7e8
stage4_data_pt:
    .quad 0xd001            # GVA 0x800000: supervisor read-only, A clear
    .zero 0xff8
.section .text.main,"ax",@progbits
.global _start
.type _start,@function
_start:
    movq %rcx, (%rbx)        # 48 89 0b, faults on the RO page (CR0.WP=1)
    movq (%rbx), %rax        # 48 8b 03, loads the stored value
.size _start, .-_start
.section .text.handler,"ax",@progbits
.global stage4_pf_handler
.type stage4_pf_handler,@function
stage4_pf_handler:
    movq %rsi, (%rdi)        # 48 89 37, repair the PTE through the RW alias
    invlpg (%rbx)            # 0f 01 3b
    leaq 8(%rsp), %rsp       # 48 8d 64 24 08, discard the #PF error code
    iretq                    # 48 cf
.size stage4_pf_handler, .-stage4_pf_handler
.section .idt,"aw",@progbits
    .zero 14*16
    .word HANDLER_RIP & 0xffff       # vector 14: interrupt gate, CS 0x08, IST 0
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
.section .data.payload,"aw",@progbits
stage4_initial: .quad PROBE_INITIAL
stage4_canary: .quad PROBE_CANARY
stage4_store: .quad PROBE_STORE_VALUE
