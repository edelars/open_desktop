# S2-42ai — bounded successful REP STOSB reducer

## Scope

This slice adds an AL-only reducer for successful Long64 executions of the exact
admitted `F3 AA` form in `s2-long-integer-v1`. It accepts full RCX counts 1–8,
either DF value, and a fully mapped byte destination span. It changes no CPU
semantics, schema, or native admission. Faults, zero count, other REP forms,
and counts above eight remain outside the reducer.

## Contract and oracle

The independent oracle fills the addressed bytes with the initial AL value,
sets RCX to zero, advances or decrements RDI by the original count according to
DF using 64-bit register wraparound, advances RIP by two, and expects one retired instruction. Every other GPR,
RFLAGS, definedness bit, code byte, data byte, region, and canary remains as
specified by the fixture. It uses no decoder, interpreter, or native executor.

The fixture must contain exact RX code bytes `F3 AA` at entry and one fully
observed RW data region of at most 64 bytes. Count is 1–8. RDI and every
iteration destination byte must lie in that data region. The reducer changes
only AL, clearing set bits from 7 down to 0 while preserving the upper 56 bits
of RAX and every other fixture field.

An observed result is eligible only when it has the requested backend identity,
matches the exact fixture identity, completes at `stop_rip`, retires once, and
matches the oracle in full architectural state, flags definedness, and layout.
Only bytes written by REP may differ from the oracle; untouched bytes are strict
canaries. The mismatch fingerprint is the exact ordered list of differing
destination offsets. The original, each candidate, and the final result require
two semantically identical callbacks. A candidate is accepted only when it
strictly reduces AL Hamming weight and preserves the original fingerprint.

The default and maximum callback budget is 20: two original callbacks, up to
eight candidate pairs, and two final confirmations. Callback faults, unsupported
admission, timeout, malformed or wrong-backend results, state/progress drift,
canary changes, flakiness, and budget exhaustion fail closed and retain report
and per-callback evidence.

## Commands

Interpreter reducer:

```sh
cargo run -p od-runner --offline -- \
  --reduce-rep-stosb tests/fixtures/x86/stage2/s2_19a_rep_stosb_one.case.json
```

Linux x86_64 native reducer, using the existing supervised native-v2 fixture
child and exact `F3 AA` admission:

```sh
cargo run -p od-x86-runner --offline --target x86_64-unknown-linux-gnu -- \
  --reduce-native-rep-stosb tests/fixtures/x86/stage2/s2_19a_rep_stosb_one.case.json
```

Both commands accept `--limit N` (4–20) and `--output-dir DIR`; output must be a
new child below the reducer's dedicated `work/stage2/` directory. Results save
the original fixture, independent oracle output, every callback result and
journal record, candidate fixtures, report, and replay fixture invocations.
Native evidence records `execution_kind: unknown` and physical Mac acceptance
as `not_run`.

## Evidence boundary

The synthetic regression exercises exact destination-offset retention while
reducing AL to zero under both interpreter and NativeX86 callback identities.
Additional regressions reject full-state drift, canary changes, initial/candidate
flakiness, callback errors, exhausted budgets, and candidate/final backend
swaps. Synthetic callbacks verify reducer decisions only; they do not establish
a CPU mismatch. A native run that reports
`no_initial_mismatch` after two callbacks does not exercise candidate shrinking.
Physical Mac acceptance remains separate.
