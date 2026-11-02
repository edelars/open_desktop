# S2-30co: bounded `FMUL ST(0), ST(i)` precision

The Long64 interpreter now admits exact bare `D8 C8..CF` and rounds the
`ST(0) * ST(i)` product directly at the precision and rounding mode selected
by one of the twelve all-exception-masked x87 control words (PC=24/53/64,
RC=nearest-even/down/up/toward-zero). It reuses the S2-29ad multiplication
precision helper. The other FMUL directions and FMULP retain their existing
exact-only admission.

The local domain requires present x87 state, absent FCS/FDS, `FSW & 0x80df ==
0`, Valid tags, and canonical nonzero Normal binary80 operands of either sign.
For `i=0`, only the ST(0) tag and value are validated and the value is squared.
The helper must return a finite Normal result; out-of-domain exact exponents,
zero, subnormal, overflow, malformed state, and unsupported FCWs are rejected
without mutation.

On success, the rounded bytes replace ST(0). PE becomes old PE OR `inexact`,
and C1 records the helper's magnitude increment. C0/C2/C3 follow the local
preserve policy for undefined fields. TOP, tags, source cells, FCW and
unrelated state are preserved; FIP receives old RIP, RIP advances by two, and
the instruction retires once. The instruction performs no data-memory access.

Focused tests cover every TOP/index pair, all twelve FCWs, sign combinations,
PE/C1 and preserved state, independent exact/inexact/tie/adjacent literals,
negative directed rounding, carry normalization and exponent endpoints, a
double-rounding discriminator, and atomic rejects. Existing exact D8 cases
remain unchanged. The prior FCW 0x027f/0x077f and `(1+2^-63)^2` rejection rows
now exercise successful admission. STOP, fetch, instruction-budget retirement,
unchanged data-memory canaries, and neighboring exact-only FMUL/FMULP forms are
covered.

Validation passed: `cargo test -p od-isa fmul --offline` (10 tests), full
`cargo test -p od-isa --offline` (1,769 tests), `cargo test -p od-testkit -p
od-runner --offline` (all 1,053 tests), `cargo fmt --all -- --check`, and
`cargo check --workspace --all-targets --offline`. No physical x86, native, or
JIT acceptance is claimed.

Semantic references: Intel SDM Rev. 093, Vol. 2A, `FMUL/FIMUL/FMULP`, pp.
3-360–3-362 (Operation and FPU Flags, p. 3-361), [official PDF](https://cdrdv2.public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=478);
Vol. 1 §8.1.5.2/Table 8-2 and §8.1.5.3 for PC/RC, plus §8.5.6, p. 8-30 for
masked precision and C1, [official PDF](https://cdrdv2.public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=236).
