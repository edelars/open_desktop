# macOS display viewer (S10-07, S10-11)

`Sources/display_viewer_main.swift` is a local smoke viewer, not the SwiftUI application of stage 11. It starts VMHost with the shared launch code (`Sources/VMHostLaunch.swift`, also used by `Sources/launcher.swift`) in the **test-only** mode `--display-smoke pattern`, receives frames over frame transport v2 ([`protocols/control/FRAME_TRANSPORT_V1.md`](../../protocols/control/FRAME_TRANSPORT_V1.md); v1 plus the cursor, S10-14) and shows them through Metal.

- `FrameReader.swift` — reader of the shared memory: `recvmsg` with `SCM_RIGHTS`, version and size checks, `fstat` before `mmap`, seqlock slot read with the generation re-check, reader states of section 7.2.
- `Viewport.swift` (S10-11) — pure integer geometry shared by the window, the offscreen dump and the unit test: `drawableSize` (physical = round(logical × backingScaleFactor)), `viewport` (`fit`: aspect-preserving letterbox/pillarbox, centred, rounded half up; `pixelExact`: 1 guest pixel = 1 drawable pixel, centred, cropped around the frame centre if the drawable is smaller: the viewport is clipped to the drawable and the texture coordinates are narrowed, so Metal never gets a viewport outside the render pass; with an odd positive difference the extra pixel goes to the right/bottom margin, with an odd negative one it is cropped on the left/top) and `guestPixel` (drawable pixel → guest pixel, the transform the mouse input of S10-16 uses; nil outside the frame rectangle).
- `MetalPresenter.swift` — `MTLTexture` (`bgra8Unorm`, bytes B, G, R, X as in the slot) filled once per frame with `replace(region:)`, fullscreen-triangle pipeline with nearest-neighbour sampling drawn into the viewport (the target is cleared to the background first, so margins are plain background), and the offscreen path (render into a texture of a given physical size, read back with `getBytes`).
- `DisplayWindow.swift` — AppKit window whose view is backed by a `CAMetalLayer`; the drawable is always bounds × `backingScaleFactor`; a resize or a scale change only redraws with a new drawable size and viewport (the shown texture is neither recreated nor dropped). "no signal", "changing mode", "disconnected" and "incompatible version" are a colour plus a text label, never a frozen frame.

The viewer links no Rust code: it only spawns VMHost, so `libod_host.a` is needed for the VMHost binary, not for the viewer. The atomic C shim is the VMHost one, `apps/vmhost/Sources/FrameTransportAtomics.h`, imported with `-import-objc-header` (not copied).

Build from the repository root (VMHost first; the debug `libod_host.a` is enough, see [`apps/vmhost/README.md`](../vmhost/README.md)):

```sh
cargo build -p od-host --offline
xcrun swiftc -module-cache-path /private/tmp/open-desktop-module-cache -parse-as-library -O -import-objc-header apps/vmhost/Sources/VMHostABI.h apps/vmhost/Sources/ControlFrame.swift apps/macos/Sources/InputProtocol.swift apps/macos/Sources/InputQueue.swift apps/vmhost/Sources/InputChannel.swift apps/vmhost/Sources/FrameTransport.swift apps/vmhost/Sources/main.swift target/debug/libod_host.a -ldl -lpthread -lm -framework Security -framework SystemConfiguration -framework CoreFoundation -framework CoreServices -framework Foundation -o target/od-vmhost-smoke
codesign --force --sign - --options runtime --entitlements apps/vmhost/Configuration/vmhost-smoke.entitlements target/od-vmhost-smoke
xcrun swiftc -module-cache-path /private/tmp/open-desktop-module-cache -parse-as-library -O -import-objc-header apps/vmhost/Sources/FrameTransportAtomics.h apps/macos/Sources/VMHostLaunch.swift apps/macos/Sources/FrameReader.swift apps/macos/Sources/Viewport.swift apps/macos/Sources/MetalPresenter.swift apps/macos/Sources/InputProtocol.swift apps/macos/Sources/InputQueue.swift apps/macos/Sources/HIDUsage.swift apps/macos/Sources/InputTranslation.swift apps/macos/Sources/InputChannel.swift apps/macos/Sources/DisplayWindow.swift apps/macos/Sources/display_viewer_main.swift -framework AppKit -framework Metal -framework QuartzCore -framework Foundation -o target/open-desktop-display-viewer
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
# Unit test of Viewport.swift (no Metal, no VMHost):
xcrun swiftc -module-cache-path /private/tmp/open-desktop-module-cache -parse-as-library apps/macos/Sources/Viewport.swift apps/macos/Tests/ViewportTests.swift -o target/viewport-tests && target/viewport-tests
```

