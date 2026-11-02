# S2-30cw — bounded reverse `FSUBR ST(i), ST(0)` precision

Implemented direct FCW-selected precision for exact bare Long64 `DC E0..E7`.
The instruction computes old `ST(0) - ST(i)` and writes old physical
`(TOP+i)&7` without popping. Execution reuses the existing
`bounded_x87_fsub_st0_sti_result` preflight and arithmetic helper; no new
arithmetic helper or exact fallback was added.

Admission requires present x87 state, absent FCS/FDS, one of the twelve
all-exception-masked PC24/53/64 FCWs, `FSW & 0x80df == 0`, Valid tags,
canonical nonzero Normal binary80 operands and normalized exponent gap≤64.
Alias `i=0` cancels and remains an atomic Unsupported result. Only finite
Normal exact or rounded outputs are accepted. The prior S2-30l exact domain
is covered by the precision-64 equal-exponent profile.

Success changes only the old physical `ST(i)` payload. PE is sticky and ORed
with `inexact`; C1 is replaced from the rounding helper's `incremented`
diagnostic. TOP, tags, source payload, C0/C2/C3 under local policy, selectors,
FCW/FDP/FOP and unrelated state are preserved. FIP receives old RIP, RIP
advances by two, and the instruction retires once. There is no data access.
Unsupported operand/environment/result cases and fetch, STOP and budget
boundaries do not partially commit. Existing exhaustive DC ModR/M and prefix
decoder tests continue to constrain the accepted encoding. `D8` neighbors,
`DE` pop behavior, JIT/native execution, general x87 exception delivery and
physical x86 acceptance remain outside this slice.

Coverage includes all 64 TOP/index combinations, twelve FCWs, four sign
combinations and all prior PE/C1 histories (12,288 cases), exact S2-30l
compatibility snapshots, the PC24 direct-versus-double-rounding discriminator,
negative directed rounding, alias cancellation, malformed and gap>64 refusals,
selectors, fetch/STOP/budget atomicity, and no data access.

Validation passed:

- Focused `cargo test -p od-isa --offline fsubr_sti_st0`: 6 tests.
- Full `cargo test -p od-isa --offline`: 1,819 tests.
- `cargo fmt --all -- --check`, `cargo check --workspace --all-targets --offline`,
  and `git diff --check` passed.
- The isolated layout checker reported its `.git` pointer and two unavailable
  historical oracle manifests already tracked by ISSUE-023; no new broken
  documentation links were found. Main layout validation passed before this
  isolated slice.
- `graphify update .` completed: 11,361 nodes, 24,461 edges, 352 communities.

Independent Sol/high review is clean (39 `fsub` tests passed); no physical
x86/JIT/native acceptance is claimed.

Sources: Intel SDM Rev. 093 Vol. 2A `FSUBR/FSUBRP/FISUBR`, including
`DC E0+i`, pp. 3-397–3-399
([official Vol. 2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=515));
Vol. 1 §§8.1.5.1–.3, Table 8-2 and §8.5.6, pp. 8-7–8-8 and 8-30
([official Vol. 1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=236)).
See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30cw-bounded-fsubr-sti-st0-dc-e0-e7),
[plan](../plans/STAGE_2.md#s2-30cw-bare-reverse-fsubr-sti-st0-with-bounded-fcw-precision),
[shared report](STAGE_2_REPORT.md#s2-30cw--bounded-reverse-fsubr-sti-st0), and
[ISSUE-019](../KNOWN_ISSUES.md#issue-019--семантика-арифметики-и-инструкций-x87-остаётся-открытой).
