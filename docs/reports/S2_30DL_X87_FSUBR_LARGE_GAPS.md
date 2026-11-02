# S2-30dl — large-gap `FSUBR ST(i), ST(0)`

**Status:** implemented in an isolated worktree; interpreter-only. No physical
x86 acceptance is claimed.

## Scope and behavior

The exact bare `DC E0..E7` path now admits normalized Normal operands whose
exponent gap is 0..32765. Only this `FSUBR ST(i), ST(0)` caller enables the
large-gap path in the existing bounded FCW-precision helper. Both original
operands pass the existing environment, tag and canonical Normal gates before
the helper applies its local sign transformation. The operation computes old
`ST(0)-ST(i)`, writes old physical `ST(i)`, and does not pop.

PC24/53/64, all four rounding modes, prior PE stickiness and C1 diagnostics
continue through the same helper. Existing gap≤64 outcomes are preserved by
that unchanged arithmetic path. Aliasing cancellation and unsupported results
remain atomic. `DC E8..EF` FSUB retains its prior large-gap support; `DE E0..E7`
FSUBRP remains capped at gap 64. JIT/native/C ABI/general x87 exception delivery
and physical x86 claims remain outside this slice.

## Coverage

An independent signed-magnitude integer oracle checks 86,016 combinations:
gaps 64, 65, 66 and 32765 across all twelve supported FCWs, all four sign
combinations, both operand magnitude orders, every TOP and destination index
1..7, and all prior PE/C1 histories. Expected significands, exponent, inexact,
and incremented C1 are derived without production arithmetic helpers. Atomic
alias and neighbor controls verify that the existing `DC E8..EF` form remains accepted and
`DE E0..E7` remains capped. A literal schema-v2 `DC E1` fixture uses actual
`ST(0)=+1` and `ST(1)=2^-65`; the runner checks the full architectural snapshot
and JIT refusal.

Intel SDM Rev. 093 Vol. 2A lists `DC E0+i` as `FSUBR ST(i), ST(0)` and states
that it subtracts `ST(i)` from `ST(0)`, storing the result to `ST(i)`; the
paired `DE E0+i` form also pops. Precision and exception policy follows the
existing project contract. References: [Intel SDM Vol. 2A, pp. 3-397–3-399](https://cdrdv2.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=515),
[Intel SDM Vol. 1, §§8.1.5.1–.3 and 8.5.6](https://cdrdv2.intel.com/929351/253665-093-sdm-vol-1.pdf#page=236).

## Verification

- Focused ISA tests: **8 passed**, including the independent large-gap oracle
  and neighbor controls.
- Literal fixture runner test: **1 passed**, including complete state and JIT
  denial.
- Full packages: `od-isa` **1,886 passed**, `od-testkit` **455 passed**, and
  `od-runner` **125 unit + 515 fixture tests passed**.
- Workspace all-target offline check passed. Formatting and `git diff --check`
  passed after applying rustfmt. The layout checker reports the isolated
  worktree's unmapped `.git` pointer plus two pre-existing missing oracle
  manifests in the shared Stage 2 report, already tracked by ISSUE-023; no new
  project path was added.
- `graphify update .` passed its AST refresh: 11,794 nodes, 25,407 edges and
  375 communities.
- Independent Sol/high review is clean after adding the full prior PE/C1
  cross-product and correcting the CPU-contract link anchor. No findings remain.

See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30dl-large-gap-fsubr-sti-st0-dc-e0-e7),
[Stage 2 plan](../plans/STAGE_2.md#s2-30dl-large-gap-fsubr-sti-st0-admission), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30dl-large-gap-reverse-destination-fsubr-fixture).
