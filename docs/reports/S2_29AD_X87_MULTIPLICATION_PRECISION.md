# S2-29ad — direct binary80 multiplication precision diagnostics

Added
`multiply_finite_to_normal_binary80_precision_with_diagnostics(lhs, rhs,
mode, precision)` for nonzero normalized finite operands at p=24/53/64. Inputs
accept either sign and Normal, Subnormal, or PseudoDenormal provenance when the
normalized significand has bit 63 set. The helper normalizes the complete
`u128` product and rounds directly to the requested significand precision. It
returns the packed normal binary80 value and mathematical `inexact` and
pre-carry `incremented` metadata. The unrounded product exponent and rounded
result must both remain normal finite binary80.

The existing `multiply_finite_to_binary80` implementation and its wider
subnormal-result domain are unchanged. This helper adds no FCW/FSW behavior,
architectural PE/C1 claims, instruction decode/execution, native/JIT/C ABI
route, or physical x86 acceptance claim.

Literal unit coverage includes exact products; both signs and all rounding
modes at every supported precision; p24/p53 even and odd halfway products and
adjacent below/above values; p64 exact midpoint parity; odd low product bits;
carry and exponent limits; accepted input provenance; malformed and zero
inputs; p64 agreement with the legacy helper in their common normal domain;
preserved legacy subnormal results; a direct p24 versus p64-then-p24
double-rounding counterexample; and input immutability.

Normative precision-control context is Intel SDM Rev. 093, Vol. 1 §8.1.5.2,
Table 8-2, pp. 8-7–8-8 (precision control applies to FMUL at p=24/53/64), and
§8.1.5.3 for rounding control ([official PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=213)).
These citations establish helper rounding parameters only; this report makes
no architectural FMUL/PE/C1 claim.

The focused multiplication tests passed 3/3. The full `od-isa` suite passed
1,764/1,764. `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`, `git diff --check`, and
`graphify update .` passed. Independent Sol/high review found no correctness
defects; the reviewer also reran the focused tests successfully (3/3). No
performance or physical x86 acceptance claim is made.
