# S2-42j — native-v2 qword PUSH/POP reducer

This slice adds a native-v2 callback adapter for the bounded S2-42g reducer.
The reducer's admitted input remains the exact `s2-long-integer-v1` qword GPR
`PUSH`/`POP` profile. Only the selected PUSH source value or the eight bytes
read by POP can change. Instruction bytes, registers outside that value, memory
layout, flags, and execution limits remain fixed.

## Invocation

On Linux x86_64, build and run:

```sh
cargo run -p od-x86-runner --release -- --reduce-native-push-pop CASE.json
```

Optional flags are `--limit N` (bounded by the reducer's existing maximum) and
`--output-dir DIR`. The default evidence directory is
`work/stage2/s2-42j-native-stack/<fixture-stem>`. Explicit output directories
must be new children of `work/stage2/s2-42j-native-stack/`; existing paths and
symlink traversal are rejected. Native callbacks use the existing native-v2
supervisor, admission checks, and disposable tracee. No CPU semantics or native
admission rules were changed in this slice.

The public interpreter API remains
`reduce_stack_fixture_with_input_digest`. Its native counterpart is
`reduce_stack_fixture_with_native_input_digest`; both call one private reducer
whose `expected_backend` is checked by every initial, candidate, and final
fingerprint validation. A wrong backend, unavailable/unsupported run, mapping
failure, timeout, malformed callback result, or callback error cannot preserve
a mismatch.

## Evidence and replay

The report records the original and reduced fixture digests, initial and final
result digests, mismatch fingerprints, callback counts, provenance, build and
machine identity, the canonical runner executable, working directory, and
quoted argument vectors for the reducer and fixture replay commands. The exact
source JSON bytes are saved as `original.case.json`; callbacks on the unchanged
fixture reuse those bytes and their raw SHA-256. Mutated candidates are strict
schema-v2 round-tripped and saved under `callbacks/candidates/` by digest.

Every callback invocation has a `callbacks/run-NNNNNN.record.json` record with
sequence, candidate digest/path, repeat number, native backend, outcome/status,
and result digest/path when a result was produced. Serialized callback results
are retained as `callbacks/run-NNNNNN.result.json`. This journal includes the
two initial calls, every reducer candidate call (including repeats), and final
confirmation calls; `initial-*.result.json` and `final-*.result.json` are also
written as summary artifacts. A reduced fixture and independent expected
results are present only when the reducer confirms a stable mismatch.

Native execution is `not_run` on non-Linux/non-x86_64 hosts, including the
physical Mac mini. A Linux target compile check does not count as hardware
execution or physical acceptance.
