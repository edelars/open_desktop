# macOS display viewer (S10-07, S10-11)

`Sources/display_viewer_main.swift` is a local smoke viewer, not the SwiftUI application of stage 11. It starts VMHost with the shared launch code (`Sources/Shared/VMHostLaunch.swift`, also used by `Sources/launcher.swift`) in the **test-only** mode `--display-smoke pattern`, receives frames over frame transport v2 ([`protocols/control/FRAME_TRANSPORT_V1.md`](../../protocols/control/FRAME_TRANSPORT_V1.md); v1 plus the cursor, S10-14) and shows them through Metal.

- `Shared/FrameReader.swift` — reader of the shared memory: `recvmsg` with `SCM_RIGHTS`, version and size checks, `fstat` before `mmap`, seqlock slot read with the generation re-check, reader states of section 7.2.
- `Viewport.swift` (S10-11) — pure integer geometry shared by the window, the offscreen dump and the unit test: `drawableSize` (physical = round(logical × backingScaleFactor)), `viewport` (`fit`: aspect-preserving letterbox/pillarbox, centred, rounded half up; `pixelExact`: 1 guest pixel = 1 drawable pixel, centred, cropped around the frame centre if the drawable is smaller: the viewport is clipped to the drawable and the texture coordinates are narrowed, so Metal never gets a viewport outside the render pass; with an odd positive difference the extra pixel goes to the right/bottom margin, with an odd negative one it is cropped on the left/top) and `guestPixel` (drawable pixel → guest pixel, the transform the mouse input of S10-16 uses; nil outside the frame rectangle).
- `MetalPresenter.swift` — `MTLTexture` (`bgra8Unorm`, bytes B, G, R, X as in the slot) filled once per frame with `replace(region:)`, fullscreen-triangle pipeline with nearest-neighbour sampling drawn into the viewport (the target is cleared to the background first, so margins are plain background), and the offscreen path (render into a texture of a given physical size, read back with `getBytes`).
- `DisplayWindow.swift` — AppKit window whose view is backed by a `CAMetalLayer`; the drawable is always bounds × `backingScaleFactor`; a resize or a scale change only redraws with a new drawable size and viewport (the shown texture is neither recreated nor dropped). "no signal", "changing mode", "disconnected" and "incompatible version" are a colour plus a text label, never a frozen frame.

The viewer links no Rust code: it only spawns VMHost, so `libod_host.a` is needed for the VMHost binary, not for the viewer. The atomic C shim `apps/macos/Sources/ODAtomics/include/FrameTransportAtomics.h` (target `ODAtomics`) is shared by the VMHost writer and the viewer reader (`FrameReader`); with direct `swiftc` builds it is imported with `-import-objc-header` (not copied).

## Swift Package (S11-05a)

`apps/Package.swift` ([ADR-0006](../../docs/decisions/ADR-0006-desktop-app-shell.md), solution (a)) builds the same code without the `xcrun swiftc` lines below; they stay valid (with the new paths) until S11-05c rewrites this README. No external dependencies. Targets that exist now:

| Target | Kind | Sources |
| --- | --- | --- |
| `ODAtomics` | C (header only plus one translation unit that compiles it) | `Sources/ODAtomics/include/FrameTransportAtomics.h` |
| `ODShared` | library, depends on `ODAtomics` | `Sources/Shared/`: `ControlFrame`, `ControlV2` (S11-04: codec of control protocol v2), `InputProtocol`, `InputQueue`, `VMHostLaunch`, `FrameReader`, `HIDUsage`, `InputTranslation`, `Viewport`. Public API for the other targets; no `FrameTransport`, no `libod_host.a` |
| `open-desktop-display-viewer` | executable | `DisplayWindow`, `MetalPresenter`, `InputChannel`, `display_viewer_main` (this directory) |
| `ODHostConstants` | C (header plus one translation unit that compiles it) | `Sources/ODHostConstants/include/ODHostConstants.h`: includes only `include/open_desktop/host_abi.h` (`OD_HOST_ABI_VERSION_V0`, `OD_FRAGMENT_MAX_LENGTH_V0`); no session or display ABI, no VMHost helpers |
| `open-desktop-launcher-smoke` | executable | `launcher.swift`; depends on `ODShared` and `ODHostConstants` (constants only), does not link `libod_host.a` |
| `OpenDesktopMacTests` | tests (XCTest) | `Tests/ViewportTests.swift`, `Tests/InputTests.swift`, `Tests/ControlV2Tests.swift` (`swift test --filter ControlV2XCTests`, 11 tests); `@testable import` of `ODShared` and of the viewer module, because the tests use internal API |

