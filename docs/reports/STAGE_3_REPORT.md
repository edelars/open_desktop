# Stage 3 — локальная bounded acceptance сводка

**Статус:** локальные bounded slices имеют отдельные evidence; Stage 3 не
завершён. Этот документ фиксирует доступный локальный результат и оставшиеся
обязательные gates, но не заменяет финальную приёмку S3-41.

| Статус | Текущее состояние |
|---|---|
| Bounded local evidence | Подтверждены только перечисленные ниже fixture/profile slices: checked-memory cache correctness (S3-18e), cold direct-entry (S3-35c), warm direct-entry (S3-36d), cached full-call (S3-36e), warm dispatcher phase attribution on two selected S3-17 strata (S3-31; no linking conclusion), FIFO16 lifecycle/counters (S3-37d), VMHost/C ABI lifecycle (S3-38), and an M1 Pro ARM64 smoke (S3-39). |
| Broad Stage 3 conditions | Не доказаны: full dispatcher/workload and memory matrix, general cold/warm usefulness, wider resource/lifecycle behavior, or final production policy. These conditions remain open. |
| Physical hardware gate | S3-21 remains deferred until the end-of-Stage-3 Mac mini checks; local Apple Silicon evidence does not replace the physical x86 comparison. |
| Production owner/MMU | Outside the current fixture scope; no production RAM/backing/mapping/MMU owner or committed-writer integration is claimed here. |

Stage 2 process launches or sampled activity are not blockers by themselves and
do not need to be stopped or quiet for this local evidence. Monitoring
limitations, including point-in-time snapshots without per-pair attribution,
remain recorded in each affected report.

## Зафиксированное локальное evidence

S3-36e implementation is committed. Release preflight passed on Apple M3 Max
with 2,400 frozen inputs and 4,800 parity trials. Calibration-only passed at
2,048 repetitions per side with no scored pairs. Three scored runs retained 20
pairs per sequence and passed all floors. Across 60 paired ratios per sequence,
medians were 2.7223 (`store_then_load`) and 2.7650 (`load_then_store`), with no
cross-stratum aggregate or general performance claim. See the [S3-36e
report](STAGE_3_S3-36e_2026-11-01.md) and independently reviewed
[suite evidence manifest](data/stage3/s3-36e/suite-manifest.json).

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

## Карта требований и границ

