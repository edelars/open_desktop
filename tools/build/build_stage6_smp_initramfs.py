#!/usr/bin/env python3
"""Build the S6-21 initramfs or verify its Linux guest observations."""

from __future__ import annotations

import argparse
import json
import os
import re
import tempfile
from pathlib import Path

from build_stage4_flat_probe import BuildError, resolve_executable, run, rust_toolchain, sha256
from build_stage4_initramfs import INIT_MODE, INIT_NAME, check_elf, pack_cpio, validate_nonce

REPO = Path(__file__).resolve().parents[2]
SOURCE = REPO / "tests/fixtures/guest/stage6/smp-init/init.S"
LAYOUT = REPO / "tests/fixtures/guest/stage4/initramfs/layout.ld"
DEFAULT_OUTPUT = REPO / "work/stage6/smp-init/initramfs.cpio"
SMP_BRINGUP = re.compile(
    r"(?:\[\s*[0-9]+\.[0-9]+\]\s*)?smp: Brought up [1-9][0-9]* nodes?, ([1-9][0-9]{0,3}) CPUs?"
)


def build_archive() -> bytes:
    clang = resolve_executable(os.environ.get("STAGE6_CLANG"), "clang")
    _, _, _, ld_lld, _, tool_env = rust_toolchain()
    env = {**tool_env, "LC_ALL": "C", "SOURCE_DATE_EPOCH": "0"}
    with tempfile.TemporaryDirectory(prefix="stage6-smp-init-") as directory:
        obj = Path(directory) / "init.o"
        elf = Path(directory) / INIT_NAME
        run(
            [
                str(clang), "--target=x86_64-unknown-none-elf", "-x", "assembler-with-cpp",
                "-ffreestanding", "-fno-pic", "-fno-pie",
                "-c", str(SOURCE), "-o", str(obj),
            ],
            env=env,
        )
        run(
            [
                str(ld_lld), "-m", "elf_x86_64", "-static", "--build-id=none", "--strip-all",
                "-z", "max-page-size=4096", "-e", "_start", "-T", str(LAYOUT),
                "-o", str(elf), str(obj),
            ],
            env=env,
        )
        executable = elf.read_bytes()

    check_elf(executable)
    return pack_cpio([(INIT_NAME, INIT_MODE, executable)])


def parse_online(cpulist: str) -> list[int]:
    cpus: list[int] = []
    for group in cpulist.split(","):
        match = re.fullmatch(r"([0-9]{1,4})(?:-([0-9]{1,4}))?", group)
        if match is None:
            raise BuildError(f"invalid online CPU list: {cpulist!r}")
        first = int(match[1])
        last = int(match[2]) if match[2] is not None else first
        if first > last or last > 4095 or (cpus and first <= cpus[-1]):
            raise BuildError(f"invalid online CPU list: {cpulist!r}")
        cpus.extend(range(first, last + 1))
    return cpus


