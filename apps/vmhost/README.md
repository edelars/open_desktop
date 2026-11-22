# VMHost S0 smoke process

`Sources/VMHostMain.swift` (until S11-05a `main.swift`: SwiftPM treats a file called `main.swift` as top-level code, which conflicts with `@main`) is a headless VMHost smoke process. It accepts one launcher connection over an inherited Unix `socketpair`, performs the version handshake, runs one validated x86 fragment through the public `od-host` C ABI, services cooperative stop requests, and returns the architectural result. The wire format is specified in [`protocols/control/CONTROL_PROTOCOL_V0.md`](../../protocols/control/CONTROL_PROTOCOL_V0.md). It does not implement a persistent VM lifecycle or a guest machine.

## Swift Package (S11-05a)

`apps/Package.swift` ([ADR-0006](../../docs/decisions/ADR-0006-desktop-app-shell.md), solution (a)) builds VMHost; it is the only normative build system (S11-05c removed the former `xcrun swiftc` command lines; the application bundle is built by `python3 tools/build/build_app_bundle.py`). The shared sources moved to `apps/macos/Sources/Shared/` (target `ODShared`, with `ControlFrame.swift` from here), the atomic shim to `apps/macos/Sources/ODAtomics/` (target `ODAtomics`, shared by the VMHost writer and the UI reader); `git mv` was used, there are no copies.

- `od-vmhost` (executable): `Sources/InputChannel.swift`, `Sources/FrameTransport.swift`, `Sources/BundleLock.swift`, `Sources/Lifecycle.swift`, `Sources/ControlV2Host.swift` (S11-04), `Sources/VMHostMain.swift`; depends on `ODShared`, `ODAtomics`, `ODHostABI`. It is the only target that links `target/<profile>/libod_host.a` (linker settings of the manifest; debug for `swift build`, release for `-c release`) and the system frameworks listed in the manifest.
- `ODHostABI` (C): `Sources/ODHostABI/include/VMHostABI.h` (engine C ABI declarations and helpers, includes `include/open_desktop/*.h` and the atomic shim) plus a translation unit that compiles it. It declares symbols and links nothing. Used by `od-vmhost` and its tests only; the launcher smoke gets the ABI constants from the separate target `ODHostConstants` (`apps/macos/Sources/ODHostConstants/`, only `host_abi.h`), so it does not see the session or display ABI nor the VMHost helpers.
- `OpenDesktopVMHostTests` (XCTest): `Tests/CursorTransportTests.swift`, `Tests/TwoScreenTransportTests.swift` (S10-15), `Tests/LifecycleTests.swift` (S11-04, `swift test --filter LifecycleXCTests`, 10 tests). It depends on `od-vmhost` (`@testable import od_vmhost`), so the test bundle links `libod_host.a` as well: that is the code of `od-vmhost`, not a second consumer of the library in the manifest. Build the staticlib before `swift test`.

```sh
cargo build -p od-host --offline            # debug staticlib; add --release for `swift build -c release`
(cd apps && swift build && swift test)
apps/.build/debug/open-desktop-launcher-smoke --vmhost "$PWD/apps/.build/debug/od-vmhost" --hex '48 2b c1 75 fb' --rax 3 --rcx 1 --budget 32
```

The binary is `apps/.build/debug/od-vmhost` (it replaces the `target/od-vmhost-smoke` of the former `swiftc` recipe). The smoke scripts in `tests/system/` take the paths as optional arguments and **default to `apps/.build/debug/`**. SwiftPM links the binary ad hoc, without hardened runtime and without entitlements; MAP_JIT works in that state, so the smokes below run the unsigned debug binary. The entitled and signed helper (`com.apple.security.cs.allow-jit`, hardened runtime) is the one inside the application bundle, see `vmhost_app_bundle_smoke.sh` below.

The launcher accepts `--stop-after-ms N` for a cooperative-stop check. For example, `--hex 75fe --stop-after-ms 25 --budget 18446744073709551615` runs a self-loop until STOP is delivered. Both programs are local smoke binaries, not a packaged app.

