# S2-42v: register CMPXCHG local corpus and replay protocol

## Scope

This protocol covers only the two existing S2-21c qword register encodings:

| Bytes | Instruction | Register relationship |
|---|---|---|
| `48 0F B1 D9` | `CMPXCHG RCX, RBX` | RAX is accumulator; RCX is destination; RBX is source |
| `48 0F B1 C9` | `CMPXCHG RCX, RCX` | source aliases destination |

Each case executes one instruction in long mode with a one-instruction budget.
The corpus has 16 frozen hand controls and 1,024 deterministic generated inputs.
It includes equality and mismatch for both encodings, exercises set and clear
outcomes for CF, PF, AF, ZF, SF, and OF, and checks all 16 GPRs, RIP, defined
RFLAGS, code bytes, outcome, retirement count, and exception observation.

The independent expected-state oracle uses Python integer arithmetic for
`RAX - RCX`; it does not call the decoder or interpreter. The frozen control
fixtures and complete expected snapshots are pinned by a SHA-256 manifest.
Generated input identity includes the exact code bytes, all GPRs, and initial
RFLAGS. Four fixed seeds and 256 sequences per seed make the generated set
reproducible.

## Local commands

From the repository root:

```sh
python3 tests/cpu/generate_stage2_cmpxchg_qword_register_corpus.py --generate
python3 tests/cpu/generate_stage2_cmpxchg_qword_register_corpus.py --verify
python3 tests/cpu/test_stage2_cmpxchg_qword_register_corpus.py
```

When a debug `od-runner` is available, run and then verify the saved local
interpreter evidence:

```sh
python3 tests/cpu/generate_stage2_cmpxchg_qword_register_corpus.py --run-interpreter --runner target/debug/od-runner
python3 tests/cpu/generate_stage2_cmpxchg_qword_register_corpus.py --verify-results --runner target/debug/od-runner
```

The saved record binds the corpus manifest, executable hash, source revision,
host and runner build provenance, exact replay command, working directory,
selected environment variables, raw stdout, and two identical observations per
case. An interrupted run writes an incomplete checkpoint with completed
observations and a list of missing attempts; it cannot be accepted as passed.
The evidence directory is local under `work/stage2/` and is not a physical
hardware result.

## Native boundary

Native execution is explicitly denied for this profile. Requesting `--native`
records `status=blocked`, `native_x86=not_run_form_not_admitted`, and
`backend_execution=not_started`, then exits unsuccessfully. No native backend
is invoked and no physical x86 claim is made.
