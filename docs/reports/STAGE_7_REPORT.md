# Этап 7. Отчёт: собственные UEFI, ACPI и платформа для Windows

Статус отчёта: **промежуточный. Этап 7 НЕ принят.** Документ фиксирует только то, что подтверждено коммитами и проверками на ветке `feat/s9-stage` (исходная точка — коммит `bacffa0`, план этапа 7). Правило статусов: задача, выполненная исполнителем, — `in_progress`, пока ревью не примет её и не отметит чекбокс в плане (`STAGE_7.md` §0); S7-00, S7-01, S7-02, S7-03, S7-12, S7-20, S7-21 и S7-30 сейчас `in_progress`, остальные задачи S7-NN не начаты. Наличие плана, этого отчёта или записей в карте проекта не закрывает ни одну задачу.

Связанные документы: [план этапа 7](../plans/STAGE_7.md), [этап 7 дорожной карты](../ROADMAP.md#этап-7-собственные-uefi-acpi-и-платформа-для-windows), [виртуальная плата](../specs/VIRTUAL_BOARD.md), [состояние VM](../specs/VM_STATE.md), [реестр проблем](../KNOWN_ISSUES.md), [отчёт этапа 9](STAGE_9_REPORT.md), [отчёт этапа 11](STAGE_11_REPORT.md).

## 1. Сводка

Всего задач S7-00…S7-41 по плану: 38 (S7-00…S7-35, S7-40, S7-41).

| Статус | Количество | Задачи |
|---|---|---|
| Принято | 0 | — |
| `in_progress` | 8 | S7-00 (выполнено, коммит `c1ae3dd`, ожидает принятия ревью); S7-01 (выполнено, коммит `426c98a`, ревью пройдено); S7-02 (выполнено, коммит `68cca38`, ревью пройдено); S7-03 (выполнено исполнителем, ожидает ревью, коммита нет); S7-12 (выполнено исполнителем, ожидает ревью, коммита нет); S7-20 (выполнено архитектором, ревью пройдено, коммита нет); S7-21 (выполнено исполнителем, ожидает ревью, коммита нет); S7-30 (выполнено исполнителем, ревью пройдено, замечания исправлены, коммита нет) |
| `not_started` | 30 | остальные задачи, см. таблицу ниже |

Чекбоксы `[ ]` в плане не отмечены: по `STAGE_7.md` §0 отмечаются только после ревью.

## 2. Таблица задач

| Задача | Статус | Коммит | Ревью | Комментарий |
|---|---|---|---|---|
| S7-00 | in_progress | `c1ae3dd` | — | Scope, отчёт этапа, обновление платы и карты, запись переноса пунктов roadmap (выполнено, коммит `c1ae3dd`, ожидает принятия ревью) |
| S7-01 | in_progress | `426c98a` | — | Сборка прошивки и EFI-приложений, host-side тесты, методика (выполнено исполнителем, ревью пройдено, замечания исправлены, коммита на момент записи нет; каркас основного образа и доказательство сборки PE32+ probe; тестовые EFI-приложения — S7-06, боевой RT-образ и размещение в FV — S7-11) |
| S7-02 | in_progress | `68cca38` | — | Контракт входа: окна, handoff, manifest платы, ABI/состояние вызова, таймеры, ROM, RT relocation (выполнено, ожидает ревью, коммита нет) |
| S7-03 | in_progress | — | — | ROM backing/shadow, загрузчик образа, handoff block, профиль платы и runner (выполнено исполнителем, ожидает ревью, коммита нет; blocker снят реализацией решения архитектора — допуск `B0+r ib` и `66 B8+r iw` в `PagedCpl0ProbeV1`; образ `--smoke` в машине: выход 0, `halted`, `retired = 4`, COM1 ровно `X`, см. раздел 5 и раздел S7-03) |
| S7-04 | not_started | — | — | Ядро прошивки: вход, таблицы страниц, аллокатор, memory map, COM1 |
| S7-05 | not_started | — | — | System Table, Boot Services, база протоколов, события, `ExitBootServices` |
| S7-06 | not_started | — | — | PE/COFF loader, Authenticode-дайджест, `LoadImage`/`StartImage`/`Exit`, Loaded Image, Device Path |
| S7-07 | not_started | — | — | Консоль: Simple Text Output/Input на COM1, ConIn/ConOut/StdErr |
| S7-08 | not_started | — | — | Первый сквозной срез: встроенные приложения → COM1 → эталон |
| S7-09 | not_started | — | — | Variable services (volatile) и versioned store codec |
| S7-10 | not_started | — | — | NVRAM-устройство в `od-machine`, драйвер прошивки, NV-переменные через сброс |
| S7-11 | not_started | — | — | Runtime Services: время, `SetVirtualAddressMap`, `ResetSystem`, RT-переменные |
| S7-12 | in_progress | — | — | Собственный AML-кодировщик и DSDT из manifest (выполнено исполнителем, ожидает ревью, коммита нет; DSDT только для PCI0 и COM1, остальное отложено на S7-14/S7-22; пробел manifest — [ISSUE-065](../KNOWN_ISSUES.md#issue-065-s7-dsdt-pci0-has-no-io-mmio-windows-or-interrupt-routes); [ACPI_SMBIOS_TABLES.md](../specs/ACPI_SMBIOS_TABLES.md#aml-и-dsdt)) |
| S7-13 | not_started | — | — | ACPI (RSDP/XSDT/FADT/MADT/MCFG/HPET/TPM2/DSDT) и SMBIOS из handoff block, configuration tables |
| S7-14 | not_started | — | — | ACPI fixed hardware в `od-machine`: PM1, PM timer, reset register, RTC/CMOS, HPET |
| S7-15 | not_started | — | — | GOP поверх собственного дисплейного устройства |
| S7-16 | not_started | — | — | Минимальный NVMe-контроллер в `od-machine` |
| S7-17 | not_started | — | — | Драйвер NVMe прошивки: Block I/O, Disk I/O, GPT partitions |
| S7-18 | not_started | — | — | Собственная FAT и Simple File System |
| S7-19 | not_started | — | — | Boot manager: `BootOrder`/`Boot####`, `\EFI\BOOT\BOOTX64.EFI`, загрузка приложения с ESP |
| S7-20 | in_progress | — | — | ADR: размещение vTPM, криптозависимости host и `no_std` прошивки, профиль команд, CRB, состояние (выполнено архитектором, ревью пройдено, замечания исправлены, [ADR-0009](../decisions/ADR-0009-vtpm-placement-and-crypto.md); ожидает ревью, коммита нет) |
| S7-21 | in_progress | — | — | Процессор команд TPM 2.0, фаза A: startup, capability, PCR, hash, random, password session (выполнено исполнителем, ожидает ревью, коммита нет; 17 команд, 88 тестов `cargo test -p od-tpm --offline`; PCR_Allocate отказ; SHA-384 банк не реализован; список не сверенных мест — [VTPM.md](../specs/VTPM.md#источники-и-сверка)) |
| S7-22 | not_started | — | — | CRB-устройство, таблица TPM2, `EFI_TCG2_PROTOCOL`, measured boot и event log (PCR0–5) |
| S7-23 | not_started | — | — | TPM фаза B: иерархии, ключи RSA/ECC, сессии, NV, persistent state v1 |
| S7-24 | not_started | — | — | Secure Boot: authenticated variables, PK/KEK/db/dbx, Authenticode в `LoadImage`, PCR7 |
| S7-25 | not_started | — | — | Проверка Secure Boot и отказов TPM/Secure Boot на собственных приложениях |
| S7-26 | not_started | — | — | Сброс и перезагрузка: `ResetSystem` → сброс машины, NVRAM/TPM переживают сброс |
| S7-27 | not_started | — | — | Измерения пути загрузки |
| S7-28 | not_started | — | — | Отказы без TPM/Secure Boot: handoff, NVRAM, PE, GPT, FAT, отсутствующий диск, EBS |
| S7-29 | not_started | — | — | Промежуточное ревью и приёмка gates G1–G3 |
| S7-30 | in_progress | — | — | Аудит фактических требований Windows boot path к прошивке и платформе (выполнено исполнителем, ревью пройдено, замечания исправлены, коммита нет; [WINDOWS_PLATFORM_REQUIREMENTS.md](../specs/WINDOWS_PLATFORM_REQUIREMENTS.md)) |
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

### S7-03. ROM backing, загрузчик образа, handoff block, профиль платы и runner

Статус `in_progress`: выполнено исполнителем (Claude Sonnet), ожидает ревью; коммита и ревью нет. Blocker (мини-образ не печатал `X`) снят реализацией допуска `MOV imm` (абзац «Реализация решения» ниже). Реализованные границы — [MACHINE_CONTRACT.md](../specs/MACHINE_CONTRACT.md), раздел «Firmware board v1 and handoff (S7-03)».

Создано: `crates/od-machine/src/firmware_board.rs` (профиль платы, загрузчик образа, handoff block v1.0, временные таблицы страниц, `LoadedFirmwareBoard::run_boot`, 13 тестов), `tools/runner/src/firmware_boot.rs` (подкоманда `boot-firmware`, 2 unit-теста), `tools/runner/tests/firmware_boot_cli.rs` (5 тестов). Изменены: `physical_memory.rs` (read backing для `Rom`, `load_rom_bytes`, отказ записи; тест существующего поведения `accesses_check_boundaries_holes_rom_and_reserved_regions` обновлён: ROM теперь читается и возвращает нули до загрузки — это и есть изменение контракта, чтение разрешено S7-02; добавлен тест `rom_is_readable_after_loader_fill_and_rejects_every_other_write`), `bus.rs` (чтение ROM через `read_physical`/`read_flat_bytes`, `load_rom_bytes`), `uart16550.rs` (`Uart16550Handle::holding_occupied` для STOP-предиката), `lib.rs` (mod и экспорт), `tools/runner/src/main.rs` (диспетчер), `tools/build/build_firmware.py` и `test_build_firmware.py` (режим `--smoke`, 3 теста), `firmware/uefi/README.md`, `docs/testing/FIRMWARE.md`, `MACHINE_CONTRACT.md`, строка статуса в плане.

Что работает (подтверждено тестами):

- ROM читается, запись гостя и обычного загрузчика — `ReadOnlyRegion(Rom)` без callback; shadow `[0x0020_0000, 0x0060_0000)` — копия, прочитанная из ROM; page generations для ROM не создаются.
- Handoff block (12 регионов, 880 байт, `N = 91`) побайтно совпадает с эталоном, сгенерированным отдельным скриптом по таблицам FIRMWARE_BOOT.md разделов 2, 4, 6 (hex в тесте `handoff_block_is_byte_identical_to_the_reference`, CRC `0xDFFE4F54`); число страниц таблиц совпадает с документом для трёх размеров RAM (110 / 2097 / 32780 при обоих экранах, NVRAM и TPM CRB; для текущей платы 91).
- Образ неверного размера, magic, версии, `link_base`, `entry_offset`, `handoff_major`, `flags`, reserved и CRC отклоняется до запуска; неверная конфигурация (RAM, `tsc_hz`, серийный номер) — тоже.
- Fetch из ROM по временным таблицам — `#PF`, error code `0x11`, линейный адрес — адрес fetch, backend-ошибки нет.
- Исполнение из shadow через существующий `run_paged_probe`: кванты заканчиваются на записи THR, байты попадают в транскрипт, виртуальное время растёт на `retired × 1 ns` (образ `MOV RAX,[RBX]; OUT DX,AL ×3; HLT`, `RBX`/`RDX` выставляет тест; ожидаемый `XXX`, 5 инструкций, 4 кванта, 5 ns); исчерпание бюджета отдаётся как `BudgetExhausted`.

Расхождения и ограничения (записаны, не подогнаны):

- **Мини-образ `--smoke` не печатал `X` в машине (blocker, снят).** До реализации `step_paged_probe` исполнял только `MOV RAX,[RBX]`, `MOV [RBX],RCX`, `IN AL,DX`, `OUT DX,AL`, `HLT`; `MOV r8,imm8` и `MOV r16,imm16` давали `Unsupported { opcode: 0xb0 }`, `retired = 0`, а контракт входа обнуляет все GPR кроме `RSP` и `RDI`. **Решение (архитектор, 2026-11-07): вариант (а)** — в `PagedCpl0ProbeV1` допускаются ровно `B0+r ib` и `66 B8+r iw`; контракт входа не меняется. Контракт — [CPU_CONTRACT.md](../specs/CPU_CONTRACT.md), «Расширение S7-03». **Реализация (исполнитель, 2026-11-07):** см. абзац «Реализация решения» ниже; пункты плана «мини-образ пишет ожидаемый байт в COM1 через runner» выполнены.
- `FIRMWARE_BOOT.md` раздел 5 не упоминает, что COM1 при входе уже настроен. Polling UART отклоняет запись THR до `LCR = 0x03` и ненулевого делителя, поэтому загрузчик, как flat probe, настраивает UART; прошивка по разделу 11 пишет те же значения. **Закрыто в документе:** в разделе 5 `FIRMWARE_BOOT.md` добавлена строка «COM1» (настроен машиной как у flat probe S4-32a), раздел 11 на неё ссылается.
- Задание называло `CR4 = 0x620` при входе; контракт (разделы 5 и 7) — `0x20` при входе и `0x620` при вызове EFI-образа; paged-квант допускает только `0x20` (`Cr4Profile::PagedMovLong64V1`). Реализовано `0x20`. **Закрыто в документе:** сам `FIRMWARE_BOOT.md` противоречия не содержал (в разделе 2 значения `CR4` нет; раздел 5 — `0x20`, раздел 7 — `0x620` при вызове образа); разделы 5 и 7 теперь явно говорят, что `0x620` — не состояние входа и ставится прошивкой в S7-04.
- Чтение ROM гостем через paged-квант по-прежнему отклоняется walker'ом (RAM-only); `page_walk.rs` не менялся. Физическое чтение ROM на шине машины — разрешено (read backing). Гостевое чтение ROM через трансляцию страниц в текущем paged-пути — отклоняется (`UnreadableRegion(Rom)`), потому что `page_walk` допускает только Ram и Framebuffer; образ исполняется из shadow, поэтому v1 это не требует. Решение о гостевом чтении ROM — открытый вопрос для S7-04. Раздел 3.1 `FIRMWARE_BOOT.md` дополнен этой пометкой.
- Значение `gsi` для устройств без IRQ и отсутствующих устройств — `0xFFFF_FFFF` (документ: «или `0xFFFF_FFFF` — нет»; для `PRESENT = 0` он явно не сказано).
- Runner использует значения по умолчанию (128 MiB, один экран, нулевой UUID, пустой серийный номер, `tsc_hz = 10^9`); опций командной строки для них нет. Строка `OD-FW STOP` разбирается только как последняя непустая строка транскрипта (STOP с последующим текстом — не STOP, выход 0; тест `fw_stop_is_recognised_only_as_the_last_non_empty_line`); при ошибке прогона (выход 5) транскрипт COM1 тоже записывается (тест `com1_bytes_survive_a_failed_run`); `--smoke` без `--out` пишет `work/stage7/mini.bin`, не `firmware.bin`; разбор заголовка образа без `expect` (`FirmwareBoardError::ImageTruncated`). Разбор `OD-FW STOP` проверен только unit-тестом на тексте (образа, печатающего её, в машине запустить нельзя по причине выше).
- Не реализовано (чужие задачи): повторное копирование shadow при сбросе (S7-26), NVRAM, TPM, ECAM, NVMe, ACPI fixed hardware, доставка IRQ.

**Реализация решения (исполнитель, 2026-11-07).** `crates/od-isa/src/execute.rs`, `step_paged_probe_inner`: добавлена переменная `base_profile` (истинна только для `step_paged_probe`: все флаги профилей CR3, CR0.WP, стека, CPL3 и `PagedCpl0PioU8V1` выключены) и ветка `Operation::MovImmediate { dst: Register, value }` с точным guard из раздела 5 (`opcode.map = Primary`; длина 2, `B0..=B7`, `Prefixes::default()`, цель `Byte`; либо длина 4, `B8..=BF`, ровно `operand_override`, цель `Word`); тело — `state.write(destination, value); state.rip = next_rip` (выход `Retired`, флаги не тронуты). Декодер `long_decode.rs` не менялся; обновлены doc-комментарий `step_paged_probe` и комментарий теста в `firmware_board.rs`. Тесты: `paged_probe_profile.rs` — `paged_probe_mov_r8_imm8_writes_only_the_target_byte` (все 8 форм `B0`–`B7`, включая AH/CH/DH/BH, шаблоны в 16 GPR), `paged_probe_mov_r16_imm16_writes_only_the_low_word` (8 форм `66 B8`–`BF`, биты 63:16 сохранены), `paged_probe_mov_immediate_neighbours_stay_unsupported` (14 форм), `other_paged_profiles_keep_mov_immediate_unsupported` (стековый, CR3, CR0.WP); `paged_probe_quantum.rs` — `paged_probe_mov_immediate_fetch_fault_on_immediate_commits_nothing` (`#PF`, error code `0x10`, адрес — начало следующей страницы, `RDX`/`RIP` не изменены, `instructions_executed = 0`); `firmware_board.rs` — `smoke_code_writes_x_to_com1_with_zero_gprs_at_entry` (все GPR кроме `RSP`/`RDI` нулевые, `Halted`, `retired = 4`, COM1 `X`, `virtual_ns = 4`); `firmware_boot_cli.rs` — `smoke_code_prints_x_and_halts` (выход 0, `halted`, `retired = 4`, `com1_bytes = 1`, транскрипт `X`).

**Фактический результат декодера на соседних формах (риск из решения не реализовался):** все 14 соседних форм дают `Unsupported { opcode }` с ожидаемым байтом: `BA F8 03 00 00`, `48 B8 …`, `66 66 BA …`, `66 41 BA …` — `0xba`/`0xb8`; `41 B0`, `66 B0`, `67 B0`, `2E B0`, `F3 B0`, `F2 B0` — `0xb0`; `40 B4` — `0xb4`; `C6 C0 58` — `0xc6`; `66 C7 C2 …` и `C7 C2 …` — `0xc7`. `#UD` ни для одной формы не получен; декодер не менялся и тесты не подгонялись. Не проверялись `F0`-формы (тест не требовался решением).

Проверки (2026-11-07, worktree `/private/tmp/open_desktop-s9`, HEAD `de701dd` + незакоммиченные изменения; обновлены после реализации допуска `MOV imm`):

- `cargo fmt --all -- --check` — красный только в `crates/od-host/src/display.rs`, `crates/od-machine/src/lib.rs` (старый блок `pub use display_device`, строка 39; добавленные в `lib.rs` строки отформатированы), `tools/power-loss/src/main.rs` (долг `main`); изменённые и новые файлы — чисто (`rustfmt`).
- `cargo test -p od-isa --offline` — все наборы ok (2131 unit; `paged_probe_profile` 17, было 13).
- `cargo test -p od-machine --offline` — 230 passed (unit), остальные наборы ok.
- `cargo test -p od-runner --offline` — все наборы ok (в т.ч. 175 unit, `firmware_boot_cli` 5).
- `cargo check --workspace --all-targets --offline` — проходит (предупреждения прежние, в `od-jit` и тестах).
- `cargo test --manifest-path firmware/uefi/Cargo.toml --offline` — 11 passed.
- `python3 -m unittest tools/build/test_build_firmware.py` — 25 tests OK.
- `python3 tools/check_layout.py 2>&1 | grep -c ERROR` — 45; `git diff --check` — чисто.
- `python3 tools/build/build_firmware.py --smoke --out work/stage7/mini.bin` (SHA-256 `6746dd71…c59f`); `boot-firmware --image work/stage7/mini.bin --com1-out work/stage7/mini_com1.txt --budget 100` (бинарь `target/debug/od-runner`) — код выхода 0, `exit = halted`, `activity = Halted`, `retired = 4`, `quanta = 2`, `virtual_ns = 4`, `com1_bytes = 1`; `printf X | cmp - work/stage7/mini_com1.txt` — совпадает (транскрипт ровно `X`).

Не проверено: исполнение настоящей прошивки (нет входа S7-04); большие RAM (4 GiB и более) загружены только как карта и счёт страниц без выделения backing; поведение при `handoff_minor_max > 0` в образе (поле не валидируется, `u16`).

### S7-20. ADR vTPM: размещение, криптозависимости, профиль, CRB, состояние

Статус `in_progress`: выполнено архитектором (Claude Opus), ожидает архитектурного ревью; коммита нет. Изменены: [ADR-0009](../decisions/ADR-0009-vtpm-placement-and-crypto.md) (новый; номера 0007 и 0008 зарезервированы планом этапа 11), [VTPM.md](../specs/VTPM.md) (новый, только раздел «Состояние и границы»), [DEPENDENCIES.md](../policies/DEPENDENCIES.md) (раздел о криптокрейтах со ссылкой на таблицу ADR), [KNOWN_ISSUES.md](../KNOWN_ISSUES.md) (ISSUE-064), [PROJECT_LAYOUT.md](../PROJECT_LAYOUT.md) (владелец `crates/od-tpm/`, стрелка `od-machine → od-tpm`, граница vTPM), [VIRTUAL_BOARD.md](../specs/VIRTUAL_BOARD.md) (строки «TPM / Secure Boot state» и «Firmware»), строка статуса S7-20 в плане. Кода, Cargo-манифестов, crate `od-tpm` и каталога `crates/od-tpm/` нет (S7-21).

Записано:

- **Размещение (D8):** `crates/od-tpm/` — процессор команд и состояние без шины; CRB — `od-machine`; выдача байтов state — `od-machine` public API; альтернативы «модуль в `od-machine`», «отдельный процесс», «TPM в прошивке», «готовая реализация» отклонены с причинами.
- **Криптография (D9):** 11 crate'ов с версиями (`=0.11.0` … `=0.10.0-rc.22`), лицензиями, `no_std`/`alloc`, features для host и прошивки и транзитивным графом (52 пакета для объединённого набора, 30 для firmware-набора).
- **Условие D9 о `rsa`:** constant-time версии нет (RUSTSEC-2023-0071, `patched = []`); выбран `=0.10.0-rc.22`, риск принят и записан как [ISSUE-064](../KNOWN_ISSUES.md#issue-064-s7-rsa-private-ops-not-constant-time).
- **Профиль, CRB, состояние:** фазы A/B со ссылками на S7-21/S7-23, `TPM_RC_COMMAND_CODE` для команд вне профиля; CRB только locality 0, без прерываний, буфер `0xF80` байт по смещению `0x80` (предложение); persistent/volatile, DRBG не сохраняется, список никогда не логируемых данных.

Проверено: версии, лицензии, features и прямые зависимости — crates.io API 2026-11-07; `#![no_std]` и обязательность `alloc` — чтением `src/lib.rs` скачанных crate'ов; транзитивный граф — `cargo tree -e normal` во временных проектах вне репозитория; advisory — `rustsec/advisory-db`. `python3 tools/check_layout.py` — 45 ошибок (не выросло); `git diff --check` чист; относительные ссылки и якоря в новых записях ведут на существующие файлы и заголовки.

Не проверено: сборка криптокрейтов под `x86_64-unknown-none` и выбор SIMD/soft backend (S7-22, S7-24); раскладка регистров CRB, минимальный размер буфера и обязательный набор команд по тексту PTP 1.07 Errata 1.1 и Library v185 (в ADR помечено «не сверено»); constant-time свойства `p256`/`crypto-bigint` — заявление поставщика, не наш факт.

### S7-21. Процессор команд TPM 2.0, фаза A

Статус `in_progress`: выполнено исполнителем (Claude Sonnet), ожидает ревью; коммита и ревью нет. Изменены: `crates/od-tpm/` (новый crate: `Cargo.toml`, `README.md`, `src/{lib,codes,marshal,command,response,dispatch,startup,capability,random,pcr,hash,state,tests}.rs`), `Cargo.toml` (член workspace), `Cargo.lock`, [VTPM.md](../specs/VTPM.md) (разделы «API», «Команды фазы A», «PCR», «Состояние», «Источники и сверка»), [PROJECT_LAYOUT.md](../PROJECT_LAYOUT.md) (crate создан, `crates/od-tpm/src` в машиночитаемом перечне), [DEPENDENCIES.md](../policies/DEPENDENCIES.md) (что подключено), [VIRTUAL_BOARD.md](../specs/VIRTUAL_BOARD.md) (две ссылки), строка статуса S7-21 в плане. ADR-0009 не менялся.

Реализовано: все 17 команд списка плана — `Startup` (CLEAR/STATE: Reset/Restart/Resume), `Shutdown`, `SelfTest`, `GetTestResult`, `GetCapability` (ALGS, HANDLES, COMMANDS, PP_COMMANDS/AUDIT_COMMANDS пустые, PCRS, TPM_PROPERTIES), `GetRandom`, `StirRandom`, `PCR_Read`, `PCR_Extend`, `PCR_Event`, `PCR_Reset`, `Hash`, `HashSequenceStart`, `SequenceUpdate`, `SequenceComplete`, `EventSequenceComplete`, `ReadClock`; password session `TPM_RS_PW`; один банк SHA-256 из 24 PCR с атрибутами по PTP 1.07 (Table 14/15); HMAC_DRBG (SHA-256) от энтропии владельца; snapshot `ODTPMSTA` v1 (seeds, база `Clock`, счётчики) с `restore` и отказом на повреждение и чужую версию. Команда вне таблицы — `TPM_RC_COMMAND_CODE` (тест проходит весь диапазон 0x11F–0x1A1 до и после `Startup`), до `Startup` — `TPM_RC_INITIALIZE`.

Не реализовано: `TPM2_PCR_Allocate` — отказ (`TPM_RC_COMMAND_CODE`), PTP 1.07 считает её обязательной; `PCR_SetAuthPolicy`/`SetAuthValue` — не поддержаны (ADR-0009, вопрос 6); банки SHA-1 (PTP 1.07: Not Allowed), SHA-384 и SHA-512 (PTP 1.07: Mandatory); `TPM_CAP_PCR_PROPERTIES`; неполный список `TPM_PT` (только реализованное); `validation` в `Hash`/`SequenceComplete` всегда NULL-ticket (нужен `proof` иерархии, фаза B); DA, HMAC/policy-сессии, localities 1–4.

Проверено (2026-11-07): `cargo test -p od-tpm --offline` — 88 тестов проходят (round-trip каждой структуры, успех и 2–3 ошибки на каждую команду, независимые векторы Python для SHA-256/PCR extend/последовательностей/DRBG, RFC 4231 для HMAC, перечень `GetCapability(COMMANDS)` против отдельно выписанного списка, `Startup` до команд, snapshot/restore, детерминированный прогон 1400 искажённых команд без паники и с корректным заголовком ответа). Сверка с открытыми текстами TPM 2.0 Library v184 (Part 1–3) и PTP 1.07 (3/23/2026), URL и дата — в [VTPM.md](../specs/VTPM.md#источники-и-сверка).

Не проверено: **текст Library v185 недоступен** (HTTP 403), сверка по v184; список мест с пометкой `NOT VERIFIED` — [VTPM.md](../specs/VTPM.md#не-сверено-с-текстом-not-verified) (код `TPM_RC_LOCALITY` для `PCR_Extend`, коды ошибок password session, DA, порядок `INITIALIZE`/`COMMAND_CODE`, значения `TPM_PT`, HMAC_DRBG по тексту SP 800-90A). Сборка под `x86_64-unknown-none` не проверялась (crate host-only). Независимое ревью не выполнено. Отклонения: `hmac` (ADR: `=0.13.0`) нет в офлайн-кеше — HMAC для DRBG написан на `sha2`; отклонение: HMAC-DRBG на sha2; S7-23 заменяет на `hmac =0.13.0` (обязательство, перенос из S7-21, см. раздел S7-23); `zeroize =1.9.1` подключён напрямую (по ADR он транзитивный; запись добавлена в ADR-0009, «Последствия»).

NOT VERIFIED (дополнение по ревью): при `TPM_ST_SESSIONS` и `authorizationSize = 0` команда без авторизуемых handle (например, `GetRandom`) обслуживается; **NOT VERIFIED against TPM 2.0 Library v185/v184: TPM_RC_AUTHSIZE при authorizationSize < 9 для SESSIONS**; поведение не менялось, пометка в `command.rs`, `dispatch.rs` и [VTPM.md](../specs/VTPM.md#не-сверено-с-текстом-not-verified), п. 11.

Отклонение, назначенное S7-22: `TPM_CAP_PCR_PROPERTIES` в `GetCapability` отвечает `TPM_RC_VALUE` с параметром 1 (PTP 4.7.1 требует `TPM_PT_PCR_RESET_Lx`); S7-22 обязана реализовать `TPM_CAP_PCR_PROPERTIES` или оставить этот ответ, записав в отчёт S7-22.

Исправления по ревью: копия `authValue` sequence-объекта в обычный `Vec` при проверке пароля убрана: `handle_auth` возвращает `AuthValue` (буфер фиксированного размера, стирается при `Drop`); поля seed'ов `Persistent` — `pub(crate)`; ограничение HMAC на 32-октетный ключ DRBG прокомментировано.

### S7-23. TPM 2.0, фаза B (не начата)

Статус `not_started`. **Обязательство S7-23 (перенос из S7-21):** заменить самописный HMAC-SHA-256 в `crates/od-tpm/src/random.rs` (HMAC_DRBG на `sha2`) крейтом `hmac =0.13.0` (ADR-0009) и перенести проверку векторами RFC 4231 на крейт.

### S7-30. Аудит требований Windows boot path

Статус `in_progress`: выполнено исполнителем, ожидает архитектурного ревью; коммита нет. Изменены: [WINDOWS_PLATFORM_REQUIREMENTS.md](../specs/WINDOWS_PLATFORM_REQUIREMENTS.md) (новый, статус «аудит, не принят»), [GUEST_REQUIREMENTS.md](../specs/GUEST_REQUIREMENTS.md) (только ссылка), строка статуса S7-30 в плане, этот отчёт. Кода нет; `PROJECT_LAYOUT.md` не менялся (документ уже указан в списке документов этапа 7).

Записано: 113 пунктов с источником, статусом и задачей плана — подтверждено 19, по документации Microsoft 62, гипотеза — не проверено 18, источник не найден 14 (подсчёт по первому статусу строки). Трасса реального загрузчика не снята (прошивки нет): все фактические обращения `bootmgfw.efi`/`winload.efi` к протоколам, переменным и TPM — гипотеза до S7-32; список команд TPM (K-01…K-11) — гипотеза, кроме опубликованного требования `TPM2_HMAC`.

Главные находки (подробности и источники — в документе):

- Образ El Torito для UEFI в ISO — `efisys.bin` (вариант с запросом по имени), в ISO есть и `efisys_noprompt.bin`/`cdboot_noprompt.efi`; `efi/boot/bootx64.efi` побайтно равен `bootmgfw.efi`; официального описания запроса «Press any key» не найдено.
- Цепочка подписи `bootmgfw.efi` содержит только `Microsoft Windows Production PCA 2011` (строки, не криптографическая проверка); KB5025885 описывает отзыв PCA 2011 через `dbx` — S7-24 должен проверить, что начальный `dbx` (D4) его не содержит.
- Minimum Hardware Requirements (June 2021) требует `TPM2_HMAC`, банки SHA-256 PCR0–23 и EK certificate — `TPM2_HMAC` нет в списках S7-21/S7-23; CPU: PAE, NX, SSE4.1, CMPXCHG16B, LAHF/SAHF, PrefetchW — текущий профиль `CPU_CONTRACT.md` их не объявляет.
- `install.wim` (8 155 984 950 байт) не помещается одним файлом в FAT32 — вход S7-35.
- Для large pages, HPET, запуска AP, SSE4.2/POPCNT, `OsIndications` источник не найден.

Проверено: `python3 tools/check_layout.py` — 45 ошибок (не выросло); `git diff --check` чист; относительные ссылки изменённых файлов ведут на существующие файлы. ISO читался только листингом `7zz l` и потоками `7zz e -so` в память (заголовки PE/WIM/FAT, boot catalog, SHA-256), без извлечения на диск и без запуска.

Не проверено: тексты UEFI 2.11 и ACPI 6.6 (uefi.org — HTTP 403), тексты TCG (открыты только страницы ресурсов), страница Microsoft «Secure MOR implementation» (HTTP 404), документ WHCP; поиск GUID протоколов в бинарях не выполнялся; `tests/fixtures/firmware/windows_media_manifest.json` не создавался (каталог создаёт S7-08); списки задач S7-13/14/23/24 в плане ссылками на пункты аудита не обновлены.

Решение для владельца и ревью плана (план не правился, §6, строка S7-32; правится только отчёт):

- Кандидат в blocked-список S7-32 (и этапов 2/6): профиль CPU не объявляет SSE4.1, CMPXCHG16B, LAHF/SAHF, PrefetchW, которые требует Windows (M2); подтверждено чтением `CPU_CONTRACT.md` (строка 624). Решение владельца — расширить CPU-профиль (этап 2/6) или признать S7-32 blocked.
- Для ревью плана: §2 плана (`STAGE_7.md:81`) утверждает «Windows требует large pages — зависимость S7-32»; по аудиту (R-03) источник не найден — blocked до S4-10 держится на гипотезе; предлагается уточнить при ревью плана.
- Риск D3/D4/D5 (для владельца): если Windows в гостевой VM применит митигацию KB5025885 (PCA 2011 в `dbx`, SVN 5.0), этот ISO (подписан только PCA 2011) в этой VM перестанет загружаться; восстановить можно только host-сбросом умолчаний (D5). D3–D5 не менялись.

### S7-12. AML-кодировщик и DSDT из manifest

Статус `in_progress`: выполнено исполнителем (Claude Sonnet), ожидает ревью; коммита и ревью нет. Контракт, источники, перечень отложенного и не сверенных мест — [ACPI_SMBIOS_TABLES.md](../specs/ACPI_SMBIOS_TABLES.md#aml-и-dsdt).

Создано (`firmware/uefi/src/acpi/`): `aml/{opcodes,names,encoder,resource,builder,validate}.rs` (кодировщик без `alloc`, интерпретатора нет), `manifest.rs` (`BoardManifest` по таблице устройств FIRMWARE_BOOT.md §6.3), `table.rs` (заголовок SDT, контрольная сумма, `validate_table`), `dsdt.rs` (`build_dsdt`, `validate_dsdt`), тесты `aml/tests.rs`, `dsdt/tests.rs` и модульные тесты `manifest.rs`/`table.rs`. Cargo-зависимости не добавлялись; `crates/` и контракты не менялись.

Что работает (подтверждено тестами):

- PkgLength 1–4 байта (границы `0x3F`, `0xFFF`, `0xFFFFF`, `0x0FFFFFFF`), NameString (`NullName`, `NameSeg`, dual, multi, `\`, `^`), Integer во всех формах, String, EISAID, `Name/Scope/Device/Method/Package/Buffer`, дескрипторы `Memory32Fixed`, `IO`, `IRQ`, Extended Interrupt, `WordBusNumber`, `DWordMemory`, `QWordMemory`, End Tag — эталонные байты вычислены вручную с выкладкой в тесте.
- DSDT для manifest: три побайтных эталона (эталон PCI0 + COM1 пересчитан после удаления `_ADR` у PCI0: Length 0xB6 -> 0xB0, контрольная сумма 0xDC -> 0x0E;  только COM2 level/active-low; пустая `\_SB`), сгенерированных независимой Python-программой по ACPI 6.4 (скрипт не сохранён). Устройство выводится только при `PRESENT = 1`; RTC, HPET, TPM, `_S5_` не выводятся (тест).
- Валидатор: длина, сигнатура, контрольная сумма (сумма таблицы по модулю 256 равна нулю), границы пакетов, повтор имён в одном Scope (при построении и на готовых байтах). Отрицательные тесты: переполнение PkgLength, недопустимое имя, ссылка на неопределённое имя, повтор имени, порча длины/суммы/сигнатуры, неописуемые записи COM1 и ECAM, малые буферы.
- Тесты: 77 в библиотеке (было 11): AML 46, DSDT 13, manifest 4, table 3, прежние 11. Из них 3 побайтных эталона DSDT и ≥ 30 тестов с эталонными байтами конструктов.

Не сделано и ограничения (записаны, не подогнаны):

- **Редакция ACPI — 6.4, не 6.6.** Страницы 6.6 вернули HTTP 403; текст прочитан через Context7 (HTML-копия 6.4). Список не сверенных значений (`ReturnOp` `0xA4`, QWord Address Space `0x8A`/43, грамматика Buffer/Package, type-specific flags номеров шин, бит 0 general flags, checksum End Tag, ID `PNP0A08`/`PNP0501`) — в ACPI_SMBIOS_TABLES.md.
- **Пробел handoff block:** в v1.0 нет окон I/O и MMIO корневого моста и маршрутов прерываний PCI. `_CRS` PCI0 — только диапазон шин, `_PRT` пуст; запись [ISSUE-065](../KNOWN_ISSUES.md#issue-065-s7-dsdt-pci0-has-no-io-mmio-windows-or-interrupt-routes). Запись ECAM `PRESENT = 0` до S4-30, поэтому до него PCI0 не выводится.
- **Отложено:** RTC `PNP0B00`, HPET `PNP0103`, `_S5_` — S7-14; TPM `MSFT0101` — S7-22; установка DSDT как configuration table и остальные таблицы — S7-13. DSDT не загружалась в машину или ОС, поведение Windows/Linux не проверялось.
- `iasl` на host отсутствует: «iasl -d как диагностика» **не сделано**.
- **Отклонение от плана:** таблица каталогов плана (`docs/plans/STAGE_7.md`, строка ~134) относит host-тесты к `firmware/uefi/tests/`; фактически тесты лежат внутри библиотеки (`#[cfg(test)]`, `firmware/uefi/src/acpi/**/tests.rs`). Причина: `cargo test` на aarch64-host не собирает бинарь с inline-asm `out dx, al`, а интеграционные тесты в `tests/` заставили бы его собирать. Сам план не менялся; решение к утверждению ревью. Подробности: интеграционные тесты в `firmware/uefi/tests/` заставили бы `cargo test` собирать бинарь, который на host (aarch64) не собирается. По той же причине `cargo check --manifest-path firmware/uefi/Cargo.toml --offline` без `--lib`/`--target` падает на `main.rs` (inline-ассемблер) и до этой задачи; проверено `--lib` и `--target x86_64-unknown-none`.

Проверки (2026-11-07, worktree `/private/tmp/open_desktop-s9`, HEAD `b459e10` + незакоммиченные изменения): результаты команд перечислены в сообщении исполнителя оркестратору; независимое ревью не выполнено.

## 4. Открытые ограничения

- Реализация этапа 7 почти не начата: есть только каркас сборки прошивки (S7-01, выполнен исполнителем, ожидает ревью; прошивка ничего не реализует); нет контракта `UEFI_PROFILE.md` и других документов из раздела 3 плана; `VTPM.md` содержит границы состояния (S7-20) и команды фазы A (S7-21, ожидает ревью), `FIRMWARE_BOOT.md` подготовлен (S7-02) — оба ожидают ревью.
- Задачи S7-15, S7-16, S7-31, S7-32, S7-34 зависят от других этапов и задач (S10-03, S4-30, этапы 5 и 6, xHCI-модель платы); см. зависимости в плане, §6.
- Внешние блокеры по колонке «Зависимости» плана, §6: S7-26 — blocked до S4-21 (lifecycle vCPU); S7-16 — PCI-привязка blocked до S4-30; S7-31 — blocked до этапа 5 и до PCI-привязки S7-16 (S4-30); S7-32 — blocked до PCI-привязки S7-16 (S4-30), S4-10 (2 MiB страницы) и этапа 6 (2 vCPU), плюс зависит от S7-35, а S7-34 — по итогам S7-30.
- Решения D8–D9 записаны в ADR-0009 (S7-20, ожидает ревью); D3–D7 ещё не воплощены в ADR — это делает S7-24.
- S7-01 п.1 нужно уточнить: target `x86_64-unknown-uefi` допустим для RT-образа (EFI-драйвера), запрет касается основного образа ROM — решение S7-02, подтвердить при ревью S7-01.

## 5. Решение S7-03 (blocker)

**Решение (архитектор, 2026-11-07): вариант (а).** Декодер `long_decode` уже разбирает `B0`–`BF` в `Operation::MovImmediate` (запись части регистра — `LongModeState::write`), поэтому вариант (а) — одна ветка допуска в `step_paged_probe_inner` без изменения декодера и S4-контрактов других профилей. Вариант (б) нарушает контракт входа S7-02 («остальные GPR = 0») и не даёт прошивке задать ни одной константы самой. Статус: blocker снят решением; S7-03 остаётся `in_progress` до реализации, тестов и ревью (исполнитель отмечает результат в разделе S7-03 выше). План этапа не меняется.

Нормативный текст — [CPU_CONTRACT.md](../specs/CPU_CONTRACT.md), «Расширение S7-03: `MOV` с непосредственным операндом в `PagedCpl0ProbeV1`»; состояние входа — [FIRMWARE_BOOT.md](../specs/FIRMWARE_BOOT.md), раздел 5 (строки `CR4`, «COM1», «Исполнимые инструкции»).

**Допускаются только** (Long64, CPL0, только `PagedCpl0ProbeV1`):

- `B0+r ib`, `r = 0…7`, длина 2, `Prefixes::default()` (без REX): `MOV AL/CL/DL/BL/AH/CH/DH/BH, imm8`; пишется только целевой байт.
- `66 B8+r iw`, `r = 0…7`, длина 4, префиксы ровно `Prefixes { operand_override: true, ..Default }`: `MOV AX/CX/DX/BX/SP/BP/SI/DI, imm16`; биты 63:16 сохраняются.

**Остаются `Unsupported`:** `B8+r id` без `66`, любые REX-формы (`40`–`4F` + `B0`–`BF`, в т. ч. `48 B8 io`), `66 B0`–`B7`, повторный `66`, `67`, сегментные, `F2`/`F3`, `C6 /0`, `C7 /0`; `F0` сохраняет классификацию декодера. Профили CR3, CR0.WP, стековый, CPL3 и `PagedCpl0PioU8V1` не расширяются.

### Список для исполнителя S7-03

Код (`od-isa`, без изменения `long_decode.rs`):

1. `crates/od-isa/src/execute.rs`, `fn step_paged_probe_inner`: в начале вычислить `let base_profile = !user_mov && !user_load_only && !allow_stack_forms && !allow_cr3 && !allow_cr0_wp && !cr0_mov_only && !allow_mov_cr0;`. В `match instruction.operation` перед `_ => return unsupported()` добавить ветку `Operation::MovImmediate { dst: Operand::Register(destination), value }` с guard: `base_profile && instruction.opcode.map == OpcodeMap::Primary && ((instruction.length == 2 && matches!(instruction.opcode.byte, 0xb0..=0xb7) && instruction.prefixes == Prefixes::default() && destination.size == OperandSize::Byte) || (instruction.length == 4 && matches!(instruction.opcode.byte, 0xb8..=0xbf) && instruction.prefixes == Prefixes { operand_override: true, ..Prefixes::default() } && destination.size == OperandSize::Word))`. Тело: `state.write(destination, value); state.rip = next_rip;` (выход `Retired`, флаги не трогать). Других мест не менять.
2. Обновить doc-комментарий `step_paged_probe` (allowlist) и комментарий к тесту в `crates/od-machine/src/firmware_board.rs`.

Тесты `od-isa` — `crates/od-isa/tests/paged_probe_profile.rs` (через существующий хелпер `step`):

3. `paged_probe_mov_r8_imm8_writes_only_the_target_byte` — таблица `B0`–`B7` с `imm8 = 0x5A`, все GPR заранее заполнены разными 64-битными шаблонами: целевой байт (биты 7:0 или 15:8) = `0x5A`, остальные биты и все прочие GPR без изменений, `RIP += 2`, `Retired`, `instructions_executed = 1`, `rflags`/`rflags_defined` без изменений, нет записей памяти и PIO.
4. `paged_probe_mov_r16_imm16_writes_only_the_low_word` — таблица `66 B8`–`BF` с `imm16 = 0x03F8`: биты 15:0 = `0x03F8`, биты 63:16 сохранены, `RIP += 4`, остальное как в п. 3.
5. `paged_probe_mov_immediate_neighbours_stay_unsupported` — для каждой формы `instructions_executed = 0`, CPU без изменений, `rflags_defined` без изменений, ожидаемый `Unsupported { opcode }`: `BA F8 03 00 00` → `0xba`; `48 B8` + 8 байт → `0xb8`; `41 B0 58` → `0xb0`; `40 B4 58` → `0xb4`; `66 B0 58` → `0xb0`; `66 66 BA F8 03` → `0xba`; `66 41 BA F8 03` → `0xba`; `67 B0 58` → `0xb0`; `2E B0 58` → `0xb0`; `F3 B0 58` → `0xb0`; `C6 C0 58` → `0xc6`; `66 C7 C2 F8 03` → `0xc7`. Если декодер классифицирует какую-то форму иначе (например, `#UD`), записать фактический результат в тест и отчёт; декодер не менять.
6. `other_paged_profiles_keep_mov_immediate_unsupported` — `B0 58` и `66 BA F8 03` через `step_paged_stack_probe`, `step_paged_cr3_probe` и `step_paged_cr0_wp_probe` (существующие фикстуры этих профилей в `execute.rs`/тестах): `Unsupported { opcode: 0xb0 }` и `Unsupported { opcode: 0xba }` соответственно, `instructions_executed = 0`, состояние без изменений.

Тесты `od-machine`:

7. `crates/od-machine/tests/paged_probe_quantum.rs`: `paged_probe_mov_immediate_fetch_fault_on_immediate_commits_nothing` — `66 BA` в последних двух байтах отображённой страницы кода, следующая страница не отображена: выход `Fault` `#PF`, error code `0x10`, линейный адрес — начало следующей страницы, `RDX` и `RIP` без изменений, `instructions_executed = 0`.
8. `crates/od-machine/src/firmware_board.rs`: заменить `mov_immediate_is_not_yet_executable_in_the_paged_quantum` на `smoke_code_writes_x_to_com1_with_zero_gprs_at_entry` — образ `B0 58 66 BA F8 03 EE F4`, без выставления GPR тестом; проверить при входе все GPR, кроме `RSP`/`RDI`, равны нулю; `run_boot(10)` → `Halted`, `retired = 4`, `com1 == b"X"`, `virtual_ns = 4`.

Тесты `od-runner`:

9. `tools/runner/tests/firmware_boot_cli.rs`: заменить `smoke_code_is_rejected_by_the_paged_quantum_until_mov_immediate_is_admitted` на `smoke_code_prints_x_and_halts` — код выхода `0`, `exit = "halted"`, `retired = 4`, `com1_bytes = 1`, транскрипт ровно `X`.

Документы после реализации (без изменения адресов, формул и плана): `docs/specs/MACHINE_CONTRACT.md`, раздел «Firmware board v1 and handoff (S7-03)» — пункт «Гостевой код ограничен allowlist…» (новый allowlist, мини-образ печатает `X`) и пункт «COM1 при входе настроен…» (убрать «раздел 5 этого не перечисляет»); `docs/testing/FIRMWARE.md` (строка таблицы уровня 3 и абзац про `--smoke`); раздел S7-03 этого отчёта — результат и проверки; строку статуса в `CPU_CONTRACT.md` («реализация — S7-03») заменить ссылкой на тесты.

Команды проверки (worktree `/private/tmp/open_desktop-s9`):

```sh
cargo fmt --all -- --check            # известный красный долг: od-host/display.rs, od-machine/lib.rs:39, power-loss
cargo test -p od-isa --offline --test paged_probe_profile
cargo test -p od-isa --offline
cargo test -p od-machine --offline
cargo test -p od-runner --offline
cargo test --offline -p od-isa -p od-machine -p od-runner -p od-jit
cargo check --workspace --all-targets --offline
python3 -m unittest tools/build/test_build_firmware.py
python3 tools/build/build_firmware.py --smoke --out work/stage7/mini.bin
cargo run --offline -p od-runner -- boot-firmware --image work/stage7/mini.bin \
  --com1-out work/stage7/mini_com1.txt --budget 100; echo "exit=$?"   # ожидается exit=0, "exit": "halted", "retired": 4
printf X | cmp - work/stage7/mini_com1.txt                           # транскрипт ровно X
python3 tools/check_layout.py 2>&1 | grep -c ERROR                    # 45
git diff --check
```
