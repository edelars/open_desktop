#!/usr/bin/env python3
"""S10-11 smoke: the display viewer scales the VMHost test pattern with its aspect ratio kept (offscreen dump).

Runs `open-desktop-display-viewer --vmhost VMHOST --dump-frame PATH --dump-size WxH [--backing-scale S] [--pixel-exact]`
(logical size in points, physical drawable = round(logical * scale)) and checks, for each case:

* the reported logical/physical sizes and the reported viewport against hard-coded expectations;
* the aspect ratio of the viewport (error below one pixel on the rounded side) and that the viewport touches two sides;
* margins outside the viewport are exactly B, G, R, X = 0 (plain background), nothing is drawn outside the viewport;
* the content inside the viewport equals nearest-neighbour sampling of tests/fixtures/guest/stage10/bars_64x48_seed0.raw:
  pixel (x, y) shows guest texel floor((x - vx + 0.5) * fw / vw), floor((y - vy + 0.5) * fh / vh). Nearest sampling never
  blends, so the tolerance for a value is 0; the only freedom is the GPU's float rounding of the sample position where it
  falls within AMBIGUITY texels of a texel boundary: either neighbouring texel is then accepted (docs/testing/DISPLAY.md 3);
* pixel-exact mode: the frame is copied byte for byte (tolerance 0) to its centred position; when the drawable is smaller
  than the frame (viewport origin < 0) the visible part is the middle of the frame, again byte for byte, and `visible=` is
  the viewport clipped to the drawable (what is handed to Metal).

Usage: python3 tests/system/display_viewer_scaled_smoke.py [VIEWER_BINARY [VMHOST_BINARY]]
"""

from __future__ import annotations

import math
import os
import re
import signal
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_VIEWER = ROOT / "target" / "open-desktop-display-viewer"
DEFAULT_VMHOST = ROOT / "target" / "od-vmhost-smoke"
REFERENCE = ROOT / "tests" / "fixtures" / "guest" / "stage10" / "bars_64x48_seed0.raw"
FRAME_W, FRAME_H = 64, 48
TIMEOUT = 30
AMBIGUITY = 0.01  # texels; band around a texel boundary where the GPU may pick either neighbour

# (name, extra viewer args, logical WxH, physical WxH, viewport x, y, w, h (full, may start below 0), pixel_exact)
CASES = [
    ("integer 2x", ["--dump-size", "128x96"], (128, 96), (128, 96), (0, 0, 128, 96), False),
    ("letterbox 100x100", ["--dump-size", "100x100"], (100, 100), (100, 100), (0, 12, 100, 75), False),
    ("pillarbox 200x50", ["--dump-size", "200x50"], (200, 50), (200, 50), (66, 0, 67, 50), False),
    ("tall 50x200", ["--dump-size", "50x200"], (50, 200), (50, 200), (0, 81, 50, 38), False),
    ("downscale 32x24", ["--dump-size", "32x24"], (32, 24), (32, 24), (0, 0, 32, 24), False),
    ("retina 64x48 points at scale 2", ["--dump-size", "64x48", "--backing-scale", "2"], (64, 48), (128, 96), (0, 0, 128, 96), False),
    ("retina 100x100 points at scale 2", ["--dump-size", "100x100", "--backing-scale", "2"], (100, 100), (200, 200), (0, 25, 200, 150), False),
    ("odd margins 99x99", ["--dump-size", "99x99"], (99, 99), (99, 99), (0, 12, 99, 74), False),
    ("pixel-exact in 100x100", ["--dump-size", "100x100", "--pixel-exact"], (100, 100), (100, 100), (18, 26, 64, 48), True),
    ("pixel-exact retina 64x48 points at scale 2", ["--dump-size", "64x48", "--backing-scale", "2", "--pixel-exact"], (64, 48), (128, 96), (32, 24, 64, 48), True),
    # Drawable smaller than the frame: the frame is cropped around its centre, never scaled (viewport origin < 0).
    ("pixel-exact cropped 10x10", ["--dump-size", "10x10", "--pixel-exact"], (10, 10), (10, 10), (-27, -19, 64, 48), True),
    ("pixel-exact cropped odd 63x47", ["--dump-size", "63x47", "--pixel-exact"], (63, 47), (63, 47), (-1, -1, 64, 48), True),
    ("pixel-exact cropped x only 40x100", ["--dump-size", "40x100", "--pixel-exact"], (40, 100), (40, 100), (-12, 26, 64, 48), True),
    ("pixel-exact cropped y only 200x30", ["--dump-size", "200x30", "--pixel-exact"], (200, 30), (200, 30), (68, -9, 64, 48), True),
    ("pixel-exact odd margins 101x101", ["--dump-size", "101x101", "--pixel-exact"], (101, 101), (101, 101), (18, 26, 64, 48), True),
    ("pixel-exact cropped retina 20x20 points at scale 2", ["--dump-size", "20x20", "--backing-scale", "2", "--pixel-exact"], (20, 20), (40, 40), (-12, -4, 64, 48), True),
]


