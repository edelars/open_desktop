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
arguments. It selects six handwritten controls first, then the recorded 1,024
S2-42a generated cases, for 1,030 total inputs. The existing integer-fixture
suite remains the default and retains its previous CLI and behavior.

## Required acceptance

Before execution, the harness validates the separately versioned S2-42p hand
control manifest and its pinned SHA-256/control-set digest. It requires exactly
the six listed control identities in fixed order, checks the four-file hand
control directory for missing/extra files, verifies all six exact fixture
hashes and input vectors, and recomputes each literal bytewise-XOR XMM0
expectation. The two S2-40b asymmetric fixtures remain byte-for-byte unchanged.
The hand-control manifest is separate from, and does not rewrite, the S2-42a
generated manifest or corpus digest.

The harness then validates the recorded generated manifest version,
generator version, exact XORPD profile, four fixed seeds, 256-case ranges,
ordered case hashes and combined corpus digest. It requires precisely the 1024
expected case files, matching case IDs, seed/profile identity, and unique full
XMM inputs. It copies exact fixture bytes into the run evidence directory and
records their hashes. It then builds `od-runner` and requires its existing
`--verify-native-xorpd-corpus` verifier to accept the full strict fixture schema
and corpus. The verifier performs two local interpreter executions per case
and checks its independent expected result.

The harness next runs all 1,030 fixtures through the interpreter and Linux x86
native runner exactly twice. Each run records its result JSON, process status, stderr, build
fingerprint, compiler, machine metadata and fixture digest. Existing result
comparison checks the fixture outcome and full state, including XMM0–15, MXCSR,
integer state, retirement and stopping RIP. Each backend's two results must
match. Both backends must also produce each hand control's fixed expected XMM0
bytes. The remote native batch receives control-first manifest order and flushes
one JSON-lines record per fixture, and
the local harness saves each case's output and process status as it arrives.
If a later case times out, earlier native results remain available beside the
partial command logs. Native unavailability, a timeout, missing or malformed output, a
nonzero result, or any mismatch fails the command; the run manifest and any
partial command/result logs remain available under `work/stage2-oracle/`.
Machine-readable evidence reports `generated=1024`, `handwritten=6`, and
`total=1030`, with separate generated-corpus and handwritten-control digests.
Native status stays `not_run` until a remote Linux x86_64 run actually starts.

## Evidence boundary

The Python unit tests use synthetic filesystem inputs and mocked process
responses to check orchestration and failure retention. They are not CPU
execution results. The six local interpreter controls pass as a separate
focused runner check; on Darwin arm64, native comparison and physical hardware
acceptance remain `not_run` until the command above runs on the supported Linux
x86_64 host. This adds six
handwritten cases and closes only this explicit parent gap; it does not close
full S2-42, perform hardware acceptance, add a new reducer, or broaden native
support.
