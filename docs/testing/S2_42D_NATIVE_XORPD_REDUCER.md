# S2-42d: bounded native XORPD reducer/replay

## Scope

This slice adds an opt-in native callback for the existing exact
`s2-long-native-xorpd-xmm0-xmm1-v1` fixture profile. The existing
`od-runner --reduce-native-xorpd` path continues using the interpreter. The new
command is `od-x86-runner --reduce-native-xorpd CASE.json [--limit N]
[--output-dir work/stage2/s2-42d-xorpd/NAME]`.

The `od-testkit` reducer owns the deterministic proposal schedule over initial
XMM0–XMM15, independent bytewise XOR oracle check, exact mismatching
register/byte-position fingerprint and acceptance predicate. The x86 runner
owns execution and calls the existing native-v2 executor directly for every
callback. This retains the same strict schema validation, execution admission,
supervised tracee, native observations and result validation as ordinary native
fixture execution. Each proposed candidate is serialized and strict-schema
parsed before execution.

## Acceptance and bounds

The default and maximum budget is 5120 native runs; the minimum is 4. Two
initial runs and two final confirmations are reserved; each candidate uses two
runs. A candidate only passes if both executions are semantically stable, the
exact initial mismatch-position fingerprint remains, the independent bytewise
XOR oracle still identifies every other architectural field as correct, and
XMM Hamming weight decreases. One deterministic pass is performed; no global
minimum is claimed. Only initial XMM values may change. Profile, instruction
bytes, memory layout, mode, feature state, MXCSR, all GPR/RFLAGS/RIP and other
fixture fields remain fixed under strict validation.

No initial mismatch yields `no_initial_mismatch` and no reduced fixture.
Unavailable native execution, faults, timeouts, malformed results, callback
errors, non-repeatable results and any architectural state drift cannot preserve
a mismatch or publish a reduced fixture.

## Evidence and replay

Output is exclusive below ignored `work/stage2/s2-42d-xorpd/`. It contains the
original fixture, two initial native results, final results when run, optional
reduced fixture, and a report with fixture/result SHA-256 values, the independent
oracle XMM0 value, initial/final fingerprints, stop reason, budget/proposal/run/
acceptance progress, provenance and exact replay commands. `no_initial_mismatch`
never writes `reduced.case.json`.

Replay a saved reduced fixture with the existing comparison path:

```sh
od-runner --fixture work/stage2/s2-42d-xorpd/NAME/reduced.case.json --engine compare --json
```

Synthetic reducer callbacks are orchestration checks only. The S2-42d build and
test slice does not claim native Linux hardware or physical Mac mini execution.
Parent S2-42 remains open for S2-21/S2-39/S2-40 dependencies and other required
corpus groups and hardware comparison.
