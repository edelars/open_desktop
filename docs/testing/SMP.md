# Методика проверок SMP и модели памяти

Статус: **S6-05, S6-08 и полный физический прогон S6-09 приняты после независимого ревью**. Реализация SMP в VM и результаты VM-прогонов отсутствуют. Этот документ задаёт входные данные, оракулы, бюджеты и правила фиксации результата; native runner не доказывает SMP в VM.

## 1. Источники и границы утверждений

- Для инструкций и архитектурных правил используем [Intel 64 and IA-32 Architectures Software Developer’s Manual](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html), редакция v093, Volume 3A, §8.2 «Memory Ordering» (страница Intel обновлена 2026-10-09; содержимое сборок документа одинаково). При добавлении теста фиксируем конкретный пример/параграф и байты инструкции; ссылка на весь раздел не заменяет точную ссылку.
- Операционная модель x86-TSO и множества поведения берутся из Sewell et al., «x86-TSO: A Rigorous and Usable Programmer’s Model for x86 Multiprocessors», CACM 2010: §1 (SB), §3 (операционная и аксиоматическая модели), §3.2 (примеры SDM), §3.3 (эмпирическая проверка). [Статья и материалы авторов](https://www.cl.cam.ac.uk/~pes20/weakmemory/index3.html), [PDF](https://www.cl.cam.ac.uk/~pes20/weakmemory/cacm.pdf).
- x86-TSO — выбранный проектом программистский оракул для write-back обычной RAM, а не заявление о полной формализации всех устройств, MMIO, DMA, non-temporal/WC accesses или всех вариантов split lock. Эти границы задаёт [план этапа 6](../plans/STAGE_6.md); будущие контракты `MEMORY_MODEL.md` и SMP уточнят их.
- Прогон на физическом x86 проверяет конкретную реализацию теста и способен обнаружить запрещённый исход. Ненаблюдение исхода за конечное число повторов **не доказывает**, что исход невозможен. Статья x86-TSO отдельно предупреждает об этом и приводит редкий исход AMD3: 4 наблюдения на 3.2 млрд запусков (§3.3). Эта частота не переносится на SB или другие тесты.

## 2. Уровни проверки и критерии

| Уровень | Pass | Отказ / недостаточное доказательство |
|---|---|---|
| Unit: ISA и машина | Проверены доступные операции, права и отказ до побочного эффекта; тесты покрывают обе стороны условий и границы ширин. | Только успешный путь или второй vCPU, который фактически не исполняет инструкции. |
| Множества модели | Собственный перебор x86-TSO и SC повторяем; классические исходы совпадают с опубликованным источником. Множества содержат происхождение каждого ожидаемого исхода. | Множество выведено из наблюдений на машине либо у исхода нет источника/свидетеля. |
| Litmus на физическом x86 | Ни один запрещённый исход не наблюдался; SB интересный исход `r0=0, r1=0` был наблюдён. | Запуск только на ARM64 host или эмуляторе; меньшее число повторов без явной записи; считать отсутствие исхода доказательством его невозможности. |
| Режим чередования | Полный перебор даёт в точности множество `SC при неделимости каждой инструкции`; большие тесты покрыты указанным диапазоном seed, а каждый seed воспроизводим. | Один seed, выборочная сверка или утверждение, что это физический x86-TSO: в этом режиме инструкция неделима. |
| Параллельный режим машины | Наблюдаемое множество — подмножество x86-TSO; запрещённых исходов нет; пройдена проверка чувствительности S6-36. | Тест не способен обнаружить намеренно внесённое нарушение порядка или не выполнен полный заданный бюджет. |
| Гостевой SMP-probe и Linux | Проверены собственный INIT/SIPI trampoline, IPI/shootdown, UART-маркеры с nonce, строки лога Linux, `/init` с nonce и счётчики IPI каждого CPU. | Harness сразу создаёт AP в long mode; лог пересказан; проверен только BSP. |
| Длительная нагрузка и масштабирование | Результат внутри гостя проверен, длительность и конфигурация записаны; 1/2/4 vCPU измерены в обоих режимах с повторениями и разбросом по `BENCHMARKS.md`. | Короткий прогон экстраполирован на длительный либо заявлено ускорение по одному числу. |

Полная матрица gates и зависимостей находится в [плане S6](../plans/STAGE_6.md#5-стенд-и-доказательства). При несовпадении между источником, собственным вычислителем и физическим x86 сначала перепроверяются fixture, кодировка и правила модели; результат сохраняется, тест не подгоняется.

## 3. Формат litmus-теста

S6-05 создаёт JSON-файлы по этому формату и отдельный собственный переборщик. Одна запись описывает один конечный машинный тест; поток — упорядоченный список инструкций. Адреса в fixture заданы явно и находятся в обычной общей write-back RAM.

Обязательные поля:

| Поле | Содержание |
|---|---|
| `schema`, `id` | Версия формата `od.smp-litmus.v1` и стабильное короткое имя. |
| `sources` | Название, автор/издатель, редакция/год, раздел или пример и URL. Для каждого ожидаемого исхода указывается источник либо `derived_by` с версией собственного переборщика. |
| `memory` | Размер/ширина каждой ячейки, адрес, начальное значение и класс памяти; aliasing задаётся явно. |
| `threads` | Номер CPU и инструкции по порядку. Для каждой: абстрактная операция, asm, байты, длина, операнды, упорядоченный список `memory_events` и метка границы инструкции. |
| `observe` | Конечные регистры и ячейки, участвующие в ключе исхода; все прочие значения исключены из сравнения явно. |
| `outcomes` | Три множества: `x86_tso`, `sc`, `interleaving_instruction_atomic`. В каждом исходе все наблюдаемые поля заданы явно. |
| `interesting` | Исходы из `x86_tso`, отсутствующие в `sc`; пустой массив, если таких нет. |

Абстрактная операция сохраняет границу **инструкции** и список её доступов к памяти. Например, `inc dword ptr [x]` — одна инструкция read-modify-write с отдельными read и write; без `LOCK` она не атомарна. `lock xadd` — одна LOCK-инструкция и один атомарный эффект. Это различие нужно, чтобы модель чередования не делала доступ атомарным по ошибке.

Минимальный пример SB (поля сокращены только для читаемости; полная fixture включает все обязательные значения):

```json
{
  "schema": "od.smp-litmus.v1",
  "id": "SB",
  "sources": [
    {
      "title": "x86-TSO: A Rigorous and Usable Programmer's Model for x86 Multiprocessors",
      "edition": "CACM 2010",
      "section": "§1, §3.3",
      "url": "https://www.cl.cam.ac.uk/~pes20/weakmemory/cacm.pdf"
    }
  ],
  "memory": {
    "width_bits": 32,
    "initial": {"x": 0, "y": 0},
    "addresses": {"x": "0x1000", "y": "0x1004"},
    "kind": "write_back_ram"
  },
  "threads": [
    {"cpu": 0, "instructions": [
      {"op": "store", "asm": "mov dword ptr [0x1000], 1", "bytes": "c7 04 25 00 10 00 00 01 00 00 00", "writes": {"x": 1}, "memory_events": [{"kind": "write", "location": "x", "value": 1}]},
      {"op": "load", "asm": "mov eax, dword ptr [0x1004]", "bytes": "8b 04 25 04 10 00 00", "reads": {"y": "eax"}, "memory_events": [{"kind": "read", "location": "y", "result": "eax"}]}
    ]},
    {"cpu": 1, "instructions": [
      {"op": "store", "asm": "mov dword ptr [0x1004], 1", "bytes": "c7 04 25 04 10 00 00 01 00 00 00", "writes": {"y": 1}, "memory_events": [{"kind": "write", "location": "y", "value": 1}]},
      {"op": "load", "asm": "mov ebx, dword ptr [0x1000]", "bytes": "8b 1c 25 00 10 00 00", "reads": {"x": "ebx"}, "memory_events": [{"kind": "read", "location": "x", "result": "ebx"}]}
    ]}
  ],
  "observe": ["cpu0.eax", "cpu1.ebx"],
  "outcomes": {
    "x86_tso": [[0, 0], [0, 1], [1, 0], [1, 1]],
    "sc": [[0, 1], [1, 0], [1, 1]],
    "interleaving_instruction_atomic": [[0, 1], [1, 0], [1, 1]]
  },
  "interesting": [[0, 0]]
}
```

The byte sequences are part of the fixture contract, not a request to an assembler at runtime. S6-05 verifies that each byte string decodes to the recorded instruction and operands; code, data, and per-thread register state must not overlap. The full SB set has this provenance: the paper's §1 source explicitly permits `(0,0)` under x86-TSO; `(0,1)`, `(1,0)`, and `(1,1)` each have an SC witness and therefore are also TSO-allowed. Their witnesses, written in global instruction order, are:

- `(0,1)`: `T0: W(x); T0: R(y); T1: W(y); T1: R(x)`.
- `(1,0)`: `T1: W(y); T1: R(x); T0: W(x); T0: R(y)`.
- `(1,1)`: `T0: W(x); T1: W(y); T0: R(y); T1: R(x)`.

Each load can only observe the initial `0` or the other thread's sole write of `1`, so these four outcomes exhaust the value domain. Both-zero is impossible under any SC interleaving: per-thread order plus the two zero reads would require the cycle `W0(x) < R0(y) < W1(y) < R1(x) < W0(x)`. The independent S6-05 enumerator must confirm these analytical sets before the corpus is accepted.

For a memory RMW instruction, store accesses in `memory_events` separately and in instruction order. An ordinary `inc [x]` has a read event followed by a write event; a `LOCK`-prefixed RMW or implicitly locked memory `XCHG` is an indivisible atomic event. A load or store has one event. Register-only work does not create a memory event, but its data dependency remains attached to the corresponding read/write in the fixture.

## 4. Computing the expected sets

For each fixture, record the output of two independent derivations:

1. **Source-derived:** transcribe the allowed/forbidden final states and their polarity from the cited source. Do not infer a complete set from a source that states only one example outcome; mark the set incomplete until the own-model derivation is checked.
2. **Own-model enumeration:** enumerate the finite state space for the x86-TSO operational model with per-CPU FIFO store buffers, forwarding from the local buffer, and explicit flush transitions. Separately enumerate SC as a total order of memory events that preserves each thread's program order; an ordinary RMW contributes read then write events, so another thread may interleave between them. An atomic `LOCK` RMW contributes one indivisible event. Record tool revision, input hash, state count, and canonical sorted outcome set.

For fixtures where the source publishes complete outcomes, the two sets must be equal. A mismatch blocks the corpus: investigate the model, fixture bytes, or transcription. The checker may deduplicate equivalent executions only after preserving their distinct final outcomes; it must not prune a state based on the expected answer.

`interleaving_instruction_atomic` schedules whole guest instructions as indivisible steps. It intentionally differs from both memory-event SC and hardware for unprefixed memory RMW instructions: `inc [x]` cannot be interrupted between its read and write in this harness. Record this set separately even when it happens to equal another oracle, as in SB.

## 5.1. Sensitivity runs required by S6-36

Sensitivity is a two-part check, and each part uses a separate test-only feature. Neither feature may be enabled in a normal product build.

1. **Host memory-order path:** run host threads in tight loops without the interpreter, calling the guest RAM access functions from S6-30/S6-32 on these patterns:
   - MP: P0 writes `x=1` then `y=1`; P1 reads `y` then `x`. The forbidden outcome is `r(y)=1, r(x)=0`.
   - 2+2W: P0 writes `x=1` then `y=1`; P1 writes `y=2` then `x=2`. The forbidden final state is `(x=1,y=2)`; `(x=2,y=1)` is allowed, for example in order `P0:x=1; P1:y=2; P1:x=2; P0:y=1`, and must not be treated as a failure.

   With the normal barriers enabled, neither forbidden outcome may appear. In a separately built test/benchmark profile with those barriers disabled, the forbidden outcome is expected to appear. Record the run count and histogram. If the weakened profile does not produce a forbidden outcome on this ARM64 host, retain that negative result; S6-36 allows G5 to rely on the second sensitivity part, a clean normal run, and the source proof from S6-04.
2. **Guest atomic path:** run a guest counter using `LOCK INC` in a test-only profile that deliberately implements `LOCK` non-atomically. The final count must be below the exact number of increments, demonstrating lost updates. Keep this feature distinct from the host-barrier feature above.

The check fails if either feature leaks into the normal profile, if the normal host path observes a forbidden outcome, if the guest weakened path still produces the exact count, or if outputs/binary hashes are not retained. The host weakened build tests sensitivity; its forbidden result is never an allowed production outcome.

## 5. Exhaustive schedules, seeds, and run budgets

### Gate ownership

The numeric ceilings and run budgets below are candidate planning values except the S6-05 exhaustive schedule threshold and seed range, and the S6-08/S6-09 native physical run budget; each has been measured and recorded below. They are not acceptance gates for this methodology document (S6-01). S6-01 is complete when the evidence levels, fixture format, outcome derivations, concrete initial numeric candidates with their arithmetic derivations and runtime estimates, schedule/seed rules, and task-owned calibration points are documented and reviewed. Each owning task must measure and record its own numeric gate before that task is accepted.

In particular, S6-05 measured the enumerator at the candidate 1,000,000-schedule ceiling and locked the exhaustive threshold and seed range before accepting its corpus. That measurement is an S6-05 gate; it does not retroactively block S6-01. S6-19 consumes the accepted threshold and seed range for exhaustive interleaving runs. S6-08/S6-09 have now measured and completed the native/physical gate; the remaining parallel-mode, STOP, save/restore, and Linux soak budgets belong to S6-36, S6-34, S6-35, and S6-37 respectively.

### Interleaving scheduler

If there are `n_i` shared-memory instructions in thread `i`, the number of order-preserving schedules is

`N = (Σ n_i)! / Π(n_i!)`.

For two threads with three such instructions each, `N = 6!/(3!·3!) = 20`. The calibrated S6-05 ceiling is to enumerate every complete schedule when `N ≤ 1,000,000`; count only memory-touching instructions while preserving per-thread order. Above that ceiling, use deterministic seeds `0…9,999` inclusive (10,000 seeds), with available instruction-step successors shuffled by `random.Random(seed)`. This is a reproducibility budget, not a statistical confidence claim. Never replace a finite enumeration below the accepted threshold with random samples.

S6-05 measured the actual `interleaving_instruction_atomic` enumerator on two threads with 12 shared loads each (`N = C(24,12) = 2,704,156` possible complete schedules). The enumerator disabled state deduplication for path counting, executed every state transition, retained its unique-state set and DFS frontier, and stopped at exactly 1,000,000 completed schedule paths. Three fresh-worker runs took 10.279–10.564 seconds, expanded 3,851,199 states each, retained 149 unique states with 12 pending, and used 27,607,040–27,705,344 bytes peak RSS. The time relative range was 2.72%. The candidate passed the 120-second and 2-GiB bounds, so no lower complete-schedule threshold was needed.

The seed range was measured separately in three fresh workers, each executing one seeded complete path for every seed from 0 through 9,999. Each run completed 10,000 paths / 250,000 expanded states, used 27,607,040–27,770,880 bytes peak RSS, and took 1.097–1.116 seconds (1.77% relative range). A 0–99 seed warm-up was excluded. These measured values lock the threshold and seed range for S6-19; they do not imply that any finite fixture was sampled. All finite S6-05 fixture schedule counts are at or below 1,000,000 and their model outcome sets are exhaustively enumerated. Raw runs, host configuration, and workload hashes: [`s6-05-enumerator-calibration.json`](../reports/data/stage6/s6-05-enumerator-calibration.json).

### Native x86 and physical x86

#### S6-08 native runner

`od-x86-runner --litmus-corpus` is a separate, Linux x86_64-only mode for the S6-05 corpus. It validates every instruction with the `od-isa` long-mode decoder and a corpus-specific exact operation/operand policy before mapping code. It requires the complete finite and formula corpus, pins each OS thread to an allowed logical CPU (preferring distinct physical cores and reporting SMT-sibling fallback), synchronizes round starts with a barrier, resets shared cells on every round, and emits per-process plus combined JSON histograms. The mandatory output path is initialized before child work and retains progress or failure details. Build revision, source fingerprint, dirty state, compiler, target/profile, and extra Rust flags are included in the result.

The mode does not widen `--fixture` or the existing native supervisor admission. Its child limits are 30 seconds CPU, 30 seconds wall time, and 256 MiB address space for each fixture process. It uses three fresh child processes per fixture. `MFENCE` and the supported `LOCK` forms are admitted only by this exact litmus policy after decoded-operation checks; unsupported forms, malformed corpus entries, and out-of-range branches fail before execution. The narrowly decoded bare `LOOP rel8` is allowed only for the formula counter group and remains unsupported by ordinary fixture admission and the interpreter/JIT.

On Linux x86_64, build the runner in release mode, then run the entire corpus with the calibrated S6-09 count. `N` and `K` are formula-counter parameters (`2 <= N <= 4`, `1 <= K <= 4096`):

```sh
cargo build --release -p od-x86-runner --locked
target/release/od-x86-runner --litmus-corpus tests/fixtures/guest/stage6/litmus --iterations 500000 --counter-n 2 --counter-k 2 --json --output work/stage6/s6-09-native-physical.json
```

The command exercises 15 finite cases from 16 JSON files (the sixteenth file defines the formula group) plus three formula identities, with three fresh processes per case. The physical Intel calibration ran all 18 cases at 10,000 rounds/process; all 54 histograms conserved their round counts, no observed outcome was outside the corresponding TSO set, and SB observed `00` once in 30,000 rounds. The ordinary `--fixture` mode also completed its S0-09 regression fixture; Linux `cargo test` passed 9 library tests and 258 runner tests. See [S6-08 calibration evidence](../reports/data/stage6/s6-08-native-calibration.md).

Calibration measured 54 process-throughput samples from 47,160 to 98,115 rounds/s (median 83,592; pooled 79,023). The original 10,000,000 rounds/process candidate would take at least 212 seconds at the slowest observed rate and exceed the 30-second child CPU/wall cap. The calibrated S6-09 budget was therefore **500,000 rounds/test/process × 3 fresh processes = 1,500,000 rounds/test**. The full run completed: 27,000,000 rounds over all 18 cases, 54 process histograms and 18 aggregate histograms conserved, no forbidden outcomes, and SB `00` appeared 19 times. Full-run throughput ranged 40,678–63,885 rounds/s (median 52,440; pooled 51,964); child durations were 7.83–12.29 seconds, and summed child elapsed was 519.592934988 seconds, excluding build and parent overhead. See [S6-09 physical evidence](../reports/data/stage6/s6-09-native-physical.md). Keep failed output under ignored `work/stage6/`; accepted raw runs and summaries belong in `docs/reports/data/stage6/`.

- Calibrated S6-09 base work is `1,500,000 × 18 = 27,000,000` rounds. This count is based on measured per-process rates and the existing 30-second CPU/wall child limits; it is not a probability guarantee. Report actual rounds and elapsed time for every case.
- For SB, an interesting `00` outcome must be observed at least once on the physical Intel x86 stand. The 10,000-round calibration already observed it once; retain that histogram with the full-run histogram. If no `00` is present in cumulative evidence, continue SB to a cumulative `3,200,000,000` rounds and retain both base and extended histograms. If it still does not appear, report the physical-x86 gate as not passed; do not weaken the model or change the expected set. The 3.2-billion ceiling is a bounded search budget informed by the scale of a rare event reported for a *different* test in the paper; it is not a probability estimate for SB.

### Parallel mode on the VM implementation

Run each fixture on the smallest configurations that can host all concurrent test threads: fixtures requiring at most two concurrent threads are exercised on both 2 and 4 vCPU; a four-thread test such as IRIW runs on 4 vCPU only. Record the applicable configurations per fixture; do not report a two-vCPU result for a four-thread test unless the harness explicitly schedules threads onto vCPUs and that schedule is itself under test. Candidate budget is `10,000,000` rounds/test/process in three fresh processes per applicable configuration (`30,000,000` rounds/test/configuration), plus the two deliberate sensitivity parts above. The observed final-state set must be a subset of `x86_tso`; a single forbidden outcome fails the run and is preserved with its binary hash and configuration. No finite run proves an outcome impossible.

### Runtime estimate and calibration

Native physical timing is now calibrated as recorded above. Parallel-mode timing remains **planning-only**: using the earlier 500,000 rounds/s assumption, the candidate `30,000,000 × Σ(applicable configurations per fixture)` can be estimated only after S6-36 defines its applicable corpus/configuration matrix. The 500,000 rounds/s assumption does not apply to the Intel S6-08 runner and is not a performance claim.

### Numeric gates still pending

The following test budgets are deliberately not invented before their harnesses and measurements exist. The owning task must write a numeric count, timeout/soak duration, arithmetic or measurement basis, and expected runtime to this document/report before it can pass:

| Task | Quantity still to calibrate | Task cannot pass until |
|---|---|---|
| S6-36 | Parallel-mode rounds/test/configuration and runtime (candidate values above) | S6-36 records rates on both applicable vCPU configurations and locks the numeric gate before parallel corpus acceptance. |
| S6-34 | Repeated STOP/kick/HLT races, participant timeout, and bounded completion time | S6-34 records repetitions, timeout, and race-window coverage from its test harness. |
| S6-35 | Save/restore cycles, pending-event variants, and failure-injection repetitions | S6-35 records an exact matrix and expected runtime before parallel save/load acceptance. |
| S6-37 | Linux boot repetitions, workload duration, and soak duration | S6-37 records counts/duration from a measured run budget; a short run cannot stand in for soak. |

## 6. Evidence and failure handling

Every run records: fixture and corpus SHA-256; test ID; source revision; runner commit; compiler/toolchain and flags; mode; CPU model, OS and thread count; iteration count; seed or explicit schedule; elapsed time; histogram; and exit/result status. Physical-x86 evidence goes in `docs/reports/data/stage6/`; temporary logs and binaries stay under ignored `work/stage6/`.

On a forbidden outcome, set/status mismatch, crash, or timeout: stop the gate, retain the raw output and exact input, and report the smallest reproducible fixture. Do not turn a forbidden result into noise, average it away, update expected outcomes from the run, or claim a pass after a later clean retry. A retry is additional evidence and must keep the first result.

The expected litmus corpus and its own TSO/SC enumerator are S6-05. S6-08's runner implementation, Linux regression tests, and full-corpus calibration are complete. The calibrated full physical run is S6-09; parallel-mode sensitivity is S6-36.
