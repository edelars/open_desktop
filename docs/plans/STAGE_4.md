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

- [x] Принято после ревью (закрыт срезами S4-27a–g, включая f1 и f2; Claude Opus high). Оговорки: доставка только плоская identity Long64/CPL0 без paged-входа и IRETQ; автоматического event pump и scheduler нет (предлагаемый владелец — S4-33, решение пользователя); тень MOV SS и NMI не моделируются (MOV SS не реализован; риск для S4-37/S4-38); fault при доставке — только отказ host-адаптера, без вложенных исключений и #DF (S4-33). Оговорки «Граница» подсрезов сохраняются.
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
- **Предусловия и границы:** caller сериализует CPU/APIC/IOAPIC, удостоверяет completed boundary, отсутствие synchronous/debug/NMI/MOV-SS inhibition, private synchronous RAM, один vCPU и ограничения ISA entry. Требуются Runnable Long64/CPL0 Running state, явный STI shadow, `rflags_defined = INTEGER_FLAGS`, отсутствие configured TSC/RDTSC и `CR0/CR3/CR4`; `Blocking` при IF=0 отклоняется без мутаций. REP iteration-budget exit делает границу непригодной для APIC entry, IRETQ и STI quantum до последующего retirement обычным `run()`.
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

## Решения по консультации Opus (2026-11-09)

Принято как решения этапа по рекомендациям архитектурного советника; каждое изменение плана ниже относится к этим решениям.

1. **Event pump** — срез **S4-27h** (разовый handler с paged IRQ) и **S4-27i** (общий interpreter-цикл: проверка APIC на границе инструкции, HLT→wait, мост EventQueue→APIC, owner-driven dispatch таймеров) перед S4-33. Владелец event pump — S4-27i; S4-33 его не строит.
2. **Paged IRQ entry и IRETQ** — срез S4-27h, не S4-38. Оговорка «только плоская identity» в S4-27 снимается ссылкой на S4-27h.
3. **Linux time source** — условие «Linux-профиль имеет источник времени» переносится в S4-37a и S4-38; S4-28 закрывается по локальному критерию.
4. **Подключение APIC timer к плате и Linux-профилю** — срез **S4-37a** (Linux-профиль платы под opt-in флагом: LAPIC с shared timer и единым clock, PIT, PIC, CMOS, 0xED, 0x80, INTIN2, PM-блок на том же clock и pump). S4-37 зависит от S4-37a.
5. **Порт-тик** — каждое чтение временного порта (0x42, 0x61, PM_TMR) синхронизирует часы по выполненным инструкциям и продвигает их на Δ = 1 мкс (владелец clock — сессия). ISSUE-067 закрывается в S4-29d.
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

- [ ] Принято после ревью.
- **Сделать:** собственная fixture повторяет `pit_calibrate_tsc` (mode 0, latch, цикл по 0x61 бит 5) и `quick_pit_calibrate` (MSB 0x42) в virtual time. Решается, как продвигается время при PIO-опросе (связь с ISSUE-067) — решение пользователя.
- **Проверка:** измеренная частота TSC; отсутствие зацикливания.
- **Зависимости:** S4-29c.

#### S4-29e. Вход IOAPIC INTIN2, только edge

- [x] Принято после ревью (Claude Opus, high). Отказ LEVEL на unmasked entry 2 — изменение поведения для гостя, согласовано контрактом.
- **Сделать:** `set_intin2` и флаг `intin2_high`; конфигурация из `pins[2]`, хранение и проверка high dword (физический APIC ID 0); проверка вектора 0x10..=0xFE; LEVEL на entry 2 — отказ; конфликт вектора с level-маршрутом INTIN4 — отказ; reset и маска (edge под маской теряется, unmask не доставляет).
- **Граница:** level, Remote IRR и другие пины не входят. Зеркалит S4-26a.

#### S4-29f. PIT канал 0 → INTIN2 (GSI 2)

