# S2-42s: REP STOSB checkpoint and replay

This local interpreter slice checks bounded continuation at the 1024-iteration
REP work limit. It uses the existing `s2-long-integer-v1` profile and the
handwritten bytes `48 85 C0 F3 AA` (`TEST RAX,RAX; REP STOSB`). Eight literal
fixtures cover counts 1024, 1025, 2048, and 2049 with DF clear and set. RAX is
`0x8877665544332211`, so STOSB writes `0x11`. Each case maps the same 4096-byte
canary data region separately from code, at `0x00200000`; RDI begins at offset
16 for DF=0 and offset 3072 for DF=1.

The first fresh-entry call has instruction budget 2. Later calls use
`run_with_rflags_defined` with budget 1 and the exact returned definedness mask.
Between calls the test serializes and restores the complete integer CPU state,
the full memory image, the definedness mask, and the successful-store journal.
No continuation-only live interpreter or memory object survives the boundary.
TEST makes AF undefined, so every phase retains mask `0x0cc7` and compares only
defined RFLAGS bits. Completion exits are StopAddress after one, two, two, and
three calls for the four respective counts. Partial exits retain RIP at REP,
reduce RCX by 1024, and move RDI by 1024 in the DF direction. The literal
expected sidecar stores every GPR, RIP, RFLAGS value and mask, retirement count,
exit, full code/data memory image, and cumulative write journal for every phase.
The fixture-level `expected_exit` describes the first bounded call: count 1024
is `stop_address` because completed REP work takes precedence at the boundary;
the larger counts first return `rep_iteration_budget_exhausted`.

The test-only memory wrapper delegates to `FlatMemory` and journals a store only
after the delegated write succeeds. The journal must contain each expected
destination byte exactly once and in DF order. The test also rejects a changed
fixture file set, stale fixture or expected-sidecar digests, changed profile,
instruction bytes, memory layout, count or pointer, undefinedness changes,
missing/duplicate stores, and missing checkpoint phases.

Run from the repository root with a new, empty evidence directory:

```sh
OD_S2_42S_EVIDENCE_DIR="$(mktemp -d)/s2-42s-run" \
  cargo test -p od-testkit --test s2_42s_rep_checkpoint --offline -- --nocapture
```

The command fails if the evidence directory already exists, any fixture or
snapshot differs, a phase/repeat is missing, or a write journal differs. It
copies each input fixture and digest, the literal snapshots and manifest, and
both serialized replay records per case to the run directory. The summary
includes source revision, Rust compiler, host target, backend, profile, flag
mask, code bytes, and explicit `native: not_run` and `physical: not_run` values.
This is bounded local evidence only; it is not native, physical, or 1000-case
corpus acceptance.
