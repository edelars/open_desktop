# S2-38ae — SUBSS m32 memory with unmasked exceptions

Implemented the distinct schema-v2 interpreter-only profile
`s2-long-subss-memory-unmasked-rax-v1` for exact Long64 `F3 0F 5C 00`
(`SUBSS XMM0,m32 [RAX]`). It reuses the existing exact decoder operation
while keeping a separate profile and control echo. It requires SSE bit 25,
full instruction fetch, exact-form admission, supported/reserved MXCSR bits,
DAZ=FTZ=0, and OM=UM=1. It admits all RC modes, IM/DM/ZM/PM mask settings and
prior sticky histories. A successful operation reads exactly four bytes from
the read-only `[RAX]` mapping and updates only XMM0.low32, preserving high96.
S2-37dk masked memory and S2-37aq register subtraction remain unchanged.

The event order is explicit: IE/DE are identified before arithmetic and an
unmasked pre-event exception commits only the applicable new MXCSR status
before raising #XM(19) or SIMD #UD(6), selected by OSXMMEXCPT. If no pre-event
fault occurs, PE is processed after the independent raw-u32 subtraction; an
unmasked PE commits status but suppresses XMM0, RIP and retirement. OE/UE are
outside this profile and remain masked by admission. AC=1 is refused before
address validation; with AC clear, unaligned access is allowed. Canonical span
validation precedes the single read, and a read failure leaves architectural
state unchanged.

## Coverage

The runner matrix has 21 operand/event pairs × 4 RC modes × 64 prior sticky
histories × 8 IM/DM/PM mask patterns × 2 OSXMMEXCPT settings: 86,016
executions. The independent oracle uses the fixture's actual memory RHS and
raw u32 operands. Cases cover halfway inexact rounding, exact predecessor,
masked overflow, DE-only and DE+PE, exact tiny results, same-sign invalid and
opposite-sign valid infinities, positive/negative SNaNs in either operand,
QNaN/SNaN selection, NaN suppressing denormal detection, signed zeros and an
exact minimum-subnormal result. Positive and negative QNaNs and SNaNs are
covered in both operand positions; NaN+subnormal cases cover either position.

Five hand-authored literal fixtures cover success, IE/#XM, IE/#UD, DE/#XM and
PE/#UD. The runner fixture test validates their outcomes and retirement,
rejects JIT/native backend labels and rejects result control-echo tampering.
Input-schema mutation checks reject a writable source mapping, non-memory
ModRM, unsupported MXCSR, and expected exits inconsistent with the raw-u32
event oracle. The dedicated profile validation derives pre-event IE/DE and
post-event PE from `independent_subss` in `od-testkit/src/v2.rs`; the runner
matrix's explicit vector event table independently checks fault and retirement
classification.
The focused ISA test checks full-fetch SSE/TS gate priority at CPL0–3 and
no-read/state rollback, PE's post-event status commit and XMM0/RIP suppression,
a successful unaligned read4 exactly once, read-error rollback, and
canonical-span #GP(0) without a data read.

## Validation

- Focused 86,016-case raw-u32 matrix: passed in 36.43 seconds, including the
  signed QNaN right-operand and NaN+subnormal operand-position cross-products.
- Focused ISA event/read/span test: passed.
- Five-fixture interpreter-only, backend-denial and result-echo test: passed.
- `cargo test -p od-isa -p od-testkit -p od-runner --offline --quiet`: passed;
  od-isa 1,957, runner 138 unit and 559 fixture tests, od-testkit 456.
- `cargo check --workspace --all-targets --offline`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `graphify update .`: passed; graph has 12,453 nodes, 26,905 edges and 390
  communities.
- Independent Sol/high review is pending after this slice is committed.

No physical x86 acceptance is claimed. The slice adds no unmasked OE/UE,
native/JIT/C ABI/generic/fragment execution or changes to S2-37dk/S2-37aq.
See the [CPU contract](../specs/CPU_CONTRACT.md#s2-38ae--exact-subss-memory-profile-with-unmasked-exceptions),
[Stage 2 plan](../plans/STAGE_2.md#s2-38ae--outcomes-for-exact-subss-memory-form),
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-38ae-subss-m32-memory-unmasked-exceptions).
