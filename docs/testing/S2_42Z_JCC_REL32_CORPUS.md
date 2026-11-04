# S2-42z: near JZ/JNZ rel32 local interpreter corpus

## Scope

This slice covers only long-mode near `JZ rel32` (`0F 84 cd`) and `JNZ rel32`
(`0F 85 cd`) in the existing `s2-long-integer-v1` profile. Each schema-v2
fixture executes one six-byte branch with an instruction budget of one. The
code region is execute-only and exactly six bytes long. The stop address is
the independently calculated target when taken, otherwise the next RIP.

The fixed entry is `0x0000000100000000`. It keeps both signed 32-bit endpoint
targets canonical in the lower canonical half. The 32 handwritten controls
cover both opcodes, both initial ZF values, and each rel32 value
`INT32_MIN`, `INT32_MIN+1`, `-129`, `-1`, `+1`, `+127`, `+128`, and `INT32_MAX`.
The taken `-6` displacement is excluded because it stops at the entry address
before the interpreter can execute the branch.

There are 1,024 generated inputs: four fixed seeds and 256 sequences per seed.
Each seed contains all 32 opcode × ZF × displacement cells eight times. All
generated architectural inputs are unique; all 16 GPRs and the defined
non-ZF RFLAGS bits vary. ZF selects the matrix cell. The fixed window and
target calculation are checked for canonicality.

The standalone integer oracle reads only the literal opcode, ZF, and four
displacement bytes. It sign-extends the 32-bit displacement, adds it to the
address after the instruction, and checks canonicality. It does not call the
production decoder, branch evaluator, or interpreter. Expected snapshots
compare all 16 unchanged GPRs, RIP, all defined RFLAGS, the unchanged six code
bytes, one retired instruction, stop-address outcome, and no exception.
Pinned controls include complete expected snapshots. The corpus manifest
binds all fixture/expected digests, seed matrix, control manifest, and corpus
digest; verification rejects missing, extra, symlinked, malformed, or changed
files.

## Reproduce and verify

From the repository root:

```sh
python3 tests/cpu/generate_stage2_jcc_rel32_corpus.py --generate
python3 tests/cpu/generate_stage2_jcc_rel32_corpus.py --verify
python3 -m unittest tests.cpu.test_stage2_jcc_rel32_corpus
python3 -m py_compile tests/cpu/generate_stage2_jcc_rel32_corpus.py tests/cpu/test_stage2_jcc_rel32_corpus.py
```

Build and run the local interpreter twice per fixture, then verify the saved
process and result records:

```sh
OPEN_DESKTOP_GIT_REVISION=$(git rev-parse HEAD) RUSTC_VERSION="$(rustc --version)" \
  cargo build --offline -p od-runner
python3 tests/cpu/generate_stage2_jcc_rel32_corpus.py --run-interpreter
python3 tests/cpu/generate_stage2_jcc_rel32_corpus.py --verify-results
```

The ignored local bundle is under
`work/stage2/s2-42z-jcc-rel32-corpus/`. The result evidence stores each
attempt's command, working directory, allowlisted environment, exit status,
stdout/stderr, parsed observation, runner hash/revision, build identity, and
host identity. It checkpoints after every case and does not treat incomplete
or timed-out attempts as passing evidence.

The completed local run had 1,056 fixtures, two interpreter executions per
fixture (2,112 executions), zero failures, and no missing attempts. The corpus
digest was
`ac74f064bc26f433c050debc5f9984c40a5fab442d2055ecbec7c42c54a2947f`; the
saved result was bound to manifest SHA-256
`9220ff0eaaccba3a317d2bd37c49f8e14e4e84170132c627471b1f5718f45c13`. The
runner was built offline in debug mode with `rustc 1.99.0` for
`aarch64-apple-darwin`; observed host identity was macOS/aarch64.

## Native denial and limits

Native execution is explicitly denied before backend invocation:

```sh
python3 tests/cpu/generate_stage2_jcc_rel32_corpus.py --native
```

The command exits unsuccessfully and writes `native_x86=not_run_form_not_admitted`
with `backend_execution=not_started`. Tests patch the process launcher to
fail if a backend is started. These results are local interpreter evidence;
native x86 and physical Mac acceptance remain `not_run`. No claim is made about
other Jcc conditions, encodings, code mappings, JIT behavior, or complete x86
control flow.

## Files

- [Generator and evidence runner](../../tests/cpu/generate_stage2_jcc_rel32_corpus.py)
- [Focused regressions](../../tests/cpu/test_stage2_jcc_rel32_corpus.py)
- [Pinned handwritten controls](../../tests/fixtures/x86/stage2/s2_42z_jcc_rel32_controls/)
