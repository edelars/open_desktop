# S2-42k: bounded branch NOP reducer

S2-42k adds an interpreter-only reducer for one exact Long64 fixture shape. It
accepts a single RX code region containing `JZ/JNZ rel8 target`, zero or more
single-byte NOPs before `MOV RAX, imm64`, more NOPs, the retained target
`MOV RBX, imm64`, trailing NOPs, and the stop address. It deletes whole NOP
instructions only. The branch and both MOV instructions remain; the target is
kept attached to the same `MOV RBX` instruction as its address moves, and the
rel8 displacement, full byte mask, code length, and stop RIP are regenerated.

The accepted encoding is deliberately narrow: `74/75 rel8`, `90`, `48 B8
imm64`, and `48 BB imm64`. `a+b` is at most 117 and total NOP count is at most
128, keeping the code within a bounded rel8 form. The full code region must fit
in one 4 KiB page. The entry and mapping base are fixed. The reducer preserves
the initial state, flags, profile, feature controls, MOV immediates, and
instruction budget. The budget must allow the full fallthrough path. Other
instructions, data regions, memory operands, faults, and unsupported execution
outcomes are outside this slice.

The oracle calculates branch selection directly from RFLAGS.ZF bit 6 and the
literal opcode. It computes the full GPR/RIP/RFLAGS state, code memory and
retired instruction count without calling the x86 decoder or interpreter. A
qualifying mismatch is only a repeatable opposite branch path whose complete
observed state matches that independently calculated alternate path; the MOV
RAX immediate must differ from initial RAX. Any other state or outcome drift
is unsupported and stops reduction.

Each original fixture, proposal, and final fixture is confirmed with two
interpreter callbacks. The fixed schedule tries NOPs in the three groups in
order; it is bounded by at most 128 proposals and 260 total callbacks. This is
a deterministic bounded reduction, not a global minimum guarantee. The exact
original input bytes and digest are retained for original callbacks and
evidence; mutated candidates use canonical fixture JSON and their own digest.

Run the local correct-fixture smoke case with:

```sh
cargo run -p od-runner --offline -- \
  --reduce-branch-nops \
  tests/fixtures/x86/stage2/s2_42k_branch_jz_taken_nop.case.json \
  --limit 4 \
  --output-dir work/stage2/s2-42k-branch/jz-smoke
```

The command should report `NoInitialMismatch` after two interpreter callbacks.
It writes the original fixture, callback records/results, and reducer report
under the selected fresh child directory. The fixture is a correctness smoke
case only; it does not claim a CPU defect or native backend result.
