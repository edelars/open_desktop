# S2-42ab — DIV BL divide-error corpus

## Scope and fault oracle

This local-only schema-v2 corpus exercises the exact Long64 encoding `F6 F3`
(`DIV BL`) and the existing S2-20a #DE behavior. It contains 1,024 generated
faulting inputs: 512 with `BL = 0` and 512 with nonzero `BL` and unsigned
`AX // BL > 255`. Eight pinned controls cover both fault classes, including
the quotient-overflow threshold for divisors 1, 2, 128, and 255.

The generator's independent integer predicate is exactly
`BL == 0 || AX // BL > 255`. Inputs with a valid quotient are rejected. In
particular, `AX = BL * 256 - 1` for nonzero `BL` has quotient 255 and is not a
fault-corpus input. The expected-state oracle does not decode or execute the
instruction: for every accepted input it expects the entire GPR array,
RFLAGS, definedness, RIP, and code bytes to remain unchanged, #DE vector 0,
unavailable error code and fault address, faulting RIP at the first opcode
byte, and zero retired instructions.

## Strict generation and replay evidence

The static control directory is `tests/fixtures/x86/stage2/s2_42ab_div_bl_fault_controls/`.
Its exact file set, fixture hashes, expected-state hashes, and control-manifest
digest are pinned by the generator. Generated inputs and aggregate expected
snapshots are serialized under `work/stage2/s2-42ab-div-bl-fault-corpus/`.
The corpus manifest binds case order, all input and expected-state digests,
control digest, seed set, and total accounting. Verification rejects missing,
extra, symlinked, or modified files and duplicate JSON keys.

Interpreter evidence records both runs per case, exact command and working
directory, selected environment values, runner executable hash, source
revision, build metadata, and host identity. Saved-result verification checks
those values against each frozen stdout, the independently generated expected
snapshot, and both repeat results. Timeout evidence retains partial streams
and timeout diagnosis; interruption saves completed repeats and enumerates
unfinished attempts.

```sh
python3 tests/cpu/generate_stage2_div_bl_fault_corpus.py --verify
python3 tests/cpu/test_stage2_div_bl_fault_corpus.py
python3 tests/cpu/generate_stage2_div_bl_fault_corpus.py --run-interpreter
python3 tests/cpu/generate_stage2_div_bl_fault_corpus.py --verify-results
python3 tests/cpu/generate_stage2_div_bl_fault_corpus.py --native
```

`--native` writes a blocked record and exits nonzero before any backend call:
DIV BL fault delivery is not admitted to the native suite. No remote host is
contacted. This protocol does not establish hardware #DE delivery, conflicting
fault priority, JIT behavior, or physical acceptance. S2-20a and S2-14
semantics are the only inherited CPU behavior under examination.
