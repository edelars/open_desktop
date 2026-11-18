# Требования Windows boot path к прошивке и платформе

**Дата:** 7 ноября 2026 года. **Статус:** **аудит, не принят.** Документ создан задачей S7-30 [плана этапа 7](../plans/STAGE_7.md) и ожидает архитектурного ревью. Он не заявляет, что Windows загружается через нашу прошивку, и не закрывает ни одной задачи S7-NN.

**Объект аудита:** официальный установочный образ из [GUEST_REQUIREMENTS.md](GUEST_REQUIREMENTS.md) и [SUPPORT_MATRIX.md](SUPPORT_MATRIX.md): `iso/Windows11_Client_x64_en-us_26300_9457.iso` (9 047 330 816 байт, SHA-256 `bd4307df…ea650` по `SUPPORT_MATRIX.md`; в этой задаче хеш всего ISO не пересчитывался). WinPE/Setup — `sources/boot.wim`, build 26100.9457 (по `GUEST_REQUIREMENTS.md`).

**Главное ограничение аудита:** прошивки ещё нет, поэтому трасса реального загрузчика (`bootx64.efi` → `bootmgfw.efi` → `winload.efi`) не снята. Всё, что относится к фактическим обращениям загрузчика (какие протоколы он ищет, какие переменные читает, какие команды TPM отправляет), остаётся **гипотезой до трассы S7-32**, даже если требование опубликовано Microsoft. Опубликованное требование говорит, что платформа обязана предоставить; трасса покажет, что выбранный build действительно вызывает.

## Статусы

| Статус | Значение |
|---|---|
| **подтверждено** | Наблюдение с записанным способом проверки: чтение байтов ISO (команда и хеш), сравнение с принятым документом проекта (файл и раздел). |
| **по документации Microsoft** | Текст страницы Microsoft открыт и прочитан 2026-11-07; ссылка и дата статьи — в таблице источников. Требование опубликовано, но его использование выбранным build не наблюдалось. |
| **гипотеза — не проверено** | Вывод или предположение без прочитанного источника и без наблюдения; в том числе пункты, чей нормативный текст (UEFI 2.11, ACPI 6.6, тексты TCG) открыть не удалось. Проверяется трассой S7-32 или задачей, указанной в строке. |
| **источник не найден** | Поиск официального источника выполнен и ничего не дал; поведение по памяти не записывается. |

## Источники

Дата доступа ко всем источникам — **2026-11-07**. «Дата статьи» — значение метаданных `ms.date` страницы Microsoft Learn (или поля даты страницы Microsoft Support в её формате). Текст страниц получен HTTP-запросом (статус 200) и разобран из HTML; PDF — `pdftotext` (poppler) из байтов, сохранённых инструментом WebFetch (SHA-256 файла записан).

| ID | Документ | URL | Дата статьи / редакция | Примечание |
|---|---|---|---|---|
| M1 | Windows 11 requirements | https://learn.microsoft.com/en-us/windows/whats-new/windows-11-requirements | 2026-07-14 | Разделы «Hardware requirements», «Virtual machine support» |
| M2 | Minimum Hardware Requirements for Windows 11 (PDF) | https://download.microsoft.com/download/7/8/8/788bf5ab-0751-4928-a22c-dffdc23c27f2/Minimum%20Hardware%20Requirements%20for%20Windows%2011.pdf | «June 2021», change log до Nov 2021; SHA-256 файла `4e29ce08154ea6be734e662985041bcc80bb8b09fa4293c819d5fca5daac160f` | §3.5 Processor, §3.6.1 TPM, §3.6.2 UEFI and Secure Boot, §3.7 Storage, §3.8 System memory |
| M3 | UEFI requirements for Windows editions on SoC platforms | https://learn.microsoft.com/en-us/windows-hardware/drivers/bringup/uefi-requirements-that-apply-to-all-windows-platforms | 2023-03-23 | Заголовок страницы — «on SoC Platforms», вводный абзац — «Windows 10 for desktop editions». Таблица помечает «Explicit Windows requirement» — сервисы и протоколы, которые вызывает компонент Windows. Применимость SoC-специфичных пунктов к x64 VM не сверена |
| M4 | UEFI firmware requirements | https://learn.microsoft.com/en-us/windows-hardware/design/device-experiences/oem-uefi | 2017-10-05 | Раздел «UEFI Runtime requirements» |
| M5 | Secure boot | https://learn.microsoft.com/en-us/windows-hardware/design/device-experiences/oem-secure-boot | 2026-03-10 | Таблица «Secure boot requirements» |
| M6 | Windows Secure Boot Key Creation and Management Guidance | https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/windows-secure-boot-key-creation-and-management-guidance | 2026-07-15 | §1.6 конфигурация 25H2+ |
| M7 | KB5025885: Windows Boot Manager revocations (CVE-2023-24932) | https://support.microsoft.com/en-us/topic/kb5025885-how-to-manage-the-windows-boot-manager-revocations-for-secure-boot-changes-associated-with-cve-2023-24932-41a975df-beb2-40c1-99a3-b3ff139f832d | `06/02/2026` (как в метаданных страницы) | Отзыв PCA 2011 через DBX, SVN, загрузочные носители |
| M8 | Windows Secure Boot certificate expiration and CA updates | https://support.microsoft.com/en-us/topic/windows-secure-boot-certificate-expiration-and-ca-updates-7ff40d33-95dc-4c3c-8725-a9b95457578e | `05/06/2026` (как в метаданных страницы) | Истечение сертификатов 2011 |
| M9 | TPM recommendations | https://learn.microsoft.com/en-us/windows/security/hardware-security/tpm/tpm-recommendations | 2025-08-15 | Таблица «TPM and Windows Features», примечание о CSM |
| M10 | ACPI system description tables | https://learn.microsoft.com/en-us/windows-hardware/drivers/bringup/acpi-system-description-tables | 2025-04-30 | Ссылается на ACPI 5.0; ориентирована на SoC/hardware-reduced |
| M11 | SMBIOS (Windows drivers, bring-up) | https://learn.microsoft.com/en-us/windows-hardware/drivers/bringup/smbios | 2024-09-26 | Рекомендуемые поля SMBIOS 3.0, типы 0/1/2/3 |
| M12 | Boot screen components | https://learn.microsoft.com/en-us/windows-hardware/drivers/bringup/boot-screen-components | 2024-09-24 | GOP → драйвер ОС, BGRT |
| M13 | StorNVMe command set support | https://learn.microsoft.com/en-us/windows-hardware/drivers/storage/stornvme-command-set-support | 2024-12-20 | Колонка «Driver Int Use» — команды, которые `stornvme` может выдавать сам |
| M14 | Windows startup issues troubleshooting | https://learn.microsoft.com/en-us/windows/client-management/advanced-troubleshooting-boot-problems | 2026-02-12 | Фазы PreBoot → Boot Manager → OS Loader → Kernel |
| M15 | BCD system store settings for UEFI | https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/bcd-system-store-settings-for-uefi | 2021-10-07 | `bootmgfw.efi`, переменные NVRAM |
| M16 | Boot to UEFI mode or legacy BIOS mode | https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/boot-to-uefi-mode-or-legacy-bios-mode | 2021-09-29 | `\EFI\BOOT\BOOTX64.EFI` на носителе WinPE/Setup |
| M17 | Windows PE (WinPE) | https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/winpe-intro | 2021-09-30 | «Hardware requirements», заметки о Setup в WinPE |
| M18 | WinPE: Create bootable media | https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/winpe-create-usb-bootable-drive | 2021-08-25 | FAT32 и файлы больше 4 GB |
| M19 | Oscdimg command-line options | https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/oscdimg-command-line-options | 2021-10-09 | El Torito: `Efisys.bin`, platform ID `0xEF`, `-bootdata` |
| M20 | Windows processor requirements | https://learn.microsoft.com/en-us/windows-hardware/design/minimum/windows-processor-requirements | 2026-08-31 | Списка инструкций нет; отсылает к M2 |
| T1 | TCG PC Client Platform TPM Profile (PTP) — страница ресурса | https://trustedcomputinggroup.org/resource/pc-client-platform-tpm-profile-ptp-specification/ | На странице есть «TCG Errata v1.1 for PTP 1.07» | Открыта только страница ресурса; текст PDF не читался |
| T2 | TCG EFI Protocol Specification — страница ресурса | https://trustedcomputinggroup.org/resource/tcg-efi-protocol-specification/ | На странице есть «Errata Version 0.6 for … Version 1.0 Revision 0.13» | То же; последняя ли это редакция — не сверено |
| T3 | TCG PC Client Platform Firmware Profile (PFP) — страница ресурса | https://trustedcomputinggroup.org/resource/pc-client-specific-platform-firmware-profile-specification/ | На странице есть «Version 1.06 Revision 52» | То же |
| T4 | TCG ACPI Specification — страница ресурса | https://trustedcomputinggroup.org/resource/tcg-acpi-specification/ | На странице есть «Version 1.50» | То же |
| T5 | TCG Platform Reset Attack Mitigation — страница ресурса | https://trustedcomputinggroup.org/resource/pc-client-work-group-platform-reset-attack-mitigation-specification/ | На странице есть «Version 1.2» | То же |
| U1 | UEFI 2.11: `03_Boot_Manager.html`, `08_Services_Runtime_Services.html`, `13_Protocols_Media_Access.html`, `32_Secure_Boot_and_Driver_Signing.html` в https://uefi.org/specs/UEFI/2.11/ | — | **HTTP 403 — не сверено** | Редакция по [VIRTUAL_BOARD.md](VIRTUAL_BOARD.md) «Нормативные источники и редакции» |
| U2 | ACPI 6.6: `05_ACPI_Software_Programming_Model.html` в https://uefi.org/specs/ACPI/6.6/ | — | **HTTP 403 — не сверено** | То же |
| C7 | Context7 | — | — | Библиотеки спецификаций UEFI/ACPI/TCG **не найдены** (только `uefi-rs`, не нормативный источник, не использовался). Найдена `/microsoftdocs/windows-driver-docs`; вместо неё прочитаны первоисточники M3, M10–M13 |
| ISO | Наблюдения в ISO | `iso/Windows11_Client_x64_en-us_26300_9457.iso` (каталог `iso/` основного checkout, только чтение) | — | 7-Zip `7zz` 26.05 (arm64): `7zz l` (листинг) и `7zz e -so` (поток в память, на диск ничего не извлекалось); Python 3.15.2 `-I`: чтение секторов 16–17 и boot catalog, разбор заголовков PE/WIM/FAT, SHA-256. Скрипты вне репозитория, не сохраняются. Файлы из ISO не запускались |
| P | Документы проекта | [GUEST_REQUIREMENTS.md](GUEST_REQUIREMENTS.md), [SUPPORT_MATRIX.md](SUPPORT_MATRIX.md), [CPU_CONTRACT.md](CPU_CONTRACT.md), [FIRMWARE_BOOT.md](FIRMWARE_BOOT.md), [DISPLAY_DEVICE.md](DISPLAY_DEVICE.md), [MACHINE_CONTRACT.md](MACHINE_CONTRACT.md), [VTPM.md](VTPM.md), [ADR-0009](../decisions/ADR-0009-vtpm-placement-and-crypto.md), [план этапа 7](../plans/STAGE_7.md) §1.1 | состояние на коммит `24c6f39` | Сравнение с требованием — статус «подтверждено» только для самого факта расхождения/совпадения |

