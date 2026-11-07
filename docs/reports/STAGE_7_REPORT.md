# Этап 7. Отчёт: собственные UEFI, ACPI и платформа для Windows

Статус отчёта: **промежуточный. Этап 7 НЕ принят.** Документ фиксирует только то, что подтверждено коммитами и проверками на ветке `feat/s9-stage` (исходная точка — коммит `bacffa0`, план этапа 7). Статус S7-00 в таблице — `not_started` до ревью (статус ставит ревью, не исполнитель); остальные задачи S7-NN не начаты. Наличие плана, этого отчёта или записей в карте проекта не закрывает ни одну задачу.

Связанные документы: [план этапа 7](../plans/STAGE_7.md), [этап 7 дорожной карты](../ROADMAP.md#этап-7-собственные-uefi-acpi-и-платформа-для-windows), [виртуальная плата](../specs/VIRTUAL_BOARD.md), [состояние VM](../specs/VM_STATE.md), [реестр проблем](../KNOWN_ISSUES.md), [отчёт этапа 9](STAGE_9_REPORT.md), [отчёт этапа 11](STAGE_11_REPORT.md).

## 1. Сводка

Всего задач S7-00…S7-41 по плану: 38 (S7-00…S7-35, S7-40, S7-41).

| Статус | Количество | Задачи |
|---|---|---|
| Принято | 0 | — |
| `not_started` | 38 | все задачи, включая S7-00 (выполнена исполнителем, ожидает ревью), см. таблицу ниже |

Чекбоксы `[ ]` в плане не отмечены: по `STAGE_7.md` §0 отмечаются только после ревью.

## 2. Таблица задач

| Задача | Статус | Коммит | Ревью | Комментарий |
|---|---|---|---|---|
| S7-00 | not_started | — | — | Scope, отчёт этапа, обновление платы и карты, запись переноса пунктов roadmap (выполнено исполнителем, ожидает ревью) |
| S7-01 | not_started | — | — | Сборка прошивки и EFI-приложений, host-side тесты, методика |
| S7-02 | not_started | — | — | Контракт входа: окна, handoff, manifest платы, ABI/состояние вызова, таймеры, ROM, RT relocation |
| S7-03 | not_started | — | — | ROM backing/shadow, загрузчик образа, handoff block, профиль платы и runner |
| S7-04 | not_started | — | — | Ядро прошивки: вход, таблицы страниц, аллокатор, memory map, COM1 |
| S7-05 | not_started | — | — | System Table, Boot Services, база протоколов, события, `ExitBootServices` |
| S7-06 | not_started | — | — | PE/COFF loader, Authenticode-дайджест, `LoadImage`/`StartImage`/`Exit`, Loaded Image, Device Path |
| S7-07 | not_started | — | — | Консоль: Simple Text Output/Input на COM1, ConIn/ConOut/StdErr |
| S7-08 | not_started | — | — | Первый сквозной срез: встроенные приложения → COM1 → эталон |
| S7-09 | not_started | — | — | Variable services (volatile) и versioned store codec |
| S7-10 | not_started | — | — | NVRAM-устройство в `od-machine`, драйвер прошивки, NV-переменные через сброс |
| S7-11 | not_started | — | — | Runtime Services: время, `SetVirtualAddressMap`, `ResetSystem`, RT-переменные |
| S7-12 | not_started | — | — | Собственный AML-кодировщик и DSDT из manifest |
| S7-13 | not_started | — | — | ACPI (RSDP/XSDT/FADT/MADT/MCFG/HPET/TPM2/DSDT) и SMBIOS из handoff block, configuration tables |
| S7-14 | not_started | — | — | ACPI fixed hardware в `od-machine`: PM1, PM timer, reset register, RTC/CMOS, HPET |
| S7-15 | not_started | — | — | GOP поверх собственного дисплейного устройства |
| S7-16 | not_started | — | — | Минимальный NVMe-контроллер в `od-machine` |
| S7-17 | not_started | — | — | Драйвер NVMe прошивки: Block I/O, Disk I/O, GPT partitions |
| S7-18 | not_started | — | — | Собственная FAT и Simple File System |
| S7-19 | not_started | — | — | Boot manager: `BootOrder`/`Boot####`, `\EFI\BOOT\BOOTX64.EFI`, загрузка приложения с ESP |
| S7-20 | not_started | — | — | ADR: размещение vTPM, криптозависимости host и `no_std` прошивки, профиль команд, CRB, состояние |
| S7-21 | not_started | — | — | Процессор команд TPM 2.0, фаза A: startup, capability, PCR, hash, random, password session |
| S7-22 | not_started | — | — | CRB-устройство, таблица TPM2, `EFI_TCG2_PROTOCOL`, measured boot и event log (PCR0–5) |
| S7-23 | not_started | — | — | TPM фаза B: иерархии, ключи RSA/ECC, сессии, NV, persistent state v1 |
| S7-24 | not_started | — | — | Secure Boot: authenticated variables, PK/KEK/db/dbx, Authenticode в `LoadImage`, PCR7 |
| S7-25 | not_started | — | — | Проверка Secure Boot и отказов TPM/Secure Boot на собственных приложениях |
| S7-26 | not_started | — | — | Сброс и перезагрузка: `ResetSystem` → сброс машины, NVRAM/TPM переживают сброс |
| S7-27 | not_started | — | — | Измерения пути загрузки |
| S7-28 | not_started | — | — | Отказы без TPM/Secure Boot: handoff, NVRAM, PE, GPT, FAT, отсутствующий диск, EBS |
| S7-29 | not_started | — | — | Промежуточное ревью и приёмка gates G1–G3 |
| S7-30 | not_started | — | — | Аудит фактических требований Windows boot path к прошивке и платформе |
| S7-31 | not_started | — | — | Linux через собственную UEFI |
| S7-32 | not_started | — | — | Установочный образ Windows через собственную UEFI до экрана Setup |
| S7-33 | not_started | — | — | Передача контрактов persistence этапу 9 (S9-26, S9-27) |
| S7-34 | not_started | — | — | USB HID клавиатура прошивки для `ConIn` |
| S7-35 | not_started | — | — | Транспорт установочного носителя: El Torito/UDF или конвертация — решение и реализация |
| S7-40 | not_started | — | — | Независимое итоговое ревью ступеней 1–6 |
| S7-41 | not_started | — | — | Документация и приёмка ступеней 1–6 |

## 3. Задачи

Разделы по задачам добавляются вместе с самой задачей. Раздел S7-00 ниже описывает подготовительные документы; по таблице выше он `not_started`: выполнено исполнителем, ожидает ревью (статус ставит ревью).

### S7-00. Scope, отчёт и плата

Изменены: этот отчёт (новый), `docs/specs/VIRTUAL_BOARD.md`, `docs/ROADMAP.md`, `docs/PROJECT_LAYOUT.md`, `docs/specs/VM_STATE.md`. Прошивка, Cargo-манифесты и каталоги `firmware/uefi/` не создавались. Проверки и результат ревью — в сопроводительном отчёте исполнителя; записи о коммите и ревью добавляются после ревью.

#### Решения владельца проекта от 2026-11-07

Решения получены от владельца проекта 2026-11-07 в ответ на вопросы S7-00, S7-20 и S7-24 и записаны в [плане, §1.1](../plans/STAGE_7.md#11-решения-владельца-проекта-2026-11-07). Пересмотр — только новым решением владельца с датой. Задачи они не закрывают.

- **D1 (2026-11-07). Пункты roadmap этапа 7, которые этап не реализует сам.** «Контроллер хранения и сетевое устройство с выбранной стратегией драйверов»: хранение (NVMe) остаётся в этапе 7 (S7-16/S7-17), сетевое устройство → S11-32 (модель 82540EM уже выбрана в `VIRTUAL_BOARD.md`); сетевые протоколы UEFI (SNP, PXE, HTTP boot) не входят в профиль прошивки v1. «Собственный vTPM 2.0 и постоянное защищённое состояние его ключей»: vTPM — этап 7, постоянное защищённое состояние ключей → S9-27. «Связь measured boot, TPM и восстановления машины»: measured boot, event log и PCR7 остаются в этапе 7 (S7-22, S7-24), restore/rollback переменных и TPM → S9-26/S9-27. Этап 7 задаёт форматы и host-API выдачи/приёма байтов NVRAM и TPM state (S7-10, S7-21, S7-23, S7-33); этап 9 — защиту, persistence и snapshot-согласование. Переносит: S7-00 (отчёт, `ROADMAP.md`), S7-33.
- **D2 (2026-11-07). Граница с этапом 8.** Этап 7 заканчивается, когда выбранный официальный установочный образ Windows через собственную UEFI доходит до экрана Setup WinPE и видит CPU, память, диск и экран (S7-32) — это и есть критерий roadmap этапа 7. Этап 8 начинается с установки на виртуальный диск и диагностики сбоев ядра и установщика. Переносит: S7-00 (отчёт, `ROADMAP.md`), S7-32.
- **D3 (2026-11-07). `db` и `KEK` по умолчанию.** `db`: Microsoft Windows Production PCA 2011, Windows UEFI CA 2023, Microsoft UEFI CA 2011, Microsoft UEFI CA 2023. `KEK`: Microsoft Corporation KEK CA 2011, Microsoft Corporation KEK 2K CA 2023. Microsoft Option ROM UEFI CA 2023 не включается: option ROM на плате нет; ADR записывает это исключение. Только публичные сертификаты из первоисточника Microsoft с записью URL, даты получения и SHA-256. Переносит: S7-24 (ADR ключей, fixtures `certs/`).
- **D4 (2026-11-07). `dbx` по умолчанию.** Начальный `dbx` — официальный опубликованный список отзывов UEFI Forum/Microsoft (URL, дата/редакция, SHA-256 записать); дальнейшие обновления `dbx` делает гость через `KEK`. Переносит: S7-24.
- **D5 (2026-11-07). `PK` и его private key.** Собственный `PK` генерируется на host при создании VM; private key существует только в памяти на время генерации самоподписанного сертификата и сразу уничтожается, нигде не хранится. Начальные `PK/KEK/db/dbx` host записывает в store напрямую (как enroll в SetupMode), подпись `PK` для этого не нужна. Восстановление и смена ключей — host-действие «сбросить ключи Secure Boot» (вернуть SetupMode или заново записать умолчания с новым `PK`). Следствие: обновления `KEK`, подписанные `PK`, внутри гостя невозможны; новый `KEK` приходит только через сброс умолчаний на host. Секрета для защиты в S9-27 у `PK` нет. Переносит: S7-24 (ADR ключей, host-API начального store).
- **D6 (2026-11-07). Режим по умолчанию.** Новая VM — UserMode с enrolled `PK/KEK/db/dbx`, `SecureBoot=1`. SetupMode или отключённый Secure Boot — явная настройка для отладки; загрузка Windows (S7-32) и прохождение gates в таком режиме — обход по §4.4 плана, в приёмку не входят; собственные сценарии S7-25 в SetupMode (enroll, отказы) — проверка, а не обход. Переносит: S7-24, S7-25, S7-32.
- **D7 (2026-11-07). Срок действия X.509.** `notBefore`/`notAfter` не проверяются ни при проверке подписи образа, ни при проверке authenticated variables: у прошивки нет доверенного времени, а на 2026-11-07 KEK CA 2011, UEFI CA 2011 и Windows Production PCA 2011 уже истекли. Отзыв — только через `dbx`. Время используется только для монотонности `TimeStamp` authenticated variables. Переносит: S7-24 (модуль `x509_policy`).
- **D8 (2026-11-07). Размещение процессора команд vTPM.** Новый crate `crates/od-tpm/`: чистая логика TPM 2.0 без шины и устройств, зависимость `od-machine → od-tpm`; CRB-устройство остаётся в `od-machine`. Карту проекта обновляет S7-20. Переносит: S7-20 (ADR vTPM, `PROJECT_LAYOUT.md`).
- **D9 (2026-11-07). Криптобиблиотеки.** Одно семейство RustCrypto на host и в `no_std` прошивке: `sha1`, `sha2`, `hmac`, `aes`, `cfb-mode`, `rsa`, `p256`/`ecdsa`, `der`, `x509-cert`, `spki` (MIT/Apache-2.0, pure Rust, `no_std`+`alloc` для прошивки). Собственные: протокол TPM, KDFa/KDFe, HMAC-DRBG, логика PKCS#7/Authenticode, политика цепочки сертификатов. Условие: для приватных операций RSA на host (в том числе `Sign`, `RSA_Decrypt`, salted `StartAuthSession`, `ActivateCredential`, `Import`) брать версию `rsa` с constant-time арифметикой; у `rsa` 0.9 есть RUSTSEC-2023-0071 (Marvin). Если на момент S7-20 такой версии нет, ADR записывает это как принятый риск и открытую проблему в `KNOWN_ISSUES.md`. Точные версии, features и транзитивные зависимости фиксирует S7-20 в `DEPENDENCIES.md`. Переносит: S7-20 (ADR vTPM, `DEPENDENCIES.md`).

Перенос пунктов roadmap этапа 7 (D1, D2) записан также в [`ROADMAP.md`](../ROADMAP.md#этап-7-собственные-uefi-acpi-и-платформа-для-windows) под абзацем этапа 7. План этапа 8 пока не существует; ссылка на него добавится, когда он появится.

## 4. Открытые ограничения

- Реализация этапа 7 не начата: нет кода прошивки, контрактов `FIRMWARE_BOOT.md`, `UEFI_PROFILE.md`, `VTPM.md` и других документов из раздела 3 плана.
- Задачи S7-15, S7-16, S7-31, S7-32, S7-34 зависят от других этапов и задач (S10-03, S4-30, этапы 5 и 6, xHCI-модель платы); см. зависимости в плане, §6.
- Внешние блокеры по колонке «Зависимости» плана, §6: S7-26 — blocked до S4-21 (lifecycle vCPU); S7-16 — PCI-привязка blocked до S4-30; S7-31 — blocked до этапа 5 и до PCI-привязки S7-16 (S4-30); S7-32 — blocked до PCI-привязки S7-16 (S4-30), S4-10 (2 MiB страницы) и этапа 6 (2 vCPU), плюс зависит от S7-35, а S7-34 — по итогам S7-30.
- Решения D3–D9 ещё не воплощены в ADR: это делают S7-20 и S7-24.
