# od-runner

Headless runner for the limited S0-09 x86 fragment profile and the experimental S2 long-mode integer fixture profile. It calls `od-isa` for interpretation and uses the macOS ARM64 host path for S0 JIT execution. Instruction semantics and ARM64 translation remain in their owning libraries. Linux builds use the interpreter without linking `od-host`; JIT requests return an explicit `unsupported` JSON result. Schema v2 JIT execution is unsupported on every host.

Example on macOS ARM64:

```sh
cargo run --offline -p od-runner -- --hex '48 2b c1 75 fb' --rax 3 --rcx 1 --budget 32 --engine both
```

Use `--hex-file PATH` to run a whitespace-separated project fixture, for example:

```sh
cargo run --offline -p od-runner -- --hex-file tests/fixtures/x86/s0_09_sub_jnz_loop.hex --rax 3 --rcx 1 --budget 32 --engine both
```

Specify exactly one code source, `--hex` or `--hex-file`. The command prints interpreter and JIT state and exits with an error if they differ. The runner does not boot a guest OS or provide a VMHost/Swift supervisor.

## Versioned fixture results

Run a JSON fixture once per engine. The result is the only stdout content; diagnostics go to stderr.

```sh
cargo run --offline -p od-runner -- \
  --fixture tests/fixtures/x86/s0_09_add_overflow.case.json \
  --engine interpreter --json
cargo run --offline -p od-runner -- \
  --fixture tests/fixtures/x86/s0_09_add_overflow.case.json \
  --engine jit --json
```

Compare two saved results against the exact fixture used to produce them:

```sh
target/debug/od-runner --compare-results interpreter.json jit.json \
  --case tests/fixtures/x86/s0_09_add_overflow.case.json --json
```

Schema v1 comparison JSON contains `field`, optional `mask`, `expected`, `actual`, and fixture/backend `context` for every difference. Schema v2 comparison reports contain `schema_version`, `case_id`, `input_sha256`, `matches`, and differing field names. Exit codes are `0` for a completed run or comparison match, `2` for a comparison mismatch, `3` for invalid input, `4` for an unsupported operation or host, and `5` for a harness failure or deadline. Unsupported engine results remain valid JSON and never appear as completed runs. Unknown CPU model, kernel, page size, compiler version, and source revision are represented as unavailable metadata; the CLI does not guess them.

The fixture command accepts `interpreter` or `jit` per invocation and dispatches strictly by schema version. The S0-09 v1 fixture contract and digests remain unchanged; it does not capture RAM or exceptions, and its in-process path has no wall-time watchdog. The native x86 runner enforces its wall-time limit in an isolated process.

### Schema v2 memory execution

```sh
target/debug/od-runner --fixture tests/fixtures/x86/stage2/mov_load_add_store.case.json \
  --engine interpreter --json
```

The three [hand-authored S2 fixtures](../../tests/fixtures/x86/stage2/README.md)
exercise MOV load/store and register aliases, including a load → qword ADD →
store across an unaligned 4 KiB/adjacent-region boundary. The v2 interpreter
executes on-demand MOV B0–BF/88–8B byte/word/dword/qword forms with the full
GPR set, plus the earlier S0 qword arithmetic and short JZ/JNZ slice.
Unsupported forms remain explicit outcomes.

Results bind to the exact fixture bytes and contain all 16 GPRs, architectural
RIP, flags and definedness, retired instruction count, complete final memory
regions, observed exception metadata, and build/host metadata. Initial memory
is in the exact input fixture, so final memory is a real before/after pair.
MOV/branches preserve the known input flags; the supported ADD/SUB/CMP forms
define every arithmetic flag. The result therefore marks the current integer
profile flags as defined. New instruction families must establish their own
definedness before execution is enabled.

Architectural exceptions retain faulting RIP and available error code/address;
they are completed CPU outcomes, not host failures. Missing fixture mappings,
permissions and backend errors produce `harness_error` rather than a fabricated
guest page fault. A data access endpoint beyond `u64::MAX` is currently reported
as a diagnostic address-overflow harness error; its architectural boundary is
not yet established. Unsupported v2 JIT requests emit schema v2 `unsupported`
with unavailable state/count and exit 4; they do not execute the interpreter.

