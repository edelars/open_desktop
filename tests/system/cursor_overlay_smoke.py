#!/usr/bin/env python3
"""S10-14 smoke: the display viewer overlays a cursor on the frame (offscreen dump), compared byte for byte with a reference
that this script builds without using any code of the overlay.

Runs `open-desktop-display-viewer --vmhost VMHOST --dump-frame PATH [--dump-size WxH] [--pixel-exact]
--dump-cursor-test X,Y,HX,HY,E` for several cases. The test-only flag injects a synthetic accepted cursor state (a fixed 4x4
image, hotspot HX,HY, position X,Y, enabled E) into the presenter through its real upload/commit path after the frame is shown.
The cursor does NOT come over the transport here: the test pattern profile never writes the cursor registers and the ABI has no
test-only call for it (DISPLAY_HOST_ABI_V1.md rule 0). Device -> ABI is covered by the od-host tests, the writer and the reader
of the cursor area by apps/vmhost/Tests/CursorTransportTests.swift.

Reference (independent of the viewer):

* guest frame: tests/fixtures/guest/stage10/bars_64x48_seed0.raw (B, G, R, X rows, 64x48);
* cursor image: 4x4, pixel (i, j) = column i, row j: if (i + j) % 4 == 0 it is (B, G, R, A) = (0, 0, 0, 0), otherwise
  B = 40 + 50 i, G = 40 + 50 j, R = 255 - 40 (i + j), A = 255. Only A in {0, 255}, so the blend below is exact;
* composite frame in guest pixels (DISPLAY_DEVICE.md 15.5): the image rectangle starts at (X - HX, Y - HY), pixels outside the
  frame are cut off; for every channel B, G, R: out = min(255, C + D (255 - A) / 255) (for A = 255: C; for A = 0, C = 0: D); the X
  byte of the frame is never changed;
* the dumped drawable: 1:1 and pixel-exact modes copy the composite frame byte for byte (tolerance 0) to the viewport, the part of
  a cropped viewport that is visible is the middle of the frame; margins are B, G, R, X = 0 (00 00 00 00); integer 2x scaling
  replicates every guest pixel 2x2. The letterboxed case samples the composite frame with nearest neighbour and accepts either
  neighbouring texel only within AMBIGUITY texels of a texel boundary (docs/testing/DISPLAY.md 3), the same freedom as
  tests/system/display_viewer_scaled_smoke.py;
* translucent image (flag T = 1, only in the 1:1 cases): A = 64 * (1 + (i + j) % 3), B = A (i + 1) // 4, G = A (j + 1) // 4,
  R = A ((i + j) % 4 + 1) // 4, so 0 < A < 255 everywhere; the reference is C + D (255 - A) / 255 rounded half up, and the
  tolerance is +-1 on B, G, R and 0 on X (DISPLAY_DEVICE.md 15.13 item 10 expectation: GPU rounding of the blend). The bars have
  D in {0, 255}, so the real value is an integer and the expected observed difference is 0; the +-1 is the declared allowance;
* `enabled = 0` must give the frame without any change (equal to the fixture);
* the reported cursor rectangle (`cursor=rect=x0,y0,x1,y1`, drawable pixels) must equal the one computed here from the viewport
  (10.8: image rectangle cut by the frame, mapped to the drawable, cut by the visible part), or `none`.

Usage: python3 tests/system/cursor_overlay_smoke.py [VIEWER_BINARY [VMHOST_BINARY]]
"""

from __future__ import annotations

import math
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_VIEWER = ROOT / "target" / "open-desktop-display-viewer"
DEFAULT_VMHOST = ROOT / "target" / "od-vmhost-smoke"
REFERENCE = ROOT / "tests" / "fixtures" / "guest" / "stage10" / "bars_64x48_seed0.raw"
FW, FH = 64, 48
CW, CH = 4, 4
TIMEOUT = 30
AMBIGUITY = 0.01  # texels around a texel boundary where the GPU may pick either neighbour


class Failure(Exception):
    pass


def check(condition: bool, message: str) -> None:
    if not condition:
        raise Failure(message)


