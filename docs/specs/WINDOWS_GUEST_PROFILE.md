# Профиль поддерживаемой конфигурации Windows-гостя

**Дата:** 2026-11-20. **Задача:** [S8-05](../plans/STAGE_8.md#s8-05-описать-профиль-поддерживаемой-конфигурации-windows-и-безопасности). **Статус:** принято после архитектурного ревью (2026-11-21); решение по шифрованию устройства записано в разделе 6. Документ описывает конфигурацию, которую этап 8 называет поддерживаемой **после** приёмки соответствующих gates. На дату документа Windows через нашу машину не запускалась (S7-32 не начата), поэтому ни одно утверждение ниже не означает, что конфигурация работает. Обновляют документ S8-50 (build после обновления), S8-70 (подпись) и S8-91 (итог).

Утверждения о поведении Windows имеют источник Microsoft или пометку «наблюдать на S8-NN». Документ не обещает больше, чем приняли этапы 7 и 10.

## Источники

Дата доступа — 2026-11-20; страницы получены HTTP-запросом (статус 200). Обозначения M1, M2, M9, M17 — как в [аудите S7-30](WINDOWS_PLATFORM_REQUIREMENTS.md#источники).

| ID | Документ | Дата статьи (`ms.date`) |
|---|---|---|
| M1 | [Windows 11 requirements](https://learn.microsoft.com/en-us/windows/whats-new/windows-11-requirements) | 2026-07-14 |
| BL | [BitLocker overview](https://learn.microsoft.com/en-us/windows/security/operating-system-security/data-protection/bitlocker/) — разделы «Device encryption», «Disable device encryption» | 2026-11-03 |
| OEMBL | [BitLocker drive encryption in Windows 11 for OEMs](https://learn.microsoft.com/en-us/windows-hardware/design/device-experiences/oem-bitlocker) | 2023-04-26 (обновлено 2025-08-12) |
| VBS | [Enable memory integrity](https://learn.microsoft.com/en-us/windows/security/hardware-security/enable-virtualization-based-protection-of-code-integrity) — раздел «Validate enabled VBS and memory integrity features» | 2026-10-21 |
| HVREQ | [Hyper-V host hardware requirements](https://learn.microsoft.com/en-us/windows-server/virtualization/hyper-v/host-hardware-requirements) | 2025-06-26 |
| PK | [Microsoft-Windows-Setup \ UserData \ ProductKey \ Key](https://learn.microsoft.com/en-us/windows-hardware/customize/desktop/unattend/microsoft-windows-setup-userdata-productkey-key) | 2017-05-02 |
| MD | [Microsoft-Windows-Setup \ ImageInstall \ OSImage \ InstallFrom \ MetaData](https://learn.microsoft.com/en-us/windows-hardware/customize/desktop/unattend/microsoft-windows-setup-imageinstall-osimage-installfrom-metadata) | 2017-05-02 |
| PDE | [PreventDeviceEncryption (unattend)](https://learn.microsoft.com/en-us/windows-hardware/customize/desktop/unattend/microsoft-windows-securestartup-filterdriver-preventdeviceencryption) | 2017-05-02; страница сама пишет «These settings only apply to Windows 8» |

## 1. Гость

| Параметр | Значение | Основание |
|---|---|---|
| ОС и редакция | Windows 11 Pro, x64, en-US | [SUPPORT_MATRIX.md](SUPPORT_MATRIX.md#платформы-и-испытательный-парк), [GUEST_REQUIREMENTS.md](GUEST_REQUIREMENTS.md) |
| Носитель | `Windows11_Client_x64_en-us_26300_9457.iso`, 9 047 330 816 байт, SHA-256 `bd4307df32bc8af33b39ccecb1174aeb345386630f89a2b86c7a4e36b55ea650`; вне Git. Подключается как сконвертированный GPT/FAT32 raw-диск ([ADR-0012](../decisions/ADR-0012-install-media-transport.md)) | SUPPORT_MATRIX, аудит S7-30 |
| Образ | `sources/install.wim` индекс 6 (в сконвертированном носителе — набор `install.swm`, `install2.swm`…) | SUPPORT_MATRIX; ADR-0012 |
| Build | База — 26300.9457 (`install.wim`); WinPE/Setup — 26100.9457 (`boot.wim`). Целевой build после накопительного обновления — 26300.9711 (KB5127648); фактический build записывает S8-50 | SUPPORT_MATRIX (база 26300.9457 и целевой 26300.9711); WinPE 26100.9457 — аудит S7-30, строка A10 ([WINDOWS_PLATFORM_REQUIREMENTS.md](WINDOWS_PLATFORM_REQUIREMENTS.md#1-носитель-и-цепочка-загрузки-наблюдения-в-iso), со ссылкой на GUEST_REQUIREMENTS.md) |
| CPU, видимый гостю | Windows-профиль `od-x86-64-v2-windows-v0.1` по [ADR-0013](../decisions/ADR-0013-windows-cpu-profile.md); идентичность CPU — вариант B (N5105), решение 2026-11-21 | ADR-0013 |

## 2. Ресурсы

| Ресурс | Значение этапа 8 | Основание и границы |
|---|---|---|
| vCPU | 2 | M1, раздел «Virtual machine support»: «Two or more virtual processors». Передачу двух vCPU даёт S6-51; верхнюю границу числа vCPU определяет S6-38 — до неё больше 2 не поддерживается |
| RAM | 4 GiB | M1: «Memory: 4 GB or greater». Больший объём — допустим картой платы ([FIRMWARE_BOOT.md](FIRMWARE_BOOT.md)), но не проверялся |
| Системный диск | 64 GiB, файловый raw-диск (sparse) на отдельном NVMe-контроллере | M1: «Storage: 64 GB or greater disk space». Файловый backend — S8-21; контроллер — S8-20 |
| Разметка | GPT: ESP, MSR, раздел Windows (и раздел восстановления, если его создаёт Setup) — разметка Setup по умолчанию или явная в ответ-файле, выбор записывает S8-22 | Раздел ОС на GPT и отдельный ESP — аудит D-02 (M3). Фактическую разметку наблюдать на S8-24 (S8-12) |
| Носитель установки | отдельный NVMe-контроллер, только чтение | S8-20, S8-21; ADR-0012 |
| Диск полезной нагрузки | отдельный NVMe-контроллер, GPT/FAT32 | S8-27; только для приёмочных прогонов, не часть пользовательской конфигурации |

## 3. Устройства платы, которые видит гость

| Устройство | Контракт | Статус на 2026-11-20 |
|---|---|---|
| CPU | [CPU_CONTRACT.md](CPU_CONTRACT.md), [ADR-0013](../decisions/ADR-0013-windows-cpu-profile.md) | Windows-профиль не реализован (S2-51…S2-58) |
| Несколько vCPU, LAPIC, IOAPIC | [SMP_CONTRACT.md](SMP_CONTRACT.md), [MACHINE_CONTRACT.md](MACHINE_CONTRACT.md) | Передача двух vCPU (S6-51) не принята |
| Карта памяти, handoff | [FIRMWARE_BOOT.md](FIRMWARE_BOOT.md), [VIRTUAL_BOARD.md](VIRTUAL_BOARD.md) | Принято в S7-02/S7-03 |
| UEFI | [UEFI_PROFILE.md](UEFI_PROFILE.md) | Путь до экрана Setup (S7-32) не начат |
| ACPI, SMBIOS | [ACPI_SMBIOS_TABLES.md](ACPI_SMBIOS_TABLES.md) | S7-13 принят |
| UEFI variables | [NVRAM_DEVICE.md](NVRAM_DEVICE.md) | Устройство есть; участник состояния VM — S9-26, не принят |
| NVMe (носитель, системный диск, диск полезной нагрузки) | [NVME_DEVICE.md](NVME_DEVICE.md) | Один контроллер принят (S7-16); три — S8-20, не начата |
| Экран (GOP framebuffer) | [DISPLAY_DEVICE.md](DISPLAY_DEVICE.md) | Только framebuffer GOP; собственного WDDM нет (S10-22) |
| TPM 2.0 (CRB) | [VTPM.md](VTPM.md#crb), [ADR-0009](../decisions/ADR-0009-vtpm-placement-and-crypto.md) | CRB принят (S7-22); фаза B — S7-23, не принята |
| Клавиатура и указатель (xHCI + USB HID) | [XHCI_DEVICE.md](XHCI_DEVICE.md) | Контракт принят, модель реализована в main (S8-42, S8-43: `540a1b8`, `c745e24`) |
| COM1 | [VIRTUAL_BOARD.md](VIRTUAL_BOARD.md) «Диагностика», [MACHINE_CONTRACT.md](MACHINE_CONTRACT.md#bounded-com1-polling-uart) | Только polling; IRQ4 для `serial.sys` — S4-24, не принят. Для гостя Windows COM1 не обещается |
| Сеть (82540EM-совместимая) | план этапа 11, S11-32 | Не реализована; до S8-40 гость без сети |

## 4. Secure Boot

Поддерживаемая конфигурация — **UserMode с `SecureBoot = 1`** и штатными ключами по [ADR-0011](../decisions/ADR-0011-secure-boot-keys.md): `db` с Microsoft Windows Production PCA 2011, Windows UEFI CA 2023, Microsoft Corporation UEFI CA 2011, Microsoft UEFI CA 2023; `KEK` Microsoft 2011 и 2023; начальный `dbx` — Microsoft Secure Boot Objects v1.7.2; PK генерирует host. S7-24 принят 2026-11-20 ([STAGE_7.md](../plans/STAGE_7.md#s7-24-реализовать-secure-boot)). Все приёмочные прогоны — только в этой конфигурации (план этапа 8, §4.2); прогон с выключенным Secure Boot — обход, gate по нему не принимается.

Известный риск, решение по которому за владельцем: если Windows в госте применит отзыв PCA 2011 (KB5025885), выбранный ISO, подписанный только PCA 2011, в этой VM перестанет загружаться до host-сброса ключей ([аудит, открытый вопрос 13](WINDOWS_PLATFORM_REQUIREMENTS.md#10-открытые-вопросы-и-не-сверено)).

## 5. TPM 2.0 и его границы

- Устройство — собственный vTPM 2.0 через CRB по адресу `0xFED4_0000`, только locality 0 ([ADR-0009](../decisions/ADR-0009-vtpm-placement-and-crypto.md), [VTPM.md](VTPM.md#crb)). Целевой профиль — PC Client PTP 1.07; соответствие и сертификация TCG не заявляются ([VTPM.md](VTPM.md#состояние-и-границы)).
- **Не реализовано до приёмки S7-23:** полный обязательный набор команд фазы B и совместимость с Windows TBS. Какие команды посылает `tpm.sys`/TBS при проверке Setup — источник не найден, **наблюдать на S8-24** (журнал CRB). Приёмочный прогон S8-24 ждёт принятого S7-23.
- **EK certificate:** шаблоны L-1/L-2 и хранение DER подтверждены с тестовым издателем; производственный издатель и доверие Windows к нему не решены ([VTPM.md](VTPM.md#endorsement-key-s7-23), [ISSUE-598](../KNOWN_ISSUES.md#issue-598)). Аттестация (Device Health Attestation, Intune и т. п.) не обещается.
- **Persistent state:** формат `ODTPMSTA` v8 — текущий, снимки v3–v7 только читаются ([VTPM.md](VTPM.md#host-api-и-persistence)); на этапе 8 runner передаёт его между холодными перезапусками (S8-23). Защита состояния на host, политика отката и связь со снимками — S9-27, не принят. Потеря файла TPM state означает для гостя новый TPM.
- **PPI (`_DSM`):** нужен ли для очистки TPM из `tpm.msc` — источник не найден (аудит T-11); очистка TPM из ОС не обещается.
- **Windows Hello и BitLocker** зависят от policy-сессий и `Unseal` (аудит K-09): вне поддерживаемой конфигурации этапа 8 (раздел 6).

## 6. BitLocker и автоматическое шифрование устройства

**Что говорит Microsoft.** BL: «Starting in Windows 11, version 24H2, the prerequisites of DMA and HSTI/Modern Standby are removed»; шифрование устройства инициализируется после завершения OOBE чистой установки с «clear key», эквивалентным приостановленному BitLocker, а при только локальных учётных записях устройство «remains unprotected even though the data is encrypted». OEMBL: автоматическое шифрование начинается во время OOBE, защита включается после входа с учётной записью Microsoft или Azure AD, «BitLocker automatic device encryption is not enabled with local accounts»; условия — TPM с поддержкой PCR 7 и включённый UEFI Secure Boot (требования HSTI/Modern Standby и DMA сняты с 24H2). Формулировки двух страниц о локальных учётных записях различаются («encrypted, but unprotected» против «not enabled»).

**Что это значит для нас (гипотеза до наблюдения).** Наш build — 26100/26300, то есть 24H2 и новее; конфигурация включает TPM и Secure Boot. Поэтому без настройки Windows, вероятно, начнёт шифрование системного диска после OOBE даже с локальной учётной записью ответ-файла S8-22. Выберет ли BitLocker привязку к PCR 7 при нашем `db` (OEMBL: лишние хеши CA перед финальным Windows Production CA мешают выбору PCR 7) — не сверено. **Наблюдать на S8-25.** Последствия включённого шифрования: фоновое шифрование всего диска на медленном пути исполнения (CPU-профиль не объявляет AES-NI) удлиняет первые загрузки и искажает замеры S8-25; после включения защитника TPM потеря TPM state до S9-27 приводит к экрану ключа восстановления.

**Варианты.**

| Вариант | Последствия |
|---|---|
| 1. Отключить автоматическое шифрование компонентом ответ-файла `Microsoft-Windows-SecureStartup-FilterDriver`, `PreventDeviceEncryption` = `true`, фаза `specialize` (PDE) — так делает генератор S8-22 (`tools/build/make_windows_unattend.py`, режим `--device-encryption prevent`, по умолчанию) | Замеры и повторяемость S8-25 без фонового шифрования; нет зависимости от S9-27. Это документированная настройка функции, а не обход проверки из словаря плана (не отключает Secure Boot, TPM и проверки Setup). Источник для Windows 11: страница OEMBL (ms.date 2023-04-26, updated_at 2025-08-12) документирует настройку через ответ-файл: «To disable BitLocker automatic device encryption, you can use an Unattend file and set PreventDeviceEncryption to True». Страница компонента PDE при этом пишет «These settings only apply to Windows 8», поэтому действие компонента на конкретной сборке — наблюдать на S8-25. Альтернатива, которую генератор не использует: параметр реестра `HKLM\SYSTEM\CurrentControlSet\Control\BitLocker\PreventDeviceEncryption` = `REG_DWORD 1` (BL, «Disable device encryption»), записанный до OOBE; переход на него — отдельное изменение S8-22, если S8-25 покажет, что компонент не действует |
| 2. Разрешить шифрование по умолчанию | Ближе к конфигурации пользователя по умолчанию, но требует принятых S7-23 (K-09) и S9-27, стоит времени на интерпретаторе и создаёт риск экрана восстановления при потере TPM state |
| 3. Отключить в приёмочном профиле, а BitLocker проверить отдельным сценарием | Отдельный сценарий (ручное включение BitLocker с TPM-защитником, перезагрузка, восстановление) — после S7-23 и S9-27, вне этапа 8 |

**Решение S8-05 (принято архитектурным ревью, Claude Opus / `high`, 2026-11-21): вариант 1.** В приёмочном профиле этапа 8 шифрование устройства отключается компонентом ответ-файла `Microsoft-Windows-SecureStartup-FilterDriver`, `PreventDeviceEncryption` = `true`, фаза `specialize`; генератор S8-22 по умолчанию выпускает его (`--device-encryption prevent`), режим `--device-encryption allow` компонент не выпускает (вариант 2, не часть приёмочного профиля). BitLocker и шифрование устройства, а также VBS, HVCI, Credential Guard, Hyper-V и WSL2 — вне поддерживаемой конфигурации этапа 8 (вариант 3 — кандидат на будущий этап).

**Лицензионная оговорка.** OEMBL предупреждает: «Disabling encryption outside of this scenario ... violates the Windows 11 licensing requirements». Толкование: запрет относится к OEM при предустановке образа на устройства; приёмочный профиль — тестовая установка владельцем проекта, не предустановка. Отключение шифрования по умолчанию для пользователей продукта этим решением не принимается — это вопрос владельцу. S8-25 проверяет, что системный том не зашифрован, штатной командой состояния BitLocker (источник команды записывает S8-25).

## 7. Функции виртуализации и изоляции

**Не поддерживается:** VBS, HVCI (Memory integrity), Credential Guard, System Guard, Hyper-V, WSL2, Windows Sandbox, nested virtualization. Их включение требует отдельного решения с полным requirement-to-test (план этапа 8, §1).

**Как Windows узнаёт об отсутствии аппаратной виртуализации.** Hyper-V и функции на его основе требуют SLAT, «VM Monitor Mode extensions» (Intel VT или AMD-V) и DEP (HVREQ; для Client Hyper-V — SLAT, M1). CPU-профиль Windows не объявляет VMX (`CPUID.1` ECX) и SVM (`CPUID.80000001` ECX) и не объявляет бит гипервизора ([ADR-0013](../decisions/ADR-0013-windows-cpu-profile.md), решение 3; номера битов — по SDM, не сверены). Memory integrity «is enabled by default on many new Windows 11 devices that meet hardware and compatibility requirements» (VBS) — наша конфигурация этим требованиям не отвечает.

**Как проверить на госте (S8-25, S8-63).**

- `Get-CimInstance -ClassName Win32_DeviceGuard -Namespace root\Microsoft\Windows\DeviceGuard` из PowerShell с повышенными правами — поля `VirtualizationBasedSecurityStatus`, `SecurityServicesRunning`, `AvailableSecurityProperties` (VBS, раздел «Use Win32_DeviceGuard WMI class»). Ожидание — VBS не включена и не работает; фактические значения записываются в отчёт.
- `msinfo32.exe` — сведения VBS в конце раздела «System Summary» (VBS).
- `systeminfo.exe` — раздел требований Hyper-V (HVREQ).

## 8. Лицензия и активация

- Установка выполняется **без ключа продукта** или с ключом владельца проекта. Ключ — секрет: в Git, отчёт и журналы не попадает; генератор ответ-файла (`tools/build/make_windows_unattend.py`) ключ не выпускает: опции нет, в шаблон он не входит. Если ключ понадобится — отдельное изменение S8-22: чтение из ignored-файла в `work/stage8/` по образцу `--password-file`.
- Редакция выбирается индексом образа: `MetaData` с ключом `/IMAGE/INDEX` и значением 6 (MD). PK: «Key specifies the 25-character product key that determines which edition of Windows to install». Нужен ли ключ, чтобы автоматическая установка с многоредакционного носителя выбрала Pro только по индексу, — **источник не найден**; наблюдать на S8-22/S8-24 (`setuperr.log`). Если ключ нужен — это вопрос доступа к владельцу (ключ владельца или его решение об использовании опубликованного Microsoft ключа установки; у такого выбора лицензионное последствие, архитектурный агент его не принимает).
- Активация Windows вне scope этапа: отчёт записывает только статус активации (не активирована/активирована) на момент прогона.

## 9. Что не обещаем

- Соответствие требованию графики Windows 11 «Compatible with DirectX 12 or later, with a WDDM 2.0 driver» (M1): у гостя только framebuffer GOP; собственный WDDM — этап 10 (S10-22 — только display-only), 3D — позже.
- Сертификацию Windows Hardware Compatibility Program/HLK и поддержку конфигурации компанией Microsoft. M1 требует, чтобы процессор VM-host соответствовал требованиям Windows 11, с оговоркой «There may be some instances where this requirement for the VM host doesn't apply»; применимость к эмулятору x64 на Apple silicon — источник не найден.
- Производительность: скорость установки, загрузки и приложений до замеров S8-04, S8-25 и S8-46…S8-49 не заявляется.
- Modern Standby, S3 (сон), S4 (гибернация) и сохранность runtime-памяти при S4 (аудит U-09).
- BitLocker, шифрование устройства, Windows Hello, аттестацию, очистку TPM из ОС (разделы 5, 6).
- VBS, HVCI, Credential Guard, System Guard, Hyper-V, WSL2, Windows Sandbox, nested virtualization (раздел 7).
- COM1 для гостя Windows до принятого S4-24; сеть до S8-40; звук, USB-устройства кроме клавиатуры и указателя, USB passthrough.
- Больше 2 vCPU до S6-38; больше 4 GiB RAM и другие размеры диска — не проверялись.
- Обновления Windows сверх выбранной ветки и build (S8-50, S8-51); ARM64-версии Windows.
