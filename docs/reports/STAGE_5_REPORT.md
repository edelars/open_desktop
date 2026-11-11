# Этап 5. Отчёт: Linux как рабочий системный стенд

Статус отчёта: **промежуточный. Этап 5 НЕ принят.** Документ фиксирует фактическую область и блокеры на 2026-11-11. Ни один сценарий этапа 5 не заявлен пройденным. Сверка выполнена с [планом этапа 5](../plans/STAGE_5.md), планами этапов 3, 4, 7, 9 и 11 и [архитектурой](../ARCHITECTURE.md).

## 1. Сводка задач

S5-00 принят после независимого архитектурного ревью; проверенный исходный коммит S5-00 — [6b61316](https://github.com/edelars/open_desktop/commit/6b613166adb8f4db3c81696c31d72bf9350c0608). Методика S5-01 подготовлена исполнителем, ожидает ревью; числовые повторы и soak duration заблокированы недостающими измерениями (ISSUE-076). Контракт S5-02 принят после независимого архитектурного ревью; проверенный исходный коммит S5-02 — `a14d08a`. Host-side backend и тесты S5-03 приняты после независимого ревью кода и контрактов; проверенный исходный коммит S5-03 — `c20e3d63595d1b7df241e83288eb43a79703fde8`. Process-exit quarantine gap остаётся открытым (ISSUE-077). Предварительный host-side headless профиль и no-guest runner S5-04 приняты после независимого ревью кода и контрактов; проверенный исходный коммит S5-04 — `aa4422bb3247e4aa24e2ec1add03e79c063ed053`; S5-05…S5-21 остаются `not_started`. Ни один сценарий A–G не принят, этап 5 не принят.

| Задача | Статус | Задача | Статус |
|---|---|---|---|
| S5-00 | `accepted` | S5-01 | `blocked` |
| S5-02 | `accepted` | S5-03 | `accepted` |
| S5-04 | `accepted` | S5-05 | `not_started` |
| S5-06 | `not_started` | S5-07 | `not_started` |
| S5-08 | `not_started` | S5-09 | `not_started` |
| S5-10 | `not_started` | S5-11 | `not_started` |
| S5-12 | `not_started` | S5-13 | `not_started` |
| S5-14 | `not_started` | S5-15 | `not_started` |
| S5-16 | `not_started` | S5-17 | `not_started` |
| S5-18 | `not_started` | S5-19 | `not_started` |
| S5-20 | `not_started` | S5-21 | `not_started` |

## 2. Границы ответственности и handoff

| Область | Владелец и задачи | Граница этапа 5 |
|---|---|---|
| NVMe-модель и guest-visible registers | Этап 7, `S7-16` | Этап 5 использует существующий контракт; не создаёт вторую модель контроллера. Предварительная host-интеграция разрешена владельцем, пока `S7-16` остаётся `in_progress` и не принятой. |
| Host-интеграция guest storage и профиль платы с диском | Этап 5, `S5-03`/`S5-04` | Этап 5 соединяет контроллер с постоянным backing и проверяет его host-side без Linux-гостя. Файловый backend живёт над `od-storage`, адаптер — в host-слое; контроллер остаётся в `od-machine`. |
| Формат, storage-семантика и долговечность | Этап 9, `S9-13`, `S9-18`, `S9-39` | Этап 9 владеет форматом и долговечностью. `S9-13` использует host-интеграцию S5-03/S5-04 как сквозной gate для guest command → backend → completion, не создавая параллельную реализацию этапа 5. `S5-17` проверяет согласованную host-side часть отказов; storage semantics остаётся у S9-18. |
| Сеть | Этап 11, `S11-30`…`S11-32` | Этап 5 задаёт и запускает сценарий F после готовности NIC и NAT; не реализует модель NIC или NAT backend. |
| Канал данных в гостя | Этап 11, `S11-20`, `S11-21`, `S11-26`, `S11-28`, `S11-29` | Этап 5 не создаёт guest agent/channel. Сценарии не должны подменять гостевые действия host-скриптами. |
| Прошивка, discovery и системные таблицы | Этап 7 и `S4-31` | Этап 5 использует готовые входные контракты; прошивка, ACPI, MCFG и PCI routing не входят в Stage 5. |
| Дисплей | Этап 10 | Не входит в рабочие сценарии этапа 5. |

По решению владельца S5-03/S5-04 поставляют host-интеграцию guest storage. Этап 9 владеет форматом и долговечностью; S9-13 применяет Stage 5 integration в своём сквозном gate, не меняя владение интеграцией и не реализуя её повторно. Аналогично S5-17 покрывает согласованную host-side часть отказов, а storage semantics остаётся у S9-18.

## 3. Внешние блокеры и ограничения

Статусы ниже сверены с планами соответствующих этапов на 2026-11-11. Предварительная интеграция не меняет ни одного статуса и не снимает наследуемые блокировки.

| Блокер | Подтверждённый статус | Последствие |
|---|---|---|
| `S4-39` — подтверждение Linux userspace | `[ ]`, не принято ([план этапа 4](../plans/STAGE_4.md)) | Гостевые сценарии остаются заблокированы до требуемого userspace профиля. |
| `S7-16` — NVMe-контроллер | `[ ]`, `in_progress`; срезы A/B выполнены исполнителем и ожидают ревью; PCI/IRQ и файловый backend не приняты ([план этапа 7](../plans/STAGE_7.md), [NVME_DEVICE.md](../specs/NVME_DEVICE.md)) | Допускается только явно предварительная интеграция. Нельзя заявлять принятый NVMe или рабочий guest disk. |
| PCI-привязка NVMe | Не реализована: class `010802`, BAR0 и MSI-X ожидаются от этапов 7 и 4 | Без PCI binding стандартный Linux `nvme` не обнаружит контроллер. |
| Маршрут прерываний NVMe | Не реализован; ожидается от этапов 7 и 4 | Без IRQ route работоспособность командного пути NVMe не доказана. |
| BAR→MMIO mapping и Command decode bits | Не входят в узко принятый `S4-30` | Нужна отдельная работа; принятие `S4-30` не закрывает эти условия. |
| `S4-31` — ACPI/discovery tables | `[ ]`, не принято; нужны MCFG, `_PRT`/`_CRS` и данные host bridge ([план этапа 4](../plans/STAGE_4.md)) | Не доказано системное обнаружение и маршрутизация NVMe в Linux-госте. |
| PCI vendor ID | Решение владельца не принято | Значение нельзя придумывать в реализации этапа 5. |
| Сеть: `S11-32`, `S11-31e` | `[ ]`, не приняты ([план этапа 11](../plans/STAGE_11.md)) | Сценарий F не может быть принят до готовности NIC и NAT измерения. |
| JIT/SMC профиль: `S3-23`, `S3-24` | Открыты; имеющиеся проверки ограничены fixtures ([план этапа 3](../plans/STAGE_3.md)) | Сценарий E не может расширять доказательство за пределы принятых CPU/JIT контрактов. |

### Перенесённые ограничения

- `S4-35`: зафиксированные хеши Linux artifacts не подтверждают provenance ядра.
- `S4-36`: initramfs собран воспроизводимым рецептом, но исполнение `/init` и полный userspace handoff не подтверждены.
- `VWC = 0`: write-through обязателен; завершённый guest `Write` должен соответствовать документированному контракту durability. Не считать неотправленный flush подтверждённым.
- Ветка `F_FULLFSYNC` → `ENOTSUP` не подтверждена на целевом стенде; не считать `fsync` эквивалентным успешной аппаратной долговечности без evidence.
- Durability namespace сейчас `unsupported`; этап 5 не меняет это объявление.
- Остановка процесса/host fault injection не является доказательством потери питания; для power-loss claim требуется отдельная приёмка `S9-39`.

## 4. Решения владельца для S5-00

| Вопрос | Решение | Применение |
|---|---|---|
| (a) Граница `S5-03`/`S5-04` и `S9-13`; `S5-17` и `S9-18` | Этап 5 интегрирует guest storage; этап 9 отвечает за формат и долговечность. | S9-13/S9-18 переиспользуют и уточняют Stage 5 integration без параллельной реализации. |
| (b) Работа до приёмки `S7-16` | Разрешена при явном статусе предварительной интеграции. | Не менять `[ ]`/`in_progress` и не заявлять controller acceptance, Linux discovery или disk scenarios. |
| (c) PCI binding и NVMe IRQ route | Ждать этапов 7 и 4. | Не добавлять эти работы в Stage 5; гостевые NVMe-сценарии заблокированы до внешних результатов. |

## 5. Проверка ссылок и планов

Идентификаторы задач S4/S7/S9/S11 и S3-23/S3-24 сверены с соответствующими файлами `docs/plans/STAGE_*.md`; статусы выше подтверждены по этим планам. Подробная отправная точка и дополнительные известные ограничения сохранены в [плане этапа 5](../plans/STAGE_5.md).

## 6. Проверки

На текущем worktree `codex`, HEAD `ada1a02`, полный запуск `cargo test --offline -p od-host -p od-storage -p od-machine` завершился с exit 0 (0 failed). Точные totals по crates агрегированы из `cargo test -- --list` и `cargo test -- --ignored --list`, выполненных после успешного прогона:

| Crate | Зарегистрировано | Passed | Ignored | Failed |
|---|---:|---:|---:|---:|
| `od-host` | 435 | 433 | 2 | 0 |
| `od-storage` | 207 | 206 | 1 | 0 |
| `od-machine` | 1167 | 1161 | 6 | 0 |
| **Всего** | **1809** | **1800** | **9** | **0** |

Отдельные видимые suite summaries включали `od-host` lib — 362 теста, `od-machine` lib — 874 passed/6 ignored, `od-storage` lib — 206 passed/1 ignored; package totals выше включают integration targets. Для списка targets использованы команды `cargo test --offline -p <crate> -- --list` и `cargo test --offline -p <crate> -- --ignored --list`; фильтр считал строки `: test`.

`python3 tools/check_layout.py` завершился с exit 1 и 45 `ERROR`, ровно на установленной baseline. Это локальные ссылки в существующих документах на отсутствующие ignored файлы под `work/stage2*` и `work/stage3*`; новых ошибок от S5-00 не добавлено. `git diff --check` завершился exit 0 без вывода.

## 7. Итоговые gates

Не закрыты. Этап 5 не принят; ни один сценарий A–G, повторная загрузка, guest disk, сеть, JIT/SMC или soak run не заявлен успешно проверенным.

## 8. Статус S5-01 (2026-11-11)

[Методика](../testing/LINUX_STAND.md) подготовлена исполнителем и ожидает независимого ревью. Срез неполон: `I_boot`/`W_boot` для целевого Linux userspace, число повторов S5-15, C+D+G cycle и длительность S5-16 не вычисляются до указанных измерений и бюджета. См. [ISSUE-076](../KNOWN_ISSUES.md#issue-076-s5-01-run-duration-inputs-missing). Checkbox S5-01 остаётся `[ ]`.

## 9. Статус S5-02 (2026-11-11)

[Host storage contract](../specs/NVME_DEVICE.md#14-постоянный-backend-на-стороне-host-s5-02) принят после независимого архитектурного ревью (gpt-6.1-sol, high); проверенный исходный коммит S5-02 — `a14d08a`; checkbox S5-02 отмечен `[x]`. Принят только контракт S5-02; этап 5 не принят. Контракт ограничен RAW host backend с `VWC = 0`, per-Write `DurableAck` и существующим generic error mapping S7-16; implementation/tests принадлежат S5-03. Explicit flush refusal в preflight блокирует attach/start без CQE и не quarantine image; runtime Flush refusal до guest mutation даёт CQE `Internal Error` и terminal adapter без quarantine; indeterminate flush error даёт sticky `ReconcileRequired`/quarantine. Flush/write failure после Write делает RAW terminal failed/quarantined до отдельно заданной inspection/reconcile; normal reopen не считается восстановлением. `S7-16` остаётся не принят, `S5-01` — blocked. Не заявляются guest integration, namespace durability или power-loss guarantee.

## 10. Статус S5-03 (2026-11-11)

Host adapter в [`persistent_nvme.rs`](../../crates/od-host/src/persistent_nvme.rs) и его тесты приняты после независимого ревью кода и контрактов (gpt-6.1-sol, high); проверенный исходный коммит S5-03 — `c20e3d63595d1b7df241e83288eb43a79703fde8`; checkbox S5-03 отмечен `[x]`. Принят только host-side prerequisite; этап 5 не принят. Он открывает существующий RAW файл через `HostFileStorage::open_read_write_exclusive`, проверяет 512-byte geometry и объявленную file-durability capability, а attachment делает только через `NvmeHandle::attach_backend` (который отвергает `CC.EN = 1`). `read_sectors`/`write_sectors` проверяют диапазон до host I/O; каждая Write выполняет `RawImage::write_aligned` → `CheckedStorage::flush_file_durable` → успешный `DurableAck` → `RawImage::reopen_read_write_checked` над тем же `CheckedStorage`. Ошибка восстановления RAW view возвращает тот же checked owner; terminal adapter удерживает его и `LOCK_EX`. `flush` также всегда вызывает file flush. Host errors возвращаются как `BackendError::Io`; выход за диапазон — `BackendError::OutOfRange`. Текущий controller mapping проверен существующим `FailingBackend` тестом: host `Io` даёт CQE `Internal Error` (`SCT=0, SC=0x06, DNR=0`); изменения `od-machine` не вносились.

При runtime Write/Flush error adapter terminally fails, сохраняет backing handle/lock и не retry/reopen. Explicit `UnsupportedDurability` на runtime Flush без прежних guest writes оставляет bytes неизменными и не ставит in-memory quarantine; ошибка после Write и любая indeterminate flush/write failure quarantine-ит текущий host lifecycle. Indeterminate `CheckedStorage` остаётся sticky `ReconcileRequired`; ошибка revalidation после ack возвращает и удерживает тот же checked owner. Это in-memory защита действует лишь пока процесс/adapter жив: после SIGKILL lock и state исчезают, а новый процесс способен открыть path без inspection/reconcile. Это открытый blocker [ISSUE-077](../KNOWN_ISSUES.md#issue-077-s5-03-raw-quarantine-not-persistent-across-process-exit); нет утверждения о содержимом или долговечности unacknowledged bytes после kill.

Проверки текущего worktree: `cargo fmt --all -- --check` — exit 0; `git diff --check` — exit 0; `cargo check --workspace --all-targets --offline` — exit 0 (2 `od-host` lib-test warnings: unused variable and unnecessary `mut`, вне нового backend/helper); `cargo test -p od-host --lib --offline` — 375 passed, 0 failed, 0 ignored; `cargo test -p od-storage --lib --offline` — 208 passed, 0 failed, 1 ignored; `cargo test -p od-machine --lib --offline a_failing_backend_completes_with_internal_error` — 1 passed, 0 failed (879 filtered out). FaultBackend моделирует явный отказ durability до mutation, отказ после Write, torn write, `StorageFull`, indeterminate flush и lost ack; теста с реально заполненным host volume нет. Для scope-сверки S5-00 его старые package totals включали integration targets и напрямую с текущими lib-only результатами не сравниваются. `python3 tools/check_layout.py` — exit 1 с 45 ERROR, не выше ранее зафиксированной baseline; все ошибки — старые локальные ссылки на отсутствующие артефакты `work/stage2*`/`work/stage3*`, новых S5-ссылок среди них нет. Команды не меняли `od-machine` и manifest crates.

Реальный macOS child-process test блокирует перед `F_FULLFSYNC` после фактического host-file write, родитель посылает SIGKILL и проверяет, что `write_sectors` не вернул успех. Это не crash-durability test: после kill состояние незавершённых bytes не читается как гарантия и не заявляется сохранным. End-to-end `F_FULLFSYNC` конкретного носителя, power-loss, namespace durability и Linux guest integration не проверялись. S5-03 принят в пределах host-side prerequisite; ISSUE-077 открыта, S7-16 не принят, S5-01 `blocked`.


## 11. Статус S5-04 (2026-11-11)

Предварительный host-side headless board profile и no-guest persistent-backend runner S5-04 приняты после независимого ревью кода и контрактов (gpt-6.1-sol, high); проверенный исходный коммит S5-04 — `aa4422bb3247e4aa24e2ec1add03e79c063ed053`; checkbox S5-04 отмечен `[x]`. Принят только host-side prerequisite; этап 5 не принят. `FirmwareBoardConfig::s5_nvme_stand()` выбирает 128 MiB RAM, polling COM1, LAPIC/IOAPIC, PM timer block, polling NVMe, без PCI config, display mappings/framebuffers, vTPM или network. Таймер стенда — `PM_TMR` на board clock; LAPIC timer, PIT и доставка IRQ отключены. Новый `display_enabled` default-on сохраняет существующие handoff golden bytes; при `false` code 4 остаётся на прежнем индексе один раз как `PRESENT = 0` с нулевыми адресами, а display MMIO/framebuffer ranges отсутствуют в bus/map. NVMe остаётся code 8 в слоте 7 и handoff v1.1; `0xFC00_1000..0xFC00_2FFF` отображается отдельным MMIO окном. Доказано `MachineMemoryMap` непересечение с RAM, ROM и другими mapped regions; PIO адреса COM1/PM находятся в отдельном пространстве, PCI conf1/ECAM не привязаны.

| Профильный ресурс | Адрес/диапазон | S5-04 состояние |
|---|---|---|
| RAM | `0x0000_0000..0x0800_0000` | 128 MiB RAM |
| COM1 | PIO `0x03F8..0x03FF` | polling UART |
| PM timer block | PIO `0x0600..0x060F` (`PM_TMR` at `0x0608`) | bound to board clock; ACPI tables/guest discovery not enabled |
| Local APIC | MMIO `0xFEE0_0000..0xFEE0_0FFF` | present; LAPIC timer and IRQ delivery disabled |
| IOAPIC | MMIO `0xFEC0_0000..0xFEC0_0FFF` | present; IRQ delivery disabled |
| PIT | PIO | disabled |
| NVMe | MMIO `0xFC00_1000..0xFC00_2FFF` | handoff code 8, slot 7, owner index 7 |
| Display registers | `0xFC00_0000..0xFC00_0FFF` | absent; unmapped hole |
| Framebuffers | primary `0xF800_0000..0xFA3F_FFFF`, second `0xF400_0000..0xF63F_FFFF` | absent; not advertised as RAM or framebuffer |
| PCI conf1 / ECAM | PIO `0x0CF8..0x0CFF`; MMIO `0xFC10_0000..0xFC1F_FFFF` | not bound |

`od-runner s5-nvme-stand --image <minor-1 firmware> --disk <new scratch path>` собирает реальную board и attached S5-03 `PersistentNvmeBackend`; `--disk` создаёт новый 8-sector/4-KiB RAW scratch image и отказывается при уже существующем пути, не перезаписывая произвольный RAW. Runner передаёт первоначально созданный `HostFileStorage` вместе с его `LOCK_EX` непосредственно в backend, без закрытия и повторного открытия пути. При более поздней ошибке новый scratch-файл может остаться. Без исполнения гостевых инструкций runner передаёт Identify Controller/Namespace, Create I/O CQ/SQ, один-sector Write, Read и Flush через `MachineBus` MMIO/RAM bindings; runner test проверяет данные backing file. Host test `backend_keeps_created_storage_identity_and_refuses_replaced_path` заменяет pathname после создания и подтверждает отказ до изменения replacement или исходного inode. FirmwareBoard test `doorbell_pending_stops_quantum_then_host_services_before_the_next_quantum` выполняет 32-bit MMIO doorbell, проверяет pending-stop predicate ровно в первой нулевой-инструкционной остановке квантa, подтверждает отсутствие CQE до host `service`, затем проверяет успешный CQE и выполнение следующего кванта до `HLT`. Команда doorbell подготовлена host test через реальную bus binding; это не утверждение, что guest CPU исполнил MMIO store. Отдельный тест `s5_nvme_stand_exposes_pm_tmr_on_board_clock_without_linux_profile` проверяет `PM_TMR` через PIO bus после продвижения board clock. Нет заявления о PCI discovery, IRQ/MSI-X, LAPIC/PIT timer, Linux guest, Windows, namespace durability или power-loss. S7-16 не принят; его ownership и API controller не менялись.

Проверки текущего worktree: `cargo test -p od-machine --lib --offline` — 878 passed, 0 failed, 6 ignored (исходник od-machine после этого прогона не менялся); `cargo test -p od-host --lib --offline` — 376 passed, 0 failed, 0 ignored; `cargo test -p od-runner --offline persistent_nvme_round_trip_uses_the_assembled_stand_bus` на `aarch64-apple-darwin` — 1 passed, 0 failed (прочие targets отфильтрованы); `cargo test -p od-runner --offline runner_refuses_an_existing_disk_path_without_modifying_it` на `aarch64-apple-darwin` — 1 passed, 0 failed (прочие targets отфильтрованы); `cargo check --workspace --all-targets --offline` — exit 0 (3 существующих warning od-host: 1 unused import в integration test, 2 unused variable/mut в lib tests); `cargo fmt --all -- --check` и `git diff --check` — exit 0. Один конкурентный прогон od-host завершился с `WriterBusy` на проверке повторного lock; точный тест и полный suite повторно прошли последовательно. `python3 tools/check_layout.py` — exit 1 с прежними 45 baseline errors по отсутствующим ignored `work/stage2*`/`work/stage3*` артефактам; новые S5-04 ссылки не увеличили count. S5-04 — `accepted` в пределах предварительного host-side профиля; S5-00/S5-02/S5-03 приняты, S5-01 `blocked`, ISSUE-077 остаётся открытой; S7-16 не принят.