| Область | Локально подтверждено | Статус и граница |
|---|---|---|
| Block dispatch и bounded execution | S3-34f local acceptance и S3-34h report-inclusive harness; dispatcher corpus запускается отдельно как `local_only` | Это ARM64/local correctness evidence; не physical x86 equivalence и не полный Stage 3 workload. См. [S3-34f](STAGE_3_S3-34f_2026-10-30.md), [S3-34h](STAGE_3_S3-34h_2026-10-30.md). |
| Checked memory lowering, ordered callbacks, and bounded cache | Узкие принятые V2 MOV singleton/pair profiles; три sequences in S3-34h; S3-18e capacity-one cache correctness with fresh invocation state; S3-35c cold direct-entry, S3-36d warm direct-entry, and S3-36e cached full-call evidence on two frozen sequences | Fixture-bounded behavior only. Timed boundaries differ and cannot be subtracted into cache cost or break-even. No general memory compiler, MMIO/atomic path, production owner/MMU, or SMC evidence. См. [JIT contract](../specs/JIT_CONTRACT.md), [S3-18e](STAGE_3_S3-18e_2026-11-01.md), [S3-20cl](STAGE_3_S3-20cl_2026-10-28.md), [S3-20cm](STAGE_3_S3-20cm_2026-10-29.md), [S3-35c](STAGE_3_S3-35c_2026-11-01.md), [S3-36d](STAGE_3_S3-36d_2026-11-01.md), [S3-36e](STAGE_3_S3-36e_2026-11-01.md), and [S3-40e](STAGE_3_S3-40e_2026-11-01.md). |
| Code invalidation, lifetime и state attribution | Fixture-bounded S3-23/24/25; S3-27c V3 и S3-27d direct singleton V2 PC maps в acceptance harness | Не общий active-code retirement/SMP reclamation, произвольное host-fault recovery или production committed-writer ownership. См. [S3-27d](STAGE_3_S3-27d_2026-10-30.md) и разделы S3-27 в [JIT contract](../specs/JIT_CONTRACT.md). |
| Events, STOP, flags и coordinator | Точные локальные S3-28 event/STOP, demand-flags и S3-29e coordinator tests прошли в harness; S3-29f добавил три timed release runs для фиксированных семи single-block cases | S3-29f не показал преимущества candidate на этой выборке; общий performance вывод и production default не установлены. См. [S3-34f](STAGE_3_S3-34f_2026-10-30.md), [S3-29f](STAGE_3_S3-29f_2026-10-30.md) и [S3-40c](STAGE_3_S3-40c_2026-10-30.md). |
| S3-30a allocator cost diagnostic | Три release runs на одном frozen six-instruction register-only multi-helper block сравнили register-homes budget4 с spill-only budget0: 60 compile и 240 execution pairs, все прошли 200 ms floor. Budget4/budget0 native medians ranged 43.227–43.520 / 44.376–45.271 ns/call по двум ZF states; code size был 3,052 / 3,040 bytes. | Ограниченная выборка без distinct branch routing (JZ+0 targets converge), pending-flags/max-pressure/memory/full-workload evidence или allocator winner. Stage2 load видна только в point-in-time snapshots; нет per-pair timestamps. S3-30 остаётся открытой. См. [S3-30a report](STAGE_3_S3-30a_2026-11-01.md) и [raw evidence](data/stage3/s3-30a/). |
| S3-30p3 allocator helper-clobber correctness | Test-only native probe теперь отдельно проверяет все шесть W64 значений (четыре register homes, две spill/reload values) после каждого из двух wrapper calls; focused test прошёл 1/1. | Закрывает только sum-only observation gap этого bounded helper probe; не доказывает pending flags, прочие pressure/path combinations или allocator policy/performance. S3-30 остаётся открытой. См. [S3-30p3 report](STAGE_3_S3-30p3_2026-11-01.md) и [test output](data/stage3/s3-30p3/focused-test.log). |
| S3-30p4 pending-flags allocator-pressure correctness | JZ/JNZ candidate/eager/interpreter comparison прошёл на four branch outcomes, `register_homes` 0..4 и budgets 0..4; captured helper operands/calls, allocation placements, entry STOP and STOP/budget continuation checked. Focused test 1/1. | Один bounded W64 fixture, не полное allocator/path coverage и без performance/default decision. S3-30 остаётся открытой. См. [S3-30p4 report](STAGE_3_S3-30p4_2026-11-01.md), [focused test](data/stage3/s3-30p4/focused-test.log) и [format/diff checks](data/stage3/s3-30p4/format-diff-check.log). |
| S3-29f eager/demand-flags single-block comparison | Три release runs завершили fixed seven-case profile; candidate/eager cross-run paired-ratio medians составили 1.003–1.086 по cases, код candidate был на 40–47% больше. Eager остаётся reference/default; общий performance вывод и policy choice не сделаны. См. [S3-29f](STAGE_3_S3-29f_2026-10-30.md). |
| VMHost/C ABI | S3-38a register-only V0 ABI integration, S3-38b process-suite evidence, S3-38c sequential cross-thread handle lifecycle | Bounded local slices; persistent cache/full session are not implemented. ISSUE-027 remains open; process suite was not retried without new basis. See [S3-38c](STAGE_3_S3-38c_2026-10-30.md), [S3-38b](STAGE_3_S3-38b_2026-10-30.md), [issue registry](../KNOWN_ISSUES.md) and [diagnostics](ISSUE_027_DIAGNOSTIC_2026-10-30.md). |
| Harness/replay/reducer | Fixed seeds, repeated corpus, callback-aware comparison, saved-case validation, reducer preservation и negative self-checks | Подтверждает только внесённые профили и локальный ARM64 runner. Полный S3-34 и physical comparison остаются открытыми. См. [S3-34h](STAGE_3_S3-34h_2026-10-30.md). |
| S3-36e cached checked-memory full-call profile | Three release runs on Apple M3 Max, 20 pairs per sequence/run, all sides passed 200 ms. Across 60 paired raw native/interpreter ratios, medians/ranges were 2.7223 (2.6348–2.8856) for `store_then_load` and 2.7650 (2.6390–2.9279) for `load_then_store`. | Ratios above 1 mean native full-call time exceeded interpreter time on these bounded fixtures; a useful target scenario is not yet proven. No cross-stratum or general memory claim. Monitoring has no per-pair attribution; Mini remains deferred. See [S3-36e](STAGE_3_S3-36e_2026-11-01.md) and [evidence manifest](data/stage3/s3-36e/suite-manifest.json). |

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
| Полные cold/warm measurements, resource/lifecycle gates и решения о default (S3-35…37, S3-40) | Не выполнены. S3-35b дал bounded cold register-only pipeline attribution, S3-35c — узкий direct-entry checked-memory cold profile; S3-36c и S3-36d — отдельные native-plus-NOP budget-exit и resident warm direct-entry checked-memory срезы; S3-36e добавил cached full-call profile. Их различающиеся timed boundaries нельзя объединить в cache-cost или break-even вывод. На S3-36e fixtures native/interpreter ratios выше 1, поэтому useful target scenario is not yet proven. S3-37d добавил три v5 sustained FIFO16 lifecycle/counter runs. Full S3-36 dispatcher/lookup, general memory/fallback warm workloads, production teardown/independent-instance evidence, SMC stress, broader RSS attribution и выбор default остаются открытыми; bounded результаты не переносятся на другие workloads. |
| Production RAM/backing/mapping/MMU owner и committed-writer integration | Не входит в текущий local fixture scope; не реализован этим отчётом и не засчитывается как локальное доказательство. |
| ISSUE-027 | Открыта: прежний timeout не локализован; три диагностических наблюдения не установили причину или исправление. Не повторять suite без нового основания. |
| M1 Pro (S3-39) | Ограниченный ARM64 smoke завершён на MacBookPro18,3 / macOS 14.6.1 для arithmetic, taken multi-block branch, accepted S3-14 helper, cooperative STOP и same-handle C ABI publication lifecycle. Это не широкая гарантия host support и не заменяет physical x86 oracle. См. [S3-39 report](STAGE_3_S3-39_M1PRO_2026-10-31.md). |
| Итоговая приёмка S3-41 | Открыта. Stage 3 нельзя объявлять завершённым по этой локальной сводке. |

