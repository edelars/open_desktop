# Методика проверки прошивки

S7-01 определяет проверку формата образа ROM и PE32+ probe и host-side тесты чистой логики. Требования к toolchain приведены в [README прошивки](../../firmware/uefi/README.md#команды). Результаты выполнения кода в машине фиксируются в [отчёте этапа 7](../reports/STAGE_7_REPORT.md).

Успешный `cargo check --workspace` прошивку **не** проверяет: она не член корневого workspace. Успешная сборка образа не доказывает работу прошивки.

## Одна команда

Запуск из корня проекта с установленными [инструментами сборки прошивки](../../firmware/uefi/README.md#команды).

```sh
python3 tests/system/firmware_boot_smoke.py &&
python3 tests/system/firmware_esp_boot_smoke.py &&
python3 tests/system/firmware_nvram_smoke.py &&
python3 tests/system/firmware_failure_smoke.py
```

Эта последовательность проверяет G1–G3. Каждый скрипт собирает необходимые
ROM и тестовые EFI-приложения и завершает запуск кодом ошибки при расхождении.
Штатный запрос `ResetSystem`, который остановил плату аппаратно, возвращает
`exit=stopped` и `stop_reason=Reset` или `Shutdown`. Для `--fv-app rt`
успех требует `Shutdown`. Возобновление работы vCPU после аппаратного
сброса относится к S7-26.

## Базовая загрузка

```sh
python3 tests/system/firmware_boot_smoke.py
```

Скрипт собирает release-версию `od-runner`, базовый ROM и ROM с тестовыми приложениями. На плате с 128 МиБ RAM он проверяет базовую диагностику и запускает `hello`, `ebs`, `vars`, `rt`, `acpi`, `gop` и `blk` столько раз, сколько задаёт манифест эталонов.

| Режим | Проверка гостя и результата runner |
|---|---|
| Без `--fv-app` | Базовый ROM завершает диагностику ядра и Boot Services по `HLT`, процесс возвращает `0`. |
| `--fv-app hello` | `LoadImage` читает PE32+ из FV по пути устройства. Приложение пишет `OD-UEFI hello 0.1.0` через `ConOut` и возвращает `EFI_SUCCESS`. Прошивка печатает статус образа и останавливается по `HLT` с кодом `0x21`, `detail = 0`. Процесс возвращает `6`. |
| `--fv-app ebs` | Приложение находит framebuffer через GOP, проверяет его тип и атрибуты в UEFI memory map и записывает пиксель. После `ExitBootServices` оно читает пиксель по тому же физическому адресу, пишет `!` напрямую в COM1 при совпадении и выполняет `HLT`. Процесс возвращает `0`. |
| `--fv-app vars` | Приложение читает встроенные переменные из Runtime Services и проверяет защиту `SetupMode`, создание, чтение, малый буфер, добавление, перечисление, удаление и возврат занятого места. Прошивка печатает `EFI_SUCCESS` и останавливается с `0x21`, `detail = 0`. Процесс возвращает `6`. |
| `--fv-app rt` | Приложение выходит из Boot Services, включает неидентичное отображение Runtime Services и завершает машину через ACPI PM1a с `Shutdown`. Процесс возвращает `0`. |
| `--fv-app acpi` | Приложение обходит EFI configuration tables, проверяет ACPI и SMBIOS и возвращает `EFI_SUCCESS`. Процесс возвращает `6` после останова `0x21`. Скрипт сверяет handoff с бинарным эталоном и побайтно сравнивает 64 КиБ ACPI из гостя с freestanding blob. |
| `--fv-app gop` | Приложение перечисляет режимы, проверяет ошибки, смену режима, доставку события таймера через вызовы GOP и четыре операции `Blt`. Затем оно рисует полосы в 640×480. Runner сохраняет кадр через `DisplayHandle::read_frame_into`. Скрипт сверяет метаданные кадра и сравнивает его с `display_pattern.py --pattern bars --seed 0` при `--tolerance 0`. Процесс возвращает `6` после останова `0x21`. |
| `--fv-app blk` | Runner подключает GPT-образ из `make_test_disk.py` через `--nvme-image`. Гость читает первые секторы разделов, печатает их имена, проверяет поиск ESP по GUID, запись на границе секторов, `FlushBlocks`, `Reset` и доставку таймера во время `ReadBlocks`. Отдельные образы проверяют восстановление по резервной GPT и доступ к пространству имён при повреждении обеих копий. Процесс возвращает `6` после останова `0x21`. |

Для каждого прогона скрипт сверяет COM1 побайтово, включая CRLF, управляющие последовательности и последний байт `!`. При расхождении он печатает diff с видимыми управляющими байтами. [Манифест](../../tests/fixtures/firmware/manifest.json) задаёт ожидаемые остановы, SHA-256 эталонов и пределы инструкций и квантов. Скрипт сверяет хеш ROM из отчёта runner с хешем собранного образа и печатает время каждого запуска. Отдельная проверка запрашивает `ebs` из базового ROM и ожидает останов `0x35` с `EFI_NOT_FOUND`.

В базовом ROM обработчик `ExitBootServices` записывает переменную с `NV` через живую Runtime Services table. После завершения EBS диагностика читает снимок в сохранённой памяти и печатает `OD-FW S7-09 variable image retained` только при совпадении записи. [Профиль переменных](../specs/UEFI_PROFILE.md#переменные).

## Повторная загрузка с NVRAM

```sh
python3 tests/system/firmware_nvram_smoke.py
```

Runner загружает `vars` с пустым устройством, получает снимок между квантами после `HLT` и создаёт новую плату с тем же ROM и `NvramDevice::with_contents`. Второй `vars` читает через `GetVariable` ранее записанные байты `od-s7-10`. Скрипт сверяет два транскрипта COM1, коды `0x21`, хеши ROM, handoff и снимка, пределы инструкций и квантов. Снимок передаётся внутри одного процесса runner. [Host API NVRAM](../specs/NVRAM_DEVICE.md#host-api).

## Отказы без TPM и Secure Boot

Команда `python3 tests/system/firmware_failure_smoke.py` собирает обычный ROM, ROM с FV-приложениями и гостевую пробу. Она выполняет три прохода на нативной плате. Параметр `--work` сохраняет входы в пустом каталоге. Готовые `--rom`, `--apps-rom` и `--app-dir` передаются вместе.

| Вход | Наблюдаемое поведение |
|---|---|
| Неизвестная major-версия handoff | Останов 0x03 с кодом major/minor. |
| CRC последнего банка legacy NVRAM повреждён | Восстанавливается NV-проба из предыдущего завершённого старта. |
| CRC обоих банков legacy NVRAM повреждены | COM1 сообщает об отброшенных банках. Гость видит volatile defaults и начинает без NV-пробы. Новый снимок открывается строгим API. |
| Выход секции PE за длину файла | Гостевой `LoadImage` возвращает `EFI_LOAD_ERROR`. |
| Повреждены основной и резервный GPT | Тот же ESP загружается в контрольном прогоне. При двух повреждениях прошивка завершает обход с 0x21/1. |
| FAT32-файл зациклен на первом кластере | Через `EFI_SIMPLE_FILE_SYSTEM_PROTOCOL` гость получает `EFI_VOLUME_CORRUPTED`. |
| В handoff отсутствует NVMe | После установки протоколов прошивка сообщает `OD-FW no storage boot devices` и завершает собственную диагностику. |
| Неверный MapKey | Гость получает `EFI_INVALID_PARAMETER`, затем успешно выходит из Boot Services с верным ключом. |
| `ResetSystem(Cold)` после EBS, доступен PM1 без reset register | FADT очищает поля reset, плата останавливается с причиной `Shutdown` через S5. |
| `ResetSystem(Cold)` после EBS, PM-блок отсутствует | COM1 сообщает об отсутствии fixed hardware. Прошивка останавливается в HLT с кодом 0x22/1. |

## Измеренная загрузка S7-22

```sh
cargo build --release -p od-runner --offline
python3 tools/build/build_firmware.py --with-test-apps \
  --target-dir work/stage7/tpm-target --out work/stage7/tpm-rom.bin
python3 tests/system/firmware_tpm_smoke.py --runner target/release/od-runner \
  --rom work/stage7/tpm-rom.bin \
  --app work/stage7/tpm-target/test-apps/x86_64-unknown-uefi/release/tpm.efi
```

Проба проверяет TPM2 в XSDT и активный SHA-256 bank через TCG2. Она повторяет цепочку digest → extend для PCR0/1/4/5, сравнивает с TPM2_PCR_Read и выводит только длину ответа GetRandom. В прогоне с ESP дополнительно проверяет BootOrder, Boot0000, действие выбора и GPT_EVENT2. После GetEventLog проверяет отдельную запись EV_NO_ACTION, а затем вызывает EBS с устаревшим и актуальным MapKey и читает retained final events из физической памяти. Для сохранения артефактов диагностики передать `--work`. [Контракт TPM](../specs/VTPM.md#event-log).

## Уровни доказательств

| Уровень | Что проверяет | Команда | Состояние |
|---|---|---|---|
| 0. Сборка образа | Toolchain, линковка, формат `ODFWIMG1` (заголовок, размер ровно `0x400000`, CRC-32) | `python3 tools/build/build_firmware.py --out work/stage7/firmware.bin` и `python3 -m unittest tools/build/test_build_firmware.py` | Есть (S7-01) |
| 0a. Сборка PE32+ EFI-образа (probe) | Что `x86_64-unknown-uefi` и `rust-lld` дают PE32+ (Machine `0x8664`, magic `0x20B`, Subsystem 10, секция `.reloc`, `DataDirectory[5]`). Это не тестовое приложение S7-06 и не RT-образ S7-11 | `python3 tools/build/build_firmware.py --efi-probe work/stage7/efi_probe.efi` | Есть (S7-01) |
| 1. Host-side unit-тесты чистой логики | Функции над байтами в `firmware/uefi/` (сейчас: заголовок образа, CRC-32, строка останова, AML-кодировщик, DSDT из manifest и валидатор таблицы — S7-12; далее PE-парсер, FAT, codec переменных, остальные таблицы ACPI и SMBIOS, команды TPM, Authenticode-хеш). Гость не исполняется | `cargo test --manifest-path firmware/uefi/Cargo.toml --offline` | Есть (S7-01), наполняется задачами S7-04…S7-24 |
| 2. Unit-тесты устройств в `od-machine` | ROM backing, handoff block, NVRAM, NVMe, TPM CRB, ACPI fixed hardware — через `MachineBus`, без прошивки | `cargo test -p od-machine --offline` (модули `firmware_board` и `physical_memory`: S7-03; далее S7-10, S7-14, S7-16, S7-22) | Есть для ROM, shadow, таблиц страниц и handoff block (S7-03) |
| 3. Freestanding прогон в машине | Runner запускает `od-machine` с образом прошивки и тестовым EFI-приложением; результат — вывод COM1, framebuffer (S10-03), PCR и event log, сравнение с эталонами скриптами `tests/system/` | `cargo run --offline -p od-runner -- boot-firmware --image <bin> --com1-out <file> --budget <steps>` (S7-03); тесты `cargo test -p od-runner --offline --test firmware_boot_cli`; `python3 tests/system/firmware_boot_smoke.py` | Мини-образ S7-03, ядро S7-04 и приложения `hello` и `ebs` (S7-08) дают проверенный вывод COM1 |
| 4. Реальная ОС | Linux (blocked до этапа 5 и S4-30) и установочный образ Windows до экрана Setup (blocked до S4-30, S4-10, этапа 6, S7-35) через ту же прошивку; факт — лог COM1/экран и перечень запрошенных протоколов (трассировка прошивки) | Появятся в S7-31, S7-32 | Не существует |

Что относится к какому уровню: логика, не зависящая от MMIO, порта ввода-вывода, аллокатора прошивки и состояния CPU, проверяется на уровне 1; всё остальное — только на уровне 3 (правило тестируемости в README прошивки). Не переносить на уровень 1 код, который иначе не проверить: тест, подменяющий железо, не доказывает его работу в машине.

## Что проверяет уровень 0 сейчас

`build_firmware.py` проверяет ELF (`ET_EXEC`, один `PT_LOAD` по `0x0020_0000`, entry внутри кода), совпадение raw-вывода `rust-objcopy` с ELF, заголовок образа и CRC-32 после сборки; печатает размер и SHA-256. `test_build_firmware.py` проверяет заголовок (поля, расположение, CRC), отказ при неверном размере и при порче magic/CRC/reserved/flags, отказ при отсутствии `clang`, `rust-objcopy` и rustup target с понятным сообщением, сверку заголовка с эталонными байтами и CRC, которые тот же набор констант проверяет Rust-тест в `src/image.rs` (два валидатора закреплены на одних байтах), проверки PE32+ на синтетическом заголовке (Machine, magic, Subsystem, `.reloc`, `DataDirectory[5]`) и полные прогоны сборки образа и probe (пропускаются, если toolchain не установлен). Полный прогон не доказывает работу образа в машине.

Probe (`--efi-probe`, `firmware/uefi/probe/efi_probe/`) — только доказательство, что PE32+ собирается и разбирается; описание и результат — [README прошивки](../../firmware/uefi/README.md#efi-probe-доказательство-сборки-pe32). Первичная проверка `RDI = 0x0010_0000` лежит в `start.S` (ассемблерный путь останова собран и дизассемблирован, в машине не исполнялся: runner — S7-03).

Проверка структуры при изменении каталогов: `python3 tools/check_layout.py` (на 2026-11-07 — 45 старых ошибок про ссылки на ignored `work/`; сравнивать число до и после, новых добавлять нельзя).

## Что проверяет S7-03 (загрузчик машины)

- `cargo test -p od-machine --offline`: ROM читается и отклоняет запись; shadow — копия, прочитанная из ROM; временные таблицы страниц 4 KiB (флаги листьев, дыры); число страниц `N` совпадает с таблицей FIRMWARE_BOOT.md раздела 4; handoff block побайтно совпадает с эталоном (hex в тесте сгенерирован отдельным скриптом по таблицам документа, а не выведен из кода); образ неверного размера, magic, версии, link base, entry, CRC и неверная конфигурация отклоняются до запуска; fetch из ROM даёт `#PF` с error code `0x11`; кванты на запись THR, транскрипт COM1 и время `retired × 1 ns` (образ `MOV RAX,[RBX]; OUT DX,AL ×3; HLT`, регистры выставляет тест).
- `cargo test -p od-runner --offline --test firmware_boot_cli`: отчёт и хеши, бюджет, отказ загрузчика без транскрипта, неверные аргументы.
- Образ `--smoke` на `boot-firmware` в машине печатает `X` и останавливается по `HLT` (выход `0`, `exit = halted`, `retired = 4`; транскрипт COM1 ровно `X`; тесты `smoke_code_writes_x_to_com1_with_zero_gprs_at_entry` и `smoke_code_prints_x_and_halts`). Это работает благодаря допуску ровно `B0+r ib` и `66 B8+r iw` в `PagedCpl0ProbeV1` (CPU_CONTRACT.md, «Расширение S7-03»); остальные формы `MOV imm` остаются `Unsupported`.

## Что проверяет S7-04 (ядро прошивки)

Этот прогон использует настройки платы по умолчанию, 128 МиБ RAM и
пустой приёмник COM1. Требуется выход `halted` и точное совпадение
транскрипта, включая CRLF.

```sh
od_fw_check=$(mktemp -d)
python3 tools/build/build_firmware.py --out "$od_fw_check/firmware.bin"
cargo run --release --offline -p od-runner -- boot-firmware \
  --image "$od_fw_check/firmware.bin" --budget 6000000 \
  --com1-out "$od_fw_check/boot_core.com1"
cmp tests/fixtures/firmware/boot_core_com1.txt "$od_fw_check/boot_core.com1"
```

Эталон содержит версию `0.1.0`, сообщения о переходе на свои
таблицы, 17 записей памяти и ключи диагностики аллокаторов. За ними идут
строки S7-05 об утилитах, сервисах памяти и базе протоколов Boot Services. Проверка RX
отдельно подаёт байт через handle UART и сравнивает дополнительную строку
с его hex-значением. [Результаты для двух конфигураций RAM](../reports/STAGE_7_REPORT.md#s7-04--версия-прошивки-и-com1).

## Что проверяет S7-05 (таблицы UEFI)

В прогоне по умолчанию строка `OD-FW S7-05 Boot Services utilities verified`
появляется после вызовов `SetMem`, `CopyMem` и `CalculateCrc32` через
фактические указатели таблицы. Строка `OD-FW S7-05 table CRC` перед ней
показывает исходные CRC трёх таблиц. Для независимой проверки после `HLT` считать
120 байт System Table с напечатанного адреса в identity-mapped RAM. В
полях по смещениям 88 и 96 находятся адреса Runtime Services (136 байт)
и Boot Services (376 байт). В каждой копии заголовка сохранить ожидаемую
контрольную сумму, обнулить четыре байта со смещения 16 и пересчитать
CRC по полному размеру таблицы через Python `zlib.crc32`. Адреса утилит
по смещениям 344, 352 и 360 в Boot Services сверить по символам ELF.
После успешного `ExitBootServices` CRC System Table меняется, его
указатель Boot Services обнулён. Последняя строка диагностики содержит
исходный адрес Boot Services для чтения её байтов извне.
[Результаты запуска и независимой сверки](../reports/STAGE_7_REPORT.md#s7-05--таблицы-uefi-и-утилиты-boot-services).

Строка `OD-FW S7-05 Boot Services memory verified` подтверждает вызовы
пяти функций по указателям той же таблицы. Диагностика запрашивает
размер карты и успешный снимок, выделяет две страницы и блок pool,
проверяет отказ повторного выделения по занятому адресу и освобождает
память. Карта возвращается с ключом `0x0f` вместо исходного `0x0b`.
[Результаты проверки сервисов памяти](../reports/STAGE_7_REPORT.md#s7-05--сервисы-памяти-uefi).

Строка `OD-FW S7-05 Boot Services protocol registry verified` подтверждает
выделение буфера pool, установку интерфейса, поиск по handle и GUID,
замену интерфейса на NULL-маркер, удаление и освобождение буфера. После
удаления повторный поиск GUID и использование прежнего handle отклоняются.
Диагностика также проверяет открытие владельцем, отказ второго драйвера,
блокировку удаления и переустановки до закрытия. `GET_PROTOCOL` оставляет
привязку доступной для последующей переустановки.
Диагностика запрашивает размер для `LocateHandle`, перечисляет handles,
находит агентов по GUID, проверяет снимок протоколов и записи обоих
открытий. Блоки результатов освобождаются через `FreePool`.
[Результаты перечисления](../reports/STAGE_7_REPORT.md#s7-05--перечисление-протоколов-и-handles).

`LocateDevicePath` проверяется на двух PCI-префиксах и пути с именем
файла. Проверка включает выбор длиннейшего совпадения, точный поиск,
фильтр по поддерживаемому протоколу, обработку первого экземпляра,
неправильную длину и сохранение указателей при ошибке.
[Результаты поиска пути устройства](../reports/STAGE_7_REPORT.md#s7-05--поиск-пути-устройства).

`InstallMultipleProtocolInterfaces` и `UninstallMultipleProtocolInterfaces`
вызываются через указатели с переменным числом параметров. Гость проверяет
пустой список, две пары на границе регистров и стека, сохранение привязок
при поздней ошибке и сравнение каждого экземпляра Device Path.
[Результаты пакетных операций](../reports/STAGE_7_REPORT.md#s7-05--пакетные-операции-с-протоколами).

Строка `OD-FW S7-05 event dispatch verified` следует за проверкой TPL,
вложенного EFI callback, потребления сигнала при `CheckEvent` и
закрытия своего события внутри уведомления группы. Порядок callback
проверяется по байтам, записанным в общий контекст.
[Результаты событий](../reports/STAGE_7_REPORT.md#s7-05--события-и-tpl).

Строка `OD-FW S7-05 protocol notifications verified` подтверждает два ключа
одного события, поиск с повтором после малого буфера, выдачу одного handle
из pool и отзыв ключей через `CloseEvent`. Callback читает привязки
через сервисы таблицы. Гость сверяет число вызовов и доступность обоих
GUID при успешной установке пакета.
[Результаты уведомлений](../reports/STAGE_7_REPORT.md#s7-05--уведомления-об-установке-протоколов).

Строка `OD-FW S7-05 boot timers and waits verified` подтверждает
ожидание относительного таймера, проверку индекса, проверку TPL,
длительность `Stall` по TSC и ровно одно периодическое уведомление
после снятия маски с отменённого таймера.
[Результаты таймеров](../reports/STAGE_7_REPORT.md#s7-05--таймеры-и-ожидание).

Строка `OD-FW S7-05 configuration tables verified` подтверждает
добавление, замену и удаление записей через ABI, публикацию до вызова
уведомления, стабильный `MapKey` при повторном использовании массива и
дескриптор `EfiRuntimeServicesData` в гостевой карте.
[Результаты конфигурационных таблиц](../reports/STAGE_7_REPORT.md#s7-05--конфигурационные-таблицы).

Строка `OD-FW S7-05 exit boot services verified` завершает проверку
устаревшего `MapKey`, повтора со свежим ключом, одного вызова BEFORE
и двух способов регистрации EXIT. В обработчике EXIT `Stall` выдерживает
срок по TSC, а ранее взведённый таймер пропускает срабатывание.
В строке указан прежний адрес Boot
Services для проверки в RAM. После успеха диагностика печатает строку
напрямую через COM1 и останавливает гостя.
[Результаты завершения Boot Services](../reports/STAGE_7_REPORT.md#s7-05--завершение-boot-services).

Строка `OD-FW S7-05 watchdog service verified` подтверждает отмену,
проверку кода и результат взведения по конфигурации платы. С включёнными
`acpi_pm`, `acpi_tables` и `pci_ecam` гость дополнительно сообщает
`watchdog armed`. Прогон с этой платой приостанавливается после строки
взведения, продвигает виртуальные часы на одну секунду и продолжает
исполнение. Ожидаются код watchdog в COM1 и останов машины по Reset.
[Результаты watchdog](../reports/STAGE_7_REPORT.md#s7-05--watchdog).

Строка `OD-FW S7-05 service ABI boundaries verified` подтверждает вызовы
слотов образов, счётчика, контроллера и пятиаргументного `GetVariable`.
Сверка таблиц покрывает 43 адреса Boot Services, NULL в Reserved и 14
адресов Runtime Services. Отдельный прогон читает адрес `ResetSystem`
из живой таблицы до выхода и входит в него с регистрами и стеком
Win64. С ACPI ожидается запрос Reset, без ACPI — останов на `HLT`.
[Результаты проверки ABI](../reports/STAGE_7_REPORT.md#s7-05--завершение-abi-таблиц-uefi).

Строка `OD-FW S7-05 runtime events registered` появляется в обработчике
BEFORE. Она подтверждает оба способа регистрации смены виртуальных
адресов, обычный runtime-таймер, закрытие записи и стабильный `MapKey`
после создания массива. После выхода проверяются сохранённые записи
и их адрес из памяти Runtime Services.
[Результаты runtime-событий](../reports/STAGE_7_REPORT.md#s7-05--события-в-runtime-памяти).

## Дайджест Authenticode S7-06

Проверки диапазонов SHA-256 входят в `cargo test --manifest-path
firmware/uefi/Cargo.toml --locked --offline pe::`. Независимый
расчёт над PE32+ выполняет `python3
firmware/uefi/tests/authenticode_reference.py image.efi`. Он читает
заголовки из файла и печатает дайджест для сравнения с результатом
`pe::authenticode_digest`. [Результаты](../reports/STAGE_7_REPORT.md#s7-06--дайджест-authenticode).


[Результаты проверки протоколов](../reports/STAGE_7_REPORT.md#s7-05--привязки-протоколов-uefi).

## Подпись тестовых PE для Secure Boot S7-25

`tools/build/sign_test_app.py` принимает неподписанный x64 EFI, PEM-сертификат и соответствующий ему RSA-2048 ключ. Для теста создаются собственные эфемерные ключи и сертификаты. Ключ используется только в scratch-каталоге, удаляемом после прогона. Инструмент создаёт новый PE с `WIN_CERTIFICATE` и сохраняет Authenticode SHA-256 исходного образа с выравниванием до восьми байт.

```sh
python3 tools/build/sign_test_app.py unsigned.efi \
  --certificate test.crt --key test.key --out signed.efi \
  --openssl /usr/bin/openssl
```

Тестовый инструмент сам формирует DER `SpcIndirectDataContent` и подписанные атрибуты PKCS#7. Для RSA он вызывает `openssl dgst` с явным PKCS#1 v1.5. Так `SpcIndirectDataContent` остаётся SEQUENCE в PKCS#7 ContentInfo. Встроенный CMS-путь OpenSSL упаковывает содержимое в OCTET STRING. На одной эфемерной паре LibreSSL 3.3.6 и OpenSSL 4.1.0 создали побайтно одинаковый PE. Независимый digest и общий verifier подтвердили допуск при зачисленном сертификате и отказ при подмене секции, отзыве и отсутствии подписи. Инструмент отклонил PSS-only и чужой ключ, повторное подписание и повторную запись выходного файла. [RFC 2315 §9.3](https://www.rfc-editor.org/rfc/rfc2315#section-9.3), [план S7-25](../plans/STAGE_7.md#s7-25-проверить-secure-boot-на-собственных-приложениях).

Для первого запуска в SetupMode подготовьте защищённый снимок через API сброса ключей команды runner. `defaults` подготавливает UserMode. Параметр `--nvram-in <protected snapshot>` выбирает холодный сброс существующего снимка с сохранением обычных переменных. Подготовленный снимок передаётся через `boot-firmware --nvram-in`.

```sh
target/release/od-runner prepare-secure-boot-nvram setup --out setup.nv
target/release/od-runner prepare-secure-boot-nvram defaults --out defaults.nv
```

В `defaults` runner запускает OpenSSL с RSA-2048 и возвращает только DER публичного PK в API provisioning. Запись приватного ключа направляется в `/dev/null`. Каждый штатный сброс создаёт новый PK. Команда требует новый выходной файл и печатает SHA-256 снимка.

`python3 tests/system/firmware_secure_boot_smoke.py` по умолчанию пять раз проверяет запуск неподписанного PE в SetupMode, зачисление `KEK`, `db`, `PK` с ESP, допуск подписанного PE после холодного старта, отказ неподписанному PE и отказ чужой подписи со штатным набором ключей. Скрипт сверяет записи зачисленного снимка и точные окончания COM1. `--work` принимает пустой каталог для сохранения открытых fixtures, сборок и логов. Готовые `--runner`, `--rom` и `--apps` передаются вместе. Каталог приложений содержит `hello.efi`, `enroll.efi` и `esp-boot-probe.efi`. Зачисляющее приложение также задаёт тестовый `BootOrder=[10,11]` из двух файлов `.OPT`. Системный сценарий ставит неподписанный `BOOTX64.EFI` первым и подписанный `CHECK.EFI` вторым. Гостевой `CHECK.EFI` проверяет `BootCurrent=11`, путь и свои `LoadOptions`. Подписанный `enroll.efi` в UserMode проверяет `EFI_SECURITY_VIOLATION` от `LoadImage` для неподписанного PE. Затем он загружает подписанный PE, добавляет его Authenticode SHA-256 в `dbx` через Auth2 с подписью KEK и проверяет `EFI_SECURITY_VIOLATION` от `StartImage`. Перед возвратом приложение также сверяет неизменность указателя EFI Image Execution Information Table и две записи отказов. Оно проверяет имя и точный Device Path неподписанного PE, конечный узел для отсутствующего пути подписанного PE и SHA-256 в списке подписей записи об отзыве. Скрипт сверяет сохранённый `dbx` и отказ тому же PE при холодном старте. Проверка штатного набора начинается с пустого `BootOrder` и завершается остановом `0x21/1`.

### Отказ TPM при Startup

При корректном CRB-ответе `TPM_RC_FAILURE` на `TPM2_Startup(CLEAR)` загрузка продолжается. Журнал COM1 содержит `OD-FW TPM unavailable: TPM2_Startup returned TPM_RC_FAILURE`. Прошивка скрывает TPM2 и устройство TPM из ACPI, не публикует TCG2 и таблицу финальных событий. При активном Secure Boot проверка образов остаётся включённой. Ошибки транспорта и прочие ответы Startup останавливают инициализацию прошивки. [TCG PFP 1.06 r52, §§3.3.2.1 и 7.1](https://medias.cdn.trustedcomputinggroup.org/production/TCG_PC_Client_Platform_Firmware_Profile_Version_1_06_Revision_52_pub_3_1184065dfc.pdf).

```sh
od_tpm_failure=$(mktemp -d)
python3 tools/build/build_firmware.py --out "$od_tpm_failure/firmware.bin" \
  --target-dir "$od_tpm_failure/firmware-target"
cargo build --manifest-path firmware/uefi/tests/apps/Cargo.toml --bin tpm-absent \
  --offline --release --target x86_64-unknown-uefi --target-dir "$od_tpm_failure/app-target"
python3 tools/build/make_test_disk.py --esp-fat32 \
  --bootx64 "$od_tpm_failure/app-target/x86_64-unknown-uefi/release/tpm-absent.efi" \
  --out "$od_tpm_failure/esp.raw"
OD_TPM_FAILURE_ROM="$od_tpm_failure/firmware.bin" OD_TPM_FAILURE_ESP="$od_tpm_failure/esp.raw" \
  cargo test --offline --release -p od-machine --lib \
  firmware_boot_continues_after_tpm_startup_failure -- --ignored --nocapture
```

Тестовая сборка `od-machine` подставляет ответ CRB для Startup. Гостевое приложение из ESP проверяет отсутствие протокола и таблицы событий. Host-тест сверяет COM1, ACPI и освобождение locality с возвратом CRB в idle.

### Проверка PCR7 и отказов Secure Boot

```sh
od_sb_pcr7=$(mktemp -d)
python3 tests/system/firmware_secure_boot_smoke.py --work "$od_sb_pcr7"
OD_SB_PCR7_ROM="$od_sb_pcr7/firmware.bin" OD_SB_PCR7_FIXTURES="$od_sb_pcr7" \
  cargo test --offline --release -p od-machine --lib \
  firmware_secure_boot_pcr7_replays_setup_enrollment_and_revocation -- --ignored --nocapture
```

Второй тест запускает три платы с действующим TPM. Он извлекает журнал Event03 из гостевой RAM, самостоятельно вычисляет SHA-256 для каждого события PCR7 и сверяет replay с `TPM2_PCR_Read` через CRB. Зачисленные и отозванные записи сверяются побайтно с исходными ESL, сохранёнными системным скриптом. PCR7 различается в SetupMode, после зачисления и после отзыва.

| Сценарий | Наблюдаемый результат |
|---|---|
| `TPM2_Startup` возвращает `TPM_RC_FAILURE` | COM1 фиксирует причину, загрузка доходит до ESP. TCG2, финальные события и объявление TPM в ACPI отсутствуют. |
| Неподписанный PE в UserMode | `LoadImage` возвращает `EFI_SECURITY_VIOLATION`. Загрузчик переходит к следующей опции. |
| Auth2 добавляет хеш подписанного PE в `dbx` | `StartImage` возвращает `EFI_SECURITY_VIOLATION` для уже загруженного PE. После холодного старта загрузчик снова отказывает этому PE. |
| Пустой исходный `BootOrder`, отказ образу по штатной политике | COM1 печатает `OD-FW no bootable option`. Машина выполняет `HLT` с кодом `0x21` и деталью `1`. |

## Что проверяет S7-12 (AML и DSDT)

- `cargo test --manifest-path firmware/uefi/Cargo.toml --offline` (77 тестов; из них AML 46, DSDT 13, manifest 4, таблица 3): эталонные байты ≥ 30 конструктов (PkgLength 1–4 байта и границы, NameString, Integer, String, EISAID, Name/Scope/Device/Method/Package/Buffer, дескрипторы ResourceTemplate), вычисленные вручную по спецификации и записанные в тесте с выкладкой; три побайтных эталона DSDT (PCI0 + COM1; только COM2 с level/active-low; без устройств), сгенерированные независимой Python-программой по ACPI 6.4 (скрипт не сохранён); отрицательные случаи — переполнение PkgLength, недопустимое имя, ссылка на неопределённое имя, повтор имени в одном Scope (при построении и в готовых байтах), порча контрольной суммы/длины/сигнатуры, неописуемые записи manifest. Это проверка формата на host: DSDT не загружалась в машину или ОС. Разбор `iasl -d` не выполнялся (`iasl` отсутствует).
- **Отклонение от плана:** host-тесты S7-12 лежат внутри библиотеки (`#[cfg(test)]`, `firmware/uefi/src/acpi/**/tests.rs`), а не в `firmware/uefi/tests/`, как в таблице каталогов `docs/plans/STAGE_7.md` (строка ~134). Причина: `cargo test` на aarch64-host не собирает бинарь с inline-asm `out dx, al`; интеграционные тесты в `tests/` заставили бы его собирать. План не менялся.
- Источник и редакция ACPI, не сверенные места — [ACPI_SMBIOS_TABLES.md](../specs/ACPI_SMBIOS_TABLES.md#aml-и-dsdt).

## Эталоны

- В Git хранятся небольшие эталоны COM1 и JSON-манифесты с хешами. Образы ESP генерируются при проверке скриптом из `tools/build/`. Чужие образы прошивок, `.aml`, дампы SMBIOS и большие файлы в Git не хранятся.
- Каталог `tests/fixtures/firmware/` содержит `boot_core_com1.txt` с диагностикой ядра и отдельные транскрипты COM1 для приложений из FV. Для `fs` скрипт `tools/build/make_test_disk.py --esp-fat32 --out <path>` создаёт GPT с разреженным FAT32 ESP. Размер файла 40 MiB даёт FAT32 необходимое число кластеров. Измеренное выделение на APFS составляет 1 753 088 байт. Скрипт пишет BPB, FSInfo, две FAT, корневой каталог и `\EFI\BOOT\README.TXT` собственным кодом. `firmware_boot_smoke.py` проверяет SHA-256, подключает файл к NVMe и сверяет COM1 с `fs_com1.txt`.
- Временные артефакты — в `work/stage7/` (каталог `work/` игнорируется Git): образ `work/stage7/firmware.bin`, cargo target `work/stage7/firmware-target/`, probe `work/stage7/efi_probe.efi` (target `work/stage7/firmware-target-efi-probe/`).
- Новый вывод сверяется с контрактом в разделах 11–12 и фиксируется вместе с изменением кода. Команда выше сохраняет транскрипт для сравнения с эталоном.

## Гостевая загрузка с ESP

`python3 tests/system/firmware_esp_boot_smoke.py` собирает stock ROM,
`hello.efi` и проверочное приложение. Генератор ESP размещает их по
путям `\EFI\BOOT\BOOTX64.EFI` и `\EFI\BOOT\CHECK.EFI` в разреженном
образе. Тест запускает прошивку без FV-селектора и сверяет полные байты
COM1, SHA-256 обоих дисков и SHA-256 сохранённых NVRAM snapshots.
Сценарии проверяют загрузку по умолчанию, отсутствие ESP, fallback после
отсутствующего файла, приоритет `BootNext` и восстановление снимка.
Проверочное приложение сверяет `BootCurrent`, `LoadOptions`, `DeviceHandle`
и оба интерфейса пути. Проверка снимка читает поколения банков и CRC.
[Формат NVRAM](../specs/NVRAM_DEVICE.md#host-api).

## Измерения

Измерения пути загрузки (S7-27) и решения по скорости (аллокатор, очереди NVMe, кеш FAT) — только по замерам: повтор ≥10 раз, первый отбрасывать, записывать среднее/p50/p95/разброс и fingerprint (Mac, macOS build, режим сборки, число квантов). До замеров выбор — гипотеза. Методика появится в этом документе вместе с S7-27.
