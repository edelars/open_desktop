# S2-42u: exact qword memory CMPXCHG corpus

S2-42u exercises only the bare effective-REX.W encoding
`48 0F B1 18` (`CMPXCHG qword [RAX], RBX`) from S2-21e, under
`s2-long-integer-v1`. Code is execute-only at `0x100000`, four bytes long,
and the one-instruction stop is `0x100004`. The qword address is fixed at
`0x2003`: the private read/write, non-executable data mapping is split at
`0x2007`, so the eight-byte operand crosses the two adjacent regions exactly
as the existing S2-21e fixtures do. The instruction budget is one and timeout
is 2,000 ms.

This is serial local interpreter evidence on private synchronous memory. It
does not establish native admission, cross-thread atomicity, SMP behavior,
hardware memory ordering, or the broader S2-21 atomic contract. Native execution
is explicitly rejected before backend invocation; summaries record
`native_x86: not_run_form_not_admitted` and `backend_execution: not_started`.
Parent S2-42 and S2-21/S2-39/S2-40 remain open.

## Controls and generated inputs

Two handwritten controls pin equality and mismatch. Their fixture bytes and
static complete expected snapshots are in
[`s2_42u_cmpxchg_qword_memory_controls/`](../../tests/fixtures/x86/stage2/s2_42u_cmpxchg_qword_memory_controls/).
The SHA-256-pinned control manifest binds their ordered IDs, fixture digests,
snapshot digests, profile, and expected document digest. Runtime corpus
generation loads these fixed snapshots and never computes their expected
answers through the oracle.

The generated set contains 1,024 cases: 256 for each fixed seed
`243f6a8885a308d3`, `13198a2e03707344`, `a4093822299f31d0`, and
`082efa98ec4e6c89`. Inputs vary the old memory qword, RBX source, all other
non-address GPRs, and defined RFLAGS bits. R15 carries a seed/sequence tag;
the generator verifies all 1,024 complete architectural inputs are distinct.
The encoding, address, mapping layout, permissions, and canary bytes are fixed.
Each seed assigns 32 cases to each of eight old-memory classes: equality with
RAX, 0, 1, RAX−1, RAX+1, signed maximum, signed minimum, and −1. Across the
generated set this yields 128 equal stores, 896 mismatches, both carry and
no-carry mismatch outcomes, and 128 signed-overflow mismatches. The exact
expected `(CF,ZF,OF)` counts are `(0,1,0):128`, `(0,0,0):384`,
`(1,0,0):384`, and `(1,0,1):128`.

The integer-only expected-state oracle compares RAX with the old memory qword,
computes subtraction flags from `RAX - M` (CF, PF, AF, ZF, SF, OF), and then
applies the CMPXCHG destination rule: equality writes RBX to memory and leaves
RAX unchanged; mismatch writes old memory to RAX and leaves memory unchanged.
The complete snapshot includes all GPRs, RIP, defined flags, both complete
split data mappings including canaries, code bytes, one retired instruction,
stop outcome, and absence of exception. It does not invoke the production
decoder or arithmetic helpers.

The exact bundle verifier rejects missing, extra, duplicate, symlinked,
traversal, or byte-modified fixtures, malformed/duplicate JSON keys, changed
provenance, altered expected snapshots, and manifest digest changes. Saved-run
verification rechecks each full result against the independent oracle and
requires two stable repeats per fixture, frozen process stdout/argv/exit status,
the process working directory and selected environment (`PATH`, locale, and
Rust logging/backtrace variables), the exact corpus and fixture digests, runner
executable hash, source revision, compiler/build, target, and machine identity.
The run summary is checkpointed after each fixture and on interruption retains
the current fixture's completed/attempted repeats plus an explicit list of
missing repeats. Timed-out process rows retain any captured partial stdout and
stderr as UTF-8 text (replacement-decoding invalid bytes), with a separate
timeout diagnosis and duration; the timeout is not recorded as a generic
process-launch failure.

## Generate, run, and verify

From the repository root, build the local runner and generate the fresh corpus:

```sh
python3 tests/cpu/generate_stage2_cmpxchg_qword_memory_corpus.py --generate
python3 tests/cpu/generate_stage2_cmpxchg_qword_memory_corpus.py --verify
OPEN_DESKTOP_GIT_REVISION=$(git rev-parse HEAD) RUSTC_VERSION="$(rustc --version)" \
  cargo build --offline -p od-runner
python3 tests/cpu/generate_stage2_cmpxchg_qword_memory_corpus.py --run-interpreter
python3 tests/cpu/generate_stage2_cmpxchg_qword_memory_corpus.py --verify-results
```

The local run performs 1,026 cases twice. An explicit native request is a
negative check: it must return nonzero, write the blocked record, and avoid
backend execution.

```sh
python3 tests/cpu/generate_stage2_cmpxchg_qword_memory_corpus.py --native
python3 -m unittest tests/cpu/test_stage2_cmpxchg_qword_memory_corpus.py
python3 -m py_compile tests/cpu/generate_stage2_cmpxchg_qword_memory_corpus.py \
  tests/cpu/test_stage2_cmpxchg_qword_memory_corpus.py
```

Run outputs use the new `work/stage2/s2-42u-cmpxchg-qword-memory-corpus/`
directory and do not replace S2-42t or S2-21e evidence. No native or physical
execution is claimed.