Run the control-lifecycle regression scenarios with `python3 tests/system/vmhost_control_race_smoke.py [LAUNCHER [VMHOST]]`. It repeats immediate STOP 100 times, checks a finite fragment that completes before its stop delay, sends incomplete header and payload bytes while execution is active, and closes a channel during an active fragment.

If a frame is cut short, the launcher gives the child up to 200 ms to
produce a wait status. It reports a signal or nonzero exit from that
status. A clean exit leaves the frame error as the diagnostic. A
live child goes through SIGTERM, the existing 500 ms cleanup window
and SIGKILL if needed.

Run `python3 -B tests/system/vmhost_launcher_eof_smoke.py LAUNCHER`
to check delayed signal, delayed nonzero exit after a partial frame,
clean exit, cleanup of a child that stays live after closing the channel,
and immediate handling of an invalid frame.

Run the two-instance crash/stop isolation scenario with `python3 tests/system/vmhost_isolation_smoke.py [VMHOST]`. It starts two active VMHost processes, sends SIGSEGV to one helper, and verifies that the other still accepts STOP and returns its result. This simulates a fatal process exit; it does not induce a fault inside generated code.

The dedicated generated-code-fault helper (`tests/system/vmhost_generated_fault_helper.swift`) is **not a SwiftPM target** (`apps/Package.swift` has no target for it; adding one is a separate change). It is the only place that still builds with `xcrun swiftc`; this is a debugging recipe, not normative. From the repository root:

```sh
cargo build --release -p od-host --offline
xcrun swiftc -module-cache-path /private/tmp/open-desktop-module-cache -parse-as-library -O -import-objc-header apps/vmhost/Sources/ODHostABI/include/VMHostABI.h apps/macos/Sources/Shared/ControlFrame.swift tests/system/vmhost_generated_fault_helper.swift target/release/libod_host.a -ldl -lpthread -lm -framework Security -framework SystemConfiguration -framework CoreFoundation -framework CoreServices -framework Foundation -o target/od-vmhost-generated-fault-smoke
codesign --force --sign - --options runtime --entitlements apps/vmhost/Configuration/vmhost-smoke.entitlements target/od-vmhost-generated-fault-smoke
python3 tests/system/vmhost_generated_fault_smoke.py [LAUNCHER [target/od-vmhost-generated-fault-smoke]]
```

The test-only helper completes HELLO/READY/RUN, then emits and executes a two-instruction ARM64 fault stub from MAP_JIT memory. The launcher must report `VMHost terminated by SIGSEGV`. The stub tests supervisor behavior; it is separate from the `od-jit` fault unit test and is not part of the shipping VMHost.

To verify local app-bundle assembly, ad-hoc signatures, and JIT execution from inside the bundle, run `sh tests/system/vmhost_app_bundle_smoke.sh [APP_BUNDLE [LAUNCHER]]`. Without arguments it builds `target/app-bundle/OpenDesktop.app` with `python3 tools/build/build_app_bundle.py` and `apps/.build/debug/open-desktop-launcher-smoke` with `swift build`, checks `plutil -lint` and `codesign --verify --strict` of the helper and the application, and runs a fragment through `Contents/Helpers/od-vmhost`. This is a local ad-hoc signed bundle, not a packaged distribution or notarized product. (The three-to-five argument form `LAUNCHER VMHOST ENTITLEMENTS [PRESERVED_APP_BUNDLE [CODESIGN_TOOL]]` with a hand-assembled bundle remains only for the frozen `tests/cpu/stage3_harness.py`; it is not normative.)

The VMHost ad-hoc signature enables Hardened Runtime with `com.apple.security.cs.allow-jit`. This confirms local signed JIT execution on the tested M3 Max only. It does not confirm Developer ID signing, notarization, a packaged VMHost distribution, or another Mac model.

