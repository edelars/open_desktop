# S2-29u — p64-aware large-gap binary80 addition bridge

Extended the composed `x87::add_finite_to_binary80` path for normalized
nonzero operands with exponent gaps greater than 64. Gaps through 64 still use
the exact shared `u128` alignment and one-round magnitude packer from S2-29t.
Larger gaps use a private bridge in `finite_arithmetic.rs` that returns the
normalized 64-bit quotient and classifies the nonzero tail as below, exactly
at, or above half. Binary32/binary64 retain their existing behavior and
diagnostics through the `BinaryFinite` adapter; their precision is at most 53
bits, so all nonzero tail representatives preserve their prior decisions.

The bridge selects compact representatives 1/4, 1/2, and 3/4 for the three
tail classes. Same-sign addition has a below-half tail. Opposite signs with a
dominant significand above `2^63` have an above-half tail after subtracting one
retained unit. At a dominant significand of `2^63`, a gap-65 subtraction is an
exact tie only when the smaller significand is exactly `2^63`; a larger smaller
significand moves below half. Gaps 66 and above move above half. This classifier
is specific to p=64; the existing binary32/binary64 adapter continues to use
the representative as a nonzero tail without changing its diagnostics.

Focused tests cover the literal results for `1 - 2^-65`,
`1 - (2^-65 + 2^-128)`, `1 - 2^-66`, both signs and operand orders across all
rounding modes, dominant significands above `2^63`, gaps 65/66/127/128/32828,
the old gap-65 coefficient-helper refusal, max-finite with signed minimum
subnormals under directed/nearest rounding, gap-64 exact-path compatibility,
zero/cancellation, and immutability. Existing binary32/binary64 large-gap
diagnostic regressions remain covered.

Focused verification: `cargo test -p od-isa
large_gap_bridge_classifies_binary80_half_tails --offline` (1 passed);
`cargo test -p od-isa
large_gap_addition_rounds_binary80_tail_categories_in_all_modes --offline` (1
passed); `cargo test -p od-isa finite_aligned_result_large_gap --offline` (2
passed); `cargo test -p od-isa
aligned_addition_packs_exact_gaps_ties_boundaries_and_overflow --offline` (1
passed); and `cargo test -p od-isa finite_addition --offline` (6 passed). Full
`cargo test -p od-isa --offline` passed (1,699 tests, no failures); `cargo
check --workspace --all-targets --offline`, `cargo fmt --all -- --check`, `git
diff --check`, and `graphify update .` passed. Independent Sol/high review
completed cleanly. No x87 instruction execution or physical x86 acceptance is
claimed.
