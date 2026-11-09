#!/usr/bin/env python3
"""Tests of the Linux artifact staging tool (S4-35).

All tests use temporary directories with tiny fake files and a private manifest copy; the real
~1.3 GB kernel files are never touched.
"""

from __future__ import annotations

import hashlib
import importlib.util
import io
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/build/stage_linux_artifact.py"
SPEC = importlib.util.spec_from_file_location("stage_linux_artifact", SCRIPT)
assert SPEC and SPEC.loader
STAGER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = STAGER
SPEC.loader.exec_module(STAGER)

BASE = "work/vms/fake-input"


def ar_archive(members: list[tuple[str, bytes]]) -> bytes:
    out = b"!<arch>\n"
    for name, data in members:
        out += f"{name:<16}{0:<12}{0:<6}{0:<6}{100644:<8}{len(data):<10}".encode() + b"`\n"
        out += data + (b"\n" if len(data) & 1 else b"")
    return out


DEB = ar_archive([("debian-binary", b"2.0\n"), ("control.tar.zst", b"c" * 200), ("data.tar.zst", b"d" * 400)])

FILES = {
    "linux-image-fake_amd64.deb": DEB,
    "vmlinuz-fake": b"vmlinuz-bytes" * 10,
    "vmlinux-fake": b"vmlinux-bytes" * 10,
    "vmlinux-debug-fake": b"debug-bytes" * 10,
    "source/linux-source_all.deb": ar_archive([("debian-binary", b"2.0\n"), ("data.tar.zst", b"s" * 300)]),
    "source/linux-source.tar.bz2": b"BZh9-not-really" * 5,
    "source/extracted/compressed-head_64.S": b"head\n",
    "source/extracted/compressed-pgtable_64.c": b"pgtable\n",
    "source/extracted/kernel-head_64.S": b"khead\n",
    "source/extracted/config": b"CONFIG_X86_64=y\n",
}


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def make_manifest(pin_sizes: bool = False) -> dict:
    def item(rel: str, **extra) -> dict:
        data = FILES[rel]
        out = {"path": f"{BASE}/{rel}", "sha256": sha(data)}
        if pin_sizes:
            out["size_bytes"] = len(data)
        out.update(extra)
        return out

    return {
        "slice": "S4-00a",
        "target": {"distribution": "Ubuntu", "kernel_source_package_version": "7.0.0-38.38",
                   "binary_artifact_directory": BASE},
        "evidence": {
            "binary_artifacts": [item(r) for r in (
                "linux-image-fake_amd64.deb", "vmlinuz-fake", "vmlinux-fake", "vmlinux-debug-fake")],
            "source_package": item("source/linux-source_all.deb", retrieval_url="https://example.invalid/x.deb",
                                   retrieval="test"),
            "source_tree_archive": item("source/linux-source.tar.bz2"),
            "source_files": [
                item("source/extracted/compressed-head_64.S", archive_path="a/head_64.S", evidence_path=None),
                item("source/extracted/compressed-pgtable_64.c", archive_path="a/pgtable_64.c", evidence_path=None),
                item("source/extracted/kernel-head_64.S", archive_path="a/kernel-head_64.S", evidence_path=None),
            ],
            "config": item("source/extracted/config", extracted_from="buildinfo-data.tar:./config"),
            "elf_comparison": {"common_allocated_file_backed_section_equality": True},
        },
        "status": {
            "exact_source_deb_and_tree_archive_recovered": True,
            "signed_dsc_recovered": False,
            "signed_inrelease_recovered": False,
            "full_source_provenance_passed": False,
        },
        "non_claims": ["not authenticated"],
    }


class StageLinuxArtifactTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        base = Path(self.tmp.name)
        self.input_dir = base / "input"
        self.stage_dir = base / "stage"
        self.evidence = base / "evidence" / "s4-35.json"
        self.manifest_path = base / "manifest.json"
        for rel, data in FILES.items():
            path = self.input_dir / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        self.write_manifest(make_manifest())

    def write_manifest(self, manifest: dict) -> None:
        for item in manifest["evidence"]["source_files"]:
            item["evidence_path"] = item["path"]
        self.manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

    def run_stager(self) -> tuple[int, str, str]:
        out, err = io.StringIO(), io.StringIO()
        code = STAGER.run(self.manifest_path, self.input_dir, self.stage_dir, self.evidence, out, err)
        return code, out.getvalue(), err.getvalue()

    def staged_files(self) -> list[str]:
        if not self.stage_dir.exists():
            return []
        return sorted(p.relative_to(self.stage_dir).as_posix() for p in self.stage_dir.rglob("*") if p.is_file())

    def assert_not_staged(self) -> None:
        self.assertEqual(self.staged_files(), [])
        self.assertFalse(self.evidence.exists())

    def test_success_stages_all_files(self) -> None:
        code, out, _ = self.run_stager()
        self.assertEqual(code, 0)
        self.assertEqual(json.loads(out)["files"], len(FILES))
        evidence = json.loads(self.evidence.read_text())
        self.assertEqual(evidence["status"], "staged (content hash verified; provenance not authenticated)")
        self.assertEqual(evidence["kernel_source_package_version"], "7.0.0-38.38")
        self.assertEqual(len(evidence["files"]), len(FILES))
        for entry in evidence["files"]:
            self.assertEqual(entry["verdict"], "ok")
            self.assertEqual(entry["sha256"], entry["expected_sha256"])
            self.assertEqual(entry["size_bytes"], len(FILES[entry["path"]]))
            self.assertEqual((self.stage_dir / entry["path"]).read_bytes(), FILES[entry["path"]])
        self.assertEqual(self.staged_files(), sorted(list(FILES) + ["STAGED.json"]))
        self.assertTrue(os.path.samefile(self.input_dir / "vmlinuz-fake", self.stage_dir / "vmlinuz-fake"))
        self.assertEqual(evidence["source_config_evidence"]["manifest_slice"], "S4-00a")
        self.assertLess(self.evidence.stat().st_size, 20 * 1024)

    def test_sha256_mismatch_is_rejected(self) -> None:
        path = self.input_dir / "vmlinux-fake"
        data = bytearray(path.read_bytes())
        data[3] ^= 0xFF
        path.write_bytes(bytes(data))
        code, _, err = self.run_stager()
        self.assertEqual(code, 1)
        self.assertIn("sha256_mismatch", err)
        self.assert_not_staged()

    def test_truncated_with_pinned_size_is_rejected(self) -> None:
        self.write_manifest(make_manifest(pin_sizes=True))
        path = self.input_dir / "vmlinuz-fake"
        path.write_bytes(path.read_bytes()[:-5])
        code, _, err = self.run_stager()
        self.assertEqual(code, 1)
        self.assertIn("truncated", err)
        self.assertIn("smaller than manifest size", err)
        self.assert_not_staged()

    def test_truncated_deb_is_diagnosed_structurally(self) -> None:
        path = self.input_dir / "linux-image-fake_amd64.deb"
        path.write_bytes(DEB[:-100])
        code, _, err = self.run_stager()
        self.assertEqual(code, 1)
        self.assertIn("truncated", err)
        self.assertIn("deb:", err)
        self.assert_not_staged()

    def test_missing_binary_is_rejected(self) -> None:
        (self.input_dir / "vmlinux-debug-fake").unlink()
        code, _, err = self.run_stager()
        self.assertEqual(code, 1)
        self.assertIn("missing", err)
        self.assert_not_staged()

    def test_missing_source_archive_is_rejected(self) -> None:
        (self.input_dir / "source/linux-source.tar.bz2").unlink()
        code, _, err = self.run_stager()
        self.assertEqual(code, 1)
        self.assertIn("source_tree_archive", err)
        self.assert_not_staged()

    def test_missing_extracted_source_is_rejected(self) -> None:
        for path in (self.input_dir / "source/extracted").iterdir():
            path.unlink()
        (self.input_dir / "source/extracted").rmdir()
        code, _, err = self.run_stager()
        self.assertEqual(code, 1)
        self.assertIn("missing", err)
        self.assertIn("config", err)
        self.assert_not_staged()

    def test_idempotent_rerun(self) -> None:
        self.assertEqual(self.run_stager()[0], 0)
        first_evidence = self.evidence.read_bytes()
        first_files = self.staged_files()
        self.assertEqual(self.run_stager()[0], 0)
        self.assertEqual(self.evidence.read_bytes(), first_evidence)
        self.assertEqual(self.staged_files(), first_files)

    def test_rerun_with_replaced_stage_file_restores_it(self) -> None:
        self.assertEqual(self.run_stager()[0], 0)
        target = self.stage_dir / "vmlinuz-fake"
        target.unlink()
        target.write_bytes(b"stale")
        self.assertEqual(self.run_stager()[0], 0)
        self.assertEqual(target.read_bytes(), FILES["vmlinuz-fake"])

    def test_rejection_after_success_removes_staged_marker(self) -> None:
        self.assertEqual(self.run_stager()[0], 0)
        self.assertTrue((self.stage_dir / "STAGED.json").exists())
        (self.input_dir / "vmlinuz-fake").write_bytes(b"corrupted")
        self.assertEqual(self.run_stager()[0], 1)
        self.assertFalse((self.stage_dir / "STAGED.json").exists())

    def test_provenance_never_authenticated(self) -> None:
        self.assertEqual(self.run_stager()[0], 0)
        provenance = json.loads(self.evidence.read_text())["provenance"]
        self.assertIs(provenance["authenticated"], False)
        self.assertEqual(provenance["missing"], ["signed_dsc_recovered", "signed_inrelease_recovered",
                                                 "full_source_provenance_passed"])
        self.assertEqual(provenance["signed_chain_files_found_unverified"], [])

    def test_provenance_stays_unauthenticated_even_with_signed_files_present(self) -> None:
        (self.input_dir / "linux_7.0.0-38.38.dsc").write_text("-----BEGIN PGP SIGNED MESSAGE-----\n")
        (self.input_dir / "InRelease").write_text("signed\n")
        self.assertEqual(self.run_stager()[0], 0)
        provenance = json.loads(self.evidence.read_text())["provenance"]
        self.assertIs(provenance["authenticated"], False)
        self.assertEqual(provenance["signed_chain_files_found_unverified"],
                         ["InRelease", "linux_7.0.0-38.38.dsc"])

    def test_broken_manifest_is_usage_error(self) -> None:
        self.manifest_path.write_text("{not json", encoding="utf-8")
        code, _, err = self.run_stager()
        self.assertEqual(code, 2)
        self.assertIn("ERROR", err)
        self.assert_not_staged()

    def test_real_manifest_lists_expected_entries_without_hashing(self) -> None:
        manifest = STAGER.read_manifest(STAGER.DEFAULT_MANIFEST)
        entries = STAGER.manifest_entries(manifest)
        self.assertEqual(len(entries), 10)
        self.assertEqual(sum(1 for e in entries if e["role"] == "binary"), 4)
        self.assertIn("signed_dsc_recovered", STAGER.provenance_missing(manifest))


if __name__ == "__main__":
    unittest.main()
