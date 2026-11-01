# S2-29r — exact unequal-exponent finite binary80 addition (bounded helper)

Added `add_finite_unequal_exponents(lhs, rhs)` for two normalized, nonzero
`X87Finite` values whose exponents differ by 1 through 62. The helper orders
the operands by exponent and returns an exact signed `i128` coefficient at the
lower exponent:

```text
(signed_significand(higher) << exponent_gap) + signed_significand(lower)
```

For the admitted gap, the shifted signed significand plus the lower
significand fits in `i128`; the largest accepted gap is exercised directly.
Equal exponents, zero operands, and gaps greater than 62 are rejected by this
helper. It does not jam, round, or mutate its inputs.

`add_finite_to_binary80(lhs, rhs, mode)` composes the existing same-exponent
path and the new unequal-exponent path with one call to
`round_pack_exact_coefficient`. One zero is neutral in either position. Two
zeros and exact cancellation return `None` because the helpers do not select a
signed zero. A result outside finite binary80 range also returns `None`; none
of these outcomes represents an architectural exception. The existing
same-exponent APIs are unchanged.

Focused unit coverage checks operand order and all sign combinations at gap
one, exact coefficient construction at gap 62, rejection at gap 63, rejection
of equal-exponent and zero operands by the unequal-only helper, normal packed
results, both zero operand positions, two-zero rejection, same-exponent
cancellation, directed rounding for a low-order inexact tail with positive
and negative results, finite overflow rejection, and input immutability.

Verification: `cargo test -p od-isa unequal_exponent --offline` passed (3
tests), `cargo test -p od-isa --offline` passed (1,682 tests),
`cargo check --workspace --all-targets --offline`, `cargo fmt --all -- --check`,
`git diff --check`, and `graphify update .` passed. `python3
tools/check_layout.py` could not validate this isolated Git worktree: it treats
the worktree's `.git` indirection file as unmapped and reports two existing
links in `STAGE_2_REPORT.md` whose local oracle manifests are absent here. No
layout files were changed. Independent review and final integration with the
shared Stage 2 documents remain pending. No x87 instruction decode/execution,
control-word/status behavior, guest exceptions, or physical x86 acceptance is
included. ISSUE-019 remains open.
