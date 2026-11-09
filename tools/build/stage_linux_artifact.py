#!/usr/bin/env python3
"""Stage the pinned Ubuntu Linux 7.0.0-38.38 artifact set (S4-35).

Reads the S4-00a manifest, verifies every listed binary artifact, source package, source
archive, selected source file and the kernel config by size (when the manifest pins one) and
SHA-256 in the input directory, and only if all of them pass places each file (hardlink,
copy as fallback) into the ignored staging directory `work/stage4/linux-7.0.0-38.38/`.
An evidence JSON is written to `docs/reports/data/stage4/s4-35-linux-artifact.json`.

Verified here: content hashes (and sizes when pinned). NOT verified here: authenticity.
The signed chain (.dsc, orig.tar.gz, diff.gz, Sources.xz, InRelease, apt-provenance.json) is
not recovered; the evidence always says `provenance.authenticated = false`.

Only the standard library is used. Kernel payloads are never written into Git: the staging
directory is ignored (`/work/**`) and the stdout of this tool is a short summary.

Exit codes: 0 staged, 1 rejected (nothing staged), 2 usage or manifest error.
"""

from __future__ import annotations

import argparse
import bz2
import hashlib
import json
import os
import shutil
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_MANIFEST = ROOT / "docs/reports/data/stage4/s4-00a-source-audit-2026-11-05.json"
DEFAULT_INPUT_DIR = ROOT / "work/vms/s2-00-ubuntu-26.04.1"
DEFAULT_STAGE_DIR = ROOT / "work/stage4/linux-7.0.0-38.38"
DEFAULT_EVIDENCE = ROOT / "docs/reports/data/stage4/s4-35-linux-artifact.json"
MARKER_NAME = "STAGED.json"
STATUS_STAGED = "staged (content hash verified; provenance not authenticated)"
CHUNK = 8 * 1024 * 1024

SOURCE_ROLES = {"source_package", "source_tree_archive", "source_file", "config"}

# Files that, if found next to the artifacts, would be candidates for a signature check.
# This tool never verifies signatures, so finding them does not change `authenticated`.
SIGNED_CHAIN_GLOBS = (
    "*.dsc",
    "*.orig.tar.*",
    "*.diff.gz",
    "Sources.xz",
    "Sources.gz",
    "InRelease",
    "Release.gpg",
    "apt-provenance.json",
)

LIMITATIONS = [
    "Only content hashes are verified; no signature or repository chain is verified.",
    "The S4-00a manifest pins no file sizes; sizes are measured and recorded here, truncation is caught by SHA-256 mismatch and by structural diagnosis (deb ar members, vmlinuz header, ELF section table, bz2 stream).",
    "No zstd stream test is run (stdlib only); the data.tar.zst inside .deb files is not decompressed.",
    "Matching hashes do not prove that the source package and config produced the staged binaries.",
    "Linux boot, runtime behaviour and physical CPU behaviour are not established by staging.",
]


class ManifestError(Exception):
    pass


def sha256_file(path: Path) -> tuple[str, int]:
    digest = hashlib.sha256()
    total = 0
    with open(path, "rb") as handle:
        while True:
            block = handle.read(CHUNK)
            if not block:
                break
            digest.update(block)
            total += len(block)
    return digest.hexdigest(), total


def read_manifest(path: Path) -> dict:
    try:
        manifest = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise ManifestError(f"cannot read manifest {path}: {error}") from error
    if not isinstance(manifest, dict) or "evidence" not in manifest or "target" not in manifest:
        raise ManifestError(f"manifest {path} has no 'target'/'evidence' sections")
    return manifest


def manifest_entries(manifest: dict) -> list[dict]:
    """Flatten the S4-00a evidence into [{role, name, rel, sha256, size_bytes?}]."""
    evidence = manifest["evidence"]
    base = manifest["target"].get("binary_artifact_directory")
    if not base:
        raise ManifestError("manifest target.binary_artifact_directory is missing")
    prefix = base.rstrip("/") + "/"

    def entry(role: str, name: str, item: dict, path_key: str = "path") -> dict:
        full = item.get(path_key)
        digest = item.get("sha256")
        if not full or not digest:
            raise ManifestError(f"manifest item {name} lacks {path_key}/sha256")
        if not full.startswith(prefix):
            raise ManifestError(f"manifest path {full} is outside {base}")
        rel = full[len(prefix):]
        if rel.startswith("/") or ".." in Path(rel).parts:
            raise ManifestError(f"unsafe manifest path: {full}")
        result = {"role": role, "name": name, "rel": rel, "sha256": digest.lower()}
        if "size_bytes" in item:
            result["size_bytes"] = int(item["size_bytes"])
        return result

    entries = []
    for item in evidence.get("binary_artifacts", []):
        entries.append(entry("binary", Path(item["path"]).name, item))
    entries.append(entry("source_package", "source_package", evidence["source_package"]))
    entries.append(entry("source_tree_archive", "source_tree_archive", evidence["source_tree_archive"]))
    for item in evidence.get("source_files", []):
        entries.append(entry("source_file", item["archive_path"], item, "evidence_path"))
    entries.append(entry("config", "config", evidence["config"]))
    rels = [item["rel"] for item in entries]
    if len(set(rels)) != len(rels):
        raise ManifestError("manifest lists the same path twice")
    return entries