Options: `--vmhost PATH` (required), `--profile pattern` (smoke only; the only value until S10-09 adds `probe`), `--dump-frame PATH`, `--dump-size WxH` (with `--dump-frame` only: the logical window size in points, default the frame size), `--backing-scale S` (with `--dump-frame` only: 1…4, default 1; the dumped drawable is round(W × S) × round(H × S) physical pixels), `--pixel-exact` (no value; default is aspect-preserving fit), `--timeout-ms N` (wait for ANNOUNCE and the first frame, default 10000), `--close-after-ms N`. Exit status: 0 success; 1 failure; 2 usage. In window mode failure means a startup error (Metal, spawn), a reader that ended in `DISCONNECTED` or `INCOMPATIBLE`, or a VMHost exit status other than 0 when the viewer stops it; the status is kept and returned by `exit` after `NSApp.terminate`. In offscreen mode: no frame, VMHost exit status other than 0 after EOF, I/O. On `DISCONNECTED`/`INCOMPATIBLE` the viewer unmaps and closes the socket and stops VMHost (EOF up to 2 s, SIGTERM, 500 ms, SIGKILL, reap) and shows VMHost's real exit status in the window text.

Without `--dump-size` and `--backing-scale` the dump has the frame size and is the 1:1 reference dump (tolerance 0). Every dump prints the guest frame size (`width`, `height`), `logical`, `physical`, `scale`, `mode`, `viewport=x,y,w,h` (full frame rectangle in drawable pixels, origin top-left, may start below 0 when pixel-exact crops) and `visible=x,y,w,h` (the part handed to Metal); the window mode prints `window geometry: logical=… physical=… scale=…` on exit. Logical (points) and physical (pixels) sizes differ on Retina and both belong in every measurement report. S10-16 uses `ViewportLayout.guestPixel` for the absolute mouse position (see the input section below).

The launcher smoke (`launcher.swift`) now also needs `Sources/VMHostLaunch.swift` in its compile command; the command in [`apps/vmhost/README.md`](../vmhost/README.md) shows it.

## S10-16 input from the window (host part written, awaiting review; NOT delivered to the guest)

Spec: [`protocols/control/INPUT_TRANSPORT_V1.md`](../../protocols/control/INPUT_TRANSPORT_V1.md). The board has no input device yet (xHCI + USB HID, `VIRTUAL_BOARD.md`), so events reach only VMHost's bounded queue; `ACCEPT` carries `GUEST_DELIVERY = 0` and the window subtitle says "input is not delivered to the guest". Manual input in a window has not been checked.

- `InputProtocol.swift` — ODIT messages (header 16 bytes, `VERSION`/`HELLO`/`ACCEPT`/`FOCUS`/`USAGE`/`MOTION`/`STATUS`), stream decoder with header-before-payload checks, `recvmsg`/`send` helpers. Shared source with VMHost (`apps/vmhost/README.md`).
- `InputQueue.swift` — the 256-record queue of section 7 (motion merging with saturation, DOWN refused when `free < 2h + 5`, `UP`/`FOCUS` by the invariant, GAINED_PENDING, `popForConsumer` with the `mode_change_count` re-check). Shared with VMHost.
- `HIDUsage.swift` — `kVK_*` → HID usage table (page 0x07), HUT 1.7 numbers and macOS SDK 27.0 `Events.h` constants; `kVK_Function` and the three media keys are listed as not transmitted with a reason.
- `InputTranslation.swift` — point → physical pixel → guest pixel through `ViewportLayout` (section 5.3), fractional accumulation of relative motion and wheel. `WHEEL_UNITS_PER_POINT = 7.5` (not measured).
- `InputChannel.swift` — UI end: HELLO/ACCEPT (5 s limit), the UI queue, non-blocking writes from its head, STATUS, focus rules, counters of dropped or ignored events. A 10 ms main-queue timer pumps the socket.
- `DisplayWindow.swift` — collects `keyDown`/`keyUp`/`flagsChanged` (local monitor, before menu key equivalents), mouse, drag, button and wheel events of the view, a `MOTION` with the click position before every button `USAGE`, `FOCUS GAINED`/`LOST` on key window, resign key/active, minimise and close.
- `VMHostLaunch.swift` — `spawnHelper(..., input:)` puts a second socketpair end on descriptor 4 and adds `--input-fd 4`; without `input` the spawn is unchanged.

