# S2-29w — scale a finite binary80 value by an exact power of two

Added `scale_finite_to_binary80(value, power, mode)` as a bounded numerical
helper. For a normalized finite input with sign `s`, significand `S`, and
exponent `e`, it represents the exact scaled value as
`(-1)^s * S * 2^((e + power) - 63)`. It first checks that the input is nonzero,
has its normalized leading bit set, and carries finite normal, subnormal, or
pseudo-denormal provenance. It then computes `e + power` with checked `i32`
addition and constructs the signed coefficient `±i128::from(S)`. That exact
coefficient and adjusted exponent are passed once to the existing
`round_pack_exact_coefficient`; this helper adds no shift, GRS, or intermediate
rounding arithmetic.

`None` means bounded helper rejection: zero, malformed normalized input,
non-finite provenance, `i32` exponent-add overflow, or a finite result outside
the binary80 range. Exact underflow is rounded once by the existing packer,
which can return signed zero, a subnormal, or minimum normal after carry. A
pseudo-denormal input is interpreted by its normalized finite numeric value and
the returned encoding is canonical. Inputs are immutable.

## Literal coverage

Unit tests use expected byte arrays written independently of the helper and
packer results. They cover:

- Both signs of exact one scaled by `2^0` and `2^±3`, under all four
  mathematical rounding modes.
- Both signs of a significand with low bit set scaled by two, proving the low
  bit is retained.
- Minimum normal scaled by one half; minimum subnormal scaled by `2^63`; maximum
  subnormal scaled by two; and pseudo-denormal canonicalization.
- Minimum subnormal scaled by one half and one quarter, for both signs, all
  rounding modes, and the resulting signed zero/minimum-subnormal choices.
- Raw subnormal significands 3, 5, and 7, normalized and then scaled by one
  half to exact midpoints at `3/2`, `5/2`, and `7/2` subnormal units. Both
  signs and all modes are covered. Nearest-even results are respectively 2,
  2, and 4 units; directed results exchange magnitude choices for negative
  inputs.
- The maximum subnormal predecessor of minimum normal scaled by one half, for
  both signs and all modes, plus the exact midpoint between maximum subnormal
  and minimum normal produced by scaling `(e=-16382, S=u64::MAX)` by one half.
  The latter checks carry into minimum normal for nearest-even and the
  sign-directed away-from-zero mode.
- Maximum finite scaled by one (accepted) and by two (rejected), for both signs
  and all modes.
- Checked addition overflow at `i32::MIN` and `i32::MAX`, distinct from the
  representable exponent sum `0 + i32::MIN`, which rounds deeply underflowed
  values to signed zero or minimum subnormal according to sign and mode.
- Zero, significand below the normalized boundary, and non-finite provenance
  rejection in every mode, plus input immutability.

The literal bytes are asserted directly; expected outputs are not obtained
from `scale_finite_to_binary80` or another system-under-test result.

## Scope and verification

This is a mathematical helper only. It adds no FCW/FSCALE mapping, x87 state or
FSW changes, guest exception or ISA behavior, or physical x86 acceptance.

Verification recorded for this change:

- `cargo test -p od-isa finite_scale --offline`: passed, 6 tests.
- Full `cargo test -p od-isa --offline`: passed, 1,712 tests.
- `cargo fmt --all -- --check`, `git diff --check`, and
  `cargo check --workspace --all-targets --offline`: passed.
- `python3 tools/check_layout.py`: reported the worktree's `.git` pointer as an
  unmapped root file and two links in `STAGE_2_REPORT.md` to gitignored
  stage-oracle manifests absent from this worktree (both manifests exist in the
  main checkout, where the layout check passes). No layout or report
  references were changed for this helper-only slice.
- Independent `gpt-6.1-sol` high review found three coverage/report
  corrections; all were fixed and the follow-up review completed cleanly.