S3-38a routes the existing v0 `od_fragment_execute` call through the eager
register-only block dispatcher. Its bounded cache and MAP_JIT arena exist only
for the synchronous call and are released before RESULT returns; no Swift
protocol or C ABI layout changed. Local arithmetic, stop-race, two-process
isolation, and app-bundle regressions passed on macOS ARM64. The generated-fault
process smoke timed out once in three runs and passed twice on retry; its cause
is unlocalized and it is not a clean gate. See the
[S3-38a report](../../docs/reports/STAGE_3_S3-38a_2026-10-29.md).

## VMHost inside `OpenDesktop.app` (S11-05b)

`python3 tools/build/build_app_bundle.py` (see [`apps/macos/README.md`](../macos/README.md)) copies the SwiftPM binary `od-vmhost` to `OpenDesktop.app/Contents/Helpers/od-vmhost` (source for the path choice: not found, see that README) and signs it ad-hoc with `--options runtime` and `apps/vmhost/Configuration/vmhost.entitlements` (`com.apple.security.cs.allow-jit`); the application has no JIT entitlement. `vmhost-smoke.entitlements` stays for the smoke scripts of the previous sections (identical content, separate file). The application finds the helper at that path; `--vmhost PATH` of the application overrides it.

## S10-06 frame publication (test-only display smoke)

`Sources/FrameTransport.swift` is the writer of [frame transport v2](../../protocols/control/FRAME_TRANSPORT_V1.md) (v1 plus the cursor, S10-14, section below): POSIX shm created by VMHost (random name, `shm_unlink` at once, one `ftruncate`), slots and seqlock publication, `ANNOUNCE` with `SCM_RIGHTS`, `MODE`/`FRAME` notification queue, `VERSION` reply to a foreign `HELLO`. Atomics go through the C shim `apps/macos/Sources/ODAtomics/include/FrameTransportAtomics.h` (target `ODAtomics`, shared with the UI reader; included by `Sources/ODHostABI/include/VMHostABI.h`, together with `include/open_desktop/display_abi_v1.h`); the ARM64 code generation of the shim was checked by disassembly (`ldapr`, `stlr`, `str`, `ldr`, `dmb ish`, `dmb ishld`). The frame is copied once: `od_display_session_read_frame` writes straight into the slot.

`--display-smoke pattern` is **test-only** and is never selected implicitly: VMHost without a guest creates an `OD_DISPLAY_PROFILE_TEST_PATTERN` session (64x48 BGRX8888 bars, seed 0), publishes its frame and notifies the reader over the socket given by `--socket-fd`. Options: `--display-poll-hz N` (check rate, default 60) and `--display-hold-until-claim 1` (holds publication until the reader writes a non-zero `reader_claim`, so a reader can observe generation 0 deterministically). The mode ends on EOF (exit status 0); a protocol error (foreign magic, malformed `HELLO`, bytes from the reader after `HELLO`) or a host refusal exits non-zero.

Build: `cargo build -p od-host --offline`, then `(cd apps && swift build)` (see the Swift Package section above).

Run the transport smoke: `python3 tests/system/display_transport_smoke.py [VMHOST]`. It starts VMHost with the socketpair on fd 3, receives `ANNOUNCE` with `recvmsg`, checks `fstat` before `mmap`, observes generation 0 and then the published generation, reads the slot with the seqlock re-check, and compares the raw frame with `tests/fixtures/guest/stage10/bars_64x48_seed0.raw` through `tests/system/display_compare.py` at tolerance 0. It also checks the `VERSION` reply to a `HELLO` with another version. The test pattern profile never changes after create, so a second generation or a `MODE` change is not produced by this scenario; that transition is not covered here.

## S10-16 input channel (host part; no delivery to the guest)

