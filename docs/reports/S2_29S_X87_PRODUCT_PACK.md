# S2-29s — exact finite binary80 product with one-round packing (bounded helper)

Added `round_pack_exact_magnitude(sign, exponent, magnitude, mode)` for the
exact value `(-1)^sign * magnitude * 2^(exponent - 127)`. The helper rejects
zero, normalizes the unsigned `u128` magnitude without losing bits, and rounds
once into a normal or subnormal binary80 result. Checked exponent failures and
finite-range overflow return `None`; underflow to zero preserves the sign. No
architectural zero selection, status flags, exceptions, control-word behavior,
or x87 instruction semantics are included.

The normalizer is exact. For rounding, the normalized value is decomposed into
`q = high64(magnitude)` and `low = low64(magnitude)`. The tail is represented
as `r = (low >> 1) | (low & 1)` over denominator `2^63`. This compression
preserves whether the tail is zero and whether it is below, equal to, or above
half; the low bit remains sticky for the packers' exact-half decisions at
reduced precision and in the subnormal range. This matters because a full
significand product may be odd in its low bit. The coefficient rounder is not
used as a narrowing intermediate.

Added `multiply_finite_to_binary80(lhs, rhs, mode)`, which composes the existing
full-`u128` `multiply_finite_significands` product with the new packer. Zero
operands remain bounded `None` results, inputs and their source-class metadata
are unchanged, and one final rounding is performed.

Focused coverage checks exact-even equivalence with the existing coefficient
rounder across three significands, two signs, five exponents and all four
rounding modes; odd low words below, at, and above normal ties; all rounding
modes and signs; normal carry; the minimum-normal/maximum-subnormal boundary;
minimum and half-minimum subnormal; signed underflow zero; finite-range and
checked-exponent rejection; odd low-bit full products; zero products; product
overflow; and input immutability.

Verification: `cargo check -p od-isa --offline` passed;
`cargo test -p od-isa exact_magnitude --offline` passed (3 tests);
`cargo test -p od-isa finite_product_rounds_full_u128_once --offline` passed
(1 test); full `cargo test -p od-isa --offline` passed (1,689 tests, no
failures); `cargo check --workspace --all-targets --offline` passed;
`cargo fmt --all -- --check` and `git diff --check` passed. `graphify update .`
completed after the code change. Independent Sol/high review completed with no
findings. No x87 instruction execution or physical x86 acceptance is claimed.