class Failure(Exception):
    pass


def check(condition: bool, message: str) -> None:
    if not condition:
        raise Failure(message)


def run(command: list[str]) -> subprocess.CompletedProcess[str]:
    """Runs the viewer in its own process group and kills whatever is left in it (the VMHost it spawned)."""
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
    group = process.pid
    try:
        stdout, stderr = process.communicate(timeout=TIMEOUT)
    except subprocess.TimeoutExpired:
        os.killpg(group, signal.SIGKILL)
        process.communicate()
        raise
    finally:
        leftover = True
        try:
            os.killpg(group, 0)
        except ProcessLookupError:
            leftover = False
        if leftover:
            os.killpg(group, signal.SIGKILL)
    if leftover:
        raise Failure("a process of the viewer's group (VMHost) outlived the viewer")
    return subprocess.CompletedProcess(command, process.returncode, stdout, stderr)


def texel(frame: bytes, x: int, y: int) -> bytes:
    offset = (y * FRAME_W + x) * 4
    return frame[offset : offset + 4]


def candidates(position: float, limit: int) -> set[int]:
    """Texel indices the sample position may select: floor(position), plus the other side near an exact boundary."""
    nearest = round(position)
    result = {min(limit - 1, max(0, math.floor(position)))}
    if abs(position - nearest) < AMBIGUITY:
        result.add(min(limit - 1, max(0, nearest)))
        result.add(min(limit - 1, max(0, nearest - 1)))
    return result


