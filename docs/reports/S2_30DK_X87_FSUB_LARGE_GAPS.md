# S2-30dk — large-gap `FSUB ST(i), ST(0)` precision

Bare Long64 `DC E8..EF` now admits canonical Normal operands with normalized
exponent gaps through 32765. The instruction computes `ST(i) - ST(0)` and
writes the old physical ST(i), without popping. The executor validates both
original operands, flips only a local ST(0) copy, then reuses the existing
S2-29af direct-addition precision helper. Gaps through 64 retain the previous
S2-30cv arithmetic and state behavior.

The existing twelve all-exception-masked PC24/53/64 FCWs, selectors, FSW mask,
Valid tags, Normal result requirement, sticky PE, local pre-carry C1 diagnostic,
TOP/tags/source bytes and unrelated state behavior remain in force. Alias
cancellation, overflow and unsupported outcomes reject atomically. `DC E0..E7`
FSUBR and `DE E8..EF` FSUBP retain their gap-64 limit. The register instruction
performs no data access. JIT/native admission and physical x86 acceptance
remain closed.

An independent integer oracle covers gaps 64/65/66/32765, both operand signs
and orders, all FCWs and TOP/non-alias-index placements, directed rounding,
PE/C1 histories and complete state. Atomic-refusal coverage includes alias and
large-gap reverse/pop neighbors. The literal schema-v2 runner fixture
`s2_30dk_fsub_sti_st0_gap65_up.case.json` carries actual ST(0)=2^-65 and checks
full x87 state plus JIT denial.

Normative opcode semantics: Intel SDM Rev. 093 Vol. 2A `FSUB/FSUBP/FISUB`,
`DC E8+i`, pp. 3-394–3-396
([official Vol. 2A PDF](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=512));
precision and masked-exception context: Vol. 1 §§8.1.5.1–.3 and §8.5.6
([official Vol. 1 PDF](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=236)).

## Verification

Passed:

- Independent large-gap oracle, atomic refusal, bounded-neighbor, and literal runner fixture tests.
- `cargo test -q -p od-isa -p od-testkit -p od-runner --offline`: od-isa 1,881; runner 125 unit + 513 fixture. Separate `cargo test -q -p od-testkit --offline`: 455.
- `cargo check --workspace --all-targets --offline`.
- `cargo fmt --all -- --check` and `git diff --check`.
- `graphify update .`: 11,765 nodes, 25,339 edges, 376 communities.
- Independent Sol/high review clean after correcting PE/C1 history coverage, stale gap-65 expectations and local links.

`python3 tools/check_layout.py` reports the isolated worktree `.git` marker as
unmapped and two pre-existing links in the shared Stage 2 report to historical
oracle manifests absent from this checkout. A targeted check of the new
S2-30dk local links passed. No project path was added outside the existing
layout.

No physical x86 execution, JIT support, native admission, or general x87
exception-delivery claim is made.

See the [CPU contract](../specs/CPU_CONTRACT.md#local-s2-30dk-large-gap-fsub-sti-st0-dc-e8ef),
[Stage 2 plan](../plans/STAGE_2.md#s2-30dk-large-gap-fsub-sti-st0-admission),
[shared Stage 2 report](STAGE_2_REPORT.md#s2-30dk--large-gap-fsub-sti-st0), and
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-30dk-large-gap-reverse-destination-fsub-fixture).
