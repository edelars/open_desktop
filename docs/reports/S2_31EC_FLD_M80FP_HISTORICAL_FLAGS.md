# S2-31ec — historical exception flags for raw FLD m80fp

S2-31ec widens only the exact bare Long64 `DB 28` `FLD m80fp [RAX]` local
status gate from `FSW & 0x80dc == 0` to `FSW & 0x80c0 == 0`. It admits and
preserves all six pre-existing exception flags IE/DE/ZE/OE/UE/PE while still
rejecting SF, ES and B. FCW, Empty destination, accepted raw classes/tags,
selectors, ten-byte payload, one-read and push-on-success rules are unchanged;
the shared FILD gate remains untouched. Native/JIT remain closed.

The execution matrix checks 64 historical flag combinations × 34 raw class
literals × 8 TOP values × 4 FCWs (69,632 full `LongModeState` comparisons).
The rollback test seeds `FSW=0x043f`, so span and failed-read paths preserve all
six sticky flags. ISA execution tests cover repeated FLD/FNSTSW/FNCLEX/retry.
A literal runner fixture covers all-flags admission/status, strict schema
checks and native/JIT denial.

## Verification

- Author check `cargo test -p od-isa --offline`: 1,928 passed
- Author check `cargo test -p od-testkit --offline`: 455 passed
- Focused fixture-v2 and runner CLI fixture tests passed in the author worktree
- Native and JIT denial test passed in the author worktree
- Author check `cargo check --workspace --all-targets --offline` passed
- Author `cargo fmt --all -- --check` and `git diff --check` passed
- Independent Sol/high review passed after the rollback coverage correction
- Graphify after main merge: 12,065 nodes / 26,206 edges / 328 communities

No physical x86 acceptance is claimed. Source semantics are documented in the
[CPU contract](../specs/CPU_CONTRACT.md#s2-31ec--historical-zeoeue-for-raw-fld-m80fp).
See the [main integration report](STAGE_2_REPORT.md#main-integration-s2-31ec)
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-31ec-historical-zeoeue-for-raw-fld-m80fp).
