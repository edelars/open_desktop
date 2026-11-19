#!/usr/bin/env python3
"""Append the Stage 6 SMP workloads to a Stage 5 rootfs boot initramfs."""

from __future__ import annotations

import argparse
import gzip
import io
from pathlib import Path
import stat
import zlib

from build_stage4_initramfs import BuildError, pack_cpio, parse_cpio, sha256


REPO = Path(__file__).resolve().parents[2]
FIXTURES = REPO / "guest/linux/tests"
INIT_NAME = "s6-smp-init"
PAYLOAD_DIR = "s6-smp"
SOURCE_NAME = "s6_smp_workloads.c"
WRAPPER_NAME = "s6_smp_workloads.sh"

STAGE5_HANDOFF = b"""/bin/busybox mount -o move /proc /newroot/proc || fail 'proc mount handoff failed'
/bin/busybox mount -o move /sys /newroot/sys || fail 'sysfs mount handoff failed'
/bin/busybox mount -o move /dev /newroot/dev || fail 'devtmpfs handoff failed'
[ -c /newroot/dev/console ] || fail 'new-root console absent'
exec </newroot/dev/console >/newroot/dev/console 2>&1
"""
STAGE5_SWITCH_ROOT = b"exec /bin/busybox switch_root /newroot /bin/sh\n"
STAGE6_SWITCH_ROOT = f"""\
mount -t tmpfs -o nodev,nosuid tmpfs /newroot/run || fail 'S6 tmpfs mount failed'
mkdir /newroot/run/open-desktop-stage6 || fail 'S6 payload directory failed'
/bin/busybox cp /{PAYLOAD_DIR}/{SOURCE_NAME} /{PAYLOAD_DIR}/{WRAPPER_NAME} /newroot/run/open-desktop-stage6/ || fail 'S6 payload copy failed'
read -r cmdline </newroot/proc/cmdline || fail 'S6 cmdline read failed'
set -f
set --
for word in $cmdline; do
  case $word in
    od.nonce=*) set -- "$@" "${{word#od.nonce=}}" ;;
  esac
done
exec /bin/busybox switch_root /newroot /bin/sh /run/open-desktop-stage6/{WRAPPER_NAME} "$@"
""".encode("ascii")


def build_archive(stage5_initramfs: bytes) -> bytes:
    base = gzip.decompress(stage5_initramfs)
    if len(base) % 4:
        raise BuildError("Stage 5 newc archive is not aligned to four bytes")
    entries = parse_cpio(base)
    names = [entry["name"].removeprefix("./") for entry in entries]
    if any(name == INIT_NAME or name.split("/", 1)[0] == PAYLOAD_DIR for name in names):
        raise BuildError("Stage 5 initramfs already contains an S6 payload path")
    init_entries = [entry for name, entry in zip(names, entries) if name == "init"]
    if len(init_entries) != 1 or not stat.S_ISREG(init_entries[0]["mode"]):
        raise BuildError("Stage 5 initramfs must contain one regular /init")
    init = init_entries[0]["data"]
    if not init.endswith(STAGE5_HANDOFF + STAGE5_SWITCH_ROOT):
        raise BuildError("Stage 5 /init is missing the proc/sys/dev and switch_root handoff")

    overlay = pack_cpio([
        (INIT_NAME, 0o100755, init.removesuffix(STAGE5_SWITCH_ROOT) + STAGE6_SWITCH_ROOT),
        (PAYLOAD_DIR, 0o040755, b""),
        (f"{PAYLOAD_DIR}/{SOURCE_NAME}", 0o100644, (FIXTURES / SOURCE_NAME).read_bytes()),
        (f"{PAYLOAD_DIR}/{WRAPPER_NAME}", 0o100755, (FIXTURES / WRAPPER_NAME).read_bytes()),
    ])
    result = io.BytesIO()
    with gzip.GzipFile(fileobj=result, mode="wb", mtime=0) as stream:
        stream.write(base)
        stream.write(overlay)
    return result.getvalue()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stage5-initramfs", type=Path, required=True, help="accepted Stage 5 gzip initramfs")
    parser.add_argument("--out", type=Path, required=True, help="new gzip initramfs path")
    args = parser.parse_args()
    try:
        archive = build_archive(args.stage5_initramfs.read_bytes())
        args.out.parent.mkdir(parents=True, exist_ok=True)
        with args.out.open("xb") as output:
            output.write(archive)
    except (BuildError, OSError, EOFError, zlib.error) as error:
        parser.exit(1, f"{error}\n")
    print(f"{args.out}: sha256 {sha256(archive)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
