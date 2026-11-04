# S2-42l — REP STOSB canonical48 boundary fault reducer

## Status and scope

This slice adds a bounded interpreter-only reducer for an exact `F3 AA`
schema-v2 case in `s2-long-integer-v1`. The mapped destination ends at the
canonical48 upper boundary. REP commits `k` byte stores and progress before the
next destination is noncanonical and raises architectural `#GP(0)`.

The implementation changes no CPU behavior, fixture schema, or backend
admission. It excludes injected faults and requires the literal architectural
exception. Native and physical execution remain `not_run`; native admission
does not accept this high-boundary mapping.

This follows the fault-specific predicate in the
[S2-42f reducer contract](S2_42_REDUCER_CONTRACT.md) and the REP progress
contract in [the Stage 2 plan](../plans/STAGE_2.md).

## Invocation

```sh
cargo run -p od-runner --offline -- \
  --reduce-rep-fault tests/fixtures/x86/stage2/s2_42l_rep_stosb_canonical48_gp.case.json \
  [--limit N] [--output-dir work/stage2/s2-42l-rep-fault/<run-id>]
```

The default callback budget is 20; the accepted range is 4 through 20. The
maximum covers two initial callbacks, two callbacks for each of up to eight
proposals, and two final confirmations. A callback is a complete interpreter
execution. Output must be a new child of `work/stage2/s2-42l-rep-fault/`; the
runner preserves the exact source JSON bytes and digest.

## Admission and independent oracle

The reducer requires Long64 and the existing `s2-long-integer-v1` profile,
exact `F3 AA` at entry, a one-instruction budget, `stop_rip=entry+2`, expected
exception, DF clear, `1 <= k <= 7`, and `k < RCX <= 8`. Memory is exactly an
execute-only two-byte code region and a fully observed RW data region of `k` bytes. Data
starts at `L-k+1`, where `L=0x0000_7fff_ffff_ffff`, and initially contains only
`FF`. Initial RDI equals the data start; AL is in `00..FE`.

The oracle independently predicts exactly `k` committed stores, `RCX-k`,
`RDI=L+1`, faulting RIP still at entry, and zero retired instructions. It
requires vector 13, error code 0, and an unavailable fault address. It does not
derive or record a linear fault address for `#GP`. All other integer state,
flags, code bytes, and layout must match exactly.

Only AL can change. The schedule clears set bits in deterministic order 7 down
to 0; high RAX bits, code, flags, count, address, and mappings stay fixed. The
mismatch fingerprint is the exact list of destination byte offsets whose
observed values differ from AL while all fault-class and progress checks pass.
Two semantically identical results are required for the original, every
candidate, and the final candidate. Invalid, unmapped, timed out, unsupported,
wrong-fault, or progress-drift results cannot preserve a mismatch.

## Evidence and verification

The output directory contains the original bytes, independent expected result,
every callback result and journal record, distinct candidates, reduced fixture
and expected result when available, hashes, stop reason, budget counters, and
replay argv/commands. Each callback record is written as that callback
completes. Native and physical status are explicitly `not_run`.

The committed literal fixture uses `k=2`, `RCX=3`, initial `RDI=0x0000_7fff_ffff_fffe`,
and `AL=FE`. A real `od-runner --fixture ... --engine interpreter --json`
execution produced `#GP(0)` at the entry RIP, no linear fault address, `RCX=1`,
`RDI=0x0000_8000_0000_0000`, no retirement, and the two bytes `FE FE`.

The integration test also supplies a synthetic callback that changes one
destination byte while preserving the exact exception and progress. It checks
that the reducer keeps that precise mismatch through AL-only reduction. This
synthetic callback is reducer evidence, not evidence of a CPU defect. A normal
CLI reducer run on the literal fixture currently reports `no_initial_mismatch`.
