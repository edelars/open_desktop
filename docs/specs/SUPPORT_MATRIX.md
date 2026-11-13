# Начальная матрица поддержки

**Версия:** 0.1. **Дата:** 22 октября 2026 года. **Статус:** S0-02 выполнен для выбора и идентификации начальной матрицы. Таблица не заявляет подтверждённую совместимость продукта.

Матрица задаёт кандидатов для этапа 0, а не подтверждённую совместимость продукта. Версии и модели со статусом «предложено» требуют доступного образа/устройства и воспроизводимой проверки до принятия как поддерживаемой конфигурации.

## Платформы и испытательный парк

| Объект | Начальный выбор | Идентификатор / доступность | Статус |
|---|---|---|---|
| Минимальный хост macOS | macOS Tahoe 26.7.1 | Локальный хост: build 25G241 | Предложено как минимальная версия первого цикла; продуктовая совместимость ещё не проверялась |
| Основной хост macOS | macOS Tahoe 26.7.1 | build 25G241; локально обнаружен MacBook Pro, Apple M3 Max, 14 CPU-ядер, 36 GB RAM, host page size 16 KiB | Ad-hoc Hardened Runtime JIT smoke с `allow-jit` вне debugger подтверждён; Developer ID/notarization и packaged delivery не проверены |
| Контрольная версия macOS | macOS Golden Gate 27.0.1 | Выпущена 28 сентября 2026 года; build и машина для проверки не зафиксированы | Предложено; контрольный прогон не выполнен |
| Контрольный Apple Silicon Mac | MacBook Pro с базовым Apple M1 Pro, 16 GB RAM | Конфигурация CPU, модель/год Mac, host page size, версия/build macOS и toolchain ещё не зафиксированы | SoC и RAM указаны владельцем; инвентаризация и контрольный прогон отложены до первого успешного запуска Windows VM на основном M3 Max |
| Linux для раннего x86-64 boot-профиля | Ubuntu Server 26.04.1 LTS, amd64; candidate kernel package linux-image-generic-hwe-26.04 version 7.0.0-38.38 | Официальный ubuntu-26.04.1-live-server-amd64.iso; SHA-256 cc8a95cde20f6ced61a322420de00f10cc3c90ced545daa46cb9c1a117f1d927 по официальному SHA256SUMS. Kernel package version сверена с [Ubuntu packages](https://packages.ubuntu.com/resolute/amd64/linux-image-generic-hwe-26.04) | Candidate baseline only; ISO/package provenance не подтверждена. Отдельный S5-07 C7 probe с kernel/initramfs hashes ниже завершился failed до /init; этот запуск не повышает Linux support. Фактические package/config hashes, accepted boot path и userspace evidence остаются открытыми |
| Коммерческий гость | Windows 11 Pro, x64, версия 26H2 | В `iso/Windows11_Client_x64_en-us_26300_9457.iso`: bootable ISO 9660, label `CCCOMA_X64FRE_EN-US_DV9`, 9,047,330,816 bytes, локальный SHA-256 `bd4307df32bc8af33b39ccecb1174aeb345386630f89a2b86c7a4e36b55ea650`. В `sources/install.wim`, индекс 6: Windows 11 Pro, x86_64, en-US, version 10.0, build 26300.9457. На 22 октября 2026 года для 26H2 опубликован cumulative update KB5127648, build 26300.9711; фактический установленный build после запуска VM нужно записать отдельно | Версия и base build подтверждены metadata WIM; Microsoft release information подтверждает update 26300.9711 от 13 октября 2026 года. Владелец сообщил, что ISO получен из официального канала Microsoft (точные URL и дата загрузки не сохранены); inbox INF просмотрены статически |
| Независимая x86-проверка | Mac mini (Late 2012), Intel x86-64, 16 GB RAM, Ubuntu | Точная модель CPU, версия/build Ubuntu, CPU stepping/microcode и toolchain ещё не зафиксированы | Модель, RAM и ОС семейства Ubuntu указаны владельцем; инвентаризация, runner и oracle-прогон относятся к перенесённому S0-12/E03 следующего этапа |

macOS 26.7.1/build 25G241 и сведения об основном Mac сняты с локального хоста 22 октября 2026 года. Владелец указал базовый M1 Pro с 16 GB RAM и Mac mini (Late 2012) с 16 GB RAM и Ubuntu. По выбранному порядку M1 Pro инвентаризируется после контрольного Windows запуска в стороннем гипервизоре на M3 Max; Mac mini и физический x86 oracle вынесены за этап вместе с S0-12/E03. Это порядок работ, а не подтверждение совместимости на дополнительных машинах.

## Failed Linux probe (not support evidence)

| Evidence без support acceptance | Run/input fingerprint | Статус |
|---|---|---|
| Stage 5 S5-07 live C7 nonce probe, smoke_not_acceptance: cpuid_divergence на leaf 0x1, retired=1,137,476,410; guest_started=true, но COM1 пуст, exact OD_NONCE=<nonce> marker отсутствует, /init не подтверждён. Series outcome failed; S5-07 остаётся blocked/unaccepted. | Kernel SHA-256 9f55ef7253055a9cf68f8a04323c93dead2fca617386d842fbde9ca7b178b27e; initramfs SHA-256 ee9183a4eded3c16a8c6a7a81c6d5bb79be8bb023f793f7cd61694ea2bd3e588; ACPI blob SHA-256 b1ba6078a4d9bc7a8d224980713d8242e4a708743ce4f56093b00a07c631ffaf (manifest 2bd690ae0817705b1b128675f9628dd7212fee74ab97c88da3599a1ae8e398f2); series JSON SHA-256 080108859b67a964d3c1eec8247b6d8155f01ef1e18ad7aef1d89a916661d194; run manifest d708f5f57cd71c795a10d51443a72a42ac2767f190e42ec5d16c9e30f69f3962; raw runner JSON 6bff397201eaafc0836aeb6dbddfa6577d4844cc4601c5680ed827b3d51eeb7c. | Failed probe, not a supported or accepted Linux configuration. /init, Linux userspace, disk discovery, NIC, NAT, guest agent and scenarios A–G remain unproven. See [Stage 5 report](../reports/STAGE_5_REPORT.md#s5-07-live-stage-4-c7-nonce-probe-2026-11-12). |

## Пользовательские сценарии

Это выбранные сценарии S0-02; workload definitions и собственные входы подготовлены в [`tests/fixtures/guest/stage0/`](../../tests/fixtures/guest/stage0/), но сценарии ещё не выполнены в Windows VM и потому не являются подтверждёнными benchmarks:

| Класс | Кандидат | Что фиксировать до замера | Состояние |
|---|---|---|---|
| CLI | 7-Zip 26.05 x64: архивирование и извлечение собственного фиксированного набора файлов | Дистрибутив/build/hash, команды, код завершения, `7z t`, манифест SHA-256 до/после | Собственный corpus добавлен; publisher hash точного x64 installer указан ниже; локальная проверка пакета и прогон отложены |
| Многопоточная обработка | 7-Zip 26.05 x64: архивирование того же набора с 1 и 2 потоками | Те же входы, 7z/LZMA2, 32 MiB dictionary, archive size, elapsed time и peak memory | Параметры и input corpus закреплены; результаты и допустимый memory/time budget не измерены |
| Настольное приложение | LibreOffice 26.8.1 x64: открыть, изменить и сохранить документ в Writer | Дистрибутив/build/hash, вход, sentinel и revision line в сохранённом ODT, elapsed time и peak memory | Собственный текстовый fixture и операция изменения закреплены; publisher hash указан ниже; ODT output validation и resource budgets ещё не проверены |
| Гостевой JIT | Node.js 24.22.1 x64 LTS: исполнить фиксированный JavaScript workload на V8 | Runtime/build/hash, seed, точный JSON result, cold process startup и workload duration | Workload из 5 млн integer iterations, seed и expected JSON закреплены и проверены host Node; publisher hash указан ниже; guest-run остаётся |
| Графика | Blender 5.2.2 LTS x64: отрендерить фиксированную сцену на CPU | Дистрибутив/build/hash, сцена, CPU engine, 640×360/16 samples, render hash, elapsed time и peak memory | Собственный сценогенератор без внешних assets закреплён; publisher hash указан ниже; прогон и cross-host render tolerance ещё не проверены |

Выбор набора приложений подтверждён владельцем; версии указаны по официальным страницам релизов ниже. Для точных x64 installer artifacts зафиксированы publisher reference SHA-256. При получении пакетов нужно сверить локальные байты с reference hash и записать фактические build/signature; само наличие reference hash не подтверждает скачанный файл или установленную версию. Валидация output на госте, cross-host tolerances и целевые временные/памятные бюджеты остаются открытыми. Сценарии ещё не выполнены, численные показатели не являются обещаниями производительности.

| Приложение | Точный Windows x64 artifact | Publisher reference SHA-256 |
|---|---|---|
| 7-Zip 26.05 | `7z2605-x64.exe` | `fd99e86f57d8f429ef5ed045bf98315af962e2309b0f1ba87a147a44c71647d0` из [официального release API](https://api.github.com/repos/ip7z/7zip/releases/tags/26.05) |
| LibreOffice 26.8.1 | `LibreOffice_26.8.1_Win_x86-64.msi` | `c6298b10cfa1748bcbedad94c62ce771e240e5021bfd6d7a967cd571b04b5312` из [официальной mirror list](https://download.documentfoundation.org/libreoffice/stable/26.8.1/win/x86_64/LibreOffice_26.8.1_Win_x86-64.msi.mirrorlist) |
| Node.js 24.22.1 | `node-v24.22.1-x64.msi` | `3bda4baf1ee709b33cf920dd633106ddb23951e322bff9c03ec9f64ab8981b01` из [официального SHASUMS256.txt](https://nodejs.org/dist/v24.22.1/SHASUMS256.txt) |
| Blender 5.2.2 | `blender-5.2.2-windows-x64.msi` | `8dac2751ebe8e3867327eec52f1ef9b344c0a27499bb6221a301b4b35f2828d1` из [официального SHA-256 manifest](https://download.blender.org/release/Blender5.2/blender-5.2.2.sha256) |

## Первичные источники

- Apple, [security releases](https://support.apple.com/en-us/100100): macOS Golden Gate 27 и обновления Tahoe; опубликованы версии 27.0.1 и 26.7.1 на дату документа.
- Microsoft, [Windows 11 release information](https://learn.microsoft.com/en-us/windows/release-health/windows11-release-information) and [KB5127648](https://support.microsoft.com/help/5127648): Windows 11 26H2 October 2026 update, build 26300.9711, published 13 October 2026. ISO media remains at base build 26300.9457; installed VM build must be recorded after applying updates.
- Ubuntu, [26.04.1 LTS release notes](https://documentation.ubuntu.com/release-notes/26.04/) и [официальные образы](https://releases.ubuntu.com/26.04.1/).
- Ubuntu, [SHA256SUMS для 26.04.1](https://releases.ubuntu.com/26.04.1/SHA256SUMS): SHA-256 серверного amd64 ISO, указанный в таблице.
- 7-Zip, [официальный сайт](https://www.7-zip.org/): версия 26.05 (21 октября 2026 года).
- The Document Foundation, [LibreOffice 26.8.1](https://blog.documentfoundation.org/blog/2026/10/08/lo-26-8-1-available-at-last/).
- Node.js, [официальный релиз 24.22.1](https://nodejs.org/en/blog/release/v24.22.1/).
- Blender, [релизы ветки 5.2 LTS](https://www.blender.org/releases/5-2/): версия 5.2.2.
