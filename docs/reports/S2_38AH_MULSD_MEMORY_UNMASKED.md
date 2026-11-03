# S2-38ah — MULSD XMM0,m64 [RAX] with unmasked exceptions

## Boundary

Adds immutable schema-v2 profile `s2-long-mulsd-memory-unmasked-rax-v1` for
exact Long64 `F2 0F 59 00`, SSE2 bit 26, and read-only 64-bit memory operand
`[RAX]`. The profile uses a single `read8`, budget one, and stop RIP=entry+4.
It requires `DAZ=FTZ=0`, reserved MXCSR bits clear, and `OM=UM=1`, while
admitting all RC values, IM/DM/ZM/PM masks, and previous sticky histories.
The masked S2-37dn memory form and register S2-37at profile remain separate.

Production execution uses the existing raw MULSD helper only after fetching
and reading the actual memory RHS. The independent od-testkit expected-value
path uses integer raw-u64 arithmetic and the fixture memory operand. It
retains original-operand NaN priority and WPR tininess-after-rounding. An
unmasked IE or DE pre-event suppresses post-computation flags and the
destination; an unmasked PE post-event commits applicable status but
suppresses XMM0, RIP advancement, and retirement. Other state, including
XMM0.high64, is preserved.

## Coverage

- Direct ISA tests cover the exact instruction and one-read unaligned success,
  SSE2/EM/TS/OSFXSR gates and fetch precedence, CPL, AC-before-address, checked
  canonical span failure, read rollback, IE/DE ordering, PE suppression, and
  #XM/#UD delivery.
- Runner matrix crosses four RC settings × 64 prior sticky histories × all 16
  IM/DM/ZM/PM combinations × both OSXMMEXCPT settings for 20 operand/event
  vectors: 163,840 independently validated executions. Exact raw-u64 pairs:

  | Vector | XMM0.low64 × m64 RHS | Event intent |
  | --- | --- | --- |
  | `snan_lhs` | `7ff0000000000001 × 3ff0000000000000` | IE, left SNaN |
  | `snan_rhs` | `3ff0000000000000 × 7ff0000000000001` | IE, right SNaN |
  | `negative_snan_lhs` | `fff0000000000001 × 3ff0000000000000` | IE, negative left SNaN |
  | `negative_snan_rhs` | `3ff0000000000000 × fff0000000000001` | IE, negative right SNaN |
  | `negative_qnan_lhs` | `fff8000000001234 × 3ff0000000000000` | negative left QNaN |
  | `negative_qnan_rhs` | `3ff0000000000000 × fff8000000001234` | negative right QNaN |
  | `qnan_subnormal` | `7ff8000000001234 × 0000000000000001` | QNaN suppresses input DE |
  | `qnan_rhs` | `3ff0000000000000 × 7ff8000000001234` | right QNaN |
  | `denormal_lhs` | `0000000000000001 × 3ff0000000000000` | DE, exact continuation when masked |
  | `denormal_inexact_tiny` | `0000000000000001 × 3ff8000000000000` | DE then tiny/inexact continuation |
  | `exact_tiny_product` | `0010000000000000 × 3fe0000000000000` | exact subnormal, no UE/PE |
  | `inexact_tiny_product` | `0010000000000001 × 3fe0000000000000` | UE/PE tiny pair |
  | `round_to_min_normal` | `0010000000000000 × 3fefffffffffffff` | WPR boundary near minimum normal |
  | `zero_infinity` | `0000000000000000 × 7ff0000000000000` | IE, zero × infinity |
  | `negative_zero_infinity` | `8000000000000000 × 7ff0000000000000` | IE with signed zero |
  | `signed_zero_product` | `8000000000000000 × 4000000000000000` | exact negative zero |
  | `signed_infinity_product` | `fff0000000000000 × 4000000000000000` | exact negative infinity |
  | `exact_normal_product` | `4000000000000000 × 4000000000000000` | exact finite product |
  | `overflow_product` | `7fefffffffffffff × 4000000000000000` | OE/PE overflow |
  | `inexact_product` | `3ff0000000000001 × 3ff0000000000001` | normal PE |

  Both subnormal-input vectors raise DE. The inexact-tiny subnormal-input case
  exercises masked DE continuation to UE/PE and unmasked DE suppression of
  later events. Exact-tiny and inexact-tiny are separate vectors.
- Event vectors cover signed QNaN/SNaN positions, QNaN+subnormal, zero×infinity,
  signed zero/infinity, exact/inexact product, overflow, tiny results and WPR
  rounding to minimum normal. Three additional literal fixtures exercise IE,
  DE, and PE exception exits; the success fixture uses the actual unaligned
  read-only memory source.
- Schema and result tests cover strict read-only scope for this profile,
  control echo, complete architectural state, MXCSR status, exception metadata,
  RIP and retirement tampering, and JIT/native denial.
- Neighboring masked SUBSD permission behavior is unchanged; no generic
  arithmetic refactor or native/JIT/C ABI/generic/fragment support was added.

## Validation

Passed in the isolated worktree:

- `cargo fmt --all -- --check`
- `cargo check --workspace --all-targets --offline`
- `cargo test -p od-isa --offline` — 1,960 passed
- `cargo test -p od-testkit --offline` — 459 passed
- `cargo test -p od-runner --offline` — 139 unit and 561 fixture tests passed
- Focused ISA profile tests — 2 passed; focused literal fixture test — 1 passed
- 163,840-run RC/sticky/mask/delivery matrix with exact/inexact tiny,
  subnormal-input DE/continuation, signed NaN permutations and event priority — passed
- `graphify update .` — 12,504 nodes, 27,011 edges, 344 communities
- `git diff --check`

Independent Sol/high review is pending. These results are host-side
interpreter evidence; no physical x86 acceptance is claimed.

Sources: Intel SDM Rev. 093, Vol. 2B MULSD pp. 4-148–4-149; Vol. 2A §2.5.3,
Table 2-20; Vol. 1 §§4.9.1.4–.6, 11.5.2–11.5.3, and 11.6.6. See the [CPU
contract](../specs/CPU_CONTRACT.md#s2-38ah--exact-mulsd-memory-profile-with-unmasked-exceptions),
[Stage 2 plan](../plans/STAGE_2.md#s2-38ah--exact-mulsd-memory-form-with-unmasked-exceptions),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-38ah-mulsd-m64-memory-unmasked-exceptions).
