# S2-42ad — IDIV BL divide-error corpus

## Scope and fault oracle

This local-only schema-v2 corpus exercises exact Long64 bytes `F6 FB`
(`IDIV BL`) against the existing local `#DE` behavior. It contains eight
handwritten pinned controls and 1,024 deterministic generated fault inputs,
split evenly between a zero signed divisor and signed quotient overflow.

The independent integer predicate interprets AX as signed 16-bit and BL as
signed 8-bit. It computes the quotient toward zero using integer magnitudes,
without floating point or Python floor division on negative values. A fault is
admitted exactly when BL is zero or the quotient is outside `[-128, 127]`.
Valid neighboring divisions at the `-128`, `127`, and `-127` boundaries are
rejected by the selector. The snapshot oracle does not decode or execute the
instruction: for every admitted input it expects all GPRs, RFLAGS and their
definedness, RIP, and code bytes unchanged; #DE vector 0; unavailable error
code and fault address; faulting RIP at the opcode; and zero retired
instructions.

## Strict generation and replay evidence

The static control directory is
`tests/fixtures/x86/stage2/s2_42ad_idiv_bl_fault_controls/`. Its exact file
set, fixture hashes, expected-state hashes, and manifest digest are pinned by
the generator. The generated manifest binds the ordered case list, fixture
and expected-state digests, control digest, seeds, and aggregate corpus digest.
Strict verification rejects extra, missing, symlinked, or modified files and
duplicate JSON keys. The pinned control-manifest SHA-256 is
`24bda9c454e217facd8f48ff4cbea12c9561c9e431d5d5ef7913a3f3afa1d5b9`.

Run the focused checks from the repository root:

```sh
python3 tests/cpu/test_stage2_idiv_bl_fault_corpus.py
python3 tests/cpu/generate_stage2_idiv_bl_fault_corpus.py --generate
python3 tests/cpu/generate_stage2_idiv_bl_fault_corpus.py --verify
python3 tests/cpu/generate_stage2_idiv_bl_fault_corpus.py --run-interpreter
python3 tests/cpu/generate_stage2_idiv_bl_fault_corpus.py --verify-results
python3 tests/cpu/generate_stage2_idiv_bl_fault_corpus.py --native
```

The corpus digest for the pinned generator and controls is
`bc58b424c0fb27e0421d0f8ea393560eaa468879b7a4cedc09758d8de8dd6560`.
Interpreter evidence records two executions per case, exact command and
working directory, selected environment values, runner executable hash,
source revision, build metadata, and host identity. Saved-result verification
binds those values to frozen stdout, the independently derived expected state,
and repeat stability.

The native request writes a blocked record and exits nonzero before backend
invocation because native `#DE` delivery is not admitted. No remote host is
contacted. This protocol makes no claim about native or physical fault
delivery, Linux signal/context mapping, JIT behavior, other IDIV encodings, or
completion of S2-14, S2-20, or parent S2-42. The instruction encoding and
signed byte-division behavior are sourced from Intel SDM revision 093, Vol.
2A, `IDIV`; the fixtures are project-owned test data, not physical oracle
results.
