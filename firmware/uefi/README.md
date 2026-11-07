# Open Desktop firmware (`firmware/uefi/`)

**Статус (S7-01): каркас сборки, а не прошивка.** Здесь доказано только, что freestanding `no_std` образ собирается, получает заголовок `ODFWIMG1` и дополняется до `0x400000` байт, а чистая логика тестируется на host. Прошивка ничего не реализует: нет разбора handoff block, памяти, страниц, аллокатора, System Table, Boot Services, PE loader, консоли, переменных, ACPI/SMBIOS, TPM и Secure Boot. Бинарь не запускался в машине (runner и ROM backing — S7-03/S7-04). «UEFI собирается» не означает «UEFI реализована».

Контракт образа и входа — [FIRMWARE_BOOT.md](../../docs/specs/FIRMWARE_BOOT.md); задача — [S7-01](../../docs/plans/STAGE_7.md#s7-01-выбрать-и-зафиксировать-сборку-прошивки-и-host-side-тестов); методика проверки — [FIRMWARE.md](../../docs/testing/FIRMWARE.md); правила каталога — [AGENTS.md](../AGENTS.md).

## Выбор сборки

| Решение | Выбор | Причина |
|---|---|---|
| Расположение | Отдельный Cargo-проект `firmware/uefi/`, не член корневого workspace; в корневом `Cargo.toml` — `exclude = ["firmware/uefi"]` | `firmware/AGENTS.md`: guest-target код не подключается в host workspace. Без `exclude` cargo отказывает (`current package believes it's in a workspace`) — проверено. `cargo check --workspace` прошивку не собирает и не проверяет |
| Target прошивки | `x86_64-unknown-none` (rustup target) | Freestanding target без ОС и без UEFI-окружения: прошивка сама является платформой. `x86_64-unknown-uefi` — target приложений под **чужой** прошивкой, для основного образа не используется (S7-01, п. 1). Custom target JSON с `build-std` не нужен: нужны `core` и `compiler_builtins`, которые уже есть в target |
| Код | Библиотека `od_firmware` (`no_std`; `std` только в `cfg(test)`) и бинарь `od-firmware` (`no_std`, `no_main`) | Чистая логика отделена от порта ввода-вывода и тестируется на host |
| Startup | `src/start.S`, собирается системным `clang --target=x86_64-unknown-none-elf` из `build.rs` | Тот же подход, что `tools/build/build_stage4_flat_probe.py` (`clang` для ассемблера); `cc`-crate и другие зависимости не нужны |
| Компоновка | Собственный `linker.ld`, `rust-lld` (линкер target `x86_64-unknown-none` из sysroot) | Один плоский `PT_LOAD` по `0x0020_0000`, без relocations; линковочный скрипт написан для этого проекта, не заимствован |
| Relocation model | `relocation-model=static` в `.cargo/config.toml` | Стандартная модель target — PIE: линкер добавляет `.rela.dyn` и `.got` внутрь образа. Основной образ плоский и перемещений не имеет (FIRMWARE_BOOT.md, раздел 9) |
| Паника | `panic = "abort"` | Нет раскрутки стека во freestanding |
| Образ | `rust-objcopy -O binary` из sysroot, затем `tools/build/build_firmware.py` пишет заголовок и дополняет до `0x400000` | Формат — FIRMWARE_BOOT.md, раздел 3.2 |
| Зависимости | Нет внешних crates (`Cargo.lock` содержит только `od-firmware`) | Политика зависимостей: «не добавлять на будущее» |

**RT-образ и тестовые EFI-приложения.** FIRMWARE_BOOT.md, раздел 9 (решение 4), собирает отдельный RT-образ (PE32+, subsystem `EFI_RUNTIME_DRIVER`) штатным target `x86_64-unknown-uefi`. Запрет «не использовать `x86_64-unknown-uefi`» из S7-01, п. 1 относится к **основному образу ROM**, как записано в разделе 9; уточнение вынесено на ревью. Тестовые EFI-приложения и RT-образ в S7-01 **не собираются**: их добавляют S7-06 (приложения) и S7-11 (боевой RT-образ и его размещение в FV, зависит от S7-06). S7-01 добавляет только **доказательство сборки PE32+ EFI-образа** — probe (раздел «EFI probe» ниже). Место под встроенный firmware volume в образе только зарезервировано комментарием в `linker.ld`; смещение и размер выберет S7-06, до этого оно не определено.

## Изменение toolchain

Для S7-01 выполнено `rustup target add x86_64-unknown-none` (2026-11-07; скачан компонент `rust-std`, toolchain `stable-aarch64-apple-darwin`). Ранее установлены были только `aarch64-apple-darwin` и `x86_64-unknown-linux-gnu`. Для probe выполнено `rustup target add x86_64-unknown-uefi` (2026-11-07; компонент `rust-std`, тот же toolchain). Теперь установлены `aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`, `x86_64-unknown-none`, `x86_64-unknown-uefi`.

## Требования

- `clang` (системный; startup-ассемблер). Нужен только для сборки образа, не для `cargo test`. Путь можно задать `OD_FW_CLANG`.
- `rustup` target `x86_64-unknown-none`: `rustup target add x86_64-unknown-none` (образ ROM); `x86_64-unknown-uefi`: `rustup target add x86_64-unknown-uefi` (только для probe).
- `rust-objcopy` из sysroot (`<sysroot>/lib/rustlib/<host>/bin/rust-objcopy`; путь можно задать `OD_FW_RUST_OBJCOPY`). `rust-lld` берётся cargo из sysroot автоматически.
- Python 3 (скрипт сборки и его тесты), `cargo`, `rustc`.

Версии, на которых проверено (2026-11-07, macOS Tahoe, aarch64):

- `rustc 1.99.0 (b940084d7 2026-09-28)`, `cargo 1.99.0 (5f94df478 2026-08-27)`, `rustup 1.29.0`, LLVM 23.1.1 (`rust-lld` — LLD 23.1.1);
- `Apple clang version 21.0.0 (clang-2100.3.34.2)`.

## Команды

Все команды — из корня репозитория.

```sh
# Образ прошивки (ROM): ELF -> raw -> заголовок ODFWIMG1 -> ровно 0x400000 байт; печатает SHA-256 и размер
python3 tools/build/build_firmware.py --out work/stage7/firmware.bin

# Host-side тесты чистой логики (clang и target x86_64-unknown-none не нужны)
cargo test --manifest-path firmware/uefi/Cargo.toml --offline

# Тесты скрипта сборки (заголовок образа, сверка с эталонными байтами Rust, проверки PE32+, отказы, полные прогоны сборки)
python3 -m unittest tools/build/test_build_firmware.py

# Доказательство сборки PE32+ EFI-образа (probe; НЕ приложение S7-06 и НЕ RT-образ S7-11), раздел «EFI probe»
python3 tools/build/build_firmware.py --efi-probe work/stage7/efi_probe.efi
```

Что делает скрипт: запускает `cargo build --release --target x86_64-unknown-none --offline` с рабочим каталогом `firmware/uefi/` (cargo читает `.cargo/config.toml` относительно текущего каталога) и `CARGO_TARGET_DIR=work/stage7/firmware-target`, проверяет ELF (`ET_EXEC`, один `PT_LOAD` по `0x0020_0000`, entry внутри кода), получает raw через `rust-objcopy`, сверяет его с ELF, записывает заголовок (`entry_offset = entry - 0x0020_0000`, CRC-32 по FIRMWARE_BOOT.md, раздел 6.6), дополняет нулями до `0x0040_0000`, валидирует результат заново и печатает размер, SHA-256, entry, CRC и версии инструментов. `RUSTFLAGS` из окружения сбрасывается: он заменил бы `.cargo/config.toml`.

Ручная сборка ELF без образа (для отладки линковки):

```sh
cd firmware/uefi && cargo build --release --target x86_64-unknown-none --offline
```

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

- Линковочный адрес `0x0020_0000` (`FIRMWARE_SHADOW`), размер образа `0x0040_0000`, вход `RIP = 0x0020_0000 + entry_offset`.
- `0x00–0x3F` — заголовок `ODFWIMG1` (линковщик только резервирует 0x40 нулевых байт в `.image_header`, поля пишет `build_firmware.py`); далее `.text` (с `_start` первым), `.rodata`, `.data`, `.bss`. Хвост до `0x400000` — нули.
- `.bss` зануляется startup-кодом; сам образ при загрузке перезаписывает shadow целиком.
- Зарезервировано, не реализовано: embedded firmware volume (S7-06), боевой RT-образ и его размещение в FV (S7-11, зависит от S7-06).

## Что делает бинарь (доказательство сборки)

`_start` (`src/start.S`): `cld`, затем **первичная проверка** `RDI = 0x0010_0000` (`mov $0x100000, %rax; cmp %rax, %rdi; jne`) до зануления `.bss` и до любого кода на Rust, как требует FIRMWARE_BOOT.md, раздел 5. При несовпадении заглушка без стека и без записи в память инициализирует COM1, печатает `OD-FW STOP code=0x01 detail=0x<RDI>` (та же строка, что `stop.rs`; опрос THRE не более 65536 раз на байт) и останавливается циклом `CLI; HLT`. Ассемблерный путь останова **собран и дизассемблирован, но не исполнялся** (runner — S7-03, вход в машине — S7-04). Затем зануление `.bss`, `RDI` передаётся в `od_fw_main` (System V ABI — это внутренний вызов, не интерфейс UEFI; интерфейсы UEFI будут `extern "efiapi"`). `od_fw_main` повторно сверяет GPA с `0x0010_0000` (defence in depth; при несовпадении та же строка через Rust-код) и запоминает его. Иначе: инициализация COM1 `0x3F8` по FIRMWARE_BOOT.md, раздел 11, одна строка `OD-FW S7-01 build proof (not a firmware)` с опросом THRE (не более 65536 опросов на байт) и `CLI; HLT` в цикле. Panic handler печатает `OD-FW STOP code=0x30 detail=<строка>` и останавливается. Handoff block не разбирается; GDT, IDT, страницы и стек прошивки не создаются (стек — тот, что дал handoff, `0x0015_0000`).

## Карта модулей

| Файл | Назначение | Зависит от железа |
|---|---|---|
| `src/lib.rs` | корень библиотеки, константа `HANDOFF_GPA` | нет |
| `src/image.rs` | заголовок образа v1, CRC-32, `validate_image` (чистая проверка образа в памяти; машинную проверку делает S7-03 в `od-machine`) | нет |
| `src/stop.rs` | форматирование строки останова `OD-FW STOP code=0xNN detail=0x...` без `core::fmt` | нет |
| `src/main.rs` | бинарь: COM1, panic handler, `od_fw_main` | порт ввода-вывода |
| `src/start.S` | вход `_start`, первичная проверка `RDI`, путь останова через COM1, зануление `.bss` | CPU |
| `build.rs` | сборка `start.S` системным `clang` и ключи линковки (только для target `x86_64-unknown-none`) | host-инструмент |
| `linker.ld` | собственный linker script | — |
| `.cargo/config.toml` | `relocation-model=static` для target прошивки | — |
| `probe/efi_probe/` | PE32+ build proof (отдельный Cargo-проект, раздел «EFI probe») | — |
| `include/`, `tests/` | пока пусты (`.gitkeep`); `tests/apps` и fixtures создают S7-06/S7-08 | — |

## Правило тестируемости

Логика, которая зависит от MMIO, порта ввода-вывода, аллокатора прошивки или состояния CPU, **не** попадает в библиотеку и не проверяется `cargo test`: она проверяется на уровне 3 (freestanding прогон в `od-machine`, [FIRMWARE.md](../../docs/testing/FIRMWARE.md)). Всё, что можно выразить функцией над байтами (PE-парсер, FAT, codec переменных, AML, ACPI/SMBIOS, разбор команд TPM, Authenticode-хеш), живёт в библиотеке с тестами на host. `std` разрешён только под `cfg(test)`; `extern crate alloc` добавит S7-04 вместе с `#[global_allocator]` (без аллокатора `alloc` не линкуется).

## Что не реализовано

Всё, кроме каркаса сборки и PE32+ probe: handoff block и его проверки (S7-04), GDT/IDT/страницы/стек/аллокатор/memory map (S7-04), System Table и Boot Services (S7-05), PE/COFF loader и образы (S7-06), консоль `ConIn`/`ConOut` (S7-07), сквозной срез и эталон (S7-08), переменные (S7-09…S7-11), RT-образ и `SetVirtualAddressMap` (S7-11), ACPI/SMBIOS (S7-12/S7-13), GOP, NVMe, FAT, boot manager, vTPM, Secure Boot, сброс. ROM backing, загрузчик образа и runner в `od-machine` — S7-03. Образ, собранный S7-01, не загружался и не исполнялся.