Найдены, но **не использованы как источник**: форумные и новостные пересказы (ntlite.com, pureinfotech.com и т. п.) — только как подсказка, где искать.

## Цели и границы

Windows boot path на UEFI x64 по M14 и M15: прошивка запускает EFI-приложение Windows Boot Manager (`bootmgfw.efi`; на съёмном носителе — по пути `\EFI\BOOT\BOOTX64.EFI`, M16), Boot Manager читает BCD и запускает OS Loader `winload.efi`, тот загружает boot-start драйверы и `ntoskrnl.exe`, затем вызывает `ExitBootServices`. Для установочного образа BCD указывает на WinPE в `sources/boot.wim`, загружаемый как RAM disk (M17).

S7-32 проверяет путь только **до экрана Setup WinPE** (решение владельца D2, [план §1.1](../plans/STAGE_7.md)): firmware → `bootx64.efi` → `bootmgfw.efi` → `winload.efi` → ядро WinPE → экран Setup на GOP, с включённым Secure Boot (D6) и видимым TPM. Установка на диск, драйверы WDDM, BitLocker, Windows Hello, attestation — этап 8 и дальше.

Этот документ даёт S7-32 чеклист (раздел «Чеклист для S7-32») и перечисляет входы для задач S7-13, S7-14, S7-15, S7-16, S7-17…S7-19, S7-21…S7-24, S7-34, S7-35.

## 1. Носитель и цепочка загрузки (наблюдения в ISO)