def provenance_missing(manifest: dict) -> list[str]:
    """Open provenance items: every S4-00a status flag that is not true."""
    status = manifest.get("status", {})
    missing = [key for key, value in status.items() if value is False]
    return missing


def diagnose_structure(path: Path, size: int) -> str | None:
    """Return a reason if the file is structurally truncated/corrupt, else None.

    Only called after a hash mismatch, to give a clearer reason than a bare mismatch. A reason
    starting with "!" is a format problem (wrong magic), not evidence of truncation.
    """
    name = path.name
    try:
        with open(path, "rb") as handle:
            head = handle.read(0x400)
            if name.endswith(".deb"):
                if head[:8] != b"!<arch>\n":
                    return "!deb: missing ar magic"
                pos = 8
                while pos < size:
                    handle.seek(pos)
                    header = handle.read(60)
                    if len(header) < 60 or header[58:60] != b"`\n":
                        return f"deb: truncated or invalid ar member header at {pos}"
                    try:
                        member = int(header[48:58].decode("ascii").strip())
                    except ValueError:
                        return f"deb: invalid ar member size at {pos}"
                    end = pos + 60 + member
                    if end > size:
                        return f"deb: ar member runs past end of file ({end} > {size})"
                    pos = end + (end & 1)
                return None
            if name.startswith("vmlinuz"):
                if len(head) < 0x210 or head[0x202:0x206] != b"HdrS":
                    return "vmlinuz: boot header 'HdrS' not found (truncated?)"
                setup_sects = head[0x1F1] or 4
                (syssize,) = struct.unpack_from("<I", head, 0x1F4)
                need = (setup_sects + 1) * 512 + syssize * 16
                if size < need:
                    return f"vmlinuz: file is {size} bytes, boot header needs at least {need}"
                return None
            if name.startswith("vmlinux"):
                if head[:4] != b"\x7fELF" or head[4] != 2:
                    return "!vmlinux: not an ELF64 file"
                (shoff,) = struct.unpack_from("<Q", head, 0x28)
                shentsize, shnum = struct.unpack_from("<HH", head, 0x3A)
                if shoff + shentsize * shnum > size:
                    return f"vmlinux: section header table ends at {shoff + shentsize * shnum}, file is {size}"
                return None
            if name.endswith(".bz2"):
                if head[:3] != b"BZh":
                    return "!bz2: missing magic"
                handle.seek(0)
                decoder = bz2.BZ2Decompressor()
                while not decoder.eof:
                    block = handle.read(CHUNK)
                    if not block:
                        return "bz2: stream ends before end-of-stream marker (truncated)"
                    try:
                        decoder.decompress(block)
                    except (OSError, ValueError) as error:
                        return f"bz2: corrupt stream ({error})"
                return None
    except OSError as error:
        return f"unreadable: {error}"
    return None


def verify_entry(entry: dict, input_dir: Path) -> dict:
    path = input_dir / entry["rel"]
    result = {
        "role": entry["role"],
        "name": entry["name"],
        "path": entry["rel"],
        "expected_sha256": entry["sha256"],
        "size_bytes": None,
        "sha256": None,
        "verdict": None,
        "reason": None,
    }
    if not path.is_file():
        result["verdict"] = "missing"
        result["reason"] = "file not found in input directory"
        return result
    size = path.stat().st_size
    result["size_bytes"] = size
    expected_size = entry.get("size_bytes")
    if expected_size is not None and size < expected_size:
        result["verdict"] = "truncated"
        result["reason"] = f"size {size} is smaller than manifest size {expected_size}"
        return result
    if expected_size is not None and size > expected_size:
        result["verdict"] = "size_mismatch"
        result["reason"] = f"size {size} is larger than manifest size {expected_size}"
        return result
    digest, measured = sha256_file(path)
    result["sha256"] = digest
    result["size_bytes"] = measured
    if digest != entry["sha256"]:
        reason = diagnose_structure(path, measured)
        if reason and not reason.startswith("!"):
            result["verdict"] = "truncated"
            result["reason"] = f"sha256 mismatch; structural check: {reason}"
        elif reason:
            result["verdict"] = "sha256_mismatch"
            result["reason"] = f"sha256 {digest} != manifest {entry['sha256']}; format check: {reason[1:]}"
        else:
            result["verdict"] = "sha256_mismatch"
            result["reason"] = f"sha256 {digest} != manifest {entry['sha256']}"
        return result
    result["verdict"] = "ok"
    return result


def check_source_tree(input_dir: Path, entries: list[dict]) -> list[str]:
    """Source/config evidence directories that must exist next to the files."""
    problems = []
    parents = {Path(item["rel"]).parent for item in entries if item["role"] in SOURCE_ROLES}
    for parent in sorted(parents):
        if not (input_dir / parent).is_dir():
            problems.append(f"source evidence directory missing: {parent}")
    return problems


