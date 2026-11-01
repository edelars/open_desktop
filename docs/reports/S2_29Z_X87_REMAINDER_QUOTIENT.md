# S2-29z — exact quotient metadata for bounded finite binary80 remainders

## Result

Added `X87FiniteRemainder` and the metadata-returning helpers
`remainder_finite_trunc_with_quotient` and
`remainder_finite_nearest_even_with_quotient`. The result carries the existing
remainder exponent/coefficient and an unsigned `u128` quotient magnitude with a
separate sign. A quotient of zero is always positive. Truncation returns the
exact truncated integer quotient; nearest-even returns the rounded integer
quotient under ties-to-even. Existing `remainder_finite_trunc` and
`remainder_finite_nearest_even` remain projections of the new helpers, so
accepted inputs, remainder values, and rejection behavior are shared.

Both paths accept only nonzero normalized significands (bit 63 set) with
Normal, Subnormal, or PseudoDenormal provenance. Exponent subtraction is
widened before evaluation. Truncation accepts every negative gap with quotient
zero and handles gaps 0 through 64 exactly. Nearest-even accepts every
negative gap: `d <= -2` returns lhs unchanged with quotient zero, while at
`d == -1`, `A <= B` returns quotient zero (including the exact half tie) and
`A > B` returns quotient one. Gaps 0 through 64 use exact division. Larger
positive gaps, zero operands, malformed normalization, and other provenance
remain rejected.

## Independent literal coverage

The new integer-only tests use fixed expected values rather than a second call
to production arithmetic. They cover:

- Quotient sign independently from the left-signed remainder, including all
  sign effects on the quotient and positive canonical zero for a negative
  numerator whose quotient rounds/truncates to zero.
- Quotient magnitudes wider than `u64`: at gap 64, `(u64::MAX << 64) / 2^63`
  is exactly `2^65 - 2` for both the truncating and nearest-even profiles.
- Nearest-even ties from odd 1 to even 2 and even 2 to even 2; the latter uses
  a gap-one ratio of 2.5. A gap-minus-one tie at 0.5 rounds to quotient zero.
- Exact quotient/remainder multiples, signed operands, and the distinct
  remainder exponent choices for negative and nonnegative gaps.
- Wrapper projection equivalence for a cross-product of valid, malformed,
  zero, nonfinite, and `i32` endpoint exponent inputs, plus explicit widest-gap
  rejection checks.

## Scope and limits

This is a mathematical helper only. No FPREM/FPREM1 or other instruction
semantics, quotient condition flags, x87 state/FCW/FSW behavior, guest
exceptions, decoder/execution changes, native/JIT/C ABI admission, physical x86
acceptance, architectural signed-zero behavior, or performance result is
claimed. ISSUE-019 remains open for architectural x87 behavior and physical
acceptance.

## Validation

`cargo test -p od-isa --offline remainder -- --nocapture` passed all 13 focused
remainder-related tests; `cargo test -p od-isa --offline` passed all 1,741
unit tests and doc tests. `cargo fmt --all -- --check`,
`cargo check -p od-isa --all-targets --offline`, and
`cargo check --workspace --all-targets --offline` passed. `graphify update .`
completed after the code change. Review follow-up corrected the nearest-even
negative-gap contract and its helper documentation to match the implemented
`d <= -2` passthrough plus the separate `d == -1` decision. A literal metadata
assertion checks the far-negative `i32::MIN` to `i32::MAX` endpoint: lhs is
preserved, quotient magnitude is zero, and quotient sign is positive. The
follow-up metadata tests passed 3/3 and the full `od-isa` suite passed 1,741;
formatting, component/workspace checks, and `git diff --check` passed again.
`python3 tools/check_layout.py` remains blocked
by the existing isolated-worktree `.git` pointer and two ignored historical
oracle manifest links documented under ISSUE-023; no layout change was made.
The initial independent review's P2 documentation finding has been corrected;
follow-up review is requested separately.

See the [Stage 2 plan](../plans/STAGE_2.md#s2-29z-exact-quotient-metadata-for-bounded-finite-binary80-remainders),
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-29z-exact-quotient-metadata-for-bounded-finite-remainders),
[shared Stage 2 report](STAGE_2_REPORT.md#s2-29z--exact-quotient-metadata-for-bounded-finite-binary80-remainders),
and [ISSUE-019](../KNOWN_ISSUES.md#issue-019--семантика-арифметики-и-инструкций-x87-остаётся-открытой).
