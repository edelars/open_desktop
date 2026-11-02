# S2-29ae — direct binary80 addition precision diagnostics

Added
`add_finite_to_normal_binary80_precision_with_diagnostics(lhs, rhs, mode,
precision)` for nonzero normalized finite operands at p=24/53/64. Inputs accept
either sign and Normal, Subnormal, or PseudoDenormal provenance when the
normalized significand has bit 63 set. Exact alignment is admitted through
exponent gap 64, then the exact nonzero magnitude is normalized and rounded
once directly to the requested precision. The result contains the packed
normal binary80 value and mathematical `inexact` and pre-carry `incremented`
metadata. The exact normalized exponent and rounded output must both remain
normal finite binary80.

Zero operands, gaps above 64, invalid precision/provenance/normalization,
checked exponent failures, exact cancellation, subnormal or zero exact results,
and rounded overflow return `None`. The existing `add_finite_to_binary80` is
unchanged and retains its zero-neutral, subnormal-result, and larger-gap
behavior. No large-gap bridge is used to enlarge this helper's domain.

Literal coverage includes gaps 0/1/63/64 in both operand orders, gap 65
rejection, the gap-64 `u128::MAX` same-sign sum, same-sign carry, opposite-sign
near and exact cancellation, all three precisions and four rounding modes,
even/odd halfway values and adjacent cases, directed rounding for both signs,
sticky-tail and direct-versus-double-rounding cases, rounding carry beyond the
maximum exponent, minimum-normal formation from maximum plus minimum
subnormal, a normal-input subtraction whose exact result is subnormal, all
accepted raw provenance classes, malformed and extreme inputs, input
immutability, and old-helper-only zero/gap successes.

Normative precision-control context is Intel SDM Rev. 093, Vol. 1 §8.1.5.2,
Table 8-2, pp. 8-7–8-8 (precision control applies to FADD/FSUB at p=24/53/64),
and §8.1.5.3 for rounding control ([official PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=213)).
These sources establish the helper's supported rounding parameters only.
This report makes no architectural FADD/FSUB, FCW, PE/C1, signed-zero,
exception, decoder/execution, native/JIT/C ABI, or physical-x86 claim.
