# Stage 5 guest rootfs fixture

This fixture is built from pinned Ubuntu packages. It is a static input for later guest scenarios; the kernel and initramfs have **not** been booted by this task.

## Reproduce

Run from the repository root with Python 3.15.2, Docker 29.8.2, a working local Docker daemon, and the pinned native ARM64 builder image:

```sh
python3 tools/build/build_stage5_rootfs.py --output work/stage5/rootfs/stage5-rootfs.raw
python3 tools/build/build_stage5_initramfs.py \
  --initramfs-output work/stage5/initramfs/stage5-initramfs.cpio.gz \
  --kernel-output work/stage5/initramfs/vmlinuz-7.0.0-38-generic
python3 tools/build/test_build_stage5_rootfs.py
python3 tools/build/test_build_stage5_initramfs.py
```

Outputs are only written under ignored `work/stage5/`. The tools refuse to replace existing paths. The RAW file is allocated through the accepted `od-image-tool create-raw` command; no third RAW writer is used.

## Root disk

- Filesystem: ext4, created directly in the RAW file (no partition table).
- Logical sector: 512 bytes; RAW size: 2,147,483,648 bytes (2 GiB).
- Root device: `/dev/nvme0n1`; intended kernel command line includes `root=/dev/nvme0n1 rootfstype=ext4 rw rdinit=/init`.
- UUID and ext4 directory hash seed: `5a5e5a05-0000-4000-8000-000000000005`; label: `ODSTAGE5`.
- e2fsprogs `mke2fs`, `debugfs`, and `e2fsck` are version 1.47.2 from the pinned builder image. The recipe disables lazy inode/journal initialization and the random metadata checksum seed, fixes filesystem creation/wtime and all inode times, runs `e2fsck -fn`, and checks its final pass.
- The input is official Ubuntu Base 26.04.1 amd64, `ubuntu-base-26.04.1-base-amd64.tar.gz`, SHA-256 `a496a960472ce474a59590b8987d3a1135d3cbef1991f3b1abe8cacfea8bf85a`. The hash was checked against the archive's signed `SHA256SUMS` manifest; the retained manifest SHA-256 is `c70149f57ea126a96a21d4093b9c3fcff94ed2b9382508b8d888a32295b263f2`, and its detached signature SHA-256 is `5d6daf115ee862e06674278d3207d39b82c23f1e8fa5a141349d80507dce87e0`.
- The exact 36 amd64 package inputs, versions, URLs, and SHA-256 values are in [stage5_rootfs_packages.json](../../../../tools/build/stage5_rootfs_packages.json). The base compiler/runtime packages come from Ubuntu snapshot `20261111T000000Z`; signed InRelease SHA-256: `45f95ce276cdba3e41870516a130e03c58b8b7a79e9546b0efe9e526d255740c`; amd64 Packages.xz SHA-256: `ed9ac41cb263efaecc5a06a0742dc56570e2fb5ff94c4f2e4b2fbdcddd73cf24`. The base archive already contains `libc6=2.43-2ubuntu2.3`; the lock pins matching `libc6-dev` and `libc-dev-bin` at the same version (SHA-256 `f48da8b7bae5c05106034d54bf6c8ea2a54654f52e11bd75dc2b991d2ef24a49` and `c3a76666e9831f02dcd594a8edb0105126a8e780889fb9659ab0610525844a4a`). The signed `20260901T000000Z` updates index is pinned by InRelease SHA-256 `4ff1ffc7960c43b26588ec8fb6dadfaea08f6f1a7348e59266050e4856f70f56` and Packages.xz SHA-256 `7c38e3657b9150664eb58886209f0612662b37940a81c12f5f5232d2f9f66cbd`. `linux-libc-dev=7.0.0-38.38` matches the kernel package version and comes from signed `20261010T000000Z` updates (InRelease SHA-256 `bfebac8843b4a69e7cb1b59dda0623369ee944c80216ea477bbe2f0a8ac4c63e`; Packages.xz SHA-256 `c14a70407207502beef7b26fa34718e82c220e298ad58cee587536d790b0530d`; package SHA-256 `0498bc9510149ef06abd6c09b8932e43c5bc71b68634b4b8f84488b2e0a20e82`). `rpcsvc-proto` is pinned from the signed `20261111T000000Z` main index. The snapshot signatures were verified with `gpgv` against the Ubuntu archive keyring (fingerprint `F6ECB3762474EDA9D21B7022871920D1991BC93C`). The recipe does not overwrite or downgrade the base runtime libc.
- The rootfs recipe compiles, links, and executes a small amd64 C program inside the target chroot; the paired test output includes `stage5-c-smoke-ok`. This verifies the development package closure beyond checking the compiler version.
- License/source records for the added development closure are taken from the exact locked `.deb` files' `usr/share/doc/<package>/copyright`: `libc6-dev` and `libc-dev-bin` identify GNU C Library source (`https://sourceware.org/git/glibc.git`, LGPL-2.1+ for the package's main files); `linux-libc-dev` identifies Linux kernel headers and GPL-2; `rpcsvc-proto` identifies `https://github.com/thkukuk/rpcsvc-proto` and BSD-3-Clause. Those copyright records remain installed in the guest rootfs. The lock entries link each package to its signed-snapshot URL and content SHA-256.
- Key guest tools include GNU tar 1.35, static BusyBox 1.37.0-7ubuntu1 (its `sha256sum` applet is run against `/etc/os-release` during the build), and x86_64 GCC 15.2.0-16ubuntu1. BusyBox is GPL-2.0-only; GCC is GPL-3+ with the GCC Runtime Library Exception. The package lock is the source of the full package versions, URLs, and content hashes; package copyright/license notices remain in the guest filesystem. The build host computes artifact hashes with Python 3.15.2 `hashlib` (PSF license).

