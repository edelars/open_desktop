# od-testkit

Test-only infrastructure for versioned CPU fixtures and machine-readable execution results. Production runtime crates must not depend on this crate.

The v1 format uses strict JSON parsing, fixed-width hexadecimal strings for 64-bit architectural values, SHA-256 digests, explicit observation availability, and bounded program/result sizes. The initial profile covers only the S0-09 register-only subset; memory and exception comparison are represented as unsupported, not as passing empty results.

`case::Fixture::from_json` validates a fixture and returns `LoadedFixture` with the digest of the exact JSON input bytes. `case::Fixture::to_json` validates before writing. `result::RunResult` provides equivalent bounded load/save operations and can check that its case ID, input digest, and profile match a supplied fixture. `compare::compare_results` rejects incompatible or incomplete runs before comparing fixture-scoped registers, RIP, masked RFLAGS, bounded memory regions, and supported exception metadata. The current S0-09 profile deliberately leaves memory and exception comparison unsupported.

`generate::generate_fixture(seed, sequence)` emits deterministic, versioned S0-09 cases. `minimize::minimize_fixture` reduces a fixture using `od-isa` instruction boundaries and a caller supplied mismatch-class callback; callers keep process and hardware execution outside testkit. Attempts count actual callback calls and require a limit of at least two. The initial mismatch and every accepted candidate must reproduce twice. Branch targets and the entry RIP are remapped when instructions are removed, and every candidate is validated again.

