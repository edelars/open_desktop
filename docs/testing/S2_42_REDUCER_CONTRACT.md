# S2-42f — proposed schema-v2 reducer contract

## Status and purpose

This is a documentation-only acceptance protocol for future bounded schema-v2
reducers. It defines the evidence needed to show that a smaller input preserves
the original architectural mismatch. It adds no executable reducer, fixture
fields, result fields, CLI command or backend admission. All new groups in the
coverage matrix below are **not implemented by this slice**; their reducer and
physical acceptance are `not_run`.

The parent [S2-42 plan](../plans/STAGE_2.md#s2-42-собрать-общий-corpus-reducer-и-harness)
remains open. The protocol makes its layout, branch, fault and partial-progress
requirements reviewable before an author implements a particular group. The
[fixture/result format](TEST_CASE_FORMAT.md) and [CPU contract](../specs/CPU_CONTRACT.md)
remain authoritative for existing fields and semantics. This document does not
define a new wire format.

## Existing implementation boundary

| Existing work | Implemented boundary | What it does not establish |
| --- | --- | --- |
| [`minimize_fixture`](../../crates/od-testkit/src/minimize.rs) | Schema-v1 `case::Fixture`; instruction removal and value reduction through a caller predicate; legacy JZ/JNZ target rewriting | Schema-v2 memory layout, generic branches, architectural fault or REP preservation. Its v1 target-remapping policy must not silently become the v2 policy. |
| [S2-42a/b](TEST_CASE_FORMAT.md#s2-42a-native-xorpd-generated-corpus) | Deterministic exact native-XORPD profile corpus and interpreter callback reducer over initial XMM bytes | Instruction deletion, memory reduction, FP fault or REP handling |
| [S2-42c](S2_42C_XORPD_CORPUS_HARNESS.md) | Strict saved XORPD corpus orchestration, repeated interpreter/native results and partial failure evidence | New profile admission or completed physical execution |
| [S2-42d](S2_42D_NATIVE_XORPD_REDUCER.md) | Native callback for the same XMM-only reducer and exact bytewise XOR oracle | A generic schema-v2 reducer or hardware acceptance for other groups |
| [S2-42l](S2_42L_REP_FAULT_REDUCER.md) | Exact interpreter `#GP(0)` after canonical48-boundary REP STOSB partial progress; AL-only reduction | Injected faults, invented linear fault addresses, native admission, or hardware acceptance |

In particular, XORPD's successful-stop predicate rejects every fault. A future
fault reducer needs its own predicate; changing the XORPD predicate to accept
an exception would weaken its existing contract.

## Ownership and prerequisites

Follow the [architecture](../ARCHITECTURE.md#5-устройство-собственного-движка)
and [project layout](../PROJECT_LAYOUT.md): proposal generation, group-specific
eligibility checks, comparison and reduction policy belong in `od-testkit`;
runner tools own backend execution, process bounds and evidence I/O. Production
CPU code must not depend on testkit. The reducer consumes the existing decoder
and fixture validators; it does not copy CPU execution semantics into a tool.
An independent expected-result oracle must not call the implementation helper
whose result it is checking.

Before enabling a group, record its exact fixture profile, instruction forms,
mode, feature/control prerequisites, comparison fields, independent reference,
backend capabilities, candidate operations and bounds. Reuse existing profile
admission. An unimplemented form or unavailable required backend is a failed
selected check; an unselected group is `not_run`. Local synthetic checks may
validate reducer orchestration while native acceptance remains `not_run`.

[S2-21, S2-39 and S2-40](../plans/STAGE_2.md) remain parent dependencies:

- S2-21 owns atomic/fence semantics and the required single-vCPU memory contract.
  Ordinary load/store reduction cannot stand in for an atomic operation.
- S2-39 owns FP/SIMD save/restore state and independent observation boundaries.
  An integer-only snapshot cannot prove preservation of omitted FP state.
- S2-40 owns the required ISA/profile inventory and uncovered mandatory forms.
  A reducer must not lower feature requirements to make a candidate executable.

A group with an already established bounded contract may be implemented before
these parents close. Such a local slice does not close S2-42 or the full selected
CPU profile. The [Stage 2 report](../reports/STAGE_2_REPORT.md) remains the source
of actual execution and review results.

## Preservation predicate

Every accepted candidate must satisfy both the immutable group contract and the
original mismatch fingerprint. Validation and a lower byte count alone are
insufficient. A group must supply the following checks before enabling a
candidate operation that can affect them.

| Property | Required preservation and rejection rule |
| --- | --- |
| Form and controls | Preserve the tested operation, operand/address widths, meaningful prefixes, operand roles, mode, feature requirements, privilege and FP control/status inputs unless that exact field is an explicitly reviewed reduction dimension. Never change the source profile to admit a candidate. |
| Code and layout | Default to fixed mapping bases, sizes, permissions, entry, stop boundary and code bytes. Code-changing reduction requires an explicit original-to-candidate instruction map, complete decoding of admitted code and a group-specific relocation policy. Keep code/data separation, aliases, alignment and page/canonical-span boundary conditions relevant to the mismatch. |
| Branches | Preserve each retained branch's destination instruction identity or explicit stop boundary, fallthrough relationship and admitted displacement width. Reject targets inside an instruction, overflow, unproved indirect targets and removal of a retained branch's target. Do not retarget a deleted target to the next instruction. |
| RIP-relative and address-bearing data | If code moves, update every admitted RIP-relative reference and every proven address-bearing field through the same map. Preserve the addressed data identity and boundary conditions. Reject relocation when any reference cannot be enumerated; do not guess that arbitrary integer data contains no address. |
| Memory and stack | Preserve mapping permissions, comparison scope/masks, relevant aliases, access widths/order, read/write/execute classification and stack update timing. Default to changing only explicitly selected data bytes. Region deletion, resizing, moving or pointer changes require a separate policy and independent evidence that the same tested access conditions remain. |
| Exit and fault | Preserve the ordered reference/subject exit classes, faulting instruction identity, vector, error-code presence/value and relevant fault-address/access-stage identity. Distinguish expected architectural exceptions from harness errors and host signals. Never turn an unmapped fixture access into an inferred guest `#PF`. |
| Partial progress | Preserve the specified successful iteration/operation count before the boundary, faulting operation's non-retirement, committed memory ranges, pending/unchanged state, count/index updates and flag definedness. Total retirement may change only by a justified mapped deletion of earlier instructions; record and check the resulting expected count. |
| Observability | Preserve the original comparison field set and masks. Do not hide a mismatch by dropping a memory region, narrowing a flags mask, making a defined bit undefined, omitting a snapshot, or replacing an observed field with `not_observed`. |

Each group's contract must state which exact values are immutable and which
values may vary while preserving a stated relation. For example, changing a
source data value may change expected stored bytes, but cannot change which
bytes were committed before a preserved fault. Expected values must be
recomputed independently for the candidate; observed subject values cannot be
used as the new expected values.

REP continuation deserves an explicit execution protocol. The existing
[S2-19i snapshot contract](TEST_CASE_FORMAT.md#s2-19i-tracked-rep-continuation-snapshots)
uses a sidecar and an intentional runner action to resume with tracked flag
definedness. Ordinary fixture execution starts fresh. A reducer cannot infer
that a saved state is a resumed invocation or manufacture missing continuation
state. If the needed boundary is not observable through the selected backend,
that reduction dimension remains unsupported.

## Candidate operations

Enable operations one group at a time; the following are eligibility rules,
not a list of currently implemented operations.

1. **Data/value simplification:** use a deterministic finite schedule over the
   reviewed register, immediate or memory-byte fields. Keep all other fixture
   fields fixed. Require strict schema revalidation and the preservation
   predicate for every candidate. A simpler NaN payload, for example, must not
   change the required signaling/quiet or exception class.
2. **Instruction deletion:** remove complete decoded instructions only after
   proving the retained entry, tested instruction, branch targets, references
   and stop boundary are represented by the relocation map. Reject a deletion
   with an unresolved reference or changed required instruction form. Re-decode
   the serialized candidate. Replacing an instruction with NOPs is a different
   transformation with different retirement semantics and requires its own
   acceptance checks.
3. **Memory trimming or relocation:** keep disabled by default. An enabled
   policy must prove retained access spans, canaries, aliases, comparison scope
   and fault boundaries remain observable and preserve the tested conditions.
   Removing bytes from comparison is not a reduction of the input.
4. **Count/control-flow simplification:** keep disabled for REP/loops until the
   protocol can preserve the required boundary and progress signature. A fault
   after one iteration is not interchangeable with the same vector after many
   committed iterations. A changed timeout or instruction limit cannot serve
   as the new mismatch.

Declare a strictly decreasing cost before the run, such as a lexicographic
tuple of instruction count, mutable input-byte count and Hamming weight.
Specify the traversal order, maximum proposals, actual backend-run budget and
wall-time/output limits. Count retries as actual runs. Reserve enough runs for
initial and final repeated confirmation; stop with the last confirmed input
when the candidate budget is exhausted. Do not claim a global minimum from a
bounded pass.

## Mismatch identity and execution protocol

The fingerprint is group-specific and versioned in the reduction evidence.
It binds the profile/contract identifier, comparison masks, ordered backend or
reference roles, preserved exit/fault/progress signature and exact differing
field/bit/byte locations. Address-bearing locations use the reviewed identity
map when relocation is enabled. A field-set match is insufficient if the
fault or progress signature changes. Wrong output values may be excluded from
the fingerprint only when the group explains why their values are not part of
the failure being preserved. Build timestamps and durations do not define
architectural equivalence; their provenance is still recorded.

1. Read the original with the fixture-size bound and strict existing schema.
   Preserve its exact accepted JSON bytes and SHA-256. Accepted whitespace or
   key order must not change the digest attached to original executions.
2. Establish the original mismatch with at least two semantically identical
   executions of each selected execution backend. An independent deterministic
   oracle supplies each candidate's expected result where that is the selected
   comparison method. No initial mismatch means no reduced fixture.
3. For every proposal, validate eligibility and serialize/reparse the fixture.
   Bind runs to that candidate's own digest. Require decreasing cost and two
   stable executions per selected backend, the same fingerprint and every
   preservation invariant. A synthetic callback is orchestration evidence only.
4. Reject invalid schema, decoder/admission rejection, `Unsupported`, timeout,
   host signal, malformed/truncated result, harness error, missing required
   observation and nonrepeatable results as mismatch-preserving candidates.
   Preserve available diagnostics. An infrastructure failure stops the run
   incomplete; it cannot be counted as a reproduced architectural defect.
5. An architectural `Exception` may be a valid reference or subject outcome
   only for a group with an explicit fault predicate. Likewise, an intentional
   REP work-limit or stop boundary requires its own progress predicate. A
   mismatch between success and a fault is a distinct ordered pair; it must not
   collapse into an unrelated fault/fault or timeout pair.
6. Re-run the final candidate with the same repeated confirmation and frozen
   comparison contract. Publish a reduced fixture only after confirmation.
   If final confirmation fails, keep partial evidence and report the reduction
   incomplete; never present an unconfirmed candidate as the result.

## Evidence and replay acceptance

Use an exclusive run directory under ignored `work/`; never replace earlier
stage evidence. Keep exact original/reduced inputs with hashes, all attempted
execution results and available partial output, independently expected results,
comparison masks, original/final fingerprint, candidate transformations and
instruction/address maps, rejection reasons, cost and budget counters. Each
record must identify its fixture digest, backend, repeat and execution status.

Record source/revision fingerprint, toolchain, build options, machine/backend
identity, profile and oracle/reducer versions, seeds and selected corpus
manifest digest where applicable. Preserve subprocess status and diagnostics
separately from guest outcomes. An interrupted run must retain the completed
records and identify missing attempts rather than manufacture success.

The replay record must contain exact arguments, working directory, relevant
environment and referenced artifact paths/digests for the existing command
that executes that fixture. Shell rendering must quote arguments correctly;
an argv record is preferable for machine replay. When a reducer entry point is
implemented, record its exact invocation too. This protocol introduces no
placeholder executable command. A different binary/source revision or missing
backend cannot be reported as the original confirmed replay.

Acceptance requires replaying both original and reduced fixtures from saved
artifacts, obtaining the same fingerprint with the same bounds and comparison
contract, and a machine-readable summary with a nonzero exit for any unavailable
required check. `no_initial_mismatch`, incomplete reduction, local orchestration
success and physical execution are distinct report states.

## Coverage matrix and completion gates

Each future implementation must expand the applicable row into concrete
handwritten cases and generated case IDs. The table lists required boundaries;
it does not assert that their corresponding CPU forms or native backends are
implemented.

| Group | Required independent boundaries | Reducer status in S2-42f |
| --- | --- | --- |
| Exact native XORPD | Zero/ones/alternating/equal operands, preserved other XMM and integer state, unchanged code/MXCSR; malformed/flaky/timeout rejection | Existing 42a–d only; no new acceptance here |
| Memory/addressing | Widths, signed displacements, aliases, unaligned and page/canonical spans, load/store order, canaries and partial-access failure policy for admitted forms | Future; `not_run` |
| Stack | Push/pop width, source/destination alias with stack/base registers, pre/post stack update, retained stack bytes and fault atomicity | Future; `not_run` |
| Branches/instruction deletion | Forward/backward/taken/untaken edges, target/end/entry retention, deleted-target rejection, displacement bounds, RIP-relative references and invalid interior targets | Future; `not_run` |
| REP | Zero count, DF directions, widths/address sizes, completion versus explicit stop/work limit, fault at first/later iteration, committed bytes and intentional resume/definedness | Future; `not_run` |
| FP and FP faults | Admitted operand classes and boundaries, rounding/masks/sticky status, gates and fault ordering, destination suppression, complete observed FP state, saved/restored state where required | S2-42o: bounded local-interpreter SQRTSS SNaN #XM reducer for one exact register form; native/physical `not_run`, broader FP acceptance open; depends on relevant S2-39 evidence |
| Atomic/fence | Admitted LOCK/form combinations, atomic access contract, alignment/fault ordering and single-vCPU side effects | Future; `not_run`; depends on relevant S2-21 evidence |
| Additional mandatory ISA | Forms and feature requirements established by S2-40 inventory, with an independent observation method | Future; `not_run` |

For every enabled group, the [S2-42](../plans/STAGE_2.md#s2-42-собрать-общий-corpus-reducer-и-harness)
acceptance still requires handwritten boundaries and at least 1000 distinct
valid generated cases across multiple fixed seeds, each run twice on
interpreter/native. A separately defined bounded set and its reason are required
for prohibited or unsafe combinations. Case count does not replace this matrix.
Missing physical execution stays `not_run` and prevents the corresponding full
acceptance; it does not invalidate honestly scoped local implementation work.

Reducer validation must also include deliberately different defect signatures,
changed fault class/progress, weakened masks, invalid candidates, missing
backends, unstable results, exhausted bounds, partial output and final-confirmation
failure. Those synthetic cases demonstrate the predicate rejects a different
failure; they do not establish a real CPU mismatch or native correctness.

The documentation slice is complete after independent review of these boundaries
against the existing contracts and local link/layout checks. Implementation
completion requires a separate executable slice and recorded results satisfying
the applicable rows. Parent S2-42 remains open until its dependencies and all
selected mandatory groups have the required evidence.

## Current coverage map — S2-42r/s/t/u/w update

The S2-42f snapshot above is retained as the historical contract-time view.
This table records the current implementation and evidence after S2-42q/r/s/t/u; it
does not rewrite the earlier snapshot. Local implementation evidence, native
execution evidence, and remaining scope are separate columns so a local pass
does not imply hardware acceptance.

| Group | Implemented coverage | Local evidence | Native/hardware evidence | Remaining scope |
|---|---|---|---|---|
| MOV memory/addressing | S2-42e: 1,024 generated cases; S2-42q: 56 static controls across four widths, load/store and seven admitted forms; combined strict acceptance and saved-run verifier | All 56 controls matched independent full snapshots twice (112 interpreter executions); S2-42e reports its 1,024 interpreter/oracle cases matched | Linux/native and physical hardware `not_run` for the S2-42q selected set | Execute all 1,080 cases twice on required native backend; complete broader S2-42 profile |
| XORPD | S2-42a/c and S2-42p: 1,024 generated plus six pinned handwritten controls and bounded reducer work | Handwritten local interpreter controls passed twice; see slice report | Linux/native and physical hardware `not_run` in the current evidence | Required native execution and broader S2-42 profile |
| Integer stack and branches | S2-42g/h stack reducers/corpus; S2-42k forward JZ/JNZ reducer; S2-42r JZ/JNZ rel8 corpus with 32 controls and 1,024 generated inputs | S2-42r: all 1,056 cases passed twice; 1,024 distinct architectural inputs and synthetic saved-run debug/release roundtrips verified | Actual native/physical `not_run` | Cross-group execution, rel32/indirect/fault branches, and remaining mandatory forms |
| Positive REP and continuation | S2-42i positive STOSB corpus; S2-42s checkpoint/replay for counts 1024/1025/2048/2049 and DF=0/1 | S2-42s passed 8×2 executions across 32 phases, exact-once write journal, serialized replay, defined-flags comparison | Native/physical `not_run` | Other REP widths/address sizes, fault/restart cases, and required native corpus remain open |
| REP faults and FP | S2-42l partial-progress fault reducer; S2-42o one bounded SQRTSS SNaN fault reducer | Exact local fault/reducer evidence is scoped to each slice | Native/physical `not_run`; broader FP acceptance remains open | Broader fault, partial-progress, FP state and save/restore requirements |
| Atomic/fence and additional mandatory ISA | S2-42t local LOCK XADD qword [RBX],RAX; S2-42u bare qword memory CMPXCHG; S2-42w bare qword memory XCHG 48 87 00 at the S2-21a split boundary | S2-42t's 1,056, S2-42u's 1,026, and S2-42w's 1,026 inputs each passed twice locally with independent full-state oracles and strict saved replay; 42u/w preserve cwd/environment and interruption evidence | Native requests explicitly blocked before backend; actual native and physical acceptance not_run | S2-21 atomic contract, ISSUE-017 LOCK questions, and S2-40 inventory remain open; these local corpora do not establish SMP/hardware atomicity or general memory ordering |

Parent S2-42 remains open, as do dependencies S2-21, S2-39 and S2-40. Physical
hardware acceptance is `not_run`. S2-42q closes the handwritten-boundary gap
for already admitted MOV memory/addressing forms; S2-42r adds a bounded rel8
branch corpus; and S2-42s adds local REP work-limit checkpoint/replay evidence.
S2-42t adds local private-RAM LOCK XADD arithmetic coverage while explicitly
preserving native denial; S2-42u adds a local CMPXCHG split-region corpus;
S2-42w adds a separate XCHG split-region corpus that preserves every RFLAGS bit.
Both explicitly deny native execution. None closes the parent reducer/corpus
requirement.