| № | Требование / факт | Источник | Статус | Как проверено / примечание | Задача |
|---|---|---|---|---|---|
| A1 | ISO — UDF 1.02 (кластер/сектор 2048 байт) с мостом ISO-9660: sector 16 — Primary Volume Descriptor `CD001`, sector 17 — Boot Record Volume Descriptor El Torito, boot catalog — LBA 22 | ISO | подтверждено | `7zz l -slt`: `Type = Udf`, `Version = 1.02`, `Sector Size = 2048`; Python: чтение секторов 16–17 | S7-35 |
| A2 | Boot catalog: validation entry (platform `0x00`, ключ `55AA`); initial entry — BIOS (`0x00`), bootable `0x88`, no emulation, 8 секторов по 512, RBA 534; section entry — platform `0xEF` (UEFI), bootable `0x88`, media 0 (no emulation), sector count **1**, RBA 536 | ISO | подтверждено | Разбор 2048 байт сектора 22. Как UEFI 2.11 определяет размер образа no-emulation при sector count 1 — не сверено (U1, 403) | S7-35 |
| A3 | Образ El Torito для UEFI (RBA 536) побайтно равен `efi/microsoft/boot/efisys.bin` (1 474 560 байт, SHA-256 `52f40745…bac7b0`), а не `efisys_noprompt.bin` (SHA-256 `bc1df11a…66c4d5`). Это FAT12: OEM `MSDOS5.0`, 512 байт/сектор, 2880 секторов | ISO | подтверждено | Сравнение 1 474 560 байт с RBA 536 и потока `7zz e -so`; BPB и сигнатура `55AA` | S7-35, S7-34 |
| A4 | В ISO есть оба варианта: `efisys.bin`/`cdboot.efi` (SHA-256 `7cd32d9c…a2e8`) и `efisys_noprompt.bin`/`cdboot_noprompt.efi` (SHA-256 `106fc457…6194`) | ISO | подтверждено | Листинг и хеши | S7-34, S7-35 |
| A5 | Официальное описание запроса «Press any key to boot from CD or DVD» и разницы `efisys.bin`/`efisys_noprompt.bin` | — | источник не найден | M19 описывает `Efisys.bin` и `-bootdata`, но не запрос и не `noprompt`; поиск по learn.microsoft.com дал только форумы | S7-34 |
| A6 | Запрос клавиши выводит `cdboot.efi` (путь CD), а запуск `\EFI\BOOT\BOOTX64.EFI` напрямую (носитель, сконвертированный в GPT/FAT, вариант (б) S7-35) запрос не выводит | — | гипотеза — не проверено | Вывод из имён файлов; строки бинарей не искались (вне рамок этой задачи). Если верно, S7-32 не blocked до S7-34 при варианте (б) S7-35 | S7-34, S7-35, S7-32 |
| A7 | `efi/boot/bootx64.efi` и корневой `bootmgfw.efi` — один и тот же файл: 3 087 400 байт, SHA-256 `4bb32c300df0815d8b6c54a0d3793ae1d78fcdc07f8e2b9f831289b2eb109092` | ISO | подтверждено | SHA-256 потоков `7zz e -so` | S7-19, S7-32 |
| A8 | Заголовки PE `bootmgfw.efi`: `Machine = 0x8664`, PE32+ (`0x20B`), `Subsystem = 10` (EFI application), `DllCharacteristics = 0x0100` (NX_COMPAT), `SizeOfImage = 0x33A000`, секции `.text TRANSIT .data .pdata .rsrc .reloc`, `.reloc` 5312 байт, **таблица импорта пуста (размер 0)**, security directory 9768 байт (`WIN_CERTIFICATE` rev `0x200`, type `0x2` — PKCS signed data). У `cdboot*.efi` — те же `Machine/Subsystem/DllCharacteristics`, импорт пуст | ISO | подтверждено по предыдущему аудиту | PE metadata записаны ранее; исходные байты сейчас отсутствуют и не пересматривались. Пустая таблица импорта не позволяет составить протокольный список по imports. Статический поиск GUID не выполнен; статус и доступность источника — в [windows_media_manifest.json](../../tests/fixtures/firmware/windows_media_manifest.json) и ISSUE-511 | S7-06, S7-24 |
| A9 | Цепочка подписи `bootmgfw.efi`, `cdboot.efi`, `cdboot_noprompt.efi` содержит строку `Microsoft Windows Production PCA 2011` (и `Microsoft Root Certificate Authority 2010`, `Microsoft Time-Stamp PCA 2010` для штампа времени). Строки `Windows UEFI CA 2023` нет | ISO | подтверждено | Поиск ASCII-строк только внутри security directory. Это не проверка подписи: цепочка и подписант криптографически не проверены (S7-24) | S7-24 |
| A10 | `sources/boot.wim`: 612 018 500 байт, заголовок `MSWIM\0\0\0`, `header_size = 208`, `version = 0x10D00`, `flags = 0x40082`, chunk 32768, часть 1/1, **2 образа** | ISO | подтверждено | Первые 208 байт потока. Build 26100.9457 и индексы 1/2 — по `GUEST_REQUIREMENTS.md` | S7-32 |
| A11 | `winload.efi` вне WIM на ISO отсутствует (0 вхождений в листинге UDF); он внутри `boot.wim` | ISO | подтверждено (отсутствие); наличие в `boot.wim` — гипотеза — не проверено | `7zz l | grep -ci winload` = 0; содержимое WIM в этой задаче не листилось | S7-32 |
| A12 | `sources/install.wim` — 8 155 984 950 байт, больше 4 GiB; по M18 носитель WinPE форматируется в FAT32, а для файлов больше 4 GB предлагается дополнительный раздел NTFS | ISO, M18 | подтверждено (размер); по документации Microsoft (ограничение FAT32) | Вариант (б) S7-35 «ISO → GPT/FAT» не может положить `install.wim` в один FAT32-раздел без разбиения/второго раздела. Нужен ли `install.wim` до экрана Setup — гипотеза — не проверено | S7-35 |
| A13 | Путь съёмного носителя `\EFI\BOOT\BOOTX64.EFI` на носителе WinPE/Setup | M16, ISO | по документации Microsoft; файл подтверждён в ISO | — | S7-19 |
| A14 | Цепочка фаз: PreBoot → Windows Boot Manager `\EFI\Microsoft\Boot\bootmgfw.efi` → OS Loader `winload.efi` → kernel | M14, M15 | по документации Microsoft | Трасса фаз — S7-32 | S7-32 |
| A15 | BCD носителя: `efi/microsoft/boot/bcd` (16 384 байт), там же `boot.stl`, `winsipolicy.p7b`, три `cipolicies/active/*.cip`, `resources/bootres.dll`, шрифты | ISO | подтверждено | Листинг. Назначение файлов `cip`/`p7b` для прошивки не требуется (читает Boot Manager) — гипотеза — не проверено | S7-35 (что копировать) |

## 2. UEFI: таблицы, Boot Services, Runtime Services, протоколы

Основной источник — таблица «Summary of requirements» M3 (ссылки на разделы UEFI 2.3.1). «Explicit» = в M3 стоит «Explicit Windows requirement».