`OpenDesktop` (S11-05b, the SwiftUI application) is described in the next section; `odctl` does not exist yet and is not declared: S11-13 adds it with its sources. The VMHost targets (`od-vmhost`, `ODHostABI`, `OpenDesktopVMHostTests`) are described in [`apps/vmhost/README.md`](../vmhost/README.md).

From the repository root (the Rust staticlib first, because `od-vmhost` links `target/<profile>/libod_host.a`; `swift build` is debug, `-c release` needs `cargo build --release -p od-host --offline`):

```sh
cargo build -p od-host --offline
(cd apps && swift build && swift test)
(cd apps && swift build -c release)   # binaries: apps/.build/release/{od-vmhost,open-desktop-display-viewer,open-desktop-launcher-smoke}
```

SwiftPM links `od-vmhost` ad hoc without entitlements: sign a copy as in the `codesign` line below before running the smokes that execute JIT (`--entitlements apps/vmhost/Configuration/vmhost-smoke.entitlements`). The smokes take the binary paths as arguments, for example `python3 tests/system/display_viewer_dump_smoke.py apps/.build/release/open-desktop-display-viewer <signed copy of od-vmhost>`.

Build from the repository root with `xcrun swiftc` (VMHost first; the debug `libod_host.a` is enough, see [`apps/vmhost/README.md`](../vmhost/README.md)):

```sh
cargo build -p od-host --offline
xcrun swiftc -module-cache-path /private/tmp/open-desktop-module-cache -parse-as-library -O -import-objc-header apps/vmhost/Sources/ODHostABI/include/VMHostABI.h apps/macos/Sources/Shared/ControlFrame.swift apps/macos/Sources/Shared/ControlV2.swift apps/macos/Sources/Shared/InputProtocol.swift apps/macos/Sources/Shared/InputQueue.swift apps/vmhost/Sources/InputChannel.swift apps/vmhost/Sources/FrameTransport.swift apps/vmhost/Sources/BundleLock.swift apps/vmhost/Sources/Lifecycle.swift apps/vmhost/Sources/ControlV2Host.swift apps/vmhost/Sources/VMHostMain.swift target/debug/libod_host.a -ldl -lpthread -lm -framework Security -framework SystemConfiguration -framework CoreFoundation -framework CoreServices -framework Foundation -o target/od-vmhost-smoke
codesign --force --sign - --options runtime --entitlements apps/vmhost/Configuration/vmhost-smoke.entitlements target/od-vmhost-smoke
xcrun swiftc -module-cache-path /private/tmp/open-desktop-module-cache -parse-as-library -O -import-objc-header apps/macos/Sources/ODAtomics/include/FrameTransportAtomics.h apps/macos/Sources/Shared/VMHostLaunch.swift apps/macos/Sources/Shared/FrameReader.swift apps/macos/Sources/Shared/Viewport.swift apps/macos/Sources/MetalPresenter.swift apps/macos/Sources/Shared/InputProtocol.swift apps/macos/Sources/Shared/InputQueue.swift apps/macos/Sources/Shared/HIDUsage.swift apps/macos/Sources/Shared/InputTranslation.swift apps/macos/Sources/InputChannel.swift apps/macos/Sources/DisplayWindow.swift apps/macos/Sources/display_viewer_main.swift -framework AppKit -framework Metal -framework QuartzCore -framework Foundation -o target/open-desktop-display-viewer
```

Run:

