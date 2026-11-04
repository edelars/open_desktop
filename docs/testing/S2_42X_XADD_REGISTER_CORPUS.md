# S2-42x: XADD RAX,RAX local interpreter corpus

## Scope and encoding

This protocol covers exactly the existing S2-21b qword register alias encoding
`48 0F C1 C0` (`XADD RAX, RAX`). Each schema-v2 fixture runs in long mode with
one instruction allowed. The code mapping contains exactly these four bytes and
is execute-only. The stop address is RIP+4, and each successful case must stop
there after one retirement without an exception.

This is a local interpreter corpus for one register alias form. It does not
admit other XADD encodings, widths, registers, memory forms, or LOCK-prefixed
forms. It does not establish native x86 behavior or full x86 coverage.

## Inputs and expected state

The pinned control set has 16 handwritten architectural inputs. It includes
RAX values 0, 1, `0x0f`, `0x10`, signed maximum and minimum, unsigned maximum,
alternating-bit patterns `0xaaaaaaaaaaaaaaaa` and `0x5555555555555555`, plus
additional mixed and near-boundary patterns. The controls, ordered manifest,
and complete static expected snapshots are under
[`tests/fixtures/x86/stage2/s2_42x_xadd_qword_register_controls/`](../../tests/fixtures/x86/stage2/s2_42x_xadd_qword_register_controls/).
The pinned control-manifest SHA-256 is
`a7cee6c93ae4a0c6772776469ff0637049d75e2221a88c64b83175c8ea9aef4a`.

The generated set contains 1,024 unique deterministic architectural inputs:
four fixed seeds, 256 sequences per seed. Every input varies the full GPR
state and initial permitted RFLAGS; directed RAX values ensure the generated
set also exercises the important ADD flag classes. Identity includes the exact
instruction bytes, all 16 GPRs, and initial RFLAGS.

The standalone Python integer oracle computes
`RAX' = (RAX + RAX) mod 2^64`. It derives CF from the wide carry, PF from even
parity of the result's low byte, AF from bit 4 of `RAX xor RAX xor result`, ZF
and SF from the result, and OF from signed-add overflow. It updates only
defined arithmetic flags under mask `0x8d5` and preserves the other defined
RFLAGS bits. The oracle does not call the production decoder or interpreter.

The full expected result checks all 16 GPRs, RIP, defined and preserved flag
bits, unchanged execute-only code bytes, stop-address outcome, exactly one
retired instruction, and an empty exception observation. The strict bundle
manifest binds every fixture and expected snapshot digest and rejects missing,
extra, symlinked, malformed, or modified files.

## Local commands

From the repository root, generate and verify the fixture bundle and run the
focused regressions:

```sh
python3 tests/cpu/generate_stage2_xadd_qword_register_corpus.py --generate
python3 tests/cpu/generate_stage2_xadd_qword_register_corpus.py --verify
python3 tests/cpu/test_stage2_xadd_qword_register_corpus.py
python3 -m py_compile tests/cpu/generate_stage2_xadd_qword_register_corpus.py tests/cpu/test_stage2_xadd_qword_register_corpus.py
```

Build the runner with source revision and compiler metadata, execute each case
twice in separate local interpreter processes, then verify the saved evidence:

```sh
OPEN_DESKTOP_GIT_REVISION=$(git rev-parse HEAD) RUSTC_VERSION="$(rustc --version)" \
  cargo build --offline -p od-runner
python3 tests/cpu/generate_stage2_xadd_qword_register_corpus.py --run-interpreter
python3 tests/cpu/generate_stage2_xadd_qword_register_corpus.py --verify-results
```

The saved record lives under
`work/stage2/s2-42x-xadd-register-corpus/`. It binds the fixture manifest,
runner executable hash, source revision, compiler/build and host identity. For
each process it stores the exact command, repository working directory,
sanitized allowlisted child environment, return code, stdout, stderr, and parsed
observation.
Verification checks that both observations for every case match the independent
oracle and are identical, and that runner revision and host identity match the
execution record.

The harness checkpoints results per case. After interruption, the final record
is non-passing: it preserves completed attempts, lists missing attempts, and
the CLI marks it failed. On a timeout it saves any partial stdout/stderr and a
timeout diagnosis. Neither condition can be accepted as a passed corpus run.

## Native boundary and limits

Native execution is explicitly denied. The denial command is:

```sh
python3 tests/cpu/generate_stage2_xadd_qword_register_corpus.py --native
```

It writes `status=blocked`,
`native_x86=not_run_form_not_admitted`, and
`backend_execution=not_started`, then exits unsuccessfully without invoking a
native backend. The evidence is local interpreter evidence only; it is not a
physical-hardware result and makes no claim about other XADD forms, memory or
LOCK semantics, or general CPU support.

## Implementation and regression files

- [Corpus generator and evidence runner](../../tests/cpu/generate_stage2_xadd_qword_register_corpus.py)
- [Focused corpus regressions](../../tests/cpu/test_stage2_xadd_qword_register_corpus.py)
- [Pinned handwritten control bundle](../../tests/fixtures/x86/stage2/s2_42x_xadd_qword_register_controls/)