Viewer option `--input-channel` (window mode only): creates the second socketpair and enables the collection above. Without it nothing changes. While the window is key and the channel is active (HANDSHAKE and UNFOCUSED states too), all keys and key equivalents (Cmd-Q, Cmd-W, ...) are swallowed by the local event monitor (open decision 3 of the spec). Ways out: the window close button, the menu with the mouse, Ctrl-C in the terminal, `--close-after-ms`. If a `keyUp` is not delivered while Command is held, the key stays pressed in the guest queue until `FOCUS LOST`; changing the window focus releases it. The wait for ACCEPT (5 s) counts from the moment the window is shown, when HELLO is sent. On an ISO keyboard Apple swaps `kVK_ISO_Section` (0x0A) and `kVK_ANSI_Grave` (0x32); the table follows the SDK names and has not been checked on hardware.

Unit tests (no AppKit, no VMHost process; 10⁶ random queue steps with a fixed seed, kVK table against the SDK header, coordinates, focus release through a real socketpair):

```sh
xcrun swiftc -module-cache-path /private/tmp/open-desktop-module-cache -parse-as-library apps/macos/Sources/Viewport.swift apps/macos/Sources/InputProtocol.swift apps/macos/Sources/InputQueue.swift apps/macos/Sources/HIDUsage.swift apps/macos/Sources/InputTranslation.swift apps/macos/Sources/InputChannel.swift apps/macos/Tests/InputTests.swift -o target/input-tests && target/input-tests
python3 tests/system/input_channel_smoke.py [target/od-vmhost-smoke]
# Window with the input channel (closes by itself after 2.5 s; prints "input channel: ..." on exit):
target/open-desktop-display-viewer --vmhost target/od-vmhost-smoke --input-channel --close-after-ms 2500
```

## S10-14 cursor overlay (written, awaiting review; live cursor not checked)

Spec: [`protocols/control/FRAME_TRANSPORT_V1.md`](../../protocols/control/FRAME_TRANSPORT_V1.md) sections 10.6 and 10.8, device: [`docs/specs/DISPLAY_DEVICE.md`](../../docs/specs/DISPLAY_DEVICE.md) 15.5.

- `FrameReader.swift` — v2 header and cursor area checks (`INCOMPATIBLE` for a wrong value), `CURSOR` messages, the cursor read of 10.6 (claim, `g1`, every field once, validation, image copy only for a new `shape_seq`, `g2`), accepted state `FrameReader.cursor`. The state is kept in every reader state and dies with the connection; the presenter draws it only over a shown frame.
- `Viewport.swift` — `CursorOverlay.placement`: image rectangle `pos − hot` cut by the frame, mapped to the drawable with the affine function of the frame, intersected with the visible part of the drawable (a `pixelExact` drawable smaller than the frame never gets a cursor viewport outside the render target), texture coordinates narrowed like `visibleUV`.
- `MetalPresenter.swift` — cursor texture (`bgra8Unorm`, premultiplied bytes unchanged) created only when `shape_seq` changes; second draw in the same render pass after the frame, own pipeline with blending `one`/`oneMinusSourceAlpha`, `writeMask` without alpha (the X byte of the frame is not changed), nearest sampling, scissor = visible part. Counters `frameUploadCount` and `cursorUploadCount` are for tests. `DisplayWindow.swift` is unchanged: a cursor update makes `pump` report a change, and the existing redraw path draws it.

Builds are the ones above (viewer and VMHost); both now speak transport v2. Tests:

```sh
# Geometry, including cursor placement (pixel-exact drawable smaller than the frame, cursor at cropped edges):
xcrun swiftc -module-cache-path /private/tmp/open-desktop-module-cache -parse-as-library apps/macos/Sources/Viewport.swift apps/macos/Tests/ViewportTests.swift -o target/viewport-tests && target/viewport-tests
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
