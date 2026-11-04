# S2-42af: qword register BT-family corpus

S2-42af adds a local deterministic corpus for four exact Long64 register
forms: `BT RAX,RCX` (`48 0F A3 C8`), `BTS RAX,RCX` (`48 0F AB C8`),
`BTR RAX,RCX` (`48 0F B3 C8`), and `BTC RAX,RCX` (`48 0F BB C8`). The scope
reuses the implemented S2-15c register/register forms. It does not cover
aliases, immediate indices, memory operands, LOCK, or native acceptance.

## Contract and flags

The selected [Intel SDM revision 093, Volume 2A](https://cdrdv2.intel.com/v1/dl/getContent/671199)
describes the instruction encodings and operation. For the qword register
forms, the low six bits of RCX select a bit in RAX. CF receives the old
selected bit. BT leaves RAX unchanged; BTS sets the selected bit, BTR clears
it, and BTC complements it. ZF is preserved. OF, SF, AF, and PF are undefined.
The result mask is `0x443`, retaining comparison of defined CF/ZF and other
preserved tracked bits while excluding those undefined arithmetic flags.

The independent oracle uses Python integer shifts and bitwise operations only;
it imports no decoder, interpreter, native executor, assembler, or emulator.
Each expected result includes all 16 GPRs, RFLAGS and its definedness mask, RIP,
code bytes, normal stop outcome, one retired instruction, and no exception.

## Inputs and frozen controls

Each of the four exact forms has 16 pinned controls. They cover selected bits
0 and 1; offsets 0, 1, 63, 64, and 127; high RCX values whose low six bits
select the tested position; zero, all-ones, alternating, mixed, and boundary
RAX patterns; and varied initial flag probes. Literal fixture bytes and full
expected states are frozen in `tests/fixtures/x86/stage2/` and protected by a
control-manifest SHA-256.

Four fixed seeds produce 256 cases per seed per exact form (1,024 generated
states each). The generator covers all 64 low-six-bit indices, including
larger RCX values, and balances selected-bit zero/one outcomes. The ordered
manifest records each fixture and expected-state digest and the complete
corpus digest. Strict verification rejects changed controls, duplicate JSON
keys, extra/missing files, duplicate or missing generated states, and digest
mismatches.

## Local generation and replay

```sh
python3 tests/cpu/generate_stage2_bt_family_qword_register_corpus.py --generate
python3 tests/cpu/generate_stage2_bt_family_qword_register_corpus.py --verify
python3 tests/cpu/test_stage2_bt_family_qword_register_corpus.py
cargo build --offline -p od-runner
python3 tests/cpu/generate_stage2_bt_family_qword_register_corpus.py --run-interpreter
python3 tests/cpu/generate_stage2_bt_family_qword_register_corpus.py --verify-results
```

The generated bundle and two-repeat interpreter evidence are stored under
ignored `work/stage2/s2-42af-bt-family-qword-register-corpus/`. Evidence
binds ordered fixture digests, manifest, runner/source identity, machine and
build metadata, command/cwd/environment, stdout, full snapshots, and repeat
identity. Captured process output is decoded strictly as UTF-8 before JSON
parsing. Interrupted or malformed-output runs preserve completed attempts,
record available process diagnostics, and list missing/failed repeats.

Native evidence is `not_run/not_selected`. Although the runner recognizes the
existing register operation, native acceptance for this new corpus is outside
the five-suite batch and has not been executed. Physical execution is also
`not_run`. This slice does not close S2-15c, parent S2-42, or its dependencies.
