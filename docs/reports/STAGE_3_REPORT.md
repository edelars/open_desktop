# Stage 3 — локальная bounded acceptance сводка

**Статус:** локальные bounded slices имеют отдельные evidence; Stage 3 не
завершён. Этот документ фиксирует доступный локальный результат и оставшиеся
обязательные gates, но не заменяет финальную приёмку S3-41.

| Статус | Текущее состояние |
|---|---|
| Bounded local evidence | Подтверждены только перечисленные ниже fixture/profile slices: exact `MOV EAX,[RBX]` checked dword load (S3-20cn) and its exact `MOV EAX,[RAX]` alias (S3-20cp), exact `MOV EAX,[RBX+disp8]` dword load (S3-20cr), exact `MOV EAX,[RBX+disp32]` dword load (S3-20cs), exact `MOV [RBX+disp8],EAX` dword store (S3-20ct), exact `MOV [RBX+disp32],EAX` dword store (S3-20cu), exact `MOV [RBX],EAX` dword store (S3-20co) and its exact `MOV [RAX],EAX` alias (S3-20cq), typed-IR qword PC-map comparator/publication lifetime tests for `48 8B 03` / `48 89 03` (S3-27f), typed-IR dword PC-map contracts for `8B 03` / `89 03` (S3-27g), published disp8/disp32 dword-load map equality/retention/republication test (S3-27h; no host-fault recovery), and S3-27i separate canonical-end/range-overflow host coverage for exact `89 83 id`, exact RIP-relative dword load `8B 05 id` (S3-20cv), exact RIP-relative dword store `89 05 id` (S3-20cw; split backing yields no SMC/alias evidence), exact address-size-32 dword load `67 8B 03` (S3-20cx), exact address-size-32 dword store `67 89 03` (S3-20cy), exact address-size-32 disp8 dword load `67 8B 43 ib` (S3-20cz), exact address-size-32 disp32 dword load `67 8B 83 id` (S3-20da), exact address-size-32 disp8 dword store `67 89 43 ib` (S3-20db), exact address-size-32 disp32 dword store `67 89 83 id` (S3-20dc; author-worktree full `od-jit` passed 315/315 before cherry-pick, not rerun on integrated main), exact address64 SIB dword load `8B 04 0B` (S3-20dd; author-worktree full `od-jit` passed 316/316 before cherry-pick, not rerun on integrated main), address64 SIB dword loads `8B 04 4B` / `8B 04 8B` / `8B 04 CB` (S3-20df; author-worktree full `od-jit` passed 318/318 before cherry-pick, not rerun on integrated main), address64 SIB dword loads `8B 44 0B/4B/8B/CB ib` at scales 1/2/4/8 (S3-20dh; author-worktree full `od-jit` passed 320/320 before cherry-pick, not rerun on integrated main), address64 SIB dword stores `89 44 0B/4B/8B/CB ib` at scales 1/2/4/8 (S3-20di; author-worktree full `od-jit` passed 321/321 before cherry-pick, not rerun on integrated main; `od-host` had 147 passes and three existing manual-corpus cases could not access ignored artifacts in the isolated worktree), address64 SIB dword loads `8B 84 0B/4B/8B/CB id` at scales 1/2/4/8 (S3-20dj; exact focused JIT/host gates and prior S3-20dd/df/dh JIT gates passed 1/1; no full `od-jit` suite claim), address64 SIB dword stores `89 84 0B/4B/8B/CB id` at scales 1/2/4/8 (S3-20dk; exact new JIT/host and preserved host/JIT regression gates passed 1/1 in author worktree before cherry-pick; no full `od-jit` suite claim), and address-size-32 SIB dword loads `67 8B 04 0B/4B/8B/CB` at scales 1/2/4/8 (S3-20dl; author-branch `od-jit` 324/324, `od-host` 150/153 because three ignored manual-corpus artifacts were unavailable), plus address-size-32 SIB dword stores `67 89 04 0B/4B/8B/CB` at scales 1/2/4/8 (S3-20dm; author-branch `od-jit` 325/325, full `od-host` not run), and address-size-32 SIB disp8 dword loads `67 8B 44 0B/4B/8B/CB ib` (S3-20dn; author-worktree `od-jit` 326/326 before cherry-pick, full `od-host` not run), plus address-size-32 SIB disp8 dword stores `67 89 44 0B/4B/8B/CB ib` (S3-20do; author-worktree `od-jit` 327/327, `od-host` 153/156 due three ignored manual corpus/cache artifacts), address-size-32 SIB disp32 dword loads `67 8B 84 0B/4B/8B/CB id` (S3-20dp; author-worktree `od-jit` 328/328 and `od-host` 154/157 with three ignored manual artifacts unavailable; not rerun on integrated main), address64 SIB dword stores `89 04 4B` / `89 04 8B` / `89 04 CB` (S3-20dg; author-worktree full `od-jit` passed 319/319 before cherry-pick, not rerun on integrated main), exact address64 SIB dword store `89 04 0B` (S3-20de; author-worktree full `od-jit` passed 317/317 before cherry-pick, not rerun on integrated main), and published disp8/disp32 dword-store map parity/lifetime (S3-27j); four exact pinned harness gates for the earlier memory tests (S3-34u), two exact pinned harness gates for dword-store admission/host behavior (S3-34v), three exact pinned harness gates for dword PC-map/publication cases (S3-34w), three exact pinned gates for S3-20cp aliased dword-load admission/PC-map/host behavior (S3-34x), three exact pinned gates for S3-20cq aliased dword-store admission/PC-map/host behavior (S3-34y), two exact pinned gates for S3-20cr disp8 dword-load site binding and host behavior (S3-34z; integrated harness and CPU discovery passed), and three exact pinned gates for S3-20cs disp32 source binding, PC map, and host behavior (S3-34aa; integrated harness and CPU discovery passed), and three exact pinned gates for S3-20ct disp8-store source binding, PC map, and host behavior (S3-34ab; integrated harness and CPU discovery passed), the exact S3-27h published dword disp8/disp32 load-map integration gate (S3-34ac; integrated harness and CPU discovery passed), three exact S3-20cu disp32-store gates (S3-34ad), and one exact S3-27i canonical-end/range-overflow gate (S3-34ae), and the exact S3-27j dword-store PC-map publication gate (S3-34af), plus three exact S3-20cv RIP-relative load gates (S3-34ag), and three S3-20cw RIP-relative store gates (S3-34ah), plus three S3-20cx addr32-load gates (S3-34ai), and two S3-20cy addr32-store gates without a PC-map claim (S3-34aj), two S3-20cz addr32 disp8-load gates without a PC-map claim (S3-34ak), two S3-20da source-site/host gates without a PC-map claim (S3-34al), and two S3-20db dword-store gates without a PC-map claim (S3-34am), and two S3-20dc dword-store source/site and host gates without a PC-map claim (S3-34an), plus the two S3-20dj disp32-load source/site and host identities pinned without a PC-map gate by S3-34au; checked-memory cache correctness (S3-18e), cold direct-entry (S3-35c), warm direct-entry (S3-36d), cached full-call (S3-36e), cached dispatcher register recurrence (S3-36f), cached-dispatch phase attribution on those six synthetic strata (S3-31b), run-local prepared-key reuse comparison on six synthetic strata (S3-31c), profile-specific offline audit of S3-31c reports (S3-40j), warm dispatcher phase attribution on two selected S3-17 strata (S3-31a; no linking conclusion), FIFO16 lifecycle/counters (S3-37d), separate VMHost/C ABI lifecycle and RESULT-state slices (S3-38a/c/d), current-source VMHost process-only regression (S3-38b), current full-local harness summary (S3-41a; independent evidence review passed), and an M1 Pro ARM64 smoke (S3-39). |
| Broad Stage 3 conditions | Не доказаны: full dispatcher/workload and memory matrix, general cold/warm usefulness, wider resource/lifecycle behavior, or final production policy. These conditions remain open. |
| Physical hardware gate | S3-21 remains deferred until the end-of-Stage-3 Mac mini checks; local Apple Silicon evidence does not replace the physical x86 comparison. |
| Production owner/MMU | Outside the current fixture scope; no production RAM/backing/mapping/MMU owner or committed-writer integration is claimed here. |

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
no state/memory commit; see [ISSUE-028](../KNOWN_ISSUES.md). On author runtime
commit `abd2af4`, `od-jit` passed 345/345; after integration at root `8052314`
with harness `ac069ca`, exact JIT/host gates passed 1/1 and the cumulative
Python Stage 3 harness passed 91/91 (including 34br). Full `od-host` was not
run. See the [S3-20eg report](STAGE_3_S3-20eg_2026-11-02.md).

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
not run. ISSUE-028 remains open for the separate S3-20eg test-coverage gap;
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
ISSUE-028 remains open for the separate S3-20eg coverage limitation. See the
[S3-20ej runtime report](STAGE_3_S3-20ej_2026-11-02.md).

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
This bounded integration evidence neither resolves ISSUE-027 nor replaces the
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
| VMHost/C ABI | S3-38a register-only V0 ABI integration, S3-38b original and current-source process-suite evidence, S3-38c sequential cross-thread handle lifecycle, and S3-38d full RESULT-state transport | Bounded local slices; persistent cache/full session are not implemented. ISSUE-027 remains open; the current process regression does not establish its resolution. See [S3-38c](STAGE_3_S3-38c_2026-10-30.md), [S3-38b current-source addendum](STAGE_3_S3-38b-current-source_2026-11-01.md), [S3-38d](STAGE_3_S3-38d-local_2026-11-01.md), [issue registry](../KNOWN_ISSUES.md) and [diagnostics](ISSUE_027_DIAGNOSTIC_2026-10-30.md). |
| Harness/replay/reducer | S3-34u pins four exact dword-load and typed-IR qword PC-map test gates; S3-34v pins two exact dword-store test gates; S3-34w pins three exact dword PC-map/publication gates; S3-34x pins three exact S3-20cp alias-load gates; S3-34y pins three exact S3-20cq alias-store gates; S3-34z pins two exact S3-20cr disp8-load site-binding and host gates; S3-34aa pins three exact S3-20cs disp32 load source-binding, PC-map, and host gates; S3-34ab pins three exact S3-20ct disp8-store source-binding, PC-map, and host gates; S3-34ac pins the S3-27h published-map integration test; S3-34ad pins three exact S3-20cu source-binding, PC-map, and host gates; S3-34ae pins the S3-27i canonical-end/range-overflow host gate; S3-34af pins the S3-27j published dword-store map integration; S3-34ag pins two S3-20cv RIP-relative dword-load JIT tests and its host test; S3-34ah pins three S3-20cw store identities; S3-34ai pins two S3-20cx JIT identities and its host test; S3-34aj pins two S3-20cy store identities and makes no PC-map claim; S3-34ak pins the S3-20cz source/site and host wrapped-EBX identities, also without a PC-map claim; S3-34al (harness commit `2751ba2`) pins the S3-20da source/site and host wrapped-EBX identities, without a PC-map claim; S3-34am (harness commit `c194891`) pins only the two S3-20db JIT-source and host identities, with no PC-map gate. Exact Rust gates passed 1/1 each, focused harness self-test passed 1/1, and the full Stage 3 harness passed 60/60; CPU discovery was not run for S3-34am. S3-34an (harness commit `11bf23b`) pins only the two S3-20dc disp32-store JIT source/site and host identities, also without a PC-map gate; both exact Rust gates and the focused harness self-test passed 1/1. The full Stage 3 harness/module and CPU discovery were not run for S3-34an. S3-34ao (harness commit `d3d2c05`) pins exactly the S3-20dd JIT admission and host full-width base/index identities, with no PC-map gate; both exact Cargo gates and focused harness self-test passed 1/1. Python byte-compilation, `git diff --check`, and Graphify query/update passed. S3-34aq (harness commit `b63f4df`) pins exactly the two S3-20df scaled-load JIT and host identities; both exact Cargo gates and focused harness self-test passed 1/1. No PC-map gate is pinned, and the full Stage 3 harness/module and CPU discovery were not run. S3-34ao remains the separate scale-1 S3-20dd gate. The full Stage 3 harness/module and CPU discovery were not run for S3-34ao. S3-34ap (harness commit `f767bd9`) pins exactly the S3-20de JIT store-admission and host full-width base/index identities; both exact Cargo gates and the focused harness self-test passed 1/1. The slice has no PC-map gate, and the full Stage 3 harness/module and CPU discovery were not run. Python byte-compilation, `git diff --check`, and Graphify query/update passed. S3-34ar (harness commit `5ab8dca`) pins exactly the two S3-20dg scaled-store JIT and host identities; both exact Cargo gates and focused harness self-test passed 1/1. No PC-map gate is pinned, and the full Stage 3 harness and CPU discovery were not run. Python byte-compilation, `git diff --check`, and Graphify query/update passed. S3-34ap remains the separate S3-20de scale-1 store gate; S3-34aq remains the S3-20df load gate. S3-34as (harness commit `ce18174`) pins exactly the S3-20dh disp8 dword-load JIT and host identities; both exact Cargo gates and focused harness self-test passed 1/1. No PC-map gate is pinned. Python byte-compilation, `git diff --check`, and Graphify query/update passed; the full harness/module and CPU discovery were not run. Prior scale-only harness gates remain separate. S3-34au (harness commit `4ef90b0`) pins exactly the S3-20dj disp32-load JIT source/site and host identities; both Cargo gates and the focused harness self-test passed 1/1. It adds no PC-map gate. Python byte-compilation, `git diff --check`, and Graphify query/update passed. The full Stage 3 harness/module and CPU discovery were not run; Mini, M1 Pro, and Stage 2 were not run. S3-34av (harness commit `27dba0e`) pins exactly the S3-20dk disp32-store JIT source/site and host identities; both exact Cargo gates and the focused harness self-test passed 1/1. It adds no PC-map gate. Python byte-compilation, `git diff --check`, and Graphify query/update passed. The full Stage 3 harness/module and CPU discovery were not run; earlier scale-only and disp8 SIB gates remain separate, and Mini, M1 Pro, and Stage 2 were not run. S3-34aw (harness commit `62a1332`) pins exactly the two S3-20dl addr32 SIB-load source/site and host identities; both exact Cargo gates and the focused harness self-test passed 1/1. It adds no PC-map gate. Python byte-compilation, `git diff --check`, and Graphify update passed. The full Stage 3 harness/module and CPU discovery were not run; prior S3-34ai/ak/al gates cover distinct addr32 load profiles, and Mini, M1 Pro, and Stage 2 were not run. S3-34ax (harness commit `0d33332`) pins exactly the S3-20dm JIT and host addr32 SIB-store identities; both exact Cargo gates passed 1/1 and the focused self-test passed. The Python validator module passed 71/71 with 170 gates, 160 acceptance cases, 10 lifecycle cases, and 188 steps. It adds no PC-map gate; the full Stage 3 harness, CPU discovery, and full hardware acceptance were not run. Python byte-compilation, `git diff --check`, and Graphify update passed; Mini, M1 Pro, and Stage 2 were not run. S3-34ay (integrated harness commit `ed413cd`) pins exactly the S3-20dn addr32 SIB disp8 JIT source/site and host identities; both Cargo gates and focused self-test passed 1/1. It adds no PC-map gate. Python byte-compilation, `git diff --check`, and Graphify update passed. Full Python module counts and CPU-discovery results were not reported; no full-module or hardware acceptance claim is made. S3-34az (integrated harness commit `c7d4e9b`) pins exactly the S3-20do addr32 SIB disp8 dword-store JIT source/site and host identities; both Cargo gates and the focused self-test passed 1/1. It adds no PC-map gate. Python byte-compilation, `git diff --check`, and Graphify update passed. Full Python module counts, CPU discovery, and hardware acceptance were not reported. The separate S3-20dc runtime commit is `28d837d`; its full `od-jit` 315/315 result belongs to author branch `feat/s3-addr32-dword-store-disp32` before cherry-pick and was not rerun on integrated main.  S3-34bm pins the exact S3-20eb RSP disp8 JIT source/site and host checked-state gates; both Rust gates passed 1/1, focused harness self-test 1/1, and the full Python harness module 86/86. The 200-gate manifest has 190 acceptance and 10 lifecycle gates with 218 local steps; Python byte-compilation, Graphify update, diff-check, and Sol review passed. Mini, M1, full `od-host`, and Stage 2 were not run. See [S3-34bm](STAGE_3_S3-34bm_2026-11-02.md). S3-34bn pins the S3-20ec RSP disp8 dword-store JIT source/site and host checked-state gates; both exact Rust gates passed 1/1 and the full Python harness passed 87/87 on integrated revision `2edeb07`. Its fail-closed self-test rejects missing gate/evidence, zero-match, ignored, failed, and wrong-identity output, and a final summary omitting the host gate. This is harness evidence only; full crate results are in the S3-20ec runtime report. See [S3-34bn](STAGE_3_S3-34bn_2026-11-02.md). S3-34bo pins the exact S3-20ed RSP disp32 dword-load JIT source/site and host checked-state gates; both exact Cargo gates passed 1/1 and the full Python harness passed 88/88 on integrated revision `238799d`. Its self-test rejects missing manifest/summary gates, zero-match, ignored, failed, and wrong-identity evidence, and a final summary omitting the host gate. This is harness evidence only; full `od-host` is not claimed. See [S3-34bo](STAGE_3_S3-34bo_2026-11-02.md). S3-34bq (harness code `28cd043`, integrated `1ba352a`) pins the exact S3-20ef address32 ESP load JIT source/site and host gates. At root `a0ce871`, with runtime `392886b` and subsequent 34br harness `ac069ca` integrated, both exact gates passed 1/1 and the cumulative Python harness passed 91/91 (including 34br); the self-test rejects missing gates/evidence, zero-match, ignored, failed, wrong-identity results, and host omission from the final summary. Harness Sol review passed; no full `od-host` claim. See [S3-34bq](STAGE_3_S3-34bq_2026-11-02.md). S3-34br (harness source `08e8ae2`, integrated `ac069ca`) pins the exact S3-20eg JIT source/site and host checked-state gates. At integrated root `8052314` with runtime `abd2af4`, both exact gates passed 1/1 and the cumulative Python harness passed 91/91 (34bq/34br). Its fail-closed self-test rejects missing or zero-match/ignored/failed/wrong-identity evidence and a final summary omitting the host gate. Manifest totals: 210 gates (200 acceptance, 10 lifecycle), 228 local steps. Harness Sol review passed; no full `od-host` or separate PC-map claim. The coverage-only limitation from runtime review is tracked in [ISSUE-028](../KNOWN_ISSUES.md); see the [S3-20eg report](STAGE_3_S3-20eg_2026-11-02.md) and [S3-34br report](STAGE_3_S3-34br_2026-11-02.md). Stage 2 remained outside this slice’s evidence; concurrent runs were ignored per user instruction. Mini/M1 remain deferred; Stage 3 remains partial. S3-34bs (harness source `e2befb4`, integrated `046f5a2`) pins the exact S3-20eh JIT source/site and host checked-state gates; at root `08b3eba` with runtime `62a516a`, both exact gates passed 1/1 and the cumulative Python harness passed 92/92 before later slices. Its fail-closed self-test rejects absent/zero-match/ignored/failed/wrong-identity gate evidence and a final summary omitting the host gate. Manifest totals: 212 gates (202 acceptance, 10 lifecycle), 230 local steps. Harness Sol review passed; no full `od-host` or separate PC-map claim. See [S3-34bs](STAGE_3_S3-34bs_2026-11-02.md). Stage 2 remained outside this slice’s evidence; its concurrent runs were ignored per user instruction. Mini/M1 remain deferred; Stage 3 remains partial. S3-34bu (harness `00430e9`, integrated `4790e76`) pins the exact S3-20ej JIT source/site and host checked-load gates; runtime `d649bd4` is integrated as `4e4fec2`. At root `4e4fec2`, both exact gates passed 1/1 and the full Python Stage 3 harness passed 94/94. The harness author branch separately reported 95/95; that result is not substituted for the verified root count. The fail-closed self-test checks the exact identities in the gate registries and manifest, source fingerprints, Cargo argv, evidence and final summary; it rejects missing/zero-match/ignored/failed/wrong-identity evidence and omission of the host gate. Manifest totals: 216 gates (206 acceptance, 10 lifecycle), 234 local steps. Harness Sol review passed; no full `od-host` or separate PC-map claim. See [S3-34bu](STAGE_3_S3-34bu_2026-11-02.md). Stage 2 remained outside this slice’s evidence; concurrent runs were ignored per user instruction. Mini/M1 remain deferred; Stage 3 remains partial. | Это проверки gate identities/fingerprints и локального gate evidence, не полный corpus/reducer или S3-34; полный S3-34 и physical comparison остаются открытыми. См. [S3-34u](STAGE_3_S3-34u_2026-11-01.md), [S3-34v](STAGE_3_S3-34v_2026-11-01.md), [S3-34w](STAGE_3_S3-34w_2026-11-01.md), [S3-34x](STAGE_3_S3-34x_2026-11-01.md), [S3-34y](STAGE_3_S3-34y_2026-11-01.md), [S3-34z](STAGE_3_S3-34z_2026-11-01.md), [S3-34aa](STAGE_3_S3-34aa_2026-11-01.md), [S3-34ab](STAGE_3_S3-34ab_2026-11-01.md), [S3-34ac](STAGE_3_S3-34ac_2026-11-01.md), [S3-34ad](STAGE_3_S3-34ad_2026-11-01.md), [S3-34ae](STAGE_3_S3-34ae_2026-11-01.md), [S3-34af](STAGE_3_S3-34af_2026-11-01.md), [S3-34ag](STAGE_3_S3-34ag_2026-11-01.md), [S3-34ah](STAGE_3_S3-34ah_2026-11-02.md), [S3-34ai](STAGE_3_S3-34ai_2026-11-02.md), [S3-34aj](STAGE_3_S3-34aj_2026-11-02.md), [S3-34ak](STAGE_3_S3-34ak_2026-11-02.md), [S3-34al](STAGE_3_S3-34al_2026-11-02.md), [S3-34am](STAGE_3_S3-34am_2026-11-02.md), and [S3-34h](STAGE_3_S3-34h_2026-10-30.md). |
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
| Полные cold/warm measurements, resource/lifecycle gates и решения о default (S3-35…37, S3-40) | Не выполнены. S3-35b дал bounded cold register-only pipeline attribution, S3-35c — узкий direct-entry checked-memory cold profile; S3-36c и S3-36d — отдельные native-plus-NOP budget-exit и resident warm direct-entry checked-memory срезы; S3-36e добавил cached checked-memory full-call, а S3-36f — cached register recurrence full-call. S3-31b отдельно атрибутировал три неперекрывающиеся фазы на тех же шести synthetic kernels, но не устанавливает total dispatcher/lookup cost на representative guest workloads; эта широкая gate остаётся открытой. Timed boundaries остаются отдельными; из них нельзя вычислить общую cache cost или break-even. На обеих bounded S3-36e/f нагрузках native/interpreter ratios выше 1, поэтому useful target scenario пока не доказан. S3-37d добавил три v5 sustained FIFO16 lifecycle/counter runs. General memory/fallback warm workloads, production teardown/independent-instance evidence, SMC stress, broader RSS attribution и выбор default остаются открытыми; bounded результаты не переносятся на другие workloads. |
| Production RAM/backing/mapping/MMU owner и committed-writer integration | Не входит в текущий local fixture scope; не реализован этим отчётом и не засчитывается как локальное доказательство. |
| ISSUE-027 | Открыта: прежний timeout не локализован; три диагностических наблюдения не установили причину или исправление. Не повторять suite без нового основания. |
| M1 Pro (S3-39) | Ограниченный ARM64 smoke завершён на MacBookPro18,3 / macOS 14.6.1 для arithmetic, taken multi-block branch, accepted S3-14 helper, cooperative STOP и same-handle C ABI publication lifecycle. Это не широкая гарантия host support и не заменяет physical x86 oracle. См. [S3-39 report](STAGE_3_S3-39_M1PRO_2026-10-31.md). |
| Итоговая приёмка S3-41 | Открыта. Stage 3 нельзя объявлять завершённым по этой локальной сводке. |

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
  [ISSUE-027 diagnostics](ISSUE_027_DIAGNOSTIC_2026-10-30.md).
- [Открытые проблемы](../KNOWN_ISSUES.md).
