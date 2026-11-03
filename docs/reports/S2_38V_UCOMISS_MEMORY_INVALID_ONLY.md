# S2-38v UCOMISS memory invalid-only profile

S2-38v adds the immutable schema-v2 interpreter profile
`s2-long-ucomiss-memory-invalid-rax-v1` for exact Long64 `0F 2E 00`
(`UCOMISS XMM0,m32 [RAX]`). The new control echo includes the independent
OSXMMEXCPT input. S2-37cr's profile and control behavior remain unchanged.

The bounded profile validates the exact three-byte form, one executable code
mapping and one read-only four-byte data mapping. Its executor preserves fetch
and prerequisite ordering, refuses AC before reading, validates the canonical
span, and performs one four-byte read. After that read, a subnormal in either
operand returns atomic Unsupported, including QNaN/SNaN mixtures. Admitted
QNaNs compare unordered without setting IE; SNaNs set IE. Masked IE commits
the status and flags; unmasked IE commits IE and raises `#XM` or arithmetic
`#UD` per OSXMMEXCPT while suppressing flags, RIP and retirement. Prior IE
alone does not fault. JIT and native backends remain denied.

The independent runner oracle covers both NaN classes on both operand sides
across IM, OSXMMEXCPT and all 64 prior sticky-status histories (1,024 cases),
plus QNaN/SNaN, QNaN/subnormal mixtures, finite/infinity/signed-zero relations,
six literal fixtures, MXCSR/profile/control/result/backend tampering and
backend denial. Direct ISA tests cover CPL0–3 and fetch, form, feature,
CR0.EM, CR0.TS, CR4.OSFXSR, AC, canonical-span and read-error ordering with
rollback checks.

The author checks passed: `cargo fmt --all -- --check`, focused
`cargo test -p od-isa --offline ucomiss_invalid_only` (3 tests),
`cargo test -p od-testkit --offline` (455 tests),
`cargo test -p od-runner --offline` (129 unit and 538 fixture tests), and
`cargo check --workspace --all-targets --offline`. The 1,024-case matrix also
passed as a focused runner invocation. Sol/high independent review found and
verified fixes for the backend guard, read-only mapping validation, and
coverage gaps; final review is clean. `graphify update .` reported 12,121
nodes, 26,183 edges and 364 communities. No physical x86 acceptance is
claimed. ISSUE-033 remains an audit of the earlier mixed-event profiles; this
invalid-only slice does not resolve it.

See the [CPU contract](../specs/CPU_CONTRACT.md#s2-38v--exact-ucomiss-memory-invalid-only-profile),
[Stage 2 plan](../plans/STAGE_2.md#s2-38v--exact-ucomiss-memory-invalid-only-profile),
[test-case format](../testing/TEST_CASE_FORMAT.md#s2-38v-ucomiss-memory-invalid-only-profile),
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-38v-ucomiss-memory-invalid-only-profile),
and [ISSUE-033](../KNOWN_ISSUES.md#issue-033-s2-37cq-comiss-mixed-event-handling-audit).
