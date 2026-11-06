# macOS display viewer (S10-07, S10-11)

`Sources/display_viewer_main.swift` is a local smoke viewer, not the SwiftUI application of stage 11. It starts VMHost with the shared launch code (`Sources/Shared/VMHostLaunch.swift`, also used by `Sources/launcher.swift`) in the **test-only** mode `--display-smoke pattern`, receives frames over frame transport v2 ([`protocols/control/FRAME_TRANSPORT_V1.md`](../../protocols/control/FRAME_TRANSPORT_V1.md); v1 plus the cursor, S10-14) and shows them through Metal.

- `Shared/FrameReader.swift` — reader of the shared memory: `recvmsg` with `SCM_RIGHTS`, version and size checks, `fstat` before `mmap`, seqlock slot read with the generation re-check, reader states of section 7.2.
- `Viewport.swift` (S10-11) — pure integer geometry shared by the window, the offscreen dump and the unit test: `drawableSize` (physical = round(logical × backingScaleFactor)), `viewport` (`fit`: aspect-preserving letterbox/pillarbox, centred, rounded half up; `pixelExact`: 1 guest pixel = 1 drawable pixel, centred, cropped around the frame centre if the drawable is smaller: the viewport is clipped to the drawable and the texture coordinates are narrowed, so Metal never gets a viewport outside the render pass; with an odd positive difference the extra pixel goes to the right/bottom margin, with an odd negative one it is cropped on the left/top) and `guestPixel` (drawable pixel → guest pixel, the transform the mouse input of S10-16 uses; nil outside the frame rectangle).
- `MetalPresenter.swift` — `MTLTexture` (`bgra8Unorm`, bytes B, G, R, X as in the slot) filled once per frame with `replace(region:)`, fullscreen-triangle pipeline with nearest-neighbour sampling drawn into the viewport (the target is cleared to the background first, so margins are plain background), and the offscreen path (render into a texture of a given physical size, read back with `getBytes`).
- `DisplayWindow.swift` — AppKit window whose view is backed by a `CAMetalLayer`; the drawable is always bounds × `backingScaleFactor`; a resize or a scale change only redraws with a new drawable size and viewport (the shown texture is neither recreated nor dropped). "no signal", "changing mode", "disconnected" and "incompatible version" are a colour plus a text label, never a frozen frame.

The viewer links no Rust code: it only spawns VMHost, so `libod_host.a` is needed for the VMHost binary, not for the viewer. The atomic C shim `apps/macos/Sources/ODAtomics/include/FrameTransportAtomics.h` (target `ODAtomics`) is shared by the VMHost writer and the viewer reader (`FrameReader`); SwiftPM imports it through the `ODAtomics` target (the header is not copied).

## Swift Package (S11-05a)

`apps/Package.swift` ([ADR-0006](../../docs/decisions/ADR-0006-desktop-app-shell.md), solution (a)) is the only normative build system of the macOS products (S11-05c removed the former `xcrun swiftc` command lines); the application bundle is built by `tools/build/build_app_bundle.py` (next section). No external dependencies. Targets that exist now:

| Target | Kind | Sources |
| --- | --- | --- |
| `ODAtomics` | C (header only plus one translation unit that compiles it) | `Sources/ODAtomics/include/FrameTransportAtomics.h` |
| `ODShared` | library, depends on `ODAtomics` | `Sources/Shared/`: `ControlFrame`, `ControlV2` (S11-04: codec of control protocol v2), `ErrorCatalog` (S11-09: user-facing words for every code, exit status and issue), `InputProtocol`, `InputQueue`, `VMHostLaunch`, `FrameReader`, `HIDUsage`, `InputTranslation`, `Viewport`. Public API for the other targets; no `FrameTransport`, no `libod_host.a` |
| `ODDisplay` | library, depends on `ODShared` (S11-10) | `DisplayWindow`, `MetalPresenter`, `InputChannel` (this directory, not moved); `package` access for the application and the viewer |
| `open-desktop-display-viewer` | executable, depends on `ODShared` and `ODDisplay` | `display_viewer_main` (this directory); keeps `--dump-frame` |
| `ODHostConstants` | C (header plus one translation unit that compiles it) | `Sources/ODHostConstants/include/ODHostConstants.h`: includes only `include/open_desktop/host_abi.h` (`OD_HOST_ABI_VERSION_V0`, `OD_FRAGMENT_MAX_LENGTH_V0`); no session or display ABI, no VMHost helpers |
| `open-desktop-launcher-smoke` | executable | `launcher.swift`; depends on `ODShared` and `ODHostConstants` (constants only), does not link `libod_host.a` |
| `OpenDesktopMacTests` | tests (XCTest) | `Tests/ViewportTests.swift`, `Tests/InputTests.swift`, `Tests/ControlV2Tests.swift` (`swift test --filter ControlV2XCTests`, 11 tests); `@testable import` of `ODShared` and `ODDisplay` (since S11-10; before it, of the viewer module), because the tests use internal API |

