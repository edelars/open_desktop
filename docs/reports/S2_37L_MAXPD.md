# S2-37l MAXPD implementation and coverage report

Date: 2026-10-29

## Implemented scope

The interpreter and schema-v2 test profile implement one local-only Long64
legacy `66 [optional adjacent REX.R/B] 0F 5F /r` XMM-register MAXPD instruction.
The form requires SSE state, all six MXCSR exception masks, DAZ/FTZ clear and
reserved MXCSR bits 16–31 clear. Any rounding-control value and prior sticky
status bits are accepted. Both full XMM operands are snapshotted; two
little-endian binary64 lanes use the raw SSE MAX selector. Invalid suppresses
new denormal status only within its own lane, then effective IE/DE are
aggregated for the single atomic destination and MXCSR commit.

The profile rejects memory forms, other mandatory prefixes, REX.W/X, duplicate
or nonadjacent prefixes, truncation, VEX/EVEX, and neighboring MAXPS/MAXSS/MAXSD
and MINPD encodings. Missing SSE or unsupported MXCSR refuses without mutation.
JIT/native/CPUID admission, unmasked delivery, #XM behavior, memory forms and
guest fault ordering remain closed. A handwritten XMM8←XMM9 fixture exercises
mixed-lane IE|DE. Physical acceptance remains part of the combined final Mac
mini substage.

## S2-37 coverage and dependencies

Completed local slices are RSQRTSS/RCPSS/RSQRTPS/RCPPS (S2-37a–d) and masked
MINSS/MAXSS/MINPS/MAXPS/MINSD/MAXSD/MINPD/MAXPD (S2-37e–l), with only the
bounded masked IE/DE subset from S2-38a. These slices do not establish the
complete SSE/SSE2 guest profile, nor do they enable CPUID SSE/SSE2.

Remaining S2-37 work is the adopted guest profile's exact scalar and packed
arithmetic and comparison inventory (including required ADD/SUB/MUL/DIV/SQRT
families and other selected SSE/SSE2 forms), followed by conversions. Each
selected form still needs decoder/profile bounds, operand and lane semantics,
MXCSR rounding and exception behavior, refusal/fault behavior, and an
independent oracle. The exact required set and order must be selected from the
guest profile before choosing another opcode. Full S2-38 exception delivery and
fault ordering, S2-39 FP/SIMD save/restore, and physical acceptance are separate
dependencies.

## Verification

- `cargo fmt --all -- --check` passed.
- `cargo test -p od-isa --offline maxpd_register_masked` passed: 3 focused tests,
  including all 256 register pairs, lane aggregation/priority, every RC value,
  sticky DE, aliasing, strict decoder negatives and atomic refusal.
- `cargo test -p od-runner --offline local_maxpd_masked_register` passed: the
  handwritten fixture, profile negatives, invalid MXCSR cases and JIT refusal.
- `cargo test -p od-testkit --offline` passed: 369 tests.
- `cargo check --workspace --all-targets --offline` passed.
- `cargo check --workspace --all-targets --offline --target
  x86_64-unknown-linux-gnu` passed.
- The native MAXPD refusal unit test was added. Its Linux-target execution could
  not link on this host (`aarch64-apple-darwin`): the Apple linker rejects GNU
  linker flags such as `--as-needed` and `-Bstatic`. The Linux target compiled
  successfully with `cargo check`; the native runtime test remains unexecuted.
- No Mac mini, SSH, or physical-hardware checks were run. Those remain deferred
  to the combined final hardware substage.

## Review

The independent documentation review found and corrected two issues: the
fixture index now describes the lane 1 destination selection accurately, and
the S2-37l dependency row names S2-36z, S2-37j/k, and bounded S2-38a. The
independent code review found no production-semantic defect and requested
isolated same-lane IE-over-DE and prior-DE coverage; literal assertions for both
cases were added and re-reviewed. Both code and documentation reviews finished
without remaining findings.
