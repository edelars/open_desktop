# Этап 6. Промежуточный отчёт

Статус отчёта: **промежуточный; этап не принят**. Сверка выполнена 2026-11-13; ветка `codex` основана на `main` `7f5546c`. На момент подготовки отчёта `main` продвинулся до `6ecbd10`: S4-38L9t принят, следующая диагностическая граница — `SUB RAX, GS:[RIP]` (арифметика с GS-префиксом). План S6 подготовлен, но ни одна задача реализации этапа 6 не принята. Этап 4 остаётся открытым; S4-21, S4-22, S4-23 и другие внешние зависимости ниже не приняты.

## 1. Сводка задач

`not_started` означает, что задача не принята после ревью. `blocked` указывает внешние незакрытые предпосылки; эти блокировки наследуются зависимыми задачами по [плану S6](../plans/STAGE_6.md#6-задачи-и-минимальные-зависимости). S6-00 подготовлен этим срезом, но до независимого ревью остаётся `not_started`.

Правило D5 о согласовании с линией S4-38 относится только к задачам и зависимым цепочкам, меняющим `od-isa`; оно не блокирует документные задачи S6-00…09 или S6-40.

| Задача | Статус | Внешние блокеры |
|---|---|---|
| S6-00 | `accepted` | независимое ревью пройдено; коммит [`38f4c81`](https://github.com/edelars/open_desktop/commit/38f4c81) |
| S6-01 | `not_started` | Методический срез прошёл ревью и закоммичен как [`5183584`](https://github.com/edelars/open_desktop/commit/5183584); numeric calibration pending для S6-05, S6-08/S6-09, S6-19/S6-36, S6-34/35/37 |
| S6-02 | `in_progress` | Первый evidence-срез по закреплённому Linux artifact и выбору INIT/SIPI; полный AP inventory и архитектурная приёмка ещё не выполнены. См. [`ap-start-inventory.json`](data/stage6/ap-start-inventory.json). |
| S6-03 | `not_started` | — |
| S6-04 | `accepted` | Документный контракт модели памяти прошёл независимое ревью `gpt-6.1-sol/high`; [коммит `4d65fbc`](https://github.com/edelars/open_desktop/commit/4d65fbc). `git diff --check` чист; `check_layout.py` сообщает только 45 существующих битых ссылок на отсутствующие work evidence артефакты, новых S6 ошибок нет. Graphify обновлён на этот коммит. Документ фиксирует x86 fences, split accesses, A/D compare-and-retry и commit/callback granularity; реализация и выбор host primitive не входят. Отдельный descriptor side-effect owner добавлен в S6-26. См. [MEMORY_MODEL.md](../specs/MEMORY_MODEL.md). |
| S6-05 | `not_started` | — |
| S6-06 | `not_started` | — |
| S6-07 | `accepted` | Независимое ревью `gpt-6.1-sol/high` прошло; [коммит `8b19597`](https://github.com/edelars/open_desktop/commit/8b19597); Graphify обновлён после inventory commit. Основной inventory: 233/233 совпадений, exact file:line coverage без пропусков; secondary ownership counts 25/218/7/6; все focused paths имеют owner, production assumptions без владельца не обнаружены. `git diff --check` чист; `check_layout.py` сообщает только 45 существующих work-evidence ошибок, S6 links clean. Код не менялся. |
| S6-08 | `not_started` | — |
| S6-09 | `not_started` | — |
| S6-10 | `blocked` | S4-21 |
| S6-11 | `blocked` | S4-21 (через S6-10) |
| S6-12 | `blocked` | S4-21 (через S6-10) |
| S6-13 | `blocked` | S4-21; согласование кода `od-isa` по D5/S4-38 |
| S6-14 | `not_started` | срезы a/b доступны; срез кода c — S4-21 и согласование по D5/S4-38 |
| S6-15 | `blocked` | S4-21, S4-22, S4-23 |
| S6-16 | `blocked` | S4-21, S4-22, S4-23 |
| S6-17 | `blocked` | S4-13 (полный срез), S4-14 (полный срез), S4-21, S4-22, S4-23 |
| S6-18 | `blocked` | S4-21, S4-31, S7-02 |
| S6-19 | `blocked` | S4-21, S4-22, S4-23 (через S6-15) |
| S6-20 | `blocked` | S4-21, S4-22, S4-23 (через S6-15/S6-19); S4-38 для `od-isa` |
| S6-21 | `blocked` | S4-13, S4-14, S4-21, S4-22, S4-23, S4-31, S4-39, S4-38 (D5), S7-02 |
| S6-22 | `blocked` | S4-13, S4-14, S4-21, S4-22, S4-23, S4-31, S4-39, S4-38 (D5), S5-06, S5-07, S7-02 |
| S6-23 | `blocked` | S4-21, S4-22, S4-23; согласование S4-38 для зависимого S6-20 |
| S6-24 | `blocked` | S4-21, S4-22, S4-23; согласование S4-38 для цепочки S6-13/S6-14c |
| S6-25 | `blocked` | S4-09 (полный объём; принятый ограниченный срез не закрывает задачу), S4-21, S4-22, S4-23 (через S6-20) |
| S6-26 | `accepted` | Документный контракт восьми descriptor-writing paths прошёл независимое архитектурное ревью `gpt-6.1-sol/high`; замечаний не осталось. Реализация документации — коммит `422d0a3`; `git diff --check` чист; `check_layout.py` сообщает 45 прежних broken links в work evidence, новых в изменённых docs нет. SMP реализация остаётся gate для S6-30/S6-31. |
| S6-30 | `blocked` | S4-21, S4-22, S4-23; S4-38 для кода `od-isa` |
| S6-31 | `blocked` | S4-09, S4-21, S4-22, S4-23; S4-38 для кода `od-isa` |
| S6-32 | `blocked` | S4-21, S4-22, S4-23; S4-38 для кода `od-isa` |
| S6-33 | `blocked` | S4-21, S4-22, S4-23 |
| S6-34 | `blocked` | S4-09, S4-21, S4-22, S4-23; S4-38 для кода `od-isa` |
| S6-35 | `blocked` | S4-09, S4-21, S4-22, S4-23, S9-24, S9-25 |
| S6-36 | `blocked` | S4-09, S4-21, S4-22, S4-23 |
| S6-37 | `blocked` | S4-09, S4-13, S4-14, S4-21, S4-22, S4-23, S4-31, S4-39, S5-06, S5-07, S7-02 |
| S6-38 | `blocked` | S4-09, S4-13, S4-14, S4-21, S4-22, S4-23, S4-31, S4-39, S5-06, S5-07, S7-02 |
| S6-40 | `blocked` | S3-25, S3-28, S4-15, S4-16 |
| S6-41 | `blocked` | S3-25, S3-28, S4-09, S4-15, S4-16, S4-21, S4-22, S4-23 |
| S6-42 | `blocked` | S3-25, S3-28, S4-09, S4-15, S4-16, S4-21, S4-22, S4-23 |
| S6-43 | `blocked` | S3-25, S3-28, S4-09, S4-15, S4-16, S4-21, S4-22, S4-23, S5-12 |
| S6-50 | `blocked` | блокировки S6-38 |
| S6-51 | `blocked` | S4-13, S4-14, S4-21, S4-22, S4-23, S4-31, S4-39, S7-02, S7-13 |
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

На 2026-11-13 ответов владельца на D1–D9 не получено. До ответа действуют следующие значения по умолчанию из плана; автор не принимает их от имени владельца:

| Вопрос | Действующее значение по умолчанию |
|---|---|
| D1 — остановка vCPU и snapshot barrier | Этап 6 отвечает за остановку vCPU и их состояние; S9-28 — за полный barrier с I/O, epoch и публикацией; формат остаётся у S9-24/S9-25. |
| D2 — запуск AP | INIT/SIPI с trampoline; окончательный выбор после фактов S6-02. |
| D3 — число vCPU | Два функционально доступны; «поддержано» и 4 vCPU определяются только измерениями S6-38; VM bundle не меняется. |
| D4 — x2APIC | Не входит, пока S6-02 не подтвердит требование выбранной ОС. |
| D5 — код до S4-21 и правки `od-isa` при S4-38 | Машинный код S6-10+ ждёт S4-21; код `od-isa` согласуется с владельцем S4-38, один автор на файл. До этого доступны S6-00…09 и S6-14a/b. |
| D6 — `CMPXCHG8B`/`CMPXCHG16B` | Однопроцессорная семантика — линия этапа 2; этап 6 даёт атомарный примитив, когда его потребует S6-31. |
| D7 — воспроизводимость seed | Побайтная воспроизводимость, если S4-22 использует счётчик инструкций; иначе временное поведение ослабляется отдельным решением. Этап 6 свою модель времени не вводит. |
| D8 — split lock и обычные невыровненные доступы | `LOCK` всегда атомарен; до S6-34 неподдерживаемые split lock дают явную ошибку со счётчиком. Обычные невыровненные доступы могут быть видны по частям; усиление — только по замерам. |
| D9 — режим по умолчанию | До G5/G8 только режим чередования; более одного vCPU помечается «функционально доступно, без ускорения». После gates параллельный режим по умолчанию, чередование остаётся для диагностики. |

## 4. Документные расхождения и передача

- Roadmap задаёт 2–4 vCPU, а `VM_BUNDLE.md` — 1–32 с неуказанным основанием. По умолчанию D3 схема bundle не меняется; поддержанный максимум определяется замерами S6-38.
- План этапа 7 относит SMP и x2APIC к этапу 6, хотя roadmap не задаёт x2APIC. По умолчанию D4 x2APIC исключён до подтверждения S6-02.
- `CPU_CONTRACT.md` говорит, что real-mode entry не требуется, тогда как гипотеза CPU-06 в `WINDOWS_PLATFORM_REQUIREMENTS.md` связывает запуск второго CPU с INIT/SIPI и real-mode trampoline. D2 остаётся предварительным до S6-02.
- В [отчёте этапа 9](STAGE_9_REPORT.md) строка S9-26 ошибочно называет UEFI зависимостью этапов 6/8; UEFI относится к этапу 7. Подтверждённая документальная ошибка зарегистрирована как [ISSUE-086](../KNOWN_ISSUES.md#issue-086-s9-26-wrong-uefi-stage-reference). Отчёт этапа 9 не менялся. **Передача владельцу S9:** исправить ссылку S9-26 на этап 7 в `STAGE_9_REPORT.md`; до этого ISSUE-086 остаётся открытой.

## 5. Проверка этого среза

S6-00: независимое ревью diff и контрактных границ агентом Codex `gpt-6.1-sol` / `high` пройдено; локальные ссылки в изменённых документах — 0 broken; `git diff --check` чист. `python3 tools/check_layout.py` сравнен с архивом базового `HEAD` `7f5546c`: базово exit 1 / 46 ошибок, сейчас exit 1 / 45, новых ошибок 0. Оставшиеся диагностики относятся к старым ссылкам на отсутствующие ignored Stage 3 evidence в `work/`.

S6-01 методический срез: независимое ревью `gpt-6.1-sol` / `high` — pass по формату, источникам, SB/SC, двум частям sensitivity и матрице 2/4 vCPU; `git diff --check`, JSON-пример и локальные ссылки проверены. Полный S6-01 остаётся `not_started` до numeric calibration gates из строки задачи и таблицы `SMP.md`. Исполнение этапа 6, запуск гостя, CPU-тесты и сборка не выполнялись: документ не заявляет поведения SMP.

### S6-02 — промежуточный evidence-срез

S4-35 Linux artifact найден в корневом checkout `main` в ignored-каталоге `work/stage4/linux-7.0.0-38.38/`; тот же комплект есть ещё в 10 из 127 зарегистрированных worktree. Поиск не обнаружил готового S6-02 inventory до этой задачи. По запросу владельца каталог скопирован в `codex/work/stage4/linux-7.0.0-38.38/`; повторная сверка всех 10 файлов с S4-35 manifest дала 20/20 совпадений между `main` и `codex`, без изменений в `main`. Повторные SHA-256 сверены с [S4-35 manifest](data/stage4/s4-35-linux-artifact.json): `vmlinuz` `9f55ef72…b27e`, `vmlinux` `ccaf5904…5034f`, `vmlinux-debug` `7e460f04…9d1f`, config `6cb18717…d32d`, архив исходников из Ubuntu source package `1610ed02…6222`. Config включает `CONFIG_SMP=y`, `CONFIG_X86_X2APIC=y`, `CONFIG_ACPI_MADT_WAKEUP=y`, `CONFIG_AMD_MEM_ENCRYPT=y`, `CONFIG_ACPI_SLEEP=y`, `CONFIG_HOTPLUG_CPU=y`. При этом S4-35 явно оставляет `authenticated=false`: совпадение файловых хешей не подтверждает подписанную source-to-binary цепочку.

По Linux `7.0.0-38.38` source tree, `CONFIG_ACPI_MADT_WAKEUP=y` позволяет обработать MADT type `0x10` Multiprocessor Wakeup и установить mailbox-вариант `wakeup_secondary_cpu_64`. `do_boot_cpu()` сначала проверяет `wakeup_secondary_cpu_64`, затем `wakeup_secondary_cpu`, и только без обоих callbacks вызывает native `wakeup_secondary_cpu_via_init`; отсутствие type `0x10` само по себе выбор не определяет. Native xAPIC path посылает INIT assert, ждёт завершения ICR, выдерживает выбранную `cpu_init_udelay`, посылает INIT deassert, а при integrated APIC — до двух SIPI на физический APIC ID. Для современного Intel/AMD/Hygon CPU default INIT delay равен нулю; каждая SIPI получает 10 мкс до проверки доставки и ещё 10 мкс после ожидания ICR. Старый APIC/CPU путь использует 10 мс INIT delay и 300/200 мкс после SIPI; boot option `cpu_init_udelay=` может переопределить значение.

Рекомендация для архитектурного ревью D2 — INIT/SIPI с 16-bit real-mode entry, далее protected 32-bit и long mode. На текущей плате MADT содержит один Local APIC CPU и не публикует MP Wakeup mailbox, а зафиксированный проектом первый SMP probe также требует INIT/SIPI. Поэтому MP Wakeup type `0x10` не следует публиковать в нашем MADT. Условие выбора Linux точнее: `do_boot_cpu()` вызывает `wakeup_secondary_cpu_64`, затем `wakeup_secondary_cpu`, иначе native `wakeup_secondary_cpu_via_init`; альтернативный callback может прийти не только из MP Wakeup entry, но и из APIC/platform override. При стандартном xAPIC пути проекта рекомендация — native INIT/SIPI, но выбранный callback пока не подтверждён запуском гостя. Рекомендация согласуется с кандидатом xAPIC, статус которого в S4-00c всё ещё `selected-but-unverified`.

Дополненный статический инвентарь закрепляет выбор Linux APIC driver и условные режимы: для x86_64 default — physical-flat xAPIC с физическими APIC ID; 2/4 vCPU с ID 0–3 адресуются без x2APIC. `CONFIG_X86_X2APIC=y` означает только наличие кода: фактический выбор зависит от CPUID/APIC_BASE, interrupt remapping и гипервизора; x2APIC может использовать physical или cluster destination, а его регистры и 64-битный ICR доступны через MSR. Маршрутизация IOAPIC/MSI получает destination mode выбранного драйвера; targeted startup IPI использует физический AP ID, а не shorthand. Зафиксированы shorthand для all/all-but-self/self и Linux topology CPUID leaf fallback цепочки Intel/AMD; guest leaf values остаются неизвестны без закреплённого CPU profile. Ожидаемые MADT type 0/type 9 записи и processor namespace описаны в JSON.

Статически просмотрены и пути NMI: обычный AP startup NMI не посылает; остановка при reboot/shutdown применяет NMI fallback после таймаута; при panic `crash_smp_send_stop()` с NMI shootdown используется, когда включён `crash_kexec_post_notifiers`, а обычная ветвь вызывает `smp_send_stop()` и может дойти до его условного NMI fallback; restart имеет NMI shootdown, KGDB — NMI roundup, а perf hard-lockup detector собран при включённой конфигурации. Исполнение этих ветвей зависит от runtime и не подтверждено запуском гостя. Поэтому прежние пробелы о destination/shorthand/topology/NMI закрыты только на уровне исходного кода, а фактический режим и работа путей в госте остаются открытыми.

Полный S6-02 остаётся открытым: inventory пока не содержит полного списка инструкций/байтов с корректным 16-bit decode. Рабочая машина — macOS `arm64`; доступный Apple LLVM `llvm-objdump` декодирует 32/64-bit x86, но не имеет 16-bit режима; Docker API недоступен. До архитектурной приёмки требуется проверить полный byte inventory штатным x86 toolchain. CPU-06 для Windows остаётся гипотезой по правилу самой задачи: источник о выборе INIT/SIPI и real-mode trampoline для обычной Windows x64 не найден; сверить трассой на этапе 8. ACPI 6.6 требует Processor Local APIC MADT entry и соответствующий processor device object в DSDT (§5.2.12.2; для x2APIC — §5.2.12.12); объект объявляется под `\_SB` с `_HID=ACPI0007` и matching `_UID` (§8.4). Текущая прошивка проекта размещает namespace в DSDT. Microsoft namespace guidance, найденное в этом срезе, прямо ограничивает область SoC-based platforms.

Начальный evidence-срез и текущее расширение APIC/NMI/topology независимо проверил `gpt-6.1-sol` / `high` — pass. В текущем ревью исправлены требования ACPI 6.6 к DSDT, условность panic NMI shootdown и topology leaf fallback; повторно сверены 26 SHA-256 исходных файлов. Полная задача S6-02 не принята. Проверки: JSON parse и `git diff --check` прошли; `check_layout.py` сейчас сообщает 45 сломанных локальных ссылок в документах Stage 2/3 и реестре проблем; ошибок, относящихся к изменённым S6-файлам, нет.


### S6-07 — inventory однопроцессорных допущений (in progress)

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
