# S2-42aa: deterministic BSWAP EAX/RAX corpus

## Scope and status

This local-only slice exercises exactly the existing S2-15b forms:

| Width | Bytes | Target |
| --- | --- | --- |
| 32 | `0F C8` | EAX, with the architectural zero-extension to RAX |
| 64 | `48 0F C8` | RAX |

It does not admit other opcode-encoded registers, 16-bit operands, other prefixes,
or memory operands. The protocol is tied to the S2-15b implementation and its
Intel SDM revision 093 BSWAP definition. Native execution and physical hardware
acceptance are `not_run`; no native backend was started.

## Inputs and oracle

Each width has 16 pinned boundary controls in
[`s2_42aa_bswap_register_controls`](../../tests/fixtures/x86/stage2/s2_42aa_bswap_register_controls/).
The control manifest pins all fixture and expected-state SHA-256 values; the
generator also pins the control-manifest digest. Controls include zero, one,
byte-order patterns, sign boundaries, all ones, and mixed-byte patterns. The
32-bit controls deliberately give RAX nonzero high halves so EAX zero-extension
is observable. The RAX controls cover 64-bit values and byte patterns.

The deterministic generator adds 1,024 unique architectural input states per
width from four fixed seeds. Two repeat executions are required for every case.
The manifest records the exact ordered case list, fixture and expected-state
digests, per-width counts, seeds, and a corpus digest. Bundle verification
rejects missing, extra, changed, symlinked, or non-regular entries.

Expected state is computed by a stand-alone integer oracle: serialize the old
operand to little-endian bytes, reverse that byte sequence, and interpret it as
little-endian. The 32-bit result is restricted to EAX, which sets RAX[63:32] to
zero; the 64-bit result replaces all of RAX. The oracle preserves the full
tracked RFLAGS value and all other GPRs, keeps code bytes unchanged, expects
exactly one retired instruction, and requires a normal stop at the next RIP.
It imports no decoder, interpreter, native executor, or production BSWAP code.

## Execution and evidence

The generator is
[`generate_stage2_bswap_corpus.py`](../../tests/cpu/generate_stage2_bswap_corpus.py).
It shares only the generic strict JSON, digest, and exact-file-set helpers in
`stage2_corpus_support.py`.

```sh
python3 tests/cpu/generate_stage2_bswap_corpus.py --generate
python3 tests/cpu/generate_stage2_bswap_corpus.py --verify
python3 tests/cpu/generate_stage2_bswap_corpus.py --run-interpreter
python3 tests/cpu/generate_stage2_bswap_corpus.py --verify-results
python3 tests/cpu/generate_stage2_bswap_corpus.py --native
```

Generated inputs and execution records belong under
`work/stage2/s2-42aa-bswap-corpus/` and are not committed. The two-repeat saved
results bind corpus manifest, fixture bytes, runner executable digest, source
revision, build, host, command line, working directory, and selected environment
values. Replay rechecks exact stdout, both complete snapshots, and repeat
stability. Interrupted runs persist completed attempts and enumerate missing
repeats. Malformed JSON, duplicate keys, incomplete snapshots, mismatched
provenance, altered fixtures, or unstable repeats fail closed. Native output
explicitly records `not_run_form_not_admitted` and `backend_execution` as
`not_started`.

## Evidence boundary

Local interpreter agreement validates only this exact BSWAP EAX/RAX corpus.
It does not establish the rest of S2-15b, physical x86 behavior, native backend
admission, JIT behavior, or completion of parent S2-42 or any dependency.

## Local result

The frozen corpus contains 2,080 cases: 16 controls and 1,024 generated cases
for each width. All 4,160 interpreter attempts passed, and repeat snapshots
were identical. The verified corpus digest is
`4350586848a02f6990f4a0089e07366b90deff500e7741723b7a5a2be4c226f8`. Saved
results are at `work/stage2/s2-42aa-bswap-corpus/interpreter-results.json`;
the verifier accepted the complete record and exact stdout replay.

The runner reported its compiled revision and compiler fields as `unavailable`.
The saved evidence separately records the source commit at execution time,
runner executable SHA-256, target, build profile, host identity, selected
environment, command, and working directory. Replay requires the recorded
source commit to match the checked-out commit. This local result does not add a
native or physical-hardware claim.
