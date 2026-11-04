# S2-42ah: native adapter for the bounded JZ/JNZ branch reducer

S2-42ah adds a `NativeX86` callback adapter for the exact S2-42k forward
`JZ`/`JNZ rel8` NOP-deletion reducer. It does not widen the accepted fixture
shape or change native admission, CPU semantics, or JIT behavior.

The testkit exposes a backend-parametrized reducer entry point. The existing
interpreter API remains an interpreter wrapper, while the native adapter
requires every callback result to identify `NativeX86` and pass the existing
schema-v2 result validation. A wrong backend, unsupported result, timeout,
malformed identity, or callback failure cannot preserve a mismatch.

The CLI is:

```sh
od-x86-runner --reduce-native-branch-nops CASE.json [--limit N] [--output-dir DIR]
```

Each original, candidate, and final callback writes a journal record and result
hash. The original raw fixture bytes and digest are preserved; mutated
candidates use canonical fixture JSON. Native execution reuses the existing
supervised `--native-v2-child` path. The report stores initial/final result
hashes, candidate decisions and relocation maps, expected-result hashes, exact
reducer and fixture replay argv, shell commands, working directory,
build/machine metadata, and the execution boundary. The reducer replay reads
the saved `original.case.json` and writes into a timestamped fresh child under
the original evidence directory, so it does not require the source fixture to
remain available or reuse an existing output directory. Physical Mac
acceptance is recorded separately and remains `not_run` unless performed
independently.

The default evidence directory is a fresh child under
`work/stage2/s2-42ah-native-branch/`. Run S2-42k's interpreter smoke command
from [its protocol](S2_42K_BRANCH_REDUCER.md) for a corresponding interpreter
baseline. Native results only establish behavior on the Linux x86_64 runner.

## Linux acceptance, 2026-11-04

The bounded Linux x86_64 smoke on the literal S2-42k JZ fixture passed with
`no_initial_mismatch` after two identical native callbacks. The saved-original
replay returned `native_x86` / `stop_address` with exit 0. No candidate was
proposed, so native NOP shrinking of a mismatching branch fixture remains
unverified. Source/binary hashes, test commands, callback hashes and the exact
local evidence path are in the
[acceptance report](../reports/S2_42AH_LINUX_ACCEPTANCE.md) and its linked
ignored `work/` verification record. Raw `execution_kind` remains `unknown`;
physical Mac acceptance remains `not_run`.
