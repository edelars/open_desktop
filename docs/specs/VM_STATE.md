# Состояние VM, lifecycle и операции сохранения

Статус: **S9-02 принято с оговорками по архитектурному ревью; S9-01 принято (сверка §2 с кодом HEAD `4dedff2` выполнена; ISSUE-057 в prototype CoW закрыт, `01ef35d`; следующий срез — см. §2.5)**. Инвентаризация выполнена по фактическому коду на 2026-11-05 (ветка `feat/s9-stage`). Операции VM из §3 не реализованы (нет lifecycle, checkpoint, restore и clone); реализованы только offline storage-компоненты §2.1 (RAW, prototype CoW write transaction и recovery на уровне `od-storage`), которые операциями VM не являются. Versioned full-VM container — TBD S9-23.

Связанные документы: [план этапа 9](../plans/STAGE_9.md), [STORAGE_FORMAT.md](STORAGE_FORMAT.md) (уровни подтверждения записи, модель отказов), [архитектура §7](../ARCHITECTURE.md#снимки), [виртуальная плата](VIRTUAL_BOARD.md), [контракт машины](MACHINE_CONTRACT.md), [методика recovery](../testing/STORAGE_RECOVERY.md).

## 1. Участники полного состояния VM

Полный checkpoint профиля `full-windows-vm` ([профили](STORAGE_FORMAT.md#2-профили)) охватывает одним consistency cut: все vCPU, RAM, device/DMA writers, interrupts, virtual time, pending/completed I/O и persistent generations дисков, UEFI variables и vTPM. Derived state (JIT cache, TLB, native pointers, host handles) не сохраняется и воссоздаётся с новыми generations.

## 2. Инвентаризация prerequisites (S9-01)

Метод: `ls`/`rg` по `crates/*/src`, `crates/*/Cargo.toml`, `apps/vmhost`, `firmware/uefi`, `guest/`, `protocols/`, `tools/` на ветке `feat/s9-stage` (HEAD `4dedff2`, 2026-11-05). Колонка «Versioned save/load» отвечает только на вопрос, существует ли в коде версионированный codec состояния VM. **Ни у одного участника полного состояния VM (§1) его нет.** Единственный версионированный on-disk формат в коде — прототип CoW (`MAGIC = b"ODCOWP\0\x01"` и поле version в root slot), он не нормативен и собирается только при `cfg(any(test, feature = "test-support"))`.

### 2.1. Storage и host I/O

| Компонент | Фактический owner / код | Versioned save/load | Blocker и следующая задача |
|---|---|---|---|
| Storage I/O contract и RAW | `od-storage`, production-модули: [`io.rs`](../../crates/od-storage/src/io.rs) (`StorageBackend`, `CheckedStorage`, `StorageCapabilities`, `DurabilityScope`, `DurableAck`, `flush_file_durable`/`flush_namespace_durable`; S9-03) и [`raw.rs`](../../crates/od-storage/src/raw.rs) (`RawImage`: `create`/`open_read_write`/`open_read_only`, `read_at`/`write_at`, aligned-варианты; S9-05) | Не применимо: RAW — bytes диска без метаданных состояния | Power-loss не проверен; S9-03/05 не отмечены принятыми в плане |
| Fault backend и prototype CoW | `od-storage`, только `cfg(any(test, feature = "test-support"))`: [`testing/`](../../crates/od-storage/src/testing/mod.rs) (`contract`, `fault`, `memory`, `rng`; S9-07), [`cow_proto.rs`](../../crates/od-storage/src/cow_proto.rs), [`cow_parse.rs`](../../crates/od-storage/src/cow_parse.rs), [`cow_tx.rs`](../../crates/od-storage/src/cow_tx.rs) (S9-08…12). Snapshots, branch и цепочка предков — прототип в [`cow_snapshot.rs`](../../crates/od-storage/src/cow_snapshot.rs) (S9-14…16, `7188582`, `33440be`); production `ParentResolver` нет | Prototype-формат с magic/version, не нормативный; в production сборке недоступен | Формат не принят: [ISSUE-057](../KNOWN_ISSUES.md#issue-057-s9-cow-zeroed-gen2-slot-indistinguishable) закрыт (`01ef35d`: create пишет два пустых корня под одним barrier); S9-40 выполнен частично — замер commit-latency ([S9_40_COMMIT_LATENCY.md](../testing/S9_40_COMMIT_LATENCY.md)), замеров RAW против CoW по throughput, чтению, write amplification, recovery и памяти нет |
| Host file I/O, durability, locking | `od-host` (production-зависимости `od-isa`, `od-jit`, `od-storage`; `test-support` только в dev-dependencies). [`storage_io.rs`](../../crates/od-host/src/storage_io.rs): `HostFileStorage` (`create_new_exclusive`, `open_read_write_exclusive` с `flock(LOCK_EX\|LOCK_NB)` и identity `(st_dev, st_ino)`; `open_read_only` без lock), `ReadLease` (`flock(LOCK_SH\|LOCK_NB)`, `31b8c2a`), flush данных файла через `fcntl(F_FULLFSYNC)` только на macOS; `namespace_durability = false` везде. RAW на реальных файлах — тесты [`raw_file_tests.rs`](../../crates/od-host/src/raw_file_tests.rs) | Нет | S9-04 частично: реализованы busy-политика export/repair и `ParentLease` ([`storage_policy.rs`](../../crates/od-host/src/storage_policy.rs), `bc7b675`); проверка цепочки предков реализована в `od-storage` (S9-16), но production `ParentResolver` нет; нет интеграции с VMHost lifecycle и non-advisory lock на сетевых ФС (у VMHost нет дисков). S9-06 частично: namespace durability ([ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed)); `flock` advisory |
| CLI image-tool | `tools/image-tool`: `od-image-tool` `info`/`check` (read-only, коды выхода 0…5; S9-19, `1590c9e`), зависит от прототипа с feature `test-support`; repair-подкоманды нет; export и salvage — `fd5ebcf` (S9-20/21) | — | S9-22 |
| Guest storage device и guest storage commands | Отсутствуют: в `crates/`, `apps/`, `firmware/` нет NVMe, PCI или иного storage device (`rg -i nvme\|pci` пуст); единственная production-реализация `BusDevice` — UART. Плата планирует NVMe поверх PCIe ([VIRTUAL_BOARD](VIRTUAL_BOARD.md)) | Нет | Блокирует S9-13; требует PCI configuration (S4-30) и диска этапа 5 roadmap |
| UEFI NVMe Block I/O | `firmware/uefi/src/` — только `.gitkeep` | — | Этап 7 roadmap |

### 2.2. CPU, RAM, устройства, время

| Компонент | Фактический owner / код | Versioned save/load | Blocker |
|---|---|---|---|
| Архитектурный CPU state | `od-isa::LongModeState` ([`state.rs`](../../crates/od-isa/src/state.rs)): `gpr[16]`, RIP, RFLAGS, `cr0`/`cr4` (`Option`), FS/GS base, `kernel_gs_base` (`Option`), selectors FS/GS/DS/ES и `Option` CS/SS, частичные descriptor caches (CS/SS/DS/ES/FS/GS), GDTR/IDTR, x87, SSE, STI shadow, activity. Многие поля `Option` и заполняются только отдельными bounded профилями. Legacy `FragmentState` (4 GPR, alias `CpuState`) в [`lib.rs`](../../crates/od-isa/src/lib.rs) | Нет: только `Clone`/`Debug`/`PartialEq` | Нет CR2, CR3, CR8, EFER, DR*, LDTR, TR, XSAVE/AVX, general MSR, pending exception/event state. CR3 и EFER (и копии CR0/CR4) передаются отдельно через `od-machine::Long64ProbeConfig::new(cr0, cr3, cr4, efer)` ([`page_walk.rs`](../../crates/od-machine/src/page_walk.rs)); MSR — только узкие профили `MsrProfile` (SYSENTER probe, FS/GS/KERNEL_GS base, синтетический `EferReadOnlyV1`; [`msr.rs`](../../crates/od-isa/src/msr.rs)). Единого owner полного CPU state нет ([ISSUE-056](../KNOWN_ISSUES.md#issue-056-s9-cpu-state-not-single-owner)). Этап 4 (S4-12, S4-18…21) открыт; SMP и план выделения состояния vCPU описаны в [этапе 6](../plans/STAGE_6.md), реализация не принята. |
| vCPU lifecycle | `od-machine::DiagnosticVcpuSession` ([`diagnostic_vcpu.rs`](../../crates/od-machine/src/diagnostic_vcpu.rs)): `Runnable`/`Paused`/`Stopped`/`Terminal`, synchronous caller-driven | Нет | Диагностический boundary, не полный lifecycle ([контракт](MACHINE_CONTRACT.md#bounded-diagnostic-vcpu-session-boundary)); S4-21 открыт. Переход к нескольким vCPU описан в [плане этапа 6](../plans/STAGE_6.md), реализация не принята. Нет safe-point barrier для S9-28 |
| Guest RAM | `od-machine::PhysicalMemory` (приватные `regions`) и `MachineMemoryMap` ([`physical_memory.rs`](../../crates/od-machine/src/physical_memory.rs)); `PhysicalMemory` принадлежит `MachineBus` | Нет; нет dirty tracking | Owner есть, codec нет → S9-24 (stopped full-RAM copy) |
| TLB/translation cache | `od-machine::TranslationProbeCache` — модуль и реэкспорт только за feature `experimental-translation-probe-cache` | Не сохраняется (derived) | Rebuild при restore; полный TLB (S4-13/14) не принят |
| Bus и device registry | `od-machine::MachineBus` ([`bus.rs`](../../crates/od-machine/src/bus.rs)): `memory: PhysicalMemory`, `devices: Vec<Box<dyn BusDevice>>`, MMIO/port bindings; trait `BusDevice` имеет только `read`/`write` | Нет: нет save/validate/load в trait | S9-25 должен добавить participant contract |
| UART 16550 (polling TX) | `od-machine::Uart16550PollingTx` ([`uart16550.rs`](../../crates/od-machine/src/uart16550.rs)): приватный `UartState` за `Arc<Mutex>`; `Uart16550Handle` alias того же состояния | Нет. Приватный `snapshot()` используется только в `PartialEq` | Codec нет; alias-handles должны быть остановлены barrier'ом до capture (S9-28) |
| UART RX event pump | `od-machine::UartRxEventPump` ([`uart_rx_events.rs`](../../crates/od-machine/src/uart_rx_events.rs)): собственный `EventQueue`, payloads и `Uart16550Handle` | Нет | S9-25 |
| Virtual clock / TSC | `od-machine::VirtualClock` ([`virtual_clock.rs`](../../crates/od-machine/src/virtual_clock.rs)): приватные `now_ns`, `tsc_hz`, `tsc`; caller-driven (`advance_time_ns`) | Нет | Политика времени при pause/suspend не задана ([архитектура](../ARCHITECTURE.md#планирование-и-устройства)); S4-22 открыт |
| Event queue / deadlines | `od-machine::EventQueue` ([`event_queue.rs`](../../crates/od-machine/src/event_queue.rs)): приватные entries `(key, deadline_ns, sequence)`, `next_sequence`, `capacity` и `EventGeneration(u64)` с приватным полем (тип реэкспортирован в `lib.rs`). `reset(generation)` выдаёт новую generation; тест `reset_invalidates_extracted_event_even_when_queue_was_empty` подтверждает, что ранее извлечённый `ScheduledEvent` становится stale | Нет | Codec нет. Restore должен начинаться с новой generation (механизм `reset` есть, ошибка `GenerationExhausted` возможна) и заново построить deadlines из сохранённых данных. S4-23 открыт |
| Local APIC, IOAPIC, APIC timer, PCI | Local APIC (с таймером) и IOAPIC в `od-machine` есть (`local_apic.rs`, `apic_pair.rs`, `io_apic.rs`); их состояние в режиме чередования снимается строкой ниже (S6-24), в формат состояния машины оно не входит. PCI — по-прежнему отдельный срез (S4-30). Прежнее «Отсутствуют» устарело | — | Этап 4; формат — S9-24/S9-25 |
| Состояние vCPU и LAPIC в режиме чередования (S6-24) | `od-machine`: `vcpu.rs` (`Vcpu` — значение, `Clone`/`Eq`), `apic_pair.rs` (`PairState`: все LAPIC с таймерами, `EventQueue` со сроками и Remote IRR маршрута IOAPIC), `smp_scheduler.rs` (`stop_all`, `SafePoint`, `SchedulerState`), `vm_snapshot.rs` (`VmSnapshot`) | Нет: значение в памяти процесса, формат файла не определён | Снимается на остановленной машине (каждый vCPU подтвердил safe point) и восстанавливается только в ту же машину; RAM, остальные устройства платы и часы в нём нет (маршрут IOAPIC в снимке есть; часы — сторож: время на остановке стоит). Восстановление возвращает generation таймеров к снятому значению: это безвредно, потому что `ScheduledEvent` границу остановки не переживает, а диспетчеризация берёт generation заново (`local_apic.rs`, `dispatch_timer`). Состояние планировщика при `record_turns` несёт копию всей записи ходов: снимок и сравнение стоят столько, сколько ходов записано. Формат, версии, manifest — S9-24/S9-25; барьер с вводом-выводом — S9-28; ISSUE-056 (единый версионированный владелец состояния CPU) остаётся открытой. Контракт: [MACHINE_CONTRACT](MACHINE_CONTRACT.md#s6-24--остановка-всех-vcpu-снимок-и-сброс) |
| Состояние vCPU и LAPIC на машине, остановленной параллельным режимом (S6-35) | `od-machine`: `parallel_run.rs` (`ParallelControl::stop_all`/`resume`/`with_vcpu`, `StopProof`), `vm_snapshot.rs` (`VmSnapshot::take_parallel`), `stage6_smp_probe.rs` (`run_parallel_to_a_stop`) | Нет: то же значение в памяти процесса (`VmSnapshot`), формат файла не определён | Снимается только при `StopProof` действующей остановки всех vCPU (каждый подтвердил safe point); ожидающие события LAPIC (INIT/SIPI/NMI, IRR/ISR, таймеры со сроками, Remote IRR) — в снимке; RAM, остальные устройства платы и часы — нет. Восстановление — только в ту же машину и в режиме чередования (`ParallelControl` не восстанавливает); детерминизм параллельного режима не заявляется. Контракт: [MACHINE_CONTRACT](MACHINE_CONTRACT.md#s6-35--состояние-на-остановленной-параллельной-машине) |
| NIC, xHCI/HID, GOP | Отсутствуют | — | Этапы 5/7 |

**Что этап 6 передаёт `S9-24`, `S9-25`, `S9-28` (S6-24, S6-34, S6-35) и что остаётся за ними.**

| Передаёт этап 6 | Кому | Что остаётся за получателем |
|---|---|---|
| Состояние всех vCPU и всех Local APIC с IOAPIC-маршрутом (ожидающие INIT/SIPI/NMI, IRR/ISR/TMR, таймеры с абсолютными сроками и `EventQueue`) как значение в памяти процесса: `VmSnapshot` ([vm_snapshot.rs](../../crates/od-machine/src/vm_snapshot.rs)), в режиме чередования и в параллельном | `S9-24` (состояние CPU), `S9-25` (участники: APIC, IOAPIC, таймер) | Формат файла и версии, manifest, кодек `VmSnapshot`; единый версионированный владелец состояния CPU ([ISSUE-056](../KNOWN_ISSUES.md#issue-056-s9-cpu-state-not-single-owner)); RAM (`S9-24`: копия остановленной RAM, dirty tracking); устройства платы (UART, PIT, PIC, ACPI PM, NVMe, дисплей: `save`/`validate`/`load` в `BusDevice`, `S9-25`); восстановление в **новую** машину (в S6 только в ту же, с теми же RAM и устройствами); часы и TSC как часть состояния (в S6 они стоят и служат сторожем, не сохраняются) |
| Остановка всех vCPU с подтверждением каждого на safe point и ограниченным временем: `SmpScheduler::stop_all`/`resume` (режим чередования), `ParallelControl::stop_all`/`resume` (параллельный режим, один поток на vCPU; `StopProof` — доказательство остановки); IPI и прерывание остановленному vCPU остаются ожидающими в его LAPIC; виртуальное время на остановке стоит | `S9-28` (barrier) | Барьер с вводом-выводом: остановка в полёте DMA/NVMe/дисплея, фиксация очередей устройств, порядок «vCPU, затем устройства»; состояние vCPU внутри host-кода JIT ([`statepoints`](../../crates/od-jit/src/statepoints.rs)); пауза со стороны host-сессии и протокола управления (S9-33) |
| Проверки: таймер не потерян и не сработал дважды, ожидающее прерывание (IRR) не потеряно ни при остановке, ни при сбросе комплекса и восстановлении в той же машине ([S6-24](MACHINE_CONTRACT.md#s6-24--остановка-всех-vcpu-снимок-и-сброс), [S6-35](MACHINE_CONTRACT.md#s6-35--состояние-на-остановленной-параллельной-машине)) | `S9-24`, `S9-25` | Повторить те же проверки поверх файла и восстановления в новую машину; сквозная проверка S6-35 с форматом снимка — blocked до `S9-24`, `S9-25` |
| — | — | Детерминизм: режим чередования с seed воспроизводим побайтно (S6-24), параллельный режим недетерминирован, и снимок на нём не делает его детерминированным; требование «восстановление даёт тот же ход» в параллельном режиме не заявляется |

Строка «versioned save/load» в таблицах выше остаётся «Нет»: этап 6 не определяет формат файла и не объявляет `VmSnapshot` снимком машины (в нём нет RAM и устройств).

### 2.3. JIT, host session, VMHost

| Компонент | Фактический owner / код | Versioned save/load | Комментарий |
|---|---|---|---|
| JIT caches | `od-jit` ([`dispatch.rs`](../../crates/od-jit/src/dispatch.rs), [`memory_cache.rs`](../../crates/od-jit/src/memory_cache.rs)): `clear`, `invalidate`, `reset_for_execution_context` | Не сохраняется (derived) | Пригодно для сброса после restore; production JIT↔memory связь (S4-15) не принята |
| JIT state points | `od-jit::statepoints::SnapshotAdmissibility` ([`statepoints.rs`](../../crates/od-jit/src/statepoints.rs)) | — | Описывает, где canonical guest state материализован в host-коде. Это вход для будущего barrier S9-28, **не** формат checkpoint |
| `ExecutionContextSnapshot` | `od-jit::translation` | — | `id + profile` контекста трансляции; не snapshot машины |
| Executable mappings | `od-host` | Не сохраняется | Host handles никогда не persistent |
| Host session ABI v1 | [HOST_SESSION_ABI_V1.md](HOST_SESSION_ABI_V1.md) | — | Сессия S0-09; нет pause/suspend/checkpoint/restore |
| Control protocol v1 | [CONTROL_PROTOCOL_V1.md](../../protocols/control/CONTROL_PROTOCOL_V1.md) | — | Только HELLO, READY, RUN, RUN_ACK, STOP, STOP_ACK, RESULT, ERROR; без reconnect и retry. Новые команды — только в S9-33 |
| VMHost | [`apps/vmhost/Sources/VMHostMain.swift`](../../apps/vmhost/Sources/VMHostMain.swift) | — | Нет VM bundle, persistent identity и межпроцессного identity lease |
| Testkit `*Snapshot`, `save()` | `od-testkit` ([`result.rs`](../../crates/od-testkit/src/result.rs), [`v2.rs`](../../crates/od-testkit/src/v2.rs)), serde JSON | — | Fixture-структуры CPU oracle cases. `od-testkit` — только dev-dependency `od-host` (и зависимость `tools/bench`); production crates от него не зависят; **не** сериализация машины |

### 2.4. Firmware, vTPM, guest

| Компонент | Код | Статус |
|---|---|---|
| UEFI variables | `firmware/uefi/src/var/`, `od-machine::NvramDevice` | Снимок двух банков `ODNVBANK` v1/v2, создание платы из байтов и первичная настройка защищённых ключей доступны. Persistence в bundle и согласованный checkpoint входят в S9-26. [Контракт NVRAM](NVRAM_DEVICE.md#host-api) |
| vTPM 2.0 | `od-tpm`, CRB и host API в `od-machine` | Доступен снимок постоянного состояния `ODTPMSTA` v5 между квантами. Читаются v3, v4 и v5. Восстановление идёт через конфигурацию новой платы. S9-27 определяет шифрование, хранение, согласованный захват и политику отката. [Контракт TPM](VTPM.md#host-api-и-persistence) |
| Guest agents | `guest/linux/agent/src/`, `guest/windows/agent/src/` — `.gitkeep`; `protocols/guest/` — `.gitkeep` | Отсутствуют; S9-34 опционален |

Контракты в этой таблице отражают текущую реализацию. Для UEFI сохраняются байты зафиксированных банков. Для TPM сохраняются только поля persistent state, а PCR, сессии и прочее состояние работающего экземпляра требуют самостоятельного контракта захвата. Интерфейсы Secure Boot приняты по S7-24. Полный профиль PTP 1.07 проходит S7-23. [Передача форматов и границ](../plans/STAGE_7.md#s7-33-передать-контракты-persistence-этапу-9).

### 2.5. Вывод S9-01

**Storage-prerequisites (есть в коде, не зависят от §2.2–§2.4).** `od-storage` не импортирует `od-machine`/`od-isa`/`od-jit`/`od-host`. Реализованы: I/O contract и RAW (production), host file backend с exclusive writer lock, `ReadLease`, busy-политика export/repair с `ParentLease` и file-data flush на macOS (`od-host`), fault backend и prototype CoW create/open/parse/transaction/recovery (только `test-support`). Не реализованы: namespace durability, snapshots/parents, сами операции export/repair (S9-20/21; есть только busy-политика примитивов), image-tool, storage-замеры.

**Full-VM prerequisites (заблокированы).** S9-13 — нет guest storage device и PCI (S4-30, этап 5). S9-24 — нет единого versioned owner CPU state ([ISSUE-056](../KNOWN_ISSUES.md#issue-056-s9-cpu-state-not-single-owner)), у RAM нет codec. S9-25 — `BusDevice` без save/load, APIC/IOAPIC/timer отсутствуют (S4-22, S4-23, S4-25…28). S9-26/27 — доступны форматы постоянного состояния и API из §2.4. Нужны защита, политика отката и включение этих owners в согласованную операцию. S9-28 — нет safe-point barrier (S4-21). S9-34 — нет guest agents и `protocols/guest`. S9-23 (manifest contract и local tests) можно проектировать заранее, но S9-24…27 требуют реальных owners.

**Следующий runnable storage slice: дополнить S9-40 недостающими замерами RAW против CoW (throughput, чтение, write amplification, recovery, память) для принятия формата S9-08.** ISSUE-057 в prototype CoW закрыт (`01ef35d`), commit-latency измерена ([S9_40_COMMIT_LATENCY.md](../testing/S9_40_COMMIT_LATENCY.md)); без полных замеров размеры блока и карты остаются гипотезами. Срез затрагивает `od-storage`/`tools/bench`, зависимости (S9-07…12) есть в коде. Почему не другие срезы: остаток S9-04 требует production `ParentResolver` и дисков VMHost; namespace-часть S9-06 ждёт проверяемого источника ([ISSUE-055](../KNOWN_ISSUES.md#issue-055-s9-host-durability-guarantee-unconfirmed)); S9-13 заблокирован устройством; S9-22 (migration) начинать после принятия формата.

Задачи этапов 5–8 указаны по roadmap без ID, так как планов этих этапов нет.

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
| guest shutdown | Guest path (ACPI/агент — компоненты отсутствуют), guest flush до backend (S9-13) | Guest-level clean shutdown только при подтверждённом guest completion | Состояние дисков не старше последнего `durable_ack` guest flush (completed-but-unsynced может пережить остановку) |
| forced stop | Single writer (S9-04) | Disk crash-consistent; RAM/device state теряется | Состояние диска не старше последнего `durable_ack`; completed-but-unsynced записи могут как пережить остановку, так и пропасть |
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
| forced stop | любое активное → `stopped` | Нет (немедленно) | Да | — | Состояние диска не старше последнего `durable_ack` (completed-but-unsynced может пережить остановку); outcome всегда `forced`, не clean |
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