The v2 `wall_timeout_ms` is a cooperative execution deadline checked at each
instruction boundary, before budget/fetch and after the stop-address check.
Its expiry produces `timed_out` with the precise current snapshot/count and
exit 5. This is not an isolated watchdog and cannot interrupt a stalled memory
backend call. Budget exhaustion and stop address are separate architectural
outcomes. These local execution checks do not establish physical x86 equivalence;
the S2 oracle expansion remains a separate task.

Generate a deterministic CPU corpus with a fixed seed and sequence count:

```sh
target/debug/od-runner --generate-corpus 0x5eedcafe 1000 work/stage1-generated-5eedcafe
```

The command records each fixture as JSON with generator provenance, seed, and byte digest. The corpus generator only emits the current register-only profile.

Reduce a stable local interpreter/JIT mismatch:

```sh
target/debug/od-runner --minimize CASE.json --output work/reduced.case.json --max-attempts 1000
```

The reducer confirms the initial mismatch and each accepted candidate twice, keeping only candidates where the same comparison field still differs. Instruction removal remaps branches and the entry RIP. It fails when no stable local mismatch exists. Use `python3 tests/cpu/stage1_oracle.py CASE.json` to reproduce and compare one fixture with the physical x86 runner before reducing an oracle mismatch; automatic reduction of physical-only differences is not currently wired into this CLI.

## Reproducible CPU benchmark

Build the release runner, then run at least 20 alternating interpreter/JIT pairs on a fixed fixture:

```sh
cargo build --release --offline -p od-runner
target/release/od-runner \
  --hex-file tests/fixtures/x86/s0_09_sub_jnz_loop.hex \
  --rax 3 --rcx 1 --budget 32 --benchmark
```

`--benchmark` first checks that both engines produce the same result, then calibrates every measurement batch to at least 200 ms and records 20 samples by default. `--samples N` accepts 20–1000 samples. Each pair alternates which engine runs first; the three cold-build measurements rotate their order. Since each engine has a different calibrated iteration count, the CSV records both batch counts and elapsed times plus normalized nanoseconds per operation. Summary lines report median, minimum, and maximum.

The output separates `decode_ns_per_op`, `translation_ns_per_op` (decode plus lowering/emission), and `compile_publish_ns_per_op` (translation, executable mapping, code copy, instruction-cache invalidation, and mapping drop). It compares repeated interpreter calls with a `CompiledFragment` whose executable mapping is reused. Each invocation starts from the same guest state and uses the same instruction budget; correctness is checked before timing and after every measured batch. Interpreter time includes decode on each invocation. The benchmark is limited to this fragment and host path; it does not measure VMHost/IPC, process launch, a guest OS, or application workloads.

Save raw output under `work/stage-0/<run-id>/` with the exact command, source revision and dirty-tree state, host model/OS/build, toolchain, and observed power/thermal/background conditions. The benchmark records every measured sample but does not collect those host metadata itself. Do not treat the result as a product speed claim or a final E04 decision before the remaining E04 scenarios and independent x86 comparison.

S2-30bz's indexed FCMOVE fixtures verify complete x87 register payloads, tags,
environment and instruction-pointer state in the interpreter runner, including
the second move in the repeat stream. Schema v2 JIT remains unsupported.
S2-30ca's `DB C9` FCMOVNE fixtures assert the full x87 result for taken and
untaken ZF outcomes, reject an invalid source with ZF=1, and explicitly deny
JIT.

S2-30cb extends those fixtures to indexed `DB C8..CF`. Runner assertions cover
self-index ZF outcomes, indices 2..7, raw payload/tag copies, repeat and FCOMI
producer streams, invalid source with ZF=1, forbidden environment, unchanged
memory and full x87 snapshots. Every fixture explicitly denies JIT. This is
local interpreter evidence; no native or physical x86 execution is claimed.

