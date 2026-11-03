# S2-38aa — SQRTPS m128 unmasked memory profile

## Implemented

Added the immutable schema-v2 profile `s2-long-sqrtps-memory-rax-unmasked-v1`
for exact Long64 `0F 51 00` (`SQRTPS XMM0,m128 [RAX]`). It requires SSE
(`CPUID.01H:EDX[25]`), XMM/MXCSR state, one executable code region and one
read-only 16-byte data region. The instruction budget is one and the stop RIP
is entry+3. Full instruction fetch and exact-form checks precede feature and
control gates. SSE/EM/OSFXSR rejection raises `#UD`; TS raises `#NM` after
those checks.

The profile accepts supported MXCSR bits under mask `0xffbf`, requires DAZ=0
and reserved bits clear, and admits any FTZ, exception masks, rounding mode,
and prior sticky flags. It requires 16-byte alignment for either AC value,
validates the canonical 16-byte span, and performs one read16. Fetch, gate,
alignment, span and memory-read failures do not partially commit architectural
state.

The four raw binary32 lanes are evaluated before exception masking. IE and DE
are aggregated first. Any unmasked IE or DE commits the aggregate IE|DE flags,
suppresses PE and the complete XMM0/RIP/retirement update, then raises `#XM`
when OSXMMEXCPT=1 or arithmetic `#UD` otherwise. If pre-events are masked, PE
is evaluated and an unmasked PE commits IE|DE|PE while suppressing the whole
instruction result. Successful execution commits all four result lanes,
aggregate status flags and RIP+3.

The existing masked-only S2-37dd memory profile and S2-37ak register profile
remain separate. JIT and native backends reject this profile.

## Verification

- `cargo test -p od-isa --offline`: 1,951 passed. This includes a direct
  10,000-case matrix of 625 neutral/IE/DE-only/PE-only/DE+PE lane products,
  all IM/DM/PM mask combinations and both OSXMMEXCPT values, plus all four RC
  modes crossed with 64 prior sticky-state combinations. An additional
  64-case set places positive/negative QNaN and SNaN in each lane, under both
  IM states and both OSXMMEXCPT mappings.
- `cargo test -p od-testkit --offline`: 456 passed. The independent integer
  raw-u32 SQRTPS oracle separately covers the same event products, mask
  combinations, OSXMMEXCPT mapping, RC modes, sticky histories and signed
  QNaN/SNaN per lane.
- `cargo test -p od-runner --offline`: 132 unit tests and 556 fixture tests
  passed. Seven S2-38aa literal fixtures cover success and unmasked IE/DE/PE
  under both exception mappings; 64 generated end-to-end cases cover signed
  QNaN/SNaN in each lane with IM masked/unmasked and both exception mappings.
  Tampering checks reject altered XMM0, RIP, IE/DE/PE flags, retirement,
  control echoes, exception vectors and `#GP(0)` error codes. Read-only
  permissions and interpreter-only backend denial are also checked.
- A focused recheck also covers an unmasked IE source behind an earlier SSE
  gate, misalignment `#GP(0)`, and noncanonical-span `#GP(0)`; all preserve the
  original MXCSR, XMM0 and RIP. The executor read-error rollback test likewise
  confirms no arithmetic flags are committed when the single read16 fails.
- `cargo check --workspace --all-targets --offline`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed. `graphify update .`
  passed. `python3 tools/check_layout.py` reports the worktree `.git` file as
  unmapped and two pre-existing broken local manifest links in
  `docs/reports/STAGE_2_REPORT.md`; it reports no S2-38aa layout issue.

No physical x86 acceptance is claimed. The checked behavior is the bounded
interpreter and its schema-v2 fixture contract.

See the [CPU contract](../specs/CPU_CONTRACT.md#s2-38aa--exact-sqrtps-memory-profile-with-unmasked-exceptions),
[stage plan](../plans/STAGE_2.md#s2-38aa--unmasked-outcomes-for-exact-sqrtps-memory-form),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md).
