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
owns execution and calls the shared `run_supervised_fixture` path for every
callback. Each candidate is serialized to the output's
`callbacks/candidates/<sha256>.case.json` before dispatch to the supervised
native-v2 child. This retains the same strict schema validation, execution
admission, supervised tracee, native observations and result validation as
ordinary native fixture execution. The original input bytes are saved and
supervised as `original.case.json`.

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
a mismatch or publish a reduced fixture. After writing the evidence and report,
the CLI returns success for `Reduced`, `NoReduction`, and `NoInitialMismatch`;
all incomplete or failed stops return exit code 5. Input and output setup errors
still return exit code 3.

## Evidence and replay

Output is exclusive below ignored `work/stage2/s2-42d-xorpd/`. It contains the
original fixture, two initial native results, final results when run, optional
reduced fixture, each distinct proposed candidate fixture under
`callbacks/candidates/`, and a report with fixture/result SHA-256 values, the
independent oracle XMM0 value, initial/final fingerprints, stop reason,
budget/proposal/run/acceptance progress, provenance and exact replay commands.
`no_initial_mismatch` never writes `reduced.case.json`.

Replay a saved reduced fixture with the existing comparison path:

```sh
od-runner --fixture work/stage2/s2-42d-xorpd/NAME/reduced.case.json --engine compare --json
```

Synthetic reducer callbacks are orchestration checks only. Bounded Linux x86_64
acceptance ran the two literal S2-40b asymmetric fixtures with the native
backend; both returned `no_initial_mismatch`, confirmed the independent oracle
state and code memory, and replayed successfully through the native backend.
No candidate shrinking was exercised. The environment reports
`execution_kind=unknown`, so this does not establish physical CPU identity or
Mac mini acceptance. See the [acceptance record](../reports/S2_42D_LINUX_ACCEPTANCE.md).
Parent S2-42 remains open for S2-21/S2-39/S2-40 dependencies and other required
corpus groups and hardware comparison.