```sh
# Window (closes with the window; --close-after-ms N closes it after N ms):
target/open-desktop-display-viewer --vmhost target/od-vmhost-smoke
# Offscreen, no window: render through Metal, read back, write the raw frame (B, G, R, X rows, stride = width * 4):
target/open-desktop-display-viewer --vmhost target/od-vmhost-smoke --dump-frame /private/tmp/frame_64x48.raw
python3 tests/system/display_compare.py tests/fixtures/guest/stage10/bars_64x48_seed0.raw /private/tmp/frame_64x48.raw --width 64 --height 48 --tolerance 0
python3 tests/system/display_viewer_dump_smoke.py [target/open-desktop-display-viewer [target/od-vmhost-smoke]]
# S10-11: scaled offscreen dump: logical window size 100x100 points at scale 2 -> physical drawable 200x200, frame letterboxed (viewport 0,25,200,150)
target/open-desktop-display-viewer --vmhost target/od-vmhost-smoke --dump-frame /private/tmp/frame_scaled.raw --dump-size 100x100 --backing-scale 2
# Pixel-exact (1 guest pixel = 1 drawable pixel, centred) in the window, or in a dump of a larger drawable:
target/open-desktop-display-viewer --vmhost target/od-vmhost-smoke --pixel-exact
python3 tests/system/display_viewer_scaled_smoke.py [target/open-desktop-display-viewer [target/od-vmhost-smoke]]
# Unit test of Shared/Viewport.swift (no Metal, no VMHost). **Deviation from the S11-05a plan:** the plan says to keep the old `xcrun swiftc` commands of the tests, but the test files are `import XCTest` now (no `@main`) and cannot be built with `swiftc` any more, so those commands are replaced by `swift test --filter ...` (reason: XCTest cases, not executables). From apps/:
(cd apps && swift test --filter ViewportXCTests)
```

Options: `--vmhost PATH` (required), `--profile pattern` (smoke only; the only value until S10-09 adds `probe`), `--dump-frame PATH`, `--dump-size WxH` (with `--dump-frame` only: the logical window size in points, default the frame size), `--backing-scale S` (with `--dump-frame` only: 1…4, default 1; the dumped drawable is round(W × S) × round(H × S) physical pixels), `--pixel-exact` (no value; default is aspect-preserving fit), `--timeout-ms N` (wait for ANNOUNCE and the first frame, default 10000), `--close-after-ms N`. Exit status: 0 success; 1 failure; 2 usage. In window mode failure means a startup error (Metal, spawn), a reader that ended in `DISCONNECTED` or `INCOMPATIBLE`, or a VMHost exit status other than 0 when the viewer stops it; the status is kept and returned by `exit` after `NSApp.terminate`. In offscreen mode: no frame, VMHost exit status other than 0 after EOF, I/O. On `DISCONNECTED`/`INCOMPATIBLE` the viewer unmaps and closes the socket and stops VMHost (EOF up to 2 s, SIGTERM, 500 ms, SIGKILL, reap) and shows VMHost's real exit status in the window text.

Without `--dump-size` and `--backing-scale` the dump has the frame size and is the 1:1 reference dump (tolerance 0). Every dump prints the guest frame size (`width`, `height`), `logical`, `physical`, `scale`, `mode`, `viewport=x,y,w,h` (full frame rectangle in drawable pixels, origin top-left, may start below 0 when pixel-exact crops) and `visible=x,y,w,h` (the part handed to Metal); the window mode prints `window geometry: logical=… physical=… scale=…` on exit. Logical (points) and physical (pixels) sizes differ on Retina and both belong in every measurement report. S10-16 uses `ViewportLayout.guestPixel` for the absolute mouse position (see the input section below).

The launcher smoke (`launcher.swift`) now also needs `Sources/Shared/VMHostLaunch.swift` in its compile command; the command in [`apps/vmhost/README.md`](../vmhost/README.md) shows it.

## OpenDesktop.app (S11-05b, written, awaiting review)

One command builds the application with VMHost inside (from the repository root, macOS only):

```sh
python3 tools/build/build_app_bundle.py            # release; prints app_bundle_path=<path> as the last stdout line
python3 tools/build/build_app_bundle.py --configuration debug --output /private/tmp/od-app
```

