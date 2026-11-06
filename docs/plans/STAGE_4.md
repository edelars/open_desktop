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

### S4-17. Собрать локальный memory/fault differential набор

- [ ] Принято после ревью.
- **Где:** `tests/system/`, инфраструктура `od-testkit`, `docs/testing/MACHINE.md`.
- **Сделать:** один входной case и нормализованный результат для interpreter uncached/cached и JIT supported subset. Сохранять exception, registers, CR2, memory deltas, A/D, device events и причину завершения.
- **Проверка:** inject намеренное различие и убедиться, что comparator его обнаруживает; undefined поля сравнивать по явной маске. Hardware evidence S4-34 подключать отдельно, не маркировать локальные cases как native.
- **Готово:** можно воспроизвести и уменьшить mismatch без копии CPU/MMU семантики в testkit; общий walker не считается независимым oracle.

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

### S4-19. Доставка исключений и восстановление после fault

- [ ] Принято после ревью.
- **Где:** delivery CPU state machine в `od-isa`, orchestration `od-machine`.
- **Сделать:** единый путь delivery с правильным frame/error code, retry RIP и return. Явно обработать fault при descriptor/stack access, правила `#DF` и machine outcome при невозможности его delivery.
- **Проверка:** guest handler `#PF` исправляет PTE, выполняет нужную invalidation и повторяет faulting instruction ровно с допустимыми effects; сломанный handler/stack приводит к ожидаемому secondary fault/shutdown.
- **Готово:** исключение реально доставлено guest handler и execution продолжается; возвращённый host enum без delivery не закрывает задачу.

### S4-20. CPL3 и системные вызовы/возвраты

- [ ] Принято после ревью.
- **Где:** `od-isa`, `tests/system/`, точные CPU prerequisites этапа 2.
- **Сделать:** реализовать переходы CPL и нужные Linux entry/return механизмы из S4-00, связанные MSR/flags и stack rules. Не расширять same-CPL IRETQ fixture claim на все формы.
- **Проверка:** CPL3 read/write/fetch protection, запрещённые privileged instructions, вход в kernel, изменение kernel-visible данных, возврат к следующей user instruction; неправильные selectors/addresses/flags на return.
- **Готово:** собственный guest CPL3 probe проходит полный roundtrip; error paths не оставляют частично переключённые привилегии.

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

### S4-25. Local APIC одного CPU

- [ ] Принято после ревью.
- **Где:** `od-machine`, CPU APIC base/MSR effects в `od-isa`.
- **Сделать:** выбранный xAPIC register model, enable/base, APIC ID, masks/priority, IRR/ISR/TMR, EOI и требуемые LVT. x2APIC и multi-CPU startup не заявлять. Необходимые self-directed IPI/NMI пути определить по manifest.
- **Проверка:** reset, masked vector, priority, pending/in-service transitions, spurious path, EOI. Ошибочные register accesses и reserved bits следуют принятой спецификации.
- **Готово:** APIC state реально влияет на pending/deliverable interrupt, а не существует только как читаемый массив registers.

### S4-26. IOAPIC и маршруты внешних IRQ

- [ ] Принято после ревью.
- **Где:** `od-machine`, board manifest.
- **Сделать:** register select/window, redirection table, mask, vector, destination, polarity/trigger mode и remote-IRR для используемых level routes. Поддержанные routes имеют один источник описания.
- **Проверка:** masked edge, unmask, level assert/deassert, повторный level после EOI, недопустимый destination/vector; гостевые config writes меняют поведение.
- **Готово:** один внешний device IRQ доходит до Local APIC и не теряется/не дублируется при предусмотренных mask/EOI transitions.

### S4-27. Соединить pending IRQ с CPU delivery

- [ ] Принято после ревью.
- **Где:** `od-machine`, delivery/interruptibility `od-isa`, JIT exits `od-jit`.
- **Сделать:** выбирать deliverable event на корректной границе instructions/blocks; учитывать IF, STI/MOV SS shadow и нужные NMI rules, frame и EOI. Согласовать HLT wakeup и очередь событий.
- **Проверка:** IRQ перед/после CLI/STI, shadow, HLT при разных условиях, pending during handler, fault при delivery; interpreter и поддержанный JIT дают одинаковую архитектурную последовательность.
- **Готово:** handler действительно выполняется и возвращает управление; queued IRQ не теряется на JIT exit, fallback или pause.

### S4-28. APIC timer

- [ ] Принято после ревью.
- **Где:** `od-machine`, virtual clock/events, system tests.
- **Сделать:** нужные count/divider/LVT registers, one-shot/periodic modes и cancel/reprogram. TSC-deadline добавлять только при обязательности и полном контракте.
- **Проверка:** нулевой count, masked expiry, перепрограммирование до deadline, periodic handler+EOI, HLT wakeup, pause/resume, reset.
- **Готово:** guest получает ожидаемое число interrupts в контролируемом virtual-time тесте, а Linux-профиль имеет работоспособный источник времени и событий.

### S4-29. Закрыть необходимые legacy зависимости