| № | Требование | Источник | Статус | Примечание | Задача |
|---|---|---|---|---|---|
| U-01 | System Table (explicit); configuration table `EFI_ACPI_TABLE_GUID` указывает на RSDP; `SMBIOS_TABLE_GUID` — на SMBIOS Entry Point; SMBIOS ≥ 2.4, разделы 3.2 «Required Structures and Data» и 4 обязательны | M3 | по документации Microsoft | Использует ли выбранный build `SMBIOS3_TABLE_GUID` (64-bit entry point, план S7-13) вместо/вместе с 32-bit — гипотеза — не проверено | S7-05, S7-13 |
| U-02 | Boot Services: все memory services (explicit) | M3 | по документации Microsoft | — | S7-05 |
| U-03 | Protocol handler services (explicit): `OpenProtocol`, `CloseProtocol`, `LocateDevicePath`, `LocateHandle` | M3 | по документации Microsoft | `HandleProtocol`, `LocateHandleBuffer`, `LocateProtocol` в M3 не названы; вызывает ли их build — гипотеза — не проверено | S7-05 |
| U-04 | Image services (explicit): `ExitBootServices` | M3 | по документации Microsoft | `LoadImage`/`StartImage` нужны самой прошивке, чтобы запустить `bootx64.efi`; вызывает ли их Boot Manager для `winload.efi` — гипотеза — не проверено | S7-05, S7-06, S7-19 |
| U-05 | Miscellaneous services (explicit): `Stall` с детерминированной (повторяемой) ошибкой | M3 | по документации Microsoft | Совместимость с опросом TSC по [FIRMWARE_BOOT.md](FIRMWARE_BOOT.md) §8 — проверяет S7-05 | S7-05 |
| U-06 | Event, timer and task services (раздел 6.1) — в перечне, без пометки explicit | M3 | по документации Microsoft | Ждёт ли Boot Manager ввод через `WaitForEvent`/таймер на экране меню — гипотеза — не проверено | S7-05, S7-07 |
| U-07 | Runtime: `GetTime`, `SetTime` (explicit; по M3 вызываются только до `ExitBootServices`); `ResetSystem` (explicit) с режимами reset и shutdown; все Variable services (explicit) | M3 | по документации Microsoft | Подтверждает минимум S7-14 (PM1a `SLP_TYP/SLP_EN` для S5, reset register, RTC) | S7-11, S7-14 |
| U-08 | Runtime: `GetTime`, `SetTime`, `UpdateCapsule`, `ResetSystem` («некоторые или все», рекомендовано все) | M4 (2017) | по документации Microsoft | Расхождение с [FIRMWARE_BOOT.md](FIRMWARE_BOOT.md) §13 «capsule update не гарантируется»: S7-11 должен записать поведение `UpdateCapsule`/`QueryCapsuleCapabilities` (статус ошибки, не `SUCCESS` без действия) | S7-11 |
| U-09 | Runtime-память неизменна по размеру и расположению при переходах S4 | M4 | по документации Microsoft | S4 вне этапа 7 (FIRMWARE_BOOT §13); запись для этапа 8 | этап 8 |
| U-10 | Windows вызывает `SetVirtualAddressMap` | — | источник не найден | В M3/M4 не упомянут; решение 4 FIRMWARE_BOOT остаётся обязательным по UEFI. Наблюдать в S7-32 | S7-11, S7-32 |
| U-11 | Required protocols: Loaded Image, Loaded Image Device Path, Device Path (explicit), Device Path Utilities, Decompress, EBC interpreter | M3 | по документации Microsoft | Decompress и EBC в профиле прошивки не планируются: отсутствие записать в `UEFI_PROFILE.md` как отступление; использует ли их Windows — гипотеза — не проверено | S7-05, S7-06 |
| U-12 | Simple Text Input (explicit, «для систем со встроенной или подключённой клавиатурой»); Simple Text Input Ex, Simple Text Output — в перечне | M3 | по документации Microsoft | В v1 ConIn — только COM1 ([план](../plans/STAGE_7.md) S7-07, S7-34) | S7-07, S7-34 |
| U-13 | GOP (explicit): `PixelFormat = PixelBlueGreenRedReserved8BitPerColor`; `PixelsPerScanLine = HorizontalResolution`; физический framebuffer (`PixelBltOnly` не поддерживается); после передачи управления boot application прошивка не трогает framebuffer; framebuffer продолжает выводиться после `ExitBootServices`; разрешение — родное для дисплея | M3 | по документации Microsoft | — | S7-15 |
| U-14 | Наше дисплейное устройство удовлетворяет U-13 по формату и stride: `FORMAT = 1` совпадает с `PixelBlueGreenRedReserved8BitPerColor`, `STRIDE = ACTIVE_WIDTH × 4` без паддинга (значит `PixelsPerScanLine = HorizontalResolution`); режим 1280 × 720 есть в списке | P: [DISPLAY_DEVICE.md](DISPLAY_DEVICE.md) §3, §11, таблица режимов ревизии 2 | подтверждено | Сравнение документов; поведение GOP не проверено | S7-15 |
| U-15 | EDID Discovered/Active protocols — в перечне M3 | M3 | по документации Microsoft | У устройства S10-03 EDID нет в контракте; нужен ли он Windows для выбора режима — гипотеза — не проверено | S7-15, S10-18 |
| U-16 | Block I/O (explicit); Disk I/O, Simple File System, Unicode Collation — в перечне без explicit. «Local storage boot»: Block I/O и Device Path для носителя с ESP и разделом ОС | M3 | по документации Microsoft | — | S7-17, S7-18 |
| U-17 | Boot Manager Windows читает файлы (BCD, `boot.wim`) собственными драйверами ФС поверх Block I/O, а не через Simple File System прошивки | M3 (косвенно: SFS не explicit) | гипотеза — не проверено | Определяет, нужна ли прошивке поддержка UDF/El Torito для `boot.wim` или только для запуска `bootx64.efi` | S7-35, S7-18 |
| U-18 | Serial I/O, SNP, PXE, BIS — в перечне M3 | M3 | по документации Microsoft | Сеть вне профиля v1 (D1); Serial I/O — решение `UEFI_PROFILE.md` | S7-05, S7-07 |
| U-19 | Firmware boot manager: поведение по умолчанию раздела 3.3 и, для multi-boot, глобальные переменные и требования раздела 3.1 (explicit) | M3 | по документации Microsoft | Номера разделов — UEFI 2.3.1; соответствие разделам UEFI 2.11 не сверено (U1, 403) | S7-19 |
| U-20 | Secure Boot по разделу 27 UEFI 2.3.1 (explicit) | M3 | по документации Microsoft | Подробно — раздел 4 | S7-24 |
| U-21 | `EFI_HASH_PROTOCOL` (SHA-256) и `EFI_RNG_PROTOCOL` — MANDATORY в M3 | M3 | по документации Microsoft (для SoC) | Нужны ли они на x64 и запрашивает ли их `bootmgfw`/`winload` (например, энтропия для загрузчика) — гипотеза — не проверено. Если трасса S7-32 покажет запрос, добавить в `UEFI_PROFILE.md` | S7-05, S7-32 |
| U-22 | `EFI_TCG2_PROTOCOL` ищется Boot Manager/OS Loader для measured boot | T2 (страница), M3 (TrEE для SoC) | гипотеза — не проверено | M3 описывает устаревший TrEE-протокол SoC; для x64 протокол TCG2 по T2 — текст не читался | S7-22 |
| U-23 | При отказе проверки подписи образ не исполняется, причина добавляется в `EFI_IMAGE_EXECUTION_TABLE` | M3 (Requirement 8) | по документации Microsoft | В плане S7-24 таблица не названа — вход S7-24 | S7-24 |
| U-24 | Partition Info protocol, CD-ROM Media Device Path, El Torito по UEFI 2.11 «ISO-9660 and El Torito» | U1 | гипотеза — не проверено | Текст UEFI 2.11 не сверен (HTTP 403) | S7-17, S7-35 |
| U-25 | Платформа — UEFI Class 3 без CSM; legacy PC/AT boot выключен | M3 (Requirement 2), M9 | по документации Microsoft | Совпадает с границами этапа (нет legacy BIOS/CSM) | — |
| U-26 | Прошивка соответствует UEFI 2.3.1 Errata или выше | M2 §3.6.2 | по документации Microsoft | Наша целевая редакция — 2.11 ([VIRTUAL_BOARD.md](VIRTUAL_BOARD.md)) | — |

## 3. ACPI и SMBIOS

