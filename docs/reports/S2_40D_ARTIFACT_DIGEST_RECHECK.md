# S2-40d — Ubuntu kernel artifact digest recheck

**Date:** 2026-11-04  
**Scope:** read-only identity check of available artifacts in the main checkout. No VM/image content was changed.

The main checkout contained ten files under
`/Users/senya/projects/open_desktop/work/vms/s2-00-ubuntu-26.04.1/`:

```text
buildinfo-data.tar
buildinfo-data.tar.zst
data.tar
data.tar.zst
kernel-payload.zst
linux-buildinfo-7.0.0-38-generic_7.0.0-38.38_amd64.deb
linux-image-7.0.0-38-generic_7.0.0-38.38_amd64.deb
vmlinux-7.0.0-38-generic
vmlinux-debug-7.0.0-38-generic
vmlinuz-7.0.0-38-generic
```

The following command was run from that directory:

```sh
sha256sum \
  linux-image-7.0.0-38-generic_7.0.0-38.38_amd64.deb \
  vmlinuz-7.0.0-38-generic \
  vmlinux-7.0.0-38-generic \
  vmlinux-debug-7.0.0-38-generic
```

Observed output:

```text
f268d635efaa2e19a18e12813ffdbc2e32db2390cf67d4416abd59292975e7aa  linux-image-7.0.0-38-generic_7.0.0-38.38_amd64.deb
9f55ef7253055a9cf68f8a04323c93dead2fca617386d842fbde9ca7b178b27e  vmlinuz-7.0.0-38-generic
ccaf590465e4e3bc6117efa8ac7cfe87ba0b348188e2608731961527dd05034f  vmlinux-7.0.0-38-generic
7e460f048c3de0f8ee85fd326f2f65d95a6ee448e57b6f5333e2be79b5be9d1f  vmlinux-debug-7.0.0-38-generic
```

All four values match the committed [Linux profile report](STAGE_2_LINUX_PROFILE.md#полученные-данные-и-происхождение) and [versioned map](data/stage2/s2-37-linux-vmlinux-map-v1.json). This independently rechecks those file digests in the main checkout; it does not authenticate their source or signatures.

The following source/signature inputs were absent from that directory: `linux_7.0.0-38.38.dsc`, `linux_7.0.0.orig.tar.gz`, `linux_7.0.0-38.38.diff.gz`, `Sources.xz`, `InRelease`, and `apt-provenance.json`. Therefore this check did not reproduce APT signature/source provenance, build-ID or allocated-section equality, source/DWARF mappings, or the recorded alternative bytes/predicates. It makes no claim about runtime alternative selection, Linux boot, or physical CPU behavior. These gaps remain tracked by [ISSUE-036](../KNOWN_ISSUES.md#issue-036--ubuntu-fpu-source-image-mapping-artifacts-are-unavailable).
