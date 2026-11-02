# S2-38o — MINSD XMM0,m64 [RAX] with unmasked IM/DM exits

## Result

Implemented the local interpreter/schema-v2 profile
`s2-long-minsd-memory-unmasked-rax-v1` for exact Long64 `F2 0F 5D 00`.
It shares the immutable MINSD feature/exception controls with S2-38g; CR0.MP
is fixed to one for the memory profile. The existing S2-37cw masked memory
path and S2-38g register path remain separate.

The profile completes stop/budget checks and a full four-byte fetch before
exact-form and SSE2/CR0.EM/CR4.OSFXSR #UD gates, followed by CR0.TS #NM. It then
checks XMM/MXCSR admission, refuses AC=1 locally, validates checked `RAX+7`
and both canonical48 endpoints, and performs exactly one read8. Memory/address
faults precede exception flags and leave state and retirement unchanged. AC=0
allows unaligned m64.

MINSD classifies original raw binary64 operands before selecting the raw
source-2 result for NaNs, equal values, and signed-zero ties. New NaN IE has
priority over DE when both events are masked. If either relevant mask is open,
new IE|DE status is committed on one arithmetic exit; OSXMMEXCPT selects #XM
or arithmetic #UD. The faulting path suppresses the complete XMM0 write and
retirement while preserving RIP. Successful execution updates only XMM0.low64,
new sticky bits, RIP and retirement. The independent schema oracle uses
integer-only raw-u64 classification and ordering.

## Evidence

Handwritten fixtures cover masked QNaN plus memory subnormal source-2 selection,
masked memory SNaN payload preservation, and unmasked QNaN #XM with XMM0/RIP
suppression. Five `od-isa` tests exercise fetch/gate ordering and fetch failure,
neighboring-form rejection, unaligned reads, address/read rollback,
signed-zero/finite/infinity ties, the IM×DM×OSXMMEXCPT event matrix, and RC×64
old sticky histories across NaN, subnormal, and mixed event/mask cases. The
runner CLI regression test forges both `jit` and `native_x86` labels and
confirms compare-results rejects them. No physical x86 acceptance is claimed.

Focused commands passed:

- `cargo test -p od-isa minsd_memory_unmasked --offline` — 5 passed.
- `cargo test -p od-runner --test fixture_v2 minsd_m64_memory_unmasked_profile_checks_mixed_priority_echo_and_backend_denial --offline` — 1 passed.

Broader verification passed: `od-isa` 1,900, `od-testkit` 455, runner binary
125 and runner `fixture_v2` 521; `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets --offline`, `git diff --check`, and
`graphify update .` passed. Graphify reports 11,899 nodes, 25,674 edges and
381 communities; Graphify labels were not refreshed without an LLM API key. `python3 tools/check_layout.py` reports the temporary
worktree's `.git` pointer and two missing archived oracle manifests linked
from the pre-existing shared report; no S2-38o layout issue was found.
Independent Sol/high review is pending parent integration review.

## Scope limits

Generic, fragment, C ABI, JIT and native execution remain denied. Unmasked
exception delivery is represented only as the local interpreter exit described
above; no physical hardware behavior is asserted.

See the [CPU contract](../specs/CPU_CONTRACT.md#s2-38o--exact-minsd-unmasked-memory-exit-for-m64-rax), [Stage 2 plan](../plans/STAGE_2.md#s2-38o-exact-long64-minsd-xmm0m64-rax-memory-profile-with-mixed-imdm-exit), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-38o-minsd-m64-unmasked-invalid-and-denormal).