| № | Требование | Источник | Статус | Примечание | Задача |
|---|---|---|---|---|---|
| C-01 | RSDP находится через EFI System Table; при наличии RSDT и XSDT Windows предпочитает XSDT | M10 | по документации Microsoft | Совпадает с планом S7-13 (RSDP rev 2 + XSDT) | S7-13 |
| C-02 | RSDT/XSDT указывает на FADT, MADT, DSDT, при необходимости SSDT; для SoC также CSRT, DBG2, BGRT, FPDT | M10 | по документации Microsoft | Страница ориентирована на SoC; обязательность CSRT/DBG2/FPDT для x64 VM — гипотеза — не проверено | S7-13 |
| C-03 | FADT: флаг `HW_REDUCED_ACPI` (бит 20) обязан стоять, если fixed hardware programming model не реализован | M10 | по документации Microsoft | План S7-13/S7-14 реализует fixed hardware и флаг не ставит — согласовано | S7-13, S7-14 |
| C-04 | `IAPC_BOOT_ARCH` «CMOS RTC Not Present» (бит 5): если установлен, платформа обязана реализовать ACPI Time and Alarm Control Method device | M10 | по документации Microsoft | Выбор S7-14: RTC/CMOS (флаг сброшен) или TAD в DSDT (S7-12) | S7-14, S7-12, S7-13 |
| C-05 | MADT: LAPIC на каждый процессор, IOAPIC, overrides | M10 (общая роль MADT) | по документации Microsoft (роль таблицы); состав полей — гипотеза — не проверено | ACPI 6.6 не сверен (U2, 403) | S7-13 |
| C-06 | Для 2 vCPU в MADT два LAPIC-входа; handoff block v1 передаёт `cpu_count = 1`, прошивка не предоставляет MP Services | P: [FIRMWARE_BOOT.md](FIRMWARE_BOOT.md) §6.1, §13; M1/M2 (2 ядра) | подтверждено (расхождение) | Нужно изменение handoff block и MADT при этапе 6 | S7-13, S7-32, этап 6 |
| C-07 | MCFG/ECAM и PCI: `stornvme` сопоставляется по `PCI\CC_010802`; контроллеры на шине обязаны иметь правильный class/subclass code | P: [GUEST_REQUIREMENTS.md](GUEST_REQUIREMENTS.md) (статический INF); M2 §3.7.2 | подтверждено (INF); по документации Microsoft (class code) | MCFG — только после S4-30 | S7-13, S7-16, S4-30 |
| C-08 | Таблица TPM2 (`StartMethod = 7`, CRB) и объект TPM в DSDT (идентификатор `_HID`) нужны `tpm.sys`, чтобы найти TPM | T4 (страница) | гипотеза — не проверено | Текст TCG ACPI 1.50 не читался; `_HID` устройства TPM в DSDT план не задаёт — открытый вопрос S7-12/S7-22 | S7-12, S7-22 |
| C-09 | HPET, PM timer — требуются ли Windows | — | источник не найден | Ни M3, ни M10 их не называют. S7-14 оставляет HPET «только при подтверждении» | S7-14 |
| C-10 | `_S5`/выключение через ACPI или через `ResetSystem(EfiResetShutdown)` | M3 (ResetSystem shutdown) | по документации Microsoft (RT); путь ACPI `_S5` — гипотеза — не проверено | — | S7-12, S7-14 |
| C-11 | BGRT: логотип в `EfiBootServicesData`, 24/32-битный bitmap, смещения X/Y | M12 | по документации Microsoft | Необязателен для экрана Setup — гипотеза — не проверено | S7-13 (опционально) |
| C-12 | SMBIOS: Windows требует ≥ 2.4 (M3); рекомендованы поля SMBIOS 3.0 для Type 0, 1, 2, 3 (UUID по RFC 4122, Serial Number и т. д.) | M3, M11 | по документации Microsoft | План S7-13 — SMBIOS 3.9, UUID и serial из handoff block — согласовано | S7-13 |

## 4. Переменные и Secure Boot

| № | Требование | Источник | Статус | Примечание | Задача |
|---|---|---|---|---|---|
| V-01 | `SecureBoot = 1`, `SetupMode = 0`, предустановленный `db` (`EFI_IMAGE_SECURITY_DATABASE`), `PK`, валидный `KEK` | M5 | по документации Microsoft | Совпадает с D6 (UserMode, `SecureBoot=1`) | S7-24 |
| V-02 | `dbx` (`EFI_IMAGE_SECURITY_DATABASE1`) обязан присутствовать; `KEK` от Microsoft; `PK` хранится в прошивке; начальные базы меняются только authenticated write | M3 (Requirements 4–7) | по документации Microsoft | Требование M3 «no additional KEKs/signatures» — для SoC; для x64 действует M6 | S7-24 |
| V-03 | Конфигурация 25H2+: `KEK` — Microsoft Corporation KEK 2K CA 2023; `db` — Windows UEFI CA 2023 (альтернатива для Linux/сторонних приложений: + Microsoft UEFI CA 2023, Microsoft Corporation UEFI CA 2011, Microsoft Option ROM UEFI CA 2023); `dbx` — последний пакет; PCA 2011, UEFI CA 2011, KEK CA 2011 — только для совместимости | M6 §1.6 | по документации Microsoft | D3 включает 2011 и 2023, без Option ROM CA — согласовано с альтернативной конфигурацией M6; базы Microsoft в формате EDK II — открытый репозиторий, на который ссылается M6 | S7-24 |
| V-04 | Для этого ISO `db` обязан доверять цепочке PCA 2011: подпись `bootmgfw.efi` содержит только `Microsoft Windows Production PCA 2011` (A9) | ISO (A9), D3 | подтверждено (строки); криптографическая проверка — гипотеза — не проверено | D3 включает PCA 2011 — согласовано | S7-24, S7-32 |
| V-05 | Windows может добавить сертификат PCA 2011 в `dbx` (митигация CVE-2023-24932) и применить SVN; после этого загрузчики, подписанные PCA 2011, не запускаются, старые носители перестают грузиться | M7 | по документации Microsoft | Следствие для D4: начальный `dbx` не должен содержать запись PCA 2011, иначе этот ISO не загрузится (вывод аудитора, не источник). Риск жизненного цикла — открытый вопрос 13 (раздел 10). Содержит ли её «официальный список» D4 — не сверено, проверяет S7-24 | S7-24, S7-32 |
| V-06 | Сертификаты 2011 истекают с июня 2026; устройства без 2023 продолжают загружаться | M8 | по документации Microsoft | Согласовано с D7 (срок X.509 не проверяется) | S7-24 |
| V-07 | Механизм time-based authenticated variables (раздел 27 UEFI 2.3.1) обязателен | M3 | по документации Microsoft | — | S7-24 |
| V-08 | Измерения Secure Boot: M3 (Requirement 23, SoC/TrEE) требует расширять PCR[03] дайджестом `PK` и `db` событиями `EV_EFI_VARIABLE_CONFIG`, дайджесты SHA-256 | M3 | по документации Microsoft (для SoC, устаревший TrEE) | План S7-22/S7-24 следует PFP (PCR7, `EV_EFI_VARIABLE_DRIVER_CONFIG`); PFP 1.06 rev 52 (T3) не читался — расхождение записано, решение за S7-24 после сверки PFP | S7-22, S7-24 |
| V-09 | `MemoryOverwriteRequestControl` по TCG Platform Reset Attack Mitigation | M3 (Requirement 24, SoC), T5 (v1.2, страница) | по документации Microsoft (для SoC); для x64 — гипотеза — не проверено | Страница Microsoft «Secure MOR implementation» по известному пути вернула HTTP 404 — не сверено. `MemoryOverwriteRequestControlLock` — источник не найден | S7-09, S7-11 |
| V-10 | `BootOrder`, `Boot####`, `BootNext`, `BootCurrent`, `Timeout` (глобальные переменные раздела 3) | M3 (разделы 3.1/3.3), U1 | по документации Microsoft (требование разделов); перечень имён — гипотеза — не проверено | UEFI 2.11 не сверен (403). Пишет ли Setup `Boot####` — этап 8 | S7-19 |
| V-11 | `OsIndications`/`OsIndicationsSupported` нужны Boot Manager (например, переход в UI прошивки) | — | источник не найден | Наблюдать в S7-32 | S7-09, S7-32 |
| V-12 | `AuditMode`, `DeployedMode`, `VendorKeys`, `SignatureSupport`, `dbt`, `dbr` — нужны ли Windows | U1 | источник не найден | План S7-24 вводит их по UEFI; текст 2.11 не сверен | S7-24 |
| V-13 | Компоненты прошивки подписаны не слабее RSA-2048/SHA-256; защита от отката прошивки | M5 | по документации Microsoft | Требование к сертифицируемым OEM-устройствам; сертификация не заявляется (план §1) | — |

## 5. TPM 2.0