def cursor_image(translucent: bool = False) -> list[tuple[int, int, int, int]]:
    """Row-major (B, G, R, A) pixels of the 4x4 test image."""
    pixels = []
    for j in range(CH):
        for i in range(CW):
            if translucent:
                a = 64 * (1 + (i + j) % 3)
                pixels.append((a * (i + 1) // 4, a * (j + 1) // 4, a * ((i + j) % 4 + 1) // 4, a))
            elif (i + j) % 4 == 0:
                pixels.append((0, 0, 0, 0))
            else:
                pixels.append((40 + 50 * i, 40 + 50 * j, 255 - 40 * (i + j), 255))
    return pixels


def blend(c: int, a: int, d: int) -> int:
    # out = min(255, C + D (255 - A) / 255), rounded half up; exact for A in {0, 255} (no rounding then).
    return min(255, c + (d * (255 - a) * 2 + 255) // 510)


def composite(frame: bytes, x: int, y: int, hot_x: int, hot_y: int, enabled: bool, translucent: bool = False) -> bytearray:
    """Guest-pixel composite of the cursor over the frame (B, G, R, X rows)."""
    out = bytearray(frame)
    if not enabled:
        return out
    image = cursor_image(translucent)
    gx0, gy0 = x - hot_x, y - hot_y
    for j in range(CH):
        for i in range(CW):
            gx, gy = gx0 + i, gy0 + j
            if not (0 <= gx < FW and 0 <= gy < FH):
                continue
            b, g, r, a = image[j * CW + i]
            offset = (gy * FW + gx) * 4
            for channel, c in ((0, b), (1, g), (2, r)):
                out[offset + channel] = blend(c, a, frame[offset + channel])
            # out[offset + 3] (X) stays the frame's byte
    return out


def expected_rect(x: int, y: int, hot_x: int, hot_y: int, viewport: tuple[int, int, int, int], target: tuple[int, int]):
    """Rectangle in drawable pixels per 10.8 steps 1-4, or None."""
    vx, vy, vw, vh = viewport
    gx0, gy0 = x - hot_x, y - hot_y
    cx0, cy0, cx1, cy1 = max(gx0, 0), max(gy0, 0), min(gx0 + CW, FW), min(gy0 + CH, FH)
    if cx1 <= cx0 or cy1 <= cy0:
        return None
    rx0, rx1 = vx + cx0 * vw / FW, vx + cx1 * vw / FW
    ry0, ry1 = vy + cy0 * vh / FH, vy + cy1 * vh / FH
    rect = (max(rx0, 0.0), max(ry0, 0.0), min(rx1, float(target[0])), min(ry1, float(target[1])))
    return rect if rect[2] > rect[0] and rect[3] > rect[1] else None


def texel_candidates(position: float, size: int) -> list[int]:
    """Texels the GPU may sample at `position` (texel units): floor, plus the neighbour when within AMBIGUITY of a boundary."""
    result = {min(size - 1, max(0, math.floor(position)))}
    for delta in (-AMBIGUITY, AMBIGUITY):
        result.add(min(size - 1, max(0, math.floor(position + delta))))
    return sorted(result)


# (name, viewer args, physical WxH, viewport x y w h (full), exact mode?, (X, Y, HX, HY, E[, T]))
CASES = [
    ("1:1 inside the frame", [], (64, 48), (0, 0, 64, 48), True, (20, 12, 1, 1, 1)),
    ("1:1 cut by the bottom-right edge", [], (64, 48), (0, 0, 64, 48), True, (62, 46, 1, 1, 1)),
    ("1:1 negative image corner (pos < hot)", [], (64, 48), (0, 0, 64, 48), True, (0, 0, 2, 2, 1)),
    ("1:1 one image pixel inside (pos 0,0 hotspot 3,3)", [], (64, 48), (0, 0, 64, 48), True, (0, 0, 3, 3, 1)),
    ("1:1 image outside the frame (pos beyond the frame)", [], (64, 48), (0, 0, 64, 48), True, (100, 10, 1, 1, 1)),
    ("1:1 disabled: no change", [], (64, 48), (0, 0, 64, 48), True, (20, 12, 1, 1, 0)),
    ("pixel-exact in 100x100, margins", ["--dump-size", "100x100", "--pixel-exact"], (100, 100), (18, 26, 64, 48), True, (20, 12, 1, 1, 1)),
    ("pixel-exact cropped 40x30, cursor at the cropped left/top", ["--dump-size", "40x30", "--pixel-exact"], (40, 30), (-12, -9, 64, 48), True, (10, 8, 0, 0, 1)),
    ("pixel-exact cropped 40x30, cursor at the cropped right/bottom", ["--dump-size", "40x30", "--pixel-exact"], (40, 30), (-12, -9, 64, 48), True, (50, 37, 0, 0, 1)),
    ("pixel-exact cropped 40x30, cursor entirely in the cropped part", ["--dump-size", "40x30", "--pixel-exact"], (40, 30), (-12, -9, 64, 48), True, (2, 2, 0, 0, 1)),
    ("integer 2x (128x96)", ["--dump-size", "128x96"], (128, 96), (0, 0, 128, 96), True, (20, 12, 1, 1, 1)),
    ("integer 2x, cut by the bottom-right edge", ["--dump-size", "128x96"], (128, 96), (0, 0, 128, 96), True, (62, 46, 1, 1, 1)),
    ("retina 64x48 points at scale 2", ["--dump-size", "64x48", "--backing-scale", "2"], (128, 96), (0, 0, 128, 96), True, (20, 12, 1, 1, 1)),
    ("1:1 translucent image 0 < A < 255 (tolerance +-1)", [], (64, 48), (0, 0, 64, 48), True, (20, 12, 1, 1, 1, 1)),
    ("pixel-exact cropped 40x30, translucent at the cropped edge (tolerance +-1)", ["--dump-size", "40x30", "--pixel-exact"], (40, 30), (-12, -9, 64, 48), True, (10, 8, 0, 0, 1, 1)),
    ("letterbox 100x100 (nearest, boundary band)", ["--dump-size", "100x100"], (100, 100), (0, 12, 100, 75), False, (20, 12, 1, 1, 1)),
]

LINE = re.compile(
    r"dump-frame ok: .*? viewport=(-?\d+),(-?\d+),(\d+),(\d+) visible=\S+ cursor=(none|rect=\S+) .*?path=")


def run_case(viewer: Path, vmhost: Path, workdir: Path, index: int, case) -> str:
    name, extra, (tw, th), viewport, exact, spec = case
    x, y, hot_x, hot_y, enabled = spec[:5]
    translucent = len(spec) > 5 and spec[5] == 1
    tolerance = 1 if translucent else 0  # B, G, R only; X is always exact
    dump = workdir / f"case{index}.raw"
    command = [str(viewer), "--vmhost", str(vmhost), "--dump-frame", str(dump), *extra,
               "--dump-cursor-test", ",".join(str(v) for v in spec)]
    result = subprocess.run(command, capture_output=True, text=True, timeout=TIMEOUT)
    check(result.returncode == 0, f"{name}: viewer exit {result.returncode}: {result.stderr.strip()}")
    match = LINE.search(result.stdout)
    check(match is not None, f"{name}: no dump-frame line in {result.stdout!r}")
    reported = tuple(int(match.group(i)) for i in range(1, 5))
    check(reported == viewport, f"{name}: reported viewport {reported}, expected {viewport}")

    frame = REFERENCE.read_bytes()
    check(len(frame) == FW * FH * 4, "reference size")
    comp = composite(frame, x, y, hot_x, hot_y, bool(enabled), translucent)
    actual = dump.read_bytes()
    check(len(actual) == tw * th * 4, f"{name}: dump has {len(actual)} bytes, expected {tw * th * 4}")

    rect = expected_rect(x, y, hot_x, hot_y, viewport, (tw, th)) if enabled else None
    # The reported rectangle: computed independently (10.8 steps 1-4), compared with a tiny float tolerance.
    got = match.group(5)
    if rect is None:
        check(got == "none", f"{name}: cursor reported as {got}, expected none")
    else:
        check(got.startswith("rect="), f"{name}: cursor reported as {got}, expected a rectangle")
        numbers = [float(v) for v in got.split(" ")[0][len("rect="):].split(",")]
        check(all(abs(a - b) < 1e-6 for a, b in zip(numbers, rect)), f"{name}: cursor rect {numbers}, expected {list(rect)}")

    vx, vy, vw, vh = viewport
    mismatches = ambiguous = margins = 0
    for py in range(th):
        for px in range(tw):
            got_pixel = actual[(py * tw + px) * 4:(py * tw + px) * 4 + 4]
            inside = vx <= px < vx + vw and vy <= py < vy + vh
            if not inside:
                margins += 1
                check(got_pixel == bytes(4), f"{name}: margin pixel ({px},{py}) is {got_pixel.hex()}, expected 00000000")
                continue
            fx = (px + 0.5 - vx) * FW / vw
            fy = (py + 0.5 - vy) * FH / vh
            xs, ys = texel_candidates(fx, FW), texel_candidates(fy, FH)
            if exact:
                xs, ys = [min(FW - 1, math.floor(fx))], [min(FH - 1, math.floor(fy))]
            options = {bytes(comp[(gy * FW + gx) * 4:(gy * FW + gx) * 4 + 4]) for gy in ys for gx in xs}
            if len(options) > 1:
                ambiguous += 1
            if not any(got_pixel[3] == o[3] and all(abs(got_pixel[k] - o[k]) <= tolerance for k in range(3)) for o in options):
                mismatches += 1
                if mismatches <= 5:
                    print(f"  {name}: pixel ({px},{py}) {got_pixel.hex()} not in {[o.hex() for o in options]}", file=sys.stderr)
    check(mismatches == 0, f"{name}: {mismatches} pixels differ from the reference")

    changed = sum(1 for k in range(0, len(comp), 4) if comp[k:k + 4] != frame[k:k + 4])
    if enabled and rect is not None:
        check(changed > 0, f"{name}: the reference cursor changes no pixel (test case is empty)")
    elif not enabled or (enabled and rect is None and (x - hot_x >= FW or y - hot_y >= FH)):
        check(changed == 0, f"{name}: the reference must equal the frame")
    # else: the cursor lies in the frame but in the cropped part; the comparison above shows the visible part unchanged.
    return f"{name}: ok drawable={tw}x{th} cursor_pixels={changed} margins={margins} ambiguous={ambiguous} rect={got}"


def main(argv: list[str]) -> int:
    viewer = Path(argv[1]) if len(argv) > 1 else DEFAULT_VIEWER
    vmhost = Path(argv[2]) if len(argv) > 2 else DEFAULT_VMHOST
    for path in (viewer, vmhost, REFERENCE):
        if not path.exists():
            print(f"error: not found: {path}", file=sys.stderr)
            return 2
    try:
        with tempfile.TemporaryDirectory(prefix="cursor_overlay_smoke_") as tmp:
            for index, case in enumerate(CASES):
                print(run_case(viewer, vmhost, Path(tmp), index, case))
    except (Failure, OSError, subprocess.TimeoutExpired) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1
    print("cursor overlay smoke ok")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