`native_xorpd::reduce_native_xorpd_fixture` is a separate profile-only reducer
for the exact S2-40b native XORPD fixture. It edits only initial XMM0–XMM15,
uses one deterministic register/byte/bit pass, and accepts a candidate only
when its total XMM Hamming weight decreases and two supplied callback results
preserve the same XMM mismatch-position fingerprint. It reserves two final
confirmation calls within a 4..=5120 actual-run budget. The callback remains
caller-owned; the testkit never starts native hardware or a process. It makes
no global-minimum claim. See [S2-42b format and replay](../../docs/testing/TEST_CASE_FORMAT.md#s2-42b-bounded-native-xorpd-xmm-input-reducer).

See [TEST_CASE_FORMAT.md](../../docs/testing/TEST_CASE_FORMAT.md) for fields, limits, dependency versions, provenance and sample files.

Schema v2 (`s2-long-integer-v1`) provides full 16-GPR/RIP/RFLAGS state,
bounded fixture RAM, fault observations and strict comparison. The locally accepted S2-08 slice
connects `od-runner --fixture --engine interpreter` to actual MOV execution
and full snapshots; v2 JIT remains explicitly Unsupported. Architectural faults,
mapping/backend errors and cooperative wall timeout have distinct outcomes.
The three [stage2 fixtures](../../tests/fixtures/x86/stage2/) exercise real
programs locally. S2-09 separately passed three hardware-layout MOV/memory
programs twice on one Intel CPU; see [CPU_ORACLE](../../docs/testing/CPU_ORACLE.md#native-v2-s2-09-границы-физического-подтверждения)
for evidence and limits. This does not verify the full stage 2 profile.
S2-10 adds five hand-authored transfer programs with canaries and intermediate
result snapshots. The eight-case v2 suite passed twice on one Intel CPU,
alongside the 1010-case legacy regression; [transfer evidence](../../docs/reports/data/stage2/transfers-2026-10-22.json)
records exact inputs and scope. Memory XCHG, privileged faults and v2 JIT remain outside that confirmation.

S2-11 adds 313 one-instruction arithmetic fixtures and a literal full-state,
flags and memory sidecar. The materializer packages handwritten templates;
it does not evaluate x86 arithmetic. The 321-case v2 suite passed twice on one
Intel CPU; [arithmetic evidence](../../docs/reports/data/stage2/arithmetic-2026-10-22.json)
links the matching legacy regression. Injected read/write faults verify local
no-commit behavior; they do not establish native guest #PF delivery.

S2-12 adds 197 literal logic cases and validates dynamic semantic flags masks:
AF becomes undefined after logic/TEST, while NOT preserves definedness. Both
materializers pass `--check`; 518 v2 cases passed twice on one Intel CPU.
[Logic evidence](../../docs/reports/data/stage2/logic-2026-10-22.json) preserves
the manifests and scope separately from accepted S2-11 evidence. Mask updates
follow shared decoded FlagEffects after retirement, without a result evaluator.

## Dependencies

- `serde` 1.0.229 with `derive`: general data model serialization.
- `serde_json` 1.0.151 with default `std`: JSON input/output.
- `sha2` 0.10.9 with default `std`: SHA-256 digests.
- `od-isa` workspace path dependency: reuse the project's decoder to locate valid instruction boundaries while reducing fixtures; testkit does not copy decoder semantics.

All are general-purpose infrastructure dependencies. None implement CPU semantics. The resolved transitive graph is pinned in the workspace `Cargo.lock`.

S2-18c adds five handwritten schema v2 fixtures for bare `A5` MOVSD: three
mapped positive cases and two local-only injected access faults. Their literal
state and full data-memory answers are serialized by
`tests/cpu/generate_stage2_movsd.py`; the generator does not calculate x86
semantics. The five cases extend the deferred physical pool from 568 to 573.

S2-30by adds five exact `s2-long-x87-state-v1` FCMOVE streams with budgets
1/2/3/6/6: bare, status observation, repeat, and two FCOMI/FCHS producers.
Operand and environment rejection cases are restricted to bare `dac9`; prefixed
and neighboring DA encodings remain outside the schema. This is fixture-format
coverage only and does not claim native or physical x86 behavior.

The S2-34.6b MOVUPS base profile extension uses the shared `od-isa` decoder to
derive RIP-relative literal addresses from initial RIP plus decoded instruction
length and signed disp32. It accepts only the same one-instruction full-state
fixture shape and direction-appropriate data mapping. This preserves the
single decoder as the encoding source; native/JIT remain denied.

S2-34.4 adds a distinct `s2-long-movups-mem-load-v1` schema-v2 profile. It
admits one literal legacy `0F 10 /r` with ModR/M.mod=00 and r/m=000, optional
REX.R, and one readable, non-writable data region covering `[RAX..RAX+15]`.
It keeps the S2-34.1 NOP-only and S2-34.2/.3 register-only profiles unchanged.
The handwritten fixture carries an unaligned source and canaries; local schema,
executor and runner checks verify state and memory preservation. Native
admission remains denied. This does not establish full Generic amd64 baseline
support or physical behavior.

S2-30cb admits exactly 19 FCMOVNE schema streams: each bare `DB C8..CF` form
alone and followed by FNSTSW, the repeated C9 stream, and two FCOMI producer
programs. Operand/environment rejects cover indexed source selection and
prefixes remain rejected. This schema allowlist is local fixture-format
coverage only; JIT/native and physical x86 behavior are not implied.

S2-30cc recorded the historical ST(1)-only `DA D1` FCMOVBE fixture schema.
S2-30cd admits exactly twenty streams: each bare `DA D0..D7` form alone and
followed by FNSTSW, plus the retained repeat and three FCOMI producer
sequences. The schema rejects prefixes, neighboring encodings, invalid
operands and forbidden FCW/FSW. It describes local fixture format only and
does not establish JIT, native or physical behavior.

S2-30ce adds exactly six `s2-long-x87-state-v1` FCMOVNBE streams for bare
`DB D1`, FNSTSW observation, repeat, and three FCOMI producer compositions.
Other DB D0..D7 forms, prefixes, and memory operands remain outside the
schema. Local fixture validation is not native or physical x86 evidence.

S2-30cg historically admitted only its exact five-instruction TEST producer
stream, with 512 success and 768 invalid class/tag interpreter snapshots.
S2-30ch admits exactly 18 FCMOVU streams: bare DA D8..DF and their FNSTSW
forms, the DA D9 repeat, and the retained producer. The schema still rejects
prefixes, neighboring encodings and all other sequences; these are local
fixture-format rules, not native or physical x86 evidence.

S2-34.1 adds the separate local-only `s2-long-x87-sse-state-v1` snapshot
profile. It requires both the existing x87 snapshot and 16 raw 16-byte XMM
registers plus raw MXCSR, verifies exact widths, and compares every XMM byte
and MXCSR. Its fixed `0x0000ffbf` diagnostic mask is profile metadata only, not
a universal hardware CPUID mask or guest `#GP` rule. A literal one-NOP fixture
checks state transfer; no SSE execution or physical verification is claimed.

S2-34.2 adds separate local fixture profile `s2-long-movups-reg-v1`, requiring
raw x87/SSE snapshots and one register-only legacy MOVUPS instruction. It
checks exact code shape, profile state and complete before/after snapshots;
native admission remains explicitly denied. See the [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-342-local-register-movups)
and [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-profile-s2-342-s2-long-movups-reg-v1).

S2-34.6a adds a distinct `s2-long-movups-mem-base-v1` schema-v2 profile. It
admits one legacy `0F 10` load or `0F 11` store with ordinary base+displacement
addressing (ModR/M modes 00/01/10, signed disp8/disp32), and REX.R/REX.B
register extensions. The handwritten `[R8-7]` fixture and schema checks retain
the earlier state-only, register and exact `[RAX]` profiles' shapes. SIB,
RIP-relative and unsupported prefix forms fail closed; native/JIT remain denied.
See the [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-346a-local-movups-basedisplacement-memory-forms)
and [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-profile-s2-346a-s2-long-movups-mem-base-v1).

S2-35.5 adds schema-v2 profile `s2-long-psubusb-reg-v1` for one register-only XMM `PSUBUSB`. It requires raw x87/SSE state and admits only `66 [one optional REX.R/B] 0F D8 /r` with ModR/M.mod=3. The strict profile rejects bare MMX, memory, neighboring prefixes and encodings; native and JIT remain denied.

S2-35.8 adds the literal local-only PADDUSW diagnostic fixture profile `s2-long-paddusw-reg-v1`; it requires raw x87/SSE state and permits only the exact register encoding.

S2-35.9 adds the literal local-only PSUBUSW diagnostic fixture profile `s2-long-psubusw-reg-v1`; it requires raw x87/SSE state and permits only `66 [one optional REX.R/B] 0F D9 /r` with ModR/M.mod=3. Eight unsigned word lanes saturate subtraction at zero; JIT/native remain denied.

S2-35.10 adds the literal local-only PADDSB diagnostic fixture profile `s2-long-paddsb-reg-v1`; it requires raw x87/SSE state and permits only `66 [one optional REX.R/B] 0F EC /r` with ModR/M.mod=3. Sixteen signed byte lanes saturate addition to [-128, 127]; JIT/native remain denied.

S2-35.12 adds literal local-only PADDSW profile `s2-long-paddsw-reg-v1`. It admits only `66 [one optional REX.R/B] 0F ED /r`, ModR/M.mod=3, with XMM0–15 destination/source registers. Eight signed little-endian word lanes saturate sums computed in i32 to [-32768, 32767]; JIT/native remain denied.

S2-35.14 adds literal local-only PADDD profile `s2-long-paddd-reg-v1`. It admits only `66 [one optional REX.R/B] 0F FE /r`, ModR/M.mod=3, with XMM0–15 destination/source registers. Four independent little-endian dword lanes add modulo 2^32; JIT/native remain denied.

S2-37a/b add distinct schema-v2 profiles `s2-long-rsqrtss-reg-v1` and
`s2-long-rcpss-reg-v1` for one legacy scalar SSE register instruction each.
Both require complete x87/SSE snapshots and preserve all state except the
destination low dword and retired RIP. The RCPSS fixture is a literal
XMM8←XMM9 case; its exact expected bits express the local deterministic helper,
not an exact hardware result. JIT/native stay denied, and physical checks belong
to the combined final Mac mini substage. ISSUE-025 remains open for RCPSS's
ambiguous tiny-result threshold.

S2-37c adds the strict `s2-long-rsqrtps-reg-v1` schema-v2 profile for one
bare or REX.R/B-extended `0F 52 /r` XMM-register instruction. The packed fixture
contains four independent literal lanes, and the profile checks exactly one
three- or four-byte instruction with no trailing data. Scalar RSQRTSS,
mandatory-prefix forms, memory operands, JIT and native execution remain denied.

S2-37d adds `s2-long-rcpps-reg-v1`, which admits exactly one bare or
REX.R/B-extended `0F 53 /r` XMM-register instruction. Its schema profile rejects
the F3 scalar RCPSS neighbor while common decoding continues to support RCPSS
and RSQRTPS separately. Four literal lanes exercise the local reciprocal value
policy, with native/JIT closed and ISSUE-025's hardware tiny-result boundary
left unresolved.

S2-37e adds schema-v2 profile `s2-long-minss-reg-masked-v1` for exactly one
Long64 register-only legacy MINSS instruction. The profile requires all six
MXCSR exception masks, DAZ/FTZ clear and reserved bits clear; it permits any RC
and sticky state. Literal expected outputs include operand-2 NaN selection and
masked IE/DE sticky bits. No fixture schema fields are added, and JIT/native
remain unsupported. Full S2-38 exception delivery/order is still open. See the
[plan](../../docs/plans/STAGE_2.md#s2-37e--local-long64-register-only-legacy-minss-with-masked-status).

S2-37f adds `s2-long-maxss-reg-masked-v1` under the same supported MXCSR
preconditions. MAXSS’s literal result is paired with independently accumulated
masked IE/DE sticky bits; unsupported MXCSR values are rejected during fixture
validation. The fixture schema remains unchanged and JIT/native remain closed.

S2-37g adds `s2-long-minps-reg-masked-v1` for one bare or REX.R/B-extended
register `0F 5D /r` form. Fixture admission requires the supported masked
MXCSR subset and rejects scalar MINSS and other prefix/operand forms. Four
literal binary32 lanes check output order and IE/DE sticky aggregation; JIT and
native admission remain closed.

S2-37h adds `s2-long-maxps-reg-masked-v1` for one bare or REX.R/B-extended
register `0F 5F /r` form. Fixture admission requires the supported masked
MXCSR subset and rejects scalar MAXSS and other prefix/operand forms. Four
literal binary32 lanes check output order and IE/DE sticky aggregation; JIT and
native admission remain closed.


S2-37i adds schema-v2 profile `s2-long-minsd-reg-masked-v1` for exactly one
Long64 register-only `F2 [optional adjacent REX.R/B] 0F 5D /r` MINSD form. It
requires the same masked-MXCSR subset as MINSS/MINPS and accepts any RC and old
sticky bits. The schema validator keeps F2 MINSD distinct from F3 MINSS and
bare MINPS, and the handwritten fixture includes upper-qword NaN/subnormal
sentinels. JIT/native remain closed. See the
[plan](../../docs/plans/STAGE_2.md#s2-37i--local-long64-register-only-legacy-minsd-with-masked-status).


The schema-v2 `s2-long-cvtsd2ss-registers-v1` profile independently validates
raw binary64-to-binary32 conversion using integer rounding and event logic. It
checks low32 result writes, high96 preservation, MXCSR status and atomic numeric
faults without production helpers or host floating point. See the [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37be-canonical-cvtsd2ss-register-pairs).

S2-37bk adds schema-v2 profile `s2-long-cvtpd2pi-register-masked-v1` for exact
`66 0F 2D C1`. Its independent raw-u64 oracle rounds binary64 lanes to signed
i32 without production conversion helpers or host FP, then validates sticky
IE/PE and the x87/MMX physical slot-0 state transition. The profile validator
locks the instruction bytes, budget, stop RIP, MXCSR subset and no-pending-x87
precondition; backend admission remains interpreter-only. See the [CPU
contract](../../docs/specs/CPU_CONTRACT.md#s2-37bk--bounded-long64-cvtpd2pi-mm0xmm1-masked-register-profile)
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37bk-cvtpd2pi-mm0xmm1-masked-register-profile).

S2-37bl adds schema-v2 profile `s2-long-cvttpd2pi-register-masked-v1` for
exact `66 0F 2C C1`. Its independent oracle truncates raw binary64 `u64`
fields toward zero without production helpers or host FP, irrespective of
MXCSR.RC, and validates sticky IE/PE plus physical MM0 state. The fixture
validator locks bytes, budget, stop RIP, MXCSR and x87 preconditions; native
and JIT admission remain closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bl--bounded-long64-cvttpd2pi-mm0xmm1-masked-register-profile)
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37bl-cvttpd2pi-mm0xmm1-masked-register-profile).

S2-37bm adds `s2-long-cvtpi2pd-register-masked-v1` for exact
`66 0F 2A C1`. Its independent oracle reads physical MM1 from raw x87 slot 1,
converts signed i32 lanes to binary64 using integer operations, and checks XMM0
plus the TOP/tag transition and full unrelated-state preservation. Validation
locks instruction bytes, gate controls, budget, stop RIP and a no-pending-x87
precondition. MXCSR is preserved without instruction-specific restrictions;
native/JIT remain denied. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bm--bounded-long64-cvtpi2pd-xmm0mm1-register-profile)
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37bm-cvtpi2pd-xmm0mm1-register-profile).

