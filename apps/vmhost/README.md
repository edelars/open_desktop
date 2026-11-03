# VMHost S0 smoke process

`Sources/main.swift` is a headless VMHost smoke process. It accepts one launcher connection over an inherited Unix `socketpair`, performs the version handshake, runs one validated x86 fragment through the public `od-host` C ABI, services cooperative stop requests, and returns the architectural result. The wire format is specified in [`protocols/control/CONTROL_PROTOCOL_V0.md`](../../protocols/control/CONTROL_PROTOCOL_V0.md). It does not implement a persistent VM lifecycle or a guest machine.

Build and run on macOS ARM64 from the repository root:

```sh
cargo build --release -p od-host --offline
xcrun swiftc -module-cache-path /private/tmp/open-desktop-module-cache -parse-as-library -O -import-objc-header apps/vmhost/Sources/VMHostABI.h apps/vmhost/Sources/ControlFrame.swift apps/vmhost/Sources/main.swift target/release/libod_host.a -ldl -lpthread -lm -framework Security -framework SystemConfiguration -framework CoreFoundation -framework CoreServices -framework Foundation -o target/od-vmhost-smoke
codesign --force --sign - --options runtime --entitlements apps/vmhost/Configuration/vmhost-smoke.entitlements target/od-vmhost-smoke
codesign --verify --strict --verbose=2 target/od-vmhost-smoke
xcrun swiftc -module-cache-path /private/tmp/open-desktop-module-cache -parse-as-library -O -import-objc-header include/open_desktop/host_abi.h apps/vmhost/Sources/ControlFrame.swift apps/macos/Sources/launcher.swift target/release/libod_host.a -ldl -lpthread -lm -framework Security -framework SystemConfiguration -framework CoreFoundation -framework CoreServices -framework Foundation -o target/open-desktop-launcher-smoke
target/open-desktop-launcher-smoke --vmhost "$PWD/target/od-vmhost-smoke" --hex '48 2b c1 75 fb' --rax 3 --rcx 1 --budget 32
```

The launcher accepts `--stop-after-ms N` for a cooperative-stop check. For example, `--hex 75fe --stop-after-ms 25 --budget 18446744073709551615` runs a self-loop until STOP is delivered. Both programs are local smoke binaries, not a packaged app.

Run the control-lifecycle regression scenarios with `python3 tests/system/vmhost_control_race_smoke.py target/open-desktop-launcher-smoke target/od-vmhost-smoke`. It repeats immediate STOP 100 times, checks a finite fragment that completes before its stop delay, sends incomplete header and payload bytes while execution is active, and closes a channel during an active fragment.

If the helper returns an error, the launcher reports it, sends SIGTERM, waits up to 500 ms, then uses SIGKILL if the helper did not exit, and reaps the child. If the control channel closes because the helper has already terminated, the launcher reports its exit status or terminating signal when available (for example, `VMHost terminated by SIGSEGV`).

Run the two-instance crash/stop isolation scenario with `python3 tests/system/vmhost_isolation_smoke.py target/od-vmhost-smoke`. It starts two active VMHost processes, sends SIGSEGV to one helper, and verifies that the other still accepts STOP and returns its result. This simulates a fatal process exit; it does not induce a fault inside generated code.

Build the dedicated generated-code-fault helper with the same control framing and host ABI:

```sh
xcrun swiftc -module-cache-path /private/tmp/open-desktop-module-cache -parse-as-library -O -import-objc-header apps/vmhost/Sources/VMHostABI.h apps/vmhost/Sources/ControlFrame.swift tests/system/vmhost_generated_fault_helper.swift target/release/libod_host.a -ldl -lpthread -lm -framework Security -framework SystemConfiguration -framework CoreFoundation -framework CoreServices -framework Foundation -o target/od-vmhost-generated-fault-smoke
codesign --force --sign - --options runtime --entitlements apps/vmhost/Configuration/vmhost-smoke.entitlements target/od-vmhost-generated-fault-smoke
python3 tests/system/vmhost_generated_fault_smoke.py target/open-desktop-launcher-smoke target/od-vmhost-generated-fault-smoke
```

The test-only helper completes HELLO/READY/RUN, then emits and executes a two-instruction ARM64 fault stub from MAP_JIT memory. The launcher must report `VMHost terminated by SIGSEGV`. The stub tests supervisor behavior; it is separate from the `od-jit` fault unit test and is not part of the shipping VMHost.

To verify local app-bundle assembly, ad-hoc signatures, and JIT execution from inside the bundle, run `sh tests/system/vmhost_app_bundle_smoke.sh target/open-desktop-launcher-smoke target/od-vmhost-smoke apps/vmhost/Configuration/vmhost-smoke.entitlements`. The script creates and removes a temporary `.app` bundle. This is a local smoke bundle, not a packaged distribution or notarized product.

The VMHost ad-hoc signature enables Hardened Runtime with `com.apple.security.cs.allow-jit`. This confirms local signed JIT execution on the tested M3 Max only. It does not confirm Developer ID signing, notarization, a packaged VMHost distribution, or another Mac model.

S3-38a routes the existing v0 `od_fragment_execute` call through the eager
register-only block dispatcher. Its bounded cache and MAP_JIT arena exist only
for the synchronous call and are released before RESULT returns; no Swift
protocol or C ABI layout changed. Local arithmetic, stop-race, two-process
isolation, and app-bundle regressions passed on macOS ARM64. The generated-fault
process smoke timed out once in three runs and passed twice on retry; its cause
is unlocalized and it is not a clean gate. See the
[S3-38a report](../../docs/reports/STAGE_3_S3-38a_2026-10-29.md).
