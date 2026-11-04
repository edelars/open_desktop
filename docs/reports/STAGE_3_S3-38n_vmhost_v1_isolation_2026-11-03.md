# S3-38n: V1 VMHost process isolation after helper SIGSEGV

Status: bounded local macOS ARM64 acceptance slice passed. This report covers
the new V1 two-helper scenario and its exact harness gate only.

## Scope and outcome

`tests/system/vmhost_control_v1_isolation_smoke.py` launches two independent
VMHost helper processes and completes V1 handshakes with both. Each accepts the
admitted `75 FE` (`JNZ -2`) loop under a distinct register snapshot and
unlimited budget. After SIGSEGV is sent only to helper A, the smoke requires
helper A to exit with `-SIGSEGV` and helper B to remain alive. Helper B accepts
STOP ID 22, returns STOP_ACK ID 22 before the full 64-byte RESULT, and reports
its unchanged GPRs, RIP, and flags with STOP reason. It then accepts the same
code under RUN ID 23 with fresh GPR state, ZF set, and budget one; the full
normal RESULT reports the fresh state, RIP 2, and one retired instruction. The
smoke closes B's control channel while idle and requires a clean exit. Socket
reads and process waits are bounded; `finally` closes channels and kills and
reaps any remaining children.

The focused smoke passed with this marker:

```text
VMHost control V1 isolation smoke passed: SIGSEGV stayed within helper A; helper B completed exact STOP, fresh RUN, and idle EOF
```

The dedicated exact-gate self-check passed 1/1. It pins the process-suite step,
command, 30-second outer timeout, source hash, binary path, and marker, and
rejects command/source/marker drift. The pre-existing V0 `process_isolation`
step remains separate. The full `--suite vmhost-process` was not run. No
generated-fault smoke or retry was run; ISSUE-044 remains open and unchanged.

## Reproduction

On the current macOS ARM64 source tree, the helper was rebuilt and ad-hoc
signed into `/tmp/s3-38n-run/artifacts/od-vmhost-smoke` with a run-scoped build
directory:

```sh
cargo build --release -p od-host --offline --target-dir /tmp/s3-38n-run/build-target
xcrun swiftc -module-cache-path /tmp/s3-38n-run/swift-module-cache -parse-as-library -O -import-objc-header apps/vmhost/Sources/VMHostABI.h apps/vmhost/Sources/ControlFrame.swift apps/vmhost/Sources/main.swift /tmp/s3-38n-run/build-target/release/libod_host.a -ldl -lpthread -lm -framework Security -framework SystemConfiguration -framework CoreFoundation -framework CoreServices -framework Foundation -o /tmp/s3-38n-run/artifacts/od-vmhost-smoke
xcrun codesign --force --sign - --options runtime --entitlements apps/vmhost/Configuration/vmhost-smoke.entitlements /tmp/s3-38n-run/artifacts/od-vmhost-smoke
xcrun codesign --verify --strict --verbose=2 /tmp/s3-38n-run/artifacts/od-vmhost-smoke
python3 tests/system/vmhost_control_v1_isolation_smoke.py /tmp/s3-38n-run/artifacts/od-vmhost-smoke
python3 tests/cpu/test_stage3_harness.py Stage3HarnessValidationTests.test_vmhost_process_v1_isolation_gate_is_pinned_once_and_fails_closed
```

The verification output confirmed a valid strict signature and
`com.apple.security.cs.allow-jit=true`. The process-smoke command exited 0;
the self-check ran exactly one test and exited 0.

## Source and artifact identity

- Source revision: `74b3a4eac9f21f1814f1fcba227bfaf78b4869e0`.
- Source fingerprint at the focused run: `8e676c5936d7ceac4dbb200ebf9abdf2d116b8c1b0cbb068eda4f593cecb1392`.
- Signed VMHost SHA-256: `1202b8c2574e0d44c449f90fd00bb82b00dd0d134a4897efe35b529df657c6e6`.
- `libod_host.a` SHA-256: `e3814318c2ca86c72ac47a44a9a1e60f40d94ab060070ccdc5ebbcbfaed4f02d`.
- Host: macOS 26.7.1 build 25G241, ARM64; Rust 1.99.0; Swift 6.4; `/usr/bin/codesign` SHA-256 `3635f18db517033a4786ea7ddb0acc1801e642eaf6171e208e24cb9a1922f518`.

These artifact hashes identify the temporary local build; binaries are not
committed. This result establishes process isolation for this admitted-code
scenario on this host. It does not establish broader V1 acceptance, Mini or
physical-x86 behavior, generated-fault behavior, or full Stage 3 completion.