- [ ] Принято после ревью.
- **Сделать:** события режимов 2 и 0 через EventQueue с поколениями (как APIC timer); reprogram, shutdown (0x30), reset; OUT0 идёт только в INTIN2; доставки в PIC IR0 нет. Гостевая probe получает N прерываний IRQ0 через GSI 2 и делает EOI. Негативы: замаскированный entry 2, нет EOI.
- **Зависимости:** S4-29a, S4-29c, S4-29e. Насос событий для гостевой probe — решение пользователя (S4-33 или локальный цикл в fixture).

#### S4-29g. Закрытие S4-29: аудит и cmdline (только документы)

- [ ] Принято после ревью.
- **Сделать:** аудит по исходникам и config выбранного Linux; выбранная cmdline (в т.ч. решение по `no_timer_check` и `io_delay`); список проверок для S4-38.

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

- [ ] Принято после ревью.
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

### S4-34. Подготовить и выполнить независимые системные hardware cases

- [ ] Методика/harness приняты; физический запуск принят отдельно.
- **Где:** `docs/testing/MACHINE.md`, собственные freestanding fixtures, `tools/x86-runner/` только для допустимой host orchestration; evidence в report/data.
- **Сделать:** составить минимальный набор для page walks/rights/A/D/CR2/errorcode, CR transitions, descriptor/delivery/return и требуемого interrupt path. Использовать единственный Mac mini по SSH `192.168.1.14`; сначала доказать способ реального CPL0 выполнения и сбора результатов. Проверить recovery, ограничения memory/device writes и независимость oracle.
- **Проверка:** фиксировать hardware identity, environment/execution kind, artifact hashes, entry state, expected defined fields и raw output. Не считать обычный Linux signal adapter проверкой всей системной семантики. Hardware topology может отличаться от virtual board: сравнивать явно общий архитектурный контракт, не абсолютные platform addresses.
- **Готово:** выбранные cases действительно выполнены независимо от нашего MMU. При отсутствии разрешённого privileged запуска статус `blocked`/`not_run`; методику нельзя отметить как physical pass. Reboot/изменение загрузки — только по отдельному конкретному разрешению согласно разделу 5.

### S4-35. Подготовить зафиксированный Linux artifact

- [ ] Принято после ревью.
- **Где:** `tools/build/` или runner staging, существующий Linux profile, ignored `work/stage4/`.
- **Сделать:** взять точный Ubuntu package/image из S4-00 и существующего profile, проверить размер/hash, происхождение и доступную цепочку подписи; сопоставить source/config/build. Не заменять kernel текущим latest без пересмотра manifest.
- **Проверка:** несовпадение hash, corrupted/truncated image, отсутствие source/config evidence; различать проверенный content hash и подтверждённую подлинность. Не переносить открытые provenance пункты в «подтверждено» без проверки.
- **Готово:** staging воспроизводим; большой kernel/package не попадает в Git; report указывает точную проверенную версию, источники и оставшиеся ограничения.

### S4-36. Собрать минимальный initramfs с настоящим `/init`

- [ ] Принято после ревью.
- **Где:** собственный source/recipe в `tests/fixtures/guest/stage4/`, сборка `tools/build/`, image в ignored work.
- **Сделать:** маленький статический CPL3 program без лишних ISA требований; нужный формат initramfs, права/mode и entry. `/init` выводит переданный run nonce, делает проверяемые syscalls и возвращается к user instructions; задать завершение/ожидание для получения timer evidence.
- **Проверка:** содержимое archive, executable ABI, отсутствие host paths/secrets, repeatable build; негативные варианты отсутствующего/неисполняемого `/init` и неверного nonce.
- **Готово:** artifact подходит точному kernel config и profile; host runner не генерирует guest marker. Диск и сеть не нужны для первого userspace.

### S4-37. Реализовать Linux direct-boot loader

