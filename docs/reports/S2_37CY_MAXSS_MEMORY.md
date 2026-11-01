# S2-37cy MAXSS m32 memory implementation report

Date: 2026-11-01

## Implemented scope

The local interpreter and schema-v2 runner implement exactly the legacy Long64
`MAXSS XMM0,m32 [RAX]` encoding `F3 0F 5F 00`, with one four-byte memory read.
The dedicated profile records SSE availability (CPUID.01H:EDX[25]), CR0.EM,
CR0.TS and CR4.OSFXSR. The memory span `[RAX,RAX+3]` must be canonical48. With
AC set, the bounded profile returns Unsupported before span validation or a
data-memory callback; with AC clear, unaligned addresses are admitted.

The profile requires all six MXCSR exception masks, DAZ and FTZ clear, and
reserved bits 16–31 clear. Rounding control and prior sticky status are
preserved. It reuses `Binary32::max_sse_raw_result`: ties and any NaN select
the raw memory operand and NaN raises IE; DE is raised only when no NaN is
present and either operand is subnormal. The operation changes only XMM0's low
dword and effective MXCSR IE/DE bits. Upper 96 XMM0 bits and unrelated state
remain unchanged. Read failures leave the complete CPU state untouched.

JIT and native backends remain unsupported for this profile, as do the generic
interpreter and fragment paths. A handwritten QNaN fixture checks raw RHS
selection, IE, high-96 preservation, and JIT/native refusal. The fixture records
Intel SDM Rev. 093 MAXSS Vol. 2B pp. 4-13–4-14, Vol. 1 §4.9.2, §§11.5.2.1–2
and Appendix D, Vol. 2A Table 2-20, and Vol. 3A §16.1.3.

## Integration boundary

This slice adds the decoder, interpreter, strict profile validation, runner
dispatch, focused behavior checks, and one literal fixture. The shared
`docs/plans/STAGE_2.md`, `docs/specs/CPU_CONTRACT.md`,
`docs/reports/STAGE_2_REPORT.md`, fixture README, and `docs/KNOWN_ISSUES.md`
remain unchanged for the planned integration pass. Physical x86 acceptance is
still part of the combined final Mac mini substage.

## Verification

- `cargo fmt --all -- --check` passed.
- `cargo check --workspace --all-targets --offline` passed.
- `cargo test -p od-isa maxss_memory_rax --offline` passed: 2 focused tests for
  exact decode, raw result/status, all RC values, seeded stickies, unaligned
  access, AC refusal, unsupported controls/MXCSR, and failed-read atomicity.
- `cargo test -p od-testkit --offline` passed: 453 tests.
- `cargo test -p od-runner --test fixture_v2 --offline` passed: 481 tests.
- Focused MAXSD regression and MAXSS literal runner fixture passed: 1 test each.
- `graphify update .` completed in the isolated worktree.

## Review

Independent code review is pending.
