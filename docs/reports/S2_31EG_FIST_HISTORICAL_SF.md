# S2-31eg — historical SF with prior IE on bounded integer stores

Exact bare Long64 FIST/FISTP forms `DF 10`, `DF 18`, `DB 10`, `DB 18`, and
`DF 38` admit historical SF only when IE is already set. The existing bounded
conversion and write-before-commit behavior is unchanged. SF and the other six
sticky status bits are retained; C1 remains defined by the conversion result.
Existing FLD/FST admission gates are unchanged.

The ISA success matrix compares the complete CPU state and all memory bytes for
266,240 executions: five forms × 13 accepted source classes/results × two signs
× four FCWs × eight TOP values × 32 combinations of the other sticky flags ×
both incoming C1 values. The accepted set is zero, exact normal, inexact normal,
fraction, infinity, quiet NaN, signaling NaN, canonical subnormal, unnormal,
pseudo-infinity, pseudo-quiet-NaN, pseudo-signaling-NaN, and finite
out-of-range integer-indefinite. Pseudo-denormals remain outside the existing
accepted set.

Twenty atomicity cases cross all five forms and both incoming C1 values with a
canonical-span fault and a denied destination write. They compare the complete
CPU snapshot and all memory bytes, proving no premature FISTP pop; the span
fault has no data access, while the denied write records only the attempted
store. Existing no-write rejection covers SF-only, ES, B, SF+ES, and SF+B.
The FNSTSW/FNCLEX/retry sequence and five literal success fixtures remain in
place, along with runner schema validation and interpreter-only backend denial.

## Verification

After the coverage correction, `cargo test -p od-isa --offline --quiet` passed
all 1,945 tests; `cargo check --workspace --all-targets --offline`,
`cargo fmt --all -- --check`, and `git diff --check` passed. `graphify update .`
refreshed the graph to 12,270 nodes, 26,469 edges, and 379 communities. The
initial implementation slice also passed `od-testkit` 455 tests and runner 131
unit + 552 fixture tests; those suites were not rerun because this follow-up
changes only ISA unit coverage and documentation.
