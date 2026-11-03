# S2-38ak — ADDPS m128 unmasked memory profile

## Implemented

Added immutable schema-v2 profile `s2-long-addps-memory-unmasked-rax-v1` for
exact Long64 `0F 58 00` (`ADDPS XMM0,m128 [RAX]`). It requires SSE, one
executable code mapping and one read-only m128 mapping, budget one, and stop
RIP=entry+3. The new `addps_memory_rax_unmasked_controls` echo is separate from
the existing masked-memory and register controls.

All three instruction bytes are fetched and the exact form is checked before
SSE/CR0.EM/CR4.OSFXSR `#UD` and CR0.TS `#NM`. Noncanonical fetch spans take
`#GP(0)` first. The profile requires MXCSR reserved bits clear, supported mask
`0xffbf`, DAZ=FTZ=0, and OM=UM=1; RC, IM/DM/ZM/PM, and old sticky histories may
vary. The m128 operand requires alignment16 regardless of AC. After alignment,
the executor validates both canonical endpoints and performs one read16.

The four lanes use pre-execution XMM0 and the actual memory RHS. IE/DE are
aggregated before mask handling; a NaN suppresses DE only in its own lane, a
signaling NaN raises IE, and opposite-sign infinities raise IE. Unmasked IE/DE
commits only the applicable pre-event sticky flags and suppresses all later
status, XMM0, RIP, and retirement changes. Otherwise all lanes are computed;
unmasked PE commits status but suppresses the whole instruction result. The
profile delivers arithmetic exceptions as `#XM` vector 19 or SIMD-generated
`#UD` vector 6 according to OSXMMEXCPT. Existing S2-37dq masked-memory and
S2-37ba register profiles remain separate. JIT, native, C ABI, generic, and
fragment routes remain closed.

## Verification

- `cargo test -p od-isa --offline --quiet`: 1,965 passed.
- `cargo test -p od-testkit --offline --quiet`: 461 passed.
- `cargo test -p od-runner --offline --quiet`: 146 unit tests and 563 fixture
  tests passed. This includes the exact `6^4 × 16 × 2` lane/mask/delivery
  matrix (41,472 actual interpreter executions) and 46 cross-lane/boundary
  vectors across RC × 64 old sticky histories × 16 masks × both delivery
  mappings (376,832 executions), each checked by the independent raw-u32
  oracle using the actual memory RHS.
- Six handwritten fixtures cover success, IE, DE, PE, cross-lane suppression,
  and noncanonical-fetch `#GP(0)` precedence over disabled feature/control
  gates. Focused ISA checks cover gate order, alignment at AC=0/1, noncanonical
  data spans, read16 rollback and no callback on refusal. Runner tampering
  checks reject altered result lanes, MXCSR, retirement, control echo,
  exception vector, and non-interpreter backend admission. Existing masked
  ADDPS fixtures continue to pass.
- Focused negative coverage rejects unknown/missing schema data, invalid MXCSR,
  writable source mappings and non-exact bytes; direct ISA probes cover CPL0–3,
  all SSE/CR gate priorities, full/truncated/noncanonical fetch, injected fetch
  and data read failures, both AC values, and no-callback rollback.
- `cargo check --workspace --all-targets --offline`, `cargo fmt --all -- --check`,
  and `git diff --check` passed. `python3 tools/check_layout.py` found only the
  linked-worktree `.git` file and two pre-existing missing oracle manifest links
  in `STAGE_2_REPORT.md`; it reported no new S2-38ak layout issue.
- `graphify update .` passed: 12,544 nodes, 27,280 edges, 385 communities.
  Independent Sol/high review is clean after corrections to validator fetch
  precedence and matrix permutation/mask/midpoint coverage.

No physical x86 acceptance is claimed. Sources: Intel SDM Rev. 093 Vol. 2A
ADDPS pp. 3-19–3-21 and §2.5.2/Table 2-19; Vol. 1 §§4.9.1.4–.6, 4.9.2,
11.5.1–11.5.3; Vol. 3A Event 19/Table 7-8.

See the [CPU contract](../specs/CPU_CONTRACT.md#s2-38ak--exact-addps-memory-profile-with-unmasked-exceptions),
[stage plan](../plans/STAGE_2.md), and [fixture notes](../../tests/fixtures/x86/stage2/README.md).