`OpenDesktop` (S11-05b, the SwiftUI application) is described in the next section; `odctl` does not exist yet and is not declared: S11-13 adds it with its sources. The VMHost targets (`od-vmhost`, `ODHostABI`, `OpenDesktopVMHostTests`) are described in [`apps/vmhost/README.md`](../vmhost/README.md).

From the repository root (the Rust staticlib first, because `od-vmhost` links `target/<profile>/libod_host.a`; `swift build` is debug, `-c release` needs `cargo build --release -p od-host --offline`):

```sh
cargo build -p od-host --offline
(cd apps && swift build && swift test)
(cd apps && swift build -c release)   # binaries: apps/.build/release/{od-vmhost,open-desktop-display-viewer,open-desktop-launcher-smoke}
```

Binaries are in `apps/.build/debug/` (or `apps/.build/release/` after `-c release`): `od-vmhost`, `open-desktop-display-viewer`, `open-desktop-launcher-smoke`, `OpenDesktop`. The smokes in `tests/system/` take these paths as optional arguments; **by default they use `apps/.build/debug/`** (the same rule everywhere: run `cargo build -p od-host --offline` and `cd apps && swift build` first). SwiftPM links `od-vmhost` ad hoc without hardened runtime and without entitlements; MAP_JIT works in that state, so the smokes run the unsigned debug binary. The signed helper with `com.apple.security.cs.allow-jit` is checked separately: `sh tests/system/vmhost_app_bundle_smoke.sh` builds the bundle (`build_app_bundle.py`, `target/app-bundle/OpenDesktop.app`) and runs a fragment through `Contents/Helpers/od-vmhost`.

Run:

```sh
# Window (closes with the window; --close-after-ms N closes it after N ms):
apps/.build/debug/open-desktop-display-viewer --vmhost apps/.build/debug/od-vmhost
# Offscreen, no window: render through Metal, read back, write the raw frame (B, G, R, X rows, stride = width * 4):
apps/.build/debug/open-desktop-display-viewer --vmhost apps/.build/debug/od-vmhost --dump-frame /private/tmp/frame_64x48.raw
python3 tests/system/display_compare.py tests/fixtures/guest/stage10/bars_64x48_seed0.raw /private/tmp/frame_64x48.raw --width 64 --height 48 --tolerance 0
python3 tests/system/display_viewer_dump_smoke.py [VIEWER [VMHOST]]
# S10-11: scaled offscreen dump: logical window size 100x100 points at scale 2 -> physical drawable 200x200, frame letterboxed (viewport 0,25,200,150)
apps/.build/debug/open-desktop-display-viewer --vmhost apps/.build/debug/od-vmhost --dump-frame /private/tmp/frame_scaled.raw --dump-size 100x100 --backing-scale 2
# Pixel-exact (1 guest pixel = 1 drawable pixel, centred) in the window, or in a dump of a larger drawable:
apps/.build/debug/open-desktop-display-viewer --vmhost apps/.build/debug/od-vmhost --pixel-exact
python3 tests/system/display_viewer_scaled_smoke.py [VIEWER [VMHOST]]
# Unit test of Shared/Viewport.swift (no Metal, no VMHost); XCTest, run from apps/:
(cd apps && swift test --filter ViewportXCTests)
```

Options: `--vmhost PATH` (required), `--profile pattern` (smoke only; the only value until S10-09 adds `probe`), `--dump-frame PATH`, `--dump-size WxH` (with `--dump-frame` only: the logical window size in points, default the frame size), `--backing-scale S` (with `--dump-frame` only: 1…4, default 1; the dumped drawable is round(W × S) × round(H × S) physical pixels), `--pixel-exact` (no value; default is aspect-preserving fit), `--timeout-ms N` (wait for ANNOUNCE and the first frame, default 10000), `--close-after-ms N`. Exit status: 0 success; 1 failure; 2 usage. In window mode failure means a startup error (Metal, spawn), a reader that ended in `DISCONNECTED` or `INCOMPATIBLE`, or a VMHost exit status other than 0 when the viewer stops it; the status is kept and returned by `exit` after `NSApp.terminate`. In offscreen mode: no frame, VMHost exit status other than 0 after EOF, I/O. On `DISCONNECTED`/`INCOMPATIBLE` the viewer unmaps and closes the socket and stops VMHost (EOF up to 2 s, SIGTERM, 500 ms, SIGKILL, reap) and shows VMHost's real exit status in the window text.