| № | Требование | Источник | Статус | Примечание | Задача |
|---|---|---|---|---|---|
| T-01 | TPM 2.0 обязателен (Windows 11), в том числе для VM (vTPM) | M1 | по документации Microsoft | — | S7-21…S7-23 |
| T-02 | Банки PCR SHA-256, PCR 0–23 в SHA-256; допустим один переключаемый банк для SHA-256 | M2 §3.6.1 | по документации Microsoft | Согласовано с S7-21 (SHA-256 обязателен). Нужен ли банк SHA-1 — источник не найден; ADR-0009 оставляет решение S7-21 | S7-21 |
| T-03 | Команда `TPM2_HMAC` обязана поддерживаться | M2 §3.6.1 | по документации Microsoft | **Нет в списках S7-21 и S7-23** — вход S7-23 (и ADR-0009, решение 3) | S7-23 |
| T-04 | EK certificate предустановлен производителем или может быть получен устройством при первой загрузке | M2 §3.6.1 | по документации Microsoft | У собственного vTPM нет производителя: кто выпускает EK certificate и в каком NV-индексе он лежит — открытый вопрос S7-23 (шаблоны EK — TCG EK Credential Profile, не читался) | S7-23 |
| T-05 | TPM 2.0 не поддерживается в Legacy/CSM; только native UEFI | M9 | по документации Microsoft | Согласовано | — |
| T-06 | Measured Boot требует TPM и UEFI Secure Boot; System Guard (DRTM) требует TPM 2.0 и UEFI; Device Encryption требует Modern Standby | M9 | по документации Microsoft | DRTM вне профиля (ADR-0009); Device Encryption/Modern Standby — вне этапа 7 | S7-22, этап 8 |
| T-07 | Setup в WinPE проверяет минимальные требования (TPM, Secure Boot): без компонентов WinPE-WMI и WinPE-SecureStartup Setup сообщает о несоответствии | M17 | по документации Microsoft (что проверка есть); способ обнаружения TPM — гипотеза — не проверено | На каком шаге Setup (до или после экрана Setup) выполняется проверка — гипотеза — не проверено | S7-32 |
| T-08 | Интерфейс CRB (`StartMethod = 7`) поддерживается `tpm.sys` без FIFO/TIS | — | источник не найден | ADR-0009 выбрал только CRB | S7-22 |
| T-09 | Минимальный размер буфера CRB и `TPM_PT_MAX_COMMAND_SIZE` достаточны для команд Windows (в ADR-0009 — 3968 байт) | T1 (страница) | гипотеза — не проверено | PTP 1.07 Errata 1.1 не читался | S7-21, S7-22 |
| T-10 | Event log TCG_2 (crypto agile) и final events table потребляются Windows через `EFI_TCG2_PROTOCOL` | T2, T3 (страницы) | гипотеза — не проверено | — | S7-22 |
| T-11 | Physical Presence Interface (ACPI `_DSM`) нужен для очистки TPM из ОС (`tpm.msc`) | — | источник не найден | Этап 8; при отсутствии — записать как ограничение | S7-22, этап 8 |

**Команды TPM, которые реально нужны Windows при загрузке.** Опубликованное требование — только `TPM2_HMAC`, банки SHA-256 PCR0–23 и EK certificate (T-02…T-04). Ни один документ Microsoft, найденный в этой задаче, не перечисляет команды, которые посылают прошивке/TPM `bootmgfw.efi`, `winload.efi`, `tpm.sys` или TBS. Поэтому весь список ниже — **гипотеза — не проверено** до трассы S7-32 (журнал CRB-устройства); его назначение — дать S7-21/S7-23 приоритеты, а не урезать профиль.

| № | Команда | Предполагаемый потребитель и момент | Статус | Задача |
|---|---|---|---|---|
| K-01 | `TPM2_Startup(CLEAR)`, `TPM2_SelfTest` | прошивка до запуска Boot Manager (S7-22); ОС, если TPM не стартован | гипотеза — не проверено | S7-21, S7-22 |
| K-02 | `TPM2_GetCapability` (`TPM_CAP_TPM_PROPERTIES`, `TPM_CAP_PCRS`, `TPM_CAP_COMMANDS`, `TPM_CAP_ALGS`, `TPM_CAP_HANDLES`) | Boot Manager/OS Loader через `EFI_TCG2_PROTOCOL.SubmitCommand`; `tpm.sys`, проверка Setup | гипотеза — не проверено | S7-21 |
| K-03 | `TPM2_PCR_Extend`/`TPM2_PCR_Event` (через `HashLogExtendEvent`), `TPM2_PCR_Read` | измерения Boot Manager/OS Loader (какие PCR — по трассе), чтение ОС | гипотеза — не проверено | S7-21, S7-22 |
| K-04 | `TPM2_GetRandom` | загрузчик/ядро как источник энтропии | гипотеза — не проверено | S7-21 |
| K-05 | `TPM2_ReadPublic`, `TPM2_CreatePrimary`, `TPM2_Load`, `TPM2_FlushContext`, `TPM2_EvictControl` | provisioning TPM в ОС (SRK, EK) | гипотеза — не проверено | S7-23 |
| K-06 | `TPM2_NV_ReadPublic`, `TPM2_NV_Read` (индекс EK certificate) | ОС при первой загрузке (T-04) | гипотеза — не проверено | S7-23 |
| K-07 | `TPM2_ContextSave`/`TPM2_ContextLoad` | TBS как resource manager | гипотеза — не проверено | S7-23 |
| K-08 | `TPM2_HierarchyChangeAuth`, `TPM2_DictionaryAttackParameters`, `TPM2_DictionaryAttackLockReset`, `TPM2_Clear` | provisioning и сброс TPM ОС | гипотеза — не проверено | S7-23 |
| K-09 | `TPM2_StartAuthSession`, `TPM2_PolicyPCR`, `TPM2_PolicySecret`, `TPM2_PolicyAuthValue`, `TPM2_Unseal` | BitLocker/Windows Hello (этап 8) | гипотеза — не проверено | S7-23, этап 8 |
| K-10 | `TPM2_HMAC` | требование M2; потребитель не указан | по документации Microsoft (требование); использование — гипотеза — не проверено | S7-23 |
| K-11 | `TPM2_PCR_SetAuthPolicy`/`TPM2_PCR_SetAuthValue` не нужны Windows (предложение ADR-0009, открытый вопрос 6) | — | источник не найден | S7-21 |

## 6. Диск: NVMe, Block I/O, GPT, FAT, ESP

