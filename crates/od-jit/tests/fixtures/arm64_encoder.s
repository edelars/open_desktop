// Reference input for the checked encoder. Assembled locally with Apple
// clang 21.0.0 for arm64-apple-macosx; expected words are in encoder tests.
.text
.globl _encoder_fixture
_encoder_fixture:
    movz x10, #0x1234
    movk x10, #0xabcd, lsl #16
    mov x19, x0
    mov x14, xzr
    add x11, x9, x10
    sub x12, x10, x9
    and x13, x9, x10
    subs xzr, x9, x10
    adds xzr, sp, #4095
    adds x14, x14, #1
    subs xzr, x9, #1
    ldr x9, [x19, #32]
    str x9, [x21]
    ldarb w9, [x22]
    stp x19, x20, [sp, #-16]!
    ldp x22, x30, [sp], #16
    b .Ltarget
    bl .Ltarget
    b.eq .Ltarget
    b.lo .Ltarget
    cbz x9, .Ltarget
    cbz w9, .Ltarget
    cbnz x9, .Ltarget
    cbnz w9, .Ltarget
    blr x16
    ret x30
.Ltarget:
    ret
