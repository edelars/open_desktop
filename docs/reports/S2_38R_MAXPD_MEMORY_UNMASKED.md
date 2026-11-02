# S2-38r — MAXPD XMM0,m128 [RAX] with unmasked IM/DM exits

The dedicated schema-v2 profile `s2-long-maxpd-memory-unmasked-rax-v1`
implements exact Long64 `66 0F 5F 00`. It has its own unmasked MAXPD memory
route and remains separate from masked S2-37cz memory MAXPD and unmasked
S2-38j register MAXPD. Admission requires SSE2, CR0.EM=0, CR4.OSFXSR=1, and
explicit CR0.TS/CR4.OSXMMEXCPT controls. The schema rejects missing or foreign
controls and admits only one executable code mapping plus the 16-byte data
mapping at RAX. Execution remains interpreter-only; JIT/native, C ABI, generic,
and fragment routes are closed.

The interpreter fetches all four instruction bytes and rejects non-exact forms
before SSE2/EM/OSFXSR #UD and TS #NM gates. It accepts supported MXCSR bits
`0xffbf`, requires reserved bits zero, DAZ/FTZ off and ZE/OE/UE/PE masked, while
IM/DM/RC and prior sticky flags vary. RAX must be 16-byte aligned for either
RFLAGS.AC value. A checked canonical48 `[RAX,RAX+15]` span precedes exactly one
all-or-error 16-byte read. Alignment/span faults make no data-memory callback;
read failure occurs before operand classification and leaves CPU state
unchanged.

Both binary64 lanes use raw MAXPD behavior with the actual memory operand as
source 2. Ties and NaNs select the raw memory bits, including SNaN payloads.
IE and DE are collected across lanes before unmasked exception delivery. An
unmasked event commits all raw sticky events and reports #XM or arithmetic #UD
from OSXMMEXCPT while preserving the full XMM0, RIP, and retirement count. A
masked completion suppresses DE only in a lane that also raised IE and commits
the full result and status together.

The independent fixture oracle classifies and orders values using raw integer
bits. Generated inputs cover all lane event-class pairs crossed with IM, DM,
OSXMMEXCPT and AC=0/1, MAX-versus-MIN discrimination, poisoned XMM1, NaN payload
and signed-zero ties, and each RC crossed with all 64 old sticky histories while new IE+DE events
are raised. Named literal #XM and arithmetic-#UD fixtures cover both
OSXMMEXCPT paths. Direct ISA checks cover exact-form and truncated-fetch order,
invalid MXCSR and absent SSE refusal, memory fault precedence, and callback
counts. Fixture validation rejects missing controls, altered exact forms,
forged control echoes, and JIT/native results presented as executed results. The
native-local-only classifier is explicitly checked for this profile.

## Verification

`od-isa` passed 1,913 tests; `od-testkit` passed 455 tests; the full runner
`fixture_v2` suite passed 527 tests. The S2-38r fixture matrix test passed 524
generated cases. `cargo fmt --all -- --check` and
`cargo check --workspace --all-targets --offline` passed. Graphify refreshed to
11,938 nodes, 25,934 edges, and 373 communities. `tools/check_layout.py` could
not pass in the isolated worktree: it flags the normal `.git` worktree pointer
file and two pre-existing report links to ignored manifests absent from this
worktree. `git diff --check` passed. Physical x86 acceptance is not claimed.

See the [CPU contract](../specs/CPU_CONTRACT.md#s2-38r--exact-maxpd-memory-unmasked-imdm)
and [Stage 2 plan](../plans/STAGE_2.md#s2-38r--exact-long64-maxpd-xmm0m128-rax-with-unmasked-imdm-exits).
