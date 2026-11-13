#!/usr/bin/env python3
"""Integration test for the pinned, deterministic Stage 5 rootfs recipe."""

from __future__ import annotations

import tempfile
import sys
import uuid
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

from tools.build.build_stage5_rootfs import PACKAGE_CACHE, WORK, build


class Stage5RootfsBuildTest(unittest.TestCase):
    def test_pair_is_reproducible_and_ignores_extra_cached_deb(self) -> None:
        WORK.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="s5-rootfs-pair-", dir=WORK) as temporary:
            output_dir = Path(temporary)
            rootfs_first = build(output_dir / "first.raw")
            stale = PACKAGE_CACHE / f"unlocked-extra-{uuid.uuid4().hex}.deb"
            stale.write_bytes(b"not a Debian archive; must never be opened")
            try:
                rootfs_second = build(output_dir / "second.raw")
            finally:
                stale.unlink(missing_ok=True)
        self.assertEqual(rootfs_first, rootfs_second, "identical inputs must produce identical RAW images")
        print(f"stage5_rootfs_sha256={rootfs_first}")


if __name__ == "__main__":
    unittest.main(verbosity=2)
