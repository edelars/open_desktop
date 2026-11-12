# Этап 4. Системная память, прерывания и машина с одним CPU — пошаговый план реализации

Статус: **план подготовлен; этап 4 не завершён**. Исходное состояние проверено 2026-11-04; принятые bounded slices отмечаются ниже отдельно, а родительская задача закрывается только после выполнения полного критерия. Чекбокс означает принятую реализацию с указанными проверками и ревью, а не наличие файла или описания.

Объём задаёт [этап 4 дорожной карты](../ROADMAP.md#этап-4-системная-память-прерывания-и-машина-с-одним-cpu). Обязательные границы: [архитектура](../ARCHITECTURE.md), [карта проекта](../PROJECT_LAYOUT.md), [CPU contract](../specs/CPU_CONTRACT.md), [JIT contract](../specs/JIT_CONTRACT.md), [виртуальная плата](../specs/VIRTUAL_BOARD.md), [профиль гостя](../specs/GUEST_REQUIREMENTS.md). Этот файл декомпозирует существующий этап, а не заменяет эти документы.

## 1. Что должно работать в конце

Один собственный x86 vCPU исполняет системный код через настоящую память машины: гостевые физические адреса, page tables, права доступа, исключения и устройства. Собственное тестовое ядро проверяет память и прерывания. Зафиксированный Linux достигает настоящего userspace через документированный direct boot.

Проверяемые результаты:

1. RAM, ROM/reserved/MMIO и отдельное пространство port I/O имеют одну согласованную карту. Гостевые 4 KiB страницы не отождествляются с host-страницами, в частности 16 KiB на основном Mac.
2. Поддержанные режимы paging дают правильные переводы, права, A/D effects, CR2 и код `#PF`. Исключения доставляются через CPU-механизм, а не только возвращаются тестовому вызову.
3. GDT/IDT/TSS, необходимые переходы CPL, возвраты, HLT и прерывания образуют работающий путь исполнения одного CPU.
4. UART, Local APIC, IOAPIC, таймер и минимальная PCI-конфигурация доступны гостю согласно зафиксированной плате. Linux обнаруживает именно реализованные устройства.
5. Production RAM/MMU работают с интерпретатором и с поддержанным memory-подмножеством JIT. Изменения кода через физические alias не оставляют исполняемые устаревшие блоки.
6. Собственный `/init` действительно работает в CPL3 выбранного Linux; есть наблюдаемые системные вызовы, возврат в userspace и прерывание таймера. Баннер ядра или строка, напечатанная host runner, успехом не считаются.

Первый вертикальный срез: physical RAM API → uncached page walk → корректное состояние Long64 с реальными page tables → маленький собственный guest probe → polling UART. Затем добавлять исключения, CPL3 и IRQ; Linux запускать после этого. Не объявлять Long64 без требуемого paging архитектурно корректным способом загрузки.

За пределами этапа: SMP, межпроцессорные shootdown и полный TSO этапа 6; x2APIC; рабочие NVMe/NIC/xHCI и долговременные Linux-нагрузки этапа 5; полная UEFI, Windows и весь набор ACPI этапа 7; графика и GUI. Initramfs позволяет получить userspace без диска. Минимальные firmware-таблицы, без которых выбранный direct boot не работает, являются явной ограниченной зависимостью этапа 4.

## 2. От чего начинать, а что пока не считать готовым

По состоянию на 2026-11-04:

| Область | Имеющиеся основания | Ограничение для этапа 4 |
|---|---|---|
| CPU | В этапе 2 реализованы многочисленные точные формы инструкций и отдельные системные случаи | Это не полный системный CPU. Ограниченный caller-driven interrupt delivery и same-CPL0 IRETQ не доказывают работу асинхронных IRQ, TSS и переходов CPL |
| Режим исполнения | Текущий `InstructionMode` поддерживает Long64 | Нельзя получить real/protected/compat mode изменением metadata fixture. Нужные режимы добавляются в `od-isa` как задачи этапа 2 |
| Память | Есть тестовые memory adapters; один вызов `GuestMemory` read/write имеет контракт complete-or-no-change | Production owner RAM/MMU в `od-machine` ещё не реализован. Контракт вызова не означает rollback всей инструкции или page walker A/D effects |
| JIT | Есть локальные memory slices, cache, writer transactions и retained handles | Fixture-bounded владение памятью не доказывает интеграцию с production RAM/MMU. Полный этап 3 и его завершающая physical memory gate ещё открыты |
| Плата | Существует целевой профиль `VIRTUAL_BOARD.md` | Описание APIC, PCI и UART не является их реализацией |
| Linux | Зафиксирован кандидат Ubuntu Generic amd64 `7.0.0-38.38`, `vmlinuz-7.0.0-38-generic`; исследован boot header | Статический header не доказывает загрузку, полный CPU/MSR профиль или корректный handoff. Точные hashes и ограничения находятся в [Linux profile](../reports/STAGE_2_LINUX_PROFILE.md) |

Фактические статусы брать из [плана этапа 2](STAGE_2.md), [его отчёта](../reports/STAGE_2_REPORT.md), [плана этапа 3](STAGE_3.md), [его отчёта](../reports/STAGE_3_REPORT.md). Не переписывать их в «этап завершён» ради начала следующего.

Этап 4 начинается параллельно этапу 3. Отсутствие полного JIT не блокирует physical RAM, page walker и интерпретаторный путь. Отсутствие конкретной CPU-семантики блокирует зависящий от неё guest-сценарий: оформить точную задачу в этапе 2 и реализовать её там. Fallback разрешён только в действительно работающий интерпретатор.

Перед каждым срезом сверять [реестр проблем](../KNOWN_ISSUES.md): особенно restart/fault state для stack/string операций, startup CR0/CR4/EFER, ограничения LOCK, production JIT memory ownership и происхождение Linux artifact. Не считать injected fault независимым физическим доказательством. Не копировать в реестр весь будущий функционал этого плана.

## 3. Правила работы для исполнителя

1. Прочитать корневой `AGENTS.md`, [карту проекта](../PROJECT_LAYOUT.md), этот план и вложенные `AGENTS.md` по пути каждого изменяемого файла. Проверить пользовательский diff до правок.
2. Брать одну задачу с выполненными обязательными зависимостями. В таблице ниже указаны минимальные зависимости; внутри задачи дополнительные условия обязательны. Если обнаружена новая зависимость, записать её до реализации.
3. Для discovery сначала выполнить Graphify query; читать исходники по найденным путям. Использовать существующие CPU/JIT контракты, не строить второй движок в runner или testkit. Внешнюю документацию библиотек получать через Context7; семантику x86 и boot протоколы — из первичных стандартов с сохранённой версией и разделом.
4. По правилам проекта: реализация кода — `gpt-6-luna`, reasoning `high`; архитектурные решения — `gpt-6-astra`, `high`; независимое ревью — `gpt-6.1-sol`, `high`. Разделять независимые роли, не поручать двум авторам одновременно связанные правки. При недоступности роли явно записать ограничение.
5. Перед кодом перечислить наблюдаемое поведение, точные поддержанные режимы и отрицательные случаи. Не вводить новый общий framework для одной задачи.
6. Реализовать минимальный рабочий срез, выполнить соответствующие проверки, передать diff и затронутые контракты на отдельное ревью. Исправить подтверждённые замечания и повторно проверить затронутую часть. Финальное ревью S4-45 не заменяет ревью каждой завершённой задачи.
7. Для Rust: форматирование, сборка затронутых компонентов и необходимые поведенческие тесты. При изменении структуры — `python3 tools/check_layout.py`; итоговая host-проверка — `cargo fmt --all -- --check` и `cargo check --workspace --all-targets --offline`. Firmware и freestanding kernel имеют отдельную сборку; успешный host workspace check их не проверяет.
8. После изменения кода обновить Graphify. В отчёте фиксировать commit/diff, сценарии, результаты и ревью; обновлять чекбоксы только по доказательствам. Новые подтверждённые проблемы и ещё не проверенные гипотезы записывать в `KNOWN_ISSUES.md` с различимым статусом.
9. Не выполнять следующую архитектурную возможность «заодно». Не заменять недостающий opcode host-функцией в loader. При unsupported сохранять RIP, bytes, mode, причину и минимальный воспроизводимый сценарий.

### Рабочие документы и размещение

При реализации создать `docs/specs/MACHINE_CONTRACT.md`, `docs/testing/MACHINE.md`, `docs/reports/STAGE_4_REPORT.md`. В них соответственно хранить контракт машины, методику и факты; ссылки на CPU/JIT/board спецификации должны заменять дублирование их нормативных правил. Малые доказательства — `docs/reports/data/stage4/`, большие локальные артефакты — ignored `work/stage4/<run-id>/`.

| Содержание | Владелец и место |
|---|---|
| CPU state, CR/MSR semantics, decode, descriptors, delivery/return, interruptibility | `crates/od-isa/src/` |
| RAM, physical map, translation, TLB, bus, vCPU orchestration, время и устройства | `crates/od-machine/src/` |
| Блоки JIT, cache keys, code dependencies, dispatcher и invalidation | `crates/od-jit/src/` |
| Host executable memory и ABI glue | `crates/od-host/src/`, `include/open_desktop/` |
| Host lifecycle/IPC | `apps/vmhost/Sources/`, существующие версии `protocols/control/` |
| Сравнение и генерация тестов без production CPU-семантики | `crates/od-testkit/src/` |
| Запуск машины, staging, orchestration сборок | `tools/runner/src/`, `tools/build/`, `tools/bench/` |
| System-сценарии и малые собственные guest fixtures | `tests/system/`, `tests/fixtures/guest/stage4/` |
| Ограниченные ACPI data artifacts, если нужны | `firmware/uefi/src/`, отдельная freestanding guest-target сборка |

Имена новых модулей внутри этих каталогов выбирать после чтения текущих исходников. Пути выше задают владельца, а не разрешение заранее создать десятки пустых файлов. Новые пути отражать в карте; новые верхнеуровневые компоненты этому этапу не нужны.

## 4. Неизменяемые границы корректности

### 4.1. Physical memory и translation

- Одна конфигурация `od-machine` определяет RAM, reserved ranges, MMIO, APIC/IOAPIC/ECAM, IRQ routes и boot reservations. E820, boot data и firmware-таблицы — представления этой конфигурации. Port I/O имеет отдельное адресное пространство.
- Все диапазоны проверяются на overflow, overlap и выход за physical address width. Для holes, ROM writes и MMIO задать точное поведение платы. Host allocation failure, повреждённый указатель и host panic не превращаются автоматически в гостевой `#PF`.
- MMU не предполагает совпадения host и guest page size. Две guest-страницы внутри одной host-страницы могут иметь разные права, code versions и TLB entries.
- Page walker читает гостевые page tables через physical memory. Права зависят от всех уровней и текущего CPU-контекста, включая выбранные WP/NXE/US и прочие реально объявленные features. LA57/PCID/SMEP/SMAP/PKE включать только при реализованной семантике и тестах.
- Запись PTE сама по себе не равна `INVLPG`. Различать архитектурные правила invalidation, внутренние версии объектов и допустимые консервативные flush. Не вводить безусловный «flush на любую RAM write» как определение x86.
- Для cross-page read/write сначала определить контракт одного memory-вызова и все необходимые проверки; не повторять MMIO side effect при retry. A/D bits page walk и уже завершённые REP iterations могут сохраняться после fault: универсальный rollback инструкции запрещён.

### 4.2. CPU, события и JIT

- CPU-семантика остаётся в `od-isa`; `od-machine` предоставляет память, устройства и pending events. Не дублировать CR/MSR, interrupt priority или exception frame в машине.
- `#PF` включает правильный linear address в CR2, error code, сохранённые RIP/состояние и порядок конкурирующих faults. Ошибка при delivery может породить другое исключение, `#DF` или определённый machine shutdown/triple-fault outcome; это не host panic.
- HLT, guest exception, IRQ pending, pause, STOP и исчерпание бюджета — разные состояния. IF, interrupt shadow и приоритеты проверяются на архитектурной границе исполнения; JIT должен выходить к ней с восстановленным состоянием.
- RAM принадлежит машине; JIT удерживает только разрешённые контрактом handles/leases. Host executable allocations принадлежат `od-host`. Не добавлять цикл зависимостей и production-зависимость на testkit.
- Любая успешная модификация исполняемых физических bytes через CPU, alias, loader или другой writer уведомляет механизм code invalidation до следующего lookup/исполнения. Dirty active block должен выйти до исполнения устаревших инструкций. Не завершённая запись не сообщает фиктивный commit; отдельно учитывать реально совершившиеся A/D/partial effects.
- Для RAM/MMIO не предполагать одинаковые правила atomic RMW. LOCK, исключения и ordering берутся из поддержанного CPU-профиля; SMP и DMA не объявляются готовыми на основании single-vCPU теста.

### 4.3. Время и происхождение доказательств

- Задать единицы virtual time, монотонность, порядок равных deadlines, pause/resume, STOP, HLT wakeup и поведение TSC. Не подменять модель времени случайной длительностью host-вызова или необоснованным равенством «одна инструкция = один наносекундный tick».
- Каждое обещанное ускорение требует сравнения с baseline на одинаковом guest-сценарии, повторных прогонов, разброса, параметров сборки и одинакового результата. Для JIT разделять cold/warm. Точный выбор TLB/очереди событий сначала гипотеза, затем измерение.
- Самосравнение двух путей, использующих один walker, не доказывает x86-семантику. Нужны спецификация, независимые ожидаемые результаты и выбранные hardware cases. Нельзя выдавать compile-only, fixture injection или наличие протокола испытаний за запуск.

## 5. Стенд и физическая проверка

Основной host — Apple Silicon; в [описании стенда](../testing/STAND.md) зафиксирован M3 Max с host page size 16 KiB. Второй ARM host использовать, когда он доступен; его отсутствие не скрывать. Перед замерами повторно снять фактическую конфигурацию.

**Единственный доступный x86 — Mac mini с Ubuntu, SSH `192.168.1.14`.** В текущем стенде используется `senya@192.168.1.14`, checkout этапа 1 — `/home/senya/projects/open_desktop-stage1`. Не считать этот checkout автоматически подходящей веткой этапа 4 и не менять его без проверки состояния. Перед будущим запуском уточнить рабочую копию, CPU, kernel, toolchain и разрешённый способ выполнения из [CPU oracle](../testing/CPU_ORACLE.md).

Usermode runner в Ubuntu годится для разрешённых CPL3 сценариев. `sudo` не превращает обычный процесс в CPL0 и не проверяет CR3/page walk/APIC/interrupt delivery. Для системной физической проверки S4-34 требуется отдельный реально исполняемый privileged/test-kernel путь с проверенным восстановлением существующей Ubuntu. Методики [LGDT](../testing/S2_41A_LGDT_ORACLE.md) и [LIDT](../testing/S2_41B_LIDT_ORACLE.md) — исходные материалы, а не готовый harness.

Сначала подготовить harness, ограничения адресов и устройств, сборку, возврат/восстановление и способ сохранения результатов. Если нужен reboot или загрузка с внешнего носителя, получить явное разрешение пользователя на конкретную операцию и окно простоя после подготовки проверяемого результата. Нельзя автоматически переразмечать диск, менять bootloader или устанавливать другое ядро на рабочую Ubuntu. Не требовать покупки другого x86. При недоступности такого запуска сохранить `blocked`/`not_run` для соответствующих hardware gates и продолжать независимую локальную работу.

В рамках подготовки этого Markdown SSH, reboot, сборка CPU и системные эксперименты **не выполняются**.

## 6. Порядок задач и зависимости

`S4-NN` в таблице означает принятую необходимую часть задачи. Условные устройства/режимы можно исключить только по результатам S4-00 с документированным основанием; неподдержанный обязательный путь нельзя объявить optional. Ни одна задача не требует заранее завершить весь этап 2 или 3.

| Задача | Минимальные зависимости | Результат |
|---|---|---|
| S4-00 | — | Точный целевой профиль и boot manifest |
| S4-01 | S4-00 | Матрица prerequisite и воспроизводимый baseline |
| S4-02 | S4-01 | Единая physical/PIO карта |
| S4-03 | S4-02 | Production RAM owner |
| S4-04 | S4-03 | Physical access и committed writes |
| S4-05 | S4-04 | MMIO/PIO bus |
| S4-06 | S4-05 | Memory bridge CPU ↔ machine |
| S4-07 | S4-06 | Uncached page walker |
| S4-08 | S4-07 | Права и точные page faults |
| S4-09 | S4-08 | A/D effects |
| S4-10 | S4-09 | Нужные размеры страниц и paging modes |
| S4-11 | S4-10 | Cross-page и partial progress |
| S4-12 | S4-11 | Исполняемые system-register effects |
| S4-13 | S4-12 | Измеренный прототип TLB |
| S4-14 | S4-13 | Корректная invalidation TLB |
| S4-15 | S4-14 | JIT с production memory owner |
| S4-16 | S4-15 | Code aliases и active block invalidation |
| S4-17 | S4-16 | Сравнение memory/fault сценариев |
| S4-18 | S4-12 | GDT/IDT/TSS |
| S4-19 | S4-18 | Доставка исключений и retry |
| S4-20 | S4-19 | CPL3 и system-call/return |
| S4-21 | S4-12 | Жизненный цикл одного vCPU |
| S4-22 | S4-21 | Virtual time/TSC |
| S4-23 | S4-22 | Очередь событий и deadlines |
| S4-24 | S4-05 | UART polling; IRQ часть после S4-27 |
| S4-25 | S4-19, S4-23 | Local APIC |
| S4-26 | S4-25 | IOAPIC |
| S4-27 | S4-26 | Сквозная доставка IRQ и EOI |
| S4-28 | S4-27 | APIC timer |
| S4-29 | S4-28 | Нужные legacy источники |
| S4-30 | S4-05 | Минимальная PCI/ECAM |
| S4-31 | S4-02, S4-29, S4-30 | Согласованные discovery tables |
| S4-32 | S4-12, S4-21, polling S4-24 | Собственное загружаемое ядро |
| S4-33 | S4-17, S4-20, S4-28, S4-32, IRQ S4-24 | System acceptance собственного ядра |
| S4-34 | S4-00, S4-01 | Независимый privileged hardware путь |
| S4-35 | S4-00 | Подлинный зафиксированный Linux artifact |
| S4-36 | S4-35 | Минимальный initramfs и `/init` |
| S4-37 | S4-20, S4-29, S4-31, S4-32, S4-36 | Linux direct-boot loader |
| S4-38 | S4-33, S4-37 | Прохождение ранней загрузки Linux |
| S4-39 | S4-38 | Настоящий Linux userspace |
| S4-40 | S4-39 | Отрицательные boot checks |
| S4-41 | S4-21, S4-39 | Headless/VMHost lifecycle |
| S4-42 | S4-17, S4-28, S4-39 | Измерения памяти, времени и JIT |
| S4-43 | S4-40, S4-41 | Reset/churn/stress |
| S4-44 | S4-33, S4-34, S4-40, S4-42, S4-43 | Воспроизводимый итоговый прогон |
| S4-45 | S4-44 | Независимое итоговое ревью |
| S4-46 | S4-45 | Документы и приёмка этапа |

Таблица не требует выполнять задачи строго по номеру. Polling UART и собственный probe выполнять сразу после их зависимостей. S4-34/35 готовить параллельно локальным срезам, чтобы внешняя проверка и происхождение Linux не стали неожиданным блокером в конце. Uncached interpreter path сохранять эталоном при добавлении TLB/JIT.

## 7. Пошаговые задания

### S4-00. Зафиксировать одну целевую машину и вход в Linux

- [ ] Принято после ревью.
- **Где:** `docs/specs/MACHINE_CONTRACT.md`, существующие `CPU_CONTRACT.md`, `GUEST_REQUIREMENTS.md`, `VIRTUAL_BOARD.md`; факты — в отчёте этапа.
- **Сделать:** составить manifest: один vCPU; точный Linux artifact; guest RAM; physical address width; CPU identity/features/MSR allowlist; paging modes/page sizes; APIC mode; timer; UART; PCI discovery; boot entry; initramfs; boot data; success evidence. Использовать существующий synthetic target `od-x86-64-v1-neutral-v0.1` как основу согласования, не копировать CPUID Mac mini.
- **Разрешить до кода:** кандидат — 64-bit direct entry; сопоставить его с исходниками и config точного Ubuntu kernel. Проверить достижимые 32-bit trampoline/LA57 transitions: название «64-bit entry» само по себе их не исключает. При обязательном пути добавить prerequisite в этап 2 или обосновать поддержанный вариант входа. Зафиксировать точные первичные спецификации и версии.
- **Проверка:** для каждого объявленного feature есть владелец, тест и статус. Для исключённого пути есть основание из выбранного kernel/boot contract. Не обещать полный x86-64 лишь по списку CPUID.
- **Готово:** отсутствуют неявные предположения о режиме входа, обнаружении CPU/таймеров/PCI и источнике boot data. Если ACPI нужны, принять data-only путь S4-31, без полной UEFI.

#### S4-00a. Зафиксировать доступные source/config inputs и пределы provenance

- [x] Ограниченный evidence capture выполнен после независимого ревью `gpt-6.1-sol/high`; приёмка родительского S4-00 не подтверждена.
- **Evidence:** [отчёт S4-00a](../reports/STAGE_4_S4-00A_2026-11-05.md) и [manifest](../reports/data/stage4/s4-00a-source-audit-2026-11-05.json).
- **Подтверждено:** recovered exact-version source `.deb`, extracted source archive, config и три source-file hashes; существующие четыре binary hashes сверены с S2-40d; Build IDs двух ELF совпадают, а read-only ELF64 parser подтвердил равенство 38 общих allocated file-backed sections (42 release sections, 57 debug sections).
- **Открыто:** подписанные `.dsc`, `orig.tar.gz`, `diff.gz`, `Sources.xz`, `InRelease` и `apt-provenance.json`; authenticated source/config-to-binary mapping. S4-00 и S4-01 остаются открытыми.
- **Проверки:** `shasum -a 256`, `file`, read-only Python 3 ELF64 section parser, `objdump -h`, `ar`, `zstd`, `tar`; ограничения инструментария и отсутствующие inputs описаны в отчёте.

#### S4-00b. Проверить boot entry и paging transitions выбранного Linux

- [x] Ограниченный evidence capture выполнен и принят после независимого ревью `gpt-6.1-sol/high`; приёмка родительских S4-00 и S4-01 не подтверждена.
- **Evidence:** [отчёт S4-00b](../reports/STAGE_4_S4-00B_2026-11-06.md) и [manifest](../reports/data/stage4/s4-00b-kernel-entry-2026-11-06.json).
- **Подтверждено:** заголовок точного `vmlinuz` — protocol 2.15, relocatable, direct 64-bit entry на `+0x200` от загруженного protected-mode payload (`setup_sects=39`, file offset `0x5200`); decompression выполняет kernel startup после входа. Официальный boot protocol требует Long64, paging, identity maps для `init_size`/zero page/command line, selectors `CS=0x10`, `DS=ES=SS=0x18` с соответствующими плоскими GDT descriptors и выключенные interrupts. В recovered source snapshot присутствуют 32-bit `startup_32` и условный LA57 selection/trampoline; четырёхуровневому профилю нужно согласованно исключить LA57 или реализовать переход. `no5lvl` отдельно не гарантирует отсутствия trampoline, если стартовый CR4.LA57 установлен.
- **Открыто:** выбрать/зафиксировать loader handoff и CPU feature profile; initramfs/boot data; аутентифицированное source-to-binary provenance; фактическая загрузка Linux. S4-00 и S4-01 остаются открытыми.
- **Проверки:** локальный read-only разбор boot header и измерение длины `vmlinuz`; SHA-256 `.deb` совпал с опубликованным checksum Ubuntu; source snapshot прочитан по ранее зафиксированным hashes. Код не менялся; тесты репозитория не запускались.

#### S4-00c. Согласовать версионированный кандидат машины и prerequisites

- [x] Подготовлен documentation inventory; независимое ревью — см. [отчёт S4-00c](../reports/STAGE_4_S4-00C_2026-11-08.md). S4-00, S4-01, полный S4-10 и Stage 4 остаются открытыми.
- **Evidence:** [отчёт S4-00c](../reports/STAGE_4_S4-00C_2026-11-08.md), [manifest v0.1](../reports/data/stage4/s4-00c-machine-candidate-v0.1.json), [machine contract](../specs/MACHINE_CONTRACT.md#candidate-linux-machine-inventory-s4-00c).
- **Границы:** один Ubuntu Generic amd64 `7.0.0-38.38`, direct64 protocol 2.15; статусы, evidence, owner и next check для CPU, MMU, карты, устройств и загрузки зафиксированы в manifest по текущему состоянию `main` (S4-10b, S4-12c–f, S4-14d–g, S4-18, S4-19, S4-20, S4-21, S4-22–S4-28).
- **Открыто:** provenance не воспроизведён независимо, карта RAM/резерваций и состояние входа не завершены, MSR/control/descriptor, Linux-путь IRQ/timer и discovery не приняты; Linux boot и runtime evidence не наблюдались.
- **Проверки:** пересчёт SHA-256 локальных артефактов и перечитывание recovered source (отчёт); валидность JSON (22 пункта); `tools/check_layout.py` и `git diff --check`; Rust-код не менялся.

### S4-01. Снять baseline и разбить зависимости CPU/JIT

- [ ] Принято после ревью.
- **Где:** `docs/testing/MACHINE.md`, `docs/reports/STAGE_4_REPORT.md`, необходимые записи в планах 2/3.
- **Сделать:** сопоставить каждый обязательный путь S4-00 с фактическим decoder/interpreter/JIT; записать поддержанные формы, вызываемые API, недостающие режимы и владельца prerequisite. Отдельно проверить fetch, control-register instructions, INVLPG, descriptor operations, exception delivery, IRET, CLI/STI/HLT и выбранный syscall путь.
- **Проверка:** воспроизвести существующие проверки затронутых контрактов; сохранить commit, host/toolchain, команды и исходный результат. Старый successful report не выдавать за текущий прогон.
- **Готово:** локальные работы отделены от CPU-blocked и hardware-blocked. Составлен один небольшой список задач для ближайшего вертикального среза; full-stage JIT не становится искусственной общей зависимостью.

#### S4-01a. Воспроизводимый diagnostic CPU-to-RAM baseline

- [x] Ограниченный baseline принят после независимого ревью `gpt-6.1-sol/high`; родительский S4-01 остаётся открытым.
- **Evidence:** [отчёт S4-01a](../reports/STAGE_4_S4-01A_2026-11-05.md) и [JSON baseline](../reports/data/stage4/s4-01a-baseline-2026-11-05.json).
- **Подтверждено:** два exact-теста реально исполнили по одному тесту: CPU interpreter через `IdentityMemoryAdapter → MachineBus → PhysicalMemory` с успешными load/store и отказом на hole `0x3000` без retirement/mutation; отдельный immutable page-walk snapshot control с разными CR3 roots. Профиль — diagnostic flat identity (`linear == physical`), не Linux manifest.
- **Матрица границ:** translation integration остаётся владельцем `od-machine`; page-fault probe/record отделён от S4-19 delivery/retry; JIT и Linux entry не покрыты. Нет claims о CPU-integrated paging, `#PF`, privileged hardware или Linux boot.
- **Проверки:** обе команды из отчёта завершились с exit code 0 и `1 passed, 0 failed, 0 ignored; 67 filtered out`; toolchain/host/page-size и исходный clean status сохранены в JSON.

#### S4-01b. Классификация LOCK MOV CR0 в потребителях JIT

- [x] Принято после независимого `gpt-6.1-sol/high` ревью; родительские S4-01 и Stage 4 остаются открытыми.
- **Evidence:** [отчёт S4-01b](../reports/STAGE_4_S4-01B_2026-11-06.md) и [JSON manifest](../reports/data/stage4/s4-01b-invalid-mov-cr0-lock-2026-11-06.json).
- **Объём:** block builder сохраняет `InvalidMovCr0Lock` как typed `UnsupportedEncoding` на точном RIP и не теряет уже построенный префикс; deferred boundary сохраняет длину 4. Bounded memory dispatcher останавливается как `Unsupported` до executor; LOCK MOV CR3 остаётся прежним. Допуск native/JIT не расширен.
- **Контрольные границы:** existing tests сохраняют ordinary Invalid/Unsupported, truncation и byte-budget поведение. CPU #UD не изменялся; существующие canaries перечислены в отчёте.
- **Ревью:** independent review found no remaining findings after the memory-spy and full-state assertions were added.

### S4-02. Описать единую карту машины

- [x] Принято после ревью.
- **Где:** `crates/od-machine/src/`, `MACHINE_CONTRACT.md`, уточнения `VIRTUAL_BOARD.md`.
- **Сделать:** валидируемые RAM/ROM/reserved/MMIO ranges и отдельная PIO map; адреса устройств и boot reservations из одного resolved manifest. Определить порядок lookup и запрет неоднозначных пересечений.
- **Проверка:** пустые/переполненные диапазоны, overlap, край physical width, alias только там, где он явно разрешён, невозможная конфигурация RAM и устройств.
- **Готово:** недопустимая карта отклоняется до allocation/старта CPU; E820 и дальнейшие tables получают данные от этого владельца.

### S4-03. Реализовать production RAM owner

- [x] Принято после ревью.
- **Где:** `crates/od-machine/src/`; host-specific allocation лишь при необходимости через разрешённую host-границу.
- **Сделать:** один владелец backing, checked offsets, время жизни и ограничение размера; определить reset/zeroing и отказ при нехватке ресурсов. Не выдавать unrestricted pointers внешним consumers.
- **Проверка:** границы RAM, выравнивание, независимые 4 KiB guest pages внутри 16 KiB host page, освобождение после последнего разрешённого handle, отказ allocation без guest exception.
- **Готово:** RAM реально читается/пишется в host tests и безопасно уничтожается; fixture adapter не является production owner.

### S4-04. Physical accesses и уведомление о записях

- [x] Принято после ревью.
- **Где:** `crates/od-machine/src/`, согласованный interface с `od-jit`.
- **Сделать:** checked physical read/write, разрешённые widths, little-endian данные, raw bytes для loader; уведомление о реально committed byte ranges. Определить поведение holes/ROM и ownership write transaction.
- **Проверка:** первая/последняя byte, overflow, range spanning, rejected writes без ложного commit, loader write и alias write через один notification path.
- **Готово:** нет публичного bypass, позволяющего изменить code bytes без уведомления; metadata не меняется при не совершившейся записи.

### S4-05. MMIO и port I/O bus

- [x] Принято после ревью.
- **Где:** `crates/od-machine/src/`; CPU IN/OUT semantics — в `od-isa`.
- **Сделать:** минимальные read/write interfaces устройств; ширины, alignment, register side effects, unknown offsets и недопустимые spanning accesses. Различать RAM и device transaction, не превращать MMIO в обычный byte slice.
- **Проверка:** маленькое собственное тестовое устройство считает обращения; один guest access имеет ровно предусмотренные side effects, включая fault/retry и invalid width.
- **Готово:** PIO/MMIO dispatch определяется картой; CPU exceptions и machine/device errors остаются разными результатами.

### S4-06. Соединить CPU memory contract с машиной

- [ ] Открыто: ранее принятый flat identity bridge не закрывает CPU-connected paging; bounded S4-06b ниже принят отдельно.
- **Где:** `crates/od-isa/src/`, `crates/od-machine/src/`, локальные contract tests.
- **Сделать:** adapter получает access kind, privilege/context и выполняет fetch/read/write через machine memory. Сохранить complete-or-no-change одного вызова `GuestMemory`; при необходимом изменении контракта сначала согласовать всех callers и тесты.
- **Проверка:** разные fetch/read/write permissions, RAM/MMIO routing, host error не становится `#PF`, fault возвращается без заранее изменённого CPU destination.
- **Готово:** интерпретатор использует production memory interface в ограниченном тесте; временный physical adapter явно не объявлен полной Long64 машиной.

#### S4-06b. Bounded CPU-connected paged MOV

- [x] Ограниченный срез принят после независимого `gpt-6.1-sol/high` review.
- `step_paged_mov` допускает только `48 8B 03` и `48 89 0B`, по одной инструкции за вызов. Публичный `od_machine::step_paged_mov` использует внутренний `PagedMovMemoryAdapter`, tracked walker и RAM owner для literal non-identity mappings; контракт `GuestMemory` уточнён для неизменного read buffer, отсутствия частичного operand write и сохраняющихся A/D effects/callbacks, включая alias.
- Review rerun: 2,014 `od-isa` unit tests и 85 `od-machine` tests; P0—P3 findings отсутствуют. См. [отчёт S4-06b](../reports/STAGE_4_S4-06B_2026-11-05.md) и [manifest](../reports/data/stage4/s4-06b-paged-mov.json).
- Это не закрывает родительский S4-06: полная CPU memory integration и остальные guest access forms остаются открытыми. ISSUE-054 остаётся открытой для cross-page/partial progress, REP/RMW и других rollback consumers.

#### S4-06c. Bounded CPU-connected paged MOV to MMIO

- [x] Принят после независимого `gpt-6.1-sol/high` review. Реализован только для двух `step_paged_mov` форм из S4-06b и существующей 4 KiB Long64 конфигурации. После translation и permission checks qword operand направляется либо в существующий `PhysicalMemory`, либо в один `MachineBus` MMIO binding.
- MMIO dispatch выполняется ровно один раз после успешной проверки; page-walk A/D writes и callbacks сохраняют фактический порядок. Device backend error остаётся backend error, не `#PF`; instruction не retire-ится. A/D effects, уже совершённые до ошибки устройства, не откатываются; автоматического retry нет.
- Проверки: 11 focused `paged_mov_step` тестов, `cargo check -p od-machine --all-targets --locked --offline`, `cargo fmt --all -- --check`, `git diff --check` прошли. Graphify обновлён в основном checkout. См. [отчёт S4-06c](../reports/STAGE_4_S4-06C_2026-11-05.md) и [manifest](../reports/data/stage4/s4-06c-paged-mmio.json). Родительский S4-06 остаётся открытым.

### S4-07. Сделать uncached page walker

- [x] Принято после ревью.
- **Где:** `crates/od-machine/src/`, CPU context contract в `od-isa`.
- **Сделать:** начать с требуемого 4-level Long64 и 4 KiB pages; обходить guest page tables через physical API. Отдельно разобрать canonical linear addresses, physical width и reserved fields. Начальные CR state fixtures должны быть архитектурно допустимы.
- **Проверка:** независимо заданные таблицы и ожидаемые адреса, переход через каждый уровень, missing entries, high addresses, некорректные bits. Ожидаемый результат нельзя вычислять тем же walker.
- **Готово:** translation работает без TLB; unsupported mode отвергается явно. Различие noncanonical `#GP`/`#SS` и `#PF` согласовано с типом доступа CPU.

### S4-08. Права доступа и точный page fault

- [x] Принято после ревью.
- Bounded local sub-slice implemented: shared `od-isa` PFEC helper and precise
  `PageFaultRecord` construction from S4-07 probe. This does not complete S4-08:
  CPU-owned paging state, full manifest feature matrix, cross-page fetch,
  exception delivery/CR2 commit, system tests and independent agent review remain
  open. See [machine contract](../specs/MACHINE_CONTRACT.md#bounded-page-fault-record-construction)
  and [stage report](../reports/STAGE_4_REPORT.md).
- **Где:** MMU в `od-machine`, fault construction/CPU state в `od-isa`, `tests/system/`.
- **Сделать:** cumulative permissions по уровням, CPL, read/write/fetch, supervisor write protection, NX и остальные обязательные features S4-00. Описать CR2, error bits, сохранённый RIP и порядок конкурирующих проверок по стандарту.
- **Проверка:** матрица present/nonpresent, user/supervisor, read/write/fetch, WP/NXE, reserved bits и boundary faults. Для instruction fetch проверять точный faulting address, а не только исходный RIP.
- **Готово:** определённые поля совпадают с независимыми expected cases; не реализованные optional protections не рекламируются CPUID/CR state.

### S4-09. Accessed/dirty bits и наблюдаемые side effects

- [x] Ограниченный локальный срез принят после независимого ревью.
- Bounded RAM-only access API implemented in `od-machine`: `access_long64_4k_ram`
  commits A updates during the walk, then validates the final RAM operand, commits
  D for permitted writes, and finally commits the operand. Literal local tests
  cover successful fetch/read/write and preserved A effects on later failures.
  S4-09b closes the injected table-write failure criterion. S4-09c adds the
  bounded four-level permission/A-D matrix, accepted after independent review.
  Full S4-09 remains open for CPU execution integration and system acceptance. See
  [S4-09b](#s4-09b-table-write-failure), [S4-09c](#s4-09c-permission-level-matrix)
  and the [machine contract](../specs/MACHINE_CONTRACT.md#tracked-permission-level-matrix-s4-09c).
- **Где:** page walker/physical writer `od-machine`, CPU contract и системные тесты.
- **Сделать:** определить, на каких уровнях и при каких успешных/неуспешных accesses меняются A/D bits. Разделить эти записи и commit конечного memory operand. Использовать общий tracked physical-write путь.
- **Проверка:** read, write, fetch, protection fault, nonpresent на разных уровнях, permission fault после предыдущих обращений; page-table bytes совпадают с ожидаемыми разрешёнными эффектами.
- **Готово:** rollback memory operand не стирает обязательные page-walk effects; тесты не требуют неизменности всей памяти после любого fault.

<a id="s4-09b-table-write-failure"></a>

#### S4-09b. Отказ отдельной записи page-table entry

- [x] Bounded test seam остаётся private внутри `od-machine`; штатный writer по-прежнему вызывает `PhysicalMemory::write_u64`. Принят после независимого `gpt-6.1-sol/high` review без оставшихся P0—P3 замечаний.
- При injected failure A на каждом из четырёх уровней ранее committed A остаются; отказавшая entry и последующие уровни не меняются. Отчёт содержит только committed `AdUpdate`, а callbacks содержат только committed writes в фактическом порядке.
- Отдельный отказ D после успешного полного A-prefix сохраняет A, не меняет leaf D и operand и не вызывает последующих callbacks.
- Каждый результат проверяет точные уровень, адрес entry, тип A/D write и физическую ошибку. Существующие `PhysicalMemory` tests остаются доказательством atomicity и отсутствия callback при обычном отказе physical write.
- Срез не добавляет production fault injection/API, whole-operation rollback, cross-page, MMIO/guest-store, exception delivery или cached A/D semantics. Panic callback остаётся host panic после уже committed write.
- Срез S4-09c дополняет permission/A-D matrix; полный S4-09 остаётся открытым для CPU integration и system acceptance.

<a id="s4-09c-permission-level-matrix"></a>

#### S4-09c. Матрица прав и A/D effects по уровням

- [x] Принят после независимого `gpt-6.1-sol/high` review без оставшихся P0—P3 замечаний.
- Тесты tracked Long64 4-level/4 KiB RAM access проверяют non-present Read/Write/Fetch на PML4, PDPT, PD и PT для CPL0/CPL3; cumulative U/S, R/W, CR0.WP и NX/NXE на каждом уровне; CPL1/CPL2 трактуются как supervisor. При non-present/reserved fault faulting entry и все нижние entries остаются неизменными, а A-prefix содержит только eligible предшественников. Protection проверяется после полного walk: A может установиться и в denied entry; все четыре eligible A updates сохраняются без D/operand commit. Для fault сверяются точный `PageFaultRecord`, полный массив `ad_updates`, callbacks и неизменность operand.
- Разрешённые writes проверяют A-prefix → leaf D → один operand commit. Предустановленные A/D не создают лишних table callbacks. Pure probe проверяется отдельно и не меняет A/D.
- Это тестовый срез: production walker/API и CPU-поведение не меняются. Нет CPU exception delivery/CR2 update, CPU execution integration, cross-page semantics, иных paging modes или system acceptance.
- Подробная матрица и результаты проверок: [отчёт S4-09c](../reports/STAGE_4_S4-09C_2026-11-06.md) и [контракт машины](../specs/MACHINE_CONTRACT.md#tracked-permission-level-matrix-s4-09c).

### S4-10. Добавить необходимые page sizes и режимы

- [x] Ограниченный S4-10a admission-boundary срез принят после независимого ревью.
- Реализовано: для текущего Long64/4 KiB профиля present PS на PDPT/PD
  отвергается как `UnsupportedPageSize` с сохранением уже committed A у предков;
  pure probe остаётся без мутаций, PML4 bit 7 остаётся reserved-bit fault, PT
  bit 7 допускается в 4 KiB leaf. Это не реализация huge pages.
- S4-10b: отдельный opt-in RAM-only профиль 2 MiB (`access_long64_2m_ram`,
  `step_paged_mov_2m_ram`) реализован как вертикальный поднабор: PDE PS=1,
  reserved bits 20:13, A/D на PDE, exact qword MOV, fetch в пределах 4 KiB
  страницы RIP. MMIO, 1 GiB, полная матрица прав и системная приёмка остаются открытыми;
  CPU-owned CR3 со смешанными картами — S4-12e.
  [Отчёт](../reports/STAGE_4_S4-10B_2026-11-08.md).
- Полный S4-10 остаётся открытым до S4-00 manifest: 2 MiB/1 GiB leaves,
  необходимые modes, alignment/reserved/permissions/A-D matrix и Linux boot
  prerequisites не приняты. См. [machine contract](../specs/MACHINE_CONTRACT.md#huge-page-и-mode-admission-boundary).
- **Где:** MMU `od-machine`; недостающие CPU modes — `od-isa` и план этапа 2.
- **Сделать:** добавить 2 MiB/1 GiB pages и иные paging modes только в объёме manifest. Проверить alignment/reserved bits, права и A/D для leaf каждого размера. Неподдержанную optional возможность отключать согласованно с CPU/boot profile.
- **Проверка:** границы huge pages, смешанная иерархия, неправильный leaf, переходы требуемых modes. Если Linux неизбежно выполняет 32-bit trampoline, закрыть CPU prerequisite, а не симулировать его host loader.
- **Готово:** каждый объявленный размер/режим исполняется и имеет отрицательные тесты; таблица исключений scope содержит доказательства отсутствия обязательного пути.

### S4-11. Cross-page accesses и частичное выполнение

- [x] Ограниченный S4-11a admission/sequence срез принят после независимого ревью.
- Реализовано: same-page Read/Write width matrix и ранний `CrossesPage` отказ
  без walk/A-D/callback/operand effects; отдельные calls сохраняют собственные
  commits, adjacent fetch evidence не объявляет CPU restart semantics.
- Bounded S4-11b ниже принят отдельно. Полный S4-11 остаётся открытым для
  cross-page stores, иных widths, stack/REP/RMW и CPU/system integration.
  См. [machine contract](../specs/MACHINE_CONTRACT.md#cross-page-admission-и-separate-call-progress).
- **Где:** `od-machine`, затронутые memory semantics `od-isa`, `tests/system/`.
- **Сделать:** определить preflight/commit для одного memory-вызова с сохранением его контракта. Разобрать boundary RAM/RAM, RAM/hole, RAM/MMIO, code fetch; не разбивать device access на произвольные повторяемые bytes.
- **Проверка:** fault на второй странице read/write/fetch, разница прав соседних 4 KiB pages, стековые операции, поддержанные REP iterations и RMW. Сравнивать RIP, registers, flags, memory и A/D effects отдельно.
- **Готово:** неподтверждённые restart cases из `KNOWN_ISSUES.md` не объявлены решёнными локальным happy path; завершённые REP iterations сохраняются согласно CPU-контракту.

#### S4-11b. Bounded CPU-connected cross-page qword read

- [x] Принят после независимого `gpt-6.1-sol/high` review без оставшихся P0—P3.
- Только `48 8B 03`, qword read через две 4 KiB страницы, все семь offsets `0xff9..0xfff`, literal non-identity mappings и две non-contiguous RAM pages. Оба walk/permission/RAM admission завершаются до operand read; A updates остаются при последующем fault; D не ставится; caller buffer и CPU state коммитятся только после полного чтения.
- Cross-page store, RAM/MMIO splitting, stack/REP/RMW и full exception delivery остаются открытыми. См. [S4-11b report](../reports/STAGE_4_S4-11B_2026-11-05.md) и [manifest](../reports/data/stage4/s4-11b-cross-page-read.json).

#### S4-11c. Bounded CPU-connected cross-page dword read

- [x] Принят после независимого `gpt-6.1-sol/high` review без оставшихся P0—P3 замечаний; первое ревью P3 evidence несоответствие исправлено и подтверждено повторно.
- Реализован отдельный точный opt-in profile `PagedMovCrossPageDwordReadLong64V1` только для bare `8B 03` (`MOV EAX,[RBX]`), Long64, RBX addressing, одному vCPU и действующей canonical48/physical36/four-level/4 KiB uncached paging configuration. Профиль S4-11b не расширен.
- Одностраничный RAM read служит контролем; проверены все crossing offsets `0xffd..0xfff`, две non-contiguous RAM pages, EAX zero-extension, RIP+2, одна retirement, точный second-page #PF, alias walk-then-sample и отказ от cross-page MMIO. Walk/permission/RAM admission предшествуют operand reads; D не устанавливается.
- Реализация и проверки записаны в [S4-11c report](../reports/STAGE_4_S4-11C_2026-11-05.md) и [manifest](../reports/data/stage4/s4-11c-cross-page-dword-read.json). Полный S4-11 остаётся открытым для stores, иных форм/widths, restart behavior и прочих memory consumers.

#### S4-11d. Bounded CPU-connected instruction fetch across a 4 KiB page

- [x] Принят после независимого `gpt-6.1-sol/high` review без оставшихся P0—P3 замечаний; добавлены точные assertions A-update journal и evidence.
- В существующем opt-in profile `PagedMovCrossPageDwordReadLong64V1` проверить только точную bare `8B 03` (`MOV EAX,[RBX]`): opcode по `RIP=page+0xfff`, ModRM на следующей linear page, две non-contiguous RAM code pages и отдельная RAM operand page. Allowlist не расширяется.
- Проверить успешное исполнение, second-fetch nonpresent (`PFEC=0x10`) и NX (`PFEC=0x11`) faults с точными адресом/сохранённым RIP, без retirement и operand walk; проверить hole/MMIO второй code page без device callback. Decoder-level однобайтовый end-of-page control подтверждает отсутствие лишней выборки вне CPU profile.
- Результаты: [S4-11d report](../reports/STAGE_4_S4-11D_2026-11-05.md) и [manifest](../reports/data/stage4/s4-11d-cross-page-instruction-fetch.json). Общий S4-11 и ISSUE-054 остаются открытыми.

#### S4-11e. Bounded CPU-connected cross-page word read

- [x] Принят после независимого `gpt-6.1-sol/high` review без оставшихся P0—P3 замечаний; исправлены два P2 admission findings и P3 рассинхронизация Graphify evidence. для точного `66 8B 03` (`MOV AX,[RBX]`), Long64, одного vCPU, canonical48/physical36/four-level/4 KiB uncached paging, CPL0/CPL3 и `RFLAGS.AC=0`.
- Проверены same-page RAM control и crossing offset `0xfff` между non-contiguous RAM pages. Обе translation/permission/RAM admission завершаются до read; AX обновляется только после полного чтения, RAX[63:16] и остальные CPU state сохраняются. Проверены точные CPL0/CPL3 second-page faults, prior A effects, отсутствие D, no-commit/no-retirement, RAM hole/MMIO отказ, alias walk-then-sample и negative profile cases.
- Реализация, проверки и reviewer findings записаны в: [S4-11e report](../reports/STAGE_4_S4-11E_2026-11-05.md), [manifest](../reports/data/stage4/s4-11e-cross-page-word-read.json), [machine contract](../specs/MACHINE_CONTRACT.md#bounded-cross-page-word-read-s4-11e). Полный S4-11 и ISSUE-054 остаются открытыми для прочих форм, stores и rollback consumers.

#### S4-11f. Bounded CPU-connected cross-page fetch for word MOV

- [x] Accepted after independent `gpt-6.1-sol/high` review with no remaining P0—P3 findings; three P2 and three P3 findings were fixed during review. Separate opt-in `PagedMovCrossPageWordFetchLong64V1` profile for exact `66 8B 03`, CPL0/AC=0/NXE=1; S4-11e V1 continues to reject split instruction fetch.
- Cover code offsets `0xffe` and `0xfff` across non-contiguous RAM pages; same-page control at `0xffd` with next page absent; operand wholly on separate RAM page. No code/operand/page-table aliases. Success preserves upper RAX and other state, advances RIP by 3 and retires once.
- Verify precise second-page non-present/NX fetch faults (`PFEC=0x10/0x11`), exact byte address/original RIP/context, A-prefix persistence, no D, no operand walk/retirement; hole/MMIO without dispatch; fetch fault precedence over an inaccessible operand; reject a cross-page data operand after decode but before its walk; require matching CR0/CR4 in all public entries; negative profile cases.
- Architecture: `gpt-6-astra/high` approved the bounded contract. Implementation, focused checks and independent review are complete with no remaining P0—P3 findings. See [S4-11f report](../reports/STAGE_4_S4-11F_2026-11-05.md), [manifest](../reports/data/stage4/s4-11f-cross-page-word-fetch.json), and [machine contract](../specs/MACHINE_CONTRACT.md#bounded-word-read-instruction-fetch-across-a-4-kib-page-s4-11f). ISSUE-054 and parent S4-11 remain open.

### S4-12. Подключить CR0/CR3/CR4, EFER и system instructions

- [x] Ограниченный S4-12a snapshot-bridge срез принят после независимого ревью.
- [x] Ограниченная реализация S4-12b CPU-side `MOV CR3,RAX` принята после
  независимого `gpt-6.1-sol/high` ревью; старты `0xff2–0xfff` отклоняются до
  fetch. Контракт и результаты: [отчёт](../reports/STAGE_4_S4-12B_2026-11-05.md)
  и [manifest](../reports/data/stage4/s4-12b-mov-cr3.json).
- [x] S4-12c применяет новый CR3 к активному machine translation state;
  следующая инструкция fetch-ится и читает данные через новый root. Проверены
  precise fetch fault после переключения и STOP сразу после MOV CR3 без walk
  root B. Независимое `gpt-6.1-sol/high` ревью одобрило код; отчётность
  синхронизирована с итогами Graphify и проверок. См.
  [отчёт](../reports/STAGE_4_S4-12C_2026-11-05.md).
- S4-12a реализует `Long64ProbeConfig::from_cpu_state`: он требует CR0 затем
  CR4 из `LongModeState`, принимает CR3/EFER явно и делегирует raw values
  текущему validator. Snapshot immutable; в этом bridge guest instructions и
  transitions не добавлены.
- Полный S4-12 остаётся открытым: EFER writes, startup transitions,
  TLB/invalidation и system acceptance не приняты.
  См. [machine contract](../specs/MACHINE_CONTRACT.md#cpu-state-to-diagnostic-translation-snapshot).
- [x] S4-12d implemented, focused gates pass and independent review is clean.
  `MOV CR0,RAX` switches only WP between `0x80010033` and `0x80000033`;
  a following exact `MOV [RBX],RCX` must use the replacement snapshot. Keep
  CPU-owned CR3, fixed CR4/EFER and reject other instructions/values as
  Unsupported (except LOCK `MOV CR0`, which raises #UD after full fetch).
  Preserve CR3 profile behavior. Reject the ordinary store outside CPL0 without
  data access; require the store operand to fit one 4 KiB page. Verify WP `1→0` successful
  supervisor store, WP `0→1` precise protection fault, STOP/fetch boundaries,
  A/D prefix behavior, store page containment and atomic retirement; keep CPU
  privilege #GP distinct from machine CPL0 admission. Results:
  [отчёт](../reports/STAGE_4_S4-12D_2026-11-06.md) and
  [manifest](../reports/data/stage4/s4-12d-cr0-wp-machine.json). See
  [CPU contract](../specs/CPU_CONTRACT.md#paged-cpu-owned-cr0wp-profile-pagedcpl0cr0wpprobev1-s4-12d)
  and [machine contract](../specs/MACHINE_CONTRACT.md#cpu-owned-cr0wp-translation-handoff-s4-12d).
- [x] S4-12e (вертикальный поднабор) реализован и проверен локально; независимое ревью — см. [отчёт](../reports/STAGE_4_S4-12E_2026-11-08.md).
  `run_paged_cr3_probe_2m_ram` — тот же CPU-owned CR3 quantum, но tracked walk принимает 2 MiB PDE
  (RAM-only); корни A и B могут отображать одни регионы 4 KiB и 2 MiB страницами. Проверены:
  равенство полного и пошагового quantum (CPU, RAM, definedness, callbacks, порядок context change),
  fault/reserved 2 MiB PDE после retired CR3 write, NX PDE при data read, отказ legacy quantum на 2 MiB,
  граница fetch и operand, STOP/budget. Загружаемый freestanding probe и 2 MiB MMIO не входят.
- [x] S4-12f (вертикальный поднабор) реализован и проверен локально; независимое ревью — см. [отчёт](../reports/STAGE_4_S4-12F_2026-11-08.md).
  Фиксированный freestanding образ `cr3_mixed_probe` (fixture + воспроизводимая сборка + строгий загрузчик
  `load_stage4_cr3_mixed_probe`) исполняет A/4 KiB → B/2 MiB → A на quantum S4-12e: гость сам читает root A из
  2 MiB страницы; HLT CR3-профиль не допускает: при budget 3 quantum завершается по budget, при большем — precise #UD на UD2-sentinel. Полный/пошаговый запуск,
  порядок context change, NX и non-present 2 MiB PDE после перехода. 2 MiB MMIO, TLB/JIT и Linux не входят.
- **Где:** semantics и decode — `od-isa`; effects на translation — `od-machine`; соответствующие задачи этапа 2.
- **Сделать:** реализовать исполняемые формы и валидацию переходов выбранного профиля, MSR allowlist, width/reserved/privilege checks, явные эффекты context change. Согласовать INVLPG и поддержку глобальных страниц/PCID с S4-14.
- **Проверка:** guest instructions меняют действующий перевод; запрещённые записи дают правильное исключение и не оставляют частично обновлённый CR state. Проверить startup state и порядок включения обязательных bits.
- **Готово:** переходы исполняются через CPU, а не только setters теста. До появления TLB uncached путь остаётся корректным baseline.

### S4-13. Выбрать простой программный TLB по замерам

- [x] Ограниченный S4-13a measurement slice принят после независимого ревью.
- Реализованы opt-in one-entry pure-translation cache, correctness gate и
  reproducible release benchmark. Три процесса × 5 synthetic profiles × 2
  modes × 20 paired samples завершены; результат и ограничения записаны в
  [S4-13a report](../reports/STAGE_4_S4-13a_2026-11-05.md).
- Полный S4-13 остаётся открытым: это не production TLB choice. Нужны tracked
  A/D semantics, mutable mappings, guest invalidation, CPU/JIT integration и
  representative boot workloads перед выбором capacity/replacement.
- **Где:** `od-machine`, `tools/bench/`, отчёт и небольшие данные этапа.
- **Сделать:** сохранить uncached baseline, сделать минимальный bounded TLB prototype. Ключ/tag отражает address space, access/context, размер страницы и все влияющие на перевод permissions; исключить хранение невалидных указателей на RAM.
- **Проверка:** одинаковые translations/faults/A/D effects cached/uncached; рабочие наборы разного размера, повторные прогоны, hit/miss, latency и память. Сначала измерить реальные случаи, затем выбрать структуру/размер.
- **Готово:** решение об оптимизации обосновано данными. При шуме не заявлять ускорение; предусмотреть отключение TLB для диагностики.

### S4-14. Реализовать TLB invalidation

- [ ] Полный S4-14 не принят.
- [x] Ограниченный S4-14a lifecycle slice принят после независимого ревью.
- `TranslationProbeCache::clear` и завершение его immutable-borrow scope
  проверены как host-controlled boundary: clear идемпотентен, не пишет RAM и
  не меняет A/D; после remap или изменения прав создаётся новый cache.
- Этот срез не добавляет guest invalidation, generation counter, INVLPG,
  CR3/EFER transitions или CPU hook. Полный S4-14 остаётся открытым.
- [x] S4-14b implements feature-independent `step_invlpg` for exact bare
  `INVLPG [RBX]`; the typed effect is consumed by a feature-gated cache adapter
  that clears the full one-entry experimental cache before reuse. LOCK, CPL,
  fetch and canonical/no-op ordering are covered. The composition verifies
  `Walk → Hit → effect → consume → Walk → Hit` while preserving S4-14a's
  immutable-memory boundary. See the [report](../reports/STAGE_4_S4-14B_2026-11-06.md).
  Full S4-14 remains open.
- [x] S4-14c implements a feature-gated mutable diagnostic session with one
  exclusive `&mut PhysicalMemory`, values-only one-entry pure cache, checked
  aligned PTE writes and synchronous INVLPG-effect consumption. Same-session
  remap and write-permission refresh, post-INVLPG `Walk`, failed-write callback
  behavior and cache preservation on non-invalidating paths passed; independent
  `gpt-6.1-sol/high` review found no remaining P0—P3 findings. This does not
  complete S4-14. See the [S4-14c report](../reports/STAGE_4_S4-14C_2026-11-05.md).
- [x] S4-14d composes the existing exact `MOV CR3,RAX` CPU effect with that
  mutable session. A retired `Cr3Write` validates and installs the replacement
  translation snapshot, then synchronously clears its one-entry cache,
  including same-value writes. Rejected/faulting paths preserve snapshot and
  cache; post-retirement reconciliation failures clear the stale entry, poison
  the session and make subsequent operations fail explicitly. Focused tests
  cover roots A/B, same-CR3 PTE remap and permission tightening, precise
  post-switch faults, rejected paths and pure fetch. This does not complete
  S4-14. See the [S4-14d report](../reports/STAGE_4_S4-14D_2026-11-06.md).
- [x] S4-14e composes a standalone MOV-only `MOV CR0,RAX` WP profile with the
  mutable session. The profile excludes S4-12d's store allowlist. The session
  accepts WP=0/1, validates and installs each retired CR0 snapshot and clears
  its cache synchronously; current INVLPG/MOV CR3 profiles stay unavailable at
  WP=0. Same-value writes conservatively invalidate as this prototype's policy.
  Acceptance includes supervisor write permissions in both WP states, exact
  rejected/faulting paths, poison behavior and restored INVLPG/MOV CR3
  admission. This does not complete S4-14. See the
  [S4-14e report](../reports/STAGE_4_S4-14E_2026-11-06.md).
- [x] S4-14f adds the separate `InvlpgWpLong64V1` / `step_invlpg_wp` CPU
  entry and `MutableTranslationProbeSession::execute_invlpg_wp`. The new
  profile admits only the existing exact bare INVLPG form under WP=0/1 and
  shares its instruction semantics with the unchanged WP=1 entry. Canonical
  retired effects clear the one-entry session cache synchronously; the
  noncanonical retired NOP and rejected/faulting paths preserve it. Focused
  coverage and feature-on/off checks passed; final independent review found no
  remaining findings. See the [S4-14f report](../reports/STAGE_4_S4-14F_2026-11-06.md).
  Full S4-14 remains open.
- [x] S4-14g implements separate `MovCr3WpLong64V1` / `step_mov_cr3_wp` and
  `MutableTranslationProbeSession::execute_mov_cr3_wp` entries for CR0.WP=0/1.
  The legacy MOV CR3 entry remains WP=1-only. The new session entry validates
  pre-fetch control state, consumes only a valid retired `Cr3Write`, installs
  the replacement snapshot and clears its one-entry cache synchronously;
  post-retirement mismatches poison the session. Focused tests, feature-off/on
  checks, formatting and JSON/diff checks passed. Final independent review
  found no remaining findings. See [S4-14g report](../reports/STAGE_4_S4-14G_2026-11-06.md) and
  [manifest](../reports/data/stage4/s4-14g-mov-cr3-wp-2026-11-06.json).
- **Где:** `od-machine`, system instruction effects `od-isa`.
- **Сделать:** CR3/context transitions, INVLPG, paging/permission-control changes, reset; global/PCID/INVPCID — только при объявленной поддержке. Развести software-cache generations и архитектурную семантику guest page tables.
- **Проверка:** remap A→B, tighten/relax rights с обязательной invalidation, stale entry после context switch, huge-page coverage, код/данные, разные guest pages внутри host page. Поведение без invalidation проверять в пределах разрешённого стандартом, не требовать несуществующей гарантии.
- **Готово:** после необходимой invalidation следующий access использует новый перевод/права; произвольная PTE write не подменяет guest-инструкцию invalidation.

### S4-15. Соединить JIT с production memory owner

- [ ] Принято после ревью.
- **Где:** `od-jit`, `od-machine`, `od-host` в существующих границах; обновление `JIT_CONTRACT.md`.
- **Сделать:** адаптировать проверенное memory-подмножество этапа 3 к RAM/MMU machine owner, handles/leases и precise exits. Проверки fault/MMIO не обходятся direct host pointer. Memory ownership не переносить в runner.
- **Проверка:** одни guest bytes через interpreter и JIT, fault до/после допустимых effects, MMIO side effects, live handle при reset/retirement, реальное освобождение после допустимого срока.
- **Готово:** хотя бы зафиксированный memory slice работает на production owner; unsupported JIT формы переходят в работающий interpreter. В отчёте указано фактическое покрытие.

#### S4-15a. Один qword load PublishedBlock через tracked RAM

- Реализация ограничена exact `48 8B 03` (`MOV RAX,[RBX]`) в существующем
  direct V2 published memory-load пути. `od-host` зависит от `od-machine`;
  scoped callback синхронный и не сохраняет memory borrow в JIT mapping или
  metadata.
- `od-machine` временно предоставляет read-only qword `GuestMemory`: один
  RAM operand в пределах guest 4 KiB страницы, tracked four-level Long64 walk,
  без MMIO dispatch. Ошибки walk/backend и уже committed A updates возвращаются
  вызывающему даже при отказе; D не устанавливается для load.
- Host entry до первого memory access принимает только точную опубликованную
  форму и runnable Long64 CPL0/CPL3 context; прочие формы/profile отклоняются.
  Исполняется ровно одна инструкция. Unsupported RAM/MMIO/cross-page access
  остаётся host/backend отказом, не guest `#PF`.
- Проверки и ограничения: [S4-15a report](../reports/STAGE_4_S4-15A_2026-11-06.md)
  и [manifest](../reports/data/stage4/s4-15a-paged-v2-load-2026-11-06.json).
  Ограниченный срез принят после независимого ревью; родительский S4-15
  остаётся открытым.

### S4-16. Physical aliases и инвалидация исполняемого кода

- [ ] Принято после ревью.
- **Где:** writers `od-machine`, code dependencies/cache/dispatch `od-jit`, system tests.
- **Сделать:** связать блоки с физическими code bytes и нужным translation context. Уведомлять о committed write через любой разрешённый writer; обработать page-table remap и изменение текущего active block.
- **Проверка:** write alias→execute alias, одна guest page внутри host page, loader patch, cross-page code, fail-before-commit, byte write в active block, повторный lookup после invalidation.
- **Готово:** старые bytes не исполняются после архитектурно обязательной точки наблюдения; удаление cache entry не освобождает ещё используемую executable allocation преждевременно.

#### S4-16a. Loader patch неактивной register-only code page через два alias

- Реализован bounded срез: один vCPU, фиксированный Long64 paging snapshot,
  один физический RAM page, два guest linear aliases и exact
  `NativeRegisterV1` register-only code. Code range целиком помещается в одну
  guest 4 KiB страницу; patch выполняется между dispatch-вызовами.
- `PhysicalMemory` хранит непереполняющиеся версии физических RAM страниц.
  Успешная запись увеличивает только версии затронутых страниц; проверка
  диапазона, типа RAM и переполнения завершается до commit. Версии выделяют
  по 8 байт на RAM guest page. Это provisional изменение write path; замеров
  цены нет и speed claim не делается.
- `MachineBus` захватывает code range tracked instruction fetch'ем первой
  byte, проверяет executable translation и RAM-backed physical page, затем
  копирует только запрошенный диапазон, сохраняя точный offset внутри страницы.
  Оба alias обязаны отображать один physical page и один и тот же byte range;
  patch допускается только внутри `[range_offset, range_offset + length)`.
  Та же page translation покрывает оставшиеся байты страницы; fetch A updates
  коммитятся walker'ом до snapshot. API принимает только `AccessOrigin::Explicit`;
  descriptor-table origin отклоняется, чтобы не подменять CPL3 проверки правами CPL0.
- `PagedCodeSession` держит bus заимствованным эксклюзивно, содержит ровно два
  alias cache entries и две executable arena slots. Loader меняет один byte
  через tracked writable alias; после успешного commit версия backing страницы
  обновляется и обе cache entries удаляются до следующего lookup. Ошибка
  admission не связывает backing, длину и alias в сессии. Ошибка write не
  коммитит сам patch byte; оба tracked walk могут отдельно закоммитить
  Accessed effects, а только write walk может закоммитить Dirty effects до
  более позднего отказа. Если page-table entry физически перекрывает code backing, эти effects могут
  изменить и code bytes/generation даже при отказе patch. Перекрытие исключено
  из гарантии «неизменённый code backing при отказе»; каждый следующий lookup
  получает свежие bytes/generation и инвалидирует выбранный alias cache до
  исполнения, поэтому устаревший блок не исполняется повторно.
- Результат native entry декодируется backend'ом до возврата; published slot и
  PC map остаются владельческими пока entry исполняется и результат проверяется.
  CPU state клонируется; только в локальной копии CR3 временно очищается на время
  fixture-dispatch, затем исходный CR3 восстанавливается до переноса состояния
  вызывающей стороне.
- Проверено host сценарием: 10-byte code range начинается с offset `0x100`;
  patch вне точного диапазона и alias с другим offset отвергаются; отказ
  admission не резервирует сессию; оба alias исполнили старые bytes, отказ
  через read-only PTE оставил две cache entries, patch через alias A
  инвалидировал обе, следующие исполнения через B и A вернули новый immediate.
  Machine coverage проверяет CPL3 fault на supervisor-only mapping для fetch и
  patch, а `DescriptorTable` origin отклоняется обеими tracked code API.
  Graphify обновлён; замечания независимого ревью исправлены и повторно проверены.
  Финальное независимое ревью завершилось без замечаний; bounded срез принят,
  родительский S4-16 остаётся открытым.
- **Ограничения:** это не включает guest JIT stores, arbitrary writer callbacks,
  remap/TLB invalidation, cross-page code, active-block mutation, MMIO/DMA или
  SMP. Полный S4-16 остаётся открытым. См. [S4-16a report](../reports/STAGE_4_S4-16A_2026-11-06.md)
  и [manifest](../reports/data/stage4/s4-16a-code-alias-patch-2026-11-06.json).

#### S4-16b. Один guest MOV store инвалидирует неактивный JIT code через physical aliases

- Реализован bounded host adapter для одного синхронного `step_paged_mov`
  между native-вызовами. Допустимы только существующие точные формы CPU
  профиля; этот срез использует `48 89 0B` (`MOV [RBX],RCX`). Один vCPU,
  неизменный Long64 paging snapshot, две alias cache entries и одна
  physical code page из S4-16a.
- Paging controls и CR3 сверяются с session snapshot. CR3 остаётся отдельным
  аргументом machine adapter. `step_guest_mov` передаёт клонированный state с
  `cr3 = None` в `od_machine::step_paged_mov`, затем восстанавливает исходный
  CR3 перед переносом результата в caller state. При machine input error caller
  state остаётся без изменений; при архитектурном fault используется обычная
  commit-семантика profile. Mode/CPL сохраняются; walker получает explicit
  access origin и проверяет фактические page permissions.
- Host собирает только фактически committed physical write callbacks, включая
  A/D writes page walker. После возврата machine step reconciliation сначала
  сверяет пересечения с physical code page, читает её текущее поколение и при
  необходимости синхронно инвалидирует обе alias entries; только потом
  возвращает guest report или host error. Поэтому callback, записанный до
  последующего #PF, тоже обновляет cache state.
- Сценарии этого среза используют RAM. Существующий `step_paged_mov` также
  умеет направлять operand access в MMIO, но MMIO поведение здесь не проверяется;
  cache reconciliation получает только committed physical RAM write callbacks.
- Проверено: успешный guest qword store заменяет immediate в code backing,
  освобождает обе idle cache entries и последующее исполнение обоих aliases
  видит новые bytes. Отдельный CPL3 faulting store коммитит PML4 Accessed
  запись по `0x7400` в той же физической странице, что и code, затем получает
  #PF на unmapped operand: CPU state и retirement остаются неизменны,
  operand write отсутствует, generation меняется и обе cache entries
  инвалидируются. Store в другую RAM page коммитится без изменения code page
  generation и сохраняет обе cache entries.
- Это не добавляет общего writer notification, remap/TLB coupling,
  MMIO/DMA validation, SMP или active-block mutation. Code range остаётся внутри одной
  страницы по ограничению S4-16a; этот срез не проверяет cross-page data
  operand и не заявляет его поведение. Производительность adapter и write path
  этим correctness slice не измерялась; speed claim отсутствует. Независимое
  ревью пройдено без замечаний; bounded срез принят, родительский S4-16 и Stage 4
  остаются открытыми.
- См. [S4-16b report](../reports/STAGE_4_S4-16B_2026-11-06.md) и
  [manifest](../reports/data/stage4/s4-16b-guest-store-2026-11-06.json).

#### S4-16c. Физическая RAM-запись загрузчика инвалидирует неактивные aliases

- Добавлен `MachineBus::patch_physical_ram_with_callback`; прежний удобный API
  делегирует ему без изменения поведения. Callback передаётся существующему
  `PhysicalMemory::write_bytes` и сообщает точный диапазон после commit.
- `PagedCodeSession::write_physical_ram` принимает типизированный физический
  адрес и байты между native вызовами. Он не выдаёт mutable bus, ссылки на RAM
  или writer closure. После успешного либо ошибочного результата все фактически
  committed диапазоны проверяются общей helper-функцией с S4-16b. Пересечение
  captured code page перечитывает поколение и синхронно инвалидирует обе
  alias cache entries. Запись в другую страницу и отказ до commit кэш сохраняют.
- Проверено: точный callback на восьмибайтовую замену immediate; последующее
  исполнение обоих aliases видит новые bytes; запись за пределами admitted
  instruction range в той же code page консервативно инвалидирует обе записи;
  запись соседней guest page по адресу `0x6000` обновляет только её поколение;
  ROM write и диапазон, пересекающий RAM/ROM boundary, не меняют bytes,
  generation или resident cache entries. Срезные проверки прошли, Graphify
  обновлён, независимое ревью завершилось без замечаний.
- Ограничения: это только один privileged physical RAM loader write между
  native вызовами. Remap/TLB, cross-page code, MMIO/DMA, SMP и изменение
  активного native block остаются открытыми; производительность не измерялась.
  Родительский S4-16 и Stage 4 остаются открытыми.
- См. [S4-16c report](../reports/STAGE_4_S4-16C_2026-11-06.md) и
  [manifest](../reports/data/stage4/s4-16c-physical-loader-2026-11-06.json).

#### S4-16d. A/D callbacks snapshot и alias patch доходят до cache invalidation

- `MachineBus` предоставляет callback-варианты tracked code snapshot и
  one-byte alias patch; прежние API сохраняют поведение через no-op callback.
  Уведомляются только реально committed физические записи page walker и
  operand byte в порядке их commit.
- `PagedCodeSession` собирает уведомления snapshot и patch и вызывает общую
  `reconcile_code_writes` до возврата результата, в том числе перед
  распространением snapshot/patch fault и admission/alias ошибок после walk.
  Поэтому A/D-запись, физически перекрывающая code backing, инвалидирует обе
  idle alias entries до следующего lookup; неуспешный operand patch не
  изображается как committed write.
- Проверено: exact callback order для четырёх Accessed обновлений read preflight,
  Dirty обновления write walk и committed patch byte; CPL3 write-protection
  failure: read preflight устанавливает PDE A-bit по `0x7010` в code backing,
  затем write walk fault-ится; committed A update и исходный patch byte
  сохраняются, а обе cache entries синхронно удаляются. Fetch fault в
  другой PML4 ветви после PML4 A update по `0x7400` также удаляет обе entries
  до Snapshot error. Machine API tests дополнительно проверяют callback ranges
  и PFEC для snapshot fault (`0x14`), write fault (`0x7`) и физического mismatch.
  S4-16a/b/c regressions и все `od-machine` unit tests проходят.
- Независимое ревью завершено; исправлены замечания к точным assertions и
  evidence. Ограничения прежние: один vCPU, одна физическая
  code page, fixed paging snapshot и inactive native blocks. Это не добавляет
  TLB/remap invalidation, cross-page code, arbitrary writers, MMIO/DMA, SMP или
  active-block retirement. Родительский S4-16 и Stage 4 остаются открытыми.
- См. [S4-16d report](../reports/STAGE_4_S4-16D_2026-11-06.md) и
  [manifest](../reports/data/stage4/s4-16d-tracked-ad-callbacks-2026-11-06.json).

#### S4-16e. Retired INVLPG сбрасывает inactive aliases после remap

- `od-machine::step_paged_invlpg` исполняет ровно существующий bounded
  `od-isa::step_invlpg` профиль через tracked RAM fetch и передаёт фактические
  A/D callbacks, `PageFaultRecord` и исходную ошибку walker.
- `PagedCodeSession::execute_invlpg` проверяет фиксированный Long64 paging
  snapshot. После retired canonical invalidation effect он консервативно
  очищает обе idle cache entries и сбрасывает backing/range/alias binding.
  Запись PTE сама cache не очищает. Noncanonical retired INVLPG без
  translation-effect и rejected input сохраняют session.
- Проверено: два aliases исполняют старый immediate; remap leaf PTE меняет
  backing, но сохраняет 2 cache entries; noncanonical и rejected INVLPG их не
  очищают; отдельные исполненные INVLPG для A и B сбрасывают session; после
  этого новый alias с другим смещением и длиной допускается к новой backing и
  исполняет новый immediate с точным retirement/RIP. Machine callbacks
  покрывают успешный fetch, noncanonical retirement, rejection, CPL3 #GP после
  fetch и fetch #PF после A/D effects.
  Все `od-machine` unit tests и S4-16a…e host tests проходят; Graphify обновлён.
- **Граница:** это session cache reset после CPU invalidation effect, а не
  production TLB. Любой canonical retired INVLPG в bounded session
  консервативно очищает обе записи; архитектурные guest invalidations для двух
  linear aliases всё равно исполняются отдельно. Cross-page code, общие
  writers, production TLB, MMIO/DMA, SMP и active-block retirement остаются
  открытыми; S4-16 и Stage 4 остаются открытыми.
- См. [S4-16e report](../reports/STAGE_4_S4-16E_2026-11-06.md) и
  [manifest](../reports/data/stage4/s4-16e-invlpg-remap-2026-11-06.json).

### S4-17. Собрать локальный memory/fault differential набор

- [ ] Принято после ревью.
- **Где:** `tests/system/`, инфраструктура `od-testkit`, `docs/testing/MACHINE.md`.
- **Сделать:** один входной case и нормализованный результат для interpreter uncached/cached и JIT supported subset. Сохранять exception, registers, CR2, memory deltas, A/D, device events и причину завершения.
- **Проверка:** inject намеренное различие и убедиться, что comparator его обнаруживает; undefined поля сравнивать по явной маске. Hardware evidence S4-34 подключать отдельно, не маркировать локальные cases как native.
- **Готово:** можно воспроизвести и уменьшить mismatch без копии CPU/MMU семантики в testkit; общий walker не считается независимым oracle.

#### S4-17a. Paged qword load: uncached interpreter против published JIT

- [x] Принято после независимого ревью.
- **Case:** один exact `48 8B 03` (`MOV RAX,[RBX]`), фиксированный Long64/CPL0
  paging snapshot, независимые RAM instances, успех и настоящий missing-leaf
  `#PF`. Code/data находятся в разных PML4 ветвях; code-path A bits заранее
  установлены, чтобы сравнивать только data-path A effects.
- **Нормализованный результат:** все 16 GPR, RIP, masked RFLAGS и definedness,
  retirement/outcome, PageFault vector/PFEC/faulting RIP/CR2 value,
  architectural CR2 availability отдельно, физические RAM deltas, упорядоченные
  committed-write ranges и device-event observation. Сценарий зарегистрирован
  как `od-host` integration target `stage4_memory_fault`.
- **Проверка:** uncached `od_machine::step_paged_mov` против опубликованного
  `PublishedCode::execute_paged_mov_rax_rbx_v2`; literal assertions для RAX/RIP,
  #PF PFEC `0`, CR2 value и сохранения destination; comparator получает
  намеренно изменённый post-run A-bit delta и обязан назвать physical address
  и differing bit. Unit tests проверяют undefined/defined flag mask,
  отсутствующие observation и JSON strictness.
- **Граница:** cached interpreter явно представлен как unsupported: существующая
  translation-probe cache не исполняет CPU и не сохраняет tracked A/D semantics.
  Reducer, прочие memory формы, MMIO/device events, stores, cross-page, fetch
  faults и hardware evidence остаются в родительском S4-17/S4-34. Общий walker
  не является независимым x86 oracle; S4-17 и Stage 4 остаются открытыми.
- См. [S4-17a report](../reports/STAGE_4_S4-17A_2026-11-06.md) и
  [manifest](../reports/data/stage4/s4-17a-paged-mov-differential-2026-11-06.json).

### S4-18. GDT, IDT и TSS через настоящую память

- [ ] Принято после ревью.
- **Где:** `od-isa`, memory adapter `od-machine`, CPU/system tests.
- **Сделать:** нужные descriptor/table instructions, descriptor caching, limit/type/present/privilege checks, выбранные TSS/stack switching поля. Читать таблицы через правильный memory/access context.
- **Проверка:** valid/invalid selector, table limit, not-present, ошибочный type, fault при чтении таблицы и нужный stack switch. Отдельно подтвердить, что давно существующий setter не подменяет guest instruction.
- **Готово:** обязательные формы manifest исполняются; ошибки классифицируются как архитектурные `#GP/#SS/#NP/#TS/#PF` по выбранному стандарту.

#### S4-18a — bounded paged LIDT

- [x] Реализация и независимое ревью пройдены; срез принят.
- **Где:** `od-isa` exact `PagedLidtLong64V1`; `od-machine` tracked RAM adapter.
- **Сделать:** принимать только `0F 01 5C 24 ib` при фиксированных Long64
  control state; читать SS:RSP+sign-extended disp8 как один RAM-only 10-byte
  pseudo-descriptor внутри одной guest page. Сохранять фактические fetch/operand
  A updates, не устанавливать D и не обходить loaded IDT base.
- **Проверка:** nonidentity и signed disp8, atomic state commit, точный operand
  #PF после fetch, LOCK #UD/truncated fetch ordering, CPL #GP, crossing and
  noncanonical source, hole/backend separation, no-MMIO path, exact callbacks
  and PTE snapshots. Исполнение gate checks, descriptor caching, delivery and
  cross-page descriptor reads остаются за пределами этого среза.
- **Результат:** [S4-18a report](../reports/STAGE_4_S4-18A_2026-11-06.md) и
  [manifest](../reports/data/stage4/s4-18a-paged-lidt-2026-11-06.json).

#### S4-18b — bounded paged LGDT and guest-loaded GDT delivery

- [x] Реализация и независимое ревью пройдены; срез принят.
- **Case:** CPU исполняет exact `0F 01 5C 24 ib` LIDT, затем exact
  `0F 01 55 ib` LGDT через paged RAM, после чего настоящий WP fault доставляется
  через guest-loaded IDT/GDT и исполняется первая handler `MOV RAX,[RBX]`.
  Между этими инструкциями host не подменяет IDTR/GDTR.
- **Профиль:** Long64/CPL0, один CPU, private 4 KiB paging, fixed CR0/CR4/EFER
  и CPU-owned CR3. LGDT читает один canonical ten-byte pseudo-descriptor внутри
  одной guest page; использует существующие decoder и `load_gdtr` семантику,
  не читает новый GDT и не обновляет segment caches.
- **Проверка:** точный GDTR/IDTR/RIP commit, сохранение всех segment caches,
  faulting store с PFEC `3`, отсутствие store/retirement, CR2, полный exception
  frame, GDT-based delivery и сохранение sentinel. Негативные случаи включают
  privilege/fetch/operand faults, неверный base, cross-page source, unsupported
  form/control state, ограничение instruction fetch на границе гостевой
  страницы (включая RIP wraparound), и RAM-only отказ без MMIO dispatch.
- **Граница:** этот срез делает LIDT/LGDT гостевыми инструкциями для одного
  проверяемого delivery path. Он не реализует TSS/IST, selector cache loading,
  descriptor Accessed writes, `#DF`, shutdown или fault recovery.
- **Архитектурный анализ:** bounded implementation order and component split
  reviewed by `gpt-6-astra/high`.
- **Независимое ревью:** все замечания закрыты, включая границу fetch и RIP
  wraparound; результаты зафиксированы в [S4-18b report](../reports/STAGE_4_S4-18B_2026-11-06.md)
  и [manifest](../reports/data/stage4/s4-18b-paged-lgdt-2026-11-06.json).

#### S4-18c — bounded target CS descriptor Accessed-bit update during paged #PF delivery

- [x] Реализация и независимое ревью пройдены; срез принят.
- **Порядок:** Intel SDM Vol. 1 §6.14.1 перечисляет same-CPL frame/error-code
  stores до загрузки нового CS:EIP; Vol. 3A §3.4.5 задаёт установку descriptor
  Accessed bit при загрузке selector. Эти разделы сами по себе не гарантируют
  fault-visible ordering implicit A-bit write относительно frame stores при
  competing faults. Этот bounded emulator slice явно выбирает порядок
  frame stores → A-bit write → commit handler CS/RIP/RSP/flags для пути
  без alias. До frame stores чистый preflight сверяет qword operands с target
  descriptor byte, GDT trace и page-table entries достижимых stack slots;
  возможные page-walker A/D writes сверяются с descriptor и GDT trace. Это сохраняет отображения
  будущих frame slots; alias отклоняется без frame write. Preflight прекращает
  проверку на первой неуспешной stack access, чтобы tracked path сохранил её
  исходную PF/backend классификацию и недостижимый slot не влиял на результат.
  На последующем отказе записи сохраняются завершённые RAM/page-walk effects и CR2 вторичного `#PF`, но CPU
  entry не коммитится. Delivery исходного `#PF` с этой ошибкой требует `#DF`;
  поскольку `#DF` ещё не реализован, report будет terminal undelivered, а
  сохранённое CPU state нельзя считать возобновляемым или аппаратно
  restartable.
- **Сделать:** разрешить A=0 у уже валидного GDT target code descriptor в
  существующем Long64/CPL0/same-CPL/IST=0 профиле. Через paged tracked RAM
  записывать только access byte, устанавливая бит A и сохраняя остальные
  descriptor bits; CS cache публиковать с обновлёнными attributes только после
  успешной записи. A=1 не должен порождать descriptor write.
- **Проверка:** guest LIDT → LGDT → WP #PF → доставка → первая handler load с
  исходным A=0; точный frame и callback order, единственная descriptor-bit
  перемена и cache commit. Дополнительно проверить A=1 на read-only GDT и
  WP secondary #PF при A=0: точные phase/address/PFEC, сохранённый frame и
  отсутствие CPU entry commit. Невалидный descriptor не записывать; direct
  physical alias и frame rewrite GDT leaf PTE (включая redirect) отклонять до
  frame stores. Сохранить RAM-only/backend distinction.
- **Граница:** только один byte write из descriptor-table context; stack writes
  остаются page-contained qword. Не добавлять TSS/IST, privilege change,
  descriptor generalization, `#DF`/shutdown, PTE repair, INVLPG, IRETQ или retry.
  S4-18 и S4-19 остаются открытыми.
- **Нормативная основа:** Intel SDM version 090, [Vol. 1 §6.14.1](https://cdrdv2-public.intel.com/874241/253665-090-sdm-vol-1.pdf)
  и [Vol. 3A §3.4.5](https://cdrdv2-public.intel.com/782157/325384-sdm-vol-3abcd.pdf).

#### S4-18d — bounded paged LTR AX

- [x] Реализован и проверен локально; независимое ревью — см. [отчёт S4-18d](../reports/STAGE_4_S4-18D_2026-11-08.md).
- **Реализовано:** `step_paged_ltr_ax` (`0F 00 D8`) в профиле LGDT/LIDT; чтение 16-байтового
  TSS-дескриптора из GDT через supervisor walk, `validate_tss_descriptor`, write-permission walk,
  D leaf, запись дескриптора с busy-битом, затем commit `TaskRegisterCache` (`LongModeState::tr`) и RIP.
  Отказы: `#UD` (LOCK), `#GP`/`#NP`, `#PF` (W), `TableAlias`/`TranslationChanged`.
- **Граница:** без чтения содержимого TSS, stack/task switching, IST, TLB/JIT, cross-page
  descriptor, MMIO-дескриптора и SMP-атомарности; CR3/EFER явные аргументы. S4-18 остаётся открытым.
- **Нормативная основа:** Intel SDM Vol. 2 (`LTR`) и Vol. 3A §7.2.3 (64-bit TSS descriptor).

#### S4-18e — bounded paged MOV DS/ES/FS/GS

- [x] Реализован и проверен локально; независимое ревью — см. [отчёт S4-18e](../reports/STAGE_4_S4-18E_2026-11-08.md).
- **Реализовано:** `step_paged_mov_ds/es/fs/gs` (`8E D8/C0/E0/E8`, источник AX) в профиле LGDT/LIDT/LTR:
  RAM-only чтение 8-байтового GDT-дескриптора через supervisor walk, существующие CPU-проверки
  (`#GP`/`#NP`), запись accessed-бита одним byte write, затем commit selector/cache/base и RIP.
- **Граница:** CPL0, только AX, без null/LDT селекторов, SS/CS, TLB/JIT, дескриптора через
  границу страницы и изоляции alias с page-table страницей. S4-18 остаётся открытым.

<a id="s4-19-доставка-исключений-и-восстановление-после-fault"></a>

### S4-19. Доставка исключений и восстановление после fault

- [x] Принято после независимого review и проверок ограниченного same-CPL
  Long64 recovery path; cached-TLB behavior остаётся вне этого результата.
- **Где:** delivery CPU state machine в `od-isa`, orchestration `od-machine`.
- **Сделать:** единый путь delivery с правильным frame/error code, retry RIP и return. Явно обработать fault при descriptor/stack access, правила `#DF` и machine outcome при невозможности его delivery.
- **Проверка:** guest handler `#PF` исправляет PTE, выполняет нужную invalidation и повторяет faulting instruction ровно с допустимыми effects; сломанный handler/stack приводит к ожидаемому secondary fault/shutdown.
- **Готово:** исключение реально доставлено guest handler и execution продолжается; возвращённый host enum без delivery не закрывает задачу.

#### S4-19a. Carry architectural CR2 in CPU state

- [x] Реализация и независимое ревью пройдены.
- **Сделать:** добавить optional CR2 в `LongModeState` с `None` у старых
  профилей; мигрировать только struct literals без `..Default`. Snapshot и
  fragment conversions, которые CR2 не представляют, должны явно отказывать,
  а не терять значение.
- **Проверка:** default/clone сохраняют контракт CR2; конвертеры reject
  `Some(CR2)`; workspace build охватывает все инициализаторы и call sites.
- **Граница:** в этом срезе нет инструкции MOV CR2. Page-fault delivery,
  установка CR2 при fault и exception recovery реализуются в S4-19b и следующих
  частях S4-19.
- **Результат:** [S4-19a report](../reports/STAGE_4_S4-19A_2026-11-06.md) и
  [manifest](../reports/data/stage4/s4-19a-cr2-state-2026-11-06.json).

#### S4-19b. Bounded paged #PF delivery and handler entry

- [x] Ограниченный срез принят после независимого code review и повторного review исправлений.
- **Case:** Long64/CPL0, один CPU, private RAM, same-CPL interrupt gate с IST=0.
  Guest выполняет `MOV [RBX],RCX` в present supervisor read-only страницу при
  `CR0.WP=1`, получает настоящий `#PF` с PFEC `3`, прежним RIP, без retirement
  и без изменения данных. Delivery фиксирует CR2, читает IDT/GDT через paged
  tracked memory и строит шестисловный 64-bit frame с PFEC; затем guest
  обработчик исполняет первую `MOV RAX,[RBX]` и наблюдает прежний sentinel.
- **Проверка:** literal state и все шесть qword frame, полный fault record,
  CR2, RF=1 в сохранённом RFLAGS образе fault-class `#PF`, page-table A/D
  effects и ordered callbacks. Синхронный `#PF` доставляется и при IF=0.
  Исходный невыравненный RSP сохраняется в frame при аппаратном выравнивании
  нового stack. Отрицательные случаи покрывают profile rejection, selector TI
  и code descriptor Accessed=0, IDT/GDT read fault, stack write failure после
  частичного commit и backend failure. Ошибка fetch первой handler instruction
  учитывается отдельно от уже успешной доставки.
- **Граница:** один #PF vector, CPL0→CPL0, IST=0, фиксированные 4 KiB tables,
  frame/table accesses в заранее ограниченных RAM mappings. Target code
  descriptor — GDT, Accessed=1, base=0, effective limit 4 GiB; доставка не
  синтезирует его Accessed-bit write. Secondary faults при delivery —
  терминальный недоставленный результат; автоматическая `#DF`, shutdown
  classification, исправление PTE, INVLPG, IRETQ и retry остаются следующими
  частями S4-19. `MOV RAX,[RBX]` доступен только в новом selector этого slice;
  родительский S4-12d probe сохраняет свой прежний allowlist. Не называть этот
  slice полным recovery.
- **Нормативная основа:** Intel SDM Vol. 3A §6.14.2, [64-Bit Mode Stack
  Frame](https://www.intel.com/content/dam/www/public/us/en/documents/manuals/64-ia-32-architectures-software-developer-vol-3a-part-1-manual.pdf)
  и Vol. 3B §20.3.1.1, [RF in the fault-class exception frame](https://cdrdv2-public.intel.com/874250/253669-090-sdm-vol-3b.pdf).
- **Результат:** реализация и проверки записаны в
  [S4-19b report](../reports/STAGE_4_S4-19B_2026-11-06.md) и
  [manifest](../reports/data/stage4/s4-19b-paged-pf-delivery-2026-11-06.json).

#### S4-19c. Secondary #PF, #DF delivery, and shutdown

- [x] Ограниченный срез принят после независимого code review; замечания P2/P3 исправлены и повторно проверены.
- **Case:** в существующем Long64/CPL0/same-CPL/IST=0 profile настоящий #PF
  при доставке исходного #PF инициирует ровно одну попытку vector 8. Успешная
  доставка #DF строит frame с error code 0, сохраняет исходный RIP только для
  диагностики, не добавляет RF и оставляет CR2 равным последнему реальному
  page fault. Page fault при доставке #DF переводит `CpuActivity` в
  `Shutdown`; нового exception delivery не выполняется. Backend/profile
  failures сами по себе не синтезируют shutdown.
- **Проверка:** A=0 target descriptor на read-only GDT даёт nested #PF, затем
  #DF и выполнение первой handler MOV; отдельный stack case проверяет #PF во
  время #DF frame, последнее CR2, сохранённый частичный frame и отсутствие
  fetch/памяти после shutdown. Проверены #DF error-code slot и RFLAGS без RF.
- **Граница:** только #PF → #DF для фактического page fault при ограниченной
  доставке #PF и shutdown для page fault при доставке #DF. Другие пары
  исключений, TSS/IST, privilege transitions, guest PTE repair, INVLPG,
  IRETQ, retry исходной инструкции и общий vCPU lifecycle остаются открытыми.
  Старый V2 testkit snapshot не кодирует Shutdown и явно отклоняет такую
  конверсию.
- **Результат:** см. [S4-19c report](../reports/STAGE_4_S4-19C_2026-11-06.md)
  и [manifest](../reports/data/stage4/s4-19c-double-fault-shutdown-2026-11-06.json).

#### S4-19d. Guest PTE repair and INVLPG

- [x] Принято после независимого ревью и проверок.
- **Case:** после реального #PF при WP=1 handler через отдельный exact qword
  `MOV [RDI],RSI` записывает новый RW leaf PTE в writable alias page-table
  page, затем исполняет настоящий `INVLPG [RBX]` для исходной faulting GVA.
  Fixture задаёт RDI и RSI так, чтобы менять только нужный leaf PTE; сам
  selector этого alias/value не ограничивает. RBX/RCX остаются без изменений.
- **Проверка:** PTE меняется только гостевой инструкцией, A и supervisor-only
  U/S=0 сохраняются, D задаётся tracked store для alias mapping, исходные данные остаются прежними,
  CR2 остаётся исходным, INVLPG ретируется ровно один раз с faulting GVA.
- **Граница:** guest IRETQ и retry исходного MOV не выполняются; кэшированной
  translation в tracked execution path нет, поэтому этот срез не утверждает
  необходимость INVLPG для видимости нового PTE или eviction stale TLB entry.
  Cross-page qword operands отклоняются до operand access.
  Следующие S4-19e1/e2 покрывают guest stack adjustment, paged IRETQ, retry и
  завершение записи.
- **Результат:** см. [S4-19d report](../reports/STAGE_4_S4-19D_2026-11-06.md)
  и [manifest](../reports/data/stage4/s4-19d-guest-pte-repair-invlpg-2026-11-06.json).

#### S4-19e1. Guest error-code discard and paged IRETQ

- [x] Принято после независимого ревью и проверок; исходную запись ещё не повторяем.
- **Case:** после S4-19d handler гостевой точной `LEA RSP,[RSP+8]`
  (`48 8D 64 24 08`) удаляет error-code slot, затем paged `IRETQ` (`48 CF`)
  читает настоящий пяти-qword same-CPL frame и CS/SS descriptors через
  tracked RAM. Возврат восстанавливает faulting RIP, исходный невыравненный
  RSP и RF=1. SS descriptor настоящий, CPL0 writable, заранее Accessed.
- **Граница профиля:** Long64 CPL0→CPL0, WP=1, точные формы инструкций,
  заранее accessed валидные CS/SS, private synchronous RAM и выключенные
  FRED/CET/shadow-stack/VMX/SMM. Live RF должен быть 0 перед LEA и IRETQ;
  сохранённый RF принимается из fault frame и восстанавливается только при
  успешном IRETQ. Ошибка tracked чтения обновляет CR2 и не частично коммитит
  возврат; уже завершённые A/D effects остаются наблюдаемыми.
- **Проверка:** весь guest путь от настоящего WP #PF через repair/INVLPG и
  error-code discard возвращает RIP к исходному MOV, сохраняет CR2, control
  registers и GPR, возвращает точный RSP/RFLAGS, не меняя faulting data.
  Хост не подменяет RIP/RSP/PTE после начала сценария.
- **Не заявлено:** повтор faulting MOV, его RF-clear timing и commit данных
  остаются в S4-19e2. Tracked path не кеширует translation, поэтому INVLPG
  necessity/stale-TLB eviction не проверяются.
- **Результат:** см. [S4-19e1 report](../reports/STAGE_4_S4-19E1_2026-11-06.md)
  и [manifest](../reports/data/stage4/s4-19e1-guest-paged-iretq-2026-11-06.json).

#### S4-19e2. Faulting MOV retry and data commit

- [x] Принято после независимого review и проверок.
- **Case:** после успешного S4-19e1 IRETQ гостевой CPU повторно исполняет
  исходный exact `MOV [RBX],RCX` с сохранённого RIP. Исправленный supervisor
  PTE допускает одну запись; следующая guest load читает новое значение.
- **Проверка:** одна успешная data write и одна retirement, RIP проходит к
  следующей guest instruction, восстановленный RF очищается в начале повторной
  инструкции по профилю Intel, итоговое значение и canary подтверждены.
- **Граница:** только same-CPL Long64 WP=1, частный RAM и заданный faulting
  store. Общая обработка debug breakpoints, FP exceptions, иных RF-переходов и
  cached TLB остаётся вне этого среза.
- **Результат:** см. [S4-19e2 report](../reports/STAGE_4_S4-19E2_2026-11-06.md)
  и [manifest](../reports/data/stage4/s4-19e2-guest-fault-retry-2026-11-06.json).

### S4-20. CPL3 и системные вызовы/возвраты

- [ ] Принято после ревью.
- **Где:** `od-isa`, `tests/system/`, точные CPU prerequisites этапа 2.
- **Сделать:** реализовать переходы CPL и нужные Linux entry/return механизмы из S4-00, связанные MSR/flags и stack rules. Не расширять same-CPL IRETQ fixture claim на все формы.
- **Проверка:** CPL3 read/write/fetch protection, запрещённые privileged instructions, вход в kernel, изменение kernel-visible данных, возврат к следующей user instruction; неправильные selectors/addresses/flags на return.
- **Готово:** собственный guest CPL3 probe проходит полный roundtrip; error paths не оставляют частично переключённые привилегии.

#### S4-20a. Synthetic paged IRETQ outer-privilege CPU semantics

- [x] CPU-only срез принят после независимого Sol/high review без замечаний
  P0–P3; machine integration S4-20b и родительский S4-20 остаются открытыми.
- **Профиль:** synthetic CPU с 48-bit linear addresses, CPUID leaf 7 равен нулю,
  `CR4.LA57=0`; Long64, один CPU и CPU-owned state для four-level paging.
  Этот профиль исключает LA57 transition только внутри данного сценария и не
  утверждает, что Linux loader уже реализован или выбранный Ubuntu artifact
  загружен. Unit tests используют guest-memory test double, а не tracked
  page-walk backend.
- **Сценарий:** CPU исполняет точный `48 CF` при CPL0 и пятисловном
  outer-return frame; user CS — плоский GDT descriptor `access=0xfb, flags=0xa`,
  user SS — плоский writable GDT descriptor `access=0xf3, flags=0xc`; оба
  заранее имеют Accessed=1. DS/ES/FS/GS selectors должны быть нулевыми.
  Входной live RF обязан быть очищен; сохранённые RFLAGS допускают только
  текущую маску integer flags, IF, IOPL и RF. Успешный
  commit меняет RIP, RSP, CS/SS selectors и caches, flags, сохраняя остальные
  CPU registers. Входные проверки требуют CR4.LA57=0.
- **Проверка:** корректные RPL/DPL/type/present для user CS/SS; неверные
  selectors/descriptors, RIP/RSP и flags завершаются `Unsupported` ограниченного
  профиля без частичного CPU commit. Guest page faults при fetch/frame/descriptor
  reads возвращаются как `Fault`; backend failures остаются `ReadAborted`.
  Архитектурные ошибки selector/return,
  требующие #GP/#SS и точной error priority за пределами принятого profile,
  остаются не реализованы. CPU не откатывает side effects, уже совершённые
  переданным memory backend, но этот slice не проверяет tracked A/D updates.
  Сохранять Intel read/fault ordering: RIP/CS/RFLAGS,
  затем проверка CS descriptor, затем RSP/SS, canonicality saved RIP и SS
  descriptor.
- **Граница:** только CPU-семантика Long64 CPL0→CPL3 с этим пятисловным frame.
  Machine-derived CPL context и исполнение user MOV выделены в S4-20b.
  SYSCALL/SYSRET, TSS stack switch, exceptions из CPL3, general-purpose IRETQ,
  полный Linux roundtrip и принятие родительского S4-20 остаются открытыми.
- **Нормативная основа:** Intel SDM revision 090, Vol. 2A `IRET/IRETD/IRETQ`
  и Vol. 3A §§7.14.2–7.14.3; [combined PDF](https://cdrdv2-public.intel.com/874240/325462-090-sdm-vol-1-2abcd-3abcd-4.pdf).
- **Результат:** [S4-20a report](../reports/STAGE_4_S4-20A_2026-11-06.md) и
  [manifest](../reports/data/stage4/s4-20a-paged-iretq-cpl3-2026-11-06.json).

#### S4-20b. Machine CPL context and one CPL3 paged MOV

- [x] Отдельный межкомпонентный срез принят после независимого
  `gpt-6.1-sol/high` review; подтверждённых замечаний P0–P3 не осталось.
- **Сценарий:** один machine wrapper выполняет paged IRETQ CPL0→CPL3, строит
  `AccessContext` только из committed CS RPL/cache и исполняет следующую exact
  `48 8B 03` (`MOV RAX,[RBX]`) на CPL3. Wrapper не принимает внешний privilege
  context или альтернативный CR3; CPU-owned CR3 остаётся активным.
- **Ограничение fetch:** все три байта инструкции должны находиться в одной
  guest page; допускается RIP offset `0xffd`. Cross-page instruction fetch и
  fault второй fetch-страницы остаются за границей этого среза.
- **Проверка:** user fetch и data read успешны; supervisor-only fetch даёт
  PFEC `0x15`, data read — PFEC `0x05` при NXE=1, с точными CR2 и saved RIP.
  Проверить user U/S protection на всех уровнях, сохранение завершённых A
  updates, отсутствие D updates/data writes у load и разделение guest faults
  от backend errors. Fault второй инструкции не откатывает уже committed
  переход CPL3; wrapper возвращает отдельные результаты IRETQ и MOV.
- **Граница:** одна точная load-форма и фиксированный four-level profile;
  user stores, cross-page instruction fetch, доставка исключений из CPL3 и
  syscall roundtrip остаются открытыми.
- **Результат:** [S4-20b report](../reports/STAGE_4_S4-20B_2026-11-06.md),
  [manifest](../reports/data/stage4/s4-20b-machine-iretq-cpl3-load-2026-11-06.json),
  [machine contract](../specs/MACHINE_CONTRACT.md#machine-iretq-followed-by-one-cpl3-load-s4-20b) и
  [verification record](../testing/MACHINE.md#s4-20b-machine-iretq-and-cpl3-load).

#### S4-20c. Bounded paged SYSCALL → kernel store → SYSRETQ → user load

- [x] Ограниченный CPU/machine vertical slice принят после независимого
  Sol/high review без замечаний P0–P3; parent S4-20 и Stage 4 остаются открытыми.
- **Начальное состояние:** синтетическое CPU-owned CPL3 state после внешней
  fixture. До исполнения заданы `EFER=0xd01`, `STAR`, `LSTAR`, `FMASK`, CR3 и
  четырёхуровневые таблицы; generic WRMSR и запуск через IRETQ в этот slice не
  входят. CPU API не принимает параллельные external CPL/CR3/EFER values.
- **Сценарий:** точные четыре retirement без host-перезаписи CPU/RAM между
  инструкциями: CPL3 `0F 05` (`SYSCALL`); CPL0 `4C 89 17`
  (`MOV [RDI],R10`) по supervisor-only LSTAR; CPL0 `48 0F 07` (`SYSRETQ`);
  CPL3 `48 8B 03` (`MOV RAX,[RBX]`) по адресу, сохранённому SYSCALL в RCX.
  `RDI` и `RBX` отображают одну физическую data page как kernel RW и user RO.
  Начальный RAX и исходное содержимое страницы отличаются от nonce в R10,
  чтобы отдельный user load был наблюдаемым.
- **CPU contract:** только Long64, canonical48, physical36, 4 KiB pages,
  `CR0=0x80010033`, `CR4=0x20`, один running CPU, без STI shadow и async event.
  Отдельное opt-in syscall-state bundle ограничивает EFER/STAR/LSTAR/FMASK,
  selector layout и flags; `SCE=0` даёт #UD для SYSCALL и SYSRETQ, SYSRETQ из
  CPL3 и noncanonical RCX дают #GP(0) без частичного privilege commit. Проверить
  RFLAGS mask SYSRETQ, OF, очистку reserved bit 15 и definedness. IF=1
  маскируется в kernel через FMASK и восстанавливается из R11. Проверить
  допустимый STAR layout, включая вычисление SS из исходного selector field;
  учитывать definedness для RCX/R11/RFLAGS, selector caches и точные RSP effects.
  Каждый отдельный шаг требует текущие `RFLAGS.TF=0` и `RFLAGS.RF=0`;
  при нарушении профиль возвращает `Rejected` до fetch без изменений CPU.
  SYSRETQ дополнительно отклоняет `R11.TF=1` после проверок SCE, CPL и
  canonical RCX; CPU не изменяется, но уже выполненные page-walk A updates и
  соответствующие callbacks при fetch сохраняются.
- **Проверка:** четыре точные кодировки, все instruction fetch/data accesses
  проходят tracked paging с CPL из committed CS. Проверить RCX/R11, FMASK,
  CS/SS selectors/caches, неизменность RSP/CR3, одну kernel write, user load
  по RBX и nonce в RAX; оба alias используют реальные U/S permissions.
  SYSCALL/SYSRETQ не читают GDT и не переключают RSP в этом сценарии. Проверить
  PFEC/CR2/RIP, A/D updates и сохранение уже завершённого перехода/записи при
  последующей ошибке; backend errors остаются отдельно от guest faults.
- **Граница:** fetch каждой инструкции и каждый qword operand должны целиком
  помещаться в одну guest page; cross-page fetch/operand остаются вне профиля.
  Межинструкционный stop/resume относится к S4-21.
- **Граница:** нет stack switch, TSS/IST, SWAPGS, async events, general WRMSR,
  compatibility SYSRET или Linux entry assembly. Начальный CPL3 fixture
  задан явно; профиль не объявляет syscall feature в DiagnosticV1 и не закрывает
  S4-00, Linux userspace или родительский S4-20.
- **Нормативная основа:** Intel SDM revision 093, Vol. 2B, `SYSCALL` и
  `SYSRET`; Vol. 3A, IA32_EFER/STAR/LSTAR/FMASK и 64-bit mode state
  ([official manuals](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html)).
- **Результат:** [S4-20c report](../reports/STAGE_4_S4-20C_2026-11-06.md),
  [manifest](../reports/data/stage4/s4-20c-paged-syscall-roundtrip-2026-11-06.json),
  [machine contract](../specs/MACHINE_CONTRACT.md#paged-syscall-kernel-store-sysretq-and-user-load-s4-20c) и
  [verification record](../testing/MACHINE.md#s4-20c-paged-syscall--kernel-store--sysretq--user-load).

#### S4-20d. CPL3 paged store → load после IRETQ

- [x] Ограниченный срез принят после независимого Sol/high review без
  замечаний P0–P3; родительский S4-20 и Stage 4 остаются открытыми.
- **Профиль:** Long64, canonical48, physical36, 4 KiB, четырёхуровневые
  таблицы, `CR0=0x80010033`, `CR4=0x20`, `EFER=0xd00`, один running CPU.
  Новый opt-in вход выводит CPL из committed CS, CR3 — из CPU; не требует
  syscall MSR и не меняет прежний load-only контракт. Текущие TF, RF и AC
  должны быть очищены до fetch; инструкция и qword operand находятся каждый
  целиком в одной странице. Для обеих форм сначала после fetch/decode проверяется
  пересечение qword с границей страницы по исходному RBX; только затем проверяется
  canonicality. При одновременном crossing и noncanonical адресе результат —
  `Rejected(CrossPageOperand)` без доступа к операнду; предшествующий fetch
  мог уже обновить A bits. Этот порядок относится только к новому opt-in входу;
  legacy load-only entry и его page-aware adapter сохраняют прежнее поведение.
  Новый вход также
  отклоняет остальные store-кодировки до доступа к операнду.
- **Сценарий:** принятый paged IRETQ выполняет существующий S4-20b `48 8B 03`
  load. Затем новый entry отдельно исполняет `48 89 0B` (запись RCX в `[RBX]`)
  и `48 8B 03` (повторное чтение в RAX). Nonce отличается от исходной памяти;
  каждый MOV имеет отдельное retirement; после каждого шага сохраняются CPL3,
  CS/SS, RSP и CR3. Следующая ошибка не откатывает завершённые переход или
  запись.
- **Приёмка:** tracked paging и фактическая запись/чтение nonce; user RO store
  и supervisor-only store дают #PF PFEC `0x07`, supervisor-only read — `0x05`;
  supervisor-only instruction fetch на каждом из четырёх уровней и NX fetch дают
  PFEC `0x15`; nonpresent user data read/store дают соответственно `0x04`/`0x06`,
  а nonpresent user instruction fetch — `0x14`.
  Для fetch-fault матрицы IRETQ завершается отдельным machine entry без user
  access; новый CPL3 MOV является первой пользовательской выборкой, и все его
  PTE A bits проверяются очищенными до шага. Проверить независимо заданные
  ordered callbacks и полные PTE A/D readbacks для первого fetch, nonpresent
  data и store-fault cases;
  защита не записывает операнд и не устанавливает D. Physical backend store
  abort подтверждается как `OperandPhysical` с точным GPA. Проверить границы
  одной страницы, ограничение flags и сохранение ранее выполненных эффектов.
- **Граница:** общие debug/alignment-check semantics, межстраничные обращения,
  privileged instructions, invalid-selector IRETQ и доставка исключений CPL3
  остаются за пределами среза.
- **Основа:** Intel SDM revision 093, Vol. 3A §§5.6–5.7
  ([Intel PDF](https://cdrdv2.intel.com/v1/dl/getContent/671190)).
- **Результат:** [S4-20d report](../reports/STAGE_4_S4-20D_2026-11-06.md),
  [manifest](../reports/data/stage4/s4-20d-paged-cpl3-store-load-2026-11-06.json),
  [machine contract](../specs/MACHINE_CONTRACT.md#paged-cpl3-mov-store-load-after-iretq-s4-20d) и
  [verification record](../testing/MACHINE.md#s4-20d-paged-cpl3-mov-store-load-after-iretq).

#### S4-20e. CPL3 paged MOV CR0 после IRETQ

- [x] Ограниченный срез принят после независимых Sol code/docs reviews без
  оставшихся замечаний; родительский S4-20 и этап 4 остаются открытыми.
- **Профиль:** Long64, canonical48, physical36, четыре уровня по 4 KiB,
  `CR0=0x80010033`, `CR4=0x20`, `EFER=0xd00`. CPL и CR3 берутся из CPU;
  некорректный CS, TF, RF и AC отклоняются до fetch. Смещение RIP больше
  `0xff1` исключено до fetch.
- **Цель:** после IRETQ пользовательский `0F 22 C0` вызывает #GP(0),
  `F0 0F 22 C0` вызывает #UD; fault при fetch имеет приоритет. Изменения
  A от завершённых page walks сохраняются; CR0 и состояние инструкции не
  фиксируются. Доставка исключения и переключение стека остаются вне среза.
- **Результаты:** direct system entry test подтверждает #GP(0), LOCK #UD,
  fetch precedence, supervisor-only/NX fetch PFEC `0x15` и nonpresent fetch
  PFEC `0x14` с точными linear address, CR2, saved RIP, A effects и callbacks;
  также проверены физический fetch backend, сохранение предыдущего store,
  граница RIP и profile admission. Page-walk A-write отказ проверен на общем
  writer seam, но не внедрён end-to-end через machine report adapter.
- **Проверки:** 10 `cpl3_` ISA тестов; 39 system test functions с одной
  дополнительной nonpresent-fetch assertion case внутри suite; targeted
  `cpl3_mov_cr0_first_fetch_enforces_user_page_permissions`; page-walk
  A-write failure matrix и fetch-specific case; `cargo check -p od-machine
  --all-targets --offline`, fmt и diff-check пройдены. Graphify после кода:
  27 555 узлов, 69 202 связи. Повторное независимое Sol code review после
  исправления P3 прошло без замечаний P0–P3; docs review также завершено без
  оставшихся замечаний.
- **Результат:** [S4-20e report](../reports/STAGE_4_S4-20E_2026-11-07.md),
  [manifest](../reports/data/stage4/s4-20e-paged-cpl3-mov-cr0-2026-11-07.json),
  [machine contract](../specs/MACHINE_CONTRACT.md#paged-cpl3-mov-cr0-after-iretq-s4-20e) и
  [verification record](../testing/MACHINE.md#s4-20e-paged-cpl3-mov-cr0-after-iretq).

### S4-21. Жизненный цикл одного vCPU

- [ ] Принято после ревью.
- **Где:** `od-machine`, CPU interruptibility contract `od-isa`, runner orchestration.
- **Сделать:** определить reset/entry, running, halted, paused, stopping/stopped, guest shutdown и faulted host session. Ввести bounded execution/poll points, сохранение точного состояния и единый STOP path.
- **Проверка:** STOP во время исполнения и HLT, pause/resume без потерянного состояния, отсутствие busy loop у остановленной машины, корректный reset без старых pending events.
- **Готово:** runner различает guest completion и host cancellation; HLT сам по себе не сообщает успешное завершение теста.

#### S4-21a. Синхронная bounded диагностическая сессия

- [x] Ограниченный S4-21a принят после независимого ревью.
- **Где:** `crates/od-machine/src/diagnostic_vcpu.rs` и экспорт crate.
- **Сделать:** одна сессия владеет `LongModeState` и `MachineBus`; синхронное выполнение только в runnable, конечный instruction budget, pause/resume без изменения CPU/RAM, идемпотентный STOP и сохранение `rflags_defined` между вызовами.
- **Границы:** STOP во время вызова поступает через существующий `od-isa` poll callback. Work-limit возвращает управление без автоматического продолжения; guest fault, unsupported instruction и backend error сохраняются как разные терминальные причины. Нет HLT wakeup, reset, IRQ, MMU, JIT, VMHost/IPC или REP continuation.
- **Проверка:** unit tests на реальный fetch/load/store через `IdentityMemoryAdapter`, эквивалентность единого бюджета и нескольких квантов вместе с defined-mask, переходы pause/resume/stop, STOP до fetch и на poll point, раздельные fault/unsupported/backend exits, возврат при work-limit.
- **Готово:** bounded сессия и её проверяемые ограничения реализованы; общий lifecycle S4-21 остаётся открытым до следующих срезов.

#### S4-21b. Единый paged probe quantum с гостевым HLT

- [x] Ограниченный S4-21b принят после независимого ревью; S4-21 остаётся открытым.
- **Где:** `od-isa::step_paged_probe` и `od-machine::run_paged_probe`.
- **Контракт:** профиль `PagedCpl0ProbeV1` допускает только `48 8B 03` (`MOV RAX,[RBX]`), `48 89 0B` (`MOV [RBX],RCX`), bare `EC`/`EE` и `F4`. CPU проверяет admission и CR0/CR4; machine проверяет CR3/EFER и подаёт page-aware memory. Quantum владеет конечным budget, STOP precedence и одним decode на границе.
- **Границы:** STOP проверяется первым, затем budget, затем fetch/decode. HLT фиксирует `Halted` и при повторном вызове не делает fetch/effects; HLT не означает guest success. Предыдущие retirement, A/D writes и callbacks остаются committed при позднем выходе; точные архитектурные fault records сохраняются. Нет exception delivery/CR2 update.
- **Проверка:** один quantum и split quanta для store→load→OUT→HLT; STOP/resume без повтора committed effects; zero/boundary budgets, HLT/no-op repeat, admission/allowlist, page-aware faults, backend/PIO errors и A/D callbacks.
- **Evidence:** [отчёт S4-21b](../reports/STAGE_4_S4-21B_2026-11-05.md), [JSON](../reports/data/stage4/s4-21b-paged-probe.json), [методика](../testing/MACHINE.md#s4-21b-bounded-paged-probe-quantum). `od-isa`: 2,014 unit + 25 integration; `od-machine`: 116; workspace check, fmt, layout и diff успешны. `graphify update .` обновил code graph; docs-aware semantic extraction не завершена.
- **Не заявлено:** Linux или собственное kernel boot, IRQ/APIC, exception delivery/CR2, полный S4-21, S4-24 или S4-32. Этап 4 остаётся открытым.

#### S4-21c. Bounded paged owner и reset UART RX

- [x] Ограниченный срез принят после независимого Sol/high review без
  замечаний P0–P3; родительский S4-21 остаётся открытым.
- **Контракт:** `PagedVcpuSession` принимает только fixed Stage4FlatProbe image; loader создаёт согласованные CPU, RAM, bus и UART. STOP фиксируется владельцем, pause/resume сохраняет guest activity; halted и остановленная сессии не исполняют quantum и не доставляют RX.
- **Reset:** replacement image и no-fetch CPU/paging checks выполняются до checked reset существующего RX pump; после успеха очередь очищена, generation увеличено, handle перенаправлен и машина заменена. Caller-owned time не изменяется. Generation token не входит в публичный API owner.
- **Проверки:** `cargo test -p od-machine --lib --offline` (120); `cargo test -p od-machine --test paged_probe_quantum --test uart_rx_events --offline` (21 + 7); `cargo fmt --all -- --check`; `cargo check -p od-machine --all-targets --offline`; diff check. Store test подставляет инструкцию только внутри теста: штатный image profile содержит load/OUT/HLT. Layout check не проходит из-за ранее зарегистрированного `smoke/` (ISSUE-056). Детали: [отчёт](../reports/STAGE_4_S4-21C_2026-11-07.md), [manifest](../reports/data/stage4/s4-21c-paged-vcpu.json), [методика](../testing/MACHINE.md#s4-21c-bounded-paged-vcpu-owner).
- **Границы:** нет runner integration, completion/cancellation predicate, IRQ wakeup или архитектурного x86 reset. S4-21 и Stage 4 остаются открытыми.

#### S4-21d. Runner completion и deterministic cancellation

- [x] Ограниченный срез принят после независимого `gpt-6.1-sol/high` ревью без замечаний к коду; замечание к формулировке README исправлено. S4-21 остаётся открытым.
- **Контракт:** `--stage4-probe` использует `PagedVcpuSession`; HLT подтверждает `guest_completed` только при трёх инструкциях, совпадении ожидаемого nonce с RAX и единственного UART byte с его младшим байтом. Отдельные статусы для HLT, несовпадения, STOP, лимита и ошибок.
- **Проверки и границы:** 8 runner CLI tests, 1 reproducible-build CLI test, 1 session drain unit test, runner all-targets check, fmt и diff прошли; см. [отчёт](../reports/STAGE_4_S4-21D_2026-11-07.md), [manifest](../reports/data/stage4/s4-21d-runner.json), [методику](../testing/MACHINE.md#s4-21d-runner-completion-and-cancellation). `--cancel-after` — детерминированный источник STOP; подключение VMHost/OS signals относится к S4-41.

### S4-22. Virtual clock и TSC

- [ ] Принято после ревью.
- **Где:** `od-machine`, TSC instruction/MSR semantics `od-isa`, machine contract.
- **Сделать:** один источник virtual time, conversion/overflow policy, pause/resume, HLT progression и выбранное отношение к host monotonic time. RDTSC/RDTSCP/related MSR/CPUID предоставлять только в принятом профиле.
- **Проверка:** монотонность, переход через большие значения, pause, delayed host scheduling, HLT и STOP; одинаковые scheduled events в deterministic test mode.
- **Готово:** Linux-visible frequency/time fields согласованы; unsupported deadline/feature не объявлен. Временная модель не зависит от числа случайных host вызовов.

#### S4-22a. Caller-driven virtual clock и bounded RDTSC

- [x] Ограниченный S4-22a принят после независимого ревью.
- **Где:** `crates/od-machine/src/virtual_clock.rs`, `diagnostic_vcpu.rs` и узкий `od-isa` execution entry.
- **Границы:** clock начинается с `now_ns=0`, имеет ненулевой фиксированный `tsc_hz` и продвигается только явным `advance_time_ns` между вызовами в Runnable. TSC вычисляется как `floor(now_ns * tsc_hz / 1e9)` с checked overflow и atomic failure. Только bare Long64 RDTSC использует clock и существующие TSD/CPL checks. Нет host-time coupling, auto-advance, RDTSCP, MSR/CPUID advertisement, HLT, IRQ, deadlines или reset.
- **Проверка:** clock rounding/large/overflow/split equivalence; реальный RDTSC через MachineBus; маска flags, EDX:EAX/RIP, split budgets, pause/STOP/zero budget, TSD/CPL/fetch failure и invalid defined mask.
- **Готово:** bounded deterministic clock/RDTSC boundary реализованы; общий S4-22 остаётся открытым для согласования частоты Linux, host monotonic time и будущей progression для HLT/events.

#### S4-22b. Часы PagedVcpuSession и UART RX

- [x] Ограниченный S4-22b принят после независимого `gpt-6.1-sol/high` ревью без замечаний; родительские S4-21 и S4-22 остаются открытыми.
- **Контракт:** `PagedVcpuSession::new` принимает явную ненулевую `tsc_hz` и владеет одним `VirtualClock`. `dispatch_rx` использует только `clock.now_ns()`, а абсолютные RX deadlines задаются в этом же домене. Выполнение, polling и число квантов часы не продвигают.
- **Lifecycle:** явное время разрешено при host Active, включая guest HLT; продвижение не будит гостя и не разрешает RX dispatch в HLT. Paused/Stopped/Terminal отвергают продвижение. Reset сохраняет clock, частоту и текущее время, очищая RX очередь как прежде; arithmetic overflow не меняет часы.
- **Проверки и границы:** `od-machine` unit tests (123), Stage 4 runner CLI (8), artifact build CLI (1), runner all-targets check, fmt и diff прошли; детали в [отчёте](../reports/STAGE_4_S4-22B_2026-11-07.md), [manifest](../reports/data/stage4/s4-22b-paged-clock.json) и [методике](../testing/MACHINE.md#s4-22b-paged-session-clock-and-uart-rx). Частота runner — диагностическая; guest CPUID/MSR, host monotonic coupling, HLT wakeup, IRQ и полный parent acceptance не заявлены.

### S4-23. Очередь событий и deadlines

- [ ] Принято после ревью.
- **Где:** `od-machine`, system tests.
- **Сделать:** простая bounded очередь для одного CPU, порядок одинаковых deadlines, cancel/reschedule, поколение reset. Пробуждение HLT и ожидание host реализовать без бесконечного polling.
- **Проверка:** одинаковые deadlines, cancel уже готового события, repeated reset, событие при pause/STOP, timer storm. Переполнение — явный результат согласно контракту.
- **Готово:** событие не теряется и не применяется к новой машине после reset; выбор структуры подтверждать замерами при оптимизации.

#### S4-23a. Bounded очередь одноразовых deadlines

- [x] Ограниченный S4-23a принят после независимого ревью.
- **Где:** `crates/od-machine/src/event_queue.rs` и public export crate.
- **Границы:** fixed-capacity queue хранит абсолютные `u64` deadlines в домене `VirtualClock`; `schedule`, `cancel`, `reschedule`, `next_deadline` и одноэлементный `pop_due` не двигают часы и не dispatch-ят CPU/device events. Порядок — deadline, затем insertion sequence; reschedule получает новую позицию среди равных deadlines.
- **Проверка:** equal/boundary/past deadlines, cancel ready event, reschedule order, duplicate/full/not-found atomicity, `u64` extremes, sequence rebase и VirtualClock readiness.
- **Готово:** bounded storage и deterministic extraction реализованы; полный S4-23 остаётся открытым для dispatch, reset/generation, HLT wakeup, IRQ и host wait.

#### S4-23b. Reset и поколения очереди

- [x] Ограниченный S4-23b принят после независимого ревью.
- **Где:** `crates/od-machine/src/event_queue.rs`, публичный API и `event_queue_api`.
- **Границы:** непрозрачный `EventGeneration` начинается с `0`; все операции очереди проверяют ожидаемое поколение до поиска/проверок ёмкости/мутаций. Reset атомарно очищает pending entries и sequence, сохраняет capacity и clock и публикует checked next generation. `GenerationExhausted` и stale operations не меняют состояние. Извлечённое событие несёт поколение для последующей проверки. Сохраняются абсолютные deadlines и порядок `(deadline, insertion sequence)`.
- **Проверка:** 122 теста `od-machine` (81 unit + 41 integration), package/workspace checks, fmt, layout, diff check и Graphify; подробности в [отчёте S4-23b](../reports/STAGE_4_S4-23B_2026-11-05.md) и [JSON evidence](../reports/data/stage4/s4-23b-event-generation.json).
- **Готово:** защита очереди от stale operations после reset принята. Полный S4-23 остаётся открытым для dispatch, IRQ и timers, HLT wakeup, host wait и конкурентного владения.

#### S4-23c. Bounded доставка UART RX по deadlines

- [x] Ограниченный S4-23c принят после независимого ревью; S4-23 и S4-24 остаются открытыми.
- **Где:** `crates/od-machine/src/uart_rx_events.rs`, `event_queue.rs`, публичный API и `uart_rx_events` integration tests.
- **Границы:** `UartRxEventPump` хранит bounded RX payload для одного UART. Capacity фиксирована; generation, deadlines и deterministic order принадлежат `EventQueue`. Dispatch соблюдает порядок generation → STOP → event budget → readiness. Успешная инъекция удаляет ровно одно событие и payload; `ReceiverBusy` сохраняет earliest event, payload, deadline и ordering position. Reset/stale generation invalidates pending work. Pump не двигает clock, не меняет CPU и не пробуждает HLT.
- **Проверка:** 129 тестов `od-machine` (81 unit + 48 integration), включая `uart_rx_events` 7/7; package/workspace checks, fmt, layout и diff check. Подробности в [отчёте S4-23c](../reports/STAGE_4_S4-23C_2026-11-05.md) и [JSON evidence](../reports/data/stage4/s4-23c-uart-rx-events.json).
- **Готово:** bounded event-to-UART-RX delivery принята. Не заявляются IRQ/APIC, baud timing, HLT wakeup, host wait или Linux boot. S4-23, S4-24 и Stage 4 остаются открытыми.

#### S4-23d. Generation-aware RX control через PagedVcpuSession

- [x] Ограниченный S4-23d принят после независимого `gpt-6.1-sol/high` ревью без замечаний. S4-23 остаётся открытым.
- **Зависимости среза:** принятые S4-21c, S4-22b и S4-23a/b/c. Это ограничение не снимает родительскую зависимость S4-23 → S4-22 → S4-21.
- **Контракт:** session публикует текущее непрозрачное RX generation, ближайший абсолютный deadline, а schedule/cancel/reschedule требуют expected generation. Устаревшее поколение отклоняется прежде lifecycle admission и queue errors. Мутации разрешены только при Active и не-Halted; deadline inspection read-only доступна во всех состояниях.
- **Проверка и границы:** 11 `paged_vcpu_session` unit tests прошли; `od-machine` all-targets check, fmt и diff check прошли. Смотрите [отчёт](../reports/STAGE_4_S4-23D_2026-11-07.md), [manifest](../reports/data/stage4/s4-23d-paged-rx-control.json), [методику](../testing/MACHINE.md#s4-23d-generation-aware-paged-rx-control). Срез не будит HLT, не реализует IRQ или host wait.

#### S4-23e. Атомарная замена однослотового deadline

- [x] Ограниченный prerequisite-срез принят после независимого `gpt-6.1-sol/high` ревью без замечаний; S4-23 и S4-28 остаются открытыми.
- **Контракт:** crate-private `EventQueue::replace_single` работает только с очередью capacity 1. Под expected generation она атомарно заменяет единственный deadline либо отменяет его и выдаёт следующее поколение; generation отличает уже извлечённое событие от нового arm того же слота.
- **Отказы:** stale generation проверяется до capacity; неподходящая ёмкость и исчерпание generation оставляют очередь, sequence и deadline без изменений. Публичный API очереди не изменён.
- **Проверки и границы:** 16 focused `event_queue` unit tests, `cargo check -p od-machine --all-targets --offline`, fmt и diff check прошли. Подробности в [отчёте](../reports/STAGE_4_S4-23E_2026-11-07.md) и [manifest](../reports/data/stage4/s4-23e-single-slot-replacement.json). Это основа для APIC timer, не его реализация; нет APIC registers, clock ownership, IRQ dispatch, HLT wakeup или Linux acceptance.

### S4-24. UART 16550A: сначала polling, затем IRQ

- [ ] Полная задача открыта. Bounded slices S4-24a, S4-24b и S4-24c приняты; IRQ и guest-visible acceptance остаются обязательными для закрытия.
- **Где:** устройство в `od-machine`, runner serial sink, system tests.
- **Сделать:** COM1 `0x3F8`, IRQ4 по `VIRTUAL_BOARD.md`; реализовать нужные registers, DLAB, line status, FIFO и interrupt conditions согласно точному профилю. Начать с реального polling TX для первого probe; IRQ подключить после S4-27.
- **Проверка:** divisor/register selection, TX/RX/status transitions, interrupt enable/ack semantics, повторное чтение side effects, raw serial bytes без добавленных host success markers.
- **Готово:** guest IN/OUT печатает известные bytes; отдельный IRQ case получает и обслуживает UART interrupt. Имитировать UART прямым вызовом host log из guest loader нельзя.

#### S4-24a. Bounded polling TX через PIO bus

- [x] Ограниченный polling TX срез принят после независимого ревью `gpt-6.1-sol/high`; родительский S4-24 остаётся открытым.
- **Evidence:** [отчёт S4-24a](../reports/STAGE_4_S4-24A_2026-11-05.md) и [JSON evidence](../reports/data/stage4/s4-24a-uart.json).
- **Граница:** `Uart16550PollingTx` подключается к COM1 `0x3F8–0x3FF` через byte-only PIO. `Uart16550Handle` управляет тем же `Arc<Mutex<UartState>>`, что и bus-owned device; передача продвигается только явными transfer/complete вызовами. Реализованы THR/DLL/DLM/LCR/LSR/SCR, ограничение 8N1 и ненулевого divisor, IER=0, FIFO disabled, IIR no-interrupt, holding/shift stages и reset.
- **Проверка:** 7 device unit tests, 2 PIO integration tests и 4 существующих `MachineBus` tests; package check, formatting, layout и diff checks прошли.
- **Не заявлено:** baud timing, RX/FIFO, IRQ4, guest CPU `IN/OUT`, Linux serial compatibility, boot или scheduler. Полная 16550A совместимость не заявлена.

#### S4-24b. Bounded CPL0 byte PIO instruction path

- [x] Ограниченный CPU-to-PIO срез принят после независимого ревью `gpt-6.1-sol/high`; родительский S4-24 остаётся открытым.
- **Evidence:** [отчёт S4-24b](../reports/STAGE_4_S4-24B_2026-11-05.md) и [JSON evidence](../reports/data/stage4/s4-24b-pio.json).
- **Граница:** `DiagnosticCpl0PioU8V1` исполняет только bare `EC`/`EE` по одному за вызов, Long64 CPL0, CR0/CR4 `None`; fetch идёт через flat identity RAM. Успех вызывает одну byte-PIO операцию, одну retirement и `RIP + 1`. COM1 поддерживает single-byte RX injection/read-clear и bounded TX.
- **Проверка:** `od-isa` — 2014 unit и 7 integration tests; полный `od-machine` — 98 tests; package/workspace checks, fmt и diff checks прошли.
- **Не заявлено:** IOPL/TSS, другие widths, immediate I/O, REP/string I/O, IRQ/APIC, Linux serial compatibility или Linux boot.

#### S4-24c. Bounded paged CPL0 byte PIO

- [x] Ограниченный paged CPU-to-PIO срез принят после независимого ревью `gpt-6.1-sol/high`; родительский S4-24 остаётся открытым.
- **Evidence:** [отчёт S4-24c](../reports/STAGE_4_S4-24C_2026-11-05.md) и [JSON evidence](../reports/data/stage4/s4-24c-paged-pio.json).
- **Граница:** CPU `step_paged_pio_u8` принимает already-selected `GuestMemory` и проверяет только CPU-side admission/CR0/CR4. Machine `step_paged_pio_u8` валидирует CR3/EFER через существующую `Long64ProbeConfig` для четырёх уровней и 4 KiB и передаёт реальный page-aware memory adapter; fetch использует `Explicit`. Машинный сценарий выполняет `MOV RAX,[RBX] → OUT DX,AL` без host mutation и переносит definedness. При PIO failure уже выполненные A/D callbacks сохраняются. Проверены UART output bytes.
- **Проверка:** `cargo test -p od-isa --all-targets --locked --offline` — 2014 unit, 6 paged и 7 legacy тестов; `cargo test -p od-machine --all-targets --locked --offline` — 105 тестов, включая 7 paged. Workspace check, fmt, layout, diff, JSON/link validation и independent review прошли.
- **Не заявлено:** mixed runner, exception delivery/CR2, IOPL/TSS, IRQ/APIC, scheduler, baud timing, JIT или Linux. Полный Stage 4 и parent S4-24 остаются открытыми.

#### S4-24d. Guest UART RX→IN→OUT echo profile

- [x] Ограниченный S4-24d принят после независимого `gpt-6.1-sol/high` ревью без замечаний P0–P3; S4-24 остаётся открытым.
- **Зависимости среза:** S4-24a/b/c, S4-21c/d, S4-22b, S4-23c/d и S4-32a. Это не закрывает parent acceptance S4-21/22/23; IRQ-вариант S4-24 по-прежнему зависит от S4-27.
- **Контракт:** отдельный `Stage4UartEchoProbeV1` raw image содержит только `IN AL,DX → OUT DX,AL → HLT` (`EC EE F4`) и использует прежний board/page-table handoff. Runner через RX event queue подаёт независимый byte; expected byte применяется только в host completion predicate. Success требует HLT, ровно 3 retirement, RIP `0x400003`, RAX равный ожиданию и реальный UART TX из одного совпадающего byte. Пустой RBR проходит штатным PIO error.
- **Проверки и границы:** 13 `PagedVcpuSession` tests, 9 runner CLI tests, 2 builder→CLI tests, runner all-targets check, fmt и diff прошли. `check_layout.py` сообщает уже известный root `smoke/` (`ISSUE-056`). См. [отчёт](../reports/STAGE_4_S4-24D_2026-11-07.md), [manifest](../reports/data/stage4/s4-24d-uart-echo.json), [методику](../testing/MACHINE.md#s4-24d-guest-uart-rxinout-echo). Гостевой LSR polling loop, IRQ/APIC, HLT wakeup, baud timing и общий scheduler не реализованы.

#### S4-24e. UART IRQ4 → IOAPIC → LAPIC → гостевой обработчик (ограниченный срез)

- [x] Ограниченный срез принят после независимого ревью (Claude Opus, high); родительский S4-24 остаётся открытым. Отчёт: [STAGE_4_S4-24E_2026-11-08.md](../reports/STAGE_4_S4-24E_2026-11-08.md).
- **Где:** `od-machine` (UART, связь с IOAPIC/LAPIC-владельцем сессии), kernel fixture `tests/fixtures/guest/stage4/`, runner, `MACHINE_CONTRACT.md`.
- **Сделать:** UART: IER (RDA и THRE), IIR/снятие прерывания (чтение RBR, IIR или запись THR); OUT2 в MCR — гейт обосновать по драйверу 8250 Linux или исключить с записью причины. Выход UART соединяется edge-сигналом с IOAPIC INTIN4 (`set_intin4`) внутри владельца сессии, детерминированно, без фонового scheduler. Гостевой профиль `Stage4UartIrqProbeV1`: LIDT, снятие маски IOAPIC записью в MMIO, STI;HLT. RX-событие из очереди → IRQ → обработчик читает IIR и RBR, пишет OUT, делает LAPIC EOI, IRETQ, HLT.
- **Проверка:** негативные кейсы: IER=0 (нет IRQ); маска IOAPIC (edge при маске теряется, снятие маски не доставляет); повторный IRQ без EOI; injected regression, которая обязана ломать проверку. Builder → runner: guest state и trace событий (IRR/ISR/EOI), бюджеты и STOP.
- **Граница:** не входит FIFO, baud timing, прерывания от LSR-ошибок и modem status, level route, JIT, совместимость с Linux 8250, production scheduler, IRQ из CPL3.
- **Готово:** guest обрабатывает RX-IRQ через IOAPIC и LAPIC; вся цепочка проверена трейсом; ограниченный срез не выдаётся за полный S4-24.

### S4-25. Local APIC одного CPU

- [x] Принято после ревью (закрыт срезами S4-25a–f; Claude Opus high). Не закрыто и вынесено: доставка с LINT/NMI/ExtINT; ESR-источники; logical destination в IOAPIC/ICR при logical mode ядра (риск S4-26/S4-38); сверка CPUID с отсутствием PMU/TM/CMCI (S4-38); проверка на настоящем Linux (S4-38); x2APIC и multi-CPU.
- **Где:** `od-machine`, CPU APIC base/MSR effects в `od-isa`.
- **Сделать:** выбранный xAPIC register model, enable/base, APIC ID, masks/priority, IRR/ISR/TMR, EOI и требуемые LVT. x2APIC и multi-CPU startup не заявлять. Необходимые self-directed IPI/NMI пути определить по manifest.
- **Проверка:** reset, masked vector, priority, pending/in-service transitions, spurious path, EOI. Ошибочные register accesses и reserved bits следуют принятой спецификации.
- **Готово:** APIC state реально влияет на pending/deliverable interrupt, а не существует только как читаемый массив registers.

#### S4-25a. Fixed-edge Local APIC register core

- [x] Ограниченный S4-25a принят после независимого `gpt-6.1-sol/high` ревью; полный S4-25 остаётся открытым.
- **Зависимости среза:** S4-05, S4-19 и необходимые event/reset части S4-23 (принятые S4-23a–d). Незакрытый parent S4-23 не считается выполненным.
- **Контракт:** отдельный `LocalApic` реализует ограниченный one-CPU xAPIC профиль как `MachineBus` device по фиксированному MMIO base `0xFEE00000`, APIC ID 0. Поддержаны ID, TPR/PPR, SVR, EOI, ISR/TMR/IRR и fixed-edge запросы векторов `16..=255`; explicit acknowledge перемещает один eligible vector из IRR в ISR. Pending inspection не потребляет событие. Источник поведения: Intel SDM release 093, Vol. 3A, Chapter 13, §§13.4.1–13.4.3, 13.4.6–13.4.8, 13.8.3–13.8.5.
- **Граница:** Version register намеренно недоступен; LVT, APIC-base MSR/relocation, x2APIC, IOAPIC/level routing, timer, CPU interrupt delivery, handler execution, IF/shadow/IDT и HLT wakeup отсутствуют. TMR показывает edge; ошибочные/неподдержанные доступы возвращают `DeviceError` и не меняют состояние. Тест подключает device через существующий `MachineBus`; это не доказывает default board wiring или guest Linux path.
- **Проверки:** 2 focused `local_apic` tests, `od-machine --all-targets` check, fmt и diff прошли. См. [отчёт](../reports/STAGE_4_S4-25A_2026-11-07.md), [manifest](../reports/data/stage4/s4-25a-local-apic-core.json) и [методику](../testing/MACHINE.md#s4-25a-fixed-edge-local-apic-register-core).

#### S4-25b. CPU-owned IA32_APIC_BASE enable

- [x] Ограниченный S4-25b принят после независимого `gpt-6.1-sol/high` ревью; S4-25 и Stage 4 остаются открытыми.
- **Контракт:** opt-in `ApicBaseCpu` исполняет только bare `RDMSR`/`WRMSR` к `IA32_APIC_BASE` (`0x1B`) в Long64/CPL0. Reset value `0xFEE00900` (fixed base `0xFEE00000`, BSP и global enable); global disable — `0xFEE00100`. Запись возвращает typed effect; `step_apic_base_msr` применяет его к xAPIC до возврата. Повторная запись того же значения не сбрасывает APIC. Global disable сбрасывает APIC state и блокирует MMIO/acknowledge; после re-enable требуется SVR software enable. Источник: Intel SDM release 093, Vol. 3A, §§13.4.3–13.4.5, 13.4.7.1–2.
- **Граница:** base relocation, BSP changes, x2APIC, общий MSR dispatcher, `PagedVcpuSession` integration, CPU interrupt delivery, IF/shadow/IDT/handler и HLT wakeup не заявлены. Прямые вызовы `ApicBaseCpu::step` обязаны применить effect до следующей guest/device operation; `step_apic_base_msr` делает это синхронно для фиксированного Local APIC.
- **Проверки:** 3 focused `od-isa` теста, 4 focused `local_apic` теста, all-targets check двух пакетов, fmt и diff прошли. Независимый Sol/high reviewer закрыл два P2 замечания. См. [отчёт](../reports/STAGE_4_S4-25B_2026-11-07.md), [manifest](../reports/data/stage4/s4-25b-apic-base.json) и [методику](../testing/MACHINE.md#s4-25b-cpu-owned-ia32_apic_base-enable).

#### S4-25c. Fixed self-IPI через xAPIC ICR

- [x] Ограниченный S4-25c принят после независимого `gpt-6.1-sol/high` ревью без замечаний; S4-25 и Stage 4 остаются открытыми.
- **Контракт:** `ICR_HIGH` (`0x310`) сохраняет только destination byte и не отправляет IPI. Поддержан только aligned Dword `ICR_LOW` (`0x300`) с Fixed delivery, Self shorthand, edge trigger, asserted level и вектором `16..=255`; destination mode игнорируется. Read-only Delivery Status при записи игнорируется и после синхронной внутренней доставки читается как Idle. Запись low атомарно сохраняет writable fields и под тем же lock ставит fixed edge в IRR; это не означает CPU handler acceptance. Повторная команда создаёт запрос заново, а coalescing определяется IRR. Основа: Intel SDM release 093, Vol. 3A, §§13.6.1–13.6.2.3, Table 13-3, §§13.8.3–13.8.5.
- **Граница:** no-shorthand/external destinations, lowest-priority, NMI/SMI/INIT/SIPI, level delivery, ESR, CPU handler/IDT/IF, HLT wakeup, IOAPIC и Linux path не реализованы. Unsupported/reserved writes возвращают локальный `DeviceError` без изменения состояния; это не гостевой `#GP`.
- **Проверки:** 6 focused `local_apic` tests, `od-machine --all-targets` check, fmt и diff прошли; независимое Sol/high ревью без замечаний. См. [отчёт](../reports/STAGE_4_S4-25C_2026-11-07.md), [manifest](../reports/data/stage4/s4-25c-xapic-self-ipi.json) и [методику](../testing/MACHINE.md#s4-25c-xapic-fixed-self-ipi).

#### S4-25d. Ограниченный CR8 ↔ xAPIC TPR

- [x] Ограниченный срез принят после независимого Sol/high ревью.
- **Контракт:** отдельный Long64 профиль исполняет только `44 0F 20 C0` и `44 0F 22 C0`, при работающем CPU, clear shadow и включённом global/software APIC. Чтение возвращает `TPR[7:4]`; запись класса `0..15` устанавливает `TPR = class << 4`. CPL вне 0 и зарезервированные биты записи вызывают `#GP(0)`, точные LOCK-формы — `#UD`. Остальные состояния и формы закрыты. Типизированный эффект применяется адаптером до возврата; вызывающая сторона обязана сериализовать все операции APIC на всё время вызова, включая fetch. Архитектурная сериализация инструкцией не заявлена. Основа: Intel SDM 093, Vol. 3A §§13.8.6–13.8.6.1 и Vol. 2B MOV to/from control registers.
- **Проверки:** 5 focused `od-isa` tests, 3 focused `od-machine` tests, package all-targets check, fmt/diff и Graphify прошли. Sol/high подтвердил закрытие P2 canonical48 и P3 ISR-bank замечаний. См. [отчёт](../reports/STAGE_4_S4-25D_2026-11-07.md), [manifest](../reports/data/stage4/s4-25d-cr8-tpr.json) и [методику](../testing/MACHINE.md#s4-25d-bounded-cr8--xapic-tpr).
- **Граница:** общее исполнение MOV CR8, CPU interrupt delivery, IDT/IF, HLT wakeup, JIT и Linux acceptance остаются вне среза. S4-25 и этап 4 открыты.

#### S4-25e. Local APIC: spurious, LVT masks, reset matrix, приоритет и EOI (ограниченный срез закрытия)

- [x] Ограниченный срез принят после независимого ревью (Claude Opus, high) с условием: родитель S4-25 не закрыт, открыт S4-25f.
- **Где:** `od-machine` (`local_apic.rs`), контракт в `MACHINE_CONTRACT.md`.
- **Сделать:** spurious-путь: вектор из SVR при удалении из IRR до принятия; маска LVT (bit 16) для всех LVT-записей, влияющая на доставку; матрица значений после reset (SVR, LVT, TPR, IRR/ISR/TMR); приоритет по ISR/TPR и EOI, снимающий наивысший ISR; ошибочные обращения (reserved bits, неподдержанные смещения, ширина) по принятой спецификации xAPIC.
- **Проверка:** reset-матрица по регистрам; masked LVT не доставляет и не теряет по контракту; spurious при гонке удаления из IRR; EOI снимает наивысший ISR; ошибочные обращения не меняют состояние. Тесты в `--lib local_apic`.
- **Граница:** x2APIC, multi-CPU startup, NMI-источники сверх manifest — не входят. Срез не закрывает родителя S4-25: LINT0/LINT1/Error LVT, Version и ESR выносятся в S4-25f.
- **Готово:** APIC state влияет на pending/deliverable interrupt; reset-матрица и spurious зафиксированы тестами.

#### S4-25f. Local APIC для Linux probe: Version, LINT0/LINT1, Error LVT и ESR (ограниченный срез)

- [x] Принято после ревью (Claude Opus, high). DFR/LDR не сверены с Linux 7.0 — см. отчёт.
- **Где:** `od-machine` (`local_apic.rs`), контракт в `MACHINE_CONTRACT.md`.
- **Сделать:** APIC Version (0x30): integrated, Max LVT = 3 (Timer, LINT0, LINT1, Error); LINT0 и LINT1 LVT — хранение, маска (bit 16), reset-значение masked; Error LVT и ESR — write-then-read по принятой спецификации xAPIC, ESR очищается записью. Perf, Thermal и CMCI не добавлять, если CPUID не объявляет PMU, TM и CMCI; записать это условие. Проверить, нужны ли DFR/LDR для выбранного ядра.
- **Проверка:** чтение Version при reset; LINT0/LINT1/Error reset-матрица с masked; запись в Error LVT и ESR (write-then-read); запись в ESR не меняет IRR/ISR; обращения к Perf/Thermal/CMCI дают UnknownOffset и закреплены тестом. Тесты в `--lib local_apic`.
- **Граница:** доставка с LINT-пинов, NMI, x2APIC, multi-CPU не входят. Linux probe APIC на полном пути — S4-38.
- **Готово:** Linux-подобный probe APIC (чтение Version, запись LVT/ESR) не получает DeviceError; родитель S4-25 закрывается после этого среза.

### S4-26. IOAPIC и маршруты внешних IRQ

- [x] Принято после ревью (закрыт срезами S4-26a, S4-26b, S4-26c; Claude Opus high). Оговорки и риски для S4-29/S4-38: active-low не моделируется (SCI GSI 9 — только под маской, unmask отказывает); нет EOI-регистра 0x40 и регистра 0x03; logical destination, lowest priority и маршрутизация на входах кроме INTIN4 не поддерживаются; раскладка регистров не сверена с текстом 82093AA.
- **Где:** `od-machine`, board manifest.
- **Сделать:** register select/window, redirection table, mask, vector, destination, polarity/trigger mode и remote-IRR для используемых level routes. Поддержанные routes имеют один источник описания.
- **Проверка:** masked edge, unmask, level assert/deassert, повторный level после EOI, недопустимый destination/vector; гостевые config writes меняют поведение.
- **Готово:** один внешний device IRQ доходит до Local APIC и не теряется/не дублируется при предусмотренных mask/EOI transitions.

#### S4-26a. Fixed-edge route IOAPIC к Local APIC

- [x] Ограниченный S4-26a принят после независимого `gpt-6.1-sol/high` ревью; parent S4-26 и Stage 4 остаются открытыми.
- **Зависимости среза:** S4-02, S4-05, S4-25a и S4-25b. Принятая часть S4-25 достаточна для fixed-edge доставки в IRR и тестов priority/acknowledge/EOI; S4-25c/d дают дополнительную регрессию Local APIC.
- **Контракт:** отдельный opt-in `IoApic` в `od-machine` занимает MMIO base `0xFEC00000`. Поддержан единственный `INTIN4 → physical APIC ID 0` маршрут: IOREGSEL/IOWIN находятся по offsets `0x00`/`0x10`, redirection entry 4 — selector `0x18`/`0x19`. Low/high Dword доступны раздельно; route поддерживает Fixed, active-high, edge-triggered, vector `0x10..=0xFE` и mask. После reset маршрут замаскирован. Version register не публикуется для частичной таблицы. Read-only Delivery Status/Remote IRR биты в low Dword читаются нулём и игнорируются при записи.
- **Переходы:** только low→high на незамаскированной линии запрашивает вектор у существующего Local APIC; edge при mask теряется, снятие mask и high→high сами по себе не доставляют IRQ. Повторный одинаковый vector coalesces через существующий IRR. Если Local APIC отклоняет edge, `set_intin4` возвращает ошибку и сохраняет прежнее состояние линии для повтора вызова. Владелец обязан сериализовать IOAPIC, Local APIC, reset и enable operations.
- **Источник:** Intel 82093AA I/O APIC, document 290566-001 (preliminary), §§3.1–3.2.4: [datasheet](https://intel-vintage-developer.eu5.org/DESIGN/PCISETS/DATASHTS/29056601.PDF). Он обосновывает выбранный профиль, не обещает совместимость со всеми IOAPIC.
- **Проверки:** 4 focused IOAPIC tests, 9 Local APIC regression tests, `od-machine --all-targets`, fmt/diff и Graphify прошли; независимый Sol/high review без оставшихся замечаний. См. [отчёт](../reports/STAGE_4_S4-26A_2026-11-07.md), [manifest](../reports/data/stage4/s4-26a-ioapic-fixed-edge.json) и [методику](../testing/MACHINE.md#s4-26a-fixed-edge-ioapic-route).
- **Граница:** доступны только один edge route и выбранные регистры. Другие redirection entries, IOAPIC Version/ID/ARB, level trigger/Remote IRR delivery, logical destinations, APIC bus, CPU acceptance/handler/IDT/IF, HLT wakeup, default board wiring и Linux discovery не реализованы. `DeviceError` не является гостевым exception.

#### S4-26b. Один level route с Remote IRR и атомарным EOI

- [x] Ограниченный S4-26b принят после исправления замечания P2 и повторного независимого `gpt-6.1-sol/high` ревью без замечаний; parent S4-26 и Stage 4 остаются открытыми.
- **Контракт:** сохраняется тот же единственный INTIN4 → physical APIC ID 0 маршрут. Опциональный active-high Fixed level режим выставляет Local APIC IRR и TMR при принятии запроса; Remote IRR отражает ожидающий EOI запрос. Уровень остаётся связан с запросом через acknowledge/ISR и снимается только matching EOI; если линия всё ещё активна и route размаскирован, новый запрос принимается атомарно в той же операции. Acceptance требует global и SVR software enable; TPR влияет на eligibility, но не отменяет acceptance. Выбранный level vector зарезервирован от edge/self-IPI использования даже для masked/inactive route.
- **Атомарность и ограничения:** IOAPIC и Local APIC используют единый узкий owner state/lock; нет callback или вложенных locks. Если активный unmasked level assertion или unmask должен немедленно принять запрос, операция при выключенном Local APIC отклоняется до изменения линии/configuration; masked assertion может обновить уровень без доставки. Global и SVR software disable vetoed без мутации при установленном Remote IRR. Конфликтующие vector/trigger изменения отклоняются, пока линия активна, Remote IRR установлен или связанный vector pending/in-service. Существующие edge→edge перенастройки сохраняют прежнее поведение. Независимые reset сохраняют соседний контроллер; при установленном Remote IRR оба veto без мутации. `reset_pair` явно сбрасывает пару. Операции устройства и полный CPU helper вызов сериализуются вызывающей стороной; mutex не удерживается во время guest fetch.
- **CPU effect:** `IA32_APIC_BASE` helper выполняет инструкцию на кандидате CPU ровно один раз и публикует CPU state только после успешного APIC enable transition. Отклонение host-side transition при Remote IRR не является гостевым `#GP` и не retire инструкцию; уже выполненные fetch/page-walk effects не откатываются.
- **Источники:** Intel SDM 253668-093US, September 2026, Vol. 3A §§13.8.4–13.8.5 ([PDF](https://cdrdv2.intel.com/v1/dl/getContent/671190)); Intel 82093AA I/O APIC, document 290566-001 preliminary, §3.2.4 ([datasheet](https://intel-vintage-developer.eu5.org/DESIGN/PCISETS/DATASHTS/29056601.PDF)).
- **Проверки:** 11 IOAPIC tests (включая level retrigger, priority/EOI, edge compatibility, reserved vector, software-enable failure и независимые/парный reset), 10 Local APIC regression tests, `cargo check -p od-machine --all-targets --offline`, fmt/diff и повторное Sol/high review прошли. Результаты Graphify и layout check записаны в [отчёте](../reports/STAGE_4_S4-26B_2026-11-07.md) и [manifest](../reports/data/stage4/s4-26b-ioapic-level.json); методика — [S4-26b](../testing/MACHINE.md#s4-26b-fixed-level-ioapic-route).
- **Граница:** только одна physical route, Fixed delivery, active-high и выбранные Dword registers; нет guest CPU handler/IDT/IF delivery, APIC bus, board wiring, Linux discovery, прочих IOAPIC entries, level-полярности, logical destinations или полного interrupt-controller reset semantics.

- **Требование из ревью S4-31f:** MADT объявляет IOAPIC с ID = 1 и 24 входами (константы прошивки). Будущий регистр ID модели должен отдавать 1, Version — MRE = 23; иначе `mp_register_ioapic` в Linux получит другой диапазон GSI или ошибку шины. Проверить в S4-38.
#### S4-26c. IOAPIC ID/Version, окно регистров и недопустимые dest/vector (ограниченный срез закрытия)

- [x] Принято после ревью (Claude Opus, high; два раунда). Оговорки: active-low SCI на GSI 9 принимается только под маской (риск S4-29/S4-38); EOI 0x40 и регистр 0x03 не моделируются; logical destination и маршрутизация на входах кроме INTIN4 не поддерживаются; раскладка не сверена с текстом 82093AA.
- **Где:** `od-machine` (`io_apic.rs`, `apic_pair.rs`), контракт в `MACHINE_CONTRACT.md`.
- **Сделать:** IOAPIC ID (регистр 0x00) = 1, как объявляет MADT (S4-31f); Version (0x01) = 0x00170020 — версия 0x20, MRE = 23 (24 входа), по принятой спецификации IOAPIC; Arbitration ID (0x02) — хранение по принятой политике. Окно IOREGSEL/IOWIN (`0xFEC00000`): неизвестный select — отказ без изменения состояния. Недопустимые dest/vector (vector < 16 для fixed, зарезервированные delivery mode) — отказ без изменения. Guest config writes через окно меняют поведение (маска, vector, trigger).
- **Проверка:** чтение ID/Version через окно; MRE = 23 совпадает с MADT (тест на согласованность); запись в IOREGSEL/IOWIN меняет redirection entry; неизвестный select и недопустимые dest/vector не меняют состояние; injected regression (ломаешь MRE или ID) — тест падает. Тесты в `--lib io_apic`.
- **Граница:** logical destination routing не входит — записать как риск при logical flat mode ядра (см. S4-25f «Открыто»); multiple IOAPIC, PCI MSI не входят. Родитель S4-26 закрывается после этого среза при условии, что S4-26a–b и этот срез покрывают «Сделать».
- **Готово:** IOAPIC отвечает ID/Version, согласованными с MADT; окно регистров — единственный путь конфигурации из гостя; ошибочные обращения не меняют состояние.

### S4-27. Соединить pending IRQ с CPU delivery

- [x] Принято после ревью (закрыт срезами S4-27a–g, включая f1 и f2; Claude Opus high). Оговорки: доставка только плоская identity Long64/CPL0 без paged-входа и IRETQ; Закрыто после S4-27: paged одного вектора (S4-27h), paged STI/HLT и wake (S4-27i4a), flat pump с границей, STI/HLT, таймером, очередью и переводом UART-probe (S4-27i1–i3, i5). Остаётся: paged-таймер (S4-37a), run_boot/плата/время по инструкциям/PIT/PIC/INTIN2 и ISSUE-067 (S4-37a), общий paged-интерпретатор и Linux EOI movl disp32 (S4-38), вложенные #PF/#NP/#DF, CPL3, IST≠0, TLB, JIT (S4-33), MOV SS (этап 2); тень MOV SS и NMI не моделируются (MOV SS не реализован; риск для S4-37/S4-38); fault при доставке — только отказ host-адаптера, без вложенных исключений и #DF (S4-33). Оговорки «Граница» подсрезов сохраняются.
- **Где:** `od-machine`, delivery/interruptibility `od-isa`, JIT exits `od-jit`.
- **Сделать:** выбирать deliverable event на корректной границе instructions/blocks; учитывать IF, STI/MOV SS shadow и нужные NMI rules, frame и EOI. Согласовать HLT wakeup и очередь событий.
- **Проверка:** IRQ перед/после CLI/STI, shadow, HLT при разных условиях, pending during handler, fault при delivery; interpreter и поддержанный JIT дают одинаковую архитектурную последовательность.
- **Готово:** handler действительно выполняется и возвращает управление; queued IRQ не теряется на JIT exit, fallback или pause.

#### S4-27a. Caller-driven APIC IRQ entry и попытка первой handler instruction

- [x] Ограниченный S4-27a принят после независимого `gpt-6.1-sol/high` ревью без замечаний; parent S4-27 и Stage 4 остаются открытыми.
- **Реализация:** `DiagnosticVcpuSession::deliver_interrupt_and_step` выбирает один APIC-eligible vector, передаёт его существующему `od-isa::deliver_maskable_interrupt` через `IdentityMemoryAdapter`, после успешного entry подтверждает именно этот vector IRR→ISR и запускает handler с instruction budget 1. Это не гарантирует retirement: fetch/исполнение может fault, остановиться на unsupported opcode или halt; успешный CPU/APIC entry остаётся committed, возвращается обычный handler `DiagnosticRunResult`. CPU state и acknowledgement публикуются только после успешной IDT entry.
- **Предусловия:** caller подтверждает завершённую instruction boundary, отсутствие pending synchronous exception/debug/NMI или другого interrupt inhibition, private synchronous RAM и сериализует полный вызов с APIC/IOAPIC operations. Поддержан только diagnostic flat identity Long64/CPL0 Running state с IF=1 и явным clear STI shadow, same-CPL interrupt gate и IST=0; остальные ограничения задаёт ISA delivery contract.
- **Отказы:** IF=0, blocking STI shadow или отсутствие eligible vector оставляют CPU/APIC без изменений; TPR-blocked request остаётся в IRR. Unsupported entry возвращается без acknowledge. Delivery abort может сохранить уже записанный frame prefix; сессия становится terminal и автоматически повторить доставку нельзя.
- **Проверки:** 9 `diagnostic_vcpu` unit tests, 10 Local APIC tests, 7 public API tests и ISA stack-store failure regression; `od-machine --all-targets`, fmt/diff и независимое Sol/high review прошли. Evidence в [отчёте](../reports/STAGE_4_S4-27A_2026-11-07.md), [manifest](../reports/data/stage4/s4-27a-apic-cpu-entry.json) и [методике](../testing/MACHINE.md#s4-27a-caller-driven-apic-interrupt-entry).
- **Граница:** нет автоматического polling/run loop, доставки на произвольной instruction/block boundary, paged CPU delivery, MOV SS/NMI, HLT wakeup, IRET/guest EOI path, JIT exits или Linux/board acceptance. Полный критерий S4-27 остаётся открытым.

#### S4-27b. Гостевой EOI, same-CPL IRETQ и resume

- [x] Ограниченный S4-27b принят после независимого `gpt-6.1-sol/high` code review и повторного docs review без оставшихся замечаний; parent S4-27 и Stage 4 остаются открытыми.
- **Реализация:** после S4-27a handler выполняет guest `MOV dword [LAPIC_EOI], EAX=0` через существующий `IdentityMemoryAdapter → MachineBus` MMIO path. Явный `run_same_cpl0_iretq` вызывает существующий `od-isa` `SameCpl0ValidFrameV1` с budget 1; после Retired обычный session `run(1)` возобновляет instruction по сохранённому RIP.
- **Подтверждено:** один level request входит через пять qword frame, handler EOI очищает Remote IRR, IRETQ восстанавливает RIP/RSP/CS/SS/RFLAGS, затем реально исполняется прерванная RAM instruction. Проверены все 16 младших выравниваний исходного RSP; при удерживаемом INTIN4 EOI атомарно принимает ровно один новый запрос в IRR, а его CPU-доставка ждёт следующую явную caller boundary.
- **Отказы:** после уже состоявшегося guest EOI fetch/frame/CS/SS read abort в IRETQ не публикует candidate CPU, не повторяет EOI и переводит сессию в terminal. Предыдущий EOI и его effects не откатываются.
- **Проверки:** 12 `diagnostic_vcpu` tests, 8 API tests, 10 Local APIC tests, 25 `od-isa::iretq` tests; package all-targets, fmt/diff и независимый Sol/high review прошли. Graphify и layout details — в [отчёте](../reports/STAGE_4_S4-27B_2026-11-07.md) и [manifest](../reports/data/stage4/s4-27b-guest-eoi-iretq.json); методика — [S4-27b](../testing/MACHINE.md#s4-27b-guest-eoi-same-cpl-iretq-and-resume).
- **Граница:** caller-driven same-CPL0 diagnostic flat identity path; нет автоматического interrupt polling, paged IRETQ, межуровневого return, полного IF/STI/MOV SS/NMI/HLT scheduler, JIT или Linux acceptance. Полный S4-27 остаётся открытым.

#### S4-27c. Caller-driven STI-profile APIC quantum

- [x] Ограниченный S4-27c принят после независимого `gpt-6.1-sol/high` review без оставшихся замечаний; parent S4-27 и Stage 4 остаются открытыми.
- **Реализация:** `DiagnosticVcpuSession::run_sti_apic_quantum` на каждой явной caller boundary проверяет один Local APIC vector через `eligible()`, затем либо выполняет существующий interrupt entry → точный acknowledge → одну попытку handler instruction, либо один quantum существующего `NonPviLong64V1` STI-профиля с instruction budget 1. Предусловия нового API проверяются до APIC и guest memory; обычный `run()` не меняет режим исполнения.
- **Поддержанный STI язык:** профиль начинает admission заново на каждом вызове; при `IF=0, Clear` допустимы CLI/STI, при `IF=1, Clear` — STI, при `IF=1, Blocking` — NOP/STI/CLI. Поэтому `CLI; STI; NOP` между вызовами удерживает запрос pending до снятия shadow. После IRQ entry обработчик ограничен тем же профилем; например, NOP как первая handler instruction даёт terminal Unsupported, но entry и APIC acknowledge остаются committed.
- **Предусловия и границы:** caller сериализует CPU/APIC/IOAPIC, удостоверяет completed boundary, отсутствие synchronous/debug/NMI/MOV-SS inhibition, private synchronous RAM, один vCPU и ограничения ISA entry. Требуются Runnable Long64/CPL0 Running state, явный STI shadow, `rflags_defined = INTEGER_FLAGS`, отсутствие configured TSC/RDTSC (ослаблен clock gate в S4-27i2, см. контракт, абзац «Ослабление gate S4-27c по clock (S4-27i2)») и `CR0/CR3/CR4`; `Blocking` при IF=0 отклоняется без мутаций. REP iteration-budget exit делает границу непригодной для APIC entry, IRETQ и STI quantum до последующего retirement обычным `run()`.
- **Проверки:** 18 `diagnostic_vcpu` tests, `od-machine --all-targets`, fmt/diff и независимое Sol/high review прошли. Graphify и layout evidence записаны в [отчёте](../reports/STAGE_4_S4-27C_2026-11-07.md) и [manifest](../reports/data/stage4/s4-27c-sti-apic-quantum.json); методика — [S4-27c](../testing/MACHINE.md#s4-27c-caller-driven-sti-profile-apic-quantum).
- **Граница:** только exact STI profile и caller-driven flat identity diagnostic path; нет произвольного instruction stream, MOV SS/NMI delivery, REP continuation внутри нового quantum, HLT wakeup, TSC/RDTSC, JIT, автоматического возврата/EOI или Linux acceptance. Полный S4-27 остаётся открытым.

#### S4-27d. Explicit HLT wait and maskable APIC wake

- [x] Ограниченный S4-27d принят после исправления P2 по STOP и повторного независимого `gpt-6.1-sol/high` review без оставшихся замечаний; parent S4-27 и Stage 4 остаются открытыми.
- **Реализация:** `DiagnosticVcpuSession::into_halt_wait` исполняет ровно bare HLT через существующий ISA `step_with_halt_profile` и передаёт владение отдельному `DiagnosticHaltWait`. `poll_apic` не исполняет guest instruction: при IF=0/нет eligible vector остаётся в ожидании; при deliverable vector использует candidate CPU, строит frame, подтверждает точный APIC vector и только после этого позволяет получить обратно session через `into_running`.
- **Resume:** успешный wake сохраняет post-HLT RIP в same-CPL interrupt frame; caller отдельно выполняет handler, гостевой EOI и `run_same_cpl0_iretq`, после чего обычный `run()` продолжает post-HLT instruction. Повторный poll после entry идемпотентно сообщает тот же vector и не подтверждает второй запрос.
- **STOP/отказы:** STOP в Waiting предотвращает wake; STOP после уже committed entry сохраняет CPU/frame/acknowledge, ставит session Stopped и запрещает `into_running`. Ошибка HLT возвращает исходную session без CPU commit; delivery abort может оставить frame-write prefix, но CPU остаётся Halted, APIC request pending, ожидание terminal и повтор запрещён. REP partial, отсутствующий shadow, неверная definedness, TSC или CR0/3/4 отклоняются до fetch.
- **Проверки:** 22 `diagnostic_vcpu` tests, `od-machine --all-targets`, fmt/diff и повторное независимое Sol/high review прошли. Graphify и layout evidence записаны в [отчёте](../reports/STAGE_4_S4-27D_2026-11-07.md) и [manifest](../reports/data/stage4/s4-27d-hlt-apic-wake.json); методика — [S4-27d](../testing/MACHINE.md#s4-27d-explicit-hlt-wait-and-maskable-apic-wake).
- **Граница:** только synchronous caller polling и maskable APIC wake на flat identity path. Нет asynchronous arrival, NMI/debug/MOV SS wake, timer/deadline source, general scheduler, JIT, автоматического handler execution или Linux acceptance. Полный S4-27 остаётся открытым.

#### S4-27e. Ordinary APIC quantum and pending IRQ during handler

- [x] Ограниченный S4-27e принят после исправления P2 по STOP и повторного независимого `gpt-6.1-sol/high` review без оставшихся замечаний; parent S4-27 и Stage 4 остаются открытыми.
- **Реализация:** новый `run_apic_quantum` на caller-certified completed boundary сначала проверяет STOP, затем при IF=1 выбирает один `eligible()` vector и выполняет candidate entry → acknowledge того же vector → обычный `run(1)` для handler. При IF=0 или отсутствии eligible vector исполняется обычный quantum. В отличие от 27c, используется полный обычный interpreter, включая текущую `rflags_defined` и configured TSC/RDTSC; требуется явный Clear STI shadow. Старые public result enums не расширяются.
- **Pending during handler:** тестовый IRQ B приходит как self-IPI во время handler IRQ A. При IF=0 запрос остаётся в IRR; после guest EOI и явного IRETQ следующий `run_apic_quantum` доставляет B до fetch возобновляемой инструкции. После второго EOI/IRETQ основной поток возобновляется.
- **STOP и отказ:** pre-entry STOP переводит session в `Stopped` до APIC/memory callbacks и сохраняет pending request; повторный quantum отклонён. `OutsideProfile` и delivery abort не переходят к обычному исполнению; abort terminal, сохраняет pending IRQ и не допускает retry. Fault/unsupported/stopped handler не откатывают успешный entry. REP partial invalidates boundary до последующего retirement.
- **Проверки:** 29 `diagnostic_vcpu` tests, `od-machine --all-targets`, fmt/diff и повторное независимое Sol/high review прошли. Graphify и layout evidence записаны в [отчёте](../reports/STAGE_4_S4-27E_2026-11-07.md) и [manifest](../reports/data/stage4/s4-27e-apic-quantum.json); методика — [S4-27e](../testing/MACHINE.md#s4-27e-ordinary-apic-quantum).
- **Граница:** обычная caller-driven interpreter session с fixed one-instruction budget; нет автоматической доставки в многоинструкционном run/JIT block, общего event queue arbitration, MOV SS/NMI, HLT wake integration, автоматического EOI/IRET или Linux acceptance. Полный S4-27 остаётся открытым.

#### S4-27f1. Bounded JIT event handoff

- [x] Ограниченный S4-27f1 принят после исправления трёх P2 и независимого повторного `gpt-6.1-sol/high` review без замечаний; parent S4-27 и Stage 4 остаются открытыми.
- **Реализация:** opt-in `DiagnosticVcpuSession::run_jit_nop` принимает стабильный `code_start`, полный ожидаемый byte range, budget, dispatch control, контролируемый executor и caller-owned event flag; exact `S0_09NopAdmission` строится внутри из bytes. Каждый вызов проверяет диапазон без overflow в одной guest 4 KiB page, его полное RAM backing и все байты; RIP должен соответствовать допустимой границе в этом полном range. Dispatcher использует event-aware execution: событие в publication window возвращает `Yield(EventPending)` до входа в native code, а executor yield корректно становится event exit без искусственного progress/counters.
- **Resume и ошибки:** event flag не сбрасывается автоматически. Владелец события явно очищает его; повторный запуск передаёт исходный полный диапазон, поэтому ветви назад и уже пройденный prefix остаются доступны. Отказы профиля, диапазона, байтов или admission происходят до executor и сохраняют сессию. Ошибка dispatch может следовать за вызовом executor: candidate CPU не публикуется, исходный CPU сохраняется, сессия становится `Stopped`, event flag остаётся у caller.
- **Проверки:** 32 `diagnostic_vcpu` tests, 412 `od-jit` unit tests, 2 ARM64 host integration tests с настоящим `PublishedBlockExecutor`, package all-target check для `od-machine`, `od-jit`, `od-host`, fmt и diff check прошли. Layout check сообщил только существующий unmapped root `smoke/` (ISSUE-056). Graphify и evidence — в [отчёте](../reports/STAGE_4_S4-27F1_2026-11-07.md), [manifest](../reports/data/stage4/s4-27f1-jit-event-handoff.json) и [методике](../testing/MACHINE.md#s4-27f1-bounded-jit-event-handoff).
- **Граница:** проверен bounded NOP fallback и host-native block handoff на ARM64, но asynchronous interruption внутри native block не реализована. Этот срез не связывает JIT exit с APIC entry/handler/EOI/IRETQ, production scheduler, event queue, paged execution или Linux acceptance; это остаётся частью открытого parent S4-27.

#### S4-27f2. Сквозной JIT event, APIC entry и resume

- [x] Ограниченный S4-27f2 принят после независимого `gpt-6.1-sol/high` review без замечаний; parent S4-27 и Stage 4 остаются открытыми.
- **Реализация:** host integration scenario использует настоящий `PublishedBlockExecutor` и существующие public machine API. После native prefix dispatcher останавливается на `EventPending` перед NOP. Caller затем подаёт один APIC request, проверяет `eligible()` и вызывает `run_apic_quantum`; frame сохраняет точный JIT boundary, а entry подтверждает вектор ровно один раз. Ограниченный guest handler читает сохранённый RIP и ISR bit, пишет Dword zero в LAPIC EOI через MMIO, проверяет очищение ISR и исполняет same-CPL `IRETQ`.
- **Resume:** после IRETQ caller явно очищает своё event flag и повторяет `run_jit_nop` с тем же полным code range и оставшимся budget. Проверено, что prefix не исполняется снова, один NOP retired интерпретатором, а native suffix завершается отдельно; JIT и handler counters проверяются раздельно. Pause/resume сохраняет CPU boundary и event до обработки.
- **Проверки:** целевой `diagnostic_jit_event` integration binary — 3 passed (включая два S4-27f1 cases), `cargo check -p od-host --all-targets --offline`, fmt/diff и независимое Sol/high review прошли. Graphify и evidence — в [отчёте](../reports/STAGE_4_S4-27F2_2026-11-07.md), [manifest](../reports/data/stage4/s4-27f2-jit-apic-resume.json) и [методике](../testing/MACHINE.md#s4-27f2-jitapiceoiiretq-resume).
- **Граница:** только один caller-driven vector на завершённой границе, без async arrival внутри native block. Event flag — отдельное уведомление; APIC acknowledge происходит только после проверки eligible и успешного entry. Нет общего scheduler/event queue, автоматического maskable polling, MOV SS/NMI, paged delivery, error recovery после guest EOI или Linux acceptance; полный S4-27 остаётся открытым.

#### S4-27g. Due EventQueue event through APIC with interpreter parity

- [x] Ограниченный S4-27g принят после независимого `gpt-6.1-sol/high` review без замечаний; parent S4-27 и Stage 4 остаются открытыми.
- **Реализация:** парный ARM64 host test заранее ставит due и future `ScheduledEvent` в существующий `EventQueue`. Синхронный caller bridge после завершения native JIT block снимает один due event, запрашивает фиксированный APIC vector и выставляет notification. Тот же deadline/queue/APIC schedule повторяется для interpreter prefix; обе ветки затем проходят одинаковые APIC entry, guest EOI, IRETQ и продолжение.
- **Доказательства:** проверены `EventPending` до NOP с точным RIP/budget/counters, единичный queue pop, IRR→ISR acknowledgement, сохранённый frame, ISR до/после guest EOI, IRETQ, сохранение notification и future event через pause/resume и одинаковые финальный `LongModeState`/observable order для JIT и interpreter.
- **Граница:** EventQueue→APIC bridge существует только как caller orchestration в тестовом сценарии; production queue routing, HLT wake из очереди, general scheduler, async producers и MOV SS/NMI не добавлены. Parent S4-27 остаётся открытым. См. [отчёт](../reports/STAGE_4_S4-27G_2026-11-07.md), [manifest](../reports/data/stage4/s4-27g-queued-irq-jit-parity.json) и [методику](../testing/MACHINE.md#s4-27g-queued-irq-and-jitinterpreter-parity).

#### S4-27h. Paged IRQ entry и IRETQ (разовый handler)

- [x] Принято после ревью (Claude Opus, high). Оговорки: только один профиль (вектор 0x41, Long64/CPL0, 4 KiB, gate 0xE, IST=0, без смены CPL); STI, HLT, `run_apic_quantum` и JIT остаются flat-only до S4-27i; отказы доставки — модель, аппаратный исход — последовательный #PF/#NP (вложенная доставка — S4-33); тип памяти APIC (PCD/PWT/PAT) не моделируется; физический адрес EOI в трассе косвенный (открыто: писать в событие Eoi фактический адрес MMIO).
- **Контракт:** APIC vector доставляется на paged Long64/CPL0 профиле (4 KiB, S4-32): стек, IDT, GDT и handler лежат на отдельных paged-страницах. Используются принятые S4-27a (APIC entry и первая handler instruction), S4-27b (EOI, same-CPL IRETQ) и примитивы paged delivery и IRETQ из S4-19b/S4-19e1. Handler — одна инструкция EOI и IRETQ на paged коде. Это снимает оговорку «только плоская identity» в S4-27 (решение 2 консультации).
- **Проверка:** builder → runner (или machine test): вектор при IF=1 доставляется, handler выполняется с paged кода, EOI, IRETQ возвращает в main. Guest state (RIP, RSP, RFLAGS, CS), IRR/ISR, EOI, A/D страниц стека, IDT и handler. Injected regression (каждая различима в evidence): IDT-вход not-present → отказ на delivery, handler не выполняется; страница стека read-only → отказ на delivery (архитектурная причина, без вложенного #PF); handler без EOI → ISR не сбрасывается, второй доставки нет.
- **Граница:** без вложенных исключений и #DF (S4-33); без MOV SS и NMI; без CPL3 delivery; без TLB-on; без event pump (S4-27i); без HLT wake из paged профиля; это не закрывает S4-27 и S4-33.
- **Готово:** кейс выполняется как guest fact; вход, возврат и инъекции различимы в evidence; S4-27 остаётся открытым до S4-27i, если не решено иначе.
- **Решения консультации Opus (S4-27h, 2026-11-09):**
  - *Q1 доставка:* новая функция `od_isa::deliver_paged_maskable_interrupt`; тело — общая приватная функция с flat-фронтом `deliver_maskable_interrupt` (без изменения его поведения и тестов; DRY по AGENTS.md — общая семантика). Предусловия как у `step_paged_iretq`: CR0 и CR4 = BASE, cr3 < 2^36, EFER = 0xd00. Запись стека — `AccessOrigin::ImplicitStack` (flat пишет `Explicit`; расхождение записать). Обёртка в машине `deliver_paged_external_interrupt` поверх `PagedExceptionMemory` с `descriptor_accessed_target: None` и без цепочки #DF; отчёт: delivery, secondary_page_fault, access_error, ad_updates, committed_writes.
  - *Q2 EOI:* вариант (a) — новый exact-профиль `89 03` (dword-store в LAPIC 0xFEE000B0) через `PagedMemoryAdapter` с MMIO-диспатчем; по образцу `PagedMovCrossPageDwordReadLong64V1`, qword-профиль не наследуется. Evidence: перевод линейного адреса в физический 0xFEE000B0, D на PTE страницы LAPIC, dword в одной странице. Тип памяти (PCD/PWT/PAT) не моделируется — записать.
  - *Q3 отказ:* внешнее прерывание — класс benign; #DF не возникает. Gate с P=0 (это и есть «IDT-вход not-present», не PTE страницы IDT) → `OutsideProfile(InvalidGate)` до commit: CPU не меняется, IRR сохранён, ISR пуст; A-биты от чтения IDT внести в evidence. Стек read-only → `InterruptDeliveryAborted{StackWrite{OldSs}, Architectural #PF}`, сессия terminal, вектор не подтверждён, CR2 не меняется. Отчёт пометить: модель; аппаратный исход — последовательный #PF/#NP; вложенная доставка — S4-33. Слово «host error» в плане заменить на «архитектурная причина».
  - *Порядок шагов:* (1) main на paged-коде на завершённой границе, IF=1 из начального состояния (не STI); (2) запрос в APIC и `eligible()`; (3) доставка на `candidate`, затем подтверждение IRR→ISR (`apic.acknowledge_vector(vector, || state = candidate)`); (4) handler: `89 03` — EOI сбрасывает ISR, проверить ISR=0 и IRR=0; (5) `48 CF` через `step_paged_iretq`; (6) ещё один шаг main. EOI строго до IRETQ. Инъекция «без EOI»: после IRETQ повторно запросить тот же вектор, показать `eligible()==None` и обычный шаг.
  - *Формулировка снятия оговорки в S4-27:* «Оговорка "только плоская identity" снята для одного профиля: одиночный внешний APIC-вектор, Long64/CPL0, 4 KiB (S4-32), CR0/CR4=BASE, EFER=0xD00, gate 0xE с IST=0 без смены CPL, handler `89 03`+`48 CF` (S4-27h). STI/HLT/`run_apic_quantum`/JIT остаются flat-only до S4-27i; вложенные события при доставке — S4-33.»
  - *Вне среза:* вложенные #PF/#NP/#DF и shutdown; IST≠0; CPL3 (TSS RSP0); запись A-бита дескриптора; paged STI и HLT; event pump; `run_apic_quantum` на paged; JIT; несколько векторов; 2 MiB, TLB, SMAP/SMEP, x2APIC; тип памяти; MOV SS и NMI.
  - *Риски:* Linux EOI через `movl` с disp32 и CR0=0x80050033 шире BASE — exact-профили не масштабируются, нужен S4-27i; модель «ack после входа» расходится с INTA на железе (решить в S4-33); S4-34 (KVM) — инъекция RO-стека даст shutdown/reset, пометить «model-only, не сравнимо».

#### S4-27i. Event pump и общий interpreter-цикл (владелец — pump)

- [x] Принято после ревью по подсрезам i1–i5 (см. подсрезы; S4-27i4b принят 2026-11-09).
- **Владелец (решение консультации Opus, S4-27i, 2026-11-09):** новый тип `InterruptPump` в `crates/od-machine/src/interrupt_pump.rs` поверх `DiagnosticVcpuSession` и `LocalApicHandle`; внутри `Cpu { Running(session) | Waiting(DiagnosticHaltWait) }`. Цикл не встраивается в session (она остаётся набором caller-driven шагов), в `run_boot` и в paged-профиль. Образец цикла — `Stage4UartIrqProbe::run/step_waiting/step_running` (S4-24e), который обобщается в pump.
- **Порядок границы:** один раз на границу инструкции перед fetch: STOP → dispatch due-событий (таймер, очередь) → если Runnable, `completed_boundary`, IF=1, shadow Clear и есть `eligible()`, доставка через `run_apic_quantum` → иначе одна инструкция. HLT не идёт в `run(1)` (там terminal `Unsupported`: у обычного интерпретатора нет HLT; см. отчёт S4-27i1): HLT → `into_halt_wait`; STI → `run_sti_apic_quantum`; IRETQ → `run_same_cpl0_iretq`. Эффекты шага видит проверка следующей границы; отдельной проверки после инструкции нет.
- **Подсрезы:**
  - **i1** — flat pump с проверкой на границе, STI/HLT/wake и IRQ от caller, без clock. Классы принятых примитивов S4-27c/d/e, новой ISA-семантики нет.
  - **i2** — owner-driven таймер: dispatch на границе и в Waiting (время до ближайшего из срока таймера APIC и очереди), `LocalApicHandle::next_timer_deadline()` (read-only), ослабление gate `run_sti_apic_quantum` по clock (меняет принятый контракт S4-27c; RDTSC внутри STI-кванта не принимается). Решение консультации по умолчанию: да. Время на инструкцию 0 нс.
  - **i3** — очередь IRQ-событий в pump: `peek_due` → `request_fixed_edge` → `pop_due` только после успеха; схлопывание due-событий с одним вектором в один IRR-фронт (явно в evidence); generation после reset; только фиксированный вектор; producer остаётся caller-driven.
  - **i4a** — paged STI/HLT-шаги и paged wake из HaltWait через `deliver_paged_external_interrupt` (S4-27h), caller-driven; один вектор, CR0/CR4=BASE.
  - **i4b** — pump поверх paged-бэкенда; трейт бэкенда вводится только здесь.
  - **i5** — перевод `Stage4UartIrqProbe` (S4-24e) на pump (DRY; его тесты служат регрессией; решение по умолчанию: да, отдельным подсрезом) и закрытие оговорок S4-27 в документации.
- **Граница всего S4-27i:** не входят run_boot и плата, Linux-профиль, PIT/PIC/INTIN2, порт-тик и синхронизация часов по инструкциям (S4-37a); вложенные #PF/#NP/#DF (S4-33); MOV SS и NMI; JIT внутри pump (`not_supported`, отмечается в S4-33); общий paged-интерпретатор и декод Linux EOI `movl` disp32 (S4-38); CPL3, IST≠0, TLB, 2 MiB, x2APIC, TSC-deadline; несколько vCPU и потоки.
- **Готово:** каждый подсрез выполняется как guest fact с инъекцией; принятые S4-27a–h и S4-28 не меняются (кроме явно записанного gate в i2); S4-27 остаётся открытым до i5.

#### S4-27i1. Flat event pump: граница инструкции, STI/HLT/wake и IRQ от caller

- [x] Принято после ревью (Claude Opus, high). Оговорки: частичный REP задерживает прерывание до конца инструкции (ISSUE-068); CLI при IF=1 вне shadow не поддержан (terminal Unsupported, ISSUE-069); HLT через `run(1)` даёт terminal `Unsupported`, а не `Halted` (поправка формулировки плана); S4-27 и S4-27i остаются открытыми.
- **Контракт:** `InterruptPump` (flat Long64/CPL0) владеет session или HaltWait и APIC. `run(budget, stop)` идёт по границам: STOP → проверка APIC (IF, shadow, completed boundary, `eligible`) → доставка или одна инструкция. HLT → `into_halt_wait`/`poll_apic`; STI → `run_sti_apic_quantum`; IRETQ → `run_same_cpl0_iretq`; остальное → `run_apic_quantum`. Выходы: `IdleHalt`, `Budget`, `Stopped`, `Terminal`, `Rejected`. Принятые API не меняются.
- **Проверка:** builder → runner или machine test. Сценарии: pending до `STI; HLT` → доставка после HLT, RIP после HLT; pending при IF=1 перед HLT → RIP указывает на HLT; `CLI; HLT` → `IdleHalt`; self-IPI B во время обработчика A → B доставлен после EOI и IRETQ, в правильном порядке. Evidence: RIP/RSP/RFLAGS, IRR/ISR, трасса событий. Инъекции: пропуск проверки в shadow; HLT в `run(1)` даёт terminal; без EOI второй доставки нет.
- **Граница:** без clock, таймера, очереди, paged и JIT; запросы только от caller или guest MMIO; S4-24e не мигрируется (это i5). Частичный REP: после незавершённого REP граница не завершена, задержка прерывания — записать как отклонение.
- **Готово:** кейсы выполняются как guest fact, инъекции различимы, тесты S4-27a–h и S4-28 не изменены; DRY-долг перед UART-probe записан; S4-27 остаётся открытым.

#### S4-27i2. Owner-driven таймер в pump: dispatch и idle advance

- [x] Принято после ревью (Claude Opus, high). Оговорки: запрет RDTSC в STI-кванте — проверка байтов по RIP, а не поле rdtsc_controls (язык профиля и так не содержит TSC; избыточный отказ при eligible-векторе записан); dispatch на границе в Running не наблюдаем (инструкции 0 нс; наблюдаемо в S4-37a); idle advance сжигает бюджет при заблокированном векторе таймера в ISR (ISSUE-070); отказы TimerDispatch/TimerAdvance/TimerNoProgress не покрыты тестами; DRY-долг загрузчика с S4-27i1.
- **Контракт:** pump на каждой границе и в Waiting вызывает `dispatch_timer` (owner-driven; S4-28b отложил это сюда). В Waiting время продвигается до ближайшего из двух сроков: таймера APIC и очереди (очередь — в i3, здесь срок очереди отсутствует). Нет срока — `IdleHalt`. Добавляется read-only `LocalApicHandle::next_timer_deadline()`. Ослабляется gate `run_sti_apic_quantum` по clock: clock разрешён, RDTSC внутри STI-кванта по-прежнему не принимается. Это меняет принятый контракт S4-27c; нужен отдельный тест. Решение консультации Opus по умолчанию: да. Время на инструкцию — 0 нс (как в S4-29d; синхронизация по инструкциям — S4-37a).
- **Проверка:** periodic таймер; guest `STI; loop: HLT; JMP`; обработчик считает в RAM; после N пробуждений счётчик = N, время = N·период. Injection: без dispatch таймера pump даёт `IdleHalt` при нулевом счётчике (кейс падает). Evidence: счётчик, число входов, время, IRR/ISR, трасса.
- **Граница:** очередь (i3); paged-таймер (S4-37a); run_boot и плата (S4-37a); время по инструкциям (S4-37a); TSC-deadline и x2APIC не входят.
- **Готово:** кейс выполняется как guest fact; инъекция различима; тесты S4-27i1 и S4-27a–h не меняются, кроме явно записанного gate.
- **Уточнение (ревью Opus):** при IF=0 в Waiting время не продвигается и результат `IdleHalt` даже при взведённом таймере; Dispatch на границе в Running в этом срезе не наблюдаем (наблюдаемо в S4-37a). Открыто: ISSUE-070.

#### S4-27i3. Очередь IRQ-событий в pump и арбитраж простоя

- [x] Принято после ревью (Claude Opus, high). Оговорки: арбитраж простоя закрывает ISSUE-070 по коду (TPR-вариант и вариант с более приоритетным вектором другого класса тестом не покрыты); отказ request_fixed_edge воспроизводится только отключением APIC caller'ом; асимметрия таймер/очередь на границе (таймер в idle advance, очередь на следующей границе); S4-27i4a/b, S4-27i5 и S4-37a — открыты.
- **Контракт:** pump владеет одной `EventQueue` с IRQ-событиями (ключ → фиксированный вектор). События ставит caller (producer остаётся caller-driven); снимает владелец на границе: `peek_due` → `request_fixed_edge` → `pop_due` только после успеха, чтобы при ошибке событие сохранилось. Несколько due-событий с одним вектором схлопываются в один IRR-фронт (явно в evidence). Generation: после reset старая generation отвергается. Только фиксированный вектор; без реестра устройств, без обобщённой выборки линий IOAPIC и без асинхронных producer.
- **Арбитраж простоя (ISSUE-070):** в `idle_advance` возвращать `IdleHalt`, если вектор таймера уже в IRR и не eligible (заблокирован ISR/TPR), вместо продвижения часов и сжигания бюджета. Срок очереди учитывается в продвижении времени в Waiting вместе со сроком таймера.
- **Проверка:** guest fact или machine-тест: due-событие и future-событие; пробуждение HLT из очереди; pause/resume сохраняет future-событие; после reset старая generation отвергается; схлопывание двух due-событий с одним вектором в один вход; инъекция «без pop после успеха» (событие теряется или дублируется — кейс падает); сценарий ISSUE-070 (вектор таймера в ISR без EOI) даёт IdleHalt, а не Budget.
- **Граница:** только фиксированный вектор; без paged (i4); без run_boot и платы (S4-37a); без арбитража приоритетов нескольких разных векторов сверх уже принятого `eligible()`; producer синхронный (caller).
- **Готово:** кейсы как guest fact или machine-тесты, инъекции различимы; ISSUE-070 закрыта или явно переписана с новым статусом; тесты S4-27i1/i2 не меняются.

#### S4-27i4a. Paged STI/HLT-шаги и paged wake из HaltWait

- [x] Принято после ревью (Claude Opus, high). Оговорки: boundary-сценарий покрыт только machine-тестом (runner запускает wake); имена флагов инъекций отличаются от S4-27h (`--inject-idt-not-present`, `--inject-stack-read-only`); отказы входа терминальны (модель; на железе — последовательный #PF/#NP, вложенная доставка — S4-33); тип памяти LAPIC не моделируется; общий интерфейс бэкенда — в S4-27i4b.
- **Контракт:** на paged Long64/CPL0 профиле (4 KiB, CR0/CR4=BASE, EFER=0xd00) появляются шаги STI и HLT, нужные pump, и paged wake из HaltWait через `deliver_paged_external_interrupt` (S4-27h). Один вектор, caller-driven: запрос ставит caller, пробуждение и вход выполняет принятый путь S4-27h, возврат — `step_paged_iretq`. Образ — тот же, что у S4-27h, но с `STI; HLT`. Допуски (как у S4-27h) и граница по CR — из принятых предусловий S4-19/S4-27h.
- **Проверка:** образ S4-27h с `STI; HLT`; A/D страниц (main, handler, IDT, GDT, стек, LAPIC); wake из HLT и вход на границе, RIP кадра, IRR/ISR, RSP/RFLAGS; инъекция «IDT P=0» даёт `OutsideProfile(InvalidGate)` до коммита и HaltWait остаётся (IRR сохранён, ISR пуст); инъекция «страница стека RO» даёт архитектурный отказ доставки (как в S4-27h). Все исходы различимы по (stop, steps, IRR/ISR, RSP, RIP, A/D).
- **Граница:** один вектор; только CR0/CR4=BASE; без paged-таймера (S4-37a); без pump поверх paged-бэкенда (i4b); без вложенных событий (S4-33); CPL3 и IST≠0 не входят; тип памяти LAPIC не моделируется.
- **Готово:** кейс выполняется как guest fact; инъекции различимы; тесты S4-27h и S4-27i1–i3 не меняются, кроме явно записанного.

#### S4-27i4b. Pump поверх paged-бэкенда

- [x] Принято после ревью (Claude Opus, high; см. отчёт). Оговорки: paged-таймер — владелец S4-37a; счётчик на paged — число возвратов handler; сравнение с flat в entry-only режиме; SVR повторяется owner'ом после глобального отключения APIC; инжекции SkipShadowCheck/HltAsOrdinary/NoTimerDispatch на paged не имеют аналога; хост и REP только flat.
- **Контракт:** трейт бэкенда вводится только здесь: `InterruptPump` работает поверх paged-памяти (S4-27i4a) и flat (S4-27i1–i3) через один интерфейс. Проверка: сценарий S4-27i3 (очередь) на paged-профиле; инъекция «pop до успешного request» на paged-профиле даёт то же различие, что на flat.
- **Граница:** без вложенных событий; без run_boot (S4-37a); без JIT (`not_supported`, S4-33).
- **Граница:** paged-таймер — владелец S4-38 (решение Opus, 2026-11-09): на 4-инструкционном `PagedBackend` кейс без доставки невыполним, а для Linux этот бэкенд не подходит; `PagedBackend` держит `Option<VirtualClock>`, shared timer требует `VirtualClockHandle`.
- **Готово:** одинаковые сценарии на обоих бэкендах дают одинаковые архитектурные последовательности; S4-27 закрыт в плане после i5 (статус не меняется); paged-таймер остаётся за S4-38 (до закрытия подсрезов i5 и S4-37a S4-27i не расширяется).

#### S4-27i5. Перевод UART-probe на pump и закрытие оговорок S4-27

- [x] Принято после ревью (Claude Opus, high; см. отчёт). Оговорки: STOP финальный (принята семантика pump, S4-27i1); pump расширен аддитивными точками хоста (PumpHost, entry-only, clock_ns), выключенными по умолчанию; RX-очередь, линия INTIN4, LIDT/IN/OUT остаются в UartOwner; S4-27i остаётся открытым из-за S4-27i4b.
- **Контракт:** `Stage4UartIrqProbe` (S4-24e) переводится на `InterruptPump` (DRY; его тесты служат регрессией); закрытие оговорок S4-27 («только плоская identity», «автоматического event pump нет») в документации, если i4a/i4b и i1–i3 закрыли их. Дубль загрузчика S4-27i1/i2/i3 (DRY-долг) устраняется общим загрузчиком.
- **Проверка:** все тесты S4-24e (`stage4_uart_irq_probe`, 10) проходят без правки ожиданий; дубль загрузчика сведён к одному модулю.
- **Граница:** поведение не меняется; новых ISA-семантик нет.
- **Готово:** S4-27 закрывается (`[x]`), оговорки перенесены в S4-37a/S4-38 явно.

## Решения по консультации Opus (2026-11-09)

Принято как решения этапа по рекомендациям архитектурного советника; каждое изменение плана ниже относится к этим решениям.

1. **Event pump** — срез **S4-27h** (разовый handler с paged IRQ) и **S4-27i** (общий interpreter-цикл: проверка APIC на границе инструкции, HLT→wait, мост EventQueue→APIC, owner-driven dispatch таймеров) перед S4-33. Владелец event pump — S4-27i; S4-33 его не строит.
2. **Paged IRQ entry и IRETQ** — срез S4-27h, не S4-38. Оговорка «только плоская identity» в S4-27 снимается ссылкой на S4-27h.
3. **Linux time source** — условие «Linux-профиль имеет источник времени» переносится в S4-37a и S4-38; S4-28 закрывается по локальному критерию.
4. **Подключение APIC timer к плате и Linux-профилю** — срез **S4-37a** (Linux-профиль платы под opt-in флагом: привязка устройств, тик PM_TMR, порт-тик, LAPIC с shared timer, RDTSC как временной порт; состав и порядок — блок S4-37a). Доставка IRQ в `run_boot`, общий paged-бэкенд pump, мост PIT→INTIN2 на плате и paged APIC timer — **S4-38** (решение Opus, 2026-11-09; граница S4-27i). S4-38 зависит от S4-37a; S4-37 (loader) от S4-37a не зависит.
5. **Порт-тик** — каждое чтение временного порта (0x42, 0x61, PM_TMR) синхронизирует часы по выполненным инструкциям и продвигает их на Δ = 1 мкс (владелец clock — сессия). ISSUE-067 закрывается в S4-37a, а не в S4-29d (уточнение по ревью S4-29d, Opus): в S4-29d тик сделан только на `0x61` в профиле калибровки; тик на `0x42` и PM_TMR, применение очереди в `run_boot`/`PagedVcpuSession` и синхронизация по инструкциям переносятся в S4-37a вместе с закрытием.
6. **Gate2 PIT** — отвергать (DeviceError): загрузка канала 2 при gate2=0; чтение счётчика или OUT2 загруженного канала при gate2=0; падение gate2 1→0 во время счёта. Чтение 0x61 при незагруженном канале 2 допустимо.
7. **Порты 0x80, 0x60, 0x64** — 0x80 явный sink (запись игнорируется, чтение 0xFF); 0x60 и 0x64 остаются ловушкой (UnmappedPort). `io_delay` в cmdline не задавать (тип по умолчанию 0x80 подтвердить в config в S4-29g). Блокер S4-38 по 0x80 снят при условии sink.
8. **Путь IRQ0** — честный, без `no_timer_check`. В S4-38: `check_timer` проходит через IOAPIC pin 2 (ISO 0→2).
9. **AEOI на мастере** — принимается (ICW4=0x03, флаг хранится без эффекта, у пары нет INTR); на slave AEOI отвергается. Открытый пункт S4-29a снимается правкой S4-29a2 до S4-29f.
10. **NMI и MOV SS** — NMI неприменимо (на плате нет источника NMI). MOV SS и её тень — обязательны: задача этапа 2 и предусловие S4-37/S4-38 (Linux `startup_64` выполняет `mov %eax,%ss`).
11. **TSC-deadline** — не реализуется и не объявляется (подтверждено).
12. **TLB** — uncached-путь закрывает S4-33. S4-13 и S4-14 — минимальный TLB на трассе Linux после S4-38 (запасной вариант: если bounded boot превышает бюджет — S4-13 раньше). TLB-on переходит в S4-42/S4-44. S4-15 и S4-16 зависят от S4-12.
13. **JIT в S4-33** — только принятые подмножества; матрица «кейс × interpreter/JIT: supported или not_supported». Модификация активного блока — `not_supported`. Новой JIT-функциональности в S4-33 нет.
14. **Huge pages** — только 2 MiB, 1 GiB не объявляется. Срез S4-10c: PDPTE.PS → гостевой #PF RSVD. Кейс S4-33 = 2 MiB плюс негатив 1 GiB.
15. **Mac mini и S4-34** — только KVM-путь (если есть VT-x): fixtures как KVM-гость, reboot, GRUB и модули ядра не трогаются; APIC, IOAPIC и таймер маркируются «kvm-emulated, не hardware». Bare-metal — только по отдельному разрешению. Доступ (проверка `vmx`, `/dev/kvm`, группа `kvm`) требует вашего разрешения; без него S4-34 — `blocked`.
16. **Тик PM_TMR и RDTSC-порт (решения Opus, 2026-11-09, S4-37a)** — (а) тик PM_TMR (Δ = 1 мкс на чтение `0x608`) ставится на всех платах с `acpi_pm`, не только под Linux-флагом: ISSUE-067 — дефект платы; устройство только ставит тик в очередь, `run_boot` применяет тики лишь от гостевых чтений внутри своего запуска, очередь сбрасывается на старте `run_boot`, чтения хоста через `bus_mut()` часы не двигают; (б) CMOS UIP (~1 с виртуального времени при 1 нс на инструкцию, ~10⁹ инструкций) решается RDTSC как временным портом с Δ = 1 мкс: цикл `udelay(100)` ≈ 10⁷ инструкций; бюджет S4-38 на этот участок — не более 2·10⁷ инструкций; (в) владелец RDTSC на плате — подсрез a6 S4-37a (нужен до S4-38: `quick_pit_calibrate`, `delay_with_tsc`).

Решения, которые остаются за пользователем: доступ к Mac mini и SSH-проверка (п. 15); порядок TLB и бюджет bounded boot (п. 12) — по умолчанию как записано; согласие на пакет (п. 1–5) — по умолчанию принято.

### S4-28. APIC timer

- [x] Принято после ревью по локальному критерию (решение Opus, п. 3): Linux time source → S4-37a/S4-38.
- **Где:** `od-machine`, virtual clock/events, system tests.
- **Сделать:** нужные count/divider/LVT registers, one-shot/periodic modes и cancel/reprogram. TSC-deadline добавлять только при обязательности и полном контракте.
- **Проверка:** нулевой count, masked expiry, перепрограммирование до deadline, periodic handler+EOI, HLT wakeup, pause/resume, reset.
- **Готово:** guest получает ожидаемое число interrupts в контролируемом virtual-time тесте, а Linux-профиль имеет работоспособный источник времени и событий.

#### S4-28a1. One-shot Local APIC timer device

- [x] Ограниченный device-срез принят после исправления P2 и повторного независимого `gpt-6.1-sol/high` ревью без замечаний. S4-28, S4-27 и Stage 4 остаются открытыми.
- **Реализация:** opt-in Local APIC timer через `MachineBus`: LVT Timer, Initial/Current Count и Divide Configuration; только one-shot. Явная timer frequency отделена от TSC frequency; timer владеет отдельными caller-driven VirtualClock и capacity-one EventQueue. Expiry ставит один fixed-edge vector в IRR; masked expiry снимает deadline без IRQ.
- **Атомарность:** rearm/cancel и reset меняют поколение единственного события; stale event не может сработать после reprogram/reset. Ошибки поколения, MMIO, reset и конфликта unmasked LVT с level-route не публикуют частичное состояние. Software disable при активном deadline отклоняется; idle timer LVT маскируется, а unmask при disabled отклоняется. Divider меняется только без активного отсчёта.
- **Проверки и ограничения:** Local APIC tests 19/19, IOAPIC regression tests 11/11, `od-machine --all-targets`, fmt и diff check прошли. См. [отчёт](../reports/STAGE_4_S4-28A1_2026-11-07.md), [manifest](../reports/data/stage4/s4-28a1-apic-one-shot.json), [контракт](../specs/MACHINE_CONTRACT.md#one-shot-local-apic-timer-s4-28a1) и [методику](../testing/MACHINE.md#s4-28a1-one-shot-local-apic-timer). Не подключены vCPU session clock, periodic/TSC-deadline, production scheduler/event pump, CPU delivery, HLT wakeup или Linux timer profile.

#### S4-28a2. Единый clock session и APIC timer

- [x] Ограниченный срез принят после закрытия трёх P2 замечаний и повторного независимого `gpt-6.1-sol/high` ревью без замечаний. S4-22, S4-28, S4-27 и Stage 4 остаются открытыми.
- **Архитектура:** `DiagnosticVcpuSession` единолично получает неповторяемое capability продвижения caller-driven `VirtualClockHandle`; RDTSC, APIC timer MMIO Current Count, arm и timer dispatch читают один clock. У shared timer нет собственного advance API; standalone режим S4-28a1 сохраняется.
- **Граница:** caller явно создаёт handle и передаёт clone в `LocalApic::with_shared_timer`, затем handle передаёт в `DiagnosticVcpuSession::new_with_clock_handle`; вторая session не может захватить те же часы, даже после drop первой. Session продвигает clock только в Runnable. При due deadline dispatch в Paused/Stopped/Terminal возвращает Busy и сохраняет событие. APIC lock не удерживается во время CPU execution. Нет автоматической dispatch, CPU delivery, HLT progression/wakeup или scheduler.
- **Проверки:** `cargo test -p od-machine --lib --offline` — 190 passed; `cargo check -p od-machine --all-targets --offline`, fmt, diff check, JSON и ссылки прошли; Graphify обновлён после финального diff. Сквозной тест выполняет RDTSC до/после session advance, читает Current Count, проверяет expiry/IRR на тех же nanoseconds и отказ dispatch в Paused/Stopped; отдельный clock test проверяет эксклюзивное владение. Отчёт: [S4-28a2](../reports/STAGE_4_S4-28A2_2026-11-07.md), evidence: [manifest](../reports/data/stage4/s4-28a2-shared-clock.json).

#### S4-28a3. Shared one-shot APIC timer wakes HLT wait

- [x] Ограниченный caller-driven child принят после независимого ревью (Claude Opus, high, 2026-11-08). Непроверенные пути gate (IF=0, masked LVT, stop в Waiting, OutsideProfile) и lock-order тестом не подтверждены — вынесены в отдельный тестовый подсрез; родитель S4-28 остаётся открытым.
- **Реализация:** `DiagnosticHaltWait` переносит единственный `VirtualClockAdvancer` и разрешает явный time advance только пока `Waiting`. Владелец отдельно вызывает `LocalApicHandle::dispatch_timer`, затем существующий `poll_apic`; это даёт one-shot timer expiry → IRR → maskable HLT wake без фонового scheduler.
- **Атомарность и lifecycle:** APIC pair lock берётся перед shared clock gate; due-event consumption и IRR commit выполняются под обоими locks. Перед interrupt delivery gate закрывается и clock lock отпускается до memory callbacks; отсутствие eligible IRQ/`OutsideProfile` возвращают wait в dispatchable `Waiting`, `Entered`/STOP/Terminal закрывают gate, `into_running` открывает его. Reset инвалидирует timer event и не двигает clock. Overflow не меняет часы/TSC.
- **Проверки:** integration case связывает один clock, HLT, истечение timer, повторный due vector во время entry, Entered gate, resume и RDTSC после IRETQ. `cargo test -p od-machine --lib --offline` — 191 passed; all-targets check, fmt, diff, JSON и локальные ссылки прошли. Graphify обновить после финального diff. См. [отчёт](../reports/STAGE_4_S4-28A3_2026-11-07.md) и [manifest](../reports/data/stage4/s4-28a3-hlt-timer.json). Periodic/TSC-deadline, автоматический event pump и host scheduling остаются открытыми.

#### S4-28a4. Caller-driven periodic Local APIC timer

- [x] Ограниченный срез принят после закрытия замечаний и повторного независимого `gpt-6.1-sol/high` ревью без P0–P3. Родительские S4-28, S4-27 и Stage 4 остаются открытыми.
- **Реализация:** LVT bit 17 выбирает one-shot/periodic; periodic Current Count повторяет интервал. Ненулевой Initial Count запускает или заменяет период; ноль отменяет. Переключение режима и divider допустимы только при пустой очереди. Periodic expiry re-arms одну capacity-one очередь с сохранением исходной фазы; опоздавшие периоды объединяются в не более чем один IRR edge на dispatch, masked expiry также re-arms без IRQ.
- **Атомарность:** deadline следующего периода вычисляется от arm epoch с checked arithmetic и округлением вверх; dispatch выбирает ближайшую границу строго после текущего времени. Ошибки deadline/generation не меняют очередь или IRR. Сохраняются lock order APIC pair → shared clock, dispatch gate, caller-driven advance/dispatch и `Busy` для MMIO при необработанном due.
- **Проверки:** тестами покрыты граница и дробный период, большой time jump без catch-up burst, masked expiry, cancel/reprogram/reset и overflow/generation failure; после более ранних timer deliveries также проверяется acknowledge/EOI. Повторный expiry с вектором, всё ещё находящимся в ISR, и отдельный periodic HLT-gate сценарий не проверены; общий shared gate остаётся покрыт one-shot integration test. Результаты и эти ограничения — в [отчёте](../reports/STAGE_4_S4-28A4_2026-11-07.md), [manifest](../reports/data/stage4/s4-28a4-apic-periodic.json), [контракте](../specs/MACHINE_CONTRACT.md#periodic-apic-timer-s4-28a4) и [методике](../testing/MACHINE.md#s4-28a4-caller-driven-periodic-apic-timer). TSC-deadline, automatic scheduler/event pump, Linux profile и родительские S4-28/S4-27 остаются открытыми.

#### S4-28b. Гостевой periodic timer: N прерываний в virtual time (закрывающий срез S4-28, без Linux-профиля)

- [x] Принято после ревью (Claude Opus, high). Отчёт: docs/reports/STAGE_4_S4-28B_2026-11-09.md. Родитель S4-28 открыт: условие «Linux-профиль имеет источник времени» — решение пользователя.
- **Где:** `od-machine` (`diagnostic_vcpu.rs`, `local_apic.rs`, `apic_pair.rs`), контракт в `MACHINE_CONTRACT.md`.
- **Сделать:** integration-тест с shared clock и periodic LVT: цикл advance → `dispatch_timer` → `run_apic_quantum` → гостевой handler (счётчик в RAM, EOI записью в MMIO) → IRETQ → resume; N прерываний, счётчик = N, RIP/RAX основного потока верны. Тот же сценарий из HLT: periodic re-arm, минимум два wake подряд. Повторный expiry, пока vector в ISR: один IRR edge, доставка только после EOI; coalescing большого скачка — одно прерывание. Долг S4-28a3: пути gate (IF=0, masked LVT, STOP в Waiting, OutsideProfile) и lock-order. Pause → due → resume: прерывание ровно один раз. Решение про TSC-deadline — записать явно: не реализуется, не объявляется в CPUID.
- **Проверка:** `cargo test -p od-machine --lib --offline`, all-targets check, fmt/diff, независимое ревью (Opus, high).
- **Граница:** автоматический event pump и scheduler не входят (владелец предложен S4-33, решение пользователя); PIT/PIC/RTC и ISSUE-067 не входят (S4-29, S4-37/S4-38); подключение APIC timer к firmware board не входит; JIT async, MOV SS, NMI, paged delivery не входят.
- **Готово:** guest получает ожидаемое число прерываний периодического таймера в virtual-time тесте с handler+EOI; долг a3 закрыт. Условие «Linux-профиль имеет источник времени» остаётся в S4-28 до решения пользователя.

### S4-29. Закрыть необходимые legacy зависимости

- [ ] Локально принято после ревью либо scope исключён по source/config audit; Linux-проверка обязательна в S4-38.
- **Где:** `od-machine`, профиль/методика в docs.
- **Сделать:** по исходникам/config выбранного Linux определить необходимость PIC, PIT, RTC/CMOS и другого timer fallback. Реализовать только реально требуемый минимум, включая mask/ack и routing handoff к APIC; документировать выбранную cmdline. Для исключённого устройства сохранить source/config основание и назначить проверку этого решения в S4-38.
- **Проверка здесь:** локальные device tests и собственный guest probe подтверждают calibration primitives, mask/ack и отсутствие дублирования legacy/IOAPIC IRQ. Linux boot trace на этом шаге ещё не требуется.
- **Готово для S4-31/37:** локальный контракт проверен; нет фиктивных registers «всегда ready» и скрытых host delays. Отсутствие Linux hang и достаточность выбранного набора устройств подтверждаются позже в S4-38, до окончательной приёмки этапа.

### S4-30. Минимальная PCI/PCIe configuration

- [x] Принято после ревью (закрыт срезами S4-30a, S4-30b, S4-30c; Claude Opus, high). Закрытие: ECAM и conf1 — на шине и в карте платы под opt-in флагом; bus 0 пустой, production-устройств на этапе 4 нет, поэтому требование «production devices имеют backend» выполняется пустым множеством. Явно не сделано и не требуется этапу 4: BAR→MMIO mapping и decode-биты Command (биты только хранятся; BAR sizing проверяется на diagnostic endpoint), MCFG (S4-31), host bridge (отложен до этапа 7 по решению пользователя), MSI/MSI-X без объявленного устройства.
- **Где:** `od-machine`, board manifest и `VIRTUAL_BOARD.md`.
- **Сделать:** один segment/root и static topology; ECAM и, если нужен выбранному Linux, другой configuration access. Absent functions, widths, readonly fields, BAR probing/sizing/alignment и decode enable определить явно. Не объявлять NVMe/NIC до их реализации.
- **Проверка:** перечисление пустых/существующих functions, BAR probe, invalid bus/device/function/offset, согласованность BAR и physical map. Использовать малый собственный тестовый endpoint, если нужен для BAR tests, и пометить его diagnostic-only.
- **Готово:** config transactions работают через guest memory/PIO, а обнаруживаемые production devices имеют реальный backend. MSI/MSI-X не требовать без объявленного устройства, которому они нужны.

#### S4-30a. ECAM window и type-0 function model (ограниченный срез)

- [x] Ограниченный срез реализован и проверен; ревью пройдено (Claude Opus, high; два раунда, замечания исправлены).
- **Сделано:** `crates/od-machine/src/pci_ecam.rs`: окно `[0xFC10_0000, 0xFC20_0000)`, bus 0, absent/width/alignment/readonly/BAR sizing, diagnostic-only тестовый endpoint в `#[cfg(test)]`.
- **Отчёт:** [STAGE_4_S4-30A_2026-11-08.md](../reports/STAGE_4_S4-30A_2026-11-08.md); evidence: [s4-30a-ecam.json](../reports/data/stage4/s4-30a-ecam.json).
- **Родительский S4-30 остаётся открытым:** нет MCFG, BAR→MMIO mapping, production device backend, legacy 0xCF8/0xCFC, host bridge; ECAM не регистрируется платой, handoff код 7 остаётся `PRESENT = 0`.

#### S4-30b. Legacy conf1 0xCF8/0xCFC поверх общей таблицы функций (ограниченный срез)

- [x] Принято после ревью (Claude Opus, high; два раунда). Отчёт: [STAGE_4_S4-30B_2026-11-08.md](../reports/STAGE_4_S4-30B_2026-11-08.md).
- **Где:** `od-machine` (`pci_ecam.rs` или общее config-ядро), `MACHINE_CONTRACT.md`, `docs/testing/MACHINE.md`.
- **Сделать:** зарегистрировать PIO-диапазон 0xCF8–0xCFF для общей таблицы функций S4-30a. Dword-запись в 0xCF8 задаёт address latch: bit31 — enable, bus 0, device, function, register (маска `0xFC`); биты 1:0 и 27:24 не поддерживаются. Обращения к 0xCFC+n шириной 1/2/4 идут к функции из latch. При bit31=0 чтение даёт все единицы, запись игнорируется; bus>0 даёт absent. Поведение 0xCFB, 0xCF9 и 0xCFA задать явно. Состояние функций общее с ECAM. Не объявлять host bridge, MCFG, E820, handoff код 7.
- **Проверка:** точная последовательность Linux `pci_check_type1`: запись в 0xCFB игнорируется, latch читается назад и восстанавливается; обращения через `MachineBus::read_port`/`write_port` с реальным `PortRegionBinding`; декодирование bus/device/function/register; байт и слово через 0xCFC+n; невыровненное обращение и qword отвергаются до изменения состояния; BAR sizing и Command через conf1 видны через ECAM и обратно; отличие bus>0 (absent через conf1, `UnknownOffset` через ECAM) задокументировано. Полные `cargo test`, `clippy`, `fmt` для `od-machine`; независимое ревью (Opus, high).
- **Готово:** Linux-подобная проба conf1 видит функцию из общей таблицы; ECAM и conf1 читают одно состояние; различия задокументированы; `check_layout` без новых ошибок.

#### S4-30c. ECAM и conf1 в карте платы и на шине (opt-in, без host bridge)

- [x] Принято после ревью (Claude Opus, high; два раунда). Отчёт: [STAGE_4_S4-30C_2026-11-08.md](../reports/STAGE_4_S4-30C_2026-11-08.md); evidence: [s4-30c-board-ecam.json](../reports/data/stage4/s4-30c-board-ecam.json).
- **Где:** `od-machine` (карта, `MachineBus`, manifest), `firmware_board.rs` (handoff код 7), `VIRTUAL_BOARD.md`, `MACHINE_CONTRACT.md`, `FIRMWARE_BOOT.md`.
- **Сделать:** зарегистрировать ECAM `[0xFC10_0000, 0xFC20_0000)` как не-RAM MMIO в единой карте платы; зарегистрировать одно PCI-config устройство на `MachineBus` (MMIO на ECAM + PIO на 0xCF8–0xCFF, общее состояние, bus 0 пустой). Добавить типизированный дескриптор в manifest (segment 0, bus 0–0, base, size). Включать через opt-in флаг платы по образцу `nvme: Option`: без флага handoff v1.0 байт-в-байт прежний; с флагом код 7 получает `PRESENT = 1` с той же base и диапазоном шин. Дефолт не менять. **Не делать:** MCFG и PNP0C02 (S4-31), генерацию E820 и loader (S4-37), host bridge и любые функции, BAR→MMIO и PCI MMIO aperture, MSI, ACPI PM.
- **Проверка:** в карте нет пересечений с RAM, ROM, framebuffer, дисплеем, NVMe, IOAPIC и LAPIC; ECAM внутри physical36 и ниже 4 GiB; `patch_physical_ram` на ECAM отвергается; через `MachineBus::read_physical` все 32 устройства bus 0 читаются как `0xFFFF_FFFF`; пробы Linux type1/type2 через `read_port`/`write_port`; запись через conf1 видна через ECAM на diagnostic endpoint в `#[cfg(test)]`; регрессия: handoff по умолчанию байт-в-байт прежний, с флагом код 7 принимает существующий DSDT-builder (`PCIE_ECAM`); независимое ревью (Opus, high).
- **Готово:** ECAM и conf1 зарегистрированы в плате под флагом; дефолтный handoff не меняется; дескриптор готов для S4-31 и S4-37.

#### S7-14a. ACPI fixed hardware для полного FADT: PM1a, PM timer, reset register (перенесён из этапа 7)

- [x] Принято после ревью (Claude Opus, high; два раунда). Отчёт: [STAGE_4_S7-14A_2026-11-08.md](../reports/STAGE_4_S7-14A_2026-11-08.md). Ограничение PM_TMR внутри кванта — ISSUE-067.
- **Почему здесь:** решение пользователя — S7-14a выполняется в этапе 4, потому что FADT (S4-31) ссылается на адреса PM-блока. Полный FADT, без `HW_REDUCED_ACPI`; reset register — отдельный порт PM-блока, 0xCF9 остаётся явной `UnknownOffset`. RTC/CMOS и HPET остаются в этапе 7 (S7-14).
- **Где:** `crates/od-machine/src/acpi_pm.rs` (PM1a evt/cnt, PM timer, reset register), `firmware_board.rs` (адреса в карте и поля handoff), `docs/specs/MACHINE_CONTRACT.md` (раздел PM-блока), `docs/specs/VIRTUAL_BOARD.md` (карта портов).
- **Сделать:** PM1a_EVT_BLK и PM1a_CNT_BLK; `SCI_EN` в PM1a_CNT установлен с сброса; `SMI_CMD = 0`; PM_TMR_BLK на 3.579545 MHz от virtual clock; reset register — SystemIO шириной 1 байт, флаг `RESET_REG_SUP`, запись `RESET_VALUE` останавливает машину с причиной `Reset` (сам сброс — S7-26). Запись `SLP_TYP/SLP_EN` для S5 → причина `Shutdown`. Адреса вне `0xCF8–0xCFF`, без пересечений с существующими PIO-устройствами. Не реализовывать HW_REDUCED, RTC/CMOS, HPET, PIC/PIT/8042, сам FADT (S4-31).
- **Проверка:** unit-тесты каждого регистра `od-machine`; PM timer монотонен и соответствует virtual clock; S5 и reset дают разные причины остановки; `0xCF9` по-прежнему `UnknownOffset`; адреса не пересекаются с ECAM, conf1, NVMe и другими портами (тест карты); регистры видны через `MachineBus::read_port`/`write_port`. Проверка ACPI mode и SCI в гостевой Linux — S4-38.
- **Готово:** полный FADT (S4-31) ссылается на существующие адреса PM-блока; `ResetSystem` и S5 используют реальные регистры.

#### S4-29a. Минимальная пара 8259 (PIC) на портах 0x20/0x21 и 0xA0/0xA1 (ограниченный срез)

- [x] Принято после ревью (Claude Opus, high). Открыто для S4-29f/g: Linux `check_timer()` пишет ICW4 с AEOI (0x03), текущая модель отвергает — решить: принимать AEOI на мастере или доказать по config, что `global_clock_event` = NULL.
- **Где:** `od-machine` (новый PIO-модуль пары 8259), контракт в `MACHINE_CONTRACT.md`.
- **Сделать:** автомат ICW1–ICW4: принимается только cascade, edge, 8086 и заданный ISA_IRQ_VECTOR; SNGL, LTIM, SFNM и buffered — DeviceError. OCW1: IMR читается обратно (на этом строит Linux `probe_8259A`: запись 0xFB, чтение). OCW2 (EOI) принимается без эффекта. OCW3 0x0A/0x0B: чтение IRR/ISR возвращает 0. У пары нет входных линий и выхода INTR.
- **Проверка:** reset-матрица; последовательность ICW1–4 и OCW1 с обратным чтением IMR; отказы SNGL/LTIM/SFNM/buffered без изменения состояния; OCW2/OCW3 по таблице; обращения через `MachineBus::read_port`/`write_port` с реальными PortRegionBinding; проверка, что порты не пересекаются с COM1, conf1, PM-блоком и NVMe.
- **Граница:** ELCR 0x4D0/0x4D1, доставка через ExtINT/LINT0, AEOI, IRQ-линии — не входят.
- **Готово:** пара 8259 отвечает на probe Linux (запись IMR, чтение назад), не даёт DeviceError на легитимной последовательности инициализации.

#### S4-29b. Определённо отсутствующие порты: CMOS 0x70/0x71 и 0xED

- [x] Принято после ревью (Claude Opus, high). **Предусловие S4-38 (блокер):** порт 0x80 без привязки останавливает машину; PIT clockevent вызывает `outb_p`, который пишет в 0x80 (`io_delay` по умолчанию). Снимается выбором `io_delay=0xed`/`udelay`/`none` в cmdline (S4-29g) или решением пользователя по порту 0x80. Проверка UIP на CMOS reg A (0xFF) — в список S4-29g/S4-38.
- **Сделать:** CMOS 0x70/0x71 и порт 0xED — явные устройства: чтение 0xFF, запись игнорируется; тест согласованности с FADT IAPC_BOOT_ARCH (NO_CMOS_RTC). Порты 0x60/0x64 (8042) и 0x80 остаются без привязки как ловушка — решение пользователя (см. S4-29 «Решения»).
- **Граница:** RTC-часы и CMOS-регистры времени — S7-14; 8042 не моделируется; может быть влит в S4-29a по решению пользователя.

#### S4-29c. Ядро PIT 8254 и порт 0x61, без IRQ

- [x] Принято после ревью (Claude Opus, high). Отклонение: счёт виден в момент записи (один такт), см. контракт. Обязательства для S4-29d: (а) fixture проверяет gate2=1 до загрузки канала 2 в `pit_calibrate_tsc` и `quick_pit_calibrate`; (б) решить — моделировать gate или явно отвергать работу канала 2 при gate2=0; (в) решить, как идут часы при PIO-опросе (ISSUE-067) для битов 4 и 5.
- **Сделать:** управляющее слово на 0x43, LSB/MSB-триггер, counter latch, режимы 0 и 2, двоичный счёт; BCD, read-back и канал 1 — DeviceError. Счётчик — точная дробь от 1 193 182 Гц по VirtualClock. Порт 0x61: бит 0 (gate2) и бит 1 хранятся, бит 5 отражает OUT2, бит 4 (refresh) задаётся в контракте.
- **Граница:** линии IRQ и события не входят (S4-29f).

#### S4-29d. Гостевая калибровка PIT и время при PIO-опросе

- [x] Принято после ревью (Claude Opus, high). Оговорки: тик только на `0x61` (`0x42` и PM_TMR — в S4-37a); инструкция = 0 нс; ISSUE-067 не закрыта, закрывается в S4-37a; gate2 отказы — изменение поведения S4-29c, записано явно.
- **Сделать:** собственная fixture повторяет `pit_calibrate_tsc` (mode 0, latch, цикл по 0x61 бит 5) и `quick_pit_calibrate` (MSB 0x42) в virtual time. Время при PIO-опросе продвигается по решению консультации Opus (раздел «Решения по консультации Opus (2026-11-09)»): порт-тик Δ=1 мкс на каждое чтение порта 0x61 (ISSUE-067); поведение gate2 — по тому же разделу (отказ, без остановки счёта). Не решать заново.
- **Проверка:** измеренная частота TSC по образцу `pit_calibrate_tsc`; отсутствие зацикливания (бюджет и STOP-причина); injected regression: отключение порт-тика (время не движется при опросе) — калибровка зацикливается и кейс падает.
- **Граница:** PIT IRQ (INTIN2) — S4-29f; настройки cmdline — S4-29g; реальное железо — не входит.
- **Зависимости:** S4-29c (принят), решение консультации Opus.
- **Готово:** измерение TSC и отсутствие зацикливания зафиксированы в evidence; регрессия различима.

#### S4-29e. Вход IOAPIC INTIN2, только edge

- [x] Принято после ревью (Claude Opus, high). Отказ LEVEL на unmasked entry 2 — изменение поведения для гостя, согласовано контрактом.
- **Сделать:** `set_intin2` и флаг `intin2_high`; конфигурация из `pins[2]`, хранение и проверка high dword (физический APIC ID 0); проверка вектора 0x10..=0xFE; LEVEL на entry 2 — отказ; конфликт вектора с level-маршрутом INTIN4 — отказ; reset и маска (edge под маской теряется, unmask не доставляет).
- **Граница:** level, Remote IRR и другие пины не входят. Зеркалит S4-26a.

#### S4-29f. PIT канал 0 → INTIN2 (GSI 2)

- [x] Принято после ревью (Claude Opus, high). Оговорки: мост — собственный EventQueue с поколениями внутри хука насоса PumpHost (S4-27i5), а не очередь InterruptPump из S4-27i3: очередь i3 ставит вектор LAPIC и обошла бы IOAPIC (отклонение записано); предел трассы насоса 512 событий ограничивает периодический PIT примерно 170 периодами (ISSUE-072, закрывается в S4-37a); PIC IR0 только наблюдается; порт 0x61 мост не потребляет; сроки фронтов на один тик раньше 8254 (наследуется из S4-29c).
- **Сделать:** события режимов 2 и 0 через EventQueue с поколениями (как APIC timer); reprogram, shutdown (0x30), reset; OUT0 идёт только в INTIN2; доставки в PIC IR0 нет. Гостевая probe получает N прерываний IRQ0 через GSI 2 и делает EOI. Негативы: замаскированный entry 2, нет EOI.
- **Зависимости:** S4-29a, S4-29c, S4-29e. Насос событий: по умолчанию — `InterruptPump` с очередью IRQ-событий и поколениями (S4-27i3), уже принятый; S4-33 его не строит. Решение зафиксировано здесь, отдельно у пользователя не спрашивалось.
- **Проверка (уточнение):** PIT ch0 mode 2 и mode 0 ставят OUT0-событие в очередь с поколением (как APIC timer, S4-28b/S4-27i2); reprogram отменяет старое поколение; shutdown (0x30) и reset отменяют события; OUT0 доходит только до INTIN2 (edge, S4-29e), PIC IR0 не получает доставки; гостевой образ (вектор из IOAPIC entry 2) получает N IRQ0, каждый со своим EOI. Инъекции: замаскированный entry 2 — событие теряется, `(…, 0 входов)`; нет EOI — второй вход не происходит; reprogram без смены поколения — старое событие доставляется (кейс падает).
- **Граница:** без PIC IR0; без реальной калибровки TSC (S4-29d уже принят); без cmdline (S4-29g); без paged; это не закрывает S4-29 целиком.

#### S4-29g. Закрытие S4-29: аудит и cmdline (только документы)

- [x] Принято с ограничениями. Ревью (Claude Opus, high): принять с замечаниями; все пять замечаний исправлены автором аудита, повторного независимого ревью правок не было. Отчёт: [STAGE_4_S4-29G](../reports/STAGE_4_S4-29G_2026-11-09.md), данные: [JSON](../reports/data/stage4/s4-29g-audit.json).
- **Выбранная cmdline (для S4-38):** `console=ttyS0,115200 earlycon=uart8250,io,0x3f8 8250.nr_uarts=1 od.nonce=<nonce>`. `io_delay` и `no_timer_check` не задаются. `od.nonce` с точкой не попадает в окружение init, но виден в `/proc/cmdline`, откуда его читает `/init` (S4-36).
- **Корректировка решения п. 7 консультации:** по config дефолт `io_delay` — `CONFIG_IO_DELAY_0XED=y` (порт 0xED), а не 0x80. Решение «`io_delay` не задавать» сохраняется; sink 0x80 нужен только для real-mode setup-кода bzImage.
- **Оговорки (не подтверждено, нужен запуск S4-38):** содержимое CPUID (ARAT, листы 0x15/0x16, бит hypervisor); синхронизация RDTSC и `udelay` с часами платы; `delay_with_tsc` требует TSC относительно часов платы не выше ~8 ГГц, иначе ветка отказа; автоопределение UART (нет FIFO и loopback в модели); связь исходников с `vmlinuz` (S4-35 проверяет только хэши).
- **Риски и блокеры для S4-38:** (1) AEOI на мастере — блокер, см. S4-29a2; (2) цикл чтения CMOS в `timekeeping_init` безусловен, при UIP=1 это около 1 с виртуального времени, затем `Unable to read current time from RTC` без паники — принято как допустимая стоимость; (3) PIT в модели не поддерживает режим 4 (`0x38`), который Linux пишет в `pit_set_oneshot`; на обычном пути LAPIC (rating 100) вытесняет PIT (rating 0), но при отказе калибровки LAPIC машина остановится; (4) PM-таймер нужен для калибровки LAPIC и для clocksource `acpi_pm`.
- **Сделать:** аудит по исходникам и config выбранного Linux; выбранная cmdline; список проверок для S4-38.

#### S4-29a2. Принять AEOI на мастере пары 8259 (блокер S4-38)

- [x] Принято после ревью (Claude Opus, high): принять с замечаниями, два замечания по формулировке контракта (reset и ICW1 про `aeoi`) исправлены. Тесты пары 8259 перепроверены координатором: `pic` 102 passed, fmt чистый. Блокер S4-38 по `check_timer` снят по коду; реальная загрузка ядра не проверялась (S4-38).
- **Контракт:** ICW4 с AEOI (0x03) на мастере принимается; флаг `aeoi` хранится без эффекта (у пары нет INTR, IRR/ISR/IMR/OCW2 не меняются); на слейве AEOI отвергается `UnsupportedConfiguration` без изменения состояния; RESET и новая ICW1 сбрасывают флаг. Источник: Linux `check_timer` → `init_8259A(1)` (`arch/x86/kernel/i8259.c:370-382`), см. аудит S4-29g. Решение п. 9 консультации Opus.
- **Проверка:** `cargo test -p od-machine --offline pic` (102 passed) и `--lib` (560 passed) в исполнении автора; мутация «откат принятия AEOI на мастере» должна ломать тесты принятия. Результаты повторно не проверены координатором до ревью.
- **Граница:** эффекта AEOI (автоматического EOI) нет и не моделируется; ELCR, IRQ-линии и доставка через INTR не входят.

### S4-31. Подготовить минимальные discovery tables

- [ ] Принято после ревью.
- **Предусловие:** S7-14a (блок выше) — FADT ссылается на адреса PM-блока. Полный FADT, без `HW_REDUCED_ACPI`; reset register — отдельный порт PM-блока; 0xCF9 остаётся `UnknownOffset`.
- **Срезы (рекомендация Opus, порядок):** S4-31a — состав таблиц (документ `ACPI_SMBIOS_TABLES.md`) и MCFG-сериализатор из записи PCIE_ECAM; S4-31b0 — const-переписывание AML/DSDT и MCFG-сериализатора (`table.rs`-хелперы) без изменения байтов (гейт: существующие golden-тесты и эталон MCFG зелёные и не меняются; MCFG перенесён сюда из S4-31d по ревью S4-31a; принято ревью Opus, отчёт `docs/reports/STAGE_4_S4-31B0_2026-11-08.md`; согласованное отступление: const-методы `BoardManifest`/`DeviceEntry` в `manifest.rs`, без них сериализаторы не const); S4-31b — DSDT для ECAM (PNP0C02 с `_CRS` на ECAM, IO 0xCF8–0xCFF у PCI0, уже в const-builder); S4-31c — расширение manifest (регион ACPI-таблиц, запись PM-блока, GSI/полярность SCI) за opt-in флагом; S4-31d — путь сборки blob и machine-side валидатор; S4-31e — FADT + FACS + `\_S5`; S4-31f — MADT; S4-31g — RSDP/XSDT и раскладка полного blob. Первый срез — S4-31a. Решения пользователя (принято «согласен» на рекомендации Opus): (1) фиксированные таблицы (FADT, MADT, MCFG, RSDP/XSDT) — способ A, `const fn` и статические байты; (2) источник PM-блока и SCI — новый код устройства в handoff v1.2; (3) типы E820 — reclaim для таблиц, NVS для FACS; (4) SCI — GSI 9, level, active-low, через ISO. Решения по открытым вопросам (приняты по рекомендациям Opus): (5) DSDT — способ A, const fn для AML/DSDT; (6) регион ACPI-таблиц — `[0x000E_0000, 0x000F_0000)`, 64 KiB, тип в карте RAM: таблицы в `[0xE_0000, 0xE_F000)` (E820 ACPI reclaim, RSDP на 0xE_0000), FACS на `[0xE_F000, 0xF_0000)` (E820 NVS, адрес FACS 0xE_F000), `[0xF_0000, 0x10_0000)` — нулевая RAM; (7) S4-29 — оставить PIT (ch0 и ch2, порт 0x61; нужен для калибровки TSC, т.к. CPUID не даёт частоту TSC) и минимальный PIC (ICW1–4/OCW1, всё замаскировано, PCAT_COMPAT=1); RTC/CMOS (0x70/0x71) — определённо-отсутствующие (чтение 0xFF, запись игнорируется, FADT NO_CMOS_RTC=1); 8042 — убрать; порт 0xED — sink для `outb_p`. **Блокер S4-38:** ISSUE-067 (PM-таймер внутри кванта) распространяется на PIT; решить до S4-38 (например, продвигать время на каждое чтение PIO-таймера).
- **Где:** resolved manifest — `od-machine`; форматы ACPI — `firmware/uefi/src/`; orchestration — `tools/build/`; отдельные firmware tests.
- **Сделать:** определить по выбранному kernel точный набор таблиц, включая необходимые RSDP/root/FADT/MADT/MCFG и их зависимости. Из machine manifest получить адреса, reservations, APIC/ECAM/routes. Не считать заранее любой короткий список достаточным.
- **Путь сборки:** отдельная freestanding guest-target сборка firmware формирует data-only ACPI blob для фиксированного manifest; packaging только извлекает bytes/sections и metadata, не дублирует serializer. Сохранить version, manifest hash, load ranges, RSDP address. UEFI services и исполнение firmware не требуются; firmware не добавляется в host workspace.
- **Проверка здесь:** checksums/lengths/references, соответствие устройствам и reservations; machine-side validator отвергает чужой manifest, overlap и out-of-range, не требуя ещё не созданного Linux loader. Изменение конфигурации требует пересборки blob. Альтернативу ACPI для discovery обосновать исходниками/config выбранного Linux; её фактическую работу проверить в S4-38.
- **Готово для S4-37:** таблицы получены реальной отдельной сборкой, независимо проверены локально и готовы к проверяемому placement. Firmware не назначает собственную карту. Обнаружение правильных CPU/APIC/ECAM самим Linux — обязательная последующая проверка S4-38, не prerequisite создания loader.

### S4-32. Собрать и загрузить собственное маленькое ядро

- [x] Принято после ревью (закрыт срезами S4-32a, S4-32b, S4-32c; отдельного подсреза S4-32d нет — закрывающая запись выполнена ревью Claude Opus high). Закрытие: собственное ядро собирается воспроизводимо, загружается на собственную probe-карту (128 KiB RAM + COM1) с проверкой границ и пересечений, исполняется через MMU и interpreter, байт UART выводится guest OUT. Boot contract явно отдельный — не Linux boot protocol и не общий ELF loader (MACHINE_CONTRACT.md, «Boot contract собственного ядра»). Оговорки: тесты лежат в `crates/od-machine/tests` и `tools/runner/tests`, а «Где» указывает `tests/system/`; сквозного CLI-случая guest fault нет (есть unit-тест и machine-тесты); полный прогон `od-runner --all-targets` в S4-32a/b/c не завершён. Не сделано и не требуется этапу 4: размещение на карте firmware board v1; ELF/segment loader; exception delivery и CR2; IRQ/HLT wakeup; JIT; кейсы S4-33. Подтверждение пользователя: «production memory» понимается как production RAM owner на probe-карте (не на board v1); entry state задаёт код, а не artifact — этого достаточно для S4-32, расширяется в S4-33 (CPL3, IRQ, exception delivery).
- **Где:** `tests/fixtures/guest/stage4/`, `tests/system/`, `tools/build/`; placement/copy — `od-machine`, orchestration — runner.
- **Сделать:** freestanding kernel/probe с отдельным linker layout, точным entry state, своими page tables/stack и polling UART. Начать с минимального входа, проверить один memory access и различимое guest completion; затем расширять теми же fixtures для S4-33.
- **Проверка:** artifact reproducibility/hash, bounds/overlap при загрузке, реальный instruction fetch через MMU и serial bytes из guest OUT. Неправильный entry/stack должен завершаться ошибкой, а не marker success.
- **Готово:** guest kernel исполняется на production memory через interpreter. Его загрузчик не выдаётся за Linux boot protocol; boot contract явно отдельный.

#### S4-32a. Воспроизводимый freestanding flat probe

- [x] Ограниченный срез принят после независимого ревью; родительский S4-32 остаётся открытым.
- **Evidence:** [отчёт S4-32a](../reports/STAGE_4_S4-32A_2026-11-05.md) и [JSON evidence](../reports/data/stage4/s4-32a-flat-probe.json).
- **Сделано:** `entry.S` и `layout.ld` формируют реальный ELF64 и фиксированный raw image; build validator проверяет PT_LOAD, page tables, code/padding, payload и hashes. `od-machine` загружает ровно `0x9000` байт verbatim по GPA `0x1000`, отвергает aliases/unexpected present entries и устанавливает фиксированный Long64/CPL0 handoff. `od-runner --stage4-probe` передаёт image в существующий bounded paged quantum и выводит фактический guest UART byte.
- **Проверено:** две clean builds с одинаковым payload дали одинаковый raw hash; loader integration — 4/4; runner CLI — 5/5; package checks, fmt и diff check прошли; реальный CLI подтвердил `MOV → OUT → HLT` и budget exhaustion с частичным состоянием.
- **Граница:** полный runner all-targets прогон не завершён и aggregate count не заявляется. Не заявляются general kernel loader, Linux boot protocol, exception delivery/CR2, IRQ/APIC, scheduler, JIT, physical hardware или полная приёмка S4-32/Stage 4.

#### S4-32b. Freestanding store→load→UART probe

- [x] Ограниченный срез принят после независимого ревью; родительский S4-32 остаётся открытым.
- **Evidence:** [отчёт S4-32b](../reports/STAGE_4_S4-32B_2026-11-05.md) и [JSON evidence](../reports/data/stage4/s4-32b-store-load-probe.json).
- **Сделано:** профиль `Stage4StoreLoadProbeV1` сохраняет layout S4-32a, но реальный guest artifact выполняет `MOV [RBX],RCX → MOV RAX,[RBX] → OUT DX,AL → HLT`. Initial qword и store-value находятся в image; loader передаёт store-value в RCX как явную часть handoff и не подставляет UART marker.
- **Проверяется:** same-input raw reproducibility, два store-value через реальный builder→CLI путь, полные RAM/RAX/UART/HLT результаты, budget 1/2/3 с частичным committed state и corrupted/equal-data rejection.
- **Граница:** это bounded interpreter/loader probe, а не общий kernel loader. Не заявляются stack execution, exception delivery/repair, alias/cross-page, IRQ/APIC, HLT wakeup, JIT, Linux boot или S4-33 completion.

#### S4-32c. Freestanding stack→UART probe

- [x] Ограниченный срез принят после независимого ревью; parent S4-32 и Stage 4 остаются открытыми.
- **Контракт:** профиль `Stage4StackProbeV1` использует отдельный opt-in
  `PagedCpl0StackProbeV1` и содержит точные байты `51 58 EE F4`
  (`PUSH RCX; POP RAX; OUT DX,AL; HLT`). Loader передаёт artifact payload в
  `RCX`, начинает с `RAX=0` и `RSP=0xa01000`; PUSH пишет qword в GPA `0xaff8`,
  POP восстанавливает `RSP=0xa01000`, а после четырёх retirement ожидаются
  payload в `RAX` и UART, `RIP=0x400004` и `Halted`.
- **Проверка:** builder→raw image→strict loader→existing paged quantum→UART;
  два payload, повторяемая сборка, бюджеты 0…4, split calls, canaries,
  corrupted code/page tables и duplicate budget. Machine tests проверяют
  committed A/D callbacks и их порядок, точный `PageFaultRecord`, полный CPU
  snapshot при stack faults, read-only PUSH без retirement, noncanonical #SS,
  cross-page/backend отказ, STOP/resume и неподдерживаемые opcode/prefix.
- **Граница:** это только bounded Long64/CPL0 interpreter evidence с уже
  принятым fixed board layout. Не закрывает parent S4-32, stack exception
  delivery/repair, CR2 commit, IRQ/APIC, HLT wakeup, JIT, Linux, hardware или
  full S4-33; ISSUE-054 остаётся открытой.

### S4-33. Принять system-набор собственного ядра

- [ ] Принято после ревью.
- **Где:** kernel fixtures, `tests/system/`, report/data stage4.
- **Сделать:** cases для RAM alias, права и `#PF` repair/retry, huge pages заявленного профиля, cross-page effects, CPL3 syscall/return, timer, UART IRQ, IOAPIC mask/EOI, HLT wakeup и self-modifying code. У каждого case отдельный идентификатор и ожидаемый результат. Fetch через границу для dword на собственном ядре остаётся открытым.
- **Проверка:** interpreter с TLB off/on и JIT только для реально поддержанных частей; injected regression обязан ломать соответствующую проверку. Сохранять guest state и event trace, а не одну строку PASS.
- **Готово:** все обязательные cases имеют факт выполнения. Общая CPU/MMU семантика ещё требует независимого evidence S4-34; локальная приёмка не выдаётся за physical.

#### S4-33a. Guest RAM alias store→load

- [x] Ограниченный case принят после независимого ревью; полный S4-33 остаётся открытым.
- **Контракт:** `Stage4AliasProbeV1` отображает GVA `0x800000` и `0xa00000` на одну RAM-страницу GPA `0x9000`. Guest исполняет `MOV [RBX],RCX; POP RAX; OUT DX,AL; HLT`; initial qword и store qword различны. Acceptance требует изменения RAM через первый GVA и чтения нового значения через stack alias, а одного HLT недостаточно.
- **Проверка:** builder manifest отражает обе mappings без противоречий; builder→runner проверяет два store values, budgets, UART, guest state и injected broken-alias outcome; machine case проверяет leaf A/D и точный порядок committed callbacks. Legacy profiles остаются строгими.
- **Граница:** uncached interpreter evidence для RAM alias. Не подтверждает TLB-on/JIT alias invalidation, cross-page semantics, fault repair, Linux или physical hardware; S4-13/14/16 и S4-18/19 остаются отдельными prerequisites.

#### S4-33b. CPL3 SYSCALL → kernel store → SYSRETQ → user load (системный кейс S4-33)

- [x] Принято после ревью (Claude Opus, high). Родитель S4-33 открыт. Оговорка: FMASK-эффект на RFLAGS не проверяется (см. отчёт «Открыто»).
- **Контракт:** кейс собственного ядра из S4-33 «CPL3 syscall/return». Использует принятый примитив S4-20c (bounded paged SYSCALL → kernel store → SYSRETQ → user load) и профиль S4-32; не требует PIT, насоса событий или IRQ.
- **Проверка:** builder → runner: user-mode код делает SYSCALL, ядро пишет qword, SYSRETQ возвращает в CPL3, user читает значение; guest state (RIP, RSP, CPL, RAX) и event trace; бюджеты и STOP-причина; injected regression: ломаешь запись ядра или возврат CPL — кейс падает.
- **Граница:** только CPL3 syscall/return; timer, IRQ, HLT wakeup, self-modifying code, huge pages, TLB-on — отдельные кейсы S4-33; это не закрывает родителя S4-33.
- **Готово:** кейс выполняется как guest fact; обе ветки (успех и инъекция) различимы в evidence.

#### S4-33c. Права (CR0.WP) и #PF repair/retry на собственном ядре (системный кейс S4-33)

- [x] Принято после ревью (Claude Opus, high). Родитель S4-33 открыт. Оговорки: нет контрольного прогона с CR0.WP=0; четвёртая инъекция для кода выхода 7 не сделана (см. отчёт «Открыто»).
- **Контракт:** кейс собственного ядра из S4-33 «права и #PF repair/retry». Использует принятые срезы S4-19b (delivery), S4-19c (#DF), S4-19d (repair и INVLPG), S4-19e1 (LEA и IRETQ), S4-19e2 (retry) и профиль S4-32; CR0.WP=1 записи в супервизорскую RO-страницу — отказ, затем repair PTE, INVLPG, IRETQ, повтор записи и загрузка.
- **Проверка:** builder → runner: sequencer из шагов fault → delivery → repair → INVLPG → discard → IRETQ → retry → load; guest state (RIP, RSP, RFLAGS.RF, CR2, PTE), A/D и committed writes по шагам; бюджеты и STOP. Injected regression: handler оставляет PTE RO (второй #PF); пропущен LEA (error code становится RIP); loader ставит RW сразу (нет delivery, кейс падает).
- **Граница:** TLB-on не проверяется (S4-14 не принят); INVLPG не заявляется как необходимый (его пропуск ничего не ломает в этом профиле); U/S, NX и #PF из CPL3 — не входят; IST/TSS, другие пары исключений, cross-page, JIT — не входят; это не закрывает родителя S4-33.
- **Готово:** кейс выполняется как guest fact; отказ, repair и повтор различимы в evidence; инъекции различимы.

#### S4-33d. Cross-page read и fetch на собственном ядре (системный кейс S4-33)

- [x] Принято после ревью (Claude Opus, high). Родитель S4-33 открыт. Оговорки: fetch через границу только word (см. «Готово»); нет контрольного прогона с отдельным NXE=0 на всём кейсе; CR2 при cross-page = выбор контракта модели (см. отчёт).
- **Контракт:** кейс собственного ядра из S4-33 «cross-page effects», только чтение и fetch. Опирается на принятые S4-11b (qword `48 8B 03`), S4-11c/d (`8B 03` с fetch через границу), S4-11e/f (`66 8B 03`, fetch на `0xffe`/`0xfff`) и профиль S4-32. Профиль `Stage4CrossPageProbeV1`: соседние GVA-страницы данных отображены на несмежные RAM-фреймы probe-карты (128 KiB); инструкция с fetch через границу стоит на стыке двух несмежных кодовых фреймов; в handoff явно заданы CPL0, AC=0, NXE=1; результат проходит через `OUT DX,AL; HLT`. Исполнение — sequencer из принятых `step_paged_mov`, `step_paged_mov_dword_read`, `step_paged_mov_word_read`, `step_paged_mov_word_read_cross_page_fetch` (экспорт `od-machine/src/lib.rs`), хвост через `step_paged_pio_u8` и quantum. Allowlist `PagedCpl0ProbeV1` не расширять (S4-32c проверяет отказ quantum на cross-page).
- **Предусловие (первый шаг реализации):** qword-путь S4-11b при NXE=1 сохраняет поведение; если нет — срез останавливается и сообщает об этом.
- **Проверка:** builder → runner. Для каждого шага фиксируются guest state (RIP, RAX, CPL), A по страницам, отсутствие D, CR2 и PFEC. Бюджеты и STOP-причина; same-page контроль; два различных значения на второй странице.
  Injected regression (каждая различима в evidence):
  - второй PTE данных not-present → #PF с адресом начала второй страницы, PFEC=0x00, без retirement, RAX не меняется, A первой страницы сохраняется;
  - второй кодовый PTE с NX → PFEC=0x11 на fetch, operand walk не выполняется;
  - loader меняет фреймы местами → неверный RAX, кейс падает;
  - вторая страница в RAM hole → отказ без retirement.
- **Граница:** cross-page store, RMW, stack, REP и RAM/MMIO split не входят (S4-11 открыт; cross-page store — отдельное архитектурное решение атомарного commit, `not_supported` в матрице); TLB-on не проверяется (S4-14 не принят); в JIT-матрице кейс `not_supported`; CPL3 и delivery #PF не входят; это не закрывает родителя S4-33. Huge pages — отдельно (после S4-10c).
- **Готово:** кейс выполняется как guest fact; успех, fault на второй странице и инъекции различимы в evidence. Оговорка: в кейсе исполняется только word fetch-cross (`66 8B 03`, S4-11f); `8B 03` с fetch через границу (S4-11d) не исполняется, так как при единственном RBX=0x800ffd любой dword пересекает границу данных, а контракт S4-11d такого операнда не покрывает; покрыто unit-тестами S4-11d.

#### S4-33e. Huge pages на собственном ядре: 2 MiB позитив и 1 GiB негатив (системный кейс S4-33)

- [x] Принято после ревью (Claude Opus, high). Родитель S4-33 открыт. Оговорки: RBX выбирает sequencer (в allowlist нет инструкции, меняющей RBX); из PFEC-литералов S4-10c исполняется только CPL0 read `0x09`; 1 GiB позитив не входит; инъекция `pdpte_ps_host_error` подставляет класс исхода, walker не откатывает (откат ловят тесты S4-10c).
- **Контракт:** кейс собственного ядра из S4-33 «huge pages заявленного профиля». Позитив: 2 MiB RAM-страница через принятый путь S4-10b (`access_long64_2m_ram`, `step_paged_mov_2m_ram`) и смешанную карту S4-12f (`stage4_cr3_mixed_probe`, 2 MiB PDE на собственном образе). Негатив: PDPTE с PS=1 (1 GiB) даёт гостевой #PF RSVD по принятому S4-10c. Исполнение — собственная фикстура и builder, runner-флаг; sequencer из принятых примитивов, профиль S4-32 (CPL0, AC=0, NXE=1); результат через `OUT DX,AL; HLT`.
- **Проверка:** builder → runner. Позитив: guest state (RIP, RAX, CPL), PDE PS=1 leaf, A/D на PDE, значение 2 MiB страницы на разных смещениях (начало, середина, конец в пределах 2 MiB). Негатив: #PF(P|RSVD) с PFEC литералом из S4-10c, CR2 = адрес доступа, A только на PML4E, без retirement. Бюджеты и STOP-причина. Injected regression (каждая различима в evidence): PDE PS в позитиве заменён на 4 KiB-таблицу (исход отличается); PDPTE PS=1 возвращён к host-ошибке `UnsupportedPageSize` (кейс падает); смещение за пределами 2 MiB даёт другой физический адрес (кейс падает).
- **Граница:** 1 GiB позитивный путь не входит (не объявлен, Page1GB=0 в CPUID); MMIO, TLB-on, JIT, CPL3, delivery #PF, cross-page и PAT не входят; PDE PS=1 в 4 KiB API остаётся `UnsupportedPageSize`; это не закрывает родителя S4-33 и S4-10.
- **Готово:** кейс выполняется как guest fact; позитив, негатив и инъекции различимы в evidence.

#### S4-10c. PDPTE.PS → гостевой #PF RSVD (подготовка S4-33e)

- [x] Принято после ревью (Claude Opus, high). Оговорки: 1 GiB не объявляется (CPUID Page1GB=0); приоритет reserved-бита PDPTE.PS относительно других reserved-битов не заявляется; pure probe проверен не во всех вариантах CPL3/NXE=0.
- **Контракт:** 1 GiB страницы не объявляются (консультация Opus, п. 14). Для Long64/4 KiB профиля PDPTE с PS=1 не является huge page и не даёт host-ошибку `UnsupportedPageSize` (как в S4-10a): это гостевой #PF с PFEC.RSVD=1 и P=1 на адрес доступа, по той же модели, что PML4 bit 7 из S4-10a (reserved-bit fault). Заменяет host-ошибку для PDPT PS в S4-10a; PD PS=1 остаётся 2 MiB по S4-10b без изменений.
- **Проверка:** unit-тесты walker/paged probe: PDPTE PS=1 → #PF(P|RSVD), CR2 = адрес доступа, A только на PML4E (предок, пройденный успешно), на PDPTE A не ставится, нет retirement; PML4 bit 7 из S4-10a не сломан; PD PS=1 (S4-10b) не сломан; PT bit 7 остаётся допустимым для 4 KiB leaf. Точные значения PFEC/A выводятся из Intel SDM и принятых правил S4-10a, не из текущего кода.
- **Граница:** без 1 GiB позитивного пути; без MMIO; без TLB-on; без записи через такой PDPTE; cross-page не входит; это не закрывает S4-10 и S4-33 (негатив 1 GiB для кейса S4-33 huge pages — отдельный кейс после этого среза).
- **Готово:** unit-тесты и regression-тесты S4-10a/S4-10b проходят; контракт и отчёт описывают замену host-ошибки на гостевой RSVD #PF; инъекция (возврат к host-ошибке) ломает тест.

#### S4-34a. Метод: реальное CPL0 исполнение через KVM на Mac mini

- [x] Метод принят после ревью (Claude Opus, high); статус S4-34 — не принят. Оговорки: один образ, по одному запуску на случай; повторяемость между запусками и перезагрузками хоста не измерена; guest CPUID = KVM_GET_SUPPORTED_CPUID хоста; irqchip/APIC/таймер отсутствуют; модель CPU из /proc/cpuinfo (план говорил lscpu). Открыто: ISSUE-071.
- **Контракт:** хост-раннер `tools/kvm-runner/` (C, один файл, gcc на хосте) создаёт VM через `/dev/kvm`, загружает собственный freestanding-образ в guest RAM, задаёт entry state (Long64/CPL0, GDT и собственные page tables гостя, EFER.LME/LMA, CR0.PG/PE, CR4.PAE), запускает `KVM_RUN` и собирает UART-байты через I/O-exit на порт `0x3f8`. Исполнение — только VT-x гостя; наш MMU не участвует.
- **Где:** Mac собирает образ (clang/lld, как фикстуры), хост на mini собирает раннер gcc'ом в `/home/senya/od-kvm/` (новый каталог; ничего вне него, boot и системные файлы не трогаются). Обмен: scp образа и раннера, результат — JSON evidence.
- **Проверка:** гость пишет `OK\n` в COM1 и `HLT`; раннер фиксирует: hardware identity (`hostname`, `uname -a`, `lscpu` model, KVM API version), SHA-256 образа и раннера, entry state (RIP, RSP, CR0/CR3/CR4/EFER, CS), последовательность exit'ов (I/O, HLT, shutdown), вывод UART, число шагов. Инъекция: образ с другой строкой или без `HLT` (шатдаун/таймаут) даёт различимый результат.
- **Граница:** один гостевой образ, без прерываний, без нашего MMU-кейса; это метод, а не приёмка S4-34 целиком; без Linux, без reboot и без изменения загрузки; `/dev/kvm` доступ — группа `kvm` (выбран вариант 1, выполнен на хосте 2026-11-09).
- **Готово:** реальное исполнение зафиксировано в evidence с hardware identity и hash'ами; инъекция различима; методика для S4-34b (наши кейсы на KVM) описана.

### S4-34. Подготовить и выполнить независимые системные hardware cases

- [ ] Методика/harness приняты; физический запуск принят отдельно.
- **Где:** `docs/testing/MACHINE.md`, собственные freestanding fixtures, `tools/x86-runner/` только для допустимой host orchestration; evidence в report/data.
- **Сделать:** составить минимальный набор для page walks/rights/A/D/CR2/errorcode, CR transitions, descriptor/delivery/return и требуемого interrupt path. Использовать единственный Mac mini по SSH `192.168.1.14`; сначала доказать способ реального CPL0 выполнения и сбора результатов. Проверить recovery, ограничения memory/device writes и независимость oracle.
- **Проверка:** фиксировать hardware identity, environment/execution kind, artifact hashes, entry state, expected defined fields и raw output. Не считать обычный Linux signal adapter проверкой всей системной семантики. Hardware topology может отличаться от virtual board: сравнивать явно общий архитектурный контракт, не абсолютные platform addresses.
- **Готово:** выбранные cases действительно выполнены независимо от нашего MMU. При отсутствии разрешённого privileged запуска статус `blocked`/`not_run`; методику нельзя отметить как physical pass. Reboot/изменение загрузки — только по отдельному конкретному разрешению согласно разделу 5.

### S4-35. Подготовить зафиксированный Linux artifact

- [x] Принято после ревью (Claude Opus, high). Оговорки: зафиксирован content hash 10 файлов по manifest S4-00a (версия 7.0.0-38.38); подлинность (provenance) НЕ подтверждена: подписанная цепочка APT, .dsc, Sources.xz и apt-provenance.json отсутствуют, PE/Authenticode не проверен; hardlink — staged-файлы разделяют inode с work/vms, потребитель обязан перехешировать по STAGED.json; zstd не распаковывается.
- **Где:** `tools/build/` или runner staging, существующий Linux profile, ignored `work/stage4/`.
- **Сделать:** взять точный Ubuntu package/image из S4-00 и существующего profile, проверить размер/hash, происхождение и доступную цепочку подписи; сопоставить source/config/build. Не заменять kernel текущим latest без пересмотра manifest.
- **Проверка:** несовпадение hash, corrupted/truncated image, отсутствие source/config evidence; различать проверенный content hash и подтверждённую подлинность. Не переносить открытые provenance пункты в «подтверждено» без проверки.
- **Готово:** staging воспроизводим; большой kernel/package не попадает в Git; report указывает точную проверенную версию, источники и оставшиеся ограничения.

### S4-36. Собрать минимальный initramfs с настоящим `/init`

- [x] Принято с ограничениями после ревью (Claude Opus). Отчёт: `docs/reports/STAGE_4_S4-36_2026-11-09.md`. Оговорки: Linux не загружался, исполнение `/init` в ядре, передача nonce через cmdline и `mount proc` не подтверждены (S4-37/S4-38); логика разбора nonce в `/init` тестами репозитория не покрыта; nonce длиннее 128 символов обрезается без ошибки, разделитель токенов только пробел, ошибки openat/read/mkdir/mount сводятся к `OD_NONCE=MISSING` rc 1 без отдельного кода (до S4-38); `exit(0)` PID 1 приводит к панике ядра; `vmlinuz_sha256` взят из манифеста S4-35 без сверки, подлинность ядра/config не подтверждена; syscall-номера сверены по таблице исходников, не исполнением.
- **Где:** собственный source/recipe в `tests/fixtures/guest/stage4/`, сборка `tools/build/`, image в ignored work.
- **Сделать:** маленький статический CPL3 program без лишних ISA требований; нужный формат initramfs, права/mode и entry. `/init` выводит переданный run nonce, делает проверяемые syscalls и возвращается к user instructions; задать завершение/ожидание для получения timer evidence.
- **Проверка:** содержимое archive, executable ABI, отсутствие host paths/secrets, repeatable build; негативные варианты отсутствующего/неисполняемого `/init` и неверного nonce.
- **Готово:** artifact подходит точному kernel config и profile; host runner не генерирует guest marker. Диск и сеть не нужны для первого userspace.

#### S4-37a. Linux-профиль платы: подсрезы a1–a6

- [ ] Принято после ревью по каждому подсрезу.
- **Граница (решение Opus, 2026-11-09):** S4-37a — привязка устройств, тик PM_TMR, порт-тик, shared timer и RDTSC на плате. Доставка IRQ в `run_boot`, общий paged-бэкенд pump, мост PIT→INTIN2 на плате и paged APIC timer — S4-38. S4-37 (loader) от S4-37a не зависит. Каждый подсрез — отдельная ветка, коммит и независимое ревью (Sonnet→Opus), слияние в main после принятия.
- **Порядок:** a1 → a2 → a3 → a4 → a5 → a6; a3 и a4 требуют a1; тест a4 опирается на a2; a6 требует a1–a3; a5 независим.
- **a1 — opt-in флаг Linux-профиля. Принято после ревью (Claude Opus): принять с замечаниями, замечания исправлены; 568 passed в od-machine lib.** Флаг в `FirmwareBoardConfig`; требует `acpi_pm`, `acpi_tables`, `pci_ecam` (иначе ошибка `Config`). Добавляет: PIT 0x40–0x43 и 0x61; пару 8259 0x20–0x21 и 0xA0–0xA1; CMOS absent 0x70–0x71; sink 0xED; sink 0x80; LAPIC `with_shared_timer(clock платы)` (a4). Не входят 0x60/0x64 (ловушка), NVMe, второй экран. Тест: PIT считает за `advance_time_ns`; ICW4=0x03 принимается; CMOS/0xED/0x80 читают 0xFF; без флага — UnmappedPort; handoff и manifest hash байт в байт прежние. Инъекции: флаг выключен — UnmappedPort; PIT на отдельном `VirtualClockHandle` — счётчик стоит.
- **a2 — тик PM_TMR (ISSUE-067, часть PM_TMR), на всех платах с `acpi_pm`. Принято после ревью (Claude Opus): принять с замечаниями, замечания исправлены; 573 passed в od-machine lib. Отклонение: инъекция «без тика» на прямолинейном образце даёт AL=0 и Halted, а не BudgetExhausted — цикл ожидания по 0x608 требует CMP/Jcc, которых в paged-профиле нет; перенесено в a6 или S4-38 (см. MACHINE_CONTRACT.md, ACPI, PM_TMR).** Устройство только ставит Δ = 1 мкс в очередь; `run_boot` применяет тики от гостевых чтений, заканчивает квант после такого чтения (stop проверяется перед каждой инструкцией, paged_probe.rs:383). Тесты под риском: `run_boot_advances_the_clock_by_retired_instructions_and_pm_tmr_sees_it` (firmware_board.rs:2434; чтение хоста без сброса очереди добавит 1 мкс), `the_pm_timer_follows_the_board_clock` (:2416), `acpi_pm_registers_answer_through_the_board_bus` (:2254). Инъекция «без тика» — BudgetExhausted.
- **a3 — тик на 0x42 (ISSUE-067, часть PIT).** Тик на 0x42 (сейчас только 0x61, pit_8254.rs:48–56), применение 0x61 и 0x42 в `run_boot`. Тест: `quick_pit_calibrate` (только MSB 0x42) и `pit_calibrate_tsc` на плате. Инъекция «тик только на 0x61» — MSB-цикл виснет до бюджета. ISSUE-067 закрывается в a2+a3. **Принято после ревью (Claude Opus): принять с замечаниями; замечания по документам и тестам исправлены. Отклонение: тест с MSB-циклом (`quick_pit_calibrate`, `pit_calibrate_tsc`) на плате не реализован — цикл требует CMP/Jcc, которых в paged-профиле нет; вместо него прямолинейные чтения 0x42 и смесь 0x61/0x42, инъекция «без тика на 0x42» (AL=0x10 против 0x0E). Полная калибровка — a6 и S4-38.**
- **a4 — APIC shared timer на плате.** Под флагом `with_shared_timer(clock платы)`; TMCCT идёт за общими часами, включая порт-тики. Тест: masked LVT, TMICT, опрос TMCCT вперемешку с PM_TMR, Δ согласованы. Инъекция: `with_timer` на приватных часах — TMCCT стоит. Без флага `has_timer()` = false. **Принято после ревью (Claude Opus): принять с замечаниями, замечания исправлены; 593 passed в od-machine lib. Частота 100 МГц — выбор платы, не факт Linux. Отклонение: гость в paged-профиле не читает MMIO APIC (MOV r32,[mem] не поддержан; qword-путь только RAM), TMCCT читает хост; после истечения срока до диспетчеризации (S4-38) запись в регистры таймера даёт Busy.**
- **a5 — ISSUE-072: кольцевая трасса pump.** `Ring(n)` со счётчиком `dropped` и сквозным `seq` рядом с `Bounded` (по умолчанию). Курсор `Stage4UartIrqProbe` (stage4_uart_irq_probe.rs:604) переводится на `seq` либо Ring запрещается вместе с хостом-конвертером. Тест: PIT IRQ0 на 1000+ периодов — в Ring Budget, в Bounded — TraceLimit. Инъекция: лимит оставлен в Ring — TraceLimit; `seq` сбрасывается — тест падает. **Принято после ревью (Claude Opus): принять с замечаниями, замечания исправлены (Ring(0) отвергается ZeroCapacity); 593 lib, 18 pit_irq0_pump, 12 stage4_uart_irq_probe. Ограничения: run_boot Ring сам не включает (S4-38 через set_trace_mode(Ring(n))); пробы runner читают трассу индексом и работают только в Bounded; классификация простоя хоста и выбор n — S4-38.** **Принято после ревью (Claude Opus): принять с замечаниями, замечания исправлены (Ring(0) отвергается ZeroCapacity; уточнены доки инъекций; ISSUE-072 обновлена). 593 / 18 / 12 passed (od-machine lib, pit_irq0_pump, stage4_uart_irq_probe); runner --tests компилируется. run_boot Ring сам не включает (S4-38); пробы с индексным курсором — только Bounded; классификация простоя хоста и выбор n — S4-38.**
- **a6 — RDTSC на плате (временной порт, решение 16б), разделён решением Opus (2026-11-09) на a6a и a6b.** **a6a (od-isa): принято после ревью** — RDTSC (0F 31) в paged-пробе Long64/CPL0 через явный источник (`step_paged_probe_with_rdtsc`, только базовый профиль, без префиксов; без источника — Unsupported{0x31}); RDTSCP и LFENCE не добавлены; LOCK-форма — #UD. **a6b (od-machine): RDTSC-порт на плате под linux_profile** — каждый RDTSC ставит 1 мкс в очередь, значение = `VirtualClock::tsc()` на границе кванта; run_boot применяет тик. Отклонение: полная `quick_pit_calibrate` с MSB-циклом на плате и UIP-цикл в госте не выполнимы без CMP/Jcc — переносятся в S4-38. Тесты a6b — прокси: `RDTSC; IN AL,DX; RDTSC; HLT` и 100×RDTSC. **a6b (od-machine): принято после ревью (Claude Opus): принять с замечаниями, замечания исправлены; 598 passed в od-machine lib.** Вывод: RDTSC на плате под linux_profile — каждый RDTSC ставит 1 мкс, значение `VirtualClock::tsc()` на границе кванта, RDTSC завершает квант. Отклонение: E4 (IN AL,imm8) не декодируется в базовом профиле (тест — MOV DX + IN AL,DX); полный MSB-цикл calibrate и UIP-цикл в госте — S4-38 (нужны CMP/Jcc).
- **Вне S4-37a (S4-38 или позже):** доставка IRQ и общий paged-бэкенд pump (S4-38); paged APIC timer (S4-38); ISSUE-069 (STI/CLI, `local_irq_*` — планируется заранее в S4-38, владелец семантики — этап 2); ISSUE-068 (REP) — только если S4-38 её выявит; классификация простоя при заблокированном векторе хоста; PIT режим 4 (`0x38`) — только если S4-38 туда попадёт.
- **Риски, записать заранее:** (а) PIT режим 4 (`0x38`) не поддержан; Linux пишет его при отказе LAPIC-таймера — остановка на 0x43; (б) CMOS UIP ~1 с — закрыто a6 как временной порт; (в) PM_TMR обязателен для калибровки LAPIC и clocksource `acpi_pm`, согласованность на Linux не проверена; (г) квант на каждое чтение временного порта — стоимость по производительности и задержка времени внутри кванта, записать как отклонение.

#### S4-37L. Декомпозиция S4-37 (решение Opus, 2026-11-09)

- **Вход:** 64-bit boot protocol на vmlinuz: RIP = `pref_address` + 0x200 (начало protected-mode части файла + 0x200), RSI = boot_params. 32-bit вход и vmlinux (ELF) отвергнуты.
- **Таблицы страниц** строит loader в S4-37 (не S4-38): 4-level identity-карта нижней RAM `[0, min(R, 0xF400_0000))` листами по 2 MiB, RW, исполняемая; MMIO не отображается.
- **Состояние на входе:** CR0 = 0x8001_0033, CR4 = 0x20 (LA57 = 0), EFER = 0xD00; GDT loader: 0x10 — 64-битный код, 0x18 — данные; CS = 0x10, DS/ES/SS = 0x18; RFLAGS = 2 (IF = 0); RSP = конец зоны стека (0x15_0000); GPR = 0 кроме RSI и RSP.
- **Ядро грузится строго по `pref_address` без relocation.** В cmdline добавляется `nokaslr` (delta = 0, `handle_relocations` не работает; проверка размещения доказывает положение ядра). Это закрывает вопрос 3 аудита S4-29g; cmdline S4-29g дополняется этим параметром.
- **Проверки живут в `od-machine`** чистыми функциями (правило S7-03: один источник правды). tools/build — только хэширование, staging и сборка evidence.
- **E820** — проекция единой карты `board_map()` плюс нарезка ACPI; `FIRMWARE_ACPI_*` выносятся в общий помощник для `handoff_regions` и E820. Запись ECAM `[0xFC10_0000, +1 MiB)` ставится безусловно при `pci_ecam`. Регионы прошивки 7–10 в Linux-пути становятся RAM.
- **CPU-признаки:** в S4-37 — только CPUID.7.ECX[16] (LA57) = 0 при CR4.LA57 = 0. Полная сверка CPUID (ARAT, листы 0x15/0x16, `verify_cpu`) — S4-38.
- **Подсрезы:**
  - [x] **L1** — разбор и проверка setup_header (`crates/od-machine/src/linux_boot.rs`, новый модуль): boot_flag, HdrS, version ≥ 0x020E, XLF_KERNEL_64, LOADED_HIGH, kernel_alignment, выравнивание pref_address, payload внутри файла, checked-арифметика; `syssize·16` → `pm_code_size` (payload и init_size проверяются относительно protected-mode кода, а не хвоста файла с подписью); экспорт наружу — три имени. Тест на синтетическом header; настоящий vmlinuz из `work/` — отдельный тест, помечается ignore, если файла нет (не считается пройденным). Вне среза: размещение, гость.
  - [x] **L2** — план размещения (чистая функция; `crates/od-machine/src/linux_placement.rs`; низкая RAM `[0, min(R, 0xF400_0000))`, ядро выше loader-зоны `[0x10_0000, 0x15_0000)`, validate перепроверяет выравнивание и порядок initrd/ядра; константа `LINUX_LOW_RAM_LIMIT` дублирует приватную `LOW_RAM_LIMIT` из firmware_board.rs — в L5/L6 сделать её общей `pub(crate)`): ядро `[pref, pref+init_size)`; initrd — сверху RAM ниже `initrd_addr_max+1`, страничное выравнивание; область loader (boot_params, cmdline, GDT, PT, стек) — константы в `[0x10_0000, pref)`; ACPI — из ValidatedAcpiBlob. Тесты: overlap, overflow, малая RAM (init_size 0x700_0000 при 128 MiB), cmdline ≥ cmdline_size, initrd выше initrd_addr_max, запрет зон `[0xE_0000, 0x10_0000)` и `< 0x9_F000`. Инъекция: initrd на ядро → Overlap.
  - [x] **L3** — E820 и сериализатор boot_params (`crates/od-machine/src/linux_boot_params.rs`; копия setup_header до `0x202 + byte[0x201]`; ACPI/NVS из констант платы с проверкой плана; RSDP целиком внутри таблиц; долги для L5/L6: дубли констант с firmware_board.rs (`LINUX_HIGH_RAM_BASE`, `HANDOFF_REGION_*`, раскладка шапки handoff) вынести одним шагом; golden 64 GiB синтетический — дамп E820 против handoff для реального R в L6; MMIO-дыры ROM и framebuffer не помечены reserved — решить в L6; S4-38: ширина физического адреса ≥ 36 в CPUID 0x80000008): (копия setup_header на 0x1F1, type_of_loader = 0xFF, cmd_line_ptr, ramdisk, e820_entries/e820_table, acpi_rsdp_addr). Golden для 128 MiB и максимума RAM; ECAM есть; диапазоны ACPI совпадают с регионами 11/12 handoff. Инъекции: без ECAM — тест падает; сдвиг FACS — расхождение с handoff.
  - [x] **L4** — рефакторинг платы (CR3 — поле `page_table_root`; `assemble_board` и `AssembledBoard::load_firmware`; golden-тест подтверждён на main до рефакторинга; долг про `handoff_regions` и `page_table_root` относится только к прошивочному пути: для Linux-платы handoff не строится, `debug_assert` на равенство `SHADOW_END` касается только прошивки) (сейчас `run` передаёт `FIRMWARE_BOOT_PAGE_TABLES_GPA`, firmware_board.rs ~937, ~950); разделение сборки платы и загрузки прошивки. Handoff, manifest hash и все тесты прошивки байт в байт прежние; golden-тест.
  - [x] **L5** — `load_linux_direct`: запись ядра, initrd, boot_params, cmdline, GDT и PT; валидатор состояния на входе; smoke: синтетический bzImage с байтами MOV/OUT/HLT по +0x200 через CR3 платы и путь 2 MiB (`run_paged_probe_2m_ram`, paged_probe.rs ~260): байт в COM1 и RSI = boot_params. Инъекции: RIP = load; CS = 0x08; IF = 1; CR4.LA57 = 1; PT без диапазона ядра — каждая различима.
  - [x] **L6** — интеграция (фасад `load_linux_direct_board` в od-machine; CLI `od-runner linux-direct` без запуска гостя, `retired = 0`; JSON-свидетельство с чтением обратно из памяти гостя; manifest hash сверяется с hash платы; blob сверяется по sha256; долги L3 — дубли констант handoff и golden для максимальной RAM — перенесены, см. отчёт среза): настоящий vmlinuz (sha256 из S4-35), blob v2 через `validate_acpi_blob` с `acpi_manifest_hash`, initramfs S4-36, cmdline S4-29g + `nokaslr`; CLI runner без запуска гостя (`tools/runner/src/`, новый модуль рядом с `firmware_boot.rs`) и JSON evidence. Ошибки до запуска (0 retired): чужой manifest, обрезанный vmlinuz, большой initrd. Положительный путь: дамп E820 с ECAM, acpi_rsdp_addr = 0xE_0000.
- **Открыто и переносится в S4-38 / другие этапы:** (1) `CpuidProfile::DiagnosticV1` с нулевыми масками признаков → `verify_cpu` ядра (kernel-head_64.S:98) провалится (нет LM и SSE2). Linux-профиль CPUID — этап 2, спланировать до S4-38. (2) Исполнение распаковщика, zstd в пределах бюджета (~10⁹ инструкций, риск), dmesg-маркеры (`BIOS-e820`, `ECAM … reserved as E820 entry`, `ACPI: RSDP`, `Command line:`, `Unpacking initramfs`, `KASLR disabled`). (3) Нужна память: ядро до ~85 MiB, минимум 128 MiB впритык. (4) ~~Проверить allowlist paged-профиля для MOV AL,imm8 и MOV DX,imm16 (smoke).~~ Закрыто smoke L5: `run_paged_probe_2m_ram` исполняет MOV AL,imm8 / MOV DX,imm16 / OUT / HLT. (5) Сочетания CR3 + 2 MiB + RDTSC/порт-тики в одном run — проверить в S4-38.

### S4-37. Реализовать Linux direct-boot loader

- [x] Принято после ревью.
- **Где:** проверка/placement/entry setup в `od-machine`, orchestration `tools/runner/src/`, `MACHINE_CONTRACT.md`.
- **Сделать:** строго по протоколу точного image проверить header, entry/alignment/relocation, разместить kernel/initrd/boot_params/cmdline/stack/page tables и table blob. E820 и reservations взять из единой карты; подготовить CR/MSR/GDT/segments/registers согласно выбранному entry. Не подставлять универсальные magic addresses из чужого примера. **Требование из ревью S4-31a (обязательно):** E820 должен содержать reserved-запись на `[0xFC10_0000, +1 MiB)` (окно ECAM). Требование действует, когда conf1 проходит `pci_sanity_check` (появилась функция host bridge/VGA или SMBIOS-год ≥ 2001) и при этом год BIOS < 2016 или DMI отсутствует: без записи Linux принимает ECAM только поздно. Альтернатива — PNP0C02 с `_CRS` (S4-31b); при ней запись E820 не обязательна, но дешева. Также: не класть zero page, cmdline и стек в `[0xE_0000, 0x10_0000)`; E820 размечает `[0xA_0000, 0x10_0000)` как reserved с ACPI reclaim (`[0xE_0000, 0xE_F000)`) и NVS (`[0xE_F000, 0xF_0000)`); ниже `0x9_F000` оставить RAM для real-mode trampoline Linux.
- **Формат blob v2 (интерфейс для этого среза, зафиксирован по ревью S4-31g):** шапка 0x60; поля 0x50 facs_offset, 0x54 facs_len (64), 0x58 facs_address (0xE_F000) описывают сегмент FACS, лежащий сразу за payload и вплотную к концу blob. Загрузчик копирует два диапазона: payload (`load_range()`, `[0xE_0000, 0xE_0340)`) и FACS (`facs_range()`, `[0xE_F000, 0xE_F040)`). E820: NVS `[0xE_F000, 0xF_0000)`, ACPI reclaim `[0xE_0000, 0xE_F000)`. RSDP на 0xE_0000 (`rsdp_address`).
- **Проверка:** overlap/overflow, слишком маленькая RAM, некорректный header, чужой table manifest, initrd за допустимым диапазоном, entry state. Ошибку сообщать до guest launch там, где она проверяется loader. E820-запись ECAM присутствует в boot_params; в S4-38 dmesg показывает `ECAM … reserved as E820 entry` или `… ACPI motherboard resource`. **Требование из ревью S4-31c:** разрез нижней RAM под ACPI (`[0xE_0000, 0xE_F000)` и `[0xE_F000, 0xF_0000)`) записан в `handoff_regions` (`firmware_board.rs`); loader должен брать его из публичных констант `FIRMWARE_ACPI_*` или из регионов 11/12 handoff, а не из второго списка констант (правило S7-03 п. 2).
- **Закрытие ISSUE-067 (перенесено из S4-29d по ревью Opus; окончательно — в a6 и S4-38):** после S4-37a2/a3 на плате тикают PM_TMR, 0x61 и 0x42 (Δ = 1 мкс, под linux_profile для PIT), очередь применяется в `LoadedFirmwareBoard::run_boot`. Остаётся: `PagedVcpuSession` и прохождение полной `quick_pit_calibrate` (`pit_verify_msb`) и ожидание по `PM_TMR` в guest-цикле — требуют CMP/Jcc в paged-профиле (a6 или S4-38). Часы синхронизируются с выполненными инструкциями. Без этого S4-38 не может считать калибровку TSC по PIT рабочей.
- **Трасса насоса для run_boot (ISSUE-072):** предел 512 событий ограничивает периодический PIT (~170 периодов); решается в подсрезе a5 S4-37a (кольцевая трасса с `dropped` и сквозным `seq`), не отключением трассы. Классификация простоя при заблокированном векторе хоста отложена (ISSUE-072 обновляется).
- **paged APIC timer (владелец — S4-38, решение Opus 2026-11-09; граница S4-27i4b):** доставка IRQ и paged-бэкенд pump, см. блок S4-37a.
- **Готово:** documented handoff воспроизводим, source/boot protocol и kernel согласованы. Loader не исполняет за гостя обязательную декомпрессию/trampoline/CPU sequence в обход принятого boot contract.

### S4-38. Довести Linux до перехода к init

- [ ] Принято после ревью.
- **Где:** `tests/system/`, runner tracing; CPU fixes — в владельце этапа 2, MMU/devices — `od-machine`.
- **Сделать:** запускать bounded boot, сохранять checkpoints: entry, decompression/relocation при наличии, paging, IDT, clock/APIC setup, initramfs, попытка exec init. На первом unsupported/fault/stall получить точный RIP/bytes/context и минимальный repro.
- **Проверка:** завершить отложенную Linux-интеграцию S4-29/31: kernel обнаруживает ровно один CPU, правильные APIC/ECAM/routes; проходит timer calibration/discovery без hang; исключённые legacy устройства действительно не нужны выбранному пути. При несоответствии исправить scope/реализацию и повторить локальные regressions и boot. Не скрывать неисправность дополнительной cmdline без объяснения и не возвращать fake CPUID/MSR/device-ready.
- **Готово:** выбранный kernel доходит до настоящего exec init без заглушек; discovery/time evidence S4-29/31 подтверждены. Сохранены точная cmdline и граница пока ещё не принятого userspace.

#### S4-38L. Декомпозиция S4-38 (предложение, 2026-11-10)

- **Проверено по образу и коду.**
  - Байты на входе `0x100_0200`: `fc fa 31 c0 8e d8 8e c0 8e d0 8e e0 8e e8 48 8d 2d …` (CLD; CLI; XOR EAX,EAX; MOV DS/ES/SS/FS/GS ← 0; LEA RBP; …; compressed-head_64.S:290–316).
  - Значения bzImage: setup_sects = 39, pm_off = 0x5000, pref = 0x100_0000, init_size = 0x44c_1000.
  - Allowlist `PagedCpl0ProbeV1` не содержит `FC`. Первый стоп на текущем пути — CLD.
  - `verify_cpu` (kernel-head_64.S:98) не проверяет результат `call`: DiagnosticV1 не останавливает ядро там. Честная верхняя граница — первая CPUID с EAX ∈ {1, 0x8000_0001}: CPUID(1) по phys `0x139_c287` (`verify_cpu+0x87`). До неё ответы совпадают с целевым профилем.
  - На честном ядре этапа 2 первый стоп — `8E D0` (MOV SS) на RIP `0x100_0208`, retired = 5.
  - Распаковщик выполняет LGDT `(%rax)`, PUSH imm + LRETQ, LIDT, чтение и запись CR4, `pushq $0; popfq` на CPL0, STD + REP MOVSQ, `jmp *%rax`, REP STOSQ. Запись распакованного ядра в `[0x100_0000, …)` вызывает #PF (ident_map_64.c:142–183), обрабатываемый `boot_page_fault`.
  - Вход распакованного ядра: e_entry `0x3f2_53e0` (`startup_64`). PT_LOAD: `[0x100_0000, +0x286_1450)` и `[0x3a0_0000, +0x160_0000)`.
- **Граница ответственности.** Без CPU-профиля Linux этапа 2 S4-38 не закрывается. В S4-38 делаем путь исполнения на плате, память/устройства/время в `od-machine`, checkpoints, эталоны и свидетельства. Каждая недостающая CPU-форма — задача этапа 2.
- **Запрещено:** подставлять CPUID/MSR; распаковывать ядро в loader за гостя; патчить байты ядра или обходить `verify_cpu`; расширять allowlist `PagedCpl0ProbeV1` под ядро (правило S7-03); добавлять параметры cmdline без записанного объяснения; вводить device-ready-заглушки.
- **Честная верхняя граница без CPU-профиля Linux.** Runner останавливается до retirement первой CPUID с EAX ∈ {1, 0x8000_0001} (`stop = cpuid_divergence`). За границей результаты диагностические и в приёмку не засчитываются.
- **Порядок:** L1 ∥ L2 → (D1) L3 → L4 → L5 → L6 → цикл L7 (до C7) → L8 (после L3) → L9 → L10 (L9 и L10 — блокер этапа 2).

- [x] **L1 — Первый стоп настоящего ядра на текущем пути (S, без решений пользователя).** Принято: фактический стоп `Unsupported 0xFC` на RIP `0x100_0200`, retired 0, байты `fc fa 31 c0` (`docs/reports/data/stage4/s4-38l1-first-stop.json`). `od-runner linux-direct --run --budget N`; Linux-плата исполняется через `run_paged_probe_2m_ram`, а не через 4 KiB walker `run`/`run_boot`. Тики linux_profile применяются как в `run_boot`.
  - Stop record (общая схема для L1–L10): exit kind, RIP, 16 байт по RIP (GVA и GPA), CPL/mode, CR0/CR3/CR4/EFER, RFLAGS, GPR, retired, quanta, virtual time, байты COM1, CPUID-профиль, linux_profile, хэши входов из S4-37 L6.
  - Ожидание: `Unsupported { opcode: 0xFC }`, RIP `0x100_0200`, retired 0, байты `fc fa 31 c0`.
  - Инъекции: budget 0 → `budget_exhausted` без fetch; синтетический bzImage из smoke L5 → Halted и байт в COM1; первый байт ядра заменён на `F4` → Halted, retired 1; путь 4 KiB walker на Linux-плате → различимая ошибка walk.
  - Evidence: `docs/reports/data/stage4/s4-38l1-first-stop.json`.
  - Ответ да/нет: fetch первой инструкции через CR3 платы работает; первый стоп — CLD.
- [x] **L2 — Независимые эталоны и статическая инвентаризация (M, без гостя, без решений).** Принято (слияние 0942a01; скрипты `tools/build/s4_38l2_*.py`, evidence `s4-38l2-oracle.json`, `s4-38l2-isa-inventory.json`).
  - **Результат (а):** payload распаковывается host-zstd; распакованный файл побайтно равен `vmlinux-7.0.0-38-generic` (sha256 `ccaf5904…034f`), PT_LOAD равны с vmlinux и vmlinux-debug; build-id `338e542e…` совпадает. Инъекции (порченый байт payload, нет trailer, чужой build-id) детектируются.
  - **Результат (б), поправка к C7:** CPUID(1) по `verify_cpu+0x87` (`0x139_c287`) — первая для вендора, отличного от GenuineIntel и AuthenticAMD (DiagnosticV1 «OpenDesktop»). На GenuineIntel первая CPUID(1) стоит раньше, по `verify_cpu+0x52` (`0x139_c252`). Checkpoint C7 привязывается к вендору строки CPUID(0), которую отдаёт профиль.
  - **Результат (в):** 519 форм / 249 семейств: exact 15, family 453, partial 16, absent 33, not_checked 2. На безусловном пути до C7 не покрыты 8 форм: MOV SS (`8E D0`, absent), LEAVE (`C9`, absent), MOV FS/GS (`8E E0/E8`, partial), LGDT [RAX] (`0F 01 10`, partial), MOV r64,CR4 и MOV CR4,r64 (partial), POPFQ на CPL0 (partial). Ещё 43 формы только в C-коде распаковщика (верхняя оценка достижимости).
  - **Результат (г):** cmdline S4-37 не даёт `earlyprintk` и не даёт uart-консоли; `early_serial_base` остаётся 0, COM1 распаковщиком не пишется. Оговорка: VGA/CRTC-порты 0x3d4/0x3d5 пишутся, если `screen_info` ненулевой; смещения `screen_info` не перепроверены.
  - **Ограничение:** статусы инвентаризации проставлены по контракту и декодеру, подтверждены не прогоном (453 формы — на уровне семейства). Косвенные переходы (37 мест) не прослежены.
 `tools/build/`.
  - (а) Распаковка host-zstd (`/opt/homebrew/bin/zstd`) payload по `payload_offset/length` без 4-байтного size trailer; PT_LOAD побайтно сверяется с vmlinux S4-35 (build-id `338e542e…`). Закрывает связку vmlinuz↔vmlinux, открытую в S4-35.
  - (б) Таблица адресов checkpoints из symtab vmlinux-debug с sha256 файла: e_entry, `verify_cpu`, CPUID(1) (0x139_c287), `x86_64_start_kernel`, `start_kernel`, `setup_arch`, `tsc_init`, `apic_intr_mode_init`, `check_timer`, `populate_rootfs`, `run_init_process`, `kernel_execve`.
  - (в) Статическая инвентаризация кодировок на пути до C7: опкод, ModRM, префиксы, первый адрес, статус в CPU_CONTRACT (есть точная форма / нет). Вход для планирования этапа 2, не свидетельство исполнения.
  - (г) По исходнику `compressed/early_serial_console.c` проверить, что без `earlyprintk` распаковщик ничего не пишет в COM1 (гипотеза, подтверждается здесь).
  - Инъекции: испорченный байт payload → падает zstd или сверка PT_LOAD; чужой vmlinux → несовпадение build-id.
  - Evidence: `s4-38l2-oracle.json`, `s4-38l2-isa-inventory.json`.
- [x] **L3 — Общий CPU-путь Linux на paged-памяти машины (L; требует D1).** Принято частично: ядро запущено на плате через набор CPL0 (`od-runner linux-direct --run --set-cpl0`, `docs/reports/data/stage4/s4-38l3-core.json`). Первый стоп — `Unsupported 0x8E` (`MOV DS, AX`) на `0x100_0204`, retired 3: блокер S2-46 (null-загрузка сегмента при загруженном кэше). Ожидание «`8E D0` на 0x100_0208, retired 5» не выполнено; причина записана. Продвижение: после S2-46 и dword-чтения через адаптер набора настоящее ядро проходит пролог сегментов и `MOV EAX,[RSI+0x230]`; первый стоп — `LGDT [RAX]` (`0F 01 10`) на `0x100_0257`, retired 22. Блокер — `LGDT`/`LRETQ` вне набора (отдельный профиль, без расширения allowlist S7-03). Продвижение: узкие записи 1/2/4 байта через адаптер набора (ревью принято, правки внесены); настоящее ядро проходит `MOV [RAX], SIL` и стопорится на `LIDT [RIP+0x17bbd]` (`0F 01 1D`) на `0x206_647c`, retired 118; следующая блокирующая форма — `LIDT m` (S2-49). Вариант B: исполнение в `run_configured` этапа 2 с принятыми формами и профилями поверх page-aware `GuestMemory` (walker 4 KiB/2 MiB, A/D, точные #PF; PIO/MMIO через `MachineBus`). Системные точки входа `od-isa` (LGDT/LIDT/LRETQ/IRETQ/MOV CRn/INVLPG) — отдельным ревью как изменение этапа 2.
  - Проверка на синтетических образах: цикл CMP/Jcc/ADD по 2 MiB; store в неотображённый GVA → точный #PF record; OUT в COM1 через bus; MOV CR3 меняет трансляцию до следующего fetch; qword через границу 4 KiB → классификация S4-11.
  - Ответ да/нет: первый стоп сместился с CLD на первую форму, которой нет в этапе 2.
- [ ] **L4 — Checkpoints, журнал CPUID и детерминизм (M, поверх L3).** Checkpoints — машинные предикаты на границе инструкции: C0 вход; C1 первый fetch из relocated-копии; C2 первый LIDT; C3 первая запись CR3; C4 первый доставленный #PF; C5 первый fetch с `0x3f2_53e0` и сверка RAM по PT_LOAD с L2; C6 CPUID(0); C7 CPUID(1) → стоп `cpuid_divergence`; C8–C17 из таблицы L2. Журнал CPUID: leaf, subleaf, RIP, ответ; расхождение — по листам {1, 0x8000_0001}.
  - **Прогресс (2026-11-10).** Граница CPUID реализована и слита (`b37d262`, S4-38L4): член `cpuid` (DiagnosticV1) в наборе по умолчанию; `CPUID` с EAX ∈ {1, 0x8000_0001} останавливается до retirement (`exit_kind = cpuid_divergence`), ответ не подставляется. Ревью Opus: принять с правками; правки внесены в `08d7e8c`. Настоящее ядро на этом наборе проходит `CPUID 0x8000_0000` (retired 138 → 162) и останавливается раньше границы: `CMP byte [RDI+0x1EF], 0` (`80 bf ef 01 00 00 00`) на `0x206_69d7`, `backend` «paged MOV profile does not support read width». Evidence: `docs/reports/data/stage4/s4-38l4-core.json`.
  - **Открыто:** C2–C7 на настоящем ядре не достигнуты. После S4-38L4a (однобайтовое чтение принято) POPFQ (`9d`) на `0x1000292` исполняется (S2-50 принято, слияние `2150f8a`), и S4-38L4b (частичный REP возобновляется платой) прогон доходит до `MOV RAX, CR3` (`0f 20 d8`) на `0x543_c0d6`: retired 1678, quanta 2148, `unsupported` (evidence `s4-38l4b-rep-run.json`). Следующая граница — `0F 1F 00` (S4-38L4c, см. ниже).

- [x] **S4-38L4c — Чтение CR3 (`MOV RAX, CR3`, `0F 20 D8`, `48 0F 20 D8`) в наборе CPL0 (S, блокер L4). Принято после ревью** (слияние `7439ffa`; ревью Opus: принять с правками; правки в `008deb8`). Гейт — существующий `cpu_owned_cr3` (нового члена нет): RAX = `state.cr3` целиком, флаги не меняются (определённость арифметических флагов сброшена, как у ReadCr4), CPL>0 не достижим через набор. Остальные префиксы и ModRM — Unsupported `0x20` (тесты на формы). Реальный прогон (бюджет 3000000): стоп `unsupported` `0x1f` — `NOP DWORD [RAX]` (`0f 1f 00`) на `0x543_b792`, retired 329560, evidence `s4-38l4c-mov-cr3-run.json`. Бюджет 50000 исчерпан раньше (`budget_exhausted`, `0x543_9a87`).
- [x] **S4-38L4d — Многобайтовый NOP `0F 1F /0` в наборе CPL0 (S, блокер L4). Принято после ревью** (слияние `5c0b4c7`; ревью Opus: принять; minor-правка тестов неканонических адресов `e975d7b`). Реальный прогон (бюджет 3000000): NOP-формы исполняются, retired 336424; стоп `backend` `CrossesPage` на `JE rel32` (`0f 84 1f 02 00 00`) на `0x543_8ffe` (evidence `s4-38l4d-nop-run.json`).
- [x] **S4-38L4e — Выборка инструкции через границу страницы в адаптере набора (S, блокер L4). Принято после ревью** (слияние `d67d3f1`; ревью Opus: принять; minor-пункты тестов — отдельным тест-коммитом). Выборка каждого байта через свою трансляцию при флаге набора; #PF — точный (CR2 — начало второй страницы, код 0x10/0x11). Реальный прогон: `JE rel32` через границу исполняется; стоп `backend` на `MOVZX r32, word [rcx]` (`0f b7 31`) на `0x543_2a44`, retired 336587 (evidence `s4-38l4e-cross-page-run.json`). Следующий срез — S4-38L4f (16-битное чтение в наборе).
- [x] **S4-38L4f — 16-битное чтение `MOVZX r32, m16` в наборе CPL0 (S, блокер L4). Принято после ревью** (слияние `519bfa6`; ревью Opus: принять; minor — тесты на `MOVSX m16`/`CMP word` на плате, слово на 0xfff внутри 2 MiB-листа и симметричная запись через границу — записаны как открытые). Слово через границу — корректный двухстраничный путь (точный #PF на второй странице). Реальный прогон: `MOVZX` исполняется, retired 372548; стоп `unsupported` `0xbc` — `F3 0F BC C0` (`REP BSF`/TZCNT EAX, EAX) на `0x541_e469` (evidence `s4-38l4f-word-run.json`).
- [x] **S4-38L4g — `F3 0F BC` (REP BSF / TZCNT) в наборе CPL0 (S, блокер L4). Принято после ревью** (слияние `b6d9ddf`; ревью Opus: принять; minor — тесты memory-формы REP BSF, REX без W/R/B, `F3 66` — записаны как открытые). Прогон: `REP BSF` исполняется как BSF, retired 390164. Новый стоп: 8-байтовая запись `MOV [RCX+4], RDI` (`48 89 79 04`) на `0x543_0920`, линейный адрес `0x545_dffc` пересекает границу 4 KiB — адаптер набора не поддерживает запись через границу (evidence `s4-38l4g-rep-bsf-run.json`). Следующий срез — S4-38L4h.
- [x] **S4-38L4h — Запись через границу страницы в адаптере набора (S, блокер L4).** Стоп: `48 89 79 04` (`MOV [RCX+4], RDI`, 8 байт) на `0x543_0920`, `CrossesPage` для записи. Сделать по образцу выборки S4-38L4e и чтения слова S4-38L4f: запись каждого байта через свою трансляцию с D-битом на листе каждой страницы; #PF на второй странице — точный (CR2 — начало второй страницы, код ошибки с W=1); состояние памяти: при отказе второй страницы не должно быть частичной записи первой (сначала трансляция обеих страниц, затем запись). Вне набора прежнее. Принято после ревью (слияние `f4af213`; ревью Opus: принять; правки формулировок и тестов `261b8a6`). Реальный прогон: запись через границу исполняется; стоп `fault` — настоящий `#PF` гостя `MOV [RDI-4], R15B` на `0x541_f236` (вектор 14, код 2, адрес `0x102_0020`), retired 390836 (evidence `s4-38l4h-cross-store-run.json`). Доставка этого `#PF` гостю — L5.

- [x] **S4-38L4b — Возобновление частичного REP набора на плате (S, блокер L4). Принято после ревью** (ревью Opus: принять с правками; слияние `5eb09cb`, правки документов `1e0cbd8`). Частичный выход `RepIterationBudgetExhausted` плата продолжает безусловно (без запроса стопа и при `remaining > 0`), а не считает Config-ошибкой. Единица бюджета — инструкция: REP засчитывается один раз, при завершении; частичный квант не тратит бюджет и не двигает виртуальное время.
  - **Ограничение (записано, решение отложено):** бюджет D3 и порог времени хоста не ограничивают REP-работу (REP на миллионы итераций — один квант по бюджету, 1 нс). Мерой служат `quanta` и evidence. Итерационная модель времени и окно IRQ между итерациями — долг до L9 (время ядра и IRQ после C7), закрывает ISSUE-068; до L9 не вводить. Граница CPUID (C7) пока не достигнута, поэтому стоп `cpuid_divergence` на реальном ядре ещё не наблюдался. Checkbox остаётся пустым, пока граница C7 не достигнута на реальном прогоне и записи checkpoints не воспроизводятся двумя прогонами.
- [x] **S4-38L4a — Однобайтовые чтения в адаптере набора CPL0 (S, блокер L4). Принято после ревью** (слияние `792da82`; ревью Opus: принять; реальный прогон на бюджете 50000 — стоп `POPFQ` `9d` на `0x1000292`, retired 1379, evidence `s4-38l4a-core.json`; нерегламентированный minor-пункт про тест через `AdapterBus::Ram` закрывает реальный прогон на плате). Новый стоп настоящего ядра: `CMP byte [RDI+0x1EF], 0` (`80 /7`) на `0x206_69d7`, retired 162. Адаптер `PagedMovMemoryAdapter::read` принимает ширины 2 (word), 4 (dword) и 8 (qword), но не 1 (`read width`). Сделать: включать однобайтовое чтение только для члена набора (по образцу `cpl0_set_dword_read`), без расширения narrow-профилей вне набора; байт читается через тот же трансляционный путь, что qword/dword, с точными A/D и без записи. Проверка: unit-тест с байтом в и через границу страницы (1-байтовое чтение не пересекает страницу), negative `Unsupported` вне набора, повтор реального прогона с новым стопом в evidence. Ревью Opus до слияния.
  - Проверка: два прогона дают побайтно одинаковые stop и checkpoint records. Инъекции: стоп по расхождению выключен → тест падает; недетерминированный источник времени → прогоны расходятся.
- [ ] **L5 — Исключения в сессии Linux-платы (M, нужен для C4).** Доставка #PF через IDT гостя (S4-19), IRETQ (S4-27h), чтение CR2 обработчиком. Проверка на fixture: обработчик отображает страницу, IRETQ, retry успешен, кадр точный, CR2 точен. Негатив: шлюз IDT отсутствует → triple fault как исход `shutdown`. Чтение CR2 — блокер этапа 2, если его нет.
  - **Конкретика по реальному ядру (2026-11-10).** Стоп — `#PF` вектора 14, код 2 (запись в непокрытую страницу, супервизор) на `0x102_0020` от `MOV [RDI-4], R15B` (`44 88 7f fc`) на `0x541_f236`, retired 390836. Для его обработки ядро использует `boot_page_fault` (IDT, установленный распаковщиком): нужны доставка исключения (шлюз IDT из `idtr`, кадр `SS/RSP/RFLAGS/CS/RIP/error`, CR2 = адрес, очистка IF по типу шлюза), чтение CR2 обработчиком (`0F 20 D0`, сейчас не поддержано — `ReadCr2` отсутствует) и `IRETQ` (`48 CF`) возврат с повтором инструкции. Делить на срезы: L5a — `MOV r64, CR2` в наборе; L5b — `IRETQ` в наборе; L5c — доставка `#PF` через IDT на плате (с инвариантом: инструкция повторяется, состояние записано).
  - **L5a — `MOV r64, CR2` в наборе CPL0: принято после ревью** (слияние `8f54afe`; ревью Opus: принять; правки о восстановлении CR2 по выходе прогона, в отличие от CR3, — `609640e`). Реальный прогон без изменений: стоп тот же `#PF` (доставки ещё нет). Следующие: L5b — `IRETQ` (`48 CF`) в наборе с проверкой CS по GDT (по образцу LRETQ S2-47b); L5c — доставка `#PF` через IDT (кадр, шлюз, CR2 с сохранением между вызовами как у CR3).
  - **L5b — `IRETQ` (`48 CF`) в наборе CPL0: принято после ревью** (слияние `e98d1b9`; ревью Opus: принять с правками, правки `dd89453`: NT=1 — #GP(0) до чтения кадра; три реализации разбора дескриптора отмечены; тесты падения записи accessed). Известное расхождение, решается в L5c: после IRETQ RF из кадра остаётся 1 (модель не сбрасывает RF после retired-инструкции; на железе сбрасывается после повтора инструкции). Реальный прогон: тот же `#PF` (IRETQ до него не доходит). Следующий — L5c: доставка `#PF` через IDT (кадр с SS/RSP всегда, RFLAGS с RF=1 по SDM для fault, код 2, CR2 с сохранением между вызовами как CR3, IF по типу шлюза, повтор инструкции).
  - **L5c — доставка `#PF` через IDT гостя: принято после ревью** (слияние `8f4a66b`; ревью Opus: принять с правками — AC не сбрасывается при входе через шлюз, RF сохраняется после повторённого IRETQ, поле CR2 платы `guest_cr2`, оговорки RepeatLimit и memory_error — `7fe0e30`). Реальный прогон: `#PF` на `0x102_0020` доставлен, обработчик выполнился (24 инструкции), инструкция повторена; стоп `unsupported` на `MOV RBX, CR2` (`0f 20 d3`), `0x543_c35e`, retired 390860 (evidence `s4-38l5c-pf-delivery-run.json`). Следующий — L5d: `MOV r64, CR2` в любой регистр.
  - **L5d — `MOV r64, CR2` в любой регистр общего назначения: принято после ревью** (слияние `cdc871b`; ревью Opus: принять с правками — восстановлены две проверки RAX в тестах od-machine, оговорка про mod, тесты нескольких REX — `82131e4`). Реальный прогон: `MOV RBX, CR2` исполняется; стоп `backend` `CrossesPage` на `MOV RCX, [RAX]` (`48 8b 08`) на `0x543_5ca0`, retired 399735 (evidence `s4-38l5d-cr2-any-reg-run.json`). Следующий — L5e: чтение qword через границу страницы в наборе (адрес `0x100_0ffa`).
- [ ] **L6 — Бюджет и производительность (S).** Измерить instr/s uncached walker; проекция до C5 (≈10⁹ инструкций, ~63 MiB вывода, REP-копирования); память платы при 128 MiB. Порог D3; превышение → решение 12 (S4-13 TLB раньше). Evidence: `s4-38l6-budget.json`.
  - **S4-38L7c — запись CR4 из регистра в наборе: принято после ревью** (слияние `fcc91f6`; ревью Opus: принять). Политика CR4: PAE обязателен, допустимы PSE/OSFXSR/OSXMMEXCPT (маска 0x600 — прежняя база); walker CR4 не читает, PSE не меняет трансляцию. Открыто minor: (а) RAX-форма `0F 22 E0` и REX-формы дают разные исходы при PAE=0 (Unsupported против #GP(0)) — долг; (б) снимок paging на плате не пересобирается после MOV CR4 (state.cr4 и paging.cr4() расходятся внутри прогона) — безвредно, пока walker CR4 не читает; (в) `interrupt.rs` внешней доставки требует `Cr4Profile::BASE` — при появлении IRQ потребуется отдельный срез. Диагностика (не засчитывается): следующая запись CR4 — `0xb0` (PGE, бит 7) по `0xffffffff8139c0c7`, retired 1137477600.
  - **S4-38L7u — `MOV r/m16/32/64, Sreg` (`[REX] 8C /r`) в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7u-mov-sreg-store`, коммиты `5c7cdd0`, `92d6f1a`, слияние `d8bf7e3`; ревью Opus: принять). Новая `MovSregStore { dst, sreg, segment }`. Регистровая форма: без REX.W — Dword с обнулением старших 32 бит, с REX.W — Qword; REX.B выбирает r8–r15, REX.R/REX.X на Sreg не влияют. Memory-форма: всегда 16 бит (REX.W не расширяет), допускается один префикс 64/65 (база FS/GS), без префикса — база 0 и #SS(0) для RSP/RBP. Селекторы: `es/ds/fs/gs_selector` — u16, `cs/ss_selector` — `Option<u16>`; при `None` — `Unsupported{0x8c}` без мутации (значение не выдумывается). Точные `48 8C E0` и `48 8C E8` (MovFs/GsSelectorToRax) остались прежними. Отказы `0x8c` без мутации: reg 6/7, LOCK, F2/F3, 66, 67, сегментный префикс у регистровой формы, `26/2E/36/3E` и повторные сегментные префиксы у memory-формы, REX не перед опкодом. Тесты: `cpl0_profile_set` 495 passed, `od-isa --lib` 2222 passed, `od-machine --lib` 870 passed (6 ignored). Замечания ревью (необязательные): фасадный тест слабее комментария (загрузочный DS уже 0x18, маркеры не проверяют 2-байтовую запись); в документе не сказано, что обнуление битов 63:32 для 32-битного приёмника — модельный выбор (по SDM зависит от реализации). **Диагностика S4-38L7u (НЕ засчитывается; evidence `s4-38l7u-diagnostic-run.json`).** Retired 1181834729 (+1179799 после `mov %ss, %eax`). Стоп — `Unsupported{opcode: 0x31}` на `0xffffffff8273440d`, байты `2e 2e 2e 31 c0` = `cs cs cs xor %eax, %eax` (три повторных префикса `2E` — padding ядра перед `31 C0`). CR0 = `0x80050033`, CR3 = `0x3ffe000`, CR4 = `0xb0`, гостевой EFER `0xd01`, RFLAGS = `0x86`; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** политика повторных сегментных префиксов CS/DS/ES/SS (`2E`/`3E`/`26`/`36`) перед обычным опкодом: декодер сейчас отказывает любой дубликат группы 2. В long mode эти префиксы не действуют (база нулевая), поэтому повтор — no-op; решение по политике — в следующем срезе (повторные 64/65 и дубликаты 66/67/F0/F2/F3 остаются отказами).
  - **S4-38L7an — `MOV r/m32, r32` и `MOV r/m64, r64` (`[64|65] [REX] 89 /r`, memory-назначение) с FS/GS в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7an-mov-store-r32-fs-gs`, коммиты `3195fb3`, `f0d5f1f`, `48f0d15`, слияние — см. граф истории; ревью Opus: принять с замечаниями, блокирующих нет). Новых операций нет: `MovFsStore`/`MovGsStore` (S4-38L7g) не привязаны к ширине (`dst.size` и вид регистра `src`); изменилась ветка `0x89` с префиксом 64/65: dword-форма без REX.W принимается, источник — ModRM.reg с REX.R (r8–r15). Dword со stale REX (REX не непосредственно перед опкодом) — отказ с 0x89 (как `8B` S4-38L7m). Qword с валидным REX.W непосредственно перед опкодом и ранним REX (`48 65 48 89 ..`) — принят, как в S4-38L7g/i и как `8B` в main: по SDM ранний REX игнорируется. **Решение оркестратора:** оставить принятым; отказ для qword дал бы асимметрию 89/8B — отдельный срез для обоих опкодов, если понадобится строгость. Тест фиксирует изоляцию раннего REX (`40–4F` перед REX.W, последний REX задаёт базу и источник). Регистровое назначение (mod=11) с FS/GS — отказ с 0x89 (как S4-38L7g); беспрефиксная `89` и CS/DS/ES/SS-формы — прежний обычный MOV (база сегмента игнорируется), покрыты тестами. Тесты: `cpl0_profile_set` 767 passed, `paged_probe_profile` 45 passed, `od-isa --lib` 2274 passed, `od-machine --lib` 896 passed (6 ignored), `od-x86-runner` 8+6 passed на macOS, `cargo fmt --check` чисто. **Открытое, не блокирует:** `native_v2.rs` не собирается под Linux (`E0063`, pre-existing); асимметрия stale REX 89/8B vs остальные FS/GS-срезы (FF, 39, 83 отказывают qword со stale REX) — записана как вопрос для отдельного среза. **Диагностика S4-38L7an (НЕ засчитывается; evidence `s4-38l7an-diagnostic-run.json`).** Retired 1208151283 → 1208151352 (+69). Граница `mov %eax, %gs:…(%rip)` по `0xffffffff813c2499` пройдена. Стоп — `Unsupported { opcode: 0x30 }` на `0xffffffff814094fb`, байты `0f 30` = `wrmsr` (ECX = `0xc0000081` IA32_STAR, EDX:EAX = `0x230010:0`). CR0 `0x80050033`, CR3 `0x3a44000`, CR4 `0xb0`, EFER walker `0xd00`, гостевой EFER `0xd01`; `exceptions_delivered` = 30; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `WRMSR IA32_STAR` (`0xC0000081`) в наборе CPL0 — запись в MSR, сейчас в наборе моделируются только FS/GS base и EFER.SCE; сначала выяснить, есть ли для STAR место в состоянии и читает ли его что-то в наборе.
  - **S4-38L7am — `LTR r/m16`, регистровая форма (`0F 00 /3`, mod=11) в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7am-ltr-reg`, коммиты `a149dab`, `fb103c9`, `c61bba4`; ревью Opus: принять с замечаниями; замечание про фасадный тест закрыто `c61bba4`). Исследование: TR в состоянии есть (`LongModeState.tr: Option<TaskRegisterCache>`, до среза заполнял только пагинированный `LTR AX`); плата при входе ставит `gdtr = {0x12_0000, 31}` и `tr = None`, TR задаёт сам гость своим LTR. Операция `LoadTr { source: Register }`. Декодер: только mod=11, опционально REX.B; REX.W, `66`, `67`, LOCK/REP и сегментные префиксы — отказ с 0x00; memory-форма и `/0`, `/1`, `/2`, `/4`–`/7` — отказ с 0x00. Гейт: CPL0, Long64, `gdtr` смоделирован. Порядок: проверки селектора и адреса без чтения памяти → два чтения qword → `validate_tss_descriptor` (тот же, что в пагинированном пути) → одна запись qword с битом занятости (бит 41) → только потом TR и RIP. Любая ошибка — TR, RIP, регистры и память не меняются. Дескриптор, пересекающий страницу (`& 0xfff > 0xff0`), — отказ с 0x00. Перенесены правила пагинированного пути (`prepare_paged_ltr_ax`/`commit`) точно; не перенесены только `TableAlias`, `TranslationChanged` и правила типа региона (их делает адаптер набора). Тесты: `cpl0_profile_set` 751 passed (16 с `ltr`), `paged_probe_profile` 44 passed, `od-isa --lib` 2274 passed, `od-machine --lib` 895 passed (6 ignored), `od-x86-runner` 8+6 passed на macOS, `cargo fmt --check` чисто. Фасадный тест на #GP (`ltr_ax_with_a_selector_beyond_the_board_gdt_limit_…`) проверяет вектор 13, код ошибки `0x40`, RIP на `LTR` и отсутствие TR. **Замечания ревью (в очередь):** запись бита занятости в GDT, отображённую только для чтения, в модели идёт обычным путём супервизора с проверкой RW/WP (при WP=1 дала бы #PF) — поведение железа и то, что Linux пишет в эту GDT именно здесь, по памяти, не проверено; ширина записи — один qword вместо 16 байт пагинированного пути (содержимое то же); декодер строже железа на REX.W и `66` (консервативно, задокументировано); для профилей mov_cr3, paged-cr3 и paged_mov отдельных тестов отказа нет (структурно закрыто гейтом `control_policy`). **Открытое, не блокирует:** `tools/x86-runner/src/native_v2.rs` не собирается под Linux (`E0063`, pre-existing). **Инцидент процесса:** агент на минуту правил два файла в основном чекауте по ошибке и откатил их; проверено — main чист, потерь нет. **Диагностика S4-38L7am (НЕ засчитывается; evidence `s4-38l7am-diagnostic-run.json`).** Retired 1208150259 → 1208151283 (+1024). Граница `ltr %ax` по `0xffffffff814094a2` пройдена (дескриптор TSS по селектору 0x40 прошёл проверки, запись бита занятости принята). Стоп — `Unsupported { opcode: 0x89 }` на `0xffffffff813c2499`, байты `65 89 05 e0 0f 07 03` = `mov %eax, %gs:0x3070fe0(%rip)` (GS + `89 /r`, MOV m32, r32, длина 7). CR0 `0x80050033`, CR3 `0x3a44000`, CR4 `0xb0`, EFER walker `0xd00`; `exceptions_delivered` = 30; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `MOV r/m32, r32` с FS/GS (`[64|65] [REX.R/X/B] 89 /r`, memory-назначение); решение, расширять ли путь `89` без префикса (сейчас отказ), за оркестратором.
  - **S4-38L7al — `SGDT m` (`0F 01 /0`, memory) с префиксом FS/GS или без него в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7al-sgdt-m`, коммиты `2e04edb`, `b4c2ab3`, `8cb138e`, слияние `63495df`; ревью Opus: принять с замечаниями, блокирующих нет; правки текста контракта — `8cb138e`). Новая операция `StoreGdtr { dst }`. Источник — `LongModeState.gdtr: Option<Gdtr>` (плата кладёт `base 0x12_0000, limit 31` при входе Linux; ядро перезагружает LGDT, уже реализованным в S2-47a). Если `gdtr == None` — отказ `Unsupported{0x01}`. Запись 10 байт: limit (2 байта LE) + base (8 байт LE) двумя перекрывающимися qword-записями `[a, a+8)` и `[a+2, a+10)` (адаптер платы принимает только 1/2/4/8 байт); порядок выбран так, что первая запись покрывает все страницы операнда. Каноничность первого и последнего (+9) байта до записи → `#GP(0)`; переполнение 2^64 → `#GP(0)`; беспрефиксная база RSP/RBP при неканоническом адресе → `#SS(0)`, с FS/GS → `#GP(0)`. Отказы с 0x01: LOCK, REP, 66, 67, REX.W, повтор и смешение 64/65, CS/DS/ES/SS, устаревший REX, регистровые формы, `/n` при n≠0. Гейт: CPL0, Long64, `gdtr.is_some()`. Тесты: `cpl0_profile_set` 735 passed, `paged_probe_profile` 43 passed, `od-isa --lib` 2273 passed, `od-machine --lib` 893 passed (6 ignored), `od-x86-runner` 8+6 passed на macOS, `cargo fmt --check` чисто. **Замечания ревью (не блокируют, в очередь):** гарантия «память не меняется при ошибке» верна при обычных условиях (адаптер проверяет обе страницы до первого байта), но не абсолютна (ошибка бэкенда второй записи после первой; первая запись в таблицы страниц для второй) — оговорено в контракте; фасадный тест пишет внутри одной страницы, пересечение границы через настоящий адаптер тестом не покрыто; эталон в `sgdt_stores_whatever_the_modeled_gdtr_holds` построен той же формулой, что реализация (частично тавтологичен). **Открытое, не блокирует:** `tools/x86-runner/src/native_v2.rs` не собирается под Linux (`E0063`, pre-existing). **Диагностика S4-38L7al (НЕ засчитывается; evidence `s4-38l7al-diagnostic-run.json`).** Retired 1208150235 → 1208150259 (+24). Граница `sgdt -0x1a(%rbp)` по `0xffffffff8140948c` пройдена. Стоп — `Unsupported { opcode: 0x00 }` на `0xffffffff814094a2`, байты `0f 00 d8` = `ltr %ax` (`0F 00 /3`, регистровая форма, длина 3). CR0 `0x80050033`, CR3 `0x3a44000`, CR4 `0xb0`, EFER walker `0xd00`, гостевой EFER `0xd01`; `exceptions_delivered` = 30; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `LTR r/m16` (`0F 00 /3`) — требует модели регистра задачи (TR) и чтения дескриптора TSS из GDT; в срезе сначала выяснить, есть ли TR в состоянии и откуда берётся значение.
  - **S4-38L7ak — `MOVSXD r64, r/m32` (`[64|65] REX.W 63 /r`, memory-источник) с FS/GS в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7ak-movsxd-fs-gs`, коммиты `1e5d405`, `5c0a2bf`, `4f06ecd`, слияние `d280dc9`; ревью Opus: принять с замечаниями, блокирующих нет). Новые операции `MovsxdFsLoad`/`MovsxdGsLoad { dst, src }` — по образцу `MovzxFsLoadW`/`MovzxGsLoadW` (S4-38L7ad). Декодер: только основная карта `63` с FS/GS, memory-источником и REX.W, без LOCK/REP/66/67 и без устаревшего REX. Исполнение: `access_address_48_with_segment_base`, `read_integer(Dword)`, знаковое расширение в 64 бита; флаги не меняются, записи в память нет; гейт CPL0. Регистровая форма с FS/GS и беспрефиксная `63` не менялись. Раньше любая `63` с FS/GS и memory отказывала с `Unsupported{0x63}`; теперь отказывают только формы вне среза (без REX.W, LOCK/REP, 66, 67, второй сегментный префикс, устаревший REX). Заменён один тест: `65 48 63 …` (теперь принимается) на `65 63 …` без REX.W (отказ 0x63). Тесты: `cpl0_profile_set` 721 passed, `paged_probe_profile` 42 passed, `od-isa --lib` 2268 passed, `od-machine --lib` 892 passed (6 ignored), `od-x86-runner` 8+6 passed на macOS, `cargo fmt --check` чисто. **Замечания ревью (в очередь):** проверка устаревшего REX в декодере (`long_decode.rs` ~9084) не покрыта ни одним тестом — форма `65 41 48 63 …` (REX.W последним после ещё одного REX) отказывает правильно, но тестом не закреплена; тесты `movsxd_with_a_zero_base_…` проверяют только адрес чтения, не значение приёмника и RIP. Формулировка причины отказа в разделе контракта исправлена (`4f06ecd`). **Открытое, не блокирует:** `tools/x86-runner/src/native_v2.rs` не собирается под Linux (`E0063`, pre-existing). **Диагностика S4-38L7ak (НЕ засчитывается; evidence `s4-38l7ak-diagnostic-run.json`).** Retired 1208148606 → 1208150235 (+1629). Граница `movslq %gs:…(%rip), %r15` по `0xffffffff813c2007` пройдена. Стоп — `Unsupported { opcode: 0x01 }` на `0xffffffff8140948c`, байты `0f 01 45 e6` = `sgdt -0x1a(%rbp)` (`0F 01 /0`, memory, длина 4). CR0 `0x80050033`, CR3 `0x3a44000`, CR4 `0xb0`, EFER walker `0xd00`, гостевой EFER `0xd01`; `exceptions_delivered` = 30; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `SGDT m` (`0F 01 /0`, memory); источник значения GDTR (`gdtr: Option<Gdtr>` в состоянии) в этом срезе не исследован, решение — за следующим срезом.
  - **S4-38L7aj — `MOV r/m32, imm32` и `MOV r/m64, imm32` (`[64|65] [REX.W] C7 /0 id`, memory) с FS/GS в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7aj-mov-imm32-fs-gs`, коммиты `19c26dd`, `2a1039b`, `033eb40`, слияние `3fd26cb`; ревью Opus: принять с замечаниями, блокирующих нет). Новые операции `MovFsMemImm`/`MovGsMemImm { dst, immediate: u64 }` — пара к `MovFsMemByteImm`/`MovGsMemByteImm` (S4-38L7ae). Декодер: общая ветка `C6`/`C7` с FS/GS и memory; `C7` без REX.W пишет dword (imm32 без расширения), с REX.W — qword (imm32 со знаковым расширением). Исполнение: гейт CPL0, `access_address_48_with_segment_base` с базой FS/GS, одна запись `write_integer(dst.size)`; назначение не читается, флаги не меняются; #GP(0) и ошибка записи не меняют состояние. RIP-relative от конца инструкции с imm32 (next RIP = адрес + 11). Регистровая форма (mod=11) с FS/GS не менялась (префикс игнорируется). Отказы с 0xc7 сохранены: `C7 /n` n≠0, LOCK/REP/REPNE, `66`, `67`, повторный сегментный префикс, устаревший REX. `88`/`89` с FS/GS — прежние отказы. Тесты: `cpl0_profile_set` 706 passed (25 отказов с точным опкодом), `paged_probe_profile` 41 passed, `od-isa --lib` 2267 passed, `od-machine --lib` 891 passed (6 ignored), `od-x86-runner` 8+6 passed на macOS, `cargo fmt --check` чисто. **Косметика ревью (не блокирует, в очередь):** в `mov_imm32_write_fault_leaves_…` для qword-случая `at` взят от граничной формы, а не от своего адреса (тест проходит по `state == before` и пустым `writes`); смещение для оракула выбирается эвристикой по байтам (`code[3] == 0xca`), а не передаётся в кортеже случая; non-canonical проверяет только верхнюю границу нижней половины, нижняя граница верхней половины не проверена. **Открытое, не блокирует:** `tools/x86-runner/src/native_v2.rs` не собирается под Linux (`E0063`, pre-existing). **Диагностика S4-38L7aj (НЕ засчитывается; evidence `s4-38l7aj-diagnostic-run.json`).** Retired 1207604143 → 1208148606 (+544463). Граница `movl $0xec, %gs:…(%rip)` по `0xffffffff83f52b4b` пройдена. Стоп — `Unsupported { opcode: 0x63 }` на `0xffffffff813c2007`, байты `65 4c 63 3d 25 b0 05 03` = `movslq %gs:0x305b025(%rip), %r15` (GS + REX.WR + `63 /r`, MOVSXD, длина 8). CR0 `0x80050033`, CR3 `0x3a44000`, CR4 `0xb0`, EFER walker `0xd00`, гостевой EFER `0xd01`; `exceptions_delivered` = 30; `memory_error` и `page_fault` отсутствуют; `cr3_changes` = 5. **Следующая граница:** `MOVSXD r64, r/m32` с FS/GS (`[64|65] REX.W 63 /r`, memory-источник); решение, расширять ли путь `63` без префикса, за оркестратором.
  - **S4-38L7ai — `LOCK ADD r/m32, r32` и `LOCK ADD r/m64, r64` (`F0 [REX.W] 01 /r`, memory-назначение, источник из ModRM.reg с REX.R) в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7ai-lock-add-reg`, коммиты `a33dff5`, `0216102`, `43d5768`, слияние `cc47a94`; ревью Opus: принять с замечаниями, блокирующих нет). Новая операция `LockAddMemReg { destination, source: RegisterView }`; исполнение скопировано с `LockAddMem`: источник читается, адрес через `access_address_48` (#GP(0), #SS(0) для RSP/RBP), одно чтение, `arithmetic_result(size, Add, …)`, одна запись всегда, флаги после записи. Декодер: LOCK только для `F0 [REX] 01 /r` с memory; регистровое назначение, `66`, `67`, сегментные префиксы, устаревший REX и повтор LOCK отказывают с 0x01; `F0 00 /r`, `F0 02/03/05` — прежние отказы. Изменено одно ожидание: `F0 48 01 03` (отказ) заменён на `F0 48 29 03` (LOCK SUB, отказ с 0x29) и регистровую `F0 48 01 C8` (отказ с 0x01). Тесты: `cpl0_profile_set` 689 passed, `paged_probe_profile` 40 passed, `od-isa --lib` 2266 passed, `od-machine --lib` 890 passed (6 ignored), `od-x86-runner` 14 passed на macOS, `cargo fmt --check` чисто. **Открытое, не блокирует:** модуль `tools/x86-runner/src/native_v2.rs` сейчас не собирается даже под Linux (`E0063`, поля `LongModeState` в тестовом конструкторе `fn state`, строка 2066; файл не менялся с 2026-11-05) — тест admission для `F0 48 01 0b` не запускается ни на одной платформе, по чтению кода форма отказывает в allowlist-`match` (`_ => Err`). Починить отдельным срезом. **Диагностика S4-38L7ai (НЕ засчитывается; evidence `s4-38l7ai-diagnostic-run.json`).** Retired 1205772058 → 1207604143 (+1832085). Граница `lock add %rdi, …(%rip)` по `0xffffffff817caaa9` пройдена. Стоп — `Unsupported { opcode: 0xc7 }` на `0xffffffff83f52b4b`, байты `65 c7 05 ca f3 4c 00 ec 00 00 00` = `movl $0xec, %gs:0x4cf3ca(%rip)` (GS + `C7 /0 id`, MOV m32, imm32, длина 11). CR0 `0x80050033`, CR3 `0x3a44000`, CR4 `0xb0`, EFER walker `0xd00`, гостевой EFER `0xd01`; `exceptions_delivered` = 30; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `MOV r/m32, imm32` и `MOV r/m64, imm32` с FS/GS (`[64|65] [REX.W] C7 /0 id`, memory); решение, расширять ли путь `C7` без префикса или завести отдельную форму, за оркестратором.
  - **S4-38L7ah — `CMP r/m32, imm8` и `CMP r/m64, imm8` (`[64|65] [REX.W] 83 /7 ib`, memory) с FS/GS в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7ah-cmp-imm-fs-gs`, коммиты `3ff9eb2`, `e73a5d2`, `8272084`, слияние `4d7be7a`; ревью Opus: принять с замечаниями, блокирующих нет). Новые операции `CmpFsMemImm`/`CmpGsMemImm { dst, immediate: u64 }` — пара к `CmpFsMem`/`CmpGsMem` (у тех источник — регистр). Декодер: новая ветка только для `83`, `/7`, FS/GS и memory; `80 /7` и `81 /7` с FS/GS отказывают с 0x80/0x81. Беспрефиксный `83 /7` и регистровая форма с префиксом 64/65 исполняются как обычный CMP (как раньше; зафиксировано регрессионными тестами). Исполнение: `access_address_48_with_segment_base`, `read_integer`, общая `arithmetic_result(size, Cmp, …)`; записи нет; imm8 расширен знаково до 64 бит и маскируется по ширине; RIP-relative от конца инструкции с imm8 (next RIP = адрес + 8). Гейт `control_policy.is_some() && Ring0`. Флаги — все шесть определены. Тесты: `cpl0_profile_set` 670 passed (оракул на 128-битной арифметике: 9 значений imm8 × 23 значения ячейки), `paged_probe_profile` 39 passed, `od-isa --lib` 2265 passed, `od-machine --lib` 889 passed (6 ignored), `cargo fmt --check` чисто. **Замечания ревью (косметика):** тест «не достижим из CPL3» проверяет только отказ на входе набора; `segment_cmp_imm8_flags_equal_the_non_prefixed_cmp` — паритет, а не независимая проверка; лишний `!matches!(size, Qword | Dword)` в декодере; замыкание `stale_rex` — третья копия правила (вынести в helper позже). Формулировка про REX.X смягчена (`8272084`). **Диагностика S4-38L7ah (НЕ засчитывается; evidence `s4-38l7ah-diagnostic-run.json`).** Retired 1205413825 → 1205772058 (+358233). Граница `cmpl $1, %gs:…` по `0xffffffff8158f69b` пройдена. Стоп — `Unsupported { opcode: 0x01 }` на `0xffffffff817caaa9`, байты `f0 48 01 3d 8f 02 dc 02` = `lock add %rdi, 0x2dc028f(%rip)` (LOCK + REX.W + `01 /r`, ADD r/m64, r64). CR0 `0x80050033`, CR3 `0x3a44000`, CR4 `0xb0`, EFER walker `0xd00`, гостевой EFER `0xd01`; `exceptions_delivered` = 30; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `LOCK ADD r/m64, r64` (и `r/m32, r32`) с префиксом LOCK (`F0 [REX.W] 01 /r`, memory); решение, расширять ли путь `F0 01` или завести отдельную форму, за оркестратором.
  - **S4-38L7ag — `LOCK AND m8, imm8` (`F0 [REX] 80 /4 ib`, memory) в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7ag-lock-and-byte-imm`, коммиты `c843445`, `acae310`, `ed18cc4`, слияние `94f9c08`; ревью Opus: принять с замечаниями, ни одно не блокирует; правка формулировки ZF — `ed18cc4`). Новая операция `LockAndMem { destination, immediate }` — копия `LockOrMem` с `LogicKind::And`, общая `logic_result`. Декодер: LOCK для `80 /4` (и только для него в группе 4); `81 /4` и `83 /4` с LOCK отказывают с 0x81/0x83 (расширение до них — однострочное, не сделано); `80 /2, /3, /5–/7` отказывают с 0x80. Исполнение: адрес без сегментной базы, `#GP(0)`/`#SS(0)` до доступа; чтение, запись всегда, флаги после записи; CF = OF = 0, SF — по старшему биту, ZF — по нулю результата, AF не определён; RIP-relative от конца инструкции с imm8 (next RIP = адрес + 8). Гейт как у `LockOrMem`. В таблицах отказов S4-38L7s убрана одна форма `f0 80 /4` (стала принятой; размер 44 → 43). Тесты: `cpl0_profile_set` 651 passed, `paged_probe_profile` 38 passed, `od-isa --lib` 2264 passed, `od-machine --lib` 887 passed (6 ignored), `cargo fmt --check` чисто. **Диагностика S4-38L7ag (НЕ засчитывается; evidence `s4-38l7ag-diagnostic-run.json`).** Retired 1204882973 → 1205413825 (+530852). Граница `lock andb $0xfe` по `0xffffffff83f4aa29` пройдена. Стоп — `Unsupported { opcode: 0x83 }` на `0xffffffff8158f69b`, байты `65 83 3d 95 79 e9 02 01` = `cmpl $0x1, %gs:0x2e97995(%rip)` (GS + `83 /7 ib`, memory RIP-relative, длина 8; далее `sete %r8b`, `test`). CR0 `0x80050033`, CR3 `0x3a44000`, CR4 `0xb0`, EFER walker `0xd00`, гостевой EFER `0xd01`; `exceptions_delivered` = 30; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `CMP r/m32, imm8` с FS/GS (`[64|65] [REX.W] 83 /7 ib`, memory); решение, расширять ли путь `83 /7` без префикса или завести отдельную форму, за оркестратором.
  - **S4-38L7af — `CALL rel32` (`E8 cd`) с сегментным префиксом CS/DS/ES/SS перед опкодом (`2E|3E|26|36 E8 cd`) в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7af-cs-call-rel32`, коммиты `1911ee1`, `6c1fd09`, `dede27b`, слияние `2256748`; ревью Opus: принять с замечаниями, замечания закрыты `dede27b`). Беспрефиксный `E8` не заперт набором CPL0; теперь ровно один префикс из `26|2E|36|3E` перед `E8` исполняется как беспрефиксный `E8` (`prefix_count == 1`). Правило «база сегмента игнорируется» принято для всех четырёх байтов явно этим срезом (общего правила контракта нет: `INVLPG m` отказывает CS/DS/ES/SS, S4-38L7ab). Повтор сегментного префикса, FS/GS в любом сочетании, 66, 67, LOCK/REP, любой REX (включая устаревший) — `Unsupported { opcode: 0xe8 }` без мутации. Исполнение: цель `RIP+6+rel32` (префикс в длине), `#GP(0)` для неканонической цели до записи; один 8-байтовый push возврата (`RIP+6`) по `RSP−8`, `#SS(0)` при неканоническом стеке; чтений нет; флаги и регистры, кроме RSP и RIP, не меняются. Вход набора при CPL3 отказывает для любых байтов; голый `run` при CPL3 исполняет префиксную форму так же, как беспрефиксный `E8`. Тесты (после `dede27b`): `cpl0_profile_set` 637 passed, `paged_probe_profile` 37 passed, `od-isa --lib` 2263 passed, `od-machine --lib` 886 passed (6 ignored), `cargo fmt --check` чисто. **Диагностика S4-38L7af (НЕ засчитывается; evidence `s4-38l7af-diagnostic-run.json`).** Retired 1202200805 → 1204882973 (+2682168). Граница `cs call rel32` по `0xffffffff83f94dd4` пройдена. Стоп — `Unsupported { opcode: 0x80 }` на `0xffffffff83f4aa29`, байты `f0 80 25 6f 39 f8 ff fe` = `lock andb $0xfe, -0x7c691(%rip)` (LOCK + `80 /4 ib`, memory RIP-relative, длина 8; далее такая же форма). CR0 `0x80050033`, CR3 `0x3a44000`, CR4 `0xb0`, EFER walker `0xd00`, гостевой EFER `0xd01`; `exceptions_delivered` = 30; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `LOCK AND m8, imm8` (`F0 [REX] 80 /4 ib`, memory); решение, расширять ли путь `F0 80|81|83 /1` (LOCK OR, S4-38L7s) на `/4` или завести отдельную форму, за оркестратором.
  - **S4-38L7ae — `MOV r/m8, imm8` (`[64|65] [REX] C6 /0 ib`, memory) с FS/GS в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7ae-gs-mov-byte-imm`, коммиты `a6faedb`, `0e73f39`, `ab60d01`, слияние `0796942`; ревью Opus: принять с замечаниями, блокирующее закрыто тестами `ab60d01`). Новые операции `MovFsMemByteImm`/`MovGsMemByteImm { dst, immediate }` порождаются только для `C6 /0` с сегментом 64/65 и memory-операндом; `/1..7` (включая XABORT `C6 F8`), `C7 /0` и `88` с FS/GS отказывают со своими опкодами; регистровая форма (mod=11) не менялась. Исполнение: `access_address_48_with_segment_base`, одна запись байта, без чтений; флаги и регистры не меняются; RIP продвигается на полную длину (imm8 учтён в RIP-relative). Неканоничный адрес — `#GP(0)`, включая базы RSP/RBP. Гейт `control_policy.is_some() && Ring0`; вне набора (голые `run`/`step`, `mov_cr3_profile`, paged-cr3-probe, `step_paged_mov`, paged-probe) — `Unsupported { opcode: 0xc6 }` без мутации. Тесты: `cpl0_profile_set` 627 passed, `paged_probe_profile` 36 passed, `od-isa --lib` 2258 passed, `od-machine --lib` 884 passed (6 ignored), `cargo fmt --check` чисто. **Замечания ревью (не блокируют):** нет теста на mod=11 с FS/GS вне набора (старое поведение); в декодере проверка `/0` стоит раньше, `size = Byte` выставляется до ветки. **Диагностика S4-38L7ae (НЕ засчитывается; evidence `s4-38l7ae-diagnostic-run.json`).** Retired 1201785902 → 1202200805 (+414903). Граница `movb $0x1,%gs:0x301a268(%rip)` пройдена. Стоп — `Unsupported { opcode: 0xe8 }` на `0xffffffff83f94dd4`, байты `2e e8 46 84 7a fe 48 8b 7d d8 e8 5d 97 f7 fd eb` = `cs call rel32` (`2E E8 cd`, длина 6). CR0 `0x80050033`, CR3 `0x3a44000`, CR4 `0xb0`, EFER walker `0xd00`, гостевой EFER `0xd01`; `exceptions_delivered` = 30; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `CALL rel32` с префиксом CS (`2E E8 cd`); в long mode CS-префикс игнорируется — решение, расширять ли путь `E8` или завести отдельную форму, за оркестратором.
  - **S4-38L7ad — `MOVZX r32/r64, r/m16` (`[64|65] [REX.W] 0F B7 /r`, memory) с FS/GS в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7ad-gs-movzx-word`, коммиты `78b4aed`, `addb260`, слияние `8eb6eb8`; ревью Opus: принять). Две новые операции `MovzxFsLoadW`/`MovzxGsLoadW { dst, src }` по образцу `MovFsLoadD`/`MovGsLoadD` (S4-38L7m): порождаются только для `0F B7` с сегментом 64/65 и memory-источником; `0F B6`, `0F BE/BF`, `63` с сегментом и памятью отказывают со своими опкодами; беспрефиксный `0F B7` и регистровая форма `65 0f b7 c3` не менялись. Приёмник: без REX.W — Dword (обнуляет старшие 32 бита), с REX.W — Qword. Порядок: каноничность (адрес+1; #GP(0); для RSP/RBP с FS/GS — #GP) → `read_integer(2)` → `state.write(dst)` → RIP; флаги не меняются, записи нет. `66` (в т.ч. с REX.W) и устаревший REX — отказ с 0xb7 для обоих размеров. Тесты: `cpl0_profile_set` 608 passed, `od-isa --lib` 2253 passed, `od-machine --lib` 883 passed (6 ignored). **Замечания ревью (косметика):** в `CPU_CONTRACT.md` (около 22815) ссылка «как у S4-38L7m» неточна — у S4-38L7m qword с устаревшим REX принимается, отказ только для dword; условие `!matches!(size, Dword | Qword)` в декодере (около 8831) недостижимо, оставлено как защитная проверка. **Диагностика S4-38L7ad (НЕ засчитывается; evidence `s4-38l7ad-diagnostic-run.json`).** Retired 1201785902 (+37865 после MOVZX). Стоп — `Unsupported{opcode: 0xc6}` на `0xffffffff8141d4e4`, байты `65 c6 05 68 a2 01 03 01` = `MOV %gs:disp32(%rip), $0x1` (`C6 /0 ib`, MOV r/m8, imm8, длина 8). CR0 = `0x80050033`, CR3 = `0x3a44000` (`cr3_changes` 5), CR4 = `0xb0`, гостевой EFER `0xd01`; `exceptions_delivered` = 30; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `MOV r/m8, imm8` с GS (`65 C6 /0 ib`, memory, store с immediate).
  - **S4-38L7ac — `TEST r/m32, imm32` и `TEST r/m64, imm32` (`[64|65] [REX.W] F7 /0 id`, memory) с FS/GS в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7ac-gs-test-dword`, коммиты `0925769`, `b46287c`, слияние `7faecb0`; ревью Opus: принять, замечания — косметика). Одна операция `TestFsMemWide`/`TestGsMemWide { src, immediate: u64 }`, ширина из `src.size`. imm32: для dword — нулевое расширение, для qword (REX.W) — знаковое (проверено тестами на `0xffffffff80000000` и `0x100000000`). Порядок: каноничность (+3 или +7; для RSP/RBP с GS — #GP(0)) → read → `logic_result(size, Test)` → rflags; записи в память нет. Приоритет REX.W над 66: форма 66+REX.W отказывает (не угадывалось). Регистровые формы `65/64 f7 c3 id` исполняются как обычный TEST (согласовано с S4-38L7o/r/p/q/aa). Выпавшие из отказов (принятые теперь): `65 f7 03 id`, `65 48 f7 03 id`, `64 f7 03 id`. Тесты: `cpl0_profile_set` 592 passed, `od-isa --lib` 2248 passed, `od-machine --lib` 882 passed (6 ignored), `paged_probe_profile` 34 passed. Замечания ревью (косметика): комментарии «refuse `[64|65] F6 /0 ib`» в тесте про F7 и два длинных комментария; тест фасада не различает сброс CF/OF промежуточных TEST (после них идёт ADD); общий хелпер `access_address_48_with_segment_base` при переполнении даёт `AddressOverflow`, а не #GP (вне среза, на будущее). **Диагностика S4-38L7ac (НЕ засчитывается; evidence `s4-38l7ac-diagnostic-run.json`).** Retired 1201748037 (+2 после TEST и `jne`). Стоп — `Unsupported{opcode: 0xb7}` на `0xffffffff8141d743`, байты `65 0f b7 3d 05 a0 01 03` = `MOVZX %gs:disp32(%rip), %edi` (`0F B7 /r`, MOVZX r32, r/m16 с GS, длина 8). CR3 = `0x3a44000` (`cr3_changes` 4), CR4 = `0xb0`, гостевой EFER `0xd01`; `exceptions_delivered` = 30; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `MOVZX r32, r/m16` (`[64|65] 0F B7 /r`) с GS в наборе CPL0.
  - **S4-38L7ab — `INVLPG m` (`0F 01 /7`, memory) в наборе CPL0 как no-op: принято после ревью** (ветка `feat/s4-38l7ab-invlpg-noop`, коммиты `7c87399`, `2c008a8`, слияние `d0931eb`; ревью Opus: принять с исправлениями — косметика). **Решение по умолчанию, пересматриваемое:** INVLPG — no-op: модель не имеет TLB, кэш трансляции выборки S4-13 прозрачен для результата; адрес не вычисляется и не проверяется (по SDM INVLPG не даёт #GP на неканоническом адресе), память и регистры не трогаются, флаги не меняются. Операция `Invlpg` (без полей). Декодер: `0F 01 /7` с memory, один FS/GS, REX без W; отказ с 0x01 — LOCK, REP, 66, 67, REX.W, второй сегментный префикс, CS/DS/ES/SS (консервативно), устаревший REX, регистровые `/7`. SWAPGS, LGDT/LIDT не изменились. Гейт `control_policy && Ring0 && Long64`. Отдельный профиль `step_invlpg`/`InvlpgLong64V1` не используется набором и не затронут. Побочный эффект декодера: безпрефиксная `0F 01 /7` и формы с REX/FS/GS дочитывают ModRM/SIB/disp до отказа (как у LOCK OR, S4-38L7s), вне набора это может дать ошибку выборки вместо `Unsupported` на границе страницы. Тесты: `cpl0_profile_set` 572 passed, `od-isa --lib` 2243 passed, `od-machine --lib` 881 passed (6 ignored), `paged_probe_profile` 33 passed. **Замечания ревью (косметика):** в фасадном тесте `linux_direct.rs` (около 3430/3439) и в документе «неканонический RDI» — это `u64::MAX`, то есть канонический адрес; настоящий неканонический проверен в `cpl0_profile_set::invlpg_with_a_non_canonical_address_is_not_a_fault`. Раздел S4-13 (CPU_CONTRACT около 21043) говорит, что INVLPG сбрасывает кэш выборки, в коде этого нет (сброс по записям в кадры таблиц, эффекта нет). **Диагностика S4-38L7ab (НЕ засчитывается; evidence `s4-38l7ab-diagnostic-run.json`).** Retired 1201748035 (+16198119 после INVLPG); `cr3_changes` 3 → 4 (CR3 `0x3a44000`). Стоп — `Unsupported{opcode: 0xf7}` на `0xffffffff8141d729`, байты `65 f7 05 fc f8 ff 02 ff ff ff 7f` = `TEST %gs:disp32(%rip), $0x7fffffff` (`F7 /0 id` с GS, memory, длина 11). CR0 = `0x80050033`, CR4 = `0xb0`, гостевой EFER `0xd01`, RFLAGS = `0x46`; `exceptions_delivered` = 30; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `TEST m32, imm32` с GS (`65 F7 /0 id`, memory), по образцу `TestGsMemWord` (S4-38L7r).
  - **S4-38L7aa — `CMP r/m64, r64` (и `r/m32, r32` без REX.W) с FS/GS (`[64|65] [REX] 39 /r`, memory-операнд) в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7aa-gs-cmp-qword`, коммиты `fbf1dd6`, `4f1e0ab`, `83a5cc1`, `1d6d144`, слияние `bf2fefc`; ревью Opus: принять с исправлениями — внесены). Новые `CmpFsMem`/`CmpGsMem { dst: MemoryOperand, src: RegisterView }`, в той же ветке ALU, что SubFsMem/SubGsMem. Порядок: каноничность (#GP(0); для RSP/RBP без префикса — по обычному пути) → read(dst.size) → `arithmetic_result(size, Cmp)` → `rflags = flags`; записи в память нет, регистры не меняются. **Исправление ревью:** регистровая форма `[64|65] 39 /r` с mod=11 НЕ сужена — префикс FS/GS у операции без memory-операнда игнорируется, как в S4-38L7o/p/q/r (условие ветки дополнено `modrm >> 6 != 3`). Внимание: S4-38L7j (SUB r64, m64) регистровую форму `2B` с FS/GS по-прежнему отказывает — это расхождение с остальной линией; вынесено отдельным вопросом. Тесты (после исправления): `cpl0_profile_set` 565 passed, `od-isa --lib` 2238 passed, `od-machine --lib` 880 passed (6 ignored). **Диагностика S4-38L7aa (НЕ засчитывается; evidence `s4-38l7aa-diagnostic-run.json`).** Retired 1185549916 (+207285 после `cmp`). Стоп — `Unsupported{opcode: 0x01}` на `0xffffffff8141d590`, байты `0f 01 3f` = `INVLPG (%rdi)` (`0F 01 /7`). CR3 = `0x3ffe000`, CR4 = `0xb0`, гостевой EFER `0xd01`, RFLAGS = `0x46`; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `INVLPG m` (`0F 01 /7`) в наборе CPL0; семантика (что делать на модели без TLB) открыта.
  - **S4-38L7z — `LOCK ADD r/m8/m32/m64, imm` (`F0 [REX] 80|81|83 /0 ib|id`, memory) в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7z-lock-add-imm8`, коммиты `4b33126`, `058140d`, слияние `3c31f81`; ревью Opus: принять). Новая `LockAddMem { destination, immediate }`, парная к `LockOrMem`; общая ветка 80/81/83 выбирает операцию по `group` (`/0` ADD, `/1` OR). Порядок: каноничность (адрес+последний байт; #GP(0); RSP/RBP — #SS(0)) → read → `arithmetic_result(size, Add)` → write всегда → флаги после записи; все шесть флагов определены. Заодно принимаются `F0 80 /0`, `F0 81 /0` и qword (REX.W); `66` с 80/81/83 — отказ; `/2`–`/7` с LOCK — отказ. Не-LOCK `80/81/83 /0` не изменились. Флаги по ручной сверке: `0x80 + (−128)` = 0 с CF=1; `0 + (−128)` = `0xffffff80` с CF=0, SF=1 (в постановке ожидалось CF=1 — ошибка постановки, зафиксировано). Тесты: `cpl0_profile_set` 544 passed, `od-isa --lib` 2233 passed, `od-machine --lib` 879 passed (6 ignored). Замечания ревью (косметика): комментарий про расширение immediate неточен (для `83` знаково расширяется до 64 бит при любой ширине, маскирование в `arithmetic_result` безопасно); одна строка комментария длиннее 100 колонок. **Диагностика S4-38L7z (НЕ засчитывается; evidence `s4-38l7z-diagnostic-run.json`).** Retired 1185342631 (+315214 после `lock addl`). Стоп — `Unsupported{opcode: 0x39}` на `0xffffffff82734608`, байты `65 48 39 15 08 8a ce 01` = `CMP %rdx, %gs:disp32(%rip)` (CMP r/m64, r64, GS, memory). CR3 = `0x3ffe000`, CR4 = `0xb0`, гостевой EFER `0xd01`, RFLAGS = `0x46`; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `CMP r/m64, r64` с GS (`65 48 39 /r`, memory), по образцу `SubGsMem` (S4-38L7j).
  - **S4-38L7y — `LOCK CMPXCHG r/m32, r32` (`F0 [REX без W] 0F B1 /r`, memory) в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7y-lock-cmpxchg-dword`, коммиты `536c80e`, `6450645`, `a63eb44`, слияние `ea968ec`; ревью Opus: принять). Расширение S4-38L7k на dword: REX без W действует (REX.R — источник r8d–r15d, REX.B/X — база и индекс); не последний REX в dword-форме — отказ (консервативно: иначе `48 f0 0f b1` молча стал бы dword). Порядок: каноничность (адрес+3; #GP(0); для RSP/RBP — #SS(0)) → read(4) → `arithmetic_result(Dword, Cmp)` → write(4) всегда → флаги и EAX после успешной записи; при неравенстве запись Dword обнуляет RAX[63:32], при равенстве RAX не меняется. Не-LOCK dword отказывается (как qword в наборе). Тесты: `cpl0_profile_set` 528 passed, `od-isa --lib` 2228 passed, `od-machine --lib` 878 passed (6 ignored). Замечание ревью (документ, не код): фраза «qword S4-38L7k без изменений, в том числе принятие REX.W до другого префикса» неточна — REX.W перед legacy-префиксом сбрасывается, принимается другой REX перед REX.W (`f0 40 48 0f b1`). **Диагностика S4-38L7y (НЕ засчитывается; evidence `s4-38l7y-diagnostic-run.json`).** Retired 1185027417 (+573 после `lock cmpxchg`). Стоп — `Unsupported{opcode: 0x83}` на `0xffffffff81537280`, байты `f0 83 44 24 fc 00` = `LOCK ADD dword -0x4(%rsp), $0` (LOCK + `83 /0 ib`, база RSP — путь #SS). CR3 = `0x3ffe000`, CR4 = `0xb0`, гостевой EFER `0xd01`, RFLAGS = `0x2`; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `LOCK ADD m32, imm8` (`F0 83 /0 ib`), по аналогии с `LockOrMem` (S4-38L7s).
  - **S4-38L7x — `pio_u8` в наборе Linux по умолчанию: принято после ревью** (ветка `feat/s4-38l7x-linux-pio-default`, коммиты `15db8ac`, `3fc89b3`, `ebbfd0e`, `c6abb0f`, слияние `9f67f68`; ревью Opus: принять с исправлениями — внесены). **Решение по умолчанию, пересматриваемое:** `pio_u8` включён в `LinuxCpl0SetMembers::linux_default()`, `minimal()` без него. Причина: без COM1 и таймера ядро не продвигается дальше. Фактический доступ шире «COM1/PIT/PM»: через `pio_u8` доступны все порты шины платы — COM1 `0x3f8..0x3ff`, PM-блок `0x600..0x60f` (PM-таймер `0x608`, только при `acpi_pm`), при `linux_profile`: PIT `0x40..0x43`, port B `0x61`, PIC `0x20..0x21` и `0xa0..0xa1`, CMOS `0x70/0x71`, `0xed`, `0x80`, и PCI conf1 `0xcf8..0xcff` (только при ECAM-устройстве). Известное ограничение: байтовые обращения к CONFIG_ADDRESS идут в `PciEcam.conf1_latch`, dword — в ячейку `pci_cell` члена `pci_config`; сейчас результат одинаков (шина пуста), при PCI-функции защёлки разойдутся. Порт берётся из `DX[15:0]`; `IN AL, DX` меняет только AL. Порядок `set_members`: `… msr:fs_gs_base, [halt], pio_u8, popf_cpl0, pci_config`. Тесты: `cpl0_profile_set` 515 passed, `od-isa --lib` 2227 passed, `od-machine --lib` 876 passed (6 ignored), `od-runner linux_direct` 25 passed (3 ignored). **Диагностика S4-38L7x (НЕ засчитывается; evidence `s4-38l7x-diagnostic-run.json`).** Retired 1185026844 (+11457 после `IN AL, DX`). Стоп — `Unsupported{opcode: 0xb1}` на `0xffffffff81501ee7`, байты `f0 0f b1 13` = `LOCK CMPXCHG r/m32, r32` (`%edx, (%rbx)`, dword, без REX.W). COM1-вывод пуст; CR0 = `0x80050033`, CR3 = `0x3ffe000`, CR4 = `0xb0`, гостевой EFER `0xd01`; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `LOCK CMPXCHG r/m32, r32` — расширение S4-38L7k на dword-форму (семантика LOCK и флаги уже решены в S4-38L7k).
  - **S4-38L7w — порты PCI-конфигурации, механизм #1 (`OUT`/`IN` dword и word на `0xCF8`/`0xCFC..0xCFF`) в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7w-pci-config-ports`, коммиты `473b6e6`, `3c47c2d`, `31e8ce8`, `cc84117`, слияние `f57a0dc`; ревью Opus: принять с исправлениями — исправления внесены). Решения по умолчанию (не из SDM, соглашения проекта): адрес конфигурации хранится (`0xCF8`), dword-запись сохраняет 32 бита; word-доступ к `0xCF8` — отказ (не замена `[15:0]`); `IN` с `0xCFC..0xCFF` даёт все единицы по размеру (dword обнуляет старшие 32 бита RAX, word сохраняет старшие); `OUT` в `0xCFC..0xCFF` отбрасывается; устройств на шине нет. Порт берётся из `DX` (младшие 16 бит). Любой REX перед `66` — отказ. Член набора `pci_config` (в `linux_default` включён, в `minimal` — нет), независим от `pio_u8`; адрес хранится в плате между запусками; `stop.pci_config_address` в JSON runner'а. Тесты: `cpl0_profile_set` 515 passed, `od-isa --lib` 2227 passed, `od-machine --lib` 874 passed (6 ignored), `od-runner linux_direct` 25 passed (3 ignored). **Диагностика S4-38L7w (НЕ засчитывается; evidence `s4-38l7w-diagnostic-run.json`).** Retired 1185015387 (+155064 после `out %eax, %dx`). Стоп — `Unsupported{opcode: 0xec}` на `0xffffffff81ff8a2c`, байты `ec c9` = `IN AL, DX` (байтовый PIO на COM1, порт `0x3f9`). `pci_config_address` = `0x80001000`; CR0 = `0x80050033`, CR3 = `0x3ffe000`, CR4 = `0xb0`, гостевой EFER `0xd01`; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** байтовый `IN AL, DX` на COM1. Для него нужно решение, включать ли `pio_u8` (COM1/PIT/PM-таймер платы) в набор Linux по умолчанию. Это открытое решение, без него срез не начинается.
  - **S4-38L7v — повторные сегментные префиксы CS/DS/ES/SS (`2E/3E/26/36`) перед обычным опкодом: принято после ревью** (ветка `feat/s4-38l7v-noop-segment-prefixes`, коммиты `d946bd1`, `950810e` и коммит с результатом прогона). Парсер префиксов не считает `duplicate` повтор `26/2E/36/3E` после такого же префикса (в 64-битном режиме базы нулевые; сохраняется последний префикс; лимит 15 байт: 13 префиксов + `31 C0` принимается, больше — `#GP(0)`). Остаются отказами: повтор и сочетания с `64/65`, повтор `66/67/F0/F2/F3`. Побочное: `2E 2E 0F 1F/18` теперь принимаются (тесты отказов переведены на `64 64`). SDM по первоисточнику не сверялась (инженерное решение). Диагностика (НЕ засчитывается; `s4-38l7v-diagnostic-run.json`): retired 1184860323 (+3025594), стоп `Unsupported{opcode: 0xef}` (`out %eax,%dx`, порт `0xcf8`) на `0xffffffff826dedb2`; CR3 `0x3ffe000`, CR4 `0xb0`, гостевой EFER `0xd01`. **Следующая граница:** `OUT DX, EAX` / `IN AX, DX` (PCI-конфигурация, порты `0xCF8/0xCFC`) в наборе CPL0.
  - **S4-38L7t — `LOCK BTR r/m64, r64` (`F0 REX.W 0F B3 /r`, memory) в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7t-lock-btr`, коммиты `4001c95`, `28b2d88`, слияние `222c2e7`; ревью Opus: принять). Операция `LockBitMem { kind: BitKind, base, source }` заменила `LockBtsMem`: `Set` (0F AB) даёт `old | mask`, `Reset` (0F B3) — `old & !mask`; адресация битовой строки, каноничность, read → write всегда, CF после успешной записи, flag_effects и гейт общие. Тесты LOCK BTS: из списка отказов убрана `f0 48 0f b3 03` (теперь принимается как BTR). Не-LOCK `0F B3` не сужен (Operation::Bit); LOCK BT/BTC (0F A3/0F BB) и LOCK 0F BA отказывают со своими опкодами. Тесты: `cpl0_profile_set` 483 passed, `od-isa --lib` 2217 passed, `od-machine --lib` 869 passed (6 ignored). Замечания ревью (косметика): нет теста на алиасинг регистра смещения и базы для BTR (есть для BTS); комментарий в `execute.rs` перенесён некорректно; документный коммит назван «accepted» до ревью (сложившийся порядок). **Диагностика S4-38L7t (НЕ засчитывается; evidence `s4-38l7t-diagnostic-run.json`).** Retired 1180654930 (+43113987 после BTR). Стоп — `Unsupported{opcode: 0x8c}` на `0xffffffff813adf2c`, байты `8c d0` = `MOV %ss, %eax` (`8C /r`, Sreg в r/m32). CR3 = `0x3ffe000`, CR4 = `0xb0`, гостевой EFER `0xd01`, RFLAGS = `0x46`; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `MOV r/m, Sreg` (`8C /r`) в наборе CPL0 — ядро читает селектор SS (затем CS, DS и т. д.) при построении кадра прерывания.
  - **S4-38L7s — `LOCK OR r/m8, imm8` (`F0 [REX] 80 /1 ib`), `LOCK OR r/m32/64, imm` (`F0 [REX.W] 81 /1`, `83 /1`), memory, в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7s-lock-or-byte`, коммиты `b95e90e`, `2cfd683`, `217ad43`, слияние `41ab418`; ревью Opus: принять). Одна операция `LockOrMem { destination, immediate }`, ширина из `destination.size`, immediate расширяется как в `binary_operation`. Порядок: каноничность до доступа (#GP(0); RSP/RBP — #SS(0)) → read → `logic_result(size, Or)` → write всегда → флаги после записи; CF=OF=0, SF/ZF/PF по результату, AF сохраняется. Не-LOCK `80/81/83 /1` не сужены (исполняются как прежде). Отказы LOCK для `/0`, `/2–/7`, mod=11, 66 (кроме 66+81 — word), 67, сегментных префиксов и REX не перед опкодом — с опкодом группы без мутации. Замечание ревью: эти отказы теперь происходят после чтения ModRM, поэтому ошибка выборки ModRM на границе страницы даёт #PF выборки раньше `Unsupported` (как у LOCK INC); `length` в `Unsupported` на 1 больше прежнего, exit-опкод не меняется. Тесты: `cpl0_profile_set` 467 passed, `od-isa --lib` 2212 passed, `od-machine --lib` 868 passed (6 ignored). Флаги: `0x01|0x02=0x03` → PF=1 (две единицы), постановка ошибочно ждала PF=0. **Диагностика S4-38L7s (НЕ засчитывается; evidence `s4-38l7s-diagnostic-run.json`).** Retired 1137540943 (+6849 после LOCK OR). Стоп — `Unsupported{opcode: 0xb3}` на `0xffffffff813c584d`, байты `f0 4d 0f b3 65 30` = `LOCK BTR %r12, 0x30(%r13)` (`F0 REX.WRB 0F B3 /r`, длина 6). CR3 = `0x3ffe000`, CR4 = `0xb0`, гостевой EFER `0xd01`, RFLAGS = `0x92`; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `LOCK BTR m64, r64` (`F0 REX.W 0F B3 /r`, memory) — парный к LOCK BTS (S4-38L7n).
  - **S4-38L7r — `TEST r/m16, imm16` (`[64|65] 66 F7 /0 iw` и `66 [64|65] F7 /0 iw`, memory) с FS/GS в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7r-gs-test-word`, коммиты `4b560eb`, `d8576c6`, слияние `5a8cc6d`; ревью Opus: принять, три замечания — только комментарии и формулировки, не блокируют). Первая word-ширина с FS/GS. Оба порядка префиксов (65 66 и 66 65) принимаются, декодер собирает префиксы независимо от порядка; повторный 66, второй сегментный префикс и устаревший REX отказывают с 0xf7. Длина включает imm16 (граничная форма — 10 байт), RIP-relative от RIP+10. Адрес = база + EA, каноничность начала и адреса+1 до чтения (#GP(0); для RSP/RBP — #GP). Один `read_integer(2)`, флаги через существующую `logic_result(Word, Test)`: CF=0, OF=0, SF по биту 15, ZF по слову, PF по младшему байту, AF не определён. Запись в память нет. REX.W вместе с 66 — отказ 0xf7 (по SDM REX.W имеет приоритет, но решение консервативное и не угадывает приоритет). Регистровая форма `66 F7 C3 iw` исполняется как обычный TEST r16 (как у S4-38L7o/p/q). Тесты (main после слияния): `cpl0_profile_set` 453 passed, `od-isa --lib` 2207 passed, `od-machine --lib` 867 passed (6 ignored), `paged_probe_profile` 28 passed. **Замечания ревью (косметика, для следующего касания файлов):** комментарий в `execute.rs` (тест bare run/step) говорит «`[64|65] F6 /0 ib`» вместо «`[64|65] 66 F7 /0 iw`»; комментарий фасадного теста про флаги первого TEST не проверяется (значения верны, следующий ADD их перезаписывает); комментарий в `long_decode.rs` «мы не угадываем приоритет» — по SDM приоритет определён (REX.W, 66 игнорируется), декодер так и считает. **Диагностика S4-38L7r (НЕ засчитывается; evidence `s4-38l7r-diagnostic-run.json`).** Retired 1137534094 (+43347 после TEST). Стоп — `Unsupported{opcode: 0x80}` на `0xffffffff813c0fcc`, байты `f0 80 4b 3d 80` = `LOCK OR m8, imm8` (`F0 80 /1 ib`, без сегментного префикса). CR3 = `0x3ffe000`, CR4 = `0xb0`, гостевой EFER `0xd01`, RFLAGS = `0x46`; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `LOCK OR m8, imm8` (`F0 80 /1 ib`, memory, без сегментного префикса) в наборе CPL0.
  - **S4-38L7q — `DEC r/m32` и `DEC r/m64` (`[64|65] [REX.W] FF /1`, memory) с FS/GS в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7q-gs-dec-dword`, коммиты `b014395`, `df42521`, слияние `1d9b055`; ревью Opus: принять, замечания косметические). Декодер принимает `/0` и `/1` одной веткой: `IncDecFsMem`/`IncDecGsMem` с полем `kind` (Inc | Dec); арм исполнения общий, новой арифметики нет (`arithmetic_result(size, kind)`). Каноничность до доступа (#GP(0); RSP/RBP — #GP), read → `arithmetic_result` → write, флаги после записи; CF сохраняется, OF только при 0x80000000→0x7fffffff. Регистровая форма `65 FF CB` исполняется как обычный DEC r32 (как INC в S4-38L7p). Устаревший REX перед FF — отказ для обеих ширин; `FE /1` с FS/GS — отказ 0xfe. Тесты INC обновлены только по форме (шаблон с `kind`) и убраны два кейса `65/64 ff 0b`, которые теперь принимаются как DEC. Тесты (main после слияния): `cpl0_profile_set` 433 passed, `od-isa --lib` 2202 passed, `od-machine --lib` 866 passed (6 ignored), `paged_probe_profile` 27 passed. **Диагностика S4-38L7q (НЕ засчитывается; evidence `s4-38l7q-diagnostic-run.json`).** Retired 1137490747 (+6 после DEC). Стоп — `Unsupported{opcode: 0xf7}` на `0xffffffff815167b4`, байты `65 66 f7 05 73 68 f0 02 01 ff` = `TESTW $0xff01, %gs:disp32(%rip)` (TEST m16, imm16, GS, префикс 66). CR3 = `0x3ffe000`, CR4 = `0xb0`, гостевой EFER `0xd01`, RFLAGS = `0x2`; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `TEST m16, imm16` с GS и префиксом 66 (`65 66 F7 /0 iw`). Это первая word-ширина с FS/GS: порядок префиксов `65 66` / `66 65`, чтение и запись word, каноничность адреса+1.
  - **S4-38L7p — `INC r/m32` и `INC r/m64` (`[64|65] [REX.W] FF /0`, memory) с FS/GS в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7p-gs-inc-dword`, коммиты `bf4ce10`, `bfa6942`, `570f1d5`, `2e2ce7f`, слияние `ffd480d`; ревью Opus: принять с исправлениями, исправления — тесты и формулировка в `2e2ce7f`). Новые `IncFsMem`/`IncGsMem`: адрес = база + EA, каноничность до доступа (#GP(0); для RSP/RBP — #GP), `read_integer` → общая `arithmetic_result(size, Inc)` → `write_integer`, флаги после успешной записи; CF сохраняется, OF/SF/ZF/AF/PF по результату. qword (REX.W) — тем же путём. Регистровая форма `65 FF C3` не отказывается (исполняется как обычный INC r32, решение зафиксировано тестом и документом). Устаревший REX перед FF отказывается для обеих ширин: для qword это осознанное расхождение с S4-38L7i (там qword-`8B` с таким REX принимается). REX.X-индекс покрыт тестом. Тесты (main после слияния): `cpl0_profile_set` 412 passed, `od-isa --lib` 2197 passed, `od-machine --lib` 865 passed (6 ignored), `paged_probe_profile` 26 passed. **Диагностика S4-38L7p (НЕ засчитывается; evidence `s4-38l7p-diagnostic-run.json`).** Retired 1137490741 (+53 после INC). Стоп — `Unsupported{opcode: 0xff}` на `0xffffffff814f6cc5`, байты `65 ff 0d 64 63 f2 02` = `DEC dword %gs:disp32(%rip)` (`FF /1`, без LOCK). CR3 = `0x3ffe000`, CR4 = `0xb0`, гостевой EFER `0xd01`, RFLAGS = `0x46`; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `DEC m32` с GS (`65 FF /1`), тот же путь, что `IncGsMem`.
  - **S4-38L7o — `TEST m8, imm8` (`[64|65] F6 /0 ib`, memory) с FS/GS в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7o-gs-test-byte`, коммиты `7885427`, `32fb666`, слияние `84dc9f0`; ревью Opus: принять). Новые `TestFsMemByte`/`TestGsMemByte`: длина включает imm8, RIP-relative от RIP следующей инструкции. Адрес = база + EA, каноничность одного байта до чтения (#GP(0); для RSP/RBP — #GP). Флаги через общую `logic_result(Byte, Test)`: CF=0, OF=0, SF/ZF/PF по байту, AF не определён (не меняется). Запись в память не делается. Регистровая форма `65 F6 C3 ib` не отказывается и исполняется как раньше (решение зафиксировано тестом и в документе). REX перед F6 допустим, REX.B/X влияют на базу и индекс; устаревший REX отказывается консервативно, как dword в S4-38L7m. Необязательное замечание ревью: тест на REX.X-индекс не добавлен. Тесты: `cpl0_profile_set` 392 passed, `od-isa --lib` 2192 passed, `od-machine --lib` 864 passed (6 ignored), `paged_probe_profile` 25 passed. **Диагностика S4-38L7o (НЕ засчитывается; evidence `s4-38l7o-diagnostic-run.json`).** Retired 1137490688 (+26 после TEST). Стоп — `Unsupported{opcode: 0xff}` на `0xffffffff814f6cb6`, байты `65 ff 05 73 63 f2 02` = `INC dword %gs:disp32(%rip)` (без LOCK, без REX.W). CR3 = `0x3ffe000`, CR4 = `0xb0`, гостевой EFER `0xd01`; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `INC m32` с GS (`65 FF /0`, dword, memory, без LOCK) в наборе CPL0.
  - **S4-38L7n — `LOCK BTS m64, r64` (`F0 REX.W 0F AB /r`, memory) в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7n-lock-bts`, коммиты `884cfcf`, `3119522`, слияние `a14174b`; ревью Opus: принять; необязательное замечание — добавить тест «неканонический EA, канонический вычисленный адрес», не блокирует). Новая `LockBtsMem`: битовое смещение — знаковое значение r64; адрес записи = EA + `(offset >> 6) * 8` (арифметический сдвиг), бит = `offset & 63`. Каноничность вычисленного адреса до доступа (#GP(0); для баз RSP/RBP — #SS(0), так как обращение идёт через SS). Порядок: read qword → установка бита → write всегда → CF = старый бит, только после успешной записи. Флаги: определён CF; ZF не меняется, PF/AF/SF/OF не определены (как у `Operation::Bit`). Не-LOCK `0F AB` и `0F BA` не сужены: у них нет операции, которой не хватает адаптеру. Тесты: `cpl0_profile_set` 373 passed, `od-isa --lib` 2187 passed, `od-machine --lib` 863 passed (6 ignored). **Диагностика S4-38L7n (НЕ засчитывается; evidence `s4-38l7n-diagnostic-run.json`).** Retired 1137490662 (+399 после LOCK BTS). Стоп — `Unsupported{opcode: 0xf6}` на `0xffffffff815166fe`, байты `65 f6 05 2c 69 f0 02` = `TEST m8, imm8` с GS (`testb $0xf0, %gs:disp32(%rip)`). CR3 = `0x3ffe000`, CR4 = `0xb0`, гостевой EFER `0xd01`, RFLAGS = `0x92`; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `TEST m8, imm8` с префиксом GS (`65 F6 /0 ib`) в наборе CPL0.
  - **S4-38L7m — загрузка dword `MOV r32, m32` (`[64|65] 8B /r` без REX.W) с FS/GS в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7m-gs-load-dword`, коммиты `067cc4f`, `c873e60`, `c77fb91`, `c4043de`, слияние `67faaa6`; ревью Opus: принять с исправлениями). Новые `MovFsLoadD`/`MovGsLoadD`: адрес = база + эффективный адрес, каноничность начала и адреса+3 до чтения (#GP(0), для RSP/RBP — #GP, не #SS), один `read_integer` размера 4, запись с обнулением старших 32 бит (в т.ч. r8–r15), флаги не меняются. Точная форма `64|65 48 8B 00` и qword-формы S4-38L7i не менялись. Сознательное сужение: dword-форма с REX, не стоящим непосредственно перед опкодом, отказывается с 0x8b (по SDM такой REX игнорируется, поэтому это консервативный отказ, не исправление); qword с таким REX принимается как раньше. По ревью исправлены комментарий и документ. Тесты: `cpl0_profile_set` 357 passed, `od-isa --lib` 2182 passed, `od-machine --lib` 862 passed (6 ignored). **Диагностика S4-38L7m (НЕ засчитывается; evidence `s4-38l7m-diagnostic-run.json`).** Retired 1137490263 (+2 после загрузки). Стоп — `Unsupported{opcode: 0xab}` на `0xffffffff83f52ac4`, байты `f0 48 0f ab 05 13 7c f7 ff` = `LOCK BTS %rax, -0x883ed(%rip)` (LOCK BTS m64, r64). CR3 = `0x3ffe000`, CR4 = `0xb0`, гостевой EFER `0xd01`; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `LOCK BTS m64, r64` (`F0 REX.W 0F AB /r`, memory).
  - **S4-38L7l — `LOCK INC m32/m64` (`F0 [REX.W] FF /0`, memory) в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7l-lock-inc`, коммиты `62c6d68`, `2d31c67`, слияние `2e56532`; ревью Opus: принять, замечания косметические). Новая `LockIncMem`: адрес без сегментной базы, каноничность до доступа (#GP(0); для RSP/RBP — #SS(0)), read → `arithmetic_result(Inc)` → write, затем флаги и RIP только после успешной записи. Флаги OF/SF/ZF/AF/PF, CF сохраняется. qword-форма (`F0 REX.W FF /0`) реализована тем же путём и задокументирована как расширение среза. Обычный `FF /0` без LOCK в наборе не сужен (исполняется через адаптер, в отличие от не-LOCK CMPXCHG). Тесты: `cpl0_profile_set` 339 passed, `od-isa --lib` 2177 passed, `od-machine --lib` 860 passed (6 ignored). **Диагностика S4-38L7l (НЕ засчитывается; evidence `s4-38l7l-diagnostic-run.json`).** Retired 1137490261 (+2043). Стоп — `Unsupported{opcode: 0x8b}` на `0xffffffff83f52aba`, байты `65 8b 05 73 a5 4c 00` = `MOV %gs:disp32(%rip), %eax` (загрузка dword с GS). Следом `f0 48 0f ab 05` = `LOCK BTS m64, r64`. CR3 = `0x3ffe000`, CR4 = `0xb0`, гостевой EFER `0xd01`; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** GS/FS-загрузка dword (`65 8B /r`); S4-38L7i покрывал только qword. Замечание на будущее: `F0 FF` в конце страницы кода теперь декодирует ModRM до отказа (ошибка выборки вместо Unsupported), как и у XADD/CMPXCHG-кандидатов.
  - **S4-38L7k — `LOCK CMPXCHG m64, r64` (`F0 REX.W 0F B1 /r`, memory) в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7k-lock-cmpxchg`, коммиты `11dda1b`, `cd2f7a8`, слияние `e0871e0`; ревью Opus: принять). Новая `LockCmpxchgMem`: адрес без сегментной базы, каноничность начала и последнего байта до доступа (#GP(0); для RSP/RBP — #SS(0)), read → write (всегда: src при равенстве, observed при неравенстве), затем флаги `CMP RAX, dest` и RAX только после успешной записи. Флаги через общую `arithmetic_result(Qword, Cmp)`. Не-LOCK форма `48 0F B1` в наборе CPL0 теперь даёт `Unsupported{0xb1}` (сознательное сужение; вне набора прежнее поведение). Тесты: `cpl0_profile_set` 325 passed, `od-isa --lib` 2172 passed, `od-machine --lib` 859 passed (6 ignored). **Замечание на будущее (ревью):** `XCHG` и `XADD` с LOCK в наборе CPL0 пока не сужены — проходят до атомарного вызова адаптера платы и заканчиваются `memory_error`/Backend, а не `Unsupported`. Выровнять, когда дойдёт до этих форм. **Диагностика S4-38L7k (НЕ засчитывается; evidence `s4-38l7k-diagnostic-run.json`).** Retired 1137488218 (+267). Стоп — `Unsupported{opcode: 0xff}` на `0xffffffff815ad6b7`, байты `f0 ff 43 58` = `LOCK INC dword 0x58(%rbx)`. CR3 = `0x3ffe000`, CR4 = `0xb0`, гостевой EFER `0xd01`; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `LOCK INC m32` (`F0 FF /0`, dword, memory) в наборе CPL0.
  - **S4-38L7j — `SUB r64, m64` (2B) с префиксом FS/GS через базу в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7j-gs-sub`, коммиты `5099b4e`, `4051041`, `a115d9a`, слияние `943ce17`; ревью Opus: принять; замечание про регистровую форму mod = 11 зафиксировано в `CPU_CONTRACT.md` как сознательное сужение, код не менялся). Новые `SubFsMem`/`SubGsMem`: адрес = база + эффективный адрес, один `read_integer`, результат и флаги — через ту же `arithmetic_result(Qword, Sub)`, что у не-префиксной формы. Неканоническое начало или последний байт → #GP(0) до чтения. Вне набора — `Unsupported{0x2b}`; dword, REX без W, LOCK/REP, `66`, `67` и регистровая форма — отказ со своим опкодом. Тесты: `cpl0_profile_set` 310 passed, `od-isa --lib` 2167 passed, `od-machine --lib` 857 passed (6 ignored). **Диагностика S4-38L7j (НЕ засчитывается; evidence `s4-38l7j-diagnostic-run.json`).** Retired 1137487951 (+9262 после границы S4-38L7i). Стоп — `Unsupported{opcode: 0xb1}` на `0xffffffff8273441c`, байты `f0 48 0f b1 13` = `LOCK CMPXCHG %rdx, (%rbx)`. CR3 = `0x3ffe000`, CR4 = `0xb0`, гостевой EFER `0xd01`; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** `LOCK CMPXCHG r/m64, r64` (`F0 48 0F B1 /r`, memory-форма) в наборе CPL0 — отдельный срез по этой границе.
  - **S4-38L7i — загрузка `MOV r64, m64` с префиксом FS/GS через базу в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7i-gs-load`, коммиты `b6d1b8e`, `ae1996d`, `92f8cab`, `72b3b04`, `ee98d08`, слияние `502c15a`; ревью Opus: принять с исправлениями — точные опкоды в тесте отказа (`8B`, а `8A` — `8A`), тест «RSP/RBP-база → #GP(0), не #SS» для загрузки и формулировка про `8A` исправлены). Форма `[64|65] REX.W 8B /r` с memory-операндом (RIP-relative, SIB, disp, приёмник r0–r15 через REX.R); точная старая форма `64|65 48 8B 00` осталась `MovFsLoad`/`MovGsLoad`, широкая — `MovFsLoadQ`/`MovGsLoadQ`. Адрес = база + эффективный адрес, неканоническое начало или последний байт → #GP(0) до чтения. Вне набора или вне CPL0 — `Unsupported{0x8b}` без мутации; `8A` — `Unsupported{0x8a}`. Тесты: `cpl0_profile_set` 293 passed, `od-isa --lib` 2162 passed, `od-machine --lib` 855 passed (6 ignored). Код написан Sonnet `high`, как требует `AGENTS.md`. **Диагностика S4-38L7i (НЕ засчитывается; evidence `s4-38l7i-diagnostic-run.json`).** Retired 1137478689 (+141 после загрузки `MOV r12, %gs:disp(%rip)`). Стоп — `Unsupported{opcode: 0x2b}` на `0xffffffff83f25fd7`, байты `65 48 2b 05 29 70 4f 00` = `SUB RAX, %gs:disp(%rip)`. CR3 = `0x3ffe000`, CR4 = `0xb0`, гостевой EFER `0xd01`; `memory_error` и `page_fault` отсутствуют. **Следующая граница:** арифметика `SUB r64, m64` с GS-префиксом (`2B /r`) — отдельный срез или обобщение ALU с FS/GS (решение по этой границе).
  - **S4-38L7h — `MOV CR3, r64` из любого регистра в наборе CPL0: принято после ревью** (ветка `feat/s4-38l7h-cr3-any-reg`, коммиты `ac0ae2e`, `50c4e6b`, `256e5a5`, `4f4444f`, слияние `13a5b44`; ревью Opus: принять с исправлениями — тесты отказа новых форм в `mov_cr3_profile` и paged-probe добавлены в `256e5a5`, формулировка про `mod ≠ 11` исправлена в `4f4444f`). Форма `0F 22` с ModRM mod = 11, reg = 3, любой rm с REX.B; REX.R недопустим; LOCK, 66, 67, REP и память — отказ без мутации. Значение проверяется как в принятой RAX-форме (биты выше 35 → #GP(0) до мутации), затем тот же эффект `Cr3Write` (перестройка walker, инвалидация кэша выборки S4-13). Вне набора или без `cpu_owned_cr3` — `Unsupported{0x22}`. Mod ≠ 11 отказан консервативно; поведение аппаратуры по SDM не сверялось. Тесты (`cpl0_profile_set`, `od-isa --lib`, `od-machine --lib`): 277 / 2157 / 853 passed, 0 failed. **Код написан моделью Haiku вместо Sonnet, как требует `AGENTS.md`** — нарушение процесса, ревью проверило код строже, дефектов не нашло.
  - **Диагностика S4-38L7h (НЕ засчитывается; `acceptance: diagnostic_not_counted`; evidence `s4-38l7h-diagnostic-run.json`).** Release, `--diagnostic-past-c7`, бюджет 3·10⁹: `loaded`, retired 1137478548 (+756 после `MOV CR3, RDI`), `memory_error` и `page_fault` отсутствуют. Стоп — `Unsupported{opcode: 0x8b}` на `0xffffffff83f25c74`, байты `65 4c 8b 25 dc 1a 51 00` = `MOV r12, %gs:disp(%rip)` (загрузка qword с префиксом GS). **Следующая граница:** загрузка `MOV r64, m64` с GS-префиксом, RIP-relative. CR3 = `0x3ffe000`, CR4 = `0xb0`, гостевой EFER `0xd01`. Причину отказа набора в S4-38L7h не расследовали: следующий срез — загрузка GS, как S4-38L7g была запись.
  - **S4-38L7g — запись qword `MOV m64, r64` с префиксом FS/GS через базу в наборе: принято после ревью** (ветка `feat/s4-38l7g-gs-store`, коммиты `69c2302`, `0485f0d`, слияние `6549713`; ревью Opus: принять с исправлениями — устаревший комментарий и точные опкоды в тесте отказов исправлены в `0485f0d`). Форма `[64|65] REX.W 89 /r` с любой memory-адресацией; адрес = база FS/GS + эффективный адрес; неканоническое начало или последний байт → #GP(0) без записи; вне набора или вне CPL0 — `Unsupported{0x89}` без мутации. Префиксы CS/DS/ES/SS на этой форме база игнорируют (по памяти SDM 64-бит, первоисточник не сверялся). Тесты: `cpl0_profile_set` 270 passed; `od-machine --lib` 852 passed, 6 ignored. Открыто minor: тест на пересечение 2^64 при записи (`AddressOverflow`), проверка цепочки REX (`65 41 48 89`), CPL3 гейт (ограничение, не ошибка).
  - **Диагностика S4-38L7g (НЕ засчитывается; `acceptance: diagnostic_not_counted`; evidence `s4-38l7g-diagnostic-past-gs-store-run.json`).** Прогон release, `--diagnostic-past-c7`, бюджет 3·10⁹: `loaded`, retired 1137477792 (+17 инструкций после записи `65 48 89 05 …`), `memory_error` и `page_fault` отсутствуют. Стоп — `Unsupported{opcode: 0x22}` на `0xffffffff82728f60`, байты `0f 22 df` = `MOV CR3, RDI` (RDI = `0x3ffe000`). Регистры на остановке: RAX = 0, RBX = `0x100000`, RSP = `0xffffffff83a03f08`, CR0 = `0x80050033`, CR4 = `0xb0`, walker EFER `0xd00`, гостевой EFER `0xd01`. **Следующая граница:** набор не принимает `MOV CR3, RDI` при CR4 = `0xb0` — причина не выяснена, следующий срез — выяснить её до расширения набора.
  - **Диагностика за C7 (S4-38L7, НЕ засчитывается; `acceptance: diagnostic_not_counted`).** Прогон с `--diagnostic-past-c7` и чтением CR4 в любой регистр (S4-38L7b, слито `4e3f1b1`): `CPUID` leaf 1 исполняется ответом DiagnosticV1, `MOV RDX, CR4` исполняется; retired 1137477598 (+3240 после C7). Следующая граница — запись CR4: `0F 22 E1` (`MOV CR4, RCX`, RCX=0x30 = PSE|PAE) по виртуальному адресу `0xffffffff8139c0c0` (ядро уже переключилось на свои таблицы: CR3 `0x3ffe000`, cr3_changes 2). Модель CR4 набора — база 0x20 (PAE), записи CR4 нет (Unsupported). Evidence: `s4-38l7-diagnostic-past-c7-run.json`. Следующий срез — S4-38L7c: запись CR4 в наборе с минимальной политикой (PSE добавлен; прочие биты — Unsupported до их срезов).
  - **C7 достигнут на настоящем ядре (2026-11-11).** Release-прогон (бюджет 3·10⁹): retired 1137474358, `cpuid_divergence` на `0x139_c287` — `CPUID` leaf 1 (C7 плана): ответ DiagnosticV1 расходится с Linux-профилем, стоп честный, ответ не подставляется. Цепочка до C7 пройдена через S4-38L6a–d: PREFETCH как NOP, WRMSR FS/GS base, непустая загрузка SS/DS/ES/FS/GS, LEAVE (`29907ae` слито в `main`; ревью Opus принято, minor — тесты неканонического последнего байта и гейта CPL3). Evidence: `s4-38l6d-cpuid-c7-run.json`. **Дальше за C7** — только диагностический opt-in (D4, не засчитывается) или решение о профиле целевого CPU (решение пользователя).
  - **C5 достигнут (2026-11-11).** Release-прогон (`./target/release/od-runner`, бюджет 3·10⁹): retired 1137474216 — первая архитектурная остановка в пределах распакованного ядра на `0x3f253f3`, сразу за `e_entry` 0x3f2_53e0: `WRMSR` IA32_GS_BASE (RCX=0xC0000101, EDX:EAX=0). **S4-38L6b — WRMSR/RDMSR FS/GS base в наборе: принято после ревью** (слияние `702a772` ветки; ревью Opus: принять; открыто minor — тесты мусора в старших битах RAX/RDX и ненулевая база на фасаде платы). Следующая граница: `MOV SS, AX` с непустым селектором 0x18 на `0x3f13304` (retired 1137474227) — S4-38L6c: непустая загрузка сегмента в наборе. Темп release ≈2,8 млн инструкций/с (406 с на 1.1e9).
  - **S4-38L6a — предвыборка `0F 18 /0..7 m` как NOP в наборе: принято после ревью** (слияние `0b39475`; ревью Opus: принять; открыто minor — тест обрезанного операнда и повтор 66 в наборе). Прогон 1·10⁹ инструкций (кэш S4-13 включён): `budget_exhausted` на `0x543_4cd3`, retired 1000000000, 25 доставок `#PF`, архитектурной остановки не найдено (evidence `s4-38l6a-prefetch-1e9-run.json`; до префетча стоп был на `0x542_482d` при retired 268756903). Темп debug-сборки ≈370 тыс. инструкций/с (≈45 мин на 1e9). Следующее: release-сборка раннера и прогон до 3·10⁹ (граница D3).
  - **Ускорение (S4-13, решение 12, 2026-11-10/11).** Кэш трансляции выборки в наборе (`88ca8aa`, ревью Opus принято с правками `6da1a5c`, слияние `34828d4`): 200 000 000 инструкций — 554 с user (было 2030 с, ≈3,7×), стоп тот же (`budget_exhausted`, `0x543_4e32`, retired 200M); контрольная точка 30M — ≈87 с (было ≈291 с). Evidence: `s4-13-fetch-cache-200m-run.json`. Остаётся: следующая граница за 200M не найдена — нужен бюджет ≥1·10⁹.
  - **Измерение (2026-11-10, main `2525843`).** Бюджет 200 000 000 инструкций: `budget_exhausted` на `0x543_4e32`, retired 200000000, без архитектурной остановки; 2030 с пользовательского CPU (≈98 тыс. инструкций/с) — evidence `s4-38l6-budget-200m-run.json`. Контрольная точка 30 000 000: `budget_exhausted` на `0x543_4d78` — ignored-тест `real_artifacts_checkpoint_at_budget_30m_past_the_qword_read` (≈294 с). По D3 (≤3·10⁹ инструкций и порог времени) при этом темпе бюджет до C5/C7 не укладывается в 2 ч: решение 12 (ускорение декодера/исполнения, S4-13 раньше) — за пользователем.
- [ ] **L7 — Цикл «стоп → задача этапа 2» до C7 (протокол).** На каждом стопе: stop record, минимальный repro, задача в STAGE_2.md. После принятия — повторный прогон; ранее пройденные checkpoints неизменны. Ожидаемые блокеры этапа 2 (гипотезы по исходникам, подтверждает L2): MOV SS null + тень; MOV FS/GS null на CPL0; LGDT/LIDT в общих адресных формах; LRETQ в общем цикле; PUSHFQ/POPFQ на CPL0; чтение CR3/CR2; запись CR4 с политикой reserved-битов; STD + REP MOVSQ при DF=1; `F3 0F BC` (BSF без BMI1), если используется; WRMSR GS_BASE на CPL0; ненулевые загрузки сегментов на CPL0.
- [ ] **L8 — IRQ и время на Linux-плате (L; после L3, от CPUID не зависит; может делиться на a–d).** Доставка IRQ в сессии и общий paged pump backend; мост PIT → INTIN2 (ISO 0→2); paged APIC timer; чтение MMIO APIC гостем; STI/CLI (ISSUE-069 — семантика этапа 2); `Ring(n)` с `dropped` (ISSUE-072). Исполнимые fixtures: `quick_pit_calibrate` с MSB-циклом, UIP-цикл CMOS через RDTSC (≤ 2·10⁷ инструкций, решение 16б), PIT mode 2 IRQ0 → pin 2 → EOI → IRETQ на 1000+ периодов, STI;HLT wake. Инъекции: ISO проигнорирован → проверка падает; без EOI второй IRQ не доставлен; тик выключен → budget; Ring с лимитом → TraceLimit. Evidence: `s4-38l8-irq-time.json`.
- [ ] **L9 — Discovery и время ядра после C7 (M, блокер этапа 2).** `CpuidProfile` по `od-x86-64-v1-neutral-v0.1` и исполнение всего, что он объявляет (x87/FXSR/SSE/SSE2, PGE, PAT, NX, SYSCALL-MSR, EFER). Проверка по dmesg через earlycon: `BIOS-e820` совпадает с S4-37 L6; ECAM reserved; `ACPI: RSDP 0x00000000000E0000`; `Command line:` точно совпадает с cmdline + `nokaslr`; один CPU; APIC/IOAPIC маршруты; TSC/PIT-калибровка без hang; `check_timer` через pin 2; legacy-устройства (0x60/0x64, PIT mode 4) не трогаются. `KASLR disabled` печатает распаковщик, он молчит — маркер переносится в checkpoint.
- [ ] **L10 — Initramfs и exec init (M, блокер этапа 2).** `Unpacking initramfs`, `Freeing initrd memory`, `Run /init as init process`, `kernel_execve` (C17), первый CPL3 fetch на e_entry `/init` из артефакта S4-36. Это «Готово» S4-38. Негатив: initramfs без `/init` → `No working init found`, panic различима, не pass. Evidence: `s4-38l10-exec-init.json`.

- **Решения пользователя (2026-11-10):** D1 = вариант B (ядро исполняется через общий путь этапа 2 поверх page-aware памяти платы); D2 = да, L3–L8 начинаются до приёмки S4-33. D3–D5 — по рекомендациям плана.
- **Статус L3 (2026-11-10): заблокирован этапом 2.** Общего пути с комбинированными профилями в `od-isa` нет (публичные входы принимают один профиль за вызов; CR0/CR4-профили жёсткие). Блокер записан в `STAGE_2.md` как S2-45 (комбинированный вход CPU-профилей). Пробы не меняли поведение; реальный первый стоп остаётся CLD на `0x100_0200`.
- **Решение пользователя (2026-11-11, CPU-профиль гостя для L9/L10): вариант A — минимальный базовый профиль.** Гостю объявляются только фичи, которые эмулятор реализует; ответ `CPUID` (leaf 1 и прочие) задаётся явно и не копирует хост-CPU. Конкретный состав фич — отдельным срезом до L9.
- **Открытые решения пользователя:**
  - **D1.** Как ядро исполняется на paged-плате. Рекомендация — вариант B (`run_configured` этапа 2 поверх paged `GuestMemory`), расширение allowlist отклонить. Где собирать системные точки входа — в `od-isa` подсрезом S4 или отдельной задачей STAGE_2.md. Нужно для L3.
  - **D2.** Начинать L1–L8 до приёмки S4-33 (формальная зависимость S4-38). Рекомендация — да.
  - **D3.** Лимиты bounded run: ≤ 3·10⁹ инструкций до C7 и порог времени хоста (2 ч до C5), после которого S4-13 TLB раньше.
  - **D4.** Диагностическое продолжение за C7 на DiagnosticV1 — только opt-in, не засчитывается. Цель — инвентаризация форм для этапа 2.
  - **D5.** `earlyprintk` не добавлять (рекомендация): cmdline S4-29g зафиксирована, свидетельства из RIP/RAM-checkpoints.
  - **D6.** Приоритеты этапа 2 из L7 — в плане этапа 2.
- **Принимается самостоятельно (обоснование в срезе):** флаг `--run` и схема stop record; 2 MiB walker для Linux-платы; checkpoints как предикаты; host-zstd как оракул; стоп по расхождению CPUID; `Ring(n)` с `dropped`; статическая инвентаризация как вход планирования.
- **Зависимости от этапа 2 (критический путь):** формы из L7 до C7; CPU-профиль Linux и объявленные им возможности (L9/L10); политика reserved-битов CR4/EFER; семантика STI/CLI (ISSUE-069). До C7 CPUID-профиль Linux не нужен.
- **Оговорки:** формальная зависимость S4-38 → S4-33 не выполнена (S4-33 открыт); связка vmlinuz↔vmlinux не проверена (S4-35 не распаковывал zstd).

### S4-39. Подтвердить Linux userspace

- [ ] Принято после ревью.
- **Где:** `tests/system/`, init fixture, report/data.
- **Сделать:** запустить точный kernel+initramfs с новым nonce. Зафиксировать выполнение `/init` в CPL3, syscall entry/return, вывод nonce через guest console/UART, работающий timer interrupt и продолжение user code после него.
- **Проверка:** минимум повторный запуск после reset; ожидаемые markers и machine observations связаны с одним run ID и artifact hashes. Timeout, guest panic, shutdown до init, неподдержанная инструкция и неправильный nonce не проходят gate.
- **Готово:** реальный Linux userspace подтверждён на interpreter path; JIT coverage указывается отдельно. Нельзя выдать полностью fallback-run за доказательство работы JIT memory integration.

### S4-40. Проверить, что boot gate не даёт ложный успех

- [ ] Принято после ревью.
- **Где:** `tests/system/`, runner result parser.
- **Сделать:** отрицательные cases: отсутствующий init, неверный nonce, corrupted boot artifact, запрещённый overlap, отключённая необходимая IRQ route, timeout/STOP до completion. Различать invalid configuration, guest failure и external cancellation.
- **Проверка:** каждый case действительно падает с ожидаемым классом результата; старый serial log не принимается за новый запуск; text marker без CPL3/syscall evidence недостаточен.
- **Готово:** boot success означает только полностью выполненный положительный сценарий S4-39.

### S4-41. Подключить машину к headless runner и VMHost

- [ ] Принято после ревью.
- **Где:** `tools/runner/src/`, `apps/vmhost/Sources/`, `od-host`; публичные headers/protocols только при реальной необходимости.
- **Сделать:** передавать validated config/artifact paths, запускать реальную machine, получать serial/events и завершать её через существующий lifecycle. Не встраивать MMU/CPU в Swift. При изменении ABI соблюдать версию и единственный нормативный header/wire source.
- **Проверка:** start/stop, pause/resume, invalid config, guest fault и повторная сессия; subprocess/FFI lifecycle без утечки RAM/executable handles и dangling callbacks.
- **Готово:** runtime status получен от реально исполняемой машины; прежний shell/debug-run не выдаётся за VM boot. UI-функции сверх запуска этапу не добавлять.

### S4-42. Измерить memory/TLB/events и влияние JIT

- [ ] Принято после ревью.
- **Где:** `tools/bench/`, `docs/reports/STAGE_4_REPORT.md`, small raw data.
- **Сделать:** сравнить uncached/cached translations, последовательные/случайные accesses, rights/fault paths, cross-page, working sets, IRQ/timer overhead и ограниченный Linux boot. Interpreter/JIT сравнивать на одинаково поддержанном сценарии, отдельно cold/warm.
- **Проверка:** повторные прогоны, variance, host/guest page sizes, CPU/OS/toolchain/build flags, artifact hashes, memory consumption и одинаковая correctness. Не выводить «ускорение MMU» только из уменьшения Linux boot time.
- **Готово:** есть воспроизводимые числа и вывод, включая отсутствие доказанного выигрыша. Усложнения TLB/event queue принимаются только при измеренной необходимости.

### S4-43. Проверить reset, churn и ограниченные ресурсы

- [ ] Принято после ревью.
- **Где:** `tests/system/`, runner stress mode без production semantics.
- **Сделать:** многократные boot/STOP/reset, remap/rights changes, code writes/invalidation, timer reprogramming и bounded cache churn. Установить явные лимиты RAM, pending events, executable cache и объёма trace.
- **Проверка:** отсутствие stale translations, старых IRQ/handles после reset, runaway host allocation и зависания STOP; сравнить корректное освобождение с baseline после повторений.
- **Готово:** выбранная длительность и число повторов отражены в отчёте. Ограниченный stress не объявляется долговременной надёжностью рабочего Linux этапа 5 или SMP-безопасностью.

### S4-44. Выполнить итоговый воспроизводимый набор

- [ ] Принято после ревью.
- **Где:** `docs/testing/MACHINE.md`, orchestration в существующих tools, итоговый report/data.
- **Сделать:** одним документированным набором собрать host и отдельные guest/firmware artifacts, выполнить local contracts, kernel system suite, Linux positive/negative boot, lifecycle/stress и выбранные hardware cases на Mac mini. Не копировать CPU-семантику в orchestration.
- **Проверка:** каждый case содержит `passed`/`failed`/`not_run`/`blocked`, причину, environment, artifact identity и log reference. Hardware gate этапа 3 остаётся в его собственном scope; не требовать полного physical roundtrip после каждого локального opcode patch.
- **Готово:** итоговые обязательные проверки действительно выполнены; недоступный hardware run остаётся видимым препятствием полной доказательной приёмке, но не обнуляет завершённые локальные milestones.

### S4-45. Независимое итоговое ревью

- [ ] Принято; все обязательные замечания закрыты.
- **Кто:** `gpt-6.1-sol`, reasoning `high`; архитектурные спорные вопросы — `gpt-6-astra`, `high`.
- **Проверить:** diff/затронутые контракты, memory lifetime и unsafe/FFI, fault ordering/A/D, CPU transitions, TLB invalidation, active-code invalidation, IRQ/time, Linux evidence и соответствие physical scope. Сверить, что тесты не подтверждают лишь собственные заглушки.
- **Готово:** конкретные замечания исправлены с повторной проверкой; остаточные ограничения отражены в реестре и support matrix. Если независимое ревью недоступно, явно записать это, не заявляя выполненное ревью.

### S4-46. Зафиксировать результат и передать зависимости этапу 5

- [ ] Этап принят по фактическим gates раздела 8.
- **Где:** `STAGE_4_REPORT.md`, этот план, `ROADMAP.md`, `PROJECT_LAYOUT.md`, `ARCHITECTURE.md`, затронутые specs/testing/support matrix и `KNOWN_ISSUES.md`.
- **Сделать:** записать реально поддержанные modes/features/devices, entry/boot command, exact artifacts, тесты и measurements, host configurations, physical limits, review result. Отдельно перечислить, что потребуется этапу 5: storage/network и устойчивые workloads; этапу 6 — SMP/ordering.
- **Проверка:** ссылки, layout, непротиворечивые статусы документов; все незакрытые выявленные проблемы имеют владельца/следующий шаг. Не называть provisional факт final и не заменять отчёт сообщением в чате.
- **Готово:** другой исполнитель воспроизводит выбранный Linux userspace из документации и видит точные границы результата.

## 8. Приёмка и основания для остановки конкретного среза

| Gate | Обязательное доказательство | Что не заменяет проверку |
|---|---|---|
| G1 — memory/CPU | S4-02…14, 18…20: production RAM/MMU, permissions/faults, системные переходы | Компиляция каркаса, тестовый injected fault без delivery |
| G2 — machine/kernel | S4-21…33: своё ядро, IRQ/timer/UART, согласованные discovery/PCI | Register stubs, вечные ready flags, host marker |
| G3 — JIT integration | S4-15…17: production owner, точные exits, aliases и active code invalidation для заявленного slice | Fixture memory ownership или run целиком через fallback |
| G4 — независимая семантика | S4-34 и 44: выбранные действительно выполненные системные hardware cases с границами сравнения | SSH connectivity, sudo usermode process, протокол будущего эксперимента |
| G5 — Linux userspace | S4-35…40: exact artifacts, CPL3 `/init`, syscall/return, nonce, timer и negative cases | Boot banner, статический boot header, только «kernel started» |
| G6 — воспроизводимость | S4-41…46: lifecycle, measurements, stress scope, документация и ревью | Непроверенный отчёт или claim об ускорении без baseline |

Полный этап принимается после обязательных gates. Частичный результат обозначать точно, например «локально приняты memory+testkernel; hardware gate blocked; Linux userspace not_run». При недоступном системном физическом запуске продолжать независимые локальные задачи, но не подменять источник доказательства.

Остановить зависимый срез и записать конкретную причину, если:

- Неизвестны требуемый CPU mode, MSR/feature или точный boot handoff. Следующий шаг — source/spec audit S4-00 и prerequisite, а не догадка.
- Нет семантики обязательной инструкции в интерпретаторе. Следующий шаг — точная задача этапа 2 с regression case.
- Нельзя соблюсти memory contract, precise fault или безопасный handle lifetime. Следующий шаг — отдельное архитектурное решение и проверка затронутых callers.
- Нужен недоступный privileged hardware run. Следующий шаг — подготовленный harness/процедура и конкретное разрешение на воздействие на стенд; локальные работы продолжаются.
- Нет надёжной привязки Linux binary к принятому artifact/источнику. Следующий шаг — закрытие provenance, а не незаметная замена kernel.

## 9. Готовый промпт для одной задачи

```text
Выполни только S4-<NN> из docs/plans/STAGE_4.md.

1. Прочитай AGENTS.md, docs/PROJECT_LAYOUT.md, задачу и её зависимости.
   Прочитай вложенные AGENTS.md для всех изменяемых каталогов.
2. Проверь существующий diff и фактическую готовность prerequisites.
   Начни discovery с Graphify. Не считай план или fixture готовой системой.
3. Укажи точный поддерживаемый сценарий и отрицательные случаи.
   CPU semantics принадлежат od-isa; RAM/MMU/devices — od-machine;
   JIT — od-jit; host executable memory — od-host. Не копируй их в tools.
4. Следуй ролям моделей из AGENTS.md. Сделай один работающий срез,
   сохрани precise faults, side effects и текущие контракты.
5. Проверь затронутые компоненты, запроси независимое ревью diff,
   исправь подтверждённые замечания. После кода обнови Graphify.
6. Запиши фактические результаты в docs/reports/STAGE_4_REPORT.md,
   обнови статус задачи и выявленные открытые проблемы в KNOWN_ISSUES.md.
   Различай local, native usermode и privileged hardware evidence.
7. Не меняй область этапа, не подключай готовый emulator/JIT/MMU,
   не маскируй отсутствующую возможность заглушкой.

Единственный x86 стенд — Mac mini Ubuntu, SSH senya@192.168.1.14.
Для CR/paging/APIC обычного процесса Ubuntu недостаточно.
Не перезагружай и не меняй загрузку Mini без отдельного разрешения.
Если prerequisite отсутствует, зафиксируй точную зависимость и выполни
независимую часть; не отмечай заблокированный критерий как passed.

В конце: что работает; изменённые файлы; выполненные проверки и результаты;
границы local/physical claim; результат ревью; следующие незакрытые пункты.
```

## 10. Проверка самого плана

Подготовка этого документа не меняет код и не подтверждает выполнение S4-задач. При проверке плана нужны согласованность с roadmap/architecture/layout, ссылки, непротиворечивые зависимости и отдельное ревью. CPU-тесты и SSH для этой документальной задачи не требуются.

Архитектурная декомпозиция проверена отдельной ролью `gpt-6-astra` / `high`: учтены незавершённый production memory owner, ограничения Long64/system semantics, точные memory side effects, отдельная физическая методика и ограниченный data-only ACPI путь.

Независимое ревью плана выполнено ролью `gpt-6.1-sol` / `high`. Найденный цикл критериев S4-29/31 ↔ Linux loader устранён: локальная готовность предшествует loader, Linux calibration/discovery проверяются в S4-38. Повторное ревью подтвердило исправление; оставшихся замечаний к плану нет.

Проверки документа 2026-11-04: 47 последовательных задач S4-00…46; известные идентификаторы и отсутствие циклов в таблице зависимостей; существующие относительные Markdown-ссылки; `python3 tools/check_layout.py` и `git diff --check` — успешно. Отдельно проверяется новый Markdown, ещё не включённый в Git diff, на пробелы в конце строк и завершающий перевод строки. Эти результаты относятся только к документации, не к реализации CPU/машины.