S2-37bn adds `s2-long-cvtpi2ps-register-masked-v1` for exact `0F 2A C1`.
Its independent integer oracle reads physical MM1 from raw x87 slot 1 and
checks binary32 rounding, sticky PE, the TOP/tag transition and preservation
of all other SSE/x87 state. Validation requires PM=1 and no pending x87 state;
the separate controls use the SSE availability bit. See the [CPU
contract](../../docs/specs/CPU_CONTRACT.md#s2-37bn--bounded-long64-cvtpi2ps-xmm0mm1-register-profile)
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37bn-cvtpi2ps-xmm0mm1-register-profile).

S2-37bo adds `s2-long-cvttps2pi-memory-rax-v1` for exact `0F 2C 00`. The
independent integer oracle converts raw binary32 lanes without host FP or
production helpers and checks sticky IE/PE plus the MM0/TOP/tag transition.
Strict validation covers SSE gates, supported MXCSR with IM/PM set, no pending
x87 state, AC=0 and a canonical eight-byte RAX span. See the [CPU
contract](../../docs/specs/CPU_CONTRACT.md#s2-37bo--bounded-long64-cvttps2pi-mm0rax-memory-profile)
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37bo-cvttps2pi-mm0rax-memory-profile).

S2-37bp adds `s2-long-cvtps2pi-memory-rax-v1` for exact `0F 2D 00`. Its
independent schema oracle consumes two raw binary32 lanes from the declared
8-byte source, implements MXCSR-directed rounding with integer logic, and
predicts MM0/x87 transition plus sticky IE/PE without host FP or ISA helpers.
Strict admission requires IM=PM=1, AC=0 and no pending x87 exception. See the
[CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bp--bounded-long64-cvtps2pi-mm0rax-memory-profile)
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37bp-cvtps2pi-mm0rax-memory-profile).