Spec: [`protocols/control/INPUT_TRANSPORT_V1.md`](../../protocols/control/INPUT_TRANSPORT_V1.md). `Sources/InputChannel.swift` is the VMHost end of the `ODIT` channel. It exists **only with `--input-fd N`** (test-only `--display-smoke` mode; without the flag the program is unchanged): it sets `FD_CLOEXEC` and non-blocking mode on that descriptor, checks every header before its payload (`recvmsg`, a descriptor or `cmsghdr` is a protocol error), answers `HELLO` with `ACCEPT` and reports `STATUS` counters at most every 100 ms. The message coding (`apps/macos/Sources/Shared/InputProtocol.swift`) and the bounded queue of section 7 (`apps/macos/Sources/Shared/InputQueue.swift`, 256 records, DOWN refusal, motion merging, GAINED_PENDING) are the same source files as in the UI, so both builds above list them. An error on this channel closes only this channel (FOCUS LOST is queued if it was FOCUSED); frame transport goes on.

**There is no consumer.** `od-host` has no input ABI (section 10), so records stay in the queue: it fills to 256 and then merges motions and refuses DOWNs. `ACCEPT` carries `GUEST_DELIVERY = 0`. Test-only `--input-trace 1` takes records from the head through `popForConsumer` (which repeats the `mode_change_count` check at the queue exit, section 8.2 item 4) and prints them to stderr as `input-trace ...`; it is the only consumer and exists for the smoke.

Smoke (needs `apps/.build/debug/od-vmhost` built as above): `python3 tests/system/input_channel_smoke.py [VMHOST]`. It starts VMHost with frame transport on fd 3 and the input channel on fd 4, checks `ACCEPT` with `GUEST_DELIVERY = 0`, `STATUS` counters (`merged_motions`, `queue_depth`), a full queue without a consumer (`rejected_downs` grows, depth stays at most 256, `dropped_unfocused` in GAINED_PENDING), 24 protocol-error cases that close only the input channel while the frame socket stays open, the trace consumer (stale absolute position reset or dropped, `FOCUS LOST` instead of `UP`), EOF while focused, and EOF on both sockets with exit status 0. The unit tests of the shared queue and protocol are in `apps/macos/Tests/InputTests.swift` (see [`apps/macos/README.md`](../macos/README.md)).

## S10-14 cursor (writer side; written, awaiting review; no live cursor yet)

Spec: [`protocols/control/FRAME_TRANSPORT_V1.md`](../../protocols/control/FRAME_TRANSPORT_V1.md) section 10 (frame transport **v2**: `version = 2` in every message and in the memory header, so a v1 reader against this VMHost, and the reverse, gets "incompatible version"), device: [`docs/specs/DISPLAY_DEVICE.md`](../../docs/specs/DISPLAY_DEVICE.md) section 15.

`Sources/FrameTransport.swift` adds the cursor area to the same shared memory (header `cursor_offset = 384`, three descriptors at 448, three 16 KiB image slots at 16384, `cursor_claim` at byte 68, frame slots from 65536) and the `CURSOR` message (`kind = 5`, no pixels). On every display check (same rate as the mode check) the loop reads the cursor through the display ABI (`od_display_session_cursor_state`); if `cursor_seq` grew it picks a free slot (not `cursor_front`, not the slot of the untrusted `cursor_claim`), copies the image straight into it with `od_display_session_read_cursor_image` only when that slot holds another shape, writes the descriptor fields and publishes with a release store of `cursor_seq`, then queues `CURSOR`. A cursor change never writes a frame slot, sends `FRAME` or changes `generation`. The notification queue keeps one pending `CURSOR` (a new one replaces it; `MODE` and `FRAME` neither remove nor are removed by it): at most 48 + 48 + 40 + 32 = 168 bytes.

The test-only `--display-smoke pattern` session never writes a cursor register and the ABI has no test-only cursor call, so this mode publishes no cursor: `display_transport_smoke.py` checks the v2 header, the cursor area header, zeroed cursor descriptors and slots, and that no `CURSOR` arrives. **A live cursor from a guest has not been shown.** The writer and the reader of the cursor area are tested together, in one process, with synthetic `CursorSnapshot` values (the ABI itself is covered by the `od-host` tests):

