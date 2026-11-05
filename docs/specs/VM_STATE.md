# Состояние VM, lifecycle и операции сохранения

Статус: **черновик S9-01/S9-02, ожидает ревью**. Инвентаризация выполнена по фактическому коду на 2026-11-05 (ветка `feat/s9-stage`). Ни одна операция из §3 не реализована; документ задаёт контракт, а не описывает работающую функциональность. Versioned full-VM container — TBD S9-23.

Связанные документы: [план этапа 9](../plans/STAGE_9.md), [STORAGE_FORMAT.md](STORAGE_FORMAT.md) (уровни подтверждения записи, модель отказов), [архитектура §7](../ARCHITECTURE.md#снимки), [виртуальная плата](VIRTUAL_BOARD.md), [контракт машины](MACHINE_CONTRACT.md), [методика recovery](../testing/STORAGE_RECOVERY.md).

## 1. Участники полного состояния VM

Полный checkpoint профиля `full-windows-vm` ([профили](STORAGE_FORMAT.md#2-профили)) охватывает одним consistency cut: все vCPU, RAM, device/DMA writers, interrupts, virtual time, pending/completed I/O и persistent generations дисков, UEFI variables и vTPM. Derived state (JIT cache, TLB, native pointers, host handles) не сохраняется и воссоздаётся с новыми generations.

## 2. Инвентаризация prerequisites (S9-01)

Метод: Graphify query по snapshot/save/restore, затем точечный `rg` по `crates/*/src`, `apps/vmhost`, `firmware/uefi`, `guest/`, `protocols/`. Колонка «Versioned save/load» отвечает только на вопрос, существует ли в коде версионированный codec. **Ни у одного компонента его нет.**

### 2.1. Storage и host I/O

| Компонент | Фактический owner / код | Versioned save/load | Blocker и следующая задача |
|---|---|---|---|
| RAW/CoW backend | `od-storage`: [`src/lib.rs`](../../crates/od-storage/src/lib.rs) — 3 строки doc-комментария, scaffold | Нет | Нет реализации → S9-03…05 |
| Host file I/O, durability, locking | `od-host` не содержит file I/O: нет `sync_all`/`fsync`/`flock`/`OpenOptions` в production коде; `od-host` не зависит от `od-storage` | Нет | S9-03/04/06; durability API не подтверждён ([ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed)) |
| CLI image-tool | `tools/image-tool/src/` — только `.gitkeep` | — | S9-19 |
| Guest storage device (NVMe) | Отсутствует в `od-machine`; плата планирует NVMe поверх PCIe ([VIRTUAL_BOARD](VIRTUAL_BOARD.md)); PCI configuration (S4-30) не принята | Нет | Блокирует S9-13; prerequisite этапа 5 roadmap (диск) |
| UEFI NVMe Block I/O | `firmware/uefi/src/` — только `.gitkeep` | — | Этап 7 roadmap |

### 2.2. CPU, RAM, устройства, время

| Компонент | Фактический owner / код | Versioned save/load | Blocker |
|---|---|---|---|
| Архитектурный CPU state | `od-isa::LongModeState` ([`state.rs`](../../crates/od-isa/src/state.rs)): GPR, RIP, RFLAGS, FS/GS base, selectors, частичные descriptor caches, GDTR/IDTR, x87/SSE, STI shadow, activity. Многие поля `Option` и заполняются только отдельными bounded профилями. Legacy `FragmentState`/`CpuState` (4 GPR) в [`lib.rs`](../../crates/od-isa/src/lib.rs) | Нет: только `Clone`/`PartialEq` | Нет CR2, CR8, DR*, LDTR, TR, XSAVE/AVX, general MSR, pending exception/event state. CR3 и EFER находятся вне CPU state — в `od-machine::Long64ProbeConfig` ([`page_walk.rs`](../../crates/od-machine/src/page_walk.rs)) и синтетическом `MsrProfile::EferReadOnlyV1` ([`msr.rs`](../../crates/od-isa/src/msr.rs)). Единого owner полного CPU state нет ([ISSUE-056](../KNOWN_ISSUES.md#issue-056-s9-cpu-state-not-single-owner)). Этап 4 (S4-12, S4-18…21) открыт; SMP — этап 6 |
| vCPU lifecycle | `od-machine::DiagnosticVcpuSession` ([`diagnostic_vcpu.rs`](../../crates/od-machine/src/diagnostic_vcpu.rs)): `Runnable`/`Paused`/`Stopped`/`Terminal`, synchronous caller-driven | Нет | Диагностический boundary, не полный lifecycle ([контракт](MACHINE_CONTRACT.md#bounded-diagnostic-vcpu-session-boundary)); S4-21 открыт. Нет safe-point barrier для S9-28 |
| Guest RAM | `od-machine::PhysicalMemory`/`MachineMemoryMap` ([`physical_memory.rs`](../../crates/od-machine/src/physical_memory.rs)) — реальный owner RAM regions | Нет; нет dirty tracking | Owner есть, codec нет → S9-24 (stopped full-RAM copy) |
| TLB/translation cache | `od-machine::TranslationProbeCache` — только за feature `experimental-translation-probe-cache` | Не сохраняется (derived) | Rebuild при restore; полный TLB (S4-13/14) не принят |
| Bus и device registry | `od-machine::MachineBus` ([`bus.rs`](../../crates/od-machine/src/bus.rs)): `Vec<Box<dyn BusDevice>>`; trait `BusDevice` имеет только `read`/`write` | Нет: нет save/validate/load в trait | S9-25 должен добавить participant contract |
| UART 16550 (polling TX) | `od-machine::Uart16550PollingTx` ([`uart16550.rs`](../../crates/od-machine/src/uart16550.rs)): приватный `UartState` за `Arc<Mutex>`; `Uart16550Handle` alias того же состояния | Нет. Приватный `snapshot()` используется только для `PartialEq` | Codec нет; alias-handles должны быть остановлены barrier'ом до capture (S9-28) |
| UART RX event pump | `od-machine::UartRxEventPump` ([`uart_rx_events.rs`](../../crates/od-machine/src/uart_rx_events.rs)): payloads + `EventQueue` | Нет | S9-25 |
| Virtual clock / TSC | `od-machine::VirtualClock` ([`virtual_clock.rs`](../../crates/od-machine/src/virtual_clock.rs)): `now_ns`, `tsc_hz`, `tsc`; caller-driven | Нет | Политика времени при pause/suspend не задана ([архитектура §5](../ARCHITECTURE.md#планирование-и-устройства)); S4-22 открыт |
| Event queue / deadlines | `od-machine::EventQueue` ([`event_queue.rs`](../../crates/od-machine/src/event_queue.rs)): `(key, deadline_ns, sequence)`, opaque `EventGeneration` с приватным полем (публичный тип `pub struct EventGeneration(u64)`, реэкспорт в `lib.rs`) | Нет | Restore должен задать политику generation: извлечённые до checkpoint `ScheduledEvent` не должны оставаться валидными. S4-23 открыт |
| Local APIC, IOAPIC, APIC timer, PCI | Отсутствуют (S4-25…30 не приняты) | — | Этап 4 |
| NIC, xHCI/HID, GOP | Отсутствуют | — | Этапы 5/7 |

### 2.3. JIT, host session, VMHost

| Компонент | Фактический owner / код | Versioned save/load | Комментарий |
|---|---|---|---|
| JIT caches | `od-jit` ([`dispatch.rs`](../../crates/od-jit/src/dispatch.rs), [`memory_cache.rs`](../../crates/od-jit/src/memory_cache.rs)): `clear`, `invalidate`, `reset_for_execution_context` | Не сохраняется (derived) | Пригодно для сброса после restore; production JIT↔memory связь (S4-15) не принята |
| JIT state points | `od-jit::statepoints::SnapshotAdmissibility` ([`statepoints.rs`](../../crates/od-jit/src/statepoints.rs)) | — | Описывает, где canonical guest state материализован в host-коде. Это вход для будущего barrier S9-28, **не** формат checkpoint |
| `ExecutionContextSnapshot` | `od-jit::translation` | — | `id + profile` контекста трансляции; не snapshot машины |
| Executable mappings | `od-host` | Не сохраняется | Host handles никогда не persistent |
| Host session ABI v1 | [HOST_SESSION_ABI_V1.md](HOST_SESSION_ABI_V1.md) | — | Сессия S0-09; нет pause/suspend/checkpoint/restore |
| Control protocol v1 | [CONTROL_PROTOCOL_V1.md](../../protocols/control/CONTROL_PROTOCOL_V1.md) | — | Только HELLO/READY/RUN/STOP; без reconnect и retry. Новые команды — только в S9-33 |
| VMHost | [`apps/vmhost/Sources/main.swift`](../../apps/vmhost/Sources/main.swift) | — | Нет VM bundle, persistent identity и межпроцессного identity lease |
| Testkit `*Snapshot`, `save()` | `od-testkit` ([`v2.rs`](../../crates/od-testkit/src/v2.rs)), serde JSON | — | Fixture-структуры CPU oracle cases. Production не зависит от `od-testkit`; **не** сериализация машины |

### 2.4. Firmware, vTPM, guest

| Компонент | Код | Статус |
|---|---|---|
| UEFI variables | `firmware/uefi/src/` — `.gitkeep` | Отсутствует; этап 7. Блокирует S9-26 и `full-windows-vm` |
| vTPM 2.0 | Нет кода ни в одном crate | Отсутствует; этап 7. Блокирует S9-27 и `full-windows-vm` |
| Guest agents | `guest/linux/agent/src/`, `guest/windows/agent/src/` — `.gitkeep`; `protocols/guest/` — `.gitkeep` | Отсутствуют; S9-34 опционален |

### 2.5. Вывод S9-01

- Offline storage не зависит ни от одного компонента §2.2–§2.4. **Следующий runnable срез: S9-03** (узкий storage I/O interface + test backend), затем S9-04/S9-05 (single writer, RAW). Durable часть S9-06 заблокирована ISSUE-055.
- Все full-VM задачи (S9-23…35) заблокированы: нет versioned codecs, единого owner CPU state, guest storage device, APIC/PCI, UEFI и vTPM. S9-23 (manifest контракт и local tests) можно проектировать заранее; S9-24…27 требуют реальных owners.
- Задачи этапов 5–8 указаны по roadmap без ID, так как планов этих этапов нет.

## 3. Операции и состояния (S9-02)

До закрытия [ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed) (S9-06) все mutating операции и durable operation record недоступны; ответ — `unsupported_durability`.

### 3.1. Состояния сессии VM

| Состояние | Смысл |
|---|---|
| `stopped` | Нет running resources; persistent root `R` (диски, UEFI, vTPM generations) определён |
| `running` | vCPU исполняют, I/O активен |
| `paused` | Исполнение остановлено, resources удержаны; данные не обязательно durable |
| `checkpointing` | Barrier/capture/publication в процессе; новые guest submissions остановлены |
| `suspended` | Опубликован durable full checkpoint `C`, running resources освобождены |
| `shutting_down` | Ожидается guest shutdown path |
| `restoring` | Private stopped target строится; не активен |
| `reconcile_required` | Был `uncertain` исход; mutating операции запрещены до reconcile |
| `failed` | Явная ошибка, VM не исполняется, пригодный persistent root **не установлен** (нужны inspect/salvage). Если root известен и generations валидны, состояние — `stopped` с пометкой причины, а не `failed` |

Модель исхода операции: `pending` → (`aborted` | `failed_before_commit` | `committed` | `failed_after_publication`) → `acknowledged`; `uncertain` — исход неизвестен вызывающему. `failed_after_publication` — исход reconcile (не `failed_before_commit`): persistent публикация операции подтверждена record, но последующий шаг прерван; для restore это значит, что новые generations подтверждены record, `checkpoint_consumed` дописан, VM `stopped` с пометкой причины.

Причины (reason) исходов VM-операций:

- `interrupted_activation` — restore/resume из `suspended` прерван после durable фазы `activating`, VM не жива (§3.2, случай (в)); исход `failed_after_publication`.

`committed` означает, что публикация произошла (durable по [STORAGE_FORMAT §6](STORAGE_FORMAT.md#6-уровни-подтверждения-записи)), `acknowledged` — что вызывающий получил ответ. Потеря ответа после commit даёт `committed` без `acknowledged`; повтор такой операции разрешён только как idempotent query по operation ID.

### 3.2. Operation ID

- Каждая mutating операция (всё, кроме pause и resume из `paused`; resume из `suspended` — это restore, у него record есть) получает operation ID, уникальный в пределах VM identity, и durable запись `operation record` **до** первого persistent mutation. Формат записи — TBD S9-23/S9-33.
- Record хранит тип, target generation/checkpoint ID и фазу. После crash или потери ответа исход определяется только по record + generation/manifest, не по наличию файла и не по времени.
- Повтор команды с тем же ID — запрос статуса, не повторное выполнение. Новый ID для той же цели при `reconcile_required` отклоняется.
- Любой `uncertain` исход переводит VM/image в `reconcile_required`; reconcile выполняется по operation ID и record (плюс generation/manifest/lease, где указано); blind retry запрещён.
- Фазы record restore (включая resume из `suspended`, который является restore) из checkpoint `C`, каждая durable до следующего шага: `validated` → `lease_acquired` → `generations_published` (новые writable children durable) → `activating` (durable intent, записывается **до** запуска первого vCPU) → `activated` → `checkpoint_consumed`. `checkpoint_consumed` — последняя фаза record, записывается после `activated` (или при reconcile случая (в)), а не в момент activation. Consumed означает: `C` больше нельзя restore под той же VM identity (иначе возник бы откат с повторным использованием identity и расходящимися writable generations); допустимы только inspect, clone с новой identity по политике S9-32 и удаление (S9-17). Правило reconcile после crash/потери ответа: по operation ID найти record и проверить identity lease (S9-30); исход определяется последней durable фазой и тем, жива ли VM. (а) Фазы `activating` нет → target не запускался, `C` не consumed, опубликованные generations — orphan для cleanup, взятый identity lease освобождается при reconcile, VM в прежнем `stopped`/`suspended`; повтор restore — только с тем же operation ID. (б) Есть `activating` или `activated`, VM активна под lease → исход `committed`, дописать `activated` (если отсутствует) и `checkpoint_consumed`. (в) Есть `activating` или `activated`, VM не жива → исход операции `failed_after_publication` с причиной `interrupted_activation` (§3.1; не clean и не тихий `stopped`); так как `activating` пишется только после `generations_published`, публикация generations подтверждена record'ом, поэтому дописывается `checkpoint_consumed`; состояние VM — `stopped` с persistent root = новые generations и явной пометкой «прерванный restore/activation»; дальнейший запуск — обычный start от этого root, не повторный restore `C`. VM здесь `stopped`, а не `failed`, потому что root известен и persistent generations валидны; `failed` зарезервирован для ситуаций без пригодного root. (г) VM с этой identity жива, но identity lease принадлежит не ей → исход не `committed`; операция отклоняется с `busy`/`conflict`, либо reconcile выполняется через текущего lease holder; `checkpoint_consumed` не дописывается.

### 3.3. Таблица A — prerequisites, consistency, durability

| Операция | Prerequisites (задачи S9 / компоненты) | Consistency | Durability |
|---|---|---|---|
| pause / resume | vCPU safe point, I/O submission stop (S9-28 частично) | Execution остановлена между инструкциями; guest state в памяти процесса | **Никакой.** Pause не flush и не checkpoint |
| suspend | Успешный full checkpoint (S9-29) + release resources (S9-31) | Coherent VM cut | Durable: checkpoint `C` опубликован до освобождения resources |
| guest shutdown | Guest path (ACPI/агент — компоненты отсутствуют), guest flush до backend (S9-13) | Guest-level clean shutdown только при подтверждённом guest completion | Диски — по последнему `durable_ack` guest flush |
| forced stop | Single writer (S9-04) | Disk crash-consistent; RAM/device state теряется | Только то, что уже имело `durable_ack` |
| disk-only snapshot | S9-14; для running VM — согласование с guest flush (S9-13) | Stopped VM: точное состояние диска; running VM: **crash-consistent** в заявленных границах | Snapshot root durable до ack |
| full checkpoint | S9-23…29; все participants профиля, общий barrier | Один epoch для CPU/RAM/devices/time/I/O/disks/UEFI/vTPM | Все artifacts durable → manifest durable → ack |
| restore | S9-30; validated полный набор, identity lease | Восстанавливается ровно checkpoint `C` | Новые writable children/generations durable до activation |
| clone | S9-32 (после S9-31, S9-22); identity policy | Из stopped disk/template или checkpoint; live CPU/RAM под новой identity — отклоняется по умолчанию | Clone bundle durable до ack; source не меняется |
| export | S9-20; read lease immutable root | Logical bytes выбранного immutable snapshot | Target durable и проверен до публикации как completed |
| repair (salvage/migration) | S9-21/S9-22; exclusive access к источнику | Новый target; источник неизменен | Target durable до активации; activation отдельна от copy |

### 3.4. Таблица B — переходы, cancellation, ID, отказы

| Операция | Allowed transitions | Cancellation point | Operation ID | Recoverable failure | Uncertain failure |
|---|---|---|---|---|---|
| pause | `running → paused` | Мгновенная, до достижения safe point | Нет (не persistent) | Safe point не достигнут в timeout → остаётся `running`, ошибка | Невозможна без persistent effect; при потере IPC — query состояния |
| resume (из pause, не из suspend) | `paused → running`. Resume из `suspended` — это restore (строка ниже) | До запуска vCPU | Нет | Ошибка → остаётся `paused` | Query состояния; persistent effect отсутствует |
| suspend | `running/paused → checkpointing → suspended`; при отказе до publication → прежнее `running`/`paused` либо `stopped` с прежним root | До публикации manifest | Да | Отказ до publication: checkpoint не selectable, VM продолжает/остаётся paused либо `stopped` с прежним root | Crash/потеря ответа около publication → `reconcile_required`, reconcile по operation ID/record, blind retry запрещён. Manifest опубликован и durable → `suspended` (resources освобождаются только после этого). Manifest не опубликован → VM в состоянии, из которого был запущен suspend (`running` или `paused`), если процесс жив и barrier согласованно отпущен; иначе (процесс завершён) → `stopped` с прежним persistent root (диски crash-consistent, как после forced stop), исход `failed_before_commit`; неопубликованная группа — orphan для отдельного cleanup |
| guest shutdown | `running → shutting_down → stopped` | До guest acknowledgement | Да | Timeout → `shutdown_timeout`, VM остаётся `running`; **не** успешный clean shutdown | Exit процесса без guest completion = forced stop outcome, не clean shutdown |
| forced stop | любое активное → `stopped` | Нет (немедленно) | Да | — | Состояние диска = последний `durable_ack`; outcome всегда `forced`, не clean |
| disk-only snapshot | `stopped/paused/running → то же` | До root publication | Да | Отказ до publication: snapshot отсутствует, диск без изменений | Crash/потеря ответа → `reconcile_required`, reconcile по operation ID/record, blind retry запрещён; по root generation: snapshot существует целиком или отсутствует |
| full checkpoint | `running/paused → checkpointing → прежнее` или `stopped` с прежним root | До публикации manifest | Да | Abort до publication: прежняя сессия согласованно возобновляется, иначе `stopped` с прежним root (`failed` — только без пригодного root) | Crash/потеря ответа → `reconcile_required`, reconcile по operation ID/record, blind retry запрещён; manifest опубликован/нет — по record и manifest; incomplete group не selectable и не освобождает pinned data |
| restore | `stopped/suspended → restoring → running/paused`; running/paused с той же identity → `busy` | До записи durable фазы `activating` | Да | Validation/resource error: target не запущен, `C` и прежний root не повреждены | Crash или потеря ответа (в том числе после успешной activation) → `reconcile_required`, reconcile по operation ID/record, blind retry запрещён; reconcile проверяет identity lease (S9-30) и последнюю durable фазу record по случаям (а)–(г) §3.2: (а) нет `activating` → target не запускался, `C` не consumed, опубликованные generations — orphan, lease освобождается; (б) есть `activating`/`activated`, VM активна под lease → `committed` + `checkpoint_consumed`; (в) есть `activating`/`activated`, VM не жива → `failed_after_publication`/`interrupted_activation`, `checkpoint_consumed`, VM `stopped` с новыми generations и пометкой «прерванный restore/activation»; (г) VM жива, но lease не у неё → не `committed`, `busy`/`conflict` либо reconcile через lease holder. Повтор только с тем же operation ID. Lease исключает два активных экземпляра |
| clone | Источник: состояние не меняется; новая VM → `stopped` | До публикации clone bundle | Да | Отказ: нет selectable clone, orphan cleanup отдельно | Crash/потеря ответа → `reconcile_required`, reconcile по operation ID/record, blind retry запрещён. Новая identity (UUID/MAC/disk IDs) записывается в record **до** публикации clone bundle; повтор с тем же ID — только query; clone опубликован целиком с этой identity или отсутствует. Без reconcile вторая identity не создаётся |
| export | Источник не меняет состояние (lease) | До публикации target | Да | Cancel/ENOSPC/existing target: completed export отсутствует | Crash/потеря ответа → `reconcile_required`, reconcile по operation ID/record, blind retry запрещён; target помечен completed или нет — по publication marker и record |
| repair | Источник read-only; target → новый image | До activation target | Да | Отказ: источник нетронут, target не активен | Crash/потеря ответа → `reconcile_required`, reconcile по operation ID/record, blind retry запрещён; исход activation — по record; источник не удаляется до подтверждения |

### 3.5. Обязательные различения

- Pause ≠ suspend: pause не создаёт durable состояния; отключение процесса во время pause теряет RAM state.
- Shutdown timeout ≠ clean shutdown; exit VMHost не доказывает guest shutdown.
- Resume сохраняет identity VM; clone создаёт новую identity по явной политике. Копирование диска не разрешает молча дублировать TPM identity/secrets, MAC/UUID, UEFI trust state.
- Disk rollback не меняет RAM/TPM/UEFI и не является full-VM restore. Согласованный откат disk+TPM не даёт anti-rollback и не отменяет последствий для внешнего доверия.
- Потеря ответа после commit никогда не ведёт к повторному destructive действию: suspend не освобождает resources повторно, restore не создаёт второй экземпляр, repair не активирует второй target, clone не порождает вторую identity.
- Единственный пригодный checkpoint не удаляется до успешного durable handoff: consumed пишется последней фазой record restore (§3.2; resume из `suspended` — это restore) после `activated`, а не при activation, либо при reconcile случая (в) прерванной activation; в обоих случаях публикация новых writable generations уже подтверждена record; consumed `C` нельзя повторно restore под той же identity; удаление — отдельная операция (S9-17).

## 4. Открытые вопросы

| Вопрос | Задача |
|---|---|
| Формат operation record и manifest | S9-23 |
| Политика virtual time при pause/suspend/resume | S9-25, архитектура §5 |
| Политика `EventGeneration` после restore | S9-25 |
| Единый owner полного CPU state | S9-24, ISSUE-056 |
| Межпроцессный identity lease primitive | S9-30 |
| Guest shutdown path (ACPI/agent) | Этапы 4/7, S9-31 |
