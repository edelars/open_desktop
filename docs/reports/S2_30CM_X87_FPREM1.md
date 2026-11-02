# S2-30cm — bounded x87 FPREM1

Implemented the exact bare Long64 `D9 F5` FPREM1 form in the `od-isa`
interpreter. The decoder rejects prefixes, WAIT/FWAIT, truncation, and
neighboring unsupported forms. JIT and native admission remain closed.

Admission requires present x87 state, absent FCS/FDS selectors, one of the
twelve all-exception-masked PC24/53/64 FCWs, `FSW & 0x80df == 0`, and Valid-tagged
canonical positive nonzero Normal values at physical TOP and TOP+1. The
preflight checks `D=e0-e1 < 64` and divisor exponent `e1 >= -16318` before
calling `remainder_finite_nearest_even_with_quotient`. This excludes partial
reduction and subnormal results; the stricter divisor bound covers the exact
`D=-1` remainder `2^(e1-64)`.

The exact signed remainder is packed into old physical TOP, with exact zero
encoded as canonical `+0` and tagged Zero. The quotient's low three bits replace
C0/C3/C1 and C2 is cleared. Sticky exception/PE bits, TOP, ST(1), all other
registers and tags, FCW, FDP, FOP, and unrelated CPU state are preserved. FIP
receives the old RIP, RIP advances by two, and the instruction retires once.
Successful memory traces contain instruction fetches only.

Coverage includes all 3,072 combinations of FCW, TOP, prior PE, and old C0–C3;
literal 5/2, 7/2, 5.5/2, exact-zero, low quotient bits 0–7 and q=8 wrap; D=-2,
-1, 0, 63, 64, and 65; the `e1=-16318` minimum-normal witness and lower-bound
denial; environment and malformed-operand denials; canaries; fetch truncation;
STOP, stop-RIP, and zero/one-retirement budget boundaries. FPREM1 remains
explicitly denied by the fragment JIT and the Linux native admission list.

## Validation

- Focused FPREM1 tests: 3/3 passed.
- Full `cargo test -p od-isa --offline`: 1,758/1,758 passed.
- Full `cargo test -p od-jit --offline`: 9/9 passed, including JIT denial.
- Independent Sol/high review was clean; reviewer focused ISA/JIT run passed 4/4.
- `cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
  and `git diff --check` passed.
- `graphify update .` completed: 11,076 nodes, 23,713 edges, 334 communities.
- Native regression source includes D9 F5 in its denial list. The regression
  test is Linux-only and therefore had no matching tests on this macOS host.

This bounded interpreter profile does not model general x87 exception
delivery/availability, JIT/native execution, schema or CLI fixtures, or
physical-x86 acceptance. ISSUE-019 remains open.
