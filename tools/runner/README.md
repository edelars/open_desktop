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

## Stage 4 freestanding probe

Run a fixed raw image produced by the Stage 4 probe build:

```sh
cargo run --offline -p od-runner -- \
  --stage4-probe work/stage4/flat_probe/flat_probe.bin --budget 16 \
  --expect-nonce 0x1122334455667788
```

The image must be exactly 0x9000 bytes and satisfy the `Stage4FlatProbeV1`
layout accepted by `od-machine::load_stage4_flat_probe`. The runner reads the
image bytes from the named file, loads that image, and runs one bounded paged
CPU quantum through `PagedVcpuSession`. `--budget` accepts 0 through 1024
instructions and defaults to 16. `--expect-nonce QWORD` accepts decimal or
`0x` hexadecimal. Success requires HLT after exactly three retired instructions,
RAX equal to the expected nonce and UART output exactly its low byte. Without
an expectation HLT is `guest_halted`; an incorrect expectation yields
`completion_mismatch`. Both are nonzero.

`--cancel-after N` deterministically permits N instruction-boundary polls before
requesting STOP. Zero cancels before fetch; two preserves the load and OUT.
The result is `host_cancelled`, with committed state and UART output. Budget
exhaustion is `work_limit`. This is a deterministic cancellation source; real
VMHost/OS signal wiring belongs to S4-41. After the quantum, the owner drains
committed polling-UART TX once.

## Stage 4 guest UART echo

Build and run the strict one-byte guest echo image:

```sh
python3 tools/build/build_stage4_flat_probe.py \
  --profile uart-echo-probe --output-dir work/stage4/uart_echo_probe
cargo run --offline -p od-runner -- \
  --stage4-uart-echo-probe work/stage4/uart_echo_probe/flat_probe.bin \
  --rx-byte 0xff --expect-byte 0xff
```

The image must satisfy `Stage4UartEchoProbeV1` and contain only `IN AL,DX`,
`OUT DX,AL`, `HLT`. `--rx-byte` and `--expect-byte` each accept decimal or
`0x` hexadecimal bytes. RX travels through the session's bounded RX event path;
the expected byte is an independent host-side predicate. Success requires the
guest's RAX, exact retirement/RIP and actual drained UART output to match. With
no injected RX byte, reading the empty RBR returns the existing `pio_error`.
See the [S4-24d report](../../docs/reports/STAGE_4_S4-24D_2026-11-07.md).

Standard output contains one deterministic JSON object. It includes
`schema_version`, `profile`, `status`, `exit`, `retired`, `rip`, `activity`,
`rflags_defined`, `serial_bytes`, `observed_rax`, `observed_data_qword`, and `error`. Values
that do not exist before machine handoff are `null`; guest integers are
hexadecimal strings and serial output is an array of byte values.

Exit codes are:

| Code | Meaning |
| --- | --- |
| `0` | `guest_completed`: HLT and the expected retirement count, nonce and UART byte match. |
| `3` | Invalid arguments, unreadable image, or image size mismatch. |
| `4` | Image rejected by the Stage 4 loader. |
| `5` | HLT without proof, completion mismatch, cancellation, work limit, guest fault, backend error, or other non-PIO execution failure. |
| `6` | Port-I/O failure. |

Execution failures emit a stable error category and diagnostic detail in
`error`. HLT without a completion expectation or with a mismatched expectation
returns code `5` with `error: null`; use `status` (`guest_halted` or
`completion_mismatch`) to identify that failed completion predicate. The runner
does not dispatch guest opcodes itself.

## Stage 4 store/load probe

Build and run the distinct store/load artifact:

```sh
python3 tools/build/build_stage4_flat_probe.py \
  --profile store-load-probe \
  --initial-value 0x0102030405060708 \
  --store-value 0x1122334455667788 \
  --output-dir work/stage4/store_load_probe
cargo run --offline -p od-runner -- \
  --stage4-store-load-probe work/stage4/store_load_probe/flat_probe.bin \
  --budget 16
```

The artifact contains separate initial and store-value qwords. The loader uses
the store-value only to initialize the documented `RCX` handoff; the guest
`MOV [RBX],RCX` performs the RAM write. A successful JSON result has profile
`Stage4StoreLoadProbeV1`, `retired: 4`, `exit: "halted"`, data and RAX equal to
the store-value, `rflags_defined: "0x0000000000000cd7"`, and one serial byte.
Budgets 1/2/3 report the committed store/load/UART prefixes and do not claim
guest completion.

## Stage 4 stack probe

Build and run the real freestanding `PUSH RCX; POP RAX; OUT DX,AL; HLT`
profile:

```sh
python3 tools/build/build_stage4_flat_probe.py \
  --profile stack-probe \
  --payload 0x1122334455667788 \
  --output-dir work/stage4/stack_probe
cargo run --offline -p od-runner -- \
  --stage4-stack-probe work/stage4/stack_probe/flat_probe.bin \
  --budget 4
```

