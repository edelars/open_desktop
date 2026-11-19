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

Native physical timing is now calibrated as recorded above. Parallel-mode timing was measured by S6-36 (section "S6-36" below; 66-140 thousand rounds/s on 2 vCPU and 46-66 thousand on 4 vCPU on the Apple M3 Max stand: the mean of three processes per test, from the data in `docs/reports/data/stage6/s6-36/litmus-parallel/`) and the gate is locked at 1,000,000 rounds/test/process in three fresh processes per applicable configuration, instead of the candidate `30,000,000` per configuration. The 500,000 rounds/s assumption does not apply to the Intel S6-08 runner and is not a performance claim.

### Numeric gates still pending

The following test budgets are deliberately not invented before their harnesses and measurements exist. The owning task must write a numeric count, timeout/soak duration, arithmetic or measurement basis, and expected runtime to this document/report before it can pass:

| Task | Quantity still to calibrate | Task cannot pass until |
|---|---|---|
| S6-36 | Parallel-mode rounds/test/configuration and runtime (candidate values above) | Recorded in S6-36 (implemented, awaiting review): 1,000,000 rounds/test/process x 3 fresh processes per applicable configuration (3,000,000/test/configuration), rates and the matrix in "S6-36" below. |
| S6-34 | Repeated STOP/kick/HLT races, participant timeout, and bounded completion time | Recorded in S6-34 (implemented, awaiting review): repetitions, timeouts and race-window coverage are in the table of "S6-34 — параллельный режим" below and in the S6-34 report. |
| S6-35 | Save/restore cycles, pending-event variants, and failure-injection repetitions | Steps 1–3 passed independent review. The matrix below covers restore within the same machine. File-based verification depends on S9-24 and S9-25. |
| S6-37 | Linux boot repetitions, workload duration, and soak duration | S6-37 records counts/duration from a measured run budget; a short run cannot stand in for soak. |

### probe v1 (S6-16e)

The project's own guest `Stage6SmpProbeV1`: the boot processor sends INIT and SIPI with ICR writes, the application processor runs the trampoline to long mode, writes a marker with the nonce to RAM and COM1, raises a flag and halts; on one vCPU the guest prints `SMP PROBE AP ABSENT <nonce>`. Contract: [MACHINE_CONTRACT.md](../specs/MACHINE_CONTRACT.md#s6-16e--probe-v1-старт-ap). Expected UART text and memory cells are derived from the protocol and the SDM and written by the builder into its manifest; the tests read them from there, never from the run.

Build and check (no toolchain besides `python3`; the image is hand-encoded and reproducible, `probe.bin` SHA-256 is the same for every build):

```
python3 tools/build/build_stage6_smp_probe.py --output-dir work/stage6/smp_probe            # normal
python3 tools/build/build_stage6_smp_probe.py --output-dir work/stage6/smp_probe_nlm --variant no_long_mode
python3 tools/build/build_stage6_smp_probe.py --output-dir work/stage6/smp_probe_nf  --variant no_flag
python3 -m unittest tools/build/test_build_stage6_smp_probe.py
cargo test -p od-machine --offline --test stage6_smp_probe -- --test-threads=1
```

The integration test builds the image itself with `python3` (it fails, it does not skip, when the builder cannot run). The nonce is written by the loader into the 64-byte info block at GPA `0x7000`; it is not in the image. The probe is checked under fixed quanta 1, 2, 7, 100, 100000 and seeds 0..16 of the interleaving scheduler (same transcript and cells every time). The seed range of the CLI gate (`od-runner --stage6-smp-probe`) and the IPI exchange are S6-16f (below).

**Limitation (measured at review, N=2).** The order of UART lines and the OK are deterministic for quanta up to about 300 000 steps: `Fixed(100)` gives OK with retired `[825, 195]`, `Fixed(100000)` gives OK `[100125, 195]`, `Fixed(300000)` gives OK `[300125, 195]`. With `Fixed(400000)` and `Fixed(1000000)` the probe gives `FAIL TIMEOUT` (the `AP1 UP` line is printed after the result; BSP retired `[328357, 195]`). The BSP executes about 327 000 steps on the `WAIT_UP` wait for the flag at any quantum (one processor: 326 steps in total without the wait), so for `Fixed` of about 327 000 steps or more (and for any quantum with which the BSP manages to run the whole wait before the AP runs) the probe gives `FAIL TIMEOUT`. There is no false OK. The fix is to raise `WAIT_UP` with a justification through the maximum quantum of the policy; `WAIT_UP` is not changed in v1 (S6-16f did it in the v2 image, below).

### probe v2 (S6-16f)

