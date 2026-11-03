# S2-31ee — historical sticky flags on exact bounded FILD forms

S2-31ee changes only the pre-read environment check shared by bare Long64
`DF 00` (FILD m16), `DB 00` (FILD m32) and `DF 28` (FILD m64). It admits
all 64 combinations of IE/DE/ZE/OE/UE/PE and rejects SF, ES and B. The existing
FCW allowlist, x87-state requirement and Empty physical push destination remain
in force; unrelated tags and payloads are not inspected. After exactly one
successful width-sized read, the existing integer-only conversion/push path
preserves all sticky flags and clears C1. Read/span failures leave complete
state unchanged. Native and JIT execution remain closed.

The ISA matrix runs 4,096 complete architectural states per width (64 sticky
combinations × 8 TOP values × 4 FCWs × both old C1 values), 12,288 states in
total. Each width uses independent literal binary80 encodings for zero, +1,
−1, signed extrema; FCW, TOP, sticky flags, C1, status condition codes, payloads,
tags, pointers, GPRs, RFLAGS, exact read width and RIP are checked. Separate
checks cover pre-read no-read refusal for SF/SF+IE/ES/B, invalid destination and
FCW, canonical range faults, injected read rollback with both C1 values,
unaligned reads, and FILD/FNSTSW/FNCLEX/retry composition. Four handwritten
schema-v2 fixtures cover all three forms, including a full-flags FILD/FNSTSW
sequence; the runner verifies literal results and rejects JIT/native backend
requests without fallback.

## Verification

- `cargo test -p od-isa --offline`: 1,936 passed
- `cargo test -p od-testkit --offline`: 455 passed
- `cargo test -p od-runner --offline`: 130 unit + 542 fixture tests passed
- `cargo check --workspace --all-targets --offline` passed
- `cargo fmt --all -- --check` and `git diff --check` passed
- `graphify update .` passed: 12,180 nodes, 26,287 edges, 376 communities
- `tools/check_layout.py` reports the linked worktree `.git` pointer as an
  unmapped root file and reports two historical fixture-manifest links under
  absent ignored `work/` data. Before this report was added, it also reported
  this report's two new links as broken; those links now resolve. No fixture
  shape or placement violation was reported.
- Independent Sol/high review completed cleanly with no findings. It checked
  the `0x80c0` gate, 12,288-state matrix, literal oracle, sticky preservation,
  rollback, no-read refusals and backend denial; 10 focused ISA checks and the
  new runner fixture/backend test passed.

## Main integration

Integrated as `ccc259c` with S2-38x. The independent Sol/high review found no
defects. Combined main verification passed: od-isa 1,937, od-testkit 455,
od-runner 130 unit + 543 fixture, workspace offline check, fmt, layout, diff
check, and Graphify 12,150/26,369/366. See the [main Stage 2 integration
report](STAGE_2_REPORT.md#main-integration-s2-31ee-s2-38x).

No physical x86 acceptance is claimed. See the [CPU contract](../specs/CPU_CONTRACT.md#s2-31ee--historical-sticky-flags-on-exact-bounded-fild-memory-forms),
[Stage 2 plan](../plans/STAGE_2.md#s2-31ee--exact-fild-historical-sticky-flag-admission),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-31ee-historical-sticky-flags-on-bounded-fild).
