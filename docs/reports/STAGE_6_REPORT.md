# Этап 6. Промежуточный отчёт

Статус отчёта: **промежуточный; этап не принят**. Сверка выполнена 2026-11-13; ветки `codex` и `main` имеют общий базовый commit `7f5546c`. На момент этой записи `main` — `dbb0cdb` (S4-38L9c принят); на 2026-11-15 S4-21 передан этапу 6 решением владельца, S4-22/S4-23 закрыты с переносом остатков. S6-04…09 приняты после независимых ревью. Полный этап 6 не принят.

## 1. Сводка задач

`not_started` означает, что задача не принята после ревью. `blocked` указывает внешние незакрытые предпосылки; эти блокировки наследуются зависимыми задачами по [плану S6](../plans/STAGE_6.md#6-задачи-и-минимальные-зависимости). S6-00 принят после независимого ревью; его evidence и статус приведены в таблице ниже.

Пометки во внешних блокерах (состояние на 2026-11-16 по [STAGE_4.md](../plans/STAGE_4.md)): `*` — задача этапа 4 принята (S4-09, S4-13, S4-14, S4-15, S4-16, S4-39 — с ограничениями; S4-31 — принят); S4-21 передан этапу 6 решением владельца 2026-11-15; S4-22/S4-23 закрыты с переносом остатков; S4-38 и S4-46 открыты. Статусы задач этапа 6 ниже не менялись.

Правило D5 о согласовании с линией S4-38 относится только к задачам и зависимым цепочкам, меняющим `od-isa`; оно не блокирует документные задачи S6-00…09 или S6-40.

| Задача | Статус | Внешние блокеры |
|---|---|---|
| S6-00 | `accepted` | независимое ревью пройдено; коммит [`38f4c81`](https://github.com/edelars/open_desktop/commit/38f4c81) |
| S6-01 | `accepted` | Методика прошла независимое ревью и закоммичена как [`5183584`](https://github.com/edelars/open_desktop/commit/5183584). Numeric candidates остаются provisional у задач-владельцев, которые ещё не измерили свои gates; S6-05 уже измерила complete-schedule threshold и seed range. S6-19 использует калиброванные S6-05 threshold/seed в режиме чередования. |
| S6-02 | `accepted` | Static byte inventory native INIT/SIPI trampoline получен из staged `vmlinuz` и повторно декодирован на физическом Intel x86 с GNU Binutils 2.46: все 149 trampoline rows и 2 common-entry инструкции совпали; физический повтор прошёл independent evidence review. Архитектурное ревью приняло статический Linux target path и D2 для документированного S6 2/4-vCPU профиля (`OpenDesktop`, APIC=1/X2APIC=0, physical-flat xAPIC, native INIT/SIPI). Текущая Stage4A реализация остаётся однопроцессорной/APIC-clear; runtime AP execution не заявляется, Windows CPU-06 отложен до трассы этапа 8. См. [`ap-start-inventory.json`](data/stage6/ap-start-inventory.json) и [x86 repeat report](data/stage6/s6-02-x86-reverification.md). |
| S6-03 | `accepted` | [SMP contract](../specs/SMP_CONTRACT.md) прошёл независимое архитектурное ревью `gpt-6.1-sol/high`. S4-25c Self-IPI сохранён; Fixed shorthand Self/All/All-but-self включены по S6-12. Время было открыто до S4-22/S4-23 (закрыты 2026-11-15 с переносом остатков); неизвестный-vendor Linux topology передан S6-18. Реализация SMP не заявляется. |
| S6-04 | `accepted` | Документный контракт модели памяти прошёл независимое ревью `gpt-6.1-sol/high`; [коммит `4d65fbc`](https://github.com/edelars/open_desktop/commit/4d65fbc). `git diff --check` чист; `check_layout.py` сообщает только 45 существующих битых ссылок на отсутствующие work evidence артефакты, новых S6 ошибок нет. Graphify обновлён на этот коммит. Документ фиксирует x86 fences, split accesses, A/D compare-and-retry и commit/callback granularity; реализация и выбор host primitive не входят. Отдельный descriptor side-effect owner добавлен в S6-26. См. [MEMORY_MODEL.md](../specs/MEMORY_MODEL.md). |
| S6-05 | `accepted` | Независимое ревью `gpt-6.1-sol/high` прошло без оставшихся замечаний. Приняты стандартно-библиотечный генератор, 15 конечных fixtures плюс один JSON-файл formula counter group и калибровка 1,000,000 complete schedules / seeds 0…9,999; см. [evidence](#s6-05--accepted-litmus-corpus-and-calibration) и [raw data](data/stage6/s6-05-enumerator-calibration.json). |
| S6-06 | `accepted` | Независимое ревью `gpt-6.1-sol/high` прошло без оставшихся замечаний. Приняты повторяемые измерения обычных access’ов и атомарных кандидатов, проверка emitted instructions и ограниченный выбор host-примитивов; сводка — [S6-06 measurements](data/stage6/s6-06-host-summary.md), сырые процессы 1–3 и disassembly сохранены рядом. |
| S6-07 | `accepted` | Независимое ревью `gpt-6.1-sol/high` прошло; [коммит `8b19597`](https://github.com/edelars/open_desktop/commit/8b19597); Graphify обновлён после inventory commit. Основной inventory: 233/233 совпадений, exact file:line coverage без пропусков; secondary ownership counts 25/218/7/6; все focused paths имеют owner, production assumptions без владельца не обнаружены. `git diff --check` чист; `check_layout.py` сообщает только 45 существующих work-evidence ошибок, S6 links clean. Код не менялся. |
| S6-08 | `accepted` | Независимые code/doc reviews пройдены; implementation commit `fa74b1c`; Linux tests 9 library + 258 runner, обычный fixture path прошёл; полный corpus 18×10,000 rounds/process ×3, 540,000 суммарных rounds, все гистограммы conservation-valid и observed outcomes допустимы; физический SB `00` зарегистрирован 1 раз. См. [calibration report](data/stage6/s6-08-native-calibration.md) и [raw JSON](data/stage6/s6-08-native-calibration.json). |
| S6-09 | `accepted` | Независимое evidence review `gpt-6.1-sol/high` пройдено; полный физический прогон завершён с exit 0: 18×500,000 rounds/process ×3 = 27,000,000; 54 process histograms and 18 aggregate histograms conserved; forbidden outcomes 0; SB `00` observed 19 times; summed child elapsed 519.593 s. См. [physical report](data/stage6/s6-09-native-physical.md) и [raw JSON](data/stage6/s6-09-native-physical.json). |
| S6-10 | `accepted` | S4-21 передан этапу 6 решением владельца 2026-11-15 |
| S6-11 | `accepted` | Реализовано 2026-11-15, ожидает независимого ревью; раздел S6-11 ниже |
| S6-12 | `in_progress` | Срез S6-12a (Fixed IPI, shorthand, физический адресат) реализован 2026-11-15, ожидает ревью; NMI открыт (blocked: D5/S4-38 для `od-isa/state.rs`); раздел S6-12a ниже |
| S6-13 | `blocked` | S4-21; согласование кода `od-isa` по D5/S4-38 |
| S6-14 | `in_progress` | S6-14a AP startup profile и S6-14b real-mode KVM oracle приняты по результатам отдельных ревью; см. ниже. Код S6-14c+ остаётся ограничен согласованием с S4-38 по D5 (S4-21 передан этапу 6). |
| S6-15 | `blocked` | S4-21 (передан этапу 6), S4-22, S4-23 (закрыты с переносом остатков) |
| S6-16 | `blocked` | S4-21 (передан этапу 6), S4-22, S4-23 (закрыты с переносом остатков) |
| S6-17 | `blocked` | S4-13, S4-14 (приняты с ограничениями), S4-21 (передан этапу 6), S4-22, S4-23 (закрыты с переносом остатков) |
| S6-18 | `blocked` | S4-21 (передан этапу 6), S4-31*, S7-02 |
| S6-19 | `blocked` | S4-21 (передан этапу 6), S4-22, S4-23 (закрыты с переносом остатков) (через S6-15) |
| S6-20 | `blocked` | S4-21 (передан этапу 6), S4-22, S4-23 (закрыты с переносом остатков) (через S6-15/S6-19); S4-38 для `od-isa` |
| S6-21 | `blocked` | S4-13*, S4-14*, S4-21 (передан этапу 6), S4-22, S4-23 (закрыты с переносом остатков), S4-31*, S4-39*, S4-38 (D5), S7-02 |
| S6-22 | `blocked` | S4-13*, S4-14*, S4-21 (передан этапу 6), S4-22, S4-23 (закрыты с переносом остатков), S4-31*, S4-39*, S4-38 (D5), S5-06, S5-07, S7-02 |
| S6-23 | `blocked` | S4-21 (передан этапу 6), S4-22, S4-23 (закрыты с переносом остатков); согласование S4-38 для зависимого S6-20 |
| S6-24 | `blocked` | S4-21 (передан этапу 6), S4-22, S4-23 (закрыты с переносом остатков); согласование S4-38 для цепочки S6-13/S6-14c |
| S6-25 | `blocked` | S4-09 (принят с ограничениями 2026-11-16), S4-21 (передан этапу 6), S4-22, S4-23 (закрыты с переносом остатков) (через S6-20) |
| S6-26 | `accepted` | Документный контракт восьми descriptor-writing paths прошёл независимое архитектурное ревью `gpt-6.1-sol/high`; замечаний не осталось. Реализация документации — коммит `422d0a3`; `git diff --check` чист; `check_layout.py` сообщает 45 прежних broken links в work evidence, новых в изменённых docs нет. SMP реализация остаётся gate для S6-30/S6-31. |
| S6-30 | `blocked` | S4-21 (передан этапу 6), S4-22, S4-23 (закрыты с переносом остатков); S4-38 для кода `od-isa` |
| S6-31 | `blocked` | S4-09*, S4-21 (передан этапу 6), S4-22, S4-23 (закрыты с переносом остатков); S4-38 для кода `od-isa` |
| S6-32 | `blocked` | S4-21 (передан этапу 6), S4-22, S4-23 (закрыты с переносом остатков); S4-38 для кода `od-isa` |
| S6-33 | `blocked` | S4-21 (передан этапу 6), S4-22, S4-23 (закрыты с переносом остатков) |
| S6-34 | `blocked` | S4-09*, S4-21 (передан этапу 6), S4-22, S4-23 (закрыты с переносом остатков); S4-38 для кода `od-isa` |
| S6-35 | `blocked` | S4-09*, S4-21 (передан этапу 6), S4-22, S4-23 (закрыты с переносом остатков), S9-24, S9-25 |
| S6-36 | `blocked` | S4-09*, S4-21 (передан этапу 6), S4-22, S4-23 (закрыты с переносом остатков) |
| S6-37 | `blocked` | S4-09*, S4-13*, S4-14*, S4-21 (передан этапу 6), S4-22, S4-23 (закрыты с переносом остатков), S4-31*, S4-39*, S5-06, S5-07, S7-02 |
| S6-38 | `blocked` | S4-09*, S4-13*, S4-14*, S4-21 (передан этапу 6), S4-22, S4-23 (закрыты с переносом остатков), S4-31*, S4-39*, S5-06, S5-07, S7-02 |
| S6-40 | `blocked` | S3-25, S3-28, S4-15*, S4-16* |
| S6-41 | `blocked` | S3-25, S3-28, S4-09*, S4-15*, S4-16*, S4-21 (передан этапу 6), S4-22, S4-23 (закрыты с переносом остатков) |
| S6-42 | `blocked` | S3-25, S3-28, S4-09*, S4-15*, S4-16*, S4-21 (передан этапу 6), S4-22, S4-23 (закрыты с переносом остатков) |
| S6-43 | `blocked` | S3-25, S3-28, S4-09*, S4-15*, S4-16*, S4-21 (передан этапу 6), S4-22, S4-23 (закрыты с переносом остатков), S5-12 |
| S6-50 | `blocked` | блокировки S6-38 |
| S6-51 | `blocked` | S4-13*, S4-14*, S4-21 (передан этапу 6), S4-22, S4-23 (закрыты с переносом остатков), S4-31*, S4-39*, S7-02, S7-13 |
| S6-60 | `blocked` | блокировки S6-35, S6-38 и S6-50 |
| S6-61 | `blocked` | блокировки S6-60 и S6-51; gate G7 не принят |

## 2. Границы ответственности и handoff

| Область | Владелец и граница |
|---|---|
| Архитектура инструкций, декодер и однопроцессорная семантика | Этап 2 / линия ISA. Этап 6 добавляет многопроцессорные эффекты и использует существующие контракты; изменения `od-isa` во время S4-38 требуют согласования и одного автора на файл (D5). |
| Перевод блоков и их жизненный цикл | Этап 3 владеет JIT cache, eviction и safepoints (S3-25/S3-28); этап 6 добавляет межпроцессорную инвалидацию только после этих зависимостей (S6-40…S6-43). |
| vCPU, LAPIC/IPI, планировщик и x86-TSO | Этап 6. Сначала режим чередования в одном host-потоке; параллельные vCPU и host memory primitives — только после gates плана. Это планируемая работа, не реализованная возможность. |
| Перевод адресов и доступы к памяти | Этап 4 владеет текущим MMU/TLB и завершает необходимые предпосылки; S6-17/S6-25 не подменяют S4-13/S4-14 и полный S4-09. |
| Linux boot path и сквозные гостевые сценарии | Этапы 4 и 5 сохраняют свои владельческие файлы и стенд. Этап 6 подключает SMP после S4-39 и S5-06/S5-07; файлы `linux-direct` менять только по согласованию, указанному в плане. |
| MADT, firmware handoff и число CPU | Этап 7 владеет `madt.rs` и handoff block (S7-13/S7-02); S6-18 меняет их только по согласованию с владельцами, одним автором и с сохранением поведения одного vCPU. |
| Настройка числа CPU в продукте | S6-51 меняет ограничение `vcpuCount == 1` в `ControlV2.swift` и отображение статуса vCPU; схема bundle не меняется. Остальные настройки и продуктовый контракт остаются у этапа 11. |
| Состояние для снимка | По умолчанию D1: этап 6 предоставляет остановку vCPU и состояние vCPU; S9-28 владеет полным barrier vCPU/I/O, epoch и публикацией, а формат задают S9-24/S9-25. |
| Windows, количество vCPU и x2APIC | По умолчанию D3: два vCPU должны быть функционально доступны; утверждение о поддержке 2–4 требует измерений S6-38. По D4 x2APIC не входит без подтверждённого требования S6-02. Windows Setup остаётся у этапа 7/8. |

## 3. Вопросы владельцу и принятые временные значения

**Решения владельца (2026-11-15, после defaults):** (1) D7 — время гостя виртуальное (шаги × 1 нс, фиксированные тики, пропуск простоя), host monotonic не виден гостю; побайтная воспроизводимость по seed сохраняется. (2) D5 — правки `crates/od-isa/src/state.rs` разрешены для NMI (S6-12), INIT/SIPI (S6-13) и S6-14c, при условии проверки активных worktree S4-38 перед каждым срезом.

**Решение владельца (2026-11-15):** значения по умолчанию из плана приняты для всех вопросов D1–D9 (ответ пользователя в сессии, режим «по умолчанию»). Для D5 это означает: код `od-isa` меняется только по согласованию с линией S4-38, один автор на файл; правки машины S6-10…S6-12a уже не блокируются S4-21 (его остатки переданы этапу 6). Срезы NMI (S6-12), INIT/SIPI (S6-13) и S6-14c по-прежнему заблокированы до согласования с S4-38.

На 2026-11-13 ответов владельца на D1–D9 не было. Владелец 2026-11-15 принял значения по умолчанию из плана для всех D1–D9 (см. строку 78). Ниже — эти значения по умолчанию:

| Вопрос | Действующее значение по умолчанию |
|---|---|
| D1 — остановка vCPU и snapshot barrier | Этап 6 отвечает за остановку vCPU и их состояние; S9-28 — за полный barrier с I/O, epoch и публикацией; формат остаётся у S9-24/S9-25. |
| D2 — запуск AP | Принято по S6-02: native INIT/SIPI с trampoline для документированного OpenDesktop physical-flat xAPIC target; runtime AP execution не заявляется. |
| D3 — число vCPU | Два функционально доступны; «поддержано» и 4 vCPU определяются только измерениями S6-38; VM bundle не меняется. |
| D4 — x2APIC | Не входит, пока S6-02 не подтвердит требование выбранной ОС. |
| D5 — код до S4-21 и правки `od-isa` при S4-38 | Машинный код S6-10+ ждёт S4-21; код `od-isa` согласуется с владельцем S4-38, один автор на файл. До этого доступны S6-00…09 и S6-14a/b. |
| D6 — `CMPXCHG8B`/`CMPXCHG16B` | Однопроцессорная семантика — линия этапа 2; этап 6 даёт атомарный примитив, когда его потребует S6-31. |
| D7 — воспроизводимость seed | Побайтная воспроизводимость, если S4-22 использует счётчик инструкций; иначе временное поведение ослабляется отдельным решением. Этап 6 свою модель времени не вводит. |
| D8 — split lock и обычные невыровненные доступы | `LOCK` всегда атомарен; до S6-34 неподдерживаемые split lock дают явную ошибку со счётчиком. Обычные невыровненные доступы могут быть видны по частям; усиление — только по замерам. |
| D9 — режим по умолчанию | До G5/G8 только режим чередования; более одного vCPU помечается «функционально доступно, без ускорения». После gates параллельный режим по умолчанию, чередование остаётся для диагностики. |

Решение владельца 2026-11-15: остатки S4-21 переданы этапу 6, поэтому S6-10 разблокирован (см. запись S6-10 в плане этапа). Это решение не отвечает на D1–D9.

## 4. Документные расхождения и передача

- Roadmap задаёт 2–4 vCPU, а `VM_BUNDLE.md` — 1–32 с неуказанным основанием. По умолчанию D3 схема bundle не меняется; поддержанный максимум определяется замерами S6-38.
- План этапа 7 относит SMP и x2APIC к этапу 6, хотя roadmap не задаёт x2APIC. По умолчанию D4 x2APIC исключён до подтверждения S6-02.
- `CPU_CONTRACT.md` описывает текущий direct Long64 BSP handoff; он не задаёт AP startup. Принятый D2/S6-02 target использует для AP native INIT/SIPI с real-mode trampoline, а S6-14 должен описать AP mode-transition profile. Гипотеза CPU-06 для Windows остаётся непроверенной до трассы этапа 8.
- В [отчёте этапа 9](STAGE_9_REPORT.md) строка S9-26 ошибочно называет UEFI зависимостью этапов 6/8; UEFI относится к этапу 7. Подтверждённая документальная ошибка зарегистрирована как [ISSUE-086](../KNOWN_ISSUES.md#issue-086-s9-26-wrong-uefi-stage-reference). Отчёт этапа 9 не менялся. **Передача владельцу S9:** исправить ссылку S9-26 на этап 7 в `STAGE_9_REPORT.md`; до этого ISSUE-086 остаётся открытой.

## 5. Проверка этого среза

S6-00: независимое ревью diff и контрактных границ агентом Codex `gpt-6.1-sol` / `high` пройдено; локальные ссылки в изменённых документах — 0 broken; `git diff --check` чист. `python3 tools/check_layout.py` сравнен с архивом базового `HEAD` `7f5546c`: базово exit 1 / 46 ошибок, сейчас exit 1 / 45, новых ошибок 0. Оставшиеся диагностики относятся к старым ссылкам на отсутствующие ignored Stage 3 evidence в `work/`.

S6-01: методика прошла независимое ревью `gpt-6.1-sol` / `high` по формату, источникам, SB/SC, двум частям sensitivity и матрице 2/4 vCPU; `git diff --check`, JSON-пример и локальные ссылки проверены. Значения S6-05 в `SMP.md` теперь основаны на измерениях; остальные владельческие numeric gates остаются provisional до своих замеров. S6-19 потребляет принятые S6-05 threshold/seed только в режиме чередования, а parallel budget принадлежит S6-36. Исполнение этапа 6, запуск гостя, CPU-тесты и сборка не выполнялись: документ не заявляет поведения SMP.

### S6-05 — accepted litmus corpus and calibration

The actual `interleaving_instruction_atomic` enumerator was measured in complete order-preserving schedule units. On a two-thread, 12-shared-load-per-thread workload (`C(24,12) = 2,704,156` total schedules), three runs stopped at exactly 1,000,000 completed paths: 3,851,199 expanded states, 149 unique states, 12 pending states, 27,607,040–27,705,344 bytes peak RSS, and 10.279–10.564 seconds (2.72% relative range). This passes the 2-GiB / 120-second budget, so the 1,000,000 complete-schedule threshold is retained and no lower candidate was needed.

Three fresh-worker runs of seeds 0…9,999 each completed 10,000 actual instruction-atomic paths, expanded 250,000 states, retained 451,163 unique-state entries across seeds with 201,163 pending states, used 27,607,040–27,770,880 bytes peak RSS, and took 1.097–1.116 seconds (1.77% relative range). Seeds 0…99 were run as an excluded warm-up. The calibration method uses deterministic `random.Random(seed)` successor shuffling; it is a reproducibility budget, not a confidence claim. Finite fixture counts are all below 1,000,000 and were exhaustively enumerated without sampling. The independent `gpt-6.1-sol/high` review passed with no remaining findings; all 11 focused tests and deterministic regeneration checks passed. S6-05 is accepted. Guest execution and native/physical-x86 validation remain downstream work. See [raw calibration data](data/stage6/s6-05-enumerator-calibration.json) and the [fixture README](../../tests/fixtures/guest/stage6/litmus/README.md).

### S6-06 — accepted host primitive measurements

Three sequential correctness-checked release processes on the M3 Max completed the full 138-row matrix: 120 ordinary access comparisons and 18 RMW comparisons. Every row retained 20 paired samples; the formal floor is 200 ms per side, while the harness required 300 ms internally. Minimum side totals were 301,715,542 ns, 300,010,582 ns, and 301,684,207 ns. The benchmark records 128-byte cache-line placement, host/build fingerprints, source and binary hashes, raw samples, and emitted-instruction disassembly. Aggregate ratio counts and spreads are defined in the [measurement summary](data/stage6/s6-06-host-summary.md); raw processes are [1](data/stage6/s6-06-host-1.json), [2](data/stage6/s6-06-host-2.json), and [3](data/stage6/s6-06-host-3.json).

The measured constrained choices are Rust `SeqCst` same-width ordinary accesses; standard-library `SeqCst` RMW for 1/2/4/8-byte classes, which emitted `LDADDALB/H/W/X`; and the LL/SC paired path for 16-byte RMW. These are host primitive selections for later implementation, not a parallel RAM backend. The measurements do not waive S6-04’s alignment, single-width, ownership, or guest x86-TSO requirements. The independent `gpt-6.1-sol/high` review passed with no remaining findings; `cargo fmt --all -- --check`, workspace offline check, and `git diff --check` passed.

### S6-08 — accepted Linux native litmus runner

The separate Linux x86_64 threaded mode ran all 18 S6-05 execution cases (15 finite plus three formula identities) on the physical Intel i5-3210M Mini. At 10,000 rounds/process × three fresh processes, all 54 process histograms conserved their requested count, every observed outcome belonged to the declared x86-TSO set, and SB `00` appeared once in the 30,000-round SB sample. `LOCK-INC` produced only 4; unlocked INC produced 2 and 4. Linux `cargo test -p od-x86-runner --locked` passed 9 library and 258 runner tests, and the existing ordinary `--fixture` command completed the S0-09 MOV immediate fixture with the expected RAX/RIP.

The 54 throughput samples ranged 47,160–98,115 rounds/s (median 83,592; pooled 79,023). This rejects the provisional 10,000,000-round/process candidate under the current 30-second child CPU/wall limits: at the slowest observed sample it projects to at least 212 seconds. The calibrated S6-09 base was 500,000 rounds/test/process ×3 (1,500,000 per test, 27,000,000 total for 18 cases), with 341.7 seconds projected from individual samples and 572.5 seconds if all processes ran at the slowest calibration rate. The full base run completed and is recorded below. Code and documentation reviews passed; implementation commit is `fa74b1c`. The remote archive was a tracked snapshot of that commit; a synthetic clean snapshot revision is recorded in the raw runner metadata and its origin marker is documented in the calibration report. See [calibration summary](data/stage6/s6-08-native-calibration.md), [raw histogram JSON](data/stage6/s6-08-native-calibration.json), [compressed Linux test log](data/stage6/s6-08-linux-cargo-test.log.gz), and [ordinary fixture result](data/stage6/s6-08-ordinary-fixture.json).

### S6-09 — accepted full physical x86 corpus run

On the same physical Intel i5-3210M, the full calibrated 18-case corpus ran for 500,000 iterations/process ×3, totaling 27,000,000 rounds. All 54 process histograms and 18 aggregate histograms conserved their counts; every observed outcome was allowed; there were no failed or timed-out child processes. SB `00` appeared 19 times in 1,500,000 SB rounds. `LOCK-INC` returned exactly 4 in all rounds; unlocked INC and the three-instruction counter showed permitted lost updates. Child runtime samples were 7.83–12.29 seconds (all within 30 seconds), with 519.592934988 seconds summed across cases. Throughput samples ranged 40,678–63,885 rounds/s (median 52,440; pooled 51,964). Independent `gpt-6.1-sol/high` evidence review passed. See [physical report](data/stage6/s6-09-native-physical.md), [raw output](data/stage6/s6-09-native-physical.json), and [compressed stdout](data/stage6/s6-09-native-physical.stdout.json.gz).

### S6-02 — промежуточный evidence-срез

S4-35 Linux artifact найден в корневом checkout `main` в ignored-каталоге `work/stage4/linux-7.0.0-38.38/`; тот же комплект есть ещё в 10 из 127 зарегистрированных worktree. Поиск не обнаружил готового S6-02 inventory до этой задачи. По запросу владельца каталог скопирован в `codex/work/stage4/linux-7.0.0-38.38/`; повторная сверка всех 10 файлов с S4-35 manifest дала 20/20 совпадений между `main` и `codex`, без изменений в `main`. Повторные SHA-256 сверены с [S4-35 manifest](data/stage4/s4-35-linux-artifact.json): `vmlinuz` `9f55ef72…b27e`, `vmlinux` `ccaf5904…5034f`, `vmlinux-debug` `7e460f04…9d1f`, config `6cb18717…d32d`, архив исходников из Ubuntu source package `1610ed02…6222`. Config включает `CONFIG_SMP=y`, `CONFIG_X86_X2APIC=y`, `CONFIG_ACPI_MADT_WAKEUP=y`, `CONFIG_AMD_MEM_ENCRYPT=y`, `CONFIG_ACPI_SLEEP=y`, `CONFIG_HOTPLUG_CPU=y`. При этом S4-35 явно оставляет `authenticated=false`: совпадение файловых хешей не подтверждает подписанную source-to-binary цепочку.

По Linux `7.0.0-38.38` source tree, `CONFIG_ACPI_MADT_WAKEUP=y` позволяет обработать MADT type `0x10` Multiprocessor Wakeup и установить mailbox-вариант `wakeup_secondary_cpu_64`. `do_boot_cpu()` сначала проверяет `wakeup_secondary_cpu_64`, затем `wakeup_secondary_cpu`, и только без обоих callbacks вызывает native `wakeup_secondary_cpu_via_init`; отсутствие type `0x10` само по себе выбор не определяет. Native xAPIC path посылает INIT assert, ждёт завершения ICR, выдерживает выбранную `cpu_init_udelay`, посылает INIT deassert, а при integrated APIC — до двух SIPI на физический APIC ID. Для современного Intel/AMD/Hygon CPU default INIT delay равен нулю; каждая SIPI получает 10 мкс до проверки доставки и ещё 10 мкс после ожидания ICR. Старый APIC/CPU путь использует 10 мс INIT delay и 300/200 мкс после SIPI; boot option `cpu_init_udelay=` может переопределить значение.

Для D2 принято значение по умолчанию: native INIT/SIPI с 16-bit real-mode entry, далее protected 32-bit и long mode. Независимое архитектурное ревью приняло этот выбор для явно заданного S6 target profile: reference CPU `OpenDesktop ` с APIC=1, X2APIC=0 и hypervisor-present=0; SMP расширение должно выставить 2/4 vCPU с APIC IDs 0…N−1, matching MADT type 0 entries и xAPIC APIC_BASE state. При этом сам reference profile и текущая Stage4A плата однопроцессорные: Stage4A имеет APIC=0, один CPU и одну MADT Local APIC entry. Это решение фиксирует будущий target contract, не утверждает реализацию или runtime выбор на текущей плате. MADT MP Wakeup type `0x10` не включается в target; Linux source-path при нём был бы другим, а отсутствие type `0x10` само по себе не исключает прочие APIC/platform callbacks. Windows CPU-06 остаётся отдельной непроверенной гипотезой до трассы этапа 8.

Дополненный статический инвентарь закрепляет выбор Linux APIC driver и условные режимы: для x86_64 default — physical-flat xAPIC с физическими APIC ID; 2/4 vCPU с ID 0–3 адресуются без x2APIC. `CONFIG_X86_X2APIC=y` означает только наличие кода: фактический выбор зависит от CPUID/APIC_BASE, interrupt remapping и гипервизора; x2APIC может использовать physical или cluster destination, а его регистры и 64-битный ICR доступны через MSR. Маршрутизация IOAPIC/MSI получает destination mode выбранного драйвера; targeted startup IPI использует физический AP ID, а не shorthand. Зафиксированы shorthand для all/all-but-self/self и Linux topology CPUID leaf fallback цепочки Intel/AMD; guest leaf values остаются неизвестны без закреплённого CPU profile. Ожидаемые MADT type 0/type 9 записи и processor namespace описаны в JSON.

Статически просмотрены и пути NMI: обычный AP startup NMI не посылает; остановка при reboot/shutdown применяет NMI fallback после таймаута; при panic `crash_smp_send_stop()` с NMI shootdown используется, когда включён `crash_kexec_post_notifiers`, а обычная ветвь вызывает `smp_send_stop()` и может дойти до его условного NMI fallback; restart имеет NMI shootdown, KGDB — NMI roundup, а perf hard-lockup detector собран при включённой конфигурации. Исполнение этих ветвей зависит от runtime и не подтверждено запуском гостя. Поэтому прежние пробелы о destination/shorthand/topology/NMI закрыты только на уровне исходного кода, а фактический режим и работа путей в госте остаются открытыми.

Побайтный inventory native INIT/SIPI trampoline декодирован из фактического staged boot image. Zstandard payload `vmlinuz` по offset `0x52cc` распакован в ELF, SHA-256 которого совпадает с manifest `vmlinux`; `.init.data` извлечённого ELF побайтно совпадает с `.init.data` `vmlinux-debug`, а `real_mode_blob` имеет ожидаемые 25 188 байт и SHA-256 `d48c472d…c80573f`. Закреплённый GNU Binutils 2.46 сначала выполнил cross-disassembly в pinned Ubuntu `linux/arm64` image, затем повторил decode на физическом Linux x86_64 Mini (Intel Core i5-3210M). Совпали все 119 rows с 16-bit decode, 29 с 32-bit decode, одна с 64-bit decode и обе common-entry инструкции. Полное 16-bit окно выводит 122 инструкции: три дополнительные инструкции декодируют 12 alignment bytes `[0x1074,0x1080)`, которые не являются строками trampoline path в JSON. Фактический execution mode у transition far jumps указан отдельно. JSON содержит смещения, байты и мнемоники, включая общий `verify_cpu`, SEV-ES обход проверки CPU, изменения CR0/CR3/CR4 и EFER, плюс указатель перехода на `secondary_startup_64` или `secondary_startup_64_no_verify`. Подробности физического повтора, команды и raw stdout — в [S6-02 x86 repeat report](data/stage6/s6-02-x86-reverification.md). Это linked bytes образа: `setup_real_mode()` применяет сегментные и линейные relocations при копировании blob в runtime low memory; фактический адрес назначения не закреплён S4-35. SHA/section byte match подтверждает идентичность staged файлов, но не подписанную upstream source-to-binary цепочку.

S6-02 принят по статическому inventory и Linux target-path evidence; независимое архитектурное ревью (`gpt-6.1-sol/high`) приняло target profile и D2. Ответа владельца на D2 нет, поэтому применено значение по умолчанию: native INIT/SIPI, real16 → protected32 → long64. Для reference `od-x86-64-v1-neutral-v0.1` и его минимального SMP расширения (vendor `OpenDesktop `, CPUID APIC=1/X2APIC=0/hypervisor=0, APIC_BASE в xAPIC state, 2 или 4 APIC IDs `0…N−1`, matching MADT type 0 entries, без `apic=`/`nolapic` или `cpu_init_udelay=` overrides) статическая цепочка Linux однозначна: нет MADT MP Wakeup type `0x10` или активного platform override, поэтому `apic_flat_64` выбирает physical-flat xAPIC с physical destination, а `smpboot` доходит до native `wakeup_secondary_cpu_via_init`. Нейтральный vendor выбирает INIT delay 10,000 μs; при ненулевом delay после SIPI — 300 μs до ICR idle и ещё 200 μs перед ESR (если нет `cpu_init_udelay=` override). Reference CPUID max leaves (basic `1`, extended `0x80000008`) не включают topology leaves `0x1f`, `0x80000026` или `0x0b`. Дополнительно, pinned `topology_common.c:158–177` не имеет parser branch для unknown vendor `OpenDesktop`: Linux читает CPUID.1 EBX initial APIC ID и вычисляет logical-count shift, но не вызывает ни `cpu_parse_topology_ext()`, ни `parse_legacy()`; topology domains остаются defaults для одного logical CPU с нулевыми shifts. Поэтому согласованные CPUID.1 EBX, APIC IDs и MADT — требование платы, но Linux package/core classification для N=2/4 этим source path не подтверждена; разрешить topology domains в S6-18. Текущая исполняемая Stage4A плата этому профилю ещё не соответствует: CPUID APIC=0, EBX сообщает один CPU/APIC ID 0, MADT содержит один enabled LAPIC. Это принятый target-анализ, не runtime-утверждение; текущая Stage4A реализация остаётся однопроцессорной/APIC-clear, и AP execution не заявляется. CPU-06 для Windows остаётся гипотезой по правилу самой задачи: публичный источник о выборе INIT/SIPI и real-mode trampoline для обычной Windows x64 не найден; сверить трассой на этапе 8. Эта проверка отложена до этапа 8 и не является новым gate S6-02. ACPI 6.6 требует Processor Local APIC MADT entry и соответствующий processor device object в DSDT (§5.2.12.2; для x2APIC — §5.2.12.12); объект объявляется под `\_SB` с `_HID=ACPI0007` и matching `_UID` (§8.4). Текущая прошивка проекта размещает namespace в DSDT. Microsoft namespace guidance, найденное в этом срезе, прямо ограничивает область SoC-based platforms.

Начальный evidence-срез, расширение APIC/NMI/topology и исходный pinned-image byte inventory ранее независимо проверил `gpt-6.1-sol` / `high` — pass. Физический x86 повтор прошёл независимое evidence review `gpt-6.1-sol` / `high` без оставшихся замечаний; отдельное архитектурное ревью (`gpt-6.1-sol` / `high`) приняло статический target path и D2 для ограниченного S6-профиля. S6-02 принят как provenance, source-path и linked-byte evidence; текущий Stage4A runtime, AP execution, APIC_BASE wiring и 2/4-vCPU реализация не подтверждены. Windows CPU-06 остаётся гипотезой с проверкой на этапе 8 согласно плану. В этом срезе проверены JSON parse, input, metadata и stdout SHA-256, строгая сверка 149 trampoline rows плюс двух common-entry адресов/байтов; `git diff --check` чист. `check_layout.py` сообщает только 45 существующих ошибок ссылок на отсутствующие work evidence, новых S6 ошибок нет.


### S6-03 — SMP contract (accepted document)

Создан [контракт SMP](../specs/SMP_CONTRACT.md) с неизменным набором vCPU, индексом/APIC ID, BSP/AP state transitions, INIT reset state по Intel SDM 253668-093US, INIT/SIPI, закрытым подмножеством физических xAPIC ICR кодировок, IOAPIC/MSI и NMI. Pinned Linux evidence требует targeted physical Fixed, Fixed all-but-self и targeted/all-but-self NMI; INIT/SIPI — targeted physical. Matrix также включает Fixed Self/All/All-but-self из S6-12 и сохраняет уже принятый S4-25c Self encoding. Неиспользуемые доставки и режимы явно отклоняются. ICR Level/Trigger кодировки для Fixed/NMI/SIPI повторяют pinned Linux source. `cpu_count`, CPUID.1 EBX и MADT согласуются; package/core topology для неизвестного vendor `OpenDesktop` не заявляется и передана S6-18. Переменная времени, TSC и all-HLT behavior открыты до полного принятия S4-22/S4-23. Safe-point остановка и pending event preservation записаны по D1; I/O barrier остаётся S9-28. Независимое архитектурное ревью `gpt-6.1-sol/high` прошло без замечаний. Код многопроцессорной платы и guest runtime не выполнялись.

### S6-14a — AP startup trampoline profile

`CPU_CONTRACT.md` now defines the AP-only profile for the statically selected
Ubuntu Generic `amd64` 7.0.0-38.38 native xAPIC INIT/SIPI path: SIPI real-16
entry, `verify_cpu`, the successful real→protected transition, flat protected
32-bit setup, paging/EFER transition, and the long-mode indirect handoff. It
states real-mode operand/address defaults and the observed prefix subset,
segment roles, code/data selectors, conditional target branches, and explicit
unsupported boundary. The separate SEV-ES entry and AMD SME branch are recorded
as present in the linked inventory but outside the selected target path. The
first two common-entry bytes are retained as handoff evidence only.

`FIRMWARE_BOOT.md` distinguishes its direct Long64 firmware entry from the
separate bounded AP startup path. Independent architecture review
`gpt-6.1-sol/high` passed without remaining findings. The reviewer checked the
tracked byte-decode evidence, selected branch paths, SIPI state, transition
modes, real-mode addressing rules, and profile exclusions. This is a
document-only result, committed as [`67a201b`](https://github.com/edelars/open_desktop/commit/67a201b):
it does not implement instruction execution, claim AP runtime, or settle the
physical relocation base. The pinned staged image has no signed source-to-binary
chain; Windows CPU-06 remains unverified.

### S6-14b — KVM real-mode oracle

`tools/kvm-runner/kvm_runner.c` now has an opt-in `--real-mode CS:IP` entry.
It loads the image at the real-mode linear address, uses the requested CS:IP,
applies the INIT CR0 preservation rule to the newly created KVM vCPU state,
and reports read-back GPRs, segment registers and hidden attributes, GDTR/IDTR,
and CR0/2/3/4/8 plus EFER. The existing Long64 setup and output path remain
unchanged. Pending userspace I/O is completed with one capability-checked
`immediate_exit` re-entry before a final snapshot; unsupported exits or failed
completion/capture report an error and omit final state. This follows the
[Linux KVM API completion rule](https://www.kernel.org/doc/html/latest/virt/kvm/api.html#kvm-exit-io-kvm-exit-mmio-kvm-exit-osi-kvm-exit-papr-kvm-exit-xen-kvm-exit-epr-kvm-exit-hypercall-kvm-exit-tdx-kvm-exit-x86-rdmsr-and-kvm-exit-x86-wrmsr).

The updated runner built on the Intel Mini with `gcc -O2 -Wall -Wextra -Werror -std=c11`. KVM API 12 and `/dev/kvm` were available. A 16-byte real-mode probe
at `0700:0000` executed register moves, port output, and HLT; read-back values
matched the probe. A second run stopped at budget 1 immediately after `OUT`;
the completion re-entry returned a valid snapshot at RIP 9 after the I/O
instruction, with UART byte `R`. `run_count + intr_count` was 2 because that
single completion re-entry is allowed after the main-loop budget is reached.
The committed-source Long64 runner and updated runner produced byte-identical
JSON for the same probe. Host/build details are in
[`host-info.txt`](data/stage6/s6-14b-kvm/host-info.txt); full output is in
[`realmode-hlt.json`](data/stage6/s6-14b-kvm/realmode-hlt.json),
[`realmode-io-budget.json`](data/stage6/s6-14b-kvm/realmode-io-budget.json),
[`long64-before.json`](data/stage6/s6-14b-kvm/long64-before.json), and
[`long64-after.json`](data/stage6/s6-14b-kvm/long64-after.json).

Independent code review `gpt-6.1-sol/high` passed after fixes for INIT EDX,
pending KVM I/O, unavailable `KVM_CAP_IMMEDIATE_EXIT`, error snapshots,
descriptor-table state, and preservation of the Long64 behavior. This verifies
that this KVM host executes a small real-mode guest and that the oracle reports
state. It does not execute the Linux AP trampoline, implement AP execution, or
accept S6-14 as a whole; S6-14c and later remain separate work.

### S6-10 — вынос состояния vCPU (реализовано, ожидает ревью)

Основание: решением владельца 2026-11-15 остатки S4-21 переданы этапу 6, S6-10 разблокирован. Чекбокс в плане не отмечен: его ставит ревью.

**Что сделано.**
- `crates/od-machine/src/vcpu.rs` (новый, зарегистрирован в `lib.rs`): `Vcpu { index, state: LongModeState, guest_cr2, guest_efer, guest_star, guest_lstar, guest_cstar, guest_sysenter_cs, guest_sysenter_esp, guest_sysenter_eip, guest_syscall_mask, guest_kernel_gs_base, guest_fp }`, конструктор `Vcpu::new(index, state)`, константа `BOOT_VCPU_INDEX = 0`. Поля и их значения по умолчанию перенесены с платы без изменения.
- `LoadedFirmwareBoard`: одиночные поля заменены на `vcpus: Vec<Vcpu>` с одним элементом; оба конструктора платы создают его через `Vcpu::new(BOOT_VCPU_INDEX, …)`. Все пути (`state`, `state_mut`, `guest_*`/`set_guest_*`, `run_boot`, CPL0-цикл) берут элемент `BOOT_VCPU_INDEX`. Публичные сигнатуры сохранены, кроме того, что `guest_*` и `guest_fp` перестали быть `const fn` (индексация `Vec` в `const fn` недоступна); потребителей в константном контексте нет.
- Сверх списка плана в `Vcpu` вынесены гостевые `CR2` и FP-состояние: это тоже архитектурные ячейки процессора, переносимые между запусками так же, как MSR. На плате оставлены `pci_config_address` (порт `0xCF8`, не процессорный) и `page_table_root`. `page_table_root` — это гостевой CR3: его записывает гость (`MOV CR3`), поэтому он принадлежит процессору и должен переехать в `Vcpu` не позже S6-15; в этом срезе он оставлен на плате, потому что план S6-10 его не перечисляет.
- Тесты: `vcpu::tests::new_vcpu_has_the_given_index_and_initial_cells` и `firmware_board::tests::board_has_exactly_one_vcpu_with_index_zero_and_state_is_read_through_it` (один элемент, индекс 0, `state()` и `set_guest_lstar` работают через элемент 0). Ожидания существующих тестов не менялись.
- Поле `Vcpu::index` в нетестовой сборке не читается; оно помечено `cfg_attr(not(test), allow(dead_code))` до S6-11.

**Проверки (команды из корня worktree).**
- `cargo check --workspace --all-targets --offline` — прошла.
- `cargo test -p od-machine --offline -- --test-threads=1` — 0 failed; lib: 1153 passed, 6 ignored; остальные наборы (8, 10, 2, 12, 14, 8 passed) без отказов. Полный набор запущен один раз на коде (последующая правка — только атрибут `allow(dead_code)` и комментарий).
- `cargo fmt -p od-machine -- --check` — прошла. `cargo fmt --all -- --check` не проходит из-за двух файлов вне среза (`crates/od-host/src/storage_abi_v1.rs`, `tools/image-tool/tests/cli.rs`); они не менялись в этом срезе.

**Контрольный прогон до/после.** Release-раннер собран `cargo build --release --locked --offline -p od-runner` на базовом коммите `90e620a` (до) и на изменённом дереве (после). Команда (входы и хеши из раздела «Прогон» в [`MACHINE.md`](../testing/MACHINE.md), артефакты из основного checkout `work/stage4/`, ACPI blob из `tests/fixtures/guest/stage4/acpi_blob/`):

```text
od-runner linux-direct --vmlinuz work/stage4/linux-7.0.0-38.38/vmlinuz-7.0.0-38-generic \
  --initramfs work/stage4/initramfs/initramfs.cpio \
  --acpi-blob tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin --nonce s6-10-ctl \
  --expected-vmlinuz-sha256 9f55ef7253055a9cf68f8a04323c93dead2fca617386d842fbde9ca7b178b27e \
  --expected-initramfs-sha256 ee9183a4eded3c16a8c6a7a81c6d5bb79be8bb023f793f7cd61694ea2bd3e588 \
  --expected-acpi-manifest-hash 2bd690ae0817705b1b128675f9628dd7212fee74ab97c88da3599a1ae8e398f2 \
  --ram-mib 128 --out <json> --run --budget 300000000 --set-cpl0 --cpuid-profile stage4a --user-mode
```

Результат: оба запуска exit 0, `stop.exit_kind = budget_exhausted`, `retired = 300000000`; JSON-отчёты (число инструкций, конечные регистры, `rip`, гостевые MSR-ячейки, `memory_sha256` — хеши загруженных образов ядра и initramfs, а не гостевой RAM после прогона, `virtual_ns`) и stdout совпали побайтно (sha256 JSON `09997821afbb332cc135ae5184476f2c9d4cc437af84f76cefcc4d2814524cb6`). Первый одиночный замер: wall 116 s до и 114 s после (дальше заменён серией ниже).

**Замер темпа (серия, чередование сторон).** Тот же запуск (`--budget 300000000`, release), порядок: до, после, до, после, до, после; «до» — раннер, собранный на `90e620a`, «после» — раннер с изменениями S6-10 (собран до правок только комментариев и текста). Время — wall, секунды, разрешение 1 s (`date +%s`); сырые значения в scratchpad исполнителя, не в репозитории. Процессор и нагрузка машины не контролировались, кроме отсутствия параллельных сборок в момент серии. JSON каждого из 6 запусков совпал побайтно с контрольным (sha256 начинается с `09997821afbb332c`).

| № | Сторона | Wall, s |
|---|---|---|
| 1 | до | 116 |
| 2 | после | 116 |
| 3 | до | 115 |
| 4 | после | 115 |
| 5 | до | 115 |
| 6 | после | 115 |

Итог: до — min 115 / max 116 (разброс 1 s), после — min 115 / max 116 (разброс 1 s); средние 115.33 s и 115.33 s. Разница между сторонами 0 s и укладывается в разброс (1 s) и в разрешение таймера. Вывод ограничен: 3 запуска на сторону при разрешении 1 s способны выявить лишь заметные (порядка процента и больше) отличия темпа.

**Ограничения и открытое.**
- Хеш гостевой RAM раннер не выдаёт; поле в этом срезе не добавлялось (нет явного решения).
- При бюджете 300000000 COM1 пуст, поэтому байты COM1 в сравнении тривиальны; полный зачётный бюджет (3000000000, ≈10 минут) не запускался.
- Замер темпа: серия 3+3 с чередованием, разрешение 1 s, разница 0 s в пределах разброса 1 s; более тонкие отличия темпа (доли процента) серия не различает.
- `page_table_root` — гостевой CR3, записывается гостем; он остаётся на плате и должен переехать в `Vcpu` не позже S6-15 (иначе второй vCPU разделит CR3 с первым). `pci_config_address` — регистр порта `0xCF8` общей платы, остаётся на плате.
- Graphify: в worktree графа не было; `GRAPHIFY_MAX_WORKERS=8 graphify update .` построил его заново (46374 узла, 128844 ребра); перед правками `graphify query` не выполнялся (графа не было), код читался по `rg`/строкам.

### S6-11 — LAPIC на каждый vCPU (реализовано, ожидает ревью)

Чекбокс в плане не отмечен: его ставит ревью. Контракт: [`MACHINE_CONTRACT.md`](../specs/MACHINE_CONTRACT.md#lapic-на-каждый-vcpu-s6-11), источник требований — [`SMP_CONTRACT.md`](../specs/SMP_CONTRACT.md) §2, §6.

**Что сделано.**
- `apic_pair.rs`: `PairState { local, route, timer }` заменён на `PairState { cpus: Vec<CpuApic>, route: Route }`, `CpuApic { local, timer }`. Один `Mutex` сохранён (level-запрос связывает LAPIC-адресата и Remote IRR). Методы, не называющие процессор (`level`, `set_line`, `set_line2`, `set_low`, `set_low2`), действуют на LAPIC, который выбирает поле адресата записи; остальные (`edge`, `eoi`, `set_enabled`, `reset_local`) принимают индекс LAPIC. В `Route` добавлено `dest: [u8; 24]`; новый `set_dest` отказывает несуществующий APIC ID до изменений и применяет к смене адресата level-входа правила смены вектора.
- `local_apic.rs`: `LocalApic::with_cpus(n)`, `with_shared_timers(clock, hz, n)` (старые конструкторы — один LAPIC); `LocalApicHandle(Arc, cpu)`, `handle_for`, `for_cpu`, `cpu_count`, `cpu_index`; чтение ID как `index << 24`; `BusDevice::read_from_cpu`/`write_from_cpu` для окна `0xFEE0_0000`.
- `io_apic.rs`: старший dword записи хранит и возвращает physical APIC ID (раньше принимался только 0).
- `bus.rs`: два метода трейта `BusDevice` с реализацией по умолчанию (`read_from_cpu`, `write_from_cpu`), параметр `vcpu` у `MachineBus::access_tracked`. `paged_memory_adapter.rs`: `AdapterBus::Machine(bus, vcpu)`, метод `vcpu()`, обе точки вызова `access_tracked` передают его; `From<&mut MachineBus>` даёт процессор 0.
- `vcpu.rs`: `MAX_VCPUS = 4`, `vcpu_set(boot_state, count)`. `firmware_board.rs`: `FirmwareBoardConfig::vcpu_count` (по умолчанию 1; вне `1..=4` — `Config`), `LoadedFirmwareBoard::vcpu_count()`, `local_apic_of(index)`.
- Тесты (17 новых, без изменения ожиданий старых; в существующих тестах только механическая замена `.local`/`.timer` на `.cpus[0].…`): `local_apic::tests` — разные APIC ID, отказ несуществующего vCPU, регистры по процессорам, запрос/EOI/выключение внутри своего LAPIC, таймер одного vCPU не меняет состояние другого (полное сравнение `CpuApic`), сброс одного LAPIC; `io_apic::tests` — edge INTIN4 и INTIN2 на APIC ID 1 поднимают запрос только у vCPU 1, level на APIC ID 1 (Remote IRR снимает только EOI vCPU 1, смена адресата при ожидающем EOI отказана атомарно), несуществующий ID отказан без изменений, при одном vCPU допустим только 0, logical/Lowest Priority/NMI отказаны, конфликт вектора с LVT Timer только в LAPIC-адресате; `vcpu::tests` — набор vCPU, `vcpu_count` платы, границы 1..=4; `paged_memory_adapter::tests` — доступ через адаптер vCPU 1 и vCPU 0 попадает в свой LAPIC (проверено мутацией: подмена индекса на 0 в `bus.rs` роняет тест).

**Проверки (однократно на финальном коде).**
- `cargo check --workspace --all-targets --offline` — прошла.
- `cargo test -p od-machine --offline -- --test-threads=1` — 0 failed; lib: 1170 passed (1153 прежних + 17 новых), 6 ignored; остальные наборы без отказов.
- `cargo fmt -p od-machine -- --check` — прошла. `cargo fmt --all -- --check` падает на двух файлах вне среза (`crates/od-host/src/storage_abi_v1.rs`, `tools/image-tool/tests/cli.rs`), они не менялись.
- Контрольный прогон `od-runner linux-direct … --run --budget 300000000 --set-cpl0 --cpuid-profile stage4a --user-mode` (команда и входы как в S6-10; release): «до» — раннер на коммите `2c49f60`, «после» — раннер с изменениями S6-11. Оба exit 0; выходной JSON совпал побайтно (sha256 `f86ef2b657a5ba42885aacf3cf36c7daf96e550d3d829cab0341e7fbb936228b`), stdout совпал побайтно. Замер темпа не делался (в срезе он не требуется; запуски шли параллельно).

**Ограничения и открытое.**
- Исполняется только процессор 0; AP существуют как `Vcpu` с `LongModeState::default()` и своим LAPIC. Путь CPL0-цикла (`SetStepMmio`) по-прежнему обслуживает только ECAM, поэтому доступ vCPU 1 к окну LAPIC проверен через paged-адаптер, а не через цикл `run_boot`; диагностические `IdentityMemoryAdapter`/`DiagnosticVcpuSession` и `read_physical`/`write_physical` — процессор 0.
- Блок передачи управления, ACPI blob (MADT/DSDT) и CPUID описывают один процессор при любом `vcpu_count` (S6-15/S7-02, SMP_CONTRACT §2).
- Предел `vcpu_count = 4` взят из SMP_CONTRACT §2; там же сказано, что `N = 4` поддерживается после gate S6-38, плата это не проверяет.
- `reset_checked`/`reset_pair` сбрасывают LAPIC своего процессора и маршруты IOAPIC; общая семантика сброса платы с несколькими LAPIC уточняется с S6-13.
- Риск для S6-13 (решить до S6-13): гость, пишущий broadcast `0xFF` в старший dword маскированного входа IOAPIC, получит фатальную ошибку MMIO, потому что `set_dest` отказывает APIC ID, которого нет среди vCPU (Linux при `clear_IO_APIC`/маскировании может писать такое значение).
- Риск для S6-15 (решить в S6-15): при N > 1 смена affinity level-входа записью «старший dword, потом младший» отказывается `set_dest`, если вход ждёт EOI или линия высока (SMP affinity в Linux меняет адресат уже работающего входа); отказ — фатальная ошибка MMIO.
- Риск для S6-13/S6-15 (провести до запуска AP): индекс vCPU не проводится от `Vcpu` на рабочих путях MMIO — `SetStepMmio` (CPL0-цикл), `read_physical`/`write_physical` и `AdapterBus::RamMmio` всегда обслуживают процессор 0; индекс несёт только `AdapterBus::Machine(bus, vcpu)`, и пока его создаёт лишь тест.
- Graphify: перед правками `graphify query` не выполнялся (в worktree графа не было); после правок `GRAPHIFY_MAX_WORKERS=8 graphify update .` выполнен (46426 узлов, 129065 рёбер).

### S6-12a — доставка IPI (Fixed, shorthand, физический адресат)

Реализовано 2026-11-15. Независимое ревью Claude Opus / high (2026-11-16): блокеров и major нет, срез принят как частичная реализация S6-12. Владелец 2026-11-16 подтвердил: (а) ожидание теста Self `(0,0)` заменено, (б) два консервативных решения (отказ всей IPI при выключенном LAPIC адресата или level-резервировании вектора) — записаны в SMP_CONTRACT §5. Замечания ревью без блокеров: тест level-резервирования при shorthand не добавлен (необязательно); гостевой WRMSR `IA32_APIC_BASE` (EN=0) — открытый вопрос для S6-13/S6-15 (Linux All-but-self к выключенному CPU станет ошибкой устройства). Статус S6-12 — `in_progress`, NMI открыт. Чекбокс в плане не отмечен. Контракт: [`MACHINE_CONTRACT.md`](../specs/MACHINE_CONTRACT.md#доставка-ipi-s6-12a); источник — матрица [`SMP_CONTRACT.md`](../specs/SMP_CONTRACT.md) §5.

**Что сделано.**
- `local_apic.rs`: запись в `ICR_LOW` разобрана функцией `decode_icr` по матрице: Fixed physical без shorthand `(0,0)`, Self `(0,0)`/`(1,0)`, All including self `(0,0)`, All-but-self `(0,0)`, вектор `0x10–0xFF`. Остальное (зарезервированные биты, режимы кроме Fixed, вектор `< 0x10`, logical без shorthand, несуществующий APIC ID включая `0xFF`, неуказанные пары Level/Trigger) — `UnsupportedConfiguration` до изменения состояния. Delivery Status всегда Idle, в хранимое значение не попадает.
- `apic_pair.rs`: `PairState::edge_all` — один запрос в IRR каждого адресата, всё или ничего (сначала проверка всех адресатов, потом запись); `edge` разложен на `edge_refused` + `raise_edge`, поведение `edge` не менялось.
- Нового замка, потока или параметра числа процессоров нет; используется существующий `vcpu_count` и один `Mutex` пары.
- `crates/od-isa/src/` не тронут.

**По строкам матрицы SMP_CONTRACT §5.**
| Строка | Состояние |
|---|---|
| Fixed, physical, без shorthand, `(0,0)` | доставляется |
| Fixed, Self, `(0,0)` или `(1,0)` | доставляется |
| Fixed, All including self, `(0,0)` | доставляется |
| Fixed, All-but-self, `(0,0)` | доставляется (при одном vCPU — никому, запись принимается) |
| NMI physical / All-but-self | **не реализовано**, явная ошибка устройства; NMI blocked: D5 / S4-38 для `od-isa/state.rs` |
| INIT assert/deassert | **не реализовано**, явная ошибка устройства (S6-13) |
| SIPI | **не реализовано**, явная ошибка устройства (S6-13) |
| все прочие значения | явная ошибка устройства, состояние всех vCPU прежнее |

**Тесты** (`local_apic.rs`, 12 новых; двенадцатый — физический адресат с выключенным LAPIC, добавлен после ревью): запись ICR vCPU 0 делает вектор ожидающим на vCPU 1 и не на vCPU 0 (и acknowledge у адресата); граничные векторы `0x10…0xFF` физического адресата; Self обоими кодированиями с двух отправителей; All и All-but-self для трёх vCPU и одного vCPU; Delivery Status; неверные пары Level/Trigger для каждой строки; каждая ошибочная клетка (logical, несуществующий APIC ID, вектор `< 0x10`, режимы 1/2/3/7, NMI/INIT/SIPI вне и внутри строк матрицы, зарезервированные биты) без изменения состояния; отказ одного адресата отменяет всю IPI; прочие регистры LAPIC IPI не меняет.

**Изменено ожидание существующего теста.** В `icr_rejections_are_atomic_and_global_reset_clears_registers` убран случай `0x4_0000 | 0x60` (Self с `(0,0)`, «deassert»): матрица SMP_CONTRACT §5 требует принимать Self с `(0,0)` (кодировка Linux `__prepare_ICR`). Остальные ожидания self-IPI, в том числе `icr_self_ipi_delivery_retrigger_and_priority`, не менялись.

**Проверки (однократно на финальном коде).**
- `cargo fmt -p od-machine -- --check` — прошла. `cargo fmt --all -- --check` падает на двух файлах вне среза (`crates/od-host/src/storage_abi_v1.rs`, `tools/image-tool/tests/cli.rs`), они не менялись.
- `cargo check --workspace --all-targets --offline` — прошла.
- `cargo test -p od-machine --offline -- --test-threads=1` — 0 failed; lib: 1181 passed (1170 прежних + 11 новых), 6 ignored; остальные наборы без отказов. После ревью добавлен ещё один тест и исправлены комментарии и тексты: полный набор не перезапускался, прогнан модуль `local_apic`.
- Контрольный прогон `linux-direct` не делался: меняется только путь записи ICR, Linux на одном vCPU пишет в ICR редко, а другие пути не затронуты; покрытие — unit-тесты.
- Graphify: в worktree графа не было, `graphify query` до правок не выполнялся; после правок `GRAPHIFY_MAX_WORKERS=8 graphify update .` выполнен (46464 узла, 129180 рёбер).

**Ограничения и открытое.**
- NMI (пункт 2 плана S6-12) не сделан: блокировка NMI до `IRET` требует правки `crates/od-isa/src/state.rs`, линия S4-38 там активна, решение D5 владельца не получено. Тест «второй NMI доставляется после `IRET`, третий теряется» открыт.
- Доставка в IRR есть, исполнение запроса адресатом на своей границе — нет: AP не запускаются (S6-13), `deliver_pending_irq` обслуживает процессор 0. Путь MMIO цикла по-прежнему не несёт индекс vCPU (риск S6-11 остаётся).
- IPI на несуществующий APIC ID (в том числе broadcast `0xFF`) отклоняется ошибкой устройства, как и в IOAPIC; на реальном железе broadcast `0xFF` допустим, матрица его не содержит. Риск для S6-13/S6-15: если Linux при остановке или старте шлёт physical broadcast, потребуется строка матрицы.
- Адресат, у которого LAPIC глобально выключен, отклоняет всю IPI (как `edge` раньше); software-disabled LAPIC запрос принимает в IRR.
- Для владельца (а): изменено ожидание существующего теста `icr_rejections_are_atomic_and_global_reset_clears_registers` (Self с `(0,0)` теперь принимается). Это следствие SMP_CONTRACT §5 / S6-03; владельцу подтвердить, что оно заменяет пункт плана «существующие тесты self-IPI проходят без изменений» (остальные ожидания self-IPI не менялись).
- Для владельца (б): если любой адресат (физический, All или All-but-self) имеет глобально выключенный LAPIC или вектор зарезервирован level-входом IOAPIC в его LAPIC, отказывает вся IPI без изменения состояния. Это консервативное решение исполнителя, в §5 его нет; тесты: отказ одного адресата при shorthand и при физическом адресате.

### S6-07 — inventory однопроцессорных допущений (accepted)

Код не менялся. Перед поиском выполнен Graphify query. Основной поиск выполнен ровно командой из плана; raw hit count **233**, число строк таблицы **233**. Сверка `file:line` охватывает весь raw hit set. Дополнительные буквальные поиски по `crates/od-machine/src` и `crates/od-host/src`: `&mut PhysicalMemory` — **25**, `&mut [u8]` — **218**, `Rc<` — **7**, `RefCell<` — **6**; overlap с основным поиском — **0** строк для каждого. Эти counts — raw `rg` результаты по src paths, включая inline unit-test blocks; отдельные `tests/` files отмечены как fixture evidence.

Коды и одинаковая для каждого hit rationale:

- `A-LOCK`: один serial instruction path делает read→write без межпроцессорного вмешательства; переключение только после инструкции сохраняет семантику (`✓`). Параллельный поток может войти между чтением и записью (`✗`). S6-20 заменяет последовательность единым guest atomic API и решает DS substitute; host primitive остаётся S6-31.
- `A-RAM`: указанный caller contract требует private synchronous RAM без concurrent observer; синхронный instruction call с boundary switching его сохраняет (`✓`), параллельный общий backing нарушает это предусловие (`✗`). S6-30 вводит shared RAM и удаляет borrowed API.
- `A-fence`: текущие fence no-op допустимы в однопроцессорном serial profile (`✓`); параллельно они не задают x86 TSO order (`✗`). S6-32 реализует fence и обычный-access order.
- `A-APIC`: the current firmware/device state has one LAPIC or APIC ID 0. Instruction-boundary serialization prevents concurrent mutation but does not provide distinct per-vCPU LAPIC state, IDs, or target routing; this assumption is false for multi-vCPU interleaving and parallel modes (`✗`/`✗`). Per-vCPU LAPIC/IPI and firmware count belong to S6-11/S6-12/S6-18; MADT edits are coordinated with S7-13.
- `A-vCPU`: bounded pump/session admission and CPU state require one vCPU. A single host-thread scheduler does not create the distinct state/session needed for multi-vCPU interleaving; separate threads need it too (`✗`/`✗`). S6-10 introduces per-vCPU state; S6-15 introduces the interleaving scheduler.
- `A-clock`: уникальная clock capability корректна при одном scheduler, который продвигает общую шкалу; параллельный режим также корректен только если один coordinator остаётся единственным advancer (`✓*`), а не каждый vCPU двигает её независимо. Это закреплено S6-15/S6-34.
- `A-memory`: текущая GuestMemory/atomic contract даёт serial effects при whole-instruction scheduling (`✓`), но не гарантирует concurrent shared-RAM serialization/order (`✗`); общая RAM — S6-30, LOCK API — S6-20, host primitive — S6-31.
- `T-LOCK` и `T-APIC`: тесты подтверждают только serial one-CPU fixture/current MADT expectations; строки не утверждают production parallel behavior, поэтому оба режима `—`. LOCK oracle tests сопровождает S6-20, APIC expectations — S6-18/S7-13. `T-fixture`: только bounded test/safety-fixture facts, без product runtime assumption; оба режима `—` и owner отсутствует именно потому, что это не product contract.
- `F-evidence`: benchmark/build/contract-profile evidence, а не guest runtime; режимы `—`, owner отсутствует. `F-contract`: факт текущего documented API scope (serial private RAM; no SMP ordering claim); `✓/✗` фиксируют, где сам текущий контракт применим, а не заявляют реализацию parallel; последующая работа принадлежит перечисленным S6 tasks. `F-status`: README сообщает, что SMP пока не реализован; `✓` означает факт для текущего baseline, `✗` — второй vCPU сейчас не предоставлен; реализация распределена по S6-10…18.

`✓` — сохраняется при переключении только на границе инструкции; `✗` — не гарантируется параллельным режимом; `✓*` — сохраняется при одном общем scheduler/advancer; `—` — режим к тестовому или документному факту неприменим. У каждой строки заполнены оба режима и owner/причина без owner.

| file:line | факт / допущение | чередование | параллельно | владелец S6 / основание |
|---|---|---:|---:|---|
| `crates/od-jit/src/memory_dispatch.rs:914` | `A-RAM` | ✓ | ✗ | S6-30; JIT lifecycle S6-40…42 |
| `crates/od-jit/src/memory_dispatch.rs:1425` | `A-RAM` | ✓ | ✗ | S6-30; JIT lifecycle S6-40…42 |
| `firmware/uefi/src/acpi/madt/tests.rs:82` | `T-APIC` | — | — | S6-18/S7-13 regression |
| `firmware/uefi/src/acpi/madt/tests.rs:118` | `T-APIC` | — | — | S6-18/S7-13 regression |
| `firmware/uefi/src/acpi/madt/tests.rs:429` | `T-APIC` | — | — | S6-18/S7-13 regression |
| `firmware/uefi/src/acpi/madt.rs:1` | `A-APIC` | ✗ | ✗ | S6-11/S6-12/S6-18; MADT also needs S7-13 |
| `firmware/uefi/src/acpi/madt.rs:22` | `A-APIC` | ✗ | ✗ | S6-11/S6-12/S6-18; MADT also needs S7-13 |
| `firmware/uefi/src/acpi/madt.rs:56` | `A-APIC` | ✗ | ✗ | S6-11/S6-12/S6-18; MADT also needs S7-13 |
| `firmware/uefi/src/acpi/madt.rs:57` | `A-APIC` | ✗ | ✗ | S6-11/S6-12/S6-18; MADT also needs S7-13 |
| `firmware/uefi/src/acpi/madt.rs:62` | `A-APIC` | ✗ | ✗ | S6-11/S6-12/S6-18; MADT also needs S7-13 |
| `firmware/uefi/src/acpi/madt.rs:298` | `A-APIC` | ✗ | ✗ | S6-11/S6-12/S6-18; MADT also needs S7-13 |
| `crates/od-jit/src/memory_load_v2.rs:873` | `A-RAM` | ✓ | ✗ | S6-30; JIT lifecycle S6-40…42 |
| `tools/build/stage5_isa_profile_forms.json:193` | `F-evidence` | — | — | — (scope/evidence only) |
| `tools/build/stage5_isa_profile_forms.json:680` | `F-evidence` | — | — | — (scope/evidence only) |
| `crates/od-jit/src/memory_cache.rs:273` | `A-RAM` | ✓ | ✗ | S6-30; JIT lifecycle S6-40…42 |
| `crates/od-jit/src/memory_cache.rs:512` | `A-RAM` | ✓ | ✗ | S6-30; JIT lifecycle S6-40…42 |
| `crates/od-jit/src/memory_cache.rs:618` | `A-RAM` | ✓ | ✗ | S6-30; JIT lifecycle S6-40…42 |
| `tools/build/s4_38l2_contract_status.py:50` | `F-evidence` | — | — | — (scope/evidence only) |
| `crates/od-isa/src/memory.rs:258` | `A-memory` | ✓ | ✗ | S6-20/S6-30 |
| `crates/od-isa/src/memory.rs:275` | `A-memory` | ✓ | ✗ | S6-20/S6-30 |
| `crates/od-isa/src/memory.rs:276` | `A-RAM` | ✓ | ✗ | S6-30 |
| `crates/od-isa/src/memory.rs:296` | `A-memory` | ✓ | ✗ | S6-20/S6-30 |
| `crates/od-isa/src/memory.rs:312` | `A-RAM` | ✓ | ✗ | S6-30 |
| `crates/od-isa/src/memory.rs:317` | `A-memory` | ✓ | ✗ | S6-20/S6-30 |
| `crates/od-machine/src/diagnostic_vcpu.rs:397` | `A-vCPU` | ✗ | ✗ | S6-10/S6-15; distinct CPU state is required even in an interleaving scheduler |
| `crates/od-machine/src/diagnostic_vcpu.rs:671` | `A-vCPU` | ✗ | ✗ | S6-10/S6-15; distinct CPU state is required even in an interleaving scheduler |
| `crates/od-machine/src/diagnostic_vcpu.rs:728` | `A-vCPU` | ✗ | ✗ | S6-10/S6-15; distinct CPU state is required even in an interleaving scheduler |
| `crates/od-machine/src/diagnostic_vcpu.rs:796` | `A-vCPU` | ✗ | ✗ | S6-10/S6-15; distinct CPU state is required even in an interleaving scheduler |
| `crates/od-machine/src/diagnostic_vcpu.rs:798` | `A-vCPU` | ✗ | ✗ | S6-10/S6-15; distinct CPU state is required even in an interleaving scheduler |
| `crates/od-machine/src/diagnostic_vcpu.rs:897` | `A-vCPU` | ✗ | ✗ | S6-10/S6-15; distinct CPU state is required even in an interleaving scheduler |
| `crates/od-machine/src/virtual_clock.rs:78` | `A-clock` | ✓ | ✓* | S6-15/S6-34; one scheduler owner |
| `crates/od-host/tests/memory_corpus.rs:713` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/tests/memory_corpus.rs:773` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/tests/memory_corpus.rs:874` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/tests/memory_corpus.rs:910` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/tests/memory_corpus.rs:1008` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:658` | `A-RAM` | ✓ | ✗ | S6-30 |
| `crates/od-host/src/lib.rs:703` | `A-RAM` | ✓ | ✗ | S6-30 |
| `crates/od-host/src/lib.rs:776` | `A-RAM` | ✓ | ✗ | S6-30 |
| `crates/od-host/src/lib.rs:1103` | `A-RAM` | ✓ | ✗ | S6-30 |
| `crates/od-host/src/lib.rs:1106` | `A-RAM` | ✓ | ✗ | S6-30 |
| `crates/od-host/src/lib.rs:1126` | `A-RAM` | ✓ | ✗ | S6-30 |
| `crates/od-host/src/lib.rs:2518` | `A-RAM` | ✓ | ✗ | S6-30 |
| `crates/od-host/src/lib.rs:2788` | `A-RAM` | ✓ | ✗ | S6-30 |
| `crates/od-host/src/lib.rs:24144` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:29223` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:29646` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:30025` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:30381` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:31971` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:34583` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:36867` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:39270` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:39299` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:39440` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:39708` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:39847` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:39866` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:40527` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:40851` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:41158` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:43317` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:43351` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:43460` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:43477` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:43533` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:44084` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:44118` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:44655` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:44694` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:44926` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:44991` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:45039` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:47097` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:47147` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:47242` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:47358` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:47435` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:47483` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:47611` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:47659` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:53677` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:53832` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:56548` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:59074` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:59090` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:59173` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:59211` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:59535` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:59572` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:60473` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:60542` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:60626` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:60733` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:61137` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:61175` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:61283` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:61320` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:61378` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:61559` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:61827` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:61867` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:62091` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:62128` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:62251` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:62315` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:62406` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:62475` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:62543` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:62699` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:62737` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:63209` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:63275` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:63455` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:63489` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:63638` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:63673` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:64165` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:64199` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:64711` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:64745` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:64945` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:64979` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:65852` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:66045` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:66120` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:66205` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:82696` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:82781` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:83833` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:83900` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:84279` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:86514` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:86549` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:86917` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:86973` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:87216` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:87249` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:87350` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:87385` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:87496` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:87531` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:87674` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:87737` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:87854` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:87926` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:87989` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:88731` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:88764` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:88848` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:89057` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:89092` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:89186` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:90614` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:90653` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:90762` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:90851` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:90924` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:91444` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:91479` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:91574` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:91609` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-host/src/lib.rs:92008` | `T-fixture` | — | — | — (fixture/test only) |
| `crates/od-testkit/src/memory.rs:152` | `F-evidence` | — | — | — (scope/evidence only) |
| `crates/od-testkit/src/memory.rs:153` | `F-evidence` | — | — | — (scope/evidence only) |
| `crates/od-isa/README.md:99` | `F-contract` | ✓ | ✗ | S6-20/S6-30/S6-32 |
| `crates/od-isa/README.md:101` | `F-contract` | ✓ | ✗ | S6-20/S6-30/S6-32 |
| `crates/od-isa/README.md:104` | `F-contract` | ✓ | ✗ | S6-20/S6-30/S6-32 |
| `crates/od-isa/README.md:149` | `F-contract` | ✓ | ✗ | S6-20/S6-30/S6-32 |
| `crates/od-machine/src/io_apic.rs:13` | `A-APIC` | ✗ | ✗ | S6-11/S6-12/S6-18; MADT also needs S7-13 |
| `crates/od-machine/src/io_apic.rs:975` | `A-APIC` | ✗ | ✗ | S6-11/S6-12/S6-18; MADT also needs S7-13 |
| `crates/od-isa/src/interrupt.rs:1281` | `A-RAM` | ✓ | ✗ | S6-30 |
| `crates/od-isa/src/iretq.rs:112` | `A-RAM` | ✓ | ✗ | S6-30 |
| `crates/od-isa/src/iretq.rs:126` | `A-RAM` | ✓ | ✗ | S6-30 |
| `crates/od-isa/src/lretq.rs:102` | `A-RAM` | ✓ | ✗ | S6-30 |
| `tools/bench/src/two_site_memory_cached.rs:849` | `F-evidence` | — | — | — (scope/evidence only) |
| `crates/od-isa/tests/cpl0_profile_set.rs:12847` | `T-LOCK` | — | — | S6-20 test oracle |
| `crates/od-isa/tests/cpl0_profile_set.rs:31038` | `T-LOCK` | — | — | S6-20 test oracle |
| `crates/od-isa/tests/cpl0_profile_set.rs:42400` | `T-LOCK` | — | — | S6-20 test oracle |
| `crates/od-isa/tests/cpl0_profile_set.rs:42401` | `T-LOCK` | — | — | S6-20 test oracle |
| `crates/od-isa/tests/cpl0_profile_set.rs:42924` | `T-LOCK` | — | — | S6-20 test oracle |
| `crates/od-isa/tests/cpl0_profile_set.rs:42925` | `T-LOCK` | — | — | S6-20 test oracle |
| `crates/od-isa/tests/cpl0_profile_set.rs:42926` | `T-LOCK` | — | — | S6-20 test oracle |
| `crates/od-isa/tests/cpl0_profile_set.rs:42966` | `T-LOCK` | — | — | S6-20 test oracle |
| `crates/od-isa/tests/cpl0_profile_set.rs:44563` | `T-LOCK` | — | — | S6-20 test oracle |
| `crates/od-isa/tests/cpl0_profile_set.rs:44564` | `T-LOCK` | — | — | S6-20 test oracle |
| `crates/od-isa/tests/cpl0_profile_set.rs:47221` | `T-LOCK` | — | — | S6-20 test oracle |
| `crates/od-isa/tests/cpl0_profile_set.rs:47494` | `T-LOCK` | — | — | S6-20 test oracle |
| `crates/od-isa/tests/cpl0_profile_set.rs:47947` | `T-LOCK` | — | — | S6-20 test oracle |
| `crates/od-machine/src/local_apic.rs:1` | `A-APIC` | ✗ | ✗ | S6-11/S6-12/S6-18; MADT also needs S7-13 |
| `crates/od-machine/src/local_apic.rs:449` | `A-APIC` | ✗ | ✗ | S6-11/S6-12/S6-18; MADT also needs S7-13 |
| `tools/bench/src/dispatch_cache_stress.rs:1116` | `F-evidence` | — | — | — (scope/evidence only) |
| `crates/od-machine/src/paged_pump_backend.rs:4` | `A-vCPU` | ✗ | ✗ | S6-10/S6-15; distinct CPU state is required even in an interleaving scheduler |
| `tools/bench/src/dispatch_cache.rs:2997` | `F-evidence` | — | — | — (scope/evidence only) |
| `crates/od-isa/src/long_decode.rs:883` | `A-LOCK` | ✓ | ✗ | S6-20, includes DS substitution |
| `crates/od-isa/src/long_decode.rs:891` | `A-LOCK` | ✓ | ✗ | S6-20, includes DS substitution |
| `crates/od-isa/src/long_decode.rs:919` | `A-LOCK` | ✓ | ✗ | S6-20, includes DS substitution |
| `crates/od-isa/src/long_decode.rs:8205` | `A-LOCK` | ✓ | ✗ | S6-20, includes DS substitution |
| `crates/od-isa/src/long_decode.rs:8212` | `A-LOCK` | ✓ | ✗ | S6-20, includes DS substitution |
| `crates/od-isa/src/long_decode.rs:8213` | `A-LOCK` | ✓ | ✗ | S6-20, includes DS substitution |
| `crates/od-isa/src/long_decode.rs:8351` | `A-LOCK` | ✓ | ✗ | S6-20, includes DS substitution |
| `crates/od-isa/src/long_decode.rs:10461` | `A-LOCK` | ✓ | ✗ | S6-20, includes DS substitution |
| `crates/od-isa/src/long_decode.rs:10462` | `A-LOCK` | ✓ | ✗ | S6-20, includes DS substitution |
| `crates/od-isa/src/long_decode.rs:10463` | `A-LOCK` | ✓ | ✗ | S6-20, includes DS substitution |
| `crates/od-machine/README.md:95` | `F-status` | ✓ | ✗ | S6-10…18 feature owners |
| `crates/od-machine/src/interrupt_pump.rs:351` | `A-vCPU` | ✗ | ✗ | S6-10/S6-15; distinct CPU state is required even in an interleaving scheduler |
| `crates/od-machine/src/interrupt_pump.rs:809` | `A-vCPU` | ✗ | ✗ | S6-10/S6-15; distinct CPU state is required even in an interleaving scheduler |
| `tools/bench/src/two_site_memory_cold.rs:767` | `F-evidence` | — | — | — (scope/evidence only) |
| `crates/od-machine/src/paged_apic_entry.rs:53` | `A-vCPU` | ✗ | ✗ | S6-10/S6-15; distinct CPU state is required even in an interleaving scheduler |
| `crates/od-machine/src/linux_direct.rs:4641` | `T-LOCK` | — | — | S6-20 test oracle |
| `crates/od-machine/src/linux_direct.rs:4818` | `T-LOCK` | — | — | S6-20 test oracle |
| `crates/od-isa/src/execute.rs:16913` | `A-RAM` | ✓ | ✗ | S6-30 |
| `crates/od-isa/src/execute.rs:36772` | `A-fence` | ✓ | ✗ | S6-32 |
| `crates/od-isa/src/execute.rs:36956` | `A-LOCK` | ✓ | ✗ | S6-20 |
| `crates/od-isa/src/execute.rs:36974` | `A-LOCK` | ✓ | ✗ | S6-20 |
| `crates/od-isa/src/execute.rs:49865` | `A-LOCK` | ✓ | ✗ | S6-20 |
| `crates/od-isa/src/execute.rs:49891` | `A-LOCK` | ✓ | ✗ | S6-20 |
| `crates/od-isa/src/execute.rs:49903` | `A-fence` | ✓ | ✗ | S6-32 |
| `crates/od-isa/src/execute.rs:55992` | `A-LOCK` | ✓ | ✗ | S6-20 |
| `crates/od-isa/src/execute.rs:56001` | `A-LOCK` | ✓ | ✗ | S6-20 |
| `crates/od-isa/src/execute.rs:56026` | `A-LOCK` | ✓ | ✗ | S6-20 |
| `crates/od-isa/src/execute.rs:56039` | `A-LOCK` | ✓ | ✗ | S6-20 |
| `crates/od-isa/src/execute.rs:56040` | `A-LOCK` | ✓ | ✗ | S6-20 |
| `crates/od-isa/src/execute.rs:56055` | `A-LOCK` | ✓ | ✗ | S6-20 |
| `crates/od-isa/src/execute.rs:56079` | `A-LOCK` | ✓ | ✗ | S6-20 |
| `crates/od-isa/src/execute.rs:56102` | `A-LOCK` | ✓ | ✗ | S6-20 |
| `crates/od-isa/src/execute.rs:56125` | `A-LOCK` | ✓ | ✗ | S6-20 |
| `crates/od-isa/src/execute.rs:56136` | `A-LOCK` | ✓ | ✗ | S6-20 |
| `crates/od-isa/src/execute.rs:56137` | `A-LOCK` | ✓ | ✗ | S6-20 |
| `crates/od-isa/src/execute.rs:56158` | `A-LOCK` | ✓ | ✗ | S6-20 |
| `crates/od-isa/src/execute.rs:56177` | `A-LOCK` | ✓ | ✗ | S6-20 |
| `crates/od-isa/src/execute.rs:56205` | `A-LOCK` | ✓ | ✗ | S6-20 |
| `crates/od-isa/src/execute.rs:56303` | `A-LOCK` | ✓ | ✗ | S6-20 |
#### Дополнительный поиск: RAM и shared state

Все четыре буквальных дополнительных набора не пересекаются с основным поиском на уровне `file:line` (0 overlap). Counts ниже оставлены как raw counts; source-поиск захватывает inline `#[cfg(test)]` blocks, поэтому test-only occurrences отдельно названы, а не выданы за production ownership.

- **`&mut PhysicalMemory`: 25 raw hits.** 20 production sites передают эксклюзивный borrow владеющего RAM `PhysicalMemory`; пять — inline test helpers (`translation_probe_cache.rs:881`, `page_walk.rs:2557`, `:2891`, `:2973`, `:3032`). Текущая конструкция запрещает одновременно выдать mutable borrow нескольким интерпретаторам, но подходит одному потоку, который меняет vCPU только на завершённых instruction steps. Parallel sharing — S6-30: один адаптер на vCPU с общей RAM и новый API. `page_walk.rs:693-700` фиксирует A/D entry как полный `write_u64`, а не CAS/atomic update; `S6-25` владеет A/D semantics, `S6-30` общей RAM, S6-31 host primitive.
- **`&mut [u8]`: 218 raw hits по 31 файлу.** 13 строк принадлежат явным тестовым путям `acpi_blob/tests.rs`, `nvme/tests.rs`, `raw_file_tests.rs`, `tests/native_register_v1_corpus.rs`; raw count также включает inline tests. Это преимущественно output/destination buffers для копирования, codec/load buffers, framebuffer copies и NVMe/DMA transfer buffers. Непосредственного выдающего RAM backing API в этих hits не найдено: `PhysicalMemory::read_bytes` копирует backing в предоставленный output; `ram_frame` выдаёт opaque handle, чтение делает scalar `read_ram_frame_u8`; guest write принимает input slice и копирует его в backing под `&mut self`. Удаление/замена любого будущего writable RAM slice API — S6-30. Device/DMA transfer buffers, если участвуют одновременно с vCPU, относятся к S6-33.
- **`Rc<`: 7 raw hits; `RefCell<`: 6 raw hits.** Production `Rc` hits: `pit_ioapic_irq_source.rs:193` и четыре поля arena/handle в `od-host/src/lib.rs:2586-2595`; две test-only `Rc` строки — `pci_ecam.rs:577` и `linux_direct.rs:11213`. Production `RefCell` hits: `bus.rs:485` (массив `BusDevice`), `pit_ioapic_irq_source.rs:193`, `fetch_translation_cache.rs:73`, `od-host/src/lib.rs:2586,2594`; `linux_direct.rs:11213` — test-only. Пересечение `Rc`/`RefCell` — 4 строки (`pit_ioapic_irq_source.rs:193`, `linux_direct.rs:11213`, `od-host/src/lib.rs:2586,2594`). `MachineBus` и PIT→IOAPIC связка содержат изменяемое device state для текущего последовательного bus dispatch; parallel access требует порядка и синхронизации S6-33. Fetch translation cache должен быть собственностью одного vCPU (S6-17), поэтому per-vCPU `RefCell` допустим при thread confinement, но не как общий cache. `od-host` `Rc<RefCell<ArenaInner>>` намеренно не `Send`/`Sync`; выбор cache ownership, MAP_JIT multi-thread execution и безопасное освобождение блоков — S6-40/S6-42.

#### Отдельно проверенные instruction/device пути

- **LOCK/XCHG.** В `execute.rs` current CPL0-set locked memory operations — `LockCmpxchgMem` (и bare `CmpxchgMemory`), `LockIncMem`, `LockDecMem`, `LockOrMem`, `LockAddMem`, `LockAddMemReg`, `LockXaddMem` (плюс `XaddMemory`/`XaddDsMem`), `LockAndMem`, `LockBitMem` register bit forms, `LockBtsMemImm`. Они читают operand, вычисляют значение, затем пишут обратно двумя `GuestMemory` вызовами; comments явно опираются на one-vCPU serial access. Такую read→write последовательность делает неделимой планировщик чередования с границей инструкции; параллельно между вызовами может вмешаться другой vCPU. Dword и qword `XchgMemory` в CPL0 set (bare или с `LOCK`) тоже идут read→write; qword вне этого bounded CPL0 path использует `atomic_exchange_integer`. Register-register `Xchg` RAM не затрагивает. Для всех этих форм контракт и общий API — S6-20; host atomics добавляет S6-31.
- **DS substitute для LOCK.** `long_decode.rs` описывает Linux `.smp_locks` substitution `F0` → `3E`; в 64-bit mode DS не меняет адрес, поэтому на однопроцессорном board decoder treats DS `XADD`/`CMPXCHG` как LOCK-equivalent. Если гостю видны два процессора, но ОС не переписала `.smp_locks`, семантика DS-формы должна соответствовать ожидаемой ОС атомарности; одной DS-prefix interpretation недостаточно. S6-20 step 6 прямо назначает решение по этой судьбе и запись в `CPU_CONTRACT.md`, поэтому это не orphan.
- **Accessed/A/D.** Page-walk A bit (и D bit на write) сейчас обновляется отдельной записью `write_u64`; конкурентное read/modify/write может терять чужой установленный бит. S6-04 уже фиксирует compare-and-retry контракт, S6-25 владеет bit path, S6-30 общей RAM API, S6-31 host atomic primitive. Descriptor Accessed writes — отдельные instruction side effects, не page-table A/D; их widths, fault/commit и callback boundaries принадлежат S6-26.
- **Descriptor updates.** В `execute.rs` effects имеют несколько путей и ширин: `far_return_cpl0` пишет CS Accessed bit целым qword (`:53698–53710`); `iretq_cpl0` отдельно пишет CS и при наличии SS descriptor qword после проверок (`:53845–53868`); общий `load_sreg_cpl0` также пишет descriptor целым qword (`:54130–54131`); отдельный bounded segment helper для DS/ES/FS/GS меняет только Accessed byte (`:54492–54496`). `load_tr_cpl0` читает 16-byte TSS descriptor двумя qword reads, затем меняет busy bit только в low qword до commit `TR` (`:54263–54320`); это отдельный путь от paged LTR. В `interrupt.rs::commit_cpl0_gate` сначала фиксируются stack-frame writes, затем при необходимости целиком записывается target code-segment descriptor; ошибка descriptor write не меняет CPU state, но уже committed stack/page-walk effects не откатывает (`:694–747`). Bounded `#PF` delivery path делает descriptor Accessed-byte write отдельной операцией после preflight и stack-slot commits; поздняя ошибка оставляет уже записанные frame slots, но CPU entry state не коммитится (`interrupt.rs:351–389`). В `page_walk.rs::commit_paged_ltr_descriptor` 16-byte TSS busy image пишется после read/validate/write-permission checks и leaf D-bit; page-table A updates с callback остаются committed при более поздней ошибке (`:1491–1497`, `:1574–1594`). Эти пути не взаимозаменяемы с ordinary store, LOCK API или A/D CAS; code comments требуют exclusive owner для paged LTR и не заявляют system-wide locked-bus atomicity. Один host thread предотвращает конкурентный вход внутрь вызова, но этого недостаточно для точной multi-vCPU guest semantics; S6-26 определит per-path width, fault order, commit and notification boundaries, отделённые от S6-20/S6-31.
- **`VirtualClockAdvancer`.** `virtual_clock.rs:78-80,112-120` создаёт одну exclusive advancement capability на time domain; `Arc<Mutex<...>>` защищает состояние, но не вводит отдельное временное продвижение каждого vCPU. Это соответствует serial scheduler S6-15; параллельно правильно лишь при единственном общем владельце времени, как требуют S6-15/S6-34.
- **`PairState`.** `apic_pair.rs:39-44` contains exactly one `local: State`, one shared IOAPIC `route`, and one timer. It is current one-vCPU device state, not a per-vCPU LAPIC array. Serialization preserves mutation safety but does not provide per-vCPU LAPIC state or APIC ID routing: multi-vCPU interleaving and parallel modes both require S6-11/S6-12/S6-18. Shared IOAPIC/PIT access can be serialized in interleaving mode; parallel access belongs to S6-33.

#### Допущения без владельца

Production предположений без S6 owner в этом inventory не осталось. Все конкурентные RAM writes сопоставлены с S6-25/S6-30/S6-31; descriptor writes — с S6-26; device/bus state — с S6-33; LAPIC state — с S6-11/S6-12; scheduler/time ownership — с S6-15/S6-34; JIT arena — с S6-40/S6-42. Test/fixture-only записи не являются неустранёнными product assumptions. Поэтому `KNOWN_ISSUES.md` не изменялся. Архитектурное ревью таблицы остаётся необходимым; S6-07 status — `in_progress`, checkbox плана не менялся.


### S6-26 — контракт побочных записей дескрипторов (accepted)

Подготовлен документный контракт для восьми путей из S6-07: `LRETQ`, `IRETQ`,
ограниченные segment-register loads, bounded `LTR`, bounded interrupt/trap
delivery, bounded `#PF/#DF` delivery и paged `LTR`. Для каждого пути зафиксированы
предшествующие проверки, логическая ширина `GuestMemory` write, точка CPU-state
commit, эффект поздней ошибки и отдельно сохраняющиеся page-walk A/D commits.
Обычные operand stores и A/D updates отделены от descriptor state.

Граница shared RAM требует атомарной descriptor-update operation с сохранением
нецелевых битов и связью с проверенным descriptor/translation snapshot.
`PhysicalMemory::write_bytes` выдаёт generation/callback за одну успешную
backing transaction; текущий `PagedMovMemoryAdapter` для разрешённого
cross-page qword store сначала проверяет обе линейные страницы, затем пишет
байты отдельно. Это может дать несколько callbacks/generation bumps и
частичную mutation при позднем отказе. Cross-page 16-byte отказ отдельно
помечен как ограничение bounded LTR profiles. Один commit инструкции не
объединяет frame slots, несколько descriptor updates и page-walk writes в
атомарный блок. Guest `LOCK` API не заменяет этот instruction-specific
контракт. SMP-safe реализация остаётся gate для S6-30/S6-31.

Источники: Intel SDM revision 093 (September 2026), Vol. 3A §§3.4.3, 3.4.5.1
и 11.1.2.1; Vol. 2A `IRET` и `LTR`; Vol. 2B `RET`. SDM задаёт implicit
`LOCK` semantics для Accessed update и locked RMW всего descriptor для `LTR`,
но не общую аппаратную ширину bus transfer или универсальный fault-rollback
протокол для всех перечисленных emulator paths. Сверка по [Vol. 3A
(revision 093)](https://cdrdv2.intel.com/v1/dl/getContent/671190), [Vol. 2A
(revision 093)](https://cdrdv2.intel.com/v1/dl/getContent/671199) и [Vol. 2B
(revision 093)](https://cdrdv2.intel.com/v1/dl/getContent/671241).

S6-26 прошёл независимое архитектурное ревью `gpt-6.1-sol/high`; замечаний не
осталось. Документный срез внесён коммитом `422d0a3`. `git diff --check` чист.
`check_layout.py` сообщает 45 старых broken
links на отсутствующие work evidence; ни одна ссылка не затрагивает изменённые
S6-26 документы. SMP реализация descriptor transactions остаётся gate для
S6-30/S6-31. Плановый checkbox оставлен без изменения согласно указанию
пользователя.
