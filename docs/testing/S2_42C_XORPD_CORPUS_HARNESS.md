# S2-42c: XORPD corpus harness

This document describes the bounded S2-42c orchestration slice. It consumes the
recorded S2-42a corpus at `work/stage2/s2-42a-xorpd/`; it does not generate or
rewrite that corpus or its manifest. S2-42 as a whole remains open. This slice
does not add an XORPD reducer, CPU semantics, native admission, fixture schema,
CPUID behavior, or FXSAVE behavior.

## Invocation

From the repository root, run:

```sh
python3 tests/cpu/stage2_oracle.py --suite xorpd-native-corpus \
  --host HOST --user USER --remote-root /absolute/remote/checkout
```

The selected suite requires exactly two repeats and rejects positional fixture
arguments. The existing integer-fixture suite remains the default and retains
its previous CLI and behavior.

## Required acceptance

Before execution, the harness validates the recorded manifest version,
generator version, exact XORPD profile, four fixed seeds, 256-case ranges,
ordered case hashes and combined corpus digest. It requires precisely the 1024
expected case files, matching case IDs, seed/profile identity, and unique full
XMM inputs. It copies exact fixture bytes into the run evidence directory and
records their hashes. It then builds `od-runner` and requires its existing
`--verify-native-xorpd-corpus` verifier to accept the full strict fixture schema
and corpus. The verifier performs two local interpreter executions per case
and checks its independent expected result.

The harness next runs each fixture through the interpreter and Linux x86 native
runner twice. Each run records its result JSON, process status, stderr, build
fingerprint, compiler, machine metadata and fixture digest. Existing result
comparison checks the fixture outcome and full state, including XMM0–15, MXCSR,
integer state, retirement and stopping RIP. Each backend's two results must
match. Native unavailability, a timeout, missing or malformed output, a
nonzero result, or any mismatch fails the command; the run manifest and any
partial command/result logs remain available under `work/stage2-oracle/`.

## Evidence boundary

The Python unit tests use synthetic filesystem inputs and mocked process
responses to check orchestration and failure retention. They are not CPU
execution results. The physical/native run for this implementation task is
`not_run`; the harness command above is the required future hardware run. This
slice establishes corpus orchestration only and does not claim that XORPD has a
new reducer or broader native support.
