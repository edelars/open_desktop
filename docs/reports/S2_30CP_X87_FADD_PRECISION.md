# S2-30cp — bounded `FADD ST(0), ST(i)` precision

The Long64 interpreter now admits exactly bare `D8 C0..C7` and rounds
`ST(0) + ST(i)` directly at the precision and rounding mode selected by one of
the twelve all-exception-masked x87 control words (PC=24/53/64,
RC=nearest-even/down/up/toward-zero). It uses the bounded S2-29ae finite
addition helper with its inexact and pre-carry increment diagnostics.

Admission requires present x87 state, absent FCS/FDS selectors,
`FSW & 0x80df == 0`, Valid tags, and canonical nonzero Normal binary80
participants of either sign. Normalized operand exponents must differ by at
most 64 before the arithmetic helper is called; the instruction gate remains
in force independently of the helper's current domain. For `i=0`, only the
physical TOP tag and payload are checked and the operand is doubled.

Only a finite Normal exact exponent and canonical Normal rounded result are
accepted. Zero/cancellation, subnormal, overflow, unsupported exponent gaps,
special or malformed operands, invalid environment, and unsupported FCWs
reject before state mutation. No guest exception delivery is modeled.

On success, the rounded bytes replace physical TOP. PE becomes old PE OR
`inexact`; C1 is replaced from the helper's `incremented` diagnostic. C0/C2/C3
are preserved under the local deterministic policy. TOP, tags, other payloads,
FCW/FDP/FOP, and unrelated CPU state are preserved. FIP receives old RIP, RIP
advances by two, and the instruction retires once. The register-only form
performs no data-memory access.

The existing exact `D8 C0..C7` precision-64 behavior remains unchanged. The
`DC` reverse-direction and `DE` pop forms retain their previous exact-only
paths; FSUB and FSUBR are unchanged. Prefix, missing-state, fetch, STOP,
zero-budget, and malformed-environment paths remain atomic. JIT and native
admission remain closed; no physical x86 acceptance is claimed.

Coverage includes all TOP/index pairs and signs across all twelve FCWs,
single-register aliasing with unrelated tags empty, sticky PE and C1
replacement, precision inexactness, exponent gaps 1/63/64 and atomic gap-65+
rejection, p53 midpoint neighbors, negative directed rounding, carry and
minimum-exponent operands, cancellation/subnormal/overflow rejection, fetch
and retirement boundaries, and no-data-access canaries. The old schema-v2
runner assertion that rejected an inexact p64 sum is now a successful literal
rounding row with sticky PE.

Validation passed:

- Focused `cargo test -p od-isa fadd --offline`: 15 tests.
- Full `cargo test -p od-isa --offline`: 1,775 tests.
- `cargo test -p od-testkit -p od-runner --offline`: 454 testkit unit tests,
  112 runner unit tests, and 487 runner fixture tests.
- `cargo fmt --all -- --check`, `cargo check --workspace --all-targets
  --offline`, and `git diff --check`: passed.
- `graphify update .`: passed; refreshed graph has 11,087 nodes, 23,925
  edges, and 322 communities.
- Independent Sol/high review: clean; no concrete defects found. The reviewer
  independently passed the 15 focused FADD tests, migrated runner fixture,
  formatting, and diff checks. Physical x86/native/JIT acceptance remains
  unverified.

Semantic references: Intel SDM Rev. 093, Vol. 2A, `FADD/FADDP/FIADD`
pp. 3-311–3-313 (Operation/FPU Flags p. 3-312),
[official Vol. 2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=429);
Vol. 1 §8.1.5.2/Table 8-2, §8.1.5.3 and §8.5.6 p. 8-30,
[official Vol. 1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=236).
