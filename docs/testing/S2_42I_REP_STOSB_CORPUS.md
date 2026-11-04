# S2-42i: deterministic REP STOSB corpus

## Scope

This slice adds a generated corpus for the existing schema-v2 profile
`s2-long-integer-v1`. It contains only the exact two-byte instruction `F3 AA`
in long mode with address size 64. It changes no CPU implementation, fixture
schema or native admission. The selected native gate accepts count 0 through 8
only when every destination byte is mapped; the work-limit and fault cases stay
outside this corpus. See the [S2-19a local contract](../plans/STAGE_2.md#s2-19a--f3-aa-rep-stosb-ограниченный-локальный-контракт).

The checked-in generator deterministically produces 1,024 cases from four
fixed seeds with 256 sequences each, plus 36 fixed boundary controls. Generated
inputs cover all 18 `(RCX, DF)` combinations for counts 0–8 and DF=0/1, with
different initial byte values, data spans and register states. Controls include
zero-count cases whose RDI points outside mapped memory, first and last valid
spans for both directions, maximum count 8, and unaligned RDI offsets. The
instruction budget is one, the stop RIP is exactly entry+2, and each successful
invocation retires exactly once including count zero.

## Independent fill oracle

The expected result is computed by a byte-fill model that does not call the
interpreter, decoder, runner or native executor. For each successful element it
writes the initial AL byte at RDI, advances RDI by +1 or −1 according to DF, and
decrements RCX. When the instruction completes, RCX is zero. Count zero performs
no destination access and leaves RDI unchanged. Flags, RSI and other unaffected
GPRs remain unchanged. The oracle checks full GPR state, RIP, RFLAGS and
definedness, one retirement, the complete code/data memory snapshots, and every
untouched data byte used as a canary.

`manifest.json` fixes the generator version, profile, exact instruction,
address size, retirement count, seeds, count/DF matrix, ordered fixture SHA-256
values, the complete expected-result artifact digest and aggregate corpus
digest. Verification regenerates and compares the entire file set byte-for-byte
and rejects missing, extra, symlink or changed files. The exact fixture schema
is still checked by the existing Rust runner.

## Commands

Generate the corpus in ignored `work/` and verify its manifest:

```sh
python3 tests/cpu/generate_stage2_rep_stosb_corpus.py
```

Verify an existing corpus without rewriting it:

```sh
python3 tests/cpu/generate_stage2_rep_stosb_corpus.py \
  --corpus-dir work/stage2/s2-42i-rep-stosb --verify
```

Run the selected suite from a development machine that can reach a supported
Linux x86_64 host over SSH:

```sh
python3 tests/cpu/generate_stage2_rep_stosb_corpus.py --run-oracle \
  --host HOST --user USER --remote-root /absolute/remote/checkout
```

The command runs all 1,060 inputs twice through the interpreter and required
Linux x86 native backend. It verifies each result against the independent fill
oracle, binds saved inputs/results to fixture digest, case, schema, profile,
backend, build and machine identity, and writes a machine-readable summary at
`work/stage2-42i-oracle/s2-42i-summary.json`. Missing native execution or any
identity/result mismatch returns nonzero. A preflight error writes a failed
summary before exiting, replacing any stale successful summary and never
starting the backend.

Verify saved oracle evidence later without contacting either backend:

```sh
python3 tests/cpu/generate_stage2_rep_stosb_corpus.py \
  --corpus-dir work/stage2/s2-42i-rep-stosb \
  --verify-oracle-run work/stage2-42i-oracle/<run-id>
```

## Evidence boundary

Seven focused Python tests cover deterministic uniqueness, the full count/DF
matrix, zero-count no-access, maximum-count and first/last/unaligned spans,
canaries, strict bundle verification, saved-result identity, and fail-closed
summary behavior. A local macOS interpreter run executed all 1,060 fixtures and
matched their independent expected results. This is interpreter evidence only.
The Linux native SSH comparison and physical Mac acceptance have not been run
and remain `not_run`; corpus generation does not imply native or physical
correctness, and S2-42 remains open.