Without `--dump-size` and `--backing-scale` the dump has the frame size and is the 1:1 reference dump (tolerance 0). Every dump prints the guest frame size (`width`, `height`), `logical`, `physical`, `scale`, `mode`, `viewport=x,y,w,h` (full frame rectangle in drawable pixels, origin top-left, may start below 0 when pixel-exact crops) and `visible=x,y,w,h` (the part handed to Metal); the window mode prints `window geometry: logical=… physical=… scale=…` on exit. Logical (points) and physical (pixels) sizes differ on Retina and both belong in every measurement report. S10-16 uses `ViewportLayout.guestPixel` for the absolute mouse position (see the input section below).

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

Unit tests (no AppKit, no VMHost process; 10⁶ random queue steps with a fixed seed, kVK table against the SDK header, coordinates, focus release through a real socketpair). Since S11-05a they are XCTest cases (`InputXCTests`, 19 tests; the former executable printed `input tests ok (2925454 checks)`, the test case prints the same number of checks).:

```sh
(cd apps && swift test --filter InputXCTests)
python3 tests/system/input_channel_smoke.py [VMHOST]
# Window with the input channel (closes by itself after 2.5 s; prints "input channel: ..." on exit):
apps/.build/debug/open-desktop-display-viewer --vmhost apps/.build/debug/od-vmhost --input-channel --close-after-ms 2500
```

## S10-14 cursor overlay (written, awaiting review; live cursor not checked)

Spec: [`protocols/control/FRAME_TRANSPORT_V1.md`](../../protocols/control/FRAME_TRANSPORT_V1.md) sections 10.6 and 10.8, device: [`docs/specs/DISPLAY_DEVICE.md`](../../docs/specs/DISPLAY_DEVICE.md) 15.5.

- `Shared/FrameReader.swift` — v2 header and cursor area checks (`INCOMPATIBLE` for a wrong value), `CURSOR` messages, the cursor read of 10.6 (claim, `g1`, every field once, validation, image copy only for a new `shape_seq`, `g2`), accepted state `FrameReader.cursor`. The state is kept in every reader state and dies with the connection; the presenter draws it only over a shown frame.
- `Shared/Viewport.swift` — `CursorOverlay.placement`: image rectangle `pos − hot` cut by the frame, mapped to the drawable with the affine function of the frame, intersected with the visible part of the drawable (a `pixelExact` drawable smaller than the frame never gets a cursor viewport outside the render target), texture coordinates narrowed like `visibleUV`.
- `MetalPresenter.swift` — cursor texture (`bgra8Unorm`, premultiplied bytes unchanged) created only when `shape_seq` changes; second draw in the same render pass after the frame, own pipeline with blending `one`/`oneMinusSourceAlpha`, `writeMask` without alpha (the X byte of the frame is not changed), nearest sampling, scissor = visible part. Counters `frameUploadCount` and `cursorUploadCount` are for tests. `DisplayWindow.swift` is unchanged: a cursor update makes `pump` report a change, and the existing redraw path draws it.

Builds are the SwiftPM ones above (viewer and VMHost); both speak transport v2. Tests:

```sh
# Geometry, including cursor placement (pixel-exact drawable smaller than the frame, cursor at cropped edges), XCTest:
(cd apps && swift test --filter ViewportXCTests)
# Offscreen overlay against a reference that the script builds itself (tolerance 0 for A in {0, 255}):
python3 tests/system/cursor_overlay_smoke.py [VIEWER [VMHOST]]
# Writer and reader of the cursor area together: see apps/vmhost/README.md
```

`--dump-cursor-test X,Y,HX,HY,E` (viewer, offscreen only, test-only, needs `--dump-frame`) injects a synthetic accepted cursor into the presenter after the frame is shown: a fixed 4x4 `BGRA8888_PREMUL` image (formula in the header of `cursor_overlay_smoke.py`), hotspot `HX,HY` (0…3), position `X,Y`, enabled `E` (0 or 1). It goes through the real `uploadCursor`/`commitCursor` and the real draw, but not through the transport or the device. The dump line gains `cursor=none` or `cursor=rect=x0,y0,x1,y1 uv=u0,v0,du,dv` (drawable pixels). Without the flag the viewer draws only what arrives in `CURSOR` messages, and the test pattern profile sends none: **a cursor from a guest on a window or in a dump has not been shown.** The window with a cursor was not looked at. The host cursor over the window (`NSCursor`) is a separate limit of S10-16.