S2-37bq adds `s2-long-cvttpd2pi-memory-rax-v1` for exact `66 0F 2C 00`.
The independent oracle classifies two raw binary64 lanes and truncates toward
zero without host FP or production conversion helpers. It validates sticky
IE/PE and the physical MM0/TOP/tag transition. The profile reads exactly one
aligned canonical m128 span, rejects misalignment with #GP regardless of CPL
or AC, and defers state mutation until the single read succeeds. Strict
admission checks the MXCSR subset, IM=PM=1 and no pending x87 exception. See
the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bq--bounded-long64-cvttpd2pi-mm0rax-aligned-m128-memory-profile)
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37bq-cvttpd2pi-mm0rax-aligned-m128-memory-profile).

S2-37br adds schema-v2 profile `s2-long-cvtpd2pi-memory-rax-v1` for exact
`66 0F 2D 00`. Its independent oracle rounds raw binary64 inputs with integer
arithmetic according to MXCSR.RC, then validates signed-i32 range and IE/PE
status. The profile has a separate `cvtpd2pi_memory_controls` field; foreign
profile controls remain rejected. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37br--bounded-long64-cvtpd2pi-mm0rax-aligned-m128-memory-profile)
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37br-cvtpd2pi-mm0rax-aligned-m128-memory-profile).

