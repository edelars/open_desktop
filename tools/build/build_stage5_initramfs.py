#!/usr/bin/env python3
"""Build the pinned Stage 5 x86_64 kernel and deterministic NVMe initramfs."""
from __future__ import annotations

import argparse
import hashlib
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

from tools.build.build_stage5_rootfs import (
    BUILDER_IMAGE, BUILDER_PACKAGE_CACHE, PACKAGE_CACHE, ROOT, WORK, BuildError,
    ROOT_DEVICE, load_builder_lock, load_kernel_lock, load_lock, prepare_inputs
)

KERNEL_VERSION = "7.0.0-38-generic"

CONTAINER_BUILD = r"""
set -euxo pipefail
test "$(uname -m)" = aarch64
mkdir -p /tmp/toolroot /tmp/modroot /tmp/kernel /tmp/initrd /tmp/busybox
while IFS= read -r filename; do
  test -f "/builder-packages/$filename"
  dpkg-deb -x "/builder-packages/$filename" /tmp/toolroot
done < /builder-packages.txt
export LD_LIBRARY_PATH=/tmp/toolroot/usr/lib/aarch64-linux-gnu:/tmp/toolroot/lib/aarch64-linux-gnu
CPIO=/tmp/toolroot/usr/bin/cpio
DEPMOD=/tmp/toolroot/usr/sbin/depmod
MODPROBE=/tmp/toolroot/usr/sbin/modprobe
ZSTD=/tmp/toolroot/usr/bin/zstd
test "$("$CPIO" --version | head -1)" = 'cpio (GNU cpio) 2.15'
while IFS= read -r filename; do
  case "$filename" in
    linux-image-*.deb) dpkg-deb -x "/packages/$filename" /tmp/kernel ;;
    linux-modules-*.deb) dpkg-deb -x "/packages/$filename" /tmp/modroot ;;
    *) echo "unexpected kernel package: $filename" >&2; exit 30 ;;
  esac
done < /kernel-packages.txt
KVER=7.0.0-38-generic
test -s "/tmp/kernel/boot/vmlinuz-$KVER"
test -s "/tmp/modroot/boot/config-$KVER"
grep -Fx 'CONFIG_BLK_DEV_NVME=m' "/tmp/modroot/boot/config-$KVER"
grep -Fx 'CONFIG_EXT4_FS=y' "/tmp/modroot/boot/config-$KVER"
if [ ! -d /tmp/modroot/lib/modules ] && [ -d /tmp/modroot/usr/lib/modules ]; then ln -s usr/lib /tmp/modroot/lib; fi
test -d "/tmp/modroot/lib/modules/$KVER"
if ! "$DEPMOD" -a -b /tmp/modroot "$KVER" >/tmp/depmod.log 2>&1; then cat /tmp/depmod.log; exit 32; fi
grep -E '(^|/)ext4\.ko$' "/tmp/modroot/lib/modules/$KVER/modules.builtin"
"$MODPROBE" --show-depends --dirname /tmp/modroot --set-version "$KVER" ext4 | grep -Fx 'builtin ext4'
"$MODPROBE" --show-depends --dirname /tmp/modroot --set-version "$KVER" nvme > /tmp/nvme-deps.txt
awk '$1 == "insmod" {print $2}' /tmp/nvme-deps.txt > /tmp/module-paths.txt
test "$(wc -l < /tmp/module-paths.txt | tr -d ' ')" = 5
while IFS= read -r source; do
  case "$source" in /tmp/modroot/*) relative="$(printf '%s' "$source" | sed 's#^/tmp/modroot##')" ;; /*) relative="$source" ;; *) exit 31 ;; esac
  test -f "/tmp/modroot$relative"
  loadpath=$(printf '%s' "$relative" | sed 's/\.zst$//')
  destination="/tmp/initrd$loadpath"
  mkdir -p "$(dirname "$destination")"
  "$ZSTD" -d -q -f "/tmp/modroot$relative" -o "$destination"
  printf '%s\n' "$loadpath" >> /tmp/module-load-order.txt
done < /tmp/module-paths.txt
busybox_filename=$(cat /busybox-package.txt)
test -n "$busybox_filename"
busybox_deb="/packages/$busybox_filename"
test -f "$busybox_deb"
dpkg-deb -x "$busybox_deb" /tmp/busybox
busybox=$(find /tmp/busybox -type f -name busybox -print -quit)
test -n "$busybox"
install -D -m 0755 "$busybox" /tmp/initrd/bin/busybox
for applet in sh mount mkdir sleep switch_root insmod; do ln -s busybox "/tmp/initrd/bin/$applet"; done
cat > /tmp/initrd/init <<'INIT'
#!/bin/sh
PATH=/bin:/sbin
export PATH
fail() { console=/dev/console; [ -e /newroot/dev/console ] && console=/newroot/dev/console; echo "stage5-initramfs: $*" >"$console"; exec /bin/busybox sh; }
mkdir -p /dev /proc /sys /newroot
mount -t devtmpfs devtmpfs /dev || fail 'devtmpfs mount failed'
mount -t proc proc /proc || fail 'proc mount failed'
mount -t sysfs sysfs /sys || fail 'sysfs mount failed'
INIT
while IFS= read -r module; do
  name=$(basename "$module" | sed 's/\.ko$//')
  printf '/bin/busybox insmod %s || fail "module load failed: %s"\n' "$module" "$name" >> /tmp/initrd/init
done < /tmp/module-load-order.txt
cat >> /tmp/initrd/init <<'INIT'
i=0
while [ ! -b @@ROOT_DEVICE@@ ] && [ "$i" -lt 60 ]; do sleep 1; i=$((i + 1)); done
[ -b @@ROOT_DEVICE@@ ] || fail 'NVMe namespace absent'
mount -t ext4 @@ROOT_DEVICE@@ /newroot || fail 'root ext4 mount failed'
mkdir -p /newroot/dev /newroot/proc /newroot/sys
/bin/busybox mount -o move /proc /newroot/proc || fail 'proc mount handoff failed'
/bin/busybox mount -o move /sys /newroot/sys || fail 'sysfs mount handoff failed'
/bin/busybox mount -o move /dev /newroot/dev || fail 'devtmpfs handoff failed'
[ -c /newroot/dev/console ] || fail 'new-root console absent'
exec </newroot/dev/console >/newroot/dev/console 2>&1
exec /bin/busybox switch_root /newroot /bin/sh
INIT
chmod 0755 /tmp/initrd/init
find /tmp/initrd -mindepth 1 -exec touch -h -d '@1' {} +
touch -h -d '@1' /tmp/initrd
cd /tmp/initrd
find . -print0 | LC_ALL=C sort -z | "$CPIO" --null -o -H newc --owner=0:0 --reproducible > /tmp/initramfs.cpio
gzip -n -9 < /tmp/initramfs.cpio > /initramfs.cpio.gz
cp "/tmp/kernel/boot/vmlinuz-$KVER" /vmlinuz
gzip -dc /initramfs.cpio.gz | "$CPIO" -it --quiet > /tmp/initramfs.list
grep -E '^(.*/)?init$' /tmp/initramfs.list
grep -F 'mount -t ext4 @@ROOT_DEVICE@@ /newroot' /tmp/initrd/init
grep -F 'mount -o move /proc /newroot/proc' /tmp/initrd/init
grep -F 'mount -o move /sys /newroot/sys' /tmp/initrd/init
grep -F 'mount -o move /dev /newroot/dev' /tmp/initrd/init
grep -F 'exec </newroot/dev/console >/newroot/dev/console 2>&1' /tmp/initrd/init
for module in hkdf nvme-auth nvme-keyring nvme-core nvme; do grep -E "^(.*/)?lib/modules/$KVER/.*/$module\.ko$" /tmp/initramfs.list; done
test "$(grep -Ec '^(.*/)?lib/modules/.*\.ko$' /tmp/initramfs.list)" = 5
! grep -E '/Users/|/home/|\.aws|\.ssh|secret|credential' /tmp/initramfs.list
cat /tmp/initramfs.list
"$DEPMOD" --version 2>&1 | grep -F 'kmod version 34.2'
"$ZSTD" --version | grep -F '1.5.7'
"$CPIO" --version | grep -F '2.15'
""".replace("@@ROOT_DEVICE@@", ROOT_DEVICE)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def build(initramfs_output: Path, kernel_output: Path) -> tuple[str, str]:
    paths = [path.resolve() for path in (initramfs_output, kernel_output)]
    for path in paths:
        try:
            path.relative_to(WORK.resolve())
        except ValueError as error:
            raise BuildError("outputs must be under ignored work/stage5") from error
        path.parent.mkdir(parents=True, exist_ok=True)
        if path.exists():
            raise BuildError(f"refusing to replace existing output: {path}")
    prepare_inputs()
    docker = shutil.which("docker")
    if not docker:
        raise BuildError("Docker CLI is required")
    builder_packages = load_builder_lock()
    kernel_packages = load_kernel_lock()
    busybox_packages = [package for package in load_lock() if package["package"] == "busybox-static"]
    if len(busybox_packages) != 1:
        raise BuildError("rootfs lock must contain exactly one busybox-static package")
    input_lists = [
        paths[0].with_name(paths[0].name + ".builder-packages.txt"),
        paths[0].with_name(paths[0].name + ".kernel-packages.txt"),
        paths[0].with_name(paths[0].name + ".busybox-package.txt"),
    ]
    if any(path.exists() for path in input_lists):
        raise BuildError("refusing to replace existing initramfs input manifest")
    input_lists[0].write_text("".join(f"{package['filename']}\n" for package in builder_packages))
    input_lists[1].write_text("".join(f"{package['filename']}\n" for package in kernel_packages))
    input_lists[2].write_text(busybox_packages[0]["filename"] + "\n")
    for path in paths:
        path.open("xb").close()
    mounts = [
        "--mount", f"type=bind,src={PACKAGE_CACHE.resolve()},dst=/packages,readonly",
        "--mount", f"type=bind,src={BUILDER_PACKAGE_CACHE.resolve()},dst=/builder-packages,readonly",
        "--mount", f"type=bind,src={input_lists[0].resolve()},dst=/builder-packages.txt,readonly",
        "--mount", f"type=bind,src={input_lists[1].resolve()},dst=/kernel-packages.txt,readonly",
        "--mount", f"type=bind,src={input_lists[2].resolve()},dst=/busybox-package.txt,readonly",
        "--mount", f"type=bind,src={paths[0]},dst=/initramfs.cpio.gz",
        "--mount", f"type=bind,src={paths[1]},dst=/vmlinuz",
    ]
    try:
        result = subprocess.run(
            [docker, "run", "--rm", "--pull=missing", "--platform", "linux/arm64", *mounts,
             BUILDER_IMAGE, "bash", "-ec", CONTAINER_BUILD],
            cwd=ROOT, check=True, capture_output=True, text=True,
        )
        print(result.stdout, end="")
    except (subprocess.CalledProcessError, OSError) as error:
        for path in paths:
            path.unlink(missing_ok=True)
        output = (error.stderr + error.stdout) if isinstance(error, subprocess.CalledProcessError) else str(error)
        raise BuildError(f"initramfs container build failed: {output.strip()}") from error
    finally:
        for path in input_lists:
            path.unlink(missing_ok=True)
    return sha256(paths[0]), sha256(paths[1])


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--initramfs-output", type=Path, default=WORK / "initramfs/stage5-initramfs.cpio.gz")
    parser.add_argument("--kernel-output", type=Path, default=WORK / f"initramfs/vmlinuz-{KERNEL_VERSION}")
    args = parser.parse_args(argv)
    try:
        initrd_sha, kernel_sha = build(args.initramfs_output, args.kernel_output)
        print(f"initramfs={args.initramfs_output.resolve()}\ninitramfs_sha256={initrd_sha}")
        print(f"kernel={args.kernel_output.resolve()}\nkernel_sha256={kernel_sha}")
        return 0
    except (BuildError, OSError) as error:
        print(f"build_error={error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