```sh
cargo build -p od-host --offline
(cd apps && swift test --filter CursorTransportXCTests)
```

`apps/vmhost/Tests/CursorTransportTests.swift` covers: the v2 layout the writer produces, publication and the shape copy count (once per slot in the writer, once in the reader), slot choice under untrusted `cursor_claim` values (0, 1…3, 4, `0xFFFF_FFFF`), a slot rewritten between `g1` and `g2` (update discarded, state kept, recovery), 24 protocol violations one by one, 14 wrong header values of v2 and the cursor area (`INCOMPATIBLE`), and queue replacement and order. The test is an XCTest case since S11-05a (`CursorTransportXCTests`, 8 tests; it imports `od-vmhost` with `@testable` and links `libod_host.a` through that target). The smokes `display_transport_smoke.py` and `input_channel_smoke.py` were updated to v2 (HELLO with version 1 or 3 is answered with `VERSION`, version 2).

## Два экрана (S10-15, тестовый путь)

`--display-smoke pattern --display-screens 2` запускает профиль 4 (DISPLAY_HOST_ABI_V1.md, раздел 10): экран 0 — полосы 64×48, экран 1 — `checker` 80×60, seed `0x204080` (эталон `tests/fixtures/guest/stage10/checker_80x60_seed0x204080.raw`). Test-only, без гостя. Каждый экран публикуется в собственный объект памяти (FRAME_TRANSPORT_V1.md, раздел 11), `ANNOUNCE` идут по порядку экранов. Флаг только для smoke. Экран без режима (экран 1 при старте без гостя) не рвёт соединение: его объект получает пределы устройства 4096×2304, окно показывает «нет сигнала» (`DisplaySmoke.writerLimits`). Тесты: `cargo build -p od-host --offline`, затем `(cd apps && swift test --filter TwoScreenTransportXCTests)` — `apps/vmhost/Tests/TwoScreenTransportTests.swift`: приём дескрипторов, флаги и поля `MODE`/`FRAME`/`CURSOR`, граница суммарной памяти, очередь (256 байт, порядок отправки), независимые поколения, отключение экрана, состояние окна экрана 1 отдельно от состояния соединения, кадры профиля 4 против эталонов. Для сборки теста SwiftPM требует `target/release/libod_host.a` рядом с отладочной (в рабочей копии без release-сборки — символическая ссылка на `target/debug/libod_host.a`).

## S11-04 `--control-v2`: lifecycle of a test-only machine (written, awaiting review)

Spec: [`protocols/control/CONTROL_PROTOCOL_V2.md`](../../protocols/control/CONTROL_PROTOCOL_V2.md) (itself **not accepted**, so numbers and codes here can change with it); bundle and lock: [`docs/specs/VM_BUNDLE.md`](../../docs/specs/VM_BUNDLE.md) 2.1, 3.5, 7. A machine here is a display session of profile 1 (`display-test-pattern`) or 4 (`display-two-screens-test`): **no guest, no guest code, no disk reachable by a guest**. "Running" means that the session is stepped at 60 Hz and its (static) frame is published; the only observable sign of work is the poll counter in `STATS`. This is not a VM.

```sh
od-vmhost --control-v2 --socket-fd 3 --bundle /abs/path/Name.odvm [--display-fd 5] [--input-fd 4]
```

The mode is chosen only by `--control-v2`; control v0, v1 and `--display-smoke` are not changed. Wrong arguments, a descriptor that is not open, equal descriptors, a bundle path that is not absolute, longer than 1023 bytes or not a directory: exit status 2, no frames. Exit statuses (spec 12.3): 0 `STOP` done or EOF in `stopped`; 2 usage; 3 `lock` refused after `HELLO`; 4 protocol error or truncated frame (the connection is closed without a reply, also for a wrong version); 5 transport lost in `running`/`paused` or the output queue did not drain for 5000 ms; 6 `failed`.

