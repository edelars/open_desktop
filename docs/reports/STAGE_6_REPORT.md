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
| S6-15 | `in_progress` | Реализовано, принято ревью (код и архитектурное §8, правки внесены 2026-11-16): планировщик `smp_scheduler.rs`, раздел времени SMP_CONTRACT §8 (принят с правками); к рабочему циклу не подключён, см. раздел S6-15 ниже. Блокеры S4-22/S4-23 закрыты 2026-11-15. |
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

### S6-14c — представление real/protected mode (принято ревью)

Добавлен модуль `crates/od-isa/src/legacy_mode.rs` (подключён строкой
`pub mod legacy_mode;` в `crates/od-isa/src/lib.rs`). Это только представление
состояния и разбор данных, без исполнения инструкций.

Что сделано:

- Константы состояния после INIT/SIPI: `INIT_CS_SELECTOR` = `0xF000`,
  `INIT_CS_BASE` = `0xFFFF_0000`, `INIT_EIP` = `0xFFF0`, `INIT_EFLAGS` = `0x2`,
  `REAL_MODE_LIMIT` = `0xFFFF`, `CR0_ET`, `CR0_INIT_PRESERVED_MASK`
  (`0x6000_0000`) и `REAL_MODE_DEFAULT_OPERAND_SIZE`. Источники: Intel SDM
  Table 12-1 (значения после INIT), её сноска 2 (сохранение CD/NW в CR0) и
  SIPI, §11.4.3 шаг 6.
- `RealModeState::after_init(old_cr0)`, `after_sipi(vector)` и
  `fetch_linear_address()`: состояние после INIT, применение вектора SIPI к
  CS/EIP и линейный адрес выборки.
- `ProtectedSegment::from_descriptor(selector, [u8; 8])` разбирает 8-байтный
  дескриптор (base, limit с учётом гранулярности, access, flags), а
  `default_operand_size()` выводит размер операнда по биту D/B.

Что НЕ сделано: исполнение инструкций; S6-13; атрибуты сегментов real mode;
CR2/CR3/CR4; EFER; GDTR/IDTR; LDTR/TR; GPR; EDX-сигнатура после INIT;
состояние `WaitForSipi`. S6-14d и далее не начаты.

GPR, EDX-сигнатура, GDTR/IDTR, LDTR/TR, CR3/CR4 из `legacy_mode.rs` (список «Not modelled here») — срез-преемник: S6-14d/S6-16 (исполнение и трамплин); activity `WaitForSipi` представлен в S6-13.

Независимое ревью Claude Opus / high: вердикт «принять» с условием исправить
замечания 1-3; исполнитель исправил замечания 1-8.

Проверки: `cargo test -p od-isa --locked --offline --lib legacy_mode` — 9 тестов
прошли; `cargo fmt` — чисто. Полный набор тестов не запускался.

Ограничения: сверка с Intel SDM выполнена по памяти, первоисточник офлайн
недоступен. Линия S4-38 завершена владельцем 2026-11-16 и не блокирует правки
`od-isa`. Чекбокс S6-14 в целом не отмечен.

### S6-14d — первая группа исполнения real mode (реализовано, ожидает ревью)

