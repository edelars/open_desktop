# S2-30db — bounded FCW-precision `FSUBP ST(i), ST(0)`

Implemented bounded FCW-precision execution for exact bare Long64 `DE E8..EF`.
The operation reuses `bounded_x87_fsub_sti_st0_result`; if that bounded gate
does not apply, the prior exact S2-30j helper remains available. It requires
present x87 state, absent FCS/FDS selectors, one of twelve all-masked PC24/53/64
FCWs, `FSW & 0x80df == 0`, Valid tags, canonical nonzero Normal operands and
normalized exponent gap≤64. Only finite Normal results are accepted. The `i=0`
alias cancels and remains atomically Unsupported.

On success, the rounded result is written to old physical `(TOP+i)&7` before
old TOP is marked Empty and TOP advances. PE is sticky and C1 is replaced by
the helper's `incremented` diagnostic. Other x87 state, GPRs and RFLAGS are
preserved; FIP receives the starting RIP, RIP advances by two, and no data
memory is accessed. Native/JIT, general x87 exception delivery and physical
x86 acceptance remain unsupported.

## Coverage and validation

- Focused FSUBP ISA tests passed 7/7. The added direct commit matrix covers
  10,752 combinations across all twelve FCWs, eight TOP values, destination
  indices 1–7, four operand sign pairs, and incoming PE/C1 histories. Alias
  cancellation is atomic for every TOP/FCW pair; normalized gap 64 succeeds
  and gap 65 refuses atomically. An independent expected-byte matrix covers
  the formerly exact-only odd 65-bit coefficient rounding across all FCWs,
  TOP/index pairs and incoming PE/C1 histories (2,688 combinations).
- Neighbor ISA filters passed: `fsub` 41, `fsubr` 17, `faddp` 7 and `fmulp` 3.
- The static schema-v2 PC24 midpoint fixture compares the complete post-state,
  exception observation and unchanged code memory. Its fixture runner test
  passed 1/1 and verifies JIT Unsupported.
- Full suites passed: `od-isa` 1,843; `od-testkit` 455; `od-runner` 119 unit
  and 500 fixture tests; `od-x86-runner` 8 library and 4 binary tests.
- `cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`
  and `git diff --check` passed. `python3 tools/check_layout.py` reports the
  isolated worktree `.git` marker and two unavailable historical oracle
  manifests linked from the shared report; no new broken local links were
  found. Graphify refreshed to 11,463 nodes, 24,821 edges and 337 communities.
  This AST-only refresh noted that saved community labels need refreshing; no
  Graphify LLM API key was configured.
- Independent Sol/high review passed clean after correction; no production
  findings remain. No physical x86 acceptance is claimed.

Sources: Intel SDM Rev. 093 Vol. 2A `FSUB/FSUBP/FISUB`, pp. 3-394–3-396;
Vol. 1 §§8.1.5.1–.3, Table 8-2 and §8.5.6, pp. 8-7–8-8 and 8-30. See the
[CPU contract](../specs/CPU_CONTRACT.md#local-s2-30db-bounded-fcw-precision-fsubp-sti-st0-de-e8i),
[plan](../plans/STAGE_2.md#s2-30db-bare-fsubp-sti-st0-with-bounded-fcw-precision),
[shared report](STAGE_2_REPORT.md#s2-30db--bounded-fsubp-sti-st0), and
[ISSUE-019](../KNOWN_ISSUES.md#issue-019--семантика-арифметики-и-инструкций-x87-остаётся-открытой).
