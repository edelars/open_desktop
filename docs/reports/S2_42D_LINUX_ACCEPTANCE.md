# S2-42d Linux native XORPD acceptance

**Date:** 2026-11-04  
**Host:** `senya@192.168.1.39`, checkout `/home/senya/projects/open_desktop`  
**Target:** Linux x86_64  
**Execution kind:** `unknown`  
**Physical Mac mini:** `not_run`

## Result

The bounded native reducer command and saved native replay completed for both
literal S2-40b asymmetric XORPD fixtures. Each reducer run used two identical
initial native callbacks and returned `no_initial_mismatch`; neither fixture
entered candidate proposal or reduction. Replay returned `native_x86` with
`stop_address` and exit code 0. The run reports' complete snapshots matched the
independent oracle state, and code memory matched the unchanged-memory oracle.
This confirms command orchestration and native replay for these inputs, but does
not exercise actual native mismatch shrinking or establish physical CPU
identity.

| Fixture | Fixture SHA-256 | Initial result SHA-256 (both callbacks) | Proposals / accepted | Replay result SHA-256 |
| --- | --- | --- | --- | --- |
| `s2_40b_xorpd_xmm0_xmm1_asymmetric_a.case.json` | `531aea09d8db83b52ae181be15e747229a2e19e8a7d66a6c651333f837fbac04` | `044802c2d90229466297dae8ab5aa4e3360a5f1ca8d8ebdf3bcde27963018824` | `0 / 0` | `c3908fd978e98e7edd7bc0a0f28313967b891b04ecd2404f1cc0d2e4b55819b0` |
| `s2_40b_xorpd_xmm0_xmm1_asymmetric_b.case.json` | `393185f8fede3aef57f95a544ef427e9fd99f8f03100a1935327efe10c7a83cc` | `142f761bece131d7ec4118823c151786dda9741e699faa6df2fee0df49d61315` | `0 / 0` | `9185a85487888ae0fe670411a9a6a9028def14a8b0be837f9e59ff9fec8b6db5` |

Both cases used profile `s2-long-native-xorpd-xmm0-xmm1-v1`, budget 4, backend
`native_x86`, and stop reason `no_initial_mismatch`. Each replay exited 0 with
outcome `stop_address`. The native result snapshots included XMM0–XMM15 and
matched the report's independent `oracle.original_expected_state`.

## Verification

- `cargo fmt --all -- --check` passed on the local `aarch64-apple-darwin` host.
- Offline Linux-target `cargo check` for `od-testkit` and `od-x86-runner` passed
  on the local host.
- Focused local `cargo test --offline -p od-testkit native_xorpd`: 11 passed.
- Linux `cargo test --offline -p od-x86-runner native_v2::tests`: 221 passed.
- Both reducer runs and both native replays exited 0. Independent state and
  memory-oracle checks passed for both run reports and all initial/replay
  result records.

Remote source hashes matched the local source files. The remote checkout was
dirty at HEAD `0a0640b5a7d54b13b3024146b5c9068f0f15bb5d`; the acceptance used the
following verified source hashes:

| Source | SHA-256 |
| --- | --- |
| `tools/x86-runner/src/native_v2.rs` | `e3896cfed0ea247cc1fad63a373cdca3c319a054c09136a4fdfcde9ff9406f6c` |
| `tools/x86-runner/src/native_fp.rs` | `bb6eb0dd9b60a6a7e60651119ba0c7423f25aab27fa9eaf00aeb47e0e2c386bf` |
| `tools/x86-runner/src/main.rs` | `f7af2f35242cc4db399e6dab6aa626e50a4356bae0594b3799c526dc6f74dd42` |
| `crates/od-testkit/src/native_xorpd.rs` | `3e1ccb3d10372351abd6f5284243f0db0a93cabcb6c33e9bdd90f8a38dd3857e` |

The Linux debug runner binary SHA-256 was
`56530a7025fd6f6e7f6f0b183b389587f6085d439aa5cddb997b663f53a9a005`;
embedded build revision was unavailable. Detailed commands and per-run report
hashes are preserved in the ignored local evidence record
[`verification.json`](../../work/stage2/s2-42d-xorpd/verification.json), with
the raw run artifacts under `work/stage2/s2-42d-xorpd/`.

S2-42 remains open. This slice does not claim native mismatch reduction, known
physical CPU identity, physical Mac mini acceptance, or completion of the other
S2-42 groups and dependencies.