Дата: 2026-11-16. Чекбокс S6-14 в `STAGE_6.md` не отмечен. Контракт: [`CPU_CONTRACT.md`](../specs/CPU_CONTRACT.md#s6-14d--первая-группа-исполнения-real-mode).

Что сделано: новый модуль `crates/od-isa/src/real_mode_exec.rs` (регистрация
`pub mod real_mode_exec;` в `lib.rs`; `legacy_mode.rs`, декодер long mode и
`LongModeState` не менялись). `RealModeCpu` = `RealModeState` (S6-14c) + 8 GPR +
признак теневого периода `MOV SS`. Это минимальный путь: в `RealModeState` нет
регистров общего назначения, а `LongModeState` заточен под long mode. Исполняются
ровно формы блока 1 trampoline: `CLI` (`fa`), `JMP ptr16:16` (`ea`), `MOV AX,CS`
(`8c c8`), `MOV DS|ES|SS,AX` (`8e d8`/`8e c0`/`8e d0`). Всё остальное, включая любой
префикс, `NOP`, `HLT` и `WBINVD`, — `Unsupported` с `CS:IP`, линейным адресом и байтами
без изменения состояния. Выборка идёт через CS с проверкой предела; режим `CR0.PE=1`
отказывает. Память — трейт `RealModeMemory` (только выборка), `ImageMemory` для
образа.

Оракул (обязательный): `tools/kvm-runner/kvm_runner.c` (не менялся, sha256 в
`host-info.txt`) собран на Intel Mini (`gcc 15.2.0`, `-Wall -Wextra -Werror`,
Linux 7.0.0-42, i5-3210M) в `/tmp/od-s6-14d`; четыре образа запущены с `--real-mode`:
`a` (`0000:1000`; формы и кодировки те же, что в `16bit-objdump`; раскладка блока
изменена: `WBINVD` (`0F 09`) удалён, `JMP` переставлен с `0x1003` на `0x1001`, после него
`90 90` (`0x1006`–`0x1007`) — мёртвое выравнивание, не исполняется; `LOCK BTS` по
`0x1010` заменён на `HLT` (`F4`) — стоп оракула; в образах `b`/`c`/`d` селектор и смещение
`JMP` подобраны вручную (`b`: `ea 08 10 00 01`; `c` и `d`: смещение `0x0006`), это не байты
objdump), `b` (`0100:1000`, сегмент дальнего перехода как у
перемещённого blob), `c` (дальний переход меняет CS: `0100:1000` -> `0200:0006`), `d`
(старт как после SIPI с вектором `0x9f`: `9f00:0000`). Каждый образ заканчивается
`HLT` (стоп оракула; в `od-isa` на нём прогон останавливается как `Unsupported`).
Данные, образы, `run-oracle.sh`, `host-info.txt`: `docs/reports/data/stage6/s6-14d/`.

Сравнение: те же образы и вход (`RealModeCpu::at_cs_ip` / `after_sipi`, CR0 до INIT =
`0x6000_0010`, EDX = сигнатура из CPUID leaf 1 оракула) прогнаны через `od-isa`
тестами `oracle_*`; **совпало** полностью: все 8 GPR, selector/base/limit CS, DS, ES, SS,
FS, GS, EFLAGS, CR0, IP (оракул = IP после `HLT` = наш IP + 1). Расхождений нет.

Тесты: 27 в прогоне `real_mode_exec legacy_mode` (18 новых: 14 unit + 4 сравнения с оракулом;
остальные 9 — `legacy_mode`). Unit-значения пересчитаны вручную по SDM: каждая форма, сохранение
старшей половины EAX, сохранение предела при загрузке сегмента, теневой период `MOV SS`,
отказ предела цели перехода, конец инструкции ровно на `0xFFFF` (EIP = `0x10000`, затем `FetchBeyondLimit`), отказы выборки, `NotRealMode`, 23 вне-профильные формы с адресом
и байтами.

Проверки: `cargo fmt -p od-isa -- --check` — чисто; `cargo check -p od-isa
--all-targets --offline` — без предупреждений; `cargo test -p od-isa --locked --offline --lib
-- real_mode_exec legacy_mode` — 27 прошли; `cargo clippy -p od-isa --all-targets` — без замечаний
по новому модулю. Полный набор od-isa не запускался.

Ограничения оракула: рабочие регистры входа фиксированы раннером (GPR = 0, EDX =
сигнатура, IF = 0), поэтому оракул не проверяет эффект `CLI`, сохранение старшей половины
EAX при `MOV AX,CS` и теневой период `MOV SS`; это покрыто только unit-тестами по SDM.
Проблем с хостом не было.

Что НЕ сделано (следующие срезы): `WBINVD`, `LOCK BTS` и цикл `JAE`/`PAUSE` (S6-14e),
32-битная загрузка ESP, `CALL verify_cpu` и сам `verify_cpu`, `LIDT`/`LGDT`, `MOV CR0`,
protected mode. Модуль не подключён к циклу исполнения машины и к S6-13/S6-16.

### S6-14e — вторая группа исполнения real mode (реализовано, ожидает ревью)

Дата: 2026-11-16. Чекбокс S6-14 в `STAGE_6.md` не отмечен. Контракт: [`CPU_CONTRACT.md`](../specs/CPU_CONTRACT.md#s6-14e--вторая-группа-исполнения-real-mode).

Что сделано: `crates/od-isa/src/real_mode_exec.rs` расширен (отдельный модуль не нужен:
состояние, выборка и отказы те же; формы S6-14d не менялись; `legacy_mode.rs`, long mode,
`LongModeState` не тронуты). Новые формы блока 1: `WBINVD` (`0f 09`, в модели без кэшей —
только `IP += 2`), `LOCK BTS DWORD PTR ds:disp16, imm8` (`66 f0 0f ba 2e d d i`; `CF` =
старый бит, бит `imm8 mod 32`, чтение-изменение-запись 4 байт; `ZF` не меняется, `OF/SF/AF/PF`
не определены — модель не трогает; одноядерная модель, атомарность не моделируется),
`JAE rel8` (`73 cb`, по `CF = 0`), `PAUSE` (`f3 90`, не `NOP`), `JMP rel8` (`eb cb`, обратный
переход цикла). Замечание к постановке: `ModRM 2e` задаёт `reg = 5`, то есть `BTS` — это
`0f ba /5` (в группе 8 `/4` = `BT`); так и в objdump. `CALL verify_cpu` не сделан (S6-14f).
Изменение интерфейса: `RealModeMemory` получил `can_write`/`write_u8`, `step`/`run` теперь
принимают `&mut M`; `ImageMemory` остался только читаемым, добавлен `ImageMemoryMut`. Новые
отказы: `DataBeyondLimit`, `DataUnreadable`, `DataUnwritable` (операнд целиком проверяется
до записи; предел DS без переноса операнда).

Оракул (обязательный): `tools/kvm-runner/kvm_runner.c` (не менялся; sha256 в `host-info.txt`)
собран заново на Intel Mini в `/tmp/od-s6-14e` (`gcc 15.2.0`, `-Wall -Wextra -Werror`, Linux
7.0.0-42, i5-3210M), запуск с `--real-mode` и `--result-block GPA:4` (слово блокировки/данных
после остановки). Образы: `a` (`0000:1000`) — точные байты `0x1000`–`0x101d` из
`16bit-objdump.txt` (раскладка objdump без изменений; единственная замена — `HLT` по `0x101e`
вместо `mov esp,…`, стоп оракула); `b` (`9f00:1000`) — то же с сегментом `9f00` в дальнем
переходе; `c` — `wbinvd`, `lock bts` дважды, `jae` (не берётся, `CF = 1`), `pause`, `jmp`
вперёд, `HLT` (раскладка изменена, формы те же: настоящий обратный переход без второго
процессора не завершается); `d` — `lock bts [0x1020], 0x3f` по слову `0x0100_0001` в образе
(раскладка изменена). Данные, образы, `run-oracle.sh`, `host-info.txt`:
`docs/reports/data/stage6/s6-14e/`.

Сравнение: те же образы и вход (`RealModeCpu::at_cs_ip`, CR0 до INIT = `0x6000_0010`, EDX =
сигнатура из CPUID leaf 1 оракула) через `od-isa` тестами `oracle_e_*` с сравнением по JSON
оракула; **совпало** полностью: 8 GPR, selector/base/limit CS/DS/ES/SS/FS/GS, EFLAGS, CR0, IP
(оракул = наш IP + 1 после `HLT`) и слово памяти (`a`, `b`: `00000000` -> `01000000`; `c`: ->
`01000000`, `EFLAGS = 3`; `d`: `01000001` -> `01000081`). Расхождений нет.

Тесты: 45 в прогоне `real_mode_exec legacy_mode` (18 новых: 14 unit + 4 сравнения с
оракулом; остальные 27 — S6-14d и `legacy_mode`). Unit-значения пересчитаны вручную по SDM:
каждая форма, `CF = 0/1`, индексы бита (`imm8 mod 32`, порядок байт), `CF` без побочных
флагов, предел DS (`0xFFFC` проходит, `0xFFFD` — нет), нечитаемый и неизменяемый операнд без
частичной записи, обрезанная команда, `JAE` по `CF` (вперёд/назад), `JMP rel8` (в т. ч.
усечение до 16 бит), цель за пределом CS, цикл блокировки с удержанным замком (4 шага на
виток, состояние повторяется) и с отпущенным (выход, слово = 1), блок 1 целиком (9 шагов),
сброс теневого периода `MOV SS`, 41 вне-профильная форма с адресом и байтами (проверка, что
состояние не меняется).

Проверки: `cargo fmt -p od-isa -- --check` — чисто; `cargo check -p od-isa --all-targets
--offline` — без предупреждений; `cargo test -p od-isa --locked --offline --lib --
real_mode_exec legacy_mode` — 45 прошли; `cargo clippy -p od-isa --all-targets` — без замечаний
по `real_mode_exec.rs`. Полный набор od-isa не запускался.

Не проверено оракулом (только unit-тесты по SDM, по памяти): путь «захвачено» целиком —
`CF = 1` и непринятый `JAE`/`PAUSE` оракул видит (образ `c`), но обратный переход при
удержанной блокировке и повторные витки — нет; `OF/SF/AF/PF` после `BTS` (входные флаги
нулевые); эффект `WBINVD` на кэши; атомарность `LOCK`; отказы (предел DS, нечитаемая и
неизменяемая память); перенос операнда через предел DS. Проблем с хостом не было.

Что НЕ сделано (следующие срезы): 32-битная загрузка ESP (`0x101e`), `CALL verify_cpu` и сам
`verify_cpu`, `TEST`/`JNE`, `LIDT`/`LGDT`, `MOV CR0`, переход в protected mode. Модуль не
подключён к циклу исполнения машины и к S6-13/S6-16.

### S6-14f — третья группа исполнения real mode (реализовано, ожидает ревью)

Дата: 2026-11-16. Чекбокс S6-14 в `STAGE_6.md` не отмечен. Контракт: [`CPU_CONTRACT.md`](../specs/CPU_CONTRACT.md#s6-14f--третья-группа-исполнения-real-mode-callret-verify_cpu).

Что сделано: `crates/od-isa/src/real_mode_exec.rs` расширен (формы S6-14d/e не менялись;
`legacy_mode.rs`, long mode, `LongModeState` не тронуты). Новые формы: `MOV ESP,imm32`
(`66 bc`, `0x101e`) и `MOV EAX,imm32` (`66 b8`), `CALL rel16` (`e8`, `0x1024`), `RET` (`c3`),
`PUSHF` (`9c`), `POPF` (`9d`), `PUSH imm8` (`6a`, со знаковым расширением до слова),
`XOR EAX,EAX` (`66 31 c0`). Из тела `verify_cpu` взято только то, что не требует `CPUID` и
`PUSHFD`/`POPFD`: начало (`pushf; push 0; popf`, `0x1080`) и оба хвоста возврата (`0x119b`,
`0x11a4`). Стек в real mode — 16 бит (оракул: `SS.B = 0`): меняется только SP, старшая
половина ESP сохраняется, слово не разрезается пределом SS и не заворачивается внутри слова.
Новый отказ `PopfUnmodelledFlags` (слово `POPF` с `TF` или `IF`: ловушки и прерывания не
моделируются; в профиле такого слова нет). Остальные отказы — прежние (`DataBeyondLimit`,
`DataUnreadable`, `DataUnwritable`, `TargetBeyondLimit`) с проверкой всего слова до записи.
Из списка `Unsupported` теста S6-14d удалены `CALL` и `MOV ESP,imm32` (теперь допущены; их
околопрофильные формы — в новом тесте `s6_14f_near_misses_are_refused`).

Оракул (обязательный): `tools/kvm-runner/kvm_runner.c` (не менялся; sha256 `969ce3a2…`
совпадает с S6-14e, собранный `kvm_runner` `5f6a2aac…` тоже) собран заново на Intel Mini в
`/tmp/od-s6-14f` (`gcc 15.2.0`, `-Wall -Wextra -Werror`, Linux 7.0.0-42, i5-3210M), запуск с
`--real-mode` и `--result-block GPA:SIZE` (слова стека после остановки). Образы:
`a` — блок 1 из objdump, `mov esp,0x6020`, `call 0x1080`, вызываемый фрагмент
`pushf; push 0; popf; popf; xor eax,eax; ret`; `b` (`9f00:1000`) — `CF = 1`, хвост
`popf; mov eax,1; ret`, SS = `0x9f00`; `c` — вложенный вызов, `PUSH imm8` (`-1`, `0x7f`,
`0x57`), `POPF` слова `0x57`; `d`, `e` — `ESP = 0x10000`, запись по `SS:0xFFFE` и сохранение
старшей половины ESP. Данные, образы, `run-oracle.sh`, `host-info.txt`:
`docs/reports/data/stage6/s6-14f/`.

Сравнение: те же образы и вход через `od-isa` тестами `oracle_f_*` по JSON оракула;
**совпало** полностью на 5 образах: 8 GPR (включая SP: `a`, `b` — `0x6020`; `c` — `0x601a`;
`d` — `0x10000`; `e` — `0x1fffe`), selector/base/limit шести сегментов, EIP, EFLAGS (`a` —
`0x46`, `b` — `0x3`, `c` — `0x57`), CR0 и слова стека (`a`: `0x601c` = `02 00`, `0x601e` = `27 10`;
`c`: `0x6018..0x601f` = `2a10 7f00 ffff 0910`). Расхождений нет.

Граница `CPUID`: `CPUID` не исполняется, поэтому значения `CPUID` в сравнении не участвуют
(хост оракула — `GenuineIntel`, профиль — `OpenDesktop `; результаты `verify_cpu` по ним
разошлись бы). Единственное значение от `CPUID` — сигнатура в EDX после INIT, взятая из JSON
оракула (как в S6-14d/e, лист 1 EAX = `0x306a9`, модель `0x3a`): **вход, не доказательство**.
Решение для S6-14g записано в контракте: сравнение с оракулом остаётся только на путях, не
зависящих от значений `CPUID` (как в S6-14f); ветки Intel/AMD `verify_cpu` (`0x10bb`–`0x1126`,
включая модель `0x3a`, `RDMSR`/`WRMSR`) в S6-14g не входят и с оракулом не сравниваются;
подстановка `CPUID` профиля через `KVM_SET_CPUID2` не добавляется; результат для профиля —
только unit-тестом по `cpuid.rs`.

Тесты: 69 в прогоне `real_mode_exec legacy_mode` (24 новых: 19 unit + 5 сравнений с
оракулом; остальные 45 — S6-14d/e и `legacy_mode`). Unit-значения пересчитаны вручную по SDM
(помечены в тестах): каждая форма, знаковое расширение `PUSH imm8`, маска `POPF`, `PUSHF`/`POPF`
с ID в старшей половине, `XOR` и флаги, SP по модулю `0x10000` и сохранение старшей половины
ESP, база SS (не DS и не CS), предел SS (`push`: `SP = 0x100` проходит, `0x101`, `0x300`, `0`,
`1` — нет; `pop`: `0xFE` проходит, `0xFF`, `0xFFFF` — нет), нечитаемый/неизменяемый стек без
частичной записи, цель за пределом CS (`CALL`, `RET`, ровно на пределе — проходит), `POPF` с
`TF`/`IF`, вызов, оканчивающийся на `0x10000`, обрезанные формы, сброс теневого периода,
31 околопрофильная форма (`66 9c`, `66 58`, `ret imm16`, `66 85 c0`, `0f a2`, …) с адресом и
байтами и неизменным состоянием, хвосты `verify_cpu` целиком по ручному расчёту.

Проверки: `cargo fmt -p od-isa -- --check` — чисто; `cargo check -p od-isa --all-targets
--offline` — без предупреждений; `cargo test -p od-isa --locked --offline --lib --
real_mode_exec legacy_mode` — 69 прошли; `cargo clippy -p od-isa --all-targets --offline` —
без замечаний по `real_mode_exec.rs`. Полный набор od-isa не запускался.

Не проверено оракулом (только unit-тесты по SDM, по памяти): отказы; `POPF` с `TF`/`IF` и
`PUSH` при `SP = 1`; сброс `AF` моделью после `XOR` (на входе `AF = 0`, `EFLAGS` входа `0x0002`, поэтому
наблюдение оракула «`AF = 0`» ничего не доказывает; SDM — «не определён»); старшая половина
EFLAGS при `POPF` (на входе нулевая); `MOV EAX,imm32` при ненулевой старшей половине EAX до
загрузки `imm32` (в образе `b` перед `mov eax,1` EAX = `0x9f00` после `MOV AX,CS`, оракул дал
`rax = 1`; старшая половина входного EAX там нулевая); вызов на конце
сегмента; середина `verify_cpu`. Проблем с хостом не было.

Что НЕ сделано (S6-14g): `TEST EAX,EAX` + `JNE` после возврата (`0x1027`, `0x102a`);
`PUSHFD`/`POPFD`, `POP EAX`, `PUSH EAX`, `MOV EBX,EAX`, `XOR EAX,imm32`, `CMP EBX,EAX`,
`JE rel16`, `CPUID`, `CMP EAX,imm8`, `JB rel16`, `XOR DI,DI`, `CMP EBX,imm32`, `JNE`, `AND`/`XOR
EDX,imm32`, `CMP EAX/EDX,imm32`, `JE rel8`, `JB rel8` (`72 47` по `0x1152`, на пути профиля не берётся) (полный список с кодировками — в контракте); далее
`LIDT`/`LGDT`, `MOV CR0`, переход в protected mode. Модуль не подключён к циклу исполнения
машины и к S6-13/S6-16.

### S6-14g — остаток форм verify_cpu (реализовано, ожидает ревью)

Дата: 2026-11-16. Чекбокс S6-14 в `STAGE_6.md` не отмечен. Контракт: [`CPU_CONTRACT.md`](../specs/CPU_CONTRACT.md#s6-14g--остаток-формы-блоков-23-без-ветвей-intelamd).

Что сделано: `crates/od-isa/src/real_mode_exec.rs` расширен (формы S6-14d/e/f, `legacy_mode.rs`,
long mode не менялись). Новые формы блоков 2 и 3 без ветвей Intel/AMD: `TEST EAX,EAX`, `JNE`/`JE`/`JB
rel8`, `JE`/`JB rel16` (`0f 84`/`0f 82`), `PUSHFD`/`POPFD`, `PUSH EAX`/`POP EAX` (32-битное слово, предел SS
на 4 байта), `MOV EBX,EAX`, `CMP EBX,EAX`, `XOR`/`CMP EAX,imm32`, `CMP EAX,imm8` (знаковое
расширение), `CMP EBX`/`EDX,imm32`, `AND`/`XOR EDX,imm32`, `XOR DI,DI`, `CPUID` (только листы `0`, `1`,
`0x80000000`, `0x80000001`). Новые отказы: `PopfdUnmodelledFlags` (слово `POPFD` с `TF`, `IF`, `VM`,
`VIF` или `VIP`; `RF` очищается, не отказ) и `CpuidLeafUnmodelled`. `POPFD` пишет маску `POPF` плюс AC и ID; `PUSHFD` кладёт
`EFLAGS & 0x00FCFFFF`. `AF` после логических операций сброшен моделью (по SDM не определён). Состояние
получило поле `cpuid: CpuidSource` (профиль `cpuid.rs` или явная таблица листов; по умолчанию
`DiagnosticV1`). Из старых тестов удалены строки форм, ставших допустимыми (`66 9c`, `66 9d`, `66 58`,
`66 50`, `66 85 c0`, `75`, `0f a2`, `72`, `74`).

**Следствие уже зафиксированного расхождения (строки 455–460 `CPU_CONTRACT.md` и решение S4-38L9): `DiagnosticV1` не цель x86-64-v1; для `verify_cpu` это значит, что путь успеха проверен только на таблице цели.** Ни один профиль `cpuid.rs` не реализует таблицу
цели `od-x86-64-v1-neutral-v0.1` из `CPU_CONTRACT.md`: в листе 1 EDX нет MSR/PAE/CX8/CMOV/SSE/SSE2,
лист `0x80000001` нулевой (нет SYSCALL/NX/LM). По пути профиля `cpuid.rs` `verify_cpu` возвращает 1:
`JNE` по `0x1142` берётся к `0x119b`, `JNE` по `0x102a` — к `0x104c` (`HLT`). Путь «успех» из S6-14a
(возврат нуля по `0x11a4`) получается только на таблице цели; она задана в тесте, `cpuid.rs` не менялся.
Значения не подгонялись ни под один источник.

Оракул (обязательный): `kvm_runner.c` (sha256 `969ce3a2…`) и собранный `kvm_runner` (`5f6a2aac…`)
совпадают с S6-14f; сборка на Intel Mini в `/tmp/od-s6-14g` (`gcc 15.2.0`, Linux 7.0.0-42, i5-3210M).
7 образов: `a` — настоящие байты `verify_cpu` `0x1080`–`0x109f` (проба ID), `b` — 24 случая флагов
`XOR`/`TEST`/`CMP`/`AND`, `c` — условные переходы в трёх состояниях флагов и два обратных, `d` — dword-стек
на границе `SP = 0` при `ESP = 0x10000`, `e` — `CPUID` (значения не сравниваются), `f`/`g` — блок 3
дословно (`EAX = 1` и `0`). Данные, образы, `run-oracle.sh`, `host-info.txt`:
`docs/reports/data/stage6/s6-14g/`.

Сравнение: тесты `oracle_g_*` по JSON оракула, **совпало полностью на 7 образах**: 8 GPR, селектор/база/предел
шести сегментов, EIP, EFLAGS, CR0, слова стека (`a`: итоговые `EFLAGS = 0x200087`, `EBX = 2`; `b`:
записанные флаги, например `0x97`, `0x816`, `0x887`, `0x13` — совпали с ручным расчётом; `d`: `ESP = 0x10000`).
Расхождений нет. Мутационная проверка: убрать ID из маски `POPFD` — падают 6 тестов (оракул `a` и unit-тесты).

Исключено из сравнения с оракулом: все значения `CPUID` (хост `GenuineIntel`, лист 1 EAX `0x306a9`,
профиль `OpenDesktop `): в образе `e` не сравниваются EAX/EBX/ECX/EDX, остальное сравнивается; ветвящиеся
по `CPUID` участки в образах не участвуют; `KVM_SET_CPUID2` не использован. Сигнатура EDX после INIT —
вход из JSON оракула, как раньше.

Тесты: 99 в прогоне `real_mode_exec legacy_mode` (прежние 69 → 99: 23 новых unit + 7 сравнений с
оракулом). Unit-значения пересчитаны вручную по SDM и перепроверены независимым скриптом: флаги `TEST`/`CMP`/`XOR`/`AND`,
знаковое расширение `imm8`, маски `PUSHFD`/`POPFD`, отказы `POPFD`, пределы SS на 4 байта (проходит
`SP = 0`, `0xFFFC`; отказ `SP` = 1–3 и `0xFFFD`–`0xFFFF`), нечитаемая/неизменяемая память без частичной
записи, цель за пределом CS (только взятые переходы), `CPUID` по профилю и по таблице, неподдерживаемые листы,
47 околопрофильных форм, обрезанные формы, сброс теневого периода; прогоны блока `verify_cpu` по байтам из
`16bit-objdump.txt` с точной последовательностью адресов: таблица цели (успех, 57 инструкций, остановка на
`LIDT` `0x102c`), три профиля `cpuid.rs` (отказ), ранние отказы (`0x10ac`, `0x1152`, `0x116a`), ветки
вендоров (остановка на `0x10e4`, `0x10c4`).

Проверки: `cargo fmt -p od-isa -- --check` — чисто; `cargo check -p od-isa --all-targets --offline` — без
предупреждений; `cargo test -p od-isa --locked --offline --lib -- real_mode_exec legacy_mode` — 99 прошли;
`cargo clippy -p od-isa --all-targets --offline` — без замечаний по `real_mode_exec.rs`. Полный набор
od-isa не запускался.

Не проверено оракулом (только unit-тесты по SDM, по памяти): отказы; обнуление VM/RF в `PUSHFD`; `PUSH`/`POP`
при `SP` = 1–3 и `0xFFFD`–`0xFFFF`; `JB`, берущийся по `0x1152`; значения `CPUID` профиля и весь путь
`verify_cpu` целиком (оракул проверяет фрагменты, не путь). Наблюдение оракула «`AF` сброшен после
`XOR`/`AND`/`TEST` при входном `AF = 1`» — свойство этого хоста, не архитектурный факт. Проблем с хостом не было.

Что остаётся до полного S6-14: ветки Intel/AMD `verify_cpu` (`0x10bb`–`0x1126`, `0x1184`–`0x1199`: `CMP ECX`,
`MOV ECX,EAX`, `AND EAX`, `SHR`, `JA`, `RDMSR`, `WRMSR`, `BTR`, `TEST DI,DI`, `66 b9`); блок 4 (`LIDT`, `LGDT`,
`MOV CR0`) и переход в protected mode (блоки 5–8); решение по расхождению `cpuid.rs` и таблицы цели;
подключение модуля к циклу исполнения, S6-13 и S6-16.

### S6-13 — INIT, SIPI и ожидание SIPI (реализовано, исправлено по ревью, ожидает повторного ревью)

Дата: 2026-11-16. Чекбокс S6-13 в `STAGE_6.md` не отмечен. Контракт: [`MACHINE_CONTRACT.md`](../specs/MACHINE_CONTRACT.md#init-sipi-и-ожидание-sipi-s6-13).

Что сделано:
- `od-isa`: `CpuActivity::WaitForSipi`. Исчерпывающих `match` по `CpuActivity` было два вне `od-isa`: `SchedActivity::from` в `idle_arbiter.rs` (теперь отображает в `SchedActivity::WaitForSipi`, который арбитр и раньше никогда не выбирал) и снимок V2 в `od-testkit/src/v2.rs` (`WaitForSipi` отвергается, как `Shutdown`: в схеме снимка его нет). Поведение прежних путей не менялось. Only the CPL0 profile set refuses non-Running activity with `NotRunning`. Other entry points do not check WaitForSipi, among them `od_isa::execute::run`, `step`, `run_with_maskable_request` and the `run_configured`-based runners (the loop checks only Halted; `!= Running` is checked only under paged CR0 profiles). The board executes only vCPU 0, so this is not reachable yet; закрыть до S6-15.
- `vcpu.rs`: у `Vcpu` поле `real_mode: Option<RealModeState>`; `Vcpu::new_ap` (активность `WaitForSipi`, `real_mode = after_init(0x6000_0010)`); `Vcpu::activity()`; `Vcpu::apply_startup(PendingStartup)`: INIT (затем SIPI) по SMP_CONTRACT §3/§4. AP: INIT даёт `RealModeState::after_init(old_cr0)` и `WaitForSipi`, SIPI в `WaitForSipi` даёт `after_sipi(vector)` и `Running`, иначе SIPI игнорируется. BSP: INIT устройство не принимает (см. исправления по ревью); ветка `apply_startup` для BSP достижима только прямым вызовом; SIPI игнорируется. `LongModeState` меняется только полем `activity`.
- `local_apic.rs`/`apic_pair.rs`: `decode_icr` возвращает `IpiCommand` (Fixed, INIT assert, INIT deassert, SIPI). В `State` добавлены `pending_init` и `pending_sipi`; запись ICR только ставит событие (INIT assert сбрасывает ранее принятый SIPI, из нескольких SIPI ждёт первый). `take_startup` отдаёт ожидающие события и при INIT приводит LAPIC к INIT-reset (включён, APIC ID = индекс, регистры/IRR/таймер сброшены, Remote IRR маршрута IOAPIC не снимается: как на железе, его снимает только EOI). LAPIC адресата, глобально выключенный, отказывает INIT assert и SIPI, как Fixed.
- `firmware_board.rs`: `LoadedFirmwareBoard::apply_pending_startup(index)` (`pub(crate)`, вызывается явно, планировщика нет) и `vcpu(index)`.
- Документы: раздел «INIT, SIPI и ожидание SIPI (S6-13)» в `MACHINE_CONTRACT.md`, поправлены места S6-12a, где INIT/SIPI числились неподдержанными; обновлена фраза в SMP_CONTRACT §6.

Изменённые существующие тесты (контракт §5, строки INIT и SIPI):
- `local_apic::tests::matrix_cells_for_nmi_init_and_sipi_are_refused_until_they_are_implemented` (S6-12a) ожидал отказ INIT assert, INIT deassert и SIPI. Переименован в `matrix_cells_for_nmi_are_refused_until_they_are_implemented`, остались только строки NMI; INIT и SIPI теперь принимаются, их поведение проверяют новые тесты.
- `vcpu::tests::the_vcpu_set_numbers_the_processors_and_gives_the_boot_state_to_the_first` (S6-10/S6-11) ждал у AP `LongModeState::default()`; теперь у AP `activity = Some(WaitForSipi)`, остальное по умолчанию.
- `idle_arbiter::tests::wait_for_sipi_and_shutdown_are_never_picked`: убран комментарий «отображение не имеет WaitForSipi», добавлена проверка `SchedActivity::from(WaitForSipi)`.
- `state::tests::fragment_adapter_rejects_any_modeled_cpu_activity`: в перечень добавлен `WaitForSipi`; `cpl0_profile_set.rs`: в таблицу отказов входа добавлен случай `wait for sipi` -> `NotRunning`.
Тесты `matrix_cells_marked_as_an_error_...` и прочие S6-12a (INIT/SIPI с shorthand, вектор Fixed < 0x10 и т. д.) прошли без изменений.

Новые тесты: `local_apic` — INIT assert ждёт только в адресате; INIT deassert без изменений; вектор SIPI (0x00..0xFF); первый из нескольких SIPI, INIT сбрасывает SIPI до себя; INIT к BSP отказывает, SIPI к BSP ждёт; все неверные кодировки INIT/SIPI отказывают без изменений состояния; EN=0 отказывает, SVR-выключенный принимает; INIT-reset LAPIC; Remote IRR не снимается. `vcpu` — BSP `Running`/AP `WaitForSipi` после включения; AP не исполняет до SIPI (состояние неизменно, INIT, deassert и SIPI к BSP его не запускают); SIPI применяется только на границе адресата (адрес `vector << 12`); второй SIPI игнорируется; INIT работающему AP; CD/NW в old CR0, Halted и Shutdown; INIT и SIPI на одной границе; INIT deassert; INIT к BSP отказывает, SIPI к BSP не меняет состояния; прямой INIT вектора BSP; EFER/CR2 после INIT; адресат вне набора vCPU.

Исправления по ревью (2026-11-16):
Решения владельца: (А) INIT не снимает Remote IRR в IOAPIC, как на железе (по SDM его снимает только EOI); (Б) отказ при `IA32_APIC_BASE.EN = 0` распространяется на INIT и SIPI, как для Fixed; программное выключение через SVR (бит 8 сброшен, `SVR = 0xFF`, `EN = 1`) INIT и SIPI принимает (SDM 10.4.3, по памяти). Записаны в SMP_CONTRACT §3, §5, §6 и MACHINE_CONTRACT.
1. (major) INIT assert, адресованный BSP, — отказ устройства `UnsupportedConfiguration` до изменения состояния (`decode_icr`, комментарий «BSP INIT is deferred to S6-15/S6-16»). Тесты, ждавшие приём INIT к BSP, переписаны на отказ (`local_apic::...init_to_the_boot_processor_is_refused_and_sipi_to_it_is_pending`, `vcpu::...init_to_the_bsp_is_refused_by_the_device...`). SIPI и INIT deassert к BSP принимаются. SMP_CONTRACT §6 исправлен: применяют только для AP, до S6-15 — тесты или ручной вызов.
2. (major) `new_ap` задаёт `guest_efer = 0`; INIT (`apply_startup`) задаёт `guest_efer = 0`, `guest_cr2 = Some(0)`. Выбор `Some(0)`, а не `None`: у BSP при включении `None` читается платой как CR2 0, но `.or(prior_cr2)` при `None` вернул бы устаревший CR2 из `state`; `Some(0)` сбрасывает явно. У `new_ap` оставлено `None` (как у BSP при включении, читается как 0). Тест `init_resets_efer_and_cr2_of_an_ap_and_keeps_the_other_guest_cells`: STAR, LSTAR, CSTAR, SYSENTER, SYSCALL_MASK, KERNEL_GS_BASE, FP и `LongModeState` (кроме `activity`) не меняются.
3. (major) Старый CR0 BSP без `real_mode` и без CR0 в состоянии — `FIRMWARE_CR0` (0x8001_0033), не CR0 включения. Тест `a_direct_init_of_the_bsp_vcpu_takes_cr0_from_the_board_not_from_power_on` ждёт литерал `0x0000_0010` (и `0x4000_0010` для CR0 с CD), посчитанный вручную по формуле `(old & 0x6000_0000) | 0x10`; согласуется с `RealModeState::after_init` (`legacy_mode.rs`, `CR0_INIT_PRESERVED_MASK`, `CR0_ET`).
4. (major, формулировка) Утверждение про `NotRunning` сужено в `MACHINE_CONTRACT.md`, этом отчёте, комментарии `CpuActivity::WaitForSipi` в `state.rs` и новом разделе `CPU_CONTRACT.md`. `execute.rs` не менялся. Формулировка уточнена повторным ревью: only the CPL0 profile set refuses non-Running activity with `NotRunning`; other entry points do not check WaitForSipi, among them `od_isa::execute::run`, `step`, `run_with_maskable_request` and the `run_configured`-based runners (the loop checks only Halted; `!= Running` is checked only under paged CR0 profiles). Закрыть до S6-15.
5. (владелец А) Снятие Remote IRR убрано из `take_startup`; тест `taking_an_init_does_not_clear_the_remote_irr_of_a_level_route_to_this_lapic` заменил прежний.
6. (владелец Б) Тест `init_and_sipi_to_a_target_with_ia32_apic_base_en_clear_are_refused_like_fixed` (INIT, SIPI и Fixed отказывают при EN=0, принимаются после включения) и `a_target_with_the_svr_software_enable_clear_accepts_init_and_sipi`. Записано в SMP_CONTRACT §5.
7. (minor) `CPU_CONTRACT.md`: в перечень значений `LongModeState::activity` добавлен `WaitForSipi`; в конец добавлен раздел «S6-13: `CpuActivity::WaitForSipi`…».
8. (minor) Тест «AP не исполняет до SIPI»: счётчика инструкций AP в плате нет, плата исполняет только vCPU 0; прогон BSP на N шагов без trampoline и Linux-загрузки недоступен. Выбран честный вариант: проверка через неизменность `LongModeState` и `real_mode` AP на всех границах (INIT deassert, SIPI к BSP, пустая граница); оговорка записана в комментарии теста. Плановый «счётчик инструкций AP равен 0» не выполнен буквально — остаётся на S6-15.
9. (minor) `apply_pending_startup` берёт `vcpus.get_mut(index)` и отказывает при отсутствии вместо индексации.
10. (minor, перенесено в S6-15) `idle_to` в `firmware_board.rs` считает все AP `WaitForSipi`; не переделывалось.
11. (minor) Комментарий ICR в `local_apic.rs` переписан короче (в пределах ширины).
12. (minor) Две заметки «решить до S6-13» из раздела S6-11 перенесены в «Перенесено в S6-15» ниже.

Перенесено в S6-15:
- Сброс платы (`reset_checked`/`reset_pair`) не возвращает AP в `WaitForSipi` и не чистит ожидающие события (INIT/SIPI, IRR) в чужих LAPIC.
- Риск записи broadcast `0xFF` в старший dword маршрута IOAPIC: `set_dest` отказывает APIC ID, которого нет среди vCPU, и это фатальная ошибка MMIO.
- `idle_to` считает все AP в `WaitForSipi` (пункт 10) — сделано в S6-15.
- Закрыть до S6-15: все точки входа, кроме CPL0 profile set, проверяют `WaitForSipi` (список в пункте 4).
- Решение владельца (INIT не снимает Remote IRR) при INIT-reset очищает ISR; последующий EOI — no-op; Remote IRR level-маршрута к этому AP остаётся; `LocalApicHandle::reset` этого CPU, `reset_io` и `set_dest` маршрута отказывают; сброс платы в модели блокируется. На железе hardware reset это снимает: артефакт модели, устранить в S6-15 (сброс платы). Места: `crates/od-machine/src/apic_pair.rs` (~322, ~581–585).

Перенесено в S6-16:
- `vcpu.rs` (~159–162): старый CR0 для INIT берётся из `real_mode` раньше, чем из `state.cr0`. После SIPI `real_mode` всегда `Some` и `state` из него не переписывается. Когда S6-16 начнёт исполнять AP и тот уйдёт в protected/long mode, следующий INIT возьмёт устаревший `real_mode.cr0`. Условие S6-16: синхронизировать или сбрасывать `real_mode` при выходе из real mode.

Открыто (оставлено как есть): гостевой WRMSR `IA32_APIC_BASE` (EN) у AP; `hardware_reset_only` у `Shutdown` не моделируется; поведение BSP `activity: None` -> `Running`.

Не сделано: исполнение trampoline и режимов (S6-16, S6-14d+); планировщик, вызов `apply_pending_startup` на границе (S6-15; сейчас только тесты); NMI (S6-12 п. 2); CPUID/MADT/handoff для `N > 1` (S6-15/S7-02); запись `real_mode` в `LongModeState` (в нём нет представления real mode).

Проверки (после исправлений по ревью 2026-11-16):
1. `cargo fmt -p od-isa -p od-machine -p od-testkit`: выполнено, затронуты только файлы среза (первый запуск упал на моей ошибке в `state.rs`, исправлено и повторено). Известные вне-срезовые ошибки `cargo fmt --all` (`crates/od-host/src/storage_abi_v1.rs`, `tools/image-tool/tests/cli.rs`) не трогались.
2. `cargo check --workspace --all-targets --offline`: успешно (прежние предупреждения `MEMORY_CORPUS_SEQUENCES`, `scale`).
3. `cargo test -p od-isa --locked --offline`: все наборы без отказов (9 наборов, 4188 тестов).
4. `cargo test -p od-machine --offline -- --test-threads=1`: lib 1412 passed, 0 failed, 7 ignored; все остальные наборы без отказов.

Ограничения и открытые вопросы:
- Риск из ревью S6-12a: Linux All-but-self (Fixed) к CPU с глобально выключенным LAPIC отказывает всей IPI. Для INIT/SIPI принято то же правило. Проверить на S6-15/S6-18, не нужен ли приём INIT/SIPI выключенным LAPIC (SDM 10.4.3).
- Гостевой WRMSR `IA32_APIC_BASE` (`step_apic_base_msr`) может выключить LAPIC AP; тогда последующие INIT/SIPI к нему отклоняются. Сочетание с INIT-reset (LAPIC после INIT остаётся включённым) отдельно не проверялось.
- INIT к BSP отказывает устройство (отложено до S6-15/S6-16). Нужно решение, как S6-16 переведёт процессор, на котором реально исполняется Linux, в состояние INIT (для BSP в SMP-сценариях Linux INIT к BSP не шлёт).
- `hardware_reset_only` у `Shutdown` не моделируется; INIT всегда возвращает AP из `Shutdown` в `WaitForSipi`.
- Поведение BSP `activity: None` -> `Running` (`Vcpu::activity`) выбрано по смыслу «состояние активности не моделируется».
- INIT-reset LAPIC обнуляет и ICR адресата, и LVT/таймер; точный список регистров по SDM §13.4.7.3 сверен по памяти.

### S6-12b — NMI (реализовано, исправлено по ревью, ожидает повторного ревью)

Дата: 2026-11-16. Чекбокс S6-12 в `STAGE_6.md` не отмечен (пункт 2 «Сделать» закрыт этим срезом, остальное принято в S6-12a). Контракт: [`MACHINE_CONTRACT.md`](../specs/MACHINE_CONTRACT.md#nmi-s6-12b), [`CPU_CONTRACT.md`](../specs/CPU_CONTRACT.md) (раздел S6-12b), [`SMP_CONTRACT.md`](../specs/SMP_CONTRACT.md) §5 и §7. SDM (§6.7.1, §10.8.1) — по памяти, источник в репозитории недоступен.

Что сделано:
- `od-isa/state.rs`: поле `LongModeState::nmi_blocked: bool` (по умолчанию `false`). Прочая семантика состояния не менялась; полные литералы `LongModeState { .. }` без `..Default` (85 в тестах `execute.rs`, по одному в `long_decode.rs`, `state.rs`, `od-testkit/src/execution_tests.rs`, `od-testkit/src/v2.rs`) получили `nmi_blocked: false`.
- `od-isa/interrupt.rs`: `deliver_nmi_cpl0_set` и `NMI_VECTOR = 2`. Общее тело с `deliver_external_interrupt_cpl0_set` (то же `deliver_interrupt_through_gate`; внешний путь вызывает его с `nmi = false`, порядок проверок и результаты внешних прерываний не менялись). Для NMI: IF не условие; `nmi_blocked` — условие (`Cpl0ExternalInterruptRefusal::NmiBlocked`, новый вариант); `Halted` принимается и становится `Running`; `Shutdown`/`WaitForSipi`/неизвестная активность — `ActivityOutsideProfile`; STI-окно держит NMI (`StiShadowActive`). При `Delivered` ставится `nmi_blocked`.
- Снятие блокировки: `IRETQ`. Путей два, точка фиксации в каждом одна: `iretq_cpl0` в `execute.rs` (CPL0-набор, включая возврат в CPL3 — `Retired` и `UserCommitted` проходят через одну фиксацию) и `step_iretq` в `iretq.rs` (локальные профили, paged и CPL3). Флаг сбрасывается только на фиксации; отказавший `IRETQ` его не меняет.
- `od-machine/local_apic.rs`, `apic_pair.rs`: `State::pending_nmi` (одна защёлка); `IpiCommand::Nmi`; `decode_icr` режим `100`: physical без shorthand и All-but-self с `(Level, Trigger) = (0,0)`, поле вектора игнорируется; NMI Self/All и прочее — `UnsupportedConfiguration` до изменения состояния; `PairState::queue_nmi` (все или никто; глобально выключенный LAPIC адресата отказывает всей IPI); `LocalApicHandle::pending_nmi`/`take_nmi`.
- Граница адресата: `LoadedFirmwareBoard::deliver_pending_nmi(index)` (`pub(crate)`, как `apply_pending_startup`: планировщика нет, вызывают тесты) и `NmiBoundary { NoPending, Held, Delivered }` в `vcpu.rs`; `deliver_nmi_at_boundary` в `paged_memory_adapter.rs` (тот же RAM-адаптер, что у `deliver_pending_irq`, без запуска инструкций).
- Документы: раздел «NMI (S6-12b)» в `MACHINE_CONTRACT.md` (заменил абзац «NMI — не реализован (blocked)», поправлены места S6-12a), раздел в `CPU_CONTRACT.md`, фразы в `SMP_CONTRACT.md` §6 и «Not yet», статус S6-12 в `STAGE_6.md`.

Вектор: 2 (шлюз IDT 16 байт по `idtr.base + 32`). Блокировка снимается `IRETQ` (любого из двух путей исполнения); ровно один запомненный NMI: вторая заявка во время блокировки остаётся в защёлке LAPIC, доставляется после `IRETQ`, третья сливается с ней и теряется.

Изменённые существующие тесты: `local_apic::tests::matrix_cells_for_nmi_are_refused_until_they_are_implemented` (S6-12a, ожидал отказ обеих строк NMI) удалён и заменён пятью тестами ниже; остальные ожидания (NMI Self/All — отказ; все прочие клетки) сохранены в `matrix_cells_marked_as_an_error_...` без изменений.

Новые тесты:
- `local_apic::tests`: `nmi_physical_is_pending_in_the_target_lapic_only_and_ignores_the_vector_field`, `nmi_all_but_self_is_pending_in_every_other_lapic_and_not_in_the_sender` (в том числе один vCPU), `nmi_self_all_and_every_other_encoding_are_refused_without_a_change`, `nmi_to_a_globally_disabled_lapic_is_refused_and_a_broadcast_is_all_or_none` (EN = 0 отказывает, SVR-выключенный принимает), `nmi_keeps_one_pending_event_and_further_ones_are_lost` (три NMI — одно событие; INIT-reset очищает защёлку).
- `vcpu::tests::an_nmi_to_a_vcpu_waiting_for_sipi_stays_pending_and_the_sender_has_none`: трёхвCPU-плата, All-but-self и targeted, AP в `WaitForSipi` — `Held`, состояние не меняется.
- `linux_direct::tests::nmi` (плата Linux direct-boot, настоящие кадры и `IRETQ` через CPL0-набор): NMI доставляется через вектор 2 при IF = 0 и IF = 1 (RIP = обработчик, CS шлюза, кадр RIP/CS/RFLAGS/старый RSP/SS, IF сброшен); «второй NMI во время обработчика доставляется после `IRETQ`, третий теряется» (`the_second_nmi_during_the_handler_is_delivered_after_iretq_and_the_third_is_lost`); NMI под STI-окном ждёт одну инструкцию и не теряется; `Halted` просыпается по NMI с IF = 0; шлюз без P — ошибка платы, NMI остаётся ожидающим, нет vCPU — ошибка.
- `od-isa/tests/cpl0_profile_set.rs`: `nmi_uses_vector_two_ignores_if_writes_the_frame_and_sets_blocking`, `a_second_nmi_is_refused_while_blocked_and_delivered_after_iretq`, `a_refused_or_aborted_nmi_does_not_set_blocking` (STI-окно, IST ≠ 0, шлюз без P, сбой записи кадра), `nmi_wakes_a_halted_cpu_and_leaves_other_activities_untouched`, `nmi_from_cpl3_uses_tss_rsp0_and_blocks`. `iretq::tests::a_retired_iretq_ends_nmi_blocking_and_a_refused_one_keeps_it` (путь `iretq.rs`), `state::tests::nmi_blocking_is_off_by_default_and_is_preserved_by_state_clone`.

Ограничения (честно):
- IST в шлюзе NMI на момент среза S6-12b не был поддержан (`UnsupportedIst`); снято срезом S6-12c (см. ниже).
- Доставки в `WaitForSipi` и `Shutdown` нет (`Held`: NMI не теряется). SDM для `Shutdown` с `hardware_reset_only` (SMP_CONTRACT §3, таблица) не моделируется.
- AP после SIPI получает защёлку, но доставить через плату не может: у его `LongModeState` нет CR0/CR4/paging (S6-16); `deliver_pending_nmi` вернёт ошибку платы, пока AP не исполняет Long64.
- Вызова из исполняющего цикла нет: `deliver_pending_nmi` вызывается явно; планировщик S6-15 должен вызывать его на границе адресата перед выбором vCPU. Цикл CPL0-набора (`deliver_pending_irq`) NMI не опрашивает.
- STI-окно (и MOV SS) держит NMI одну инструкцию: консервативно; точное поведение SDM по памяти.
- INIT-reset, сброс LAPIC и глобальное выключение стирают защёлку вместе с `State::default()`: решение реализации, SDM не сверен (открытый вопрос).
- `FragmentState::try_from` (JIT/фрагменты) поле `nmi_blocked` не проверяет: у состояния, в котором оно может быть `true`, `activity`/`sti_shadow` уже `Some`, и адаптер его отвергает.
- NMI от LINT1 и PMU не входят.

#### Исправления по ревью (2026-11-16)

Ревью Opus: принять с условием. Статус: реализовано, исправлено по ревью, ожидает повторного ревью (правки меняют код `vcpu.rs`, `firmware_board.rs`, `od-testkit/src/v2.rs`).

Решения владельца (не обсуждались):
- (а) INIT-reset и `IA32_APIC_BASE.EN = 0` стирают защёлку NMI, как в срезе; оставлено. Дополнение по пункту 3: INIT сбрасывает и `nmi_blocked`.
- (г) NMI, принятый в ICR, до S6-15 не доставляется рабочим циклом; доставка на границе — явный вызов `deliver_pending_nmi` (тесты). Условие S6-15: планировщик вызывает её на границе каждого vCPU.
- (б) IST шлюза NMI: заведена задача **S6-12c** (чтение `IST[n]` из TSS рядом с `read_tss_rsp0`, `blocked`: нет, обязательна до приёмки S6-18) — в `STAGE_6.md` отдельный раздел «S6-12c» после S6-12 и строка в таблице задач; нумерация прежних задач не менялась.

Исправления:
1. (документы) `IRETQ`, завершившийся fault (`#GP`/`#PF`), `nmi_blocked` не меняет; SDM снимает блокировку и при `IRET` с fault. Записано как сознательное отступление (блокировка снимается только успешным `IRETQ`, при fault остаётся до следующего успешного; по памяти, не сверено) в `CPU_CONTRACT.md` (S6-12b), `MACHINE_CONTRACT.md` (блокировка) и `SMP_CONTRACT.md` §7. Код не менялся.
2. (документы) NMI в `Shutdown` (`hardware_reset_only=false`): SMP_CONTRACT §3 требует handler вектора 2, реализация даёт `Held`. Записано в `SMP_CONTRACT.md` §6 как открытое расхождение, владелец — S6-15 (планировщик) либо отдельный срез; до тех пор не реализовано. Код не менялся.
3. (код) `Vcpu::apply_startup` (INIT) сбрасывает `state.nmi_blocked = false` вместе с остальным INIT-reset. Тест `vcpu::tests::init_ends_the_nmi_blocked_state_of_an_ap_and_of_the_bsp` (AP и BSP; SIPI блокировку не трогает).
4. (код) `deliver_pending_nmi`: `real_mode.is_some()` — явная ошибка платы `Config("deliver_pending_nmi: NMI in real mode not modelled")` до изменения состояния, NMI остаётся ожидающим (выбрана ошибка, а не `Held`: `Held` означал бы, что NMI ждёт архитектурного события, а в реальном режиме доставка в модели невозможна вообще, и планировщик S6-15 не должен молча ждать). Проверка стоит после архитектурных удержаний, поэтому AP в `WaitForSipi` остаётся `Held`. Paging берётся из `vcpu.guest_efer` (без расширения API; guest меняет только `SCE`, он допустим в `Long64ProbeConfig`), а не из `FIRMWARE_EFER`; для vCPU с `guest_efer` без LME/LMA — ошибка «no paging for the vCPU». Тест `vcpu::tests::a_started_ap_in_real_mode_is_an_error_for_an_nmi_and_the_nmi_stays_pending`. Ограничение: остальные ходоки платы по-прежнему берут константу `FIRMWARE_EFER` (это прежнее решение S4-38L7f, не менялось).
5. (код) `Held` — только для архитектурных удержаний: `nmi_blocked`, `sti_shadow = Some(Blocking)`, активность `Shutdown`/`WaitForSipi`. `activity: None` и `sti_shadow: None` — явная ошибка платы (`"deliver_pending_nmi: vCPU activity not modelled"`, `"deliver_pending_nmi: STI shadow not modelled"`) до любого изменения состояния. Doc-комментарий обновлён, «never silently dropped» сохранено. Тест `linux_direct::tests::nmi::a_state_that_is_not_modelled_is_an_error_not_a_hold` (оба случая: ошибка, состояние неизменно, защёлка взведена); существующие тесты `Held` (STI-окно `Some(Blocking)`, `WaitForSipi`, блокировка) проходят без изменений.
6. (код) `od-testkit/src/v2.rs`: `IntegerState::try_from(&LongModeState)` и `IntegerState::from_cpu_for` при `nmi_blocked = true` отказывают `FormatError::InvalidField { field: "state.nmi_blocked", .. }`, по образцу `WaitForSipi`/`Shutdown`. Тест `v2_tests::integer_snapshot_export_rejects_nmi_blocking_instead_of_dropping_it` (оба пути; состояние по умолчанию экспортируется).
7. (документы) Ссылки на SDM в `SMP_CONTRACT.md` приведены к нумерации `MACHINE_CONTRACT.md` (Vol. 3A 6.7.1, 10.8.1; пометка «по памяти, источник недоступен»); `§7.7.1–7.7.3` убраны. Фраза в §10: «доставка NMI из планировщика и на границе AP — S6-15/S6-16». Примечание: строка 9 SMP_CONTRACT по-прежнему называет издание 093 как источник остальных разделов (11.4.1, 12.1.1, 13.x) — их нумерацию не сверяли.

Проверки после правок: `cargo fmt -p od-isa -p od-machine -p od-testkit -- --check` — чисто; `cargo check --workspace --all-targets --offline` — без ошибок (предупреждения вне среза: `page_walk.rs` неиспользуемая функция, `od-host`); `cargo test -p od-isa --locked --offline` — все прошли (lib 2335, `cpl0_profile_set` 1601, 0 упавших); `cargo test -p od-machine --offline -- --test-threads=1` — 1713 прошли, 0 упавших, 7 ignored. Новый тест `od-testkit` собран `cargo check --all-targets`, отдельно не запускался.

Проверки (2026-11-16):
1. `cargo fmt -p od-isa -p od-machine -p od-testkit -- --check`: чисто (после `cargo fmt` по этим пакетам; известные вне-срезовые файлы `crates/od-host/src/storage_abi_v1.rs`, `tools/image-tool/tests/cli.rs` не трогались).
2. `cargo check --workspace --all-targets --offline`: успешно (прежние предупреждения `MEMORY_CORPUS_SEQUENCES`, `scale`; предупреждение `access_long64_4k_internal_with_writer` в `page_walk.rs` относится к lib-сборке и не из среза).
3. `cargo test -p od-isa --locked --offline`: все наборы без отказов (2335 + 1601 + 126 + 29 + 6 + 76 + 7 + 15 = 4195 тестов; было 4188, +7).
4. `cargo test -p od-machine --offline -- --test-threads=1`: lib 1423 passed, 0 failed, 7 ignored; все остальные наборы без отказов.

### S6-12c — IST для шлюза NMI (принято ревью Claude Opus/high (2026-11-16), замечания закрыты)

Что сделано (`crates/od-isa/src/interrupt.rs`):
- `read_tss_ist(state, memory, context, ist)` рядом с `read_tss_rsp0`: `ISTn` (n = 1..=7) — qword по `TR.base + 0x24 + 8·(n−1)` (IST7 — байты `0x54..=0x5b`), чтение как обращение к таблице дескрипторов. Правила как у RSP0: нет TR — `MissingTaskRegister`; лимит TR меньше `offset + 7` — `TssIstBeyondLimit`; неканонический адрес (в том числе переполнение `base + offset`) или неканоническое значение — `NonCanonicalIst`; сбой чтения памяти — `Aborted(TssIstRead)`. Все отказы до записи, состояние не меняется, `nmi_blocked` не взводится. Значение стека, не вмещающее кадр, даёт прежний `StackAddressOverflow`.
- `resolve_cpl0_gate` принимает параметр `allow_ist`; `ResolvedGate` получил `ist`, `privilege_change` и `new_stack` (вместо `kernel_rsp`). В `deliver_interrupt_through_gate` при IST ≠ 0 (разрешён только для NMI) стек — `TSS.IST[n]` (приоритетнее RSP0, и из CPL3), кадр те же пять слов со старыми SS и RSP, выровнен вниз до 16; SS на CPL0 не перезагружается, из CPL3 — null SS, как без IST. IST = 0 — поведение прежнее (RSP0 при CPL3, иначе текущий RSP; TSS на CPL0 не читается).
- Заменены поля: `PF`/`GP` и внешняя доставка теперь пишут `gate.new_stack`; поведение без IST не менялось.
- Добавлены `Cpl0ExternalInterruptRefusal::{TssIstBeyondLimit, NonCanonicalIst}` и `Cpl0ExternalInterruptPhase::TssIstRead`.

Тесты (`od-isa/tests/cpl0_profile_set.rs`, 7 новых + 1 исправлен; после ревью добавлены `nmi_from_cpl3_with_an_unreadable_ist_does_not_fall_back_to_rsp0` и `nmi_through_a_trap_gate_with_ist_reports_the_trap_gate`, `ist7_limit_boundary_is_0x5b` (лимит `0x5a` — отказ, `0x5b` — доставка), из `every_ist_entry_of_the_tss_is_read_at_its_own_offset` убрана тавтологичная проверка): `nmi_through_an_ist_gate_stores_the_frame_on_the_tss_ist_stack` (кадр на IST2 `TSS+0x2c`, старый стек нетронут, RSP0 не читается, SS не меняется, `nmi_blocked`), `every_ist_entry_of_the_tss_is_read_at_its_own_offset` (IST1..IST7), `nmi_with_ist_zero_ignores_the_tss_at_cpl0`, `nmi_through_an_ist_gate_refuses_a_missing_or_bad_tss_without_any_write` (нет TR, лимит `0x32`/`0`, неканонические значение, база и переполнение, малый стек; лимит `0x33` проходит), `nmi_through_an_ist_gate_aborts_on_a_failed_tss_read_and_changes_nothing`, `nmi_from_cpl3_through_an_ist_gate_uses_the_ist_stack_not_rsp0`, `ist_is_only_supported_for_the_nmi_delivery`. Тест `a_refused_or_aborted_nmi_does_not_set_blocking`: проверка «IST 2 — `UnsupportedIst`» заменена на «IST 2 без TR — `MissingTaskRegister`» (IST для NMI теперь поддержан). Остальные тесты `UnsupportedIst` (`#PF`, `#GP`, `INT3`, внешний вектор) не менялись и проходят.

Ограничения (честно):
- IST поддержан только для NMI (`deliver_nmi_cpl0_set`). `#PF`, `#GP`, `INT3` и внешний вектор через шлюз с IST ≠ 0 по-прежнему `Refused(UnsupportedIst)` (план: «не менять IST для других векторов»); расширение — снятие запрета `allow_ist` в `resolve_cpl0_gate` и выбор TSS-пути в соответствующей обёртке.
- Вложенный NMI на том же IST-стеке не моделируется как отдельный случай: пока `nmi_blocked`, второй NMI отказывает `NmiBlocked`; стек `IST[n]` при каждой доставке берётся с вершины заново, как по SDM.
- Источник SDM (Vol. 3A 7.7, 6.14.5) по памяти: текста нет в репозитории. Ошибки TSS, которые CPU сообщил бы как `#TS`/`#SS`, здесь явные отказы, без доставки (как у RSP0).
- Адаптер `od-machine` (`paged_memory_adapter.rs`) не менялся. Чтение TSS через общий `GuestMemory`; тестов на уровне платы с IST≠0 нет — критерий «Linux-образец шлюза NMI принимается» подтверждён на уровне od-isa (шлюз 0x8E, CS 0x10, IST 2); плата-тест с IST 2 и TSS — до S6-18.

Проверки (2026-11-16): `cargo fmt -p od-isa -p od-machine -p od-testkit -- --check` — чисто; `cargo check --workspace --all-targets --offline` — без ошибок (прежние предупреждения вне среза); `cargo test -p od-isa --locked --offline` — без отказов (2335 + 1608 + 126 + 29 + 6 + 76 + 7 + 15; было 1601 в `cpl0_profile_set`, +7); `cargo test -p od-machine --offline -- --test-threads=1` — lib 1426 passed, 0 failed, 7 ignored, остальные наборы без отказов.

### S6-15 — планировщик режима чередования (реализовано, принято ревью)

**Статус: принято ревью (код и архитектурное §8, правки внесены 2026-11-16).** Чекбокс в плане отмечен. Блокеры S4-22 и S4-23 закрыты 2026-11-15 (ISSUE-104); работа идёт по их принятым результатам, S4 не переоткрывался и S4-код не менялся.

**Шаг 0 — раздел времени.** `docs/specs/SMP_CONTRACT.md` §8 переписан по принятым S4-22/S4-23 и D7 (STAGE_6.md, раздел 11; этот отчёт, раздел 3): единый владелец часов `ClockOwner` (один `VirtualClockAdvancer`), время гостя виртуальное (инструкция = 1 нс, общее для всех vCPU, TSC = `now_ns × tsc_hz / 10^9` без смещения на vCPU), тики PM/PIT/RDTSC платы начисляются обычными наносекундами, простой пропускается прыжком к ближайшему сроку, host monotonic гостю не виден; арбитр простоя для N vCPU (порядок проверок, правило полезного срока таймера, `NoProgress`/`AllIdle`); квант, seed, расписание, порядок границы — S6-15. Источники: `clock_owner.rs`, `idle_arbiter.rs`, `virtual_clock.rs`, `STAGE_4_REPORT.md` (S4-23g/h/i/j). **Раздел ещё не прошёл архитектурное ревью**, которого требует пункт 0 плана: ревью — за ревьюером. Расхождение описания с кодом устранено правкой документа, код не менялся: строка §8 теперь определяет шаг `REP` как срез частичного прогресса до `REP_ITERATION_WORK_LIMIT` = 1024 итераций (`RunExit::RepIterationBudgetExhausted`), а не одну итерацию (граница — точка, где интерпретатор отдаёт управление). **Открыто (перенесено в S6-19):** проверить, что перебор расписаний покрывает границы REP с учётом 1024-итерационного среза (SMP_CONTRACT §8).

**Что сделано** (`crates/od-machine/src/smp_scheduler.rs`, новый; `lib.rs`; `firmware_board.rs`; контракт — `MACHINE_CONTRACT.md`, «Планировщик режима чередования (S6-15)»):
1. Цикл `SmpScheduler::run` (один host-поток): на каждой границе проверяет STOP и общий бюджет, применяет границу каждого vCPU по порядку индексов (INIT и SIPI — `apply_pending_startup`, затем NMI — `deliver_pending_nmi`; INIT приоритетнее NMI), будит halted vCPU с IF = 1 и принимаемым вектором, затем исполняет ход следующего `Running` vCPU по кругу вызовами `SmpHost::run` (существующий цикл интерпретатора; планировщик семантики исполнения не копирует). Переключение только между шагами; шаг — завершённая инструкция или возврат интерпретатора с частичным прогрессом `REP`.
2. Квант: `QuantumPolicy::Fixed(n)` (по умолчанию 1024, не измерено), `Seeded { seed, min, max }` (SplitMix64, пределы по умолчанию 1..=256, один шаг возможен; алгоритм записан в `MACHINE_CONTRACT.md` и проверен эталонными значениями для seed 0), `Schedule(Vec<usize>)` — явное расписание «чей шаг» (вход для S6-19; `executed_schedule()` выдаёт записанный прогон в этой же форме).
3. Пропуск: `WaitForSipi` и `Shutdown` не исполняются никогда; `Halted` — пока нет принимаемого вектора при IF = 1 либо NMI/INIT/SIPI на границе. **Проверено, что AP вне `WaitForSipi` в рабочих путях недостижимы:** `apply_pending_startup` и `deliver_pending_nmi` вызывают только тесты и (после этого среза, но без подключения к рабочему циклу) `BoardSmpHost`; `vcpu_count > 1` задаётся только конфигурацией платы, ни `od-runner`, ни `linux_direct` его не выставляют; рабочий цикл исполняет один vCPU 0. Через планировщик AP выходит из `WaitForSipi` по SIPI в `Running` с `real_mode = Some`; `BoardSmpHost::run` такой vCPU отказывается исполнять (`Config`, до S6-16), а не пропускает и не гонит как long mode.
4. Простой: когда `Running` нет, один вызов `ClockOwner::idle_step`, часы одним прыжком идут к ближайшему сроку среди очередей событий всех vCPU (правило арбитра), таймеры диспетчеризуются; единственный двигатель времени — `ClockOwner`. Каждый вызов хоста режется `ClockOwner::step_budget`, поэтому моменты тиков не зависят от кванта.
5. Общий бюджет в retired-инструкциях, счётчики `VcpuCounters` на каждый vCPU; STOP читается на каждой границе и передаётся хосту предикатом остановки на каждый шаг хода; при нём ни один vCPU не исполняется дальше.
6. Один vCPU: результат побайтно совпадает с прямым прогоном интерпретатора (состояние, память, число инструкций, часы = число инструкций) при всех политиках (тест `one_vcpu_gives_the_result_of_a_direct_run_under_every_policy`). **Контрольный прогон S6-10 (`od-runner linux-direct … --budget 300000000`, ~2 мин, `sha256` JSON `09997821afbb332c…`) не повторялся:** рабочий цикл `run_linux_boot_cpl0_set` планировщиком не пользуется и не менялся (единственная правка платы на его пути — `idle_to`, и она достижима только при включённом `enable_clock_owner`, чего контрольный прогон не делает), поэтому его результат побайтно тот же по построению, но это не измерено. Проверка одного vCPU сделана на тестовом хосте с настоящим `od_isa::execute`, а не на Linux-образе.
7. `idle_to` (`firmware_board.rs`) строит виды арбитра из `Vcpu::activity()` и IF каждого vCPU (раньше: BSP halted, все AP `WaitForSipi`); добавлены `Board::vcpu_mut` и `clock_owner_mut`. Тест `idle_to_shows_every_vcpu_with_its_own_activity`.
8. `BoardSmpHost`: `SmpHost` для `LoadedFirmwareBoard` с включённым владельцем часов; границу делают `apply_pending_startup` и `deliver_pending_nmi`. Квант исполнения — замыкание вызывающего (см. ограничения).

**Тесты** (`smp_scheduler::tests`, 20, и один в `firmware_board::tests`; на настоящем `od_isa::execute`, тождественная RAM, по одному LAPIC на vCPU на общих часах): эталон SplitMix64 и границы диапазона; отказ политики без кванта; один vCPU равен прямому прогону при шести политиках; один и тот же seed дважды даёт те же счётчики, ячейку и расписание, соседние seed 1..=8 — разные расписания и разные конечные значения гонки (неатомарный инкремент общей ячейки); записанное расписание воспроизводится точно; явное расписание исполняется как задано (потерянное обновление даёт 1, последовательное — 2; префикс; ошибки `ScheduleCpuNotRunning` и `ScheduleCpuOutOfRange`); длинный `REP` (2500 итераций) на vCPU 0 не держит vCPU 1 дольше кванта (первый ход — два шага частичного прогресса, затем ход vCPU 1); все в `HLT`, таймер одного LAPIC будит его, время прыгает к ближайшему сроку (срок vCPU 1 раньше срока vCPU 0), `now = retired + idle`; `WaitForSipi`, `Shutdown` и halted с IF = 0 пропущены, при IF = 1 будится; «никто не работает» — `AllIdle` без движения часов; STOP во время хода второго vCPU (первый отработал квант целиком, второй три шага из десяти, дальше никто, STOP не липкий в планировщике); общий бюджет режет последний ход; ход режется сроком таймера (`[(0, 5), (0, 995)]`); тики устройств начисляются после retired; несовпадение числа vCPU; на плате: INIT и SIPI применяются на границе адресата (INIT раньше SIPI), vCPU в real mode отказывает хост; NMI для AP в `WaitForSipi` остаётся ожидающим (не ошибка), для AP в real mode — ошибка `Config` и NMI остаётся; хосту нужен владелец часов. После ревью добавлено: тест `an_nmi_on_the_boundary_comes_before_the_wake_of_a_halted_vcpu_with_if_clear` (NMI на границе раньше пробуждения halted vCPU с IF = 0 и с IF = 1 при принимаемом векторе: vCPU становится `Running`, путь `wake` не вызывается; границу имитирует `Rig::boundary` — настоящую доставку вектора 2 через IDT и страницы тест планировщика не проверяет: для `BoardSmpHost` она требует платы со страничным разбором и IDT, это покрыто тестами `deliver_pending_nmi` S6-12b, в том числе `a_halted_vcpu_wakes_for_an_nmi`); тест воспроизведения расписания и `race()` сравнивают всю RAM и состояние каждого vCPU. Мутации: убрать счёт частичного `REP` как шага ломает тест `REP`; убрать `step_budget` из хода ломает тест срока таймера.

**Не сделано и ограничения.**
- INIT к BSP — отложен до S6-16 (SMP_CONTRACT §4 / §6; в S6-15 не сделан).
- Сверка с контрольным Linux-прогоном S6-10 перенесена в срез, который подключит планировщик к циклу (S6-16/S6-17); до тех пор — только сверка с прямым прогоном интерпретатора.
- Бюджет считает только retired-инструкции; частичный REP его не расходует; остановка долгого REP — только STOP; предела шагов в S6-15 нет (предел — S6-21).
- Расписание воспроизводит прогон, если хост не зависит от разрезания вызовов и за ход не ставились события границы для исполняющего vCPU. Событие, поставленное во время хода (ICR), применяется на следующей границе — в записанном и в повторном прогоне граница может разойтись. Это ограничение для S6-19.
- `SmpHost::run`: `device_ns` начисляется после вызова целиком; время устройства зависит от числа вызовов — хост обязан вызывать `run` так, чтобы тики не пропускались. Замыкание `BoardSmpHost::run` не должно вызывать `charge_retired` / `charge_ns` / `idle_to` / методы `ClockOwner` (часы двигает только планировщик): условие подключения в S6-16/S6-17.
- Планировщик не подключён к `run_linux_boot_cpl0_set` и paged-пробам: тот цикл исполняет только BSP (общий `page_table_root` на плате, источник IRQ привязан к BSP, журнал COM1 и тики устройств — локальные состояния цикла), а AP до S6-16 исполняться не могут; копировать цикл в `BoardSmpHost` запрещено правилами проекта. Подключение — когда S6-16/S6-17 вынесут квант цикла; `BoardSmpHost::run` принимает его замыканием. Поэтому доставка NMI на границе (`NmiBoundary::Delivered`) на плате через планировщик тестом не покрыта (покрыта прежними тестами `deliver_pending_nmi` S6-12b/c); через планировщик проверены ветки `Held` и ошибка real mode.
- Событие, поставленное во время хода (запись ICR), применяется на следующей границе, после хода; хост не завершает вызов из-за такого события (SMP_CONTRACT §8). Допустимая задержка доставки, но грубее аппаратной для NMI самому себе.
- Ошибка границы (например, NMI в real mode) не обновляет счётчики этой границы.
- Квант по умолчанию 1024 и пределы seed 1..=256 не измерены.
- `page_table_root` (гостевой CR3) остаётся на плате: AP в long mode появится только в S6-16, тогда он и переедет в `Vcpu` (иначе второй vCPU разделит CR3 с первым).
- Из «Перенесено в S6-15» (S6-13) **не сделано и остаётся открытым**: сброс платы не возвращает AP в `WaitForSipi` и не чистит чужие LAPIC; риск broadcast `0xFF` в старшем dword маршрута IOAPIC; проверки `WaitForSipi` в точках входа `od_isa::execute` вне CPL0-набора (планировщик их не вызывает, но сами точки не проверяют); артефакт модели с Remote IRR при INIT (сброс платы). Сделано из этого списка: `idle_to`.
- Перенесено в S6-16: гостевой WRMSR `IA32_APIC_BASE.EN` у AP; `hardware_reset_only` у `Shutdown` (INIT возвращает AP из `Shutdown` в `WaitForSipi`); синхронизация `real_mode.cr0` с `state.cr0` при выходе AP из real mode; исполнение AP (real mode, trampoline); CPUID/MADT/handoff для N > 1 (S7-02).
- Не входит: потоки, приоритеты, «честность» (нет замера), параллельный режим.

**Проверки (2026-11-16).**
1. `cargo fmt -p od-isa -p od-machine -p od-testkit -- --check`: чисто (после `cargo fmt` по этим пакетам; вне-срезовые `crates/od-host/src/storage_abi_v1.rs`, `tools/image-tool/tests/cli.rs` не трогались).
2. `cargo check --workspace --all-targets --offline`: успешно (прежние предупреждения вне среза).
3. `cargo test -p od-isa --locked --offline`: без отказов (2335 + 1611 + 126 + 29 + 6 + 76 + 7 + 15; `od-isa` этим срезом не менялся, рост `cpl0_profile_set` с 1608 до 1611 — не из среза).
4. `cargo test -p od-machine --offline -- --test-threads=1`: lib 1446 passed, 0 failed, 7 ignored (было 1426, +20: 19 планировщика и `idle_to`); все остальные наборы без отказов. Полный набор запущен один раз на коде (после него правились только документы).

#### Архитектурное ревью §8 (2026-11-16)

Правки внесены в `SMP_CONTRACT.md` §8–§10 и `MACHINE_CONTRACT.md`.

- Решение по шагу REP — вариант (а): срез L = `REP_ITERATION_WORK_LIMIT` = 1024 итерации остаётся; L нормативна для версии 1 контракта; граница — точка возврата интерпретатора из незавершённого REP. Вариант L = 1 отвергнут (L как параметр прогона — только если S6-19 понадобится).
- Зафиксировано: единственный двигатель времени — планировщик; хост часы не двигает; тик устройства завершает вызов хоста; ничто, кроме retired-инструкций, тиков и пропуска простоя, времени не стоит; порядок границы планировщика (STOP, бюджет, INIT/SIPI, NMI, пробуждение, ход или `idle_step`); бюджет не ограничивает число шагов при длинном REP; STOP проверяется между итерациями REP; значения кванта по умолчанию и пределы seed временные.
- Открытое перенесено: S6-16 (сроки платы в `SmpHost`, конец вызова на тике и записи в таймер LAPIC, квант-функция из `run_linux_boot_cpl0_set`, внешнее прерывание на частичном REP, NMI в real mode, Fixed в AP; см. STAGE_6.md, S6-16, п. 6); S6-19 (эталон режет REP по L; ограничение `executed_schedule()`); S6-21 (предел шагов для длинного REP); S6-38 (замер темпа и значения кванта по умолчанию).

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
- `reset_checked`/`reset_pair` сбрасывают LAPIC своего процессора и маршруты IOAPIC; общая семантика сброса платы с несколькими LAPIC не решена в S6-13: перенесено в S6-15 (см. раздел S6-13).
- Риск (не решён в S6-13, перенесён в S6-15): гость, пишущий broadcast `0xFF` в старший dword маскированного входа IOAPIC, получит фатальную ошибку MMIO, потому что `set_dest` отказывает APIC ID, которого нет среди vCPU (Linux при `clear_IO_APIC`/маскировании может писать такое значение).
- Риск для S6-15 (решить в S6-15): при N > 1 смена affinity level-входа записью «старший dword, потом младший» отказывается `set_dest`, если вход ждёт EOI или линия высока (SMP affinity в Linux меняет адресат уже работающего входа); отказ — фатальная ошибка MMIO.
- Риск для S6-13/S6-15 (провести до запуска AP): индекс vCPU не проводится от `Vcpu` на рабочих путях MMIO — `SetStepMmio` (CPL0-цикл), `read_physical`/`write_physical` и `AdapterBus::RamMmio` всегда обслуживают процессор 0; индекс несёт только `AdapterBus::Machine(bus, vcpu)`, и пока его создаёт лишь тест.
- Graphify: перед правками `graphify query` не выполнялся (в worktree графа не было); после правок `GRAPHIFY_MAX_WORKERS=8 graphify update .` выполнен (46426 узлов, 129065 рёбер).

### S6-12a — доставка IPI (Fixed, shorthand, физический адресат)

Реализовано 2026-11-15. Независимое ревью Claude Opus / high (2026-11-16): блокеров и major нет, срез принят как частичная реализация S6-12. Владелец 2026-11-16 подтвердил: (а) ожидание теста Self `(0,0)` заменено, (б) два консервативных решения (отказ всей IPI при выключенном LAPIC адресата или level-резервировании вектора) — записаны в SMP_CONTRACT §5. Замечания ревью без блокеров: тест level-резервирования при shorthand не добавлен (необязательно); гостевой WRMSR `IA32_APIC_BASE` (EN=0) — открытый вопрос для S6-13/S6-15 (Linux All-but-self к выключенному CPU станет ошибкой устройства). Статус S6-12 — `in_progress`, NMI открыт. Линия S4-38 завершена (отмечено владельцем 2026-11-16) и больше не блокирует `od-isa/state.rs`. Чекбокс в плане не отмечен. Контракт: [`MACHINE_CONTRACT.md`](../specs/MACHINE_CONTRACT.md#доставка-ipi-s6-12a); источник — матрица [`SMP_CONTRACT.md`](../specs/SMP_CONTRACT.md) §5.

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
