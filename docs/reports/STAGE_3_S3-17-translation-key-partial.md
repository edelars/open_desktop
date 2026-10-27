# S3-17: typed translation key partial

Date: 2026-10-26

## Implemented slice

`od_jit::translation::TranslationKey` is a typed, immutable compile identity
for one `BasicBlock`. Construction requires the full `CodeView`, the selected
entry and `CompileFlavor`, the block's terminal classification and actual
instruction ranges, and caller-provided `CodeDependencySpan` metadata. The
key copies all CodeView bytes, including unread tail bytes, and retains base,
exclusive end, terminal classification, complete dependency spans, and exact
instruction fetch ranges. Since `BasicBlock` is public caller data, key
construction re-decodes each claimed instruction against the same CodeView,
with reads bounded to the claimed instruction range, and rejects mismatched
decoder output.

Dependency spans must cover the full CodeView without gaps or overlap and must
be executable. Mapping and backing identities and versions are nonzero typed
values; repeated mapping identities must describe one consistent snapshot,
and aliases may use distinct mapping IDs with the same backing ID/version.
Versions expose checked increment that refuses wrap. Key hashing is only a
candidate accelerator: equality compares all key fields and complete bytes.

The identity/version provider remains caller-owned. The current CodeView is
not connected to an MMU owner, so the implementation does not derive IDs from
host pointers, invent generations, or claim production invalidation. No
production cache, reuse, or eviction was added. Runtime budget, STOP, GPR and
RFLAGS do not enter the key API. S3-17 remains open.

## Verification

- `cargo test -p od-jit translation::tests --offline` — 8 passed.
- `cargo test -p od-jit --offline` — 78 passed after source-mismatch regression.
- `cargo check --workspace --all-targets --offline` — passed.
- `cargo fmt --all -- --check` — passed.
- `git diff --check` — passed.
- `graphify update .` — passed; refreshed the workspace code graph.
- Independent review — clean after fixing the source-binding finding. Reviewer
  confirmed bounded re-decode rejects foreign records without reading beyond
  their claimed instruction ranges.

Cases cover same RIP with changed bytes/tail, flavor/base/end/terminal,
mapping/backing identity and versions, executable permission, aliases, a fetch
range spanning multiple dependency regions, coverage gaps/overlap, arithmetic
overflow, inconsistent snapshots, non-wrapping versions, runtime-state
exclusion, and full equality after forced hash collisions. No live cache or
runtime invalidation behavior was tested or is claimed.

## Lookup benchmark prototype (implementation only)

Added `tools/bench` as an isolated harness prototype. On Apple Silicon macOS,
`record --case-list PATH` wraps the existing iterative dispatch executor and
records the state RIP at each actual block entry before publishing/executing
that block. It checks interpreter/JIT retirement counts, budget conservation,
state, and expected exit before retaining the trace. The trace stores frozen
fixture/code digests, code bytes and caller identity/version metadata. Trace
capture and local release measurements have been run on the frozen 1,010-input
fixture corpus; the exact trace and three process reports are summarized in
[the lookup measurement report](STAGE_3_S3-17-lookup-measurements-2026-10-26.md).

`measure --trace PATH` validates the serialized trace, reconstructs typed
translation keys outside timed regions, and creates the required corpus and
synthetic scenario families. Replay re-decodes complete fixture bytes and
validates admission/entry/block record boundaries before key construction;
observed unique-key workloads preserve originating fixture associations across
multiple keys per fixture. Vec and `std::collections::HashMap` use exact key
equality and the same complete dependency snapshot validator. Cold
construction+insert is timed separately from warmed lookup+dependency
validation. Cold repetitions run in bounded micro-batches. Each
sample sums only table-construction timer intervals; table destruction and
between-batch pauses are excluded from `elapsed_ns`, while report
`total_elapsed_ns` includes them. Raw samples record the number of cold
micro-batches. Calibration
keeps the 200 ms minimum and can exceed the per-batch table count by running
additional bounded batches. The 128 MiB logical budget subtracts resident suite
data and the execution Vec/HashMap copies before bounding one cold micro-batch;
it counts inline key/table slots and owned key buffers. Cold HashMaps reserve
for their known entry count before insertion. HashMap control bytes, allocator
metadata, runtime overhead, and process RSS are not measured, so this is an
explicit logical-storage bound rather than an RSS guarantee. The raw trace
buffer and parsed trace are dropped before timing. Each of the four kernels is
independently calibrated;
samples alternate order and report raw operation counts, summed elapsed
nanoseconds, batch counts, median/min/max/IQR. Trace schema 2 stores capture host
data, build-bound toolchain/revision/source fingerprint/profile, and a separate
runtime working-directory/revision/dirty context. Measure report schema 3 adds
the per-sample batch count and includes the exact input trace SHA-256 and the
corresponding build/runtime metadata for the measure binary.

The recorder is gated to macOS ARM64; the measure/replay command is portable.
Hard input/visit/key/scenario/payload/time bounds are enforced. Tests verify
trace reconstruction and every required scenario family, stale-hit behavior
after a successful old-key lookup, Vec/HashMap result parity, malformed
input/limit rejection, retirement count validation, build/runtime metadata,
and sample-configuration bounds. `cargo test -p od-bench --offline` passed 13/13;
`cargo test -p od-jit --offline` passed 79/79; `cargo test -p od-host --offline`
passed 27/27; `cargo check --workspace --all-targets --offline`,
`cargo fmt --all -- --check`, `git diff --check`, and `graphify update .` passed.
`python3 tools/check_layout.py` in the linked worktree reports `.git` as an
unmapped root file and two pre-existing broken links in `docs/reports/STAGE_2_REPORT.md`;
the primary-checkout layout run remains with the root task.
The earlier `od-bench synthetic` CLI smoke was manually interrupted before it
produced a report; it is not counted as a benchmark result. Three subsequent
release `measure` runs completed the recorded trace and all 18 scenarios. The
lookup report is limited to table construction and lookup on prepared keys; it
does not establish end-to-end dispatcher/JIT speed or a production cache policy.
S3-17 remains open for implementation review and production cache integration.
