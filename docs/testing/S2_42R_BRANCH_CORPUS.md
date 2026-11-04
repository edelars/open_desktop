# S2-42r: JZ/JNZ rel8 branch corpus

S2-42r supplies a deterministic interpreter corpus for the existing
`s2-long-integer-v1` profile. Each fixture contains exactly one short `JZ`
(`74 ib`) or `JNZ` (`75 ib`) instruction at offset 128 or 192 in one 512-byte
read/execute mapping filled with NOPs. The instruction budget is one. The stop
address is the branch target when taken and the next instruction otherwise.

**Status:** the full 1,056-case corpus has passed two local interpreter runs per
case and saved-result verification (2,112 exact executions; corpus digest
`4394020ce98b7cd70e7219fb656e4557d992b766e4626e2ce07c8176e024d26c`).
Subsequent Linux x86_64 native acceptance passed all 1,056 cases twice at
source fingerprint
`ccfd49a28ea6ddf804f2124dab30b0e569b7e1666a8808e0296b12f642f96f49`; 2,112
saved results per backend matched. Evidence: [summary](../../work/stage2-42r-oracle/s2-42r-summary.json)
and `work/stage2-42r-oracle/stage2-20261104-115710-057e3506/`. Physical Mac
acceptance remains `not_run`.

## Corpus and independent expected state

The matrix has 1,024 generated cases: four fixed 64-bit seeds, 256 cases each.
Each seed contains every opcode × ZF × displacement cell exactly eight times.
The eight signed rel8 values are −128, −127, −16, −3, +1, +16, +126, and +127.
Every selected target is inside the mapping and outside its own two-byte
instruction. The corpus also includes 32 checked-in handwritten controls,
one for every matrix cell, with complete static state and code snapshots in
`tests/fixtures/x86/stage2/s2_42r_branch_controls/`.

Generated cases vary the full 16-register input, every defined non-ZF RFLAGS
bit, and the entry offset; all 1,024 generated architectural input states are
unique. ZF remains the cell selector and reserved RFLAGS bits remain clear.

The expected-state oracle reads the literal opcode and RFLAGS.ZF bit 6, sign
extends the raw rel8 byte, and computes target as `next_rip + displacement`.
It does not call a decoder, condition-code helper, or interpreter code. Every
expected record contains all 16 GPR values, RIP, RFLAGS and definedness, the
full unchanged code mapping, stop-address outcome, one retired instruction,
and a null exception. Hard-coded SHA-256 anchors pin all 32 handwritten
expected snapshots. The manifest binds ordered fixture digests, expected
record digests, seed/matrix metadata, and the complete corpus digest. Strict
verification rejects missing, extra, symlinked, or byte-modified files.

## Generate and verify

From the repository root:

```sh
python3 tests/cpu/generate_stage2_branch_corpus.py --generate
python3 tests/cpu/generate_stage2_branch_corpus.py --verify
```

The generated bundle is stored under ignored `work/stage2/s2-42r-branch-corpus/`.
Build the local runner, execute all 1,056 cases twice through the interpreter,
and verify the saved evidence:

```sh
OPEN_DESKTOP_GIT_REVISION=$(git rev-parse HEAD) RUSTC_VERSION="$(rustc --version)" \
  cargo build --offline -p od-runner
python3 tests/cpu/generate_stage2_branch_corpus.py --run-interpreter
python3 tests/cpu/generate_stage2_branch_corpus.py --verify-results
```

The saved local summary binds the manifest, ordered fixture digests, runner
binary and revision, machine/build identity, and both complete results for each
case. A successful local run records `physical_native_acceptance: not_run`;
this is not native hardware evidence.

## Native admission

`--run-oracle` passes the exact 1,056 fixture paths to the existing generic
`tests/cpu/stage2_oracle.py` two-repeat interpreter-versus-Linux-x86-64 native
oracle. Acceptance requires the generic oracle manifest and every result file
to be complete, the repeat comparison to report zero differences, and each
backend result independently to match the full expected snapshot. Equal wrong
results from both backends therefore fail. If the physical native host is
unavailable, the command returns failure and writes machine-readable evidence
with native/physical acceptance left `not_run`. JIT is not selected and no
physical Mac acceptance is claimed.

Saved native acceptance is accepted only when the oracle process and every
recorded orchestration command exit successfully, frozen inputs match the full
corpus byte-for-byte, and each backend/repeat has a zero-exit process record.
Malformed or incomplete manifests, failed or timed-out commands, missing frozen
fixtures, and missing process records fail closed.

Focused checks:

```sh
python3 -m unittest tests/cpu/test_stage2_branch_corpus.py
python3 -m py_compile tests/cpu/generate_stage2_branch_corpus.py tests/cpu/test_stage2_branch_corpus.py
```
