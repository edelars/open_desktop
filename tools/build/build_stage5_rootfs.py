#!/usr/bin/env python3
"""Build the reproducible Stage 5 Ubuntu amd64 ext4 root disk."""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORK = ROOT / "work/stage5"
BASE_URL = "https://cdimage.ubuntu.com/ubuntu-base/releases/26.04/release/ubuntu-base-26.04.1-base-amd64.tar.gz"
BASE_SHA256 = "a496a960472ce474a59590b8987d3a1135d3cbef1991f3b1abe8cacfea8bf85a"
BASE_ARCHIVE = WORK / "sources/ubuntu-base-26.04.1/ubuntu-base-26.04.1-base-amd64.tar.gz"
PACKAGE_LOCK = ROOT / "tools/build/stage5_rootfs_packages.json"
PACKAGE_CACHE = WORK / "packages"
BUILDER_PACKAGE_LOCK = ROOT / "tools/build/stage5_builder_packages.json"
BUILDER_PACKAGE_CACHE = WORK / "builder-packages"
KERNEL_PACKAGE_LOCK = ROOT / "tools/build/stage5_kernel_packages.json"
BUILDER_IMAGE = "ubuntu@sha256:86eb1bc1059720a19a777ba5a109e9ee3690d5dfe3e319871a6d6beb86fb7f85"
SECTOR_BYTES = 512
IMAGE_BYTES = 2 * 1024 * 1024 * 1024
ROOT_UUID = "5a5e5a05-0000-4000-8000-000000000005"
ROOT_DEVICE = "/dev/nvme0n1"

CONTAINER_BUILD = r"""
set -eu
test "$(uname -m)" = aarch64
mke2fs -V 2>&1 | grep -F 'mke2fs 1.47.2 '
debugfs -V 2>&1 | grep -F 'debugfs 1.47.2 '
mkdir /tmp/rootfs
tar --numeric-owner -xzf /inputs/base.tar.gz -C /tmp/rootfs
while IFS= read -r filename; do dpkg-deb -x "/packages/$filename" /tmp/rootfs; done < /inputs/rootfs-package-files.txt
mkdir -p /tmp/rootfs/usr/share/doc/stage5-rootfs
cp /inputs/package-lock.json /tmp/rootfs/usr/share/doc/stage5-rootfs/packages.json
chroot /tmp/rootfs /usr/bin/dpkg-query -W -f='${Version}\n' libc6 | grep -Fx '2.43-2ubuntu2.3'
mkdir -p /tmp/rootfs/tmp
cat > /tmp/rootfs/tmp/stage5-c-smoke.c <<'C'
#include <stdio.h>
int main(void) { puts("stage5-c-smoke-ok"); return 0; }
C
chroot /tmp/rootfs /usr/bin/gcc-15 -Wall -Wextra -Werror /tmp/stage5-c-smoke.c -o /tmp/stage5-c-smoke
test "$(od -An -t u2 -j 18 -N 2 /tmp/rootfs/tmp/stage5-c-smoke | tr -d ' ')" = 62
chroot /tmp/rootfs /tmp/stage5-c-smoke | grep -Fx 'stage5-c-smoke-ok'
rm /tmp/rootfs/tmp/stage5-c-smoke.c /tmp/rootfs/tmp/stage5-c-smoke
rm -rf /tmp/rootfs/var/cache/apt /tmp/rootfs/var/lib/apt/lists /tmp/rootfs/var/log
mkdir -p /tmp/rootfs/var/cache/apt /tmp/rootfs/var/lib/apt/lists /tmp/rootfs/var/log /tmp/rootfs/tmp /tmp/rootfs/var/tmp
: > /tmp/rootfs/etc/machine-id
find /tmp/rootfs -mindepth 1 -exec touch -h -d '@1' {} +
touch -h -d '@1' /tmp/rootfs
E2FSPROGS_FAKE_TIME=1 mke2fs -F -t ext4 -b 4096 \
  -U 5a5e5a05-0000-4000-8000-000000000005 -L ODSTAGE5 \
  -E hash_seed=5a5e5a05-0000-4000-8000-000000000005,lazy_itable_init=0,lazy_journal_init=0,root_owner=0:0 \
  -O ^metadata_csum_seed -d /tmp/rootfs /image >/tmp/mke2fs.log 2>&1
: > /tmp/time-normalize.cmds
while IFS= read -r -d '' path; do
    guest_path="/${path#/tmp/rootfs/}"
    case "$guest_path" in *'"'*|*'\'*) echo "unsupported debugfs path: $guest_path" >&2; exit 4;; esac
    for field in atime mtime ctime crtime; do
        printf 'set_inode_field "%s" %s @1\n' "$guest_path" "$field" >> /tmp/time-normalize.cmds
    done
    for field in atime_extra mtime_extra ctime_extra crtime_extra; do
        printf 'set_inode_field "%s" %s 0\n' "$guest_path" "$field" >> /tmp/time-normalize.cmds
    done
done < <(find /tmp/rootfs -mindepth 1 -print0)
printf 'set_super_value wtime 1\n' >> /tmp/time-normalize.cmds
E2FSPROGS_FAKE_TIME=1 debugfs -w -f /tmp/time-normalize.cmds /image >/tmp/debugfs-normalize.log 2>&1
test "$(od -An -t u2 -j 18 -N 2 /tmp/rootfs/usr/bin/x86_64-linux-gnu-gcc-15 | tr -d ' ')" = 62
test "$(od -An -t u2 -j 18 -N 2 /tmp/rootfs/usr/bin/busybox | tr -d ' ')" = 62
chroot /tmp/rootfs /usr/bin/gcc-15 --version | grep -F '15.2.0-16ubuntu1'
chroot /tmp/rootfs /usr/bin/busybox --list | grep -Fx modprobe
chroot /tmp/rootfs /usr/bin/tar --version | grep -F 'GNU tar'
chroot /tmp/rootfs /usr/bin/busybox --list | grep -Fx sha256sum
chroot /tmp/rootfs /usr/bin/busybox sha256sum /etc/os-release | awk 'length($1) == 64 { found=1 } END { exit !found }'
if find /tmp/rootfs -print | grep -E '/(Users/[^/]+|home/[^/]+|\.aws|\.ssh|id_rsa|id_ed25519|credentials)(/|$)'; then exit 41; fi
e2fsck -fn /image >/tmp/e2fsck.log 2>&1
grep -F 'Pass 5: Checking group summary information' /tmp/e2fsck.log
"""


