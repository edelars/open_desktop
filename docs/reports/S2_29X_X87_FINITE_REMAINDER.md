# S2-29x — exact truncated finite x87 remainder decomposition

## Result

Added `remainder_finite_trunc(lhs, rhs) -> Option<(i32, i128)>` in
`crates/od-isa/src/x87.rs`. The pair `(exponent, coefficient)` represents
`coefficient * 2^(exponent - 63)`. The helper accepts normalized nonzero
`Normal`, `Subnormal`, and `PseudoDenormal` finite inputs. Invalid provenance,
zero, or a significand without bit 63 set returns `None`.

This is an exact mathematical helper only. It does not implement FPREM or
FPREM1, partial reduction, quotient/status bits, architectural zero sign, FCW or
FSW behavior, exceptions, state mutation, or physical x86 acceptance.

## Integer contract

For normalized inputs `lhs = (-1)^sA * A * 2^(eA-63)` and
`rhs = (-1)^sB * B * 2^(eB-63)`, compute the exponent gap in widened arithmetic:

```text
d = i64(eA) - i64(eB)
```

- If `d < 0`, the normalized significand range proves `|lhs| < |rhs|`.
  Truncation therefore uses quotient zero, and the helper returns the signed
  left significand at `eA`.
- If `0 <= d <= 64`, form `N = u128(A) << d`, `D = u128(B)`, and `R = N % D`.
  Return `(eB, sign(lhs) * i128(R))`. The divisor sign does not affect the
  truncating remainder sign.
- If `d > 64`, return `None` as the bounded-helper refusal.

The widened subtraction cannot overflow for any `i32` exponents. Since `A` is
at most 64 bits and `d` is at most 64, `N` fits in `u128`. Since `0 <= R < B`
and `B <= u64::MAX`, both positive and negative coefficients fit in `i128`.
The exact identity `N = floor(N / D) * D + R` establishes the returned
decomposition without rounded division, multiplication, or subtraction. An
exact zero is returned as `(eB, 0)`; the helper does not assign it an
architectural sign.

## Literal coverage

Unit tests use literal normalized coefficients and expected tuples, independent
of the implementation under test. Coverage includes:

- `5.5 % 2 = 1.5`, all four input sign combinations, and `±6 % ±2 = 0`;
- a left operand smaller than the right operand, including its signed source
  coefficient and exponent;
- exponent gaps 0, 1, 63, 64, and 65;
- the `d=64` equal-maximum-significand exact-zero case;
- minimum normal modulo maximum subnormal, yielding the minimum subnormal;
- raw subnormal `7 % 3`, pseudo-denormal/canonical-normal equivalence, and
  both exponent `i32` endpoints;
- zero, missing normalization bit, invalid source classes, and input
  immutability.
- literal binary80 packing of the exact `1.5` and minimum-subnormal
  remainders in all four mathematical modes.

## Checks

- Focused `cargo test -p od-isa --offline finite_truncated_remainder` — passed
  (3 tests).
- `cargo test -p od-isa --offline` — passed (1717 tests).
- `cargo check -p od-isa --all-targets --offline` — passed.
- `cargo check --workspace --all-targets --offline` — passed.
- `cargo fmt --all -- --check` — passed.
- Independent `gpt-6.1-sol`/high review passed with no remaining findings. It
  checked the signed remainder rules, gap-boundary residues, subnormal and
  pseudo-denormal vectors, exponent endpoint safety, validation, and literal
  binary80 composition bytes.
