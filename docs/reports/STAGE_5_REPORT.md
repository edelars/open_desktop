# Этап 5. Отчёт: Linux как рабочий системный стенд

Статус отчёта: **промежуточный. Этап 5 НЕ принят.** Документ фиксирует фактическую область и блокеры по состоянию на 2026-11-12. Ни один гостевой сценарий этапа 5 не заявлен пройденным. Сверка выполнена с [планом этапа 5](../plans/STAGE_5.md), планами этапов 3, 4, 7, 9 и 11 и [архитектурой](../ARCHITECTURE.md).

## 1. Сводка задач

S5-00 принят после независимого архитектурного ревью; проверенный исходный коммит S5-00 — [6b61316](https://github.com/edelars/open_desktop/commit/6b613166adb8f4db3c81696c31d72bf9350c0608). S5-01 остаётся заблокированным до получения недостающих измерений для числовых повторов и soak duration (ISSUE-076). Контракт S5-02 принят после независимого архитектурного ревью; проверенный исходный коммит S5-02 — `a14d08a`. Host-side backend и тесты S5-03 приняты после независимого ревью кода и контрактов; проверенный исходный коммит S5-03 — `c20e3d63595d1b7df241e83288eb43a79703fde8`. Process-exit quarantine gap остаётся открытым (ISSUE-077). Предварительный host-side headless профиль и no-guest runner S5-04 приняты после независимого ревью кода и контрактов; проверенный исходный коммит S5-04 — `aa4422bb3247e4aa24e2ec1add03e79c063ed053`. S5-05 принят после независимого ревью сборочных рецептов и артефактов: paired tests подтвердили идентичные rootfs, initramfs и kernel hashes, `e2fsck -fn`, C compile/link/run, точную package-lock selection и initramfs mount/console handoff. Linux guest boot и disk scenario не выполнялись. S5-17 host-side failure tests приняты после независимого ревью; они не закрывают ISSUE-077 и не включают guest-visible error проверки. S5-21 имеет **blocked** статус: подготовлен воспроизводимый промежуточный static inventory, но конечный profile registry не позволяет назвать точное missing-form число; E guest JIT/SMC input ещё не определён. Гостевые binaries не запускались. [Fixture README](../../tests/fixtures/guest/stage5/README.md) содержит S5-05 команды и provenance; [S5-05 evidence](data/stage5/s5-05-rootfs-pair-review.log), [initramfs evidence](data/stage5/s5-05-initramfs-pair-review.log) и решённая [ISSUE-078](../KNOWN_ISSUES.md#issue-078-s5-05-rootfs-extraction-toolchain-blocked) содержат доказательства сборки. S5-06…S5-16 и S5-18…S5-20 остаются `not_started`. Ни один сценарий A–G не принят, этап 5 не принят.

| Задача | Статус | Задача | Статус |
|---|---|---|---|
| S5-00 | `accepted` | S5-01 | `blocked` |
| S5-02 | `accepted` | S5-03 | `accepted` |
| S5-04 | `accepted` | S5-05 | `accepted` |
| S5-06 | `not_started` | S5-07 | `not_started` |
| S5-08 | `not_started` | S5-09 | `not_started` |
| S5-10 | `not_started` | S5-11 | `not_started` |
| S5-12 | `not_started` | S5-13 | `not_started` |
| S5-14 | `not_started` | S5-15 | `not_started` |
| S5-16 | `not_started` | S5-17 | `accepted` |
| S5-18 | `not_started` | S5-19 | `not_started` |
| S5-20 | `not_started` | S5-21 | `blocked` |

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

## 13. Статус S5-17 (2026-11-11)

S5-17 принят после независимого host-side ревью; checkbox отмечен `[x]`. Гостевая проверка CQE остаётся blocked до `S4-39`; этот срез не запускает Linux guest и не принимает `S7-16`.

`S9-18` уже реализует CoW capacity/quota checks до backend calls, `zero_range`, `discard_range` и fault-injected ENOSPC по шагам CoW commit/compaction; эти ветви не дублировались. Его предел остаётся прежним: нет теста на реально заполненном host volume, и формат CoW не принят. S5-17 отдельно проверяет host RAW/backend lifecycle: `s5_17_storage_full_write_is_typed_and_preserves_readable_image` inject-ит no-write `StorageFull` существующим `FaultBackend`, ожидает typed `od_machine::BackendError::Io`, unchanged volatile/durable bytes и проверяет чтение через уже открытый test observer. Adapter на этом же случае остаётся terminal/quarantined и сам чтение/повтор записи отвергает. Это не означает разрешение на production reopen/reconcile.

Host open tests `s5_17_empty_raw_open_returns_typed_error_without_mutation_or_panic` и `s5_17_partial_sector_raw_open_returns_typed_error_without_mutation_or_panic` проверяют `PersistentNvmeOpenError::RawImage(RawError::{ZeroSize,SizeNotBlockMultiple})` и неизменность исходных файлов. RAW не имеет собственного header или ожидаемой длины, поэтому его произвольное содержимое и секторно-выровненное сокращение namespace невозможно распознать как порчу при open. `s5_17_second_lock_ex_open_returns_typed_busy_and_keeps_image_readable` получает `PersistentNvmeOpenError::WriterOpen(WriterOpenError::WriterBusy)` при втором `LOCK_EX`, пока первый адаптер сохраняет доступ к исходному образу.

Process-kill evidence переиспользует существующий изолированный child-process test `killing_between_host_write_and_durable_ack_cannot_emit_write_success`: тест удерживает реальный host-file write, останавливает child перед `F_FULLFSYNC`, посылает kill и проверяет отсутствие backend return/success. Он не читает образ после kill и ничего не утверждает о сохранности незавершённых bytes или recoverability; [ISSUE-077](../KNOWN_ISSUES.md#issue-077-s5-03-raw-quarantine-not-persistent-across-process-exit) остаётся открытой.

Текущий controller contract в [NVME_DEVICE.md §13.2](../specs/NVME_DEVICE.md) отображает `BackendError::Io` на generic `Internal Error` (`SCT=0, SC=0x06, DNR=0`); status value сверено с Linux `nvme.h`, но status conditions и DNR не проверены против NVMe Base 2.4. Гостевой CQE данного host-only среза не исполнялся.

Проверки: повторно выполнены `cargo test -p od-host --lib --offline` — 379 passed, 0 failed, 0 ignored (17.60s); `cargo test -p od-storage --lib --offline` — 208 passed, 0 failed, 1 ignored (9.47s). `git diff --check` — clean; `python3 tools/check_layout.py` — exit 1, 45 baseline errors, без новых S5 ссылок. `rustfmt --edition 2024 --check crates/od-host/src/persistent_nvme.rs` проходит; `cargo fmt --all -- --check` находит только ранее существующие formatting diffs в `crates/od-host/src/storage_abi_v1.rs` и `tools/image-tool/tests/cli.rs`, которые не менялись в S5-17. Нет evidence на реальном ENOSPC/полном томе, power loss, bytes после SIGKILL, namespace durability, гостевой ошибке или восстановлении после process exit. S5-17 принят в host-side границах; S9-18 и ISSUE-077 сохраняют свои владельцев и статусы.

## 14. Статус S5-21 (2026-11-12)

S5-21 сейчас **blocked**; checkbox остаётся `[ ]`. Воспроизводимый static inventory является промежуточным evidence slice, но не удовлетворяет полному acceptance criterion: registry неполон, 273 610 rows не получили точного disposition against accepted profiles, а guest JIT/SMC binary для E ещё не определён. Зависимость `S5-05` принята и указана в таблице плана. Проверялся только принятый S5-05 RAW rootfs SHA-256 `c2b2c94665ea256c6fa19c7eeebfc8451589028df3da00b2e1e05c7473dd2895` (2 GiB, ext4, root `/dev/nvme0n1`). Ни один ELF не запускался. Из образа через `debugfs` извлечены оболочки `bash`, `dash` (`/bin/sh`), `tar`, GCC driver, `cc1`, `collect2`, `as` и `ld.bfd`; guest paths, ELF machine, build ID и SHA-256 сохранены в [машиночитаемом inventory](data/stage5/s5-21-isa-inventory.json.gz).

Дизассемблирование выполнено GNU `x86_64-linux-gnu-objdump 2.46-3ubuntu2`, `-d --insn-width=16 -M intel`, внутри digest-pinned Linux/ARM64 builder `ubuntu@sha256:86eb1bc1059720a19a777ba5a109e9ee3690d5dfe3e319871a6d6beb86fb7f85`. Версия и SHA-256 четырёх `.deb` зафиксированы в [`stage5_isa_packages.json`](../../tools/build/stage5_isa_packages.json), проверены против подписанного `Packages.xz`; конечный profile registry — [`stage5_isa_profile_forms.json`](../../tools/build/stage5_isa_profile_forms.json), `registry_complete=false`. Команда и repeatability evidence: [s5-21-repeatability.json](data/stage5/s5-21-repeatability.json). Два полных прогона после последних parser/normalizer исправлений дали побайтно одинаковый JSON, SHA-256 `5a964f8f9323181799e3737ba2017a66847e55c21181f2581e58e1a39388bdba` для обоих выходов. Они содержат 6 510 400 instruction rows, 192 mnemonics, 560 coarse operand signatures и 43 577 encoding-shape keys; `--insn-width=16` позволил сохранить все байты длинных инструкций, continuation rows — 0. `py_compile` и `python3 tools/build/test_analyze_stage5_isa.py` — 19 passed. Нормализованный evidence exporter `py_compile` и `python3 tools/build/test_export_stage5_isa_evidence.py` — 4 passed; два сжатых выхода совпали побайтно.

| Исполняемый ELF | Инструкции | Точные encoding keys | Нормализованные формы | Unclassified rows |
|---|---:|---:|---:|---:|
| `bash` | 251 204 | 8 008 | 612 | 17 834 |
| `dash` | 20 759 | 2 307 | 392 | 1 802 |
| `tar` | 74 186 | 4 994 | 543 | 5 648 |
| `gcc-driver` | 126 633 | 8 170 | 653 | 7 473 |
| `gcc-cc1` | 5 787 291 | 39 112 | 1 181 | 228 196 |
| `gcc-collect2` | 85 743 | 6 644 | 571 | 5 800 |
| `gcc-as` | 91 696 | 5 654 | 628 | 3 569 |
| `gcc-ld.bfd` | 72 888 | 4 892 | 553 | 3 288 |

Точный encoding-shape слой сохраняет ordered prefix bytes, opcode, ModRM/SIB, operand signature, address form/size/segment и длину; byte values непосредственного и displacement поля не делят одну shape на разные forms. Отдельная нормализация сводит 43 577 shapes к 1 269 semantic instruction forms: она сохраняет semantic prefix order (`FS`/`GS`/`LOCK`/`REP`/operand-size/address-size/REX), normalizes opcode-embedded GPR numbers, сохраняет opcode/group extension, operand classes/widths, ModRM register-vs-memory и address form. Номера GPR и literal displacement/immediate bytes сами по себе не создают новые формы. Сжатый файл содержит per-binary frequencies, ELF hashes/build IDs, classification scopes, candidate frequencies и top unclassified forms.

| Сопоставление с конечным реестром | Статические строки | Толкование |
|---|---:|---|
| `local_interpreter_profile` | 6 234 439 | Bounded local interpreter evidence; не доказывает machine CPL3/Linux userspace execution. |
| `machine_cpl3_profile` | 2 351 | Точные S4-20b/d load/store row encodings; runtime page/state/entry gates неизвестны из ELF. |
| Conditional CPL0/profile candidates | 113 190 hits across 113 113 unique rows | Labels are non-exclusive; 40 candidate rows also match a supported local profile. C has 20 016 unique candidate rows, D has 108 875; C/D overlap. Candidate presence does not upgrade support. |
| `unclassified` | 273 610 | Не покрыто evidence текущего конечного registry; ISA gap candidate, не доказательство отсутствия реализации. |
| `proven_missing` | 0 | Нет exact совпадений с явными exclusions; это не утверждение полной поддержки. |

Registry помечен `registry_complete=false`: он включает документированные bounded core/local families и выбранные exact CPL0/CPL3 profiles этапов 2/4, но не задаёт disposition для каждой observed semantic form. Максимальные unclassified frequencies C (`bash`, `dash`, `tar`) — 25 284 rows; среди top individual normalized forms memory NOP `0F 1F /0` — 5 470, `ENDBR64` — 3 806, memory NOP 16-bit forms — 3 058. D (`bash`, `dash`, compiler/linker/assembler) — 267 962 rows; top individual normalized forms: `ENDBR64` — 54 400, `MOVAPS mem128,xmm` — 23 789, `MOVUPS mem128,xmm` — 17 973, `MOVQ xmm,r64` — 14 526, `MOVQ xmm,mem64` — 10 728. Здесь есть forms без CPL3 evidence, XMM/SSE формы без принятого общего профиля и формы с runtime gates, не наблюдаемыми в ELF; это не guest pass/fail и не exact missing count. Таблица приоритетов ниже отдельно суммирует variants, имеющие один mnemonic+operand signature.

C содержит 346 149 статических rows (320 865 профильно-классифицированы и 25 284 unclassified); D — 6 436 214 rows (6 168 252 профильно-классифицированы и 267 962 unclassified). Наборы не аддитивны: C использует `bash`, `dash`, `tar`; D включает те же `bash`/`dash` плюс toolchain. Это workload relevance фактических ELF, не доказательство исполнения внутри гостя.

Частотный приоритет для Stage 2/4: сначала определить применимый CPL3 disposition для `ENDBR64` и часто встречающихся XMM/SSE forms; далее зафиксировать CPL0/CPL3 границу memory NOP, для которых найдено 54 671 статическое candidate row (C 14 705, D 51 681). По точным opcode/prefix/operand-shape кандидатам линии `S4-38L7`: FS/GS qword load — 1 213 rows всего (C 689, D 1 030); FS/GS qword SUB — 1 861 (C 813, D 1 640); LOCK CMPXCHG qword — 36 (C 0, D 36); других L7 candidate форм в наборе не найдено. C/D числа пересекаются по `bash`/`dash`, их нельзя складывать. Это эвристика приоритета, не подтверждение прохождения сценария или удовлетворения runtime gates.

| Следующий вопрос/работа | Наблюдаемые частоты C / D | Приоритет и владелец |
|---|---:|---|
| Определить CPL3 disposition для `ENDBR64` (текущий S2-48 evidence условен CPL0) | 3 806 / 54 400 candidate rows | Высокий; Stage 2 определяет конечное применимое profile, Stage 4 проверяет machine integration. |
| Определить bounded CPL3 XMM/SSE subset (`MOVAPS mem128,xmm`, `MOVUPS mem128,xmm`, `MOVQ xmm,r64`, `MOVQ xmm,mem64`) | 785 / 25 005; 495 / 20 829; 112 / 14 526; 130 / 12 288 | Высокий для D compiler workload; Stage 2 ISA profile, Stage 4 machine profile. Частоты объединяют отдельные нормализованные prefix/address variants по mnemonic+operand signature. |
| Уточнить CPL0-vs-CPL3 status memory multi-byte NOP (`S4-38L4d`) | 14 705 / 51 681 candidate rows | Средний; Stage 4. Candidate rows не равны user-mode acceptance. |
| Проверить observed `S4-38L7i` FS/GS qword load candidates | 689 / 1 030 | Средний; Stage 4, после определения state/paging gates. |
| Проверить observed `S4-38L7j` FS/GS qword SUB candidates | 813 / 1 640 | Средний; Stage 4, после определения prefix/state/paging gates. |
| Проверить observed `S4-38L7k` LOCK CMPXCHG qword candidates | 0 / 36 | Следующий после qword-load gate; Stage 4. |
| Сценарий E JIT/SMC guest executable | Не измеримо / не измеримо | Не назначать нулевую частоту; S5-06 определяет guest program, S3-23/S3-24 остаются JIT gates. |

E имеет статус `not_yet_defined/unmeasurable`: в принятых статических входах нет guest JIT/SMC программы или её runtime-generated code. Не присваиваем E нулевую частоту; S5-06 ещё должен определить guest binary, а JIT-профиль зависит от S3-23/S3-24. Гостевая система и исследованные ELF не boot/run-tested. Registry limitation и следующий шаг Stage 2/4 указаны в [ISSUE-079](../KNOWN_ISSUES.md#issue-079-s5-21-accepted-profile-registry-is-incomplete).
