# S2-42h: deterministic qword GPR PUSH/POP corpus

## Scope

S2-42h adds a deterministic schema-v2 corpus for the existing positive
`s2-long-integer-v1` qword GPR PUSH/POP forms from S2-17a. Each fixture executes
one exact instruction and stops at the next RIP with an instruction budget of
one. The corpus has 1,024 generated cases (four fixed seeds × 256 sequences)
plus 32 fixed control cases. All 32 operation/register combinations occur in
both sets: PUSH and POP for RAX–R15, including RSP aliases.

The instruction bytes are exactly the S2-17a forms: `50`–`57`/`58`–`5F` for
RAX–RDI and `41 50`–`41 57`/`41 58`–`41 5F` for R8–R15. This corpus excludes
16-bit overrides, immediate PUSH, memory operands, CALL/RET, REP, and faulting
cases. It changes no CPU behavior, fixture schema, native admission, or runner
CLI.

## Inputs and independent oracle

Each fixture maps one executable code region and a fully readable/writable
256-byte stack region. Every byte is compared, including canaries outside the
single 8-byte transfer. Generated data is deterministic per seed and sequence;
the fixed controls use explicit register patterns and `0xA5` stack fill. Cases
exercise the first and last valid spans, interior positions, and unaligned
stack offsets. The exact instruction/register matrix and span metadata are in
the manifest.

The independent oracle models only byte transfer and architectural ordering; it
does not decode or execute the instruction. For PUSH, it captures the source
before decrementing RSP and stores that source at `old RSP - 8`. In particular,
PUSH RSP stores the old RSP. For POP, it loads eight bytes at old RSP, increments
RSP, then writes the destination. For POP RSP, the popped value replaces the
incremented RSP. The expected artifact records all GPRs, next RIP, RFLAGS and
definedness, exact retirement count, complete code/stack memory, and access
metadata.

## Manifest and verification

The generator writes the 1,056 fixtures, `expected.json`, and strict
`manifest.json` under `work/stage2/s2-42h-stack/`. It records generator and
manifest versions, profile, seeds, generated/handwritten counts, operations,
registers, every ordered fixture digest, the expected artifact digest, boundary
vectors, and the ordered corpus digest. Verification reconstructs the expected
byte set and rejects duplicate JSON keys, missing/extra files, symlinks, and
any differing fixture, manifest, or expected-result byte.

To generate and verify the local corpus without CPU execution:

```sh
python3 tests/cpu/generate_stage2_stack_corpus.py
python3 tests/cpu/generate_stage2_stack_corpus.py --verify work/stage2/s2-42h-stack
```

## Interpreter/native orchestration

The single selected-suite command is:

```sh
python3 tests/cpu/generate_stage2_stack_corpus.py --run-oracle \
  --host HOST --user USER --remote-root /absolute/remote/checkout
```

It invokes the existing `tests/cpu/stage2_oracle.py` for all 1,056 cases with
two interpreter and two Linux x86_64 native repetitions, then checks every
result's schema version, profile, backend, exact fixture digest, build and
machine provenance against its run-manifest entry before comparing it against
the independent stack oracle and full comparison mask.
Missing input/result/backend, nonzero status, a host other than Linux x86_64,
malformed snapshot, mismatch, or missing repetition returns nonzero and writes
`s2-42h-summary.json` with `status: failed`. Evidence remains under
`work/stage2-42h-oracle/`; physical Mac acceptance is explicitly `not_run`.

## Evidence boundary

The local interpreter run checked all 1,056 fixtures against the independent
oracle with no mismatches. Focused Python tests cover determinism, distinct
cases, the complete 32-combination matrix, exact encodings, PUSH RSP/POP RSP
ordering, first/last and unaligned spans, canaries, manifest tampering, missing
native failure, and malformed-summary replacement. A complete synthetic
two-repeat matrix checks the verifier success path; it is orchestration evidence
only. These checks and the local interpreter run do not establish Linux native
or physical Mac execution. S2-42 remains open for other corpus groups, generic
reducers, S2-21/S2-39/S2-40 dependencies, and hardware acceptance.
