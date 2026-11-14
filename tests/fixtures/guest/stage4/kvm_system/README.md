# Stage 4 independent KVM system fixtures

These are small project-authored freestanding x86-64 images run as CPL0 guests
by `tools/kvm-runner/`. KVM executes guest instructions on the Intel host CPU;
the Open Desktop interpreter, JIT, and MMU do not participate. Evidence must
label externally injected vectors as `kvm-injected`, and must not claim that
KVM's virtual APIC, IOAPIC, PIT, or timer matches physical device topology.

The runner enters each raw image at GPA/RIP `0x1000` with the existing identity
mapping and initial GDT. Fixtures may build their own IDT, TSS, and page tables
inside the 2 MiB guest RAM. Keep the image at or below 4 KiB.

## Result block

Each image writes raw observations to GPA `0x18000`; the runner copies at most
512 bytes after a successful terminal HLT. The block is little-endian:

| Offset | Type | Meaning |
|---:|---|---|
| 0 | u64 | Magic `0x314d564b57483453` (`S4HWKVM1` in memory) |
| 8 | u32 | Format version, currently `1` |
| 12 | u32 | Case ID assigned by the builder |
| 16 | u32 | Completion marker `0x434f4d50` (little-endian bytes `PMOC`) |
| 20 | u32 | Number of valid u64 observation words |
| 24 | u32 | Number of guest fault/interrupt events recorded |
| 28 | u32 | Reserved, must be zero |
| 32 | u64[] | Case-specific raw observations, documented beside each case |

The completion marker only shows that guest code reached its final checkpoint.
It is not a pass verdict. The host-side builder validates the case ID and
compares architecturally defined fields independently. Record implementation-
defined observations, including A bits on fault paths, without treating them as
fixed architectural expectations.

## Case matrix

All payload words are little-endian u64 values. Word indices below are relative
to payload offset 32; exact offsets and expectations are encoded in
`tools/build/build_stage4_kvm_system.py` and retained verbatim in evidence.

| ID | Image | Payload summary | Events |
|---:|---|---|---:|
| 101 | `paging.S` read4k | value, PML4E/PDPTE/PDE/PTE, physical sentinel, CR3 | 0 |
| 102 | `paging.S` write4k | value, four paging entries, physical value, CR3 | 0 |
| 103 | `paging.S` not-present | PFEC, CR2, fault RIP, four paging entries, backing value | 1 |
| 104 | `paging.S` WP enabled | PFEC, CR2, fault RIP, four entries, backing value, CR0 | 1 |
| 105 | `paging.S` WP disabled | written value, four entries, CR0, fault count | 0 |
| 106 | `paging.S` NX fetch | NX capability/CPUID, PFEC, CR2, fault RIP, four entries, EFER | 1 |
| 107 | `paging.S` CR3 switch | values through roots A/B, active CR3, leaf entries, roots | 0 |
| 108 | `paging.S` INVLPG remap | old/new values, PTE before/after, backing values | 0 |
| 201 | `control.S` | CR0 transitions, CR3 readbacks, CR4/PGE transitions, CPUID support | 0 |
| 0x21 | `descriptors.S` tables | GDTR/IDTR, TR, GDT entries after LTR, all TSS qwords | 0 |
| 0x22 | `descriptors.S` same-CPL INT | pre-state, saved RIP/CS/RFLAGS/RSP/SS, handler flags, post-state | 1 |
| 0x23 | `privilege.S` CPL3 roundtrip | saved user frame selectors/flags/stack, second INT, terminal marker | 2 |
| 0x24 | `privilege.S` CPL3 #GP | CR0, error code, fault/return frames, handler CR0, marker | 2 |
| 0x31 | `interrupt.S` injected vector | vector, saved frame, handler flags/marker, post-IRET resume | 1 |

Case 0x31 queues vector 0x41 with userspace `KVM_INTERRUPT` after the first
guest HLT. It is evidence for this KVM delivery path, not for physical APIC,
IOAPIC, PIT, or timer behavior.

Generated images and objects stay in ignored `work/`; only source, build
recipes, tests, and bounded result evidence belong in Git.