The runner reports the payload observed in the guest stack and `RAX`, the
restored `RSP`, retirement count, UART bytes and the bounded quantum exit. The
profile is local interpreter evidence and does not claim exception delivery,
IRQ/APIC, HLT wakeup, Linux or full S4-32 acceptance.

## Versioned fixture results

Run a JSON fixture once per engine. The result is the only stdout content; diagnostics go to stderr.

```sh
cargo run --offline -p od-runner -- \
  --fixture tests/fixtures/x86/s0_09_add_overflow.case.json \
  --engine interpreter --json
cargo run --offline -p od-runner -- \
  --fixture tests/fixtures/x86/s0_09_add_overflow.case.json \
  --engine jit --json
cargo run --offline -p od-runner -- \
  --fixture tests/fixtures/x86/s0_09_add_overflow.case.json \
  --engine jit-dispatch --json
```

Compare two saved results against the exact fixture used to produce them:

```sh
target/debug/od-runner --compare-results interpreter.json jit.json \
  --case tests/fixtures/x86/s0_09_add_overflow.case.json --json
```

Schema v1 comparison JSON contains `field`, optional `mask`, `expected`, `actual`, and fixture/backend `context` for every difference. Schema v2 comparison reports contain `schema_version`, `case_id`, `input_sha256`, `matches`, and differing field names. The `--fixture` and `--compare-results` commands use exit codes `0` for a completed run or comparison match, `2` for a comparison mismatch, `3` for invalid input, `4` for an unsupported operation or host, and `5` for a harness failure or deadline. Unsupported engine results remain valid JSON and never appear as completed runs. Unknown CPU model, kernel, page size, compiler version, and source revision are represented as unavailable metadata; the CLI does not guess them.

## Integer diagnostic trace and replay

The opt-in `--diagnostic CASE.json` path records up to 64 single-step records
for schema-v2 `s2-long-integer-v1` fixtures. Each record includes integer state
before and after execution, the defined RFLAGS mask, retirement count, outcome,
faulting RIP when applicable, and bounded byte-level changes to fixture RAM.
Use `--steps 1` for one step (the default), or `--steps 0` for an empty trace;
`--max-event-bytes` caps changed memory bytes recorded, up to 2048. The input
fixture is limited to 512 KiB and mapped RAM to 64 KiB. JIT and native diagnostic
requests and every other profile are rejected explicitly.

The output embeds the fixture bytes and limits needed for deterministic replay:

```sh
cargo run --offline -p od-runner -- --diagnostic CASE.json --steps 16 > trace.json
cargo run --offline -p od-runner -- --diagnostic-replay trace.json
```

Replay recomputes the trace and rejects a changed request or result. This
interpreter diagnostic does not provide JIT support or broaden the established
fixture profile. Trace and replay commands return success on success and a
nonzero process status on invalid input, unsupported requests, or replay
mismatches; error details go to stderr.

The fixture command accepts `interpreter`, `jit`, `jit-block`, or `jit-dispatch` per invocation and dispatches strictly by schema version. `jit-block` runs one bounded block; `jit-dispatch` iteratively runs admitted blocks with an exact remaining budget. Both have distinct result backend tags and are opt-in on fixture schema v1 only. Unsupported forms and host errors do not trigger interpreter fallback. Schema v2 continues to deny JIT execution. The S0-09 v1 fixture contract and digests remain unchanged; it does not capture RAM or exceptions, and its in-process path has no wall-time watchdog. The native x86 runner enforces its wall-time limit in an isolated process.

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


### S2-37ce — CVTSI2SS m32

The runner dispatches exact `F3 0F 2A 00` fixtures to the local
`s2-long-cvtsi2ss-memory32-rax-v1` interpreter profile. The independent oracle
reads signed i32 from fixture memory and rounds directly to binary32. Matrix
coverage spans all RC modes, integer precision boundaries, old PE histories,
high96/x87 preservation, strict schema/tamper checks and JIT/native denial.
See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37ce--bounded-long64-cvtsi2ss-xmm0dword-ptr-rax-m32-memory-profile).

### S2-37cf — CVTSI2SS m64

The runner dispatches exact `F3 48 0F 2A 00` fixtures to the local interpreter
profile. Its 112-case matrix spans seven signed i64 boundaries and midpoint
vectors across all RC modes, two supported MXCSR mask settings, and incoming PE
histories. The independent oracle observes direct-to-binary32 rounding,
high96/x87 preservation, gates, schema/tamper rejection, AC refusal, and JIT/
native denial. The literal is the double-rounding guard value
`0x0040000040000001`. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37cf--bounded-long64-cvtsi2ss-xmm0qword-ptr-rax-m64-memory-profile).

### S2-37cg — CVTSD2SI EAX,m64

