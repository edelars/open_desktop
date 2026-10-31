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

After independent source review, collect three release processes sequentially
during a Stage2-quiet window:

```sh
for run in 1 2 3; do
  cargo run --release -p od-bench --offline -- cached-native-flags-compare \
    --corpus work/stage3-native-register-v1/manual/native-register-v1-observations.json \
    --process-run "$run" \
    --output "work/tmp/stage3/s3-40d/cached-native-flags-$run.json"
done
```

No S3-40d performance result is recorded yet. The harness implementation and
focused non-performance checks do not constitute a scored run.

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

### S3-35b: cold dispatcher pipeline attribution

`od-bench cold-phases` uses the same frozen S3-17 case list and trace on the
real FIFO16 dispatch/cache/compiler path. It reports distinct intervals for
block build/decode and admission, key/snapshot/lookup, IR lowering, allocation,
emission plus byte encoding/finalization, publication, and host execution
boundary. A hit still includes block build and key/lookup. Guards, cache install
and accounting remain in `other_dispatch_ns`; they are not misreported as a
compiler phase. Stage intervals are non-overlapping and are components of the
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
