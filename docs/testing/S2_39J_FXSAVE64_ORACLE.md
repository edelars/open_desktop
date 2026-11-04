# S2-39j — successful CPL3 FXSAVE64 oracle protocol

**Status:** normative protocol for the separate S2-39k independent harness.
Cases A/B/A/B passed on Linux x86_64 on 2026-11-04 after linked-code/ABI
inspection; see the [acceptance report](../reports/S2_39K_LINUX_ACCEPTANCE.md).
Raw `execution_kind=unknown` is preserved; separately established physical
acceptance, parent S2-39 and Stage 2 remain open. This protocol document
does not itself add fixture admission, CPU implementation or JIT support.

## Scope and sources

The sole instruction under test is the existing S2-39d Long64 encoding
`48 0F AE 40 40`, `FXSAVE64 [RAX+0x40]`, length five, at CPL3. This protocol
covers successful completion on a supported FXSR/SSE long-mode host whose OS
enables the required FP/SSE state. Record CPU capabilities and OS environment;
an unsuitable host is unavailable coverage, not an architectural test failure.
There are no EM/TS/OSFXSR gate variations, faults, privileged instructions,
FXRSTOR, or save/restore round-trip cases here.

The existing [S2-39a evidence](../reports/STAGE_2_LINUX_PROFILE.md#s2-39a--fxsave-evidence-only)
pins Intel SDM Rev.093 (September 2026), Vol.2A pp.3-409–3-419, Tables
3-45–3-49, and Vol.1 §§10.5.1–10.5.3, 11.6.5–11.6.6. The
[S2-39d contract](../specs/CPU_CONTRACT.md#s2-39d--exact-long64-fxsave64-rax0x40)
provides the bounded local form. Those existing references are reused here;
this documentation slice does not claim a fresh PDF verification. Before
harness admission, review the concrete setup/observation instruction bytes
and completion mechanism against those sources and the host ABI.
The Ubuntu source/image mapping proves static reachability, not execution of
this instruction or Linux boot.

## Independent inputs

Each repetition starts from fresh literal inputs, not a prior save image.
Let `D` be the actual allocated destination: record its address, require
16-byte alignment, a fully mapped writable canonical `[D,D+512)` span without
address wrap, and set `RAX=D-0x40`. Use a dedicated page with the entire span
and adjacent 32-byte guards accessible. Code, input literals, observations
and guards must not overlap the destination. Record the test entry `E` and
verify the executable bytes at `[E,E+5)` exactly match the five bytes above.

| Input | Case A | Case B |
|---|---|---|
| MXCSR literal | `0x00001f80` | `0x00007f80` |
| MXCSR expected bytes `[24,28)` | `80 1F 00 00` | `80 7F 00 00` |
| Initial destination, all 512 bytes | `A5` repeated | `5A` repeated |
| Guard `[D-32,D)` | `3C` repeated | `C3` repeated |
| Guard `[D+512,D+544)` | `C3` repeated | `3C` repeated |
| Successful next instruction address | `E+5` | `E+5` |
| Completed instruction under test | exactly one | exactly one |
| Linux native result, 2026-11-04 | passed, repetitions 0 and 2 | passed, repetitions 1 and 3 |
| Independently established physical acceptance | open (`execution_kind=unknown`) | open (`execution_kind=unknown`) |

Before 2026-11-04 this protocol had no executed-case evidence. The
[S2-39k acceptance report](../reports/S2_39K_LINUX_ACCEPTANCE.md) records the
subsequent Linux native A/B/A/B run; raw `execution_kind=unknown` is preserved.

Both MXCSR values keep exception masks set and reserved bits, DAZ and FTZ
clear; B changes rounding control. No arithmetic result is used to derive an
expected register value. The following are literal 16-byte strings in
increasing memory-address order (low byte of each XMM first), not host-endian
integer text. Every register has a distinguishable pattern within each case,
and each register changes between cases.

| Register | Case A bytes | Case B bytes |
|---|---|---|
| XMM0 | `00 01 02 03 04 05 06 07 08 09 0A 0B 0C 0D 0E 0F` | `FF FE FD FC FB FA F9 F8 F7 F6 F5 F4 F3 F2 F1 F0` |
| XMM1 | `10 11 12 13 14 15 16 17 18 19 1A 1B 1C 1D 1E 1F` | `EF EE ED EC EB EA E9 E8 E7 E6 E5 E4 E3 E2 E1 E0` |
| XMM2 | `20 21 22 23 24 25 26 27 28 29 2A 2B 2C 2D 2E 2F` | `DF DE DD DC DB DA D9 D8 D7 D6 D5 D4 D3 D2 D1 D0` |
| XMM3 | `30 31 32 33 34 35 36 37 38 39 3A 3B 3C 3D 3E 3F` | `CF CE CD CC CB CA C9 C8 C7 C6 C5 C4 C3 C2 C1 C0` |
| XMM4 | `40 41 42 43 44 45 46 47 48 49 4A 4B 4C 4D 4E 4F` | `BF BE BD BC BB BA B9 B8 B7 B6 B5 B4 B3 B2 B1 B0` |
| XMM5 | `50 51 52 53 54 55 56 57 58 59 5A 5B 5C 5D 5E 5F` | `AF AE AD AC AB AA A9 A8 A7 A6 A5 A4 A3 A2 A1 A0` |
| XMM6 | `60 61 62 63 64 65 66 67 68 69 6A 6B 6C 6D 6E 6F` | `9F 9E 9D 9C 9B 9A 99 98 97 96 95 94 93 92 91 90` |
| XMM7 | `70 71 72 73 74 75 76 77 78 79 7A 7B 7C 7D 7E 7F` | `8F 8E 8D 8C 8B 8A 89 88 87 86 85 84 83 82 81 80` |
| XMM8 | `80 81 82 83 84 85 86 87 88 89 8A 8B 8C 8D 8E 8F` | `7F 7E 7D 7C 7B 7A 79 78 77 76 75 74 73 72 71 70` |
| XMM9 | `90 91 92 93 94 95 96 97 98 99 9A 9B 9C 9D 9E 9F` | `6F 6E 6D 6C 6B 6A 69 68 67 66 65 64 63 62 61 60` |
| XMM10 | `A0 A1 A2 A3 A4 A5 A6 A7 A8 A9 AA AB AC AD AE AF` | `5F 5E 5D 5C 5B 5A 59 58 57 56 55 54 53 52 51 50` |
| XMM11 | `B0 B1 B2 B3 B4 B5 B6 B7 B8 B9 BA BB BC BD BE BF` | `4F 4E 4D 4C 4B 4A 49 48 47 46 45 44 43 42 41 40` |
| XMM12 | `C0 C1 C2 C3 C4 C5 C6 C7 C8 C9 CA CB CC CD CE CF` | `3F 3E 3D 3C 3B 3A 39 38 37 36 35 34 33 32 31 30` |
| XMM13 | `D0 D1 D2 D3 D4 D5 D6 D7 D8 D9 DA DB DC DD DE DF` | `2F 2E 2D 2C 2B 2A 29 28 27 26 25 24 23 22 21 20` |
| XMM14 | `E0 E1 E2 E3 E4 E5 E6 E7 E8 E9 EA EB EC ED EE EF` | `1F 1E 1D 1C 1B 1A 19 18 17 16 15 14 13 12 11 10` |
| XMM15 | `F0 F1 F2 F3 F4 F5 F6 F7 F8 F9 FA FB FC FD FE FF` | `0F 0E 0D 0C 0B 0A 09 08 07 06 05 04 03 02 01 00` |

## Setup, execution and independent observation

1. Prefill and capture the complete destination and guards before setup.
   Load every XMM0–15 from the literal table with independently authored
   `MOVDQU` loads and load the literal MXCSR with `LDMXCSR`. Do not use the
   production decoder, state conversions, executor, FXSAVE serializer, or an
   FXRSTOR image to construct the expected state or the setup sequence.
2. In one audited assembly-controlled region, store all sixteen live XMM
   values with `MOVDQU` to separate pre-observation storage and use `STMXCSR`
   for independent MXCSR readback. Verify these bytes against the literals.
   Execute exactly one instance of the five test bytes. Observe all sixteen
   XMM registers and MXCSR again with `MOVDQU`/`STMXCSR` into separate storage
   before any compiler-generated code or library call can alter state.
3. Capture the raw 512-byte destination and both guards without normalization
   or serialization through the implementation under test. Copying/reporting
   may occur after live-state observations are secured. Retain the original
   bytes even when a comparator excludes fields.
4. Independently establish successful progression from `E` to `E+5` and one
   completion of the instruction under test. The harness must record
   its exact stop/marker mechanism and raw observed RIP, and distinguish any
   marker/trap RIP adjustment from the expected instruction boundary. Setup,
   observations and marker instructions are outside that one-instruction
   count; it is not a hardware performance-counter retirement claim. A crash,
   unexpected signal or unavailable observation is never silently counted as
   success or reinterpreted as an architectural fault result in this protocol.
5. Keep the load → pre-observation → test → post-observation region free of
   compiler/libc SIMD or x87 clobbers, ABI calls and instrumentation. Verify
   the emitted bytes/disassembly, all register and memory clobber declarations,
   and control flow. A future implementation must preserve the caller's state
   outside the measured region and document signal/debugger interference
   handling. Unverified setup or interference invalidates a run.

Evidence classification distinguishes unavailable prerequisites from failed
attempts. `not_run` is reserved for a prerequisite that prevents the first
tracee execution (for example, an unsupported host or unavailable ptrace
setup). Once a tracee has been attempted, any unexpected stop, signal, exit,
marker, or observation result is `rejected`, never `not_run`. Preserve the raw
`waitpid` status with its decoded signal or exit status, stop
RIP/registers and any image, guard, or observation bytes that were available.
Include completed earlier repetitions in the same rejected
record; do not discard them when a later repetition fails. Rejected evidence
cannot pass the comparison contract.
If a marker stop was already verified, retain its RIP/RAX/signal code as a
separate marker snapshot. Fields for a later wait describe only that later
wait and remain absent when the wait or corresponding ptrace read fails.

## Comparison contract

All image offsets are relative to `D`; intervals are half-open. Comparison
uses independent literal byte slices and the raw independent observations,
never a second call to the production serializer. Baseline A/B do not define
x87 preparation, so x87 fields are deliberately outside their comparison mask.

| Image slice / observation | Pass/fail rule |
|---|---|
| MXCSR `[24,28)` | Exact four bytes from the case; independently captured pre/post `STMXCSR` must equal that literal |
| XMM `[160,416)` | For each i=0…15, exact table bytes at `[160+16*i,176+16*i)`; pre/post `MOVDQU` observations must also equal the table |
| Software tail `[464,512)` | All 48 bytes equal that case's original destination canary, verified against the pre-dump |
| Guards outside the image | Both 32-byte guards unchanged; a discrepancy invalidates successful bounded-write acceptance and needs investigation |
| Completion | Expected next RIP `E+5`, exactly one successful test instruction, no unexpected exit |
| MXCSR_MASK `[28,32)` | Record raw four bytes with CPU identity; observation only. Do not replace zero, normalize the mask, or require equality with synthetic target `0x0000ffbf` |
| FCW `[0,2)`, FSW `[2,4)`, abridged FTW `[4,5)` | Excluded in baseline A/B. Compare only in a separately defined case whose x87 preparation, expected header and independent verification are explicit and reviewed |
| FOP, FIP, FDP and x87 register payload | Raw evidence only; no comparison without defined independent sources. Empty x87 payload is not assumed zero |
| Reserved/unused bytes, including `[416,464)`, byte 5 and x87 slot padding | Retain raw pre/post values but exclude from pass/fail; do not assert canary preservation or zero writes |

REX.W=1 uses the 64-bit FIP/FDP layout; there are no FCS/FDS fields here.
No selector check may be inferred from pointer bytes. No equality assertion
covers all 512 bytes. The existing interpreter's 13 write callbacks,
per-callback partial-write policy and preserved-hole behavior are local API
properties, not hardware write counts or reserved-byte guarantees. Neither
this successful-case protocol nor guard checks claim whole-area atomic write,
fault rollback, page-fault order, or bus transaction behavior.

## Evidence and future acceptance

Run at least two independent repetitions of each case in alternating order
A, B, A, B, restoring fresh inputs each time. Keep per-repetition raw evidence;
never average, discard or mask a disagreement. Record:

- CPU vendor/family/model/stepping, raw relevant CPUID leaves (including FXSR
  and SSE), CPU affinity, OS/kernel and whether the execution is physical or
  virtualized; virtualized evidence is not mislabeled physical acceptance.
- Source revision and SHA-256 hashes of harness sources, literal tables,
  emitted executable/test/setup/observation bytes, build tool versions/flags,
  executable, protocol/comparison-mask version, and independent comparator.
- Case ID, repeat index, actual code/input/output/guard addresses and mapping
  permissions, RAX, entry/stop observation, input MXCSR/XMM literals, full
  pre/post destination and guards, independent pre/post register observations,
  raw MXCSR_MASK, exit status and every compared/excluded byte range.
- Separate setup validation, local interpreter result (if later collected),
  hardware observation and comparison result. A local fixture pass or
  host-specific mask cannot substitute for missing hardware evidence.

The dedicated S2-39k harness implementation is described in
[S2_39K_FXSAVE64_ORACLE.md](S2_39K_FXSAVE64_ORACLE.md). This protocol remains
normative for its cases and comparison rules. One A/B/A/B Linux native run
passed after emitted-code/ABI inspection on 2026-11-04; see the separate
[acceptance report](../reports/S2_39K_LINUX_ACCEPTANCE.md). Its raw
`execution_kind=unknown` remains unchanged, and separately established physical
Mac acceptance remains open. See [S2-39l](S2_39L_FXSAVE64_REPLAY.md) for a
local cross-check of only the protocol-defined bytes and progression against
this saved evidence; it is not additional native or physical acceptance.
See [CPU oracle boundaries](CPU_ORACLE.md), the
[S2-40a matrix](../reports/STAGE_2_LINUX_PROFILE.md#s2-40a--матрица-требований-ubuntu-generic-amd64)
and [Stage 2 plan](../plans/STAGE_2.md#s2-39j-successful-cpl3-fxsave64-oracle-protocol).

Open follow-on cases remain owned by S2-39/S2-41: explicitly prepared and
verified x87 headers; nonempty x87 stack/TOP/tag and defined FIP/FDP/FOP
sources; plain/legacy layouts and selectors; additional address forms;
alignment/canonicality/page faults and control gates; FXRSTOR and mixed-state
round-trips; privileged/system integration. Their absence is a coverage
boundary, not a newly confirmed implementation defect. Existing x87 and
broader save/restore limitations remain in
[ISSUE-019](../KNOWN_ISSUES.md#issue-019--семантика-арифметики-и-инструкций-x87-остаётся-открытой)
and the broader scope following
[ISSUE-026](../KNOWN_ISSUES.md#issue-026--plain-long64-fxsavefxrstor-target-lacks-selector-state).
