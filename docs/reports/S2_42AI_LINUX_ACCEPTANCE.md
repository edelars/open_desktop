# S2-42ai Linux REP STOSB reducer acceptance

**Date:** 2026-11-04  
**Host:** `senya@192.168.1.39`, temporary checkout `/tmp/open_desktop-s2-42ai-acceptance`  
**Commit:** `8bc984cd74b6e1d53241f4ba602dc0be755dc2dd`  
**Target:** Linux x86_64, `x86_64-unknown-linux-gnu`  
**Execution kind:** `unknown`  
**Physical Mac mini:** `not_run`

The user-specified `/home/senya/projects/open_desktop` checkout had a different
ref and unrelated dirty files, so it was left untouched. A clean temporary
checkout was created from the reviewed S2-42ai branch bundle. The generated
S2-42i corpus verified at digest
`85206b1c6b8ae1b3bd62a5470b87682b1d5cda2138bb337bee00ec7305c1aafd`.

## Native reducer run

The bounded run used the native-admitted RX fixture
`s2_42i_hand_last_count_1_df_0.case.json`, containing one `F3 AA` execution
with RCX=1 and DF=0. Command:

```sh
cargo run --offline -p od-x86-runner -- \
  --reduce-native-rep-stosb \
  work/stage2/s2-42i-rep-stosb/s2_42i_hand_last_count_1_df_0.case.json \
  --limit 4 \
  --output-dir work/stage2/s2-42ai-native-rep-stosb/linux-rx-count1-df0-20261104
```

The command exited 0 with stop reason `no_initial_mismatch`. It performed two
identical supervised `native_x86` callbacks, proposed zero candidates, and
accepted zero candidates. Both initial result JSON files had SHA-256
`92de373b9f7d28e0082e985f26dcc39a9c57e78a5636360b3deb13b234d6f857`.
The reducer's complete snapshot matched its independent byte-fill oracle.

Saved replay command:

```sh
target/debug/od-x86-runner \
  --fixture work/stage2/s2-42ai-native-rep-stosb/linux-rx-count1-df0-20261104/original.case.json \
  --json
```

Replay exited 0 with backend `native_x86`, outcome `stop_address`, and one
retirement. Its state, RFLAGS definedness, code bytes, and full data bytes
matched the independent oracle snapshot. Replay JSON SHA-256:
`826f69c908f00eac7a2923f92d7a001daf26d36ae3e75dced2c41ab2637fb690`.

This smoke verifies the native callback, evidence path and replay for one
successful input. The input had no initial mismatch, so candidate shrinking and
real native mismatch reduction were not exercised.

## Verification and provenance

- Linux `cargo test --offline -p od-x86-runner native_v2::tests`: 221 passed,
  0 failed, 19 filtered.
- Local focused `od-runner` reducer regression: 1 passed; it covers the RX
  input, the X-only preflight rejection, the flags-mask definedness case and
  synthetic failure paths.
- Local formatting, affected checks and Graphify refresh passed as recorded in
  the implementation report.
- Source hashes matched the local committed files:

| Source | SHA-256 |
| --- | --- |
| `crates/od-testkit/src/rep_stosb_reducer.rs` | `749ec43260172b9f6bc01ebb2818076d2fed3beab20389c196fd81d82db11ff4` |
| `tools/x86-runner/src/native_v2.rs` | `efa39f3171b978fd53e5bba92fab4a42b4431c59232f8c3542d9dffe74e54351` |
| `tools/runner/tests/fixture_v2.rs` | `4cba1857d3e8e90b262f66efd6eba57a9ab0e2711b368dd3bbdcf761cde66311` |

Fixture SHA-256:
`e7c8d2636dfa4a57554f35b287685ddf385942ddee3865bc9a13ac43e1e4c6eb`. The
debug runner binary SHA-256 was
`97b0b00eb50ea0a19c10677433fc49f632cf7260449d09996c2831f1b67fdd9f`; the
reducer report SHA-256 was
`213c1a9cb93f31ad2f177b7a3bbe334c945a963cee4902a9245817c9abc1eac9`.

The machine reported Intel Core i5-3210M, kernel `7.0.0-38-generic`, page size
4096, and rustc `1.99.1 (58cf8f9c3 2026-10-13)`. The runner embedded target and
debug profile; its embedded source revision/compiler fields were unavailable.
The OS-reported `execution_kind=unknown` is preserved and does not establish
physical CPU identity. Raw artifacts and the verification record are preserved
under ignored `work/stage2/s2-42ai-native-rep-stosb/`; see the
[verification record](../../work/stage2/s2-42ai-native-rep-stosb/verification.json).

S2-42 remains open for the other corpus/reducer requirements and S2-21/39/40
dependencies. Physical Mac acceptance remains separate.