S2-30cc's exact bare `DA D1` FCMOVBE fixture runner compares full machine
state and memory for the truth table, TOP wrap, both repeated transfers and
three FCOMI producers; invalid participant and environment fixtures remain
atomic. Every success and rejection fixture denies JIT. No native or physical
x86 execution is claimed.

S2-30ce fixture runs compare full machine and memory snapshots for bare
FCMOVNBE, signed-zero transfer, repeat, and FCOMI-produced conditions. Both
repeat instructions are checked by interpreter state tests; the runner denies
JIT for all success and rejection fixtures.

S2-35.7's local-only register `PSUBW` fixture exercises `66 [one optional
REX.R/B] 0F F9 /r`; each of eight little-endian u16 lanes subtracts modulo
65536. JIT remains unsupported. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#local-register-only-psubw-profile--s2-357).

S2-35.8’s local-only register `PADDUSW` fixture checks eight unsigned word lanes with per-lane saturation. It remains interpreter-only and JIT/native denied.

S2-35.9’s local-only register `PSUBUSW` fixture checks eight unsigned word lanes with per-lane saturation to zero. It remains interpreter-only and JIT/native denied.

S2-35.10’s local-only register `PADDSB` fixture checks signed byte addition with saturation in all 16 lanes. It remains interpreter-only and JIT/native denied.

S2-37bk adds the literal schema-v2 `CVTPD2PI MM0,XMM1` fixture for exact
`66 0F 2D C1`. The runner validates the raw-bit binary64-to-i32 result,
MXCSR sticky IE/PE, MM0 slot/TOP/tag transition, full-fetch SSE2 gates and
profile refusals. Clear non-IM/PM masks are accepted when IM and PM remain set.
JIT/native execution remains unsupported; physical x86 acceptance is separate.
See the [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37bk-cvtpd2pi-mm0xmm1-masked-register-profile).

S2-37bl adds the literal schema-v2 CVTTPD2PI fixture for exact `66 0F 2C C1`.
The runner checks RC-independent truncation, independently aggregated sticky
IE/PE, MM0 slot/TOP/tag state, SSE2 gates, strict profile validation and
interpreter-only admission. Physical x86 acceptance remains separate. See the
[fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37bl-cvttpd2pi-mm0xmm1-masked-register-profile).

S2-37bm adds exact register-source `CVTPI2PD XMM0,MM1` (`66 0F 2A C1`). Its
literal fixture uses physical MM1 even with TOP=3, while the interpreter
transitions TOP/tags and preserves MXCSR and x87 payloads. The runner validates
the independent raw-integer oracle, varied MXCSR, gate cases, strict encoding
and backend denial. Physical x86 acceptance remains separate.

S2-37bn adds exact register-source `CVTPI2PS XMM0,MM1` (`0F 2A C1`) as a
schema-v2 interpreter-only profile. Its runner matrix exercises all RC modes,
physical MM1 selection across TOP/tag values, sticky PE and x87/XMM
preservation, SSE gates, strict validation, and native/JIT refusal. See the
[CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bn--bounded-long64-cvtpi2ps-xmm0mm1-register-profile)
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37bn-cvtpi2ps-xmm0mm1-register-profile).

S2-37bo adds the literal `CVTTPS2PI MM0,[RAX]` fixture. The runner verifies
RC-independent truncation, sticky IE/PE aggregation, exact-lane sticky
preservation, one unaligned canonical 8-byte read, MM0 state transition, strict
gates/admission and interpreter-only execution. Direct ISA tests verify that
invalid spans, AC/pending-x87/MXCSR refusals and a failed data read leave CPU
state unchanged without a data read on pre-read refusals. Physical x86
acceptance is separate.

S2-37bp adds exact `CVTPS2PI MM0,[RAX]` (`0F 2D 00`). The literal fixture
provides one unaligned m64 source; the runner matrix checks MXCSR rounding,
IE/PE generation and sticky preservation, MM0 transition, gate/admission
cases, malformed inputs, output tampering and interpreter-only execution.
Direct ISA tests cover the exact read span and atomic failure behavior. The
SDM operand table/Operation specify m64 and `SRC[63:0]`, followed here despite
the contradictory 128-bit prose. Physical x86 acceptance is separate. See the
[CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bp--bounded-long64-cvtps2pi-mm0rax-memory-profile)
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37bp-cvtps2pi-mm0rax-memory-profile).

S2-37bq adds exact `CVTTPD2PI MM0,[RAX]` (`66 0F 2C 00`). Its aligned m128
fixture and runner matrix cover RC-independent truncation, clear-flag IE/PE
generation, sticky preservation, the MM0/TOP/tag transition, all 15
misalignment offsets, gate and encoding rejection, tampered output, and
backend denial. Direct ISA tests establish the full CPL×AC misalignment matrix,
zero callbacks on pre-read failures, one 16-byte callback on success, and
rollback after a failed read.
No physical x86 acceptance is claimed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bq--bounded-long64-cvttpd2pi-mm0rax-aligned-m128-memory-profile)
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37bq-cvttpd2pi-mm0rax-aligned-m128-memory-profile).

S2-37br runs the exact local-only CVTPD2PI MM0,[RAX] profile with aligned
m128 memory and MXCSR.RC rounding followed by range validation. The runner
fixture matrix does not establish physical x86 acceptance. See the [CPU
contract](../../docs/specs/CPU_CONTRACT.md#s2-37br--bounded-long64-cvtpd2pi-mm0rax-aligned-m128-memory-profile)
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37br-cvtpd2pi-mm0rax-aligned-m128-memory-profile).

S2-37bs runs the exact local-only schema-v2 `CVTDQ2PD XMM0,[RAX]`
(`F3 0F E6 00`) profile. It reads exactly 8 bytes at RAX, converts the signed
integer lanes exactly, replaces XMM0, and preserves MXCSR and all x87 state.
Fixtures exercise supported MXCSR variations and pending x87 canaries; JIT and
native requests are denied. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bs--bounded-long64-cvtdq2pd-xmm0rax-m64-memory-profile), [plan](../../docs/plans/STAGE_2.md#s2-37bs--bounded-long64-cvtdq2pd-xmm0rax-m64-memory-profile), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37bs-cvtdq2pd-xmm0rax-m64-memory-profile).

S2-37bt runs the local-only schema-v2 legacy `CVTDQ2PS XMM0,[RAX]`
(`0F 5B 00`) profile. The matrix covers RC and per-lane PE, sticky status,
aligned AC values, misalignment faults, x87 preservation, schema/tamper checks
and JIT/native denial. Direct ISA tests provide callback-count evidence; runner
snapshots do not record callbacks. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bt--bounded-long64-cvtdq2ps-xmm0rax-aligned-m128-memory-profile), [plan](../../docs/plans/STAGE_2.md#s2-37bt--bounded-long64-cvtdq2ps-xmm0rax-aligned-m128-memory-profile), and [report](../../docs/reports/STAGE_2_REPORT.md#s2-37bt--cvtdq2ps-xmm0rax-aligned-m128-memory-profile).

S2-37bu runs the local-only schema-v2 `CVTTPS2DQ XMM0,[RAX]` (`F3 0F 5B 00`)
profile. Literal/runner coverage checks RC-independent truncation, invalid and
inexact lane flag aggregation, sticky status preservation, pending x87
preservation, representative alignment/gate/schema/tamper cases, and JIT/native
denial. Direct ISA tests establish the full misalignment cross product and
callback atomicity; runner snapshots do not expose callback counts. See the
[CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bu--bounded-long64-cvttps2dq-xmm0rax-aligned-m128-memory-profile) and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37bu-cvttps2dq-xmm0rax-aligned-m128-memory-profile).

S2-37bv runs the local-only schema-v2 `CVTPS2DQ XMM0,[RAX]` (`66 0F 5B 00`)
profile. The runner covers four MXCSR rounding modes, tie/subnormal and signed
range edges, mixed IE+PE, sticky flags, aligned AC inputs at Ring0, schema
rejection, output tampering, and JIT/native Unsupported. Direct ISA tests own
all-CPL alignment and data callback assertions; runner snapshots do not count
callbacks. Physical x86 acceptance was not performed.

S2-37bw executes the local-only schema-v2 `CVTTPD2DQ XMM0,[RAX]` (`66 0F E6 00`)
profile through the interpreter. It reads one aligned m128 source, truncates
binary64 lanes toward zero before range checks, zeroes XMM0.high64, and records
masked sticky IE/PE. JIT/native remain Unsupported; no physical x86 acceptance
is implied. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bw--bounded-long64-cvttpd2dq-xmm0rax-aligned-m128-profile).

S2-37bx runs the local-only schema-v2 legacy `CVTPD2DQ XMM0,[RAX]`
(`F2 0F E6 00`) profile. It reads one aligned m128 source, rounds binary64
lanes according to MXCSR.RC before range checking, and reports masked IE/PE
sticky flags. Native/JIT/C ABI remain closed; physical x86 acceptance is not
claimed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bx--bounded-long64-cvtpd2dq-xmm0rax-aligned-m128-profile).

S2-37by runs exact legacy `CVTPS2PD XMM0,[RAX]` (`0F 5A 00`) through the
interpreter using one m64 read. Its fixture matrix covers NaN payloads,
subnormal widening, RC/FTZ independence, MXCSR sticky state and x87 canaries.
The integer oracle is independent of host FP; JIT/native remain Unsupported.
See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37by--bounded-long64-cvtps2pd-xmm0rax-m64-memory-profile).


### S2-37bz — CVTSS2SD m32 memory profile

The exact `F3 0F 5A 00` scalar form reads four bytes from RAX once and writes
only the low 64 bits of XMM0. It widens binary32 exactly, adding masked IE for
SNaN quieting and DE for nonzero subnormal input; it preserves all upper XMM0
bits, MXCSR control/sticky state, and x87 canaries. RC/FTZ do not affect the
result. Native/JIT/C ABI remain closed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37bz--bounded-long64-cvtss2sd-xmm0rax-m32-memory-profile).

### S2-37ca — CVTSD2SS m64 memory profile

The exact `F2 0F 5A 00` profile has a literal fixture and a RC×FTZ matrix over
WPR boundaries, overflow, NaN classes, infinities, zeros, and subnormals. The
runner verifies sticky status, high-XMM preservation, schema/tamper behavior,
and JIT/native denial. Direct ISA tests cover gates, no-SSE refusal, unaligned
single-read behavior, and read rollback. No physical x86 acceptance is claimed.
See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37ca--bounded-long64-cvtsd2ss-xmm0rax-m64-memory-profile).


### S2-37cb — CVTPD2PS aligned m128 profile

The exact `66 0F 5A 00` fixture profile reads one aligned 16-byte source,
narrows both lanes with RC/WPR/FTZ semantics, and remains interpreter-only. The
runner checks lane exception aggregation and strict schema/backend closure;
direct ISA tests cover callback count and alignment. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37cb--bounded-long64-cvtpd2ps-xmm0rax-aligned-m128-memory-profile).

### S2-37cc — CVTSI2SD m32

The runner dispatches exact `F2 0F 2A 00` fixtures to the local interpreter
profile and preserves backend closure. The matrix covers integer boundaries,
RC variation, and MXCSR=0/PM=0.


### S2-37cd — CVTSI2SD m64

The runner dispatches exact `F2 48 0F 2A 00` fixtures to the bounded
interpreter profile. Its matrix covers signed i64 boundaries, RC-dependent
rounding around 2^53, sticky PE, high-XMM and pending-x87 preservation, strict
schema/tamper checks, and backend denials. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37cd--bounded-long64-cvtsi2sd-xmm0qword-ptr-rax-m64-memory-profile).
