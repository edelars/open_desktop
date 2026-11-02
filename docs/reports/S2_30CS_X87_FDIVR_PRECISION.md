# S2-30cs — bounded precision-aware x87 FDIVR

The interpreter admits only exact bare Long64 `D8 F8..FF` (`FDIVR
ST(0), ST(i)`). The local bounded profile computes old `ST(i) / ST(0)` by
passing the numerator and denominator directly, in that order, to the existing
`divide_finite_to_normal_binary80_precision_with_diagnostics` helper. It does
not derive FDIVR through a negated or subtracted FDIV result. The helper rounds
once at the FCW-selected p24, p53, or p64 and RC mode.

Admission requires present x87 state, absent FCS/FDS, one of the twelve
all-exception-masked precision/rounding FCWs, and `FSW & 0x80df == 0`. Used
physical registers must have Valid tags and canonical nonzero Normal binary80
operands of either sign. For `i=0`, only physical TOP is validated; the exact
result is canonical `+1`. For `i>0`, only finite Normal exact quotient domains
and finite Normal rounded results are accepted. Zero divisor, zero, subnormal,
special or malformed operands, zero/subnormal/overflow results, unsupported
FCW/status and out-of-domain exponents refuse atomically.

Success replaces old physical TOP without popping, sets `PE = old_PE ||
inexact`, replaces C1 with the helper's magnitude `incremented` diagnostic,
and preserves C0/C2/C3 under the local deterministic policy. TOP, tags, source
cells, selectors, FCW and unrelated CPU/x87 state are preserved. FIP receives
old RIP, RIP advances by two, the instruction retires once, and no data-memory
callback occurs. D8 F0..F7 FDIV is unchanged; DC/DE directions remain
exact-only. JIT/native execution and guest exception delivery remain closed.

The former p64 inexact runner refusal for `D8 F9` now verifies rounded `1/3`.
Genuine invalid-state and out-of-domain refusals remain. Additional literal
coverage checks all twelve FCWs, all TOP/index pairs and sign combinations,
sticky PE and C1 replacement, exact self-index, midpoint parity and neighbors,
discarded tails, carry, direct operand order, minimum/maximum Normal results,
source/tag/full-state canaries, malformed/special/zero values, unsupported
status/control, FCS/FDS, fetch/truncation, prefixes, STOP, stop-RIP, budget,
fetch-only behavior and backend denial.

## Verification

- Focused `od-isa` FDIVR tests: 6 passed; adjacent FDIV tests: 4 passed.
- Focused `od-testkit` FDIVR tests: 3 passed.
- Full `od-isa`: 1,792 passed; `od-testkit`: 455 passed; `od-runner`: 113 unit
  tests and 490 fixture tests passed.
- `cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
  and `git diff --check` passed.
- `python3 tools/check_layout.py` reported only the existing `.git` root marker
  and two unavailable historical oracle manifests tracked by ISSUE-023.
- `graphify update .` refreshed the worktree graph to 11,189 nodes, 24,175 edges,
  and 334 communities.
- Independent Sol/high review is complete and clean; it reran focused FDIVR
  filters: `od-isa` 8/8 and `od-testkit` 4/4.

No native x87 execution or physical x86 acceptance is claimed. ISSUE-019
remains open. Sources: Intel SDM Rev. 093 Vol. 2A `FDIVR` Operation/FPU Flags
Affected; Vol. 1 §8.1.5.2/Table 8-2, §8.1.5.3 and §8.5.6 for PC/RC and masked
precision behavior ([Vol. 2A](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=451),
[Vol. 1](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=236)).
See the [plan](../plans/STAGE_2.md#s2-30cs-bare-fdivr-st0-sti-with-bounded-fcw-precision),
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-30cs-bounded-fdivr-st0-sti-d8-f8ff),
[shared Stage 2 report](STAGE_2_REPORT.md#s2-30cs--bounded-fdivr-st0-sti), and
[ISSUE-019](../KNOWN_ISSUES.md#issue-019--семантика-арифметики-и-инструкций-x87-остаётся-открытой).
