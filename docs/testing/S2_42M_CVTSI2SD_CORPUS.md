# S2-42m: CVTSI2SD interpreter corpus

This slice adds a deterministic local corpus for the existing exact profile
`s2-long-cvtsi2sd-r32-exact-v1`. It executes only `F2 0F 2A C0`
(`CVTSI2SD XMM0,EAX`) in Long64 mode with one RX code mapping, instruction
budget 1, and stop address `entry+4`. It does not change CPU semantics, schema,
or backend admission.

## Scope and oracle

The generator creates 1,024 matrix cases from four fixed seeds, 256 per seed,
plus nine literal handwritten controls. The matrix mixes signed i32 values,
zero and ±1, signed endpoints, powers of two and neighbors, ±(2^24−1),
±(2^24+1), and seeded pseudo-random values. MXCSR cycles through all four
rounding-control encodings and varies sticky flags, exception masks, and FTZ;
`CR4.OSXMMEXCPT` is exercised both ways. DAZ and reserved MXCSR bits are never
set. The literal controls have fixed expectations independent of the matrix
generator.

Expected binary64 encodings are assembled with integer operations: signed i32
absolute value, sign/exponent construction, and an exact integer significand
shift. No host floating point or production CPU helper participates in the
generated-case oracle. The nine handwritten controls use a separate fixed
table of literal expected encodings. Each expected record includes the complete expected architectural
state. The verifier checks full GPRs, RIP, RFLAGS and definedness, all XMM
registers (including preserved XMM0.high64), MXCSR and the accepted control
fields, code bytes, stop outcome, no exception, and one retired instruction.

## Generate and verify

From the repository root:

```sh
python3 tests/cpu/generate_stage2_cvtsi2sd_corpus.py --generate
python3 tests/cpu/generate_stage2_cvtsi2sd_corpus.py --verify
```

The default corpus and saved run are under the ignored `work/stage2/s2-42m-cvtsi2sd/`
directory. Generation binds ordered case IDs, exact fixture hashes, and exact
expected-record hashes in `manifest.json`; verification rejects extra/missing
files, symlinks, altered bytes, and duplicate JSON keys.

Build the local runner and execute every fixture twice through the
interpreter-only CLI:

```sh
cargo build --offline -p od-runner
python3 tests/cpu/generate_stage2_cvtsi2sd_corpus.py --run-interpreter
python3 tests/cpu/generate_stage2_cvtsi2sd_corpus.py --verify-results
```

The run summary binds the manifest digest, runner executable digest, source
revision, host OS/architecture, ordered fixture digests, and both full result
objects per case. Any unavailable runner, nonzero runner exit, unexpected
profile/backend, state mismatch, provenance mismatch, or repeat difference
fails with a nonzero exit and a machine-readable failure summary. Result
verification rechecks saved results against the fixture bytes, expected
records, run identities, and order.

## Backend boundary and evidence

This exact profile is interpreter-only. Native execution is closed; an
explicit `--native` request fails with a nonzero status. A successful
interpreter corpus run records `native_x86: not_run_profile_closed`, which is
not a native pass or physical-machine claim. JIT is not selected. The corpus
does not claim native, physical, or integrated-machine validation.

Focused integrity tests:

```sh
python3 -m unittest tests/cpu/test_stage2_cvtsi2sd_corpus.py
python3 -m py_compile tests/cpu/generate_stage2_cvtsi2sd_corpus.py tests/cpu/test_stage2_cvtsi2sd_corpus.py
```