Files: `apps/macos/Sources/Shared/ControlV2.swift` (codec, target `ODShared`; reused by the application and the CLI), `Sources/Lifecycle.swift` (state machine and the order of checks of spec 7.2, request IDs and the ring of 64, `ATTACH_RESOURCE`; no sockets, tested with a fake runner), `Sources/ControlV2Host.swift` (`poll(2)` loop, `recvmsg` with ancillary data, output queue and backpressure, `STATS`, the display session runner), `Sources/BundleLock.swift` (`<bundle>/lock`).

**Implemented**

- `lock` (VM_BUNDLE 2.1) at process start, before `HELLO`: `open(O_RDWR|O_CREAT|O_EXCL|O_CLOEXEC|O_NOFOLLOW, 0600)` + `flock(LOCK_EX|LOCK_NB)`; on `EEXIST` `open` without `O_CREAT|O_EXCL`; `fstat` regular file; `lstat` identity after the lock; `ENOENT` at step 2 repeats once. Swift over `open`/`flock` (called directly from Darwin); there is no C ABI for the lock in `od-host` and none was added (open question 12 of VM_BUNDLE section 11). On failure the answer to a valid `HELLO` is a fatal `ERROR` (`busy`, `lock_changed`, `lock_invalid`, `io_error`; `failed_kind = 0`, request ID 0) instead of `READY`, then exit 3; a first frame that is not `HELLO` is exit 4, and so is EOF before `HELLO` when the lock failed (the controller got neither `READY` nor a reason). With the lock held, EOF before `HELLO` is exit 0 (EOF in `stopped` without a request in flight, spec 12.3): the spec does not name EOF before `HELLO` as a separate case (a gap of the spec, not edited here).
- `HELLO`/`READY` (profile 2), `CONFIGURE` (every field of spec 6.1 against its limit; structure errors are fatal, value errors are `invalid_config`/`invalid_name` with the payload offset in `aux0`, state unchanged), `QUERY` (`STATE`, any state), `START`, `PAUSE`, `RESUME`, `STOP`, request ID rules (strictly sequential, replay of a completed request returns the stored `ACK`/`ERROR` with the `replayed` flag, `request_id_conflict`, `request_id_stale`), `STATE_CHANGED` before the terminal frame.
- `START` checks in the order of spec 7.3: a disk (`disk_count > 0`) is `unsupported_durability` before anything else (so is a non-zero operation ID, and `STOP` of a machine with a disk); then `unsupported` for `network`, `audio`, `clipboard_sync`, `boot_media`, shared folders (`aux0` is the offset of the first such field); then `host_memory_exceeded` (`memory_mib` above half of `hw.memsize`). Then the profile session is created; an ABI failure there is `failed` (`STATE_CHANGED -> failed`, fatal `ERROR host_failure`, exit 6).
- `PAUSE` stops polling and publication at once (the safe point is immediate, `pause_timeout` cannot happen here); the last frame stays in its slot. `RESUME` starts polling again; a hook for the `TIME_SYNC` of S11-22 is the empty `ProfileRunner.resume(reason:)`.
- `STOP` in `running`/`paused`: session destroyed, `STATE_CHANGED -> stopped`, `ACK`, EOF, exit 0. In `stopped`: `ACK` without `STATE_CHANGED`, exit 0 (spec 7.2 note 1).
- `STATS` (32 bytes) not more often than once per 1000 ms, only in `running` and `paused`; the 1000 ms are counted from the moment the frame is put in the output queue, and `state` is read at that moment (a delayed `STATS` never carries `running` after `paused`): `poll_count` (steps of the display session since START), `frames_published`, `rss_bytes` (`task_info(MACH_TASK_BASIC_INFO).resident_size`, the current resident size, not the peak). Without `--display-fd` the session is stepped and counted but nothing is published (`frames_published` stays 0).
- `--display-fd`: frame transport (FRAME_TRANSPORT_V1.md, `FrameTransport.swift` unchanged) on that descriptor after `START`: `HELLO` of the reader is read without blocking, then `ANNOUNCE` with `SCM_RIGHTS`, `MODE`, `FRAME`. An error on this socket closes only the frame link, the machine goes on. `--input-fd`: the existing input channel (`InputChannel.swift`) is serviced in the same loop; the v2 smoke does not exercise it (`input_channel_smoke.py` covers the channel in the `--display-smoke` mode).
- `ATTACH_RESOURCE`: `recvmsg` with ancillary room for 64 descriptors; every received descriptor is set `FD_CLOEXEC` and, unless exactly one arrived untruncated, closed at once, so no outcome leaves a surplus descriptor open; two or more descriptors, `MSG_CTRUNC`, another `cmsg`, a descriptor in a read that does not start a frame, a descriptor with another kind, none with `ATTACH_RESOURCE` are protocol errors (exit 4). Roles `boot_media`, `disk`, `shared_folder_root`, `drop_file` are checked as in spec 8 (`fstat` type, `fcntl(F_GETFL)` access mode, slot conflicts, 8 `drop_file`) and the descriptor is **stored by (role, index) and not used for anything**. The write lease of a disk outside the bundle, the open of the image on `START`, `shared_folder_root` and `drop_file` semantics belong to later tasks.

