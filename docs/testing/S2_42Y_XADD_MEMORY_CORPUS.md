# S2-42y: local bare-memory XADD corpus

S2-42y covers exactly the existing S2-21d bytes `48 0F C1 00`
(`XADD qword [RAX], RAX`) under `s2-long-integer-v1`. The encoding has no
`LOCK` prefix. Code is execute-only at `0x100000`, is four bytes long, and the
one-instruction stop address is `0x100004`. Initial RAX, which is also the
memory effective address, is fixed at `A = 0x3000fb`.

The qword operand begins three bytes into the adjacent private read/write,
non-executable regions: `[0x3000f8, 0x300100)` and
`[0x300100, 0x300108)`. Thus it crosses the region boundary at `0x300100`.
Unmodified bytes are `0xa5` canaries. The instruction budget is one and the
fixture wall timeout is 2,000 ms.

## Oracle and coverage

The independent integer oracle snapshots pre-instruction address `A`, old
memory qword `M`, all GPRs, flags, code, and both complete data regions. It
sets final RAX to `M`, writes `(M + A) mod 2^64` to the qword at the old address
`A`, and computes CF, PF, AF, ZF, SF, and OF from that addition. It preserves
all other GPRs and the non-arithmetic RFLAGS bits, both canary areas, and code
bytes. Expected state requires RIP `0x100004`, one retired instruction,
`stop_address`, and no exception. The oracle uses integer arithmetic only and
does not call the production decoder or flag helpers.

Eight handwritten controls pin their full fixture bytes and expected
snapshots in
[`s2_42y_xadd_qword_memory_controls/`](../../tests/fixtures/x86/stage2/s2_42y_xadd_qword_memory_controls/).
The control manifest binds ordered IDs, fixture digests, expected-snapshot
digests, and the expected document digest; the generator pins the manifest
digest. The controls cover ordinary zero/one and boundary values, AF from
`M = 5`, signed overflow from `M = 0x7fffffffffffffff`, unsigned carry and
wrap, and a wrap-to-zero sum with `M = -A mod 2^64`. Runtime generation checks
these frozen snapshots against the independent oracle.

The generated set contains 1,024 unique inputs: 256 for each fixed seed
`243f6a8885a308d3`, `13198a2e03707344`, `a4093822299f31d0`, and
`082efa98ec4e6c89`. It assigns 64 cases to each of 16 old-memory classes,
including zero, one, AF carry, signed limits, negative values, wrap-to-zero,
wrap-near-zero, and ordinary sparse/mixed values. Other GPRs and defined
initial RFLAGS bits vary; R15 contains a seed/sequence tag. The exact encoding,
RAX/address, split mappings, permissions, and canary pattern remain fixed.

The corpus manifest orders controls before generated cases and binds each
fixture and expected snapshot, the frozen controls manifest, the expected
file, and the combined corpus digest. Exact-bundle checks reject missing,
extra, symlinked, or byte-modified files; duplicate JSON keys; malformed
manifest shapes; changed controls; and digest mismatches.

## Local execution and saved evidence

From the repository root, generate and verify the corpus, build the runner with
the branch revision and compiler identity, run all 1,032 cases twice through
the local interpreter, and replay-verify the saved results:

```sh
python3 tests/cpu/generate_stage2_xadd_qword_memory_corpus.py --generate
python3 tests/cpu/generate_stage2_xadd_qword_memory_corpus.py --verify
OPEN_DESKTOP_GIT_REVISION=$(git rev-parse HEAD) RUSTC_VERSION="$(rustc --version)" \
  cargo build --offline -p od-runner
python3 tests/cpu/generate_stage2_xadd_qword_memory_corpus.py --run-interpreter
python3 tests/cpu/generate_stage2_xadd_qword_memory_corpus.py --verify-results
```

The evidence records the exact corpus and fixture digests, runner executable
hash, source revision, compiler, target, build profile, host identity, command
arguments, repository working directory, and complete result for each repeat.
The child process receives only the selected recorded variables `PATH`,
`LANG`, `LC_ALL`, `RUST_LOG`, and `RUST_BACKTRACE`; absent values are omitted
from its environment and retained as `null` in provenance. Replay verification
checks the frozen stdout against the saved observation, the independent oracle,
all provenance, and equality of both repeats.

The result summary is first written as incomplete and checkpointed after each
case. On interruption, completed attempts remain in the checkpoint and every
uncompleted case/repeat is listed in `missing_attempts`. Timeout records retain
partial stdout/stderr (invalid UTF-8 is replacement-decoded) and a separate
timeout diagnosis with the configured duration. Focused regressions exercise
these paths, malformed output and field types, stable repeats, and saved replay:

```sh
python3 -m unittest tests/cpu/test_stage2_xadd_qword_memory_corpus.py
python3 -m py_compile tests/cpu/generate_stage2_xadd_qword_memory_corpus.py \
  tests/cpu/test_stage2_xadd_qword_memory_corpus.py
```

The explicit native request is a negative check: it exits nonzero, records
`status: blocked`, `native_x86: not_run_form_not_admitted`, and
`backend_execution: not_started` before any backend execution:

```sh
python3 tests/cpu/generate_stage2_xadd_qword_memory_corpus.py --native
```

This is serial local-interpreter evidence over private synchronous memory. It
does not establish one backend operation or rollback on errors; those are
separately exercised by the focused S2-21d tests. Neither S2-42y nor those
focused checks establish cross-thread atomicity, SMP behavior, or hardware
atomicity. No native or physical hardware execution is performed; S2-21 and
parent S2-42 remain open.
