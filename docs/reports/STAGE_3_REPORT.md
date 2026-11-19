# Stage 3 — локальная bounded acceptance сводка

**Статус:** локальные bounded slices имеют отдельные evidence; Stage 3 не
завершён. Этот документ фиксирует доступный локальный результат и оставшиеся
обязательные gates, но не заменяет финальную приёмку S3-41.

| Статус | Текущее состояние |
|---|---|
| Bounded local evidence | Подтверждены только перечисленные ниже fixture/profile slices: exact `MOV EAX,[RBX]` checked dword load (S3-20cn) and its exact `MOV EAX,[RAX]` alias (S3-20cp), exact `MOV EAX,[RBX+disp8]` dword load (S3-20cr), exact `MOV EAX,[RBX+disp32]` dword load (S3-20cs), exact `MOV [RBX+disp8],EAX` dword store (S3-20ct), exact `MOV [RBX+disp32],EAX` dword store (S3-20cu), exact `MOV [RBX],EAX` dword store (S3-20co) and its exact `MOV [RAX],EAX` alias (S3-20cq), typed-IR qword PC-map comparator/publication lifetime tests for `48 8B 03` / `48 89 03` (S3-27f), typed-IR dword PC-map contracts for `8B 03` / `89 03` (S3-27g), published disp8/disp32 dword-load map equality/retention/republication test (S3-27h; no host-fault recovery), and S3-27i separate canonical-end/range-overflow host coverage for exact `89 83 id`, exact RIP-relative dword load `8B 05 id` (S3-20cv), exact RIP-relative dword store `89 05 id` (S3-20cw; split backing yields no SMC/alias evidence), exact address-size-32 dword load `67 8B 03` (S3-20cx), exact address-size-32 dword store `67 89 03` (S3-20cy), exact address-size-32 disp8 dword load `67 8B 43 ib` (S3-20cz), exact address-size-32 disp32 dword load `67 8B 83 id` (S3-20da), exact address-size-32 disp8 dword store `67 89 43 ib` (S3-20db), exact address-size-32 disp32 dword store `67 89 83 id` (S3-20dc; author-worktree full `od-jit` passed 315/315 before cherry-pick, not rerun on integrated main), exact address64 SIB dword load `8B 04 0B` (S3-20dd; author-worktree full `od-jit` passed 316/316 before cherry-pick, not rerun on integrated main), address64 SIB dword loads `8B 04 4B` / `8B 04 8B` / `8B 04 CB` (S3-20df; author-worktree full `od-jit` passed 318/318 before cherry-pick, not rerun on integrated main), address64 SIB dword loads `8B 44 0B/4B/8B/CB ib` at scales 1/2/4/8 (S3-20dh; author-worktree full `od-jit` passed 320/320 before cherry-pick, not rerun on integrated main), address64 SIB dword stores `89 44 0B/4B/8B/CB ib` at scales 1/2/4/8 (S3-20di; author-worktree full `od-jit` passed 321/321 before cherry-pick, not rerun on integrated main; `od-host` had 147 passes and three existing manual-corpus cases could not access ignored artifacts in the isolated worktree), address64 SIB dword loads `8B 84 0B/4B/8B/CB id` at scales 1/2/4/8 (S3-20dj; exact focused JIT/host gates and prior S3-20dd/df/dh JIT gates passed 1/1; no full `od-jit` suite claim), address64 SIB dword stores `89 84 0B/4B/8B/CB id` at scales 1/2/4/8 (S3-20dk; exact new JIT/host and preserved host/JIT regression gates passed 1/1 in author worktree before cherry-pick; no full `od-jit` suite claim), and address-size-32 SIB dword loads `67 8B 04 0B/4B/8B/CB` at scales 1/2/4/8 (S3-20dl; author-branch `od-jit` 324/324, `od-host` 150/153 because three ignored manual-corpus artifacts were unavailable), plus address-size-32 SIB dword stores `67 89 04 0B/4B/8B/CB` at scales 1/2/4/8 (S3-20dm; author-branch `od-jit` 325/325, full `od-host` not run), and address-size-32 SIB disp8 dword loads `67 8B 44 0B/4B/8B/CB ib` (S3-20dn; author-worktree `od-jit` 326/326 before cherry-pick, full `od-host` not run), plus address-size-32 SIB disp8 dword stores `67 89 44 0B/4B/8B/CB ib` (S3-20do; author-worktree `od-jit` 327/327, `od-host` 153/156 due three ignored manual corpus/cache artifacts), address-size-32 SIB disp32 dword loads `67 8B 84 0B/4B/8B/CB id` (S3-20dp; author-worktree `od-jit` 328/328 and `od-host` 154/157 with three ignored manual artifacts unavailable; not rerun on integrated main), address64 SIB dword stores `89 04 4B` / `89 04 8B` / `89 04 CB` (S3-20dg; author-worktree full `od-jit` passed 319/319 before cherry-pick, not rerun on integrated main), exact address64 SIB dword store `89 04 0B` (S3-20de; author-worktree full `od-jit` passed 317/317 before cherry-pick, not rerun on integrated main), and published disp8/disp32 dword-store map parity/lifetime (S3-27j); four exact pinned harness gates for the earlier memory tests (S3-34u), two exact pinned harness gates for dword-store admission/host behavior (S3-34v), three exact pinned harness gates for dword PC-map/publication cases (S3-34w), three exact pinned gates for S3-20cp aliased dword-load admission/PC-map/host behavior (S3-34x), three exact pinned gates for S3-20cq aliased dword-store admission/PC-map/host behavior (S3-34y), two exact pinned gates for S3-20cr disp8 dword-load site binding and host behavior (S3-34z; integrated harness and CPU discovery passed), and three exact pinned gates for S3-20cs disp32 source binding, PC map, and host behavior (S3-34aa; integrated harness and CPU discovery passed), and three exact pinned gates for S3-20ct disp8-store source binding, PC map, and host behavior (S3-34ab; integrated harness and CPU discovery passed), the exact S3-27h published dword disp8/disp32 load-map integration gate (S3-34ac; integrated harness and CPU discovery passed), three exact S3-20cu disp32-store gates (S3-34ad), and one exact S3-27i canonical-end/range-overflow gate (S3-34ae), and the exact S3-27j dword-store PC-map publication gate (S3-34af), plus three exact S3-20cv RIP-relative load gates (S3-34ag), and three S3-20cw RIP-relative store gates (S3-34ah), plus three S3-20cx addr32-load gates (S3-34ai), and two S3-20cy addr32-store gates without a PC-map claim (S3-34aj), two S3-20cz addr32 disp8-load gates without a PC-map claim (S3-34ak), two S3-20da source-site/host gates without a PC-map claim (S3-34al), and two S3-20db dword-store gates without a PC-map claim (S3-34am), and two S3-20dc dword-store source/site and host gates without a PC-map claim (S3-34an), plus the two S3-20dj disp32-load source/site and host identities pinned without a PC-map gate by S3-34au; checked-memory cache correctness (S3-18e), cold direct-entry (S3-35c), warm direct-entry (S3-36d), cached full-call (S3-36e), cached dispatcher register recurrence (S3-36f), cached-dispatch phase attribution on those six synthetic strata (S3-31b), run-local prepared-key reuse comparison on six synthetic strata (S3-31c), profile-specific offline audit of S3-31c reports (S3-40j), warm dispatcher phase attribution on two selected S3-17 strata (S3-31a; no linking conclusion), FIFO16 lifecycle/counters (S3-37d), separate VMHost/C ABI lifecycle and RESULT-state slices (S3-38a/c/d), current-source VMHost process-only regression (S3-38b), current full-local harness summary (S3-41a; independent evidence review passed), and an M1 Pro ARM64 smoke (S3-39). |
| Broad Stage 3 conditions | Не доказаны: full dispatcher/workload and memory matrix, general cold/warm usefulness, wider resource/lifecycle behavior, or final production policy. These conditions remain open. |
| Physical hardware gate | On Mini (192.168.1.39), the S3-14 register-only gate passed (13 one-block fixtures × 2 repeats) and the S3-15 `jit-dispatch` physical corpus gate passed (10 manual + 1,000 generated fixtures × 2 repeats); all preflights passed 613/613 tests and every pairwise/repeat comparison had zero differences. S3-14 is complete and S3-15 remains partial; the final S3-21 memory/hardware gate, production owner/MMU integration, performance acceptance, and broader Stage 3 conditions remain open. M1 Pro remains deferred. |
| Production owner/MMU | Outside the current fixture scope; no production RAM/backing/mapping/MMU owner or committed-writer integration is claimed here. |

