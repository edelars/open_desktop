# S0-09 x86 byte fixtures

These project-owned hex inputs exercise the exact decoder/interpreter/JIT subset in [`CPU_CONTRACT.md`](../../../docs/specs/CPU_CONTRACT.md). The bytes are written in Intel opcode order and separated by spaces. `SHA256SUMS` fixes the committed fixture files.

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