## Два экрана (S10-15, тестовый путь)

- `--screens 2` — viewer ждёт ANNOUNCE обоих экранов, создаёт два окна (экран 0 — основное, его закрытие завершает viewer и VMHost; экран 1 — только окно) и запускает VMHost с `--display-screens 2` (профиль 4, DISPLAY_HOST_ABI_V1.md, раздел 10).
- `--dump-screen 1` (нужен `--screens 2`) — offscreen-дамп экрана 1; без флага дампится экран 0.
- Пример (после `cargo build -p od-host --offline` и `cd apps && swift build`):

```bash
apps/.build/debug/open-desktop-display-viewer --vmhost apps/.build/debug/od-vmhost --screens 2 --dump-screen 1 --dump-frame /private/tmp/screen1.raw
```

Экран 1 в тестовом профиле — шахматка 32×24 (клетка 8×8), проверяется побайтно эталоном, который строит скрипт. Окна глазами не проверены; сквозной путь «регистры гостя → VMHost» для экрана 1 не проверен (нужен probe S10-08).

## User-facing errors (S11-09)

`Sources/Shared/ErrorCatalog.swift` (`ODShared`, shared with the future CLI) maps every `ControlV2ErrorCode`, `ControlV2AckCode`, exit disposition, `ClientIssue` and `LibraryIssue` to a title, an explanation, an action, a severity and a "Подробности" flag; `Sources/App/ErrorPresentation.swift` adds the code, the request ID, the `vmhost.log` line and the time. The catalog, the codes and the rules are in [`docs/specs/USER_FACING_ERRORS.md`](../../docs/specs/USER_FACING_ERRORS.md); `Tests/ErrorCatalogTests.swift` fails when a code has no entry (`swift test --filter ErrorCatalog`). The texts are localization keys with Russian default values in the code. **`Localizable.strings` (ru, en) is the next step**: the SwiftPM targets have no resources yet, and adding a resource without `resources: [.process(...)]` breaks the build.

## Окно машины и ввод внутри приложения (S11-10)

Реализовано, ревью не пройдено; окно и взаимодействие с ним глазами не проверены (см. «Не проверено»).

**Что происходит.** На событии `running` (`stateChanged`, только оно меняет строку машины) приложение открывает окно машины:

- `MachineController` запускает VMHost с `--display-fd 5` и `--input-fd 4`, если у него есть «хозяин окон» (`MachineWindowHost`); без хозяина запуск прежний (ни кадров, ни ввода — так в тестах библиотеки).
- `MachineWindowCenter` (`Sources/App/MachineWindowController.swift`) забирает родительские концы сокетов (`takeDisplaySocket()` и `takeInputSocket()` отдают дескриптор один раз за запуск) и создаёт `MachineWindowController`: `DisplayWindow` + `MetalPresenter` + `InputChannelUI` из цели `ODDisplay` (тот же код, что у viewer) и читатель кадров `ODShared.FrameReader` в своём потоке (`MachineFrameSession`, правила commit/discard/clear как в `ViewerSession.step`, экран 0).
- Порядок `running` на дескрипторе 3 и первого `ANNOUNCE` на дескрипторе 5 (CONTROL_PROTOCOL_V2.md, раздел 13, п. 11) окну не нужен: оно открывается на `running`, читатель шлёт `HELLO` тогда же (VMHost читает его после `START`), до `ANNOUNCE` окно пишет «Нет сигнала», предел ожидания 10 с.
- Состояние окна: `MachineWindowModel.overlay(run:frame:)` (чистая функция, `Sources/App/MachineWindowModel.swift`). «Нет сигнала», «Пауза», «Остановлено» (и «Остановлено: сбой машины»), «Несовместимая версия» (с причиной), «Смена режима экрана», «Экран отключён гостем», «Останавливается…», «Запуск…» — текст на однотонном фоне; кадр в этих состояниях не рисуется (текстура остаётся в презентере, но окно её не выводит: после «Продолжить» кадр на месте). Кадр рисуется только при `running` и состоянии читателя `showing`.
- Остановка машины: окно остаётся и говорит «Остановлено»; читатель и канал ввода закрыты. Следующий запуск открывает новое окно на том же месте, старое закрывается без вопроса.
- Закрытие окна при `running` или `paused`: вопрос «Закрыть окно и остановить машину?». Окно нельзя открыть заново до следующего запуска (кадры передаются один раз за запуск), поэтому «Остановить и закрыть» отправляет `STOP`. Если `STOP` сейчас невозможен (в полёте другой запрос), окно не закрывается. В остальных состояниях закрытие без вопроса.
- Несколько экранов: пока один (экран 0). Тестовый профиль с двумя экранами показывает экран 0; второй экран и его окно — остаток S10-15.