S3-56 completed the generic-register comparison on the five-case S3-14
corpus and both historical comparisons. Warm uses the five S3-36b inputs.
Cold uses the original S3-35a trace and all 1,010 byte-verified fixtures.
On the latter, the generic/FIFO ratios of paired JIT/interpreter medians
were 1.101, 1.094 and 1.085 across three rounds. This evidence covers the
native-register workload. The Core line passed S3-57 on `11cbe6b`.
Final Stage 3 acceptance remains open in S3-41.
[Cold evidence](STAGE_3_S3-56_S3-17_2026-11-19.md)
[Matched cold and warm evidence](STAGE_3_S3-56_MATCHED_2026-11-19.md)
[Core review and final-state repeat](STAGE_3_CORE_REPORT.md#10-s3-57--финальное-независимое-ревью-линии)

S3-26f source support and unrun evidence were added for the exact uncached
`48 89 03 48 89 02` two-store pair. Source/site binding, ordered typed IR,
second helper operation index 4, host metadata, explicit cached rejection,
uncached continuation selection, and focused evidence gates are recorded in
the [S3-26f report](STAGE_3_S3-26f_2026-11-03.md). Builds, tests, formatting,
and acceptance gates were not run; independent review is pending. The pair
remains private ordinary RAM with per-write all-or-nothing semantics and is
not transactional. Full S3-26/S3-27, owner/MMU and physical-hardware gates stay
open.

Stage 2 process launches or sampled activity are not blockers by themselves and
do not need to be stopped or quiet for this local evidence. Monitoring
limitations, including point-in-time snapshots without per-pair attribution,
remain recorded in each affected report.

## Зафиксированное локальное evidence

S3-20ec adds the exact address64 RSP+signed-disp8 dword store `89 44 24 ib`
through shared typed IR. Its JIT source/site gate covers all 256 displacement
bytes; the host gate covers signed edges, address wrap, state/RAM parity,
faults, atomicity, STOP, and no-replay. On runtime author commit `7ae5578`,
the `od-jit` library passed 341/341, `od-host` passed 170/170 using a fresh
writable corpus directory seeded from immutable baseline inputs, and adjacent
S3-20ea/S3-20eb regressions passed 1/1. After integration as runtime commit
`c9195cd` and harness commit `2edeb07`, the new S3-20ec JIT/host gates passed
1/1 and the full Python harness passed 87/87. The
fresh directory was needed because a stale ignored reducer output in the
isolated worktree made one full-suite run refuse an overwrite; the issue was
resolved as test-artifact setup and did not require a product-code change. No
broad performance, PC-map, owner/MMU, or physical-hardware claim follows. See
the [S3-20ec report](STAGE_3_S3-20ec_2026-11-02.md).

S3-20ed adds the exact address64 RSP+signed-disp32 dword load `8B 84 24 id`.
The JIT source/site fixture covers nine representative signed values and
mutations to all four displacement bytes; the host fixture checks interpreter
parity, width-four reads, faults, overflow/backend atomicity, STOP, and
no-replay. On author revision `c9530b1`, `od-jit` passed 342/342 and adjacent
S3-20dz/S3-20eb host gates passed 1/1. After integration as `238799d`, the
new exact JIT/host gates passed 1/1 and the full Python harness passed 88/88.
Full `od-host` was not run. This bounded fixture does not establish a broad
PC-map or performance result. See the
[S3-20ed report](STAGE_3_S3-20ed_2026-11-02.md).


S3-20ee adds the exact address64 RSP+signed-disp32 dword store `89 84 24 id`.
The JIT source/site fixture checks nine representative signed values and all
four displacement bytes; the host fixture checks interpreter parity, one
width-four write, wrap-to-zero, faults, atomicity, STOP, and no-replay. On
author runtime commit `7a07588`, `od-jit` passed 343/343 and exact S3-20ee plus
adjacent S3-20ec JIT/host gates passed 1/1. After root integration (`6cac944`
runtime, `1f32667` harness), the exact S3-20ee gates passed 1/1 and the full
Python Stage 3 harness passed 89/89. Full `od-host` was not run. This bounded
fixture does not establish a broad PC-map, performance, owner/MMU, or hardware
result. See the [S3-20ee report](STAGE_3_S3-20ee_2026-11-02.md).

S3-20ef adds only the address-size-32 ESP-base dword load `67 8B 04 24`.
Its effective address is `u64(RSP as u32)`, ignoring high RSP bits; a dword
starting at `0xffff_fffe` reads contiguously through `0x1_0000_0001` without
per-byte 32-bit wrap. The bounded host case checks interpreter parity,
injected read-error atomicity, STOP/budget, and callback retry without a
second read; no RSP `#SS` claim is made. On author runtime commit `392886b`,
`od-jit` passed 344/344 and the exact JIT/host, legacy negative JIT, and
adjacent S3-20cx/S3-20cz host gates passed 1/1. After root integration
(`a0ce871` runtime, `1ba352a` harness), exact gates passed 1/1 and the full
Python Stage 3 harness passed 91/91. Full `od-host` was not run. See the
[S3-20ef report](STAGE_3_S3-20ef_2026-11-02.md).

S3-20eg adds only the address-size-32 ESP-base dword store `67 89 04 24`.
It zero-extends `RSP as u32` for the start address, then writes a contiguous
four-byte span, including across 4 GiB. The runtime fixture compares interpreter
state/RAM, and callback STOP retry is explicitly rejected without a second
write. The Sol review recorded a nonblocking coverage gap: backend-error and
entry STOP/budget cases do not explicitly retry successfully after asserting
no state/memory commit; see [ISSUE-045](../KNOWN_ISSUES.md). On author runtime
commit `abd2af4`, `od-jit` passed 345/345; after integration at root `8052314`
with harness `ac069ca`, exact JIT/host gates passed 1/1 and the cumulative
Python Stage 3 harness passed 91/91 (including 34br). Full `od-host` was not
run. See the [S3-20eg report](STAGE_3_S3-20eg_2026-11-02.md).

Runtime follow-up commit `525e310` adds successful retry assertions after
backend errors and entry STOP/budget guards. Sol/high static review passed,
but those assertions have not been executed. Historical `od-jit`, exact-gate,
and Python harness results above predate this follow-up. ISSUE-045 remains
open awaiting execution acceptance; see the [S3-20eg report](STAGE_3_S3-20eg_2026-11-02.md).

S3-20eh adds only the address-size-32 ESP-base dword load with signed disp8
`67 8B 44 24 ib`. The effective address adds sign-extended disp8 to low32 RSP,
wraps modulo 2^32, then zero-extends; a four-byte read remains contiguous
across 4 GiB. High RSP bits are ignored. The host fixture covers wrap,
poisoned high bits, interpreter parity, and callback STOP replay rejection
without a second read. On author runtime commit `62a516a`, `od-jit` passed
346/346, the exact JIT/host gates and adjacent S3-20ef/S3-20cz host gates
passed 1/1, and fmt/check plus Sol review passed. At root `08b3eba` with harness
`046f5a2`, exact JIT/host gates passed 1/1 and the full Python Stage 3 harness
passed 92/92 after 34bs integration. Full `od-host` was not run. See the
[S3-20eh report](STAGE_3_S3-20eh_2026-11-02.md).

S3-20ei adds only the address-size-32 ESP-base dword store with signed disp8
`67 89 44 24 ib`. Its EA wraps low32 RSP plus sign-extended disp8 modulo
2^32, then zero-extends; the contiguous four-byte write stores low32 RAX,
including across 4 GiB. JIT binds all signed disp8 encodings to their sites;
the host fixture checks interpreter parity, wrap, entry/backend-error retries,
and callback STOP no-replay. On author runtime commit `cb02fb8`, `od-jit`
passed 347/347 and exact S3-20ei JIT/host gates and S3-20eg/S3-20eh neighbor gates passed
1/1; cargo check/fmt and runtime Sol review passed. At integrated root `2784629`
with harness source `42719175` integrated as `d8e1065`, exact JIT/host gates
passed 1/1 and the full Python Stage 3 harness passed 93/93. Full `od-host` was
not run. ISSUE-045 remains open for the separate S3-20eg test-coverage gap;
S3-20ei does not close it. See the [S3-20ei report](STAGE_3_S3-20ei_2026-11-02.md).

S3-20ej adds only the address-size-32 ESP-base dword load with signed disp32
`67 8B 84 24 id`. Its EA wraps low32 RSP plus sign-extended disp32 modulo
2^32, then zero-extends; the four-byte read remains contiguous across 4 GiB.
The JIT source/site test checks ten representatives and all displacement
bytes; the host fixture checks `-128`/`+127`, wrap-to-zero, poisoned high RSP
bits, parity, and entry STOP/budget retry. On author runtime commit `d649bd4`,
`od-jit` passed 348/348; exact S3-20ej JIT/host gates and adjacent S3-20da/S3-20dx host gates
passed 1/1, with cargo check/fmt, Graphify, and Sol review passing. At root
`4e4fec2`, exact gates passed 1/1 and the full Python Stage 3 harness passed
94/94 with harness source `00430e9` integrated as `4790e76` (216 gates: 206
acceptance, 10 lifecycle; 234 local steps). Full `od-host` was not run.
ISSUE-045 remains open for the separate S3-20eg coverage limitation. See the
[S3-20ej runtime report](STAGE_3_S3-20ej_2026-11-02.md).

S3-20ek adds only the address-size-32 ESP-base dword store with signed disp32
`67 89 84 24 id`. Its EA wraps low32 RSP plus signed disp32 modulo 2^32 and
zero-extends; the contiguous width-four write stores low32 RAX and advances
to `A+8`. On author runtime commit `34ad7e2`, the exact JIT gate passed 1/1
and `od-jit` passed 349/349; cargo check/fmt, Graphify and Sol runtime review
passed. The exact host test initially failed on an incorrect expected target
(`0x7fff_ffff`); the fixture was corrected to `0x8000_1fff` and statically
reviewed, but the test was not rerun; Mini execution remains deferred. The initial host run preceded the execution-deferral message reaching the reviewer. Its host
gate remains pending. The full Python Stage 3 harness passed 94/94 at root
revision `190a61c`. Full `od-host` was not run. See the
[S3-20ek runtime report](STAGE_3_S3-20ek_2026-11-02.md).

S3-20el adds only the exact unprefixed absolute SIB disp32 Dword load
`8B 04 25 id`; the signed displacement forms the 64-bit absolute EA, the
checked read is four bytes, and RIP advances to `A+7`. On author runtime
`aa8e38f`, formatting, test-target compilation, the exact JIT gate (1/1), and
diff-check passed on Mac15,11. These checks ran despite the user's Mini
deferral, following a mistaken orchestration instruction; this deviation is
not a Mini/M1 gate. The exact host test was compiled but not executed. Cleanup
`d483a63` was integrated as `0903953`; static Sol review and diff-check passed
with no test/build. Full `od-jit`/`od-host` and Mini/M1 physical gates remain
pending. ISSUE-045 remains open in [the issue registry](../KNOWN_ISSUES.md); Stage 2 remained outside this slice's
evidence and concurrent runs were ignored per user instruction. Stage 3
remains partial. See the [S3-20el report](STAGE_3_S3-20el_2026-11-02.md).

S3-20em adds only the exact unprefixed absolute SIB disp32 Dword store
`89 04 25 id`: the signed disp32 forms the 64-bit absolute EA, one checked
four-byte write stores low32 RAX, and RIP advances to `A+7`. The bounded
host fixture checks the last valid four-byte range at `u64::MAX-3`, adjacent
range faults before callback, backend-error no-commit/retry, entry guards,
and callback STOP commit without replay. Runtime source
`c6149ae34f9b74516f301ab6016f8f87d9def24a` was integrated as `5d8e9ac`;
Sol/high runtime re-review passed after the exact publication-whitelist fix.
No tests/build/fmt/Cargo checks ran due to the Mini deferral; exact JIT and
host gates remain pending until end-of-stage checks. Separate harness source
`b165126c83378cbc69168f5201dc3ddabd340d48` was integrated as `a70a022`;
its static manifest reports 222 gates (212 acceptance, 10 lifecycle) and
240 local steps, harness review passed, and no Python tests ran. ISSUE-045
remains open. Stage 2 remained outside this slice's evidence; concurrent runs
were ignored per user instruction. Mini/M1 remain deferred; Stage 3 remains
partial. See the [S3-20em report](STAGE_3_S3-20em_2026-11-02.md).

S3-20en adds only the exact address-size-32 absolute SIB disp32 Dword load
`67 8B 04 25 id`. The addr32 disp32 is reduced modulo 2^32 and zero-extended;
one checked four-byte read can cross 4 GiB without per-byte wrap, EAX
zero-extends into RAX, and the guest boundary is `A+8`. Its JIT fixture covers
nine signed displacement representatives and source/site identity for all
eight bytes. The host fixture checks one read at `0xffff_ffff`, state/flag
preservation, entry guards, backend-read failure without commit followed by
retry, and callback STOP commit with no replay.

Runtime source `4234628a6f06f6656f234c2c124504fc2f80b750` was integrated as
`6588d50`; independent Sol/high static review passed. Host identity follow-up
`a202412` was integrated as `ba6d8e1` to match the exact identity pinned by
harness 34by. No tests, builds, formatting, Cargo checks, or Python harness
tests were run for this slice under the user's Mini deferral, so both exact
runtime Cargo gates remain pending. Harness source `082d05d12e4a3334a638fd319d2efa24fbcc533e`
was integrated as `9353bfa`; its manifest records 224 Rust gates (214
acceptance, 10 lifecycle) and 242 local steps. Static harness review passed;
no Python tests were run. ISSUE-045 remains open. Stage 2 remained outside
this slice's evidence; concurrent runs were ignored per user instruction.
Mini/M1 remain deferred; Stage 3 remains partial. See the
[S3-20en report](STAGE_3_S3-20en_2026-11-02.md).

S3-20eo adds only the exact address-size-32 absolute SIB disp32 Dword store
`67 89 04 25 id`. The decoder-owned disp32 is reduced modulo 2^32 and
zero-extended; one checked width-four write stores low32 RAX and can cross
4 GiB without per-byte wrap. The instruction boundary is `A+8`. Its JIT
fixture covers six displacement representatives and source/site binding for
all eight bytes. The host fixture checks a contiguous write starting at
`0xffff_fffe`, low32 RAX payload, backend-error no-commit followed by retry,
entry guards, and callback STOP commit without replay.

Runtime source `c98a28d` was integrated as `4970c5c`; independent Sol/high
static review passed after coverage fixes. No tests, builds, formatting,
Cargo checks, or Python harness tests ran due to the user's Mini deferral, so
the exact runtime gates remain pending. Harness source `7ee99dc` was
integrated as `5069141`; its manifest records 226 Rust gates (216 acceptance,
10 lifecycle) and 244 local steps. Static harness review passed; no Python
tests ran. ISSUE-045 remains open. Stage 2 remained outside this slice's
evidence; concurrent runs were ignored per user instruction. Mini/M1 remain
deferred; Stage 3 remains partial. See the
[S3-20eo report](STAGE_3_S3-20eo_2026-11-02.md).

S3-34bz pins the exact S3-20eo JIT source/site and host zero-extended-address
identities. Harness source `7ee99dc` was integrated as `5069141`; the static
manifest records 226 Rust gates (216 acceptance, 10 lifecycle) and 244 local
steps. Independent Sol/high harness review passed. No Python harness
validation or Cargo gate ran for this slice. S3-20eo runtime source `c98a28d`
is integrated as `4970c5c` and passed static review, but its runtime gates
were not executed; this harness is not runtime evidence. ISSUE-045 remains
open. Stage 2 remained outside this slice's evidence; concurrent runs were
ignored per user instruction. Mini/M1 remain deferred; Stage 3 remains
partial. See the [S3-34bz report](STAGE_3_S3-34bz_2026-11-02.md).

S3-34ca pins the exact S3-20ep JIT source/site and host identities. Harness
source `8b05d2f` was integrated as `02c92fa`; the static manifest records 228
Rust gates (218 acceptance, 10 lifecycle) and 246 local steps. Independent
Sol/high static review passed. No Python or harness execution was performed,
so there is no harness pass or runtime evidence from this slice. S3-20ep
runtime source `ee36c4c` is now integrated as `07c2beb`; static review passed
after the IR profile nesting and host mapping lease lifetime fixes. Runtime
gates remain pending because no tests, builds, formatting, Cargo checks, or
Python harness tests were run. ISSUE-045 remains open. Stage 2 remained outside
this slice's evidence; concurrent runs were ignored per user instruction.
Mini/M1 remain deferred; Stage 3 remains partial. See the
[S3-34ca report](STAGE_3_S3-34ca_2026-11-02.md).

S3-20ep adds only the exact unprefixed RCX-indexed no-base SIB disp32 Dword
load `8B 04 0D disp32`. The decoder-owned effective address is the full RCX
plus sign-extended disp32 with wrapping u64 arithmetic; one checked
four-byte read is used, EAX zero-extends into RAX, and the instruction
boundary is `A+7`. Runtime source `ee36c4c` was integrated as `07c2beb`;
independent Sol/high static review passed after fixing IR profile nesting and
host mapping lease lifetime. No tests, builds, formatting, Cargo checks, or
Python harness tests ran, so the exact runtime gates remain pending.

Harness S3-34ca pins the JIT source/site and host identities for this slice.
Harness source `8b05d2f` was integrated as `02c92fa`; its static manifest has
228 Rust gates (218 acceptance, 10 lifecycle) and 246 local steps. Static
review passed, but no Python/harness execution occurred. No runtime evidence
is claimed from those harness gates. ISSUE-045 remains open. Stage 2 remained
outside this slice's evidence; concurrent runs were ignored per user
instruction. Mini/M1 remain deferred; Stage 3 remains partial. See the
[S3-20ep report](STAGE_3_S3-20ep_2026-11-02.md).

S3-34cb pins the exact S3-20eq JIT source/site and host identities. Harness
source `e16f6fd` was integrated as `3fa9448`; the static manifest records 230
Rust gates (220 acceptance, 10 lifecycle) and 248 local steps. Independent
Sol/high static review passed. No Python harness test or other harness
execution was performed, and no runtime evidence or pass is claimed.
S3-20eq runtime source `c384bce` is now integrated as `8c50c90`; independent
Sol/high static review passed after the compiler/block/host byte gates and
host overflow/backend expectation fixes. Runtime gates remain pending because
no tests, builds, formatting, Cargo checks, or Python harness tests were run.
ISSUE-045 remains open. Stage 2 remained outside this slice's evidence;
concurrent runs were ignored per user instruction. Mini/M1 remain deferred;
Stage 3 remains partial. See
the [S3-34cb report](STAGE_3_S3-34cb_2026-11-02.md).

S3-20eq adds only the exact unprefixed RCX-indexed no-base SIB disp32
Dword store `89 04 0D disp32`. The decoder-owned effective address uses full
RCX plus signed disp32 with wrapping u64 arithmetic; one checked width-four
write stores low32 RAX and advances to `A+7`. Runtime source `c384bce` was
integrated as `8c50c90`; independent Sol/high static review passed after fixes
to compiler/block/host byte gates and host overflow/backend expectations. No
tests, builds, formatting, Cargo checks, or Python harness tests ran, so the
exact runtime gates remain pending.

S3-34cb pins this slice's exact JIT and host identities. Its source `e16f6fd`
was integrated as `3fa9448`; its historical manifest records 230 Rust gates
(220 acceptance, 10 lifecycle) and 248 local steps, with static review passed
and no Python/harness execution. The later S3-34cc aggregate counts are not
attributed to S3-34cb. ISSUE-045 remains open. Stage 2 remained outside this
slice's evidence; concurrent runs were ignored per user instruction. Mini/M1
remain deferred; Stage 3 remains partial. See the
[S3-20eq report](STAGE_3_S3-20eq_2026-11-02.md).

S3-34cc pins the exact S3-20er JIT source/site and host identities for the
RCX-indexed no-base scaled SIB Dword loads. Harness source `af854bf` was
integrated as `0567f37`; its historical static manifest records 232 Rust
gates (222 acceptance, 10 lifecycle) and 250 local steps. Sol/high static
review passed; no Python/harness execution was performed. Runtime source
`0ad1f51` is now integrated as `edf50fe` and passed static review after the
stale 4D/8D negatives were removed while retaining the source-forge scale-2
versus original-0D distinction. No tests, builds, formatting, Cargo checks,
or Python tests ran; runtime gates remain pending and the harness report is
not runtime evidence. ISSUE-045 remains open. Stage 2 remained outside this
slice's evidence; concurrent runs were ignored per user instruction. Mini/M1
remain deferred; Stage 3 remains partial. See the
[S3-34cc report](STAGE_3_S3-34cc_2026-11-02.md).

S3-34cd pins the exact S3-20es JIT source/site and host identities for
RCX-indexed no-base scaled SIB Dword stores. Harness source `beb1c93` was
integrated as `a76e732`; its static manifest records 234 Rust gates (224
acceptance, 10 lifecycle), 252 local steps, and 101 self-test names.
Independent Sol/high static review passed; no Python/harness execution was
performed. Runtime source `ca14bac` is now integrated as `0bf9a46` and passed
Sol/high static review after fixture corrections. No tests, builds,
formatting, Cargo checks, or Python tests ran, so the exact runtime gates
remain pending; the harness report itself is not runtime evidence. ISSUE-045
remains open. Stage 2 remained outside this slice's evidence; concurrent runs
were ignored per user instruction. Mini/M1 remain deferred; Stage 3 remains
partial. See the [S3-20es report](STAGE_3_S3-20es_2026-11-02.md) and
[S3-34cd report](STAGE_3_S3-34cd_2026-11-02.md).

S3-34ce pins the exact S3-20et JIT source/site and host identities for the
address-size-32 RCX-indexed no-base SIB Dword load. Harness source `2f3198f`
was integrated as `4626a9e`; its historical static manifest records 236
Rust gates (226 acceptance, 10 lifecycle), 254 local steps, and 102
self-test names. Independent static review passed; no Python/harness
execution occurred. Runtime source `8981622` is now integrated as `2b04cfd`
and passed Sol/high static review after an IR nesting correction. No tests,
builds, formatting, Cargo checks, or Python tests ran; runtime gates remain
pending and this harness report is not runtime evidence. ISSUE-045 remains
open. Stage 2 remained outside this slice's evidence; concurrent runs were
ignored per user instruction. Mini/M1 remain deferred; Stage 3 remains
partial. See the [S3-34ce report](STAGE_3_S3-34ce_2026-11-02.md).

S3-20et adds only the exact eight-byte addr32 RCX-indexed no-base SIB
Dword-load singleton `67 8B 04 0D disp32`. Its effective address is
`zero_extend((low32(RCX) + signed_disp32) mod 2^32)`; the checked read is one
contiguous four-byte access, including a 4-GiB crossing, EAX zero-extends
into RAX, and the guest boundary is `A+8`. Runtime source `8981622` was
integrated as `2b04cfd`; Sol/high static review passed after correcting IR
profile nesting. No tests, builds, formatting, Cargo checks, or Python
tests ran, so both exact runtime gates remain pending. ISSUE-045 remains
open. Stage 2 remained outside this slice's evidence; concurrent runs were
ignored per user instruction. Mini/M1 remain deferred; Stage 3 remains
partial. See the [S3-20et report](STAGE_3_S3-20et_2026-11-02.md) and
[S3-34ce report](STAGE_3_S3-34ce_2026-11-02.md).

S3-34cf pins the exact S3-20eu JIT source/site and host identities for the
address-size-32 RCX-indexed no-base SIB Dword store. Harness source `2d12a6b`
was integrated as `11520e9`; its historical static manifest records 238
Rust gates (228 acceptance, 10 lifecycle), 256 local steps, and 103
self-test names. Independent Sol/high static review passed; no Python/harness
execution occurred. Runtime source `a70dedd` is now integrated as `d56732d`
and passed Sol/high static review after block candidate and host publication
byte-gate fixes. No tests, builds, formatting, Cargo checks, or Python tests
ran, so the exact runtime gates remain pending; the harness report is not
runtime evidence. ISSUE-045 remains open. Stage 2 remained outside this
slice's evidence; concurrent runs were ignored per user instruction. Mini/M1
remain deferred; Stage 3 remains partial. See the [S3-20eu report](STAGE_3_S3-20eu_2026-11-02.md) and
[S3-34cf report](STAGE_3_S3-34cf_2026-11-02.md).

S3-34cg pins the exact S3-20ev JIT source/site and host identities for the
address-size-32 RCX-indexed no-base SIB Dword load at scales 2, 4, and 8.
Harness source `7e4f5e8` was integrated as `44e76ed`; its historical static
manifest records 240 Rust gates (230 acceptance, 10 lifecycle), 258 local
steps, and 104 self-test names. Independent Sol/high static review passed;
no Python/harness execution occurred. Runtime source `581a5c8` is now
integrated as `a2dc7b8` and passed Sol/high static review. No tests, builds,
formatting, Cargo checks, or Python tests ran, so the exact runtime gates
remain pending; this harness report is not runtime evidence. ISSUE-045
remains open. Stage 2 remained outside this slice's evidence; concurrent runs
were ignored per user instruction. Mini/M1 remain deferred; Stage 3 remains
partial. See the [S3-20ev report](STAGE_3_S3-20ev_2026-11-02.md) and
[S3-34cg report](STAGE_3_S3-34cg_2026-11-02.md).

S3-34ch pins the exact S3-20ew JIT source/site and host identities for the
address-size-32 RCX-indexed no-base SIB Dword stores at scales 2, 4, and 8.
Harness source `5c916db` was integrated as `29afd24`; its historical static
manifest records 242 Rust gates (232 acceptance, 10 lifecycle), 260 local
steps, and 105 self-test names. Independent Sol/high static review passed; no
Python/harness execution occurred. Runtime source `75346b6` is now integrated
as `af05d53` and passed Sol/high static review after the matcher whitelist,
`EA=0` retry, and signed-boundary fixture fixes. No tests, builds,
formatting, Cargo checks, or Python tests ran; runtime gates remain pending
and this harness report is not runtime evidence. ISSUE-045 remains open.
Stage 2 remained outside this slice's evidence; concurrent runs were ignored
per user instruction. Mini/M1 remain deferred; Stage 3 remains partial. See
the [S3-34ch report](STAGE_3_S3-34ch_2026-11-02.md).

S3-20ew admits only the eight-byte forms with exactly the `67` address-size
override prefix and no others: `67 89 04 4D/8D/CD disp32` for addr32
RCX-indexed, no-base SIB Dword stores at scales 2, 4, and 8. The effective
address is `zero_extend((low32(RCX)*scale + signed disp32) mod 2^32)`. Each
checked write is contiguous and four bytes wide, stores low32 RAX, and may
cross 4 GiB; GPRs/RFLAGS are preserved, with boundary `A+8`. Runtime source
`75346b6` is integrated as `af05d53`; Sol/high static review passed after the
matcher whitelist, `EA=0` retry, and signed-boundary fixture fixes. No tests,
builds, formatting, Cargo checks, or Python tests ran, so exact gates remain
pending. ISSUE-045 remains open. Stage 2 remained outside this slice's
evidence; concurrent runs were ignored per user instruction. Mini/M1 remain
deferred; Stage 3 remains partial. See the [S3-20ew report](STAGE_3_S3-20ew_2026-11-02.md).

S3-20er adds only the unprefixed seven-byte address64 RCX-indexed no-base
SIB Dword-load singletons `8B 04 4D/8D/CD disp32` (scales 2/4/8). Effective
address calculation uses full-width RCX times the encoded scale plus
sign-extended disp32 with wrapping u64 arithmetic; each checked read is four
contiguous bytes, EAX zero-extends into RAX, and RIP advances to `A+7`. Runtime
source `0ad1f51` was integrated as `edf50fe`; independent Sol/high static
review passed after removing the stale 4D/8D negative cases and preserving the
source-forge scale-2 versus original-0D distinction. No tests, builds,
formatting, Cargo checks, or Python harness tests ran, so the exact runtime
gates remain pending.

Harness S3-34cc pins these exact JIT and host identities. Its source
`af854bf` was integrated as `0567f37`; the historical manifest records 232
Rust gates (222 acceptance, 10 lifecycle) and 250 local steps. Static review
passed, but no Python/harness execution occurred. These counts are specific to
34cc and do not include later aggregate revisions. ISSUE-045 remains open.
Stage 2 remained outside this slice's evidence; concurrent runs were ignored
per user instruction. Mini/M1 remain deferred; Stage 3 remains partial. See
the [S3-20er report](STAGE_3_S3-20er_2026-11-02.md).

S3-36e implementation is committed. Release preflight passed on Apple M3 Max
with 2,400 frozen inputs and 4,800 parity trials. Calibration-only passed at
2,048 repetitions per side with no scored pairs. Three scored runs retained 20
pairs per sequence and passed all floors. Across 60 paired ratios per sequence,
medians were 2.7223 (`store_then_load`) and 2.7650 (`load_then_store`), with no
cross-stratum aggregate or general performance claim. See the [S3-36e
report](STAGE_3_S3-36e_2026-11-01.md) and independently reviewed
[suite evidence manifest](data/stage3/s3-36e/suite-manifest.json).

S3-36f measured the full capacity-one cached dispatcher against the
interpreter on two synthetic helper-free register recurrences at 1, 64, and
4,096 loop iterations. Three release processes retained 60 pairs per
stratum, and all timing floors, cache counters, and retirement checks passed.
Across all six strata, native/interpreter median ratios were 3.7205–4.8938
and every observed paired ratio was above 1. The interpreter was faster on
these fixtures, so they provide no useful native target scenario and justify
no runtime/cache policy change. This synthetic result does not generalize to
guest workloads or close Stage 3 performance acceptance. See the
[S3-36f report](STAGE_3_S3-36f_2026-11-01.md) and the
[suite evidence manifest](data/stage3/s3-36f-20261101/suite-manifest.json).

S3-31b added an explicit observed cached-dispatch path over the same six
S3-36f synthetic strata. Three release processes retained 60 alternating
pairs per stratum and all 720 sides passed 200 ms. Recorded phase shares were
approximately 21–22% build, 43–44% key/snapshot/first lookup, and 4–5% host
execution boundary for form A; form B recorded 32–33%, 39%, and 5–6%
respectively. These are observed-path shares with timer instrumentation; the
residual contains repeated validation and dispatcher/control work. The
observed/ordinary pair delta includes instrumentation and timing/path noise,
and the `b_64` range crosses parity. The result is diagnostic only: phase
labels do not measure total lookup cost or pure generated-code time, and they
do not establish a linking benefit or production policy change. See the
[S3-31b report](STAGE_3_S3-31b_2026-11-01.md) and
[suite manifest](data/stage3/s3-31b-20261101/suite-manifest.json).

S3-31c compared an opt-in run-local prepared-key candidate against the
ordinary cached NativeRegisterV1 dispatcher and interpreter on six synthetic
recurrence strata. Three release processes retained 60 paired triples per
stratum; all 1,080 measured sides passed the 200 ms floor. At 64 and 4,096
iterations, candidate/baseline median ratios were below 1 in both forms, but
candidate/interpreter medians were below 1 only for `b_64` and `b_4096`.
These bounded results do not establish broad workload speedup or justify a
runtime/default-policy change. The candidate remains opt-in. The timed
processes share a clean committed source revision; the correctness-only
preflight was rerun at that same revision and fingerprint in a clean temporary
detached worktree. See the
[S3-31c report](STAGE_3_S3-31c_2026-11-01.md) and
[suite manifest](data/stage3/s3-31c-20261101/suite-manifest.json).

Последний report-inclusive локальный harness run (30 октября baseline):

- Run: `stage3-20261030-054347-8d2c65aebe`, `local_only`, macOS 26.7.1 ARM64.
- Исходная ревизия harness evidence: `fa80ab0da77b3de268c6adf649dc2a92de9dde20`.
- Fingerprint начала и конца: `caa5eec7590595b66910ce855e6c6eba4b476b59c1f7704619b273eb10e970c9`.
- Результат: 31 обязательный шаг прошёл, 0 завершился ошибкой; 33 шага других
  suites остались `not_run`. Fingerprint `caa5eec…` на revision `fa80ab0…` не
  покрывает более поздний HEAD и не служит общей проверкой последующих slices;
  более поздние результаты имеют отдельные bounded reports/evidence ниже.
- Stage 1 dispatcher child: 10 ручных + 1,000 generated input, 2 повтора
  (2,020 результатов); evidence остаётся `local_only`.
- Memory corpus: 3,600 уникальных входов ×2; отдельные named fault, STOP,
  budget и continuation случаи; точный S3-34h gate прошёл.
- Lifecycle/acceptance включает выбранные регрессии для cache/lifetime,
  invalidation, V3 и direct V2 PC maps, event/STOP, demand flags, coordinator
  и V0 ABI. Перечень точных identities и argv находится в исходном run summary.
- Input manifest SHA-256:
  `9860c7e0c453fe98dee086866ab086cdc840cee1fa632f133f1077d857339048`.
- Summary SHA-256:
  `b9c26b9eed6d42791e6ebfa26a8e3a8347fe6785eb137f2be3cc63d16a44fbdc`.
- Команда: `python3 tests/cpu/stage3_harness.py --suite local`.
- Игнорируемые локальные evidence хранятся в
  `work/stage3-harness/stage3-20261030-054347-8d2c65aebe/`; run manifest и
  summary намеренно не копируются в Git.

The later current-source VMHost process run was recorded separately from the
30 October report-inclusive local harness run above. It passed its selected
process suite (33 passed, 0 failed); 120 steps and all other suites, including
Stage 2 and Mini, remained `not_run`. Its independent binding review passed.
This bounded integration evidence neither resolves ISSUE-044 nor replaces the
separate S3-38d RESULT-state evidence. See the [S3-38b current-source
addendum](STAGE_3_S3-38b-current-source_2026-11-01.md) and its
[committed evidence bundle](data/stage3/s3-38b-20261101/).

The current full-local harness invocation is a separate record from that
process-only run. Its summary records 120 passed, 0 failed, and 33 `not_run`;
local lifecycle, local acceptance, memory corpus, and RESULT-state suites
passed, while Stage 1 dispatch was `local_only`. The Stage 1 child ran 10 manual
and 1,000 generated inputs twice, producing 2,020 local results. The earlier
attempt stopped with `FileExistsError` after 106 passed because the run's
`artifacts` directory already existed and contained files. Commit `020f79d`
added safe reuse of a real directory and a self-check rejecting symlinks; the
accepted retry records the updated harness self-check and completes the
selected local suites. The failure receipt is preserved beside the accepted
evidence. Process-only, physical x86, Mac mini, performance, owner-backed, and
Stage 2 suites remain unrun. Independent evidence/binding review passed with
no findings; the summary does not close S3-41 or Stage 3. See the
[current S3-41a report](STAGE_3_S3-41a-current-local_2026-11-01.md) and its
[evidence bundle](data/stage3/s3-41a-local-20261101/).

**Follow-up, 3 November 2026:** a newer `--suite local` attempt
(`stage3-20261103-123052-8fa49da8df`) failed before selected suites began
because the mandatory self-test exceeded its 120-second limit. Its interrupted
temporary root changed the source fingerprint and masked the timeout; the
test-generated directory was removed and the starting fingerprint restored.
A follow-up raises that bounded timeout to 600 seconds, places test temporary
directories under ignored `work/stage3-harness/`, and preserves the primary
failure if fingerprint checking also fails. The focused regression passed 1/1,
and a post-fix full-local rerun attempt exceeded the context tool's 240-second
RPC window without a final summary. No full local-suite result confirms the
repair. See [ISSUE-050](../KNOWN_ISSUES.md#issue-050).

## Карта требований и границ

| Область | Локально подтверждено | Статус и граница |
|---|---|---|
| Block dispatch и bounded execution | S3-34f local acceptance и S3-34h report-inclusive harness; dispatcher corpus запускается отдельно как `local_only` | Это ARM64/local correctness evidence; не physical x86 equivalence и не полный Stage 3 workload. См. [S3-34f](STAGE_3_S3-34f_2026-10-30.md), [S3-34h](STAGE_3_S3-34h_2026-10-30.md). |
| Checked memory lowering, ordered callbacks, and bounded cache | Узкие принятые V2 MOV singleton/pair profiles, включая S3-20cn exact dword load, S3-20cp exact RAX alias, S3-20cr exact RBX+disp8 и S3-20cs exact RBX+disp32 loads, S3-20ct exact RBX+disp8 and S3-20cu exact RBX+disp32 dword stores, S3-20cv exact RIP-relative `8B 05 id` dword load, S3-20cw exact RIP-relative `89 05 id` dword store, S3-20cx exact addr32 `67 8B 03` dword load, S3-20cy exact addr32 `67 89 03` dword store, S3-20cz exact addr32 disp8 `67 8B 43 ib` dword load, S3-20da exact addr32 disp32 `67 8B 83 id` dword load, S3-20db exact addr32 disp8 `67 89 43 ib` dword store, S3-20di exact address64 SIB disp8 dword store `89 44 0B/4B/8B/CB ib` (author-worktree `od-jit` 321/321; three existing `od-host` manual-corpus cases could not access ignored artifacts), S3-20dj exact address64 SIB disp32 dword load `8B 84 0B/4B/8B/CB id` (focused JIT/host and prior scale/disp8 JIT gates passed; no full `od-jit` run claimed), and S3-20dk exact address64 SIB disp32 dword store `89 84 0B/4B/8B/CB id` (exact new JIT/host gates and the preserved host/JIT regression gates passed in author worktree; no full `od-jit` run claimed), and S3-20dl exact addr32 SIB dword load `67 8B 04 0B/4B/8B/CB` (author-worktree `od-jit` 324/324; `od-host` 150/153 because three existing manual-corpus artifacts were absent), and S3-20dm exact addr32 SIB dword store `67 89 04 0B/4B/8B/CB` (author-branch `od-jit` 325/325; full `od-host` not run), and S3-20dn exact addr32 SIB disp8 dword load `67 8B 44 0B/4B/8B/CB ib` (author-worktree `od-jit` 326/326 before cherry-pick; full `od-host` not run), plus S3-20do exact addr32 SIB disp8 dword store `67 89 44 0B/4B/8B/CB ib` (author-worktree `od-jit` 327/327, `od-host` 153/156 due three missing ignored manual corpus/cache artifacts), S3-20co exact dword store and S3-20cq exact RAX alias; три sequences in S3-34h; S3-18e capacity-one cache correctness with fresh invocation state; S3-35c cold direct-entry, S3-36d warm direct-entry, and S3-36e cached full-call evidence on two frozen sequences | Fixture-bounded behavior only. Timed boundaries differ and cannot be subtracted into cache cost or break-even. No general memory compiler, MMIO/atomic path, production owner/MMU, or SMC evidence. См. [JIT contract](../specs/JIT_CONTRACT.md), [S3-18e](STAGE_3_S3-18e_2026-11-01.md), [S3-20cn](STAGE_3_S3-20cn_2026-11-01.md), [S3-20cp](STAGE_3_S3-20cp_2026-11-01.md), [S3-20co](STAGE_3_S3-20co_2026-11-01.md), [S3-20cq](STAGE_3_S3-20cq_2026-11-01.md), [S3-20cr](STAGE_3_S3-20cr_2026-11-01.md), [S3-20cs](STAGE_3_S3-20cs_2026-11-01.md), [S3-20ct](STAGE_3_S3-20ct_2026-11-01.md), [S3-20cu](STAGE_3_S3-20cu_2026-11-01.md), [S3-20cv](STAGE_3_S3-20cv_2026-11-01.md), [S3-20cx](STAGE_3_S3-20cx_2026-11-02.md), [S3-20cy](STAGE_3_S3-20cy_2026-11-02.md), [S3-20cz](STAGE_3_S3-20cz_2026-11-02.md), [S3-20da](STAGE_3_S3-20da_2026-11-02.md), [S3-20db](STAGE_3_S3-20db_2026-11-02.md), [S3-20di](STAGE_3_S3-20di_2026-11-02.md), [S3-20cl](STAGE_3_S3-20cl_2026-10-28.md), [S3-20cm](STAGE_3_S3-20cm_2026-10-29.md), [S3-35c](STAGE_3_S3-35c_2026-11-01.md), [S3-36d](STAGE_3_S3-36d_2026-11-01.md), [S3-36e](STAGE_3_S3-36e_2026-11-01.md), and [S3-40e](STAGE_3_S3-40e_2026-11-01.md). |
| Code invalidation, lifetime и state attribution | Fixture-bounded S3-23/24/25; S3-27c V3, S3-27d direct singleton V2, S3-27e two-site V2, S3-27f typed-IR qword PC maps, and S3-27g typed-IR dword PC-map attribution/publication lifetime | Не общий active-code retirement/SMP reclamation, произвольное host-fault recovery или production committed-writer ownership. См. [S3-27d](STAGE_3_S3-27d_2026-10-30.md), [S3-27e](STAGE_3_S3-27e_2026-11-01.md), [S3-27f](STAGE_3_S3-27f_2026-11-01.md), [S3-27g](STAGE_3_S3-27g_2026-11-01.md) и разделы S3-27 в [JIT contract](../specs/JIT_CONTRACT.md). |
| Events, STOP, flags и coordinator | Точные локальные S3-28 event/STOP, demand-flags и S3-29e coordinator tests прошли в harness; S3-29f добавил три timed release runs для фиксированных семи single-block cases | S3-29f не показал преимущества candidate на этой выборке; общий performance вывод и production default не установлены. См. [S3-34f](STAGE_3_S3-34f_2026-10-30.md), [S3-29f](STAGE_3_S3-29f_2026-10-30.md) и [S3-40c](STAGE_3_S3-40c_2026-10-30.md). |
| S3-30a allocator cost diagnostic | Три release runs на одном frozen six-instruction register-only multi-helper block сравнили register-homes budget4 с spill-only budget0: 60 compile и 240 execution pairs, все прошли 200 ms floor. Budget4/budget0 native medians ranged 43.227–43.520 / 44.376–45.271 ns/call по двум ZF states; code size был 3,052 / 3,040 bytes. | Ограниченная выборка без distinct branch routing (JZ+0 targets converge), pending-flags/max-pressure/memory/full-workload evidence или allocator winner. Stage2 load видна только в point-in-time snapshots; нет per-pair timestamps. S3-30 остаётся открытой. См. [S3-30a report](STAGE_3_S3-30a_2026-11-01.md) и [raw evidence](data/stage3/s3-30a/). |
| S3-30b distinct-branch allocator configuration cost | Три release runs, 60 raw pairs на каждый набор: budget0/budget4 ratio medians/ranges — compile 0.996036170827 (0.962808301520–1.045968410700), JZ-taken 1.019455351209 (0.981907318288–1.048155961189), JZ-fallthrough 1.038610853426 (1.008862994910–1.086055631502); все 360 timed sides прошли 200 ms floor. Budget4: 4 homes/10 spills, 3,052 bytes; budget0: 0 homes/14 spills, 3,040 bytes; frame обоих — 288 bytes. | Один Jcc-terminated block; suffix/dispatcher не измерялись. Только budget configuration cost, не marginal spill cost, allocator algorithm/winner/default или downstream behavior. S3-30 остаётся открытым. См. [S3-30b report](STAGE_3_S3-30b_2026-11-01.md) и [suite manifest/raw evidence](data/stage3/s3-30b/suite-manifest.json). |
| S3-30p3 allocator helper-clobber correctness | Test-only native probe теперь отдельно проверяет все шесть W64 значений (четыре register homes, две spill/reload values) после каждого из двух wrapper calls; focused test прошёл 1/1. | Закрывает только sum-only observation gap этого bounded helper probe; не доказывает pending flags, прочие pressure/path combinations или allocator policy/performance. S3-30 остаётся открытой. См. [S3-30p3 report](STAGE_3_S3-30p3_2026-11-01.md) и [test output](data/stage3/s3-30p3/focused-test.log). |
| S3-30p4 pending-flags allocator-pressure correctness | JZ/JNZ candidate/eager/interpreter comparison прошёл на four branch outcomes, `register_homes` 0..4 и budgets 0..4; captured helper operands/calls, allocation placements, entry STOP and STOP/budget continuation checked. Focused test 1/1. | Один bounded W64 fixture, не полное allocator/path coverage и без performance/default decision. S3-30 остаётся открытой. См. [S3-30p4 report](STAGE_3_S3-30p4_2026-11-01.md), [focused test](data/stage3/s3-30p4/focused-test.log) и [format/diff checks](data/stage3/s3-30p4/format-diff-check.log). |
| S3-29f eager/demand-flags single-block comparison | Три release runs завершили fixed seven-case profile; candidate/eager cross-run paired-ratio medians составили 1.003–1.086 по cases, код candidate был на 40–47% больше. Eager остаётся reference/default; общий performance вывод и policy choice не сделаны. См. [S3-29f](STAGE_3_S3-29f_2026-10-30.md). |
| VMHost/C ABI | S3-38a register-only V0 ABI integration, S3-38b original and current-source process-suite evidence, S3-38c sequential cross-thread handle lifecycle, and S3-38d full RESULT-state transport | Bounded local slices; S3-38e adds a bounded local Rust session owner, while VMHost IPC RUN/session integration remains absent. ISSUE-044 remains open; the current process regression does not establish its resolution. See [S3-38c](STAGE_3_S3-38c_2026-10-30.md), [S3-38b current-source addendum](STAGE_3_S3-38b-current-source_2026-11-01.md), [S3-38d](STAGE_3_S3-38d-local_2026-11-01.md), [S3-38e](STAGE_3_S3-38e_session_owner_2026-11-03.md), [issue registry](../KNOWN_ISSUES.md) and [diagnostics](STAGE_3_ISSUE-044_DIAGNOSTIC_2026-10-30.md). |
| Harness/replay/reducer | S3-34u pins four exact dword-load and typed-IR qword PC-map test gates; S3-34v pins two exact dword-store test gates; S3-34w pins three exact dword PC-map/publication gates; S3-34x pins three exact S3-20cp alias-load gates; S3-34y pins three exact S3-20cq alias-store gates; S3-34z pins two exact S3-20cr disp8-load site-binding and host gates; S3-34aa pins three exact S3-20cs disp32 load source-binding, PC-map, and host gates; S3-34ab pins three exact S3-20ct disp8-store source-binding, PC-map, and host gates; S3-34ac pins the S3-27h published-map integration test; S3-34ad pins three exact S3-20cu source-binding, PC-map, and host gates; S3-34ae pins the S3-27i canonical-end/range-overflow host gate; S3-34af pins the S3-27j published dword-store map integration; S3-34ag pins two S3-20cv RIP-relative dword-load JIT tests and its host test; S3-34ah pins three S3-20cw store identities; S3-34ai pins two S3-20cx JIT identities and its host test; S3-34aj pins two S3-20cy store identities and makes no PC-map claim; S3-34ak pins the S3-20cz source/site and host wrapped-EBX identities, also without a PC-map claim; S3-34al (harness commit `2751ba2`) pins the S3-20da source/site and host wrapped-EBX identities, without a PC-map claim; S3-34am (harness commit `c194891`) pins only the two S3-20db JIT-source and host identities, with no PC-map gate. Exact Rust gates passed 1/1 each, focused harness self-test passed 1/1, and the full Stage 3 harness passed 60/60; CPU discovery was not run for S3-34am. S3-34an (harness commit `11bf23b`) pins only the two S3-20dc disp32-store JIT source/site and host identities, also without a PC-map gate; both exact Rust gates and the focused harness self-test passed 1/1. The full Stage 3 harness/module and CPU discovery were not run for S3-34an. S3-34ao (harness commit `d3d2c05`) pins exactly the S3-20dd JIT admission and host full-width base/index identities, with no PC-map gate; both exact Cargo gates and focused harness self-test passed 1/1. Python byte-compilation, `git diff --check`, and Graphify query/update passed. S3-34aq (harness commit `b63f4df`) pins exactly the two S3-20df scaled-load JIT and host identities; both exact Cargo gates and focused harness self-test passed 1/1. No PC-map gate is pinned, and the full Stage 3 harness/module and CPU discovery were not run. S3-34ao remains the separate scale-1 S3-20dd gate. The full Stage 3 harness/module and CPU discovery were not run for S3-34ao. S3-34ap (harness commit `f767bd9`) pins exactly the S3-20de JIT store-admission and host full-width base/index identities; both exact Cargo gates and the focused harness self-test passed 1/1. The slice has no PC-map gate, and the full Stage 3 harness/module and CPU discovery were not run. Python byte-compilation, `git diff --check`, and Graphify query/update passed. S3-34ar (harness commit `5ab8dca`) pins exactly the two S3-20dg scaled-store JIT and host identities; both exact Cargo gates and focused harness self-test passed 1/1. No PC-map gate is pinned, and the full Stage 3 harness and CPU discovery were not run. Python byte-compilation, `git diff --check`, and Graphify query/update passed. S3-34ap remains the separate S3-20de scale-1 store gate; S3-34aq remains the S3-20df load gate. S3-34as (harness commit `ce18174`) pins exactly the S3-20dh disp8 dword-load JIT and host identities; both exact Cargo gates and focused harness self-test passed 1/1. No PC-map gate is pinned. Python byte-compilation, `git diff --check`, and Graphify query/update passed; the full harness/module and CPU discovery were not run. Prior scale-only harness gates remain separate. S3-34au (harness commit `4ef90b0`) pins exactly the S3-20dj disp32-load JIT source/site and host identities; both Cargo gates and the focused harness self-test passed 1/1. It adds no PC-map gate. Python byte-compilation, `git diff --check`, and Graphify query/update passed. The full Stage 3 harness/module and CPU discovery were not run; Mini, M1 Pro, and Stage 2 were not run. S3-34av (harness commit `27dba0e`) pins exactly the S3-20dk disp32-store JIT source/site and host identities; both exact Cargo gates and the focused harness self-test passed 1/1. It adds no PC-map gate. Python byte-compilation, `git diff --check`, and Graphify query/update passed. The full Stage 3 harness/module and CPU discovery were not run; earlier scale-only and disp8 SIB gates remain separate, and Mini, M1 Pro, and Stage 2 were not run. S3-34aw (harness commit `62a1332`) pins exactly the two S3-20dl addr32 SIB-load source/site and host identities; both exact Cargo gates and the focused harness self-test passed 1/1. It adds no PC-map gate. Python byte-compilation, `git diff --check`, and Graphify update passed. The full Stage 3 harness/module and CPU discovery were not run; prior S3-34ai/ak/al gates cover distinct addr32 load profiles, and Mini, M1 Pro, and Stage 2 were not run. S3-34ax (harness commit `0d33332`) pins exactly the S3-20dm JIT and host addr32 SIB-store identities; both exact Cargo gates passed 1/1 and the focused self-test passed. The Python validator module passed 71/71 with 170 gates, 160 acceptance cases, 10 lifecycle cases, and 188 steps. It adds no PC-map gate; the full Stage 3 harness, CPU discovery, and full hardware acceptance were not run. Python byte-compilation, `git diff --check`, and Graphify update passed; Mini, M1 Pro, and Stage 2 were not run. S3-34ay (integrated harness commit `ed413cd`) pins exactly the S3-20dn addr32 SIB disp8 JIT source/site and host identities; both Cargo gates and focused self-test passed 1/1. It adds no PC-map gate. Python byte-compilation, `git diff --check`, and Graphify update passed. Full Python module counts and CPU-discovery results were not reported; no full-module or hardware acceptance claim is made. S3-34az (integrated harness commit `c7d4e9b`) pins exactly the S3-20do addr32 SIB disp8 dword-store JIT source/site and host identities; both Cargo gates and the focused self-test passed 1/1. It adds no PC-map gate. Python byte-compilation, `git diff --check`, and Graphify update passed. Full Python module counts, CPU discovery, and hardware acceptance were not reported. The separate S3-20dc runtime commit is `28d837d`; its full `od-jit` 315/315 result belongs to author branch `feat/s3-addr32-dword-store-disp32` before cherry-pick and was not rerun on integrated main.  S3-34bm pins the exact S3-20eb RSP disp8 JIT source/site and host checked-state gates; both Rust gates passed 1/1, focused harness self-test 1/1, and the full Python harness module 86/86. The 200-gate manifest has 190 acceptance and 10 lifecycle gates with 218 local steps; Python byte-compilation, Graphify update, diff-check, and Sol review passed. Mini, M1, full `od-host`, and Stage 2 were not run. See [S3-34bm](STAGE_3_S3-34bm_2026-11-02.md). S3-34bn pins the S3-20ec RSP disp8 dword-store JIT source/site and host checked-state gates; both exact Rust gates passed 1/1 and the full Python harness passed 87/87 on integrated revision `2edeb07`. Its fail-closed self-test rejects missing gate/evidence, zero-match, ignored, failed, and wrong-identity output, and a final summary omitting the host gate. This is harness evidence only; full crate results are in the S3-20ec runtime report. See [S3-34bn](STAGE_3_S3-34bn_2026-11-02.md). S3-34bo pins the exact S3-20ed RSP disp32 dword-load JIT source/site and host checked-state gates; both exact Cargo gates passed 1/1 and the full Python harness passed 88/88 on integrated revision `238799d`. Its self-test rejects missing manifest/summary gates, zero-match, ignored, failed, and wrong-identity evidence, and a final summary omitting the host gate. This is harness evidence only; full `od-host` is not claimed. See [S3-34bo](STAGE_3_S3-34bo_2026-11-02.md). S3-34bq (harness code `28cd043`, integrated `1ba352a`) pins the exact S3-20ef address32 ESP load JIT source/site and host gates. At root `a0ce871`, with runtime `392886b` and subsequent 34br harness `ac069ca` integrated, both exact gates passed 1/1 and the cumulative Python harness passed 91/91 (including 34br); the self-test rejects missing gates/evidence, zero-match, ignored, failed, wrong-identity results, and host omission from the final summary. Harness Sol review passed; no full `od-host` claim. See [S3-34bq](STAGE_3_S3-34bq_2026-11-02.md). S3-34br (harness source `08e8ae2`, integrated `ac069ca`) pins the exact S3-20eg JIT source/site and host checked-state gates. At integrated root `8052314` with runtime `abd2af4`, both exact gates passed 1/1 and the cumulative Python harness passed 91/91 (34bq/34br). Its fail-closed self-test rejects missing or zero-match/ignored/failed/wrong-identity evidence and a final summary omitting the host gate. Manifest totals: 210 gates (200 acceptance, 10 lifecycle), 228 local steps. Harness Sol review passed; no full `od-host` or separate PC-map claim. The coverage-only limitation from runtime review is tracked in [ISSUE-045](../KNOWN_ISSUES.md); see the [S3-20eg report](STAGE_3_S3-20eg_2026-11-02.md) and [S3-34br report](STAGE_3_S3-34br_2026-11-02.md). Stage 2 remained outside this slice’s evidence; concurrent runs were ignored per user instruction. Mini/M1 remain deferred; Stage 3 remains partial. S3-34bs (harness source `e2befb4`, integrated `046f5a2`) pins the exact S3-20eh JIT source/site and host checked-state gates; at root `08b3eba` with runtime `62a516a`, both exact gates passed 1/1 and the cumulative Python harness passed 92/92 before later slices. Its fail-closed self-test rejects absent/zero-match/ignored/failed/wrong-identity gate evidence and a final summary omitting the host gate. Manifest totals: 212 gates (202 acceptance, 10 lifecycle), 230 local steps. Harness Sol review passed; no full `od-host` or separate PC-map claim. See [S3-34bs](STAGE_3_S3-34bs_2026-11-02.md). Stage 2 remained outside this slice’s evidence; its concurrent runs were ignored per user instruction. Mini/M1 remain deferred; Stage 3 remains partial. S3-34bu (harness `00430e9`, integrated `4790e76`) pins the exact S3-20ej JIT source/site and host checked-load gates; runtime `d649bd4` is integrated as `4e4fec2`. At root `4e4fec2`, both exact gates passed 1/1 and the full Python Stage 3 harness passed 94/94. The harness author branch separately reported 95/95; that result is not substituted for the verified root count. The fail-closed self-test checks the exact identities in the gate registries and manifest, source fingerprints, Cargo argv, evidence and final summary; it rejects missing/zero-match/ignored/failed/wrong-identity evidence and omission of the host gate. Manifest totals: 216 gates (206 acceptance, 10 lifecycle), 234 local steps. Harness Sol review passed; no full `od-host` or separate PC-map claim. See [S3-34bu](STAGE_3_S3-34bu_2026-11-02.md). Stage 2 remained outside this slice’s evidence; concurrent runs were ignored per user instruction. Mini/M1 remain deferred; Stage 3 remains partial. S3-34bv (harness source `7b1651f`, integrated `60f9e7a`) pins the S3-20ek JIT source/site and host wrapped-ESP identities. At root `b2f6bd8`, the exact JIT gate passed 1/1; the host gate remains pending due to the Mini deferral. The author's Python suite passed 95/95, while the root Python session ended before a final result, so no root full-suite pass is claimed. The fail-closed self-test rejects source-pin changes, missing gate/evidence, zero-match, ignored, failed, or wrong-identity test output, and omission of the host gate from the final summary. Manifest totals: 218 gates (208 acceptance, 10 lifecycle), 236 local steps. No full `od-host` or separate PC-map claim. See [S3-34bv](STAGE_3_S3-34bv_2026-11-02.md) and [S3-20ek runtime report](STAGE_3_S3-20ek_2026-11-02.md). Stage 2 remained outside this slice’s evidence; concurrent runs were ignored per user instruction. Mini/M1 remain deferred; Stage 3 remains partial. S3-34bw (harness source `4dc35bd4b6308943dd62055e2a8fdb2164c67872`, integrated `9f5ea21`) pins the S3-20el absolute SIB disp32 dword-load JIT source/site and host signed-absolute-address identities. The author worktree Python validation module passed 96/96 on the local Mac15,11 (Mini-class) while Mini checks were deferred; this was the same orchestration error and is not Mini/M1 acceptance. Manifest totals are 220 Rust gates (210 acceptance, 10 lifecycle) and 238 local steps. The fail-closed self-test rejects changed source fingerprints, missing/zero-match/ignored/failed/wrong-identity gate evidence, and omission of the host gate from the final summary. At the harness authoring/integration revision `9f5ea21`, the runtime was not yet integrated and neither Cargo gate was executed as part of 34bw. Runtime source `aa8e38f` was integrated later as `e564943`; no runtime gate result or behavior claim is attributed to this harness slice. Sol/high harness review passed with no findings. No root full-suite or full `od-host` result is claimed; Mini/M1/Stage 2 evidence is absent, and Stage 3 remains partial. See [S3-34bw](STAGE_3_S3-34bw_2026-11-02.md). S3-34bx (harness source `b165126c83378cbc69168f5201dc3ddabd340d48`, integrated `a70a022`) pins the exact S3-20em JIT source/site and host signed-absolute-address identities. Static manifest totals: 222 gates (212 acceptance, 10 lifecycle), 240 local steps; independent Sol/high harness review passed. No Python, Cargo, or runtime gate was run for this slice under the Mini deferral. Runtime source was integrated later as `5d8e9ac`; no Cargo gate result is claimed. ISSUE-045 remains open; Stage 3 remains partial. See [S3-34bx](STAGE_3_S3-34bx_2026-11-02.md). | Это проверки gate identities/fingerprints и локального gate evidence, не полный corpus/reducer или S3-34; полный S3-34 и physical comparison остаются открытыми. См. [S3-34u](STAGE_3_S3-34u_2026-11-01.md), [S3-34v](STAGE_3_S3-34v_2026-11-01.md), [S3-34w](STAGE_3_S3-34w_2026-11-01.md), [S3-34x](STAGE_3_S3-34x_2026-11-01.md), [S3-34y](STAGE_3_S3-34y_2026-11-01.md), [S3-34z](STAGE_3_S3-34z_2026-11-01.md), [S3-34aa](STAGE_3_S3-34aa_2026-11-01.md), [S3-34ab](STAGE_3_S3-34ab_2026-11-01.md), [S3-34ac](STAGE_3_S3-34ac_2026-11-01.md), [S3-34ad](STAGE_3_S3-34ad_2026-11-01.md), [S3-34ae](STAGE_3_S3-34ae_2026-11-01.md), [S3-34af](STAGE_3_S3-34af_2026-11-01.md), [S3-34ag](STAGE_3_S3-34ag_2026-11-01.md), [S3-34ah](STAGE_3_S3-34ah_2026-11-02.md), [S3-34ai](STAGE_3_S3-34ai_2026-11-02.md), [S3-34aj](STAGE_3_S3-34aj_2026-11-02.md), [S3-34ak](STAGE_3_S3-34ak_2026-11-02.md), [S3-34al](STAGE_3_S3-34al_2026-11-02.md), [S3-34am](STAGE_3_S3-34am_2026-11-02.md), and [S3-34h](STAGE_3_S3-34h_2026-10-30.md). |
| S3-36e cached checked-memory full-call profile | Three release runs on Apple M3 Max, 20 pairs per sequence/run, all sides passed 200 ms. Across 60 paired raw native/interpreter ratios, medians/ranges were 2.7223 (2.6348–2.8856) for `store_then_load` and 2.7650 (2.6390–2.9279) for `load_then_store`. | Ratios above 1 mean native full-call time exceeded interpreter time on these bounded fixtures; a useful target scenario is not yet proven. No cross-stratum or general memory claim. Monitoring has no per-pair attribution; Mini remains deferred. See [S3-36e](STAGE_3_S3-36e_2026-11-01.md) and [evidence manifest](data/stage3/s3-36e/suite-manifest.json). |
| S3-36f cached register recurrence full-call profile | Three release runs on Apple M3 Max; 20 pairs per stratum/run across six fixed strata, all sides passed 200 ms. Across 60 paired ratios per stratum, native/interpreter medians were 4.7661 (`a_1`), 4.8890 (`a_64`), 4.8938 (`a_4096`), 3.7205 (`b_1`), 3.8092 (`b_64`), and 3.7805 (`b_4096`); every observed paired ratio exceeded 1. | Interpreter was faster on this synthetic recurrence; no useful native target scenario, broad guest claim, or runtime/cache policy change follows. First dispatch includes compile/publication/execution and is not decomposed into compile-only cost. Runtime `working_tree_dirty` is unknown because the probe timed out; Mini remains deferred. See [S3-36f](STAGE_3_S3-36f_2026-11-01.md) and [evidence manifest](data/stage3/s3-36f-20261101/suite-manifest.json). |
| S3-31b cached NativeRegisterV1 phase attribution | Three release runs, 20 alternating pairs per six S3-36f strata/run; all 720 sides passed 200 ms. Median observed/ordinary ratios ranged 1.0900–1.1182; the `b_64` paired range crossed parity. Median observed-path phase shares: block build 21–22% / 32–33%, key/snapshot/first lookup 43–44% / 39%, host execution boundary 4–5% / 5–6% for forms A / B. | Synthetic recurrence only. Shares include instrumentation and do not represent production phase proportions; `KeySnapshotAndLookup` excludes repeated validation and `HostExecutionBoundary` includes the wrapper. Net paired delta is not isolated observer cost. No linking benefit/default change or broad S3-31 closure. Runtime dirty status unknown; Mini remains deferred. See [S3-31b](STAGE_3_S3-31b_2026-11-01.md) and [manifest](data/stage3/s3-31b-20261101/suite-manifest.json). |
| S3-31c run-local prepared-key reuse | Three release processes, 20 paired triples in each of six synthetic strata per process; all 1,080 timed sides passed 200 ms. Candidate/baseline median ratios were 0.2591–0.3532 at 64/4,096 iterations and 1.1352–1.1998 at one iteration. Candidate/interpreter medians were below 1 only for `b_64` and `b_4096`. | Synthetic recurrence only. No broad workload speedup, runtime/default-policy change, or broad S3-31 closure; candidate remains opt-in. Timed source tree is clean at the recorded revision, timed runtime working-tree status is unknown; untimed preflight uses the same clean revision/fingerprint from a temporary detached worktree, and Mini remains deferred. See [S3-31c](STAGE_3_S3-31c_2026-11-01.md) and [manifest](data/stage3/s3-31c-20261101/suite-manifest.json). |

## Оставшиеся gates

| Gate | Статус |
|---|---|
| Тройное сравнение interpreter/JIT/physical x86 для обязательных принятых профилей (S3-21) | Не выполнено; Mac mini проверка отложена до конца локальной работы Stage 3. |
| S3-18d1 dispatch/cache cold-sweep observation | Выполнены три локальных release runs на одном frozen corpus; это partial evidence, не full cold compile+first-execution comparison. См. [отчёт](STAGE_3_S3-18d1_2026-10-30.md). |
| S3-18e bounded checked-memory cache | Runtime slice committed and focused local checks passed for the two frozen memory-pair profiles. It retains one published handle with per-invocation CPU/RAM/control state; this is fixture-only correctness evidence, without production owner/MMU/SMC lifecycle or performance conclusions. См. [отчёт](STAGE_3_S3-18e_2026-11-01.md). |
| S3-35a cold first corpus comparison | Три release runs завершены: cold FIFO block dispatcher 6.64–6.69 ms/sweep, interpreter 0.164–0.166 ms/sweep на одном frozen register corpus. Это bounded first-run результат, не общий speed claim. См. [отчёт](STAGE_3_S3-35a_2026-10-30.md). |
| S3-36a resident warm register-only comparison | Три release runs завершены на resident prefix из 14 frozen cases: FIFO 12.129–12.343 µs/sweep, interpreter 2.019–2.053 µs/sweep. Interpreter был примерно в 6.0× быстрее на этом slice; branch/helper/memory matrix не измерена. См. [отчёт](STAGE_3_S3-36a_2026-10-30.md). |
| S3-36b branch/helper warm fixture comparison | Три release runs на пяти accepted S3-14 fixtures: FIFO 3.736–3.959 µs/sweep, interpreter 0.726–0.751 µs/sweep; interpreter примерно в 5.3× быстрее на этом профиле. Memory и полный workload matrix не измерены. См. [отчёт](STAGE_3_S3-36b_2026-10-30.md). |
| S3-36c resident warm NOP-fallback comparison | Три sequential release runs на 19 frozen seed/index cases, с 20 парами на case: run-level median-of-case-medians cached/interpreter ratios ranged 5.223–5.259 для two-native+NOP stratum и 4.313–4.376 для three-native+NOP stratum. Stage 2 cargo/rustc activity была видна во всех runs; point-in-time Graphify CPU sample был в run 3. Pair timestamps отсутствуют. Результат относится только к этим `BudgetExhausted` inputs, не к общей fallback/memory/full-workload производительности. См. [S3-36c](STAGE_3_S3-36c_2026-10-31.md) и [evidence](data/stage3/s3-36c/). |
| S3-36d resident warm direct-entry checked-memory profile | Три release runs на двух frozen S3-34a sequences (1,200 inputs each): 60 pairs per sequence across runs, all sides passed 200 ms. Raw-total native/interpreter ratio medians/ranges: `store_then_load` 1.3718847445 (1.3116083380–1.4203506060), `load_then_store` 1.3662312961 (1.2865399535–1.4421564464). No cross-stratum aggregate. Stage 2/Graphify observations are point-in-time; no per-pair timestamps, runtime dirty status unknown. This is bounded warm direct-entry evidence with no dispatcher/cache, not general memory performance. See [S3-36d](STAGE_3_S3-36d_2026-11-01.md) and [evidence](data/stage3/s3-36d/). |
| S3-35b cold FIFO pipeline attribution | Три release-process runs на одном frozen register-only corpus: ordinary dispatch median 6.752 ms/sweep, observed 6.986 ms/sweep; paired observed/ordinary ratio median 1.0301 (IQR 1.0261–1.0342, full range 0.9831–1.1075). Per-phase attribution измерена; это ограниченный cold ARM64 slice, не общий performance вывод или выбор default. См. [отчёт](STAGE_3_S3-35b_2026-10-31.md). |
| S3-31 warm dispatcher phase attribution | Три bounded release runs на двух отдельных frozen S3-17 strata; 60 raw paired ratios per stratum, все sample-side floors пройдены. Observed/ordinary ratio medians/ranges: `recorded_multiblock` 1.098251684 (1.064753209–1.118670359), `self_loop` 1.117014264 (1.093563537–1.159989642); phase shares и counters зафиксированы в [отчёте](STAGE_3_S3-31_2026-11-01.md). Ratio — net difference двух full call paths, не pure observer cost; total lookup, linking benefit and broad S3-31 acceptance remain open. См. [suite manifest](data/stage3/s3-31/suite-manifest.json). |
| S3-35c cold direct-entry checked-memory profile | Три release runs на двух frozen S3-34a sequences (2,400 inputs): 60 paired samples, все floors прошли; raw-total native/interpreter ratio median 71.4765, range 66.7736–93.9423, без trimming. Stage 2 был виден во всех monitoring envelopes; per-pair timing отсутствует. Это узкий direct-entry compile+publish+execute+drop fixture, не broad memory/full-workload claim и не default/winner. См. [отчёт](STAGE_3_S3-35c_2026-11-01.md) и [архив](data/stage3/s3-35c/). |
| S3-37a bounded FIFO16 churn observation | Три процесса по 128 sweeps на frozen register corpus: одинаковые 168,944 evictions и resident high-water 16 entries / 21,640 code bytes / 50,124 logical metadata bytes. RSS, teardown, SMC и broader workloads не измерены; это не leak-free/lifecycle evidence. См. [отчёт](STAGE_3_S3-37a_2026-10-30.md). |
| S3-37b process-wide RSS snapshots | Три процесса завершили 128 sweeps; baseline RSS 5,024–5,056 KiB, до drop 5,808–6,000 KiB, после drop 5,744–5,936 KiB. RSS остался выше baseline и не атрибутируется cache/runtime; lifecycle/leak-free gate не закрыт. См. [отчёт](STAGE_3_S3-37b_2026-10-30.md). |
| S3-37d sustained FIFO16 lifecycle stress | Три v5 scored runs, each using one runtime and one arena at a time: 45,436,794 epochs (671.161 s; run 1 exit code unobserved) and 46,049,027 epochs in two runs (679.267 s and 688.094 s; both exit 0). All included 128 checkpoints and interpreter parity. The reported Stage2-quiet windows describe those runs, not a requirement for Stage2-quiet execution; Stage 2 launches/activity are not blockers. This is bounded counters and fixture lifecycle evidence, not production SMC/owner/MMU, concurrent arenas/SMP, or leak-free behavior. Mini remains deferred. Exact evidence is in the [S3-37d report](STAGE_3_S3-37d_2026-10-31.md). |
| S3-40a предварительная сводка решений | Исходный ledger зафиксировал сравнения allocator, flags, cache, linking и IR; timed flags evidence добавлено позднее в S3-29f/S3-40c. Финальный production выбор/default не сделан; S3-40 остаётся открытым. Cold full corpus и warm resident prefix нельзя объединять в break-even. См. [S3-40a](STAGE_3_S3-40a_2026-10-30.md), [S3-29f](STAGE_3_S3-29f_2026-10-30.md) и [S3-40c](STAGE_3_S3-40c_2026-10-30.md). |
| S3-40b дополнение к предварительному реестру | На момент дополнения добавлены отдельные bounded branch/helper warm результаты S3-36b и process-wide RSS snapshots S3-37b. Позднее S3-35b добавил cold pipeline attribution, а S3-37d — sustained lifecycle/counter evidence; production default, общий break-even и более широкие component-timing/resource/lifecycle gates остаются открытыми. См. [S3-40b](STAGE_3_S3-40b_2026-10-30.md), [S3-35b](STAGE_3_S3-35b_2026-10-31.md) и [S3-37d](STAGE_3_S3-37d_2026-10-31.md). |
| S3-40c flags strategy evidence | S3-29f single-block ratios не показали преимущества demand-flags candidate и зафиксировали больший код; сравнение ограничено семью случаями и не выбирает production default. См. [отчёт](STAGE_3_S3-40c_2026-10-30.md). |
| S3-40d composed cached flags comparison | Три release runs на frozen S3-34p corpus: median paired candidate/eager ratios 1.020–1.029 cold и 1.014–1.025 repeat; разброс crossing parity, candidate публикует на 14.27%/13.07% больше кода. Нет основания менять eager default или выбирать winner. Run2 принят как noisy bounded; run3 включал low-load Stage 2 activity; высоконагруженная попытка сохранена как diagnostic. Числовой exit status runs 2/3 не captured. См. [S3-40d report](STAGE_3_S3-40d_2026-10-31.md) и [raw evidence](data/stage3/s3-40d/). |
| S3-40e checked-memory timing ledger | Сопоставлены в ledger bounded S3-35c cold direct-entry, S3-36d warm direct-entry и S3-36e cached full-call profiles. Их native timed boundaries различаются, поэтому их ratios нельзя вычитать для оценки cache cost или break-even. Two-sequence fixture only; no general memory claim or production policy. См. [S3-40e](STAGE_3_S3-40e_2026-11-01.md), [S3-35c](STAGE_3_S3-35c_2026-11-01.md), [S3-36d](STAGE_3_S3-36d_2026-11-01.md) и [S3-36e](STAGE_3_S3-36e_2026-11-01.md). |
| S3-40f allocator decision addendum | Синтезированы отдельно S3-30a same-target и S3-30b distinct-target budget4-vs-budget0 configuration-cost evidence, плюс bounded correctness S3-30p3/p4. Текущий homes4 baseline/default остаётся: evidence недостаточно для смены allocator/default. Следующий gate — representative hot workload и мотивированная альтернатива, сравнённые на одинаковых условиях. Это не algorithm comparison или finding, что diagnostic budget0 лучше; fixtures и timed boundaries не объединяются. Полный S3-30/S3-40 остаётся открыт; production owner/MMU, broader workloads и Mini/physical gates не закрыты. См. [S3-40f](STAGE_3_S3-40f_2026-11-01.md). |
| S3-40g aliased ADD allocator diagnostic | Один frozen 27-byte W64 `ADD RAX,RAX` fixture: три sequential release processes, 60 pooled pairs на set, все 360 timed sides прошли 200 ms floor. Все pooled median ratios ниже 1, но диапазоны пересекают parity; fixture-only evidence не подтверждает speedup, allocator winner или default. S3-32 и S3-40 остаются открыты. См. [S3-40g](STAGE_3_S3-40g_2026-11-01.md) и [S3-32a report](STAGE_3_S3-32a_2026-11-01.md). |
| S3-40j S3-31c report validator | Profile-specific offline audit recomputed candidate/baseline and candidate/interpreter ratio summaries from raw elapsed samples; validated six strata × 60 pairs, schedule/workload/source consistency, and untimed preflight metadata. The saved audit returned `passed`; focused suite reported 16 tests OK and independent code review passed. | Audit of S3-31c evidence only; no new benchmark, full S3-40 decision or Stage 3 completion. See [S3-31c report audit](STAGE_3_S3-31c_2026-11-01.md#bounded-s3-40-audit-of-the-s3-31c-reports) and [validator audit JSON](data/stage3/s3-31c-20261101/validator-audit.json). |
| Полные cold/warm measurements, resource/lifecycle gates и решения о default (S3-35…37, S3-40) | Не выполнены. S3-35b дал bounded cold register-only pipeline attribution, S3-35c — узкий direct-entry checked-memory cold profile; S3-36c и S3-36d — отдельные native-plus-NOP budget-exit и resident warm direct-entry checked-memory срезы; S3-36e добавил cached checked-memory full-call, а S3-36f — cached register recurrence full-call. S3-31b отдельно атрибутировал три неперекрывающиеся фазы на тех же шести synthetic kernels, но не устанавливает total dispatcher/lookup cost на representative guest workloads; эта широкая gate остаётся открытой. Timed boundaries остаются отдельными; из них нельзя вычислить общую cache cost или break-even. В профилях S3-36e/f native/interpreter ratios выше 1. В новых S0-09 countdown профилях JIT быстрее для 64 и 4096 итераций с одним RIP в обеих фазах. Короткие cold и оба two-RIP профиля быстрее на интерпретаторе. Сравнения и решение о сохранении изменений приведены в [S3-40k](../plans/STAGE_3.md#s3-40k-повторные-входы-в-s0-09-dispatcher). S3-37d добавил три v5 sustained FIFO16 lifecycle/counter runs. S3-37f проверил 64 обхода заполненного кеша штатного Session, STOP и восстановление аренды после clear и drop. См. [S3-37f](../plans/STAGE_3.md#s3-37f-давление-на-кеш-штатного-s0-09-session). General memory/fallback warm workloads, production teardown/independent-instance evidence, SMC stress, broader RSS attribution и выбор default остаются открытыми; bounded результаты не переносятся на другие workloads. |
| Production RAM/backing/mapping/MMU owner и committed-writer integration | Не входит в текущий local fixture scope; не реализован этим отчётом и не засчитывается как локальное доказательство. |
| ISSUE-044 | Открыта: прежний timeout не локализован; три диагностических наблюдения не установили причину или исправление. Не повторять suite без нового основания. |
| M1 Pro (S3-39) | Ограниченный ARM64 smoke завершён на MacBookPro18,3 / macOS 14.6.1 для arithmetic, taken multi-block branch, accepted S3-14 helper, cooperative STOP и same-handle C ABI publication lifecycle. Это не широкая гарантия host support и не заменяет physical x86 oracle. См. [S3-39 report](STAGE_3_S3-39_M1PRO_2026-10-31.md). |
| Итоговая приёмка S3-41 | Открыта. Stage 3 нельзя объявлять завершённым по этой локальной сводке. |

## S3-20fy and S3-34dj status

S3-20fy implements the exact address-size-32 RCX-base Dword store with signed
disp8 (`67 89 41 ib`), and S3-34dj pins its JIT and host gate identities. The
shared host fixture helper accepts the exact addr32 singleton `67 89 01` and
disp8 form `67 89 41 ib`; it applies signed disp8 modulo-32 arithmetic to the
starting EA while retaining full RCX and preserving adjacent address64 store
contracts. Runtime and harness independent Sol/high reviews passed. Behavior,
build, formatting, Cargo, Python harness, and Mini checks remain deferred until
the end of Stage 3; M1 regression is deferred by the user. See
[S3-20fy](STAGE_3_S3-20fy_2026-11-02.md) and
[S3-34dj](STAGE_3_S3-34dj_2026-11-02.md).

## S3-20fz and S3-34dk status

S3-20fz specifies the exact addr32 RCX-base Dword load with signed disp32
(`67 8B 81 id`): wrapped low32 RCX plus signed i32 displacement, one
contiguous checked four-byte read, zero-extension to RAX, preserved RCX/GPRs/
RFLAGS, and successful boundary `A+7`. Runtime commits `ac69bcd`/`baba8f8`
and harness commit `2de1cab` are integrated; runtime and harness passed
independent Sol/high static review. S3-34dk pins the exact gate identities.
Manifest totals 300 Rust gates (290 acceptance + 10 lifecycle), 318 local
steps, and 134 self-test names are pinned and statically reviewed but not
executed. Behavior, build, formatting, Cargo, Python harness, and Mini checks
were not run and remain deferred. Published IR uses immutable compiler-issued
source without fetching guest instruction bytes; instruction-fetch errors are
checked through the demand interpreter path. M1 regression is deferred by the
user. Stage 2 and production owner/MMU remain outside scope, and Stage 3
remains partial. See
[S3-20fz](STAGE_3_S3-20fz_2026-11-02.md) and
[S3-34dk](STAGE_3_S3-34dk_2026-11-02.md).

## S3-20ga and S3-34dl status

S3-20ga plans the exact address32 RCX-base Dword store `67 89 81 id` with
signed modulo-32 starting-EA arithmetic, one contiguous checked write4 of
low32 RAX, preserved GPR/RFLAGS, and successful boundary `A+7`. Scope fences
keep the load, singleton/disp8 neighbors, other bases, SIB, and added prefixes
separate. Runtime must use immutable compiler-issued source without guest
instruction fetch; demand-interpreter fetch errors propagate directly.
Runtime commit `b7b361b` and harness commit `c2a3e8f` are integrated and both
passed independent Sol/high static review. Exact identities and totals of 302
gates (292 acceptance + 10 lifecycle), 320 local steps, and 135 self-test
names are pinned and reviewed, but not executed. No behavior tests, build,
formatting, Cargo, Python harness, or Mini checks were run. Published IR uses
immutable compiler-issued source without runtime guest instruction fetch;
instruction-fetch errors are checked through the demand interpreter path.
Stage 2 and production owner/MMU remain out of scope; Stage 3 remains partial.
See [S3-20ga](STAGE_3_S3-20ga_2026-11-02.md) and
[S3-34dl](STAGE_3_S3-34dl_2026-11-02.md).

## Источники

- [План Stage 3](../plans/STAGE_3.md) и [дорожная карта](../ROADMAP.md#этап-3-первый-jit-x86--arm64).
- [JIT contract](../specs/JIT_CONTRACT.md) и [JIT testing](../testing/JIT.md).
- [S3-27d direct V2 PC maps](STAGE_3_S3-27d_2026-10-30.md),
  [S3-27e two-site V2 PC-map attribution](STAGE_3_S3-27e_2026-11-01.md),
  [S3-27f typed-IR qword PC-map comparator and publication lifetime](STAGE_3_S3-27f_2026-11-01.md),
  [S3-27g dword PC-map contracts](STAGE_3_S3-27g_2026-11-01.md),
  [S3-34u exact local harness gates](STAGE_3_S3-34u_2026-11-01.md),
  [S3-34v exact dword-store harness gates](STAGE_3_S3-34v_2026-11-01.md),
  [S3-34w exact dword PC-map harness gates](STAGE_3_S3-34w_2026-11-01.md),
  [S3-34x exact aliased dword-load harness gates](STAGE_3_S3-34x_2026-11-01.md),
  [S3-34y exact aliased dword-store harness gates](STAGE_3_S3-34y_2026-11-01.md),
  [S3-34z exact disp8 dword-load harness gates](STAGE_3_S3-34z_2026-11-01.md),
  [S3-34aa exact disp32 dword-load harness gates](STAGE_3_S3-34aa_2026-11-01.md),
  [S3-34ab exact disp8 dword-store harness gates](STAGE_3_S3-34ab_2026-11-01.md),
  [S3-34ac exact published dword-load PC-map integration gate](STAGE_3_S3-34ac_2026-11-01.md),
  [S3-34ad exact disp32 dword-store harness gates](STAGE_3_S3-34ad_2026-11-01.md),
  [S3-34ae exact canonical-end/range-overflow harness gate](STAGE_3_S3-34ae_2026-11-01.md),
  [S3-34af exact published dword-store PC-map integration gate](STAGE_3_S3-34af_2026-11-01.md),
  [S3-34ag exact RIP-relative dword-load harness gates](STAGE_3_S3-34ag_2026-11-01.md),
  [S3-34ah exact RIP-relative dword-store harness gates](STAGE_3_S3-34ah_2026-11-02.md),
  [S3-34ai exact address-size-32 dword-load harness gates](STAGE_3_S3-34ai_2026-11-02.md),
  [S3-34aj exact address-size-32 dword-store harness gates](STAGE_3_S3-34aj_2026-11-02.md),
  [S3-34ak exact S3-20cz addr32 disp8 dword-load harness gates](STAGE_3_S3-34ak_2026-11-02.md),
  [S3-34al exact S3-20da source-site and host harness gates](STAGE_3_S3-34al_2026-11-02.md),
  [S3-34am exact S3-20db addr32 disp8 dword-store harness gates](STAGE_3_S3-34am_2026-11-02.md),
  [S3-34an exact S3-20dc addr32 disp32 dword-store harness gates](STAGE_3_S3-34an_2026-11-02.md),
  [S3-34ao exact S3-20dd address64 SIB dword-load harness gates](STAGE_3_S3-34ao_2026-11-02.md),
  [S3-34ap exact S3-20de address64 SIB dword-store harness gates](STAGE_3_S3-34ap_2026-11-02.md),
  [S3-34aq exact S3-20df address64 SIB dword-load scale 2/4/8 harness gates](STAGE_3_S3-34aq_2026-11-02.md),
  [S3-34ar exact S3-20dg address64 SIB dword-store scale 2/4/8 harness gates](STAGE_3_S3-34ar_2026-11-02.md),
  [S3-34as exact S3-20dh address64 SIB disp8 dword-load harness gates](STAGE_3_S3-34as_2026-11-02.md),
  [S3-34au exact S3-20dj address64 SIB disp32 dword-load harness gates](STAGE_3_S3-34au_2026-11-02.md),
  [S3-34av exact S3-20dk address64 SIB disp32 dword-store harness gates](STAGE_3_S3-34av_2026-11-02.md),
  [S3-34aw exact S3-20dl address-size-32 SIB dword-load harness gates](STAGE_3_S3-34aw_2026-11-02.md),
  [S3-34ax exact S3-20dm address-size-32 SIB dword-store validator gates](STAGE_3_S3-34ax_2026-11-02.md),
  [S3-34ay exact S3-20dn address-size-32 SIB disp8 dword-load harness gates](STAGE_3_S3-34ay_2026-11-02.md),
  [S3-34az exact S3-20do address-size-32 SIB disp8 dword-store harness gates](STAGE_3_S3-34az_2026-11-02.md),
  [S3-20cn exact dword checked-memory load](STAGE_3_S3-20cn_2026-11-01.md),
  [S3-20cp exact aliased dword checked-memory load](STAGE_3_S3-20cp_2026-11-01.md),
  [S3-20co exact dword checked-memory store](STAGE_3_S3-20co_2026-11-01.md),
  [S3-20cq exact aliased dword checked-memory store](STAGE_3_S3-20cq_2026-11-01.md),
  [S3-20cr exact dword checked-memory load with disp8](STAGE_3_S3-20cr_2026-11-01.md),
  [S3-20cs exact dword checked-memory load with disp32](STAGE_3_S3-20cs_2026-11-01.md),
  [S3-20ct exact dword checked-memory store with disp8](STAGE_3_S3-20ct_2026-11-01.md),
  [S3-20cu exact dword checked-memory store with disp32](STAGE_3_S3-20cu_2026-11-01.md),
  [S3-20cv exact RIP-relative dword checked-memory load](STAGE_3_S3-20cv_2026-11-01.md),
  [S3-20cx exact address-size-32 dword checked-memory load](STAGE_3_S3-20cx_2026-11-02.md),
  [S3-20cy exact address-size-32 dword checked-memory store](STAGE_3_S3-20cy_2026-11-02.md),
  [S3-20cz exact address-size-32 disp8 dword checked-memory load](STAGE_3_S3-20cz_2026-11-02.md),
  [S3-20da exact address-size-32 disp32 dword checked-memory load](STAGE_3_S3-20da_2026-11-02.md),
  [S3-20db exact address-size-32 disp8 dword checked-memory store](STAGE_3_S3-20db_2026-11-02.md),
  [S3-20dc exact address-size-32 disp32 dword checked-memory store](STAGE_3_S3-20dc_2026-11-02.md),
  [S3-20dd exact address64 SIB dword checked-memory load](STAGE_3_S3-20dd_2026-11-02.md),
  [S3-20df exact address64 SIB dword checked-memory loads at scales 2/4/8](STAGE_3_S3-20df_2026-11-02.md),
  [S3-20dg exact address64 SIB dword checked-memory stores at scales 2/4/8](STAGE_3_S3-20dg_2026-11-02.md),
  [S3-20dh exact address64 SIB dword checked-memory loads with disp8](STAGE_3_S3-20dh_2026-11-02.md),
  [S3-20di exact address64 SIB dword checked-memory stores with disp8](STAGE_3_S3-20di_2026-11-02.md),
  [S3-20dj exact address64 SIB dword checked-memory loads with disp32](STAGE_3_S3-20dj_2026-11-02.md),
  [S3-20dk exact address64 SIB dword checked-memory stores with disp32](STAGE_3_S3-20dk_2026-11-02.md),
  [S3-20dl exact address-size-32 SIB dword checked-memory loads](STAGE_3_S3-20dl_2026-11-02.md),
  [S3-20dm exact address-size-32 SIB dword checked-memory stores](STAGE_3_S3-20dm_2026-11-02.md),
  [S3-20dn exact address-size-32 SIB dword checked-memory loads with disp8](STAGE_3_S3-20dn_2026-11-02.md),
  [S3-20do exact address-size-32 SIB dword checked-memory stores with disp8](STAGE_3_S3-20do_2026-11-02.md),
  [S3-20dp exact address-size-32 SIB dword checked-memory loads with disp32](STAGE_3_S3-20dp_2026-11-02.md),
  [S3-20de exact address64 SIB dword checked-memory store](STAGE_3_S3-20de_2026-11-02.md),
  [S3-20cw exact RIP-relative dword checked-memory store](STAGE_3_S3-20cw_2026-11-01.md),
  [S3-27i disp32 dword-store canonical-end/range-overflow checks](STAGE_3_S3-27i_2026-11-01.md),
  [S3-27j published dword-store PC-map lifetime](STAGE_3_S3-27j_2026-11-01.md),
  [S3-27h published dword load PC-map lifetime](STAGE_3_S3-27h_2026-11-01.md),
  [S3-31c run-local prepared-key reuse](STAGE_3_S3-31c_2026-11-01.md),
  [S3-31c suite manifest](data/stage3/s3-31c-20261101/suite-manifest.json),
  [S3-31c validator audit](data/stage3/s3-31c-20261101/validator-audit.json),
  [S3-30a allocator cost diagnostic](STAGE_3_S3-30a_2026-11-01.md),
  [S3-30b distinct-branch allocator configuration cost](STAGE_3_S3-30b_2026-11-01.md),
  [S3-30b raw-pair suite manifest](data/stage3/s3-30b/suite-manifest.json),
  [S3-30p3 allocator helper-clobber correctness](STAGE_3_S3-30p3_2026-11-01.md),
  [S3-30p4 pending-flags allocator-pressure correctness](STAGE_3_S3-30p4_2026-11-01.md),
  [S3-35c cold direct-entry checked-memory profile](STAGE_3_S3-35c_2026-11-01.md),
  [S3-36d warm direct-entry checked-memory profile](STAGE_3_S3-36d_2026-11-01.md),
  [S3-34f local gates](STAGE_3_S3-34f_2026-10-30.md),
  [S3-34h memory corpus](STAGE_3_S3-34h_2026-10-30.md),
  [S3-39 M1 Pro smoke](STAGE_3_S3-39_M1PRO_2026-10-31.md),
  [S3-40d composed cached flags comparison](STAGE_3_S3-40d_2026-10-31.md),
  [S3-40f allocator decision addendum](STAGE_3_S3-40f_2026-11-01.md),
  [S3-40g aliased ADD allocator diagnostic](STAGE_3_S3-40g_2026-11-01.md),
  [S3-38c sequential-thread lifecycle](STAGE_3_S3-38c_2026-10-30.md),
  [S3-38b VMHost process suite](STAGE_3_S3-38b_2026-10-30.md),
  [S3-38b current-source process regression](STAGE_3_S3-38b-current-source_2026-11-01.md),
  [S3-38d RESULT-state transport](STAGE_3_S3-38d-local_2026-11-01.md) и
  [ISSUE-044 diagnostics](STAGE_3_ISSUE-044_DIAGNOSTIC_2026-10-30.md).
- [Открытые проблемы](../KNOWN_ISSUES.md).

S3-34dm adds a deterministic differential corpus for the existing S3-20eg
singleton store `67 89 04 24`: three fixed seeds and a local SplitMix64 stream
produce 400 unique CPU/RAM inputs per seed,
with two reset-state executions per backend and full CPU/RAM, `A+4`, retirement,
single contiguous callback, and canary checks. Zero, unaligned, and 4-GiB
crossing address classes are included. The Rust identity is
`tests::fixed_seed_addr32_rsp_dword_store_corpus_matches_interpreter_twice_per_input`;
the harness gate is `s3_34dm_addr32_rsp_dword_store_fixed_seed_corpus`.
Corpus commit `c7bb9c7` and harness commit `2fb1265` are integrated; S3-34.1,
S3-34.3, and S3-34.5 passed independent static Sol/high review. The exact
`od-host` corpus gate passed 1/1 and the fail-closed Python harness self-test
exited 0. The local macOS/aarch64 run covers 1,200 unique initial states twice
per backend against the interpreter. At the S3-34dm cut, pinned totals were
303 gates (293 acceptance + 10 lifecycle), 321 steps, and 136 self-test names.
The current manifest after S3-34dr reports 324 gates (306 acceptance + 18
lifecycle), 342 steps, 149 self-test names, and 18 source pins; the full
manifest was not run. S3-34.2 broader execution acceptance remains pending.
ISSUE-045 was resolved separately. Physical x86/Mini, full Stage 3,
production owner/MMU and performance acceptance remain open; M1 is deferred.
No opcode/runtime semantics were added. See the
[S3-34dm report](STAGE_3_S3-34dm_2026-11-02.md).

S3-34dn implements one create-only mismatch artifact and validated replay for
the same exact S3-34dm profile. Runtime commits `f858b7a`/`10447f0`/`ee2af23`
and harness commits `89c5e28`/`13b7790` are integrated and passed independent
static Sol/high review. Commit `ee2af23` adds self-check assertions for
create-only/no-overwrite behavior and invalid-version/RIP rejection; these
assertions remain unexecuted. The self-check identity is
`s3_34dn_replay_artifact_self_checks_roundtrip_and_fail_closed`; the optional
saved-artifact replay identity is `replay_saved_s3_34dn_mismatch_artifact`.
Harness gate `s3_34dn_replay_artifact_self_check` and fail-closed self-test
`test_s3_34dn_replay_artifact_self_check_gate_is_pinned_and_fail_closed` are
pinned. Totals are 304 gates (294 acceptance + 10 lifecycle), 322 steps, and
137 self-test names. All are unexecuted; no tests or self-checks were run.

Replay validates the 15-field versioned schema, exact profile/source/RIP and
input constraints, sparse RAM layout, and RAM-layout FNV-1a 64-bit digest.
It replays saved CPU/RAM state, retaining seed/index only as provenance, and
requires deterministic two-repeat interpreter/JIT observations for
`reproduced`. A valid replay without the captured mismatch reports
`not_reproduced`; invalid artifacts fail validation. The optional replay test
also reports `not_reproduced` when no artifact is supplied, which is not replay
evidence. Its synthetic mismatch self-check is labeled and does not establish
runtime behavior. It covers serialization/classification, selected
profile/source/address/layout/digest rejection, create-only/no-overwrite, and
invalid-version/RIP rejection. S3-34do subsequently implemented the reducer;
S3-34.2 execution acceptance remains pending. ISSUE-045 remains open until
S3-20eg execution acceptance; ISSUE-046 remains open pending the exact S3-34dn
gate run. No behavior, runtime API, dependency, or opcode claim is added. See the
[S3-34dn report](STAGE_3_S3-34dn_2026-11-03.md).

S3-34do implements a bounded reducer for one validated S3-34dn artifact.
Runtime commits `5ae5e47`/`0e64b07` and harness commit `2c24eec` passed
independent static Sol/high review. Rust reducer identity:
`reduce_saved_s3_34do_value_only_artifact`; self-check identity:
`s3_34do_value_only_reducer_self_checks_are_bounded_and_preserve_signature`;
harness gate: `s3_34do_value_only_reducer_self_check`; fail-closed harness
self-test: `test_s3_34do_value_only_reducer_self_check_gate_is_pinned_and_fail_closed`.
At the S3-34do integration revision, totals were pinned at 305 gates (295
acceptance + 10 lifecycle), 323 steps, and 138 self-test names. The current
manifest after S3-34dr has 324 gates (306 acceptance + 18 lifecycle), 342
steps, 149 self-test names, and 18 source pins. The exact Rust synthetic
self-check passed 1/1 and the pinned Python self-test exited 0; this did not
run the full manifest. No real artifact was reduced.

The reducer requires the original artifact to reproduce twice and stops with
`not_reproduced` without output otherwise. It makes deterministic value-only
candidate attempts, capped at 256, and requires lower mutable-value popcount
plus the exact stable mismatch signature. It preserves profile/address/RSP/
flags/provenance/layout and the checked single width-four callback protocol,
then writes a reduced artifact create-only after acceptance. Critical fence:
`classify_replay` validates only the original artifact against its full saved
observations; candidate validation uses a separate candidate-specific ordinary
replay validator. The reducer self-check uses synthetic data only; it does not
establish JIT parity or real-artifact replay. No real-artifact or runtime
success claim follows. No generic framework, runtime API, dependency, or
opcode is added. S3-34.2 execution acceptance remains pending. ISSUE-045 is
resolved for the separate S3-20eg retry-assertion host gate; ISSUE-046 is
resolved for the S3-34dn artifact-protection self-check only. Stage 3 remains
partial. See the
[S3-34do report](STAGE_3_S3-34do_2026-11-03.md).

## Current-revision acceptance status — `f1670fc`

### Earlier successful evidence

The accepted S3-41a full-local run `stage3-20261101-190932-58293bb642` records
120 passed, 0 failed, and 33 `not_run` at source revision
`020f79d74566ad6abdc8d2fa4ad8d6e0bfaf65ea`. The earlier S3-34br exact JIT/host
gates passed 1/1 and its cumulative Python validation module passed 91/91 at
integrated revision `8052314`. These are successful results for their recorded
source revisions; the S3-34br result predates the ISSUE-045 retry assertions,
and neither run includes the later S3-34dn/do changes.

### Required correctness acceptance

At `f1670fc`, the gate inventory is statically checked and pinned at 305 gates
(295 acceptance + 10 lifecycle), 323 local steps, and 138 self-test names.
These gates and self-tests have not been executed against this revision. Run
the existing exact gates documented in the [JIT testing reference](../testing/JIT.md):

- ISSUE-045 host gate: `s3_34br_host_addr32_rsp_dword_store_zero_extended_esp`;
  its fail-closed harness self-test is
  `test_s3_34br_addr32_rsp_dword_store_gates_are_pinned_and_fail_closed`.
- ISSUE-046/S3-34dn artifact-schema self-check:
  `s3_34dn_replay_artifact_self_checks_roundtrip_and_fail_closed`, pinned by
  harness gate `s3_34dn_replay_artifact_self_check`; the harness's fail-closed
  registry self-test is
  `test_s3_34dn_replay_artifact_self_check_gate_is_pinned_and_fail_closed`.
- S3-34do reducer self-check:
  `s3_34do_value_only_reducer_self_checks_are_bounded_and_preserve_signature`,
  pinned by harness gate `s3_34do_value_only_reducer_self_check`; the harness's
  fail-closed registry self-test is
  `test_s3_34do_value_only_reducer_self_check_gate_is_pinned_and_fail_closed`.

At snapshot revision `f1670fc`, ISSUE-045 and ISSUE-046 were **OPEN** pending
their exact acceptance gates. ISSUE-045 was subsequently resolved by the
separate S3-20eg retry-assertion host gate; ISSUE-046 was resolved by the
synthetic S3-34dn artifact-protection self-check. The historical gate
inventory is unchanged.

### Subsequent S3-34do synthetic self-check evidence — 2026-11-03

The exact reducer self-check
`s3_34do_value_only_reducer_self_checks_are_bounded_and_preserve_signature`
passed 1/1, and pinned Python self-test
`test_s3_34do_value_only_reducer_self_check_gate_is_pinned_and_fail_closed`
exited 0. These gates use synthetic data only; no real S3-34dn artifact was
replayed or reduced, and no JIT/interpreter parity or S3-34.2 runtime
acceptance is established. ISSUE-045 is resolved for the separate S3-20eg
retry-assertion host gate; ISSUE-046 is resolved for the S3-34dn artifact
protection self-check only. The current manifest is 324 gates (306
acceptance + 18 lifecycle), 342 steps, 149 self-test names, and 18 source
pins; this exact run did not execute the full manifest. Mini was unavailable,
M1 remains deferred, and physical-x86, production owner/MMU, performance, and
broader Stage 3 acceptance remain open. Stage 3 remains partial; see the
[S3-34do report](STAGE_3_S3-34do_2026-11-03.md).

### Layout acceptance

The linked-worktree `.git` pointer fix in `f1670fc` passed independent static
Sol High review. On 2026-11-18, the `codex` linked worktree at `3476780` had
45 broken links to local evidence and no Git metadata error. The original
artifacts were restored from the main checkout under the same ignored
`work/` paths. This restored 22 Stage 3 report links to 21 files, plus 23
links to existing Stage 2 evidence used by the repository-wide check.
After restoration, `python3 tools/check_layout.py` exited 0 with
`Layout OK: 54 required directories, 7 crate scaffolds, 14 instruction scopes.`

This verifies [ISSUE-042](../KNOWN_ISSUES.md#issue-042) and the layout check
on this checkout. [ISSUE-053](../KNOWN_ISSUES.md#issue-053) describes how the
ignored artifacts affect reproducibility in a fresh checkout. Rerun the
checker for the final S3-41 revision.

### Full local selector, 2026-11-18

`python3 tests/cpu/stage3_harness.py --suite local` finished with
`local_only` on macOS 26.7.1 ARM64 with Rust 1.99.0. All 344 selected steps
passed; 40 unselected steps retained `not_run`. Source revision:
`6cd0c8ebfaaee96f846734ec17414ef9950dc672`. Both S3-01 documentation drafts
(`BENCHMARKS.md` and the baseline report) were present for the entire run.
The tree fingerprint was identical at start and finish:
`25a252b2a6d1b63c4325c1e15fbc692421b253e4a46d9a191253a9456463ac65`.

- 152 required harness self-tests and 326 exact Rust gates passed.
- The local dispatcher oracle compared 10 manual and 1,000 generated
  inputs twice, producing 2,020 comparison results.
- The memory corpus checked 4,800 unique inputs with two replays per input.
- All 14 VMHost RESULT steps passed, including release build, signed
  executables, fixed budgets and both STOP scenarios.

The saved run is `stage3-20261118-223445-2770f791c4` under
`work/stage3-harness/`. Its input manifest SHA-256 is
`8cbbe0da0af178eadd13b2a2a17a4503121b786e1676f9671eb35c5e40266d4b`.
The summary SHA-256 is
`ceb0a2db4ed270893cff4f2409024dfe05269c0ad5adf7dfc7f70b2363cc975d`.
The full VMHost process selector, physical x86, Mac mini, performance,
owner-backed and Stage 2 suites have status `not_run` in this summary.
Final S3-41 acceptance remains open.

### Local fault boundary regression run, 2026-11-18

`python3 tests/cpu/stage3_harness.py --suite local` finished with
`local_only` on macOS 26.7.1 ARM64. All 348 selected steps passed, and 40
unselected steps retained `not_run`. This includes 154 harness self-tests,
330 exact Rust gates and all 14 local VMHost RESULT steps. The three S3-27
fault-boundary gates and the S3-29e coordinator regression each ran once
and passed. The harness self-test step took 115.067 seconds against its
600-second timeout. A prior full local run took 124.931 seconds in the same
step, beyond the former 120-second limit. Both runs preserved their source
fingerprints. [ISSUE-050](../KNOWN_ISSUES.md#issue-050) is closed.

The saved run is `stage3-20261118-235652-df63ad5f87` under
`work/stage3-harness/`, at source revision
`899e1decc80a7ff7902669ef5e30436bb7c2a293`.
The tree fingerprint at start and finish was
`fc9a37057387ad37eecf6d07d661d9e8a5f3264aa24e7d6b485f6c10ef6485b6`.
The input manifest SHA-256 is
`c6566f8daf0c569ecd517345f41530b77020b1e32e2e28aaa42fc80da42aafde`.
The summary SHA-256 is
`1a07d43f085b3cee67c24de4df716777cd34e0a614efddeb2f151dbc120c9685`.
Physical x86 and Mac mini checks remain deferred. Final S3-41 acceptance
remains open.

### Local tracked source and reducer run, 2026-11-19 UTC

`python3 tests/cpu/stage3_harness.py --suite local` finished with
`local_only`. All 351 selected steps passed and 40 unselected steps retained
`not_run`. The run includes 155 harness self-tests, 333 exact Rust gates
and all 14 local VMHost RESULT steps. Both S3-23 tracked source revocation
gates and the S3-34do controlled mismatch replay and reduction gate passed
under their exact test identities. The preceding `od-host` library run
passed 486 tests and reported one ignored test.

The saved run is `stage3-20261119-002751-6155f78b83` under
`work/stage3-harness/`, at source revision
`c0b1fdd2a6e5ade30bfade318663be20386ee1f7`. The source fingerprint was
`17d2246f9fad84d3c17ef2c41b5aab4d99a52767a4d2f814d232836c8d625b66`
at start and finish. The input manifest SHA-256 is
`64a2cc24096b2111ef65802db791746c8f6bf1ec858e5c86b0a77878fb81cbca`.
The summary SHA-256 is
`ddfdd63108894f040b060077dd2ac872e01b28594f8a4bb1d9bccedb6e6ce223`.
Physical x86 and Mac mini checks remain deferred. S3-41 acceptance remains
open.

### Local prepared-key dispatch run, 2026-11-19 UTC

`python3 tests/cpu/stage3_harness.py --suite local` completed with
`local_only`. All 351 selected steps passed, including 155 harness self-tests,
333 exact Rust gates and 14 local VMHost RESULT steps. The 40 unselected
steps retained `not_run`.

The saved run is `stage3-20261119-020430-2fad681d44` at source revision
`1204eb52305285a55ba233e81a5358c68c262408`. The source fingerprint was
`ee5eccb91bbdb13c421a9b70ccf7ec3be0daff427c41618e939a60f8ec507f6e`
at start and finish. The input manifest SHA-256 is
`0aaca3395cd4fef52d6a46d2dc125fd2af5bb9eeea63e791b447a2f0609f62a0`.
The summary SHA-256 is
`a0905ea2d2cc349a6c034693301324d416f754e6cc27ceed72cbc34bb8573159`.

### Unsupported 0F C2 decode boundary, 2026-11-19 UTC

The Stage 3 MOV comparison exposed a baseline host panic when a REX
prefix reached the general `0F C2` opcode match without satisfying any
CMP profile guard. The decoder now returns its typed `Unsupported`
at that boundary. The exact regression covers seven unadmitted REX
prefixes, including a fault injected at the opcode and one at the
first unconsumed byte. The complete `od-isa` library suite passed
2,607 tests. Physical x86 and Mac mini checks remain deferred.
[ISSUE-522](../KNOWN_ISSUES.md#issue-522)

### Local next-block SMC regression, 2026-11-19 UTC

Run `stage3-20261119-053704-96e70bb3d6` passed all 359 selected local
steps. The new S3-24c gate writes the immediate of the block at the
following RIP through direct and alias addresses. The V3 writer
finishes, both stale successor entries are removed, and fresh
execution matches the interpreter at both addresses. The test covers
one committed store, precise retirement and available arena slots.
[S3-24c and run bindings](../plans/STAGE_3.md#s3-24c-запись-в-отдельный-блок-по-следующему-rip)

### Full local selector after S3-31g, 2026-11-19 UTC

`python3 tests/cpu/stage3_harness.py --suite local` completed as
`local_only`. All 359 selected steps passed, including the 157 harness
self-tests, the Stage 1 local dispatcher child, the memory differential
corpus and the local VMHost RESULT contract. The Stage 1 child checked 10
manual and 1,000 generated fixtures twice, giving 2,020 results. The
memory corpus checked 4,800 inputs twice. SHA-256 verification matched
all 359 selected step logs and both Stage 1 child JSON files.

| Run binding | Value |
| --- | --- |
| ID | `stage3-20261119-065023-2986c23515` |
| Revision | `d99ce3b5f399b07202c3b3208a2d9ef8b83cbb58` |
| Source fingerprint at start and end | `c0d4dc4739e33a8c15e827208523bf5efc1543bd50b835194ce56b279de5b86c` |
| Input manifest SHA-256 | `ab93994816db24f9a5ef771b9d28f5197cc2542b1545c2e97903d0cdefa9a800` |
| Summary SHA-256 | `2ab7728681732a32fc82ab0edc7886f27997f4cf106ac8978fe969fd04087078` |
| Stage 1 child summary SHA-256 | `c32fabbf1793ef9b6cb42810af2c25e54b422de58d82d8b5f94d9bb548d88a6c` |
| Stage 1 child manifest SHA-256 | `977b9a560b6d339cc7a88892c7de3cbca5cd0d8e945c5ed3de9c31d6bc4e9c7f` |

The first attempt `stage3-20261119-064621-9c8ffb2c8d` ended at the
fingerprint guard after all 157 self-tests returned successfully. An
untracked scratch directory created during that attempt entered the
fingerprint. The directory was ignored before this clean rerun.

The 46 unselected steps retained `not_run`. The signed process
selector from S3-31g is a separate run with its own bound source snapshot.
Physical x86 and Mac mini checks remain deferred. Final S3-41 acceptance
remains open.
[Signed VMHost run and S3-31g results](STAGE_3_S3-31g_2026-11-19.md#verification)

### Local and VMHost selectors after the MOV decoder change, 2026-11-19 UTC

Both selectors used the clean `e31c105` tree after S3-40o
and its performance reports. `python3 tests/cpu/stage3_harness.py --suite local`
returned `local_only`. All 359 selected steps passed, including 157
harness self-tests, 2,020 dispatcher child results and 4,800 memory
inputs with two runs per input. The local VMHost RESULT contract passed.

`python3 tests/cpu/stage3_harness.py --suite vmhost-process` returned
`vmhost_process_only`. All 46 selected steps passed, including control races,
independent VMHost processes, invalid-state connection closure and the
generated-code fault probe.

| Binding | Local | VMHost process |
|---|---|---|
| Run ID | `stage3-20261119-093244-0384d8b398` | `stage3-20261119-100818-3d90f786cc` |
| Selected steps passed | 359 | 46 |
| Unselected steps | 46 | 359 |
| Input manifest SHA-256 | `e8e301511a4857937e2c1e9f4e867695fedf1106a8cdd6236a41c36e566af211` | `e8e301511a4857937e2c1e9f4e867695fedf1106a8cdd6236a41c36e566af211` |
| Summary SHA-256 | `2955e673b2e648345b75e07569975b3e9793951f0338d8b3235facd2b7ae1382` | `0e76cae843ba0249edff389cfcf817db7ffa898d3bcc03b66d49ef69ab4b8d2f` |

Both runs recorded revision `e31c105ee88f66ccfc5f83e3aa40fd756c693d90` and the same source
fingerprint `82530645720cc00b959e1b96402884b38294a7f882d7fb67c378661aa202c374` at start and end.
An independent audit compared all 405 selected step logs with their saved
SHA-256 values. The dispatcher child's summary
`ea0b6d2874ee58837dfc1dbe8866fd6866b255b41e4a4b2d9e78324c4e0a35fa` and manifest
`b5d181fbd6b11aa36b7e0dd9deeb2aecefdcb425fe479ac67f4b77498c451eca` also matched.
Raw run directories are under `work/stage3-harness/`.
Physical x86 and Mac mini stayed `not_run` in both selectors.
Final S3-41 acceptance remains open.

#### Historical x87 baseline checks

All five tests named in [ISSUE-040](../KNOWN_ISSUES.md#issue-040)
passed in the 2,608-test `od-isa` library run for `b7c9c92`.
`cargo test --offline -p od-isa --lib` produced the log SHA-256
`2a5ecab29a6b6fed2eea2fc68eb2459196b37e65e732b2d089ea63ce804aa19a`.
Comparing the tests with the `9a1376b` baseline shows earlier updates
to the FABS/FCHS status masks, FST/FSTP special-value negatives and
the FCOMI/FNINIT neighboring encodings as admitted FCMOV forms and
decode errors changed. This resolves the old library-suite blocker
for the five recorded failures.

### Signed VMHost selector after interpreter specialization, 2026-11-19 UTC

`python3 tests/cpu/stage3_harness.py --suite vmhost-process` returned
`vmhost_process_only`. All 46 selected steps passed on the source tree
containing S3-40t, including the generated-code fault probe, control
races, process isolation and invalid-state connection closure.
The 359 unselected local steps retained `not_run`. Physical x86 and Mac
mini suites also retained `not_run`. A separate audit verified all 46
step logs and the input manifest.

| Binding | Value |
|---|---|
| Run ID | `stage3-20261119-110128-bed10f404f` |
| Revision | `9a6e4e9f12b9299c20e8f338654e414f837bbf7b` |
| Source fingerprint at start and end | `33b21ff8347ace127feee75433f21bef390ad5151f653992cd81c76ed15e827c` |
| Input manifest SHA-256 | `6ff5202189414872a28acef549025cecd577c4d073d1fe279d50243d0d6cf28d` |
| Summary SHA-256 | `6ee113625e4b80b2d66e69c1c031ae4eca6ebf5bebddea9953f9fdd1cfcf4460` |

The full 359-step local run above pertains to the post-MOV source
snapshot. S3-40t has its own [ISA and host checks](STAGE_3_S3-40t_2026-11-19.md#проверки-рабочего-дерева).
Final S3-41 acceptance remains open.

### Full VMHost process selector, 2026-11-18

The initial process run at `30306ed` reproduced
[ISSUE-058](../KNOWN_ISSUES.md#issue-058). It stopped at
`process_control_race` after 23 passing steps. The negative HELLO fixture
used version 1, which VMHost supports during the initial handshake. The
fixture now sends version 65,535. The signed baseline binary passed the
control smoke with the corrected fixture. The Stage 3 process manifest
records the SHA-256 of that input.

`python3 tests/cpu/stage3_harness.py --suite vmhost-process` then passed
40 of 40 selected steps in `stage3-20261118-225053-e0017f44c9`. The
`vmhost_process_only` result includes signed V0/V1 control, independent
processes, app-bundle verification and generated-fault smoke. The other
344 steps retained `not_run`. [ISSUE-044](../KNOWN_ISSUES.md#issue-044)
remains open after this successful fault-smoke observation.

The run used `30306ed` with the harness/fixture fix and the two S3-01
documentation drafts. The worktree fingerprint was unchanged throughout
(`9de3d442a51c3ab03b089193ba94036f081c703aa550bbd49a4681a7976104d6`).
The input manifest digest is
`0c074efab92952cd2b68786ee1e6f862715488cfc3410b97f26f9bd535ea5008`.
The summary digest is
`51971a6c12b831191dd9970341687116a34560c4e2875d0267dd25262dcc2c36`.
Both files are under the run ID in `work/stage3-harness/`.

### Optional real-artifact replay or reduction

Replay (`OD_S3_34DN_REPLAY_ARTIFACT`) and reduction
(`OD_S3_34DO_REDUCE_ARTIFACT`) are optional diagnostics for an actual saved
mismatch artifact. Their real-artifact runs were not performed. An absent
artifact or a `not_reproduced` outcome caused by no supplied artifact is not a
successful replay or reduction and does not satisfy the required correctness
gates. The profile-specific behavior and existing commands remain documented
in the [S3-34dn](../testing/JIT.md#s3-34dn-create-only-mismatch-artifact-and-validated-replay-for-s3-34dm)
and [S3-34do](../testing/JIT.md#s3-34do-bounded-value-only-reducer-for-one-s3-34dn-artifact)
JIT sections.

### Local and signed VMHost selectors after S3-40v

On 19 November 2026, both selectors completed on
`d9ecaa241c02a2f9b14134224bbc23bb1bc7e340`. The `local` selector
passed all 359 selected steps, including 157 harness self-tests, 2,020
dispatcher results from 1,010 inputs, 4,800 memory inputs replayed twice,
and the signed VMHost RESULT-state contract. The separate
`vmhost-process` selector passed all 46 selected steps, including the
control races, process isolation and the expected launcher report of
`SIGSEGV` during the generated-code fault smoke. ISSUE-044 remains open.

Both commands use `python3 tests/cpu/stage3_harness.py --suite` with the
selection named above. The common source fingerprint at the start and
end of both runs is
`59ba9accfcf1d49f621e1be5edb6faf2220f8e73188e2b34d470d897b222bd15`.
The common input manifest SHA-256 is
`a543fbec6dbe8259b9d426c22426f7d3ba131b99b3a06f010a58b31b6e6269e0`.

| Selection | Run ID | Summary SHA-256 |
| --- | --- | --- |
| Local | `stage3-20261119-124109-ee27a7b780` | `530669c5468bafbf74529d7086ede28a51d2485cc76c63e31108d42ae82b415b` |
| VMHost process | `stage3-20261119-125558-5d9aa79440` | `7a8d330345acb6b38c1ecbd05a1b4ae2d04e75a691e6e6a93b603469388e87d9` |

The harness validator passed again for both summaries. SHA-256 and source
fingerprints matched all 405 selected step records. Both archives retain
the exact summaries, manifests and logs, including subprocess logs. The local
archive also retains the Stage 1 child summary and oracle manifest. Their
SHA-256 values are bound by the parent summary. Signed executable and
larger generated artifact identities remain in the recorded evidence.

- [Local archive, 415 files](data/stage3/s3-41-d9ecaa2-20261119/local.json.gz) has SHA-256 `85f4a32eadb3a661516b299a0e57461b675f39e45c755f16cf2b103196a1b272`.
- [VMHost process archive, 174 files](data/stage3/s3-41-d9ecaa2-20261119/vmhost-process.json.gz) has SHA-256 `e8333a942fc0d4d30100fb1f936a603312faa9f4e05405df90cbf1cd458de14d`.

The archives expose each UTF-8 member under `files` and its digest under
`file_sha256`. Both results cover this local macOS ARM64 snapshot.
Final S3-41 acceptance remains open, including the deferred hardware checks.

### Local and signed VMHost selectors after S3-40x

On 19 November 2026, both selectors passed on
`5275d67d6e136c43e9dbb3658586f139342eb74c`. The `local` selector
passed all 359 selected steps, including 157 harness self-tests,
2,020 dispatcher results from 1,010 inputs, 4,800 memory inputs replayed
twice, and the signed VMHost RESULT-state contract. The separate
`vmhost-process` selector passed all 46 selected steps, including
process isolation, control races and the expected launcher report of
`SIGSEGV` in the generated-code fault smoke. ISSUE-044 remains open.

Both runs use `python3 tests/cpu/stage3_harness.py --suite` followed by
the corresponding selector. Their common start and end fingerprint is
`20afdb1c48beefddc60a4501bc7d2215d4797e33eeded2adb0a9503d2c532059`.
Their input manifest SHA-256 is
`38fec7a5104c5dae3ebf18cb107a56d98b666ea221dbccfe5c495ec05360fa61`.

| Selection | Run ID | Summary SHA-256 |
| --- | --- | --- |
| Local | `stage3-20261119-140632-8c6a8a224c` | `e36bf34dcbfd7be07552ca31454eb5a44529eefb53822398f0adc88ec89ea843` |
| VMHost process | `stage3-20261119-140933-e835171c13` | `c5a1f5f61f8e8e60d302c00e8108a8d0045a970de1ec646d50d20dfb65edaeaa` |

The official validator passed again for both summaries. The selected
step logs matched all 405 recorded SHA-256 values and source fingerprints.
The local archive includes the Stage 1 child summary and oracle manifest
with the hashes recorded by its parent.

- [Local archive, 415 files](data/stage3/s3-41-5275d67-20261119/local.json.gz) has SHA-256 `6a3045381717880de57164c144a3f0d3b798881e595f275018eeeaca572e179f`.
- [VMHost process archive, 174 files](data/stage3/s3-41-5275d67-20261119/vmhost-process.json.gz) has SHA-256 `79170817b52221a07be10d6279f92b4686db414ffbd09d7bcd68d5b0bea56b79`.

Both results cover the same local macOS ARM64 snapshot. Final S3-41
acceptance remains open, including the deferred hardware checks.

### Full local and VMHost process checks on M1 Pro after the Swift fix

Both selectors ran from clean `f6b5214` on MacBookPro18,3, Apple M1 Pro,
macOS 14.6.1 (23G93), Xcode 15.4 / Swift 5.10, and rustc and Cargo 1.99.0.
The first attempt on `06f0875` passed 348 local steps before Swift failed
to type-check `NotificationQueue.bufferedBytes`. The process selector
stopped at the same compiler error. `f6b5214` splits the byte count into
typed sums and preserves the queue behavior.
[Swift build fix and exact compiler replay](STAGE_3_S3-38d-local_2026-11-01.md#vmhost-build-on-swift-510-2026-11-19)

| Selector | Run | Passed | Exit |
| --- | --- | ---: | ---: |
| `local` | `stage3-20261119-165435-b3d334ca7f` | 359/359 | 0 |
| `vmhost-process` | `stage3-20261119-170237-5e58d2ae65` | 46/46 | 0 |

The local selector passed 157 harness self-tests, both repeats of the
1,010 Stage 1 inputs, both repeats of the 4,800 memory inputs, and the
signed VMHost RESULT check. The process selector passed the signed helper,
launcher, lifecycle, isolation and generated-code fault checks. The
validators accepted both summaries on the original run directories.
All 405 distinct selected steps matched their raw log digests after
transfer. The Stage 1 child summary and input manifest also matched their
recorded digests.

Both runs retained source fingerprint
`1d2b69641f72256f603e0fb7f9658fd82efb94011b78dd8c65163a8ec8216086`
and input manifest SHA-256
`301d452726359564f312308e7c868ab3d6a58192838c5c9556e89472e104f2d1`.
Run the same selectors in a clean `f6b5214` checkout with the given tools
and macOS deployment target 14.0.

```sh
export MACOSX_DEPLOYMENT_TARGET=14.0
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_BUILD_RUSTFLAGS
unset CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS
python3 tests/cpu/stage3_harness.py --suite local
python3 tests/cpu/stage3_harness.py --suite vmhost-process
```

- The local archive contains 415 files and has SHA-256 `f41e2a67af238c5128d8383e1b847b15931b4ca8d06c9146b29eb312303d5c14`. [Local archive](data/stage3/s3-41-m1pro-f6b5214-20261119/local.json.gz)
- The process archive contains 174 files and has SHA-256 `bcce08310ae84e9168dbcb920cd4e8a683dbc3a11108799f4aa7026cc5152782`. [VMHost process archive](data/stage3/s3-41-m1pro-f6b5214-20261119/vmhost-process.json.gz)

### M1 Pro checks at 8c159c8

Both selectors used clean `8c159c8` on the same M1 Pro and toolchain.
The path and content hash matched the local checkout across all 12,202
tracked files. Both runs reported the same start and end fingerprint
`8e21f03f807dbe20363d2659b3e5a289cf212cf02f027e1f39446538be06c34f`
and the input manifest SHA-256
`e02811d871baf271a973003f119d306f0c221cd00db4daa83a05eb938d5b346b`.

| Selector | Run | Passed | Failed | Exit |
| --- | --- | ---: | ---: | ---: |
| `local` | `stage3-20261119-191048-c8b76d5691` | 359 | 0 | 0 |
| `vmhost-process` | `stage3-20261119-192202-3e0efe50b1` | 45 | 1 | 1 |

The local run passed the live final-summary validator. The process
run stopped at `process_generated_fault`. The launcher reported a
closed channel during a frame. The required diagnostic names SIGSEGV.
Process-group cleanup confirmed an empty group. Twelve diagnostic
repeats with the exact signed launcher and helper plus phase tracing
recorded the same transport error followed by a child wait status of 11.
The original failure and the traced repeats are retained. The next
check isolates the order of channel close and wait status.
[ISSUE-523](../KNOWN_ISSUES.md#issue-523)

The transferred evidence was checked against all 405 selected step logs,
58 VMHost child logs, the input manifests, and the local Stage 1 child
summary and manifest.

- The local archive contains 415 files, SHA-256 `7ceb2f32bce3b0e07cad3b5f5b0e4a15c310232cbcee2f5070212130dbc15129`. [Local archive](data/stage3/s3-41-m1pro-8c159c8-20261119/local.json.gz)
- The rejected process run and traced repeats contain 210 files, SHA-256 `ef288ad76a53a74358d7f5624e6507a020be8834c5f0bf9cad7d91d1f1f1f942`. [VMHost diagnostic archive](data/stage3/s3-41-m1pro-8c159c8-20261119/vmhost-process.json.gz)

### Launcher exit reporting after a truncated frame

A test helper completed HELLO/READY and RUN/RUN_ACK, closed the
control socket, and then terminated by SIGSEGV. The signed pre-fix
launcher reported a truncated frame in this sequence. When a frame
is truncated, the launcher now waits up to 200 ms for the actual
child status. SIGSEGV and ordinary nonzero exit use that status.
A clean exit retains the transport diagnostic. A live child goes
through the existing bounded TERM/KILL cleanup. `ECHILD` ends
child handling without signalling the PID.

`python3 -B tests/system/vmhost_launcher_eof_smoke.py LAUNCHER`
passed five scenarios on the signed ARM64 launcher and on a launcher
linked against a separately compiled `ODShared` module. The signed
generated-code fault helper passed without phase tracing. The
RESULT/STOP and external SIGSEGV smokes also passed.
[ISSUE-523](../KNOWN_ISSUES.md#issue-523)

### M1 Pro checks after the launcher fix

Both selectors used clean `5bc5ec6` on the Apple M1 Pro, macOS
14.6.1 (23G93), Xcode 15.4, Swift 5.10, and Rust and Cargo 1.99.0.
The source fingerprint at the start and end of both runs was
`51751612572dab828ab86c7c87f967d70ec6139e9f07ce92d24f3fc9c6b04d97`.
The common input manifest SHA-256 was
`3957c327264db12bd7452f44172fd5a6dce5d61293b58024246986b0bd224455`.

| Selector | Run | Passed | Exit |
| --- | --- | ---: | ---: |
| `vmhost-process` | `stage3-20261119-195349-0e67093787` | 46/46 | 0 |
| `local` | `stage3-20261119-195544-278191cd8c` | 359/359 | 0 |

`process_generated_fault` reported SIGSEGV and left an empty process
group. The same signed launcher passed the five separate EOF regression
scenarios. Its SHA-256 was
`1650568528a8db04925cdd9fb566d54dabcf8b68d02d91c921292c54424284d4`.
The local selector passed the signed VMHost RESULT check, both
repeats of the 1,010 Stage 1 inputs, and both repeats of the 4,800
memory inputs.

Both final summaries passed the validator on the original run
directories. Offline verification matched all 405 selected step logs,
58 child logs, both input manifests, and the local Stage 1 child
summary and manifest. The process archive also contains stdout and
exit status from the separate launcher regression.

- The local archive contains 415 files, SHA-256 `79aa524d27424073346408addef7ffd155db2f7aff3ee7256847f08e4244137b`. [Local archive](data/stage3/s3-41-m1pro-5bc5ec6-20261119/local.json.gz)
- The process archive contains 176 files, SHA-256 `0b519b6b3c0ab1f66b0b6aeef3763b2f01578ffcb89a2dd1270c82647e9e0738`. [VMHost process archive](data/stage3/s3-41-m1pro-5bc5ec6-20261119/vmhost-process.json.gz)

### Native session ABI V1 notification failure
The signed native ARM64 C client verified public ABI layout and normal
notification delivery. With the default `SIGPIPE` disposition and an injected
reader loss before STOP, the `ef44df6` client exited by signal 13. The fixed
client observed `INVALID_STATE` with unchanged output, destroyed the failed
session and acquired a fresh one. With one available FD, both builds returned
`ALLOCATION_FAILED`, retained the request ID high-water mark and recovered
with a greater ID. Failure injection at pipe `fcntl` setup remains open.
The signed cases reproduced on both local macOS 26.7.1 and the M1 Pro
with macOS 14.6.1.
[Native C smoke and reproduction](STAGE_3_S3-38f_session_proxy_2026-11-03.md#native-c-client-and-a-lost-notification-reader-2026-11-19)
### M1 Pro checks after S3-40ah

Both selectors used clean `de1f446` on the Apple M1 Pro with macOS
14.6.1 (23G93), Xcode 15.4, and Rust and Cargo 1.99.0. Both start
and end fingerprints were
`847ff4b4c613ae59d2717455c1292220f3624493b399c1fad6a06e5ecd596071`.
The input manifest SHA-256 was
`edcf3516f85aca2020b771a6ea334db380b4beb19b51a9f2a9c261cecfe7f332`.

| Selector | Run | Passed | Exit |
| --- | --- | ---: | ---: |
| `vmhost-process` | `stage3-20261119-204828-0ada889e9c` | 46/46 | 0 |
| `local` | `stage3-20261119-205026-e90da94c3d` | 359/359 | 0 |

The process selector recorded SIGSEGV and an empty group for
`process_generated_fault`. The local selector passed the signed VMHost
RESULT check, both repeats of the 1,010 Stage 1 inputs, and both
repeats of the 4,800 memory inputs. The final summaries passed the
validator on the original run directories. The archived SHA-256
checks match all 405 selected step logs, 58 child logs and the
local Stage 1 child summary and manifest.

- The local archive contains 415 files, SHA-256 `4435276ee46ca326c548edf2f87de6ae3a344e2f2154eb0c8ed688c01d7f2e1e`. [Local archive](data/stage3/s3-41-m1pro-de1f446-20261119/local.json.gz)
- The process archive contains 174 files, SHA-256 `c8e56dee3e71b0ae487196d77a3958ed6dbc10f17ef5c754d845b241a1271175`. [VMHost process archive](data/stage3/s3-41-m1pro-de1f446-20261119/vmhost-process.json.gz)
### Hardware acceptance

The M1 Pro passed both ARM64 selectors on `de1f446` and passed the
signed native C ABI cases for lost notification and pipe creation failure.
The Mac mini configuration, native execution on physical x86 and
final S3-41 acceptance remain open.

### Performance measurements

Performance remains a separate acceptance area; no performance result for
`f5d2b70` is claimed. Use the existing cold/warm measurement procedures in
[BENCHMARKS.md](../testing/BENCHMARKS.md); this update introduces no new
benchmark command or manifest.

S3-36g measured the retained S0-09 `Session::run` on matched warm inputs.
At 64 and 4096 iterations with one RIP, the dispatcher took 7.6% and 5.6%
of the previous JIT time by the median absolute comparison. The one-pass
case cost 3.75% more. The two-RIP controls showed 2.22–3.47% median normalized
overhead. The signed process suite passed 46/46 with the timed dispatcher
patch. Broader workload acceptance remains open.
[S3-36g report and raw evidence](STAGE_3_S3-36g_2026-11-19.md)

S3-35e measured session initialization and first RUN with an empty code
cache on the same six inputs. At 64 and 4096 iterations with one RIP, the
current JIT beat the interpreter in all scored pairs. The median times
were 10.6 µs versus 27.5 µs and 163 µs versus 1752 µs respectively. Short
and alternating-RIP inputs still favored the interpreter. The broader cold
matrix and fine-grained compiler phases remain open.
[S3-35e report and raw evidence](STAGE_3_S3-35e_2026-11-19.md)

S3-36h measured the dispatcher with one normal-hit search and same-RIP
continuation selection. Median absolute one-pass time fell 3.73% relative
to `06bb39a`. The normalized medians fell 2.82% and 3.02% for the two-RIP
loops. Those loops still favored the interpreter. The signed process suite
passed 46/46 with the measured patch.
[S3-36h report and raw evidence](STAGE_3_S3-36h_2026-11-19.md)

S3-35f repeated the cold measurement on the dispatcher revision used in
S3-36h. The JIT still beat the interpreter on the 64- and 4096-iteration
one-RIP profiles in every pair. Relative to the previous dispatcher, the
normalized ratios stayed below 1 in each of four rounds for the long
one-RIP profile and the two-RIP controls. The other three ranges crossed
parity. Both two-RIP profiles remained slower than the interpreter.
[S3-35f report and raw evidence](STAGE_3_S3-35f_2026-11-19.md)

S3-35g and S3-36i measured four mixed register profiles containing
actual CLD/STD handoffs. The cold and warm runs used the same signed
release executable and four balanced rounds. All 512 timed sides
matched interpreter-visible state and retirement. The interpreter
was faster in every pair. Warm JIT/interpreter medians on the two
mixed loops were 2.102 and 2.101. Native retirement, helper steps and
cache activity were checked separately.
[S3-35g/S3-36i report and raw evidence](STAGE_3_S3-35g_S3-36i_2026-11-19.md)

S3-36j used the post-touch index in NativeRegisterV1 dispatch and
removed the second cache search on a hit. Both warm straight-line
mixed FIFO profiles improved in each raw and normalized round. The
normalized medians fell by 1.86% and 2.58%. The warm loop ranges
crossed parity, and the interpreter was faster in every pair. Warm and
cold used signed matched builds with full-state parity and identical
cache accounting.
[S3-36j report and raw evidence](STAGE_3_S3-36j_2026-11-19.md)

S3-35h and S3-36k compare cold and resident calls to the same
`MemoryPairCache::run` on both frozen S3-34a sequences. The current
build had a median cold call around 7.89 µs and a median resident
call around 0.30 µs. The interpreter took about 0.50 µs. The older
build replay still favoured the interpreter in every warm pair.
The estimated crossover for the current build is 34–44 calls
under unchanged residency.
[Matched report and replay](STAGE_3_S3-35h_S3-36k_2026-11-19.md)

S3-40m measured two ordinary interpreter steps and two decodes
on the same S3-34a inputs used for the whole-call slowdown. The
current/historical ratio stayed above four for each control.
A current-build probe bound to empty execution profiles reduced
the paired run time by about 26%. The decoder and remaining
execution cost still require localization.
[Interpreter controls and ISSUE-521 evidence](STAGE_3_S3-40m_2026-11-19.md)

S3-40n compared the ordinary decoder with two MOV-only clones on
the same checked-memory inputs. Decoder medians went from 185–276 ns
to 95–136 ns with the original MOV arm and to 36–47 ns when the clone
also omitted general predispatch checks. The combined execution
probe took 212–316 ns with full state and memory-event parity.
The result is scoped to the two fixed instruction streams.
[MOV decoder controls and raw evidence](STAGE_3_S3-40n_2026-11-19.md)

S3-40o added a short route for primary `89` and `8B` with exactly one
REX.W. Four decoder builds agreed on 943,170 results and ordered fetch
traces. The selected inlined operand parser gave a 0.211 median paired
time ratio on the target group and 1.009–1.016 on the three controls.
The separate-process full-call results were inconclusive.
[Decoder change, measurements and reproduction](STAGE_3_S3-40o_2026-11-19.md)

S3-40p compared complete interpreter calls with the two decoder paths
in the same signed binary on the 2,400 S3-34a inputs. Candidate/reference
median paired ratios were 0.663 and 0.666 across 32 pairs on each
sequence. The harness checked complete state, RAM, ordered accesses,
retirement, budgets and STOP behavior. The remaining historical
gap and current JIT/interpreter ratios require separate measurements.
[Full-call measurements and reproduction](STAGE_3_S3-40p_2026-11-19.md)

S3-40q repeated the S3-34a cold/warm JIT and interpreter comparison
on `b7c9c92` and `d24865d` with common sweep counts. On the current build,
warm native/interpreter ratios were 0.901 and 0.908. Cold ratios were
21.738 and 21.772. The interpreter still takes about 2.8 times the
historical time on these streams. The throughput benefit is bounded
to repeated hits on the two checked-memory streams.
[Matched cold, warm and historical measurements](STAGE_3_S3-40q_2026-11-19.md)

S3-40r paired ordinary interpreter calls with a diagnostic clone
specialized for empty execution profiles on the S3-34a corpus.
Candidate/reference medians were 0.629 and 0.628. All 64 pairs showed
a lower time, with state, RAM, ordered effects, STOP and budget checks.
The measured gain includes the specialized code layout and call route.
[Full-call diagnostic and source bindings](STAGE_3_S3-40r_2026-11-19.md)

S3-40s compared two shared-source interpreter prototypes with the
original call route in each signed binary. Derived `Default` gave
paired ratios of 1.063 and 1.060, and its call remained in the
specialized step. An associated constant gave 0.700 and 0.699.
Both choices retained state, RAM, STOP and budget parity on S3-34a.
[Prototype evidence and reproduction](STAGE_3_S3-40s_2026-11-19.md)

S3-40t adopted the statically empty profile route for ordinary
`execute::run`. The standalone builds agreed across four groups of
2,400 inputs. Ordinary-run groups improved in all matched process
rounds. The configured XORPD range straddled parity. The benchmark
`__text` grew by 150,528 bytes and signed VMHost `__text` shrank
by 45,760 bytes. Current JIT and historical comparisons remain to be measured.
[Control measurements and binary sizes](STAGE_3_S3-40t_2026-11-19.md)

S3-40u repeated the same checked-memory comparison after S3-40t.
On `e74f721`, warm native/interpreter medians were 1.125 and 1.128,
and cold medians were 28.160 and 28.019. The interpreter still took
2.226 and 2.250 times the historical duration on the frozen corpus.
The one-cold/repeated-warm model found no crossover in 47 of 48
current observations. S3-40q remains specific to its earlier build.
[Post-specialization performance measurements](STAGE_3_S3-40u_2026-11-19.md)

S3-40v reuses the validated capacity-one cache key on an exact match.
Across six matched rounds, median candidate/baseline warm ratios were
0.714 and 0.713. The candidate beat the interpreter in 35/36 store-load
and 36/36 load-store warm pairs, with medians of 0.799 and 0.802.
Cold/interpreter stayed near 27. Evidence covers the two local ARM64
fixtures and one retained key.
[Exact-key measurements and reproduction](STAGE_3_S3-40v_2026-11-19.md)

S3-40w compared `d9ecaa2` with `d24865d` in six matched process
rounds, using ordinary interpreter RUN, two public STEP calls and two
long-mode decodes as separate controls. The current RUN ratios were
2.073 and 2.107, STEP ratios were 2.794 and 2.872, and decode ratios
were 0.740 and 0.730. The two measured profiles retain an interpreter
gap. ISSUE-521 remains open.
[Interpreter measurements and reproduction](STAGE_3_S3-40w_2026-11-19.md)

S3-40x reuses the statically empty-profile executor for ordinary
STEP. The two S3-34a STEP-pair ratios were 0.747 and 0.743, with
a gain in every matched round. RUN and decode ranges crossed
parity. The signed benchmark text section fell by 184,192 bytes.
The differential test covers MOV access, a data fault and REP STOP.
The historical RUN gap remains open.
[STEP measurements and reproduction](STAGE_3_S3-40x_2026-11-19.md)

S3-40y rechecked the two S3-34a sequences on Apple M1 Pro. The warm
`5275d67` JIT took 0.8094 and 0.8205 of its paired interpreter time, and
cold took roughly 31 times interpreter time. Every between-build JIT
range for `5275d67` versus `d9ecaa2` crossed parity.
[M1 Pro checked-memory measurements](STAGE_3_S3-40y_2026-11-19.md)

S3-40z compared `5275d67` and `d24865d` on M1 Pro at RUN budgets 0, 1
and 2. Current/historical median ratios were about 1.285 for budget 0,
2.056–2.075 for budget 1 and 2.140–2.143 for budget 2. The absolute gap
grew from about 5 ns at budget 0 to about 158 ns at budget 2. ISSUE-521
remains open for phase attribution and resolution.
[M1 Pro RUN timing and reproduction](STAGE_3_S3-40z_2026-11-19.md)

S3-40aa retained the gap on M1 Pro when the experimental RUN path
used instructions decoded before timing. The pooled current-minus-
historical gap was 162.2–163.0 ns for public RUN and 176.5–176.8 ns
for predecoded RUN. The measured paths matched architectural and data
access outcomes, and the predecoded path made zero FETCH callbacks.
[Decode boundary controls and raw evidence](STAGE_3_S3-40aa_2026-11-19.md)

S3-40ab compared predecoded RUN, RUN using an out-of-line tracked STEP,
and a direct pair of calls to the same STEP. On both sequences the
current-minus-historical gap remained at 170.3–170.5 ns in the direct
pair. Disassembly verified the shared call destination and the untimed
checks verified ordered memory effects, two retirements and zero FETCH.
[RUN loop controls and raw evidence](STAGE_3_S3-40ab_2026-11-19.md)

S3-40ac compared the original pair and two variants of one STEP path
with a common decoder call. Bypassing the selected entry checks changed
the current pooled medians by less than 1 ns and left a 167.4–168.0 ns
gap with the historical revision. The exact input prefixes and data
accesses matched. The remaining postdecode MOV path is tracked in
ISSUE-521. [STEP entry checks and raw evidence](STAGE_3_S3-40ac_2026-11-19.md)

S3-40ad kept the existing STEP admission and called the same MOV
transfer helper through a narrow branch before the final dispatch.
The direct path took 1.0080 and 1.0147 of the ordinary dispatch time in
the current build and retained a 170.3–170.7 ns gap with the historical
revision. The exact instruction prefixes and ordered memory accesses
matched across all 2,400 inputs.
[Final MOV dispatch and raw evidence](STAGE_3_S3-40ad_2026-11-19.md)

S3-40ae changed six unit-variant predicates identified in the M1 Pro
profile. The signed measured STEP went from 99 to 93 static
`Operation::eq` call sites. Ordinary RUN took 5.62–6.74% less time at
budgets 1 and 2 across the two MOV sequences by paired medians. All
six rounds improved at both nonzero budgets. The zero-budget ranges
included 1.0. The bounded MOV and x87 correctness checks passed.
[RUN comparison and profiles](STAGE_3_S3-40ae_2026-11-19.md)

S3-40af grouped four adjacent x87 checks with the same admission
helper. The signed STEP went from 93 to 90 static `Operation::eq`
call sites. On M1 Pro ordinary RUN took 2.58–3.28% less time at
budgets 1 and 2 on the two MOV sequences. All six rounds improved.
The MOV checks and focused x87 cases passed.
[RUN comparison and evidence](STAGE_3_S3-40af_2026-11-19.md)

S3-40ag directly compared `5bc5ec6` with `d24865d` after both x87
guard changes. The paired ratios at budget 2 were 1.9716 and
1.9770 on the two MOV sequences. The pooled differences were
134.2 and 134.7 ns. The difference at budget 0 was 5.3 ns.
The remaining interpreter gap is tracked in ISSUE-521.
[Historical comparison and evidence](STAGE_3_S3-40ag_2026-11-19.md)

S3-40ah replaced the RDTSC, CPUID, SYSCALL and SYSRET variant
comparisons with `matches!` after attributing four direct equality
calls in the profiled M1 Pro STEP. Those calls disappeared in the
replacement build. The paired median RUN times fell by 5.91–6.70%
for the two MOV sequences at budgets 1 and 2. The budget-zero
ranges included unity. Both builds passed the MOV state and
access checks and all 6,007 `od-isa` tests.
[Profiles, RUN timings and verification](STAGE_3_S3-40ah_2026-11-19.md)

S3-40ai directly compared `de1f446` with `d24865d` on M1 Pro after
the system instruction guard change. The budget-two ratios were
1.8436 and 1.8499 on the two MOV sequences. The pooled gaps were
117.6 and 118.0 ns at budget two and 4.7 ns at budget zero.
ISSUE-521 remains open for the historical interpreter gap.
[Matched comparison and evidence](STAGE_3_S3-40ai_2026-11-19.md)

S3-40aj used fresh S3-40ai samples to locate HLT, INT3 and CLI
comparison calls. The candidate replaces four outer predicates
including the adjacent STI check. The signed STEP has 81 static
`Operation::eq` calls, down from 86. On M1 Pro its paired RUN
medians fell by 3.39–3.86% at budgets 1 and 2. The budget-zero
ranges include unity. Both builds passed focused guard and MOV
correctness checks.
[RUN timings, samples and verification](STAGE_3_S3-40aj_2026-11-19.md)

S3-37g timed between-call backing invalidation and redispatch separately.
Deleting the oldest pair took median 0.221 µs at two resident entries
and 0.364 µs at sixteen. Both first calls on the new backing version
took about 5.25 µs together, including recompilation and publication.
All 33.19 million scored epochs recovered two slots before publication
and restored the resident footprint after the two calls.
[S3-37g report and raw evidence](STAGE_3_S3-37g_2026-11-19.md)

S3-37h measured actual native V3 stores into the current artifact's
pending tail. Across 4.20 million epochs, the complete active-write
interval including the invalidation callback took median 0.454 µs
for direct stores and 0.459 µs through an alias. Both first tail
dispatches took about 5.5 µs together, about four times their paired
warm interval. Each epoch verified one committed store, rejection
of the stale active handle and restoration of resident accounting.
[S3-37h report and raw evidence](STAGE_3_S3-37h_2026-11-19.md)

S3-37i ran the retained S0-09 Session in two independent signed
processes. Each completed 64 pressure and lifecycle checkpoints over
at least 660 seconds. Together they verified 335,331,840
publications and 335,327,744 evictions. Full occupancy held
at 16 entries per process. Checkpoints verified STOP before RUN, typed
entry refusal, exclusive lease enforcement, clear, refill and destroy.
The report keeps process RSS snapshots separate from per-Session code
and logical metadata accounting.
[S3-37i report and raw evidence](STAGE_3_S3-37i_2026-11-19.md)

S3-31g added invocation-local reuse of the previous resident S0-09 entry.
For the two alternating-RIP inputs, normalized warm time was 9.05% and
7.36% of the prior dispatcher. Normalized cold time was 20.36% and 7.52%.
All 32 JIT/interpreter pairs for each two-RIP profile and timing mode
favoured the JIT. The 64- and 4096-iteration warm one-RIP controls were
2.30% and 4.16% slower after normalization. Direct linking and broader
workloads remain open.
[S3-31g report, verification and raw evidence](STAGE_3_S3-31g_2026-11-19.md)

S3-31h measured the current and pre-memo S0-09 routes on `5968160`,
with `nice -5` verified at the start of each measuring process. The
same-process run found a 1.9–2.6% penalty on the long one-RIP profiles.
Moving index rebasing to the transition path recovered about 1% there.
With separate binaries, the prototype had higher normalized
JIT/interpreter ratios on two RIPs. The current implementation is
retained. On this build, JIT won every measured cold and warm pair at
64 and 4,096 iterations for one and two RIPs.
[Current register-loop timings and decision](STAGE_3_S3-31h_2026-11-19.md)

S3-31i ran the same seven warm S0-09 profiles on M1 Pro from `5c92bf7`.
The current build added 2.01% and 2.82% to raw JIT time on the two
long one-RIP loops. Both six-round ranges were above the ranges of
the repeated-current control. With two RIPs, current took 8.23%
and 6.70% of the original JIT time. The dispatcher is retained and
the one-RIP regression remains open in ISSUE-520.
[M1 Pro warm results and evidence](STAGE_3_S3-31i_2026-11-19.md)

S3-31j measured a candidate that skips rewriting the previous RIP and
index on the same-RIP branch. The signed assembly confirmed the
skipped payload store. The two long one-RIP profiles yielded raw
JIT medians of 1.0001 and 1.0002 times stock, with ranges overlapping
the same-file A/A control. The stock dispatcher is retained.
[Results and linked code](STAGE_3_S3-31j_2026-11-19.md)

S3-31k passed the memo address to the resolved-hit path directly.
The M1 Pro signed caller uses four instructions instead of ten.
The warm paired JIT medians fell by 1.99–2.49% on the two long
same-RIP profiles and by 2.62–3.25% on two alternating-RIP profiles.
All four ranges lie below their same-file A/A ranges.
[Warm JIT results, source and disassembly](STAGE_3_S3-31k_2026-11-19.md)

Stage 3 remains partial. Stage 2 and production owner/MMU integration remain
outside this fixture evidence. Do not repeat the ISSUE-044 VMHost process suite
without a new diagnostic basis; its existing status and procedure remain in
[ISSUE-044](../KNOWN_ISSUES.md).

### S3-16n bounded-memory exact NOT RCX fallback

The opt-in bounded memory dispatcher admits exact bare `48 F7 D1` only after
strict dispatch returns `Unsupported`, then calls the existing interpreter on
an immutable three-byte fetch view. A host fixture composes native pair → NOT
RCX interpreter step → native pair, checks full CPU/RAM parity and accounting
at budgets 0..=5, stop-RIP boundaries, and atomic STOP after the native prefix,
and preserves strict/opt-in rejection of neighboring NOT RAX. A separate od-jit regression covers bare-NOP fallback
at `CodeView` end `u64::MAX`. Rust identities are
`memory_dispatch::tests::not_rcx_fallback_is_exact_and_strict_run_stays_strict`,
`memory_dispatch::tests::nop_fallback_handles_code_end_at_u64_max`, and
`tests::bounded_memory_dispatch_not_rcx_fallback_composes_native_prefix_and_suffix`;
harness gates are `s3_16_not_rcx_fallback`,
`s3_16_nop_u64_max_regression`, and `s3_16_not_rcx_host_continuation`. At
verification time the harness manifest contained 324 gates (306 acceptance +
18 lifecycle), 342 local steps, and 149 self-test names. All three exact Rust
gates passed (1/1 each), as did the fail-closed self-check. These were local
ARM64 host checks; Mini and M1 checks remain deferred. Strict `run`, native
admission, unrelated unsupported instructions, and production owner/MMU
integration remain outside this slice. S3-16 remains partial; see the
[focused report](STAGE_3_S3-16-nop-fallback-partial.md),
[plan](../plans/STAGE_3.md#s3-16-подключить-interpreter-fallback), and
[JIT testing reference](../testing/JIT.md#s3-16n-bounded-memory-exact-not-rcx-fallback).
