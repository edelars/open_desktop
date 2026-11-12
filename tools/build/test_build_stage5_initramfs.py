#!/usr/bin/env python3
"""Paired deterministic Stage 5 initramfs/kernel build test."""

from __future__ import annotations

import sys
import tempfile
import unittest
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

from tools.build.build_stage5_initramfs import CONTAINER_BUILD, build
from tools.build.build_stage5_rootfs import BUILDER_PACKAGE_CACHE, PACKAGE_CACHE, ROOT_DEVICE, WORK


class Stage5InitramfsBuildTest(unittest.TestCase):
    def test_pair_is_reproducible_and_ignores_extra_cached_debs(self) -> None:
        self.assertEqual(ROOT_DEVICE, "/dev/nvme0n1")
        self.assertIn(f"mount -t ext4 {ROOT_DEVICE} /newroot", CONTAINER_BUILD)
        WORK.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="s5-initramfs-pair-", dir=WORK) as temporary:
            output_dir = Path(temporary)
            first = build(output_dir / "first.cpio.gz", output_dir / "first.vmlinuz")
            rogue_files = [
                PACKAGE_CACHE / f"unlocked-package-{uuid.uuid4().hex}.deb",
                BUILDER_PACKAGE_CACHE / f"unlocked-builder-{uuid.uuid4().hex}.deb",
            ]
            for path in rogue_files:
                path.write_bytes(b"not a Debian archive; must never be opened")
            try:
                second = build(output_dir / "second.cpio.gz", output_dir / "second.vmlinuz")
            finally:
                for path in rogue_files:
                    path.unlink(missing_ok=True)
        self.assertEqual(first, second, "identical locks must yield identical initramfs and kernel")
        print(f"stage5_initramfs_sha256={first[0]}")
        print(f"stage5_kernel_sha256={first[1]}")


if __name__ == "__main__":
    unittest.main(verbosity=2)
