# S2-42g — bounded qword GPR PUSH/POP reducer

## Status and scope

This slice adds an interpreter-callback reducer for exact one-instruction
schema-v2 qword GPR PUSH/POP fixtures in the existing `s2-long-integer-v1`
profile. It implements no CPU behavior, fixture field, or backend admission.
Native and physical execution remain `not_run`; the parent S2-42 requirement
and dependencies S2-21/S2-39/S2-40 remain open.

The reducer follows the proposed
[S2-42f acceptance contract](S2_42_REDUCER_CONTRACT.md), scoped to the successful
one-instruction stack forms described by
[S2-17a](../plans/STAGE_2.md#s2-17-стек-callret-и-косвенное-управление).
It does not accept memory PUSH/POP, immediate PUSH, flags-stack operations,
fault fixtures, 16-bit forms, extra prefixes, loops, or multi-instruction code.

## Invocation

```sh
cargo run -p od-runner --offline -- \
  --reduce-push-pop tests/fixtures/x86/stage2/s2_17a_push_rax.case.json \
  [--limit N] [--output-dir work/stage2/s2-42g-stack/<run-id>]
```

The default callback budget is 132, and the accepted range is 4 through 132.
The default output path is a new child of
`work/stage2/s2-42g-stack/` named from the input fixture. An explicit output
path must also be a new child there. The command rejects symlink path
components, bounds source fixture reads by the schema-v2 size limit, and
preserves the original fixture JSON bytes and SHA-256.

## Admission and oracle

The command validates the existing fixture schema and then requires all of the
following:

- profile `s2-long-integer-v1`, mode `long64`, expected stop-address exit,
  instruction budget 1, and `stop_rip` immediately after the single opcode;
- exactly one RX `code` region at the initial RIP and one RW, non-executable
  `stack` region, both fully compared;
- exactly `50`–`5f` or `41 50`–`41 5f`, with no other prefixes or bytes.

The independent oracle reads/writes the stack as little-endian bytes. PUSH
uses the original RSP as the source for `PUSH RSP`, stores at `RSP - 8`, and
commits the decremented RSP only after the bounded write. POP reads from old
RSP; ordinary destinations commit `RSP + 8`, while `POP RSP` replaces RSP with
the popped value. The oracle preserves and compares the complete integer
state, defined flags, and every byte in both mapped regions.

## Candidate schedule and mismatch identity

The schedule is deterministic and only clears set input bits, from bit 63 to
bit 0. For PUSH it changes only the selected source GPR; `PUSH RSP` has no
mutable value and therefore can be checked but not reduced. For POP it changes
only the eight initial bytes at old RSP. Candidate cost is the Hamming weight
of that value and must strictly decrease.

The original, each candidate, and the final candidate must each produce two
semantically identical interpreter results. A candidate is retained only if
the same nonempty mismatch fingerprint appears twice. Fingerprints identify
the differing GPR, RIP, defined RFLAGS bit, or memory-region byte offset.
Unsupported, exception, timeout, signal, malformed, missing-snapshot, and
harness-error outcomes do not preserve an architectural mismatch. If no
initial mismatch exists, the command emits `no_initial_mismatch` without a
reduced fixture. An exhausted budget or unstable/failing callback returns a
nonzero status.

The maximum 132 callbacks covers two initial runs, up to 64 two-run bit-clear
proposals, and two final confirmations. The report distinguishes candidate
proposals, candidate pairs, accepted candidates, and actual callbacks.

## Evidence

The run directory contains the exact `original.case.json`, the independent
`expected-original.json`, every callback result and record under `callbacks/`,
each distinct candidate fixture, and `report.json`. The report binds input and
result hashes, mismatch fingerprints, operation, stop reason, budget counters,
provenance, toolchain, revision and replay `argv`/shell commands. Callback
records are written as each run completes so an interrupted run retains
completed evidence.

The original and reduced fixtures replay through the existing interpreter CLI.
The report explicitly records `native=not_run` and `physical_hardware=not_run`;
synthetic callback tests establish reducer orchestration only.

Focused testkit tests cover PUSH RSP ordering, POP RSP overwrite, REX.B register
decoding, stable mismatch reduction, no-initial-mismatch behavior, invalid
instruction rejection, callback failure, and budget exhaustion. A local CLI
smoke run of the S2-17a PUSH RAX and POP RSP fixtures returned
`no_initial_mismatch` after two interpreter callbacks each; this is not native
or physical acceptance.