What it does: `cargo build --release -p od-host --offline` (the staticlib that only `od-vmhost` links), `swift build -c release --product OpenDesktop` and `--product od-vmhost` from `apps/`, then the layout below, `plutil -lint`, ad-hoc `codesign --options runtime` of the helper (entitlements `apps/vmhost/Configuration/vmhost.entitlements`: `com.apple.security.cs.allow-jit` only) and then of the application (`apps/macos/Configuration/OpenDesktop.entitlements`: empty, no App Sandbox, no JIT; ADR-0006 (b)), and `codesign --verify --strict` of both. The default output is `target/app-bundle/OpenDesktop.app` (`--output DIR` changes the directory).

```text
OpenDesktop.app/Contents/Info.plist            (apps/macos/Resources/Info.plist)
OpenDesktop.app/Contents/MacOS/OpenDesktop     (SwiftPM product OpenDesktop, apps/macos/Sources/App/)
OpenDesktop.app/Contents/Helpers/od-vmhost     (SwiftPM product od-vmhost)
```

- **Helper path `Contents/Helpers/od-vmhost`: source not found.** Context7 has no Apple documentation of the bundle structure (queries for SwiftPM and for bundle layout returned only SwiftPM manifest docs and Bazel `rules_apple`, whose `additional_contents` says only that files go to a chosen subdirectory of `Contents`). The choice is a convention, not a verified Apple rule. A reviewer recalls that Apple TN2206 mentions `Contents/Helpers` for helper programs; this was not checked through Context7 and is not relied on. What was checked on this machine instead: `codesign --verify --strict` and `--strict --deep` pass for the helper and for the application, the helper keeps its own entitlements (`codesign -d --entitlements -`), and the helper runs from that path (`open-desktop-launcher-smoke --vmhost .../Contents/Helpers/od-vmhost` returns a result). The alternative is `Contents/MacOS`. Note that `_CodeSignature/CodeResources` of the application records `Helpers/od-vmhost` with a `cdhash` and a `requirement`, that is, the helper is signed as nested code: the application must be signed after the helper, which the script does.
- **The application finds VMHost inside its bundle** (`Bundle.main.bundleURL + Contents/Helpers/od-vmhost`, `Sources/App/AppOptions.swift`); `--vmhost PATH` (or `--vmhost=PATH`) overrides it for smoke. Unknown arguments of the system are ignored; `--vmhost` without a value exits with code 2. The application does not link `libod_host.a`, does not execute guest code and does not spawn VMHost yet (S11-06 and later).
- **Content of the window:** title "Open Desktop", the text "Библиотека машин — S11-06" and one line with the VMHost path and whether it is found. This is an intermediate fact: the window opens, the library does not exist.
- **Deployment target:** `apps/Package.swift` declares `platforms: [.macOS(.v14)]` and `Info.plist` has `LSMinimumSystemVersion` 14.0: the oldest macOS verified by the project (M1 Pro, macOS 14.6.1, `docs/testing/STAND.md`) and the value of `tests/system/vmhost_app_bundle_smoke.sh`. Before S11-05b SwiftPM built for 12.0 (checked with `vtool` on an empty package). The bundle was built and run on macOS 26.7 only; **running on macOS 14 was not re-checked for this bundle**.
- `CFBundleIdentifier` is `dev.opendesk.OpenDesktop` (the smoke bundle uses `dev.opendesk.stage0.smoke`); it is a working value, to be fixed with the Developer ID work of stage 12.
- Not checked: the window and its text by eye (only that a window of the process exists and is on screen), behaviour with Developer ID signing or notarization (stage 12), Gatekeeper, access to user files from the bundle (ADR-0006 (b): without App Sandbox, file access is not verified), launch through Finder or `open`, macOS 14.
- Tests: `Tests/AppOptionsTests.swift` (`swift test --filter AppOptionsXCTests`, 5 tests): argument parsing and the helper location (no window, no process).

## S10-16 input from the window (host part written, awaiting review; NOT delivered to the guest)

Spec: [`protocols/control/INPUT_TRANSPORT_V1.md`](../../protocols/control/INPUT_TRANSPORT_V1.md). The board has no input device yet (xHCI + USB HID, `VIRTUAL_BOARD.md`), so events reach only VMHost's bounded queue; `ACCEPT` carries `GUEST_DELIVERY = 0` and the window subtitle says "input is not delivered to the guest". Manual input in a window has not been checked.

