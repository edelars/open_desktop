# S2-29t — shared exact aligned addition for binary80 (bounded helper)

Extracted the common finite-significand alignment arithmetic into private
`finite_arithmetic::add_finite_aligned`. The existing public
`binary_float::add_finite_aligned(BinaryFinite, BinaryFinite)` remains an
adapter with the same zero and cancellation behavior. The shared core accepts
normalized nonzero `u64` significands, aligns both exact signed magnitudes at
the lower exponent, and returns the sign, exponent, and full `u128` magnitude.
It accepts exponent gaps through 64; for the maximum inputs the largest
intermediate is exactly
`(2^64 - 1) * 2^64 + (2^64 - 1) == u128::MAX`. Wider gaps are rejected.
Zeros remain neutral, and two zeros or exact cancellation produce the
signless-zero sentinel.

`x87::add_finite_to_binary80` now composes that core with
`round_pack_exact_magnitude` once. It checks `exponent + 64` before packing,
preserves odd alignment tails, and rejects a zero result or finite overflow.
The older same-exponent and unequal-exponent coefficient helpers remain
available with their prior bounds and behavior. No intermediate signed cast,
x87 instruction, control-word or status behavior, or guest exception semantics
were added. The binary32/binary64 public adapter and its existing rounding
diagnostics remain in place.

Coverage includes the gap-62 legacy comparison; exact gap-63 ulp; gap-64
half-ulp ties with even and odd retained significands; an odd low-significand
sticky tail above the gap-64 tie; both operand orders, signs, and all rounding
modes; opposite-sign tails; the exact predecessor of one (`1 - 2^-64`);
gap-65 and checked-exponent rejection; full-`u128` maximum addition; zero and
cancellation; maximum-finite overflow; and input immutability. Existing
binary-float addition regressions are also included in focused checks.

Verification: focused tests passed with 1 x87 aligned-add test, 1 shared-core
test, and 6 existing `finite_addition` regressions. Full `cargo test -p od-isa
--offline` passed (1,694 tests, no failures); `cargo check --workspace
--all-targets --offline`, `cargo fmt --all -- --check`, `git diff --check`, and
`graphify update .` passed. Independent Sol/high review completed cleanly after
correcting the composed API gap documentation and testing exponent overflow
after an admitted gap-63 alignment. This bounded helper does not claim x87
instruction execution or physical x86 acceptance.
