# S5-21 ranked static residual handoff

This ranking is derived from the compact static inventories. It measures observed instruction-row coverage opportunity only; it does not estimate runtime frequency, speedup, implementation cost, or support status.

Rootfs compact SHA-256: `5b688b74a78417bab072e2e4c3f749cb187feb7aa6fc259f3ae07edb118079f2`  
E-inclusive compact SHA-256: `40dae558902d1713b6aef259cf99d7646957c13a1c7e8ca3e38c4012a727c239`

| Population | Rows | Normalized forms | Exact keys |
|---|---:|---:|---:|
| Rootfs strict residual | 97808 | 157 | 1973 |
| E-inclusive strict residual | 97809 | 157 | 1973 |
| E delta | 1 | 0 | 0 |

A normalized form is included only when its aggregate is entirely unclassified and has empty conditional-candidate, local-candidate, evidence-scope, and profile-match maps; every exact encoding under it must also satisfy those conditions. A partial exact-profile match excludes the whole normalized form from this residual ranking.

All rows below remain unclassified. The ranking is a Stage 2/4 handoff, not a claim that the instruction is unsupported.

## Scenario C

Binaries: bash, dash, tar. Rootfs residual: 2213 rows, 61 forms, 322 exact keys. E delta: 0 rows.

### Top normalized forms

| # | Signature | Prefixes | Opcode | Group | ModR/M | Address | Rows | Exact keys | Binary distribution |
|---:|---|---|---|---:|---|---|---:|---:|---|
| 1 | `movaps mem128,xmm` | `(none)` | `0F 29` |  | memory | `base_only/a64` | 534 | 19 | bash=306, dash=70, tar=158 |
| 2 | `movdqa xmm,mem128` | `operand_size` | `0F 6F` |  | memory | `rip_relative/a64` | 269 | 2 | bash=248, dash=3, tar=18 |
| 3 | `movaps mem128,xmm` | `(none)` | `0F 29` |  | memory | `rip_relative/a64` | 246 | 8 | bash=217, dash=3, tar=26 |
| 4 | `movdqa xmm,mem128` | `operand_size` | `0F 6F` |  | memory | `base_only/a64` | 218 | 16 | bash=158, dash=1, tar=59 |
| 5 | `movq xmm,r64` | `operand_size rex_w` | `0F 6E` |  | register | `none/aNone` | 112 | 35 | bash=46, dash=14, tar=52 |
| 6 | `notrack jmp r64` | `ds` | `FF` | 4 | register | `none/aNone` | 110 | 3 | bash=61, dash=23, tar=26 |
| 7 | `movdqu xmm,mem128` | `rep rex` | `0F 6F` |  | memory | `base_only/a64` | 76 | 21 | bash=26, dash=1, tar=49 |
| 8 | `movq mem64,xmm` | `operand_size` | `0F D6` |  | memory | `base_only/a64` | 61 | 16 | bash=35, dash=5, tar=21 |
| 9 | `movq xmm,mem64` | `rep` | `0F 7E` |  | memory | `rip_relative/a64` | 61 | 6 | bash=50, dash=6, tar=5 |
| 10 | `movd xmm,r32` | `operand_size` | `0F 6E` |  | register | `none/aNone` | 57 | 19 | bash=45, dash=4, tar=8 |

### Top exact encoding shapes

| # | Prefix bytes | Opcode | ModR/M | Address shape | Rows | Binary distribution |
|---:|---|---|---|---|---:|---|
| 1 | `66` | `0F 6F` | `mod=0,reg=0,rm=5` | `rip_relative/a64` | 268 | bash=247, dash=3, tar=18 |
| 2 | `(none)` | `0F 29` | `mod=0,reg=0,rm=5` | `rip_relative/a64` | 237 | bash=208, dash=3, tar=26 |
| 3 | `(none)` | `0F 29` | `mod=2,reg=0,rm=5` | `base_only/a64` | 163 | bash=93, dash=7, tar=63 |
| 4 | `66` | `0F 6F` | `mod=2,reg=0,rm=5` | `base_only/a64` | 109 | bash=77, dash=1, tar=31 |
| 5 | `3E` | `FF` | `mod=3,reg=4,rm=0` | `none/aNone` | 98 | bash=59, dash=17, tar=22 |
| 6 | `(none)` | `0F 29` | `mod=1,reg=0,rm=5` | `base_only/a64` | 87 | bash=53, dash=13, tar=21 |
| 7 | `66` | `0F 6F` | `mod=1,reg=0,rm=5` | `base_only/a64` | 49 | bash=37, tar=12 |
| 8 | `F3` | `0F 7E` | `mod=0,reg=0,rm=5` | `rip_relative/a64` | 41 | bash=30, dash=6, tar=5 |
| 9 | `(none)` | `0F 29` | `mod=1,reg=7,rm=5` | `base_only/a64` | 36 | bash=22, dash=7, tar=7 |
| 10 | `66 48` | `0F 6E` | `mod=3,reg=0,rm=0` | `none/aNone` | 35 | bash=11, dash=10, tar=14 |

## Scenario D

Binaries: bash, dash, gcc-as, gcc-cc1, gcc-collect2, gcc-driver, gcc-ld-bfd. Rootfs residual: 97152 rows, 149 forms, 1937 exact keys. E delta: 0 rows.

### Top normalized forms

