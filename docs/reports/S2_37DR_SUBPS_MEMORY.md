# S2-37dr — SUBPS XMM0,m128 [RAX]

## Implemented boundary

This slice adds one exact legacy encoding, `0F 5C 00`, for
`SUBPS XMM0,m128 [RAX]`. It is a three-byte, one-retirement,
interpreter-only schema-v2 profile. SSE availability is required; complete
instruction fetch precedes #UD/#NM admission. The accepted MXCSR domain is
`0xffbf` with all six exception masks enabled, DAZ/FTZ and reserved bits clear.
All RC settings and all prior sticky combinations are accepted.

RAX must be 16-byte aligned regardless of AC. The implementation checks the
canonical48 span `[RAX,RAX+15]` before issuing exactly one `read16`. Rejected
controls, alignment/span failures and memory faults do not commit architectural
state. Four memory lanes are subtracted from the old XMM0 lanes with
`Binary32::subtract_raw_result`; raised IE/DE/OE/PE are combined across lanes
and with prior sticky status. Other state and memory are preserved. Native x86,
JIT, C ABI, generic and fragment paths remain denied. The fixture schema uses
an independent integer-only `independent_subps` oracle with the actual memory
operand.

## Arithmetic coverage and UE rationale

The runner's baseline case crosses four RC modes by all 64 prior sticky masks.
Three additional memory-specific vectors each cross that same matrix:

- QNaN/SNaN ordering with signs and payloads, plus opposite infinities.
- Mixed lanes raising IE, DE, OE and PE, with explicit output assertions and
  sticky aggregation assertions.
- A half-ULP tie, both zero-sign combinations, and the normal/subnormal
  boundary. It asserts RC-sensitive tie results and the lane results/status.

The mixed-lane case checks IE|DE|OE|PE (`0x2b`) across lanes and verifies that
incoming sticky bits remain set. No finite binary32 subtraction vector can
newly raise UE. For finite same-sign operands, a subnormal exact difference
falls in the operands' magnitude range where Sterbenz's lemma makes subtraction
exact. Opposite-sign operands add magnitudes; if their result is subnormal,
both operands must be subnormal and their values share the binary32
minimum-subnormal quantum, so the sum is exact. Therefore the S2-37dr arithmetic domain has no
finite tiny-and-inexact case; the matrix explicitly checks preservation of an
incoming UE sticky bit. This is a domain rationale, not a claim that the
underlying exception mechanism is untested elsewhere.

Direct ISA tests cover decoder exactness, full fetch and SSE/control gates,
invalid MXCSR, mandatory alignment and canonical-span checks, no-read/no-commit
failures, and read16 fault rollback. Fixture validation checks output payload,
memory, control echo and provenance tampering; backend relabeling and execution
are denied.

## Verification

- Focused runner test `subps_m128_memory_uses_independent_memory_lanes_and_denies_backends`: passed (1/1) after vector expansion.
- Full sequential `cargo test -p od-runner --test fixture_v2 --offline`: passed (498/498, 63.46s).
- Previously observed transient failures were rerun in isolation and passed: `cmppd_m128_memory_covers_predicates_lanes_and_independent_exceptions` (1/1) and `cmpsd_register_pairs_predicates_raw_classes_exceptions_and_denial` (1/1).
- The prior slice verification passed: od-isa 1,840; od-testkit 455; runner unit tests 119; x86-runner library 8 and binary 4; workspace all-target offline check; fmt; diff check. Final follow-up verification repeated workspace all-target offline check, fmt and diff check; Graphify update reported 11,488 nodes, 24,739 edges and 359 communities.
- The isolated worktree layout check reports its `.git` marker as unmapped and two historical oracle manifest references from `STAGE_2_REPORT.md` as missing. This slice added no paths. The main checkout is unaffected.

The full fixture rerun is sequential. No physical x86 execution or unmasked
#XM delivery is claimed.
