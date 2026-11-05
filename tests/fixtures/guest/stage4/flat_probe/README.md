# Stage 4 flat probe fixture

This fixture is a project-owned freestanding x86-64 assembly input. The build
script assembles `entry.S`, links it with `layout.ld` into an ELF64 intermediate,
and extracts a fixed-size raw image using the installed Rust toolchain's
`ld.lld` and `rust-objcopy`. It does not synthesize instruction bytes.

The raw image is `0x9000` bytes and is loaded at GPA `0x1000`:

| Raw offset | Guest physical range | Contents |
|---:|---:|---|
| `0x0000` | `0x1000..0x6fff` | PML4, PDPT, PD, and three 4 KiB page tables |
| `0x6000` | `0x7000..0x7007` | five guest instruction bytes at GVA `0x400000`, followed by three zero padding bytes |
| `0x6008..0x6fff` | `0x7008..0x7fff` | zero padding in the mapped code page |
| `0x7000..0x7fff` | `0x8000..0x8fff` | zero padding |
| `0x8000` | `0x9000..0x9fff` | payload qword at GVA `0x800000` |

The page tables map code read/execute, data read/write/NX, and a separate
read/write/NX stack at GVA `0xa00000` to GPA `0xa000`. The stack page is outside
the raw image; the loader must allocate and zero it. Entry state uses `RIP =
0x400000`, `RBX = 0x800000`, `RSP = 0xa01000`, and `DX = 0x3f8`. The assembly
executes `MOV RAX,[RBX]`, `OUT DX,AL`, then `HLT`.

Build into the ignored `work/stage4/` tree:

```sh
python3 tools/build/build_stage4_flat_probe.py --payload 0x1122334455667788
```

By default output is `work/stage4/flat_probe/`. A different output directory
may be selected with `--output-dir`, but it must remain under `work/stage4/` and
be absent or empty. The script discovers `clang`, `rustc`, `ld.lld`, and
`rust-objcopy`; `STAGE4_CLANG`, `STAGE4_LD_LLD`, and `STAGE4_RUST_OBJCOPY` can
select explicit executable paths. On macOS, it finds the Rust toolchain's
`libLLVM.dylib` and explicitly sets `DYLD_LIBRARY_PATH` for LLD/objcopy.

The output directory contains `flat_probe.elf`, `flat_probe.bin`, and
`build_manifest.json`. The manifest records source hashes, exact tool paths and
versions, commands, payload, fixed layout, and ELF/raw SHA-256 hashes. The raw
hash is expected to match across clean builds with the same sources, tools, and
payload. All generated outputs stay ignored under `work/stage4/`.
