#!/usr/bin/env python3
"""Package the Stage 6 SMP workloads or verify their Linux guest report."""

from __future__ import annotations

import argparse
import gzip
import io
import json
from pathlib import Path
import re
import stat
import zlib

from build_stage4_initramfs import BuildError, pack_cpio, parse_cpio, sha256
from build_stage6_smp_initramfs import SMP_BRINGUP, parse_online, read_com1_text


REPO = Path(__file__).resolve().parents[2]
FIXTURES = REPO / "guest/linux/tests"
INIT_NAME = "s6-smp-init"
PAYLOAD_DIR = "s6-smp"
SOURCE_NAME = "s6_smp_workloads.c"
WRAPPER_NAME = "s6_smp_workloads.sh"
WORKLOAD_COUNTS = (
    ("counter", 20_000),
    ("condition", 256),
    ("exchange", 128),
    ("mapping", 32),
    ("processes", 32),
)
PASS = re.compile(
    r"OD_S6_22_PASS nonce=([A-Za-z0-9._-]+) case=([a-z]+)"
    r" cpus=([0-9]{1,3}),([0-9]{1,3}) checked=([0-9]{1,8})"
)
LINUX_FAILURE = re.compile(
    r"\b(?:BUG|Oops):|Kernel panic|detected (?:soft|hard) lockup"
    r"|\brcu\b[^\r\n]*\bdetected stalls?\b|\btask\b[^\r\n]*\bblocked for more than [0-9]+ seconds",
    re.I,
)

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
compute_requested=0
for word in $cmdline; do
  case $word in
    od.nonce=*) set -- "$@" "${{word#od.nonce=}}" ;;
    od.s6_compute=*)
      [ "$compute_requested" -eq 0 ] || fail 'S6 compute option repeated'
      compute_requested=1
      compute_setting=${{word#od.s6_compute=}}
      ;;
  esac
done
if [ "$compute_requested" -eq 1 ]; then
  set -- "$@" --compute "$compute_setting"
fi
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


def read_workload_lines(path: Path, nonce: str) -> list[str]:
    if re.fullmatch(r"[A-Za-z0-9._-]{1,128}", nonce) is None:
        raise BuildError("invalid workload nonce")
    # Kernel output may be interleaved with complete marker lines on COM1.
    lines = [line.strip() for line in read_com1_text(path).splitlines()]
    if any(LINUX_FAILURE.search(line) for line in lines):
        raise BuildError("kernel reported a BUG, panic, oops, lockup or task/CPU stall")
    return lines


def verify_bringup(lines: list[str], first_marker: int, expected_online: int) -> None:
    brought_up = [
        (index, int(match[1]))
        for index, line in enumerate(lines)
        if (match := SMP_BRINGUP.fullmatch(line)) is not None
    ]
    if len(brought_up) != 1 or brought_up[0][0] >= first_marker or brought_up[0][1] != expected_online:
        raise BuildError("expected one matching kernel SMP bring-up summary before the workloads")


def verify_online(line: str, task: int, nonce: str, expected: int) -> list[int]:
    prefix = f"OD_S6_{task}_ONLINE nonce={nonce} cpus="
    if not line.startswith(prefix):
        raise BuildError("missing matching sysfs online CPU list at workload completion")
    online = parse_online(line.removeprefix(prefix))
    if online != list(range(expected)):
        raise BuildError("guest sysfs online CPUs differ from the expected topology")
    return online


def verify_workloads(path: Path, nonce: str, expected_online: int) -> dict[str, object]:
    if expected_online not in (2, 4):
        raise BuildError("expected 2 or 4 online CPUs")
    lines = read_workload_lines(path, nonce)
    markers = [(index, line) for index, line in enumerate(lines) if line.startswith("OD_S6_22_")]
    if len(markers) != len(WORKLOAD_COUNTS) + 3:
        raise BuildError("expected source, all five workloads, online CPUs and completion")
    source_hash = sha256((FIXTURES / SOURCE_NAME).read_bytes())
    if markers[0][1] != f"OD_S6_22_SOURCE nonce={nonce} sha256={source_hash}":
        raise BuildError("guest source hash or nonce mismatch")
    if markers[-1][1] != f"OD_S6_22_COMPLETE nonce={nonce}":
        raise BuildError("missing matching workload completion")
    online = verify_online(markers[-2][1], 22, nonce, expected_online)
    verify_bringup(lines, markers[0][0], expected_online)
    cpus: tuple[int, int] | None = None
    counts = {}
    for (case, checked), (_, line) in zip(WORKLOAD_COUNTS, markers[1:-2], strict=True):
        match = PASS.fullmatch(line)
        if match is None or match[1] != nonce or match[2] != case or int(match[5]) != checked:
            raise BuildError(f"invalid result for workload {case}")
        previous, current = int(match[3]), int(match[4])
        if not 0 <= previous < current < expected_online:
            raise BuildError(f"invalid CPU pair for workload {case}")
        if cpus is not None and cpus != (previous, current):
            raise BuildError(f"CPU pair changed in workload {case}")
        cpus = previous, current
        counts[case] = checked
    return {
        "nonce": nonce,
        "source_sha256": source_hash,
        "online_cpus": online,
        "workload_cpus": cpus,
        "checked": counts,
    }


def verify_compute(path: Path, nonce: str, expected_online: int, compute: str) -> dict[str, object]:
    match = re.fullmatch(r"(1|2|4),([1-9][0-9]{0,19}),([0-9a-fA-F]{16})", compute)
    if match is None:
        raise BuildError("--compute needs WORKERS,BLOCKS,16-HEX-DIGIT-CHECKSUM")
    workers, blocks, checksum = int(match[1]), int(match[2]), match[3].lower()
    if expected_online not in (1, 2, 4) or workers > expected_online or not workers <= blocks < 2**64:
        raise BuildError("invalid CPU topology or compute block count")
    lines = read_workload_lines(path, nonce)
    markers = [(index, line) for index, line in enumerate(lines) if line.startswith("OD_S6_38_")]
    source_hash = sha256((FIXTURES / SOURCE_NAME).read_bytes())
    expected = [
        f"OD_S6_38_SOURCE nonce={nonce} sha256={source_hash}",
        f"OD_S6_38_BEGIN nonce={nonce} workers={workers} blocks={blocks}",
        f"OD_S6_38_END nonce={nonce}",
        f"OD_S6_38_COMPLETE nonce={nonce} workers={workers} blocks={blocks} checksum={checksum}",
    ]
    if len(markers) != 5 or [line for _, line in markers[:-1]] != expected:
        raise BuildError("expected one ordered compute result with matching nonce, source, blocks and checksum")
    verify_bringup(lines, markers[0][0], expected_online)
    return {
        "nonce": nonce,
        "source_sha256": source_hash,
        "online_cpus": verify_online(markers[-1][1], 38, nonce, expected_online),
        "workers": workers,
        "blocks": blocks,
        "checksum": checksum,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stage5-initramfs", type=Path, help="accepted Stage 5 gzip initramfs")
    parser.add_argument("--out", type=Path, help="new gzip initramfs path")
    parser.add_argument("--verify-report", type=Path, help="LinuxDirect JSON with the guest serial transcript")
    parser.add_argument("--nonce", help="expected nonce in the workload report")
    parser.add_argument("--expect-online", type=int, choices=(1, 2, 4), help="expected CPUs brought online by Linux")
    parser.add_argument("--compute", help="verify S6-38 instead of S6-22: WORKERS,BLOCKS,CHECKSUM")
    args = parser.parse_args()
    if args.verify_report is not None:
        if args.stage5_initramfs is not None or args.out is not None:
            parser.error("--verify-report is separate from initramfs packaging")
        if args.nonce is None or args.expect_online is None:
            parser.error("--verify-report requires --nonce and --expect-online")
        if args.compute is None and args.expect_online == 1:
            parser.error("S6-22 needs --expect-online 2 or 4")
        try:
            if args.compute is None:
                report = verify_workloads(args.verify_report, args.nonce, args.expect_online)
            else:
                report = verify_compute(args.verify_report, args.nonce, args.expect_online, args.compute)
            print(json.dumps(report, indent=2))
        except (BuildError, OSError, json.JSONDecodeError) as error:
            parser.exit(1, f"{error}\n")
        return 0
    if args.stage5_initramfs is None or args.out is None:
        parser.error("packaging requires --stage5-initramfs and --out")
    if args.nonce is not None or args.expect_online is not None or args.compute is not None:
        parser.error("--nonce, --expect-online and --compute require --verify-report")
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
