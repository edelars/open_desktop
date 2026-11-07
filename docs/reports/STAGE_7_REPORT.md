# Этап 7. Отчёт: собственные UEFI, ACPI и платформа для Windows

Статус отчёта: **промежуточный. Этап 7 НЕ принят.** Документ фиксирует только то, что подтверждено коммитами и проверками на ветке `feat/s9-stage` (исходная точка — коммит `bacffa0`, план этапа 7). Правило статусов: задача, выполненная исполнителем, — `in_progress`, пока ревью не примет её и не отметит чекбокс в плане (`STAGE_7.md` §0); S7-00, S7-01, S7-02 и S7-20 сейчас `in_progress`, остальные задачи S7-NN не начаты. Наличие плана, этого отчёта или записей в карте проекта не закрывает ни одну задачу.

Связанные документы: [план этапа 7](../plans/STAGE_7.md), [этап 7 дорожной карты](../ROADMAP.md#этап-7-собственные-uefi-acpi-и-платформа-для-windows), [виртуальная плата](../specs/VIRTUAL_BOARD.md), [состояние VM](../specs/VM_STATE.md), [реестр проблем](../KNOWN_ISSUES.md), [отчёт этапа 9](STAGE_9_REPORT.md), [отчёт этапа 11](STAGE_11_REPORT.md).

## 1. Сводка

Всего задач S7-00…S7-41 по плану: 38 (S7-00…S7-35, S7-40, S7-41).

| Статус | Количество | Задачи |
|---|---|---|
| Принято | 0 | — |
| `in_progress` | 4 | S7-00 (выполнено, коммит `c1ae3dd`, ожидает принятия ревью); S7-01 (выполнено, коммит `426c98a`, ревью пройдено); S7-02 (выполнено, коммит `68cca38`, ревью пройдено); S7-20 (выполнено архитектором, ревью пройдено, коммита нет) |
| `not_started` | 34 | остальные задачи, см. таблицу ниже |

Чекбоксы `[ ]` в плане не отмечены: по `STAGE_7.md` §0 отмечаются только после ревью.

## 2. Таблица задач

| Задача | Статус | Коммит | Ревью | Комментарий |
|---|---|---|---|---|
| S7-00 | in_progress | `c1ae3dd` | — | Scope, отчёт этапа, обновление платы и карты, запись переноса пунктов roadmap (выполнено, коммит `c1ae3dd`, ожидает принятия ревью) |
| S7-01 | in_progress | `426c98a` | — | Сборка прошивки и EFI-приложений, host-side тесты, методика (выполнено исполнителем, ревью пройдено, замечания исправлены, коммита на момент записи нет; каркас основного образа и доказательство сборки PE32+ probe; тестовые EFI-приложения — S7-06, боевой RT-образ и размещение в FV — S7-11) |
| S7-02 | in_progress | `68cca38` | — | Контракт входа: окна, handoff, manifest платы, ABI/состояние вызова, таймеры, ROM, RT relocation (выполнено, ожидает ревью, коммита нет) |
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
| S7-20 | in_progress | — | — | ADR: размещение vTPM, криптозависимости host и `no_std` прошивки, профиль команд, CRB, состояние (выполнено архитектором, ревью пройдено, замечания исправлены, [ADR-0009](../decisions/ADR-0009-vtpm-placement-and-crypto.md); ожидает ревью, коммита нет) |
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

Разделы по задачам добавляются вместе с самой задачей. Раздел S7-00 ниже описывает подготовительные документы; по таблице выше он `in_progress`: выполнено, коммит `c1ae3dd`, ожидает принятия ревью (чекбокс в плане ставит ревью).

### S7-00. Scope, отчёт и плата

Изменены: этот отчёт (новый), `docs/specs/VIRTUAL_BOARD.md`, `docs/ROADMAP.md`, `docs/PROJECT_LAYOUT.md`, `docs/specs/VM_STATE.md`. Прошивка, Cargo-манифесты и каталоги `firmware/uefi/` не создавались. Проверки и результат ревью — в сопроводительном отчёте исполнителя; коммит — `c1ae3dd`; запись о ревью добавляется после принятия.

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

### S7-01. Сборка прошивки и host-side тесты

Статус `in_progress`: выполнено исполнителем, ожидает ревью; коммита и ревью нет. Это каркас сборки, а не прошивка: образ не загружался и не исполнялся в машине.

Создано: `firmware/uefi/` (`Cargo.toml`, `Cargo.lock`, `build.rs`, `linker.ld`, `.cargo/config.toml`, `.gitignore`, `README.md`, `src/{lib,image,stop,main}.rs`, `src/start.S`), `firmware/uefi/probe/efi_probe/` (`Cargo.toml`, `Cargo.lock`, `.gitignore`, `src/main.rs`), `tools/build/build_firmware.py`, `tools/build/test_build_firmware.py`, `docs/testing/FIRMWARE.md`. Изменены: корневой `Cargo.toml` (`exclude = ["firmware/uefi"]`; без него cargo отказывает: `current package believes it's in a workspace`), `docs/PROJECT_LAYOUT.md` (абзац в «Неочевидные границы»), строка статуса S7-01 в плане. Удалён `firmware/uefi/src/.gitkeep` (в каталоге появились файлы, правило 5 карты проекта).

Решения (подробности и причины — [README прошивки](../../firmware/uefi/README.md)):

- **Target:** `x86_64-unknown-none`, отдельный Cargo-проект вне workspace; startup `start.S` собирается системным `clang --target=x86_64-unknown-none-elf` из `build.rs`; компоновка `rust-lld` со своим `linker.ld` (один плоский `PT_LOAD` по `0x0020_0000`); `relocation-model=static` (стандартная модель target — PIE, он кладёт `.rela.dyn`/`.got` в образ). `x86_64-unknown-uefi` для основного образа не используется.
- **Изменение toolchain:** `rustup target add x86_64-unknown-none` (2026-11-07, компонент `rust-std`). До этого были установлены `aarch64-apple-darwin` и `x86_64-unknown-linux-gnu`. Для probe добавлено `rustup target add x86_64-unknown-uefi` (2026-11-07, компонент `rust-std`); линкер target — `rust-lld` в режиме `lld-link` из sysroot (`<sysroot>/lib/rustlib/aarch64-apple-darwin/bin/rust-lld`, обёртка `gcc-ld/lld-link`).
- **Версии:** `rustc 1.99.0 (b940084d7 2026-09-28)`, `cargo 1.99.0`, `rustup 1.29.0`, LLD 23.1.1, `Apple clang version 21.0.0 (clang-2100.3.34.2)`.
- **Образ:** `0x0040_0000` байт, заголовок `ODFWIMG1` по FIRMWARE_BOOT.md, раздел 3.2 (однозначен; значения `entry_offset`, CRC-32 пишет `build_firmware.py`, линковщик резервирует 0x40 нулевых байт), CRC-32 по разделу 6.6. Первый вход — `_start` по `0x0020_0040`.
- **Содержимое бинаря:** `_start` (`cld`, первичная проверка `RDI = 0x0010_0000` в ассемблерной заглушке до кода на Rust — иначе `OD-FW STOP code=0x01 detail=<RDI>` в COM1 и `CLI; HLT`; затем зануление `.bss`, `RDI` → `od_fw_main`, где проверка повторена как defence in depth), COM1, строка `OD-FW S7-01 build proof (not a firmware)`, `CLI; HLT`; panic handler печатает `OD-FW STOP code=0x30` и останавливается. Handoff block не разбирается.
- **PE32+ probe:** `firmware/uefi/probe/efi_probe/` (отдельный Cargo-проект, `no_std`/`no_main`, `extern "efiapi" fn efi_main` возвращает `EFI_SUCCESS`), режим `python3 tools/build/build_firmware.py --efi-probe <path>` проверяет `Machine = 0x8664`, magic `0x20B`, `Subsystem = 10`, секцию `.reloc` и `DataDirectory[5]`. Это доказательство сборки, не тестовое приложение S7-06 и не RT-образ S7-11.
- **Не сделано по заданию:** тестовые EFI-приложения (S7-06), боевой RT-образ и его размещение в FV (S7-11, зависит от S7-06), каталоги `firmware/uefi/tests/apps`, `tests/fixtures/firmware`, модули PE/FAT/ACPI, мини-образ для проверки загрузчика машины (S7-03). Место под встроенный firmware volume только зарезервировано комментарием в `linker.ld`.

Проверено (2026-11-07, `/private/tmp/open_desktop-s9`, база `68cca38`):

- `python3 tools/build/build_firmware.py --out work/stage7/firmware.bin` — образ создан, размер `0x400000` (4194304 байт), SHA-256 `892709e3a446fc45b91126a24345fe4543150b743932f25d00854d5f54aa4168`, `entry_offset = 0x40`, `image_crc32 = 0x04038f0a`; повторный прогон и сборка из второго каталога дали тот же SHA-256. Дизассемблирование `.text` (`objdump`) подтвердило `_start` по `0x200040`.
- `python3 tools/build/build_firmware.py --efi-probe work/stage7/efi_probe.efi` — PE-проверка прошла: 3072 байта, SHA-256 `08b0406801a500d1e6843742f6ebb6bd2964dff8c5ec05461d6ba65a6f35aa78`, `Machine=0x8664`, `Subsystem=10`, секция `.reloc` есть, `DataDirectory[5]` rva `0x4000` size `0xC`.
- `cargo test --manifest-path firmware/uefi/Cargo.toml --offline` — 11 тестов проходят (заголовок образа и CRC-32 с контрольным значением `0xCBF43926`, отказы по размеру/magic/CRC/entry/reserved/flags, строка останова, эталонные байты заголовка и CRC образа, общие с Python-тестом).
- `python3 -m unittest tools/build/test_build_firmware.py` — 22 теста проходят (заголовок, сверка с эталонными байтами Rust-валидатора, отказ при неверном размере, отказ при отсутствии `clang`/`rust-objcopy`/rustup target с сообщением, проверка ELF, проверки PE32+ на синтетическом заголовке, полные прогоны сборки образа и probe).
- `cargo check --workspace --all-targets --offline` — проходит (прошивку не собирает). `cargo fmt --all -- --check` — **не проходит из-за ранее существовавших расхождений** в файлах, которые S7-01 не менял: `crates/od-host/src/display.rs`, `crates/od-machine/src/lib.rs`, `tools/power-loss/src/main.rs` (на базе `68cca38`); код прошивки отформатирован `cargo fmt --manifest-path firmware/uefi/Cargo.toml` (проверка чистая). Расхождения не исправлялись как не относящиеся к задаче.
- `python3 tools/check_layout.py` — 45 ошибок, как до изменения (новых нет); `git diff --check` чист.

Не проверено: исполнение образа в `od-machine` (ROM backing и runner — S7-03, вход и COM1 в машине — S7-04); сборка на другом хосте; ассемблерный путь останова при `RDI != 0x0010_0000` (собран и дизассемблирован, не исполнялся); воспроизводимость на другой машине или версии toolchain (образ воспроизводим при одинаковом toolchain и исходниках; проверено сборкой из двух каталогов, SHA-256 совпал); SHA-256 probe в другом каталоге отличался (10 байт, предположительно отладочная запись PDB), причина не исследована; `cargo test` с `--offline` проверен только при пустом дереве зависимостей (внешних crates нет).

### S7-02. Контракт входа в прошивку и handoff block

Статус `in_progress`: документ подготовлен исполнителем, ожидает архитектурного ревью; коммита и ревью нет. Изменены: [FIRMWARE_BOOT.md](../specs/FIRMWARE_BOOT.md) (новый), [VIRTUAL_BOARD.md](../specs/VIRTUAL_BOARD.md) (раздел «Карта адресов firmware board v1», строки «Загрузочный путь» и «Firmware»), [MACHINE_CONTRACT.md](../specs/MACHINE_CONTRACT.md) (ссылка в «Адреса и карта»), строка статуса S7-02 в плане. Кода, Cargo-манифестов и содержимого `firmware/uefi/` нет.

Решено (подробности и исполнители — `FIRMWARE_BOOT.md`, раздел 14):

- **Окна.** ROM `[0xFFC0_0000, 0x1_0000_0000)`, 4 MiB, образ ровно `0x0040_0000` байт с заголовком `ODFWIMG1`; окно NVRAM `[0xFF00_0000, 0xFF04_1000)` — store 256 KiB + страница регистров; `[0xFF04_1000, 0xFFC0_0000)` не отображается (резерв роста). Handoff block — `0x0010_0000`, до 64 KiB. Shadow — `[0x0020_0000, 0x0060_0000)`, стек — `[0x0011_0000, 0x0015_0000)`, временные таблицы — с `0x0060_0000`. RAM: от 128 MiB до `0xF_F400_0000`, кратно 2 MiB, ниже 4 GiB — не выше `0xF400_0000`.
- **Решение 1 — исполнение из ROM:** вариант (а), shadow-копия в RAM; ROM read-only; запись — backend-ошибка `ReadOnlyRegion(Rom)`, гостевой fetch из ROM нормативно даёт `#PF` (error code `0x11`) на page walk из-за `P|NX` на листьях ROM при `EFER.NXE = 1`. Причина: backing, page generations, code tracking и instruction fetch в `od-machine` есть только для `Ram` (`physical_memory.rs`, S4-27b).
- **Решение 2 — состояние при вызове EFI-образа:** собственные identity-таблицы 4 KiB, `CR0 = 0x8001_0033`, `CR4 = 0x620`, `EFER = 0xD00`, FCW `0x037F`, MXCSR `0x1F80`, DF = 0, IF = 1 на `TPL_APPLICATION`, стек 256 KiB (≥ 128 KiB свободно) с выравниванием 16 и shadow space, ABI `efiapi`.
- **Решение 3 — таймеры:** вариант (а), опрос TSC в перечисленных точках; рекомендация плана подтверждена; переход на LAPIC timer после S4-27/S4-28. Вывод проверен и для базы `c1ae3dd`, и для `main` `4dcc340`: S4-27c–e в `main` — только flat identity-диагностика, S4-28 не начат. Добавлено требование к runner S7-03: продвигать виртуальное время на `retired_instructions × 1 ns` после каждого кванта, иначе `Stall` не завершится (время в машине продвигается только явно).
- **Решение 4 — RT relocation:** отдельный PE32+ RT-образ (subsystem `EFI_RUNTIME_DRIVER`) с `.reloc`, загружаемый собственным PE loader; повторное применение DIR64 только к словам со значением, сохранённым при загрузке, плюс `ConvertPointer`. Static-PIE отклонён: одна компоновка не гарантирует, что код, достижимый из RT, не окажется в BS-памяти.
- **Handoff block v1.0:** заголовок `0xA0` байт (magic `ODHANDOF`, версия, размеры, CRC-32, `cpu_count = 1`, `tsc_hz`, UUID и серийный номер с длиной), регионы по 32 байта (10 типов), устройства по 48 байт (COM1, LAPIC, IOAPIC, дисплей, NVRAM, TPM CRB, ECAM с флагом `PRESENT`). Неизвестная версия — останов `OD-FW STOP code=0x03`.
- **Таблицы страниц 4 KiB:** 2097 страниц (≈ 8,19 MiB) при 4 GiB RAM; 32780 страниц (`0x0800_C000` байт ≈ 128,05 MiB) при максимальной RAM ширины 36; формула записана, длину региона машина передаёт в handoff block.

Расхождения плана с фактами репозитория (записаны, не подогнаны):

- План (S7-02, п. 1) предлагает пометить handoff block в карте как `Reserved`. В `physical_memory.rs` регион `Reserved` не имеет backing и отклоняет чтение, а доступ через границу регионов отклоняется (`AccessCrossesRegion`). Поэтому handoff block и остальные boot reservations — поддиапазоны одного региона `Ram`, «зарезервированные» типом в handoff block и в UEFI memory map (`EfiReservedMemoryType` для handoff block).
- План называет строку `VIRTUAL_BOARD.md` «Boot path»; в файле она называется «Загрузочный путь» — изменена она.
- Решение 4 собирает RT-образ штатным target `x86_64-unknown-uefi`, тогда как S7-01, п. 1 запрещает этот target «для самой прошивки». В документе это уточнено (запрет — для основного образа ROM); уточнение вынесено на ревью и должно попасть в README S7-01.
- Запись `MOV CR4` с OSFXSR/OSXMMEXCPT допущена в `CPU_CONTRACT.md` только профилем `FpInitLong64V1` (S2-25b); исполнение в paged-сессии не подтверждено — возможный blocker S7-04, записан в `FIRMWARE_BOOT.md`, раздел 7.
- Переход на селекторы прошивки (`CS = 0x08` через `LRETQ`/`IRETQ`, `SS/DS/ES/FS/GS = 0x10`) в paged Long64 не поддержан: `LRETQ` и `MOV DS/ES` есть только как локальные профили `od-isa`, `MOV FS/GS` — только CPL3, `MOV SS` нет, paged `IRETQ` — только узкие профили S4-19e1/S4-20a. Записано как blocker S7-04 в `FIRMWARE_BOOT.md`, раздел 7.
- Консоль COM1: перенос holding → shift в `uart16550.rs` делает только вызывающий; чтобы строки останова не терялись при большом кванте, выбран контракт «квант заканчивается на записи THR» через существующий STOP-предикат (`FIRMWARE_BOOT.md`, раздел 11; реализуют S7-03 и S7-07).

Проверено: пересечения окон с `[0xF400_0000, 0xFC00_1000)`, `0xFEC0_0000`, `0xFED4_0000`, `0xFEE0_0000` и граница RAM `0xF400_0000` пересчитаны `python3`; совместимость с `MachineMemoryMap::new(36, …)`; `python3 tools/check_layout.py` — число ошибок не выросло (45); `git diff --check` чист; относительные ссылки изменённых файлов ведут на существующие файлы. Текст UEFI 2.11 §2.3.4 сверить не удалось (страница uefi.org вернула HTTP 403); сверку значений FCW/MXCSR выполняет S7-05.

### S7-20. ADR vTPM: размещение, криптозависимости, профиль, CRB, состояние

Статус `in_progress`: выполнено архитектором (Claude Opus), ожидает архитектурного ревью; коммита нет. Изменены: [ADR-0009](../decisions/ADR-0009-vtpm-placement-and-crypto.md) (новый; номера 0007 и 0008 зарезервированы планом этапа 11), [VTPM.md](../specs/VTPM.md) (новый, только раздел «Состояние и границы»), [DEPENDENCIES.md](../policies/DEPENDENCIES.md) (раздел о криптокрейтах со ссылкой на таблицу ADR), [KNOWN_ISSUES.md](../KNOWN_ISSUES.md) (ISSUE-064), [PROJECT_LAYOUT.md](../PROJECT_LAYOUT.md) (владелец `crates/od-tpm/`, стрелка `od-machine → od-tpm`, граница vTPM), [VIRTUAL_BOARD.md](../specs/VIRTUAL_BOARD.md) (строки «TPM / Secure Boot state» и «Firmware»), строка статуса S7-20 в плане. Кода, Cargo-манифестов, crate `od-tpm` и каталога `crates/od-tpm/` нет (S7-21).

Записано:

- **Размещение (D8):** `crates/od-tpm/` — процессор команд и состояние без шины; CRB — `od-machine`; выдача байтов state — `od-machine` public API; альтернативы «модуль в `od-machine`», «отдельный процесс», «TPM в прошивке», «готовая реализация» отклонены с причинами.
- **Криптография (D9):** 11 crate'ов с версиями (`=0.11.0` … `=0.10.0-rc.22`), лицензиями, `no_std`/`alloc`, features для host и прошивки и транзитивным графом (52 пакета для объединённого набора, 30 для firmware-набора).
- **Условие D9 о `rsa`:** constant-time версии нет (RUSTSEC-2023-0071, `patched = []`); выбран `=0.10.0-rc.22`, риск принят и записан как [ISSUE-064](../KNOWN_ISSUES.md#issue-064-s7-rsa-private-ops-not-constant-time).
- **Профиль, CRB, состояние:** фазы A/B со ссылками на S7-21/S7-23, `TPM_RC_COMMAND_CODE` для команд вне профиля; CRB только locality 0, без прерываний, буфер `0xF80` байт по смещению `0x80` (предложение); persistent/volatile, DRBG не сохраняется, список никогда не логируемых данных.

Проверено: версии, лицензии, features и прямые зависимости — crates.io API 2026-11-07; `#![no_std]` и обязательность `alloc` — чтением `src/lib.rs` скачанных crate'ов; транзитивный граф — `cargo tree -e normal` во временных проектах вне репозитория; advisory — `rustsec/advisory-db`. `python3 tools/check_layout.py` — 45 ошибок (не выросло); `git diff --check` чист; относительные ссылки и якоря в новых записях ведут на существующие файлы и заголовки.

Не проверено: сборка криптокрейтов под `x86_64-unknown-none` и выбор SIMD/soft backend (S7-22, S7-24); раскладка регистров CRB, минимальный размер буфера и обязательный набор команд по тексту PTP 1.07 Errata 1.1 и Library v185 (в ADR помечено «не сверено»); constant-time свойства `p256`/`crypto-bigint` — заявление поставщика, не наш факт.

## 4. Открытые ограничения

- Реализация этапа 7 почти не начата: есть только каркас сборки прошивки (S7-01, выполнен исполнителем, ожидает ревью; прошивка ничего не реализует); нет контракта `UEFI_PROFILE.md` и других документов из раздела 3 плана; `VTPM.md` содержит только границы состояния (S7-20), `FIRMWARE_BOOT.md` подготовлен (S7-02) — оба ожидают ревью.
- Задачи S7-15, S7-16, S7-31, S7-32, S7-34 зависят от других этапов и задач (S10-03, S4-30, этапы 5 и 6, xHCI-модель платы); см. зависимости в плане, §6.
- Внешние блокеры по колонке «Зависимости» плана, §6: S7-26 — blocked до S4-21 (lifecycle vCPU); S7-16 — PCI-привязка blocked до S4-30; S7-31 — blocked до этапа 5 и до PCI-привязки S7-16 (S4-30); S7-32 — blocked до PCI-привязки S7-16 (S4-30), S4-10 (2 MiB страницы) и этапа 6 (2 vCPU), плюс зависит от S7-35, а S7-34 — по итогам S7-30.
- Решения D8–D9 записаны в ADR-0009 (S7-20, ожидает ревью); D3–D7 ещё не воплощены в ADR — это делает S7-24.
- S7-01 п.1 нужно уточнить: target `x86_64-unknown-uefi` допустим для RT-образа (EFI-драйвера), запрет касается основного образа ROM — решение S7-02, подтвердить при ревью S7-01.
