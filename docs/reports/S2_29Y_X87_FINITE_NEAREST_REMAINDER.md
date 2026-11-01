# S2-29y — exact finite nearest-even remainder helper

## Result

Added `remainder_finite_nearest_even(lhs, rhs) -> Option<(i32, i128)>` to
`crates/od-isa/src/x87.rs`. The returned `(e, c)` is the exact coefficient
`c * 2^(e - 63)` for
`lhs - nearest_integer_ties_even(lhs / rhs) * rhs`.

The helper accepts only normalized, nonzero values whose provenance is
Normal, Subnormal, or PseudoDenormal and whose significand has bit 63 set. It
computes the exponent difference after widening to `i64`. Gaps at most -2
return lhs unchanged. At -1, magnitudes at or below the half-way point return
lhs (including the tie, whose integer quotient zero is even); above half, the
exact coefficient `A - 2B` is returned at the lhs exponent. Gaps 0 through 64
use a `u128` shifted numerator, quotient, and doubled remainder. The quotient
increments exactly when `2R > B` or when `2R == B` and the quotient is odd.
Gap 65 and above, malformed inputs, and zero return `None`. The signed
coefficient is adjusted by the lhs sign; rhs sign does not affect the result.
Exact zero is `(rhs.exponent, 0)` without architectural zero-sign semantics.

The literal test matrix covers:

| Gap | A | B | Result coefficient |
|---:|---:|---:|---:|
| 0 | `C000000000000000` | `8000000000000000` | `-0x4000000000000000` |
| 1 | `A000000000000000` | `8000000000000000` | `+0x4000000000000000` |
| 1 | `E000000000000000` | `8000000000000000` | `-0x4000000000000000` |
| -1 | `8000000000000000` | `8000000000000000` | lhs unchanged |
| -1 | `8000000000000001` | `8000000000000000` | `1 - 0x8000000000000000` |
| -1 | `8000000000000000` | `8000000000000001` | lhs unchanged |
| -2 | `FFFFFFFFFFFFFFFF` | `8000000000000000` | lhs unchanged |
| 63 | `8000000000000000` | `8000000000000001` | `1` |
| 64 | `8000000000000000` | `8000000000000001` | `2` |
| 64 | `FFFFFFFFFFFFFFFF` | `8000000000000001` | `6` |
| 64 | `FFFFFFFFFFFFFFFF` | `FFFFFFFFFFFFFFFF` | `0` |
| 65 | normalized | normalized | `None` |

Additional literal coverage exercises lhs/rhs sign combinations, both sides
of the half-way decision and even/odd quotient ties, exact `+6 % +2`,
`+6 % -2`, `-6 % +2`, and `-6 % -2` zero coefficients, normalized subnormal
inputs (`minimum-normal % maximum-subnormal = minimum-subnormal`, and raw
subnormal `7 % 4` units = `-1` unit), pseudo-denormal equivalence, forged
`i32` exponent extremes, malformed/zero/special values, and input immutability.
Composition through `round_pack_exact_coefficient` checks literal ten-byte
binary80 encodings for exact `-0.5` and negative minimum-subnormal results under
all four mathematical rounding modes.

## Scope and validation

This is an integer helper only. It adds no FPREM1 or other instruction,
partial-reduction/C2 behavior, quotient bits, FCW/FSW behavior, guest
exceptions, ISA admission, or physical x86 acceptance. Performance was not
measured or claimed.

Validation completed:

- `cargo test -p od-isa finite_nearest_even_remainder --offline`: 4 passed.
- `cargo test -p od-isa --offline`: 1724 passed; doc tests passed (0 tests).
- `cargo fmt --all -- --check`: passed.
- `cargo check -p od-isa --all-targets --offline`: passed.
- `cargo check --workspace --all-targets --offline`: passed.
- `graphify update .`: completed; graph rebuilt with 10,875 nodes and 23,239
  edges.
- `python3 tools/check_layout.py`: reported existing repository issues: the
  `.git` entry is treated as an unmapped root file, and two old links in
  `docs/reports/STAGE_2_REPORT.md` point to absent manifests under `work/`.
  Neither report concerns files changed by this task.

Independent Sol/high code review found no defects. The review confirmed the
d=-1 even tie, gap-64 wide quotient/remainder, sign correction, exponent
extremes, validation, and literal composition encodings.
