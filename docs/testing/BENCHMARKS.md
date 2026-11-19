# Протокол измерений и отчёта

Статус: протокол подготовлен для S0-06; S0-13/E04 baseline выполнен в границах текущего headless прототипа на основном M3 Max. Измерены CPU pilot по четырём fixtures, VMHost до/после completion signal, public C ABI execute, busy-helper STOP и peak-RSS для одного/двух helper processes; данные и ограничения записаны в [отчёте S0-13](../reports/STAGE_0_CPU_BASELINE_2026-10-22.md). Это не benchmark полной VM. Отложены контролируемые повторы, сравнение пакетного и мелкого ABI вызова (пакетного API пока нет), длительная нагрузка и JIT code-mapping size.

Для ограниченного CPU-среза [`od-runner --benchmark`](../../tools/runner/README.md#reproducible-cpu-benchmark) собирает decode, translation/lowering, executable publication, повторный interpreter и повторный JIT с чередованием порядка и 20 samples по умолчанию. Каждая серия калибруется отдельно до batch не короче 200 ms; raw samples включают объём работы и elapsed time. Это не покрывает cold process/VMHost launch, ABI/IPC и resource measurements.

## S3-29f: single-block eager/demand-flags comparison

`od-bench flags-compare` is an opt-in Apple Silicon macOS release measurement
with a fixed seven-case profile of already accepted S0-09 W64 register-only
forms. Cases cover replaced flag writers observed by JNZ, a budget exit between
writers, aliased ADD operands, and both taken and not-taken JZ/JNZ outcomes.
The exact instruction forms are `48 03 C0`, `48 03 C3`, and `48 3B C3` with
short JZ/JNZ. The report binds case bytes and initial states to a deterministic
profile SHA-256 and embeds source fingerprints for the harness, compiler, host
execution and interpreter; callers cannot select arbitrary programs.

Before timing any case, the harness compiles and interpreter-checks all seven
cases, then compares eager and uncached candidate execution with the
interpreter for complete
`LongModeState`, branch continuation, exit accounting, retirement, and
remaining budget. Therefore a late-case preflight failure cannot leave a
partial timed series. Each pair uses the same number of executions and alternates
variant order. The 20 pairs are calibrated to at least 200 ms of summed
`execute_block` call durations per variant. State reset, compilation, code
validation, publication, post-sample comparison, and mapping drop are outside
the timed intervals. Repeated executions within a sample use the same live
published mapping. One `PublishedCode` mapping is published at a time and is
dropped before its paired variant is published; compile time, emitted code
size, and publication duration are reported separately.

Reproduce one process with:

```sh
cargo run --release -p od-bench --offline -- flags-compare \
  --process-run 1 \
  --output work/tmp/stage3/s3-29f/flags-compare-1.json
```

The earlier rejected preflight used ADD/SUB encodings outside this fixed
subset. It produced no timing or output artifact and is recorded in the S3-29f
method report as a rejected attempt, not a benchmark result. Its exact command,
source revision, and profile hash were not preserved; successful run JSON does
not synthesize this historical record.

Timed calls include the `execute_block` safe wrapper, runtime state
materialization/snapshot, and arithmetic-flags helper work. This path does not
include dispatcher lookup, shared-cache admission, interpreter fallback,
checked-memory helpers, guest-memory workloads, physical x86, or Mini. It is
single-block strategy evidence only and does not establish a production
default or full warm-runtime result. Three sequential release processes
completed the seven-case profile. Across the three process-level paired-ratio
medians, candidate/eager ratios ranged from 1.003352 to 1.085941 by case; all
seven were above 1, with aliased ADD near parity. Candidate code was 40–47%
larger. This bounded result does not prove a general slowdown or support a
default choice. Source/profile fingerprints, raw samples, code sizes and
process details are in the [S3-29f report](../reports/STAGE_3_S3-29f_2026-10-30.md).

## S3-29g: cached NativeRegisterV1 dispatcher comparison

`od-bench cached-flags-compare` is a fixed-profile Apple Silicon macOS
release measurement of the complete cached NativeRegisterV1 dispatcher. It
calls the existing eager and opt-in demand-flags-candidate cached entrypoints
with the same `SingleEntryBlockCache` policy and `PublishedBlockExecutor`.
The workload is versioned and embedded in the benchmark: ADD→CMP writer
replacement with JNZ outcomes, SUB→INC partial-writer preservation with JNZ
and a bare-NOP fallthrough, CMP→LEA flag preservation with JZ outcomes, and a
bounded SUB/JNZ back-edge loop. Instructions use the exact admitted
NativeRegisterV1 forms; no caller-supplied code is admitted.

Every case is admitted and compared with the interpreter before any sample.
The comparison checks the full `LongModeState`, exit, total retirement and
remaining budget. A direct compile probe records emitted code sizes and hashes;
the demand-flags candidate must expose its pending-flags PC-map witness. The
timed call uses the actual cached dispatcher, including lookup, cold compile
and publication, branch redispatch, native execution, and bare-NOP
interpreter fallback. State reset, CodeView creation, prewarm, cache creation,
result validation, and mapping drop are outside each timed interval.

Cold samples create a fresh empty single-entry cache for each guest call and
require exactly one miss, compile, and publication. The bounded loop may then
record hits after that first cold miss. Resident samples first execute the
same case to populate the cache; measured calls must record at least one hit
per call and zero miss, compile, publication, or replacement deltas. The
resident cache and its MAP_JIT mapping are dropped before the other strategy
is published. A multi-block/capacity-churning sequence is never described as
resident or warm.

Each case/mode has 20 paired eager/candidate samples with alternating order,
identical input and call counts, and summed dispatch duration of at least
200 ms per variant. Calibration raises a common call count when needed; a
short sample causes the whole paired series to restart at that common count.
Run three sequential release processes on the same host:

First run the explicit release preflight. It checks all admissions, compiler
probes, full interpreter parity, cache miss/compile/publication and resident
execution counters for both strategies, and writes `timing_started: false`.
Only after this artifact is valid should the timed runs start:

```sh
cargo run --release -p od-bench --offline -- cached-flags-compare \
  --preflight-only \
  --output work/tmp/stage3/s3-29g/preflight.json

cargo run --release -p od-bench --offline -- cached-flags-compare \
  --process-run 1 \
  --output work/tmp/stage3/s3-29g/cached-flags-compare-1.json
```

The report preserves raw samples, paired ratios, cache counters, native and
interpreter retirements, block count, resident code/metadata bytes, fixed
profile and source hashes, host/toolchain data, timer boundaries and mapping
policy. Results apply only to this fixed register fixture profile and current
host; they do not select a global default or establish memory-helper, physical
x86, Mini, or full Stage 3 performance behavior.
Three M3 Max release runs completed after preflight. The
[S3-29g report](../reports/STAGE_3_S3-29g_2026-10-31.md) records the bounded
result and links the raw preflight and process artifacts.

## S3-40d: cached composed NativeRegisterV1 eager/demand-flags comparison

`od-bench cached-native-flags-compare` is a separate, opt-in Apple Silicon
release harness over the exact S3-34p generated-corpus inputs. It accepts only
the 1,200 ordered cases with frozen input digest
`fnv1a64:172fb17b01c502df`, and validates each input digest, seed/index order,
the enclosing observation-envelope digest, and the stored interpreter
observation before preparing the profile. The S3-34n generator prefixes two
or three generated native forms (with a RIP-relative LEA in position two),
then appends a JZ/MOV/JNZ control-flow suffix, an optional bare NOP, and an
optional bounded back-edge. Input zero is a separate four-native-form anchor.
This is composed-corpus evidence and does not repeat the S3-29f seven-case
single-block comparison or the smaller S3-29g fixed profile.

Before timing, both cached entrypoints run against a recomputed interpreter
oracle for all cases. The gate checks complete `LongModeState`, exit, total
retirement and remaining budget. Preflight also records exact first-dispatch
and immediate-repeat cache deltas for each input and strategy. A composed call
can compile several blocks, hit a block again, or replace the one cache entry
during control-flow redispatch. Timed samples must match those frozen deltas.
The dispatch uses the same immutable bytes, input state, execution context,
dependency span and capacity-one `SingleEntryBlockCache` policy for eager and
candidate.

Cold samples create an empty one-entry cache per input and time its first
dispatch, including all block compiles and publications. Repeat samples execute
the same input once to prepare the cache, then time its second complete
dispatch; this is a repeat-dispatch measurement, not a hit-only claim. The
published mapping is dropped before moving to the next input or strategy, so
there is no cross-input residency claim. Twenty paired samples alternate
strategy order. Both sides use the same number of complete corpus sweeps, and
calibration/restarts require at least 200 ms of summed entrypoint time on each
side. `CodeView`, admission, cache construction, stop/control setup, state
reset, first dispatch in repeat mode, oracle validation and teardown are
outside timed intervals. The timer wraps the cached dispatcher entrypoint.

Reports include raw sample durations and work totals, exact aggregate
hit/miss/compile/publication/replacement counts and total bytes emitted by
successful publications, separately from peak resident code/metadata and final
per-case resident code sizes. The bench-only `phase-observation` feature adds
one checked cumulative byte addition after each successful publication; it
does no work on cache hits. That publication-path instrumentation cost is
included for both strategies and has not been separately latency-measured.
Raw eager/candidate calibration sweeps and each discarded short sample series
are retained in the JSON report with trigger pair, previous/next sweep counts,
and the shortest sample duration, so the selected sweep count and restart
decisions can be audited.
Reports also include frozen input digest and file SHA-256, source fingerprints,
host and toolchain metadata. All samples include all 1,200 inputs; the exact
per-sweep counter baselines for eager and candidate are reported separately.
Guard-only inputs retain their observed zero-cache behavior, while composed
branch paths can record multiple misses, compiles, publications, replacements
and internal hits.
Eager remains the reference/default regardless of outcome. This profile does
not cover owner/MMU/SMC integration, a retained working set, Mini, physical
x86, Stage 2 integration or the full Stage 3 performance gate. Mini checks
remain deferred to the end of Stage 3.

Run an explicit preflight before any scored process:

```sh
cargo run --release -p od-bench --offline -- cached-native-flags-compare \
  --corpus work/stage3-native-register-v1/manual/native-register-v1-observations.json \
  --preflight-only --output work/tmp/stage3/s3-40d/preflight.json
```

After independent source review, collect three release processes sequentially.
A Stage 2 test launch alone is not a reason to stop a run. Record observed
processes and host-load snapshots. If actual concurrent work materially changes
load or is associated with a timing disturbance, retain the run as diagnostic
evidence and repeat the full process; do not trim individual pairs after
seeing their results.

```sh
for run in 1 2 3; do
  cargo run --release -p od-bench --offline -- cached-native-flags-compare \
    --corpus work/stage3-native-register-v1/manual/native-register-v1-observations.json \
    --process-run "$run" \
    --output "work/tmp/stage3/s3-40d/cached-native-flags-$run-reproduced.json"
done
```

Use a fresh output path for every reproduction attempt so prior reports and
their evidence bindings remain immutable.

Three scored M3 Max release processes completed against the frozen corpus.
Candidate/eager paired-ratio medians were 1.020–1.029 in cold mode and
1.014–1.025 in repeat mode; paired ranges cross parity and their spread is
larger than the central differences. Candidate published 14.27% more code
bytes in cold mode and 13.07% more in repeat mode. No strategy winner or default
change is supported. Run 2 is accepted as noisy bounded evidence after a
high-load attempt was retained separately as diagnostic; run 3 included
low-load Stage 2 activity. Stage 2 snapshots are point-in-time, and numeric
executor exits for accepted runs 2 and 3 were not captured. See the
[S3-40d result report](../reports/STAGE_3_S3-40d_2026-10-31.md) and its
[raw data](../reports/data/stage3/s3-40d/).

## S3-40g: aliased W64 ADD allocator diagnostic A/B

This ledger entry records the S3-32a bounded diagnostic on one frozen
27-byte distinct-JZ fixture containing `ADD RAX,RAX` W64. The measurement release
binary was built from source revision
`958be7edf97dd0e421454630285dc4d6e711b2a7` (source SHA-256
`97d0130cb448f4834840fcbbeadb449067c6b05a0d664c4d99d03e7f1a4898c3`), with
binary SHA-256
`98e7fafa0a548e4eb56d77639949527be0b6c8b0b2fb076feb8e96d7b6165a65`.
Measurements ran on Apple M3 Max, macOS 26.7.1, aarch64, rustc 1.99.0. The
release build source tree was clean; runtime `working_tree_dirty` is `null`
because Git status timed out, so runtime-tree cleanliness is not asserted.

Three sequential release processes retained 20 paired samples per JZ stratum
and process (60 pooled pairs per set). All 360 timed sides met the 200 ms
floor. Candidate/baseline ratios from pooled pair totals were:

| Set | Median (range), n=60 |
|---|---:|
| Compile | 0.9832762133712801 (0.9488835862269013–1.062649364816329) |
| JZ taken | 0.9878736671103938 (0.9749361861693031–1.0766848438450791) |
| JZ fallthrough | 0.9975100869584979 (0.9322534409534861–1.0375123643358157) |

Every range crosses 1. The candidate's lower pooled medians are observations
on this fixture only; they support no speedup, allocator winner, or default
claim. The profile changed code bytes from 3,052 to 3,032, frame size from
288 B to 272 B, and `ReadGpr` / temporary-definition / spill counts from
10/14/10 to 9/13/9; helper capture/save/reload counts stayed 3/3/3. This is
not general workload evidence. The broader S3-32 gate remains open; S3-40 and
S3-41 remain open. Mini checks remain deferred until the end of Stage 3.
The exact fixture, boundaries, raw samples, and manifest are preserved in the
[S3-32a source report](../reports/STAGE_3_S3-32a_2026-11-01.md) and its
[suite manifest](../reports/data/stage3/s3-32a/suite-manifest.json); the
[S3-40g report](../reports/STAGE_3_S3-40g_2026-11-01.md) summarizes this
ledger update without duplicating raw data.

## S3-40j: offline validator for saved S3-31c reports

`tools/bench/validate_s3_31c.py` audits only the S3-31c NativeRegisterV1
prepared-key profile. It checks the frozen profile/workload and sample
schedule, validates the optional preflight schema and build identity, then
recomputes candidate/baseline and candidate/interpreter ratios and ranges from
the raw elapsed samples. It is an offline report audit and does not start a
benchmark or select a production policy.

Run it against the archived process reports and untimed preflight:

```sh
python3 -u tools/bench/validate_s3_31c.py \
  docs/reports/data/stage3/s3-31c-20261101/process-1.json \
  docs/reports/data/stage3/s3-31c-20261101/process-2.json \
  docs/reports/data/stage3/s3-31c-20261101/process-3.json \
  --preflight docs/reports/data/stage3/s3-31c-20261101/preflight-release-final.json \
  > docs/reports/data/stage3/s3-31c-20261101/validator-audit.json
```

The saved audit returned `status: passed`: six strata, 60 pairs each, all
retained sides at or above 200 ms, and source revision
`dcd1bfe5db813a2647c100c4c64a008dca9bd21c` with clean build metadata. The
correctness-only preflight uses the same source/fingerprints but preserves its
own clean detached work directory. The focused Python test suite passed 16
tests and independent code review passed. See the [bounded S3-31c report](../reports/STAGE_3_S3-31c_2026-11-01.md) and [saved audit JSON](../reports/data/stage3/s3-31c-20261101/validator-audit.json). This does not complete full S3-40 or Stage 3.

## Подготовка прогона

Перед измерением закрепить:

- сценарий и критерий корректности результата;
- commit исходников, чистоту/состояние дерева и параметры сборки;
- машину, ОС/build, SDK/compiler/runtime и режим питания;
- точную версию приложения, входные данные и SHA-256 каждого файла;
- seed, настройки сценария, число потоков и ограничения памяти;
- команду запуска, метод измерения и путь к сырым данным.

Сначала проверяется корректность. Неверный результат исключает замер из сравнения. Для сценария, который меняет входные данные, использовать отдельную идентичную копию или восстанавливать fixture перед каждым прогоном.

## Сравнительный протокол

1. Прогреть/откалибровать harness и убедиться, что его стоимость мала относительно измеряемой работы. Как начальный ориентир, операция должна занимать сотни миллисекунд или дольше; более короткую операцию пакетировать без изменения семантики.
2. Сравнивать варианты A/B на одной машине, с одинаковыми входами, настройками и критерием корректности. Чередовать порядок A/B или рандомизировать его, чтобы снизить влияние дрейфа температуры и фоновой нагрузки.
3. Выполнить не менее 20 пар прогонов после калибровки. Сохранять каждое сырое значение; публиковать число выборок, медиану и разброс, а не только среднее.
4. Измерять холодный запуск/холодный кэш отдельно от прогретого режима. Не смешивать их в одной статистике.
5. Записывать температуру/thermal throttling, состояние питания, заметную фоновую нагрузку и существенное изменение потребления памяти. Прервать и повторить серию, если условия существенно изменились.
6. Не делать вывод о p95 по нескольким десяткам общих прогонов. Указывать ограничения выборки; для p95 заранее собирать достаточное число независимых наблюдений.

Скорость признаётся улучшенной только при повторяемой разнице, превышающей наблюдаемый разброс, и при сохранении корректности. При недостаточных данных вывод обозначается предварительной гипотезой. Не принимать архитектурное решение по скорости без подходящих замеров.

## JIT этапа 3: workload и уровни прогрева

Замеры S0-13 относятся к direct fragment path и host wrapper на указанной
машине. Для block JIT используются отдельные серии `od-bench`. При сравнении
вариантов сохранять fingerprints исходников, фикстур и порядка обхода корпуса.

| Workload | Состав | Доступность сравнения |
|---|---|---|
| Linear integer | `MOV`/`ADD`/`SUB`/`CMP` из S0-09 без memory | Interpreter и legacy fragment через `od-runner`; сравнения block JIT ограничены корпусами конкретных серий |
| Conditional branch | `JZ`/`JNZ`, taken и not taken | [S3-36b](#s3-36b-bounded-branchhelper-warm-profile) использует замороженный набор из пяти S3-14 cases |
| Short/long loop | Одни и те же guest instructions с разным числом итераций | [S3-36f](#s3-36f-cached-nativeregisterv1-register-recurrence-full-call-profile) проверяет две register recurrences при 1, 64 и 4 096 итерациях |
| Multi-block | Связанные basic blocks и повторные переходы | [S3-35a](#s3-35a-cold-first-full-corpus-execution) измерял dispatcher на замороженном S3-17 trace; оригинальные list и trace требуются для повтора |
| Helper-heavy | Частые записи флагов с одинаковой x86 semantics | S3-36b покрывает только свои branch/helper cases; общий helper-heavy workload и сопоставимые счётчики для всех вариантов не измерены |
| Memory/fallback/invalidation | Checked accesses, interpreter exits, изменённый guest code | Использовать узкие серии с собственной проверкой корректности. Например, [S3-35c](#s3-35c-cold-direct-entry-checked-memory-comparison) измеряет direct-entry memory, а [S3-36c](#s3-36c-resident-warm-nop-fallback-comparison) — resident NOP fallback |

Для каждого workload сохранить exact guest bytes, initial CPU state, число
исполняемых guest instructions, instruction budget, exit/branch behavior и
ожидаемое состояние. До timing подтвердить корректность каждого сравниваемого
варианта на том же input. Физический x86 oracle — обязательная correctness
проверка там, где её требует план, но его время на отдельной Ubuntu-машине не
является прямым A/B performance comparison с ARM64 host.

Для проверки общего register-only входа можно прогнать одну и ту же
[S0-09 fixture](../../tests/fixtures/x86/s0_09_add_overflow.case.json).

```sh
cargo build --release --offline -p od-runner
for engine in interpreter jit jit-block jit-dispatch; do
  target/release/od-runner --fixture tests/fixtures/x86/s0_09_add_overflow.case.json \
    --engine "$engine" --json
done
```

Сверять `input_sha256`, `status`, `exit_reason`, `instructions_executed` и
`snapshot`; поле `backend` подтверждает путь каждого прогона. Этот preflight
проверяет общий вход и выход для поддержанной формы и оставляет таймеры
самим benchmark-командам.

Различать три измеряемых уровня:

1. **Cold process:** отдельный запуск и startup целевого runner; измерять
   внешним supervisor, поскольку `od-runner --benchmark` не включает старт
   процесса.
2. **Cold compilation/code cache:** `od-bench cold-first-run` измеряет первый
   sweep при пустом in-process cache, в том числе decode, compilation, ARM64
   emission, publication и guest execution. Поле compile/publish/drop у
   `od-runner --benchmark` относится к legacy fragment без постоянного block
   cache и покрывает подготовку и освобождение mapping. Первое исполнение
   требует отдельного замера с соответствующими границами таймера.
3. **Warm execution:** уже опубликованный код и одинаковые повторные inputs.
   Время нового block JIT включает предусмотренные dispatcher/cache lookup и
   state materialization; время interpreter включает decode при каждом вызове,
   если так устроен сравниваемый API.

Runner калибрует серии до batch не короче 200 ms, сохраняет 20 повторных
samples по умолчанию и чередует порядок двух сравниваемых вариантов. Для
нескольких вариантов сохранять сбалансированный переменный порядок, счётчики
guest work и raw duration каждого sample. Раздельно фиксировать median,
разброс, code size и существенные изменения RSS. M3 Max и M1 Pro сравнивать
только внутри одной машины и одной конфигурации. Если вариант, workload или
уровень прогрева недоступен, отметить его `not available` и указать
задачу-prerequisite; не подменять его старым числом S0-13.

## Шаблон результата

```text
Сценарий / версия:
Дата и оператор:
Исходники (commit, состояние дерева):
Хост (модель, CPU, RAM, OS/build, toolchain):
Сборка и параметры:
Приложение / runtime и версия:
Входы и SHA-256:
Seed / настройки / число потоков:
Проверка корректности и ожидаемый результат:
Метрика и способ измерения:
Режим: холодный / прогретый
Условия: питание, температура, фоновые процессы:
Число пар / сырые результаты A / сырые результаты B:
Медиана и разброс A / B:
Пиковая или измеренная память A / B:
Отклонённые прогоны и причина:
Вывод и ограничения:
Путь к сырым данным:
```

## Хранение

Сырые данные хранить вне Git в `work/` с именем, включающим сценарий, дату и commit. Краткий отчёт, ссылки на данные и выводы по незавершённым измерениям хранить в `docs/reports/` после появления конкретного результата. Пока результат не получен, отчёт не создавать и производительность продукта не заявлять.

## S3-17: translation-key lookup prototype

`tools/bench` содержит отдельный prototype, не production cache. На ARM64
macOS `record` читает newline-separated список путей к frozen fixture-v1
JSON, запускает interpreter и текущий iterative dispatcher для correctness,
а `RecordingExecutor` записывает `state.rip` при каждом реальном входе в
`execute_one_block` до вызова `PublishedBlockExecutor`. Trace не выводится из
финальных snapshots. На других платформах `record` явно отказывает; готовый
trace можно проверить и измерить через portable `measure`.

Пример после подготовки неизменённого списка case paths и создания `work`
директории:

```sh
cargo run --release -p od-bench --offline -- record \
  --case-list work/s3-17/cases.txt --output work/s3-17/dispatch-trace.json
cargo run --release -p od-bench --offline -- measure \
  --trace work/s3-17/dispatch-trace.json --process-run 1 \
  --output work/s3-17/measure-1.json
```

Для независимых process-level repeats запускать measure с `--process-run 2`
и `3` и разными output paths. Trace и key construction выполняются до timing.
Каждый suite чередует Vec/HashMap порядок на 20–100 paired samples; все четыре
kernel (cold Vec, cold HashMap, warm Vec, warm HashMap) калибруются отдельно до
batch не короче 200 ms. Сохраняются raw elapsed/operation samples и
median/min/max/IQR. Оба варианта ищут полный `TranslationKey` и выполняют один
и тот же `dependencies_match_snapshot`; stale scenarios сначала находят старый
key, затем обнаруживают mismatch текущей версии/permission.
Cold construction uses bounded micro-batches. Each raw sample sums only the
table-construction timer intervals; it drops each batch between intervals.
`elapsed_ns` excludes destruction and pauses, while report `total_elapsed_ns`
includes them. Raw report schema 3 records the number of micro-batches per
sample. Calibration retains the 200 ms minimum by increasing total repetitions
across bounded batches. Before cloning execution tables, admission accounts
for resident suite storage and the execution Vec/HashMap copies, then limits
one cold batch to the remaining 128 MiB logical budget. This accounting includes
inline key/table slots and owned key buffers; cold HashMaps reserve for their
known entry count before insertion. HashMap control bytes, allocator metadata,
runtime overhead, and process RSS are not measured.
Три завершённых process runs на frozen corpus и их ограничения записаны в
[отчёте S3-17](../reports/STAGE_3_S3-17-lookup-measurements-2026-10-26.md).
Полная fixture-v1 decode/admission проверяется до построения сценариев;
каждый replay entry и каждый record принадлежат известным instruction
boundaries. Сценарий observed unique keys сохраняет исходное fixture
соответствие, включая несколько разных block keys одной программы. Полный
30-minute limit включает подготовку/replay validation; report отдельно
показывает `preparation_elapsed_ns`, которое не входит в timed series. Trace
schema хранит capture host (OS/version, architecture/CPU), build-bound
compiler/revision/source fingerprint/profile и отдельный runtime
cwd/repository-revision/dirty context. Measure report сохраняет SHA-256 точных
trace JSON bytes и соответствующую build/runtime metadata самого измерения.

### S3-18d1: bounded native dispatch/cache measurements

`od-bench dispatch-cache` is opt-in and requires Apple Silicon macOS. It accepts
only the frozen S3-17 case list and dispatcher trace (checked by exact SHA-256),
loads all fixture-v1 inputs, validates full decode/admitted trace boundaries,
and computes the interpreter oracle before timing. No substitutes or generated
inputs are accepted. Scenario selection is deterministic and written into the
report: the original 1010-case cold sweep, two/four repeated corpus sweeps,
hot single-block sets of 1/8/16, a recorded multi-block fixture with at most 16
distinct entries, the 17-key capacity edge, and hot-8/hot-16 sequences followed
by a distinct cold insertion.

The four variants are uncached `PublishedBlockExecutor`, one-entry cache over a
16-slot arena, multi-entry FIFO(16), and opt-in LRU(16). `MultiEntryBlockCache::new`
continues to select FIFO. Preflight, interpreter oracle, state reset and result
validation are outside timed intervals; each timed call includes dispatch
decode/key work, compile/publication misses, eviction and native execution.
Each paired sample runs the same input sequence the same number of times across
all variants. Per-variant calibration chooses one common repetitions count for
the scenario; every measured sample must accumulate at least 200 ms or the
entire run fails without writing a result. Samples rotate variant order and
record elapsed nanoseconds, calls, sweeps, cache counters/high-water, median,
min/max, IQR and paired ratios. The report also records arena slots, free slots,
slot size and actual page-rounded mapping bytes. Output is written atomically.
`corpus-cold-sweep` starts each measured sweep with an empty cache. The 17-key
capacity sequence is warmed once and retained between timed sweeps to measure
steady churn. Arena construction/destruction itself is outside timed intervals;
the cold label refers to an empty code cache, not cold-process or full mapping
lifecycle cost.

The exact frozen inputs are `work/tmp/stage3/s3-17/cases.txt` and
`work/tmp/stage3/s3-17/dispatch-trace.json`; their accepted SHA-256 values are
`b9276b52556b3692552de074e83cd8aef0c777f29861bd19ab09c188e41615d3` and
`bf8d5efe0c52a47cbb4682f67ea73a82e684d463733aff3be60fed0b280bd5b3`.
Collect three separate release process runs after the benchmark path has been
reviewed:

```sh
for run in 1 2 3; do
  cargo run --release -p od-bench --offline -- dispatch-cache \
    --case-list work/tmp/stage3/s3-17/cases.txt \
    --trace work/tmp/stage3/s3-17/dispatch-trace.json \
    --process-run "$run" \
    --output "work/tmp/stage3/s3-18/dispatch-cache-$run.json"
done
```

Three sequential release process runs were collected on 2026-10-30. Raw reports,
host/revision/input hashes, cold-sweep medians and limits are recorded in the
[S3-18d1 report](../reports/STAGE_3_S3-18d1_2026-10-30.md). This is bounded
dispatch/cache evidence only; it does not measure interpreter or legacy-fragment
timing, arena/cache construction/destruction, or a single cold compile+first
execution. No product speed or cache-policy conclusion is made. Full Stage 3
performance acceptance and S3-18 remain open.

### S3-30a: bounded register-home allocator cost probe

`od-bench allocator-cost` is an opt-in Apple Silicon macOS release diagnostic.
It compares the existing `register_homes` budget4 baseline against budget0
spill-only configuration on one frozen six-instruction register-only
multi-helper block. Two initial states produce ZF=1 and ZF=0; the final `JZ +0`
converges to `code_end` on both outcomes, so the fixture does not measure
distinct branch routing. The command changes neither the allocation algorithm
nor the production default.

Build the release runner in an isolated target directory, run its non-scored
preflight, then use a distinct output path for each sequential process:

```sh
CARGO_TARGET_DIR=/tmp/od-bench-s3-30a-target cargo build --release --offline -p od-bench
/tmp/od-bench-s3-30a-target/release/od-bench allocator-cost \
  --preflight-only --output work/tmp/stage3/s3-30a/preflight-current.json
for run in 1 2 3; do
  /tmp/od-bench-s3-30a-target/release/od-bench allocator-cost \
    --process-run "$run" \
    --output "work/tmp/stage3/s3-30a/run${run}-fa0c132.json"
done
```

The archived standalone preflight report was built from the earlier reviewed
source revision `80d4bd3289727b211a620ba08965efc9bf57bea1`. Each scored process
at `fa0c1329a432885432bb8ada3e45c50edc69b830` performs its own correctness
preflight and per-sample validation. Full CPU state, exit, retired count and
remaining budget must match; only the specified `BlockComplete` versus
`InstructionBudgetExhausted` end-of-block result is normalized. One published
mapping is retained at a time. Compile batches have 32,768 calls and include
destruction of each unpublished `CompiledBlock`; publication and published
mapping destruction are reported separately. Repeated interpreter calls and
direct `PublishedCode::execute_block` native block-entry calls are timed; cache
lookup and dispatcher work are excluded. State setup/reset and validation are
outside the interval. The shared calibrated count is 8,388,608 calls per side,
with a 200 ms floor; each process records 20 alternating-order paired compile
samples comparing the two budgets and 20 interpreter/native execution pairs
for each case and budget. Every pair in all three reports passed the floor.

Three sequential release processes on Apple M3 Max were reviewed at source
revision `fa0c1329a432885432bb8ada3e45c50edc69b830`. Across 60 retained samples
per case/budget, raw-total-derived native medians were 43.5195 ns/call
(41.9681–44.9925) at budget4 and 44.3761 (42.5741–46.0665) at budget0 for
ZF=1; for ZF=0 they were 43.2271 (41.3485–47.8473) and 45.2711
(43.4873–46.5666), respectively. Paired compile budget0/budget4 ratio median
was 0.99284 (range 0.95969–1.03218, 60 pairs). Budget4 emitted 3,052 bytes
with four register-home values and ten spilled temporaries; budget0 emitted
3,040 bytes with no homes and fourteen spills. These overlapping, bounded
measurements do not select an allocator or a production default. Stage2
cargo/rustc activity appeared in point-in-time snapshots during all three
processes; there are no per-pair timestamps to attribute timing variation.
Runtime `working_tree_dirty` is unknown because metadata collection timed out;
run1's numeric executor exit was unavailable, although its report completed
without failure and all sample floors passed. Mac mini checks remain deferred
until the end of Stage 3. See the [S3-30a report](../reports/STAGE_3_S3-30a_2026-11-01.md)
and [reviewed raw evidence](../reports/data/stage3/s3-30a/).

### S3-30b: bounded distinct-branch allocator-configuration profile

`od-bench allocator-cost-branch-routing` is a separate opt-in Apple Silicon
macOS release diagnostic. It preserves the S3-30a six-instruction prefix and
frozen input states, but changes the final `JZ` displacement to `+10` and adds
an admitted `MOV RBX,imm64` fallthrough suffix. The exact 27 bytes are
`48 03 C0 48 2B D2 48 03 C3 48 2B C9 48 3B C3 74 0A 48 BB 11 00 FF EE DD CC BB AA`;
SHA-256 is
`72210fb82b0ffa9f88f10f63697a1051347d07587e3569028947119726fac8c7`. At base
RIP `0x1000`, the JZ-taken input (ZF=1) exits at `0x101b`; the not-taken input
(ZF=0) exits at the fallthrough RIP `0x1011`. The suffix is outside the
Jcc-terminated emitted block and outside the timed native call. This measures
the configuration cost of `register_homes=4` versus diagnostic `0`; it does
not isolate marginal spill cost, choose an allocator algorithm, or select a
production default.

Build the release runner in an isolated target directory. Run a correctness
preflight and an unscored calibration-only pass before collecting three
sequential scored process reports:

```sh
CARGO_TARGET_DIR=/tmp/od-bench-s3-30b-target cargo build --release --offline -p od-bench
/tmp/od-bench-s3-30b-target/release/od-bench allocator-cost-branch-routing \
  --preflight-only --process-run 1 \
  --output work/tmp/stage3/s3-30b/preflight.json
/tmp/od-bench-s3-30b-target/release/od-bench allocator-cost-branch-routing \
  --calibration-only --process-run 1 \
  --output work/tmp/stage3/s3-30b/calibration-only.json
for run in 1 2 3; do
  /tmp/od-bench-s3-30b-target/release/od-bench allocator-cost-branch-routing \
    --process-run "$run" \
    --output "work/tmp/stage3/s3-30b/run${run}.json"
done
```

The preflight checks both exact branch targets, full-state parity, exit and
retirement accounting, and allocation limits before timing. Calibration-only
emits no scored samples; each scored process independently repeats correctness
preflight and calibrates interpreter/native repetition counts for both budgets
and branch cases, writing its own adjacent calibration journal. The standalone
calibration report is a feasibility check and does not supply repetition
counts to the scored runs.
All durations are retained raw. A pair is one sequential budget4/budget0
comparison for the same branch input; pair order alternates, and each side is
published, warmed, timed, then dropped before the other side is published.
There are 20 such native allocator-configuration pairs per branch input per
process. Every timed compile or execution side must meet the 200 ms floor; any
short side leaves the full process diagnostic-only rather than trimming or
scoring only passing pairs.
The profile also records 20 paired compile samples per process. Scored reports
include the per-side warm-up count (64 calls per publication) and the pair
orders so the protocol can be audited.

Timer boundaries remain separate: compile samples time block construction,
lowering, allocation, ARM64 emission/finalization, and destruction of each
unpublished compiled block; publication and published-mapping drop are timed
and reported separately. Warm-up calls are outside the execution timer. The
execution timer covers repeated direct `PublishedCode::execute_block` calls,
including safe host wrapper/context setup, generated entry, and result copy/
decode; state setup and full-state/exit validation are outside it. The harness
retains only one `MAP_JIT` mapping at a time. It does not measure cache lookup,
dispatcher traversal, or the fallthrough suffix, and the execution timing is
not pure guest-instruction time. This profile adds no allocator/runtime policy
change. Three release runs on Apple M3 Max completed with all 360 measured
compile/execution sides above the 200 ms floor; each native side used
8,388,608 calls. From the 60 raw budget0/budget4 pair totals per set, compile
ratio median/range was 0.996036170827 (0.962808301520–1.045968410700), JZ-taken
was 1.019455351209 (0.981907318288–1.048155961189), and JZ-fallthrough was
1.038610853426 (1.008862994910–1.086055631502). These are pooled raw-pair
ratios, not ratios of run medians. Budget4 used four home values and ten spills
with 3,052 emitted bytes; budget0 used no homes and fourteen spills with 3,040
bytes; both frames were 288 bytes. This remains a bounded configuration-cost
comparison, not marginal spill cost, allocator algorithm/winner/default
evidence, or downstream dispatcher behavior. Runtime Git status timed out
after two seconds, so runtime `working_tree_dirty` is unknown (`null`); the
three runtime revisions differ while the built source revision/fingerprint is
common. Mac mini checks remain deferred until the end of Stage 3. See the
[S3-30b report](../reports/STAGE_3_S3-30b_2026-11-01.md) and
[suite manifest with checksums and raw-derived summaries](../reports/data/stage3/s3-30b/suite-manifest.json).

### S3-35a: cold first full-corpus execution

`od-bench cold-first-run` is an opt-in Apple Silicon macOS release benchmark
for the full frozen S3-17 register-only case list and trace. It validates the
exact input hashes and admitted trace boundaries, decodes/admit-checks all
1,010 cases, and computes the interpreter oracle before timing. It also runs
and discards a native correctness preflight before timed samples.

Each sample consists of complete corpus sweeps. Interpreter and default
FIFO(16) block dispatcher samples use the same cases, input states, sweep
count, and output validation. Sample order alternates within pairs. Every cold
dispatcher sweep starts with a newly initialized runtime, arena, and empty
code cache. Its timed total includes runtime/cache/arena initialization plus
dispatch lookup, decode/key work, compilation, publication, and first corpus
execution. Cache reuse within that same corpus sweep is intentional and is
part of the workload. Interpreter timing includes its full case execution.
Fixture loading, interpreter oracle/preflight, state cloning/reset,
post-execution validation, process launch, and runtime/cache/arena teardown are
outside the timed intervals; teardown remains included in whole-run wall time.
Reports persist build/revision/fingerprint and host/toolchain metadata, input
hashes, exact timing scope, calibrated/common sweep counts, raw samples and
counters, median/range/IQR, and paired per-sweep interpreter/FIFO ratios.
After the independent per-variant calibration, a paired calibration pass
measures both variants at the same sweep count and raises that count until
each timed sample reaches 200 ms. Measured pairs retain that fixed common
count. If either side of any measured pair falls below the threshold, the
partial series is discarded, the shared count is raised from that pair's
observed duration, and the full series restarts. The report records these
series restarts with the short pair's timings and old/new sweep counts; no
short pair is accepted into reported statistics. This restart metadata is part
of the cold-first-run report schema 2.

After review, collect three separate release process runs:

```sh
for run in 1 2 3; do
  cargo run --release -p od-bench --offline -- cold-first-run \
    --case-list work/tmp/stage3/s3-17/cases.txt \
    --trace work/tmp/stage3/s3-17/dispatch-trace.json \
    --process-run "$run" \
    --output "work/tmp/stage3/s3-35/first-run-$run.json"
done
```

Three release process runs were collected on 2026-10-30. The cold FIFO
dispatcher measured 6.64–6.69 ms per 1,010-case sweep; interpreter measured
0.164–0.166 ms. The [S3-35a report](../reports/STAGE_3_S3-35a_2026-10-30.md)
records normalized medians, raw JSON, source/input hashes, calibration
restarts, and two earlier failed calibration attempts. Those attempts produced
no result report and are excluded from the statistics. This is one bounded
cold register-only workload; it does not support speed claims for other
workloads. Full pipeline component timings, warm execution and resource gates
remain open. Legacy fragment timing remains omitted because equivalent
admission and semantics were not established for this corpus.

### S3-35b: cold dispatcher pipeline attribution

`od-bench cold-phases` uses the same frozen S3-17 case list and trace on the
real FIFO16 dispatch/cache/compiler path. It reports distinct intervals for
block build/decode and admission, key/snapshot/lookup, IR lowering, allocation,
emission plus byte encoding/finalization, publication, and host execution
boundary. Normal lookup includes block build and key lookup. When the
dispatcher reuses the index for the current or previous RIP, the build
phase checks the saved index and the lookup phase records the hit. Hit
handling includes adjusting the saved index after LRU reordering. Guards,
cache install and other accounting remain in `other_dispatch_ns`.
Stage intervals are non-overlapping and are components of the
instrumented dispatch pass, not standalone end-to-end measurements.
An observed phase spans the `Instant` sample taken by `begin` through the clock
read taken by `end`; it includes the wrapped operation and callback/wrapper
transit between those samples. Recorder bookkeeping after the end sample is
outside that phase. The paired ordinary/observed totals measure overall
instrumentation overhead.

The ordinary path uses a statically selected no-op observer; only the bench
dependency enables `od-jit/phase-observation`, and wall-clock phase timestamps
are available through a separate observed dispatcher entry point. Each sweep
constructs a fresh FIFO16 runtime/cache/arena. Input parsing, interpreter-oracle
checking, per-case state clone and output validation are outside dispatch-call
intervals; initialization is recorded separately and teardown is outside them.
Ordinary and observed preflight must match each case's interpreter result for
all `LongModeState` fields, masking only fixture-declared undefined RFLAGS bits,
and match aggregate calls, retirement, native blocks and all cache counters
before sampling. Phase event counts are checked against visits, cache
misses/compiles, publications and host executions.

Each paired sample must independently contain at least 200 ms of dispatch
intervals; short samples discard and restart the series at a larger common
sweep count, persisting each floor-triggering pair and restart decision in the
report. Collect three sequential release processes after independent source
review and during a quiet host window. Preserve raw samples,
source/input hashes, host/toolchain metadata and pair order. Report paired
observer overhead and uncertainty across all 60 pairs. Do not sum phase medians
or compare phase components with differently scoped whole-path reports as an
exact total. This slice does not select a cache policy or change a default.

```sh
for run in 1 2 3; do
  cargo run --release -p od-bench --offline -- cold-phases \
    --case-list work/tmp/stage3/s3-17/cases.txt \
    --trace work/tmp/stage3/s3-17/dispatch-trace.json \
    --process-run "$run" \
    --output "work/tmp/stage3/s3-35/pipeline-clean-$run.json"
done
```

Three clean release process runs completed on 2026-10-31 after independent
source review. Across 60 accepted paired samples the ordinary FIFO16 dispatch
median was 6.752 ms per 1,010-case sweep and the observed median was 6.986 ms.
The observed/ordinary ratio median was 1.0301 (IQR 1.0261–1.0342; range
0.9831–1.1075). Per-phase event medians, spread, host/source/input hashes, and
raw JSON hashes are in the [S3-35b report](../reports/STAGE_3_S3-35b_2026-10-31.md).
An initial provisional series was excluded when a Stage 2 test appeared during
that window; the test was not stopped, and the complete three-run series was
repeated after it exited. This evidence covers only the frozen register-only
corpus and does not select an optimization or default.

### S3-31: bounded warm dispatcher phase harness

`od-bench warm-dispatch-phases` is an opt-in bounded measurement harness for the
accepted S3-17 frozen case list and dispatch trace. It checks the case-list
SHA-256 `b9276b52556b3692552de074e83cd8aef0c777f29861bd19ab09c188e41615d3`
and trace SHA-256
`bf8d5efe0c52a47cbb4682f67ea73a82e684d463733aff3be60fed0b280bd5b3`
(1,010 inputs, 1,322 visits). The two strata stay separate:

| Stratum | Frozen input and visits | Fixture SHA-256 |
|---|---|---|
| `recorded_multiblock` | case index 9, `gen_v1_000000005eedcafe_00000009`, entry RIPs `[0, 15]` | `bb7f1c493a7d7ef128e87fc0b1b3b10c58f176d292bd8d186adc33a1f9424913` |
| `self_loop` | case index 1003, `s0_09_sub_jnz_loop`, entry RIPs `[0, 0, 0]` | `720abcca7c3bbc11593b1b6a1a908cb24206eadb7df3d3eeed14b2340091227c` |

Each stratum uses one shared warmed FIFO(16) engine and arena. An ordinary
cold prewarm compiles and admits the resident entries; then ordinary and
observed hit-only preflight calls run on that same cache, compare full result
and state against the interpreter oracle (masking only fixture-declared
undefined RFLAGS bits), and check per-call counter deltas.
Calibration and measurements alternate the two paths on the shared runtime.
Each sample records cumulative cache counter snapshots before and after its
sweep and the resulting delta; expected hits equal visits per call times
sweeps, with no misses, compiles, publications, or evictions. Full result/state
validation is outside each timed call.

For each of three release process runs, the harness collects 20 paired samples
per stratum. Each side of every pair must reach a 200 ms timer floor; if not,
the paired series restarts with a larger common sweep count. The `order` field
records which path ran first. Keep stratum results separate. Three bounded
release process runs are complete; cross-run synthesis is in the
[S3-31 report](../reports/STAGE_3_S3-31_2026-11-01.md) and
[suite manifest](../reports/data/stage3/s3-31/suite-manifest.json). Broad
S3-31 acceptance, total lookup attribution, and any linking claim remain open.

The full timed interval wraps `Variant::call` or `Variant::call_observed`, so
it includes adapter setup (CodeView, dependency and control/context setup) as
well as the complete dispatcher call. State cloning/reset, PhaseRecorder
construction/reset, full result/state validation, and phase-total extraction
are outside the interval. Warm active phases are block build/admission,
`key_snapshot_and_initial_lookup`, and the host execution boundary. The lookup
phase covers minimum sizing, key construction, initial snapshot validation,
one `cache.find`, and its hit/miss update for normal lookup. When reusing
the current or previous RIP, the build phase checks the saved index and
the lookup phase records the hit. Hit handling includes adjusting the
saved index after LRU reordering. Next-entry selection at continuation, cached-snapshot
and code-view revalidation, guards, accounting, other cache bookkeeping,
and adapter and recorder work contribute to the
unattributed residual. The host boundary includes backend
and arena checks, host context, generated entry, and result decoding, so it is
not pure guest execution time.

Observed/ordinary ratios are the net difference between the two full call
paths, including recorder work and any generic-instantiation differences;
they are not a pure observer-cost estimate. The phase split does not isolate
total lookup cost, pure generated-code host-execution time, or a linking
benefit. High-water resident counters and capacity-derived available slots
are not live occupancy. No linking, cache-policy, production-default, or
performance conclusion follows from the harness alone.

```sh
for run in 1 2 3; do
  cargo run --release -p od-bench --offline -- warm-dispatch-phases \
    --case-list work/tmp/stage3/s3-17/cases.txt \
    --trace work/tmp/stage3/s3-17/dispatch-trace.json \
    --process-run "$run" \
    --output "work/tmp/stage3/s3-31/warm-dispatch-$run.json"
done
```

### S3-35c: cold direct-entry checked-memory comparison

`od-bench two-site-memory-cold` is an opt-in Apple Silicon release profile for
the exact two-sequence S3-34a subset from the shared `od-testkit` provider:
three fixed seeds and 400 cases for each sequence/seed combination, 2,400
unique inputs total. The third S3-34a sequence is excluded. Every input gets
two preflight executions per backend. Preflight and every result compare full
CPU state, RAM, normalized exit, retirement, remaining budget, and exact
ordered callback records (addresses and widths), including deterministic
same-backend equality across the two trials.

For each native timing repetition, the timer starts before IR memory-block
compile and includes publication, direct typed entry execution, and explicit
destruction of `PublishedCode` and the IR block. The interpreter timer includes
only interpreter execution. Fixture construction, state/RAM reset, and full
result validation are outside both timers. This path has no dispatcher,
VMHost, or cache/artifact reuse across inputs/repetitions.

Each process calibrates independently, then runs 20 alternating pairs at 1,024
repetitions per side. Both sides must meet the 200 ms floor in every pair; a
failed floor classifies the whole process diagnostic-only and preserves all
samples without trimming. A previous lower-cap calibration that missed the
interpreter floor is retained as diagnostic; the separate calibration-only run
met the floor at 1,024. The projection artifact estimates 16.7555 minutes for
cumulative calibration through the reserve cap of 2,048 plus 20 pairs at 2,048.
This is linear inference, not a measured full-run duration; it excludes setup,
teardown, and other overhead. Scored runs calibrated and measured at 1,024.

Scored commands, each with a distinct output path:

```sh
for run in 1 2 3; do
  cargo run --release -p od-bench --offline -- two-site-memory-cold \
    --process-run "$run" \
    --output "work/tmp/stage3/s3-35c/run$run.json"
done
```

Three reviewed release process runs completed on 2026-11-01. All 60 timing
pairs passed the floor. The raw-total native/interpreter ratio median was
71.4765 (range 66.7736–93.9423); run medians were 70.0309, 71.7841, and
71.1482. No sample was trimmed. Stage 2 compiler activity overlapped sampled
intervals in all runs, with Graphify activity sampled in runs 1 and 3. Monitoring
is point-in-time, reports have no per-pair timestamps, and exact impact on
individual pairs is unknown. Run1/run2 numeric executor exits were not retained
(success is inferred from report status/all-floor gate); run3 exit0 was captured.
Runtime dirty-tree status is unknown in all reports. This is a bounded fixture
result only; it does not select a production default or general winner. See the
[S3-35c report](../reports/STAGE_3_S3-35c_2026-11-01.md),
[reviewed suite manifest](../reports/data/stage3/s3-35c/suite-manifest.json),
and [raw evidence archive](../reports/data/stage3/s3-35c/).

### S3-35d: fresh runner process comparison

The process benchmark runs interpreter, legacy fragment JIT
(`--engine jit`) and the uncached IR dispatcher (`--engine jit-dispatch`)
on the same five S3-14 branch/helper fixtures. The case list, trace and
each fixture's bytes must match the recorded S3-36b digests. The trace
binds the inputs. The interpreter output from the current build provides
the comparison state. All variants pass a preflight, and every timed
launch must match that output, including exit, retirement and snapshot.

Each fixture starts a fresh release `od-runner` process. The timed interval
includes spawn, fixture parsing, engine execution, JSON transfer and child
exit. JSON parsing and validation in the benchmark controller are outside
the timer. Each sweep sums five intervals and retires 14 guest instructions.
No guest code survives between processes. macOS file caches are left in
their normal state. These timings include startup and cannot replace the
in-process cold or warm measurements above.

From the repository root:

```sh
OPEN_DESKTOP_GIT_REVISION="$(git rev-parse HEAD)" \
  RUSTC_VERSION="$(rustc --version)" cargo build --release --offline -p od-runner
mkdir -p work/tmp/s3-35d
for run in 1 2 3; do
  python3 tools/bench/stage3_cold_process.py --output "work/tmp/s3-35d/run-$run.json"
done
```

The script needs local macOS host-model information from `sysctl`. It
calibrates a common sweep count to at least 300 ms per engine, then repeats
every engine order four times across 24 groups. Each scored sample must
reach 200 ms. Three sequential runs on Mac15,11 / Apple M3 Max, macOS
26.7.1 (25G241), used release revision `31ecf35` and 32 sweeps per sample.
All 34,560 scored child launches passed the input and parity checks. The
shortest sample was 404.262 ms. One-minute host load averages measured
at the start and end of each run ranged from 8.02 to 12.04. Power and
thermal state were not sampled.

Median ms per five-process sweep, with observed minima and maxima:

| Run | Interpreter | Legacy fragment JIT | IR dispatcher |
| --- | ---: | ---: | ---: |
| 1 | 14.073 [12.964, 35.738] | 14.082 [12.633, 31.541] | 14.314 [13.099, 17.561] |
| 2 | 15.459 [13.879, 35.571] | 15.277 [13.680, 36.319] | 15.587 [13.780, 36.734] |
| 3 | 14.168 [13.573, 35.639] | 14.150 [13.644, 36.686] | 14.277 [13.843, 36.749] |

Across the 72 paired observations, the legacy/interpreter median time
ratio was 0.9877 [0.5010, 1.5477] and dispatcher/interpreter was 1.0065
[0.3829, 1.3372]. The observed spread crosses parity for both comparisons.
The measurements do not establish a speed difference or a default policy.
A pilot under the workspace sandbox did not write a report because
`sysctl` was denied. The measured runs queried the host before calibration.
Raw evidence is archived as [run 1](../reports/data/stage3/s3-35d-cold-process/run-1.json),
[run 2](../reports/data/stage3/s3-35d-cold-process/run-2.json) and
[run 3](../reports/data/stage3/s3-35d-cold-process/run-3.json).

### S3-36a: bounded resident warm register-only execution

`od-bench warm-first-run` is an opt-in Apple Silicon macOS release benchmark
for a deterministic prefix of the frozen S3-17 cases that fits within the
FIFO(16) cache. It validates and admits the full frozen case list and trace,
then selects complete cases from the prefix until their summed per-case
`unique_entries` budget reaches the cache capacity. This conservative sum is
an upper bound, not the global count of distinct cache keys. It prewarms one FIFO runtime outside timed
samples and retains that runtime through calibration and measurement. Timed
dispatcher calls must report cache hits with zero additional misses, compiles,
publications, or evictions. Each sample resets guest state from the same
fixture values and validates full outcomes outside the timed call intervals.
Interpreter samples run the same selected cases and work. Samples alternate
order, use a common calibrated sweep count, and report raw counts, normalized
medians/range/IQR, cache counters and paired per-sweep ratios.

The complete 1,010-case frozen corpus is unsuitable as a resident warm sample:
FIFO(16) capacity is smaller than the active working set. A recorded cold
corpus sweep observed 1,320 misses/compiles and only 2 hits, so repeating the
whole corpus would measure churn/recompilation, not steady-state cache hits.
Three release runs measured the warm FIFO dispatcher at 12.129–12.343 µs per
14-case sweep and the interpreter at 2.019–2.053 µs. The interpreter was about
6.0× faster on this slice; this does not support a result for other workloads.
The raw JSON, hashes, counter checks, restart data and timing boundaries are in
the [S3-36a report](../reports/STAGE_3_S3-36a_2026-10-30.md). This path covers
only a bounded register-only resident slice; S3-36b records separate narrow
branch/helper evidence. Memory and the full workload/resource matrix remain
open.

### S3-36b: bounded branch/helper warm profile

`od-bench warm-branch-helper` is an opt-in Apple Silicon macOS release
benchmark bound to the exact case-list and trace digests for the five accepted
S3-14 fixtures: four conditional-branch boundary cases and
`s3_14_live_through_helpers`. It validates interpreter/JIT outcomes and
retirements before timing, selects a deterministic prefix that fits FIFO(16),
prewarms one retained FIFO runtime, and rejects timed dispatcher samples unless
they contain hits with zero miss/compile/publication/eviction deltas. Calibrated
interpreter and dispatcher samples alternate over the same reset inputs. Raw
sample durations, paired ratios, counters, input hashes and timer boundaries
are recorded. Three release runs measured FIFO at 3.736–3.959 µs/sweep and
interpreter at 0.726–0.751 µs/sweep; the interpreter was about 5.3× faster on
these fixtures. The fixed list, trace, raw results, hashes and timer scope are
in the [S3-36b report](../reports/STAGE_3_S3-36b_2026-10-30.md). This is
bounded branch/helper evidence; it does not cover memory, fallback or
invalidation workloads and does not complete S3-36.

### S3-56: generic register cold and warm on the same resident corpus

The existing `cold-first-run` and `warm-first-run` commands accept
`--generic-register` on Apple Silicon macOS release builds. This opt-in
selects the same frozen five S3-14 cases and trace used by S3-36b and runs
`NativeRegisterV1Admission` through `run_multi_cached_native_register_v1_controlled`
with eager flags and FIFO(16). Without the flag both commands retain their
S3-17 profiles. The default FIFO `cold-first-run` also accepts the exact
S3-14 case list and trace. Use `warm-branch-helper` for the corresponding
resident FIFO profile on those inputs. Other commands reject the flag.

Both list and trace SHA-256 values, each fixture/program hash, trace visits,
interpreter outcome and retirement are validated before timing. The generic
admission is built before timing. Preflight rejects a bare NOP: it is the only
current NativeRegisterV1 interpreter handoff, and each successful measured
generic dispatch also requires `retired_interpreter == 0`. Thus the recorded
zero fallback-helper calls are inferred from this checked eligibility and
successful dispatch, not from an instrumented helper-call counter. This is a
native-only workload; it does not exercise the blocked S3-55 mixed fallback.

Cold samples sum fresh FIFO arena/cache initialization and calls over all five
cases; warm samples use one prewarmed resident FIFO cache and reject any
measured miss, compilation, publication or eviction. The interpreter runs the
same cases and reset states. State cloning, fixture/trace loading, admission,
oracle preparation and result validation remain outside timed call intervals.
Each timed generic `Variant::call` includes benchmark-wrapper selection,
`CodeView`/dependency and fixture-context construction, and initialization of
the stop byte as well as the dispatch itself; it is not a bare dispatcher-only
latency. The generic/legacy comparison therefore includes these harness costs.
Each process uses the existing common-sweep calibration: 20 alternating pairs,
at least 200 ms for each side; an under-duration sample discards and restarts
the series. Reports include raw sample times, retirement split, cache counters,
derived hit rate, median/range/IQR, source fingerprint and host configuration.

For example, from the repository root, use fresh output paths:

```sh
mkdir -p work/tmp/s3-56-repro
for run in 1 2 3; do
  cargo run --release -p od-bench --offline -- cold-first-run --generic-register \
    --case-list tests/fixtures/x86/stage3/s3_14_branch_helper_case_list.txt \
    --trace docs/reports/data/stage3/s3-36b/trace.json --process-run "$run" \
    --output "work/tmp/s3-56-repro/cold-$run.json"
  cargo run --release -p od-bench --offline -- warm-first-run --generic-register \
    --case-list tests/fixtures/x86/stage3/s3_14_branch_helper_case_list.txt \
    --trace docs/reports/data/stage3/s3-36b/trace.json --process-run "$run" \
    --output "work/tmp/s3-56-repro/warm-$run.json"
done
```

For a FIFO comparison on this same corpus, run `cold-first-run` with the
paths above and omit `--generic-register`. Run `warm-branch-helper` with
those paths for the warm side.
[Matched FIFO/generic runs](../reports/STAGE_3_S3-56_MATCHED_2026-11-19.md)

The [S3 Core report](../reports/STAGE_3_CORE_REPORT.md)
records the six process runs and their limitations. S3-35a's 1,010-case cold
corpus differs from this five-case workload; its numbers are context, not a
direct comparison. S3-36b uses the same five-case digests and is comparable as
a bounded historical warm observation, with the source-revision difference
reported. Neither comparison accepts Stage 3 as a whole.

### S3-36c: resident warm NOP-fallback comparison

`od-bench warm-native-fallback` selects 19 seed/index identities from the
frozen S3-34p NativeRegisterV1 corpus. The preflight recomputes all 1,200
interpreter observations, binds the exact corpus bytes and input digest, and
checks both seed and index for the selected set. It rejects a selected input
unless it reaches `BudgetExhausted`, consumes one interpreter-retired NOP at
the budget boundary after either two or three native instructions (later code
bytes can remain unexecuted), and its eager repeat
dispatch performs exactly one cache hit with zero misses, compiles,
publications, or replacements. The ten two-native cases and nine three-native
cases remain separate strata.

For each input in turn, the release runner creates one capacity-one cache,
executes and validates one untimed prewarm, and retains that single executable
mapping throughout calibration and measurement. It drops the mapping before
moving to the next case. Twenty interpreter/cached pairs alternate order for
each case; a shared repetition count is calibrated to at least 200 ms per
side, with the whole series restarted if a pair falls below the floor. The
timed intervals cover the complete eager dispatcher or interpreter execution;
state reset, `CodeView`/control setup, validation, and cache-counter checks are
outside timing. Every timed cached dispatch must add one hit and no other cache
activity. Reports retain per-case raw pairs, ratios and spread, retirement
strata, prewarm counters, resident high-water sizes, and host/source metadata.

Run a non-scored preflight first:

```sh
cargo run --release --offline -p od-bench -- warm-native-fallback \
  --corpus work/stage3-native-register-v1/manual/native-register-v1-observations.json \
  --preflight-only --output work/tmp/stage3/s3-36c/preflight.json
```

After the implementation commit and independent source review, collect three
release processes sequentially with distinct report paths. A Stage 2 test
launch by itself is not a stop condition; record actual process/load snapshots
and keep or repeat a run based on observed impact and paired spread. Do not
trim pairs after seeing results. Three runs completed with all 20 retained
pairs per input. The run-level median-of-case-medians for cached-dispatch /
interpreter elapsed ratios ranged from 5.223–5.259 for the 10 two-native-plus-NOP
cases and 4.313–4.376 for the nine three-native-plus-NOP cases. Stage 2
compiler/test activity
overlapped snapshots in all runs; run 3 also had a brief Graphify update
overlap; a point-in-time snapshot sampled that process at 89.5% CPU. The
reconstructed snapshots lack per-pair wall-clock times, so overlap
effects cannot be attributed. This profile measures only the frozen warm
native-plus-NOP `BudgetExhausted` inputs; it does not establish general
fallback, memory, physical x86, Mac mini, or full Stage 3 performance behavior.
See the [S3-36c report](../reports/STAGE_3_S3-36c_2026-10-31.md) and its
[reviewed suite evidence](../reports/data/stage3/s3-36c/measurement-suite-manifest.json).

```sh
for run in 1 2 3; do
  cargo run --release --offline -p od-bench -- warm-native-fallback \
    --corpus work/stage3-native-register-v1/manual/native-register-v1-observations.json \
    --process-run "$run" \
    --output "work/tmp/stage3/s3-36c/warm-native-fallback-$run.json"
done
```

### S3-36d: resident warm direct-entry checked-memory profile

`od-bench two-site-memory-warm` is an opt-in Apple Silicon macOS profile over
only the original two S3-34a sequences: `store_then_load` and
`load_then_store`, three fixed seeds, and 400 inputs per seed/sequence (1,200
per sequence, 2,400 total). It excludes the later `load_then_load` sequence.
Two untimed preflight trials per input/backend compare complete CPU state,
RAM, exit/retirement/budget and callback events; this gives 4,800 executions
per backend and 4,800 parity trials. Each backend also completes one validated
warmup sweep per sequence.

For each sequence, the runner compiles and publishes one block, retains that
same mapping throughout preflight, warmup, calibration and scoring, then drops
it before compiling the next sequence. Timed samples call the direct typed
native entry or interpreter only. Reset and validation are outside the timers.
The profile has no dispatcher lookup or cache activity, so it reports no
cache hit/miss counters. Calibration-only passed the 200 ms common floor at
2,048 repetitions per side; three scored runs kept all 20 alternating pairs
per sequence and all pairs passed the 200 ms floor.

The pooled raw-total native/interpreter ratio median/range was 1.3718847445
(1.3116083380–1.4203506060) for `store_then_load` and 1.3662312961
(1.2865399535–1.4421564464) for `load_then_store`. These per-sequence results
are not combined across strata. They describe only this bounded resident
direct-entry fixture; they do not establish general memory performance,
production behavior, or a global winner. Full S3-36 dispatcher/lookup and
workload coverage remains open.

Monitoring is point-in-time. Stage 2 test processes appeared idle at sampled
scored-run snapshots; the calibration envelope also records Stage 2 compiler
activity after or near that short run. With no per-pair timestamps, overlap
and pair-level impact are unknown. Runtime dirty status is `null` because the
git status probe timed out. Mac mini checks were not run and remain deferred to
the end of Stage 3. No owner/MMU, VMHost, SMC, other memory forms, or broader
guest workloads are covered.

The preflight and calibration-only invocations are:

```sh
cargo run --release -p od-bench --offline -- two-site-memory-warm \
  --preflight-only --process-run 1 \
  --output work/tmp/stage3/s3-36d/preflight.json
cargo run --release -p od-bench --offline -- two-site-memory-warm \
  --calibration-only --process-run 1 \
  --output work/tmp/stage3/s3-36d/calibration.json
```

The three scored processes use `--process-run 1`, `2`, and `3` with distinct
output paths. The [S3-36d report](../reports/STAGE_3_S3-36d_2026-11-01.md)
and [reviewed evidence archive](../reports/data/stage3/s3-36d/) preserve the
reports, journals, monitoring envelopes, and suite manifest.

```sh
for run in 1 2 3; do
  cargo run --release -p od-bench --offline -- two-site-memory-warm \
    --process-run "$run" \
    --output "work/tmp/stage3/s3-36d/run$run.json"
done
```

### S3-36e: resident capacity-one cached checked-memory full-call profile

`od-bench two-site-memory-cache` uses only the same two frozen S3-34a
sequences, with 1,200 inputs per sequence. It retains one capacity-one
`MemoryPairCache` and one published mapping for each sequence. The native
timer covers the complete `MemoryPairCache::run`: profile/context/source and
dependency/key checks, cache lookup and counters, and typed host execution.
State/RAM/callback reset and complete result validation stay outside the timer.
Interpreter calls are timed separately. Each run requires one setup
miss/compile/publication followed by hit-only warmup, calibration, and scoring;
cache state is cleared between sequence strata.

The release preflight passed on Apple M3 Max/aarch64/macOS 26.7.1 with the
frozen input digest and 15-file source fingerprint. It performed 4,800
interpreter and 4,800 cached executions and 4,800 parity trials. Each sequence
reported 2,399 hits, one miss/compile/publication, no replacements, and one
resident entry. `timing_started` was false. Runtime Git dirty status is `null`
because the two-second Git probe timed out; the build source tree was clean.
The calibration-only release run passed the 200 ms floor for both sequences
at 2,048 repetitions per side, with 12 retained attempts per sequence and no
scored pairs. The outer `cargo` command envelope was 55.671 s; the report's
`total_elapsed_ns` was 10.621381750 s. A point-in-time envelope observed
Graphify in 4 of 14 snapshots during the before/early portion and a Stage 2
`od-runner` test in 8 of 14 snapshots; its `rustc` child appeared in six later
samples. Sampled one-minute load averages ranged from
2.16 to 2.61; the visible `rustc` samples ranged from 0.0% to 120.3% CPU.
Stage 2 was not stopped. The envelope has no per-call timestamps, so overlap
with timed calls cannot be assigned. At the time of calibration, no scored
result was available.

Scored release run 1 completed with 20 retained alternating pairs per
sequence; every side exceeded the 200 ms floor. The raw
`native_total_ns/interpreter_total_ns` median and range were 2.7592
(2.6746–2.8856) for `store_then_load` and 2.8393 (2.7617–2.9279) for
`load_then_store`. The sequence strata remain separate; later processes are
reported independently below.
The point-in-time envelope covered an outer 156.740 s command on M3 Max and
sampled one-minute load averages from 1.73 to 3.95. Graphify appeared in 7 of
34 snapshots. A Stage 2 `od-runner` test was visible in 14 of 34 snapshots
during the earlier portion of the envelope; a `rustc` child reached 21.2% CPU
in one sample. The sparse monitor
has no per-pair timing, so it cannot attribute overlap to individual samples.
Stage 2 was not stopped.

Scored release run 2 also retained all 20 alternating pairs per sequence and
passed the 200 ms floor for every side. Its raw
`native_total_ns/interpreter_total_ns` median/range was 2.6903
(2.6348–2.7647) for `store_then_load` and 2.7630 (2.7334–2.7913) for
`load_then_store`; the strata remain separate. The process recorded exactly
49,152,000 hit-only calls per sequence and no scored misses, replacements,
compiles, or publications. The envelope covered 146.600 s, with one-minute
load averages 1.84–3.29 and a Stage 2 `od-runner` test visible in 22 of 32
snapshots. Its `rustc` child reached 25.9% and 26.0% CPU in two samples; the
benchmark's own release `rustc` compile reached 109.6% in one. Graphify was not
visible. No per-pair timestamps are available.

Scored release run 3 retained 20 alternating pairs per sequence, with every
side above the 200 ms floor. Median/range was 2.7391 (2.6830–2.7733) for
`store_then_load` and 2.7306 (2.6390–2.8354) for `load_then_store`; keep the
strata separate. Scored counters again showed 49,152,000 hits per sequence and
no misses, replacements, compiles, or publications. The 136.516 s outer
envelope sampled load averages from 2.13 to 7.85, with Graphify absent. Stage 2
`od-runner` cargo tests appeared in 9 of 30 snapshots and `od-x86-runner` cargo
tests in 20 of 30. Rustc samples included the benchmark's `od_bench` compile at
524.3% CPU and Stage 2 `od_testkit`/`od_x86_runner` compiles at 14.9–53.9%.
Stage 2 was not stopped. These are sparse outer-process samples without
per-pair timestamps, so they cannot assign load to individual measurements.

Across the three release processes, the median and full range of paired
`native_total_ns/interpreter_total_ns` ratios were 2.7222892353
(2.6348242133–2.8855539942) for `store_then_load` and 2.7650079504
(2.6390215880–2.9278959372) for `load_then_store`. There were 60 pairs per
sequence; the two strata remain separate. A ratio above one means interpreter
time was lower on these inputs. This bounded result does not generalize to
other memory forms or guest workloads.

The [suite manifest](../reports/data/stage3/s3-36e/suite-manifest.json),
SHA-256 `e643469ccb79ad69ebb9347f657c680f62b90c4a7e9c1d1c29111e4ad5a57c3f`,
binds 13 preflight, calibration, scored-report, journal, and monitoring files.

Run the release preflight first:

```sh
cargo run --release --offline -p od-bench -- two-site-memory-cache \
  --preflight-only --process-run 1 \
  --output work/tmp/stage3/s3-36e/preflight.json
```

The [S3-36e report](../reports/STAGE_3_S3-36e_2026-11-01.md) and
[preflight artifact](../reports/data/stage3/s3-36e/preflight-release-run1.json),
[calibration artifact](../reports/data/stage3/s3-36e/calibration-run1.json),
and [scored runs 1](../reports/data/stage3/s3-36e/run1.json) and
[2](../reports/data/stage3/s3-36e/run2.json) and
[3](../reports/data/stage3/s3-36e/run3.json) record completed gates. The
three-run set is complete for this bounded profile. Mini checks remain deferred
to the end of Stage 3.

### S3-36f: cached NativeRegisterV1 register recurrence full-call profile

`od-bench cached-native-loop` compares the complete capacity-one cached
dispatcher call with the interpreter for two fixed helper-free loops. Form A
is `LEA RAX,[RBX+RCX]; XCHG RAX,RBX; JNZ entry` (3 guest instructions per
iteration). Form B adds a second `LEA; XCHG` pair (5 instructions per
iteration). Each form is measured at 1, 64, and 4,096 iterations. The budgets
are respectively 3/192/12,288 and 5/320/20,480 instructions. These are
synthetic register recurrences, not guest software or production workloads.

The native timer encloses each complete
`run_cached_native_register_v1_controlled` call, including validation,
capacity-one cache lookup, published native execution, and exit. State reset,
code-view construction, full-state validation, and serialization are outside
the call timer. The interpreter times the full `run` call on the same bytes,
state, and budget. Preflight checks zero work, exact full budget, a budget
boundary one instruction before loop completion, and `StopAddress` after the
first LEA; it compares full CPU state, flags, RIP, exit, retirement, and
remaining budget. It also checks the independent wrapping recurrence, one
publication, cache counters, and emitted/logical resident bytes. Each scored
native invocation must be hit-only with no miss, compile, publication, or
eviction. The report records the first full cache-miss dispatch separately;
that duration includes compile, publication, and execution, and is not a
compile-only measurement or break-even input.

Run the release preflight, then three separate release processes:

```sh
cargo run --release --offline -p od-bench -- cached-native-loop \
  --preflight-only --output work/tmp/stage3/s3-36f/preflight.json
for run in 1 2 3; do
  cargo run --release --offline -p od-bench -- cached-native-loop \
    --process-run "$run" --output "work/tmp/stage3/s3-36f/run-$run.json"
done
```

Each process retains 20 alternating pairs per stratum, with a 200 ms minimum
for every side and calibrated common call counts. Across the three M3 Max
release runs all six strata passed the floor and accounting checks. The
60-pair median native/interpreter ratios were 4.7661 (`a_1`), 4.8890
(`a_64`), 4.8938 (`a_4096`), 3.7205 (`b_1`), 3.8092 (`b_64`), and 3.7805
(`b_4096`); every paired ratio was above parity. Interpreter was faster on
these inputs, so this profile supplies no useful native target scenario and
does not support a cache or runtime default change. It does not close S3-36 or
general Stage 3 performance acceptance. Runtime `working_tree_dirty` is
unknown because the two-second Git status probe timed out, although the build
source tree was clean at the recorded commit. Mini checks remain deferred to
the end of Stage 3. See the [S3-36f report](../reports/STAGE_3_S3-36f_2026-11-01.md)
and [suite manifest](../reports/data/stage3/s3-36f-20261101/suite-manifest.json).

### S3-31b: cached NativeRegisterV1 phase attribution on S3-36f loops

`od-bench cached-native-loop-phases` runs the same two fixed kernels and six
strata as S3-36f: `a`/`b` at 1, 64, and 4,096 loop iterations. It compares the
ordinary cached dispatcher with an explicit observed entry using the same
capacity-one cache and published block. It records separate ordinary and
observed full-call elapsed time, raw paired ratio and signed full-path delta,
phase totals/counts for observed calls, signed residual, and cache counters.
The delta is observed-minus-ordinary for the complete measured paths; it
includes observer instrumentation and timing/path noise and is not an isolated
observer cost or a correction.

Three non-overlapping phases are recorded: `BlockBuildAndAdmission` wraps
only the block builder; `KeySnapshotAndLookup` wraps minimum cache sizing and
capacity, key construction, first snapshot validation, first cache lookup and
counter update; `HostExecutionBoundary` wraps only
`backend.execute_cached_block`, including its host wrapper/result decoding.
The repeated snapshot validation and `matches_code_view`, run guards,
stop/event/budget checks, fallback, retirement aggregation, post-call
validation, and observer hook overhead remain outside the measured phases and
are represented in the residual. The key phase is not total lookup/validation
cost and the execution boundary is not pure generated-code time. Warm
`IrLowering`, `Allocation`, `EmissionAndFinalization`, and `Publication` phase
counts are zero; cache counters separately require one initial compile and
publication only.

Preflight requires full interpreter/ordinary/observed state and accounting
parity, one resident entry, hit-only warm calls, and phase counts equal to
visits (`calls × loop iterations`). It checks zero work, entry STOP, event,
and pre-admission code mismatch; errors inside measured phases are untested.
Run the release preflight followed by three independent processes:

```sh
cargo run --release --offline -p od-bench -- cached-native-loop-phases \
  --preflight-only --output work/tmp/stage3/s3-31b/preflight.json
for run in 1 2 3; do
  cargo run --release --offline -p od-bench -- cached-native-loop-phases \
    --process-run "$run" --output "work/tmp/stage3/s3-31b/run-$run.json"
done
```

Each process keeps 20 alternating pairs per stratum, calibrates equal calls
within the pair, and requires every side to exceed 200 ms. All 720 sides
passed. The six 60-pair median observed/ordinary ratios ranged from 1.0900 to
1.1182; the `b_64` range crossed parity. Median observed-path shares were
21–22% build, 43–44% key/snapshot/first lookup, and 4–5% execution boundary
for form `a`, and 32–33%, 39%, and 5–6% for form `b`. These are descriptive
shares for this synthetic workload, not exact production runtime shares.
They do not establish a linking benefit or justify a cache/runtime policy
change. Runtime `working_tree_dirty` is unknown because the two-second Git
status probe timed out, although build source tree was clean at the recorded
revision. Mini checks remain deferred. See the
[S3-31b report](../reports/STAGE_3_S3-31b_2026-11-01.md) and
[suite manifest](../reports/data/stage3/s3-31b-20261101/suite-manifest.json).

### S3-31c: run-local prepared-key reuse candidate

`od-bench cached-native-loop-reuse` compares the ordinary cached NativeRegisterV1
dispatcher, an opt-in prepared-key reuse candidate, and the synchronous
interpreter on the two fixed S3-36f recurrence kernels at 1, 64, and 4,096
iterations. The candidate prepares one key for each invocation and records
subsequent reuse within that invocation. Each preflight stratum checks full
guest-state, retired-instruction, and budget parity; the six partial/control
cases cover continuation after LEA/XCHG, before JNZ, STOP before XCHG, STOP at
JNZ, and wrapping operand reuse.

Run the release preflight, followed by three independent timing processes:

```sh
cargo run --release --offline -p od-bench -- cached-native-loop-reuse \
  --preflight-only --output work/tmp/stage3/s3-31c/preflight.json
for run in 1 2 3; do
  cargo run --release --offline -p od-bench -- cached-native-loop-reuse \
    --process-run "$run" --output "work/tmp/stage3/s3-31c/process-$run.json"
done
```

Each timing process retains 20 paired triples per stratum (baseline, candidate,
interpreter), with order rotated across the six permutations and a 200 ms floor
for every measured side. State reset and CodeView/control setup are outside the
timed interval; candidate memo setup/use/drop are inside its dispatcher call.
The completed profile retained 60 triples per stratum and all 1,080 sides
passed the floor (minimum 223,746,544 ns). Ratios are candidate elapsed time
divided by baseline or interpreter elapsed time; below 1 is lower elapsed time.
Results apply only to these synthetic recurrences. At one iteration the
candidate/baseline medians are above 1; at 64 and 4,096 iterations they are
below 1 in both forms. Candidate/interpreter medians are above 1 for all `a`
strata and `b_1`, below 1 only for `b_64` and `b_4096`. No broad workload
speedup or default/policy change follows; the candidate remains opt-in and
S3-31 remains open. The untimed correctness preflight and timed process
artifacts share clean source revision and fingerprints
`dcd1bfe5db813a2647c100c4c64a008dca9bd21c`; the preflight ran from a temporary
detached worktree, while timing ran from the main working directory. See the [S3-31c report](../reports/STAGE_3_S3-31c_2026-11-01.md) and
[suite manifest](../reports/data/stage3/s3-31c-20261101/suite-manifest.json).

### S3-31d. Exact dependency snapshot comparison

Three local A/B release rounds used `cached-native-loop` with its six fixed
S3-36f profiles. The standard cached dispatcher now accepts an exact match
against the key's validated dependency snapshot without allocating or sorting.
The profile medians improved by 9.3–16.3% after normalization against the
interpreter control. The [measurement report](../reports/STAGE_3_S3-31d_2026-11-19.md)
records raw ranges, source provenance, reproduction commands and the archive.

### S3-31e. Prepared-key reuse after S3-31d

The S3-31c procedure above was repeated on clean revision `b142e96` after
the exact dependency snapshot comparison landed. The validator accepted
60 triples per profile. All retained samples at 64 and 4,096 iterations
favored prepared-key reuse against ordinary JIT and the interpreter.
At one iteration the prepared variant's median exceeded ordinary JIT in
both profiles. Prepared-key reuse remains opt-in. See the
[refresh report and evidence archive](../reports/STAGE_3_S3-31e_2026-11-19.md).

### S3-31f. Prepared-key ownership on one-pass dispatch

Two matched release builds used the S3-31c timing path with scoring limited
to `a_1` and `b_1`. Both builds ran the full six-profile correctness preflight.
The revised opt-in path improved both one-pass medians across rounds after
normalizing against the ordinary cached dispatcher. The
[report and evidence archive](../reports/STAGE_3_S3-31f_2026-11-19.md)
contain raw ranges, the benchmark patch, reproduction steps and build hashes.

### S3-35e. Cold retained-session first execution

Both signed builds use a fresh `Session` for every scored JIT call on the
same six countdown profiles as S3-36g. The contiguous timer spans session
creation and first execution. The phase split records initialization and
the first RUN with the two cold-cache probes. The driver checks empty
counters, exact publications, state and retired work on each call. Process
launch has its own S3-35d measurement. The archive contains the fixed-call
schedule, signed build provenance, raw times and the offline audit.
[Report and reproduction](../reports/STAGE_3_S3-35e_2026-11-19.md)

### S3-35f. Cold first RUN after the cache-index change

The same cold driver and six profiles compare `06bb39a` with `2b0f598`.
Four process rounds balance execution order and retain 32 paired cold
samples per build and profile. Each pair checks the published work,
architectural state and retirement. The archive contains the build
scripts, fixed schedule, raw data and offline audit.
[Report and reproduction](../reports/STAGE_3_S3-35f_2026-11-19.md)

### S3-35g / S3-36i. Cold and warm mixed register dispatch

One signed release build compares the interpreter with the S3-55 mixed
cache route on two straight-line streams and two loops with CLD/STD
steps. Cold includes admission and fresh cache/arena construction.
Warm times a retained FIFO with hits and zero publication. Four rounds
share fixed counts across the two modes and check guest state and
native/interpreter retirement for every call.
[Report and reproduction](../reports/STAGE_3_S3-35g_S3-36i_2026-11-19.md)

### S3-36j. Single NativeRegisterV1 cache search on a hit

Signed matched release builds compare the old dispatcher with the
post-touch index path on the four mixed FIFO eager profiles. The cold
and warm modes each use four matched rounds, eight pairs per profile
per process and a 30 ms timing floor. The archive preserves fixed
work and cache footprints, both raw and interpreter-normalized
comparisons, and separate LRU correctness smoke results.
[Report and reproduction](../reports/STAGE_3_S3-36j_2026-11-19.md)

### S3-35h / S3-36k. Matched checked-memory cache calls

The experiment times the same complete capacity-one cache call
on an empty cache and on a resident entry. Every cold call compiles
and publishes once. Warm samples record hits only. The interpreter
runs the same frozen S3-34a inputs. Each signed revision contributes
18 matched triples per sequence and verifies full state, RAM,
ordered callbacks and exact cache counters.
[Report and reproduction](../reports/STAGE_3_S3-35h_S3-36k_2026-11-19.md)

### S3-40m. Interpreter timing controls for the S3-34a corpus

Matched release builds run the same complete interpreter call,
two `step` calls and two decoder calls over the same 2,400 inputs.
The current build adds an empty-profile probe. Four rounds share
fixed counts, alternate revision order and rotate timing sides.
The artifact preserves profile construction, architectural parity
checks and the raw intervals.
[Report and reproduction](../reports/STAGE_3_S3-40m_2026-11-19.md)

### S3-40n. MOV decoder controls for S3-34a

The signed release driver times the ordinary decoder, two clones
that retain the original MOV arm, and the corresponding execution
controls. Every decoder trial compares the complete decoded
instructions on the two fixed byte streams. Execution trials check
state, RAM, ordered callbacks and retirement. The archive preserves
three processes with six rotations per sequence.
[Report and reproduction](../reports/STAGE_3_S3-40n_2026-11-19.md)

### S3-40o. Single-REX.W primary MOV

The signed release comparison tests four decoder builds for exact decoded
results and ordered fetch requests, including injected failures. A second
driver times the selected and original decoder in the same process.
It keeps per-side call counts fixed and alternates measurement order.
Four processes give a 0.211 median candidate/reference ratio on target
MOV and 1.009–1.016 on register, neighboring MOV and `0F` controls.
[Report and reproduction](../reports/STAGE_3_S3-40o_2026-11-19.md)

### S3-40p. Full interpreter calls with the updated MOV decoder

One signed build compares ordinary `execute::run` and the equivalent
path using the previous decoder on the same 2,400 frozen inputs.
Full-call median paired ratios are 0.663 and 0.666 for the two S3-34a
sequences. The audit retains all 128 intervals and checks their
schedule, call counts and corpus identity.
[Report and reproduction](../reports/STAGE_3_S3-40p_2026-11-19.md)

### S3-40q. Cold and warm checked-memory JIT after the MOV decoder update

Two signed builds run the same frozen corpus and fixed sweep counts
in four alternating process pairs. Each process measures all six
cold/warm/interpreter orders per sequence. The current warm ratios
are 0.901 and 0.908 versus the current interpreter, and the cold
ratios are 21.738 and 21.772. Cross-revision interpreter ratios
use medians from separate processes within each round.
[Report and reproduction](../reports/STAGE_3_S3-40q_2026-11-19.md)

### S3-40r. Full interpreter calls with statically empty execution profiles

The signed release diagnostic couples ordinary `execute::run` and a clone
with statically empty execution profiles in one binary. Both paths
use the post-S3-40o decoder. Full-call median paired ratios on the two
S3-34a streams are 0.629 and 0.628 across four processes.
[Report and reproduction](../reports/STAGE_3_S3-40r_2026-11-19.md)

### S3-40s. Empty-profile configuration prototypes

Separate signed diagnostics pair each prototype with the ordinary
call route. The derived-default ratios are 1.063 and 1.060.
The associated-constant ratios are 0.700 and 0.699.
Each of the two archives preserves 128 intervals and the measured
step prologues.
[Report and reproduction](../reports/STAGE_3_S3-40s_2026-11-19.md)

### S3-40t. Empty-profile route and machine-code size

Two standalone signed builds run the checked-memory, register,
neighboring and configured XORPD controls. Four alternating process
pairs share sweeps for each group. The archive retains digests of the
complete outcomes and ordered effects, raw timings and section sizes
for the benchmark and signed VMHost. The configured XORPD time range
crosses parity.
[Report and reproduction](../reports/STAGE_3_S3-40t_2026-11-19.md)

### S3-40u. Cold and warm checked-memory JIT after empty-profile specialization

The S3-40q driver replays the same corpus with S3-40t applied.
All 288 intervals and four paired process rounds are preserved.
Current warm native/interpreter ratios are 1.125 and 1.128.
Cold ratios are 28.160 and 28.019. Forty-seven current triples
have no modeled crossover.
[Report and reproduction](../reports/STAGE_3_S3-40u_2026-11-19.md)

### S3-36g. Warm retained Session on one- and two-RIP loops

The signed release comparison times complete `Session::run` and interpreter
calls on the same S0-09 countdown loops. It checks full state and retirement
outside timing and verifies hit-only cache counters across each warm sample.
The archive includes the fixed counts, matched builds, alternating order,
individual timings and offline audit. [Report and reproduction](../reports/STAGE_3_S3-36g_2026-11-19.md)

### S3-36h. Warm hit with the post-touch cache index

Two matched signed builds use the S3-36g driver with the same six warm
profiles. The candidate reuses the post-touch cache index and records a
continuation index only for same-RIP returns. All six profiles use fixed
counts and alternating call order. [Report and raw audit](../reports/STAGE_3_S3-36h_2026-11-19.md)

### S3-31g. Cached dispatch across RIP transitions

Two signed release comparisons against the same baseline use the six
S3-35f and S3-36h countdown inputs and a three-RIP control. The warm timer
covers RUN with resident entries. The cold timer includes a fresh
session, empty-cache probes and first RUN. Each comparison uses four
matched rounds per mode, eight JIT/interpreter pairs per profile per
process and calibrated fixed counts. The selected change trades a
small slowdown in long one-RIP controls for a repeatable reduction in
dispatch time on the two-RIP profiles.
[Report and raw audits](../reports/STAGE_3_S3-31g_2026-11-19.md)

### S3-37a: bounded production FIFO16 corpus churn observation

`od-bench cache-churn` is an opt-in Apple Silicon macOS release command. It
accepts only the frozen S3-17 case-list and trace, admits all 1,010 cases, and
runs 128 complete corpus sweeps on one retained production FIFO16 runtime,
cache, and arena. Each sweep records timed dispatch-call duration, input calls,
native block executions, retirements, counter deltas, resident high-water,
arena reservation, and a derived unoccupied-slot value (capacity minus
resident-entry high-water, not an allocator counter). The report also records cumulative hit, miss,
compile, publication, and eviction totals. The existing 30-minute process
limit applies; runs fail unless the full work count and bounded FIFO
eviction/recompile invariants are observed.

The timing excludes fixture admission, per-case state cloning/reset and
correctness validation, runtime initialization/teardown, and process launch.
S3-37a's recorded runs did not measure process RSS or
teardown/independent-instance lifecycle. This
fixed corpus repetition is bounded capacity-churn evidence only; it is not a
long leak-free stress result and does not cover SMC invalidation or broader
workloads. Three release runs each completed all 128 sweeps with identical
totals: 168,960 misses/compiles/publications, 168,944 evictions, and 256 hits.
Resident high-water was 16 entries, 21,640 code bytes, and 50,124 logical
metadata bytes. This is bounded capacity-churn evidence, not a long leak-free
stress result. See the
[S3-37a report](../reports/STAGE_3_S3-37a_2026-10-30.md).

### S3-37b: process-wide RSS snapshots

The `cache-churn` report schema 2 records process RSS before FIFO runtime
creation (after corpus preparation), after the fixed sweeps and before runtime
drop, and after an explicit drop of the runtime/cache/arena. On macOS the
current process is sampled with `/bin/ps -o rss= -p <pid>`; the reported value
is the parsed KiB count converted to bytes. Sampling runs outside the timed
dispatch intervals. On other targets, or when sampling fails, RSS is `null`
and the report includes a reason. Each value is process-wide: fixtures, the
runner, and all other live allocations contribute, so these samples do not
attribute bytes to the cache or establish leak-free lifecycle behavior. Three
release processes completed the same 128 sweeps with all snapshots available:
pre-runtime RSS was 5,024–5,056 KiB, pre-drop was 5,808–6,000 KiB, and
post-drop was 5,744–5,936 KiB. Each post-drop sample was 64 KiB below its
pre-drop sample and remained 688–912 KiB above that process's pre-runtime
baseline. This is process-wide RSS observation only; it does not prove memory
was released or retained by the cache itself. See the
[S3-37b report](../reports/STAGE_3_S3-37b_2026-10-30.md).

### S3-37c: deterministic FIFO16 invalidation and runtime lifecycle

`od-bench cache-lifecycle` is an opt-in native Apple Silicon release command
over 17 one-entry fixtures from the frozen S3-17 corpus. It fills the existing
production FIFO16 cache, issues a controlled stop-RIP at the target's current
guest boundary and requires `StopAddress` (the stop-RIP exit, distinct from
async `StopRequested`), repeats the newest key, inserts one additional key to
observe FIFO replacement, invalidates the selected target backing between synchronous calls,
dispatches changed admitted code under the advanced backing version, repeats it
for a hit, advances the supported execution-context generation, explicitly
clears, drops the runtime, and creates a fresh runtime with a later context
generation.

Each dispatch validates full CPU state (with the fixture's undefined-RFLAGS
mask), exit, block count, retirement count, and remaining budget against the
interpreter oracle. The report records current resident entries/code/metadata
from the cache's current counters, allocator `available_slots()` snapshots,
and high-water counters as separate fields. It includes fixture and corpus
hashes, identities and versions, exact action order, cache counters, state and
diagnostic elapsed times for invalidation/recompile/clear/drop. Those timings
are not performance claims.

Invalidation is explicitly between calls and uses the supported cache API. It
does not demonstrate production self-modifying-code ownership, guest faults,
automatic clearing at STOP, concurrent arenas, or leak-free behavior. The STOP
step checks that no lookup, dispatch, or cache clear occurred; explicit clear
is a separate step. Three clean release runs and their raw evidence are
recorded in the [S3-37c report](../reports/STAGE_3_S3-37c_2026-10-31.md).

### S3-37d: sustained deterministic mixed FIFO16 lifecycle stress

`od-bench cache-lifecycle-stress-calibrate` chooses a fixed epoch count from
the same sequence used by the scored command. Profile v5 runs 512 warm-up
epochs followed by eight 65,536-epoch measurement batches on one retained
runtime/cache/arena. It measures each batch's epoch-body time separately and
uses the fastest observed batch rate when sizing for the minimum 11-minute
target. Warm-up time is not treated as scored loop time. It records the same
128 bounded RSS/report/atomic-write checkpoints across calibration; with 524,800
epochs these are spaced about every 4,100 epochs, instead of every four epochs
in v4. Checkpoint/report-write and fixed process costs are measured separately
from batch body time. Calibration caps scored runs at 50 million epochs,
rejects a cap that cannot reach the target, and reserves one minute below the
30-minute process deadline. The exact recommended epoch count and process run
1..=3 are required. Calibration binds the frozen corpus, source fingerprint,
build/host metadata, warm-up/batch/checkpoint schedule, timing arithmetic, and
payload integrity digest. Both calibration and scored runs require the frozen
S3-17 case list and trace.

Each epoch retains one FIFO16 runtime/cache/arena and performs a hit on the
active entry, a unique pressure-key miss with exactly one FIFO eviction, a
direct between-call invalidation of the active backing with exactly one entry
removed, a compile/publication of alternate admitted bytes under the advanced
backing version, then a repeat hit. Original and alternate bytes have distinct
interpreter results and alternate by epoch. Every dispatched case is checked
against its interpreter oracle for full GPR state, RIP, masked flags, exit,
retirement and remaining budget. Every 128 epochs, the command clears and
requires zero current resident entries/code/metadata plus all 16 arena slots,
then refills. Every 512 epochs, it clears, drops the sole runtime and arena,
reopens one runtime, executes after reopen with a bumped execution context, and
refills. It never holds more than one MAP_JIT arena at once.

Reports keep current occupancy separate from high-water accounting and include
cumulative cache/dispatch counters, separate elapsed totals for pressure miss,
version recompile dispatch, hit dispatch, invalidation, clear, teardown and
reopen. Progress is written atomically as a bounded array of at most 128
checkpoints; each checkpoint includes a process-wide RSS snapshot at the same
end-of-epoch lifecycle point. Completed reports include actual elapsed time
and whether the minimum duration was met. A run below 660 seconds is persisted
with `status: under-target` and exits with an error; it cannot count as
sustained-duration evidence. Aggregate counters reconcile against exact
schedule totals, and counter regression is an error. RSS is auxiliary and is
not evidence of a leak-free result. Retain calibration, scored artifacts and
raw JSON. Run three scored release processes sequentially during a Stage2-quiet
window; an overlap with Stage 2 makes that attempt inconclusive.

This profile exercises supported cache APIs only. It does not demonstrate
production owner notification, guest-write SMC, active-code modification,
fault cleanup, concurrent arenas, SMP, or leak-free behavior. Mini checks
remain deferred to the end of Stage 3. Three sequential v5 scored processes
completed with exact totals and each met the 660-second target: run1
45,436,794 epochs / 671.161073750 seconds; run2 46,049,027 epochs /
679.267265042 seconds; run3 46,049,027 epochs / 688.093795250 seconds. Source,
calibration and each scored artifact passed independent review. The v4 run1
completed 29,682,315 epochs with exact counters in 439.482 seconds, below
target; it remains a separate lifecycle/counter diagnostic and is not combined
with v5. Raw JSON and limitations are recorded in the
[S3-37d report](../reports/STAGE_3_S3-37d_2026-10-31.md) and its
[raw data](../reports/data/stage3/s3-37d/). Full S3-37 remains open.

Recorded trace дополняется ограниченными synthetic-сценариями:
1/8/64/512/4096 keys и observed unique count, hot loops на 1/8 keys, reverse
unique sequence, alternating hit/miss, stale mapping/backing/execute
permission, aliases с общим backing и 1/2/4 dependency spans. Synthetic-
сценарии помечаются в отчёте и не выдаются за workload trace. Размер
логической памяти является нижней оценкой; отчёт исключает метаданные
аллокатора и RSS процесса, которые prototype не измеряет.

Ограничения: не более 1 010 inputs; 4 KiB code и 64 KiB fixture JSON на input;
4,096 visits per input; 65,536 visits total; 4,096 keys; 128 MiB logical
payload; 24 scenarios; 10 minutes trace capture; 30 minutes measure run. Limit
нарушение завершает операцию ошибкой. Тесты покрывают malformed trace,
admission сценариев, parity вариантов и stale hit. Для lookup-structure
prototype default не выбран и ускорение не заявляется. Recorded corpus runs
для dispatch/cache, cold first execution, warm resident execution и FIFO16
churn описаны отдельно в отчётах S3-18d1, S3-35a, S3-36a и S3-37a.

### S3-37g. Backing invalidation and first redispatch

The signed FIFO16 driver measures separate intervals for ready dispatch,
a current-version notification, deletion of the oldest two entries
sharing a backing, and first redispatch at both addresses after a version
advance. It checks full CPU state against the interpreter and records
the exact code, metadata and slot counts at 2 and 16 resident entries.
The archive contains source, calibration, scored samples and an offline
audit. [Report and reproduction](../reports/STAGE_3_S3-37g_2026-11-19.md)

### S3-37h. Native write to the active V3 tail

The signed release driver uses the existing tracked owner in a
test-only source snapshot. It measures active compilation and
publication, the actual store with its invalidation callback, and
first and warm dispatch of the updated tail at both addresses.
Each epoch verifies the retired prefix and store, rejection of the
stale V3 handle, native tail parity with the interpreter and slot
reuse at bounded capacity.
[Report and reproduction](../reports/STAGE_3_S3-37h_2026-11-19.md)

### S3-37i. Sustained S0-09 Session pressure and process memory

The signed release driver runs two concurrent processes, each with
one live retained Session, 20 immutable guest blocks and FIFO16.
Each process executes 64 checkpoints over at least 660 seconds.
Sweeps verify interpreter-equivalent state and exact publication,
hit and eviction counts. Checkpoints verify STOP, entry refusal,
clear, a fresh lease, refill and destruction. The Mach self-task
RSS snapshots are per process. Logical code and metadata occupancy
come from the Session counters. The archive reproduces both the
build and the offline audit.
[Report and reproduction](../reports/STAGE_3_S3-37i_2026-11-19.md)

## S4-13a: uncached page walk versus experimental one-entry probe cache

`od-bench page-walk-compare` compares the existing pure `probe_long64_4k`
baseline against the opt-in `od-machine` feature
`experimental-translation-probe-cache`. It does not measure tracked A/D writes,
operand accesses, guest invalidation, CPU/JIT integration, or Linux-derived
traces. The one-entry candidate borrows one immutable `PhysicalMemory`; it
caches only successful 4 KiB translations and reports `Hit` separately from a
fresh `Walk`. A miss calls the pure probe. This experiment does not select a
production TLB or invalidation policy.

The synthetic fixture is fixed at 32 MiB RAM, physical width 36, CR0
`0x80010011`, CR3 `0x1000`, CR4 `0x20`, EFER `0xd00`, Long64 canonical48,
four-level paging, 4 KiB pages, and explicit Ring3 reads. A versioned SHA-256
covers the complete RAM image and ordered request profile. Profiles contain
1, 2, 16, 256, or 4,096 pages and offsets 0, 8, 64, and 4,088. The 2-page
profile alternates pages for each offset; the other profiles visit all four
offsets before moving to the next page. Before timing, every request must match
its literal expected physical address and the cache result must equal the
baseline. RAM hashes before and after preflight/timing must match. Expected
address/parity comparisons run only in preflight; timed paths share the same
request loop, per-request checksum barrier, and per-episode checksum barrier.

Each profile has two modes: clear the one-entry cache at the start of each
episode, and prewarm one complete episode before retaining the cache across
timed episodes. Calibration doubles a common episode count until both variants
exceed 200 ms. Each mode then records 20 paired samples with alternating
baseline/candidate order; if any sample falls below 200 ms, the whole series is
discarded and retained in the JSON, the batch count is increased, and the full
series restarts. Three release processes run sequentially on the same host.
Reports retain raw pairs, calibration attempts, discarded short series, hit and
walk counts, ns/request, requests/second, candidate/baseline ratios and their
median/min/max/IQR, fixture/request/cache storage sizes, hashes, and host/build
metadata. No process RSS or cold-host-cache claim is made.

Run from the repository root, writing a separate artifact for each process:

```sh
cargo run --release -p od-bench --offline -- page-walk-compare \
  --process-run 1 --output work/tmp/stage4/s4-13a/run-1.json
cargo run --release -p od-bench --offline -- page-walk-compare \
  --process-run 2 --output work/tmp/stage4/s4-13a/run-2.json
cargo run --release -p od-bench --offline -- page-walk-compare \
  --process-run 3 --output work/tmp/stage4/s4-13a/run-3.json
```

These synthetic probes establish only the cost of this diagnostic pure-walk
API on the measured host and Rust release build. They do not establish a
representative guest workload or determine whether a production TLB is useful.