| # | Signature | Prefixes | Opcode | Group | ModR/M | Address | Rows | Exact keys | Binary distribution |
|---:|---|---|---|---:|---|---|---:|---:|---|
| 1 | `movaps mem128,xmm` | `(none)` | `0F 29` |  | memory | `base_only/a64` | 23789 | 51 | bash=306, dash=70, gcc-as=152, gcc-cc1=22403, gcc-collect2=304, gcc-driver=406, gcc-ld-bfd=148 |
| 2 | `movq xmm,r64` | `operand_size rex_w` | `0F 6E` |  | register | `none/aNone` | 14526 | 113 | bash=46, dash=14, gcc-as=73, gcc-cc1=14038, gcc-collect2=83, gcc-driver=181, gcc-ld-bfd=91 |
| 3 | `movq xmm,mem64` | `rep` | `0F 7E` |  | memory | `base_only/a64` | 10728 | 68 | bash=19, dash=3, gcc-as=8, gcc-cc1=10626, gcc-collect2=33, gcc-driver=35, gcc-ld-bfd=4 |
| 4 | `movaps xmm,mem128` | `(none)` | `0F 28` |  | memory | `base_only/a64` | 9164 | 32 | gcc-as=1, gcc-cc1=9059, gcc-collect2=41, gcc-driver=59, gcc-ld-bfd=4 |
| 5 | `movdqa xmm,mem128` | `operand_size` | `0F 6F` |  | memory | `base_only/a64` | 8212 | 50 | bash=158, dash=1, gcc-as=33, gcc-cc1=7783, gcc-collect2=84, gcc-driver=101, gcc-ld-bfd=52 |
| 6 | `movq mem64,xmm` | `operand_size` | `0F D6` |  | memory | `base_only/a64` | 7102 | 57 | bash=35, dash=5, gcc-as=15, gcc-cc1=6979, gcc-collect2=26, gcc-driver=36, gcc-ld-bfd=6 |
| 7 | `notrack jmp mem64` | `ds` | `FF` | 4 | memory | `scaled_index/a64` | 3803 | 7 | gcc-cc1=3718, gcc-collect2=31, gcc-driver=54 |
| 8 | `movdqu xmm,mem128` | `rep` | `0F 6F` |  | memory | `base_only/a64` | 3406 | 121 | bash=19, dash=1, gcc-as=34, gcc-cc1=3174, gcc-collect2=70, gcc-driver=73, gcc-ld-bfd=35 |
| 9 | `movhps xmm,mem64` | `(none)` | `0F 16` |  | memory | `base_only/a64` | 2024 | 54 | bash=4, dash=2, gcc-as=3, gcc-cc1=1970, gcc-collect2=16, gcc-driver=20, gcc-ld-bfd=9 |
| 10 | `movd xmm,r32` | `operand_size` | `0F 6E` |  | register | `none/aNone` | 1561 | 45 | bash=45, dash=4, gcc-as=13, gcc-cc1=1478, gcc-collect2=5, gcc-driver=6, gcc-ld-bfd=10 |

### Top exact encoding shapes

| # | Prefix bytes | Opcode | ModR/M | Address shape | Rows | Binary distribution |
|---:|---|---|---|---|---:|---|
| 1 | `(none)` | `0F 29` | `mod=0,reg=0,rm=4` | `base_only/a64` | 5781 | gcc-cc1=5758, gcc-collect2=7, gcc-driver=16 |
| 2 | `(none)` | `0F 28` | `mod=0,reg=0,rm=4` | `base_only/a64` | 5038 | gcc-cc1=5031, gcc-collect2=1, gcc-driver=6 |
| 3 | `(none)` | `0F 29` | `mod=2,reg=0,rm=4` | `base_only/a64` | 3721 | gcc-cc1=3652, gcc-collect2=30, gcc-driver=39 |
| 4 | `66 48` | `0F 6E` | `mod=3,reg=1,rm=0` | `none/aNone` | 3276 | bash=2, gcc-as=2, gcc-cc1=3256, gcc-collect2=4, gcc-driver=6, gcc-ld-bfd=6 |
| 5 | `3E` | `FF` | `mod=0,reg=4,rm=4` | `scaled_index/a64` | 3218 | gcc-cc1=3170, gcc-collect2=18, gcc-driver=30 |
| 6 | `(none)` | `0F 29` | `mod=1,reg=0,rm=4` | `base_only/a64` | 3063 | gcc-cc1=2980, gcc-collect2=28, gcc-driver=55 |
| 7 | `F3` | `0F 7E` | `mod=0,reg=0,rm=3` | `base_only/a64` | 2733 | gcc-as=1, gcc-cc1=2731, gcc-ld-bfd=1 |
| 8 | `F3` | `0F 7E` | `mod=0,reg=0,rm=4` | `base_only/a64` | 2692 | gcc-cc1=2691, gcc-driver=1 |
| 9 | `66` | `0F D6` | `mod=0,reg=0,rm=4` | `base_only/a64` | 2599 | gcc-cc1=2596, gcc-collect2=1, gcc-driver=2 |
| 10 | `66 48` | `0F 6E` | `mod=3,reg=0,rm=0` | `none/aNone` | 2166 | bash=11, dash=10, gcc-as=33, gcc-cc1=2014, gcc-collect2=14, gcc-driver=55, gcc-ld-bfd=29 |