## Источники

- [План Stage 3](../plans/STAGE_3.md) и [дорожная карта](../ROADMAP.md#этап-3-первый-jit-x86--arm64).
- [JIT contract](../specs/JIT_CONTRACT.md) и [JIT testing](../testing/JIT.md).
- [S3-27d direct V2 PC maps](STAGE_3_S3-27d_2026-10-30.md),
  [S3-30a allocator cost diagnostic](STAGE_3_S3-30a_2026-11-01.md),
  [S3-30p3 allocator helper-clobber correctness](STAGE_3_S3-30p3_2026-11-01.md),
  [S3-30p4 pending-flags allocator-pressure correctness](STAGE_3_S3-30p4_2026-11-01.md),
  [S3-35c cold direct-entry checked-memory profile](STAGE_3_S3-35c_2026-11-01.md),
  [S3-36d warm direct-entry checked-memory profile](STAGE_3_S3-36d_2026-11-01.md),
  [S3-34f local gates](STAGE_3_S3-34f_2026-10-30.md),
  [S3-34h memory corpus](STAGE_3_S3-34h_2026-10-30.md),
  [S3-39 M1 Pro smoke](STAGE_3_S3-39_M1PRO_2026-10-31.md),
  [S3-40d composed cached flags comparison](STAGE_3_S3-40d_2026-10-31.md),
  [S3-38c sequential-thread lifecycle](STAGE_3_S3-38c_2026-10-30.md),
  [S3-38b VMHost process suite](STAGE_3_S3-38b_2026-10-30.md) и
  [ISSUE-027 diagnostics](ISSUE_027_DIAGNOSTIC_2026-10-30.md).
- [Открытые проблемы](../KNOWN_ISSUES.md).
