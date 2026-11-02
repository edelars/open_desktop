# S2-30cu — bounded reverse x87 FMUL precision

The Long64 interpreter admits exact bare `DC C8..CF` (`FMUL ST(i), ST(0)`).
It computes the full product through the S2-29ad direct precision helper at the
FCW-selected PC24/53/64 and RC. Both operands must be Valid-tagged canonical
nonzero Normal binary80 values of either sign. Admission requires present x87
state, absent FCS/FDS, one of twelve all-exception-masked FCWs and
`FSW & 0x80df == 0`. The `i=0` alias validates one physical TOP tag and squares
ST(0), which may be inexact.

Only finite Normal exact-product and rounded results are admitted. On success,
the result is written only to old physical `(TOP+i)&7`; PE is old PE OR
`inexact`, C1 is replaced by the helper's pre-carry magnitude `incremented`,
and C0/C2/C3 follow the deterministic local policy. TOP, tags, source cells,
FCW/FDP/FOP and unrelated state are preserved; FIP receives old RIP, RIP
advances by two, and the instruction retires once. No data-memory access
occurs. Refusals and fetch, stop, and budget boundaries are atomic.

The precision-aware path is limited to `Operation::FmulStSt0`. Existing
`D8 C8..CF` FMUL precision, exact-only `DE C8..CF` FMULP, and other reverse
arithmetic forms are unchanged. No general x87 exception delivery,
native/JIT/schema admission, or physical x86 acceptance is implied.

Coverage includes:

- An 11,520-case literal matrix spanning all 64 TOP/index pairs, all twelve
  FCWs, all four sign combinations for distinct operands, both alias-square
  signs, and all prior PE/C1 combinations.
- Literal halfway/adjacent results, direct-versus-double-rounding, carry
  normalization, negative directed rounding, unchanged `D8` precision and
  exact-only `DE` FMULP neighbor regressions.
- Invalid FCW/FSW, selector, tag, zero/special/malformed operands, product
  exponent overflow and underflow refusals, plus full state snapshots for
  refusals, truncated fetch, STOP, budget exhaustion and requested stop.
- Destination-only payload changes, TOP/tag/source/status preservation,
  sticky PE, C1 replacement, FIP/RIP/retirement, and fetch-only memory access.

Validation passed:

- Focused `cargo test -p od-isa fmul --offline`: 13 tests.
- Full `cargo test -p od-isa --offline`: 1,805 tests.
- `cargo test -p od-testkit -p od-runner --offline`: 455 testkit tests,
  114 runner unit tests and 492 runner fixture tests.
- `cargo fmt --all -- --check`, `cargo check --workspace --all-targets
  --offline` and `git diff --check` passed.
- `python3 tools/check_layout.py` reported only the isolated-worktree `.git`
  marker and two missing historical oracle manifests already tracked by
  ISSUE-023; it reported no new broken documentation links.
- `graphify update .` completed: 11,300 nodes, 24,290 edges and 342
  communities.

Independent Sol/high review: clean, with no remaining findings. The reviewer
reran all 13 focused FMUL tests and `git diff --check`. Physical x86, native
and JIT acceptance are not claimed.

Sources: Intel SDM Rev. 093 Vol. 2A `FMUL/FMULP/FIMUL`, including `DC C8+i`
encoding and Operation/FPU Flags Affected, pp. 3-360–3-362
([official PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=478));
Vol. 1 §§8.1.5.1–.3, Table 8-2, and §8.5.6, pp. 8-7–8-8 and 8-30
([official PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=236)).
See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30cu-bounded-fmul-sti-st0-dc-c8cf),
[plan](../plans/STAGE_2.md#s2-30cu-bare-reverse-fmul-sti-st0-with-bounded-fcw-precision),
[shared report](STAGE_2_REPORT.md#s2-30cu--bounded-reverse-fmul-sti-st0), and
[ISSUE-019](../KNOWN_ISSUES.md#issue-019--семантика-арифметики-и-инструкций-x87-остаётся-открытой).