S2-37bs adds the separate schema-v2 `s2-long-cvtdq2pd-memory-v1` profile for
exact `F3 0F E6 00` `CVTDQ2PD XMM0,[RAX]`. Its independent oracle constructs
binary64 bits using integer operations only, checks gates and the one m64
source, and verifies complete MXCSR/x87 preservation. Literal fixtures include
pending x87 canaries; they establish local interpreter behavior only. See the
[CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bs--bounded-long64-cvtdq2pd-xmm0rax-m64-memory-profile), [plan](../../docs/plans/STAGE_2.md#s2-37bs--bounded-long64-cvtdq2pd-xmm0rax-m64-memory-profile), and [report](../../docs/reports/STAGE_2_REPORT.md#s2-37bs--cvtdq2pd-xmm0rax-m64-memory-profile).

S2-37bt adds separate schema-v2 profile `s2-long-cvtdq2ps-memory-rax-v1` and
`cvtdq2ps_memory_controls`. Its independent oracle uses integer bit construction
to round four i32 lanes to binary32, checks aggregate sticky PE, one aligned
m128 source, and full x87 preservation. Foreign profile controls are rejected.
See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bt--bounded-long64-cvtdq2ps-xmm0rax-aligned-m128-memory-profile), [plan](../../docs/plans/STAGE_2.md#s2-37bt--bounded-long64-cvtdq2ps-xmm0rax-aligned-m128-memory-profile), and [report](../../docs/reports/STAGE_2_REPORT.md#s2-37bt--cvtdq2ps-xmm0rax-aligned-m128-memory-profile).

S2-37bu adds a separate schema-v2 `s2-long-cvttps2dq-memory-rax-v1` profile
and `cvttps2dq_m128_controls` field. The raw-bit integer oracle implements
truncation toward zero, lane-aggregated IE/PE and full XMM0 output without host
floating point or production conversion helpers. It checks exact encoding,
16-byte aligned source, and profile-specific gate/admission ordering. See the
[CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bu--bounded-long64-cvttps2dq-xmm0rax-aligned-m128-memory-profile), [plan](../../docs/plans/STAGE_2.md#s2-37bu--bounded-long64-cvttps2dq-xmm0rax-aligned-m128-memory-profile), and [report](../../docs/reports/STAGE_2_REPORT.md#s2-37bu--cvttps2dq-xmm0rax-aligned-m128-memory-profile).

S2-37bv adds the exact schema-v2 `s2-long-cvtps2dq-memory-rax-v1` profile and
separate `cvtps2dq_m128_controls` field. Its independent raw-binary32 integer
oracle rounds according to MXCSR.RC before checking signed-i32 bounds, aggregates
IE/PE, and validates atomic one-span outcomes without host floating point or
production conversion helpers. DAZ=0 and IM=PM=1 are local admission rules under
the supported MXCSR mask `0xffbf`; native/JIT/C ABI stay closed.

S2-37bw adds the exact schema-v2 `s2-long-cvttpd2dq-memory-rax-v1` profile.
Its integer-only oracle decodes binary64 raw u64 lanes, truncates toward zero,
checks the truncated i32 range, and computes masked sticky IE/PE without host
floating point or production helpers. A separate control echo and strict
foreign-field checks keep this m128 profile distinct from CVTTPD2DQ registers.
See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bw--bounded-long64-cvttpd2dq-xmm0rax-aligned-m128-profile).

S2-37bx adds the exact schema-v2 `s2-long-cvtpd2dq-memory-rax-v1` profile for
legacy `F2 0F E6 00`. Validation isolates its MXCSR and prerequisite controls;
the independent raw-u64 oracle covers MXCSR.RC rounding and signed-i32 range
checks without host floating point or production conversion helpers. See the
[CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bx--bounded-long64-cvtpd2dq-xmm0rax-aligned-m128-profile).

S2-37by adds exact schema-v2 `s2-long-cvtps2pd-memory-rax-v1` controls for the
legacy `0F 5A 00` m64 form. Its integer-only oracle widens raw binary32 lanes,
models SNaN IE and subnormal DE, and checks sticky MXCSR plus full XMM0 state.
Separate controls and strict admission keep it isolated from register CVTPS2PD;
the oracle does not use host FP or production conversion helpers. See the
[CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37by--bounded-long64-cvtps2pd-xmm0rax-m64-memory-profile).


### S2-37bz — CVTSS2SD m32 memory profile

The exact `F3 0F 5A 00` scalar form reads four bytes from RAX once and writes
only the low 64 bits of XMM0. It widens binary32 exactly, adding masked IE for
SNaN quieting and DE for nonzero subnormal input; it preserves all upper XMM0
bits, MXCSR control/sticky state, and x87 canaries. RC/FTZ do not affect the
result. Native/JIT/C ABI remain closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bz--bounded-long64-cvtss2sd-xmm0rax-m32-memory-profile).