def verify_case(name: str, dump: bytes, stdout: str, logical, physical, viewport, pixel_exact: bool, frame: bytes) -> str:
    match = re.search(
        r"mode=(\S+) logical=(\d+)x(\d+) physical=(\d+)x(\d+) scale=(\S+) viewport=(-?\d+),(-?\d+),(\d+),(\d+) visible=(\d+),(\d+),(\d+),(\d+)", stdout
    )
    check(match is not None, f"{name}: no geometry in the viewer output")
    assert match is not None
    mode = match.group(1)
    got_logical = (int(match.group(2)), int(match.group(3)))
    got_physical = (int(match.group(4)), int(match.group(5)))
    got_viewport = tuple(int(match.group(i)) for i in range(7, 11))
    got_visible = tuple(int(match.group(i)) for i in range(11, 15))
    check(mode == ("pixel-exact" if pixel_exact else "fit"), f"{name}: mode {mode}")
    check(got_logical == logical, f"{name}: logical {got_logical}, expected {logical}")
    check(got_physical == physical, f"{name}: physical {got_physical}, expected {physical}")
    check(got_viewport == viewport, f"{name}: viewport {got_viewport}, expected {viewport}")
    pw, ph = physical
    expected_visible = (max(viewport[0], 0), max(viewport[1], 0))
    expected_visible += (min(viewport[0] + viewport[2], physical[0]) - expected_visible[0], min(viewport[1] + viewport[3], physical[1]) - expected_visible[1])
    check(got_visible == expected_visible, f"{name}: visible {got_visible}, expected {expected_visible}")
    check(len(dump) == pw * ph * 4, f"{name}: dump size {len(dump)}, expected {pw * ph * 4}")
    vx, vy, vw, vh = viewport

    # Not stretched: aspect error of the viewport is below one pixel on the rounded side, and it touches two sides.
    if not pixel_exact:
        check(abs(vw * FRAME_H - vh * FRAME_W) <= max(FRAME_W, FRAME_H) // 2, f"{name}: aspect {vw}x{vh} differs from {FRAME_W}x{FRAME_H}")
        check(vw == pw or vh == ph, f"{name}: viewport {vw}x{vh} does not touch two sides of {pw}x{ph}")
        check(abs(vx - (pw - vx - vw)) <= 1 and abs(vy - (ph - vy - vh)) <= 1, f"{name}: viewport is not centred")
    else:
        check((vw, vh) == (FRAME_W, FRAME_H), f"{name}: pixel-exact viewport is scaled")

    margin_pixels = 0
    content_mismatches = 0
    ambiguous = 0
    for y in range(ph):
        row = dump[y * pw * 4 : (y + 1) * pw * 4]
        for x in range(pw):
            actual = row[x * 4 : x * 4 + 4]
            if not (vx <= x < vx + vw and vy <= y < vy + vh):
                margin_pixels += 1
                check(actual == b"\x00\x00\x00\x00", f"{name}: margin pixel ({x},{y}) is {actual.hex()}, expected background 00000000")
                continue
            if pixel_exact:
                check(actual == texel(frame, x - vx, y - vy), f"{name}: pixel-exact pixel ({x},{y}) differs from the frame")
                continue
            us = candidates((x - vx + 0.5) * FRAME_W / vw, FRAME_W)
            vs = candidates((y - vy + 0.5) * FRAME_H / vh, FRAME_H)
            if len(us) * len(vs) > 1:
                ambiguous += 1
            if not any(actual == texel(frame, u, v) for u in us for v in vs):
                content_mismatches += 1
                if content_mismatches <= 5:
                    print(f"{name}: pixel ({x},{y}) is {actual.hex()}, expected one of texels {sorted(us)} x {sorted(vs)}", file=sys.stderr)
    check(content_mismatches == 0, f"{name}: {content_mismatches} pixels differ from nearest-neighbour sampling")
    return f"{name}: ok logical={logical[0]}x{logical[1]} physical={pw}x{ph} viewport={vx},{vy},{vw},{vh} margin_pixels={margin_pixels} ambiguous_pixels={ambiguous}"


def main(argv: list[str]) -> int:
    viewer = Path(argv[1]) if len(argv) > 1 else DEFAULT_VIEWER
    vmhost = Path(argv[2]) if len(argv) > 2 else DEFAULT_VMHOST
    for path in (viewer, vmhost, REFERENCE):
        check(path.exists(), f"missing {path}")
    frame = REFERENCE.read_bytes()
    check(len(frame) == FRAME_W * FRAME_H * 4, "reference has the wrong size")
    try:
        with tempfile.TemporaryDirectory(prefix="display_viewer_scaled_") as workdir:
            dump_path = Path(workdir) / "scaled.raw"
            for name, args, logical, physical, viewport, pixel_exact in CASES:
                result = run([str(viewer), "--vmhost", str(vmhost), "--dump-frame", str(dump_path), *args])
                check(result.returncode == 0, f"{name}: viewer exit {result.returncode}: {result.stderr.strip()}")
                check("state=SHOWING width=64 height=48" in result.stdout, f"{name}: viewer did not report SHOWING 64x48")
                print(verify_case(name, dump_path.read_bytes(), result.stdout, logical, physical, viewport, pixel_exact, frame))

            # Bad geometry options are usage errors (exit 2), not silent fallbacks.
            for bad in (["--dump-size", "0x10"], ["--dump-size", "10"], ["--dump-size", "axb"], ["--backing-scale", "0"], ["--backing-scale", "9"]):
                result = run([str(viewer), "--vmhost", str(vmhost), "--dump-frame", str(dump_path), *bad])
                check(result.returncode == 2, f"{' '.join(bad)}: exit {result.returncode}, expected 2")
            result = run([str(viewer), "--vmhost", str(vmhost), "--dump-size", "10x10"])
            check(result.returncode == 2, f"--dump-size without --dump-frame: exit {result.returncode}, expected 2")
    except (Failure, subprocess.TimeoutExpired) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1
    print("display viewer scaled smoke ok")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