**Deviations from the spec and the plan (written down on purpose)**

- "In flight" (spec 5): the implementation is strictly sequential, which is read as "the terminal response is put in the output queue before the next request is read". So `operation_in_progress` and `in_progress` are never sent. This differs from the letter of spec 5, where a request is in flight until its terminal response is *sent*; with a full output queue the next request is not read (backpressure, 12.2), so a controller cannot observe a difference except the absence of those two answers.
- Plan S11-04 item 1: the `lock` is a Swift wrapper over `open`/`flock` in `od-vmhost`, without a C ABI in `od-host`; VM_BUNDLE section 11 item 12 stays an open question.

**Not implemented / answered `unsupported`:** `SET_CLIPBOARD` and kind 40-43 (`unsupported`), `SHUTDOWN` (always `unsupported_durability` in `running` until S9-06, `invalid_state` elsewhere; `unsupported_no_agent` is reachable only after S9-06; `shutting_down` is never entered), `pause_timeout`, requests in flight (every request completes before the next frame is read, so `in_progress` and `operation_in_progress` are never sent), `logs/vmhost.log` and its rotation (VM_BUNDLE 2.2), a handler for `SIGTERM` (the default action ends the process; the kernel drops the lock), the open of a disk image by `dir_fd` + name (no disk can start before S9-06).

Smoke: `python3 tests/system/vmhost_control_v2_lifecycle_smoke.py [apps/.build/debug/od-vmhost]` (about 5 s; build `cargo build -p od-host --offline`, then `cd apps && swift build`). It covers HELLO/READY, six-plus invalid `CONFIGURE`, a valid one, replay, `request_id_conflict`/`request_id_stale`, non-zero operation ID, `PREPARE_DISK` answers, `START` with the frame transport on a second socket (`ANNOUNCE` with a descriptor, `MODE`, `FRAME`), `STATS` (counter grows; frozen while paused; at most 1 Hz, gaps between two STATS at least 0.9 s), `PAUSE`, `RESUME`, `STOP`, exit 0; disk, devices and memory refusals; `ATTACH_RESOURCE` with real `SCM_RIGHTS` descriptors (modes, types, conflicts); 21 protocol errors (status 4, no reply); the lock (`busy` for a second VMHost, `lock_invalid` for a directory, a symlink and a FIFO, EOF before `HELLO` with a busy lock = 4 and with a held lock = 0, reuse after exit); usage errors (status 2); EOF while running (status 5). Unit tests: `swift test --filter ControlV2XCTests` and `--filter LifecycleXCTests`.

## PREPARE_DISK (S11-08a)