### S2-37ca — CVTSD2SS m64 memory profile

The `s2-long-cvtsd2ss-memory-rax-v1` schema profile uses a separate control
field and strict foreign-field admission. Its independent raw-u64 oracle
rounds by integer bit operations, checks WPR tininess, models FTZ and sticky
IE/DE/OE/UE/PE, and preserves XMM0 high bits. It does not use host FP or a
production conversion helper. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37ca--bounded-long64-cvtsd2ss-xmm0rax-m64-memory-profile).


### S2-37cb — CVTPD2PS m128 memory profile

The strict schema-v2 `s2-long-cvtpd2ps-memory-rax-v1` profile uses its own
control field and an independent raw-u64 integer oracle for two-lane binary64
narrowing, WPR/FTZ flags and full destination state. The oracle does not use host
FP or production conversion helpers. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37cb--bounded-long64-cvtpd2ps-xmm0rax-aligned-m128-memory-profile).

### S2-37cc — CVTSI2SD m32 memory profile

The strict `s2-long-cvtsi2sd-memory32-rax-v1` profile has its own gate field and
uses an independent raw-i32 integer oracle. The oracle constructs the exact
binary64 encoding without host floating point or production conversion helpers;
it checks XMM0 low64 output, high64 preservation, and exact MXCSR preservation.