The runner dispatches exact `F2 0F 2D 00` fixtures to the local schema-v2
interpreter profile. It reads the eight-byte source at original RAX, rounds to
signed i32 using MXCSR.RC before range checking, and verifies EAX zero-extension
back to RAX. Its independent oracle reads fixture memory directly and uses raw
binary64 bit decomposition. The runner matrix validates RC/boundary/flags,
subnormal no-DE, x87 preservation, gates, AC refusal, AC plus a noncanonical
span, strict schema and backend closure. JIT/native results are expected
Unsupported; no physical x86 execution is claimed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37cg--bounded-long64-cvtsd2si-eaxqword-ptr-rax-m64-to-r32-memory-profile).

### S2-37ch — CVTSS2SI EAX,m32

The runner dispatches exact `F3 0F 2D 00` to the local schema-v2 interpreter
profile. The independent oracle reads raw binary32 from fixture memory and
rounds before i32 range checking. Its matrix checks all RC modes, signed limits,
rounding ties, subnormal/NaN/infinity behavior, sticky flags, RAX zero-extension,
x87 preservation, strict schema, tampering and backend closure. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37ch--bounded-long64-cvtss2si-eaxdword-ptr-rax-m32-to-r32-memory-profile).

### S2-37ci — CVTTSS2SI EAX,m32

The schema-v2 fixture runner has a separate interpreter-only profile for exact
legacy `F3 0F 2C 00`, reading one binary32 from `[RAX]`. Its independent oracle
uses raw binary32 bits and integer truncation toward zero, regardless of
MXCSR.RC, then checks signed i32 range and sticky IE/PE. The literal fixture
uses +1.5 with round-up selected to distinguish truncation from CVT rounding.
JIT/native requests return `unsupported`; no physical x86 execution is claimed.
See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37ci--bounded-long64-cvttss2si-eaxdword-ptr-rax-m32-to-r32-memory-profile).

### S2-37cj — CVTTSD2SI EAX,m64

The runner has a separate strict schema-v2 interpreter-only profile for exact
`F2 0F 2C 00`. Its independent oracle reads raw u64 memory bits, truncates
binary64 toward zero, then checks i32 range and sticky IE/PE; it does not use
host floating point or production conversion helpers. The literal +1.5 case
selects round-up in MXCSR and still expects EAX=1+PE. JIT/native return
`Unsupported`; this does not claim physical x86 acceptance. See the [CPU
contract](../../docs/specs/CPU_CONTRACT.md#s2-37cj--bounded-long64-cvttsd2si-eaxqword-ptr-rax-m64-to-r32-memory-profile).

### S2-37ck — CVTSD2SI RAX,m64

The fixture runner dispatches exact `F2 48 0F 2D 00` through its separate
schema-v2 interpreter profile. The oracle reads raw binary64 memory bits,
rounds using MXCSR.RC, checks signed-i64 range, and validates full RAX output,
sticky status, and source aliasing without host FP. Focused runner validation
passed 1/1; JIT/native backends remain denied. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37ck--bounded-long64-cvtsd2si-raxqword-ptr-rax-m64-to-r64-memory-profile).


### S2-37cl — CVTSS2SI RAX,m32

The fixture runner supports only exact `F3 48 0F 2D 00` through a separate strict interpreter profile. The testkit oracle covers 20 source vectors × 4 RC modes × 4 IE/PE histories (320 comparisons); AC=1, malformed controls, tampering, and backend denial are also checked.

### S2-37cm — CVTTSS2SI RAX,m32

The runner has a separate strict interpreter profile for exact
`F3 48 0F 2C 00`. A literal -1.5 source with MXCSR round-down proves truncation
toward zero and the full 64-bit RAX result. The independent oracle covers 320
comparisons across raw values, all RC values, and incoming IE/PE histories;
malformed/foreign controls, AC refusal, tampering, and JIT/native denial are
checked. This is Ring0 fixture evidence, not physical-x86 acceptance. See the
[CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37cm--bounded-long64-cvttss2si-raxdword-ptr-rax-m32-to-r64-memory-profile).

### S2-37cn — CVTTSD2SI RAX,m64

The runner has a separate strict interpreter profile for exact legacy
`F2 48 0F 2C 00`. Its integer-only binary64 oracle truncates toward zero,
checks signed-i64 range, preserves sticky flags, and verifies full-width RAX
output. The Ring0 matrix covers 304 comparisons across raw values, RC modes,
and incoming IE/PE histories; malformed/foreign controls, tampering, and
backend denial are also checked. AC=1 refusal is covered by direct ISA tests.
No physical x86 acceptance is claimed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37cn--bounded-long64-cvttsd2si-raxqword-ptr-rax-m64-to-r64-memory-profile).

### S2-37co — COMISD XMM0,m64

