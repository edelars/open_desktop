# S2-30ct — bounded reverse x87 FADD precision

The Long64 interpreter admits exact bare `DC C0..C7` (`FADD ST(i), ST(0)`).
It snapshots both operands and uses the direct binary80 addition helper at the
precision and rounding mode selected by one of twelve all-exception-masked
PC24/53/64 FCWs. The local profile requires present x87 state, absent FCS/FDS,
`FSW & 0x80df == 0`, Valid tags and canonical nonzero Normal operands of either
sign. For `i=0`, only physical TOP is checked and doubled. The instruction
enforces normalized exponent gap≤64 even though the helper also supports
larger gaps. Only finite Normal exact and rounded results are accepted.

Success writes the rounded result only to old physical `(TOP+i)&7`, sets PE to
old PE OR `inexact`, replaces C1 from the helper's pre-carry magnitude
`incremented` diagnostic, and preserves other condition codes under the local
policy. It preserves TOP, tags, source payloads, FCW/FDP/FOP and unrelated
state; FIP receives old RIP, RIP advances by two and the instruction retires
once. The register form makes no data access. Unsupported environment,
operands, exponent gaps, zero/subnormal/overflow results, fetch/STOP/budget
boundaries and other denials are atomic. The `D8` precision path, `DE` pop
forms and other DC operations are unchanged. No general x87 exception
delivery, JIT/native admission or physical x86 acceptance is claimed.

Coverage includes all eight TOP values, all eight indices, all twelve FCWs,
four sign combinations and the self-alias; exact/inexact sums, midpoint
neighbors, directed rounding, carry, exponent gaps 1/63/64/65+, cancellation,
underflow/overflow, tags, status/environment gates, source preservation and
atomic refusal. Existing D8 FADD and DE FADDP coverage exercises adjacent
forms.

Validation passed:

- Focused `cargo test -p od-isa fadd --offline`: 19 tests.
- Full `cargo test -p od-isa --offline`: 1,800 tests.
- `cargo test -p od-testkit -p od-runner --offline`: 455 testkit tests,
  113 runner unit tests and 491 runner fixture tests.
- `cargo fmt --all -- --check` and
  `cargo check --workspace --all-targets --offline`.
- `graphify update .`: 11,271 nodes, 24,218 edges and 348 communities.
- `python3 tools/check_layout.py`: reports the worktree `.git` marker and two
  unavailable historical oracle manifests already listed by ISSUE-023; all
  new local report links resolve.
- Independent Sol/high review: clean, with no remaining findings.

Semantic sources: Intel SDM Rev. 093 Vol. 2A `FADD/FADDP/FIADD`, including
`DC C0+i`, pp. 3-311–3-313 (Operation/FPU Flags Affected p. 3-312)
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=429));
Vol. 1 §8.1.5.1 (exception masks), §8.1.5.2/Table 8-2 (precision control),
§8.1.5.3 (rounding control), and §8.5.6 (masked precision exception),
pp. 8-7–8-8 and 8-30
([official PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=236)).
See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30ct-bounded-fadd-sti-st0-dc-c0c7),
[plan](../plans/STAGE_2.md#s2-30ct-bare-reverse-fadd-sti-st0-with-bounded-fcw-precision),
[shared report](STAGE_2_REPORT.md#s2-30ct--bounded-reverse-fadd-sti-st0), and
[ISSUE-019](../KNOWN_ISSUES.md#issue-019--семантика-арифметики-и-инструкций-x87-остаётся-открытой).