## Kernel and initramfs

- Builder: Ubuntu ARM64 image pinned to `ubuntu@sha256:86eb1bc1059720a19a777ba5a109e9ee3690d5dfe3e319871a6d6beb86fb7f85`; local image ID observed as `sha256:490aa88f39f72f944cc111695f97fa6f78f4f3cc047a333f9448e6f03d4089b1`, platform `linux/arm64`. No Docker daemon is needed inside the image.
- Native builder tools: GNU cpio 2.15 (GPL-3+), kmod/depmod 34.2 (kmod tools GPL-2+; libkmod LGPL-2.1+), zstd 1.5.7 (CLI GPL-2+; library BSD-3-Clause/GPL-2+), GNU tar 1.35 (GPL-3+), e2fsprogs 1.47.2 (upstream GPL-2.0), GNU gzip 1.14 (GPL-3+), uutils sort 0.10.0 (MIT/Apache-2.0), Bash 5.3.9 (GPL-3+), and dpkg-deb 1.23.7 (GPL-2+). These system packages are fixed by the Ubuntu builder image digest; the exact additional ARM64 package inputs are hash-pinned in [stage5_builder_packages.json](../../../../tools/build/stage5_builder_packages.json), using signed ARM64 Packages.xz index SHA-256 `537715817d9ed77cc919f5123edb91a48d9ae97abf05588a8f18208cc7227e08`. Package copyright notices are in the pinned image. Python uses only its standard library (PSF license); SHA-256 is calculated with `hashlib`.
- Kernel source is the Ubuntu `linux-image-7.0.0-38-generic` and `linux-modules-7.0.0-38-generic` packages, version 7.0.0-38.38, pinned with SHA-256 values and snapshot URLs in [stage5_kernel_packages.json](../../../../tools/build/stage5_kernel_packages.json). The module package config says `CONFIG_BLK_DEV_NVME=m` and `CONFIG_EXT4_FS=y`; `modules.builtin` and `modprobe --show-depends ext4` confirm ext4 is built in.
- The initramfs is a deterministic `newc` archive compressed with `gzip -n -9`. Its module closure is derived from pinned `depmod`/ `modprobe --show-depends nvme`, then exactly five x86_64 modules are decompressed at build time: `hkdf`, `nvme-auth`, `nvme-keyring`, `nvme-core`, and `nvme`. The init script loads them in dependency order, waits for `/dev/nvme0n1`, mounts the raw ext4 filesystem, moves `/proc`, `/sys`, and `/dev` into the new root, redirects the console, and switches root to `/bin/sh`. The authoritative kernel root argument is `root=/dev/nvme0n1`; the ext4 UUID is metadata only. Ext4 itself is not included as a module.
- The initramfs recipe consumes only exact filenames from the package locks after SHA-256 verification. Extra or stale `.deb` files in either cache are ignored; each paired test injects an invalid unlocked cache file between fresh builds to verify this behavior.
- The ARM64 image is a build host only. Guest kernel, BusyBox, and GCC binaries are x86_64; the rootfs test checks GCC and BusyBox ELF machine value 62.

## Verified hashes

The rootfs and initramfs have separate paired tests. Each builds two fresh artifacts, inserts an invalid unlocked `.deb` in the relevant cache between builds, and verifies reproducibility and lock-only cache consumption. Both copies matched:

| Artifact | SHA-256 | Size |
|---|---|---:|
| ext4 root disk | `c2b2c94665ea256c6fa19c7eeebfc8451589028df3da00b2e1e05c7473dd2895` | 2,147,483,648 bytes |
| initramfs gzip/newc | `17621413fae2456b911fa99d2949daeca8cc13801c5d5a4951b85030f8d8f4b1` | 1,356,016 bytes |
| Linux kernel image | `9f55ef7253055a9cf68f8a04323c93dead2fca617386d842fbde9ca7b178b27e` | 17,320,328 bytes |

Both paired tests passed on 2026-11-11: rootfs `Ran 1 test in 16.542s; OK`; initramfs `Ran 1 test in 4.626s; OK`. The rootfs container validated the C compile/link/run, cpio/kernel tooling versions, secret/path exclusion, and `e2fsck -fn` on both filesystem builds. The initramfs container validated the cpio listing, mount/console handoff commands, absence of host-path/secret names, exact NVMe module closure, built-in ext4, tool versions, and kernel config. Full command outputs are retained in [rootfs paired evidence](../../../../docs/reports/data/stage5/s5-05-rootfs-pair-review.log) and [initramfs paired evidence](../../../../docs/reports/data/stage5/s5-05-initramfs-pair-review.log).

## Provenance and limits

This Stage 5 image is reconstructed from a signed Ubuntu archive manifest and hash-pinned package files. That does not repair the separate S4-35 limitation: the older S4-35 staged kernel was accepted with content hashes but without a confirmed signed APT/source chain. This S5-05 builder independently pins the kernel package inputs, but it does not retroactively certify S4-35.

No Linux boot or guest disk scenario is claimed here. PCI binding and interrupt routing remain Stage 7/4 gates; S5-01 remains blocked on measured boot/cycle/soak inputs. The rootfs and initramfs are artifacts for later extraction/guest scenarios, not evidence that Linux discovers the NVMe controller.