`PREPARE_DISK` ([CONTROL_PROTOCOL_V2.md section 9](../../protocols/control/CONTROL_PROTOCOL_V2.md)) is answered in `stopped` only (table 7.2; any other state is `invalid_state`). `Lifecycle.prepareDisk` checks in this order: the request ID rules (section 5), the state, then `ControlV2PrepareDisk.validate()` (format, zero capacity, disk name: `invalid_argument` / `invalid_name`), then calls the storage ABI of S11-07. The VMHost opens `<--bundle>/disks` itself (`open(O_DIRECTORY | O_NOFOLLOW | O_RDONLY | O_CLOEXEC)`; a missing directory or a symlink is `io_error` with `errno`, the directory is never created here) and calls `od_storage_create_raw_v1(dir_fd, name, name_len, capacity_bytes, 0, &error)` (`include/open_desktop/storage_abi_v1.h`, included by `ODHostABI`'s `VMHostABI.h`; the symbols come from `libod_host.a`). The version `od_storage_abi_version()` is compared with the header once per request; a mismatch is `failed` (`host_failure`, status 6). Without a bundle path (unit tests only) the answer is `unsupported`.

The ABI call is synchronous and has no progress callback, so the only `PROGRESS` is the final one (`completed == total == capacity_bytes`) before the `ACK`; a refusal sends no `PROGRESS`. Replays of a request ID are answered from the ring (`ACK` with the replayed flag, never a second creation). The durability of the created file is not claimed (ISSUE-055, S9-06).

Mapping of the statuses of `od_storage_create_raw_v1` to `ERROR` codes (`DiskImageStorage.map`, tested in `LifecycleTests.swift`):

| ABI status | `ERROR` code | `aux0`, `aux1` | Source |
|---|---|---|---|
| `OK` (0) | `ACK ok` | | CONTROL_PROTOCOL_V2 section 9 |
| `INVALID_ARGUMENT` (1) | `invalid_argument` | `8`, `0` (capacity) | section 9 ("not a multiple of 512, above `i64::MAX`"); with a non-zero `os_errno` (a bad `dir_fd`) it is `io_error` instead: decision without source |
| `INVALID_NAME` (2) | `invalid_name` | `0`, `0` | section 9 |
| `ALREADY_EXISTS` (3) | `disk_exists` | `0`, `0` | section 9 and 12.1 code 15 |
| `INSUFFICIENT_SPACE` (6) | `insufficient_space` | required bytes, available bytes | section 9 and 12.1 code 16 |
| `IO_ERROR` (11) | `io_error` | `errno`, `0` | section 9 |
| `NOT_FOUND`, `NOT_REGULAR_FILE`, `WRITER_BUSY`, `INVALID_IMAGE`, `TRUNCATED`, `SIZE_MISMATCH` (4, 5, 7-10) and any other number | `io_error` | `os_errno`, `0`; the status number is in `detail` | decision without source: these are results of `inspect`, not of `create` (STORAGE_HOST_ABI_V1.md), so they must not happen. `WRITER_BUSY` in `create` (STORAGE_HOST_ABI_V1.md, section 7) means that the entry is kept: after `io_error` a file may remain in `disks/`, and a repeat then gets `disk_exists`. No protocol code is added |

The `INVALID_ARGUMENT` status with `os_errno` 0 is mapped to `invalid_argument` (`aux0` 8, the capacity): the VMHost checks the capacity before the call, so the ABI can answer it only for an invalid `struct_size` or non-zero `flags`, which is a bug of the host itself. This mapping is a decision without source, kept deliberately so that such a bug is not reported as a client error only if the VMHost keeps its own check first. A version mismatch of the ABI is `failed` with `host_failure`, exit 6: decision without source (control v2 does not define the place of this check).

Tests: `python3 tests/system/vmhost_control_v2_prepare_disk_smoke.py` (creation and `stat`, replay, conflict, existing name, bad names and capacities, no space, `running`/`paused`, missing and linked `disks/`) and `swift test --filter LifecycleXCTests`.
