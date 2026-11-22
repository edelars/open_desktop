#!/usr/bin/env python3
"""S7-31n: build the efivarfs/dmesg /init (tests/fixtures/guest/s7_31n/init.S) into a newc cpio.

The init is a freestanding static ELF built with the stage 4 toolchain recipe (clang for the
assembler, ld.lld from the Rust toolchain, the stage 4 linker script). The archive goes to
work/stage7/s7-31n/initramfs.cpio and is never committed. The init only uses instruction forms
the CPL3 allow-list of the CPU model already admits (see the file header).
"""
from __future__ import annotations

import argparse
import os
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from build_stage4_flat_probe import resolve_executable, run, rust_toolchain, sha256  # noqa: E402
from build_stage4_initramfs import INIT_MODE, INIT_NAME, pack_cpio  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "tests/fixtures/guest/s7_31n"
LAYOUT = ROOT / "tests/fixtures/guest/stage4/initramfs/layout.ld"
DEFAULT_OUT = ROOT / "work/stage7/s7-31n"


def build(out: Path) -> Path:
    clang = resolve_executable(os.environ.get("STAGE4_CLANG"), "clang")
    _rustc, _host, _version, ld_lld, _objcopy, tool_env = rust_toolchain()
    env = tool_env.copy()
    env.update({"LC_ALL": "C", "SOURCE_DATE_EPOCH": "0"})
    out.mkdir(parents=True, exist_ok=True)
    obj, elf = out / "init.o", out / "init"
    run([str(clang), "--target=x86_64-unknown-none-elf", "-x", "assembler-with-cpp", "-ffreestanding",
         "-fno-pic", "-fno-pie", "-c", "init.S", "-o", str(obj)], env=env, cwd=FIXTURE)
    run([str(ld_lld), "-m", "elf_x86_64", "-static", "--build-id=none", "--strip-all", "-z",
         "max-page-size=4096", "-e", "_start", "-T", str(LAYOUT), "-o", str(elf), str(obj)],
        env=env, cwd=FIXTURE)
    archive = out / "initramfs.cpio"
    archive.write_bytes(pack_cpio([(INIT_NAME, INIT_MODE, elf.read_bytes())]))
    return archive


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out-dir", type=Path, default=DEFAULT_OUT, help="inside work/ (default work/stage7/s7-31n)")
    args = parser.parse_args()
    archive = build(args.out_dir)
    print(f"{archive} sha256={sha256(archive.read_bytes())}")


if __name__ == "__main__":
    main()
