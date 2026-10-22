# S0-09 and S1-06 x86 byte fixtures

These project-owned hex inputs exercise the exact decoder/interpreter/JIT subset in [`CPU_CONTRACT.md`](../../../docs/specs/CPU_CONTRACT.md). The bytes are written in Intel opcode order and separated by spaces. Each positive `.hex` fixture has a `.case.json` companion with initial state, profile, timeout, comparison scope, provenance and a SHA-256 over decoded instruction bytes. `SHA256SUMS` fixes both fixture forms.

Run the supported cases from the repository root with `cargo run --offline -p od-runner -- ... --engine both`; each command compares interpreter and JIT results:

| Fixture | Command arguments after `--` | Expected state/result |
|---|---|---|
| `s0_09_add_overflow.hex` | `--hex-file tests/fixtures/x86/s0_09_add_overflow.hex --rax 18446744073709551615 --rcx 1 --budget 8 --engine both` | RAX=0, RCX=1, RIP=3, RFLAGS=0x57, reason=ReachedCodeEnd, 1 instruction |
| `s0_09_sub_jnz_loop.hex` | `--hex-file tests/fixtures/x86/s0_09_sub_jnz_loop.hex --rax 3 --rcx 1 --budget 32 --engine both` | RAX=0, RCX=1, RIP=5, RFLAGS=0x46, reason=ReachedCodeEnd, 6 instructions |
| `s0_09_cmp_jz.hex` | `--hex-file tests/fixtures/x86/s0_09_cmp_jz.hex --rax 9 --rcx 9 --budget 8 --engine both` | RAX=9, RCX=9, RIP=5, RFLAGS=0x46, reason=ReachedCodeEnd, 2 instructions |
| `s0_09_mov_imm64.hex` | `--hex-file tests/fixtures/x86/s0_09_mov_imm64.hex --budget 8 --engine both` | RAX=0x8877665544332211, RIP=10, RFLAGS=2, reason=ReachedCodeEnd, 1 instruction |

The unsupported fixture is rejected rather than approximated:

```sh
cargo run --offline -p od-runner -- --hex-file tests/fixtures/x86/s0_09_unsupported.hex --budget 8 --engine both
```

The command must exit with an invalid-code/decode error. These fixtures cover only the limited fragment profile; they do not validate a bootable x86 CPU.

The JSON companions now feed the v1 runner. Run each positive case once per engine with `--fixture CASE.json --engine interpreter|jit --json`; stdout contains one result JSON document. Compare saved outputs with `--compare-results RESULT_A.json RESULT_B.json --case CASE.json --json`. The `s0_09_unsupported.case.json` companion exercises the explicit `unsupported` result path and exits with code 4. Expected exit is part of the fixture contract, not a recorded native result. These runner cases still do not provide RAM or exception coverage.

Six additional, manually encoded boundary cases for signed overflow, subtraction borrow, signed comparison, taken/not-taken JZ and initial flag preservation are documented in [`S1_06_BOUNDARY.md`](S1_06_BOUNDARY.md). Each case has a strict v1 fixture and is included in `SHA256SUMS`.