### S2-37cd — CVTSI2SD m64 memory profile

The strict `s2-long-cvtsi2sd-memory64-rax-v1` profile has its own control field
and an integer-only raw-i64→binary64 oracle. It models RC-dependent ties and PE
stickies without host FP or production conversion helpers, and validates
low64 output, high64 preservation, optional x87 canaries, and exact MXCSR state.
See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37cd--bounded-long64-cvtsi2sd-xmm0qword-ptr-rax-m64-memory-profile).


### S2-37ce — CVTSI2SS m32 memory profile

The exact schema-v2 `s2-long-cvtsi2ss-memory32-rax-v1` profile owns a separate
control field and strict foreign-field admission. Its oracle reads the raw
little-endian i32 from fixture memory, then constructs the binary32 result with
integer rounding logic; it does not use host FP, the register-source result
parser, or a production conversion helper. It verifies low32 output, high96 and
x87 preservation, and sticky PE. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37ce--bounded-long64-cvtsi2ss-xmm0dword-ptr-rax-m32-memory-profile).

### S2-37cf — CVTSI2SS m64 memory profile

The strict `s2-long-cvtsi2ss-memory64-rax-v1` field is separate from the m32
profile. Its independent oracle reads raw fixture memory as signed i64 and
constructs binary32 via integer rounding, without host FP, production helpers,
or a register-source parser. It checks direct-rounding vectors including
`0x0040000040000001`, XMM0 low32/high96, sticky PE, and optional x87 state.
PM=0, DAZ=1, and reserved controls are malformed schema inputs; AC=1 is a
runtime-refusal fixture. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37cf--bounded-long64-cvtsi2ss-xmm0qword-ptr-rax-m64-memory-profile).