- [ ] Принято после ревью.
- **Где:** проверка/placement/entry setup в `od-machine`, orchestration `tools/runner/src/`, `MACHINE_CONTRACT.md`.
- **Сделать:** строго по протоколу точного image проверить header, entry/alignment/relocation, разместить kernel/initrd/boot_params/cmdline/stack/page tables и table blob. E820 и reservations взять из единой карты; подготовить CR/MSR/GDT/segments/registers согласно выбранному entry. Не подставлять универсальные magic addresses из чужого примера. **Требование из ревью S4-31a (обязательно):** E820 должен содержать reserved-запись на `[0xFC10_0000, +1 MiB)` (окно ECAM). Требование действует, когда conf1 проходит `pci_sanity_check` (появилась функция host bridge/VGA или SMBIOS-год ≥ 2001) и при этом год BIOS < 2016 или DMI отсутствует: без записи Linux принимает ECAM только поздно. Альтернатива — PNP0C02 с `_CRS` (S4-31b); при ней запись E820 не обязательна, но дешева. Также: не класть zero page, cmdline и стек в `[0xE_0000, 0x10_0000)`; E820 размечает `[0xA_0000, 0x10_0000)` как reserved с ACPI reclaim (`[0xE_0000, 0xE_F000)`) и NVS (`[0xE_F000, 0xF_0000)`); ниже `0x9_F000` оставить RAM для real-mode trampoline Linux.
- **Формат blob v2 (интерфейс для этого среза, зафиксирован по ревью S4-31g):** шапка 0x60; поля 0x50 facs_offset, 0x54 facs_len (64), 0x58 facs_address (0xE_F000) описывают сегмент FACS, лежащий сразу за payload и вплотную к концу blob. Загрузчик копирует два диапазона: payload (`load_range()`, `[0xE_0000, 0xE_0340)`) и FACS (`facs_range()`, `[0xE_F000, 0xE_F040)`). E820: NVS `[0xE_F000, 0xF_0000)`, ACPI reclaim `[0xE_0000, 0xE_F000)`. RSDP на 0xE_0000 (`rsdp_address`).
- **Проверка:** overlap/overflow, слишком маленькая RAM, некорректный header, чужой table manifest, initrd за допустимым диапазоном, entry state. Ошибку сообщать до guest launch там, где она проверяется loader. E820-запись ECAM присутствует в boot_params; в S4-38 dmesg показывает `ECAM … reserved as E820 entry` или `… ACPI motherboard resource`. **Требование из ревью S4-31c:** разрез нижней RAM под ACPI (`[0xE_0000, 0xE_F000)` и `[0xE_F000, 0xF_0000)`) записан в `handoff_regions` (`firmware_board.rs`); loader должен брать его из публичных констант `FIRMWARE_ACPI_*` или из регионов 11/12 handoff, а не из второго списка констант (правило S7-03 п. 2).
- **Готово:** documented handoff воспроизводим, source/boot protocol и kernel согласованы. Loader не исполняет за гостя обязательную декомпрессию/trampoline/CPU sequence в обход принятого boot contract.

### S4-38. Довести Linux до перехода к init

- [ ] Принято после ревью.
- **Где:** `tests/system/`, runner tracing; CPU fixes — в владельце этапа 2, MMU/devices — `od-machine`.
- **Сделать:** запускать bounded boot, сохранять checkpoints: entry, decompression/relocation при наличии, paging, IDT, clock/APIC setup, initramfs, попытка exec init. На первом unsupported/fault/stall получить точный RIP/bytes/context и минимальный repro.
- **Проверка:** завершить отложенную Linux-интеграцию S4-29/31: kernel обнаруживает ровно один CPU, правильные APIC/ECAM/routes; проходит timer calibration/discovery без hang; исключённые legacy устройства действительно не нужны выбранному пути. При несоответствии исправить scope/реализацию и повторить локальные regressions и boot. Не скрывать неисправность дополнительной cmdline без объяснения и не возвращать fake CPUID/MSR/device-ready.
- **Готово:** выбранный kernel доходит до настоящего exec init без заглушек; discovery/time evidence S4-29/31 подтверждены. Сохранены точная cmdline и граница пока ещё не принятого userspace.

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