def stage_file(source: Path, destination: Path) -> str:
    """Hardlink (copy as fallback); idempotent. Returns 'hardlink' or 'copy'."""
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.exists() or destination.is_symlink():
        try:
            if os.path.samefile(source, destination):
                return "hardlink"
        except OSError:
            pass
        destination.unlink()
    try:
        os.link(source, destination)
        return "hardlink"
    except OSError:
        pass
    temp = destination.with_name(destination.name + ".part")
    shutil.copyfile(source, temp)
    os.replace(temp, destination)
    return "copy"


def relative_for_report(path: Path) -> str:
    try:
        return path.resolve().relative_to(ROOT).as_posix()
    except ValueError:
        return str(path)


def signed_files_present(input_dir: Path) -> list[str]:
    found = set()
    if input_dir.is_dir():
        for pattern in SIGNED_CHAIN_GLOBS:
            for hit in input_dir.rglob(pattern):
                if hit.is_file():
                    found.add(hit.relative_to(input_dir).as_posix())
    return sorted(found)


def build_evidence(manifest: dict, manifest_path: Path, input_dir: Path, stage_dir: Path,
                   results: list[dict]) -> dict:
    manifest_digest, _ = sha256_file(manifest_path)
    version = manifest["target"].get("kernel_source_package_version")
    evidence = manifest["evidence"]
    return {
        "slice": "S4-35",
        "distribution": manifest["target"].get("distribution"),
        "kernel_source_package_version": version,
        "architecture": "amd64 generic",
        "status": STATUS_STAGED,
        "manifest": {"path": relative_for_report(manifest_path), "sha256": manifest_digest},
        "input_dir": relative_for_report(input_dir),
        "stage_dir": relative_for_report(stage_dir),
        "file_count": len(results),
        "files": results,
        "provenance": {
            "authenticated": False,
            "missing": provenance_missing(manifest),
            "signed_chain_files_found_unverified": signed_files_present(input_dir),
            "note": "S4-35 verifies content hashes only; it does not verify signatures or the source-to-binary mapping.",
        },
        "source_config_evidence": {
            "manifest": relative_for_report(manifest_path),
            "manifest_slice": manifest.get("slice"),
            "source_package_retrieval_url": evidence["source_package"].get("retrieval_url"),
            "source_package_retrieval": evidence["source_package"].get("retrieval"),
            "config_extracted_from": evidence["config"].get("extracted_from"),
            "elf_comparison": evidence.get("elf_comparison"),
            "non_claims": manifest.get("non_claims", []),
        },
        "limitations": LIMITATIONS,
    }


def write_json(path: Path, data: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temp = path.with_name(path.name + ".part")
    temp.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    os.replace(temp, path)


def run(manifest_path: Path, input_dir: Path, stage_dir: Path, evidence_path: Path,
        out=sys.stdout, err=sys.stderr) -> int:
    try:
        manifest = read_manifest(manifest_path)
        entries = manifest_entries(manifest)
    except (ManifestError, KeyError) as error:
        print(f"ERROR: {error}", file=err)
        return 2

    marker = stage_dir / MARKER_NAME
    results = [verify_entry(item, input_dir) for item in entries]
    problems = [f"{r['role']} {r['path']}: {r['verdict']} ({r['reason']})" for r in results if r["verdict"] != "ok"]
    if not any(r["role"] in SOURCE_ROLES and r["verdict"] == "missing" for r in results):
        problems += check_source_tree(input_dir, entries)

    if problems:
        # Rejected: never leave a "staged" state behind; no staging writes, evidence untouched.
        if marker.exists():
            marker.unlink()
        print(f"REJECTED: {len(problems)} problem(s); nothing staged", file=err)
        for problem in problems:
            print(f"  - {problem}", file=err)
        summary = {"status": "rejected", "problems": len(problems),
                   "verdicts": {r["path"]: r["verdict"] for r in results}}
        print(json.dumps(summary, indent=2), file=out)
        return 1

    if marker.exists():
        marker.unlink()
    for entry, result in zip(entries, results):
        result["stage_method"] = stage_file(input_dir / entry["rel"], stage_dir / entry["rel"])
    evidence = build_evidence(manifest, manifest_path, input_dir, stage_dir, results)
    write_json(evidence_path, evidence)
    write_json(marker, evidence)
    print(json.dumps({
        "status": evidence["status"],
        "files": len(results),
        "provenance_authenticated": False,
        "evidence": relative_for_report(evidence_path),
        "stage_dir": relative_for_report(stage_dir),
    }, indent=2), file=out)
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--input-dir", type=Path, default=DEFAULT_INPUT_DIR)
    parser.add_argument("--stage-dir", type=Path, default=DEFAULT_STAGE_DIR)
    parser.add_argument("--evidence", type=Path, default=DEFAULT_EVIDENCE)
    args = parser.parse_args(argv)
    return run(args.manifest, args.input_dir, args.stage_dir, args.evidence)


if __name__ == "__main__":
    sys.exit(main())