The runner admits the separate strict profile `s2-long-comisd-memory-rax-v1`
for exact legacy `66 0F 2F 00`. Its raw-bit oracle compares XMM0.low64 with
one eight-byte m64 read, checks COMISD relation flags, applies NaN-priority
IE/DE status, and verifies preservation of the remaining state. The focused
fixture passed 1/1 with 192 matrix comparisons across relations, RC/FTZ
settings, and prior sticky histories. AC=1 refusal is covered by both the
runner fixture and direct ISA; span faults and read-failure atomicity are
covered by direct ISA tests only.
Native/JIT/C/fragment/generic execution remains closed; the runner result is
host-side evidence and does not claim physical x86 execution. See the [CPU
contract](../../docs/specs/CPU_CONTRACT.md#s2-37co--bounded-long64-comisd-xmm0qword-ptr-rax-m64-memory-profile).

### S2-37cp — UCOMISD XMM0,m64

The runner admits the separate strict profile `s2-long-ucomisd-memory-rax-v1`
for exact legacy `66 0F 2E 00`. Its raw-bit oracle compares XMM0.low64 with
one eight-byte m64 read and checks relation flags, UCOMISD's QNaN-no-IE and
NaN-suppresses-DE behavior, sticky status, and remaining-state preservation.
The focused fixture passed 1/1 with 224 comparisons across relation/class
pairs, RC/FTZ settings, and prior IE/DE histories. The runner checks a valid
baseline foreign-field rejection and JIT denial. CP direct tests cover AC,
missing-SSE, span and read-failure refusals; malformed-MXCSR runtime refusals
are covered by the shared CO direct preflight tests. The runner matrix uses
Ring0; direct numeric checks cover CPL0–3 while other direct probes use Ring0.
Native/JIT/C/fragment/generic execution remains closed. See the [CPU
contract](../../docs/specs/CPU_CONTRACT.md#s2-37cp--bounded-long64-ucomisd-xmm0qword-ptr-rax-m64-memory-profile).

### S2-37cq — COMISS XMM0,m32

The runner accepts the separate profile `s2-long-comiss-memory-rax-v1` only
for exact legacy `0F 2F 00`. Its independent raw-binary32 oracle checks one
four-byte source read, relation flags, COMISS IE for QNaN/SNaN, NaN-before-DE
priority, sticky MXCSR status, and remaining state. The focused fixture passed
1/1 with 224 matrix comparisons across relation/class pairs, RC/FTZ settings,
and prior IE/DE histories. Shared COMISD/UCOMISD-path regressions passed: the
COMISD runner 1/1 (192 comparisons) and UCOMISD runner 1/1 (224). AC=1
refusal, foreign-control rejection, and JIT denial are covered; checked-span faults and read-failure atomicity are covered
by direct ISA tests. The full runner suite passed 112 unit tests and 473
fixtures. Native/JIT/C/fragment/generic execution remains closed; this
host-side result is not physical x86 evidence. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37cq--bounded-long64-comiss-xmm0dword-ptr-rax-m32-memory-profile).

### S2-37cr — UCOMISS XMM0,m32

The runner admits `s2-long-ucomiss-memory-rax-v1` only for exact legacy
`0F 2E 00`. Its independent raw-binary32 oracle checks a four-byte source
read, relation flags, UCOMISS QNaN/SNaN IE priority, NaN-before-DE priority,
sticky MXCSR status, and remaining state. The focused matrix passed 1/1 with
288 comparisons (nine relation/class pairs, four RC values, two FTZ settings,
four incoming IE/DE histories). AC=1 refusal, valid-baseline foreign-field
rejection, and JIT denial are covered by the runner; checked-span and
read-failure atomicity are covered by direct ISA tests. Native/JIT/C/fragment/
generic execution remains closed. Full package suites passed: od-isa 1,674,
od-testkit 453, od-runner 112 unit + 474 fixtures, and od-x86-runner 8+4;
workspace all-target check and layout passed. Independent code/schema and final docs/evidence reviews passed. This is not physical x86 evidence. See the
[CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37cr--bounded-long64-ucomiss-xmm0dword-ptr-rax-m32-memory-profile).

### S2-37cs — CMPSS XMM0,m32,imm8

The strict `s2-long-cmpss-memory-rax-v1` profile admits exact
`F3 0F C2 00 pp` for pp=0..7. The independent raw-bit oracle checks the eight
predicate results and sticky IE/DE behavior across 1,024 predicate/relation/
RC/FTZ/history combinations, plus 48 NaN/denormal-priority cases. The matrix
includes QNaN IE only for predicates 1/2/5/6, SNaN IE for all predicates,
NaN suppression of DE, and subnormal DE in the absence of NaNs. A literal
QNaN/subnormal case checks unordered output and pending-x87/RFLAGS preservation.

The runner also checks AC=1 refusal, full-fetch pp8 local refusal before
missing-SSE/TS gate faults, valid-baseline foreign-control rejection, and JIT
denial. It uses Ring0; CPL0–3 coverage belongs to the direct ISA relation
matrix. Decoder and direct ISA checks cover span and read-failure atomicity.
`pp>7` is a local unsupported-profile result, not an architectural `#UD`.
Native/JIT/C/fragment/generic execution remains closed; no physical x86
acceptance is claimed. Full suites passed: od-isa 1,678, od-testkit 453,
od-runner 112 unit + 475 fixtures, and od-x86-runner 8+4; workspace all-target
check, formatting, layout, Graphify (10,615/22,791/311), diff, and 13 anchors
passed. Independent code/schema and docs/evidence review passed. See the
[CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37cs--bounded-long64-cmpss-xmm0m32-raximm8-memory-profile).

### S2-37ct — CMPSD XMM0,m64,imm8

The strict interpreter fixture profile `s2-long-cmpsd-memory-rax-v1` uses
exact legacy `F2 0F C2 00 pp`, with pp=0..7. The raw-binary64 oracle checks
1,080 cases: 1,024 predicate/relation/RC/FTZ/sticky-history comparisons and
56 NaN/denormal-priority comparisons. It preserves XMM0.high64, RFLAGS, and
pending x87 while changing only XMM0.low64 and applicable IE/DE sticky bits.
The runner matrix is Ring0; direct predicate/relation ISA coverage spans
CPL0–3. Direct ISA checks also cover valid-predicate #UD/#NM ordering, absent
SSE state, AC=1, malformed MXCSR profile controls, full-fetch pp>7 refusal,
truncated immediate, canonical span, and read-failure rollback. The runner
checks AC=1 and pp>7 local Unsupported outcomes, including pp8 before missing
SSE/TS gates, plus foreign-field and JIT denial. Values above seven are local
profile refusals, not architectural `#UD`. Full validation passed: od-isa
1,680, od-testkit 453, od-runner 112 unit + 476 fixtures, od-x86-runner 8+4,
and the workspace all-target check. Formatting, layout, Graphify
(10,632/22,836/310), diff, and 13 anchors passed. Independent code/schema and
final docs/source/evidence reviews passed. Native/JIT/C/fragment/generic
execution remains closed; no physical x86 acceptance is claimed. See the
[CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37ct--bounded-long64-cmpsd-xmm0m64-raximm8-memory-profile).

### S2-37cu — CMPPD XMM0,m128,imm8

The fixture runner admits the exact local-only `s2-long-cmppd-memory-rax-v1`
profile. Its independent raw-bit oracle checked 1,040 packed-lane cases: 1,024
predicate/relation/RC/FTZ/sticky-history comparisons plus 16 QNaN/subnormal
lane-order checks. The literal verifies QNaN in one lane does not suppress DE
from a subnormal in the other. Aligned AC=1 is admitted; misaligned AC=1
produces `#GP(0)`. The runner also checks pp8 local refusal, foreign-control
rejection, output tampering, and JIT/native denial. Runner cases use Ring0; direct ISA tests cover predicate/relation/alignment
across CPL0–3, with Ring0-only gate/span/read-failure probes. Full suites
passed: 112 runner unit + 477 fixture tests; native execution and physical
x86 acceptance are not claimed. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37cu--bounded-long64-cmppd-xmm0m128-raximm8-memory-profile).

