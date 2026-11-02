# S2-37eb — local Long64 memory RSQRTSS XMM0,m32 [RAX]

## Scope

The bounded schema-v2 profile `s2-long-rsqrtss-memory-rax-v1` admits exact
legacy `F3 0F 52 00`. It requires SSE bit 25, CR0.EM=0, CR4.OSFXSR=1 and then
CR0.TS=0 after complete fetch. MXCSR admission is strict: supported bits only,
all six exception masks set, DAZ=FTZ=0 and reserved bits clear. RC and prior
sticky bits are accepted and preserved.

With AC=0 the profile accepts unaligned RAX, validates canonical48 endpoints
for `[RAX,RAX+3]` and performs exactly one read4. With AC=1 it returns local
Unsupported before span validation or the data callback. The operation uses only
the memory dword and `Binary32::reciprocal_sqrt_sse_raw()`, updates XMM0.low32,
and preserves its high96, other vector state, MXCSR, flags and memory. Failed
gates, address checks and reads do not commit. JIT/native/generic/fragment/C ABI
paths are closed.

The schema-v2 result oracle checks local special-value relations and independently
checks the integer `3/8192` relative-error envelope for positive normal outputs.
It does not require correctly-rounded or hardware-identical approximation bits.
The handwritten fixture uses unaligned source data `4.0`, poison destination
bytes and an adjacent canary. The strict fixture test also covers AC=1 with a
noncanonical pointer (Unsupported before span access) and AC=0 at the same
pointer (#GP(0)), gates, tampering, and backend denial.

## Validation

The implementation suites passed: `od-isa` 1,885, `od-testkit` 455, runner
125 unit + 515 fixture tests. After the permission-gate test fix, focused ISA and
runner tests passed. A direct ISA callback-count test covers SSE unavailable,
CR0.EM and CR4.OSFXSR both alone and with CR0.TS set, plus CR0.TS alone. It
asserts #UD/#NM priority, complete fetch, zero data callbacks, zero retirement,
and unchanged state. The schema/runner test exercises the same gate outcomes and
checks the output vector, no retirement and unchanged mapped memory. The existing AC/noncanonical priority,
success/high96/MXCSR, profile-tampering and JIT/native denial checks remain. Workspace
`cargo check --workspace --all-targets --offline`, `cargo fmt --all -- --check`,
and `git diff --check` passed. Graphify AST update completed: 11,753 nodes,
25,474 edges, 322 communities. `tools/check_layout.py` reported only the
isolated worktree `.git` marker and two unavailable historical manifests
already recorded under ISSUE-023; it found no new path/link errors. Independent
Sol/high review is pending in the parent integration. No physical x86 acceptance
is claimed.
