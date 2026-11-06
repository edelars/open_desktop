# macOS display viewer (S10-07, S10-11)

`Sources/display_viewer_main.swift` is a local smoke viewer, not the SwiftUI application of stage 11. It starts VMHost with the shared launch code (`Sources/VMHostLaunch.swift`, also used by `Sources/launcher.swift`) in the **test-only** mode `--display-smoke pattern`, receives frames over frame transport v1 ([`protocols/control/FRAME_TRANSPORT_V1.md`](../../protocols/control/FRAME_TRANSPORT_V1.md)) and shows them through Metal.

- `FrameReader.swift` — reader of the shared memory: `recvmsg` with `SCM_RIGHTS`, version and size checks, `fstat` before `mmap`, seqlock slot read with the generation re-check, reader states of section 7.2.
- `Viewport.swift` (S10-11) — pure integer geometry shared by the window, the offscreen dump and the unit test: `drawableSize` (physical = round(logical × backingScaleFactor)), `viewport` (`fit`: aspect-preserving letterbox/pillarbox, centred, rounded half up; `pixelExact`: 1 guest pixel = 1 drawable pixel, centred, cropped around the frame centre if the drawable is smaller: the viewport is clipped to the drawable and the texture coordinates are narrowed, so Metal never gets a viewport outside the render pass; with an odd positive difference the extra pixel goes to the right/bottom margin, with an odd negative one it is cropped on the left/top) and `guestPixel` (drawable pixel → guest pixel, the transform the mouse input of S10-16 will use; nil outside the frame rectangle).
- `MetalPresenter.swift` — `MTLTexture` (`bgra8Unorm`, bytes B, G, R, X as in the slot) filled once per frame with `replace(region:)`, fullscreen-triangle pipeline with nearest-neighbour sampling drawn into the viewport (the target is cleared to the background first, so margins are plain background), and the offscreen path (render into a texture of a given physical size, read back with `getBytes`).
- `DisplayWindow.swift` — AppKit window whose view is backed by a `CAMetalLayer`; the drawable is always bounds × `backingScaleFactor`; a resize or a scale change only redraws with a new drawable size and viewport (the shown texture is neither recreated nor dropped). "no signal", "changing mode", "disconnected" and "incompatible version" are a colour plus a text label, never a frozen frame.

The viewer links no Rust code: it only spawns VMHost, so `libod_host.a` is needed for the VMHost binary, not for the viewer. The atomic C shim is the VMHost one, `apps/vmhost/Sources/FrameTransportAtomics.h`, imported with `-import-objc-header` (not copied).

Build from the repository root (VMHost first; the debug `libod_host.a` is enough, see [`apps/vmhost/README.md`](../vmhost/README.md)):

```sh
cargo build -p od-host --offline
xcrun swiftc -module-cache-path /private/tmp/open-desktop-module-cache -parse-as-library -O -import-objc-header apps/vmhost/Sources/VMHostABI.h apps/vmhost/Sources/ControlFrame.swift apps/vmhost/Sources/FrameTransport.swift apps/vmhost/Sources/main.swift target/debug/libod_host.a -ldl -lpthread -lm -framework Security -framework SystemConfiguration -framework CoreFoundation -framework CoreServices -framework Foundation -o target/od-vmhost-smoke
codesign --force --sign - --options runtime --entitlements apps/vmhost/Configuration/vmhost-smoke.entitlements target/od-vmhost-smoke
xcrun swiftc -module-cache-path /private/tmp/open-desktop-module-cache -parse-as-library -O -import-objc-header apps/vmhost/Sources/FrameTransportAtomics.h apps/macos/Sources/VMHostLaunch.swift apps/macos/Sources/FrameReader.swift apps/macos/Sources/Viewport.swift apps/macos/Sources/MetalPresenter.swift apps/macos/Sources/DisplayWindow.swift apps/macos/Sources/display_viewer_main.swift -framework AppKit -framework Metal -framework QuartzCore -framework Foundation -o target/open-desktop-display-viewer
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

Without `--dump-size` and `--backing-scale` the dump has the frame size and is the 1:1 reference dump (tolerance 0). Every dump prints the guest frame size (`width`, `height`), `logical`, `physical`, `scale`, `mode`, `viewport=x,y,w,h` (full frame rectangle in drawable pixels, origin top-left, may start below 0 when pixel-exact crops) and `visible=x,y,w,h` (the part handed to Metal); the window mode prints `window geometry: logical=… physical=… scale=…` on exit. Logical (points) and physical (pixels) sizes differ on Retina and both belong in every measurement report. S10-16 will use `ViewportLayout.guestPixel` for mouse coordinates; no input is implemented here.

The launcher smoke (`launcher.swift`) now also needs `Sources/VMHostLaunch.swift` in its compile command; the command in [`apps/vmhost/README.md`](../vmhost/README.md) shows it.
