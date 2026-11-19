# Open Desktop firmware (`firmware/uefi/`)

S7-01 задаёт отдельную freestanding сборку образа ROM, host-side тесты чистой логики и сборку PE32+ EFI probe. Текущий прогресс загрузки в машине описан в [отчёте этапа 7](../../docs/reports/STAGE_7_REPORT.md).

Контракт образа и входа — [FIRMWARE_BOOT.md](../../docs/specs/FIRMWARE_BOOT.md); задача — [S7-01](../../docs/plans/STAGE_7.md#s7-01-выбрать-и-зафиксировать-сборку-прошивки-и-host-side-тестов); методика проверки — [FIRMWARE.md](../../docs/testing/FIRMWARE.md); правила каталога — [AGENTS.md](../AGENTS.md).

## Выбор сборки

| Решение | Выбор | Причина |
|---|---|---|
| Расположение | Отдельный Cargo-проект `firmware/uefi/`, не член корневого workspace; в корневом `Cargo.toml` — `exclude = ["firmware/uefi"]` | `firmware/AGENTS.md`: guest-target код не подключается в host workspace. Без `exclude` cargo отказывает (`current package believes it's in a workspace`) — проверено. `cargo check --workspace` прошивку не собирает и не проверяет |
| Target прошивки | `x86_64-unknown-none` (rustup target) | Штатный freestanding target предоставляет `core` и `compiler_builtins`. Основной образ ROM сам предоставляет гостевую платформу. Custom target потребовал бы отдельной сборки этих компонентов через `build-std` |
| Код | Библиотека `od_firmware` (`no_std`; `std` только в `cfg(test)`) и бинарь `od-firmware` (`no_std`, `no_main`) | Чистая логика отделена от порта ввода-вывода и тестируется на host |
| Startup | `src/start.S`, собирается системным `clang --target=x86_64-unknown-none-elf` из `build.rs` | Тот же подход, что `tools/build/build_stage4_flat_probe.py` (`clang` для ассемблера); `cc`-crate и другие зависимости не нужны |
| Компоновка | Собственный `linker.ld`, `rust-lld` (линкер target `x86_64-unknown-none` из sysroot) | Один плоский `PT_LOAD` по `0x0020_0000`, без relocations; линковочный скрипт написан для этого проекта, не заимствован |
| Relocation model | `relocation-model=static` в `.cargo/config.toml` | Стандартная модель target — PIE: линкер добавляет `.rela.dyn` и `.got` внутрь образа. Основной образ плоский и перемещений не имеет (FIRMWARE_BOOT.md, раздел 9) |
| Паника | `panic = "abort"` | Нет раскрутки стека во freestanding |
| Образ | `rust-objcopy -O binary` из sysroot, затем `tools/build/build_firmware.py` пишет заголовок и дополняет до `0x400000` | Формат — FIRMWARE_BOOT.md, раздел 3.2 |
| Зависимости | `sha2 =0.11.1` без default features, версии зафиксированы в `Cargo.lock` | SHA-256 для Authenticode. На `x86_64-unknown-none` применяется `sha2_backend="soft"` |

**RT-образ и тестовые EFI-приложения.** Target `x86_64-unknown-uefi` собирает встроенный PE32+ Runtime Driver с subsystem `EFI_RUNTIME_DRIVER` и обязательной секцией `.reloc`. Сборщик помещает драйвер в firmware volume каждого обычного ROM. Тем же target собираются PE32+ probe S7-01 и тестовые приложения. [Контракт входа и переноса RT](../../docs/specs/FIRMWARE_BOOT.md#9-решение-4-перемещение-rt-части-после-setvirtualaddressmap).

## Изменение toolchain

Для S7-01 выполнено `rustup target add x86_64-unknown-none` (2026-11-07; скачан компонент `rust-std`, toolchain `stable-aarch64-apple-darwin`). Ранее установлены были только `aarch64-apple-darwin` и `x86_64-unknown-linux-gnu`. Для probe выполнено `rustup target add x86_64-unknown-uefi` (2026-11-07; компонент `rust-std`, тот же toolchain). Теперь установлены `aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`, `x86_64-unknown-none`, `x86_64-unknown-uefi`.

## Требования

- `clang` (системный; startup-ассемблер). Нужен только для сборки образа, не для `cargo test`. Путь можно задать `OD_FW_CLANG`.
- Для обычного ROM нужны оба Rust target. Установка выполняется через `rustup target add x86_64-unknown-none x86_64-unknown-uefi`.
- `rust-objcopy` из sysroot (компонент `rustup component add llvm-tools`; путь можно задать `OD_FW_RUST_OBJCOPY`). `rust-lld` берётся cargo из sysroot автоматически.
- Python 3 (скрипт сборки и его тесты), `cargo`, `rustc`.

Версии, на которых проверено (2026-11-07, macOS Tahoe, aarch64):

- `rustc 1.99.0 (b940084d7 2026-09-28)`, `cargo 1.99.0 (5f94df478 2026-08-27)`, `rustup 1.29.0`, LLVM 23.1.1 (`rust-lld` — LLD 23.1.1);
- `Apple clang version 21.0.0 (clang-2100.3.34.2)`.

## Команды

Все команды выполняются из корня репозитория. После установки toolchain
проверка S7-01 собирает оба образа и запускает тесты одной командой.

```sh
python3 tools/build/build_firmware.py --out work/stage7/firmware.bin &&
python3 tools/build/build_firmware.py --efi-probe work/stage7/efi_probe.efi &&
cargo test --manifest-path firmware/uefi/Cargo.toml --offline &&
python3 -m unittest tools/build/test_build_firmware.py
```

Отдельные команды сборки, тестирования и smoke-прогона приведены ниже.

```sh
# Образ прошивки (ROM): ELF -> raw -> заголовок ODFWIMG1 -> ровно 0x400000 байт; печатает SHA-256 и размер
python3 tools/build/build_firmware.py --out work/stage7/firmware.bin

# ROM с Runtime Driver и приложениями hello, echo, ebs, rt и vars.
python3 tools/build/build_firmware.py --with-test-apps --out work/stage7/firmware-with-apps.bin

# Побайтовая проверка COM1 для базового ROM и запусков hello, ebs и vars из FV
python3 tests/system/firmware_boot_smoke.py

# Host-side тесты чистой логики (clang и target x86_64-unknown-none не нужны)
cargo test --manifest-path firmware/uefi/Cargo.toml --offline

# Тесты скрипта сборки (заголовок образа, сверка с эталонными байтами Rust, проверки PE32+, отказы, полные прогоны сборки)
python3 -m unittest tools/build/test_build_firmware.py

# Доказательство сборки PE32+ EFI-образа (probe; НЕ приложение S7-06 и НЕ RT-образ S7-11), раздел «EFI probe»
python3 tools/build/build_firmware.py --efi-probe work/stage7/efi_probe.efi

# Мини-образ ODFWIMG1 для проверки загрузчика машины (S7-03): не прошивка, toolchain не нужен
python3 tools/build/build_firmware.py --smoke --out work/stage7/mini.bin

# Запуск образа в машине (S7-03): отчёт JSON, транскрипт COM1 в файл
cargo run --offline -p od-runner -- boot-firmware --image work/stage7/mini.bin --com1-out work/stage7/mini_com1.txt --budget 100
```

Без `--out` `--smoke` пишет в `work/stage7/mini.bin` (никогда в `work/stage7/firmware.bin` — путь настоящего образа прошивки). `--smoke` пишет образ ровно `0x400000` байт с обычным заголовком `ODFWIMG1` и входом `entry_offset = 0x40` с кодом `mov al,'X'; mov dx,0x3F8; out dx,al; hlt` (`B0 58 66 BA F8 03 EE F4`, 64-битный режим). **S7-03:** paged-квант машины допускает `MOV r8,imm8` (`B0+r ib`) и `66 MOV r16,imm16` (`66 B8+r iw`), поэтому `boot-firmware` на этом образе завершается `exit = halted`, `retired = 4`, код выхода `0`, и пишет в COM1 ровно `X`. Подробности — [MACHINE_CONTRACT.md](../../docs/specs/MACHINE_CONTRACT.md#firmware-board-v1-and-handoff-s7-03).

Что делает скрипт: запускает `cargo build --release --target x86_64-unknown-none --offline` с рабочим каталогом `firmware/uefi/` (cargo читает `.cargo/config.toml` относительно текущего каталога) и `CARGO_TARGET_DIR=work/stage7/firmware-target`, проверяет ELF (`ET_EXEC`, один `PT_LOAD` по `0x0020_0000`, entry внутри кода), получает raw через `rust-objcopy`, сверяет его с ELF, записывает заголовок (`entry_offset = entry - 0x0020_0000`, CRC-32 по FIRMWARE_BOOT.md, раздел 6.6), дополняет нулями до `0x0040_0000`, валидирует результат заново и печатает размер, SHA-256, entry, CRC и версии инструментов. `RUSTFLAGS` из окружения сбрасывается: он заменил бы `.cargo/config.toml`.

Ручная сборка ELF без образа (для отладки линковки):

```sh
cd firmware/uefi && cargo build --release --target x86_64-unknown-none --locked --offline
```
Пакеты для этой сборки должны быть в кеше Cargo. При необходимости
установить их отдельно через `cargo fetch --target x86_64-unknown-none --locked`.
Стандартный backend `sha2` на freestanding target наблюдался с отказом
компиляции LLVM. Настройка target в `.cargo/config.toml` выбирает
программный backend и сохраняет `relocation-model=static`.
[ADR-0009, проверка no_std](../../docs/decisions/ADR-0009-vtpm-placement-and-crypto.md#сборка-в-no_std-прошивке-условие-проверки-не-результат).

Бинарь собирается **только** с `--target x86_64-unknown-none`; `cargo build` без target (и `cargo test`) не собирает бинарь на host: он помечен `test = false`.

Воспроизводимость: образ воспроизводим при одинаковом toolchain и одинаковых исходниках; проверено сборкой из двух каталогов (SHA-256 совпал). На другой машине или версии toolchain — не проверено.

## EFI probe (доказательство сборки PE32+)

`firmware/uefi/probe/efi_probe/` — отдельный маленький Cargo-проект (`[workspace]` в его манифесте, не член корневого workspace и не часть основной прошивки): `#![no_std]`, `#![no_main]`, `extern "efiapi" fn efi_main(image, system_table) -> usize`, возвращающий `0` (`EFI_SUCCESS`), и panic handler. Target `x86_64-unknown-uefi`; линкует его сам rustc через `rust-lld` в режиме `lld-link` из sysroot (`<sysroot>/lib/rustlib/<host>/bin/rust-lld`; обёртка `gcc-ld/lld-link` рядом; на этой машине — `~/.rustup/toolchains/stable-aarch64-apple-darwin/lib/rustlib/aarch64-apple-darwin/bin/rust-lld`). Внешних crates нет.

```sh
python3 tools/build/build_firmware.py --efi-probe work/stage7/efi_probe.efi
```

Скрипт собирает probe (`cargo build --release --target x86_64-unknown-uefi --offline`, `CARGO_TARGET_DIR=work/stage7/firmware-target-efi-probe`), читает PE-заголовки и завершается ошибкой, если не выполнено хотя бы одно: `Machine = 0x8664`, magic PE32+ `0x20B`, `Subsystem = 10` (EFI_APPLICATION), есть секция `.reloc`, `DataDirectory[5]` (Base Relocation Table) непуст и лежит внутри `.reloc`. Печатает размер, SHA-256, найденный путь линкера и версии. Результат 2026-11-07: 3072 байта, `Machine=0x8664`, `Subsystem=10`, секции `.text .rdata .eh_fram .reloc`, `DataDirectory[5]` rva `0x4000` size `0xC`. SHA-256 probe воспроизводится в одном каталоге; при сборке из другого каталога он отличался (10 байт, предположительно идентификатор отладочной записи PDB) — это не проверялось подробнее.

Это **не** тестовое приложение S7-06 и **не** RT-образ S7-11: probe ничего не делает с System Table, не имеет subsystem 12 и не кладётся в firmware volume. Он доказывает только, что toolchain выдаёт PE32+ с `.reloc`, который сможет разобрать PE loader S7-06.

## Раскладка образа

- В `.rodata` находится firmware volume с встроенным PE32+ Runtime Driver. Флаг `--with-test-apps` добавляет приложения. Для каждого содержимого архива используется отдельный каталог сборки по SHA-256. [Формат FV](../../docs/specs/UEFI_PROFILE.md#встроенный-архив-fv).
- Линковочный адрес `0x0020_0000` (`FIRMWARE_SHADOW`), размер образа `0x0040_0000`, вход `RIP = 0x0020_0000 + entry_offset`.
- `0x00–0x3F` — заголовок `ODFWIMG1` (линковщик только резервирует 0x40 нулевых байт в `.image_header`, поля пишет `build_firmware.py`); далее `.text` (с `_start` первым), `.rodata`, `.data`, `.bss`. Хвост до `0x400000` — нули.
- `.bss` зануляется startup-кодом; сам образ при загрузке перезаписывает shadow целиком.

## Вход текущего бинаря

`_start` проверяет GPA в `RDI`, зануляет `.bss` и передаёт управление
`od_fw_main` по внутреннему System V ABI. Ошибка GPA останавливает прошивку
со строкой `OD-FW STOP` через COM1. Rust-точка входа сверяет GPA и разбирает
handoff block. После инициализации COM1 она печатает версию из Cargo.
Затем прошивка готовит собственные таблицы 4 KiB и резерв
стека, переключает гостевые CR3 и RSP, загружает свои GDT и IDT, устанавливает
селекторы и включает `CR4.OSFXSR/OSXMMEXCPT`. Затем прошивка печатает
начальную карту EFI в COM1.
Вывод включает MapKey, версию, число дескрипторов и запись `OD-FW MEM`
для каждого диапазона. Затем прошивка проверяет выделение, запись, чтение
и возврат страниц во всех трёх режимах и для двух блоков pool. В COM1
печатаются ключи после обеих проверок. Перед `HLT` прошивка опрашивает
COM1 RX и выводит hex-значение одного доступного байта.
[Проверка S7-04](../../docs/testing/FIRMWARE.md#что-проверяет-s7-04-ядро-прошивки).

## Карта модулей

| Файл | Назначение | Зависит от железа |
|---|---|---|
| `src/lib.rs` | корень библиотеки, константа `HANDOFF_GPA` | нет |
| `src/acpi/aml/` | собственный AML-кодировщик (S7-12): `opcodes`, `names`, `encoder` (PkgLength, NameString, Integer, String), `resource` (ResourceTemplate), `builder` (Scope/Device/Name/Method/Package/Buffer, повтор имён), `validate`; без `alloc`, тесты в `aml/tests.rs`. Контракт и не сверенные места — [ACPI_SMBIOS_TABLES.md](../../docs/specs/ACPI_SMBIOS_TABLES.md#aml-и-dsdt) | нет |
| `src/acpi/{manifest,table,dsdt}.rs` | `BoardManifest` (таблица устройств handoff block v1.0), заголовок SDT и контрольная сумма, DSDT из manifest (`\_SB.PCI0`, `\_SB.COM1`; только `PRESENT = 1`) и её валидатор. Остальные таблицы ACPI и SMBIOS — S7-13 | нет |
| `src/image.rs` | заголовок образа v1, CRC-32, `validate_image` (чистая проверка образа в памяти; машинную проверку делает S7-03 в `od-machine`) | нет |
| `src/idt.rs` | формирование 256 гостевых interrupt gates | нет |
| `src/stop.rs` | форматирование строки останова `OD-FW STOP code=0xNN detail=0x...` без `core::fmt` | нет |
| `src/mem/` | начальная карта EFI, внутренние аллокаторы страниц и pool, общий MapKey | нет |
| `src/var/` | таблица переменных, политика UEFI и codec `ODVARSTR` | нет |
| `src/rt/variable.rs` | EFIAPI для переменных и снимок в Runtime Services Data | нет |
| `src/gop/` | Graphics Output Protocol на основном дисплее, Memory Mapped Device Path и четыре операции `Blt` ([профиль UEFI](../../docs/specs/UEFI_PROFILE.md#gop)) | MMIO и framebuffer |
| `src/nvme/` | опрос фиксированного контроллера, admin- и I/O-очереди и передача данных через DMA ([контракт NVMe](../../docs/specs/NVME_DEVICE.md)) | MMIO и RAM |
| `src/block/` | Block I/O, Disk I/O, проверка GPT, Device Path и Partition Info ([профиль хранения](../../docs/specs/UEFI_PROFILE.md#хранилище)) | драйвер NVMe |
| `src/fat/` | Чтение FAT12, FAT16 и FAT32 с проверкой каталогов и цепочек | байтовый доступ к разделу |
| `src/fs/` | Simple File System и File Protocol для чтения ESP ([профиль](../../docs/specs/UEFI_PROFILE.md#файловая-система)) | FAT, Disk I/O |
| `src/main.rs` | бинарь: начало загрузки, диагностика, panic handler | CPU |
| `src/serial.rs` | инициализация COM1, bounded TX и RX polling | порт ввода-вывода |
| `src/start.S` | вход `_start`, проверка `RDI`, останов через COM1, зануление `.bss`, установка GDT и селекторов, stubs IDT | CPU |
| `build.rs` | сборка `start.S` системным `clang` и ключи линковки (только для target `x86_64-unknown-none`) | host-инструмент |
| `linker.ld` | собственный linker script | — |
| `.cargo/config.toml` | `relocation-model=static` для target прошивки | — |
| `probe/efi_probe/` | PE32+ build proof (отдельный Cargo-проект, раздел «EFI probe») | — |
| `tests/apps/` | EFI-приложения `hello`, `echo`, `ebs` и `vars` для проверки сервисов в госте | EFI ABI |

## Тесты AML и DSDT

`cargo test --manifest-path firmware/uefi/Cargo.toml --offline` запускает тесты библиотеки, включая `acpi::aml::tests` (эталонные байты конструктов и отрицательные случаи), `acpi::dsdt::tests` (три побайтных эталона DSDT, сгенерированных независимой Python-программой по ACPI 6.4; скрипт не сохранён), `acpi::manifest::tests` и `acpi::table::tests`. Тесты лежат внутри библиотеки (`#[cfg(test)]`), а не в `tests/`: интеграционный тест заставил бы `cargo test` собирать бинарь, а он на host не собирается (inline-ассемблер `out dx, al`). На aarch64 host `cargo check` без `--lib` или `--target x86_64-unknown-none` по той же причине падает на бинаре (прежнее поведение, не связанное с AML); проверка библиотеки — `cargo check --manifest-path firmware/uefi/Cargo.toml --offline --lib`, freestanding-вариант — `cd firmware/uefi && cargo check --offline --target x86_64-unknown-none`.

## Правило тестируемости

Логика, которая зависит от MMIO, порта ввода-вывода, аллокатора прошивки или состояния CPU, **не** попадает в библиотеку и не проверяется `cargo test`: она проверяется на уровне 3 (freestanding прогон в `od-machine`, [FIRMWARE.md](../../docs/testing/FIRMWARE.md)). Всё, что можно выразить функцией над байтами (PE-парсер, FAT, codec переменных, AML, ACPI/SMBIOS, разбор команд TPM, Authenticode-хеш), живёт в библиотеке с тестами на host. `std` разрешён только под `cfg(test)`; `extern crate alloc` добавит S7-04 вместе с `#[global_allocator]` (без аллокатора `alloc` не линкуется).

## Статус реализации

За прогрессом ядра, драйверов и сервисов следите по
[отчёту этапа 7](../../docs/reports/STAGE_7_REPORT.md).
