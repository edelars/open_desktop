# S2-42t: bounded LOCK XADD memory corpus

S2-42t adds a deterministic corpus for exactly five bytes,
`F0 48 0F C1 03` (`LOCK XADD qword [RBX], RAX`), under schema-v2 profile
`s2-long-integer-v1`. The code mapping is execute-only at `0x100000` and is
exactly five bytes. A single 128-byte private data mapping at `0x200000` is
read/write and non-executable. The qword operand begins at offset 1 or 120;
every other byte is the `0xa5` canary. The stop address is `0x100005`, the
instruction budget is one, and the fixture timeout is 2,000 ms.

This is a serial local interpreter profile over private synchronous `FlatMemory`.
It checks byte results and architectural state. It does not establish full x86
LOCK ordering, cross-thread or SMP atomicity, shared-memory behavior, DMA or
MMIO behavior, hardware `#AC`, split-lock behavior, or general memory ordering.
The broader LOCK/backend contract remains open in ISSUE-017. Native execution
is explicitly denied by this corpus command; it is not native-ready and no
physical-hardware acceptance is claimed.

## Literal controls and generated cases

The controls directory contains 32 ordered handwritten fixtures: pairs 00–15,
each with an unaligned offset 1 case followed by an aligned last-valid offset
120 case. `controls_manifest.json` pins every fixture digest and every expected
snapshot digest. `expected.json` contains the 32 static full snapshots; the
corpus generator reads and verifies these records and never calculates control
answers from its arithmetic oracle. S2-21f's existing
`LOCK XADD qword [RAX], RAX` alias/split fixture is a separate case and is not
included or relabeled here.

The generated set has 1,024 cases: 256 for each of four fixed seeds
`243f6a8885a308d3`, `13198a2e03707344`, `a4093822299f31d0`, and
`082efa98ec4e6c89`. Even sequences use offset 1 and odd sequences use offset
120. Inputs vary full-width source and memory values, permitted RFLAGS bits,
and other GPRs. Every byte outside the qword operand stays the fixed `0xa5`
canary in controls and generated cases. R15 contains a seed/sequence tag, making every
generated architectural input unique independent of case IDs.

The expected-state oracle uses integer arithmetic only. It computes
`R = (M + S) mod 2^64`, carry from the wide sum, signed overflow from
`(~(M xor S) & (M xor R)) >> 63`, auxiliary carry from `(M xor S xor R) >> 4`,
and sign, zero, and low-byte even parity. It changes flags under mask `0x8d5`
and preserves all other bits. The expected full snapshot checks RAX receives
the old memory value, the memory operand receives the sum, all other GPRs and
the full code/data mappings remain exact, the stop RIP is reached, one
instruction retires, and no exception is reported. It does not invoke the
production decoder or arithmetic helpers.

The bundle manifest orders controls before generated inputs and binds each
fixture and expected snapshot digest plus a whole-corpus digest. Verification
rejects missing, extra, duplicate, symlinked, traversal, or byte-modified
inputs. The saved local summary stores both complete runner results and frozen
process records for every repeat, including command, exit status, stdout, and
stderr. Result verification replays each snapshot against the independent
expected record and checks the executable, source revision, compiler/build,
machine, input digests, and repeat stability.

## Generate and verify

From the repository root:

```sh
python3 tests/cpu/generate_stage2_lock_xadd_corpus.py --generate
python3 tests/cpu/generate_stage2_lock_xadd_corpus.py --verify
```

Build the local runner, run every case twice through the interpreter, then
verify the frozen evidence:

```sh
OPEN_DESKTOP_GIT_REVISION=$(git rev-parse HEAD) RUSTC_VERSION="$(rustc --version)" \
  cargo build --offline -p od-runner
python3 tests/cpu/generate_stage2_lock_xadd_corpus.py --run-interpreter
python3 tests/cpu/generate_stage2_lock_xadd_corpus.py --verify-results
```

A successful interpreter run records `native_x86: not_run_form_not_admitted`.
The explicit denial regression is:

```sh
python3 tests/cpu/generate_stage2_lock_xadd_corpus.py --native
```

This command must exit nonzero, write a machine-readable blocked reason, and
report `backend_execution: not_started`. It must not invoke a native backend.

Focused checks:

```sh
python3 -m unittest tests/cpu/test_stage2_lock_xadd_corpus.py
python3 -m py_compile tests/cpu/generate_stage2_lock_xadd_corpus.py tests/cpu/test_stage2_lock_xadd_corpus.py
```
