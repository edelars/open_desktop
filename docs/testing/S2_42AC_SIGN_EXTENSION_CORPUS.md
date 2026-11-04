# S2-42ac CDQE/CQO local corpus contract

This slice exercises exactly the existing S2-15a long64 byte forms `48 98`
(CDQE) and `48 99` (CQO). The fixture profile is `s2-long-integer-v1`; this
corpus does not admit alternate opcode widths, prefixes, encodings, or native
execution.

## Inputs and integer oracle

The checked-in `tests/fixtures/x86/stage2/s2_42ac_sign_extension_controls/`
directory contains sixteen pinned controls per opcode, with independently frozen
expected snapshots and a SHA-256-pinned control manifest. The deterministic
generator adds four seeds × 256 inputs per opcode (1,024 CDQE and 1,024 CQO
inputs), for 2,080 total cases. It rejects duplicate generated input tuples and binds every fixture and
expected snapshot in the ordered corpus manifest. Every case is run twice.

The Python expected-state oracle uses only integer bit operations. CDQE sets
RAX to the sign extension of the initial EAX bits, ignoring initial RAX[63:32].
CQO preserves RAX and sets RDX to `0` or `0xffffffffffffffff` from initial
RAX[63]. Both preserve the other fourteen GPRs, the complete RFLAGS value and
definedness mask, and code bytes; RIP advances by two, one instruction retires,
the stop-address outcome is reached, and no exception is reported.

## Commands and evidence

Run `python3 tests/cpu/generate_stage2_sign_extension_corpus.py --generate`,
then `--verify`. `--run-interpreter` records two full observations, runner
revision/executable digest, build and host provenance, exact argv/cwd/environment,
stdout/stderr, status, timeouts, and an ordered missing-attempt list. It writes
an incomplete summary before preflight and checkpoints each case; interruption
leaves completed attempts replayable. `--verify-results` replays saved evidence
against the fixture bytes, independent oracle, manifest, and runner identity.
Tampered results, stale success, altered provenance, mismatched repeats, or
missing/extra files fail closed.

`--native` writes a blocked record with `backend_execution=not_started` and
returns failure before any backend invocation. No remote host is contacted.
These local corpus checks do not claim physical x86 acceptance or native
backend support.
