// Reference input for the S6-41b encoder forms (LDSET/LDCLR and the logical,
// bitfield and CSET forms of the flag sequence). Assembled locally with Apple
// clang for arm64-apple-macosx15.0 -march=armv8.4-a and read back with
// `otool -tV`; expected words are in the encoder tests.
.text
.globl _fx
_fx:
    ldsetb w10, w11, [x12]
    ldseth w10, w11, [x12]
    ldset w10, w11, [x12]
    ldset x10, x11, [x12]
    ldsetalb w10, w11, [x12]
    ldsetalh w10, w11, [x12]
    ldsetal w10, w11, [x12]
    ldsetal x10, x11, [x12]
    ldsetal xzr, x11, [sp]
    ldclrb w10, w11, [x12]
    ldclrh w10, w11, [x12]
    ldclr w10, w11, [x12]
    ldclr x10, x11, [x12]
    ldclralb w10, w11, [x12]
    ldclralh w10, w11, [x12]
    ldclral w10, w11, [x12]
    ldclral x10, x11, [x12]
    orr x9, x10, x11
    orr x9, x10, x11, lsl #7
    orr x9, xzr, x11, lsr #3
    eor x9, x10, x11
    eor x9, x9, x9, lsr #4
    eor x9, x10, xzr, lsl #63
    orn x9, xzr, x11
    orn x9, x10, x11, lsr #1
    lsl x9, x10, #1
    lsl x9, x10, #56
    lsl x9, x10, #63
    lsr x9, x10, #1
    lsr x9, x10, #32
    lsr x9, x10, #63
    ubfx x9, x10, #0, #1
    ubfx x9, x10, #4, #1
    ubfx x9, x10, #7, #1
    ubfx x9, x10, #0, #8
    ubfx x9, x10, #31, #1
    ubfx x9, x10, #63, #1
    cset x9, eq
    cset x9, lo
    cset x15, eq