def parse_observation(
    transcript: str, nonce: str, *, require_smp_evidence: bool = False
) -> dict[str, object]:
    validate_nonce(nonce)
    lines = transcript.splitlines()
    if any(line.startswith("OD_SMP_ERROR ") for line in lines):
        raise BuildError("guest /init reported an observation error")
    begin = [i for i, line in enumerate(lines) if line.startswith("OD_SMP_V1 ")]
    ends = [i for i, line in enumerate(lines) if line == "OD_SMP_END"]
    if len(begin) != 1 or len(ends) != 1 or lines[begin[0]] != f"OD_SMP_V1 {nonce}":
        raise BuildError("expected a single complete guest observation with the requested nonce")

    body = lines[begin[0] + 1 : ends[0]]
    if (
        len(body) < 5
        or not body[0].startswith("OD_CPU_ONLINE ")
        or not body[1].startswith("OD_CPU_COUNT ")
        or body[2] != "OD_IRQ_BEGIN"
        or body[-1] != "OD_IRQ_END"
    ):
        raise BuildError("malformed guest observation block")
    online = body[0].removeprefix("OD_CPU_ONLINE ")
    cpus = parse_online(online)
    if body[1] != f"OD_CPU_COUNT {len(cpus)}":
        raise BuildError("guest CPU count does not match its online list")

    interrupts = body[3:-1]
    if interrupts[0].split() != [f"CPU{cpu}" for cpu in cpus]:
        raise BuildError("/proc/interrupts CPU header does not match the online list")
    ipis: dict[str, list[int]] = {}
    for line in interrupts[1:]:
        label, separator, fields = line.strip().partition(":")
        if label not in {"RES", "CAL", "TLB"}:
            continue
        tokens = fields.split()
        counts = tokens[:len(cpus)]
        if (
            not separator
            or label in ipis
            or len(counts) != len(cpus)
            or any(re.fullmatch(r"[0-9]+", count) is None for count in counts)
            or (len(tokens) > len(cpus) and re.fullmatch(r"[0-9]+", tokens[len(cpus)]))
        ):
            raise BuildError(f"invalid guest IPI row: {line!r}")
        ipis[label] = [int(count) for count in counts]

    if require_smp_evidence:
        if len(cpus) < 2:
            raise BuildError("SMP evidence requires at least two online CPUs")
        brought_up = [
            int(match[1])
            for line in lines[:begin[0]]
            if (match := SMP_BRINGUP.fullmatch(line.strip())) is not None
        ]
        if brought_up != [len(cpus)]:
            raise BuildError("expected one matching kernel SMP bring-up summary before the observation")
        for index, cpu in enumerate(cpus):
            if not any(counts[index] > 0 for counts in ipis.values()):
                raise BuildError(f"CPU{cpu} has no nonzero guest IPI count")

    return {"nonce": nonce, "online": online, "cpus": cpus, "interrupts": interrupts, "ipi_counts": ipis}


def read_com1_text(path: Path) -> str:
    report = json.loads(path.read_text())
    try:
        transcript = report["stop"]["com1_text"]
    except (KeyError, TypeError) as error:
        raise BuildError("missing stop.com1_text in the Linux run report") from error
    if not isinstance(transcript, str):
        raise BuildError("stop.com1_text is not text")
    return transcript


def verify_report(
    path: Path, nonce: str, expected_online: int | None, *, require_smp_evidence: bool = False
) -> dict[str, object]:
    transcript = read_com1_text(path)
    observation = parse_observation(transcript, nonce, require_smp_evidence=require_smp_evidence)
    if expected_online is not None and observation["cpus"] != list(range(expected_online)):
        raise BuildError(f"expected online CPUs 0 through {expected_online - 1}")
    return observation


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--out", type=Path, help="initramfs archive path")
    mode.add_argument("--verify-report", type=Path, help="Linux run JSON to verify")
    parser.add_argument("--nonce", help="expected nonce in the guest observation")
    parser.add_argument("--expect-online", type=int, help="require this many online CPUs starting at zero")
    parser.add_argument(
        "--require-smp-evidence", action="store_true",
        help="require the kernel bring-up summary and a nonzero guest IPI count for each online CPU",
    )
    args = parser.parse_args()
    if args.verify_report is not None and args.nonce is None:
        parser.error("--verify-report requires --nonce")
    if args.verify_report is None and (
        args.nonce is not None or args.expect_online is not None or args.require_smp_evidence
    ):
        parser.error("--nonce, --expect-online and --require-smp-evidence apply to --verify-report")
    if args.expect_online is not None and not 1 <= args.expect_online <= 4096:
        parser.error("--expect-online must be between 1 and 4096")
    try:
        if args.verify_report is not None:
            observation = verify_report(
                args.verify_report, args.nonce, args.expect_online,
                require_smp_evidence=args.require_smp_evidence,
            )
            print(json.dumps(observation, indent=2))
            return 0
        output = args.out or DEFAULT_OUTPUT
        archive = build_archive()
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(archive)
    except (BuildError, OSError, ValueError) as error:
        parser.exit(1, f"{error}\n")
    print(f"{output}: sha256 {sha256(archive)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