The same guest with the IPI exchange and the negative cases; the image is chosen when it is built (`--scenario`), the nonce and the number of vCPUs when it is run. Contract: [MACHINE_CONTRACT.md](../specs/MACHINE_CONTRACT.md#s6-16f--обмен-ipi-и-отрицательные-случаи); forms: [CPU_CONTRACT.md](../specs/CPU_CONTRACT.md#s6-16f--формы-кода-probe-v2-обмен-ipi-idt). Expected transcripts and cells are the builder's manifest (protocol and SDM), never a run.

| Scenario | What the BSP does after "AP up" | Expected end |
|---|---|---|
| `normal` | prints the result | `SMP PROBE OK <nonce>`, all idle |
| `exchange` | K rounds: Fixed `0x30` to APIC ID 1, wait for the reply counter (the AP handler counts, writes EOI, sends Fixed `0x31` to APIC ID 0, `IRETQ`; the BSP handler counts, EOI, `IRETQ`), then checks both counters = K and the start counter = 1 | `SMP PROBE OK K=16 <nonce>`, ap_rx = bsp_rx = 16, IRR and ISR of both LAPICs empty |
| `resipi` | a second SIPI to the running AP, then as `exchange` | the same; `started` stays 1, start counter 1 (the AP is not restarted) |
| `bad-ipi` | prints `SMP PROBE BAD-IPI <nonce>`, writes ICR to APIC ID `vcpu_count` | the run ends with a device error of the LAPIC (a `VcpuQuantum` board error whose MMIO write to `0xFEE00000` failed with `UnsupportedConfiguration`); the marker is the last line; IRR and ISR of every LAPIC empty |
| `nmi` | NMI (ICR delivery mode 100b) to APIC ID 1, waits for the cell of the vector-2 handler | `SMP PROBE NMI OK <nonce>`; the board walks the NMI delivery on the CR3 of the AP, so the same holds when the AP's CR3 is not the board's `page_table_root` (a test patches the trampoline to a copy of the PML4; fixed in S6-16f after review) |
| `shootdown` (S6-17, probe v3 below) | K rounds: PTE of a shared page remapped, Fixed `0x32` to the AP, the AP runs `INVLPG` and reads the page again | `SMP PROBE SHOOTDOWN OK K=16 <nonce>`, every value the AP read after its `INVLPG` is the new frame's |
| any, N=1 | `SMP PROBE AP ABSENT <nonce>` | no success line |

```
python3 tools/build/build_stage6_smp_probe.py --output-dir work/stage6/smp_probe --scenario exchange --rounds 16
python3 -m unittest tools/build/test_build_stage6_smp_probe.py
cargo test -p od-machine --offline --test stage6_smp_probe
cargo test -p od-runner --offline --test stage6_smp_probe_cli
od-runner --stage6-smp-probe work/stage6/smp_probe/probe.bin --nonce TOKEN --vcpus 2 --seed 7
```

`od-runner --stage6-smp-probe IMAGE --nonce TOKEN [--vcpus N] [--scenario NAME] [--quantum Q | --seed S | --schedule CPU:STEPS,...] [--budget B] [--record-schedule]` prints one JSON object: retired, turns, steps, wakes, INIT/SIPI/NMI counters and activity per vCPU, COM1 as text and hex, the cells of the protocol, the vectors still set in each Local APIC, the reason the run ended, the quantum policy and the SHA-256 of the image. The same arguments give the same bytes. A schedule names the vCPU of every step and is normally the `schedule` of an earlier `--record-schedule` run with `--quantum 1` (one step per turn: the schedule is then exactly the cutting of the run). `--scenario` is a declaration checked against `build_manifest.json` beside the image.

**K and the waits.** K = 16 rounds (the plan's value; the builder takes `--rounds 1..=100`). The waits are `WAIT_UP = WAIT_RX = 0x100000` iterations of a five-instruction loop, about 5.24 million BSP steps. Rule: a wait cannot run out inside one turn of the BSP while the AP has not had its turn, so the largest `Fixed` quantum of the probe is **4 000 000 steps** (`STAGE6_SMP_PROBE_MAX_FIXED_QUANTUM`); the seeded policy gives at most 256. Measured in release with `--nonce ab`, N=2 (retired [BSP, AP]): `exchange` `Fixed(1)` OK [1333, 419]; `Fixed(7)` [1323, 419]; `Fixed(100)` [2468, 419]; `Fixed(1024)` [17573, 419]; `Fixed(100000)` [1 700 168, 419]; `Fixed(1000000)` [17 000 168, 419]; `Fixed(4000000)` OK [68 000 168, 419] (32 s, needs `--budget` above 68 million); `normal` `Fixed(5000000)` OK, `Fixed(6000000)` `FAIL TIMEOUT` (BSP 5 243 560 steps, `AP1 UP` after the failure line: no false OK). A larger `Fixed` quantum is outside the range of the probe. The budget of a `Fixed(Q)` run is about K·Q plus a small constant.

**Seed ranges (the probe, scenario `exchange`, N=2).**
- CI: seeds `0..=63` through the CLI (`tools/runner/tests/stage6_smp_probe_cli.rs`, `every_seed_gives_the_same_transcript_and_cells_and_the_same_seed_the_same_bytes`): every seed runs twice, the JSON is byte-identical, COM1 is the manifest transcript for every seed, the cells are the manifest's, `started = [0, 1]`, no vector left in any LAPIC, at least two seeds differ in their number of turns. In-process (`crates/od-machine/tests/stage6_smp_probe.rs`) the exchange, the second SIPI and the NMI also run under fixed quanta 1, 2, 7, 100, 1024 (and 100000 for the exchange) and seeds `0..16`, and the missing-APIC-ID case under the same set. Debug build: the CLI test file takes about 70 seconds.
- Proof: seeds `0..=9999` in a release build, `docs/reports/data/stage6/s6-16f-smp-probe-seeds-0-9999.json`: 10 000 runs, 140.7 seconds (14.07 ms per run, the 0..=255 smoke run took 3.6 seconds), one distinct transcript, 21 distinct turn counts (39..59), 816 distinct pairs of retired counts, image SHA-256 `8b45797e...43f0` (nonce `cli-nonce-16f`). The range was not reduced: its time is calibrated by that run, 2.35 minutes. Command: `OD_S6_SMP_SEEDS=0..=9999 OD_S6_SMP_SEEDS_OUT=docs/reports/data/stage6/<file>.json cargo test --release -p od-runner --offline --test stage6_smp_probe_cli -- --ignored seed_proof --nocapture`. This is a reproducibility budget (as in section 5), not a statistical claim.

**Not covered by an oracle.** The expected transcripts and cells come from the protocol and the SDM by hand; there is no independent model of the interleaving of this guest (the S6-05 enumerator covers the litmus corpus only). The second SIPI is proved by `started = 1`, start counter = 1 and an unchanged transcript; the model does not count an ignored SIPI, so "ignored" is not observed directly. The NMI on another CR3 and the ICR device error are recorded as the board behaves, not as a hardware-confirmed rule.

### probe v3 (S6-17, scenario `shootdown`)

The same guest with a guest-run TLB shootdown on shared page tables (`--scenario shootdown`, K by `--rounds`, 16 by default). Contract: [MACHINE_CONTRACT.md](../specs/MACHINE_CONTRACT.md#s6-17--согласование-адресных-пространств-и-shootdown); property and rules for future caches: [MEMORY_MODEL.md](../specs/MEMORY_MODEL.md).

| Step | Who | What |
|---|---|---|
| setup | image | `PD[1] -> PT_L`, `PT_L[1]` maps X = `0x201000` to frame A (`0xC000`, value `0xA1A1A1A1` at `+0x100`); frame B is `0x7000` (value `0xB2B2B2B2`) |
| prime | AP | reads X before the flag "AP up" (first translation, frame A) |
| round r | BSP | stores the PTE of X (frame B for odd r, A for even r), sends Fixed `0x32` to APIC ID 1, waits for the post counter = r |
| shootdown | AP handler | `INVLPG [X]`, reads X (cell `0xC0C0`), counts the acknowledgement (`0xC030`), EOI, `IRETQ` |
| post | AP main loop | `CLI`, compare acknowledgements with posts, `STI; HLT` when equal (no lost wake-up), otherwise reads X (`0xC0C8`) and counts the post (`0xC038`) |
| check | BSP | acknowledgements = r, both values the AP read after its `INVLPG` equal the new frame's value; `SMP PROBE SHOOTDOWN OK K=<K> <nonce>` |

From memory of the SDM (not checked against the source) a processor may use the old translation until its own invalidation and must not after it; the probe asserts only the second. **Sensitivity: not checked.** No translation cache of a vCPU outlives a quantum on the board (the quantum has no fetch cache, data walks are uncached, `INVLPG m` is a no-op in the model without a TLB), so the property holds trivially, and a test with the `INVLPG` cut out of the image gives the same result (`without_the_invlpg_...`): it records this and must start failing when a translation cache that outlives HLT or a quantum exists. What the scenario does prove: the AP really reads through the shared page table (a test with the BSP's PTE store cut out gives `FAIL SHOOTDOWN`), the IPI reaches the AP in every schedule, the handler with `INVLPG [r8]` runs in the AP quantum, and the order "PTE written, IPI, INVLPG, read" gives the new frame for every round and quantum. **Limitation:** when the BSP's wait for the post counter runs out (at most `WAIT_RX` iterations) the probe prints `FAIL SHOOTDOWN`, not `FAIL TIMEOUT`, so a timeout and a mismatch of the values are indistinguishable from the UART in this scenario.

```
python3 tools/build/build_stage6_smp_probe.py --output-dir work/stage6/smp_probe_sd --scenario shootdown --rounds 16
cargo test -p od-machine --offline --test stage6_smp_probe
cargo test -p od-runner --offline --test stage6_smp_probe_cli
OD_S6_SMP_SEEDS=0..=9999 OD_S6_SMP_SCENARIO=shootdown OD_S6_SMP_SEEDS_OUT=docs/reports/data/stage6/s6-17-smp-probe-shootdown-seeds-0-9999.json \
  cargo test --release -p od-runner --offline --test stage6_smp_probe_cli -- --ignored seed_proof
```

**Seed ranges (scenario `shootdown`, N=2).** CI: seeds `0..=63` through the CLI (`the_shootdown_scenario_ends_ok_under_every_seed_and_leaves_the_cells_of_the_manifest`) and quanta 1, 2, 7, 100, 100000 and seeds 0..16 through the library. Proof: seeds `0..=9999` in a release build, `docs/reports/data/stage6/s6-17-smp-probe-shootdown-seeds-0-9999.json`: 10 000 runs, 152.6 seconds (15.3 ms per run), one distinct transcript, 31 distinct turn counts (40..70), 459 distinct pairs of retired counts, image SHA-256 `22e60a47...b0513`. As in v2, the expected transcript and cells come from the protocol and the SDM by hand; there is no independent oracle of the interleaving.

### S6-34 — параллельный режим: потоки vCPU, kick, остановка

Параллельный режим (один host-поток на vCPU, [MACHINE_CONTRACT.md](../specs/MACHINE_CONTRACT.md#s6-34--потоки-vcpu-kick-и-остановка)) не детерминирован: seed, кванты и записанное расписание ему не нужны, и результат прогона не равен функции аргументов. Поэтому проверяется результат гостя и инварианты, а не порядок; каждый тест повторяется много раз, каждое ожидание ограничено по времени (зависание или потеря пробуждения — падение теста с причиной, а не зависание), а чувствительность каждой проверки показана заменой кода на неверный (от противного; запись в [контракте](../specs/MACHINE_CONTRACT.md#s6-34--потоки-vcpu-kick-и-остановка) и в отчёте).

```
cargo test -p od-machine --offline --lib parallel_run -- --test-threads=1      # потоки, kick, остановка, шлюз split LOCK, приёмник пробуждения
cargo test -p od-machine --offline --lib -- fetch_translation_cache pte_rewritten set_step_view   # поколения кеша, межпоточный обход, вид набора CPL0 на &MachineBus
cargo test -p od-machine --offline --test stage6_smp_probe parallel            # probe v2/v3 на потоках: все сценарии x кванты 1, 7, 100, 1024
cargo test -p od-runner --offline --test stage6_smp_probe_cli parallel         # CLI --parallel
od-runner --stage6-smp-probe work/stage6/smp_probe/probe.bin --nonce TOKEN --vcpus 2 --parallel --quantum 100 [--timeout-s 120]
# длинная форма (release, `#[ignore]`): probe (exchange и shootdown, кванты по кругу), остановка посреди обмена, пробуждение, litmus split LOCK
OD_S6_PAR_RUNS=2000 cargo test --release -p od-machine --offline --test stage6_smp_probe -- --ignored parallel_probe_many_runs --nocapture
OD_S6_STOP_RUNS=1000 cargo test --release -p od-machine --offline --lib -- --ignored parallel_run::tests::stop_in_the_middle_many_runs --nocapture
OD_S6_WAKE_ROUNDS=50000 cargo test --release -p od-machine --offline --lib -- --ignored parallel_run::tests::wake_up_many_rounds --nocapture
OD_S6_LITMUS_RUNS=200 cargo test --release -p od-machine --offline --lib -- --ignored parallel_run::tests::split_lock_litmus_many_runs --nocapture
```

В JSON параллельного пробника `idle_heals` показывает число решений о пробуждении по готовому вектору LAPIC при повторно наблюдаемом простое без kick. Длинный прогон `parallel_probe_many_runs` проверяет ноль в каждом штатном exchange и shootdown. Тест `the_idle_arbiter_counts_recovery_of_a_pending_vector_without_a_kick` пропускает ровно один kick и проверяет доставку вектора и счётчик 1.

Что и сколько раз (значения записаны в отчёте S6-34, раздел «Тесты»; пределы: ожидание конца прогона 60 с, остановка 5–10 с, шлюз split `LOCK` 20 с, тайм-аут теста остановки 60 мс):

| Проверка | Повторы | Что должно быть |
|---|---|---|
| probe S6-16 на потоках (`normal`, `exchange`, `resipi`, `nmi`, `shootdown`) | 4 кванта x 6 раундов x 5 сценариев по умолчанию (`OD_S6_PAR_RUNS` меняет число раундов); длинная — 2000 пар `exchange`+`shootdown` | транскрипт и ячейки из манифеста сборщика, INIT/SIPI по одному, оба vCPU `Halted`, IRR/ISR всех LAPIC пусты |
| обмен IPI на фикстуре машины (2 vCPU) | 6 квантов x 25 | оба счётчика обработчиков равны числу раундов, LAPIC пусты, потоки соединены |
| остановка посреди обмена IPI (`stopping_in_the_middle_of_an_ipi_exchange_and_resuming_ends_right`) | 100, квант 1, порог остановки 40…4000 retired всех vCPU; не меньше половины остановок попадает внутрь обмена | счётчики инструкций, вызовы, ячейки и часы не растут при остановке (5 мс), после `resume` обмен кончается верно |
| потерянное пробуждение | (а) `a_wake_up_posted_around_the_sleep_is_never_lost`: 3000 IPI спящему vCPU, задержка 0…6000 витков между счётчиком обработчика и следующей постановкой (сканирует путь от `IRETQ` до `park`); (б) `a_wake_up_posted_while_the_vcpu_is_between_its_look_and_its_park_is_never_lost`: 500 раундов, окно «просмотр LAPIC — `park`» удерживается намеренно (тестовая точка `pre_sleep`), IPI ставится внутри окна; BSP занят (`jmp .`), иначе арбитр простоя лечит потерю | каждый IPI обработан (счётчик точен), ожидание каждого ограничено 20 с; от противного: kick без флага и сброс флага после просмотра роняют тесты |
| IPI спящему остановленному vCPU | 50 | до `resume` ни вызова, ни шага, ни часов, вектор в IRR; после — обработан и снят |
| тайм-аут остановки | 1 (+ повтор остановки) | ошибка называет vCPU, запрос снят, вторая остановка проходит |
| остановка спящих | 1 | подтверждение за < 1 с, оба `Asleep(Halted)`, вызовов нет за 100 мс сна |
| состояние и RAM остановленного | 300 остановок | регистр и память согласованы (разность 0 или 1), счётчик не идёт назад |
| конец прогона, ошибка, паника | 1 каждый | первая причина, все потоки соединены (`threads_started == threads_exited`) |
| litmus-счётчик с `LOCK INC` | выровненный: 2 и 4 vCPU x 5 x 1500; split (через границу строки/страницы, 2 МиБ лист и два кадра): 2 и 4 vCPU x 5 x 400; смешанный (split на одних vCPU и выровненный `LOCK INC` на общих байтах у других): 2 и 4 vCPU x 5 x 300, оба режима таблиц | итог точен; `split_locks` каждого vCPU = числу операций; в гонке за шлюз проигрывает кто-то (`split_losses > 0`) |
| кеш перевода | 20 000 переписываний PTE другим потоком | ни одного ответа старее завершённой записи |

Окно гонок: тест остановки сам считает, сколько остановок попало внутрь обмена (порог ≥ 50 %); тест пробуждения заменяет «случайные попадания» сканированием окна задержкой; тест шлюза `simultaneous_split_lock_requests_have_a_loser_and_lose_nothing` проверяет, что проигравшие действительно были.

### S6-35 — состояние на остановленной параллельной машине

Параллельный режим не детерминирован, поэтому проверяется итог гостя и сама остановка, а не побайтное равенство с непрерывным прогоном. Контракт: [MACHINE_CONTRACT.md](../specs/MACHINE_CONTRACT.md#s6-35--состояние-на-остановленной-параллельной-машине). Это состояние vCPU и Local APIC в одной машине, а не снимок машины и не файл (формат — `S9-24`, `S9-25`).

```
cargo test -p od-machine --offline --lib -- parallel_snapshot_tests vm_snapshot                     # таймеры, IRR, отказы, kick остановленного спящего AP, S6-24
cargo test -p od-machine --offline --test stage6_smp_probe -- a_parallel_run_stopped_at_a_random_moment   # probe v2/v3: остановка на потоках, сброс, restore, продолжение в режиме чередования
# длинная форма (release, `#[ignore]`), 50 раундов (seed 0..50), число меняется переменной
OD_S6_SNAP_RUNS=50 cargo test --release -p od-machine --offline --test stage6_smp_probe -- --ignored parallel_stop_and_restore_many_runs --nocapture
OD_S6_SNAP_TIMER_RUNS=50 cargo test --release -p od-machine --offline --lib -- --ignored parallel_snapshot_tests::timers_across_the_stop_many_runs --nocapture
```

Число повторов в `SMP.md` для S6-35 раньше не было задано (строка таблицы «Numeric gates» ссылалась на этот раздел); записано здесь: длинная форма — **50 раундов, seed `0..50`** (probe: exchange и shootdown по очереди; фикстура платы: seed задаёт квант потоков 1/7/64 и момент остановки 5000…100000 ns), обычный прогон — 8 раундов probe и 6 раундов фикстуры. Ожидаемое время: см. раздел S6-35 отчёта (измерено). Моменты остановки и квант потоков берутся из seed; продолжение идёт в режиме чередования с `Seeded(seed)`.

| Проверка | Повторы | Что должно быть |
|---|---|---|
| probe v2 `exchange` и v3 `shootdown` на потоках, остановка по числу retired из seed, сброс комплекса, восстановление, продолжение в режиме чередования | 8 (обычный), 50 (`#[ignore]`) | состояние на остановке == состояние машины после прогона; IRR/ISR до сброса == в снимке, после сброса пусто, после `restore` равно снятому; итог — транскрипт и ячейки из манифеста сборщика, LAPIC пусты, оба vCPU `Halted`; в длинной форме не меньше четверти остановок внутри обмена |
| фикстура платы: таймеры (периодический на BSP, одноразовый на AP) и IPI остановленному спящему AP, сброс, восстановление | 6 (обычный), 50 (`#[ignore]`) | сроки до сброса == после `restore`, очереди после сброса пусты; обработчик AP вызван ровно 2 раза; обработчик BSP + запрос в полёте == `now / период`; одноразовый таймер израсходован |
| снимок без остановки, по `proof` снятой остановки, по `proof` более ранней остановки | в каждом раунде фикстуры | `NotStopped`, `NotStopped`, `StaleSafePoint` |
| kick остановленного спящего AP во время снимка (m2 ревью S6-34) | 300 остановок | окно m2 не воспроизводится; точный тест — S6-36. Снимок взят каждый раз, IPI не потерян |

### S6-36 — litmus в параллельном режиме и чувствительность

Контракт: [MACHINE_CONTRACT.md](../specs/MACHINE_CONTRACT.md#s6-36--litmus-в-параллельном-режиме-и-чувствительность). Данные: [`docs/reports/data/stage6/s6-36/`](../reports/data/stage6/s6-36/). Параллельный режим недетерминирован: проверяется, что наблюдаемое множество — подмножество x86-TSO fixture, а не побайтное равенство. Прогон без проверки чувствительности доказательством не считается (разделы 5.1 и ниже).

**Числовой gate (записан здесь, строка таблицы «Numeric gates» закрыта).** Темп измерен на этом host (Apple M3 Max, 14 ядер, release): 2 vCPU 66–140 тыс. раундов/с, 4 vCPU 46–66 тыс. раундов/с (среднее трёх процессов на тест, по тестам — `litmus-parallel/summary.md`; отдельные процессы по JSON: 59–163 тыс. на 2 vCPU и 11–76 тыс. на 4 vCPU, нижние значения — при занятом хосте). Кандидат 10 000 000 раундов на тест на процесс дал бы около 70–220 с на тест на процесс, то есть свыше двух часов на конфигурацию на каждый процесс; число снижено: **1 000 000 раундов на тест на процесс, три свежих процесса на конфигурацию = 3 000 000 раундов на тест на конфигурацию**; фактическое время одного процесса — 2 vCPU около 3 мин, 4 vCPU около 5–6 мин, счётчики около 2 мин, три процесса вместе около 28 мин. Для host-теста MP и 2+2W — 100 000 000 раундов каждого шаблона на сборку (около 46 с). Гость с `LOCK INC` — 20 000 инкрементов на vCPU, 2 и 4 vCPU.

**Матрица конфигураций.** Тест до двух потоков (14 fixture: `2+2W`, `LB`, `LOCK-CMPXCHG-MUTEX`, `LOCK-XADD`, `MP`, `SB-LOCK-LOCAL`, `SB-LOCK-XADD`, `SB-LOCK`, `SB-MFENCE`, `SB-XCHG`, `SB`, `STORE-FORWARDING`, `UNLOCKED-INC`, `XCHG-MUTEX`; счётчики `LOCK-INC`, `UNLOCKED-INC`, `THREE-INSN-COUNTER` при N = 2, K = 2) — на **2 и 4 vCPU**; `IRIW` (четыре потока) — **только 4 vCPU** (на 2 vCPU — `not_applicable`, результата нет); счётчики N = 4, K = 2 (`LOCK-INC`, `UNLOCKED-INC`) — 4 vCPU. Потоки litmus размещаются на vCPU всеми инъективными отображениями по кругу (раунд `r` берёт размещение `r mod P`).

```
cargo build --release -p od-runner --locked --offline
# три свежих процесса на конфигурацию (--vcpus 2 и 4); для IRIW на 2 vCPU результат not_applicable
target/release/od-runner --stage6-litmus --corpus tests/fixtures/guest/stage6/litmus --parallel --vcpus 2 --iterations 1000000
target/release/od-runner --stage6-litmus --corpus tests/fixtures/guest/stage6/litmus --parallel --vcpus 4 --iterations 1000000
target/release/od-runner --stage6-litmus --formula tests/fixtures/guest/stage6/litmus/counters-formula.json --counter LOCK-INC --threads 2 --rounds 2 --parallel --vcpus 2 --iterations 1000000
cargo test -p od-runner --offline --test stage6_litmus_parallel_cli
cargo test -p od-machine --offline --lib -- litmus_parallel
```

Выход 5 при запрещённом исходе; вывод несёт `build` (`weak_host_ordering`, `nonatomic_lock_test` — оба `false` в штатной сборке), число размещений, гистограмму с первым раундом и размещением каждого исхода, исходы x86-TSO, которых не было, `late_barrier_rounds` (раунды, где стартовый барьер не прошёл до первой инструкции: 0), время и скорость.

**Чувствительность, часть 1 (host).** Штатная сборка: ни одного запрещённого исхода. Сборка с `weak-host-ordering`: MP наблюдается; 2+2W не наблюдался: какой порядок нарушен, тест не различает; 2+2W, которому нужна перестановка записей, не наблюдался.

**Что именно ослаблено в сборке `weak-host-ordering` (ревью S6-36, условие (в)).** Чувствительность показана для порядка пути записи ЦЕЛИКОМ: обычные доступы `SeqCst` -> `Relaxed`, host-fence на гостевые fences не ставятся, а `Release`-подъём поколения страницы (`physical_memory.rs`, `fetch_add`) тоже `Relaxed`. То есть показано, что сломанный путь в сумме виден, а не что каждое из трёх звеньев по отдельности нужно. Кеш кода в такой сборке НЕ гарантирует публикацию записи (читатель, увидевший новое поколение, не обязан видеть данные): допустимо только для тестовой сборки. Предварительный прогон с `Release`-поколением (ослаблены только обычные доступы и fence) не сохранён: решение ослабить и поколение принято по результатам прогона, который не сохранён; данных о нём в `docs/reports/data/stage6/s6-36/` нет, и они здесь не восстанавливаются.

Значения по умолчанию host-теста: `#[ignore]`-прогоны `host_order_litmus` берут 20 000 000 раундов каждого шаблона (`OD_S6_HOST_ORDER_ROUNDS`), а число gate 100 000 000 задаётся этой переменной окружения при запуске; быстрая форма без `--ignored` — 200 000 раундов. Вывод `--parallel` несёт информационное поле `outside_sc` (наблюдённые исходы вне SC-множества fixture, но внутри x86-TSO; пусто для счётчиков, не отказ).

**Статистическая сила gate (ограничение, ревью S6-36, условие (а)).** При 0 запрещённых исходов из 3 000 000 раундов (правило трёх) верхняя граница 95% на раунд — около 1·10^-6 на тест и конфигурацию; при кандидате 30 000 000 — около 1·10^-7. Это не вероятностная гарантия: раунды не независимы (барьеры, цикл размещений, один host), другой host (Intel, другое поколение ARM) не проверялся. Гостевой корпус не измерил чувствительность к перестановкам host: он ловит неатомарный `LOCK` (на 200 000 раундов), но в сборке `weak-host-ordering` не прогонялся; чувствительность к перестановкам показана только host-тестом (MP, 334 и 462 из 100 000 000, то есть около (3–5)·10^-6 на раунд). Основание достаточности барьеров — источники S6-04 (`MEMORY_MODEL.md`) и S6-06 (замер host-примитивов), а не этот gate. Открыто (не обязательно): прогнать корпус в `weak-host-ordering` на тех же 1 000 000 раундов на процесс и записать, видит ли он перестановку.

```
OD_S6_HOST_ORDER_ROUNDS=100000000 cargo test --release -p od-machine --offline --lib -- --ignored host_order_litmus --nocapture
OD_S6_HOST_ORDER_ROUNDS=100000000 cargo test --release -p od-machine --offline --lib --features weak-host-ordering -- --ignored host_order_litmus --nocapture
cargo test -p od-machine --offline --lib -- host_order_litmus         # быстрая форма: 200 000 раундов, штатная сборка
```

Задержка после барьера регулируется `OD_S6_HOST_ORDER_JITTER` (по умолчанию 4 `spin_loop`; при 64 и более MP почти не наблюдался: за 50 000 000 раундов 1, при 256 — 0). Результат записан в контракте и `host-order-*.txt`.

**Чувствительность, часть 2 (гость).**

```
cargo test -p od-machine --offline --lib -- parallel_run::tests::the_lock_inc_counter --nocapture                                # штатная: точный счёт
cargo test -p od-machine --offline --lib --features nonatomic-lock-test -- parallel_run::tests::the_lock_inc_counter --nocapture   # счёт ниже точного
CARGO_TARGET_DIR=/private/tmp/s636/target_nonatomic cargo build --release --offline -p od-runner --features od-machine/nonatomic-lock-test
```

Ослабляющие сборки гоняют только для проверки чувствительности: в них тесты с точным счётом и порядком `SeqCst` падают, так и задумано. Штатный профиль их не включает (`cargo metadata`, `features-check.txt`).

**Условия 1–3 (барьер потока, `READ_WAIT`).** `cargo test -p od-machine --offline --lib -- parallel_snapshot_tests vm_snapshot parallel_run`. Ревью S6-36, M-1: окно барьера сужено, не устранено (поток, разбуженный ДО остановки, на несколько инструкций опускает `safe` под остановкой с доказательством); `READ_WAIT` (2 с) возвращён в `take_parallel`, по истечении — `SnapshotError::SafeFlagTimeout`, не `StaleSafePoint`; тест `a_thread_between_its_two_stores_in_the_barrier_is_waited_for_and_not_refused` (вторая `#[cfg(test)]`-зацепка `standing_window`). Мутация (условие 1 убрано) валит тест окна в 20 раундах из 20, `mutation-barrier-window.txt`.

**Результат корпуса (2026-11-18).** 30 прогонов (3 свежих процесса x (2 vCPU и 4 vCPU) x (корпус, 3 счётчика N = 2, K = 2) + счётчики N = 4 на 4 vCPU), все exit 0: **111 000 000 раундов, запрещённых исходов нет**, `late_barrier_rounds` 0; на 2 vCPU 14 fixture + 3 счётчика, IRIW `not_applicable`; на 4 vCPU 15 fixture + 3 счётчика + 2 счётчика N = 4. Наблюдены все исходы множества x86-TSO, кроме `00` у `SB` и `SB-LOCK-LOCAL` (3 из 4): при `SeqCst` он недостижим, это допустимо; IRIW — 15 из 15. Таблица по тестам: [`litmus-parallel/summary.md`](../reports/data/stage6/s6-36/litmus-parallel/summary.md), сырые JSON рядом.

### S6-19 — litmus в режиме чередования

Реализовано (S6-19; **после S6-20 интерпретатор исполняет и формы `LOCK`/`XCHG` корпуса, все 15 конечных fixture и счётчик LOCK-INC — `equal`**, абзац «S6-20» ниже): `crates/od-machine/src/litmus_interleaving.rs` (драйвер; публичный API поверх него — S6-19b; планировщик S6-15 в крейте `pub(crate)`), тесты там же, данные [`s6-19-litmus-interleaving.json`](../reports/data/stage6/s6-19-litmus-interleaving.json). Контракт: [MACHINE_CONTRACT.md](../specs/MACHINE_CONTRACT.md#s6-19--litmus-в-режиме-чередования). Чекбокс S6-19 не отмечен: S6-19 готов к закрытию (литмусы `equal`), закрывается после ревью S6-20. Флаг `od-runner --stage6-litmus`, публичный API и CLI-тест — S6-19b.

**Что делает драйвер.** Один vCPU на поток теста, сразу в long mode, плоская identity RAM 64 КиБ (код потока по адресу из `code_regions` fixture, ячейки по адресам fixture). Это harness litmus по SMP_CONTRACT 4.4, **не старт AP**; так же записано в JSON (`harness`). Байты берутся из fixture как есть (после последней инструкции потока гость ничего не исполняет: поток кончается, когда RIP равен концу кода). Эталон «SC при неделимости инструкции» — поле `outcomes.interleaving_instruction_atomic` fixture (S6-05, выведено из источников собственным переборщиком), для счётчиков — формула `formula_oracle` из `counters-formula.json`; из прогонов эталон не берётся. Режим памяти, который проверяется: «SC, каждая инструкция неделима» — не x86-TSO и не параллельный режим.

**Полный перебор.** Расписание — последовательность «чей шаг» по **всем** инструкциям (не только по обращающимся к памяти: сокращение, разрешённое S6-01, не использовалось), подаётся планировщику как `QuantumPolicy::Schedule`, шаг = ход в одну инструкцию. Число расписаний `(Σ n_i)! / Π n_i!`; порог 1 000 000 (S6-05). Условие: наблюдённое множество **равно** эталону. Сравнение даёт три класса: «запрещённый» (наблюдён, нет в эталоне — дефект, эталон не подгоняется), «не покрыт перебором» (есть в эталоне, не достигнут — не ошибка машины), «равно».

**Режим seed.** Seed `0..=9999` (SMP.md §5): seed даёт **случайный порядок шагов** (`random_schedule`): на каждом шаге поток выбирается среди ещё не исчерпанных (по `steps_per_thread`) через SplitMix64(seed) (`next_u64() % число_открытых`), итоговая последовательность подаётся планировщику как `QuantumPolicy::Schedule` (одна запись = один шаг). Детерминирован по seed; любой поток может пойти первым. `QuantumPolicy::Seeded` драйвером **не используется** (первая версия среза его использовала; см. объяснение слабого покрытия ниже). Условие: наблюдённое множество — **подмножество** эталона; не достигнутый эталонный исход записывается как «не покрыт seed», не как ошибка. Применён к тестам, чьё число расписаний выше порога, и как перекрёстная проверка на конечных. Драйвер не учитывает `LOOP` (он не обращается к памяти; счётчики шагов — по шагам с памятью; формула fixture учитывает `LOOP`, сокращение записано: тело корпуса повторено K раз).

**Результат (release, 4,4 с на весь корпус после S6-20; 3,7 с до него, без семи заблокированных fixture).** Запрещённых исходов — **0**; эталонных исходов, не достигнутых полным перебором, — **0**. Разрезанных инструкций (вызов интерпретатора кончился не на границе инструкции и не на RIP того же `REP`) — 0, неполных прогонов — 0.

| Тест | vCPU | Расписаний | Исходов (= эталон) |
|---|---|---|---|
| SB | 2 | 6 | 3 (`(0,0)` не достигнут: это только x86-TSO, ожидаемо) |
| MP | 2 | 6 | 3 (запрещённый `eax=1, ebx=0` отсутствует и в fixture `x86_tso`) |
| LB | 2 | 6 | 3 |
| 2+2W | 2 | 6 | 3 |
| IRIW | 4 | 180 | 15 |
| SB-MFENCE | 2 | 20 | 3 |
| STORE-FORWARDING | 2 | 3 | 2 |
| UNLOCKED-INC (fixture) | 2 | 2 | 1 (всегда 2: потери обновления нет) |
| LOCK-CMPXCHG-MUTEX (S6-20) | 2 | 2 | 2 (взаимное исключение: ровно один захват) |
| LOCK-XADD (S6-20) | 2 | 2 | 2 (билеты 0 и 1 в обоих порядках; итог 2) |
| XCHG-MUTEX (S6-20) | 2 | 2 | 2 |
| SB-LOCK, SB-XCHG, SB-LOCK-XADD (S6-20) | 2 | 20 | 3 (`(0,0)` недостижим) |
| SB-LOCK-LOCAL (S6-20) | 2 | 10 | 3 |
| LOCK-INC, N·K = 2·1 … 4·1, 2·3, 3·2 (S6-20) | 2–4 | 2…90 | 1 (`N*K` при любом расписании) |
| UNLOCKED-INC, N·K = 2·1 … 4·1, 2·3, 3·2 | 2–4 | 2…90 | 1 (`N*K`) |
| THREE-INSN-COUNTER N·K = 2·1, 2·2, 2·3, 3·1, 4·1 | 2–4 | 20; 924; 48 620; 1 680; 369 600 | 2; 3; 5; 3; 4 (весь диапазон формулы, включая потерю обновлений) |

Seed (порог превышен): THREE-INSN-COUNTER N=3, K=2 (17 153 136 расписаний) — 10 000 seed, 5 исходов, все в диапазоне формулы; N=4, K=4 (2,36e26 расписаний) — 9 значений из 15 возможных (2…16; 6 остальных seed не достигли, это не ошибка), все в диапазоне; UNLOCKED-INC N=4, K=4 (63 063 000) — один исход 16. Перекрёстная проверка по seed (0..=9999, случайный порядок шагов) на всех исполняемых конечных тестах: запрещённых нет, **покрытие полное** (достигнуты все эталонные исходы: SB 3 из 3, включая `[1,0,1,1]` — cpu1 раньше cpu0; IRIW 15 из 15; MP, LB, 2+2W, SB-MFENCE — 3 из 3; STORE-FORWARDING 2 из 2; UNLOCKED-INC 1 из 1). Тест `seeds_reach_every_expected_outcome_of_every_finite_fixture` проверяет это на всех 15 fixture (до S6-20 — на 8 исполняемых; формы `LOCK` корпуса и LOCK-INC тоже покрыты полностью: seed 0..=9999 достигают все эталонные исходы, запрещённых нет). Конечные тесты по-прежнему принимаются по полному перебору (равенство), seed — подмножество и перекрёстная проверка. Для больших тестов покрытие seed ниже перебора только на THREE-INSN-COUNTER N=4, K=4 (значения 11…16 близки к полностью последовательному исполнению, при равномерном выборе потока на шаге они редки; записано как «не покрыт seed»).

**Причина слабого покрытия в первой версии среза (исправлено).** Первая версия использовала `QuantumPolicy::Seeded` с квантами 1…4. Планировщик обходит vCPU по кругу, начиная с cpu0 (`next_cpu = 0`, `smp_scheduler.rs`), поэтому исходы, в которых поздний поток идёт раньше cpu0, были недостижимы при любом seed: SB `[1,0]` (2 из 3), IRIW — всегда `[1,1,1,1,1,1]` (1 из 15). Прежнее объяснение «короткие программы чаще исполняются потоком целиком» было неверным. Исправление — случайный порядок шагов, описанный выше.

**Повтор и воспроизводимость.** `0..=9999` — один прогон (на больших счётчиках). Побайтный повтор (два прогона, равенство результатов) — `0..=199` на SB, MP, IRIW (`the_same_seed_gives_the_same_exploration_and_every_outcome_is_expected`). Для THREE-INSN-COUNTER N=3, K=2 первый свидетель каждого исхода прогона `0..=99` переигрывается отдельно как явное расписание и даёт тот же исход (`a_big_counter_is_explored_by_seeds_as_a_subset_of_the_formula`). Детерминизм `random_schedule` — тест `random_schedule_is_deterministic_...`.

**Доказательство чередования между инструкциями.** THREE-INSN-COUNTER N=2, K=1 при `Fixed(1)` (круговое чередование по одному шагу): `x = 1` (потеря обновления), при `Fixed(1024)`: `x = 2`; перебор достигает 1 и 2. Каждое пойманное расписание воспроизводится как `Schedule` (тест).

**Отличия от настоящего x86 (записано и в JSON).** `INC [x]` без `LOCK` здесь неделим (всегда `N*K`; на x86 возможна потеря обновления — S6-09 её наблюдал); SB-исход «оба прочитали 0» недостижим (он только в x86-TSO).

**Граница REP (L = 1024, SMP_CONTRACT §8).** Эталон для REP-тестов — отдельный абстрактный перебор в тесте («SC, `REP` режется после L, 2L, … итераций»), не общий код с машиной. `rep stosd` на vCPU 0, две загрузки на vCPU 1; полный перебор всех расписаний: при count = 2500 (3 шага: 1024, 1024, 452) пара ячеек (1023, 1024) даёт `(1,0)`, пара (1022, 1023) — не даёт, пара (2047, 2048) даёт, пара (1024, 2047) — нет; count = 1024 (L) — один шаг, неделим, пустого шага после него нет; count = 0 — один шаг без эффекта; count = L+1 и 2L — два шага. Контрольные значения эталона с другим разрезом (1 итерация, неделимый `REP`) отличаются от эталона L = 1024 (тест). Драйвер не режет инструкцию: каждый вызов интерпретатора кончается на границе инструкции либо на RIP той же `REP` (счётчик `instructions_cut` = 0 на всём корпусе). Корпус S6-05 `REP` не содержит; REP-тесты — дополнение этого среза, не часть корпуса.

**S6-20: формы `LOCK` и `XCHG` корпуса исполняются.** До S6-20 голый `od_isa::execute::run` отказывал в формах корпуса (`LOCK INC dword [abs]`, `LOCK CMPXCHG`, `LOCK XADD dword`, `XCHG dword` с памятью): 7 fixture и формула LOCK-INC были `Unsupported` и в проверку не входили. S6-20 допускает эти формы в голом `run` (CPU_CONTRACT.md, «S6-20 — единый атомарный API LOCK»): каждая обращается к памяти одним вызовом `GuestMemory::atomic_rmw_integer`, который в режиме чередования неделим, потому что vCPU не переключается внутри инструкции. Результат: все 15 конечных fixture и формула LOCK-INC — **`equal`** полным перебором (эталон — поле fixture, не прогон; тест `the_whole_finite_corpus_is_exhaustive_and_equals_its_set`, `the_seven_lock_and_xchg_fixtures_are_equal_to_their_sets`, `the_lock_inc_counter_is_exact_under_every_schedule`), 0 запрещённых, 0 не покрытых; seed 0..=9999 — то же. Байты корпуса и инструкции не подменялись; список «заблокированных» из теста убран. Данные [`s6-19-litmus-interleaving.json`](../reports/data/stage6/s6-19-litmus-interleaving.json) переписаны прогоном-доказательством: 41 запись S6-19, не относившаяся к заблокированным, не изменилась (сравнено по содержимому), добавлено 27 записей (7 fixture и 7 размеров LOCK-INC × перебор и seed; N=4, K=4 — только seed). Остальные `LOCK`-формы (`LOCK DEC`/`OR`/`ADD`/`AND`/`BTS`) в голом `run` остаются `Unsupported` с опкодом, адресом и байтами в выводе раннера (CLI-тест `a_lock_form_outside_the_admitted_set_is_unsupported_with_the_instruction_and_its_bytes`); корпус их не использует. Не покрыто по-прежнему: параллельные host-потоки и x86-TSO-переупорядочение (S6-36); `LOOP` (интерпретатор его не исполняет: счётчики — тело корпуса, повторённое K раз, множество то же по формуле). Различие с настоящим x86, которое это не закрывает: `LOCK INC` здесь равен `INC` без `LOCK` по результату (обе неделимы); проверка LOCK как *барьера* x86-TSO — параллельный режим, не этот.

Команды: `cargo test -p od-machine --offline --lib -- litmus_interleaving`; прогон-доказательство `OD_S6_LITMUS_OUT=$PWD/docs/reports/data/stage6/s6-19-litmus-interleaving.json cargo test --release -p od-machine --offline --lib -- --ignored litmus_interleaving_proof`.

### S6-19b — публичный загрузчик и `od-runner --stage6-litmus`

Реализовано (2026-11-17, ожидает ревью): публичный API `od-machine` (`LitmusCase`, `LitmusPolicy`, `run_litmus`, `LitmusReport`, `LitmusVerdict`; модуль `crates/od-machine/src/litmus.rs`) поверх драйвера S6-19, флаг раннера (`tools/runner/src/stage6_litmus.rs`), CLI-тест `tools/runner/tests/stage6_litmus_cli.rs`. Контракт: [MACHINE_CONTRACT.md](../specs/MACHINE_CONTRACT.md#s6-19b--публичный-загрузчик-и-флаг---stage6-litmus). Планировщик (`SmpScheduler`, `QuantumPolicy`) наружу не открыт. Чекбокс S6-19 не отмечен (S6-19 готов к закрытию после ревью S6-20: литмусы `equal`).

```text
od-runner --stage6-litmus (--fixture FILE | --corpus DIR |
                           --formula FILE --counter ID --threads N --rounds K)
          [--mode exhaustive|seed] [--seeds A..=B] [--budget SCHEDULES]
          [--quantum Q | --schedule CPU:STEPS,...]
```

- `--fixture` — один конечный fixture `tests/fixtures/guest/stage6/litmus/*.json`; `--corpus` — все `*.json` каталога, кроме `counters-formula.json`, в порядке имён файлов; `--formula` — один счётчик (`LOCK-INC`, `UNLOCKED-INC`, `THREE-INSN-COUNTER`) для N потоков (1…14) и K кругов (тело корпуса K раз, `LOOP` не исполняется).
- По умолчанию `--mode exhaustive` (каждое расписание шагов; не больше `--budget` расписаний, по умолчанию 1 000 000, иначе код 3). `--mode seed`: seed из `--seeds` (по умолчанию `0..=9999`), каждый seed — случайный порядок шагов (SplitMix64), как S6-19. `--quantum Q` (круговой обход по Q шагов) и `--schedule` (явное расписание, по записи на шаг; число шагов каждого потока обязано совпасть с `steps_per_thread`) — один прогон одного случая; с `--corpus` не принимаются.
- Вывод — один JSON-объект на stdout (ключи упорядочены, одинаковые аргументы дают одинаковые байты; времени в выводе нет): `schema` (`od.stage6-litmus-run.v1`), `status` (`ok`/`failed`; при отказе `invalid_input`/`loader_error`/`guest_error` и `error{kind,detail}`), `harness`, `memory_model`, `difference_from_x86`, `exploration` (режим, диапазон seed или лимит), `summary` (`cases`, `equal`, `forbidden`, `not_covered`, `unsupported`, `incomplete`), `cases[]`. Случай: `id`, `source`, `fixture_sha256` (SHA-256 байтов файла fixture или формулы; детерминирован), `vcpus`, `threads` (asm), `steps_per_thread`, `complete_schedules`, `runs`, `observe`, `histogram[]` (`outcome`, `runs`, `frequency`, `first` — расписание (`"0011"`), seed или квант, `in_expected_set`), `expected_interleaving_instruction_atomic`, `forbidden_outcomes`, `not_covered_by_exploration`, `x86_tso_only_outcomes`, `instructions_cut`, `rep_slices`, `incomplete_runs`, `verdict`, `unsupported`.
- `verdict`: `equal` (наблюдённое = эталон fixture), `forbidden` (наблюдён исход вне эталона: дефект), `not_covered` (запрещённых нет, но эталонный исход не достигнут: в полном переборе — дефект, в seed и одиночном прогоне — только покрытие), `unsupported` (интерпретатор не исполняет форму; `unsupported{instruction, bytes, cpu, rip, opcode, reason}`; ничего не запускалось, `runs` = 0, гистограмма пуста; в эталон/формулу это не превращается), `incomplete` (поток не закончил или инструкция разрезана; проверяется выше `forbidden`: незавершённый прогон не считается запрещённым исходом, `forbidden_outcomes` при этом всё равно перечисляются, код выхода 5).
- Коды выхода: **0** — все случаи `equal`, `unsupported` (выбрано: известный пробел интерпретатора — не провал, но и не `equal`; после S6-20 на корпусе S6-05 таких случаев нет, путь проверяется копией fixture с формой `LOCK OR`; в `summary.unsupported` отдельно) либо `not_covered` в seed/одиночном прогоне; **3** — неверные аргументы или полный перебор выше `--budget`; **4** — fixture не прочитан или не разобран (в том числе неизвестный счётчик, N вне 1…14); **5** — `forbidden`, `incomplete`, `not_covered` в полном переборе, ошибка гостя.

- Ограничение `REP`: через публичный загрузчик fixture `REP` не нарезается: каждая инструкция — ровно один шаг (`steps = 1`), `steps_per_thread` это отражает. Нарезка `REP` с известным счётчиком на срезы по 1024 итерации (`L = 1024`) есть только в тестовом эталоне (`rep_case` в `litmus_interleaving.rs`), а не в загрузчике; в корпусе S6-05 `REP` нет.
- Состав вывода и §6: вывод `--stage6-litmus` несёт `fixture_sha256` каждого случая, режим и seed/расписание (`exploration`, `first`). Source revision, runner commit, toolchain, CPU/OS, время выполнения в нём нет (вывод побайтно повторяем), поэтому сам по себе он не evidence-файл §6: эти поля записывает отчёт, прикладывающий вывод.

Результат на корпусе S6-05 (`--corpus`, полный перебор): при срезе S6-19b было 8 `equal` и 7 `unsupported` (LOCK-CMPXCHG-MUTEX, LOCK-XADD, SB-LOCK, SB-LOCK-LOCAL, SB-LOCK-XADD, SB-XCHG, XCHG-MUTEX; LOCK-INC счётчик тоже); **после S6-20 — 15 fixture = 15 `equal`**, счётчик LOCK-INC тоже `equal`; ожидания CLI-теста `stage6_litmus_cli.rs` изменены с «unsupported = 7» на «equal = 15, unsupported = 0» (эталон читается из fixture, не из прогона; гистограммы и число расписаний всех 15 совпадают с `s6-19-litmus-interleaving.json`, проверено тестом). Команды: `cargo test -p od-runner --offline --test stage6_litmus_cli`; `od-runner --stage6-litmus --corpus tests/fixtures/guest/stage6/litmus`; `od-runner --stage6-litmus --fixture tests/fixtures/guest/stage6/litmus/sb.json --mode seed --seeds 0..=99`.

### S6-21 — гостевые доказательства запуска Linux SMP

Для одно-vCPU проверки seed соберите штатный `od-runner`. Пример использует зафиксированное ядро S6-10 из локального каталога этапа 4 и initramfs наблюдения, который собирается из исходников фикстуры.

```sh
profile=stage4a
proof_dir="work/stage6/s6-21-seeded-cli/$profile"
mkdir -p "$proof_dir"
python3 tools/build/build_stage6_smp_initramfs.py --out "$proof_dir/initramfs.cpio"
cargo build --release --locked --offline -p od-runner
kernel=work/stage4/linux-7.0.0-38.38/vmlinuz-7.0.0-38-generic
acpi=tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin
nonce=SMPUpObservation20261119
for attempt in first second; do
  report="$proof_dir/seed-$attempt.json"
  target/release/od-runner linux-direct --vmlinuz "$kernel" \
    --initramfs "$proof_dir/initramfs.cpio" --acpi-blob "$acpi" \
    --expected-vmlinuz-sha256 9f55ef7253055a9cf68f8a04323c93dead2fca617386d842fbde9ca7b178b27e \
    --expected-initramfs-sha256 5481c1399df1ebce44316a32a2c29857f596ac7e7b5e2c8af3e93cc4e21a276d \
    --expected-acpi-manifest-hash 2bd690ae0817705b1b128675f9628dd7212fee74ab97c88da3599a1ae8e398f2 \
    --ram-mib 128 --nonce "$nonce" --out "$report" --run --budget 2500000000 \
    --set-cpl0 --cpuid-profile "$profile" --user-mode --vcpus 1 --seed 24301
  python3 tools/build/build_stage6_smp_initramfs.py \
    --verify-report "$report" --nonce "$nonce" --expect-online 1
done
cmp "$proof_dir/seed-first.json" "$proof_dir/seed-second.json"
```

Для проверки с IOAPIC установите `profile=stage4a-apic` в этом рецепте. После проверки nonce и CPU0 строка таймера в гостевом `/proc/interrupts` должна содержать `IO-APIC` и `2-edge`. [Результаты с IOAPIC](../reports/STAGE_6_REPORT.md#s6-21-шаг-2--seed-с-ioapic).

Для прогона на двух vCPU используйте те же ядро, initramfs и хеш устройств. Фикстура S6-21 содержит две записи LAPIC. Замените в команде выше профиль, blob, число vCPU и nonce следующими значениями.

```sh
--cpuid-profile stage4a-apic --vcpus 2 --seed 24301 \
  --acpi-blob tests/fixtures/guest/stage6/smp-init/acpi-2-vcpus.bin --nonce smpBoot20261119A
```

В двухпроцессорном отчёте `interleaved_scheduler.cpu_stats` содержит retired, режим исполнения, RIP, RFLAGS, GPR, CR3 и EFER каждого vCPU. Поле `stop.retired` содержит суммарное число выполненных инструкций. При остановке на инструкции `interleaved_scheduler.terminal_vcpu` указывает процессор, которому принадлежат регистры и байты в `stop`.

Для зачётной проверки передайте raw JSON `linux-direct` и nonce, с которым стартовал гость.

```sh
python3 tools/build/build_stage6_smp_initramfs.py \
  --verify-report work/stage6/s6-21/boot.json \
  --nonce smpBoot20261119A --expect-online 2 --require-smp-evidence
```

Проверка требует один полный блок `/init`, online CPU `0–1`, согласованный заголовок `/proc/interrupts` и предшествующую строку ядра `smp: Brought up …, 2 CPUs`. На каждом online CPU хотя бы один из рядов `RES`, `CAL` или `TLB` должен иметь ненулевое значение. Формат строки ядра зафиксирован в S6-02. Предупреждения ядра и детерминизм повторных запусков входят в общую проверку S6-21. Для одно-vCPU наблюдения используйте только `--expect-online 1`. [План S6-21](../plans/STAGE_6.md#s6-21-запустить-второй-процессор-в-linux) · [Инвентарь S6-02](../reports/data/stage6/ap-start-inventory.json).

### S6-37 — LinuxDirect на одном рабочем потоке

Rust API `run_linux_direct_board_set_user_mode_threaded` проверяется с прежними ядром, ACPI и initramfs наблюдения из S6-21. Интеграционный тест требует точный бюджет в 2,5 млрд retired, один стартовавший и завершившийся поток, исполнение в CPL3, завершённые системные вызовы и полный блок гостевого `/init`.

```sh
cargo test --locked --offline -p od-machine --lib threaded_linux_
out=work/stage6/s6-37-threaded-linux
mkdir -p "$out"
for quantum in 1024 4096; do
  OD_S6_LINUX_QUANTUM="$quantum" cargo test --release --locked --offline -p od-machine \
    --test stage6_linux_seeded -- --ignored --exact a_vcpu_worker_reaches_real_linux_guest_init --nocapture \
    > "$out/boot-$quantum.log" 2>&1 || exit 1
  python3 - "$out/boot-$quantum.log" "$out/guest-$quantum.json" <<'PY'
import json
from pathlib import Path
import sys

transcript = Path(sys.argv[1]).read_text()
Path(sys.argv[2]).write_text(json.dumps({"stop": {"com1_text": transcript}}))
PY
  python3 tools/build/build_stage6_smp_initramfs.py \
    --verify-report "$out/guest-$quantum.json" --nonce SMPUpObservation20261119 --expect-online 1 \
    || exit 1
done
```

Проверки на 2 и 4 online vCPU и длительные гостевые нагрузки относятся к полной приёмке S6-37. [Отчёт запуска](../reports/STAGE_6_REPORT.md#s6-37--linuxdirect-на-рабочем-потоке-vcpu)

### S6-37 — LinuxDirect с IOAPIC

Запуск Stage4AApic использует те же ядро, initramfs и проверку `/init`, что и запуск с 8259. Гостевой `/proc/interrupts` также должен содержать строку таймера через IOAPIC.

```sh
cargo test --locked --offline -p od-machine --lib threaded_linux_ioapic_
out=work/stage6/s6-37-threaded-ioapic
mkdir -p "$out"
for quantum in 1024 4096; do
  OD_S6_LINUX_QUANTUM="$quantum" cargo test --release --locked --offline -p od-machine \
    --test stage6_linux_seeded -- --ignored --exact a_vcpu_worker_reaches_real_linux_guest_init_through_ioapic --nocapture \
    > "$out/boot-$quantum.log" 2>&1 || exit 1
done
```

Каждый лог проверяется тем же `build_stage6_smp_initramfs.py --verify-report`, что и в опыте с 8259 выше. [Результаты](../reports/STAGE_6_REPORT.md#s6-37--linuxdirect-с-ioapic-на-рабочем-потоке).

### S6-22 — проверка гостевых нагрузок

На macOS arm64 передайте `--rootfs-raw` рабочую копию принятого образа ext4. Runner открывает её на запись с эксклюзивной блокировкой, подключает к PCI/NVMe и добавляет `rdinit=/s6-smp-init`. Для каждого повтора создавайте новую копию образа и указывайте свободный путь отчёта. Установите `kernel`, `overlay` и `accepted_raw` в пути проверенных файлов, а `kernel_sha256` и `overlay_sha256` в ожидаемые хеши ядра и initramfs. Соберите release runner и задайте `budget` по длительности прогона.

```sh
mkdir -p work/stage6/s6-22-rootfs
run_dir=$(mktemp -d work/stage6/s6-22-rootfs/seeded.XXXXXX)
cp -c "$accepted_raw" "$run_dir/working.raw"
nonce=s6-workloads20261119
target/release/od-runner linux-direct \
  --vmlinuz "$kernel" --initramfs "$overlay" \
  --acpi-blob tests/fixtures/guest/stage6/smp-init/acpi-nvme-2-vcpus.bin \
  --expected-vmlinuz-sha256 "$kernel_sha256" --expected-initramfs-sha256 "$overlay_sha256" \
  --expected-acpi-manifest-hash a15686d043b7ce44650d6e847d1c965fdc5e5a38d8938a9c1eda120d5dfe81af \
  --ram-mib 128 --nonce "$nonce" --out "$run_dir/boot.json" \
  --rootfs-raw "$run_dir/working.raw" --run --budget "$budget" \
  --set-cpl0 --cpuid-profile stage4a-apic --user-mode --vcpus 2 --seed 24301
python3 tools/build/build_stage6_smp_rootfs_initramfs.py \
  --verify-report "$run_dir/boot.json" --nonce "$nonce" --expect-online 2
```

Проверка сверяет сводку ядра о поднятых CPU, SHA-256 исходника из гостя, nonce, пару процессоров и полный результат пяти нагрузок. Перед завершением гостевой сценарий читает `/sys/devices/system/cpu/online`. Для поточного запуска на четырёх vCPU используйте `--expect-online 4`. Длительность, повторяемость и расход памяти проверяются по критериям отдельного прогона.

### S6-38 — вычислительная нагрузка гостя

`s6_smp_workloads` делит заданное общее число блоков на соседние непересекающиеся диапазоны. Блок `i` начинает со значения `i + 1` и выполняет 256 раундов сдвигов, XOR и умножения `uint64_t`. Контрольная сумма складывает значения блоков по модулю 2⁶⁴. Один, два и четыре рабочих потока выполняют одинаковый объём работы и проверяют число обработанных блоков и ожидаемую контрольную сумму. Каждый поток закреплён на отдельном разрешённом CPU. [Гостевой исходник](../../guest/linux/tests/s6_smp_workloads.c).

Рецепт S6-22 собирает initramfs с обоими режимами. Для вычислительной проверки добавьте к параметрам ядра `od.s6_compute=<потоки>,<блоки>,<сумма>` вместе с `rdinit` и `od.nonce`. Без параметра `od.s6_compute` выполняются пять проверок S6-22. [Рецепт](../../tools/build/build_stage6_smp_rootfs_initramfs.py).

```text
rdinit=/s6-smp-init od.nonce=compute od.s6_compute=4,4097,ec4675d8578a4e14
```

Сверьте контрольную сумму для выбранного объёма до прогонов. Для 4097 блоков независимый расчёт выглядит так.

```python
blocks = 4097
mask = (1 << 64) - 1
checksum = 0
for seed in range(1, blocks + 1):
    value = seed
    for _ in range(256):
        value ^= value >> 12
        value ^= (value << 25) & mask
        value ^= value >> 27
        value = (value * 0x2545f4914f6cdd1d) & mask
    checksum = (checksum + value) & mask
print(f"{checksum:016x}")
```

Для многопоточной серии используйте 1, 2 и 4 потока при соответствующем числе vCPU. Для цены простаивающих vCPU используйте один рабочий поток при каждом числе vCPU. Границы сбора времени по часам хоста и CPU-времени VM задают `OD_S6_38_BEGIN` и `OD_S6_38_END`. Подготовка потоков и компиляция идут перед `BEGIN`. Интервал включает освобождение барьера, вычисления, проверку привязки и завершение потоков. Сверка суммы идёт после `END`. В сравнение входит прогон с `OD_S6_38_COMPLETE`, ожидаемым nonce и точным результатом. Объём 4097 блоков использован для проверки корректности. Для замеров отдельно калибруйте объём и применяйте правила повторов, разброса и окружения. [Методика замеров](BENCHMARKS.md#сравнительный-протокол) · [Локальная проверка](../reports/data/stage6/s6-38-compute-package.txt).

### S6-43a — cross-modifying code на двух vCPU с JIT

Проба проверяет два vCPU на одноинструкционном профиле JIT. A пишет код и публикует сигнал. B читает сигнал, в сериализующем варианте исполняет CPUID и затем запускает новую форму X. Эталон CPL0 использует те же байты гостя. [Отчёт](../reports/STAGE_6_S6-43a_2026-11-18.md).

| Проверка | Повторы | Что должно быть |
| --- | --- | --- |
| быстрая форма (`cargo test -p od-host --offline --lib -- s6_43a`) | три протокола: 600 сигналов (чередование) и 2000 сигналов (`run_host_threads`) на протокол | после сигнала B исполняет только новые байты (`stale 0`, `wrong 0`); регистры, RAM и записи данных равны интерпретатору; изменяемая инструкция исполнена нативно до записи и после неё в каждом цикле (иначе прогон не засчитывается), доля интерпретатора записана |
| длинная форма (release, `#[ignore]`) | 2 000 000 сигналов на протокол (6 000 000 всего); не меньше 10 000; число меняет `OD_S6_43A_CYCLES` | то же |

```text
cargo test --release -p od-host --offline --lib -- --ignored s6_43a_long_parallel_run --nocapture
```

Цепочка G-XMC использует правила порядка S6-40. В предыдущей пробе с CLD контроль `weak-host-ordering` дал 0 устаревших исходов, а мутация сверки поколения и байтов обнаруживалась host-wait протоколом. Эти исторические результаты приведены в отчёте.

## 6. Evidence and failure handling

Every run records: fixture and corpus SHA-256; test ID; source revision; runner commit; compiler/toolchain and flags; mode; CPU model, OS and thread count; iteration count; seed or explicit schedule; elapsed time; histogram; and exit/result status. Physical-x86 evidence goes in `docs/reports/data/stage6/`; temporary logs and binaries stay under ignored `work/stage6/`.

On a forbidden outcome, set/status mismatch, crash, or timeout: stop the gate, retain the raw output and exact input, and report the smallest reproducible fixture. Do not turn a forbidden result into noise, average it away, update expected outcomes from the run, or claim a pass after a later clean retry. A retry is additional evidence and must keep the first result.

The expected litmus corpus and its own TSO/SC enumerator are S6-05. S6-08's runner implementation, Linux regression tests, and full-corpus calibration are complete. The calibrated full physical run is S6-09; parallel-mode sensitivity is S6-36 (implemented, awaiting review; see "S6-36" above).