- `Shared/InputProtocol.swift` — ODIT messages (header 16 bytes, `VERSION`/`HELLO`/`ACCEPT`/`FOCUS`/`USAGE`/`MOTION`/`STATUS`), stream decoder with header-before-payload checks, `recvmsg`/`send` helpers. Shared source with VMHost (`apps/vmhost/README.md`).
- `Shared/InputQueue.swift` — the 256-record queue of section 7 (motion merging with saturation, DOWN refused when `free < 2h + 5`, `UP`/`FOCUS` by the invariant, GAINED_PENDING, `popForConsumer` with the `mode_change_count` re-check). Shared with VMHost.
- `Shared/HIDUsage.swift` — `kVK_*` → HID usage table (page 0x07), HUT 1.7 numbers and macOS SDK 27.0 `Events.h` constants; `kVK_Function` and the three media keys are listed as not transmitted with a reason.
- `Shared/InputTranslation.swift` — point → physical pixel → guest pixel through `ViewportLayout` (section 5.3), fractional accumulation of relative motion and wheel. `WHEEL_UNITS_PER_POINT = 7.5` (not measured).
- `InputChannel.swift` — UI end: HELLO/ACCEPT (5 s limit), the UI queue, non-blocking writes from its head, STATUS, focus rules, counters of dropped or ignored events. A 10 ms main-queue timer pumps the socket.
- `DisplayWindow.swift` — collects `keyDown`/`keyUp`/`flagsChanged` (local monitor, before menu key equivalents), mouse, drag, button and wheel events of the view, a `MOTION` with the click position before every button `USAGE`, `FOCUS GAINED`/`LOST` on key window, resign key/active, minimise and close.
- `Shared/VMHostLaunch.swift` — `spawnHelper(..., input:)` puts a second socketpair end on descriptor 4 and adds `--input-fd 4`; without `input` the spawn is unchanged.

Viewer option `--input-channel` (window mode only): creates the second socketpair and enables the collection above. Without it nothing changes. While the window is key and the channel is active (HANDSHAKE and UNFOCUSED states too), all keys and key equivalents (Cmd-Q, Cmd-W, ...) are swallowed by the local event monitor (open decision 3 of the spec). Ways out: the window close button, the menu with the mouse, Ctrl-C in the terminal, `--close-after-ms`. If a `keyUp` is not delivered while Command is held, the key stays pressed in the guest queue until `FOCUS LOST`; changing the window focus releases it. The wait for ACCEPT (5 s) counts from the moment the window is shown, when HELLO is sent. On an ISO keyboard Apple swaps `kVK_ISO_Section` (0x0A) and `kVK_ANSI_Grave` (0x32); the table follows the SDK names and has not been checked on hardware.

Unit tests (no AppKit, no VMHost process; 10⁶ random queue steps with a fixed seed, kVK table against the SDK header, coordinates, focus release through a real socketpair). Since S11-05a they are XCTest cases (`InputXCTests`, 19 tests; the former executable printed `input tests ok (2925454 checks)`, the test case prints the same number of checks). **Deviation from the S11-05a plan:** the plan says to keep the old `xcrun swiftc ... InputTests.swift` command, but the file is `import XCTest` now (no `@main`) and cannot be built with `swiftc` any more, so the command is replaced by `swift test --filter InputXCTests` (reason: XCTest cases, not an executable):

```sh
(cd apps && swift test --filter InputXCTests)
python3 tests/system/input_channel_smoke.py [target/od-vmhost-smoke]
# Window with the input channel (closes by itself after 2.5 s; prints "input channel: ..." on exit):
target/open-desktop-display-viewer --vmhost target/od-vmhost-smoke --input-channel --close-after-ms 2500
```

## S10-14 cursor overlay (written, awaiting review; live cursor not checked)

