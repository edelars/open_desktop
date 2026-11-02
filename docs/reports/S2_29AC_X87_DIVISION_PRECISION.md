# S2-29ac — direct binary80 division precision helper

`od_isa::x87::divide_finite_to_normal_binary80_precision_with_diagnostics`
divides two nonzero normalized finite binary80 operands and rounds the exact
quotient directly to 24, 53, or 64 significand bits. It returns the packed
normal binary80 value plus helper-local `inexact` and `incremented` booleans.
Both signs are accepted; input provenance may be Normal, Subnormal, or
PseudoDenormal when the normalized significand has bit 63 set.

The helper reuses exact finite-significand division, quotient-to-precision
rounding, and normal-significand packing. It rejects unsupported precision,
zero or malformed inputs, an unrounded quotient exponent outside `-16382..=
16383`, and any rounded result that is not normal finite binary80. It does not
round through p64 before narrower precision. `inexact` includes discarded
quotient bits and a nonzero division remainder; `incremented` records an
increase of the target-precision truncated magnitude before carry
normalization. The existing `divide_finite_to_binary80` implementation and its
subnormal results are unchanged.

Literal tests cover exact quotients; 1/3 for p24/p53/p64, both signs, and all
rounding modes; even/odd ties and adjacent below/above-midpoint values;
discarded quotient bits with zero remainder; nonzero remainder; carry,
minimum/maximum normal exponent, overflow carry, and rejection at unrounded
exponent -16383. A literal input pair distinguishes direct p24 rounding from
p64-then-p24 double rounding. Invalid precision, zero, malformed
significands/provenance, accepted subnormal/pseudo-denormal provenance,
immutability, and p64 agreement with the legacy helper in the common normal
domain are also covered.

Focused tests passed 3/3. The full `od-isa` suite passed 1,758/1,758; workspace
format check and offline all-target check passed. The helper is arithmetic
infrastructure only: it adds no FCW/FSW semantics, PE/C1 effects, instruction
decode/execution, native/JIT/C ABI route, physical x86 claim, or performance
claim. Independent Sol/high review found no concrete defects; the reviewer also
reran the focused tests successfully (3/3).
