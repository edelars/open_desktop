# S2-38aj — unmasked DIVSD m64 memory profile

## Scope

Implemented the separate schema-v2 interpreter profile
`s2-long-divsd-memory-unmasked-rax-v1` for exact Long64 `F2 0F 5E 00`
(`DIVSD XMM0,m64 [RAX]`). It requires SSE2 bit 26, one read8 from read-only
memory, budget one, and stop RIP at entry+4. Existing masked-memory S2-37dp
and register S2-37ax profiles remain distinct. Native and JIT execution are
denied for this profile.

The profile admits supported MXCSR state with reserved bits clear,
`DAZ=FTZ=0`, and `OM=UM=1`; all rounding modes, sticky histories, IM/DM/ZM/PM
masks, and OSXMMEXCPT routes are represented in immutable profile/control
snapshots. Gate, CPL, AC, canonical-span, and read ordering follows the S2-38aj
contract. A successful execution reads the actual m64 operand once.

## Arithmetic and exception behavior

The executor preserves original-operand NaN selection and quieting. SNaN,
`0/0`, and `∞/∞` raise IE; QNaN alone does not. Pre-events are classified in
IE, ZE, DE order. A finite nonzero value, including a subnormal, divided by
signed zero raises ZE; `∞/0` does not. NaNs suppress same-operation ZE/DE.
Unmasked pre-events commit applicable new sticky bits and suppress later
OE/UE/PE, destination, RIP, and retirement. With pre-events masked, rounding
and WPR tininess-after-rounding determine PE/UE; unmasked PE suppresses the
full destination and retirement. OSXMMEXCPT selects #XM vector 19 or
SIMD-generated #UD vector 6, without an error code.

The testkit oracle is integer-only raw-u64 rational arithmetic and takes its
right operand from the actual fixture memory. The in-process interpreter
matrix uses 20 operand/event vectors across all 4 RC × 64 prior sticky states
× 16 IM/DM/ZM/PM settings × 2 OSXMMEXCPT settings (163,840 executions),
checking each result and status against that oracle. The vectors include both
signed SNaNs in either position, QNaN suppression, IE/ZE/DE combinations,
exact and inexact ratios, exact tiny, half-minsub, a min-normal boundary,
overflow, DE suppression of later PE/OE, and a masked-pre-event path to
unmasked PE.

Five handwritten literals cover success and IE/ZE/DE/PE delivery. Direct ISA
and runner tests also check event status, state suppression, gate order,
malformed exact form, CPL0–3, AC before read, canonical code/data spans, one
read8, rollback, control/status/exception/destination/retirement tampering,
strict permissions, and backend denial. Existing masked permission behavior
remains covered by S2-37dp tests.

## Validation

Validation passed in the isolated worktree: `cargo test -p od-isa -p
od-testkit -p od-runner --offline` (1,963 ISA, 461 testkit, 140 runner unit,
and 563 fixture tests), `cargo check --workspace --all-targets --offline`,
`cargo fmt --all -- --check`, `git diff --check`, and the 163,840-case focused
actual-interpreter matrix. `graphify update .` refreshed the worktree graph to
12,550 nodes. `python3 tools/check_layout.py` passes in the unchanged main
checkout; in the linked worktree it reports the `.git` pointer as an unmapped
root file and flags two pre-existing links to ignored `work/` manifests.
Independent Sol/high review found no remaining defects in ISA semantics,
gates/rollback, strict validation/oracle, dispatch, fixtures, or docs. The
reviewer's full-matrix attempt timed out after 60 seconds without a failure;
the successful 163,840-case matrix recorded above is our run. This report
records host-side interpreter evidence only; it does not claim physical x86
acceptance.