- [ ] Локально принято после ревью либо scope исключён по source/config audit; Linux-проверка обязательна в S4-38.
- **Где:** `od-machine`, профиль/методика в docs.
- **Сделать:** по исходникам/config выбранного Linux определить необходимость PIC, PIT, RTC/CMOS и другого timer fallback. Реализовать только реально требуемый минимум, включая mask/ack и routing handoff к APIC; документировать выбранную cmdline. Для исключённого устройства сохранить source/config основание и назначить проверку этого решения в S4-38.
- **Проверка здесь:** локальные device tests и собственный guest probe подтверждают calibration primitives, mask/ack и отсутствие дублирования legacy/IOAPIC IRQ. Linux boot trace на этом шаге ещё не требуется.
- **Готово для S4-31/37:** локальный контракт проверен; нет фиктивных registers «всегда ready» и скрытых host delays. Отсутствие Linux hang и достаточность выбранного набора устройств подтверждаются позже в S4-38, до окончательной приёмки этапа.

### S4-30. Минимальная PCI/PCIe configuration

- [ ] Принято после ревью.
- **Где:** `od-machine`, board manifest и `VIRTUAL_BOARD.md`.
- **Сделать:** один segment/root и static topology; ECAM и, если нужен выбранному Linux, другой configuration access. Absent functions, widths, readonly fields, BAR probing/sizing/alignment и decode enable определить явно. Не объявлять NVMe/NIC до их реализации.
- **Проверка:** перечисление пустых/существующих functions, BAR probe, invalid bus/device/function/offset, согласованность BAR и physical map. Использовать малый собственный тестовый endpoint, если нужен для BAR tests, и пометить его diagnostic-only.
- **Готово:** config transactions работают через guest memory/PIO, а обнаруживаемые production devices имеют реальный backend. MSI/MSI-X не требовать без объявленного устройства, которому они нужны.

### S4-31. Подготовить минимальные discovery tables

- [ ] Принято после ревью.
- **Где:** resolved manifest — `od-machine`; форматы ACPI — `firmware/uefi/src/`; orchestration — `tools/build/`; отдельные firmware tests.
- **Сделать:** определить по выбранному kernel точный набор таблиц, включая необходимые RSDP/root/FADT/MADT/MCFG и их зависимости. Из machine manifest получить адреса, reservations, APIC/ECAM/routes. Не считать заранее любой короткий список достаточным.
- **Путь сборки:** отдельная freestanding guest-target сборка firmware формирует data-only ACPI blob для фиксированного manifest; packaging только извлекает bytes/sections и metadata, не дублирует serializer. Сохранить version, manifest hash, load ranges, RSDP address. UEFI services и исполнение firmware не требуются; firmware не добавляется в host workspace.
- **Проверка здесь:** checksums/lengths/references, соответствие устройствам и reservations; machine-side validator отвергает чужой manifest, overlap и out-of-range, не требуя ещё не созданного Linux loader. Изменение конфигурации требует пересборки blob. Альтернативу ACPI для discovery обосновать исходниками/config выбранного Linux; её фактическую работу проверить в S4-38.
- **Готово для S4-37:** таблицы получены реальной отдельной сборкой, независимо проверены локально и готовы к проверяемому placement. Firmware не назначает собственную карту. Обнаружение правильных CPU/APIC/ECAM самим Linux — обязательная последующая проверка S4-38, не prerequisite создания loader.

### S4-32. Собрать и загрузить собственное маленькое ядро

- [ ] Принято после ревью.
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
- **Сделать:** cases для RAM alias, права и `#PF` repair/retry, huge pages заявленного профиля, cross-page effects, CPL3 syscall/return, timer, UART IRQ, IOAPIC mask/EOI, HLT wakeup и self-modifying code. У каждого case отдельный идентификатор и ожидаемый результат.
- **Проверка:** interpreter с TLB off/on и JIT только для реально поддержанных частей; injected regression обязан ломать соответствующую проверку. Сохранять guest state и event trace, а не одну строку PASS.
- **Готово:** все обязательные cases имеют факт выполнения. Общая CPU/MMU семантика ещё требует независимого evidence S4-34; локальная приёмка не выдаётся за physical.

#### S4-33a. Guest RAM alias store→load

- [x] Ограниченный case принят после независимого ревью; полный S4-33 остаётся открытым.
- **Контракт:** `Stage4AliasProbeV1` отображает GVA `0x800000` и `0xa00000` на одну RAM-страницу GPA `0x9000`. Guest исполняет `MOV [RBX],RCX; POP RAX; OUT DX,AL; HLT`; initial qword и store qword различны. Acceptance требует изменения RAM через первый GVA и чтения нового значения через stack alias, а одного HLT недостаточно.
- **Проверка:** builder manifest отражает обе mappings без противоречий; builder→runner проверяет два store values, budgets, UART, guest state и injected broken-alias outcome; machine case проверяет leaf A/D и точный порядок committed callbacks. Legacy profiles остаются строгими.
- **Граница:** uncached interpreter evidence для RAM alias. Не подтверждает TLB-on/JIT alias invalidation, cross-page semantics, fault repair, Linux или physical hardware; S4-13/14/16 и S4-18/19 остаются отдельными prerequisites.

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
- **Сделать:** строго по протоколу точного image проверить header, entry/alignment/relocation, разместить kernel/initrd/boot_params/cmdline/stack/page tables и table blob. E820 и reservations взять из единой карты; подготовить CR/MSR/GDT/segments/registers согласно выбранному entry. Не подставлять универсальные magic addresses из чужого примера.
- **Проверка:** overlap/overflow, слишком маленькая RAM, некорректный header, чужой table manifest, initrd за допустимым диапазоном, entry state. Ошибку сообщать до guest launch там, где она проверяется loader.
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
