# S2-42ah — bounded Linux native branch reducer acceptance, 2026-11-04

## Result

The native adapter ran the literal
`tests/fixtures/x86/stage2/s2_42k_branch_jz_taken_nop.case.json` on Linux
x86_64. It completed two identical callbacks with `backend=native_x86` and
reported `no_initial_mismatch`. Replaying the saved `original.case.json`
returned `native_x86` / `stop_address`, exit 0.

The run proposed zero NOP candidates, accepted zero candidates, and produced no
reduced fixture. It confirms bounded native execution and saved replay for this
fixture; it does not exercise branch NOP shrinking or demonstrate a CPU defect.

## Provenance and verification

- Host: `senya@192.168.1.39`, Linux x86_64, Intel Core i5-3210M,
  kernel `7.0.0-38-generic`.
- Remote checkout HEAD: `0a0640b5a7d54b13b3024146b5c9068f0f15bb5d`, with a dirty
  working tree. The relevant branch reducer and native runner sources matched
  the local main source hashes listed in the verification record.
- `target/debug/od-x86-runner` SHA-256:
  `4ced29b08ddc09626173d0892d34786bb6821263f458d25649de5cfba771f2a4`.
  Embedded revision is unavailable; exact source hashes are preserved.
- `cargo test --offline -p od-testkit branch_reducer::tests -- --nocapture`:
  11 passed, 0 failed.
- Linux `cargo test --offline -p od-x86-runner native_v2::tests`:
  220 passed, 0 failed (19 filtered).
- Linux-target `cargo check` and `cargo fmt --all -- --check` passed.

The raw run report and saved replay are in ignored
`work/stage2/s2-42ah-native-branch/acceptance-20261104-jz-taken/`. The compact
[verification record](../../work/stage2/s2-42ah-native-branch/verification.json)
contains commands, source/binary and report hashes, callback results and
replay outcome.

`execution_kind` remains `unknown`; physical Mac acceptance is `not_run`. This
bounded result does not close the actual native mismatch-reduction criterion or
parent S2-42.