**Меню.** «Вид»: «По размеру окна» (`PresentMode.fit`), «Пиксель в пиксель» (`.pixelExact`), «Во весь экран» (`toggleFullScreen`, Control-Command-F); действуют на окно машины, которое сейчас ключевое, иначе недоступны. «Машина»: «Запустить», «Приостановить», «Продолжить», «Остановить» (по `MachineActionTable`; цель — машина ключевого окна машины, иначе выбранная в библиотеке) и «Захватить ввод» / «Освободить ввод».

**Захват ввода (открытые решения 2 и 3 `INPUT_TRANSPORT_V1.md`, раздел 11; решение этого среза, поведение UI, в протокол не входит).**

- Два режима: «не захвачено» (по умолчанию при открытии окна) и «захвачено». Захват только явный: комбинация **Control-Option-Command-G** (физическая клавиша `G`, раскладка не важна) или пункт меню «Машина → Захватить ввод». Щелчок по окну не захватывает.
- Освобождение: та же комбинация (в режиме «захвачено» её обрабатывает окно до перевода клавиши в HID, гостю она не уходит) или пункт меню. Кроме того захват снимается сам: пауза, остановка, потеря ключевого окна (в том числе смена приложения), сворачивание, закрытие окна, закрытие канала ввода VMHost. При снятии гость получает `FOCUS LOST` и отпускает все нажатые клавиши, включая Control, Option и Command комбинации.
- Захват возможен только при `running`, ключевом окне и канале ввода в состоянии READY; иначе пункт меню неактивен, а комбинация ничего не меняет.
- «Не захвачено»: окно ничего не отправляет в канал, все клавиши и Cmd-сочетания (Cmd-Q, Cmd-W, Cmd-N …) идут приложению и меню, курсор хоста виден.
- «Захвачено»: все клавиши, включая Cmd-сочетания, идут в канал ввода (локальный монитор забирает их до меню), приложение их не видит. Курсор хоста скрыт (прозрачный курсор) над содержимым окна и только в этом режиме. Курсор не отвязывается от мыши (`CGAssociateMouseAndMouseCursorPosition` не используется) и может выйти за окно; при выходе за окно он снова виден, а абсолютная позиция берётся как раньше.
- Индикатор: заголовок окна, «<имя> — ввод захвачен (⌃⌥⌘G — освободить)» / «<имя> — ввод не захвачен (⌃⌥⌘G — захватить)»; при состоянии машины не `running` — «<имя> — приостановлена» и т. п.
- Подзаголовок: «ввод в гостевую систему пока не доставляется» (`GUEST_DELIVERY = 0`), через « · » строка профиля («Тестовый узор (test-only, без гостя)»).
- **Конфликт с системными сочетаниями.** Страница Apple «Mac keyboard shortcuts» (support.apple.com/en-us/102650) перечисляет сочетания с тремя модификаторами: Control-Option-Command-8, -запятая, -точка, -кнопка питания; сочетания Control-Option-Command-G там нет. Это не проверка конфликта с пользовательскими сочетаниями (System Settings → Keyboard → Keyboard Shortcuts), с Accessibility и со сторонними программами: **они не проверены**. Сочетание не меняется пользователем; настройка комбинации — остаток.

**Проверки без окна.** `Tests/MachineWindowTests.swift` (`swift test --filter MachineWindow`): текст по состояниям (в том числе «кадр только при running + showing»), заголовок и подзаголовок, модель захвата (переходы, условия, освобождение), комбинация, выбор режима масштабирования, политика закрытия, хозяин окон контроллера и тест с настоящим `od-vmhost`: кадры и канал ввода доходят до стороны окна (`MachineFrameSession`, `MetalPresenter`, `InputChannelUI`), `GUEST_DELIVERY = 0`, конец машины заканчивает читатель.

**Не проверено.** Окно глазами (оверлеи, заголовок, подзаголовок, меню «Вид» и «Машина», порядок меню: `CommandMenu("Вид")` может оказаться рядом с системным меню «View»), полноэкранный режим, скрытие курсора, переключение захвата с клавиатуры, вопрос при закрытии, перенос позиции окна на новый запуск, поведение под несколькими мониторами и @2x. Скриншотов нет.

**Viewer.** `open-desktop-display-viewer` остаётся (цель, `--dump-frame`, `--input-channel`, `--screens 2`): `DisplayWindow`, `MetalPresenter`, `InputChannel` вынесены в цель `ODDisplay` без переноса файлов, поведение viewer не менялось (по умолчанию ввод «захвачен», как раньше).
