# Протокол измерений и отчёта

Статус: протокол подготовлен для S0-06; S0-13/E04 baseline выполнен в границах текущего headless прототипа на основном M3 Max. Измерены CPU pilot по четырём fixtures, VMHost до/после completion signal, public C ABI execute, busy-helper STOP и peak-RSS для одного/двух helper processes; данные и ограничения записаны в [отчёте S0-13](../reports/STAGE_0_CPU_BASELINE_2026-10-22.md). Это не benchmark полной VM. Отложены контролируемые повторы, сравнение пакетного и мелкого ABI вызова (пакетного API пока нет), длительная нагрузка и JIT code-mapping size.

Для ограниченного CPU-среза [`od-runner --benchmark`](../../tools/runner/README.md#reproducible-cpu-benchmark) собирает decode, translation/lowering, executable publication, повторный interpreter и повторный JIT с чередованием порядка и 20 samples по умолчанию. Каждая серия калибруется отдельно до batch не короче 200 ms; raw samples включают объём работы и elapsed time. Это не покрывает cold process/VMHost launch, ABI/IPC и resource measurements.

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

Старые значения S0-13 характеризуют direct fragment path и host wrapper на
конкретной машине. Они остаются историческим baseline и не переносятся на
будущий block JIT. Перед выбором lowering/allocator сохранить текущие fixture
и corpus fingerprints. Текущий runner сравнивает interpreter и legacy fragment
JIT; новый block JIT включается после S3-13.

| Workload | Состав | Доступность сравнения |
|---|---|---|
| Linear integer | Повторяемые `MOV`/`ADD`/`SUB`/`CMP` из S0-09 без memory | Interpreter и legacy fragment path; новый block JIT после S3-13 |
| Conditional branch | Короткая программа с `JZ`/`JNZ`, одинаковыми taken/not-taken inputs | Одноблочные cases после S3-13; полный Stage 1 corpus после dispatcher S3-15 |
| Short/long loop | Одни и те же guest instructions с разным числом loop iterations | Legacy path — только для его собственного сравнения; block JIT требует S3-15 и точной interpreter semantics |
| Multi-block | Несколько связанных basic blocks и повторные переходы между ними | После dispatcher и cache lookup; не включать в ранний one-block result |
| Helper-heavy | Частые `ADD`/`SUB`/`CMP` и eager materialization flags через helper | Только при одинаковой x86 flag semantics и измерении фактических helper calls |
| Memory/fallback/invalidation | Checked accesses, interpreter exits, изменённый guest code | Добавлять вместе с соответствующей реализацией и correctness evidence; сейчас не измерять |

Для каждого workload сохранить exact guest bytes, initial CPU state, число
исполняемых guest instructions, instruction budget, exit/branch behavior и
ожидаемое состояние. До timing подтвердить корректность каждого сравниваемого
варианта на том же input. Физический x86 oracle — обязательная correctness
проверка там, где её требует план, но его время на отдельной Ubuntu-машине не
является прямым A/B performance comparison с ARM64 host.

Различать три измеряемых уровня:

1. **Cold process:** отдельный запуск и startup целевого runner; измерять
   внешним supervisor, поскольку `od-runner --benchmark` не включает старт
   процесса.
2. **Cold compilation/code cache:** программа при пустом in-process cache,
   включая decode/build/lowering, ARM64 emission, publication и первое
   исполнение. Текущий `od-runner --benchmark` отдельно измеряет translation и
   compile/publish/drop, но не исполняет код в этой cold-серии; полная
   cold-compilation-plus-first-execution метрика пока недоступна. Не получать
   её сложением медиан отдельных серий. Для legacy fragment path явно указывать
   отсутствие постоянного block cache.
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
This path covers only a bounded register-only resident slice. No measurement
has been collected; it supports no performance conclusion and does not complete
the branch/helper/memory or resource portions of S3-36. See the
[S3-36a implementation report](../reports/STAGE_3_S3-36a_2026-10-30.md).

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
admission сценариев, parity вариантов и stale hit. Реальный corpus trace и
timed run ещё не выполнялись; lookup structure не выбрана, ускорение не
заявляется.
