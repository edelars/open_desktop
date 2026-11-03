# S2-38ab — unmasked SQRTPD m128 memory profile

Implemented the immutable schema-v2 interpreter profile
`s2-long-sqrtpd-memory-rax-unmasked-v1` for exact Long64 `66 0F 51 00`
(`SQRTPD XMM0,m128 [RAX]`). It has a separate
`sqrtpd_memory_rax_unmasked_controls` echo and requires SSE2 bit 26, one
executable mapping, one read-only 16-byte data mapping, budget one and stop
RIP=entry+4. Complete instruction fetch precedes the SSE2/CR0.EM/CR4.OSFXSR
`#UD` gates and CR0.TS `#NM` gate.

The profile accepts supported MXCSR mask `0xffbf`, reserved bits clear and
DAZ=0, while allowing FTZ, all exception masks, RC and prior sticky flags.
The executor checks 16-byte alignment regardless of AC, validates the
canonical `[RAX,RAX+15]` span, and performs exactly one read16. It classifies
both raw-u64 lanes before mask decisions. If any new IE/DE is unmasked, it
commits all applicable aggregate IE|DE without computing PE, and raises `#XM`
or SIMD arithmetic `#UD` based on OSXMMEXCPT. Otherwise it computes both lanes,
aggregates PE and applies PM. Any unmasked SIMD event preserves the complete
XMM0, RIP and retirement count.

The independent integer-only raw-u64 oracle and in-process runner matrix cover
25 lane-event products (neutral, IE, DE-only, PE-only and DE+PE on both lanes),
8 IM/DM/PM patterns, both OSXMMEXCPT values, 4 RC modes and 64 prior sticky
histories: 102,400 cases. The matrix completed in 45.01 seconds. A pinned
oracle assertion confirms `sqrt(0x1) = 2^-537` exactly with DE only; operands
`0x2` and `0x3` produce DE+PE. Mixed-lane IE+DE+PE cases verify packed
pre-event aggregation and whole-XMM suppression.

Three fixtures cover success, unmasked IE via `#XM`, and unmasked PE via
arithmetic `#UD`. Focused executor checks cover feature/control gate priority,
truncated fetch before gates, CPL0–3, AC-independent alignment, canonical
faults, one-read rollback, pre-event PE suppression, postcompute PE, and atomic
destination suppression. Fixture checks cover missing controls, writable
source rejection, control/status/XMM/retirement/vector/`#GP(0)` error-code
tampering, and JIT/native denial. Existing masked S2-37de and register S2-37al
profiles remain separate.

## Verification

- `cargo fmt --all -- --check` — passed.
- `cargo check --workspace --all-targets --offline` — passed.
- `cargo test -p od-isa --offline` — 1,948 passed.
- `cargo test -p od-testkit --offline` — 455 passed.
- `cargo test -p od-runner --offline` unit suite — 133 passed, including the
  102,400-case matrix.
- Focused SQRTPD fixture test — passed. The full 556-test runner fixture suite
  was started but stopped after the unit suite completed; the three S2-38ab
  fixtures and their tampering/backend checks passed in the focused test.
- `git diff --check` — passed before the implementation commit.
- `graphify update .` — passed (12,338 nodes / 26,626 edges / 378 communities).
- `python3 tools/check_layout.py` — did not pass in this isolated worktree:
  the linked `.git` marker is an unmapped root entry, and two existing
  `STAGE_2_REPORT.md` links point to work manifests absent from this checkout.
  No project layout was changed in this slice.

The implementation is committed in isolated worktree commit `4907bba`.
Independent Sol/high review is pending in the integrating task. Native/JIT,
C ABI, generic/fragment execution and physical x86 acceptance remain closed;
no physical-hardware run is claimed.
