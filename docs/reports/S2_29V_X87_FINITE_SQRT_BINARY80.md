# S2-29v — pure finite x87 square root helper

## Result

Added `sqrt_finite_to_binary80(value: X87Finite, mode: RationalRoundingMode) -> Option<X87Binary80>` as a value-only helper in `crates/od-isa/src/x87.rs`. It accepts a positive, nonzero normalized finite input and returns a normal finite binary80 encoding rounded to 64 significand bits. Zero, negative, and malformed non-normalized values return `None`.

This slice does not implement FSQRT, consult or update the x87 control word, mutate x87 state, set status flags, deliver exceptions, or claim physical ISA validation.

## Integer contract

For normalized `x = S * 2^(e-63)`, the shared `finite_arithmetic` helper forms:

```text
E = floor(e / 2)
b = e mod 2, using Euclidean division
N = S << (63 + b)
q = floor(sqrt(N))
R = N - q*q
sqrt(x) = sqrt(N) * 2^(E-63)
```

`R` remains `u128`. The floor-root invariant is `q*q <= N` and `R < 2*q + 1`; the implementation never forms `(q+1)^2`, including when `q == u64::MAX`. Since the exact root is `q + δ`, `0 <= δ < 1`, nearest rounding increments exactly when `R > q`: the midpoint comparison is `sqrt(N) > q + 1/2`, equivalent to `R > q + 1/4`; integer `R` makes an exact midpoint impossible. Toward-positive increments iff `R != 0`; toward-zero and toward-negative never increment for this positive input.

The rounded magnitude is held in `u128` so `u64::MAX + 1` is represented as `2^64` and passed to the existing normal packer, which handles the significand carry and exponent increment.

## Literal coverage

The helper has literal ten-byte raw result vectors for all four rounding modes covering these `(S, e, E, q, R)` decompositions:

| `S` | `e` | `E` | `q` | `R` |
|---:|---:|---:|---:|---:|
| `8000000000000000` | 0 | 0 | `8000000000000000` | 0 |
| `9000000000000000` | 3 | 1 | `c000000000000000` | 0 |
| `8000000000000000` | 1 | 0 | `b504f333f9de6484` | `7e8efaacbb989bf0` |
| `8000000000000001` | 0 | 0 | `8000000000000000` | `8000000000000000` |
| `ffffffffffffffff` | 0 | 0 | `b504f333f9de6483` | `16898e114af5564f7` |
| `ffffffffffffffff` | 1 | 0 | `ffffffffffffffff` | `ffffffffffffffff` |

Additional literal ten-byte vectors exercise exponents `-1`, `-3`, `-16445`, and `16383` in all four modes; unit checks cover equivalent pseudo-denormal and minimum-normal representations; positive and negative zero rejection; negative finite rejection; malformed significand rejection; and deterministic samples of the full `u128` integer square-root domain invariant.

The relocation of `integer_sqrt_u128` and the exact `(E, q, R)` decomposition keeps binary32 and binary64 square roots on the same prior rounding bridge: those paths still pass only an exact/nonzero-tail representative to their existing packer. Existing literal raw binary32 and binary64 square-root tests remain in place as regression protection.

## Sources and boundaries

- Intel, *64 and IA-32 Architectures Software Developer’s Manual*, Revision 093, Volume 1 §4.2.2 for floating-point data formats and the extended-precision representation; [Intel SDM landing page](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
- The output rounding modes here are the explicit mathematical modes in `RationalRoundingMode`; they are not read from FCW. The exact radicand, floor root, and remainder derivation above define this helper’s contract. The implementation is independent integer arithmetic and does not assert that an x87 instruction or physical processor was exercised.

## Checks

- `cargo fmt --all -- --check` — passed.
- `cargo test -p od-isa --offline` — passed: 1706 tests, 0 failures.
- `cargo check -p od-isa --all-targets --offline` — passed.
- `graphify update .` — completed.
- Independent review by `gpt-6.1-sol` at high reasoning — passed with no blocking defects or remaining findings. Review covered the exponent decomposition, exact `u128` remainder, rounding modes, `2^64` carry, rejection inputs, and binary32/binary64 regression paths.
