# S2-29af — large-gap direct binary80 addition precision diagnostics

Extended
`add_finite_to_normal_binary80_precision_with_diagnostics(lhs, rhs, mode,
precision)` to accept exponent gaps greater than 64 while retaining its exact
`u128` path through gap 64. Wider gaps use the existing
`finite_large_gap_rounding_bridge`, then round its normalized quotient and
nonzero tail directly at p=24/53/64. Every successful bridged result reports
`inexact = true`; `incremented` records whether rounding raised the truncated
target-precision significand before packing. Both the exact result exponent
and packed result must remain normal finite binary80.

The bridge's `(E, q, tail)` represents an exact normalized coefficient
`q + tau`, with `0 < tau < 1`. The existing quotient rounder combines
discarded bits from `q` with the nonzero tail at the requested precision. For
p<64, a discarded quotient suffix at the midpoint is strictly above halfway
once `tau` is included, independent of retained parity. At p=64 the bridge's
below/half/above classification is consumed directly. The legacy
`add_finite_to_binary80` implementation and the bridge itself are unchanged.

Literal coverage includes the gap-64 exact-alignment regression; gaps 65, 66,
127, 128, and the maximum raw binary80 distance of 32828; both operand orders,
sign combinations, all four modes, and all three precisions; p24/p53 discarded
suffixes immediately below, at, and above halfway with even and odd retained
quotients; gap-65 p64 half/below-half cases for a power-of-two dominant value;
gap-66 above-half; directed negative rounding and overflow; legacy p64 value
comparison; all-successes-inexact diagnostics; rejection of checked synthetic
`i32` exponent bounds; and the existing zero, malformed input, cancellation,
provenance, and input immutability contracts. Synthetic `i32` exponent cases
exercise checked helper bounds and do not represent raw binary80 encodings.

Normative precision-control context is Intel SDM Rev. 093, Vol. 1 §8.1.5.2,
Table 8-2, pp. 8-7–8-8 (precision control applies to FADD/FSUB at p=24/53/64),
and §8.1.5.3 for rounding control
([official PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=213)).
The quotient-tail bridge and its rounding proof are project arithmetic, not
claims from the SDM. This report makes no architectural FADD/FSUB, FCW, PE/C1,
signed-zero, exception, decoder/execution, native/JIT/C ABI, or physical-x86
claim.

Checks: the focused new large-gap test and the updated existing direct-addition
test passed; `cargo test -p od-isa --offline` passed 1771 tests; `cargo fmt
--all -- --check` passed; and `cargo check --workspace --all-targets --offline`
passed. Graphify was refreshed after the source change.