### S2-37ci — CVTTSS2SI EAX,m32

The strict `s2-long-cvttss2si-r32-memory-rax-v1` schema profile has its own
controls field and requires exact `F3 0F 2C 00`, one four-byte source mapping at
RAX, and the bounded IM/PM/DAZ/MXCSR policy. Its independent oracle reads raw
u32 from fixture memory, computes truncation and signed-range flags with
integer operations, and validates EAX zero-extension to RAX without host FP or
production conversion helpers. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37ci--bounded-long64-cvttss2si-eaxdword-ptr-rax-m32-to-r32-memory-profile).

### S2-37cj — CVTTSD2SI EAX,m64

The strict `s2-long-cvttsd2si-r32-memory-rax-v1` profile has its own control
snapshot and rejects foreign controls. The oracle parses raw binary64 bits from
the eight-byte fixture memory source, truncates toward zero independently of RC,
then checks signed i32 range and validates IE/PE sticky status, RAX
zero-extension, snapshot state and exact exception metadata. It uses integer
logic only, without host FP or production conversion helpers. See the [CPU
contract](../../docs/specs/CPU_CONTRACT.md#s2-37cj--bounded-long64-cvttsd2si-eaxqword-ptr-rax-m64-to-r32-memory-profile).

### S2-37ck — CVTSD2SI RAX,m64 memory profile

The independent oracle reads raw u64 from the fixture's m64 span and performs
binary64 rounding plus signed-i64 range checking using integer bit operations.
It checks full-width RAX output/aliasing, sticky IE/PE, read atomicity, and
state preservation without host FP or the production conversion helper. See
the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37ck--bounded-long64-cvtsd2si-raxqword-ptr-rax-m64-to-r64-memory-profile).


### S2-37cl — CVTSS2SI RAX,m32 memory oracle

The separate schema-v2 profile parses raw binary32 memory bits and uses integer classification, MXCSR rounding, and signed-i64 range checks. It validates full RAX aliasing, sticky IE/PE, exception metadata, and state preservation without host FP or production conversion helpers.

### S2-37cm — CVTTSS2SI RAX,m32 memory oracle

The separate schema-v2 profile validates the exact `F3 48 0F 2C 00` memory
form and carries a dedicated SSE-control echo. Its independent oracle reads
raw binary32 bits, applies TowardZero regardless of MXCSR.RC, checks signed-i64
range, and validates sticky IE/PE plus full RAX output without host FP or a
production conversion helper. The runner exercises 20 raw values × 4 RC modes ×
4 sticky histories (320 comparisons); the focused raw-integer anchor test passed
1/1.

### S2-37cn — CVTTSD2SI RAX,m64 memory oracle

The strict schema-v2 profile carries a dedicated control echo and independent
oracle. It parses the source as raw u64 binary64, truncates toward zero, checks
signed-i64 range and sticky IE/PE, and validates full RAX aliasing and state
preservation without host FP or production conversion helpers. The runner
exercises 19 source values × 4 RC values × 4 sticky histories (304 comparisons).

`memory_corpus` provides the deterministic seed/index generator shared by the
S3-34a host correctness corpus and its bounded benchmark subset. The fixed
S3-34a constants name exactly two original sequences, three seeds, and 400
inputs per sequence/seed; the later load-then-load extension remains available
to the host correctness corpus through the same generator but is not part of
`S3_34A_SEQUENCES`.