class BuildError(RuntimeError):
    pass


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run(command: list[str]) -> str:
    result = subprocess.run(command, cwd=ROOT, check=False, capture_output=True, text=True)
    if result.returncode:
        raise BuildError(f"command failed ({result.returncode}): {' '.join(command)}\n{(result.stderr or result.stdout).strip()}")
    return result.stdout


def download_verified(url: str, destination: Path, expected: str) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    if not destination.exists():
        partial = destination.with_suffix(destination.suffix + ".partial")
        try:
            with urllib.request.urlopen(url, timeout=90) as response, partial.open("wb") as target:
                shutil.copyfileobj(response, target)
            actual = sha256(partial)
            if actual != expected:
                raise BuildError(f"SHA256 mismatch for {destination.name}: {actual}")
            partial.replace(destination)
        finally:
            partial.unlink(missing_ok=True)
    actual = sha256(destination)
    if actual != expected:
        raise BuildError(f"SHA256 mismatch for {destination.name}: {actual}")


def load_lock() -> list[dict[str, str]]:
    try:
        packages = json.loads(PACKAGE_LOCK.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise BuildError(f"cannot read package lock: {error}") from error
    required = {"filename", "package", "sha256", "url", "version"}
    if not isinstance(packages, list) or not packages:
        raise BuildError("package lock must be a non-empty JSON list")
    if any(
        set(package) != required or len(package["sha256"]) != 64
        or Path(package["filename"]).name != package["filename"]
        for package in packages
    ):
        raise BuildError("unexpected package-lock entry")
    return packages


def load_builder_lock() -> list[dict[str, str]]:
    try:
        packages = json.loads(BUILDER_PACKAGE_LOCK.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise BuildError(f"cannot read builder package lock: {error}") from error
    required = {"architecture", "filename", "package", "sha256", "url", "version"}
    if not isinstance(packages, list) or not packages:
        raise BuildError("builder package lock must be a non-empty JSON list")
    if any(
        set(package) != required or package["architecture"] != "arm64"
        or len(package["sha256"]) != 64 or Path(package["filename"]).name != package["filename"]
        for package in packages
    ):
        raise BuildError("unexpected builder package-lock entry")
    return packages


def load_kernel_lock() -> list[dict[str, str]]:
    try:
        packages = json.loads(KERNEL_PACKAGE_LOCK.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise BuildError(f"cannot read kernel package lock: {error}") from error
    required = {"filename", "package", "sha256", "url", "version"}
    if not isinstance(packages, list) or len(packages) != 2:
        raise BuildError("kernel package lock must contain the image and modules packages")
    if any(
        set(package) != required or len(package["sha256"]) != 64
        or Path(package["filename"]).name != package["filename"]
        for package in packages
    ):
        raise BuildError("unexpected kernel package-lock entry")
    if {package["package"] for package in packages} != {
        "linux-image-7.0.0-38-generic", "linux-modules-7.0.0-38-generic"
    }:
        raise BuildError("kernel lock does not match the Stage 5 pinned kernel")
    return packages


def prepare_inputs() -> None:
    download_verified(BASE_URL, BASE_ARCHIVE, BASE_SHA256)
    for package in load_lock():
        download_verified(package["url"], PACKAGE_CACHE / package["filename"], package["sha256"])
    for package in load_builder_lock():
        download_verified(package["url"], BUILDER_PACKAGE_CACHE / package["filename"], package["sha256"])
    for package in load_kernel_lock():
        download_verified(package["url"], PACKAGE_CACHE / package["filename"], package["sha256"])


def build(output: Path) -> str:
    output = output.resolve()
    try:
        output.relative_to(WORK.resolve())
    except ValueError as error:
        raise BuildError("output must be under ignored work/stage5") from error
    output.parent.mkdir(parents=True, exist_ok=True)
    if output.exists():
        raise BuildError(f"refusing to replace existing output: {output}")
    prepare_inputs()
    docker = shutil.which("docker")
    if not docker:
        raise BuildError("Docker CLI is required")
    run([
        "cargo", "run", "--offline", "-p", "od-image-tool", "--bin", "od-image-tool", "--",
        "create-raw", str(output.parent), output.name, str(IMAGE_BYTES),
    ])
    package_list = output.parent / f"{output.name}.package-files.txt"
    package_list.write_text("".join(f"{package['filename']}\n" for package in load_lock()))
    mounts = [
        "--mount", f"type=bind,src={BASE_ARCHIVE.resolve()},dst=/inputs/base.tar.gz,readonly",
        "--mount", f"type=bind,src={PACKAGE_CACHE.resolve()},dst=/packages,readonly",
        "--mount", f"type=bind,src={PACKAGE_LOCK.resolve()},dst=/inputs/package-lock.json,readonly",
        "--mount", f"type=bind,src={package_list.resolve()},dst=/inputs/rootfs-package-files.txt,readonly",
        "--mount", f"type=bind,src={output},dst=/image",
    ]
    try:
        validation = run([docker, "run", "--rm", "--pull=missing", "--platform", "linux/arm64", *mounts,
                          BUILDER_IMAGE, "bash", "-ec", CONTAINER_BUILD])
    except BuildError:
        output.unlink(missing_ok=True)
        raise
    finally:
        package_list.unlink(missing_ok=True)
    print(validation, end="")
    return sha256(output)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=WORK / "rootfs/stage5-rootfs.raw")
    args = parser.parse_args(argv)
    try:
        digest = build(args.output)
        print(f"image={args.output.resolve()}\nsha256={digest}\nbytes={IMAGE_BYTES}\nsector_bytes={SECTOR_BYTES}\nroot={ROOT_DEVICE}\nroot_uuid={ROOT_UUID}")
        return 0
    except (BuildError, OSError) as error:
        print(f"build_error={error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
