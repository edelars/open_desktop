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
host pointers, invent generations, or claim production invalidation. No cache,
lookup table, reuse, eviction, or data-structure benchmark was added. Runtime
budget, STOP, GPR and RFLAGS do not enter the key API. S3-17 remains open.

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
