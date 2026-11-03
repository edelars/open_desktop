# S2-31ed — historical SF with pre-existing IE for raw FLD m80fp

S2-31ed extends only exact bare Long64 `DB 28` (`FLD m80fp [RAX]`). The
profile admits historical SF when IE is already set, while preserving the
prior IE/DE/ZE/OE/UE/PE flags and continuing to reject ES and B. SF without IE
remains a pre-read Unsupported case. The bounded DB28 gate is local; FILD and
other x87 gates are unchanged. Success retains the historical SF and sticky
flags, clears C1, and uses the existing raw ten-byte push behavior. Native/JIT
remain closed.

The complete-state ISA matrix adds 69,632 SF+IE combinations: 32 combinations
of DE/ZE/OE/UE/PE × 34 accepted raw binary80 literals × 8 TOP values × 4 FCWs
× both old C1 values. The existing 69,632 SF=0 cases remain in the same
matrix. Rejection coverage includes SF-only with both C1 values; ES and B
alone and together over an otherwise admissible SF+IE state; all such gates
are checked before data reads. Span and injected read failures preserve full
state with C1 clear/set; the repeated FLD/FNSTSW/FNCLEX/retry sequence covers
both SF=0 and SF+IE. FILD continues to reject SF even with IE.

The added literal fixture carries a signaling NaN source with SF+IE and all
other historical exception flags already set. Schema and runner checks cover
the new positive fixture; backend tests retain interpreter-only admission.

## Verification

- `cargo test -p od-isa --offline`: 1,932 passed
- `cargo test -p od-testkit --offline`: 455 passed
- `cargo test -p od-runner --offline`: 130 unit + 540 fixture tests passed
- `cargo check --workspace --all-targets --offline` passed
- `cargo fmt --all -- --check` and `git diff --check` passed
- Graphify update: 12,146 nodes / 26,227 edges / 401 communities
- `tools/check_layout.py` could not complete in this linked worktree: it treats
  the worktree's `.git` pointer file as an unmapped root file, and two linked
  manifests under ignored local `work/` data are unavailable in this checkout.
  The main checkout previously passed layout validation; rerun after integration.
- Parent's sequential implementation review found no remaining blocking defects.
  An independent Sol/high agent review was unavailable because all agent slots
  were occupied; this limitation was recorded for the slice.

No physical x86 acceptance is claimed. See the [CPU contract](../specs/CPU_CONTRACT.md#s2-31ed--historical-sf-with-pre-existing-ie-for-raw-fld-m80fp),
[Stage 2 plan](../plans/STAGE_2.md#s2-31ed-historical-sf-with-pre-existing-ie-for-raw-fld-m80fp),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-31ed-historical-sf-with-prior-ie-on-raw-fld-m80fp).
