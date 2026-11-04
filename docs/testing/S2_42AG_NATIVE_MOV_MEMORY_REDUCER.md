# S2-42ag — supervised native MOV qword memory reducer

## Scope

This slice adds a native callback adapter for the existing S2-42n MOV memory
reducer. It accepts one schema-v2 `s2-long-integer-v1` Long64 instruction:
`48 89 03` (`MOV qword ptr [RBX], RAX`). The reducer still varies RAX only;
it reuses the independent byte-store oracle, preservation fingerprint, repeat
checks and bounded callback schedule from `od-testkit`. The interpreter API
and native-v2 admission profile are unchanged.

Each native callback runs the existing native-v2 fixture child under the shared
supervisor, including its watchdog, resource limits and normalized timeout,
signal and harness-error outcomes. The callback journal records backend
identity and persists candidate, expected result and observed result before
the reducer can accept a candidate. A wrong backend, incomplete snapshot,
drift, flaky result, callback failure, budget exhaustion or failed final
confirmation cannot publish a reduced fixture.

## Invocation and evidence

```sh
cargo run --offline -p od-x86-runner -- \
  --reduce-native-mov-memory \
  tests/fixtures/x86/stage2/s2_42n_mov_qword_rbx_rax.case.json \
  [--limit N] \
  [--output-dir work/stage2/s2-42ag-native-mov-memory/<run-id>]
```

The default callback budget is 132; the accepted range is 4–132. The output
directory must be fresh and remain under
`work/stage2/s2-42ag-native-mov-memory/`. It contains the exact original input,
the independent expected result, callback journal and result files, reducer
report, hashes, provenance, and replay command vectors. Replay uses the saved
`original.case.json` and a new output destination.

## Linux validation on 2026-11-04

The current three-file source patch was applied to the confirmed Linux x86_64
checkout after its `native_v2.rs` and `native_fp.rs` inputs matched the local
base byte-for-byte. The resulting three files matched the local working tree
by SHA-256 before tests and execution. The Linux native test group passed with
220 tests and 0 failures (19 filtered); the focused child-dispatch test passed
1/1. The local `od-testkit` reducer tests passed 5/5. Formatting and Linux
x86_64 target checks also passed. Commands, counts and source hashes are in the
[verification record](../../work/stage2/s2-42ag-native-mov-memory/linux-smoke-20261104/verification.json).
The Linux binary reports its build revision/compiler as unavailable, so the
record binds validation to the checked source hashes rather than an embedded
revision claim.

A bounded smoke using the literal S2-42n fixture returned
`no_initial_mismatch` after two supervised native callbacks. Both callback
records identify `native_x86`, end at `stop_address`, and have identical result
SHA-256 `896e6a841977c4d9c222ef0f270f2ea6185728c04175428a0dd66a0fb5be7931`.
Replaying the saved original fixture through the recorded fixture path also
returned `native_x86` / `stop_address` with exit 0. No CPU mismatch was found.
This literal fixture had no initial mismatch: native candidate proposals and
pairs were zero, and no reduced fixture was produced. Native reduction of an
actual mismatching fixture therefore remains unverified; synthetic reducer
tests do not replace that acceptance.

Ignored evidence is under
`work/stage2/s2-42ag-native-mov-memory/linux-smoke-20261104/`. Its reducer
report SHA-256 is
`3a5f0c80355212e415dba3400c03d1139aae6168830de6522016f241d6acbc97`;
the exact saved-original replay result is
`02dc778762a60ef727a19fbf5bb4dd79a4b6998f90b7636ec2d8794397fc7a81`.

This validates one bounded native reducer path; it does not demonstrate a CPU
defect, close parent S2-42, cover the other lettered suites, or establish
physical Mac acceptance. S2-21, S2-39 and S2-40 remain open.

## Synthetic callback orchestration coverage

The testkit also exercises the native adapter with synthetic callbacks whose
`RunResult.backend` is `native_x86`. These tests inject one stable wrong byte
inside the MOV destination qword, then verify the reducer reaches `Reduced`,
proposes and runs candidate pairs, lowers RAX Hamming weight, preserves the
initial/final mismatch fingerprint, and leaves the fixture's memory and other
architectural inputs unchanged. Separate negative cases return an
interpreter-labelled result during candidate evaluation and final confirmation;
both fail closed without publishing a reduced fixture.

This is synthetic orchestration coverage only. The callback does not execute
native x86, and the test does not establish physical/native Linux mismatch
shrinking. The literal supervised Linux run above still found no initial
mismatch, so an actual native mismatch reduction remains unverified.
The focused testkit reducer group now passes 7/7; its command and updated
source hash are recorded in the linked verification record.
