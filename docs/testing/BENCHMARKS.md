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
