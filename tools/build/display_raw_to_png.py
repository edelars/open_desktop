#!/usr/bin/env python3
"""Convert a screen dump of od-runner (S8-11) to a PNG, with the standard library only.

Input: a file written by `od-runner boot-firmware --framebuffer-dir` (a 64-byte header, then the
frame; layout in docs/testing/WINDOWS.md, section "Журнал фаз и дампы экрана"), or, with
--raw-size WxH, a headerless frame such as the one of `--framebuffer-out` (4 bytes per pixel,
no padding). Pixel format: BGRX8888 (the only format of the display device, format value 1);
the X byte is ignored. Output: 8-bit RGB PNG.
"""

from __future__ import annotations

import argparse
import struct
import sys
import zlib
from pathlib import Path

MAGIC = b"ODFBRAW1"
HEADER_BYTES = 64
FORMAT_BGRX8888 = 1
MAX_SIDE = 16384
# magic, header size, width, height, stride, format, reserved, generation, mode changes,
# virtual ns, retired instructions.
HEADER = struct.Struct("<8s6I4Q")


class DumpError(Exception):
    pass


def parse_dump(data: bytes) -> tuple[dict[str, int], bytes]:
    """Return (header fields, frame bytes) of a dump with a header."""
    if len(data) < HEADER_BYTES or data[:8] != MAGIC:
        raise DumpError("not a screen dump: the ODFBRAW1 header is missing")
    (_, header_bytes, width, height, stride, pixel_format, _, generation, mode_changes, virtual_ns,
     retired) = HEADER.unpack_from(data)
    if header_bytes != HEADER_BYTES:
        raise DumpError(f"unsupported header size {header_bytes}")
    fields = {
        "width": width,
        "height": height,
        "stride": stride,
        "format": pixel_format,
        "generation": generation,
        "mode_change_count": mode_changes,
        "virtual_ns": virtual_ns,
        "retired": retired,
    }
    frame = data[HEADER_BYTES:]
    check_frame(fields, frame)
    return fields, frame


def check_frame(fields: dict[str, int], frame: bytes) -> None:
    width, height, stride = fields["width"], fields["height"], fields["stride"]
    if fields["format"] != FORMAT_BGRX8888:
        raise DumpError(f"unsupported pixel format {fields['format']} (expected {FORMAT_BGRX8888})")
    if not (1 <= width <= MAX_SIDE and 1 <= height <= MAX_SIDE):
        raise DumpError(f"unsupported size {width}x{height}")
    if stride < width * 4:
        raise DumpError(f"stride {stride} is smaller than {width} pixels")
    if len(frame) != stride * height:
        raise DumpError(f"frame has {len(frame)} bytes, the mode needs {stride * height}")


def chunk(kind: bytes, payload: bytes) -> bytes:
    body = kind + payload
    return struct.pack(">I", len(payload)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)


def to_png(fields: dict[str, int], frame: bytes) -> bytes:
    width, height, stride = fields["width"], fields["height"], fields["stride"]
    rows = bytearray()
    for y in range(height):
        line = frame[y * stride : y * stride + width * 4]
        rows.append(0)  # PNG filter type: none
        rgb = bytearray(width * 3)
        rgb[0::3] = line[2::4]  # R
        rgb[1::3] = line[1::4]  # G
        rgb[2::3] = line[0::4]  # B
        rows += rgb
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(bytes(rows), 6))
        + chunk(b"IEND", b"")
    )


def parse_size(text: str) -> tuple[int, int]:
    try:
        width, height = (int(part) for part in text.lower().split("x"))
    except ValueError:
        raise argparse.ArgumentTypeError("expected WIDTHxHEIGHT") from None
    return width, height


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Convert an od-runner screen dump to PNG.")
    parser.add_argument("input", type=Path)
    parser.add_argument("output", type=Path, help="PNG to write (an existing file is refused)")
    parser.add_argument(
        "--raw-size",
        type=parse_size,
        metavar="WxH",
        help="the input has no header: BGRX8888, width*4 bytes per row",
    )
    args = parser.parse_args(argv)
    try:
        data = args.input.read_bytes()
        if args.raw_size:
            width, height = args.raw_size
            fields = {"width": width, "height": height, "stride": width * 4, "format": FORMAT_BGRX8888}
            check_frame(fields, data)
            frame = data
        else:
            fields, frame = parse_dump(data)
        png = to_png(fields, frame)
        with args.output.open("xb") as out:
            out.write(png)
    except (DumpError, OSError) as error:
        print(f"display_raw_to_png: {error}", file=sys.stderr)
        return 2
    print(f"{args.output}: {fields['width']}x{fields['height']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