### S2-37cv — CMPPS XMM0,m128,imm8

The runner admits the strict interpreter-only `s2-long-cmpps-memory-rax-v1`
profile for exact `0F C2 00 pp`, pp=0..7. Its independent raw-binary32
oracle checks 448 cases: 256 predicate/RC/FTZ/sticky-history cases and 192
QNaN/SNaN/subnormal lane-priority cases. Additional probes cover aligned
AC=1, pp8 local Unsupported, foreign-control rejection, output tampering,
and JIT/native denial. The runner matrix uses Ring0; direct relation and
alignment tests cover CPL0–3, while direct gate, fetch, span, and read-failure
probes use Ring0. Full fetch precedes local pp refusal and gates. There is no
physical x86 acceptance claim. Full validation passed: 112 runner unit + 478
fixture tests, plus od-isa 1,692, od-testkit 453, and od-x86-runner 8+4;
workspace all-target check passed. Independent code/schema and final
docs/evidence reviews are complete. See the [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37cv--bounded-long64-cmpps-xmm0m128-raximm8-memory-profile).


### S2-37cw — MINSD XMM0,m64 [RAX]

The runner accepts the strict local-only `s2-long-minsd-memory-rax-v1` profile for exact `F2 0F 5D 00`. The raw-u64 oracle matrix contains 144 checks across operand classes, RC values, and prior IE/DE stickies. Additional runner checks cover AC=1 local refusal (including AC-before-bad-span), output tampering, foreign-control rejection, and JIT/native denial. The numerical matrix runs at Ring0. Direct ISA tests cover gates, span and read rollback. The native-local-only classifier passed 1/1. Full fetch and gates precede span validation/read. Full suites passed: od-isa 1,697; od-testkit 453; od-runner 112 unit + 479 fixture tests; od-x86-runner 8+4. Workspace all-target check, fmt, layout, diff check, and 16 links passed; Graphify refreshed to 10,711/23,033/308. Independent code/schema and final docs/evidence reviews are complete. No physical x86 acceptance is claimed. See [plan](../../docs/plans/STAGE_2.md#s2-37cw--exact-legacy-long64-minsd-xmm0m64-rax), [CPU contract](../../docs/specs/CPU_CONTRACT.md#s2-37cw--bounded-long64-minsd-xmm0m64-rax-memory-profile), [report](../../docs/reports/STAGE_2_REPORT.md#s2-37cw--minsd-xmm0m64-rax), and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37cw--minsd-xmm0m64-rax).


### S2-37cx — MAXSD XMM0,m64 [RAX]

The fixture runner accepts exact `F2 0F 5F 00` under `s2-long-maxsd-memory-rax-v1` and invokes the independent raw-u64 oracle. Its focused fixture compares 160 combinations of numeric ordering/ties, NaNs, subnormals, RC values, and old IE/DE histories. AC=1 local refusal is exercised in the runner; span/read rollback and gate paths are covered by direct ISA tests. JIT/native return Unsupported, and the native-local-only classifier passed 1/1. Focused decoder/direct ISA is 3/3, runner 1/1, od-testkit test-target check and native-local-only classifier 1/1 passed. Full suites passed: od-isa 1,702, od-testkit 453, od-runner 112 unit + 480 fixtures, and od-x86-runner 8+4; workspace all-target check passed. Graphify refreshed to 10,738 nodes / 23,093 edges / 311 communities; formatting, layout, diff, and 12 links passed. Independent code/schema and final docs/evidence reviews are complete and clean. No physical x86 acceptance is claimed.

### S2-37ds — MULPS XMM0,m128 [RAX]

The runner accepts only exact legacy `0F 59 00` under
`s2-long-mulps-memory-rax-v1`. The interpreter profile uses SSE bit 25,
masked-only MXCSR, one aligned canonical m128 read and the independent raw-bit
`independent_mulps` fixture oracle. All RC and prior sticky histories are
covered; JIT/native and unmasked/#XM paths remain closed. See the [CPU
contract](../../docs/specs/CPU_CONTRACT.md#s2-37ds--masked-only-legacy-long64-mulps-xmm0m128-rax)
and [fixture notes](../../tests/fixtures/x86/stage2/README.md#s2-37ds--mulps-xmm0m128-rax).

### S2-38x — UCOMISD m64 invalid-only memory profile

The runner admits the separate strict profile `s2-long-ucomisd-memory-rax-invalid-v1`
for exact `66 0F 2E 00`. Its raw-u64 oracle keeps QNaN quiet and raises IE only
for SNaN; an unmasked SNaN suppresses EFLAGS/RIP/retirement and selects `#XM`
or arithmetic `#UD` from OSXMMEXCPT. After one successful read, any subnormal
operand is atomic Unsupported. The focused matrix covers both NaN positions
and classes × IM × OSXMMEXCPT × all 64 old sticky states (1,024 combinations),
plus RC, finite boundaries, schema/result tampering, and interpreter-only
backend enforcement. Direct ISA tests cover CPL0–3. No physical x86 execution is
claimed; see the [standalone report](../../docs/reports/S2_38X_UCOMISD_MEMORY_INVALID_ONLY.md).

## Linux direct boot loader (S4-37L6)

`linux-direct` loads a Linux bzImage, an initramfs and the ACPI blob (version 2) onto the
Linux-profile board and writes JSON evidence. Without `--run` it does NOT start the guest:
`retired` is 0 and `guest_started` is false.

```sh
cargo run --offline -p od-runner -- linux-direct \
  --vmlinuz work/stage4/linux-7.0.0-38.38/vmlinuz-7.0.0-38-generic \
  --initramfs work/stage4/initramfs/initramfs.cpio \
  --acpi-blob tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin \
  --nonce <token> --ram-mib 128 --out work/stage4/linux_direct/evidence.json \
  --expected-vmlinuz-sha256 9f55ef7253055a9cf68f8a04323c93dead2fca617386d842fbde9ca7b178b27e \
  --expected-acpi-manifest-hash 2bd690ae0817705b1b128675f9628dd7212fee74ab97c88da3599a1ae8e398f2 \
  --expected-initramfs-sha256 <hex>
```

All flags are required (`--expected-initramfs-sha256` too, like the kernel hash); a flag value
may not start with `--`. `--ram-mib` is at most 8192: the board still builds its firmware boot
tables with 4 KiB leaves, which takes minutes and gigabytes on a larger RAM (they are dropped for
the Linux profile; building them only for the firmware profile is a known debt).

`--expected-acpi-manifest-hash` must be the manifest hash of the board this command assembles
(ECAM, PM block and ACPI tables, no NVMe; `2bd690ae...` is the hash of the fixture blob). od-machine
computes the board's own hash and refuses a different expected hash as `acpi_manifest_mismatch`
before it looks at the blob; the blob is then validated against the board's hash.

The command line is `console=ttyS0,115200 earlycon=uart8250,io,0x3f8 8250.nr_uarts=1
od.nonce=<nonce> nokaslr`; the nonce is whatever the operator passes (`[A-Za-z0-9._-]`, at most 64
characters). Exit codes: 0 loaded, 3 invalid input (arguments, unreadable file or unwritable
`--out`), 4 refused. A refusal from the loading pipeline (exit 4, and an unreadable input file
after arguments were accepted) prints and writes `status: refused` to `--out` with `error.code`,
`error.stage` and `error.detail`; an argument error and an unwritable `--out` (exit 3) are only
printed, since there is no valid place to write them. Codes in order of checks:
`invalid_input` (exit 3), `vmlinuz_sha256_mismatch`, `initrd_too_large` (file larger than RAM,
by file size), `initramfs_sha256_mismatch`, `bzimage_truncated`, `bzimage_payload_out_of_file`,
`bzimage_header_invalid`, `board_assembly_failed` (invalid board configuration, also later in
assembly), `acpi_manifest_mismatch`, `acpi_blob_invalid`, `cmdline_invalid`, `cmdline_too_long`,
`initrd_too_large` (placement), `placement_invalid`, `e820_invalid`, `boot_params_invalid`,
`e820_handoff_mismatch`, `guest_memory_access`, `load_failed` (a sanity limit of the loader
itself, for example page tables that do not fit the zone; not an operator input), and
`entry_state_invalid`. `bzimage_header_invalid` also appears after placement: a protected-mode
code size smaller than the 64-bit entry offset 0x200 is refused as `EntryOutsideKernel`. The evidence lists the input hashes and sizes, the placement plan and the entry registers. The
E820 map, `acpi_rsdp_addr`, the command line (`cmdline_in_memory`), the RSDP signature
(`acpi_rsdp_signature_in_memory`, `"RSD PTR "`) and the SHA-256 of the kernel code and initrd
bytes (`memory_sha256`, with `*_matches_file`) are read back from guest memory, not copied from
the inputs.

### `--run --budget N` (S4-38L1)

`--run --budget <n>` (a bare `--run` and a value flag, one needs the other) runs the loaded board for
at most `n` instructions after loading, through `od_machine::run_linux_direct_board`: the 2 MiB
walk (`run_paged_probe_2m_ram`, not the 4 KiB walker), the base CPU profile (no CPUID/MSR; the
allowlist is not widened) and the board CR3, with the PIT, PM_TMR and RDTSC ticks of the Linux
profile applied as in `run_boot`. The evidence gets a `stop` object, `guest_started` becomes true and
`retired` is the count of the run; every other field is as without `--run`. `guest_started` means
"the run was requested by `--run`" (also with `--budget 0`), not that an instruction retired; read
`retired` for that. A failure of the run itself (`run_failed`, status `refused`, exit 4; only a
virtual-clock overflow today) keeps `guest_started: true` and reports `retired` as the count known
at the error (`null` plus a note in the detail when unknown). The budget is plain decimal digits
(a leading `+` is `invalid_input`). `stop` holds: `exit_kind`
(`budget_exhausted`, `halted`, `unsupported` with `unsupported_opcode`, `fault`, `backend`,
`shutdown`, ...), `rip`, `rip_gpa`, `rip_bytes` (16 bytes at RIP, each byte translated through the
same walk without writing accessed/dirty bits), `mode`, `cpl`, `cr0`, `cr3`, `cr4`, `efer`, `rflags`,
`gpr`, `retired`, `quanta`, `virtual_ns`, `tsc_hz`, COM1 (`com1_hex`, `com1_text`), `cpu_profile`,
`linux_profile`, `page_fault`, `memory_error` and the input hashes. `cpu_profile` is the base
paged profile (`PagedCpl0ProbeV1`, no CPUID/MSR) with one addition: under the Linux profile a bare
`RDTSC` is executed (`step_paged_probe_with_rdtsc`, 1 us tick as in `run_boot`). `cpl` is the RPL
of the CS selector (0 when absent) while the interpreter always runs Ring0; `rip_bytes` reads any
region with backing (ROM and framebuffer too), byte by byte, so the window may cross a 4 KiB
boundary that a CPU fetch never crosses. On the real kernel, with the `cpuid` member (S4-38L4), the one-byte read of the set's adapter (S4-38L4a) and POPFQ on CPL0 (S2-50), a run with `--budget 50000` and nonce `s4-38l4a` runs past `MOV RAX, CR3` (`0f 20 d8`, S4-38L4c: the set reads CR3 under `cpu_owned_cr3`) and the partial `REP MOVSQ` at `0x100_02aa` (S4-38L4b, resumed by the board). The multi-byte NOPs (`0F 1F /0`, S4-38L4d) retire in the set. The 6-byte `JE rel32` (`0f 84 1f 02 00 00`) that crosses the 4 KiB boundary `0x543_9000` is fetched across the two pages and executes (S4-38L4e). The 16-bit read `MOVZX r32, word [rcx]` (`0f b7 31`) is admitted by the set's adapter (S4-38L4f). `REP BSF` (`f3 0f bc c0`) executes as BSF on the profile without BMI1 (S4-38L4g), the 8-byte store across the 4 KiB boundary (`48 89 79 04`) is written by the set's adapter (S4-38L4h), and the guest `#PF` at linear `0x102_0020` is delivered through the guest IDT: the boot page-fault handler maps the page and returns with `IRETQ` (S4-38L5c). With `--budget 3000000` the run stops `unsupported` at `MOV RBX, CR2` (`0f 20 d3`) on RIP `0x543_c35e` after `retired` 390860: the set models `MOV RAX, CR2` only (S4-38L5d, next slice). Evidence: `docs/reports/data/stage4/s4-38l5c-pf-delivery-run.json` (earlier: `s4-38l4h-cross-store-run.json`). A budget of 50000 is exhausted earlier (`budget_exhausted`, RIP `0x543_9a87`). Evidence: `docs/reports/data/stage4/s4-38l4f-word-run.json` (earlier: `s4-38l4e-cross-page-run.json`, `s4-38l4d-nop-run.json`, `s4-38l4c-mov-cr3-run.json`, `s4-38l4b-rep-run.json`). Before S4-38L4b this run was `refused` at the partial REP (`docs/reports/data/stage4/s2-50-popf-run.json`). The instruction count depends on the nonce length (the cmdline carries it). Before this run stopped at POPFQ `9d` on `0x1000292` with `retired` 1379 (`docs/reports/data/stage4/s4-38l4a-core.json`). Before it the null segment loads (S2-46), the ENDBR64 and LGDT/LRETQ forms (S2-47, S2-48), the narrow stores (S4-38L3), `LIDT` (S2-49), `CPUID` 0x8000_0000 (S4-38L4) and `CMP byte [RDI+0x1EF], 0` (S4-38L4a) all retire. The CPUID boundary for leaves 1 and 0x8000_0001 is not reached on this run. Evidence: `docs/reports/data/stage4/s4-38l4a-core.json` (earlier: `s4-38l4-core.json` at budget 1000, `s4-38l3-core.json`).

### `--run --budget N --set-cpl0` (S4-38L3, S4-38L4)

`--set-cpl0` (needs `--run`) runs the board through the CPL0 profile set of od-isa instead of the base
probe profile (`od_machine::run_linux_direct_board_set`, members `LinuxCpl0SetMembers::linux_default()`).
The members are, in `stop.set_members` order, `cli`, `cpu_owned_cr3` and `cpuid:DiagnosticV1` (S4-38L4;
before it the set had `cli` and `cpu_owned_cr3` only). A `MOV CR3` continues the run (`cr3_changes`,
`last_cr3_change`), `stop.path` is `cpl0_set` and `stop.set_exit_detail` is the set's own final exit.
No other member (`rdtsc`, `msr`, `halt`, `pio_u8`) is on by default; each is added by the slice whose
first stop needs it.

CPUID boundary (S4-38L4, checkpoint C7 of the plan). With the `cpuid` member a `CPUID` of leaf 0, 0x80000000
or any other leaf except the two below is answered by DiagnosticV1 (vendor `OpenDesktop`, zero feature
masks). A `CPUID` whose EAX is 1 or 0x80000001 is not answered: the run stops BEFORE retiring it with
`stop.exit_kind = "cpuid_divergence"`, `stop.cpuid_divergence_leaf` (`"0x1"` or `"0x80000001"`; `null` for
every other exit), `stop.rip` on the `CPUID` (or on its legacy/REX prefix), `retired` counting the
instructions before it, and no register changed (`set_exit_detail` is `CpuidDivergence { leaf: .. }`).
This is an honest edge, not a substitution: the DiagnosticV1 feature words for those leaves are not the
target profile's, so they are neither supplied nor is the kernel patched. Whatever a run does past a
`cpuid_divergence` stop is diagnostic only. Without the `cpuid` member `CPUID` stays `unsupported`
(`0xa2`). The set executes only a bare `0F A2`: a prefixed `CPUID` (`66`, `48`, `F3`, ...) with a leaf
other than the two above stays `unsupported` (`0xa2`); the boundary still sees a prefixed leaf 1 or
0x80000001 and stops there. `schema_version` stays 1 (`cpuid_divergence_leaf` and the new `exit_kind` are additive).
