# S6-02 physical x86 disassembly repeat

Status: the physical-host byte decode passed comparison with the existing
staged-image inventory. This evidence does not accept the full S6-02 task.

## Inputs and host

- Host: Intel Core i5-3210M @ 2.50 GHz, Linux `7.0.0-42-generic`, `x86_64`.
- Tool: GNU Binutils `x86_64-linux-gnu-objdump` 2.46.
- Trampoline input: ignored local artifact
  `work/stage6/s6-02-disassembly/real_mode_blob-from-vmlinuz.bin`, 25,188 bytes,
  SHA-256 `d48c472d9051f65d62efdb34bdabdb3fb261aefbb7873a3637860ecc4c80573f`.
  It was extracted from the staged Linux 7.0.0-38.38 image and transferred
  to the Mini only after its checksum matched this value. The kernel bytes are
  not stored in Git, as required by the S6-02 plan.
- Common-entry input: ignored local artifact
  `work/stage6/s6-02-disassembly/common64-entry.bin`, 8 bytes,
  SHA-256 `8b2bf191b2ccd5e35785621387dd8354b3158cbdc2d0d04aded99f34c2a461e7`.
  These are the first bytes at `secondary_startup_64` (`0xffffffff8139c090`)
  from the staged `vmlinux-debug` ELF (SHA-256
  `7e460f048c3de0f8ee85fd326f2f65d95a6ee448e57b6f5333e2be79b5be9d1f`).
  The bytes decode as `call verify_cpu` followed by the aliased
  `secondary_startup_64_no_verify` entry `xor r15d,r15d` at `0xffffffff8139c095`.

## Commands and raw output

All commands ran on the physical x86 host. The three trampoline commands were:

```sh
x86_64-linux-gnu-objdump -D -b binary -m i386 -M intel,addr16,data16 --insn-width=16 --start-address=0x1000 --stop-address=0x11a9 real_mode_blob.bin
x86_64-linux-gnu-objdump -D -b binary -m i386 -M intel,addr32,data32 --insn-width=16 --start-address=0x2000 --stop-address=0x206c real_mode_blob.bin
x86_64-linux-gnu-objdump -D -b binary -m i386:x86-64 -M intel --insn-width=16 --start-address=0x20f0 --stop-address=0x20f6 real_mode_blob.bin
```

The common 64-bit entry bytes were disassembled separately so each output has
the corresponding linked address. The first command uses the first five bytes
of `common64-entry.bin` and VMA `0xffffffff8139c090`; the second used
`tail -c +6 common64-entry.bin > common64-no-verify.bin` and VMA
`0xffffffff8139c095`:

```sh
x86_64-linux-gnu-objdump -D -b binary -m i386:x86-64 -M intel --insn-width=16 --adjust-vma=0xffffffff8139c090 --start-address=0xffffffff8139c090 --stop-address=0xffffffff8139c095 common64-entry.bin
x86_64-linux-gnu-objdump -D -b binary -m i386:x86-64 -M intel --insn-width=16 --adjust-vma=0xffffffff8139c095 common64-no-verify.bin
```

Raw stdout is preserved in this directory:

| Output | SHA-256 | Bytes |
|---|---|---:|
| [`16bit-objdump.txt`](s6-02-x86/16bit-objdump.txt) | `ecf7df9ee7f039ba8a4dace1517520a62135aacbc1e573cd6b039c3e86355ba1` | 9,000 |
| [`32bit-objdump.txt`](s6-02-x86/32bit-objdump.txt) | `ed4fbac2eadeed6fbca3f04190995a516b2050db9c2d271afafc28d22cc617b4` | 2,308 |
| [`64bit-objdump.txt`](s6-02-x86/64bit-objdump.txt) | `018dacb8587c74ab00d5f79f3d58c272b7bbd5f914d373c18c5227de3228d0c3` | 215 |
| [`common-verify-objdump.txt`](s6-02-x86/common-verify-objdump.txt) | `820e67b4eeb9854596af5b35a4b15dd91b26227d2f97843159a4970a5f2270f8` | 196 |
| [`common-no-verify-objdump.txt`](s6-02-x86/common-no-verify-objdump.txt) | `d9821e72ad14cf29d54cefaa5ab2809562f701f7a118e462c851f3b77c4c8861` | 191 |
| [`host-info.txt`](s6-02-x86/host-info.txt) | `ad48d05b522e12944f7031d0c8962327133bd58b71059b4591249504a7112bff` | 297 |

## Comparison

The 16-bit window decoded 122 instructions. Three additional instructions
consume the nonzero alignment bytes in `[0x1074, 0x1080)`, which the inventory
does not classify as trampoline path rows. After excluding this interval, all
119 recorded 16-bit rows match by offset and bytes. All 29 recorded protected
32-bit rows and the one long-mode trampoline row also match exactly. The two
common-entry rows match both linked address and bytes. There are no missing,
mismatched, or unexplained rows in the compared inventory.

The repeat verifies byte decoding on the physical x86 toolchain. It does not
verify which AP callback or APIC mode the final guest selects at runtime, the
guest CPU profile, or AP execution. S6-02 remains in progress pending its
independent evidence review, architecture acceptance, and resolution of the
runtime Linux path. Windows CPU-06 remains an unverified hypothesis reserved
for the Stage 8 trace, as allowed by the S6-02 plan.
