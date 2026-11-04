# S2-42w: local qword memory XCHG corpus

S2-42w covers exactly the existing S2-21a instruction bytes
`48 87 00` (`XCHG qword [RAX], RAX`) under the `s2-long-integer-v1`
profile. Code is execute-only at `0x100000`, has three bytes, and stops at
`0x100003`. Initial RAX is fixed at `0x200ffe`. The eight-byte operand crosses
the adjacent private read/write, non-executable regions split at `0x201000`:
the left region is `[0x200ff8, 0x201000)` and the right region begins at
`0x201000`. Other bytes are `0xa5` canaries. The instruction budget is one and
the timeout is 2,000 ms.

The independent integer oracle snapshots pre-instruction RAX before applying
the exchange. It writes that original `0x200ffe` value across the split qword,
sets final RAX to the arbitrary old memory qword, preserves every other GPR
and all defined RFLAGS bits, advances RIP by three, checks both complete data
regions and the code bytes, and requires exactly one retired instruction,
`stop_address`, and no exception. No subsequent instruction dereferences the
exchanged-in RAX value.

## Fixed controls and generated inputs

Two fixed controls use distinct arbitrary old-memory values. Their fixture
bytes and complete expected snapshots are stored in
[`s2_42w_xchg_qword_memory_controls/`](../../tests/fixtures/x86/stage2/s2_42w_xchg_qword_memory_controls/).
The controls manifest pins ordered case identities, fixture and snapshot
digests, and the generator pins the exact manifest digest. Runtime generation
loads the frozen snapshots; it does not derive control expectations through
the oracle.

The generated set has 1,024 inputs: 256 for each seed
`243f6a8885a308d3`, `13198a2e03707344`, `a4093822299f31d0`, and
`082efa98ec4e6c89`. It varies every non-address GPR, initial RFLAGS, and the
old memory qword. The generator verifies all complete architectural inputs
are unique. R15 also carries a seed and sequence tag. The bytes, mapping
addresses, permissions, and canary values stay fixed.

The complete corpus manifest orders the two controls before the 1,024
generated cases and binds fixture and expected-snapshot digests plus the
whole-corpus digest. Strict verification rejects missing, extra, duplicate,
symlinked, traversal, or modified files, malformed/duplicate JSON keys,
changed control provenance, altered static snapshots, and manifest changes.

## Local execution and evidence

Run from the repository root. Generate and verify the fresh corpus:

```sh
python3 tests/cpu/generate_stage2_xchg_qword_memory_corpus.py --generate
python3 tests/cpu/generate_stage2_xchg_qword_memory_corpus.py --verify
```

Build the local runner with its source/compiler identity, run each of the
1,026 cases twice through the interpreter, and verify the saved evidence:

```sh
OPEN_DESKTOP_GIT_REVISION=$(git rev-parse HEAD) RUSTC_VERSION="$(rustc --version)" \
  cargo build --offline -p od-runner
python3 tests/cpu/generate_stage2_xchg_qword_memory_corpus.py --run-interpreter
python3 tests/cpu/generate_stage2_xchg_qword_memory_corpus.py --verify-results
```

Saved replay verification binds exact fixture and corpus digests, complete
runner results, stable repeats, frozen argv/stdout/stderr/exit status, runner
executable hash, source revision, compiler/build, target, machine identity,
repository working directory, and the selected process environment. The child
runner receives only the recorded `PATH`, locale, and Rust logging/backtrace
variables. The summary is checkpointed after each fixture; interruption keeps
completed attempts and records the current repeat plus every missing repeat.
Timeouts preserve partial output and a separate timeout diagnosis.

The explicit native request is a negative check and must fail while recording
`status: blocked`, `native_x86: not_run_form_not_admitted`, and
`backend_execution: not_started` before backend execution:

```sh
python3 tests/cpu/generate_stage2_xchg_qword_memory_corpus.py --native
```

Focused checks:

```sh
python3 -m unittest tests/cpu/test_stage2_xchg_qword_memory_corpus.py
python3 -m py_compile tests/cpu/generate_stage2_xchg_qword_memory_corpus.py \
  tests/cpu/test_stage2_xchg_qword_memory_corpus.py
```

This is local interpreter evidence over private synchronous memory. It does
not claim that the corpus performs one atomic backend call, that an error
leaves memory unchanged, or that SMP behavior is correct; those obligations
belong to S2-21a's dedicated contract/tests. Native and physical hardware
execution are not performed. This slice does not close S2-21 or parent S2-42.