Spec: [`protocols/control/FRAME_TRANSPORT_V1.md`](../../protocols/control/FRAME_TRANSPORT_V1.md) sections 10.6 and 10.8, device: [`docs/specs/DISPLAY_DEVICE.md`](../../docs/specs/DISPLAY_DEVICE.md) 15.5.

- `Shared/FrameReader.swift` — v2 header and cursor area checks (`INCOMPATIBLE` for a wrong value), `CURSOR` messages, the cursor read of 10.6 (claim, `g1`, every field once, validation, image copy only for a new `shape_seq`, `g2`), accepted state `FrameReader.cursor`. The state is kept in every reader state and dies with the connection; the presenter draws it only over a shown frame.
- `Shared/Viewport.swift` — `CursorOverlay.placement`: image rectangle `pos − hot` cut by the frame, mapped to the drawable with the affine function of the frame, intersected with the visible part of the drawable (a `pixelExact` drawable smaller than the frame never gets a cursor viewport outside the render target), texture coordinates narrowed like `visibleUV`.
- `MetalPresenter.swift` — cursor texture (`bgra8Unorm`, premultiplied bytes unchanged) created only when `shape_seq` changes; second draw in the same render pass after the frame, own pipeline with blending `one`/`oneMinusSourceAlpha`, `writeMask` without alpha (the X byte of the frame is not changed), nearest sampling, scissor = visible part. Counters `frameUploadCount` and `cursorUploadCount` are for tests. `DisplayWindow.swift` is unchanged: a cursor update makes `pump` report a change, and the existing redraw path draws it.

Builds are the ones above (viewer and VMHost); both now speak transport v2. Tests:

```sh
# Geometry, including cursor placement (pixel-exact drawable smaller than the frame, cursor at cropped edges), XCTest since S11-05a (deviation from the S11-05a plan, see the Viewport unit test above: `swift test --filter` replaces the `xcrun swiftc ... ViewportTests.swift` command):
(cd apps && swift test --filter ViewportXCTests)
# Offscreen overlay against a reference that the script builds itself (tolerance 0 for A in {0, 255}):
python3 tests/system/cursor_overlay_smoke.py [target/open-desktop-display-viewer [target/od-vmhost-smoke]]
# Writer and reader of the cursor area together: see apps/vmhost/README.md
```

`--dump-cursor-test X,Y,HX,HY,E` (viewer, offscreen only, test-only, needs `--dump-frame`) injects a synthetic accepted cursor into the presenter after the frame is shown: a fixed 4x4 `BGRA8888_PREMUL` image (formula in the header of `cursor_overlay_smoke.py`), hotspot `HX,HY` (0…3), position `X,Y`, enabled `E` (0 or 1). It goes through the real `uploadCursor`/`commitCursor` and the real draw, but not through the transport or the device. The dump line gains `cursor=none` or `cursor=rect=x0,y0,x1,y1 uv=u0,v0,du,dv` (drawable pixels). Without the flag the viewer draws only what arrives in `CURSOR` messages, and the test pattern profile sends none: **a cursor from a guest on a window or in a dump has not been shown.** The window with a cursor was not looked at. The host cursor over the window (`NSCursor`) is a separate limit of S10-16.

## Два экрана (S10-15, тестовый путь)

- `--screens 2` — viewer ждёт ANNOUNCE обоих экранов, создаёт два окна (экран 0 — основное, его закрытие завершает viewer и VMHost; экран 1 — только окно) и запускает VMHost с `--display-screens 2` (профиль 4, DISPLAY_HOST_ABI_V1.md, раздел 10).
- `--dump-screen 1` (нужен `--screens 2`) — offscreen-дамп экрана 1; без флага дампится экран 0.
- Пример (после сборки по строке 18 и `target/od-vmhost-smoke` из `apps/vmhost/README.md`):

```bash
target/open-desktop-display-viewer --vmhost target/od-vmhost-smoke --screens 2 --dump-screen 1 --dump-frame /private/tmp/screen1.raw
```

Экран 1 в тестовом профиле — шахматка 32×24 (клетка 8×8), проверяется побайтно эталоном, который строит скрипт. Окна глазами не проверены; сквозной путь «регистры гостя → VMHost» для экрана 1 не проверен (нужен probe S10-08).