| № | Требование | Источник | Статус | Примечание | Задача |
|---|---|---|---|---|---|
| D-01 | Block I/O и Device Path для носителя с ESP и разделом ОС; управление flash — в прошивке | M3 | по документации Microsoft | — | S7-17 |
| D-02 | Раздел ОС — на GPT; отдельный ESP обязателен | M3 «Installation requirements» | по документации Microsoft | Касается целевого диска установки (этап 8); загрузочный носитель — см. раздел 1 | S7-17, этап 8 |
| D-03 | Контроллеры хранения поддерживают загрузку через EFI и device paths по EDD-3; правильный PCI class/subclass | M2 §3.7.2 | по документации Microsoft | EDD-3 не читался | S7-16, S7-17 |
| D-04 | `stornvme` — boot-start, сопоставление `PCI\CC_010802` | P: [GUEST_REQUIREMENTS.md](GUEST_REQUIREMENTS.md) | подтверждено (статический разбор INF/WIM) | ОС видит NVMe только после PCI-привязки (S4-30) | S7-16, S7-32 |
| D-05 | Команды, которые `stornvme` может выдавать сам («Driver Int Use»): admin — Delete/Create I/O SQ и CQ, Get Log Page, Identify, Set Features (только Host Controlled Thermal Management), Get Features, Asynchronous Event Request, Firmware Commit, Firmware Image Download, Directive Send/Receive, Format NVM, Security Send/Receive, Sanitize; I/O — Flush, Write, Read. Abort в колонке «StorNVMe Supported» не отмечен | M13 | по документации Microsoft | «Может выдавать» ≠ «требует». Минимальный набор S7-16 не содержит Directive, Security, Firmware, Format, Sanitize; переносит ли `stornvme` статус `Invalid Command Opcode` на них — гипотеза — не проверено (наблюдать в S7-32) | S7-16 |
| D-06 | Пишет ли Setup/Boot Manager в ESP через Simple File System прошивки во время пути до экрана Setup | — | источник не найден | Гипотеза: запись на целевой ESP делает Windows своими драйверами (этап 8), а носитель Setup — только чтение. До трассы S7-18 сохраняет `EFI_WRITE_PROTECTED` для записи | S7-18 |
| D-07 | Блок 2048 байт у CD-носителя (UDF/El Torito) | ISO (A1) | подтверждено | Вариант (а) S7-35 требует устройства с LBA 2048 | S7-35 |

## 7. Экран и ввод

| № | Требование | Источник | Статус | Примечание | Задача |
|---|---|---|---|---|---|
| G-01 | GOP — раздел 2, U-13…U-15 | M3 | по документации Microsoft | — | S7-15 |
| G-02 | Переход «GOP → родной видеодрайвер ОС» на чёрном фоне; логотип в BGRT | M12 | по документации Microsoft | На экране Setup используется framebuffer GOP (inbox базовый видеодрайвер) — гипотеза — не проверено | S7-15, S10-18 |
| G-03 | Дисплей не меньше 720p, 8 бит на канал | M1 | по документации Microsoft | Режим 1280 × 720 есть у устройства (U-14) | S7-15 |
| I-01 | Simple Text Input нужен для выбора в меню загрузки при наличии клавиатуры | M3 | по документации Microsoft | — | S7-07, S7-34 |
| I-02 | В WinPE/Setup есть inbox `usbxhci.inf`, `usbhub3.inf`, `input.inf` (HID) | P: [GUEST_REQUIREMENTS.md](GUEST_REQUIREMENTS.md) | подтверждено (статический разбор) | Ввод в UI Setup — устройство ОС (xHCI-модель), не прошивка; для достижения экрана Setup (D2) ввод не нужен — гипотеза — не проверено | S7-34, этап 8 |
| I-03 | Нужна ли клавиатура на пути до экрана Setup | — | гипотеза — не проверено | Зависит от A6 и варианта S7-35: при El Torito с `efisys.bin` — вероятно нужен ответ на запрос; при (б) — не нужен. Решение «blocked ли S7-32 до S7-34» — после S7-35 и трассы | S7-32, S7-34, S7-35 |

## 8. Память, CPU, таймеры

| № | Требование | Источник | Статус | Примечание | Задача |
|---|---|---|---|---|---|
| R-01 | RAM ≥ 4 GB (Windows 11, VM) | M1, M2 §3.8 | по документации Microsoft | FIRMWARE_BOOT допускает до `0xF_F400_0000` | S7-32 |
| R-02 | WinPE базовый — 512 MB; для RAM disk boot нужен непрерывный участок RAM, вмещающий весь WIM; прошивке рекомендовано держать свои резервирования в начале или в конце адресного пространства | M17 | по документации Microsoft | `boot.wim` = 612 018 500 байт (A10). В карте [FIRMWARE_BOOT.md](FIRMWARE_BOOT.md) §2 нижняя RAM — один регион до `low_end ≤ 0xF400_0000`, резервирования — в первых мегабайтах: при RAM ≥ 4 GiB непрерывного участка хватает — подтверждено (сравнение) | S7-04, S7-32 |
| R-03 | Windows/`winload` используют страницы 2 MiB (large pages) | — | источник не найден | Утверждение плана (S7-32, blocked до S4-10) не имеет источника; гипотеза — не проверено. Наблюдать на S7-32: `UnsupportedPageSize` из машины | S4-10, S7-32 |
| CPU-01 | ≥ 2 ядер (2 vCPU для VM); x64; PAE, NX, SSE4.1; CMPXCHG16B, LAHF/SAHF, PrefetchW; процессор из списка поддерживаемых поколений | M2 §3.5 | по документации Microsoft | M1 и M20 списка инструкций не содержат (проверено аудитом) | S7-32, этап 6 |
| CPU-02 | Текущий целевой CPU-профиль `od-x86-64-v1-neutral-v0.1` не объявляет SSE4.1, CMPXCHG16B, LAHF/SAHF и PrefetchW: `CPUID.1 ECX = 0`, в `CPUID.80000001` перечислен только EDX (SYSCALL, NX, LM) | P: [CPU_CONTRACT.md](CPU_CONTRACT.md) «Ubuntu Generic amd64» | подтверждено чтением CPU_CONTRACT.md (строка 624); решение за владельцем | Для Windows нужен отдельный CPU-профиль; в плане этапа 7 такой зависимости S7-32 нет — открытый вопрос 6 (раздел 10). Расположение битов в CPUID — по Intel SDM, в этой задаче не сверено | S7-32, этапы 2/6 |
| CPU-03 | SSE4.2/POPCNT обязательны для 24H2 и новее | — | источник не найден | В M1/M2/M20 не упомянуто; сторонние сообщения не используются | S7-32 |
| CPU-04 | XSAVE/AVX обязательны | — | источник не найден | CPU_CONTRACT держит `CR4.OSXSAVE = 0` | S7-32 |
| CPU-05 | Состояние CPU при вызове EFI-образа (`CR4 = 0x620`, identity 4 KiB, `efiapi`) соответствует UEFI 2.11 §2.3.4 | P: [FIRMWARE_BOOT.md](FIRMWARE_BOOT.md) §7; U1 | гипотеза — не проверено | Текст §2.3.4 не сверен (403), как и в S7-02 | S7-05 |
| CPU-06 | Запуск второго процессора ядром Windows требует INIT/SIPI с real-mode trampoline (или иного механизма из MADT) | — | источник не найден | Если верно, конфликтует с «real mode не требуется» в CPU_CONTRACT — риск для этапа 6 и S7-32; гипотеза — не проверено | этап 6, S7-32 |
| CPU-07 | SLAT нужен только для Client Hyper-V | M1 | по документации Microsoft | VBS/HVCI/Hyper-V — вне scope | — |
| TM-01 | Источник времени ОС: LAPIC timer, HPET, PM timer, TSC — что из этого требуется | — | источник не найден | S7-14 делает PM timer; HPET — только при подтверждении (C-09); APIC timer — S4-28 | S7-14, S4-28 |
| TM-02 | Время суток: `GetTime`/`SetTime` до `ExitBootServices` (U-07) и RTC CMOS либо ACPI TAD (C-04) | M3, M10 | по документации Microsoft | — | S7-11, S7-14 |

## 9. Список зависимостей от плана

| Задача | Вход из этого документа |
|---|---|
| S7-05 | U-01…U-06, U-11, U-18, U-21, CPU-05: обязательные BS/RT и протоколы; отступления (Decompress, EBC) записать в `UEFI_PROFILE.md` |
| S7-06 | A8, U-04, U-11: PE32+ без импорта, NX_COMPAT, Authenticode-таблица 9768 байт |
| S7-07 | U-06, U-12, I-01 |
| S7-09 | V-09, V-11: `MemoryOverwriteRequestControl`, `OsIndications*` — кандидаты, подтверждаются трассой |
| S7-11 | U-07…U-10, TM-02: `UpdateCapsule` (статус ошибки), shutdown/reset, `SetVirtualAddressMap` наблюдать |
| S7-12 | C-04 (TAD при отсутствии CMOS RTC), C-08 (объект TPM в DSDT), C-10 (`_S5`) |
| S7-13 | U-01, C-01…C-07, C-11, C-12: XSDT, FADT без `HW_REDUCED_ACPI`, MADT (два LAPIC при 2 vCPU), MCFG после S4-30, SMBIOS ≥ 2.4 |
| S7-14 | U-07, C-03, C-04, C-09, TM-01, TM-02: PM1/S5, reset, PM timer, RTC; HPET — источника нет |
| S7-15 | U-13…U-15, G-01…G-03: формат, `PixelsPerScanLine`, физический framebuffer, работа после EBS, 720p |
| S7-16 | C-07, D-03…D-05: команды `stornvme`, PCI class `010802` |
| S7-17 | U-16, U-24, D-01, D-02 |
| S7-18 | U-16, U-17, D-06: запись в ESP до трассы не нужна |
| S7-19 | A7, A13, U-04, U-19, V-10 |
| S7-21 | T-02, T-09, K-01…K-04, K-11 |
| S7-22 | U-22, C-08, T-06, T-08…T-11, V-08, K-01, K-03 |
| S7-23 | T-03 (`TPM2_HMAC` — нет в плане), T-04 (EK certificate), K-05…K-10 |
| S7-24 | A8, A9, U-20, U-23, V-01…V-08, V-12: `EFI_IMAGE_EXECUTION_TABLE`; PCA 2011 в `db` и **не** в начальном `dbx` |
| S7-32 | весь документ как чеклист; CPU-01…CPU-06, R-01…R-03, C-06, T-07, I-03 |
| S7-34 | A4…A6, I-01…I-03 |
| S7-35 | A1…A6, A12, A15, U-17, U-24, D-07 |

## 10. Открытые вопросы и не сверено

1. **Трасса загрузчика не снята** (нет прошивки). Все пункты «гипотеза» по обращениям `bootmgfw.efi`/`winload.efi` к протоколам, переменным и TPM проверяются только в S7-32 (журнал прошивки и CRB).
2. **Поиск GUID протоколов в бинарях** (план S7-30, п. 1) — обязательный deliverable S7-30 и пока не выполнен: ISO и Windows boot binaries отсутствуют в текущем workspace. Предыдущий аудит читал листинг/заголовки и записал хеши, но GUID scan не делал. [Manifest](../../tests/fixtures/firmware/windows_media_manifest.json) сохраняет только прежние проверенные metadata, помечает их не пересчитанными и фиксирует search status `not_performed`; он не является доказательством scan. Выполнение остаётся ожидающим доступного исходного артефакта; это не решение о переносе scan на S7-32.
3. **UEFI 2.11 и ACPI 6.6** на uefi.org вернули HTTP 403 — не сверено (как и в S7-02). Тексты TCG (PTP 1.07 Errata 1.1, EFI Protocol, PFP, ACPI, Reset Attack Mitigation) и TPM Library v185 не читались; открыты только страницы ресурсов (T1–T5), и последние ли там редакции — не сверено.
4. **M3 — страница «on SoC Platforms»**: какие её пункты (EFI_HASH, EFI_RNG, MOR, TrEE, PCR[03]) обязательны для x64 VM — не сверено. Документ WHCP (System.Fundamentals.Firmware/TPM20) не открывался.
5. **«Secure MOR implementation»** (Microsoft Learn) по пути `…/drivers/bringup/secure-mor-implementation` — HTTP 404, не сверено.
6. **CPU-профиль для Windows** (CPU-02) — кандидат в blocked-список S7-32 (и этапов 2/6): профиль CPU не объявляет SSE4.1, CMPXCHG16B, LAHF/SAHF, PrefetchW, которые требует Windows (M2); решение владельца — расширить CPU-профиль (этап 2/6) или признать S7-32 blocked. Статус: подтверждено чтением CPU_CONTRACT.md (строка 624); решение за владельцем; в плане такой зависимости нет.
7. **Запуск AP** (CPU-06) и **large pages** (R-03): источник не найден, риски для этапа 6, S4-10 и S7-32.
8. **`dbx` по умолчанию и PCA 2011** (V-05): проверить в S7-24, что начальный список не отзывает PCA 2011 (вывод аудитора, не источник). См. также вопрос 13.
9. **EK certificate** собственного vTPM (T-04): кто выпускает, где хранится — S7-23.
10. **`install.wim` > 4 GiB** (A12): вариант (б) S7-35 требует второго раздела, разбиения или нужен ли `install.wim` до экрана Setup — проверить.
11. **«Press any key»** (A5, A6): официального источника нет; S7-34 остаётся условной зависимостью S7-32 до решения S7-35.
12. `tests/fixtures/firmware/windows_media_manifest.json` создан как воспроизводимая запись ранее опубликованных ISO/bootmgfw metadata. Сам ISO, `bootx64.efi`, `bootmgfw.efi` и извлечённый `winload.efi` в текущем workspace отсутствуют; размер/хеш ISO и hash bootmgfw не пересчитывались, GUID scan не выполнен. План S7-13/14/23/24 теперь ссылается на применимые audit IDs. S7-30 остаётся `in_progress` до получения доступного источника, выполнения статического GUID search и review обновлённых результатов.
13. **Жизненный цикл `dbx` и PCA 2011 (для владельца; риск для D3/D4/D5)**: если Windows внутри гостевой VM применит митигацию KB5025885 (PCA 2011 в `dbx` и SVN 5.0, M7), этот ISO (подписан только PCA 2011, V-04) в этой VM перестанет загружаться; восстановить можно только host-сбросом умолчаний (D5). В D4 гость обновляет `dbx` через `KEK` — это не противоречит D4, но риск остаётся. D3–D5 этим аудитом не меняются; вопрос — нужна ли политика (запрет/предупреждение/документированный сброс) — за владельцем.

## 11. Что не входит в аудит

- Запуск Windows или любых файлов из ISO; извлечение файлов ISO на диск; хранение ISO и его файлов в Git.
- Копирование кода, таблиц или строк из бинарей Windows.
- Требования этапа 8: установка на диск, запись `Boot####` установщиком, BitLocker, Windows Hello, attestation, WDDM, Modern Standby, S3/S4.
- VBS, HVCI, Credential Guard, System Guard/DRTM, Hyper-V, WSL2 (M1, M9; [GUEST_REQUIREMENTS.md](GUEST_REQUIREMENTS.md)).
- Сеть (SNP, PXE, HTTP boot; решение D1) и модель NIC (S11-32).
- Сертификация UEFI/TPM/WHCP и требования к OEM-прошивкам (подпись прошивки, HSTI, защита от отката).

## Чеклист для S7-32

Каждый пункт S7-32 отмечает «наблюдалось / не наблюдалось» с ссылкой на журнал: A6, A7, A11, U-03, U-04, U-06, U-07, U-10, U-13, U-16, U-17, U-21, U-22, U-23, C-01, C-02, C-06, C-08, V-01, V-04, V-09…V-11, T-07, K-01…K-10, D-05, D-06, R-02, R-03, CPU-01, CPU-06, TM-01, I-03. Статусы в этом документе после S7-32 обновляются только по журналу, со ссылкой на коммит и отчёт.
