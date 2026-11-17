# Этап 7. Отчёт: собственные UEFI, ACPI и платформа для Windows

Статус отчёта: **промежуточный. Этап 7 НЕ принят.** Документ фиксирует только то, что подтверждено коммитами и проверками на ветке `feat/s9-stage` (исходная точка — коммит `bacffa0`, план этапа 7). Правило статусов: задача, выполненная исполнителем, — `in_progress`, пока ревью не примет её и не отметит чекбокс в плане (`STAGE_7.md` §0); S7-00, S7-01, S7-02, S7-03, S7-12, S7-20, S7-21, S7-23 (только срез A) и S7-30 сейчас `in_progress`, остальные задачи S7-NN не начаты. Наличие плана, этого отчёта или записей в карте проекта не закрывает ни одну задачу.

Связанные документы: [план этапа 7](../plans/STAGE_7.md), [этап 7 дорожной карты](../ROADMAP.md#этап-7-собственные-uefi-acpi-и-платформа-для-windows), [виртуальная плата](../specs/VIRTUAL_BOARD.md), [состояние VM](../specs/VM_STATE.md), [реестр проблем](../KNOWN_ISSUES.md), [отчёт этапа 9](STAGE_9_REPORT.md), [отчёт этапа 11](STAGE_11_REPORT.md).

## 1. Сводка

Всего задач S7-00…S7-41 по плану: 38 (S7-00…S7-35, S7-40, S7-41).

| Статус | Количество | Задачи |
|---|---|---|
| Принято | 0 | — |
| `in_progress` | 10 | S7-00 (выполнено, коммит `c1ae3dd`, ожидает принятия ревью); S7-01 (выполнено, коммит `426c98a`, ревью пройдено); S7-02 (выполнено, коммит `68cca38`, ревью пройдено); S7-03 (выполнено исполнителем, ожидает ревью, коммита нет); S7-12 (выполнено исполнителем, ожидает ревью, коммита нет); S7-16 (срезы A и B выполнены исполнителем, ожидают ревью, коммита нет; PCI-привязка — S4-30, прерывания не реализованы); S7-20 (выполнено архитектором, ревью пройдено, коммита нет); S7-21 (выполнено исполнителем, ожидает ревью, коммита нет); S7-23 (срез A — коммит `435d7f6`, ожидает ревью; срезы B, C1 и C2 выполнены исполнителем, ожидают ревью, коммита нет; S7-23 не принимается целиком); S7-30 (выполнено исполнителем, ревью пройдено, замечания исправлены, коммита нет) |
| `not_started` | 28 | остальные задачи, см. таблицу ниже |

Чекбоксы `[ ]` в плане не отмечены: по `STAGE_7.md` §0 отмечаются только после ревью.

## 2. Таблица задач

| Задача | Статус | Коммит | Ревью | Комментарий |
|---|---|---|---|---|
| S7-00 | in_progress | `c1ae3dd` | — | Scope, отчёт этапа, обновление платы и карты, запись переноса пунктов roadmap (выполнено, коммит `c1ae3dd`, ожидает принятия ревью) |
| S7-01 | in_progress | `426c98a` | — | Сборка прошивки и EFI-приложений, host-side тесты, методика (выполнено исполнителем, ревью пройдено, замечания исправлены, коммита на момент записи нет; каркас основного образа и доказательство сборки PE32+ probe; тестовые EFI-приложения — S7-06, боевой RT-образ и размещение в FV — S7-11) |
| S7-02 | in_progress | `68cca38` | — | Контракт входа: окна, handoff, manifest платы, ABI/состояние вызова, таймеры, ROM, RT relocation (выполнено, ожидает ревью, коммита нет) |
| S7-03 | in_progress | — | — | ROM backing/shadow, загрузчик образа, handoff block, профиль платы и runner (выполнено исполнителем, ожидает ревью, коммита нет; blocker снят реализацией решения архитектора — допуск `B0+r ib` и `66 B8+r iw` в `PagedCpl0ProbeV1`; образ `--smoke` в машине: выход 0, `halted`, `retired = 4`, COM1 ровно `X`, см. раздел 5 и раздел S7-03) |
| S7-04 | in_progress | `e1ef0f1`, `6553d56`, `d5d6e22`, `b219d0e`, `7bb0108`, `dd57771`, `717fed3`, `e32436b`, `9439e93`, `ebb9ef0`, `571dcc5`, `63d55e1`, `9aff40f`, `743f381`, `9d336c1`, `346948b`, `976c7e2`, `315d5d8`, `331c81b`, `d2aedac`, `83a3b1e`, `8e1b25d`, `fd422dd`, `a9c5566`, `01b4a72`, `a33dd5e`, `832276d`, `1529954`, `46e1792`, `1016c25`, `e16e880`, `d584420`, `652c3a8`, `7bf7a91`, `5afb70b` | previous slices reviewed and committed; ISSUE-178 and ISSUE-179 committed; ISSUE-180 and ISSUE-181 committed; ISSUE-182 committed; ISSUE-183 and ISSUE-184 committed; ISSUE-185 through ISSUE-203 committed; ISSUE-204 committed; ISSUE-205 and ISSUE-206 committed; ISSUE-207 committed `4cf38f4`; ISSUE-208 committed `7c9258c`; ISSUE-209 committed `389f5a7`; ISSUE-210 committed `e16e880`; ISSUE-211 committed `d584420`; ISSUE-212 committed `652c3a8`; ISSUE-213 committed `7bf7a91`; ISSUE-214 committed `5afb70b`; ISSUE-215 committed `466fb9c`; ISSUE-216 reviewed/committed `edc52c9`; ISSUE-217 reviewed/committed `deead7c`; ISSUE-218 reviewed/committed `1020422`; ISSUE-219 committed `4a2ea9a`; ISSUE-220 independently reviewed and committed `1070a35`; ISSUE-221 independently reviewed and committed `3b62438` | handoff parser connected; base profile admits exact `8B 4E 0C`/`8B 4E 10` tracked dword reads, exact `81 F9 A0 00 00 00` (`CMP ECX,0xa0`), `89 CD` (`MOV EBP,ECX`), `8D B9 60 FF FF FF` (`LEA EDI,[RCX-0xA0]`), `81 FF 61 FF 00 00` (`CMP EDI,0xff61`), `40 0F 92 C7` (`SETB DIL`), `48 39 EA` (`CMP RDX,RBP`), and `41 0F 93 C0` (`SETAE R8B`), plus exact `41 20 F8` (`AND R8B,DIL`) and `40 F6 C5 07` (`TEST BPL,7`), plus `40 0F 94 C7` (`SETZ DIL`) and `41 84 F8` (`TEST R8B,DIL`), plus `0F 84 B4 00 00 00` (`JZ rel32 +0xB4`) and `0F B7 CB` (`MOVZX ECX,BX`) plus exact `49 B8 FC FF FF FF FF FF FF 7F` (`MOV R8,0x7ffffffffffffffc`) and `8B 7E 14` (`MOV EDI,dword ptr [RSI+0x14]`) and `41 B9 FF FF FF FF` (`MOV R9D,0xffffffff`) plus exact `45 31 D2` (`XOR R10D,R10D`) and `45 31 DB` (`XOR R11D,R11D`), `66 90` (NOP), `46 0F B6 34 1E` (`MOVZX R14D,byte ptr [RSI+R11]`), exact `46 0F B6 7C 1E 01` (`MOVZX R15D,byte ptr [RSI+R11+1]`), `4D 89 DF` (`MOV R15,R11`), `4D 21 C7` (`AND R15,R8`), `45 89 CC` (`MOV R12D,R9D`), `45 89 E1` (`MOV R9D,R12D`), `41 C1 E9 08` (`SHR R9D,8`), `45 31 FE` (`XOR R14D,R15D`), exact SIB `46 33 0C B5 80 26 20 00` (`XOR R9D,dword ptr [R14*4+0x202680]`), exact `49 83 C3 02` (`ADD R11,2`), exact `4C 39 DD` (`CMP RBP,R11`), plus `41 C1 EC 08` (`SHR R12D,8`), plus `49 83 FF 14` (`CMP R15,0x14`) and `45 0F 44 F2` (`CMOVZ R14D,R10D`) and `45 0F 44 FA` (`CMOVZ R15D,R10D`) and `45 0F B6 C9` (`MOVZX R9D,R9B`) and `45 31 F1` (`XOR R9D,R14D`); exact JNZ `75 AE` and `75 4A`, exact `NOT R9D` (`41 F7 D1`), `CMP EDI,R9D` (`44 39 CF`), `JNZ rel8 +0x4A` (`75 4A`) and `MOV R14D,dword ptr [RSI+0x34]` (`44 8B 76 34`) and exact `CMP EDI,0x20` (`83 FF 20`) and `44 8B 7E 30` (`MOV R15D,dword ptr [RSI+0x30]`, ISSUE-199), exact `4C 01 FF` (`ADD RDI,R15`, ISSUE-207) and exact `48 39 EF` (`CMP RDI,RBP`, ISSUE-208), exact `77 3D` (`JA rel8 +0x3d`, ISSUE-209), exact `44 8B 6E 3C` (`MOV R13D,dword ptr [RSI+0x3c]`, ISSUE-210), `44 8B 66 40` (`MOV R12D,dword ptr [RSI+0x40]`, ISSUE-211), `44 8B 46 44` (`MOV R8D,dword ptr [RSI+0x44]`, ISSUE-212), `85 C9` (`TEST ECX,ECX`, ISSUE-213), `74 35` (`JZ rel8 +0x35`, ISSUE-214) and `41 83 FC 07` (`CMP R12D,7`, ISSUE-215) are admitted; runner retired 8902 instructions and stopped at `41 F6 C5 07` (`TEST R13B,7`, ISSUE-220, RIP `0x200f23`); exact `48 89 74 24 10` (`MOV [RSP+0x10],RSI`, ISSUE-219), `48 89 54 24 30` (`MOV [RSP+0x30],RDX`, ISSUE-218), `41 83 F8 30` (`CMP R8D,0x30`, ISSUE-217) and `74 D0` (`JZ rel8 -0x30`, ISSUE-216) are admitted; ISSUE-200, ISSUE-202 through ISSUE-205 are admitted and committed, ISSUE-206 is admitted and committed; ISSUE-207 is admitted and committed; ISSUE-208 is admitted and committed; ISSUE-209 is admitted and committed; ISSUE-210 is admitted and committed; ISSUE-211 is admitted and committed; ISSUE-212 is admitted and committed; ISSUE-213 is admitted and committed; ISSUE-214 is admitted and committed; ISSUE-215 is admitted and committed `466fb9c`; ISSUE-216 is admitted and committed `edc52c9`; ISSUE-217 is admitted and committed `deead7c`; ISSUE-218 is admitted and committed `1020422`; ISSUE-219 is admitted, independently reviewed and committed `4a2ea9a`; ISSUE-220 (`41 F6 C5 07`, `TEST R13B,7`) is admitted and committed `1070a35`; ISSUE-221 (`74 59`, `JZ rel8 +0x59`) is admitted and committed `3b62438`; ISSUE-222 (`48 89 44 24 08`, `MOV [RSP+8],RAX`) is admitted and committed `31887b9`; ISSUE-223 (`41 81 FD A0 00 00 00`, `CMP R13D,0xA0`) is admitted and committed `fe92cfd`; ISSUE-224 (`72 2A`, `JB/JNAE rel8 +0x2a`) is admitted and committed `597bf68`; ISSUE-225 (`4B 8D 04 64`, `LEA RAX,[R12+R12*2]`) is admitted and committed `313369c`; ISSUE-226 (`48 C1 E0 04`, `SHL RAX,4`) is admitted and committed `f16d39c`; ISSUE-227 (`4C 01 E8`, `ADD RAX,R13`) is admitted and committed `180e6f3`; ISSUE-228 (`48 39 E8`, `CMP RAX,RBP`) is admitted and committed `530446f`; ISSUE-229 (`77 1A`, `JA rel8 +0x1a`) is admitted and independently approved; ISSUE-230 (`4C 39 EF`, `CMP RDI,R13`) is admitted narrowly and independently approved; ISSUE-231 (`76 22`, `JBE rel8 +0x22`) is admitted narrowly and independently approved; ISSUE-232 (`89 1C 24`, `MOV dword ptr [RSP],EBX`) is admitted narrowly and independently approved; ISSUE-239 (`E8 79 05 00 00`, `CALL rel32 +0x579`) is admitted narrowly with tracked qword return-RIP push and A/D/WP fault evidence; ISSUE-240 (`50`, `PUSH RAX`) is admitted narrowly with tracked qword stack write and A/D/WP fault evidence; ISSUE-241 (`4D 85 C0`, `TEST R8,R8`) is admitted narrowly with qword logical flag and AF-undefinedness coverage; ISSUE-242 (`0F 84 49 01 00 00`, `JZ rel32 +0x149`) is admitted narrowly with signed rel32, flag-preservation, taken-only canonicality and fetch-fault evidence; ISSUE-243 (`48 89 F0`, `MOV RAX,RSI`) is admitted and independently reviewed with no findings; ISSUE-244 (`48 8D 71 04`, `LEA RSI,[RCX+4]`) is admitted narrowly and independently reviewed with no findings; ISSUE-245 (`48 39 D6`, `CMP RSI,RDX`) is admitted narrowly and independently reviewed with no findings; ISSUE-246 (`0F 87 CD 02 00 00`, `JA rel32 +0x2cd`) is admitted narrowly and independently reviewed with no findings; ISSUE-247 (`4C 8D 51 08`, `LEA R10,[RCX+8]`) is admitted narrowly and independently reviewed with no findings; ISSUE-248 (`49 39 D2`, `CMP R10,RDX`) is admitted narrowly and independently reviewed with no findings; ISSUE-249 (`0F 87 D7 02 00 00`, `JA rel32 +0x2d7`) is implemented narrowly, independently reviewed with no findings; ISSUE-250 (`48 8D 71 10`, `LEA RSI,[RCX+0x10]`) is implemented narrowly independently reviewed with no findings; the runner retires 8,940 and stops at `0F 87 E4 02 00 00` (`JA rel32 +0x2e4`, ISSUE-251, RIP `0x201598`); ISSUE-238 (`4D 89 E1`, `MOV R9,R12`) is admitted and independently reviewed and approved with no findings; ISSUE-237 (`4D 89 F0`, `MOV R8,R14`) is admitted and independently reviewed and approved with no findings; ISSUE-236 (`4C 89 F9`, `MOV RCX,R15`) is admitted and independently reviewed and approved with no findings; ISSUE-235 (`48 8B 54 24 30`, `MOV RDX,qword ptr [RSP+0x30]`) is admitted narrowly and independently reviewed and approved with no findings; ISSUE-234 (`48 8B 74 24 10`, `MOV RSI,qword ptr [RSP+0x10]`) is admitted narrowly and independently reviewed and approved with no findings; ISSUE-233 (`48 8D 7C 24 18`, `LEA RDI,[RSP+0x18]`) is admitted and independently reviewed and approved with no findings |
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
| S7-16 | in_progress | — | — | Минимальный NVMe-контроллер в `od-machine` (срез A — admin — и срез B — I/O `Read`/`Write`/`Flush` на in-memory backend — выполнены исполнителем, ожидают ревью, коммита нет; PCI-привязка — S4-30, прерывания не реализованы; значения спецификации не сверены) |
| S7-17 | not_started | — | — | Драйвер NVMe прошивки: Block I/O, Disk I/O, GPT partitions |
| S7-18 | not_started | — | — | Собственная FAT и Simple File System |
| S7-19 | not_started | — | — | Boot manager: `BootOrder`/`Boot####`, `\EFI\BOOT\BOOTX64.EFI`, загрузка приложения с ESP |
| S7-20 | in_progress | — | — | ADR: размещение vTPM, криптозависимости host и `no_std` прошивки, профиль команд, CRB, состояние (выполнено архитектором, ревью пройдено, замечания исправлены, [ADR-0009](../decisions/ADR-0009-vtpm-placement-and-crypto.md); ожидает ревью, коммита нет) |
| S7-21 | in_progress | — | — | Процессор команд TPM 2.0, фаза A: startup, capability, PCR, hash, random, password session (выполнено исполнителем, ожидает ревью, коммита нет; 17 команд, 88 тестов `cargo test -p od-tpm --offline`; PCR_Allocate отказ; SHA-384 банк не реализован; список не сверенных мест — [VTPM.md](../specs/VTPM.md#источники-и-сверка)) |
| S7-22 | not_started | — | — | CRB-устройство, таблица TPM2, `EFI_TCG2_PROTOCOL`, measured boot и event log (PCR0–5) |
| S7-23 | in_progress (срез A — коммит `435d7f6`, ожидает ревью; срезы B, C1, C2 и C3 выполнены исполнителем, ожидают ревью, коммита нет; EK-шаблоны и сертификат EK ждут решения владельца; S7-23 НЕ принимается до решения по EK) | `435d7f6` (срез A) | — | TPM фаза B: иерархии, ключи RSA/ECC, сессии, NV, persistent state v1. **Срез A** (иерархии Platform/Owner/Endorsement/Null, HMAC-сессии, база NV; `hmac =0.13.0`; 133 теста) — коммит `435d7f6`, ожидает ревью; **срез B выполнен исполнителем** (объекты и ключи RSA-2048/ECC P-256: `CreatePrimary`, `Create`, `Load`, `LoadExternal`, `ReadPublic`, `Unseal`, `EvictControl`, `Sign`, `VerifySignature`; `rsa =0.10.0-rc.22`, `p256 =0.14.0`; 171 тест), ожидает ревью, коммита нет; **срез C1 выполнен исполнителем** (`RSA_Encrypt`, `RSA_Decrypt` (RSAES, OAEP, NULL), `ECDH_ZGen`, DA — `DictionaryAttackLockReset`/`DictionaryAttackParameters`, snapshot `ODTPMSTA` v2 с auth иерархий, NV и persistent-объектами; 199 тестов), ожидает ревью, коммита нет; **срез C2 выполнен исполнителем** (policy-сессии и 7 команд `Policy*`, `ContextSave/Load` для объектов и сессий, `TPM2_HMAC` с keyed-hash ключами (RFC 4231), `Quote`, `Certify` с проверкой подписи внешним Python; 232 теста), ожидает ревью, коммита нет; **срез C3 выполнен исполнителем** (NV `TPM_NT_COUNTER`/`BITS`/`EXTEND` с `NV_Increment`/`SetBits`/`Extend`, `TPM2_GetTime`, `signHandle` = `TPM_RH_NULL` у `Quote`/`Certify`/`GetTime`, шифрование параметров сессий AES-128-CFB и XOR, snapshot v3 вместо v2; 264 теста), ожидает ревью, коммита нет; **не сделано:** шаблоны и сертификат EK (ждут решения владельца), `PolicySecret` с `expiration` ≠ 0, `PolicySigned/PolicyNV/PolicyLocality/PolicyPassword` и др., `SetPrimaryPolicy`, шифрование параметров сессией без авторизуемого handle — [VTPM.md](../specs/VTPM.md#фаза-b-срез-c1-rsa_encryptdecrypt-ecdh_zgen-da-persistent-state-v2-s7-23) |
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

### S7-23. TPM 2.0, фаза B (срезы A, B, C1, C2 и C3 выполнены, S7-23 не принимается до решения по EK)

Статус `in_progress`: срез A — коммит `435d7f6`, ожидает ревью; срез B выполнен исполнителем (Claude Sonnet), ожидает ревью, коммита нет (раздел «Срез B» ниже); срез C1 выполнен исполнителем (Claude Sonnet), ожидает ревью, коммита нет (раздел «Срез C1» ниже); срез C2 выполнен исполнителем (Claude Sonnet), ожидает ревью, коммита нет (раздел «Срез C2» ниже); срез C3 выполнен исполнителем (Claude Sonnet), ожидает ревью, коммита нет (раздел «Срез C3» ниже). Задача разбита оркестратором на срезы: **A** — иерархии, HMAC-сессии, NV-база, замена самописного HMAC крейтом `hmac` (выполнен); **B** — ключи RSA/ECC и команды с объектами (выполнен; policy-сессии и шифрование параметров перенесены в срез C); **C** — разбит на **C1** (RSA/ECDH, DA, persistent state v2; выполнен) и **C2** (policy-сессии, контексты, `TPM2_HMAC`, `Quote`, `Certify`; выполнен); **C3** (NV counter/bits/extend, `GetTime`, NULL `signHandle`, шифрование параметров, snapshot v3; выполнен); за пределами C3 остаются шаблоны EK и сертификат EK (ждут решения владельца проекта). Чекбокс задачи в плане не отмечен.

**S7-23 НЕ принимается до решения по EK.** Срезы A, B, C1, C2 и C3 выполнены и ожидают ревью; остался обязательный пункт плана: шаблоны EK по TCG EK Credential Profile и сертификат EK (T-04) — ждут решения владельца проекта (редакция профиля, источник сертификата). DA-защита (lockout) реализована в срезе C1; в срезах A и B ошибка авторизации всегда была `TPM_RC_BAD_AUTH` без счётчика попыток.

Изменены (срез A): `crates/od-tpm/` (`Cargo.toml`, `README.md`, новые модули `src/{crypto,hierarchy,session,nv}.rs` и `src/tests_b.rs`; изменены `lib,codes,marshal,command,response,dispatch,startup,capability,random,pcr,hash,state,tests`), `Cargo.lock`, [VTPM.md](../specs/VTPM.md) (раздел «Фаза B, срез A», NOT VERIFIED 12–20, «Отклонения»), [DEPENDENCIES.md](../policies/DEPENDENCIES.md), [PROJECT_LAYOUT.md](../PROJECT_LAYOUT.md) (строка `crates/od-tpm/src/`), [ADR-0009](../decisions/ADR-0009-vtpm-placement-and-crypto.md) (одна фраза в «Последствиях»: feature `alloc` у `zeroize`), строка статуса S7-23 в плане и в этом отчёте.

Реализовано (13 команд, всего в таблице 30): `TPM2_HierarchyControl`, `HierarchyChangeAuth`, `Clear`, `ClearControl`; `StartAuthSession` (только `TPM_SE_HMAC`, unbound или bound к иерархии, без salt, `symmetric` NULL), `FlushContext` (сессии и sequence-объекты); `NV_DefineSpace`, `NV_UndefineSpace`, `NV_ReadPublic`, `NV_Write`, `NV_Read` и сверх списка плана `NV_WriteLock`, `NV_ReadLock`. Иерархии Platform/Owner/Endorsement/Null (seeds из DRBG фазы A, Null seed новый на каждый Reset; Lockout — только как authValue), `authValue` с `HierarchyChangeAuth` (старый перестаёт работать, хвостовые нули отбрасываются), enables `phEnable/phEnableNV/shEnable/ehEnable` по Part 3, 9.3.1 (Reset/Restart/Resume), `Clear` (новый storage seed, сброс auth и enables, удаление NV владельца, `Clock` с нуля). HMAC-сессии по Part 1, 17.6 (cpHash/rpHash, KDFa, bound-сессия, пустой HMAC, `continueSession`, смена `nonceTPM`); HMAC работает и для команд фазы A. NV: атрибуты `PPWRITE/OWNERWRITE/AUTHWRITE/PPREAD/OWNERREAD/AUTHREAD/WRITEALL/WRITEDEFINE/WRITE_STCLEAR/READ_STCLEAR/NO_DA/PLATFORMCREATE` и вычисляемые `WRITTEN/READLOCKED/WRITELOCKED`, `TPM_RC_NV_RANGE/NV_LOCKED/NV_DEFINED/NV_SPACE/NV_UNINITIALIZED/NV_AUTHORIZATION`, доступность индексов по `phEnableNV`/`shEnable`. `GetCapability`: handles (постоянные, NV, сессии), свойства сессий/NV/`PERMANENT`/`STARTUP_CLEAR`, перечень команд = таблица (тест сверяет с отдельно выписанным списком из 30 кодов).

`hmac =0.13.0` подключён (`default-features = false`, `zeroize`): версии не было в офлайн-кеше, `cargo fetch` получил её и два новых транзитивных пакета (`ctutils 0.4.8`, `cmov 0.5.4`, оба `Apache-2.0 OR MIT`); самописный HMAC в `random.rs` удалён (обязательство S7-21 выполнено), векторы RFC 4231 (случаи 1, 2, 6) и DRBG проходят с прежними значениями. `zeroize` получил feature `alloc`.

Не реализовано в срезе A (срезы B/C): все объектные команды и RSA/ECC; policy-сессии и `Policy*`; шифрование параметров и audit; salted-сессии; `ContextSave/Load`; `SetPrimaryPolicy`, `ChangePPS/EPS`; DA (`DictionaryAttack*`; ошибка авторизации всегда `TPM_RC_BAD_AUTH`, `inLockout` не бывает); NV counters/bits/extend/pin (определение такого индекса — `TPM_RC_ATTRIBUTES` по Part 3, 31.3.1) и атрибуты `POLICYREAD/POLICYWRITE/POLICY_DELETE/GLOBALLOCK/ORDERLY/CLEAR_STCLEAR`; `TPM2_HMAC` (нужен объект-ключ); proof-значения и тикеты (поэтому `validation` в `Hash` по-прежнему NULL-ticket); persistent state v1 — auth иерархий и NV-индексы **не входят в snapshot** (`restore` даёт пустые auth и пустой NV; тест фиксирует это).

Отклонения: `NV_WriteLock/ReadLock` сверх списка плана; лимиты NV (128 индексов, 16384 октета, 8500 на индекс, 512 на буфер) — выбор реализации при выполненных минимумах PTP 1.07; 64 сессии (ёмкость = `ACTIVE_SESSIONS_MAX`); `TPM_PT_*` перечисляют только реализованное. Устаревшая фраза в строке статуса S7-21 плана заменена на «заменено в срезе A S7-23».

Проверено (2026-11-07): `cargo test -p od-tpm --offline` — 133 теста проходят (было 88); для каждой новой команды — успех, 2–3 ошибки, отказ авторизации (пароль и HMAC), не загруженная сессия; независимый вектор сессионного обмена из Python (`hmac`/`hashlib`) воспроизводит nonce DRBG, ключ KDFa, HMAC команды и ответа для unbound и bound сессии. `cargo clippy -p od-tpm --offline --all-targets`, `cargo fmt -p od-tpm -- --check`, `cargo check --workspace --all-targets --offline`, `python3 tools/check_layout.py` (число ошибок не выросло), `git diff --check` — см. итоговый отчёт исполнителя.

Не проверено: **текст Library v185 недоступен** (HTTP 403), сверка с v184 (Part 1: 9.4.10.2, 11.2–11.5, 12.4.5, 14, 16.6–16.8, 17.6; Part 2: Tables 20, 25, 30, 36, 40, 42, 43, 58–70, 159, 227–235; Part 3: 9.3.1, 11.1, 24.2, 24.6–24.8, 28.4, 30.2, 31.1–31.14; PTP 1.07: Table 2, Table 12, 4.6.2). Все коды ошибок и детали, которых открытый текст не называет, перечислены в [VTPM.md](../specs/VTPM.md#не-сверено-с-текстом-not-verified), пп. 12–20, и помечены `NOT VERIFIED` в исходниках (`hierarchy.rs`, `session.rs`, `nv.rs`, `dispatch.rs`, `capability.rs`). Сборка под `x86_64-unknown-none` не проверялась (crate host-only). Независимое ревью не выполнено. Требования ADR-0009 к секретам: seeds, `authValue`, ключи сессий, данные NV и выход KDFa хранятся в стираемых при `Drop` типах (`zeroize`), HMAC сравнивается за постоянное время (`Mac::verify_slice`), пароль и `authValue` — без ранних выходов; `Debug` ни одного нового типа не печатает секрет (тест). Условие constant-time для RSA (ISSUE-064) к срезу A не относится: приватных RSA-операций нет.

#### Срез B (объекты и ключи), выполнен исполнителем, ожидает ревью, коммита нет

Изменены: `crates/od-tpm/` (`Cargo.toml`; новые модули `src/object.rs`, `src/object_cmd.rs`, `src/tests_obj.rs`; изменены `lib,codes,response,marshal,crypto,dispatch,state,startup,hierarchy,session,capability,tests,tests_b`), `Cargo.lock`, [VTPM.md](../specs/VTPM.md) (раздел «Фаза B, срез B», NOT VERIFIED 21–32, «Отклонения», «Состояние»), [DEPENDENCIES.md](../policies/DEPENDENCIES.md), [KNOWN_ISSUES.md](../KNOWN_ISSUES.md) (ISSUE-064), [PROJECT_LAYOUT.md](../PROJECT_LAYOUT.md), `crates/od-tpm/README.md`, строка статуса S7-23 в плане и в этом отчёте.

Факты: 9 команд (всего в таблице 39): `CreatePrimary`, `Create`, `Load`, `LoadExternal` (только публичная часть), `ReadPublic`, `Unseal`, `EvictControl`, `Sign`, `VerifySignature`. Объекты: RSA-2048 (RSASSA-PKCS1-v1_5/SHA-256), ECC NIST P-256 (ECDSA/SHA-256), sealed data; primary выводится детерминированно из seed иерархии и шаблона (HMAC_DRBG на KDFa; тесты: тот же TPM, второй TPM, snapshot → `restore`), child — случайный ключ DRBG. `outPrivate` защищён по формулам Part 1, 20.3–20.5. Тикеты (creation, verified) — HMAC под proof иерархии; proof выводятся из seeds. `GetCapability`: `ALGS` (RSA, KEYEDHASH, SHA256, RSASSA, ECDSA, ECC), `ECC_CURVES` (P-256), persistent handles, `TPM_PT_HR_PERSISTENT*`; перечень команд = таблица (39). `Clear` уничтожает объекты Storage и Endorsement и меняет `shProof`/`ehProof`; `HierarchyControl` сбрасывает transient-объекты выключаемой иерархии; `Startup`/`_TPM_Init` сбрасывают transient-объекты.

Зависимости: `rsa =0.10.0-rc.22` (`default-features = false`, `pkcs1v15`), `p256 =0.14.0` (`default-features = false`, `ecdsa`); оба и их 16 новых транзитивных пакетов получены `cargo fetch` (в офлайн-кеше не было); дальше `--offline` проходит. `der`, `spki`, `aes`, `cfb-mode`, `sha1`, `x509-cert` в срезе B не нужны и не добавлены. Подробности и лицензии — [DEPENDENCIES.md](../policies/DEPENDENCIES.md).

Ограничения и отклонения (подробно в VTPM.md): (1) **`outPrivate` не использует AES** — шифрование потоком KDFa вместо AES-128-CFB (при этом шаблон storage key называет AES-128-CFB); блобы не совместимы с другими TPM; RSA хранится как `p ‖ q`; формат блоба не стабилен до среза C. (2) Primary выводится собственным алгоритмом (Part 1 называет это «vendor option»), поиск простых RSA — алгоритм `rsa`/`crypto-primes` при зафиксированных версиях: смена версии может изменить RSA-primary. (3) `Hash`/`SequenceComplete` выдают только NULL-тикеты: restricted signing key не может подписать (`TPM_RC_TICKET`). (4) Policy-сессии нет: объекты с `userWithAuth` CLEAR создаются, но не авторизуются (`TPM_RC_AUTH_UNAVAILABLE`). (5) Persistent-объекты только в памяти, в snapshot не входят. (6) Приватный RSA — `rsa` без constant-time ([ISSUE-064](../KNOWN_ISSUES.md#issue-064-s7-rsa-private-ops-not-constant-time) обновлена: код, использующий `rsa`, есть; принятый риск D9 действует; риск — timing-атака на host-процесс с vTPM). (7) Все коды ответа, которых открытый текст v184 не называет (Part 4 недоступна), — `NOT VERIFIED` (VTPM.md, пп. 21–32, и комментарии в `object.rs`, `object_cmd.rs`, `marshal.rs`, `dispatch.rs`, `capability.rs`). Не сделано (срез C): `RSA_Encrypt/Decrypt`, `ECDH_ZGen`, `ContextSave/Load`, DA, persistent state v1, policy-сессии, шифрование параметров, EK, `Quote/Certify/GetTime`, `TPM2_HMAC`, `ObjectChangeAuth`.

Проверено (2026-11-07): `cargo test -p od-tpm --offline` — 171 тест проходит (было 133); независимые эталоны: RSA-2048 и ECDSA P-256 подписи OpenSSL 4.1.0 проходят `LoadExternal` → `VerifySignature`, их корректность подтверждена Python (`pow`, аффинная проверка ECDSA); подписи эталонных primary-ключей (ECC, RSA) проверены тем же кодом Python и закреплены тестом; байты `TPMT_PUBLIC` написаны вручную по Part 2. Прочие проверки (`clippy`, `fmt`, `cargo check --workspace`, `check_layout.py`, `git diff --check`) — в итоговом отчёте исполнителя. Не проверено: текст Library v185 недоступен (сверка по v184, Part 4 не использовалась); сборка под `x86_64-unknown-none` не проверялась; независимое ревью не выполнено; constant-time RSA и ECDSA не проверялись (ISSUE-064; для ECDSA — заявление поставщика).

#### Срез C1 (RSA_Encrypt/Decrypt, ECDH_ZGen, DA, persistent state v2), выполнен исполнителем, ожидает ревью, коммита нет

Изменены: `crates/od-tpm/` (`Cargo.toml` — feature `hazmat` у `rsa`; новые модули `src/asym.rs`, `src/da.rs`, `src/snapshot.rs`, `src/tests_c1.rs`; изменены `lib,codes,response,marshal,object,crypto,dispatch,state,startup,hierarchy,nv,capability,tests,tests_b,tests_obj`; `README.md`), [VTPM.md](../specs/VTPM.md) (раздел «Фаза B, срез C1», формат snapshot v2, NOT VERIFIED 35–44, «Отклонения», исправление записи «Part 1 недоступна»), [DEPENDENCIES.md](../policies/DEPENDENCIES.md), [ADR-0009](../decisions/ADR-0009-vtpm-placement-and-crypto.md) (одна фраза), [KNOWN_ISSUES.md](../KNOWN_ISSUES.md) (ISSUE-064: появилась `RSA_Decrypt`), [PROJECT_LAYOUT.md](../PROJECT_LAYOUT.md), строка статуса S7-23 в плане и в этом отчёте.

Факты: 5 команд (всего в таблице 44): `TPM2_RSA_Encrypt` (`0x174`), `TPM2_RSA_Decrypt` (`0x159`), `TPM2_ECDH_ZGen` (`0x154`), `TPM2_DictionaryAttackLockReset` (`0x139`), `TPM2_DictionaryAttackParameters` (`0x13A`). RSA: схемы NULL («сырой» RSA), RSAES-PKCS1-v1_5, RSAES-OAEP (SHA-256) по Part 3, Table 42; шаблоны ключей принимают схемы RSAES/OAEP/ECDH. ECDH: возвращается точка `(x, y)`. DA: защищены объекты без `noDA`, NV-индексы без `NO_DA`, `lockoutAuth`; неверный authValue защищённой сущности — `TPM_RC_AUTH_FAIL` и `failedTries` + 1, `TPM_RC_LOCKOUT` при `failedTries >= maxTries`, самовосстановление раз в `recoveryTime`, блокировка `lockoutAuth` на `lockoutRecovery`; значения по умолчанию 32 / 600 с / 86400 с — выбор реализации. Snapshot `ODTPMSTA` v2: seeds, `ownerAuth`/`endorsementAuth`/`lockoutAuth` и флаги, DA, NV (атрибуты, auth, данные), persistent-объекты с приватными частями; версия 1 отвергается; **snapshot — чувствительные данные** (открытый текст; защита — S9-27). Part 1 v184 скачивается и прочитана (11.7, 17.8, 20.1–20.5): формулы `outPrivate` и правила DA сверены, ранняя запись «Part 1 недоступна» исправлена; пункт (1) «Ограничений и отклонений» среза B («`outPrivate` не использует AES») устарел — после ревью среза B `outPrivate` защищается AES-128-CFB.

Зависимости: новых пакетов нет, `Cargo.lock` не менялся. `rsa =0.10.0-rc.22` — добавлен feature `hazmat` («сырой» RSA; OAEP и RSAES доступны без feature). `p256 =0.14.0` — feature `ecdh` **не включён** (ADR-0009 его называет): `ECDH_ZGen` возвращает точку, а `ecdh::diffie_hellman` даёт только `x`; умножение точки — `arithmetic` (входит в `ecdsa`).

Отклонения и ограничения: RSAES-расшифровка — implicit rejection `rsa` вместо `TPM_RC_VALUE`; DA защищает только объекты, NV-индексы и `lockoutAuth` (а не все иерархии: так в Part 1, 17.8.1); объекты платформенной иерархии и индексы `PLATFORMCREATE` из DA исключены; значения DA по умолчанию — выбор; `platformAuth` в snapshot не входит (volatile по Part 3, 9.3.1); не реализованы policy-сессии, шифрование параметров, EK, `ContextSave/Load`, `Quote/Certify/GetTime`, `TPM2_HMAC` (срез C2). Приватный RSA — `rsa` без constant-time: ISSUE-064 обновлена, `RSA_Decrypt` — ещё одно место риска (принятый риск D9 действует).

Проверено (2026-11-07): `cargo test -p od-tpm --offline` — 199 тестов проходят (было 174: минус один устаревший тест snapshot, плюс 1 unit-тест `da.rs` и 25 тестов `src/tests_c1.rs`); независимые эталоны: шифртексты RSAES/OAEP (с меткой и без) и «сырой» RSA, сделанные Python (`hashlib`, `pow`, padding проверен `openssl pkeyutl` в обе стороны), расшифрованы TPM; шифртексты TPM расшифрованы `openssl pkeyutl -decrypt` (OpenSSL 4.1.0); общая точка ECDH, вычисленная Python (аффинная арифметика P-256 с закрытым ключом OpenSSL, `openssl pkeyutl -derive` дал тот же `x`), совпала с `ECDH_ZGen`. Не проверено: текст Library v185 недоступен (сверка по v184, Part 4 не использовалась); пометки `NOT VERIFIED` — VTPM.md, пп. 35–44; сборка под `x86_64-unknown-none` не проверялась; независимое ревью не выполнено; constant-time RSA и ECDH не проверялись.

#### Срез C2 (policy-сессии, ContextSave/Load, TPM2_HMAC, Quote, Certify), выполнен исполнителем, ожидает ревью, коммита нет

Изменены: `crates/od-tpm/` (новые модули `src/policy.rs`, `src/context.rs`, `src/attest.rs`, `src/tests_c2.rs`; изменены `lib,codes,response,marshal,object,object_cmd,session,dispatch,state,startup,capability,tests,tests_b,tests_obj`; `README.md`), [VTPM.md](../specs/VTPM.md) (раздел «Фаза B, срез C2», NOT VERIFIED 45–52, «Отклонения», «Источники и сверка»), [DEPENDENCIES.md](../policies/DEPENDENCIES.md) (запись «новых пакетов нет»), [PROJECT_LAYOUT.md](../PROJECT_LAYOUT.md), строка статуса S7-23 в плане и в этом отчёте.

Факты: 12 команд (всего в таблице 56): `TPM2_PolicyRestart` (`0x180`), `PolicyPCR` (`0x17F`), `PolicyOR` (`0x171`), `PolicyAuthValue` (`0x16B`), `PolicyCommandCode` (`0x16C`), `PolicySecret` (`0x151`), `PolicyGetDigest` (`0x189`), `ContextSave` (`0x162`), `ContextLoad` (`0x161`), `TPM2_HMAC` (`0x155`), `Quote` (`0x158`), `Certify` (`0x148`). Policy-сессии (`TPM_SE_POLICY`, `TPM_SE_TRIAL` в `StartAuthSession`): digest по Part 3, 23.x, векторы digest проверены Python; авторизация объекта policy-сессией (`policyDigest` = `authPolicy`, `commandCode`, `cpHash`, `pcrUpdateCounter`, HMAC с authValue при `PolicyAuthValue`), ADMIN-роль для `objectHandle` `Certify` (`adminWithPolicy`, `PolicyCommandCode`), policy-отказ без DA. Контексты: формат по Part 1, 28.3 (KDFa-ключ и IV, AES-128-CFB, HMAC под proof иерархии); отказ при повреждении любого бита, при смене seed иерархии (`Clear`), на Reset, для выключенной иерархии, для `stClear` на Restart; сессии сохраняются и загружаются один раз. Keyed-hash ключи с `sign` и схемой HMAC/NULL; `TPM2_HMAC` проходит RFC 4231 (случаи 1–4). `Quote` и `Certify` подписывают `TPMS_ATTEST`; подписи (ECDSA P-256 и RSASSA-2048) проверены внешним Python; обфускация `clockInfo`/`firmwareVersion` ключей Storage по Part 3, 18.1.

Зависимости: новых пакетов нет, `Cargo.toml` и `Cargo.lock` не менялись.

Отклонения и ограничения: при `userWithAuth` CLEAR пароль и HMAC теперь `TPM_RC_POLICY_FAIL` (Part 3, 5.6, шаг 7) вместо `TPM_RC_AUTH_UNAVAILABLE` среза B — два теста обновлены; `PolicySecret` только с `expiration` 0 (NULL-билет, пустой `timeout`); контексты sequence-объектов и `TPM_RC_CONTEXT_GAP` не реализованы; `signHandle` `TPM_RH_NULL` для аттестаций не поддержан; `TPM2_HMAC` вместо `TPM2_MAC`; не реализованы `PolicySigned`, `PolicyTicket`, `PolicyNV`, `PolicyLocality`, `PolicyPassword`, `PolicyCpHash`, `PolicyAuthorize` и др.; keyed-hash: restricted и XOR-ключи, HMAC-подпись через `TPM2_Sign` — нет. **Не сделано из плана S7-23 (поэтому задача не принимается целиком):** шифрование параметров (`TPMA_SESSION.decrypt/encrypt`, AES-CFB/XOR; salted-сессии), шаблоны EK по TCG EK Credential Profile и сертификат EK (T-04), NV `Increment/Extend/SetBits` и счётчики, `TPM2_GetTime`, `SetPrimaryPolicy`. Windows-зависимые команды: `ContextSave/Load` (K-07) и `TPM2_HMAC` (T-03/K-10) реализованы; K-09 (`PolicyPCR/PolicySecret/PolicyAuthValue/Unseal`) покрыты; что из этого вызовет Windows — гипотеза до трассы S7-32.

Проверено (2026-11-07): `cargo test -p od-tpm --offline` — 232 теста проходят (было 202); независимые эталоны: digest policy (Python `hashlib`), HMAC-SHA-256 RFC 4231 (Python `hmac`), подпись `Quote` (ECDSA P-256, аффинная арифметика Python, детерминированная подпись закреплена) и `Certify` (RSASSA, `pow` против EMSA-PKCS1-v1_5); структуры `TPMS_ATTEST` собраны в тестах побайтно по Part 2. Не проверено: текст Library v185 недоступен (сверка по v184, Part 4 не использовалась; пометки `NOT VERIFIED` — VTPM.md, пп. 45–52); сборка под `x86_64-unknown-none` не проверялась; независимое ревью не выполнено; constant-time RSA и ECDSA не проверялись (ISSUE-064).

#### Срез C3 (NV counter/bits/extend, GetTime, signHandle NULL, шифрование параметров, snapshot v3), выполнен исполнителем, ожидает ревью, коммита нет

Изменены: `crates/od-tpm/` (новый модуль `src/paramenc.rs`, новый `src/tests_c3.rs`; изменены `lib,codes,marshal,nv,attest,session,context,dispatch,snapshot,state,capability,policy` (только комментарий), `tests,tests_b,tests_c1,tests_c2`; `README.md`), [VTPM.md](../specs/VTPM.md) (раздел «Фаза B, срез C3», NOT VERIFIED 53–58, формат snapshot v3, «Источники и сверка»), [DEPENDENCIES.md](../policies/DEPENDENCIES.md) (запись «новых пакетов нет»), [PROJECT_LAYOUT.md](../PROJECT_LAYOUT.md), строка статуса S7-23 в [STAGE_7.md](../plans/STAGE_7.md), этот отчёт.

Факты: 4 новые команды (всего в таблице 60): `TPM2_NV_Increment` (`0x134`), `NV_SetBits` (`0x135`), `NV_Extend` (`0x136`), `TPM2_GetTime` (`0x14C`). `DefineSpace` принимает `TPM_NT_COUNTER`, `BITS`, `EXTEND` (размеры 8, 8, 32; `TPM_RC_SIZE`), PIN и reserved — `TPM_RC_ATTRIBUTES`; `NV_Write` не-ordinary индекса — `TPM_RC_ATTRIBUTES`. Счётчик начинает с наибольшего значения любого счётчика плюс 1 (хранится в snapshot), переполнение — `TPM_RC_NV_RANGE`. `Extend` — `SHA-256(old ‖ data)` (векторы Python `hashlib`). `GetTime` — `TPMS_ATTEST` `TPM_ST_ATTEST_TIME`, подпись проверена внешним Python (аффинный ECDSA P-256), внешние `clockInfo`/`firmwareVersion` обфусцируются для не-Endorsement/Platform ключей, внутренние — в открытом виде. `signHandle` `TPM_RH_NULL` у `Quote`/`Certify`/`GetTime` — NULL-подпись (Part 3, 18.1). Шифрование параметров: `StartAuthSession` принимает `symmetric` AES-128-CFB и XOR(SHA-256); атрибуты `decrypt`/`encrypt` на авторизующих сессиях (первый параметр команды / ответа, cpHash по шифртексту, rpHash после шифрования); векторы XOR и AES-CFB — Python `hmac` и `openssl enc`. Snapshot **v3** (v2 отвергается, не читается молчаливо): поле «наибольшее значение NV-счётчика»; `restore` проверяет согласованность каждого NV-индекса с его типом. `TPM_ALG_XOR` в `TPM_CAP_ALGS`.

Зависимости: новых пакетов нет, `Cargo.toml` и `Cargo.lock` не менялись.

Отклонения и ограничения: `PolicySecret` по-прежнему только с `expiration` 0 (`timeout`, `timeEpoch`, `resetCount` в тексте implementation-specific; ticket без `PolicyTicket`); шифрование параметров — только на сессиях, авторизующих handle команды (сессия только для шифрования — `TPM_RC_AUTHSIZE`), без salted-сессий и `bind` к объекту; тестами покрыты `NV_Write` (decrypt) и `NV_Read` (encrypt), остальные команды списков `decrypt`/`encrypt` используют тот же общий код; списки команд выведены из таблиц Part 3, не из Part 4; `qualifiedSigner` при NULL `signHandle` — пустой буфер (NOT VERIFIED); не реализованы EK-шаблоны и сертификат (решение владельца), `PolicySigned`/`PolicyNV` и др., `NV_Certify`, PIN-индексы, `ORDERLY`.

Проверено (2026-11-07): `cargo test -p od-tpm --offline` — 264 теста проходят (было 232: 29 новых в `src/tests_c3.rs`, 3 unit-теста `paramenc.rs`; ожидания прежних тестов обновлены: 60 команд, 13 алгоритмов, snapshot v3); `cargo clippy`, `cargo fmt --check`, `cargo check --workspace` — см. итог исполнителя. Не проверено: текст Library v185 недоступен, сверка по v184; список мест с пометкой `NOT VERIFIED` — [VTPM.md](../specs/VTPM.md); ревью не проводилось.

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

- S7-23, срез A (вопрос сверх плана): одобрить ли `NV_WriteLock`/`NV_ReadLock` сверх профиля плана S7-23 (без них `WRITEDEFINE`/`WRITE_STCLEAR`/`READ_STCLEAR` не имеют эффекта). Реализованы исполнителем без решения владельца; решение за владельцем. Контракт — [VTPM.md](../specs/VTPM.md#фаза-b-срез-a-иерархии-hmac-сессии-nv-база-s7-23).

- S7-23, срез B (вопрос сверх плана): принять ли в срезе B `outPrivate`, защищённый по формулам Part 1, 20.3–20.5, но шифруемый потоком KDFa вместо AES-128-CFB (`aes`/`cfb-mode` по ADR-0009 не подключены: срез B их не требует), с записанным несовпадением формата блоба с другими TPM, либо потребовать подключить `aes`/`cfb-mode` в срезе B. Решение за владельцем. Контракт — [VTPM.md](../specs/VTPM.md#фаза-b-срез-b-объекты-и-ключи-s7-23).

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

### S7-16. Минимальный NVMe-контроллер в `od-machine` (срез A)

Статус: `in_progress`. **Срез A выполнен исполнителем, ожидает ревью, коммита нет. Срез B (I/O `Read`/`Write`/`Flush`, backend диска) выполнен исполнителем, см. раздел «Срез B» ниже; прерывания не реализованы.** Контракт — [NVME_DEVICE.md](../specs/NVME_DEVICE.md).

Создано: `docs/specs/NVME_DEVICE.md`; `crates/od-machine/src/nvme/{mod,controller,queues,admin,io,backend,tests}.rs`. Изменено: `firmware_board.rs` (`FirmwareBoardConfig::nvme`, запись device code 8, `LoadedFirmwareBoard::nvme()`, `run_boot` обслуживает контроллер), `lib.rs` (экспорты), `FIRMWARE_BOOT.md` (§2, §6.3), `MACHINE_CONTRACT.md` (раздел «NVMe controller»), `VIRTUAL_BOARD.md` (строка «Boot storage»), `STAGE_7.md` (статус S7-16).

Что работает (подтверждено тестами, `cargo test -p od-machine --offline`: 289 тестов, было 232; новые — 53 в модуле `nvme` и 4 в `firmware_board`):

- Регистры `CAP`, `VS`, `INTMS`, `INTMC`, `CC`, `CSTS`, `AQA`, `ASQ`, `ACQ` (в том числе половины 64-битных регистров по `Dword`), doorbells; ошибки ширины/выравнивания/доступа — `DeviceError`; `CC.EN` 0→1 создаёт admin-очереди и ставит `RDY`, неверный `CC` ставит `CFS`, `CC.EN` 1→0 сбрасывает очереди и features; `SHN` выставляет `SHST`.
- Admin-очереди и I/O-очереди (до четырёх пар), SQE 64 байта, CQE 16 байт, фазовый бит с переворотом на проходе, заполненная CQ останавливает извлечение команд, PRP1/PRP2/PRP list/цепочка листов, DMA только в RAM.
- Admin-команды: `Identify` (CNS 00/01/02/03; структуры Identify Controller и Namespace проверены побайтно), `Create/Delete I/O SQ/CQ` (SQ без CQ — `Completion Queue Invalid`, CQ с SQ — `Invalid Queue Deletion`), `Set/Get Features` (Arbitration, Power Management, Temperature Threshold, Error Recovery, Number of Queues, Interrupt Coalescing, Interrupt Vector Configuration, Write Atomicity Normal, Asynchronous Event Configuration), `Get Log Page` (Error Information с живыми записями, SMART/Health, Firmware Slot), `Asynchronous Event Request` (остаётся в очереди, завершается событием doorbell-ошибки), `Abort`. Неподдержанный opcode — `Invalid Command Opcode`.
- Плата: `FirmwareBoardConfig::nvme = None` (по умолчанию) оставляет handoff v1.0 байт в байт (`REFERENCE_HANDOFF_HEX` не менялся); `Some` добавляет запись кода 8, регион `MMIO` `[0xFC00_1000, +0x2000)`, `version_minor = 1`, подключает устройство к шине; образ с `handoff_minor_max = 0` отклоняется.

Не сделано и разрывы со спецификацией (записаны, не подогнаны):

- **Нет I/O `Read`/`Write`/`Flush` — разрыв с обязательными командами NVM command set (срез B).** Сейчас I/O-команда завершается `Invalid Command Opcode`; контроллер диск не читает. `NvmeBackend` — только интерфейс без реализации. Доказательство для S7-17 и этапа 5 срезом A не получено.
- **Значения сверены с Linux `include/linux/nvme.h` (master, 2026-11-07), не с текстом NVMe Base 2.4.** Текст NVMe Base 2.4 (и Command Set 1.3, PCIe Transport 1.4) получить не удалось: страница спецификаций не содержит прямых ссылок, PDF 2.4 по предполагаемому адресу — 404, PDF 2.3 из архива `WebFetch` не принял (больше 10 MiB), индексатор вернул сырые потоки PDF без текста. Сверено с заголовком Linux: регистры и doorbells, биты CC/CSTS/CAP (MQES, TO, CSS, MPSMIN), admin-opcode и I/O Flush/Write/Read, CNS 0–3, FID, LID 0x01–0x03, статусы, смещения Identify и log pages. NOT VERIFIED: `CAP.CQR`, AER info 0/1, правила PRP, умолчания features, поведение при ошибках, обязательность команд, LID `0x00`/`0x05`, `VS` 2.0 против 2.4 (NVME_DEVICE.md §1 и §11, шапки файлов). Заявлена версия `VS = 2.0.0`, а не 2.4.0.
- **Открытый вопрос (вероятное отклонение).** `CSTS.CFS` выставляется при `CC.EN = 1` с `IOSQES ≠ 6` или `IOCQES ≠ 4` (`controller.rs`, `enable`); по спецификации (неподтверждённо) эти поля проверяются при создании I/O-очереди, а не при включении. Поведение и тесты не менялись; помечено в коде и NVME_DEVICE.md §11.
- **Разрыв в тестах.** Путь «гость пишет doorbell → квант останавливается по `doorbell_pending` → `service` между квантами» тестом не покрыт: интерпретатор поддерживает 64-битный `mov [rbx], rcx`, а doorbell — 32-битный (`Dword`), поэтому запись doorbell гостем end-to-end не тестируется. На host-стороне `service` покрыт (`run_boot_executes_commands_submitted_before_the_guest_starts`). Код не менялся (NVME_DEVICE.md §3, §12).
- `VID`, `SSVID`, `IEEE` = 0 (PCI vendor ID и OUI проекту не выданы); `Power On Hours` и `Controller Busy Time` = 0 (у устройства нет времени); Supported Log Pages (`0x00`) и Commands Supported and Effects (`0x05`) не реализованы — их обязательность не сверена. Это открытые вопросы среза A (первый кандидат в дефекты, вместе с `VS = 2.0.0`; NVME_DEVICE.md §11).
- Прерывания не доставляются (`gsi = 0xFFFF_FFFF`, `INTMS/INTMC`, `IEN`, coalescing хранятся без эффекта); контроллер виден только прошивке (PCI-привязка blocked до S4-30), ОС его не найдут — S7-31, S7-32 остаются blocked.
- Команды завершаются в `NvmeHandle::service` между квантами, а не сразу после doorbell; время исполнения не моделируется. Прошивка репозитория пока не читает запись NVMe (handoff v1.1) — S7-17.
- Не делалось по заданию: Namespace Management, firmware download/commit, несколько NS, SGL, `Device Self-test`, RTC/PM (S7-14), PCI-привязка (S4-30).

Проверки (2026-11-07, worktree `/private/tmp/open_desktop-s9`): `cargo test -p od-machine --offline` — 289 passed; `cargo fmt -p od-machine -- --check` — чисто; `cargo clippy -p od-machine --offline --all-targets` — 0 предупреждений в `nvme/`, остальные предупреждения прежнего кода (`od-isa`, `io_apic.rs`, `firmware_board.rs:278` и др.) не трогались; `cargo check --workspace --all-targets --offline` — успешно; `python3 tools/check_layout.py` — 45 `ERROR` (как до изменений); `git diff --check` — чисто; в `nvme/` вне тестов нет `unwrap`/`expect`/`panic!`/`println!`. Независимое ревью не выполнялось (ожидается).

### S7-16. Минимальный NVMe-контроллер в `od-machine` (срез B: I/O)

Статус: `in_progress`. **Срез B выполнен исполнителем, ожидает ревью, коммита нет.** Контракт — [NVME_DEVICE.md](../specs/NVME_DEVICE.md), раздел 13. PCI-привязка — S4-30 (blocked), прерывания не реализованы (нет IRQ-маршрута).

Изменено: `nvme/{io,backend,controller,queues,admin,mod,tests}.rs`, `lib.rs` (экспорт `InMemoryBackend`, `NvmeAttachError`), `NVME_DEVICE.md`, `MACHINE_CONTRACT.md`, `VIRTUAL_BOARD.md`, `STAGE_7.md` (статус S7-16).

Что работает (`cargo test -p od-machine --offline`: 307 тестов в библиотеке, было 289; новые — 15 в `nvme/tests.rs`, 2 в `backend.rs`, 1 в `queues.rs`):

- `Read`, `Write`, `Flush` из очередей I/O: SLBA (`CDW10/11`), NLB 0-based (`CDW12[15:0]`), PRP1/PRP2/PRP list, максимум 1 MiB (`MDTS = 8`); `Write` сначала читает весь буфер гостя, потом пишет в backend (ошибка PRP не меняет диск).
- Проверки и статусы (без паник): NSID ≠ 1 — `Invalid Namespace or Format`; `SLBA + NLB` вне `NSZE` (с защитой от переполнения) — `LBA Out of Range`; NLB больше `MDTS`, `FUA`/`LR`/`PRINFO`/`DTYPE` (`CDW12[31:16]`), `DSM` (`CDW13`), `PSDT`/`FUSE` — `Invalid Field`; PRP вне RAM — `Data Transfer Error`; неровный `PRP2` — `PRP Offset Invalid`; отказ backend — `Internal Error`; неизвестный I/O-opcode — `Invalid Command Opcode`.
- `trait NvmeBackend` (`sector_count`, `read_sectors`, `write_sectors`, `flush`) и `InMemoryBackend` (нулевой вектор фиксированного размера); подключение — `NvmeHandle::attach_backend` только при `CC.EN = 0`, `NSZE` = размер backend, Identify согласован. Файлового ввода-вывода в `od-machine` нет; файл/CoW подключает владелец машины над `od-storage`.
- SMART/Health: `Host Read/Write Commands` и `Data Units Read/Written` — живые счётчики.
- Тесты: Write → Read на границе страницы (PRP1 + PRP2) и через PRP list, максимальная передача, Flush (доходит до backend), LBA/NSID/NLB, неподдержанные поля, плохие PRP (диск не меняется), отказ backend, без backend, полная CQ (запись не исполняется до сдвига `head`), I/O без созданной SQ. Тест «гость пишет doorbell» не делался (разрыв среза A); покрыто через `service` напрямую.

Решения: LBA 512 байт, 4 KiB LBA **не поддержан** (один формат `LBAF0`); без backend устройство существует и отвечает `Namespace Not Ready` (не «не подключать устройство»); `Flush` с `NSID = 0xFFFF_FFFF` отвергается; `Flush` всегда вызывает `flush` backend, хотя `VWC = 0`.

Значения `LBA Out of Range` (`0x80`), `Namespace Not Ready` (`0x82`), `Internal Error` (`0x06`), `Invalid Field` (0x2), `Data Transfer Error` (0x4), `Invalid Namespace` (0xB), DNR (0x4000) и opcode `0x00/01/02` сверены с Linux `nvme.h`; NOT VERIFIED against NVMe Base 2.4: DNR для статусов среза B и условия их применения; раскладка `CDW12`/`CDW13` Read/Write; порядок проверок; поведение `Flush`; единицы SMART data units. Список — NVME_DEVICE.md §13.5.

Не сделано: LBA 4 KiB; `Dataset Management`/`Write Zeroes`/`Compare`/`Write Uncorrectable`/`Verify`/`Copy`; `FUA`, защита данных; Namespace Management, несколько NS; SGL; файловый backend; прерывания; PCI-привязка (S4-30); Supported Log Pages (`0x00`) и Commands Supported and Effects (`0x05`) (разрыв среза A); RTC/PM (S7-14). Команда в очереди без созданной SQ невозможна: doorbell несозданной очереди — событие «Write to Invalid Doorbell Register» среза A, команда не извлекается, completion нет (`Invalid Queue Identifier` — статус admin-команд Create/Delete).

Проверки (2026-11-07, worktree `/private/tmp/open_desktop-s9`): `cargo test -p od-machine --offline` — 307 passed (библиотека); `cargo clippy -p od-machine --offline --all-targets` — 0 предупреждений в `nvme/`; `cargo fmt -p od-machine -- --check` — чисто; `cargo check --workspace --all-targets --offline` — успешно; `python3 tools/check_layout.py` — 45 `ERROR` (как до изменений); `git diff --check` — чисто; в `nvme/` вне тестов нет `unwrap`/`expect`/`panic!`/`println!`. Независимое ревью не выполнялось (ожидается).

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

**Остаются `Unsupported`:** `B8+r id` без `66`, любые REX-формы (`40`–`4F` + `B0`–`BF`, в т. ч. `48 B8 io`), `66 B0`–`B7`, повторный `66`, `67`, сегментные, `F2`/`F3`, `C6 /0`, остальные `C7 /0` кроме exact S7-04 `48 C7 C0 id`; `F0` сохраняет классификацию декодера. Профили CR3, CR0.WP, стековый, CPL3 и `PagedCpl0PioU8V1` не расширяются.

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

### S7-04. Срез: безопасный разбор заголовка handoff

Статус: частичный срез реализован (кодовый коммит `66fd6dc`); S7-04 не закрыта. Добавлен allocation-free `Handoff::parse` в `firmware/uefi/src/handoff.rs`: проверяются magic, major/minor, размер фиксированного заголовка, размер блока, CRC до разбора таблиц, размеры записей (region 32 байта, device 48 байт), границы, выравнивание и отсутствие пересечения таблиц. Парсер возвращает заимствованный view и не выделяет память. Отдельная запись `[workspace]` в `firmware/uefi/Cargo.toml` нужна, чтобы Cargo сохранял standalone-проект прошивки при тестировании worktree, вложенного в другой checkout.

Проверено: `cargo test --manifest-path firmware/uefi/Cargo.toml --offline` — 158 passed; `cargo check --manifest-path firmware/uefi/Cargo.toml --target x86_64-unknown-none --offline` — успешно; `rustfmt --check firmware/uefi/src/handoff.rs` — чисто; `git diff --check` — чисто. Полный `cargo fmt` отдельного проекта пока не проходит из-за уже существующих отличий форматирования в файлах ACPI; файлы вне среза оставлены без изменений. Независимое ревью среза (gpt-6.1-sol, high): после исправления размера device-записи, порядка CRC-проверки, выравнивания и перекрытия таблиц замечаний, блокирующих объявленный scope, нет.

Ограничения этого среза: содержимое device records, header reserved/CPU count/TSC/serial и соответствие регионов точной карте платы остаются непроверенными. Не реализованы entry/stack, собственные page tables, аллокаторы, UEFI memory map, COM1 boot log и запуск полного образа в машине.

#### S7-04. Срез: структурная проверка записей карты памяти

Статус: реализован в коммите `47e205d`, прошёл независимое ревью; S7-04 целиком не закрыта. Парсер проверяет допустимые типы регионов, включая ACPI types только с handoff minor 2, атрибуты (framebuffer screen number — 8 бит), 4 KiB alignment, ненулевую длину, checked end до `2^36`, owner index, reserved=0, сортировку и отсутствие пересечений. Проверяется число регионов `1..=128` и device_count по версиям (v1.0 — 7, v1.1 — 8, v1.2 — 8 или 9); схема minor выше 2 пока не поддерживается.

Проверки: `cargo test --manifest-path firmware/uefi/Cargo.toml --offline` — 160 passed; `cargo check --manifest-path firmware/uefi/Cargo.toml --target x86_64-unknown-none --offline` — успешно; `rustfmt --check firmware/uefi/src/handoff.rs` и `git diff --check` — чисто. Ревьюер gpt-6.1-sol / high: блокирующих замечаний нет.

#### S7-04. Срез: проверка таблицы устройств и фиксированных полей заголовка

Статус: реализован в коммите `e1a7b16` и прошёл независимое ревью; S7-04 целиком не закрыта. Проверяются уникальность обязательных device types 1–7, допустимые типы расширений для minor 1/2 и их `PRESENT`, маски flags/irq_flags, PIO/MMIO ranges и нулевые адреса отсутствующих устройств. Header validation проверяет board profile, `cpu_count = 1`, ненулевой `tsc_hz`, serial length/ASCII/padding и reserved/flags. Для minor 0/1/2 проверяется ожидаемое число и наличие расширенных устройств. Device-specific `param0/param1`, ожидаемые адреса всей платы и точное покрытие карты памяти пока не сверяются.

Проверки: `cargo test --manifest-path firmware/uefi/Cargo.toml --offline` — 162 passed; `cargo check --manifest-path firmware/uefi/Cargo.toml --target x86_64-unknown-none --offline` — успешно; `rustfmt --check firmware/uefi/src/handoff.rs` и `git diff --check` — чисто. Ревьюер gpt-6.1-sol / high: дефект `PRESENT` для расширений 8/9 исправлен, блокирующих замечаний нет.

#### S7-04. Срез: проверка boot reservations и объёма временных page tables

Статус: реализован в коммите `b1c49f3`, прошёл независимое ревью; S7-04 целиком не закрыта. Добавлен `Handoff::parse_for_boot`, который совмещает структурный парсинг с проверкой фиксированных диапазонов HANDOFF, BOOT_STACK, FIRMWARE_SHADOW, BOOT_PAGE_TABLES и ACPI_TABLES/ACPI_NVS. Размер временных таблиц сверяется с количеством уникальных отображённых 2 MiB, 1 GiB и 512 GiB блоков из region table плюс PML4; RESERVED не учитывается, как в расчёте `od-machine`. Метаданные parsed view стали private с read-only accessors, чтобы вызывающий не мог нарушить проверенные границы.

Проверки включают v1.0 и v1.2 ACPI split, неверную длину page-table reservation и разреженную карту с framebuffer, MMIO, ROM, RESERVED gap и RAM выше 4 GiB; расчёт тестовой карты независимо сверяется через `BTreeSet`. `cargo test --manifest-path firmware/uefi/Cargo.toml --offline` — 164 passed; `cargo check --manifest-path firmware/uefi/Cargo.toml --target x86_64-unknown-none --offline` — успешно; `rustfmt --check firmware/uefi/src/handoff.rs` и `git diff --check` — чисто. Ревьюер gpt-6.1-sol / high: блокирующих замечаний нет.

Ограничения: пока не сверяются все регионы и устройства с полным ожидаемым board map, owner/address пары устройств, device-specific параметры и exact RAM coverage. Runner исполняет `CLD`, но затем блокируется на `C7 MOV RAX,imm32` (ISSUE-101), до Rust entry и COM1 не доходит. В исходнике startup есть проверка RDI, зануление `.bss` и передача аргумента; отдельный стек не устанавливается. Также остаются CR3/page table construction, аллокаторы, UEFI memory map и итоговые COM1 строки с версией/картой.

#### S7-04. Срез: разбор handoff из entry и stop-коды

Статус: реализован в коммите `e1ef0f1`; независимое ревью прошло после исправления отдельного stop-кода shadow reservation; S7-04 целиком не закрыта. `od_fw_main` после проверки фиксированного `RDI` создаёт срез handoff окна `[0x0010_0000, 0x0011_0000)`, вызывает `Handoff::parse_for_boot` с minor max `0` (совпадает с текущим `HANDOFF_MINOR_MAX` image builder), при ошибке выдаёт stop-код по §12, а при успехе — `OD-FW S7-04 handoff validated` и останавливается. Добавлены stop-коды `0x02–0x0C`; unsupported version включает `major << 16 | minor`, shadow reservation выдаёт отдельный `0x0C`. Обновлён комментарий бинаря: это валидирующий entry-срез, не полная UEFI.

Проверки: `cargo test --manifest-path firmware/uefi/Cargo.toml --offline --quiet` — 165 passed; freestanding `cargo check --manifest-path firmware/uefi/Cargo.toml --target x86_64-unknown-none --offline --quiet` — успешно; scoped `rustfmt --check` и `git diff --check` — чисто; `python3 tools/build/build_firmware.py --out /private/tmp/od-fw-s7-04.bin` собрал образ 4 MiB (`entry_offset=0x40`, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04.bin --com1-out /private/tmp/od-fw-s7-04.com1 --budget 10000` завершилась `status=error`, `exit=unsupported`, `Unsupported { opcode: 252 }` на `RIP=0x200040`, `retired=0`, `com1_bytes=0`: первый `CLD` в `start.S` не допущен paged CPU profile, до Rust entry машина не дошла. Это зафиксировано в [ISSUE-100](../KNOWN_ISSUES.md#issue-100-s7-04-paged-cpu-profile-does-not-admit-cld-at-firmware-entry). Ревью: независимое, без оставшихся blockers; итоговая повторная проверка подтвердила исправление соответствия кода `0x0C` и регрессионные assertions.


<a id="s7-04-cpu-profile-cld"></a>
#### S7-04 CPU-profile slice: bare `CLD`

Статус: реализован в коммите `6553d56`, независимое ревью пройдено; изменения находятся только в codex2. В `PagedCpl0ProbeV1` допущена ровно Primary `FC`, длина 1, без префиксов; `step_paged_probe_with_rdtsc` наследует это расширение, остальные профили его не получают. Retire очищает только DF, добавляет DF к `rflags_defined` и продвигает RIP. Тесты проверяют DF=1 и DF=0, сохранение полного остального CPU state, fetch fault, STD/префиксные формы и отказ stack/CR3/CR0.WP/CPL3 профилей. Спецификации: [CPU_CONTRACT](../specs/CPU_CONTRACT.md) и [FIRMWARE_BOOT](../specs/FIRMWARE_BOOT.md).

Проверки: `cargo test --offline -p od-isa --quiet` — 4174 passed; `cargo test --offline -p od-runner --test firmware_boot_cli --quiet` — 5 passed; `rustfmt --check --edition 2024` для двух затронутых Rust-файлов и `git diff --check` — чисто. Повторный runner-прогон на том же 4 MiB образе SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe` вернул `status=error`, `exit=unsupported`, `Unsupported { opcode: 199 }`, `retired=1`, `RIP=0x200041`, `com1_bytes=0`; байты следующей инструкции — `48 C7 C0 00 00 10 00` (`MOV RAX,imm32`). Это подтверждает bare `CLD` в реальном paged runner и отдельно локализует следующий предел, не проверяет Rust handoff parser. [ISSUE-100](../KNOWN_ISSUES.md#issue-100-s7-04-paged-cpu-profile-does-not-admit-cld-at-firmware-entry) закрыта только для bare `CLD`; текущая блокировка — [ISSUE-101](../KNOWN_ISSUES.md#issue-101-s7-04-paged-cpu-profile-does-not-admit-c7-mov-imm32-at-entry).

<a id="s7-04-cpu-profile-c7"></a>
#### S7-04 CPU-profile slice: entry `MOV RAX,imm32`

В базовый `PagedCpl0ProbeV1` добавлена только Primary `48 C7 C0 id` (`MOV RAX,imm32`): REX.W, `/0`, регистровый operand RAX, ровно 7 байт. Исполняется после стандартной проверки режима/profile/CR0/CR4/activity; записывает sign-extended immediate в RAX, продвигает RIP на 7 и возвращает одну retired instruction. Другие регистры, flags/definedness, память и I/O не меняются. Memory operand, соседние регистры/ширины/префиксы и другие paged profiles остаются отклонёнными. Декодер не изменялся. Независимое ревью: блокирующих замечаний по коду нет; исправлены диагностика следующего opcodes и проверка fault на первом immediate byte.

Проверка: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 80 passed. Образ собран `python3 tools/build/build_firmware.py --out /private/tmp/od-fw-s7-04-c7.bin`, 4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`. Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-c7.bin --com1-out /private/tmp/od-fw-s7-04-c7.com1 --budget 10000` retired `CLD` и C7 (`retired=2`), затем вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 57 }` (decimal = `0x39`), `RIP=0x200048`; байты — `48 39 C7` (`CMP RDI,RAX`), `com1_bytes=0`. Это подтверждает только исполнение двух entry instructions; Rust entry, parser и COM1 остаются непроверенными в машине. [ISSUE-101](../KNOWN_ISSUES.md#issue-101-s7-04-paged-cpu-profile-does-not-admit-c7-mov-imm32-at-entry) закрыта узко; текущая блокировка — [ISSUE-103](../KNOWN_ISSUES.md#issue-103-s7-04-paged-cpu-profile-does-not-admit-jcc-jne-at-firmware-entry).

<a id="s7-04-cpu-profile-cmp"></a>
#### S7-04 CPU-profile slice: entry `CMP RDI,RAX`

В базовый `PagedCpl0ProbeV1` добавлена только точная Primary `48 39 C7` (`CMP RDI,RAX`): REX `0x48`, qword, длина 3, `/r` register operands RDI/RAX. Используется существующая арифметическая семантика Cmp, GPR не меняются, арифметические RFLAGS становятся defined, RIP увеличивается на 3. Другие CMP forms и остальные paged profiles остаются вне allowlist. Code review прошёл без блокеров; после него добавлены fetch-fault через границу страницы и явный CPL3 reject.

Проверка: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 84 passed; `rustfmt --check --edition 2024` и `git diff --check` — чисто. Образ `/private/tmp/od-fw-s7-04-cmp.bin` (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) в runner retired 3 инструкции, затем остановился на `75 24` (`JNZ rel8`, `Unsupported { opcode: 117 }`, decimal = `0x75`) на `RIP=0x20004b`, `com1_bytes=0`. Это доказывает только трёхинструкционное продвижение; Rust entry, parser и COM1 не достигнуты. Следующий blocker — [ISSUE-103](../KNOWN_ISSUES.md#issue-103-s7-04-paged-cpu-profile-does-not-admit-jcc-jne-at-firmware-entry).

<a id="s7-04-cpu-profile-jnz"></a>
#### S7-04 CPU-profile slice: exact entry `JNZ rel8`

В базовом `PagedCpl0ProbeV1` допущена только Primary форма `75 24`: JNZ/JNE rel8 с точным displacement `+0x24`, длина 2, без префиксов. Ветка использует `ConditionCode::NotEqual::evaluate`: при ZF=0 переходит на `next_rip + 0x24`, при ZF=1 продолжает с `next_rip`. Flags, definedness, регистры, память и I/O сохраняются; canonicality проверяется только для взятой ветви. Ревью выявило ложную проверку non-canonical fall-through, её исправили и добавили два граничных теста. Все другие Jcc/JMP формы, displacement и профили остаются вне allowlist.

Проверка: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 89 passed. Образ `/private/tmp/od-fw-s7-04-jnz.bin` (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) в runner retired 4 инструкции; JNZ не взят при RDI=RAX, затем runner остановился на `49 89 F8` (`MOV R8,RDI`, `Unsupported { opcode: 137 }`, decimal = `0x89`) на `RIP=0x20004d`, `com1_bytes=0`. Это доказывает только четыре entry instructions; Rust entry, parser и COM1 не достигнуты. [ISSUE-103](../KNOWN_ISSUES.md#issue-103-s7-04-paged-cpu-profile-does-not-admit-jcc-jne-at-firmware-entry) решена в точной форме `75 24`; следующая блокировка — [ISSUE-104](../KNOWN_ISSUES.md#issue-104-s7-04-paged-cpu-profile-does-not-admit-mov-r8-rdi-at-firmware-entry).

<a id="s7-04-cpu-profile-mov-r8-rdi"></a>
#### S7-04 CPU-profile slice: exact entry `MOV R8,RDI`

В `PagedCpl0ProbeV1` добавлена только Primary `49 89 F8`: REX.W+B, длина 3, qword, destination R8, source RDI. Используется существующая операция MOV; запись меняет только R8 и RIP на `+3`, сохраняя остальные GPR, flags и `rflags_defined`. Memory/I/O нет. Соседние регистры, направления, ширины, префиксы, memory forms, stack/CR3/CR0.WP и CPL3 профили остаются отклонёнными. Код и runtime slice ожидают независимого ревью.

Проверка: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 93 passed; `rustfmt --check --edition 2024` и `git diff --check` — чисто. Образ `/private/tmp/od-fw-s7-04-mov-r8.bin` (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) в runner retired 5 инструкций, затем остановился на `48 8D 3D 09 2B 00 00` (`LEA RDI,[RIP+0x2b09]`, `Unsupported { opcode: 141 }`, decimal = `0x8d`) на `RIP=0x200050`, `com1_bytes=0`. Это доказывает только пять entry instructions; Rust entry, parser и COM1 не достигнуты. [ISSUE-104](../KNOWN_ISSUES.md#issue-104-s7-04-paged-cpu-profile-does-not-admit-mov-r8-rdi-at-firmware-entry) решена узко; следующая блокировка — [ISSUE-105](../KNOWN_ISSUES.md#issue-105-s7-04-paged-cpu-profile-does-not-admit-lea-rdi-rip-relative-at-firmware-entry).


<a id="s7-04-cpu-profile-lea"></a>
#### S7-04 CPU-profile slice: exact entry `LEA RDI,[RIP+0x2b09]`

Добавлена только форма `48 8D 3D 09 2B 00 00` в базовый `PagedCpl0ProbeV1`; адрес вычисляется через существующий `effective_address` относительно next RIP, без чтения памяти. Тест проверяет точную цель, изменение только RDI/RIP, неизменность flags/definedness, соседние формы, CPL3/прочие профили и атомарный fetch fault.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 97 passed. `python3 tools/build/build_firmware.py --out /private/tmp/od-fw-s7-04-lea.bin` успешно собрал 4 MiB образ, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`. `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-lea.bin --com1-out /private/tmp/od-fw-s7-04-lea.com1 --budget 10000` вернул `status=error`, `exit=unsupported`, `Unsupported { opcode: 141 }`, `retired=6`, `RIP=0x200057`, `com1_bytes=0`. Bytes at image offset `0x57`: `48 8d 0d 02 2b 00 00` (`LEA RCX,[RIP+0x2b02]`), следующий blocker [ISSUE-106](../KNOWN_ISSUES.md#issue-106-s7-04-paged-cpu-profile-does-not-admit-lea-rcx-rip-relative-at-firmware-entry). Реализация ожидает независимого ревью.


<a id="s7-04-cpu-profile-lea-rcx"></a>
#### S7-04 CPU-profile slice: exact entry `LEA RCX,[RIP+0x2b02]`

В базовый paged CPL0 профиль добавлена только точная форма `48 8D 0D 02 2B 00 00`. Как и LEA RDI, она использует существующий next-RIP effective-address расчёт, не обращается к памяти и меняет только RCX/RIP. Соседние формы, другие профили, CPL3 и fetch fault проверены.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 101 passed. Сборка `/private/tmp/od-fw-s7-04-lea-rcx.bin` успешна, 4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`. Runner command завершился `status=error`, `exit=unsupported`, `Unsupported { opcode: 41 }`, `retired=7`, `RIP=0x20005e`, `com1_bytes=0`. Байты на image offset `0x5e` — `48 29 f9` (`SUB RCX,RDI`), текущий blocker [ISSUE-107](../KNOWN_ISSUES.md#issue-107-s7-04-paged-cpu-profile-does-not-admit-sub-rcx-rdi-at-firmware-entry). Изменение ожидает независимого ревью.


<a id="s7-04-cpu-profile-sub"></a>
#### S7-04 CPU-profile slice: exact entry `SUB RCX,RDI`

В базовый paged CPL0 профиль допущены только `48 29 F9`. Однозначно: Intel order `SUB RCX,RDI` (`RCX := RCX - RDI`); AT&T assembler spelling `subq %rdi,%rcx`. Реализация использует существующий `ArithmeticKind::Sub`. Тесты проверяют результат, полный набор арифметических flags и definedness, сохранение остальных регистров/state, альтернативные opcode/регистры/ширины/префиксы/память, fetch fault и profile/CPL3 isolation.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 105 passed. Firmware image `/private/tmp/od-fw-s7-04-sub.bin` собрана успешно (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner returned `status=error`, `exit=unsupported`, `Unsupported { opcode: 49 }`, `retired=8`, `RIP=0x200061`, `com1_bytes=0`. Байты по offset `0x61`: `31 C0` (`XOR EAX,EAX`), следующий blocker [ISSUE-108](../KNOWN_ISSUES.md#issue-108-s7-04-paged-cpu-profile-does-not-admit-xor-eax-eax-at-firmware-entry). Slice ожидает независимого ревью.


<a id="s7-04-cpu-profile-xor"></a>
#### S7-04 CPU-profile slice: exact entry `XOR EAX,EAX`

В base `PagedCpl0ProbeV1` допущено только `31 C0`. Существующий `LogicKind::Xor` рассчитывает результат/flags; write в EAX обнуляет верхние 32 бита RAX. CF/OF очищаются, ZF/PF/SF определяются, concrete AF остается прежним, но AF исключается из definedness как undefined по контракту логических операций. Соседние формы, profile/CPL3 isolation и fetch fault проверены.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 109 passed. Firmware image `/private/tmp/od-fw-s7-04-xor.bin` собрана успешно (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner вернул `status=error`, `exit=unsupported`, `Unsupported { opcode: 170 }`, `retired=9`, `RIP=0x200063`, `com1_bytes=0`. Байты по offset `0x63`: `F3 AA` (`REP STOSB`), следующий blocker [ISSUE-109](../KNOWN_ISSUES.md#issue-109-s7-04-paged-cpu-profile-does-not-admit-rep-stosb-at-firmware-entry). Срез ожидает независимого ревью.


<a id="s7-04-cpu-profile-rep-stosb"></a>
#### S7-04 CPU-profile slice: exact `REP STOSB`

Базовый paged CPL0 профиль принимает только bare `F3 AA` с address size 64 и byte element. Нулевой count продвигает RIP без memory access. Иначе каждое успешное store берёт AL, пишет в `[RDI]`, обновляет RDI по DF и уменьшает RCX. На fault ранее выполненные итерации остаются committed, текущий iteration не обновляет счетчики, RIP остаётся на REP. Count больше `REP_ITERATION_WORK_LIMIT` (1024) отклоняется до side effects; это ограничивает работу одного CPU step. Между итерациями асинхронный STOP не poll-ится, только на boundaries инструкций; максимум 1024 stores.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 116 passed; zero/positive counts, DF forward/backward, memory bytes, flags and definedness retention, partial-progress page fault, fetch fault, cap, neighbors, CPL3 and other paged profiles. Firmware `/private/tmp/od-fw-s7-04-rep-stosb.bin` built successfully (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner: `status=error`, `exit=unsupported`, `Unsupported { opcode: 137 }`, `retired=10`, `RIP=0x200065`, `com1_bytes=0`. Bytes at offset `0x65` are `4C 89 C7` (`MOV RDI,R8`); next blocker [ISSUE-110](../KNOWN_ISSUES.md#issue-110-s7-04-paged-cpu-profile-does-not-admit-mov-rdi-r8-at-firmware-entry). Slice awaits independent review.


<a id="s7-04-cpu-profile-mov-rdi-r8"></a>
#### S7-04 CPU-profile slice: exact entry `MOV RDI,R8`

В base `PagedCpl0ProbeV1` добавлена только `4C 89 C7` (REX.W+R, qword register-register MOV, destination RDI/source R8). Использована существующая MOV семантика; меняются только RDI и RIP. Tests проверяют full state, exact register/encoding, prefix/opcode/direction/width/memory neighbors, fetch fault, CPL3 и stack/CR3/CR0.WP isolation.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 121 passed. Образ `/private/tmp/od-fw-s7-04-mov-rdi-r8.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner returned `status=error`, `exit=unsupported`, `Unsupported { opcode: 232 }`, `retired=11`, `RIP=0x200068`, `com1_bytes=0`. Bytes at image offset `0x68`: `E8 83 01 00 00` (`CALL rel32`), next blocker [ISSUE-111](../KNOWN_ISSUES.md#issue-111-s7-04-paged-cpu-profile-does-not-admit-call-rel32-at-firmware-entry). Slice awaits independent review.

#### S7-04 CPU-profile slice: exact firmware `CALL rel32`

В base `PagedCpl0ProbeV1` добавлен только firmware CALL `E8 83 01 00 00`: Primary `E8`, длина 5, без префиксов, Qword, displacement `+0x183`. Использованы существующие canonical-target check и qword stack push: при успехе return RIP `0x20006d` записывается по `[RSP-8]`, RSP уменьшается на 8, управление переходит на `0x2001f0`; для начального firmware RSP store address — `0x14fff8`. Проверены full-state результат, flags/definedness, соседние target/opcode/prefix формы, CPL3 и другие paged profiles, fetch fault, #SS noncanonical stack и stack-write #PF atomicity.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 127 passed. Образ `/private/tmp/od-fw-s7-call.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-call.bin --com1-out /private/tmp/od-fw-s7-call.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 131 }` (`0x83`), `retired=12`, `RIP=0x2001f0`, `com1_bytes=0`. Следующие байты Rust prologue: `48 83 EC 58` (`SUB RSP,0x58`), открыта [ISSUE-112](../KNOWN_ISSUES.md#issue-112-s7-04-rust-entry-sub-rsp-imm8-unsupported). ISSUE-111 закрыта только для exact firmware CALL; parser и COM1 остаются не достигнуты. Срез ожидает независимое ревью.

#### S7-04 CPU-profile slice: exact Rust-entry `SUB RSP,0x58`

В base `PagedCpl0ProbeV1` добавлена только exact форма `48 83 EC 58`: qword SUB, destination RSP, sign-extended imm8 `0x58`, без других префиксов. Используется существующая SUB arithmetic/flags семантика. Инструкция меняет RSP и шесть arithmetic flags/definedness, но не обращается к стеку и не проверяет canonicality результата; при firmware RSP `0x14fff8` результат — `0x14ffa0`. Остальные forms и paged profiles не расширены.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 133 passed; проверены full state, flags/definedness, noncanonical result без fault, fetch fault на immediate через границу страницы, unsupported neighbors, CPL3 и stack/CR3/CR0.WP isolation. Образ `/private/tmp/od-fw-s7-sub-rsp.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-sub-rsp.bin --com1-out /private/tmp/od-fw-s7-sub-rsp.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 129 }` (`0x81`), `retired=13`, `RIP=0x2001f4`, `com1_bytes=0`. Следующие байты: `48 81 FF 00 00 10 00` (`CMP RDI,0x00100000`); открыта [ISSUE-113](../KNOWN_ISSUES.md#issue-113-s7-04-rust-entry-cmp-rdi-imm32-unsupported). ISSUE-112 закрыта только для exact stack adjustment; parser и COM1 остаются не достигнуты. Срез ожидает независимое ревью.

#### S7-04 CPU-profile slice: exact Rust-entry `CMP RDI,0x00100000`

В base `PagedCpl0ProbeV1` добавлена только форма `48 81 FF 00 00 10 00`: qword CMP, destination RDI, sign-extended imm32 `0x00100000`, ровно REX.W без иных префиксов. Существующая CMP семантика обновляет все шесть арифметических flags/definedness и сохраняет GPR/другие flags; сравнение с ожидаемым handoff GPA устанавливает ZF и PF. Data memory и I/O не затрагиваются. Проверены full state, fetch fault immediate через границу страницы, соседние формы и CPL3/stack/CR3/CR0.WP isolation.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 137 passed. Образ `/private/tmp/od-fw-s7-cmp-imm.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-cmp-imm.bin --com1-out /private/tmp/od-fw-s7-cmp-imm.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 117 }` (`0x75`), `retired=14`, `RIP=0x2001fb`, `com1_bytes=0`. Следующие bytes: `75 77` (`JNZ rel8`); открыта [ISSUE-114](../KNOWN_ISSUES.md#issue-114-s7-04-rust-entry-jnz-rel8-0x77-unsupported). ISSUE-113 закрыта только для exact CMP; parser и COM1 остаются не достигнуты. Срез ожидает независимое ревью.

#### S7-04 CPU-profile slice: exact Rust-entry `JNZ +0x77`

К ранее допущенному exact `75 24` в base `PagedCpl0ProbeV1` добавлена только форма `75 77`: JNZ/JNE rel8, длина 2, displacement `+0x77`, без префиксов. Реиспользованы условие и taken-target canonical check; not-taken ветвь продвигает RIP на 2 без проверки canonicality fall-through. Flags, definedness и остальной CPU state неизменны. Tests проверяют taken/not-taken для обеих точных форм, full state, флаги, noncanonical target/fall-through, fetch fault на rel8, neighbors и CPL3/другие paged profiles.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 137 passed. Образ `/private/tmp/od-fw-s7-jnz77.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-jnz77.bin --com1-out /private/tmp/od-fw-s7-jnz77.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 141 }` (`0x8d`), `retired=15`, `RIP=0x2001fd`, `com1_bytes=0`. Следующие bytes: `48 8D 7C 24 08` (`LEA RDI,[RSP+8]`); открыта [ISSUE-115](../KNOWN_ISSUES.md#issue-115-s7-04-rust-entry-lea-rdi-rsp-disp8-unsupported). ISSUE-114 закрыта только для exact second JNZ; parser и COM1 остаются не достигнуты. Срез ожидает независимое ревью.

#### S7-04 CPU-profile slice: exact Rust-entry `LEA RDI,[RSP+8]`

В base `PagedCpl0ProbeV1` добавлена только форма `48 8D 7C 24 08`: qword LEA в RDI из `[RSP+8]`, exact RSP SIB base, без индекса, scale 1, displacement 8, только REX.W и длина 5. Использован `MemoryOperand::effective_address`, без чтения гостевой памяти; меняются только RDI и RIP, flags/definedness сохраняются. Также проверено, что noncanonical effective address сам по себе не fault для LEA.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 141 passed; full state, flags/definedness, отсутствие data access, fetch fault на disp8 через page boundary, neighbors, CPL3 и stack/CR3/CR0.WP isolation. Образ `/private/tmp/od-fw-s7-lea-rsp.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-lea-rsp.bin --com1-out /private/tmp/od-fw-s7-lea-rsp.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 190 }` (`0xbe`), `retired=16`, `RIP=0x200202`, `com1_bytes=0`. Следующие bytes: `BE 00 00 10 00` (`MOV ESI,0x00100000`, handoff GPA zero-extended to RSI); следующая инструкция `BA 00 00 01 00` загружает размер handoff block (64 KiB) в EDX; открыта [ISSUE-116](../KNOWN_ISSUES.md#issue-116-s7-04-rust-entry-mov-esi-imm32-unsupported). ISSUE-115 закрыта только для exact LEA; parser и COM1 остаются не достигнуты. Срез ожидает независимое ревью.

#### S7-04 CPU-profile slice: exact Rust-entry `MOV ESI,0x00100000`

В base `PagedCpl0ProbeV1` допущен только `BE 00 00 10 00`: dword immediate `0x00100000` записывается в ESI и архитектурно обнуляет старшие 32 бита RSI. Остальное CPU state, включая flags/definedness, сохраняется. Другие immediate/register/width/prefix формы, CPL3 и прочие paged profiles не расширены.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 145 passed; full state и zero-extension, flags/definedness, neighbors, fetch fault на последнем immediate byte через границу страницы, CPL3 и stack/CR3/CR0.WP isolation. Образ `/private/tmp/od-fw-s7-mov-esi.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-mov-esi.bin --com1-out /private/tmp/od-fw-s7-mov-esi.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 186 }` (`0xba`), `retired=17`, `RIP=0x200207`, `com1_bytes=0`. Следующие bytes: `BA 00 00 01 00` (`MOV EDX,0x00010000`, размер handoff block 64 KiB); ISSUE-116 решена только для exact `MOV ESI`, открыт [ISSUE-117](../KNOWN_ISSUES.md#issue-117-s7-04-rust-entry-mov-edx-imm32-unsupported). Полная проверка handoff ещё не завершена; COM1 не достигнут. Срез ожидает независимое ревью.

#### S7-04 CPU-profile slice: exact Rust-entry `MOV EDX,0x00010000`

В base `PagedCpl0ProbeV1` допущен только `BA 00 00 01 00`: dword immediate `0x00010000` записывается в EDX и архитектурно обнуляет старшие 32 бита RDX. Остальное CPU state, включая flags/definedness, сохраняется. Другие immediate/register/width/prefix формы, CPL3 и прочие paged profiles не расширены.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 149 passed; full state и zero-extension, flags/definedness, neighbors, fetch fault на последнем immediate byte через границу страницы, CPL3 и stack/CR3/CR0.WP isolation. Образ `/private/tmp/od-fw-s7-mov-edx.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-mov-edx.bin --com1-out /private/tmp/od-fw-s7-mov-edx.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 49 }` (`0x31`), `retired=18`, `RIP=0x20020c`, `com1_bytes=0`. Следующие bytes: `31 C9` (`XOR ECX,ECX`); ISSUE-117 решена только для exact загрузки размера в EDX, открыт [ISSUE-118](../KNOWN_ISSUES.md#issue-118-s7-04-rust-entry-xor-ecx-ecx-unsupported). Полная проверка handoff ещё не завершена; COM1 не достигнут. Срез ожидает независимое ревью.

#### S7-04 CPU-profile slice: exact Rust-entry `XOR ECX,ECX`

К exact `31 C0` в base `PagedCpl0ProbeV1` добавлена только форма `31 C9`. Переиспользуется `LogicKind::Xor`/`logic_result`; ECX обнуляется с zero-extension в RCX, CF/OF очищаются, ZF/PF устанавливаются, SF очищается, а AF сохраняет concrete value при снятии definedness. Остальное CPU state сохраняется; соседние формы и прочие profiles не расширены.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 150 passed; full state/zero-extension, flags/AF definedness (AF исходно defined), neighbors, cross-page fetch fault на ModRM без commit, CPL3 и stack/CR3/CR0.WP isolation. Образ `/private/tmp/od-fw-s7-xor-ecx.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-xor-ecx.bin --com1-out /private/tmp/od-fw-s7-xor-ecx.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 232 }` (`0xe8`), `retired=19`, `RIP=0x20020e`, `com1_bytes=0`. Следующие bytes: `E8 8D 02 00 00` (`CALL rel32`, target `0x2004a0`); ISSUE-118 решена только для exact XOR ECX, открыт [ISSUE-119](../KNOWN_ISSUES.md#issue-119-s7-04-rust-entry-call-rel32-unsupported). Полная проверка handoff ещё не завершена; COM1 не достигнут. Срез ожидает независимое ревью.

#### S7-04 CPU-profile slice: second exact Rust-entry `CALL rel32`

В base `PagedCpl0ProbeV1` к firmware CALL с displacement `+0x183` добавлена только вторая форма `E8 8D 02 00 00` с displacement `+0x28d`. Переиспользуются canonical-target check и существующий stack push: return RIP `0x200213` записывается на стек до перехода на target `0x2004a0`. Любой fetch/stack fault не коммитит CPU state; другие displacement и profiles не расширены.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 155 passed; target/return-address push, stack write PF, noncanonical RSP (#SS), noncanonical target priority over bad stack (#GP), cross-page fetch fault, neighboring displacements, CPL3 и stack/CR3/CR0.WP isolation. Образ `/private/tmp/od-fw-s7-call-rust.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-call-rust.bin --com1-out /private/tmp/od-fw-s7-call-rust.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 86 }` (`0x56`), `retired=20`, `RIP=0x2004a0`, `com1_bytes=0`. Target bytes `41 56` (`PUSH R14`); ISSUE-119 решена только для двух exact calls, открыт [ISSUE-120](../KNOWN_ISSUES.md#issue-120-s7-04-rust-entry-push-r14-unsupported). Полная проверка handoff ещё не завершена; COM1 не достигнут. Срез ожидает независимое ревью.

#### S7-04 CPU-profile slice: exact Rust-helper `PUSH R14`

В base `PagedCpl0ProbeV1` допущена только `41 56`. Переиспользуется `StackRegister`/`push_stack_value`: сохраняется qword R14, RSP уменьшается на 8 только после успешной stack write, затем продвигается RIP. Flags и другие registers сохраняются; neighbors и другие profiles не расширены.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 161 passed; full state/RSP, qword stack bytes, stack PF/SS, fetch fault на opcode `56` после префикса REX `41`, neighboring registers/prefixes и CPL3/stack/CR3/CR0.WP isolation. Образ `/private/tmp/od-fw-s7-push-r14.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-push-r14.bin --com1-out /private/tmp/od-fw-s7-push-r14.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 83 }` (`0x53`), `retired=21`, `RIP=0x2004a2`, `com1_bytes=0`. Следующий байт `53` — `PUSH RBX`; ISSUE-120 решена только для `41 56`, открыт [ISSUE-121](../KNOWN_ISSUES.md#issue-121-s7-04-rust-entry-push-rbx-unsupported). Полная проверка handoff ещё не завершена; COM1 не достигнут.

#### S7-04 CPU-profile slice: exact Rust-helper `PUSH RBX`

В base `PagedCpl0ProbeV1` допущена только bare форма `53`. Переиспользуется существующая stack-register push семантика: исходный RBX записывается qword на `[RSP-8]`, RSP/RIP обновляются после успешной записи. Flags и другие registers сохраняются; формы с REX, иной register/width и другие profiles не расширены.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 167 passed; full state/RSP, все qword bytes, stack PF/SS, fetch fault на opcode, соседние REX/opcode/width forms и CPL3/stack/CR3/CR0.WP isolation. Образ `/private/tmp/od-fw-s7-push-rbx.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-push-rbx.bin --com1-out /private/tmp/od-fw-s7-push-rbx.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 129 }` (`0x81`), `retired=22`, `RIP=0x2004a3`, `com1_bytes=0`. Следующие bytes: `48 81 EC B8 00 00 00` (`SUB RSP,0xb8`); ISSUE-121 решена только для bare `53`, открыт [ISSUE-122](../KNOWN_ISSUES.md#issue-122-s7-04-rust-helper-sub-rsp-imm32-unsupported). Полная проверка handoff ещё не завершена; COM1 не достигнут.


#### S7-04 CPU-profile slice: exact Rust-helper `SUB RSP,0xb8`

В base `PagedCpl0ProbeV1` допущена только форма `48 81 EC B8 00 00 00`: qword SUB RSP, imm32 `0xb8`. Переиспользуется существующая арифметическая семантика SUB и обновление arithmetic flags/definedness. Это регистровая операция без stack access и canonicality check для результирующего RSP. Соседние immediate/opcode/register/width/prefix forms и прочие profiles остаются unsupported.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 172 passed; full state, flags/definedness, noncanonical result, cross-page immediate fetch-fault atomicity, соседние формы и CPL3/stack/CR3/CR0.WP isolation. Образ `/private/tmp/od-fw-s7-sub-rsp-b8.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-sub-rsp-b8.bin --com1-out /private/tmp/od-fw-s7-sub-rsp-b8.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 137 }` (`0x89`), `retired=23`, `RIP=0x2004aa`, `com1_bytes=0`. Байты по адресу — `48 89 FB` (`MOV RBX,RDI`), открыт [ISSUE-123](../KNOWN_ISSUES.md#issue-123-s7-04-rust-helper-mov-rbx-rdi-unsupported). Полная проверка handoff ещё не завершена; COM1 не достигнут. Срез ожидает независимое ревью.


#### S7-04 CPU-profile slice: exact Rust-helper `MOV RBX,RDI`

В base `PagedCpl0ProbeV1` допущена только точная qword форма `48 89 FB`: RBX получает исходный RDI. Используется существующая MOV semantics; флаги и definedness не меняются. Добавлены rejection соседних register/direction/width/prefix/memory кодировок, fetch fault на ModRM через границу страницы и CPL3/других paged profiles.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 176 passed. Образ `/private/tmp/od-fw-s7-mov-rbx-rdi.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-mov-rbx-rdi.bin --com1-out /private/tmp/od-fw-s7-mov-rbx-rdi.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 141 }` (`0x8d`), `retired=24`, `RIP=0x2004ad`, `com1_bytes=0`. Байты по адресу — `48 8D 7C 24 50` (`LEA RDI,[RSP+0x50]`), открыт [ISSUE-124](../KNOWN_ISSUES.md#issue-124-s7-04-rust-helper-lea-rdi-rsp-disp8-unsupported). Полная проверка handoff ещё не завершена; COM1 не достигнут. Срез ожидает независимое review.


#### S7-04 CPU-profile slice: exact Rust-helper `LEA RDI,[RSP+0x50]`

Exact stack-relative LEA allowlist в base `PagedCpl0ProbeV1` расширен с displacement `0x08` на две формы: `48 8D 7C 24 08` и `48 8D 7C 24 50`. Для обеих используется existing effective-address semantics без чтения памяти; canonicality результата не проверяется, flags/definedness остаются прежними. Другие displacements и profiles не допускаются.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 176 passed; обе exact формы, canonical/noncanonical effective addresses, full state/flags, отсутствие memory access, fetch faults, neighbors и CPL3/other-profile isolation. Образ `/private/tmp/od-fw-s7-lea-rdi-rsp50.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-lea-rdi-rsp50.bin --com1-out /private/tmp/od-fw-s7-lea-rdi-rsp50.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 232 }` (`0xe8`), `retired=25`, `RIP=0x2004b2`, `com1_bytes=0`. Байты — `E8 59 08 00 00` (`CALL rel32`, target `0x200d10`), открыт [ISSUE-125](../KNOWN_ISSUES.md#issue-125-s7-04-rust-helper-call-rel32-unsupported). Полная проверка handoff ещё не завершена; COM1 не достигнут. Срез ожидает независимое review.


#### S7-04 CPU-profile slice: exact Rust-helper `CALL rel32`

Existing exact CALL allowlist расширен третьей формой `E8 59 08 00 00` (displacement `+0x859`) в base `PagedCpl0ProbeV1`. Переиспользуются canonical-target check и stack push semantics: из `0x2004b2` CALL сохраняет return RIP `0x2004b7` и переходит на `0x200d10` только после успешной записи. Другие displacement и profiles остаются unsupported.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 178 passed; target/return-address push, stack #PF/#SS, noncanonical target #GP priority over bad stack, fetch fault, adjacent displacements и CPL3/other-profile isolation. Образ `/private/tmp/od-fw-s7-call-859.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-call-859.bin --com1-out /private/tmp/od-fw-s7-call-859.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 137 }` (`0x89`), `retired=26`, `RIP=0x200d10`, `com1_bytes=0`. Bytes at target — `48 89 F8` (`MOV RAX,RDI`), открыт [ISSUE-126](../KNOWN_ISSUES.md#issue-126-s7-04-rust-helper-mov-rax-rdi-unsupported). Полная проверка handoff ещё не завершена; COM1 не достигнут. Срез ожидает независимое review.


#### S7-04 CPU-profile slice: exact Rust-helper `MOV RAX,RDI`

В base `PagedCpl0ProbeV1` допущена только exact qword register form `48 89 F8`: RAX получает исходный RDI. Используется существующая MOV-семантика; flags/definedness и остальные registers сохраняются. Соседние register/direction/width/prefix/memory формы, CPL3 и другие profiles остаются unsupported.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 182 passed; full state, flags/definedness, fetch fault на ModRM через границу страницы, neighbors и CPL3/other-profile isolation. Образ `/private/tmp/od-fw-s7-mov-rax-rdi.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-mov-rax-rdi.bin --com1-out /private/tmp/od-fw-s7-mov-rax-rdi.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 129 }` (`0x81`), `retired=27`, `RIP=0x200d13`, `com1_bytes=0`. Следующие bytes — `48 81 FA 9F 00 00 00` (`CMP RDX,0x9f`), открыт [ISSUE-127](../KNOWN_ISSUES.md#issue-127-s7-04-rust-helper-cmp-rdx-imm32-unsupported). Полная проверка handoff ещё не завершена; COM1 не достигнут. Срез ожидает независимое review.


#### S7-04 CPU-profile slice: exact Rust-helper `CMP RDX,0x9f`

В base `PagedCpl0ProbeV1` допущена только форма `48 81 FA 9F 00 00 00` (`CMP RDX,sign-extended imm32 0x9f`). Переиспользуется существующая qword CMP semantics: RDX остаётся без изменений, flags и definedness получают ожидаемый результат. Другие immediate/register/width/prefix формы, CPL3 и другие profiles не допускаются.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 186 passed; equality и upper-qword сравнение, full state/flags/definedness, immediate fetch fault, sign-extended negative neighbor, остальные neighbors и CPL3/other-profile isolation. Образ `/private/tmp/od-fw-s7-cmp-rdx-9f.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-cmp-rdx-9f.bin --com1-out /private/tmp/od-fw-s7-cmp-rdx-9f.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 119 }` (`0x77`), `retired=28`, `RIP=0x200d1a`, `com1_bytes=0`. Байты — `77 0E` (`JA/JNBE rel8`), открыт [ISSUE-128](../KNOWN_ISSUES.md#issue-128-s7-04-rust-helper-jcc-ja-rel8-unsupported). Полная проверка handoff ещё не завершена; COM1 не достигнут. Срез ожидает независимое review.


#### S7-04 CPU-profile slice: exact Rust-helper `JA/JNBE +0x0e`

В base `PagedCpl0ProbeV1` допускается только `77 0E` (`JA/JNBE rel8`, `ConditionCode::Above`) с displacement `+0x0e`. Переиспользуется shared conditional branch path: условие CF=0 и ZF=0, taken-target canonicality проверяется до commit, not-taken fall-through заранее не проверяется; flags/definedness не меняются.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 191 passed; таблица CF/ZF, undefined CF/ZF retention, taken/not-taken canonicality, fetch fault, neighbors и CPL3/other-profile isolation. Образ `/private/tmp/od-fw-s7-ja-rel8.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-ja-rel8.bin --com1-out /private/tmp/od-fw-s7-ja-rel8.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 191 }` (`0xbf`), `retired=29`, `RIP=0x200d2a`, `com1_bytes=0`. Bytes — `48 BF 4F 44 48 41 4E 44 4F 46` (`MOV RDI,imm64`), открыт [ISSUE-129](../KNOWN_ISSUES.md#issue-129-s7-04-rust-helper-mov-rdi-imm64-unsupported). Полная проверка handoff пока не завершена; COM1 не достигнут. Срез ожидает независимое review.


#### S7-04 CPU-profile slice: exact Rust-helper `MOV RDI,handoff magic`

В base `PagedCpl0ProbeV1` допущена только точная форма `48 BF 4F 44 48 41 4E 44 4F 46` (`MOV RDI,0x464f444e4148444f`). Переиспользуется `MovImmediate`; RDI получает qword magic, остальные GPR/RFLAGS сохраняются. Neighbor immediate bits, registers, widths, prefixes, CPL3 и другие profiles остаются unsupported.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 195 passed; full-state retention, zero и bit-flipped immediates, register/width/prefix neighbors, fetch fault на последнем immediate byte через границу страницы, CPL3 и other-profile isolation. Образ `/private/tmp/od-fw-s7-mov-rdi-magic.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-mov-rdi-magic.bin --com1-out /private/tmp/od-fw-s7-mov-rdi-magic.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 57 }` (`0x39`), `retired=30`, `RIP=0x200d34`, `com1_bytes=0`. Байты — `48 39 3E` (`CMP qword ptr [RSI],RDI`), открыт [ISSUE-130](../KNOWN_ISSUES.md#issue-130-s7-04-rust-helper-cmp-memory-handoff-magic-unsupported). Полная проверка handoff ещё не завершена. Срез ожидает независимое ревью.


#### S7-04 CPU-profile slice: exact Rust-helper `CMP qword ptr [RSI],RDI`

В base `PagedCpl0ProbeV1` допущена только `48 39 3E`: qword `[RSI]` читается и сравнивается с RDI существующими memory-read и CMP semantics; data operand и GPR не пишутся, page-walk A/D side effects остаются по paged memory contract, flags/definedness/RIP коммитятся после успешного чтения. Точная addressing form обязательна; neighbor operands, addressing modes, prefixes, CPL3 и другие profiles остаются unsupported.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 201 passed; arithmetic flags/definedness, read #PF atomicity, noncanonical address #GP before read, injected GuestMemory read fault без CPU-state commit (тестовый fixture не моделирует page-walk), addressing/register/width/prefix neighbors и profile/CPL isolation. Образ `/private/tmp/od-fw-s7-cmp-handoff-magic.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-cmp-handoff-magic.bin --com1-out /private/tmp/od-fw-s7-cmp-handoff-magic.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 117 }` (`0x75`), `retired=31`, `RIP=0x200d37`, `com1_bytes=0`. Байты — `75 38` (`JNZ +0x38`), открыт [ISSUE-131](../KNOWN_ISSUES.md#issue-131-s7-04-rust-helper-jnz-plus-0x38-unsupported). Полная проверка handoff ещё не завершена. Срез ожидает независимое ревью.


#### S7-04 CPU-profile slice: exact Rust-helper `JNZ +0x38`

В base `PagedCpl0ProbeV1` расширен только точный JNZ rel8 `75 38` наряду с `75 24` и `75 77`. Переиспользуется общий `ConditionCode::NotEqual` path; taken target проверяется на canonicality до commit, fall-through проверяется только следующим fetch. Flags/definedness и прочее CPU state сохраняются.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 201 passed; taken/not-taken, full state/flags/definedness, noncanonical taken #GP и noncanonical fall-through, fetch fault для всех трёх точных displacement, neighbors, CPL3/other-profile isolation. Образ `/private/tmp/od-fw-s7-jnz-38.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-jnz-38.bin --com1-out /private/tmp/od-fw-s7-jnz-38.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 85 }` (`0x55`), `retired=32`, `RIP=0x200d39`, `com1_bytes=0`. Байты — `55` (`PUSH RBP`), открыт [ISSUE-132](../KNOWN_ISSUES.md#issue-132-s7-04-rust-helper-push-rbp-unsupported). Полная проверка handoff ещё не завершена. Срез ожидает независимое ревью.


#### S7-04 CPU-profile slice: exact bare Rust-helper `PUSH RBP`

В base `PagedCpl0ProbeV1` допущен только bare opcode `55`. Переиспользуется существующий `push_stack_value`: qword RBP записывается на `[RSP-8]`, RSP/RIP коммитятся после успешной записи, flags/остальные GPR сохраняются. Neighbor opcodes/prefixes, CPL3 и другие paged profiles остаются unsupported.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 207 passed; full state/RSP/qword bytes, stack #PF/#SS и fetch atomicity, neighboring opcode/register/width/prefix forms, CPL3 и other-profile isolation. Образ `/private/tmp/od-fw-s7-push-rbp.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-push-rbp.bin --com1-out /private/tmp/od-fw-s7-push-rbp.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 87 }` (`0x57`, после REX `0x41`), `retired=33`, `RIP=0x200d3a`, `com1_bytes=0`. Байты — `41 57` (`PUSH R15`), открыт [ISSUE-133](../KNOWN_ISSUES.md#issue-133-s7-04-rust-helper-push-r15-unsupported). Полная проверка handoff ещё не завершена. Срез ожидает независимое ревью.


#### S7-04 CPU-profile slice: exact Rust-helper `PUSH R15`

В base `PagedCpl0ProbeV1` допускается только `41 57`. Используется существующий stack helper: qword R15 записывается на `[RSP-8]`, RSP/RIP обновляются после успешной записи; flags и остальные registers сохраняются. Соседние формы, CPL3 и другие paged profiles не расширены.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 213 passed; full state/RSP/qword bytes, stack #PF/#SS, fetch fault на opcode byte `57` после REX `41`, neighbors, CPL3/other-profile isolation. Образ `/private/tmp/od-fw-s7-push-r15.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-push-r15.bin --com1-out /private/tmp/od-fw-s7-push-r15.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 85 }` (`0x55`, после REX `0x41`), `retired=35`, `RIP=0x200d3e`, `com1_bytes=0`. Следующие байты — `41 55` (`PUSH R13`); открыт [ISSUE-134](../KNOWN_ISSUES.md#issue-134-s7-04-rust-helper-push-r13-unsupported). Полная проверка handoff не завершена. Срез ожидает независимое ревью.


#### S7-04 CPU-profile slice: exact Rust-helper `PUSH R13`

В base `PagedCpl0ProbeV1` допускается только `41 55`. Используется существующий stack helper: qword R13 записывается на `[RSP-8]`, RSP/RIP обновляются после успешной записи; flags и остальные registers сохраняются. Соседние формы, CPL3 и другие paged profiles не расширены.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 219 passed; full state/RSP/qword bytes, stack #PF/#SS, fetch fault на opcode byte `55` после REX `41`, neighbors, CPL3/other-profile isolation. Образ `/private/tmp/od-fw-s7-push-r13.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-push-r13.bin --com1-out /private/tmp/od-fw-s7-push-r13.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 84 }` (`0x54`, после REX `0x41`), `retired=36`, `RIP=0x200d40`, `com1_bytes=0`. Следующие байты — `41 54` (`PUSH R12`); открыт [ISSUE-135](../KNOWN_ISSUES.md#issue-135-s7-04-rust-helper-push-r12-unsupported). Полная проверка handoff не завершена. Срез ожидает независимое ревью.


#### S7-04 CPU-profile slice: exact Rust-helper `PUSH R12`

В base `PagedCpl0ProbeV1` допускается только `41 54`. Используется существующий stack helper: qword R12 записывается на `[RSP-8]`, RSP/RIP обновляются после успешной записи; flags и остальные registers сохраняются. Соседние формы, CPL3 и другие paged profiles не расширены.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 225 passed; full state/RSP/qword bytes, stack #PF/#SS, fetch fault на opcode byte `54` после REX `41`, neighbors, CPL3/other-profile isolation. Образ `/private/tmp/od-fw-s7-push-r12.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-push-r12.bin --com1-out /private/tmp/od-fw-s7-push-r12.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 131 }`, `retired=38`, `RIP=0x200d43`, `com1_bytes=0`. Байты — `48 83 ec 38` (`SUB RSP,0x38`); открыт [ISSUE-136](../KNOWN_ISSUES.md#issue-136-s7-04-rust-helper-sub-rsp-imm8-0x38-unsupported). Полная проверка handoff не завершена. Срез ожидает независимое ревью.


#### S7-04 CPU-profile slice: exact Rust-helper `SUB RSP,0x38`

Base `PagedCpl0ProbeV1` теперь допускает exact qword register-only `48 83 EC 38` наряду с существующим `48 83 EC 58`. Переиспользуется текущая arithmetic SUB semantics: RSP и arithmetic flags/definedness обновляются; операция не обращается к памяти и не проверяет canonicality нового RSP. Все прочие формы и профили остаются unsupported.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 228 passed; оба SUB RSP imm8 варианта проверены по full state/flags, noncanonical result без memory access, immediate fetch-fault atomicity, neighbors и CPL3/other-profile isolation. Образ `/private/tmp/od-fw-s7-sub-rsp-38.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-sub-rsp-38.bin --com1-out /private/tmp/od-fw-s7-sub-rsp-38.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 183 }` (`0xB7`), `retired=39`, `RIP=0x200d47`, `com1_bytes=0`. Байты — `0F B7 7E 08` (`MOVZX EDI,word ptr [RSI+8]`); открыт [ISSUE-137](../KNOWN_ISSUES.md#issue-137-s7-04-rust-helper-movzx-edi-word-rsi-plus-8-unsupported). Полная проверка handoff не завершена. Срез ожидает независимое ревью.


#### S7-04 CPU-profile slice: exact Rust-helper MOVZX word fields

В base `PagedCpl0ProbeV1` допускаются только `0F B7 7E 08` (`MOVZX EDI,[RSI+8]`) и `0F B7 5E 0A` (`MOVZX EBX,[RSI+0xA]`); unsigned `Operation::Extend` читает word через `read_operand`, записывает точный 32-bit destination с zero-extension, сохраняет flags и продвигает RIP после успешного чтения. `PagedProbeMemory` теперь пропускает tracked Word read для этого точно gated CPU instruction.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile --quiet` — 238 passed; обе destination/displacement пары, zero-extension/full state, flags, injected GuestMemory read-fault atomicity, noncanonical #GP, first-form fetch fault, neighbors, CPL3/other-profile isolation. Injected Memory fixture не доказывает page walk/cross-page; отдельно `cargo test --offline -p od-machine --test paged_probe_quantum paged_probe_handoff_field_reads_use_tracked_page_walk_and_zero_extend_registers -- --exact` подтвердил обе загрузки через реальный tracked 4-level page walk и A bits верхних уровней и leaf PTE кода/данных (без page crossing). Образ `/private/tmp/od-fw-s7-movzx-handoff-field.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-cmp-bx-cx.bin --com1-out /private/tmp/od-fw-s7-cmp-bx-cx.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 151 }` (`0x97`), `retired=44`, `RIP=0x200d58`, `com1_bytes=0`. Байты — `0F 97 C1` (`SETA CL`); открыт [ISSUE-142](../KNOWN_ISSUES.md#issue-142-s7-04-rust-helper-seta-cl-unsupported). Полная проверка handoff не завершена. Срез ожидает независимое ревью.

#### S7-04 CPU-profile slice: exact Rust-helper `CMP DI,1`

Base `PagedCpl0ProbeV1` admits only `66 83 FF 01`: 16-bit CMP of DI against sign-extended imm8 1. The shared arithmetic semantics ignore upper RDI bits, preserve every GPR, and update arithmetic flags and definedness.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_cmp_di_imm8 -- --nocapture` — 4 passed; full `paged_probe_profile` — 242 passed; same exact bytes remain unsupported in CPL3 and other paged profiles. Covered DI compare with nonzero upper RDI, flags/definedness, immediate fetch fault, negative sign-extended immediate and neighboring register/operation/width/prefix forms. Образ `/private/tmp/od-fw-s7-cmp-di.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner retired 44 instructions, исполнил `66 39 CB` (`CMP BX,CX`) и остановился на `0F 97 C1` (`SETA CL`, ISSUE-142, RIP `0x200d58`). Полная проверка handoff не завершена.

#### S7-04 CPU-profile slice: exact Rust-helper `JNZ +0x2A`

Base `PagedCpl0ProbeV1` now admits `75 2A` alongside its three previously allowed short branches. It reuses the existing JNZ semantics: ZF is read without changing flags or definedness, a taken target is checked for canonicality, and a not-taken fall-through is checked only on the next fetch.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile firmware_entry_jnz -- --nocapture` — 5 passed; full `paged_probe_profile` — 242 passed; other paged profiles and CPL3 still reject JNZ. Existing matrices cover both ZF outcomes, undefined ZF, taken-only canonicality, rel8 fetch fault atomicity, exact displacement neighbors, prefixes and other branch forms. Образ `/private/tmp/od-fw-s7-jnz-2a.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-cmp-bx-cx.bin --com1-out /private/tmp/od-fw-s7-cmp-bx-cx.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 151 }`, `retired=44`, `RIP=0x200d58`, `com1_bytes=0`; следующий blocker — [ISSUE-142](../KNOWN_ISSUES.md#issue-142-s7-04-rust-helper-seta-cl-unsupported). Полная проверка handoff не завершена.

#### S7-04 CPU-profile slice: exact Rust-helper `CMP BX,CX`

Base `PagedCpl0ProbeV1` admits only `66 39 CB` for this 16-bit register-register comparison. The decoded `39 /r` form compares r/m16 BX as the left operand with reg16 CX as the right operand; shared arithmetic CMP updates flags and definedness without changing registers.

Проверки: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_cmp_bx_cx -- --nocapture` — 5 passed; full `paged_probe_profile` — 247 passed; the form remains unsupported in CPL3 and other paged profiles. Covered differing upper 48-bit register values with equal low words, low-word `BX=0`/`CX=1` operand-order flags, full CPU state and flags/definedness, ModRM fetch fault atomicity, reversed operands/direction, other operations, widths and prefixes. Образ `/private/tmp/od-fw-s7-cmp-bx-cx.bin` собран (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner-команда `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-cmp-bx-cx.bin --com1-out /private/tmp/od-fw-s7-cmp-bx-cx.com1 --budget 10000` вернула `status=error`, `exit=unsupported`, `Unsupported { opcode: 151 }`, `retired=44`, `RIP=0x200d58`, `com1_bytes=0`; следующий blocker — [ISSUE-142](../KNOWN_ISSUES.md#issue-142-s7-04-rust-helper-seta-cl-unsupported). Полная проверка handoff не завершена.


#### S7-04 CPU-profile slice: exact Rust-helper `SETA CL`

Base `PagedCpl0ProbeV1` admits only `0F 97 C1`. `Operation::SetCondition` now retains raw ModRM because the architectural ignored reg field must be distinguished by the exact profile; shared SETcc execution is unchanged. `Above` reads CF/ZF, writes only CL, preserves other RCX bytes and RFLAGS, and leaves flag-definedness unchanged.

Checks: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_seta_cl -- --nocapture` — 4 passed. Decoder tests `setcc_decodes_byte_registers_memory_addresses_and_ignored_modrm_reg` and `setcc_prefix_boundaries_and_fetch_faults_are_explicit` passed. Coverage includes truth table with CF/ZF undefined, full-state preservation, final-byte fetch-fault atomicity, `0F 97 C9` and neighboring encodings, CPL3 and other paged profiles. Firmware image `/private/tmp/od-fw-s7-seta-cl.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner stopped after 45 retired instructions at `66 83 FB 03` (`CMP BX,3`, ISSUE-143), `RIP=0x200d5b`, `Unsupported { opcode: 131 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `CMP BX,3`

Base `PagedCpl0ProbeV1` now admits `66 83 FB 03` alongside the existing `66 83 FF 01`. Both are exact 16-bit register/immediate CMP pairings. The immediate byte is sign-extended by the decoder; CMP reads BX's low word, leaves registers unchanged and updates arithmetic flags/definedness.

Checks: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_cmp_bx_imm8 -- --nocapture` — 4 passed. Covered BX below/equal/above 3 with nonzero upper RBX, full-state preservation, flags/definedness, immediate fetch fault, negative sign-extension and neighboring operations/registers/widths/prefixes, CPL3 and other paged profiles. Firmware image `/private/tmp/od-fw-s7-cmp-bx-3.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner stopped after 46 retired instructions at `40 0F 93 C7` (`SETAE DIL`, ISSUE-144), `RIP=0x200d5f`, `Unsupported { opcode: 147 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `SETAE DIL`

Base `PagedCpl0ProbeV1` admits only `40 0F 93 C7`; the guard requires REX `40`, `AboveOrEqual`, raw ModRM `C7`, and DIL. Shared SETcc evaluation reads CF and writes only DIL, preserving the upper 56 bits of RDI, RFLAGS, and flag-definedness.

Checks: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_setae_dil -- --nocapture` — 4 passed; CF truth table, undefined CF, full state, final-byte fetch-fault atomicity, neighbor and ignored-ModRM.reg rejection, CPL3 and other paged profiles. Firmware image `/private/tmp/od-fw-s7-setae-dil.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner stopped after 47 retired instructions at `40 08 CF` (`OR DIL,CL`, ISSUE-145), `RIP=0x200d63`, `Unsupported { opcode: 8 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `OR DIL,CL`

Base `PagedCpl0ProbeV1` admits only `40 08 CF` and reuses `LogicKind::Or` byte semantics. The result updates only DIL; upper RDI and RCX are preserved. CF/OF clear, SF/ZF/PF reflect the byte result, and AF's concrete value is retained while its definedness is cleared.

Checks: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_or_dil_cl -- --nocapture` — 4 passed. Cases cover multiple 8-bit results/status flags, AF initially defined then undefined, full-state preservation, final-byte fetch-fault atomicity, unprefixed BH neighbor, neighboring source/direction/operation/width/REX forms, CPL3 and other paged profiles. Firmware image `/private/tmp/od-fw-s7-or-dil-cl.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner stopped after 48 retired instructions at `74 3B` (`JZ +0x3B`, ISSUE-146), `RIP=0x200d66`, `Unsupported { opcode: 116 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `JZ +0x3B`

Base `PagedCpl0ProbeV1` admits only `74 3B` for this short JZ. The shared branch path reads ZF without changing flags/definedness, validates canonicality only when taken, and defers fall-through fetch validation.

Checks: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_jz_rel8 -- --nocapture` — 5 passed; taken/not-taken with defined/undefined ZF, taken-only canonicality, rel8 fetch-fault atomicity, neighbors, CPL3 and other paged profiles. Firmware image `/private/tmp/od-fw-s7-jz-3b.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner took the branch to `0x200da3` and stopped after 49 retired instructions at `8B 4E 0C` (`MOV ECX,dword ptr [RSI+0xC]`, ISSUE-147), `Unsupported { opcode: 139 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper dword fields `MOV ECX,[RSI+0xC]` and `MOV ECX,[RSI+0x10]`

Base `PagedCpl0ProbeV1` admits only `8B 4E 0C` and `8B 4E 10`. `PagedProbeMemory` routes both four-byte accesses through tracked `AccessWidth::Dword`; successful reads zero-extend into RCX, and read faults leave CPU state unchanged.

Checks: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_mov_ecx_dword_handoff_field -- --nocapture` — 4 passed, covering both displacements: full-state/zero-extension, injected read-fault atomicity, neighboring offsets/registers/widths/prefixes, CPL3 and other paged profiles. Real integration `cargo test --offline -p od-machine --test paged_probe_quantum paged_probe_handoff_field_reads_use_tracked_page_walk_and_zero_extend_registers -- --exact` executes the `+0xC` and `+0x10` reads through four-level tables, checks final RCX and unchanged handoff memory, and verifies upper-level and code/data leaf Accessed bits; no cross-page behavior is claimed. Firmware image `/private/tmp/od-fw-s7-mov-ecx-dword-10.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner stopped after 54 retired instructions at `8D B9 60 FF FF FF` (`LEA EDI,[RCX-0xA0]`, ISSUE-152), `RIP=0x200db7`, `Unsupported { opcode: 141 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `CMP ECX,0xa0`

Base `PagedCpl0ProbeV1` admits only `81 F9 A0 00 00 00`. This is a dword compare, so high RCX bits are ignored; registers remain unchanged and shared CMP semantics update arithmetic flags and their definedness.

Checks: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_cmp_ecx_imm32 -- --nocapture` — 4 passed. Cases cover equal and greater ECX with nonzero upper RCX, full state/flags/definedness, neighboring immediate/register/width/prefix forms including the qword sign-extended encoding, last-immediate-byte fetch-fault atomicity, CPL3 and other paged profiles. Firmware image `/private/tmp/od-fw-s7-cmp-ecx-a0.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner stopped after 54 retired instructions at `8D B9 60 FF FF FF` (`LEA EDI,[RCX-0xA0]`, ISSUE-152), `RIP=0x200db7`, `Unsupported { opcode: 141 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `JNZ rel32 +0xD6`

Base `PagedCpl0ProbeV1` admits only `0F 85 D6 00 00 00` for this near JNZ. It reuses shared branch evaluation and signed rel32 target calculation; flags and definedness remain unchanged, and canonicality is checked only for a taken target.

Checks: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_jnz_rel32 -- --nocapture` — 4 passed. Tests cover taken/fall-through with defined and undefined ZF, taken-only canonicality, a fault on the last displacement byte, neighboring conditions/displacements/prefixes/branch opcodes, plus CPL3 and other-profile rejection. Firmware image `/private/tmp/od-fw-s7-jcc-near.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner retired 54 instructions; ZF was set, the branch fell through, the second dword load and `MOV EBP,ECX` retired, and execution stopped at `8D B9 60 FF FF FF` (`LEA EDI,[RCX-0xA0]`, ISSUE-152), `RIP=0x200db7`, `Unsupported { opcode: 141 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `MOV EBP,ECX`

Base `PagedCpl0ProbeV1` admits only `89 CD` (`MOV EBP,ECX`) as this 32-bit register move. Shared MOV semantics write only EBP and zero-extend the result into RBP, preserving flags and all other state.

Checks: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_mov_ebp_ecx -- --nocapture` — 4 passed: high RCX source bits ignored, RBP zero-extension, full-state/flags, ModRM fetch-fault atomicity, neighbor and other-register/direction/width/prefix/memory rejection, plus CPL3 and other profiles. Runner retired 54 instructions and stopped at `8D B9 60 FF FF FF` (`LEA EDI,[RCX-0xA0]`, ISSUE-152), `RIP=0x200db7`, `Unsupported { opcode: 141 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `LEA EDI,[RCX-0xA0]`

Base `PagedCpl0ProbeV1` admits only `8D B9 60 FF FF FF`. It calculates the effective address from RCX and signed disp32 `-0xA0`; the dword write to EDI zero-extends into RDI. The operation preserves flags and performs no data-memory access or canonicality check.

Checks: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_lea_edi_rcx_minus_a0 -- --nocapture` — 4 passed: signed displacement and 32-bit truncation, full-state/flags/definedness, noncanonical effective address without memory access, final disp32-byte fetch-fault atomicity, neighbors, CPL3 and other-profile rejection. The full profile suite passed 288 tests. Firmware image `/private/tmp/od-fw-s7-lea-edi-rcx.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner retired 55 instructions and stopped at `81 FF 61 FF 00 00` (`CMP EDI,0xff61`, ISSUE-153), `RIP=0x200dbd`, `Unsupported { opcode: 129 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `CMP EDI,0xff61`

Base `PagedCpl0ProbeV1` admits only `81 FF 61 FF 00 00`. Shared dword CMP semantics ignore upper RDI bits, preserve registers, and update arithmetic flags and definedness.

Checks: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_cmp_edi_imm32 -- --nocapture` — 4 passed, covering less/equal/greater, high RDI bits, full-state/flags/definedness, immediate neighbors, final immediate-byte fetch-fault atomicity, CPL3 and other-profile isolation. The full profile suite passed 292 tests. Firmware image `/private/tmp/od-fw-s7-cmp-edi-ff61.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner retired 56 instructions and stopped at `40 0F 92 C7` (`SETB DIL`, ISSUE-154), `RIP=0x200dc3`, `Unsupported { opcode: 146 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `SETB DIL`

Base `PagedCpl0ProbeV1` admits only `40 0F 92 C7`. The guard requires bare REX `40`, `ConditionCode::Below`, raw ModRM `C7`, and DIL; shared SETcc reads CF, writes only DIL, preserves upper RDI and RFLAGS, and does not change flag-definedness.

Checks: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_setb_dil -- --nocapture` — 4 passed, covering both CF outcomes with defined and undefined CF metadata, full-state/flags retention, ModRM fetch-fault atomicity, neighboring conditions/registers/prefixes/REX, CPL3 and other-profile isolation. The full profile suite passed 296 tests. Firmware image `/private/tmp/od-fw-s7-setb-dil.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner retired 57 instructions and stopped at `48 39 EA` (`CMP RDX,RBP`, ISSUE-155), `RIP=0x200dc7`, `Unsupported { opcode: 57 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `CMP RDX,RBP`

Base `PagedCpl0ProbeV1` admits only `48 39 EA`. The encoding compares RDX minus RBP, leaves both GPRs unchanged, and uses shared qword CMP flag/definedness semantics.

Checks: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_cmp_rdx_rbp -- --nocapture` — 4 passed: equal, less, greater and overflow-sensitive qword cases, operand-order flags, full-GPR/flags/definedness preservation, ModRM fetch-fault atomicity, neighboring encodings, CPL3 and other-profile rejection. The full profile suite passed 300 tests. Firmware image `/private/tmp/od-fw-s7-cmp-rdx-rbp.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner retired 58 instructions and stopped at `41 0F 93 C0` (`SETAE R8B`, ISSUE-156), `RIP=0x200dca`, `Unsupported { opcode: 147 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `SETAE R8B`

Base `PagedCpl0ProbeV1` admits only `41 0F 93 C0`. The guard requires REX `41`, `AboveOrEqual`, raw ModRM `C0`, and R8B; shared SETcc reads CF, writes only R8B, preserves the remaining register state and RFLAGS, and leaves flag-definedness unchanged.

Checks: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_setae_r8b -- --nocapture` — 4 passed: both CF outcomes with defined and undefined metadata, R8B-only byte write, RAX/AH and other GPR preservation, full flags/definedness, fetch-fault atomicity, neighbor/profile/CPL rejection. The full profile suite passed 304 tests. Firmware image `/private/tmp/od-fw-s7-setae-r8b.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner retired 59 instructions and stopped at `41 20 F8` (`AND R8B,DIL`, ISSUE-157), `RIP=0x200dce`, `Unsupported { opcode: 32 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `AND R8B,DIL`

Base `PagedCpl0ProbeV1` admits only `41 20 F8`, using existing byte AND semantics. Only R8B changes; CF/OF are cleared, SF/ZF/PF follow the result, and AF keeps its concrete value while becoming undefined.

Checks: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_and_r8b_dil -- --nocapture` — 4 passed; the full `paged_probe_profile` suite passed 308 tests. Coverage includes result/flags, initially-defined AF becoming undefined, full-state preservation, R8B-only write, final ModRM fetch-fault atomicity, neighbors, CPL3 and other-profile isolation. Firmware image `/private/tmp/od-fw-s7-and-r8b-dil.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner retired 60 instructions, then stopped at `40 F6 C5 07` (`TEST BPL,7`, ISSUE-158), `RIP=0x200dd1`, `Unsupported { opcode: 246 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `TEST BPL,7`

Base `PagedCpl0ProbeV1` admits only `40 F6 C5 07`, reusing byte `LogicKind::Test`. Registers remain unchanged; logic flags update and AF's concrete value is retained while its definedness is cleared.

Checks: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_test_bpl_imm8 -- --nocapture` — 4 passed; full profile suite — 312 passed. Tests cover seven BPL values, flags/definedness, register retention, immediate-byte fetch-fault atomicity, neighboring encodings, CPL3 and other-profile isolation. Firmware image `/private/tmp/od-fw-s7-test-bpl-7.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner retired 61 instructions, then stopped at `40 0F 94 C7` (`SETZ DIL`, ISSUE-159), `RIP=0x200dd5`, `Unsupported { opcode: 148 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `SETZ DIL`

Base `PagedCpl0ProbeV1` admits only `40 0F 94 C7`, reusing shared SETcc `Equal` semantics. It writes only DIL, preserves upper RDI and flags, and leaves flag-definedness unchanged.

Checks: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_setz_dil -- --nocapture` — 4 passed; full profile suite — 316 passed. Coverage includes both ZF outcomes with defined and undefined metadata, full-state/flags preservation, exact REX/ModRM, fetch-fault atomicity, neighboring conditions/registers/prefixes, CPL3 and other-profile isolation. Firmware image `/private/tmp/od-fw-s7-setz-dil.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner retired 62 instructions then stopped at `41 84 F8` (`TEST R8B,DIL`, ISSUE-160), `RIP=0x200dd9`, `Unsupported { opcode: 132 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `TEST R8B,DIL`

Base `PagedCpl0ProbeV1` admits only `41 84 F8`, using shared byte TEST semantics. Registers remain unchanged, logical flags update, and AF becomes undefined.

Checks: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_test_r8b_dil -- --nocapture` — 4 passed; full profile suite — 320 passed. Tests cover several byte results, AF initially defined then undefined, full-state preservation, ModRM fetch-fault atomicity, neighboring encodings, CPL3 and other-profile isolation. Firmware image `/private/tmp/od-fw-s7-test-r8b-dil.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner retired 63 instructions, then stopped at `0F 84 B4 00 00 00` (`JZ rel32 +0xB4`, ISSUE-161), `RIP=0x200ddc`, `Unsupported { opcode: 132 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `JZ rel32 +0xB4`

Base `PagedCpl0ProbeV1` admits only `0F 84 B4 00 00 00`. Shared JZ semantics read ZF, preserve flags/definedness, calculate signed rel32 targets, and check canonicality only for taken branches.

Checks: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_jz_rel32 -- --nocapture` — 5 passed; full profile suite — 325 passed. Tests cover taken/not-taken and undefined ZF, taken-target canonicality, deferred fall-through validation, final displacement-byte fetch fault, neighbors, CPL3 and other-profile isolation. Firmware image `/private/tmp/od-fw-s7-jz-rel32-b4.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner retired 64 instructions; ZF was clear, so it fell through and stopped at `0F B7 CB` (`MOVZX ECX,BX`, ISSUE-162), `RIP=0x200de2`, `Unsupported { opcode: 183 }`, `com1_bytes=0`; full handoff validation remains incomplete.

#### S7-04 CPU-profile slice: exact Rust-helper `MOVZX ECX,BX`

Base `PagedCpl0ProbeV1` admits only `0F B7 CB`; it reuses unsigned register `Operation::Extend`, zero-extends BX into ECX/RCX, preserves flags and performs no memory access.

Checks: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_movzx_ecx_bx -- --nocapture` — 4 passed; full profile suite — 329 passed. Tests cover high-RCX zero-extension, full-state/flags retention, unsupported neighbors, ModRM fetch-fault atomicity, CPL3 and other-profile isolation. Firmware image `/private/tmp/od-fw-s7-movzx-ecx-bx.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner retired 65 instructions, then stopped at `49 B8 FC FF FF FF FF FF FF 7F` (`MOV R8,0x7ffffffffffffffc`, ISSUE-163), `RIP=0x200de5`, `Unsupported { opcode: 184 }`, `com1_bytes=0`; full handoff validation remains incomplete.

#### S7-04 CPU-profile slice: exact Rust-helper `MOV R8,imm64`

Base `PagedCpl0ProbeV1` admits only `49 B8 FC FF FF FF FF FF FF 7F` (`MOV R8,0x7ffffffffffffffc`). The existing MOV-immediate write changes only R8 and RIP; flags and all other state are preserved.

Checks: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_mov_r8_imm64 -- --nocapture` — 4 passed; full profile suite — 333 passed. Tests cover exact immediate/destination, full-state/flags preservation, neighboring encodings, final-immediate fetch-fault atomicity, CPL3 and other-profile isolation. Firmware image `/private/tmp/od-fw-s7-mov-r8-imm64.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner retired 66 instructions, then stopped at `8B 7E 14` (`MOV EDI,dword ptr [RSI+0x14]`, ISSUE-164), `RIP=0x200def`, `Unsupported { opcode: 139 }`, `com1_bytes=0`; full handoff validation remains incomplete.

#### S7-04 CPU-profile slice: exact `MOV EDI,dword ptr [RSI+0x14]`

Base `PagedCpl0ProbeV1` admits only `8B 7E 14`, a dword read from RSI plus exact disp8 `0x14` into EDI. The existing tracked read translates through four-level paging; EDI zero-extends into RDI and flags stay unchanged.

Checks: 5 focused `paged_probe_profile` tests and all 338 profile tests pass. The synthetic cases cover exact address width/destination, injected read fault, noncanonical address, neighbors and isolation. Two targeted `paged_probe_quantum` integration checks pass, including a standalone four-level walk that asserts Accessed bits in paging entries. Image `/private/tmp/od-fw-s7-mov-edi-dword-rsi-14.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner retired 67 instructions then stopped at `41 B9 FF FF FF FF` (`MOV R9D,0xffffffff`, ISSUE-165), `RIP=0x200df2`, `Unsupported { opcode: 185 }`, `com1_bytes=0`; full handoff validation remains incomplete.

#### S7-04 CPU-profile slice: exact Rust-helper `MOV R9D,0xffffffff`

Base `PagedCpl0ProbeV1` admits only `41 B9 FF FF FF FF`. The existing 32-bit MOV-immediate write zero-extends into R9 and preserves flags and other state.

Checks: 4 focused tests and all 342 `paged_probe_profile` tests pass. Coverage includes full-state/flags preservation, adjacent values/registers/widths/prefixes, final-immediate fetch-fault atomicity, CPL3 and other-profile isolation. Firmware image `/private/tmp/od-fw-s7-mov-r9d-imm32.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner retired 68 instructions then stopped at `45 31 D2` (`XOR R10D,R10D`, ISSUE-166), `RIP=0x200df8`, `Unsupported { opcode: 49 }`, `com1_bytes=0`; full handoff validation remains incomplete.

#### S7-04 CPU-profile slice: exact Rust-helper `XOR R10D,R10D`

Base `PagedCpl0ProbeV1` admits only `45 31 D2` (`XOR R10D,R10D`) with exact REX/ModRM, dword width and length. It reuses existing logical-XOR semantics: R10D zero-extends into R10; logical flags update, AF concrete value is retained but becomes undefined; unrelated CPU state is preserved.

Checks: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_xor_r10d -- --nocapture` — 2 passed; all `paged_probe_profile` tests — 344 passed; `cargo check --offline -p od-isa --all-targets` and scoped rustfmt passed. Tests cover exact result/full state, flags and AF definedness, ModRM fetch-fault atomicity, neighboring forms, CPL3 and other-profile isolation. `python3 tools/build/build_firmware.py --out /private/tmp/od-fw-s7-04-issue166.bin` built a 4 MiB image, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`. `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-issue166.bin --com1-out /private/tmp/od-fw-s7-04-issue166.com1 --budget 10000` retired 69 instructions and stopped at `45 31 DB` (`XOR R11D,R11D`, ISSUE-167), `RIP=0x200dfb`, `Unsupported { opcode: 49 }`, `com1_bytes=0`; full handoff validation remains incomplete.

#### S7-04 CPU-profile slice: exact Rust-helper `XOR R11D,R11D`

Base `PagedCpl0ProbeV1` additionally admits only `45 31 DB` (`XOR R11D,R11D`) with exact REX/ModRM, dword width and length. The existing logical-XOR semantics zero-extend R11D into R11, update arithmetic flags, preserve AF's concrete value while clearing its definedness, and preserve unrelated CPU state.

Checks: `cargo test --offline -p od-isa --test paged_probe_profile rust_helper_xor_r11d -- --nocapture` — 2 passed; full `paged_probe_profile` — 346 passed; `cargo check --offline -p od-isa --all-targets` and scoped rustfmt passed. Tests cover full state/result, flags and AF definedness, ModRM fetch-fault atomicity, neighboring forms, CPL3 and other-profile isolation. `python3 tools/build/build_firmware.py --out /private/tmp/od-fw-s7-04-issue167.bin` built a 4 MiB image (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-issue167.bin --com1-out /private/tmp/od-fw-s7-04-issue167.com1 --budget 10000` retired 70 instructions and stopped at `66 90` (`Unsupported { opcode: 144 }`), `RIP=0x200dfe`, `com1_bytes=0` (ISSUE-168); full handoff validation remains incomplete.

#### S7-04 CPU-profile slice: exact operand-size-prefixed NOP `66 90`

Base `PagedCpl0ProbeV1` admits only `66 90` (`NOP` / `XCHG AX,AX` encoding), with exact primary opcode, length, word size and operand-size-only prefix. The branch advances RIP by two and leaves all other CPU state, flags and definedness unchanged. The decoder and general JIT NOP policy remain untouched; bare `90` stays outside this paged-profile slice.

Checks: `cargo test --offline -p od-isa --test paged_probe_profile prefixed_nop -- --nocapture` — 4 passed; `other_paged_profiles_keep_s7_entry_instructions_unsupported` passed; JIT test `nop_admission_is_s0_09_plus_only_exact_bare_90_and_valid_boundaries` passed; full `paged_probe_profile` — 350 passed; `cargo check --offline -p od-isa --all-targets`, scoped rustfmt and `git diff --check` passed. Coverage includes full-state/flags/definedness retention, fetch-fault atomicity, neighboring encodings and CPL3/profile isolation. Image `/private/tmp/od-fw-s7-04-issue168.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner `cargo run --offline -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-issue168.bin --com1-out /private/tmp/od-fw-s7-04-issue168.com1 --budget 10000` retired 71 instructions then stopped at `46 0F B6 34 1E` (`MOVZX R14D,byte ptr [RSI+R11]`, ISSUE-169), `RIP=0x200e00`, `Unsupported { opcode: 182 }`, `com1_bytes=0`; full handoff validation remains incomplete.

#### S7-04 CPU-profile slice: exact Rust-helper `MOVZX R14D,byte ptr [RSI+R11]`

Base `PagedCpl0ProbeV1` admits only `46 0F B6 34 1E`. The exact REX/ModRM/SIB tuple selects a byte read from RSI+R11 into R14D; existing unsigned MOVZX/read semantics zero-extend R14D and preserve flags. `PagedProbeMemory` now supports one-byte tracked reads. No generic MOVZX or new profile is introduced.

Checks: `cargo test --offline -q -p od-isa --test paged_probe_profile` — 356 passed; targeted `paged_probe_movzx_r14d_byte_rsi_r11_uses_tracked_four_level_walk` passed. Synthetic tests cover full state/flags, zero-extension, injected read fault, noncanonical address, SIB fetch-fault atomicity, neighboring encodings, CPL3 and other-profile isolation. The four-level integration verifies accessed bits through the data leaf PTE; it does not assert cross-page behavior. `cargo check --offline -q -p od-isa -p od-machine --all-targets` passed; rustfmt check on the four changed Rust files and `git diff --check` passed. Workspace `cargo fmt --all -- --check` reports an unrelated existing formatting diff in `crates/od-host/src/storage_abi_v1.rs:139`, which was not changed. Image `/private/tmp/od-fw-s7-04-issue169.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner command `cargo run --offline -q -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-issue169.bin --com1-out /private/tmp/od-fw-s7-04-issue169.com1 --budget 10000` retired 72 instructions, then stopped at `4D 89 DF` (`MOV R15,R11`, ISSUE-170), `RIP=0x200e05`, `Unsupported { opcode: 137 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `MOV R15,R11`

Base `PagedCpl0ProbeV1` admits only `4D 89 DF`, the qword register transfer R15←R11 with exact REX/opcode/ModRM and length. The existing MOV semantics copy the source to R15 and advance RIP; flags and other state remain unchanged.

Checks: four focused `rust_helper_mov_r15_r11` tests passed; `other_paged_profiles_keep_s7_entry_instructions_unsupported` passed; all 360 `paged_probe_profile` tests passed. Tests cover full-state and flag retention, fetch fault on ModRM, neighboring register/direction/width/prefix/memory forms, CPL3 and other-profile isolation. `cargo check --offline -p od-isa --all-targets` and scoped rustfmt passed. Image `/private/tmp/od-fw-s7-04-issue170.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner `cargo run --offline -q -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-issue170.bin --com1-out /private/tmp/od-fw-s7-04-issue170.com1 --budget 10000` retired 73 instructions and stopped at `4D 21 C7` (`AND R15,R8`, ISSUE-171), `RIP=0x200e08`, `Unsupported { opcode: 33 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `AND R15,R8`

Base `PagedCpl0ProbeV1` admits only `4D 21 C7`, the qword logical AND R15←R15 AND R8 with exact REX/opcode/ModRM and length. Existing logical-AND semantics are reused: R15 alone is written; CF and OF clear; ZF/PF/SF track the result; AF becomes undefined.

Checks: four focused `rust_helper_and_r15_r8` tests passed; `other_paged_profiles_keep_s7_entry_instructions_unsupported` passed; all 364 `paged_probe_profile` tests passed. Coverage includes result and full-state vectors, flags/AF-definedness, cross-page ModRM fetch-fault atomicity, neighboring pairs/directions/widths/prefixes/memory, CPL3 and other-profile isolation. `cargo check --offline -p od-isa --all-targets`, scoped rustfmt and `git diff --check` passed. Image `/private/tmp/od-fw-s7-04-issue171.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner `cargo run --offline -q -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-issue171.bin --com1-out /private/tmp/od-fw-s7-04-issue171.com1 --budget 10000` retired 74 instructions and stopped at `45 89 CC` (`MOV R12D,R9D`, ISSUE-172), `RIP=0x200e0b`, `Unsupported { opcode: 137 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `MOV R12D,R9D`

Base `PagedCpl0ProbeV1` admits only `45 89 CC`, the dword register transfer R12D←R9D with exact REX/opcode/ModRM and length. Existing MOV semantics zero-extend the write into R12 and preserve flags and unrelated state.

Checks: four focused `rust_helper_mov_r12d_r9d` tests passed; `other_paged_profiles_keep_s7_entry_instructions_unsupported` passed; all 368 `paged_probe_profile` tests passed. Coverage includes source high bits ignored, destination zero-extension, full-state/flag retention, ModRM fetch-fault atomicity, neighboring register/direction/width/prefix/memory forms, CPL3 and other-profile isolation. `cargo check --offline -p od-isa --all-targets`, scoped rustfmt and `git diff --check` passed. Image `/private/tmp/od-fw-s7-04-issue172.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner `cargo run --offline -q -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-issue172.bin --com1-out /private/tmp/od-fw-s7-04-issue172.com1 --budget 10000` retired 75 instructions and stopped at `41 C1 EC 08` (`SHR R12D,8`, ISSUE-173), `RIP=0x200e0e`, `Unsupported { opcode: 193 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `SHR R12D,8`

Base `PagedCpl0ProbeV1` admits only `41 C1 EC 08`, the dword logical right shift of R12D by the exact immediate count 8. Existing shift/count semantics are reused; R12D zero-extends into R12, and CF/SF/ZF/PF become defined while AF/OF concrete bits are preserved but undefined.

Checks: four focused `rust_helper_shr_r12d_8` tests passed; `other_paged_profiles_keep_s7_entry_instructions_unsupported` passed; all 372 `paged_probe_profile` tests passed. Coverage includes shift results, flags/count definedness, AF/OF concrete retention, full-state zero-extension, final-immediate fetch-fault atomicity, adjacent count/destination/operation/prefix/memory forms, CPL3 and other-profile isolation. `cargo check --offline -p od-isa --all-targets`, scoped rustfmt and `git diff --check` passed. Image `/private/tmp/od-fw-s7-04-issue173.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner `cargo run --offline -q -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-issue173.bin --com1-out /private/tmp/od-fw-s7-04-issue173.com1 --budget 10000` retired 76 instructions and stopped at `49 83 FF 14` (`CMP R15,0x14`, ISSUE-174), `RIP=0x200e12`, `Unsupported { opcode: 131 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `CMP R15,0x14`

Base `PagedCpl0ProbeV1` admits only `49 83 FF 14` (`CMP R15,0x14`). Opcode `83` sign-extends the immediate byte to qword width; the existing CMP arithmetic semantics update flags while retaining all GPRs.

Checks: four focused `rust_helper_cmp_r15_imm8` tests passed; `other_paged_profiles_keep_s7_entry_instructions_unsupported` passed; all 376 `paged_probe_profile` tests passed. Coverage includes equality, borrow, non-borrow, high qword bits, signed overflow, flags/definedness and full-state retention, final-immediate fetch-fault atomicity, neighboring register/operation/immediate/width/prefix/memory forms, CPL3 and other-profile isolation. `cargo check --offline -p od-isa --all-targets`, scoped rustfmt and `git diff --check` passed. Image `/private/tmp/od-fw-s7-04-issue174.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner `cargo run --offline -q -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-issue174.bin --com1-out /private/tmp/od-fw-s7-04-issue174.com1 --budget 10000` retired 77 instructions and stopped at `45 0F 44 F2` (`CMOVZ R14D,R10D`, ISSUE-175), `RIP=0x200e16`, `Unsupported { opcode: 68 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `CMOVZ R14D,R10D`

Base `PagedCpl0ProbeV1` admits only `45 0F 44 F2`. REX.R/B encode R14D and R10D; the existing CMOVZ condition uses concrete ZF. On the true path R10D is written; on the false path the old low R14D is retained. Both dword paths zero-extend into R14. Flags and `rflags_defined` are unchanged, including when ZF is marked undefined.

Checks: four focused `rust_helper_cmovz_r14d_r10d` tests passed; `other_paged_profiles_keep_s7_entry_instructions_unsupported` passed; all 380 `paged_probe_profile` tests passed. Coverage includes true/false ZF with defined/undefined metadata, full-state/flags retention, destination zero-extension, final-ModRM fetch-fault atomicity, neighboring condition/register/width/prefix/memory forms and CPL/profile isolation. `cargo check --offline -p od-isa --all-targets`, scoped rustfmt and `git diff --check` passed. Image `/private/tmp/od-fw-s7-04-issue175.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner `cargo run --offline -q -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-issue175.bin --com1-out /private/tmp/od-fw-s7-04-issue175.com1 --budget 10000` retired 78 instructions and stopped at `46 0F B6 7C 1E 01` (`MOVZX R15D,byte ptr [RSI+R11+1]`, ISSUE-176), `RIP=0x200e1a`, `Unsupported { opcode: 182 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `MOVZX R15D,byte ptr [RSI+R11+1]`

The base paged profile now admits this one signed disp8 byte-read tuple in addition to the earlier R14D no-displacement form. REX `46` selects R15D and R11; ModRM/SIB select `[RSI+R11+1]`; `read_operand` keeps the access tracked and faulting.

Checks: five focused `rust_helper_movzx_r15d_rsi_r11_plus_one` tests passed; all 385 `paged_probe_profile` tests and all 33 `paged_probe_quantum` tests passed. Coverage includes zero-extension/full-state/flag retention, injected read fault, fetch fault on the final displacement byte, neighboring positive/negative displacements and register/REX/prefix/width/address forms, plus CPL/profile isolation. A real four-level machine integration test passed and checks the code/data leaf-PTE Accessed bits and R15D value. `cargo check --offline -p od-isa --all-targets`, scoped rustfmt and `git diff --check` passed. Image `/private/tmp/od-fw-s7-04-issue176.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner `cargo run --offline -q -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-issue176.bin --com1-out /private/tmp/od-fw-s7-04-issue176.com1 --budget 10000` retired 79 instructions and stopped at `45 0F 44 FA` (`CMOVZ R15D,R10D`, ISSUE-177), `RIP=0x200e20`, `Unsupported { opcode: 68 }`, `com1_bytes=0`; full handoff validation remains incomplete.

#### S7-04 CPU-profile slice: exact Rust-helper `CMOVZ R15D,R10D`

The base paged profile admits only `45 0F 44 FA` for this additional tuple. Exact REX `45` with ModRM `FA` selects R15D/R10D; shared CMOV semantics use concrete ZF, preserve flag values and definedness, and zero-extend R15D even on the false path.

Checks: four focused `rust_helper_cmovz_r15d_r10d` tests and other-profile isolation passed; all 389 `paged_probe_profile` tests and `cargo check --offline -p od-isa --all-targets` passed. Coverage includes taken/not-taken with defined/undefined ZF, full-state/flag retention, false-path zero-extension, final-ModRM fetch fault, neighbors and CPL/profile isolation. Scoped rustfmt and `git diff --check` passed. Image `/private/tmp/od-fw-s7-04-issue177.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner `cargo run --offline -q -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-issue177.bin --com1-out /private/tmp/od-fw-s7-04-issue177.com1 --budget 10000` retired 80 instructions and stopped at `45 0F B6 C9` (`MOVZX R9D,R9B`, ISSUE-178), `RIP=0x200e24`, `Unsupported { opcode: 182 }`, `com1_bytes=0`; full handoff validation remains incomplete.

#### S7-04 CPU-profile slice: exact Rust-helper `MOVZX R9D,R9B`

The base paged profile admits only `45 0F B6 C9` for this register pair. REX.R and REX.B with ModRM `C9` select R9D and its low-byte alias R9B. Existing unsigned register `Extend` semantics read R9B, write R9D, zero-extend R9 and preserve flags.

Checks: four focused `rust_helper_movzx_r9d_r9b` tests and other-profile isolation passed; all 393 `paged_probe_profile` tests and `cargo check --offline -p od-isa --all-targets` passed. Coverage includes low-byte alias mapping, dword zero-extension, full state/flags/definedness, final-ModRM fetch-fault atomicity, neighboring aliases/prefixes and CPL/profile isolation. Scoped rustfmt and `git diff --check` passed. Image `/private/tmp/od-fw-s7-04-issue178.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner `cargo run --offline -q -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-issue178.bin --com1-out /private/tmp/od-fw-s7-04-issue178.com1 --budget 10000` retired 81 instructions and stopped at `45 31 F1` (`XOR R9D,R14D`, ISSUE-179), `RIP=0x200e28`, `Unsupported { opcode: 49 }`, `com1_bytes=0`; full handoff validation remains incomplete.

#### S7-04 CPU-profile slice: exact Rust-helper `XOR R9D,R14D`

The base paged profile adds only `45 31 F1`. Opcode `31` writes the ModRM r/m destination, so R9D is XORed with the ModRM reg source R14D. Shared XOR flags apply; the R9D result zero-extends R9 and AF becomes undefined while its concrete bit is preserved.

Checks: three focused `rust_helper_xor_r9d_r14d` tests, CPL3 and other-profile isolation, all 396 `paged_probe_profile` tests, and `cargo check --offline -p od-isa --all-targets` passed. Tests verify result/operand routing, full state, flags/definedness, zero-extension, AF definedness, final-ModRM fetch-fault atomicity and neighboring forms. Scoped rustfmt and `git diff --check` passed. Image `/private/tmp/od-fw-s7-04-issue179.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner `cargo run --offline -q -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-issue179.bin --com1-out /private/tmp/od-fw-s7-04-issue179.com1 --budget 10000` retired 82 instructions and stopped at `46 33 24 8D 80 26 20 00` (`XOR R12D,dword ptr [R9*4+0x202680]`, ISSUE-180), `RIP=0x200e2b`, `Unsupported { opcode: 51 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact SIB memory XOR (ISSUE-180)

The base paged profile now admits only `46 33 24 8D 80 26 20 00`, decoded as `XOR R12D,dword ptr [R9*4+0x202680]`. The exact REX/opcode/length/destination and SIB/index/scale/displacement/address-size/operand-size tuple is guarded. The operation reads through the existing tracked dword operand path before modifying CPU state; shared XOR result and flag semantics apply, including R12 zero-extension and AF undefinedness.

Checks: six focused `rust_helper_xor_r12d_sib` tests and the real tracked four-level test `paged_probe_xor_r12d_sib_dword_uses_tracked_four_level_walk` passed. Tests cover exact effective address and width, full-state and flag behavior, synthetic read fault and final displacement fetch fault, neighboring forms, CPL3/other-profile isolation, and Accessed bits through code/data leaf PTEs. Firmware image `/private/tmp/od-fw-s7-04-issue180.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner retired 83 and stopped at `45 0F B6 F4` (`MOVZX R14D,R12B`, ISSUE-181), RIP `0x200e33`, `Unsupported { opcode: 182 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact register MOVZX (ISSUE-181)

The base paged profile admits only `45 0F B6 F4`, decoded as `MOVZX R14D,R12B`. REX.R selects R14D and REX.B selects R12B. Existing unsigned `Operation::Extend` behavior reads the low byte and writes a dword, zero-extending R14; flags and their definedness are preserved. Neighboring aliases, widths, prefixes and memory forms remain outside the allowlist.

Checks: four focused `rust_helper_movzx_r14d_r12b` tests, the other-profile isolation test, all 406 `paged_probe_profile` tests, and `cargo check --offline -p od-isa --all-targets` passed. Coverage includes full-state/value routing, dword zero-extension, flags/definedness, final-ModRM fetch fault and neighboring forms. The firmware image `/private/tmp/od-fw-s7-04-issue181.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner retired 84 and stopped at `45 89 E1` (`MOV R9D,R12D`, ISSUE-182), RIP `0x200e37`, `Unsupported { opcode: 137 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact dword register MOV (ISSUE-182)

The base paged profile admits only `45 89 E1`, decoded as `MOV R9D,R12D`. Exact REX/opcode/ModRM/length/operand-size and register views are checked. The existing register MOV semantics preserve R12, copy its low dword to R9D (zero-extending R9), advance RIP by three, and preserve flags/definedness. Neighboring transfer forms and other profiles remain unsupported.

Checks: four focused `rust_helper_mov_r9d_r12d` tests and the other-profile isolation test passed; all 410 `paged_probe_profile` tests and `cargo check --offline -p od-isa --all-targets` passed. Coverage includes source/destination routing, full-state and flag retention, dword zero-extension, final-ModRM fetch-fault atomicity, neighboring direction/register/width/prefix/memory forms and CPL3/profile isolation. Firmware image `/private/tmp/od-fw-s7-04-issue182.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner retired 85 and stopped at `41 C1 E9 08` (`SHR R9D,8`, ISSUE-183), RIP `0x200e3a`, `Unsupported { opcode: 193 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact dword SHR (ISSUE-183)

The base paged profile admits only `41 C1 E9 08`, decoded as `SHR R9D,8`. The guard constrains REX, opcode, length, dword width, destination, operation and immediate count. Shared shift helpers calculate the result and count-aware flags; the dword result zero-extends R9. For count 8, AF/OF retain deterministic concrete values while their definedness is cleared. Neighboring forms and other profiles remain unsupported.

Checks: four focused `rust_helper_shr_r9d_8` tests and the other-profile isolation test passed; all 414 `paged_probe_profile` tests and `cargo check --offline -p od-isa --all-targets` passed. Tests cover result/flags/definedness, full state, zero-extension, final-immediate fetch-fault atomicity, neighboring encodings and CPL3/profile isolation. Scoped rustfmt and `git diff --check` passed. Firmware image `/private/tmp/od-fw-s7-04-issue183.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner `cargo run --offline -q -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-issue183.bin --com1-out /private/tmp/od-fw-s7-04-issue183.com1 --budget 10000` retired 86 instructions and stopped at `45 31 FE` (`XOR R14D,R15D`, ISSUE-184), `RIP=0x200e3e`, `Unsupported { opcode: 49 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact register XOR (ISSUE-184)

The base paged profile admits only `45 31 FE`, decoded as `XOR R14D,R15D`. ModRM.r/m plus REX.B is destination R14D; ModRM.reg plus REX.R is source R15D. Shared XOR semantics zero-extend the dword result into R14, update the logic flags, preserve AF's concrete bit and clear AF-definedness. The allowlist remains limited to this exact register pair and encoding.

Checks: four focused `rust_helper_xor_r14d_r15d` tests, the other-profile isolation test, all 418 `paged_probe_profile` tests, and `cargo check --offline -p od-isa --all-targets` passed. Tests cover distinct operand values/order, full state, flags/AF-definedness, zero-extension, ModRM fetch-fault atomicity, neighboring encodings and CPL3/profile isolation. Scoped rustfmt and `git diff --check` passed. Firmware image `/private/tmp/od-fw-s7-04-issue184.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner `cargo run --offline -q -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-issue184.bin --com1-out /private/tmp/od-fw-s7-04-issue184.com1 --budget 10000` retired 87 instructions and stopped at `46 33 0C B5 80 26 20 00` (`XOR R9D,dword ptr [R14*4+0x202680]`, ISSUE-185), `RIP=0x200e41`, `Unsupported { opcode: 51 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact tracked SIB memory XOR (ISSUE-185)

The base paged profile admits only `46 33 0C B5 80 26 20 00`, decoded as `XOR R9D,dword ptr [R14*4+0x202680]`. REX.R/X select R9D and R14; SIB requires scale 4 and no base, with exact positive displacement. The dword is read via the existing tracked guest-memory path before CPU state changes. Shared XOR semantics zero-extend R9D and update logic flags; AF's concrete bit is preserved while its definedness clears.

Checks: six focused profile tests passed for address, value, flags, read-fault and fetch-fault atomicity, noncanonical effective address, neighbors and CPL3/profile isolation. The machine integration test `paged_probe_xor_r9d_r14_sib_dword_uses_tracked_four_level_walk` passed and verified Accessed bits for the code and data leaf PTEs; it makes no cross-page claim. All 424 `paged_probe_profile` tests, all 35 `paged_probe_quantum` tests, `cargo check --offline -p od-isa -p od-machine --all-targets`, scoped rustfmt and `git diff --check` passed. Firmware image `/private/tmp/od-fw-s7-04-issue185.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner `cargo run --offline -q -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-issue185.bin --com1-out /private/tmp/od-fw-s7-04-issue185.com1 --budget 10000` retired 88 instructions and stopped at `49 83 C3 02` (`ADD R11,2`, ISSUE-186), `RIP=0x200e49`, `Unsupported { opcode: 131 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact qword ADD imm8 (ISSUE-186)

The base paged profile admits only `49 83 C3 02`, decoded as `ADD R11,2`. Exact REX/opcode/ModRM/length/width/register/immediate checks keep the operation bounded. The decoder sign-extends imm8 and shared qword ADD semantics update R11 and arithmetic flags; no memory is accessed.

Checks: four focused `rust_helper_add_r11_imm8_two` tests and other-profile isolation passed; all 428 `paged_probe_profile` tests and `cargo check --offline -p od-isa --all-targets` passed. Tests cover result/flags and definedness including carry/zero/sign/overflow, immediate sign-extension neighbors, full-state retention, final immediate-byte fetch-fault atomicity, adjacent operators/registers/widths/prefixes and CPL3/profile isolation. Scoped rustfmt and `git diff --check` passed. Firmware image `/private/tmp/od-fw-s7-04-issue186.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner `cargo run --offline -q -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-issue186.bin --com1-out /private/tmp/od-fw-s7-04-issue186.com1 --budget 10000` retired 89 instructions and stopped at `4C 39 DD` (`CMP RBP,R11`, ISSUE-187), `RIP=0x200e4d`, `Unsupported { opcode: 57 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact qword CMP (ISSUE-187)

The base paged profile admits only `4C 39 DD`, decoded as `CMP RBP,R11`. Exact REX/opcode/ModRM/length/qword and decoded register views preserve Intel destination/source order. Shared CMP updates arithmetic flags/definedness without changing GPRs.

Checks: four focused `rust_helper_cmp_rbp_r11` tests and other-profile isolation passed; all 432 `paged_probe_profile` tests and `cargo check --offline -p od-isa --all-targets` passed. Tests cover distinct operand order, flag cases including borrow and signed overflow, definedness, unchanged GPRs, final-ModRM fetch-fault atomicity, neighboring encodings and CPL3/profile isolation. Scoped rustfmt and `git diff --check` passed. Firmware image `/private/tmp/od-fw-s7-04-issue187.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner `cargo run --offline -q -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-issue187.bin --com1-out /private/tmp/od-fw-s7-04-issue187.com1 --budget 10000` retired 90 instructions and stopped at `75 AE` (`JNZ rel8 -0x52`, ISSUE-188), `RIP=0x200e50`, `Unsupported { opcode: 117 }`, `com1_bytes=0`; full handoff validation remains incomplete.


<a id="s7-04-cpu-profile-slice-exact-rust-helper-jnz-minus-0x52"></a>
#### S7-04 CPU-profile slice: exact Rust-helper JNZ rel8 −0x52 (ISSUE-188)

Base `PagedCpl0ProbeV1` admits only exact `75 AE`, decoded as short `JNZ` with signed displacement −0x52. Shared `ConditionCode::NotEqual` behavior reads ZF, preserves flags and their definedness, and selects the signed target or fall-through. Canonicality is checked only for a taken target; the branch performs no data access.

Checks: five focused `firmware_entry_jnz_*` tests cover taken/not-taken behavior and complete CPU-state retention, flags/definedness, signed target calculation, a noncanonical taken target, rel8 fetch-fault atomicity, neighboring encodings, CPL3 and other-profile isolation. All 432 `paged_probe_profile` tests and `cargo check --offline -p od-isa --all-targets` passed; scoped rustfmt and `git diff --check` passed. Image `/private/tmp/od-fw-s7-04-issue188.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner command `cargo run --offline -q -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-issue188.bin --com1-out /private/tmp/od-fw-s7-04-issue188.com1 --budget 10000` retired 8871 instructions and stopped at `41 F7 D1` (`NOT R9D`, ISSUE-189), `RIP=0x200e52`, `Unsupported { opcode: 247 }`, `com1_bytes=0`; full handoff validation remains incomplete.


<a id="s7-04-cpu-profile-slice-exact-rust-helper-not-r9d"></a>
#### S7-04 CPU-profile slice: exact Rust-helper NOT R9D (ISSUE-189)

Base `PagedCpl0ProbeV1` admits only exact `41 F7 D1` (`NOT R9D`). Existing NOT semantics invert the low dword and zero-extend the result into R9; flags and `rflags_defined` remain unchanged. The guard requires exact REX/opcode/ModRM/length/width and decoded register destination. Neighboring forms remain unsupported.

Checks: four focused `rust_helper_not_r9d_*` tests cover full-state inversion, R9 zero-extension from source values with nonzero high bits, flags and definedness retention, ModRM fetch-fault atomicity, neighboring register/group/width/prefix/memory forms, CPL3 and other-profile isolation. All 436 `paged_probe_profile` tests and `cargo check --offline -p od-isa --all-targets` passed; scoped rustfmt and `git diff --check` passed. Image `/private/tmp/od-fw-s7-04-issue189.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner command `cargo run --offline -q -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-issue189.bin --com1-out /private/tmp/od-fw-s7-04-issue189.com1 --budget 10000` retired 8872 instructions and stopped at `44 39 CF` (`CMP EDI,R9D`, ISSUE-190), `RIP=0x200e55`, `Unsupported { opcode: 57 }`, `com1_bytes=0`; full handoff validation remains incomplete.


<a id="s7-04-cpu-profile-slice-exact-rust-helper-cmp-edi-r9d"></a>
#### S7-04 CPU-profile slice: exact Rust-helper CMP EDI,R9D (ISSUE-190)

Base `PagedCpl0ProbeV1` admits only exact `44 39 CF`, decoded as dword `CMP EDI,R9D`. Exact REX.R and ModRM routing retain Intel destination/source order; shared dword CMP changes flags/definedness and does not write registers.

Checks: four focused `rust_helper_cmp_edi_r9d_*` tests cover differing operand values/direction, ignored high halves, equality/borrow/overflow flags, definedness, unchanged GPRs, ModRM fetch fault, neighbors and CPL3/other-profile isolation. All 440 `paged_probe_profile` tests and `cargo check --offline -p od-isa --all-targets` passed; scoped rustfmt and `git diff --check` passed. Image `/private/tmp/od-fw-s7-04-issue190.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner command `cargo run --offline -q -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-issue190.bin --com1-out /private/tmp/od-fw-s7-04-issue190.com1 --budget 10000` retired 8873 instructions and stopped at `75 4A` (`JNZ rel8 +0x4A`, ISSUE-191), `RIP=0x200e58`, `Unsupported { opcode: 117 }`, `com1_bytes=0`; full handoff validation remains incomplete.


<a id="s7-04-cpu-profile-slice-exact-rust-helper-jnz-plus-0x4a"></a>
#### S7-04 CPU-profile slice: exact Rust-helper JNZ +0x4A (ISSUE-191)

Base `PagedCpl0ProbeV1` admits only exact `75 4A`, short JNZ with displacement +0x4A. The existing condition logic preserves flags/definedness, calculates both taken and fall-through RIPs, and checks canonicality only for the taken target.

Checks: five focused `firmware_entry_jnz_*` tests cover taken/not-taken target selection, full CPU-state and flags/definedness retention, noncanonical target handling, rel8 fetch-fault atomicity, neighbors and CPL3/other-profile isolation. All 440 `paged_probe_profile` tests and `cargo check --offline -p od-isa --all-targets` passed; scoped rustfmt and `git diff --check` passed. Image `/private/tmp/od-fw-s7-04-issue191.bin` built (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`). Runner command `cargo run --offline -q -p od-runner -- boot-firmware --image /private/tmp/od-fw-s7-04-issue191.bin --com1-out /private/tmp/od-fw-s7-04-issue191.com1 --budget 10000` retired 8874 instructions and stopped at `44 8B 76 34` (`MOV R14D,dword ptr [RSI+0x34]`, ISSUE-192), `RIP=0x200e5a`, `Unsupported { opcode: 139 }`, `com1_bytes=0`; full handoff validation remains incomplete.


<a id="s7-04-exact-mov-r14d-dword-rsi-plus-0x34"></a>
### S7-04 CPU profile slice: exact Rust-helper `MOV R14D,dword ptr [RSI+0x34]`

The base paged profile admits only bytes `44 8B 76 34`, decoded as dword read from RSI+0x34 into R14D. The existing tracked memory-read path performs the read before the architectural register write; dword write zero-extends R14. Memory faults preserve the complete CPU state and definedness. Adjacent displacements, destinations, widths, directions, addressing forms and other profiles stay unsupported.

Validation: six focused profile tests cover exact read/result, R14 zero-extension, full-state and flags retention, injected read fault, noncanonical effective address, final-disp8 fetch fault, neighboring forms, CPL3 and other-profile isolation. The four-level machine integration test verifies code/data leaf PTE Accessed bits and no data write; it makes no cross-page claim. All 446 `paged_probe_profile` and 36 `paged_probe_quantum` tests, `cargo check --offline -p od-isa -p od-machine --all-targets`, scoped rustfmt and `git diff --check` passed. Firmware image `/private/tmp/od-fw-s7-04-issue192.bin` (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) ran 8,875 instructions, then stopped at exact `41 8D 7E FF` (`LEA EDI,[R14-1]`, ISSUE-193) at RIP `0x200e5e`; COM1 is empty and full handoff validation remains incomplete.


<a id="s7-04-exact-lea-edi-r14-minus-one"></a>
### S7-04 CPU profile slice: exact Rust-helper `LEA EDI,[R14-1]`

The base paged profile admits only `41 8D 7E FF`: exact REX.B selection of R14, LEA dword destination EDI, qword address calculation and signed disp8 `-1`. The shared EA calculation wraps in 64-bit address arithmetic; writing EDI truncates to low32 and zero-extends RDI. No memory read, flags mutation or canonicality check occurs. All adjacent encodings remain outside the profile.

Validation: four focused tests cover result across low32/64-bit wrap boundaries, noncanonical effective addresses, full-state and flags-definedness retention, no memory/PIO access, final disp8 fetch-fault atomicity, neighbors and CPL3 isolation; the other-profile isolation vector passes. All 450 `paged_probe_profile` tests, `cargo check --offline -p od-isa --all-targets`, scoped rustfmt and `git diff --check` passed. Firmware image `/private/tmp/od-fw-s7-04-issue193.bin` (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) ran 8,876 instructions, then stopped at exact `81 FF 80 00 00 00` (`CMP EDI,0x80`, ISSUE-194) at RIP `0x200e62`; COM1 is empty and full handoff validation remains incomplete.


<a id="s7-04-exact-cmp-edi-0x80"></a>
### S7-04 CPU profile slice: exact Rust-helper `CMP EDI,0x80`

The base paged profile admits only `81 FF 80 00 00 00`: dword opcode `81 /7`, ModRM `FF`, EDI destination and immediate `0x80`. Shared dword CMP semantics ignore RDI's high half, update arithmetic flags/definedness and do not write GPRs. Other immediates, registers, widths, opcode groups and memory forms remain outside the allowlist.

Validation: four focused tests cover high-half independence, equality/less/greater flags, complete state and definedness, final immediate fetch-fault atomicity, adjacent group/immediate/width/prefix/memory forms and CPL3/profile isolation. All 454 `paged_probe_profile` tests, `cargo check --offline -p od-isa --all-targets`, scoped rustfmt and `git diff --check` passed. Firmware image `/private/tmp/od-fw-s7-04-issue194.bin` (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) ran 8,877 instructions, then stopped at exact `73 4C` (`JAE/JNB rel8 +0x4C`, ISSUE-195) at RIP `0x200e68`; COM1 is empty and full handoff validation remains incomplete.


<a id="s7-04-exact-jae-plus-0x4c"></a>
### S7-04 CPU profile slice: exact Rust-helper `JAE rel8 +0x4C`

The base paged profile admits only `73 4C`. `ConditionCode::AboveOrEqual` tests CF: CF=0 takes `next_rip+0x4c`, CF=1 falls through; both paths preserve flags and definedness. Only the taken target receives the shared canonicality check. Other branch encodings remain outside the profile.

Validation: five focused tests cover both outcomes, signed target, flags/definedness, taken-only canonicality, rel8 fetch-fault atomicity, adjacent branches and CPL3/profile isolation. All 459 `paged_probe_profile` tests, `cargo check --offline -p od-isa --all-targets`, scoped rustfmt and `git diff --check` passed. Firmware image `/private/tmp/od-fw-s7-04-issue195.bin` (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) ran 8,878 instructions, then stopped at `8B 7E 38` (`MOV EDI,dword ptr [RSI+0x38]`, ISSUE-196) at RIP `0x200e6a`; COM1 is empty and full handoff validation remains incomplete.


<a id="s7-04-exact-mov-edi-rsi-plus-0x38"></a>
### S7-04 CPU profile slice: exact Rust-helper `MOV EDI,dword ptr [RSI+0x38]`

The base paged profile admits only `8B 7E 38`, a four-byte tracked dword read from RSI+0x38 into EDI. The read completes before the register write; writing EDI zero-extends RDI, and memory faults leave CPU state untouched. The real four-level test verifies the data leaf PTE Accessed bit; no cross-page claim is made.

Validation: six focused profile tests cover exact address/result and zero-extension, full state/flags, injected read fault, noncanonical address, final disp8 fetch fault, neighboring forms and CPL3/profile isolation. The machine integration test verifies tracked four-level code/data page walks, both leaf-PTE A bits, unchanged data, and RDI zero-extension. All 465 `paged_probe_profile` and 37 `paged_probe_quantum` tests, `cargo check --offline -p od-isa -p od-machine --all-targets`, scoped rustfmt and `git diff --check` passed. Firmware image `/private/tmp/od-fw-s7-04-issue196.bin` (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) ran 8,879 instructions, then stopped at exact `83 FF 20` (`CMP EDI,0x20`, ISSUE-197) at RIP `0x200e6d`; COM1 is empty and full handoff validation remains incomplete.

<a id="s7-04-exact-cmp-edi-imm8-0x20"></a>
### S7-04 CPU profile slice: exact Rust-helper `CMP EDI,0x20`

The base `PagedCpl0ProbeV1` profile admits only no-prefix bytes `83 FF 20`: CMP group `/7`, dword EDI destination, and imm8 `0x20` sign-extended to dword. Existing arithmetic CMP semantics set flags/definedness and preserve all GPRs. Other immediate values, registers, operand widths, groups, prefixes, and memory forms remain unsupported.

Independent review passed after fixing the CMP operand width and adding a low-32-bit regression vector. No findings remain.

Validation: four focused `rust_helper_cmp_edi_imm8_0x20_*` tests and the other-profile isolation test passed; all 469 `paged_probe_profile` tests; full `cargo test --offline -p od-isa --quiet` passed 4565 tests across targets, `cargo check --offline -p od-isa --all-targets`, scoped rustfmt and `git diff --check` passed. Tests cover upper-RDI independence, equality/less/greater results, flag-definedness, GPR preservation, fetch-fault atomicity on the immediate, neighboring forms, and CPL3 isolation. Firmware image `/private/tmp/od-fw-s7-04-issue197.bin` (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) ran 8,880 instructions, then stopped at `75 50` (`JNZ rel8 +0x50`, ISSUE-198) at RIP `0x200e70` with `Unsupported { opcode: 117 }`; COM1 is empty and full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `JNZ rel8 +0x50` (ISSUE-198)

The base paged profile admits only no-prefix `75 50`. Shared `ConditionCode::NotEqual` reads ZF, preserves flags and `rflags_defined`, selects the signed rel8 target or fall-through, and checks canonicality only when taken. Adjacent displacement, condition, prefix, CPL3 and other-profile forms remain unsupported. Independent review passed with no remaining findings.

Validation: five focused `firmware_entry_jnz_*` tests passed, including taken/not-taken complete-state retention, taken-only noncanonical target, rel8 fetch-fault atomicity, neighboring forms and CPL3 isolation. All 469 `paged_probe_profile` tests and `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets` passed. Firmware image `/private/tmp/od-fw-s7-04-issue198.bin` (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) ran 8,881 instructions, then stopped at `44 8B 7E 30` (`MOV R15D,dword ptr [RSI+0x30]`, ISSUE-199), RIP `0x200e72`, `Unsupported { opcode: 139 }`; COM1 is empty and full handoff validation remains incomplete. Scoped `rustfmt --edition 2024 --check` and `git diff --check` passed.


#### S7-04 CPU-profile slice: exact Rust-helper `MOV R15D,dword ptr [RSI+0x30]` (ISSUE-199)

The base paged profile admits only `44 8B 7E 30`. The decoder confirms REX.R extends ModRM.reg=7 to R15D; the tracked dword read must succeed before the dword write zero-extends R15. The existing `44 8B 76 34` remains the separate R14D form. Other registers, displacements, widths, prefixes, addressing forms and profiles remain unsupported.

Independent review approved with no remaining findings. Validation: five focused R15D tests and all 474 `paged_probe_profile` tests passed. The real four-level machine test verified the low dword from a known qword, unchanged full qword source, unchanged R14 and Accessed bits in code/data leaf PTEs. All 38 `paged_probe_quantum` tests passed. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, `rustfmt --edition 2024 --check` for touched Rust files and `git diff --check` passed. Full `od-isa` tests passed (2,322). The full `od-machine` suite had 1,195 passed, 2 failed and 6 ignored; both failures are the Linux-direct CLD expectation mismatch documented as ISSUE-201, and independent reruns reproduced both. The focused `paged_probe_quantum` suite passed all 38 tests. Firmware image `/private/tmp/od-fw-s7-04-issue199.bin` (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) ran 8,882 instructions, then stopped at `41 F6 C7 07` (`TEST R15B,7`, ISSUE-200), RIP `0x200e76`, `Unsupported { opcode: 246 }`; COM1 is empty and full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `TEST R15B,7` (ISSUE-200)

The base paged profile admits only `41 F6 C7 07`. REX.B plus ModRM C7 selects R15B. The shared byte TEST semantics compute `R15B & 7`, update logical flags only, retain AF's concrete value and clear AF-definedness; all GPRs remain unchanged. Neighboring immediates, registers, prefixes and memory forms, and CPL3 remain outside the allowlist.

Independent review approved with no remaining findings. Validation: four focused `rust_helper_test_r15b_imm8_*` tests and all 478 `paged_probe_profile` tests passed. Tests cover multiple R15 byte patterns, logical flags, AF definedness, complete state retention, fetch-fault atomicity on the immediate, neighbors, prefixes and CPL3 isolation. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped Rust 2024 rustfmt and `git diff --check` passed. Existing ISSUE-201 still causes the full `od-machine` library test run to report 1,195 passed, 2 failed and 6 ignored; both failures reproduce independently and are unrelated to this profile slice. Firmware image `/private/tmp/od-fw-s7-04-issue200.bin` (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) ran 8,883 instructions, then stopped at `74 64` (`JZ rel8 +0x64`, ISSUE-202), RIP `0x200e7a`, `Unsupported { opcode: 116 }`; COM1 is empty and full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `JZ rel8 +0x64` (ISSUE-202)

The base `PagedCpl0ProbeV1` admits only exact no-prefix bytes `74 64`. Shared `ConditionCode::Equal` semantics test ZF, preserve all flags and definedness, and select `next_rip+0x64` when ZF=1 or fall-through when ZF=0. Only a taken target is canonicality-checked. Other conditions, displacements, prefixes, near forms, CPL3 and other profiles remain unsupported.

Independent review passed; slice committed as `be51d06`. Five focused `rust_helper_jz_rel8_*` tests cover both ZF values with defined/undefined ZF, full CPU-state retention, taken and fall-through RIPs, taken-only canonicality, rel8 fetch-fault atomicity, adjacent forms/prefixes and CPL3 isolation. The other-profile isolation test passed; all 478 `paged_probe_profile` tests passed. `cargo test --offline -p od-isa --quiet` passed all 2,322 library, 1,591 decoder, 126 execution, 29 boundary, 6 flag, 478 paged-profile, 7 profile-isolation and 15 descriptor tests. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped Rust 2024 rustfmt and `git diff --check` passed. The 4 MiB firmware image `/private/tmp/od-fw-s7-04-issue202.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) ran 8,884 instructions, then stopped at `41 81 FF A0 00 00 00` (`CMP R15D,0xA0`, ISSUE-203), RIP `0x200ee0`, `Unsupported { opcode: 129 }`, `com1_bytes=0`; full handoff validation remains incomplete. Existing ISSUE-201 still tracks two unrelated Linux direct-boot CLD expectation failures (1,195 passed, 2 failed, 6 ignored in the full `od-machine` test run).


#### S7-04 CPU-profile slice: exact Rust-helper `CMP R15D,0xA0` (ISSUE-203)

The base `PagedCpl0ProbeV1` admits only exact bytes `41 81 FF A0 00 00 00`: REX.B selects R15D and opcode `81 /7` compares the dword against imm32 `0xA0`. Shared CMP arithmetic semantics update flags and `rflags_defined`; all GPRs remain unchanged. The dword immediate is not sign-extended to qword width. Other destinations, immediate values, group extensions, widths, prefixes, CPL3 and other profiles remain unsupported.

Independent review passed; slice committed as `9c5ec8b`. Four focused `rust_helper_cmp_r15d_imm32_0xa0_*` tests cover arithmetic flags/definedness, full GPR/state retention, final-imm32 fetch-fault atomicity, neighbors and profile/CPL isolation. All `od-isa` package suites passed: 2,322 library, 1,591 decoder, 126 execution, 29 boundary, 6 flag, 482 paged-profile, 7 profile-isolation and 15 descriptor tests. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped Rust 2024 rustfmt and `git diff --check` passed. Firmware image `/private/tmp/od-fw-s7-04-issue203.bin` (4 MiB, SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) ran 8,885 instructions, then stopped at `72 4C` (`JB/JNAE rel8 +0x4c`, ISSUE-204), RIP `0x200ee7`, `Unsupported { opcode: 114 }`, `com1_bytes=0`; full handoff validation remains incomplete. Existing ISSUE-201 still tracks two unrelated Linux direct-boot CLD expectation failures (1,195 passed, 2 failed, 6 ignored in the full `od-machine` test run).


#### S7-04 CPU-profile slice: exact Rust-helper `JB rel8 +0x4c` (ISSUE-204)

Base `PagedCpl0ProbeV1` admits only exact no-prefix bytes `72 4C`. Shared `ConditionCode::Below` tests CF, preserves RFLAGS and `rflags_defined`, and selects `next_rip+0x4c` for CF=1 or fall-through for CF=0. Only a taken target is canonicality-checked. Other conditions, displacements, prefixes, near forms, CPL3 and other profiles remain unsupported.

Independent review passed with no remaining findings. Five focused `rust_helper_jb_plus_4c_*` tests cover both CF values with defined and undefined CF, full-state retention, target/fall-through, noncanonical fall-through deferral and taken-target #GP, rel8 fetch-fault atomicity, neighbor forms and CPL3 isolation; the other-profile isolation test passed. All 487 `paged_probe_profile` tests passed. All `od-isa` package suites passed: 2,322 library, 1,591 decoder, 126 execution, 29 boundary, 6 flag, 487 paged-profile, 7 profile-isolation and 15 descriptor tests. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped Rust 2024 rustfmt and `git diff --check` passed. Full `cargo test --offline -p od-machine --quiet` produced 1,195 passed, 2 failed and 6 ignored; both failures are the already documented ISSUE-201 stale CLD expectations (expected unsupported opcode 0xFC; actual stop opcode 0xFA) and unrelated to this branch slice. The 4 MiB firmware image `/private/tmp/od-fw-s7-04-issue204.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) ran 8,886 instructions, then stopped at `4C 89 F7` (`MOV RDI,R14`, ISSUE-205), RIP `0x200ee9`, `Unsupported { opcode: 137 }`, `com1_bytes=0`; full handoff validation remains incomplete. Existing ISSUE-201 still tracks two unrelated Linux direct-boot CLD expectation failures (1,195 passed, 2 failed, 6 ignored in the full `od-machine` test run).


#### S7-04 CPU-profile slice: exact Rust-helper `MOV RDI,R14` (ISSUE-205)

Base `PagedCpl0ProbeV1` admits only exact bytes `4C 89 F7`. With opcode `89`, REX.W+R (`4C`) and ModRM `F7`, R14 is the full-width source and RDI is the destination. The move changes only RDI and RIP; flags and flag-definedness remain unchanged. Other source/destination registers, widths, directions, prefixes, CPL3 and other profiles remain unsupported.

Independent review approved with no remaining findings. Four focused `rust_helper_mov_rdi_r14_*` tests cover full-width copying, all-other-state retention, fetch-fault atomicity on ModRM, adjacent register/opcode/width/prefix forms, CPL3 and other-profile isolation. All 491 `paged_probe_profile` tests passed. All `od-isa` package suites passed: 2,322 library, 1,591 decoder, 126 execution, 29 boundary, 6 flag, 491 paged-profile, 7 profile-isolation and 15 descriptor tests. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped Rust 2024 rustfmt and `git diff --check` passed. The full od-machine run recorded under ISSUE-201 remains 1,195 passed, 2 failed and 6 ignored; both failures are stale CLD expectations and unrelated to this MOV. The 4 MiB firmware image `/private/tmp/od-fw-s7-04-issue205.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) ran 8,887 instructions, then stopped at `48 C1 E7 05` (`SHL RDI,5`, ISSUE-206), RIP `0x200eec`, `Unsupported { opcode: 193 }`, `com1_bytes=0`; full handoff validation remains incomplete. Existing ISSUE-201 still tracks two unrelated Linux direct-boot CLD expectation failures (1,195 passed, 2 failed, 6 ignored in the full `od-machine` test run).


#### S7-04 CPU-profile slice: exact Rust-helper `SHL RDI,5` (ISSUE-206)

Base `PagedCpl0ProbeV1` admits only exact bytes `48 C1 E7 05`: qword opcode `C1 /4`, RDI destination, imm8 `05`. The shared qword count mask `0x3f` leaves effective count five. Shared shift semantics update CF/SF/ZF/PF, leave AF undefined, and mark OF undefined for count>1 while preserving its concrete value. Among GPRs only RDI changes; RIP advances and flags follow the stated semantics. Other counts, operations, registers, widths, count sources, prefixes, CPL3 and other profiles remain unsupported.

Independent review approved the fix for the discovered `/6` SAL alias admission: the decoder retains raw ModRM and the exact gate requires `E7`; `48 C1 F7 05` is now a regression neighbor. Four focused `rust_helper_shl_rdi_5_*` tests cover qword results, CF/SZP, AF/OF definedness, complete-state retention, final-imm8 fetch-fault atomicity, neighboring encodings, CPL3 and other-profile isolation. All 495 `paged_probe_profile` tests and every `od-isa` package test suite passed. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets` and the independent `cargo check --offline --workspace --all-targets`, Rust 2024 scoped rustfmt and `git diff --check` passed. The full `od-machine` suite reports 1,195 passed, 2 failed, 6 ignored; both failures are the previously registered ISSUE-201 stale CLD expectations (actual `Unsupported { opcode: 250 }`, expected `Unsupported { opcode: 252 }`). The 4 MiB firmware image `/private/tmp/od-fw-s7-04-issue206.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) ran 8,888 instructions, then stopped at `4C 01 FF` (`ADD RDI,R15`, ISSUE-207), RIP `0x200ef0`, `Unsupported { opcode: 1 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `ADD RDI,R15` (ISSUE-207)

Base `PagedCpl0ProbeV1` admits only `4C 01 FF`. With opcode `01`, exact REX `4C` and ModRM `FF`, Intel operand order selects qword RDI as destination and qword R15 as source. Shared qword ADD semantics determine the result, all six arithmetic flags and their definedness; among GPRs only RDI changes. Neighboring direction/register/width/prefix/memory forms and other profiles remain unsupported.

Independent review approved with no remaining findings. Four focused `rust_helper_add_rdi_r15_*` tests cover representative result/flags including carry, zero, parity and signed overflow; full-state/source preservation; arithmetic flag definedness; final-ModRM fetch-fault atomicity; neighboring encodings; CPL3 and other-profile isolation. All 499 `paged_probe_profile` tests passed. All `od-isa` package suites passed (2,322 library, 1,591 decoder, 126 execution, 29 boundary, 6 flag, 499 paged-profile, 7 profile-isolation and 15 descriptor tests). Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, Rust 2024 scoped rustfmt and `git diff --check` passed. The full `od-machine` suite reports 1,195 passed, 2 failed, 6 ignored; both failures are the registered ISSUE-201 stale CLD expectations (actual `Unsupported { opcode: 250 }`, expected `Unsupported { opcode: 252 }`). The 4 MiB firmware image `/private/tmp/od-fw-s7-04-issue207.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) ran 8,889 instructions, then stopped at `48 39 EF` (`CMP RDI,RBP`, ISSUE-208), RIP `0x200ef3`, `Unsupported { opcode: 57 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `CMP RDI,RBP` (ISSUE-208)

Base `PagedCpl0ProbeV1` admits only `48 39 EF`. Opcode `39` retains Intel r/m destination, reg source order; exact REX.W and ModRM select qword RDI−RBP. Shared qword CMP semantics update CF/PF/AF/ZF/SF/OF and mark all six arithmetic flags defined without changing any GPR. Other directions, registers, widths, prefixes and profiles remain unsupported.

Independent review approved with no remaining findings. Four focused `rust_helper_cmp_rdi_rbp_*` tests cover equality, borrow, parity, signed overflow, complete GPR/state retention, arithmetic flag definedness, final-ModRM fetch-fault atomicity, neighbors and CPL3 isolation. All 503 `paged_probe_profile` tests passed. All `od-isa` package suites passed (2,322 library, 1,591 decoder, 126 execution, 29 boundary, 6 flag, 503 paged-profile, 7 profile-isolation and 15 descriptor tests). Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, Rust 2024 scoped rustfmt and `git diff --check` passed. Full `od-machine` suite: 1,195 passed, 2 failed, 6 ignored; both failures are the registered ISSUE-201 stale CLD expectations (actual `Unsupported { opcode: 250 }`, expected `Unsupported { opcode: 252 }`). The 4 MiB firmware image `/private/tmp/od-fw-s7-04-issue208.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) ran 8,890 instructions, then stopped at `77 3D` (`JA rel8 +0x3d`, ISSUE-209), RIP `0x200ef6`, `Unsupported { opcode: 119 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `JA rel8 +0x3d` (ISSUE-209)

Base `PagedCpl0ProbeV1` admits only exact no-prefix bytes `77 3D`, a short JA/JNBE with displacement +0x3d. Shared `ConditionCode::Above` tests `CF=0 && ZF=0`; RFLAGS and `rflags_defined` are preserved. Taken branches use `next_rip+0x3d`, not-taken branches use fall-through, and only taken targets are canonicality-checked. Other conditions, displacements, prefixes and profiles remain unsupported.

Independent review approved with no remaining findings. Five focused `rust_helper_ja_plus_3d_*` tests cover all CF/ZF outcomes, definedness retention, exact branch destinations, taken-only canonicality, rel8 fetch-fault atomicity, adjacent forms and CPL3; other-profile isolation passes. All 508 `paged_probe_profile` tests passed. All `od-isa` package suites passed (2,322 library, 1,591 decoder, 126 execution, 29 boundary, 6 flag, 508 paged-profile, 7 profile-isolation and 15 descriptor tests). Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, Rust 2024 scoped rustfmt and `git diff --check` passed. The full `od-machine` suite reports 1,195 passed, 2 failed, 6 ignored; both failures are the registered ISSUE-201 stale CLD expectations (actual `Unsupported { opcode: 250 }`, expected `Unsupported { opcode: 252 }`). The 4 MiB firmware image `/private/tmp/od-fw-s7-04-issue209.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) ran 8,891 instructions, then stopped at `44 8B 6E 3C` (`MOV R13D,dword ptr [RSI+0x3c]`, ISSUE-210), RIP `0x200ef8`, `Unsupported { opcode: 139 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `MOV R13D,dword ptr [RSI+0x3c]` (ISSUE-210)

Base `PagedCpl0ProbeV1` admits only `44 8B 6E 3C`. REX.R selects R13D from ModRM.reg=5; opcode `8B` with the exact ModRM and disp8 reads a dword through `[RSI+0x3c]`. The shared tracked read completes before the dword write zero-extends R13; other GPRs and flags remain unchanged. Other destinations, displacements, widths, prefixes, addressing forms and profiles remain unsupported.

Independent review approved the exact change with no remaining findings; ISSUE-210 was committed as `e16e880`. Six focused `rust_helper_mov_r13d_dword_handoff_field_*` tests cover successful load/zero-extension, state and flags, data read faults, noncanonical address, final disp8 fetch fault, neighboring/REX forms and CPL3 isolation; `other_paged_profiles_keep_s7_entry_instructions_unsupported` confirms this exact instruction remains refused by the other paged profiles. The tracked four-level machine test confirms correct dword extraction from a qword, preserved source/flags and Accessed bits. All 514 `paged_probe_profile` tests and all `od-isa` package suites passed. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped Rust 2024 rustfmt and `git diff --check` passed. The full `od-machine` package suite was not rerun; ISSUE-201 remains open for its two stale CLD-unsupported expectations (actual stop is CLI `0xFA` after admitted CLD `0xFC`). The 4 MiB firmware image `/private/tmp/od-fw-s7-04-issue210.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) ran 8,892 instructions, then stopped at `44 8B 66 40` (`MOV R12D,dword ptr [RSI+0x40]`, ISSUE-211), RIP `0x200efc`, `Unsupported { opcode: 139 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `MOV R12D,dword ptr [RSI+0x40]` (ISSUE-211)

Base `PagedCpl0ProbeV1` admits only `44 8B 66 40`. Exact REX `44` extends ModRM.reg=4 to R12D; opcode `8B`, ModRM `66` and disp8 `40` select `[RSI+0x40]`. The existing tracked dword read completes before the dword write zero-extends R12; flags and other GPRs remain unchanged. This narrow successful four-level read confirms Accessed bits in the code/data leaf PTEs; it does not establish general cross-page or rollback semantics (ISSUE-054).

Independent review approved the change with no remaining findings; ISSUE-211 was committed as `d584420`. Six focused `rust_helper_mov_r12d_dword_handoff_field_*` tests cover successful load/zero-extension, state and flags, data read faults, noncanonical address, final disp8 fetch fault, neighboring/REX forms, CPL3 and other-profile isolation. The tracked four-level machine test confirms extraction of the high dword from a known qword, source and flags retention, and code/data leaf Accessed bits. All 520 `paged_probe_profile` tests and all `od-isa` package suites passed (2,322 library, 1,591 decoder, 126 execution, 29 boundary, 6 flags, 520 paged-profile, 7 profile-isolation and 15 descriptor tests). All 40 `paged_probe_quantum` tests passed. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped Rust 2024 rustfmt and `git diff --check` passed. The full `od-machine` package suite was not rerun; ISSUE-201 remains open for its two stale CLD-unsupported expectations (actual stop is CLI `0xFA` after admitted CLD `0xFC`). The 4 MiB firmware image `/private/tmp/od-fw-s7-04-issue211.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) ran 8,893 instructions, then stopped at `44 8B 46 44` (`MOV R8D,dword ptr [RSI+0x44]`, ISSUE-212), RIP `0x200f00`, `Unsupported { opcode: 139 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `MOV R8D,dword ptr [RSI+0x44]` (ISSUE-212)

Base `PagedCpl0ProbeV1` admits only `44 8B 46 44`. Exact REX `44` extends ModRM.reg=0 to R8D; opcode `8B`, ModRM `46` and disp8 `44` select `[RSI+0x44]`. The existing tracked dword read completes before the dword write zero-extends R8; flags and other GPRs remain unchanged. This narrow successful four-level read confirms Accessed bits in the code/data leaf PTEs; it does not establish general cross-page or rollback semantics (ISSUE-054).

Independent review approved the change with no remaining findings. Six focused `rust_helper_mov_r8d_dword_handoff_field_*` tests cover successful load/zero-extension, state and flags, data read faults, noncanonical address, final disp8 fetch fault, neighboring/REX forms, CPL3 and other-profile isolation. The tracked four-level machine test confirms extraction of the high dword from a known qword, source and flags retention, and code/data leaf Accessed bits. All 526 `paged_probe_profile` tests and all `od-isa` package suites passed (2,322 library, 1,591 decoder, 126 execution, 29 boundary, 6 flags, 526 paged-profile, 7 profile-isolation and 15 descriptor tests). All 41 `paged_probe_quantum` tests passed. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped Rust 2024 rustfmt and `git diff --check` passed. The full `od-machine` package suite was not rerun; ISSUE-201 remains open for its two stale CLD-unsupported expectations (actual stop is CLI `0xFA` after admitted CLD `0xFC`). The 4 MiB firmware image `/private/tmp/od-fw-s7-04-issue212.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) ran 8,894 instructions, then stopped at `85 C9` (`TEST ECX,ECX`, ISSUE-213), RIP `0x200f04`, `Unsupported { opcode: 133 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `TEST ECX,ECX` (ISSUE-213)

Base `PagedCpl0ProbeV1` admits only exact bytes `85 C9`. The shared 32-bit TEST helper computes CF/PF/ZF/SF/OF from ECX AND ECX; CF and OF clear, SZP reflect the dword result, and AF's concrete value is preserved while its definedness is cleared. No GPR changes; computed logical flags become defined. Other operand pairs, widths, prefixes, immediate/memory forms and profiles remain unsupported.

Independent review approved the change with no remaining findings. Four focused `rust_helper_test_ecx_ecx_*` tests cover zero/parity/sign outcomes, full CPU state and GPR retention, unrelated flags, AF value and definedness, logical-flag definedness, final-ModRM fetch-fault atomicity, neighboring encodings and CPL3 isolation; other-profile isolation passes. All 530 `paged_probe_profile` tests and all `od-isa` package suites passed (2,322 library, 1,591 decoder, 126 execution, 29 boundary, 6 flags, 530 paged-profile, 7 profile-isolation and 15 descriptor tests). Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, Rust 2024 rustfmt and `git diff --check` passed. The full `od-machine` package suite was not rerun; ISSUE-201 remains open for its two stale CLD-unsupported expectations (actual stop is CLI `0xFA` after admitted CLD `0xFC`). The 4 MiB firmware image `/private/tmp/od-fw-s7-04-issue213.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) ran 8,895 instructions, then stopped at `74 35` (`JZ rel8 +0x35`, ISSUE-214), RIP `0x200f06`, `Unsupported { opcode: 116 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `JZ rel8 +0x35` (ISSUE-214)

Base `PagedCpl0ProbeV1` admits only exact no-prefix bytes `74 35`. The branch reads only ZF, preserves RFLAGS and `rflags_defined`, selects `next_rip+0x35` when taken or fall-through otherwise, and checks canonicality only for a taken target. All other conditions, displacements, prefixes, near forms and profiles remain unsupported.

Independent review approved the change with no remaining findings. The existing JZ rel8 profile tests now also cover +0x35 with both ZF values and defined/undefined input metadata, exact branch targets, full-state retention, taken-only canonicality, rel8 displacement-fetch-fault atomicity, neighboring branch/displacement/prefix forms, CPL3 and other-profile isolation. All 530 `paged_probe_profile` tests and all `od-isa` package suites passed (2,322 library, 1,591 decoder, 126 execution, 29 boundary, 6 flags, 530 paged-profile, 7 profile-isolation and 15 descriptor tests). Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, Rust 2024 rustfmt and `git diff --check` passed. The full `od-machine` package suite was not rerun; ISSUE-201 remains open for its two stale CLD-unsupported expectations (actual stop is CLI `0xFA` after admitted CLD `0xFC`). The 4 MiB firmware image `/private/tmp/od-fw-s7-04-issue214.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) ran 8,896 instructions, then stopped at `41 83 FC 07` (`CMP R12D,7`, ISSUE-215), RIP `0x200f3d`, `Unsupported { opcode: 131 }`, `com1_bytes=0`; full handoff validation remains incomplete.


#### S7-04 CPU-profile slice: exact Rust-helper `CMP R12D,7` (ISSUE-215)

Base `PagedCpl0ProbeV1` admits only exact bytes `41 83 FC 07`. Exact REX `41`, opcode `83` group `/7`, ModRM `FC`, and imm8 `07` select dword CMP of low R12D against 7. Shared arithmetic semantics produce 32-bit CF/PF/AF/ZF/SF/OF and mark those flags defined; all GPRs remain unchanged. Values above 16 bits distinguish this dword form from word CMP, while upper 32 bits of R12 are ignored.

Independent review approved the change with no remaining findings. Four focused `rust_helper_cmp_r12d_imm8_7_*` tests cover equality, borrow, greater-than and signed overflow; >16-bit values and ignored upper qword bits; full GPR retention; arithmetic flags/definedness; final-immediate fetch-fault atomicity; neighboring immediate/group/destination/width/prefix forms; and CPL3/other-profile isolation. All 534 `paged_probe_profile` tests and all `od-isa` package suites passed (2,322 library, 1,591 decoder, 126 execution, 29 boundary, 6 flags, 534 paged-profile, 7 profile-isolation and 15 descriptor tests). Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, Rust 2024 rustfmt and `git diff --check` passed. The full `od-machine` package suite was not rerun; ISSUE-201 remains open for its two stale CLD-unsupported expectations (actual stop is CLI `0xFA` after admitted CLD `0xFC`). The 4 MiB firmware image `/private/tmp/od-fw-s7-04-issue215.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) ran 8,897 instructions, then stopped at `74 D0` (`JZ rel8 -0x30`, ISSUE-216), RIP `0x200f41`, `Unsupported { opcode: 116 }`, `com1_bytes=0`; full handoff validation remains incomplete.

#### S7-04 CPU-profile slice: exact Rust-helper `JZ rel8 -0x30` (ISSUE-216)

The base `PagedCpl0ProbeV1` profile now admits only the exact no-prefix bytes `74 D0` at the current firmware helper site. It uses the shared JZ condition (ZF only), preserves flags and definedness, chooses `next_rip - 0x30` when taken and the two-byte fall-through otherwise, and applies canonicality only to a taken target. Only RIP changes; GPRs, flags/definedness and other CPU state are preserved.

Existing JZ rel8 tests were extended to cover this displacement with ZF set/clear and defined/undefined metadata, exact targets, negative taken-only canonicality, rel8 displacement-fetch-fault atomicity, adjacent displacement/branch and prefix encodings, CPL3 rejection, and other-profile isolation. Focused tests passed (5 JZ rel8 cases and 1 profile-isolation case); all 534 paged-profile tests and all od-isa package suites passed (2,322 library, 1,591 decoder, 126 execution, 29 boundary, 6 flags, 534 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, Rust 2024 rustfmt and `git diff --check` passed; check emitted only the existing unused `access_long64_4k_internal_with_writer` warning in `od-machine`. Independent review approved with no remaining findings. The 4 MiB firmware image `/private/tmp/od-fw-s7-04-issue216.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retired 8,898 instructions and stopped at `41 83 F8 30` (`CMP R8D,0x30`, ISSUE-217), RIP `0x200f13`, `Unsupported { opcode: 131 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact Rust-helper `CMP R8D,0x30` (ISSUE-217)

Base `PagedCpl0ProbeV1` admits only `41 83 F8 30`: exact REX `41`, opcode `83` group `/7`, ModRM `F8`, imm8 `30`, and dword operand size. Shared arithmetic semantics compare R8D with 0x30, update and define the six arithmetic flags, and preserve every GPR. The test matrix includes values above 16 bits to distinguish dword from word width and high-qword values to prove those bits are ignored.

Four focused tests pass, covering values above 16 bits, ignored upper qword bits, result flags/definedness, full GPR retention, final-immediate fetch-fault atomicity, neighboring immediates/groups/registers/widths/addressing/prefixes, and CPL3/other-profile isolation. All 538 paged-profile tests and all od-isa suites passed (2,322 library, 1,591 decoder, 126 execution, 29 boundary, 6 flags, 538 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, Rust 2024 rustfmt and `git diff --check` passed; cargo check emitted only the existing unused `access_long64_4k_internal_with_writer` warning in `od-machine`. Independent review approved with no remaining findings. Firmware image `/private/tmp/od-fw-s7-04-issue217.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retired 8,900 instructions and stopped at `48 89 54 24 30` (`MOV qword ptr [RSP+0x30],RDX`, ISSUE-218), RIP `0x200f19`, `Unsupported { opcode: 137 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for its unrelated two stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact Rust-helper qword stack store (ISSUE-218)

Base `PagedCpl0ProbeV1` now admits only `48 89 54 24 30` (`MOV qword ptr [RSP+0x30],RDX`). The exact decode guard requires REX.W, the `89` store direction, a qword register source RDX, and the RSP/no-index/scale-1/qword `[+0x30]` SIB disp8 address. Execution uses `write_operand` and the tracked GuestMemory write path; success changes only RIP, while the tested same-page supervisor write-protection #PF does not retire or change CPU state. Four-level page-walk Accessed updates persist, the operand remains unchanged, and leaf Dirty stays clear on that denied write. This does not establish arbitrary backend-error rollback or cross-page behavior (ISSUE-054).

Five focused `paged_probe_profile` tests pass (success, injected write page fault, displacement-fetch fault, neighbors and CPL3); all 543 paged-profile tests passed. All od-isa suites passed (2,322 library, 1,591 decoder, 126 execution, 29 boundary, 6 flags, 543 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). The full `od-machine --test paged_probe_quantum` suite passed all 43 tests, including both new store tests: successful four-level tracked qword write with destination/callback/PDE-PTE A/D assertions, plus write-protection fault with precise #PF, no retirement or operand commit, and no Dirty bit. Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, Rust 2024 rustfmt and `git diff --check` passed; cargo check emitted only the existing unused `access_long64_4k_internal_with_writer` warning in `od-machine`. Independent review approved with no remaining findings. The 4 MiB image `/private/tmp/od-fw-s7-04-issue218.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retired 8,901 instructions, then stopped at `48 89 74 24 10` (`MOV qword ptr [RSP+0x10],RSI`, ISSUE-219), RIP `0x200f1e`, `Unsupported { opcode: 137 }`, with `com1_bytes=0`. Full handoff validation remains incomplete; ISSUE-201's two stale CLD-unsupported machine expectations remain unrelated.


#### S7-04 CPU-profile slice: exact Rust-helper qword stack store from RSI (ISSUE-219)

Base `PagedCpl0ProbeV1` now admits only `48 89 74 24 10` (`MOV qword ptr [RSP+0x10],RSI`) alongside the previously admitted exact RDX spill. The new tuple requires qword source RSI, RSP base, no SIB index, scale 1 and exact disp8 `0x10`; it uses the same tracked memory write path. Success changes only RIP. Five focused ISA tests cover the value/address, CPU state, injected write #PF, final-displacement fetch fault, and neighboring source/displacement/addressing/width/prefix/profile forms. Two machine tests verify the four-level tracked write, A/D updates and same-page supervisor write-protection #PF atomicity. These tests do not establish general backend rollback or cross-page behavior (ISSUE-054). All 548 paged-profile tests and all od-isa suites passed (2,322 library, 1,591 decoder, 126 execution, 29 boundary, 6 flags, 548 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). The full `od-machine --test paged_probe_quantum` suite passed all 45 tests. Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, Rust 2024 rustfmt and `git diff --check` passed; cargo check emitted only the existing unused `access_long64_4k_internal_with_writer` warning in `od-machine`. Independent review approved with no findings.

Independent review approved with no remaining findings. Firmware image `/private/tmp/od-fw-s7-04-issue219.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retired 8,902 instructions, then stopped at `41 F6 C5 07` (`TEST R13B,7`, ISSUE-220), RIP `0x200f23`, `Unsupported { opcode: 246 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for the two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact Rust-helper `TEST R13B,7` (ISSUE-220)

Base `PagedCpl0ProbeV1` admits only exact `41 F6 C5 07`. REX.B with ModRM.rm=5 selects R13B, distinct from BPL under REX `40`; TEST uses shared byte logical semantics, writes no GPR, updates CF/PF/ZF/SF/OF, preserves the concrete AF bit while marking AF undefined, and preserves unrelated flags/definedness.

Four focused ISA tests passed for value outcomes, complete GPR retention, flag results/definedness, immediate-fetch fault atomicity, adjacent encodings/prefixes, CPL3 and other-profile isolation; `other_paged_profiles_keep_s7_entry_instructions_unsupported` also passed. All od-isa suites passed (2,322 library, 1,591 decoder, 126 execution, 29 boundary, 6 flags, 552 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, Rust 2024 rustfmt and `git diff --check` passed; cargo check emitted only the existing unused `access_long64_4k_internal_with_writer` warning in `od-machine`. Independent review approved with no remaining findings. Firmware image `/private/tmp/od-fw-s7-04-issue220.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retired 8,903 instructions, then stopped at `74 59` (`JZ rel8 +0x59`, ISSUE-221), RIP `0x200f27`, `Unsupported { opcode: 116 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for the two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact Rust-helper `JZ rel8 +0x59` (ISSUE-221)

Base `PagedCpl0ProbeV1` admits only exact no-prefix `74 59`. The branch shares the existing JZ condition path: reads ZF, preserves flags and definedness, selects the signed +0x59 target or fall-through, and validates canonicality only for a taken target.

Five focused JZ rel8 tests passed with this displacement added to the defined/undefined ZF matrix, taken-only canonicality and rel8 fetch-fault cases; adjacent displacement/condition/prefix/near forms, CPL3 and other-profile isolation were covered. The cross-profile isolation test also passed. All od-isa suites passed (2,322 library, 1,591 decoder, 126 execution, 29 boundary, 6 flags, 552 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, Rust 2024 rustfmt and `git diff --check` passed; cargo check emitted only the existing unused `access_long64_4k_internal_with_writer` warning in `od-machine`. Independent review approved with no remaining findings. Firmware image `/private/tmp/od-fw-s7-04-issue221.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retired 8,904 instructions, then stopped at `48 89 44 24 08` (`MOV qword ptr [RSP+8],RAX`, ISSUE-222), RIP `0x200f82`, `Unsupported { opcode: 137 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for the two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact Rust-helper qword stack store from RAX (ISSUE-222)

Base `PagedCpl0ProbeV1` admits only exact `48 89 44 24 08` (`MOV qword ptr [RSP+8],RAX`) alongside the previously admitted exact RDX and RSI RSP-relative spills. The exact tuple requires RAX source, RSP base, no SIB index, scale 1, disp8 8, and qword address/access sizes. The tracked GuestMemory store preserves GPRs, flags and definedness; only RIP changes on success. General backend rollback and cross-page behavior remain outside evidence (ISSUE-054).

Five focused ISA tests passed (success/state, injected write #PF, displacement-fetch fault, neighbors and CPL3); other-profile isolation passed. Two `paged_probe_quantum` machine tests passed, covering successful four-level tracked storage with A/D assertions and same-page supervisor write-protection fault atomicity. All od-isa suites passed (2,322 library, 1,591 decoder, 126 execution, 29 boundary, 6 flags, 557 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 47 `od-machine --test paged_probe_quantum` tests passed. Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, Rust 2024 rustfmt and `git diff --check` passed; cargo check emitted only the existing unused `access_long64_4k_internal_with_writer` warning in `od-machine`. Independent review approved with no remaining findings. Firmware image `/private/tmp/od-fw-s7-04-issue222.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retired 8,905 instructions, then stopped at `41 81 FD A0 00 00 00` (`CMP R13D,0xA0`, ISSUE-223), RIP `0x200f87`, `Unsupported { opcode: 129 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for the two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact Rust-helper `CMP R13D,0xA0` (ISSUE-223)

Base `PagedCpl0ProbeV1` admits only exact `41 81 FD A0 00 00 00`. REX.B selects R13D through ModRM r/m; opcode `81 /7` and imm32 `0xa0` select dword CMP. Shared arithmetic semantics compare low R13D, update/define CF/PF/AF/ZF/SF/OF, and preserve all GPRs. The upper 32 register bits do not participate.

Four focused tests pass, covering values above 16 bits, ignored upper qword bits, GPR preservation, arithmetic flags/definedness, final-immediate fetch-fault atomicity, neighboring REX/register/immediate/width forms, and CPL3/other-profile isolation; the cross-profile isolation test passes. All od-isa suites passed (2,322 library, 1,591 decoder, 126 execution, 29 boundary, 6 flags, 561 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, Rust 2024 rustfmt and `git diff --check` passed; cargo check emitted only the existing unused `access_long64_4k_internal_with_writer` warning in `od-machine`. Independent review approved with no remaining findings. Firmware image `/private/tmp/od-fw-s7-04-issue223.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retired 8,906 instructions, then stopped at `72 2A` (`JB/JNAE rel8 +0x2a`, ISSUE-224), RIP `0x200f8e`, `Unsupported { opcode: 114 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for the two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact Rust-helper `JB/JNAE rel8 +0x2A` (ISSUE-224)

Base `PagedCpl0ProbeV1` admits only exact no-prefix `72 2A`. The shared JB condition reads CF only, preserves RFLAGS and definedness, selects the signed rel8 target or fall-through, and checks canonicality only for a taken target.

Five focused tests passed for taken/not-taken behavior with defined and undefined CF, full state/definedness retention, taken-only canonicality, rel8 fetch-fault atomicity, neighboring conditions/displacements/prefixes/near form and CPL3; other-profile isolation passed. All od-isa suites passed (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 566 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, `cargo fmt -p od-isa -- --check` and `git diff --check` passed; check emitted only the existing `od-machine` dead-code warning. Workspace-wide `cargo fmt --all -- --check` reports a formatting difference in untouched `crates/od-host/src/storage_abi_v1.rs:139`. Firmware image `/private/tmp/od-fw-s7-04-issue224.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retired 8,907 instructions, then stopped at `4B 8D 04 64` (`LEA RAX,[R12+R12*2]`, ISSUE-225), RIP `0x200f90`, `Unsupported { opcode: 141 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for the two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact Rust-helper `LEA RAX,[R12+R12*2]` (ISSUE-225)

Base `PagedCpl0ProbeV1` admits only exact `4B 8D 04 64`. REX.W selects qword RAX; REX.X/B extend SIB index/base to R12; scale 2 computes wrapping `R12+2*R12`. LEA writes only RAX/RIP and preserves flags/definedness without memory access or canonicality checks.

Four focused tests passed for REX/SIB routing, qword overflow and noncanonical results, flags/definedness, no memory/PIO effects, final-SIB fetch-fault atomicity, neighboring forms and CPL3; other-profile isolation passed. Independent review approved with no remaining findings. All od-isa suites passed (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 570 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, `cargo fmt -p od-isa -- --check` and `git diff --check` passed; check emitted only the existing `od-machine` dead-code warning. Workspace-wide `cargo fmt --all -- --check` reports formatting differences in untouched `crates/od-host/src/storage_abi_v1.rs:139` and `tools/image-tool/tests/cli.rs:109`. Firmware image `/private/tmp/od-fw-s7-04-issue225.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retired 8,908 instructions, then stopped at `48 C1 E0 04` (`SHL RAX,4`, ISSUE-226), RIP `0x200f94`, `Unsupported { opcode: 193 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for the two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact Rust-helper `SHL RAX,4` (ISSUE-226)

Base `PagedCpl0ProbeV1` admits only exact `48 C1 E0 04`, pinning REX.W, qword width, immediate count 4, destination RAX, and raw ModRM `/4` (the `/6` SAL alias is excluded even though the decoder normalizes it to SHL). Shared qword shift semantics update the result and flags; only RAX changes, CF/PF/ZF/SF are defined and AF/OF are undefined for count 4.

Four focused tests passed for edge values, qword results, flags/definedness, only-RAX mutation, final-imm8 fetch-fault atomicity, `/6` and adjacent group/register/count/width/prefix forms, CPL3 and profile isolation. Independent review approved with no remaining findings. All od-isa suites passed (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 574 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, `cargo fmt -p od-isa -- --check` and `git diff --check` passed; check emitted only the existing `od-machine` dead-code warning. Workspace-wide `cargo fmt --all -- --check` reports formatting differences in untouched `crates/od-host/src/storage_abi_v1.rs:139` and `tools/image-tool/tests/cli.rs:109`. Firmware image `/private/tmp/od-fw-s7-04-issue226.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retired 8,909 instructions, then stopped at `4C 01 E8` (`ADD RAX,R13`, ISSUE-227), RIP `0x200f98`, `Unsupported { opcode: 1 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for the two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact Rust-helper `ADD RAX,R13` (ISSUE-227)

Base `PagedCpl0ProbeV1` admits only `4C 01 E8`. REX.W/R and ModRM select qword RAX destination and R13 source in Intel operand order. Shared qword ADD changes RAX and arithmetic flags/definedness while preserving all other GPRs.

Four focused tests passed for carry/overflow/sign/zero, high-bit operands, operand routing, only-RAX mutation, arithmetic flags/definedness, final-ModRM fetch-fault atomicity, neighboring opcode/register/REX/width/prefix/memory forms, CPL3 and profile isolation. Independent review approved with no remaining findings. All od-isa suites passed (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 578 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, `cargo fmt -p od-isa -- --check` and `git diff --check` passed; check emitted only the existing `od-machine` dead-code warning. Workspace-wide `cargo fmt --all -- --check` reports formatting differences in untouched `crates/od-host/src/storage_abi_v1.rs:139` and `tools/image-tool/tests/cli.rs:109`. Firmware image `/private/tmp/od-fw-s7-04-issue227.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retired 8,910 instructions, then stopped at `48 39 E8` (`CMP RAX,RBP`, ISSUE-228), RIP `0x200f9b`, `Unsupported { opcode: 57 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for the two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact Rust-helper `CMP RAX,RBP` (ISSUE-228)

Base `PagedCpl0ProbeV1` admits only exact `48 39 E8`, qword CMP RAX−RBP using opcode 39 Intel destination/source order. Shared arithmetic flags and definedness are updated; CMP writes no GPR.

Four focused tests passed for equality/borrow/overflow, high-bit qword operands, complete register preservation, flags/definedness, final-ModRM fetch-fault atomicity, neighboring forms, CPL3 and profile isolation. Independent review approved with no remaining findings. All od-isa suites passed (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 582 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, `cargo fmt -p od-isa -- --check` and `git diff --check` passed; check emitted only the existing `od-machine` dead-code warning. Workspace-wide `cargo fmt --all -- --check` reports formatting differences in untouched `crates/od-host/src/storage_abi_v1.rs:139` and `tools/image-tool/tests/cli.rs:109`. Firmware image `/private/tmp/od-fw-s7-04-issue228.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retired 8,911 instructions, then stopped at `77 1A` (`JA rel8 +0x1a`, ISSUE-229), RIP `0x200f9e`, `Unsupported { opcode: 119 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for the two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact Rust-helper `JA rel8 +0x1A` (ISSUE-229)

Base `PagedCpl0ProbeV1` admits only exact no-prefix `77 1A`. The shared JA condition reads CF=0 and ZF=0, preserves RFLAGS and definedness, selects the signed rel8 target or fall-through, and checks canonicality only for a taken target.

Five focused tests passed for the CF/ZF truth table with defined and undefined flags, full state/definedness retention, taken-only canonicality, rel8 fetch-fault atomicity, neighboring conditions/displacements/prefixes/near form and CPL3; other-profile isolation passed. Independent review approved with no findings. All od-isa suites passed (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 587 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, `cargo fmt -p od-isa -- --check` and `git diff --check` passed; check emitted only the existing `od-machine` dead-code warning. Workspace-wide `cargo fmt --all -- --check` reports formatting differences in untouched `crates/od-host/src/storage_abi_v1.rs:139` and `tools/image-tool/tests/cli.rs:109`. Firmware image `/private/tmp/od-fw-s7-04-issue229.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retired 8,912 instructions, then stopped at `4C 39 EF` (`CMP RDI,R13`, ISSUE-230), RIP `0x200fa0`, `Unsupported { opcode: 57 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for the two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.

#### S7-04 CPU-profile slice: exact Rust-helper `CMP RDI,R13` (ISSUE-230)

Independent review approved with no remaining findings.


Base `PagedCpl0ProbeV1` admits only exact `4C 39 EF`, qword CMP RDI−R13 using opcode 39 Intel destination/source order and REX.R source selection. Shared arithmetic flags and definedness are updated; CMP writes no GPR.

Four focused tests passed for equality, borrow, signed overflow, high-bit qword operands (including nonzero upper dword), complete register preservation, flags/definedness, final-ModRM fetch-fault atomicity, adjacent register/opcode/width/prefix forms, CPL3 and profile isolation. All od-isa suites passed (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 591 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, `cargo fmt -p od-isa -- --check` and `git diff --check` passed; the check emits only the existing `od-machine` dead-code warning. Workspace-wide `cargo fmt --all -- --check` reports pre-existing formatting differences in untouched `crates/od-host/src/storage_abi_v1.rs:139` and `tools/image-tool/tests/cli.rs:109`. Firmware image `/private/tmp/od-fw-s7-04-issue230.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retired 8,913 instructions, then stopped at `76 22` (`JBE rel8 +0x22`, ISSUE-231), RIP `0x200fa3`, `Unsupported { opcode: 118 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for the two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.

#### S7-04 CPU-profile slice: exact Rust-helper `JBE rel8 +0x22` (ISSUE-231)

Independent review approved with no remaining findings.

Base `PagedCpl0ProbeV1` admits only exact no-prefix `76 22`. The shared JBE condition reads CF=1 or ZF=1, preserves RFLAGS and definedness, chooses the signed rel8 target or fall-through, and checks canonicality only for a taken target.

Five focused tests passed for the CF/ZF truth table with defined and undefined flags, state/definedness retention, taken-only canonicality, rel8 fetch-fault atomicity, adjacent conditions/displacements/prefixes/near form and CPL3; other-profile isolation passed. All od-isa suites passed (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 596 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, `cargo fmt -p od-isa -- --check` and `git diff --check` passed; check emitted only the existing `od-machine` dead-code warning. Workspace-wide `cargo fmt --all -- --check` reports pre-existing differences in untouched `crates/od-host/src/storage_abi_v1.rs:139` and `tools/image-tool/tests/cli.rs:109`. Firmware image `/private/tmp/od-fw-s7-04-issue231.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retired 8,914 instructions, then stopped at `89 1C 24` (`MOV dword ptr [RSP],EBX`, ISSUE-232), RIP `0x200fc7`, `Unsupported { opcode: 137 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for the two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.

#### S7-04 CPU-profile slice: exact Rust-helper `MOV dword ptr [RSP],EBX` (ISSUE-232)

Independent review approved with no remaining findings.

Base `PagedCpl0ProbeV1` admits only exact `89 1C 24`, with ModRM/SIB selecting dword EBX to `[RSP]`. The CPU profile uses the tracked dword write path; GPRs and flags are preserved. The paged probe memory adapter now passes dword and qword write widths to the same tracked page-walk machinery.

Five focused ISA tests passed for the exact store address/value, complete state preservation, injected write/fetch-fault atomicity, neighbor addressing/source/width/prefix forms, CPL3 and profile isolation. Two four-level machine tests passed: success wrote exactly four data bytes, set the expected A/D bits, and preserved CPU state; a supervisor WP fault preserved CPU/data state, set accessed, and left leaf dirty clear. All od-isa suites passed (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 601 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); `cargo test --offline -p od-machine --test paged_probe_quantum --quiet` passed all 49 tests. Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, `cargo fmt -p od-isa -p od-machine -- --check` and `git diff --check` passed; the check emits only the existing `od-machine` dead-code warning. Workspace-wide `cargo fmt --all -- --check` reports pre-existing formatting differences in untouched `crates/od-host/src/storage_abi_v1.rs:139` and `tools/image-tool/tests/cli.rs:109`. Firmware image `/private/tmp/od-fw-s7-04-issue232.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retired 8,915 instructions, then stopped at `48 8D 7C 24 18` (`LEA RDI,[RSP+0x18]`, ISSUE-233), RIP `0x200fca`, `Unsupported { opcode: 141 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for the two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact Rust-helper `LEA RDI,[RSP+0x18]` (ISSUE-233)

Base `PagedCpl0ProbeV1` admits only exact `48 8D 7C 24 18`. REX.W selects a qword destination; ModRM selects RDI and SIB selects RSP as base with no index; disp8 `0x18` produces the effective offset written to RDI. LEA performs no memory access and no canonicality check. Flags/definedness and all other GPRs are preserved. Other displacements, indexes (including REX.X), widths, prefixes, CPL3 and other profiles remain outside this exact admission.

Four focused paged-profile tests pass for effective-address calculation and state preservation, instruction-fetch-fault atomicity, neighboring displacement/index/width/prefix forms, and CPL3 rejection; the profile-isolation suite passes. All od-isa tests pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 601 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). `cargo test --offline -p od-machine --test paged_probe_quantum --quiet` passes all 49 tests. Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, `cargo fmt -p od-isa -p od-machine -- --check` and `git diff --check` pass; check emits only the existing `od-machine` dead-code warning. Firmware image `/private/tmp/od-fw-s7-04-issue233.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,916 instructions, then stops at `48 8B 74 24 10` (`MOV RSI,qword ptr [RSP+0x10]`, ISSUE-234), RIP `0x200fcf`, `Unsupported { opcode: 139 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for the two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact Rust-helper `MOV RSI,[RSP+0x10]` (ISSUE-234)

Base `PagedCpl0ProbeV1` admits only exact `48 8B 74 24 10`. REX.W and opcode 8B select qword load; ModRM selects RSI and SIB selects RSP base with no index and disp8 `0x10`. The load uses the tracked memory read path. It commits RSI and RIP only after success, preserving all other GPRs and flags/definedness; a data fault leaves state unchanged. Neighboring encodings, widths, prefixes, CPL3 and other profiles remain outside this admission.

Six focused profile tests pass for qword load/state, injected data and fetch fault atomicity, noncanonical and canonical-boundary #SS(0) atomicity, neighboring addressing/register/width/prefix forms and CPL3; other-profile isolation passes. A four-level machine test passes: RSI receives the qword and page-table accessed bits are set, while data dirty bits remain clear. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 607 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). The full `paged_probe_quantum` suite passes all 50 tests. Scoped check, format and diff checks pass; check emits the existing `od-machine` dead-code warning. Firmware image `/private/tmp/od-fw-s7-04-issue234.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,917 instructions, then stops at `48 8B 54 24 30` (`MOV RDX,qword ptr [RSP+0x30]`, ISSUE-235), RIP `0x200fd4`, `Unsupported { opcode: 139 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for the two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact Rust-helper `MOV RDX,[RSP+0x30]` (ISSUE-235)

Base `PagedCpl0ProbeV1` admits only exact `48 8B 54 24 30`. REX.W and opcode 8B select qword load; ModRM selects RDX and SIB selects RSP base with no index and disp8 `0x30`. The load uses the tracked memory read path. It commits RDX and RIP only after success, preserving all other GPRs and flags/definedness; a data fault leaves state unchanged. Neighboring encodings, widths, prefixes, CPL3 and other profiles remain outside this admission.

Five focused profile tests pass for qword load/state, injected data and fetch fault atomicity, neighboring destination/addressing/width/prefix forms and CPL3; other-profile isolation passes. A four-level machine test passes: RDX receives the qword, page-table accessed bits are set, and the data leaf dirty bit remains clear. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 612 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). The full `paged_probe_quantum` suite passes all 51 tests. Scoped cargo check, format and diff checks pass; check emits the existing `od-machine` dead-code warning. Firmware image `/private/tmp/od-fw-s7-04-issue235.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,918 instructions, then stops at `4C 89 F9` (`MOV RCX,R15`, ISSUE-236), RIP `0x200fd9`, `Unsupported { opcode: 137 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for the two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact Rust-helper `MOV RCX,R15` (ISSUE-236)

Base `PagedCpl0ProbeV1` admits only exact `4C 89 F9`. REX.W selects qword width, REX.R selects R15 as source, and ModRM selects RCX as destination. The complete 64-bit value is copied; other GPRs and flags/definedness are preserved. Other register moves, REX forms, opcode direction, widths, prefixes, CPL3 and profiles remain outside this exact admission.

Four focused profile tests pass for high-bit qword transfer/state preservation, instruction-fetch-fault atomicity, neighboring register/REX/opcode/width/prefix forms and CPL3; other-profile isolation passes. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 616 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). Scoped check, format and diff checks pass. Firmware image `/private/tmp/od-fw-s7-04-issue236.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,919 instructions, then stops at `4D 89 F0` (`MOV R8,R14`, ISSUE-237), RIP `0x200fdc`, `Unsupported { opcode: 137 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for the two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact Rust-helper `MOV R8,R14` (ISSUE-237)

Base `PagedCpl0ProbeV1` admits only exact `4D 89 F0`. REX.W selects qword width, REX.R extends the ModRM reg source to R14 and REX.B extends the r/m destination to R8. The complete 64-bit value is copied; other GPRs and flags/definedness are preserved. Other register/REX forms, opcode direction, widths, prefixes, CPL3 and profiles remain outside this exact admission.

Four focused profile tests pass for high-bit qword transfer/state preservation, instruction-fetch-fault atomicity, neighboring register/REX/opcode/width/prefix forms and CPL3; other-profile isolation passes. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 620 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). Scoped check, format and diff checks pass. Firmware image `/private/tmp/od-fw-s7-04-issue237.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,920 instructions, then stops at `4D 89 E1` (`MOV R9,R12`, ISSUE-238), RIP `0x200fdf`, `Unsupported { opcode: 137 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for the two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `JZ rel32 +0x149` (ISSUE-242)

Base `PagedCpl0ProbeV1` admits only exact no-prefix `0F 84 49 01 00 00`. It adds the signed rel32 displacement `+0x149` to next RIP. At firmware RIP `0x20156e`, taken target is `0x2016bd` and fall-through is `0x201574`. Only ZF is read; flags/definedness are preserved, and canonicality is checked only for a taken target. Fetch faults do not retire or mutate CPU state.

Five focused tests cover ZF/definedness states, exact target/fall-through, taken-only canonicality (including a noncanonical target), final-byte fetch-fault atomicity, adjacent displacement/condition/prefix forms, CPL3 and profile isolation. Full od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 639 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 55 `paged_probe_quantum` tests pass. Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, `cargo fmt --check`, and `git diff --check` pass. The check reports the existing page-walk dead-code warning. Independent review approved with no findings.

Firmware image `/private/tmp/od-fw-s7-04-issue242.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,931 instructions and stops at `48 89 F0` (`MOV RAX,RSI`, ISSUE-243), RIP `0x201574`, `Unsupported { opcode: 137 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.

#### S7-04 CPU-profile slice: exact helper `TEST R8,R8` (ISSUE-241)

Base `PagedCpl0ProbeV1` admits only exact `4D 85 C0`, a qword `TEST R8,R8`. It writes no GPR result; CF/OF are cleared, SF/ZF/PF follow the full-width logical result, and AF is marked undefined while retaining its concrete value. CF/PF/ZF/SF/OF become defined and AF becomes undefined; definedness of all unrelated flags is preserved. All unrelated flag values and all GPRs remain unchanged.

Four focused profile tests cover zero/parity/sign/all-ones cases, full CPU state and flag-definedness, fetch-fault atomicity, neighboring registers/widths/prefixes, CPL3 and other-profile isolation. The other-profile isolation test also passes. Full od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 634 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 55 `paged_probe_quantum` tests pass. Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, `cargo fmt --check`, and `git diff --check` pass. The check reports the existing page-walk dead-code warning. Independent review approved with no findings.

Firmware image `/private/tmp/od-fw-s7-04-issue241.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,930 instructions and stops at `0F 84 49 01 00 00` (`JZ rel32 +0x149`, ISSUE-242), RIP `0x20156e`, `Unsupported { opcode: 132 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.

#### S7-04 CPU-profile slice: exact helper `PUSH RAX` (ISSUE-240)

Base `PagedCpl0ProbeV1` admits only exact bare opcode `50` as qword `PUSH RAX`. The original 64-bit RAX value is written to `[RSP-8]` through the tracked stack path; RSP and RIP commit only after success, with all other GPRs and flags/definedness preserved.

Four focused profile tests cover high-bit qword data/state, #SS/#PF/fetch-fault atomicity, neighboring registers and prefixes, CPL3 rejection, and other-profile isolation. Two four-level machine tests cover successful A/D updates and same-page supervisor write-protection fault atomicity. Full od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 630 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 55 `paged_probe_quantum` tests pass. Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, `cargo fmt --check`, and `git diff --check` pass. The check reports the existing page-walk dead-code warning. Independent review approved with no findings.

Firmware image `/private/tmp/od-fw-s7-04-issue240.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,929 instructions and stops at `4D 85 C0` (`TEST R8,R8`, ISSUE-241), RIP `0x20156b`, `Unsupported { opcode: 133 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.

#### S7-04 CPU-profile slice: exact Rust-helper `CALL rel32 +0x579` (ISSUE-239)

Base `PagedCpl0ProbeV1` admits only `E8 79 05 00 00` at RIP `0x200fe2`. The instruction targets `0x201560` and pushes return RIP `0x200fe7` to `[RSP-8]` through the existing tracked qword store path; RSP and RIP commit only after the write succeeds.

Focused profile tests cover target calculation, exact return-address bytes, state and flags/definedness retention, fetch/stack faults, adjacent displacement and prefix forms, CPL3 rejection, and isolation from other paged profiles. Four-level machine tests verify stack-store A/D behavior and same-page supervisor write-protection fault atomicity. Independent review approved with no findings. Full od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 626 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 53 `paged_probe_quantum` machine tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped `cargo fmt --check`, and `git diff --check` pass. The check reports the existing `access_long64_4k_internal_with_writer` dead-code warning.

Firmware image `/private/tmp/od-fw-s7-04-issue239.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,928 instructions and stops at `50` (`PUSH RAX`, ISSUE-240), RIP `0x20156a`, `Unsupported { opcode: 80 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.

#### S7-04 CPU-profile slice: exact Rust-helper `MOV R9,R12` (ISSUE-238)

Base `PagedCpl0ProbeV1` admits only exact `4D 89 E1`. REX.W selects qword width, REX.R extends the ModRM reg source to R12 and REX.B extends the r/m destination to R9. The complete 64-bit value is copied; other GPRs and flags/definedness are preserved. No additional operand width is admitted by this slice; the pre-existing exact dword `45 89 E1` (`MOV R9D,R12D`) remains separately supported. Other register/REX forms, opcode direction, prefixes, CPL3 and profiles remain outside this exact admission.

Four focused profile tests pass for high-bit qword transfer/state preservation, instruction-fetch-fault atomicity, neighboring register/REX/opcode/width/prefix forms and CPL3; other-profile isolation passes. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 624 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). Scoped check, format and diff checks pass. Firmware image `/private/tmp/od-fw-s7-04-issue238.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,921 instructions, then stops at `E8 79 05 00 00` (`CALL rel32 +0x579`, ISSUE-239), RIP `0x200fe2`, `Unsupported { opcode: 232 }`, with `com1_bytes=0`. Full handoff validation remains incomplete. ISSUE-201 remains open for the two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `MOV RAX,RSI` (ISSUE-243)

Base `PagedCpl0ProbeV1` admits only exact `48 89 F0`. Opcode 89 selects the ModRM reg source RSI and r/m destination RAX; the operation copies the complete qword and preserves all other GPRs, RFLAGS and definedness. Final-byte instruction-fetch faults do not retire or mutate state. Neighboring direction/register/REX/width/prefix forms, CPL3, and other paged profiles remain unsupported.

Four focused profile tests cover high-bit transfer/state preservation, fetch-fault atomicity, neighbors, CPL3 and profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 643 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 55 `paged_probe_quantum` machine tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets` and `git diff --check` pass. scoped `cargo fmt -p od-isa -p od-machine -p od-runner -- --check` and `git diff --check` pass; independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue243.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,932 instructions and stops at `48 8D 71 04` (`LEA RSI,[RCX+4]`, ISSUE-244), RIP `0x201577`, `Unsupported { opcode: 141 }`, with `com1_bytes=0`. Full od-isa, paged-quantum, cargo check and scoped formatting checks pass; independent review approved with no findings. ISSUE-201 remains open for two unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `LEA RSI,[RCX+4]` (ISSUE-244)

Base `PagedCpl0ProbeV1` admits only exact `48 8D 71 04`. This is qword LEA with RCX base and signed disp8 +4. It writes the wrapping effective offset to RSI without memory access or canonicality checking; other GPRs, flags and definedness remain unchanged. A fetch fault on the displacement byte does not retire or mutate CPU state. Neighboring registers/displacements/widths/prefixes, CPL3, and other profiles remain unsupported.

Four focused profile tests cover high and noncanonical offsets, wraparound, full-state and flag-definedness preservation, fetch-fault atomicity, neighboring forms, CPL3 and profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 647 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 55 `paged_probe_quantum` machine tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets` passes. Scoped formatting and diff checks pass; independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue244.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,933 instructions and stops at `48 39 D6` (`CMP RSI,RDX`, ISSUE-245), RIP `0x20157B`, `Unsupported { opcode: 57 }`, with `com1_bytes=0`. ISSUE-201 remains open for the unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `CMP RSI,RDX` (ISSUE-245)

Base `PagedCpl0ProbeV1` admits only exact `48 39 D6`. Opcode 39 compares qword r/m destination RSI against ModRM reg source RDX using shared RSI−RDX CMP semantics. No GPR is written; arithmetic flags and definedness are updated. Final-byte instruction-fetch faults do not retire or mutate CPU state. Neighbor register orders, reverse opcode, widths, prefixes, CPL3 and other paged profiles remain unsupported.

Four focused profile tests cover equality, carry, overflow, sign/parity/AF, high-bit correctness, GPR preservation, flag definedness, fetch-fault atomicity, neighboring forms, CPL3 and profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 651 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 55 `paged_probe_quantum` machine tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets` passes. Scoped `cargo fmt -p od-isa -p od-machine -p od-runner -- --check` and `git diff --check` pass; independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue245.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,934 instructions and stops at `0F 87 CD 02 00 00` (`JA rel32 +0x2cd`, ISSUE-246), RIP `0x20157E`, `Unsupported { opcode: 135 }`, with `com1_bytes=0`. ISSUE-201 remains open for the unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `JA rel32 +0x2cd` (ISSUE-246)

Base `PagedCpl0ProbeV1` admits only exact no-prefix `0F 87 CD 02 00 00`. The signed rel32 `+0x2cd` is added to next RIP; JA tests CF=0 and ZF=0. RFLAGS and definedness remain unchanged. The target is checked for canonicality only when taken. Fetch faults at any instruction byte do not retire or mutate state. Adjacent displacements/conditions, prefixes, short JA, JMP, CPL3 and other paged profiles remain unsupported.

Five focused profile tests cover all CF/ZF combinations and definedness, taken/fall-through RIP, taken-only canonicality, final-byte fetch-fault atomicity, neighboring forms, CPL3 and profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 656 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 55 `paged_probe_quantum` machine tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets` passes. Scoped `cargo fmt -p od-isa -p od-machine -p od-runner -- --check` and `git diff --check` pass; independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue246.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,935 instructions and stops at `4C 8D 51 08` (`LEA R10,[RCX+8]`, ISSUE-247), RIP `0x201584`, `Unsupported { opcode: 141 }`, with `com1_bytes=0`. ISSUE-201 remains open for the unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `LEA R10,[RCX+8]` (ISSUE-247)

Base `PagedCpl0ProbeV1` admits only exact `4C 8D 51 08`. REX.R selects R10 as a qword destination; RCX plus signed disp8 +8 is computed with wrapping and no memory access or canonicality check. Only R10/RIP change, preserving flags, definedness and all other GPRs. A final-byte fetch fault does not retire or mutate state. Neighboring registers/displacements/widths/prefixes, CPL3 and other profiles remain unsupported.

Four focused profile tests cover high/noncanonical inputs, wrapping, full-state preservation, memory non-access, fetch-fault atomicity, neighboring forms, CPL3 and profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 660 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 55 `paged_probe_quantum` machine tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets` passes. Scoped `cargo fmt -p od-isa -p od-machine -p od-runner -- --check` and `git diff --check` pass; independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue247.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,936 instructions and stops at `49 39 D2` (`CMP R10,RDX`, ISSUE-248), RIP `0x201588`, `Unsupported { opcode: 57 }`, with `com1_bytes=0`. ISSUE-201 remains open for the unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `CMP R10,RDX` (ISSUE-248)

Base `PagedCpl0ProbeV1` admits only exact `49 39 D2`. REX.B selects qword R10 as the r/m destination and ModRM reg selects RDX as source; shared CMP evaluates R10−RDX. No GPR is written; arithmetic flags and definedness are updated. A final-byte instruction-fetch fault does not retire or mutate state. Neighboring register/order/REX/width/prefix forms, CPL3 and other paged profiles remain unsupported.

Four focused profile tests cover equality, carry, overflow, sign/parity/AF, high-bit correctness, GPR preservation, flag definedness, fetch-fault atomicity, neighboring forms, CPL3 and profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 664 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 55 `paged_probe_quantum` machine tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets` passes. Scoped `cargo fmt -p od-isa -p od-machine -p od-runner -- --check` and `git diff --check` pass; independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue248.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,937 instructions and stops at `0F 87 D7 02 00 00` (`JA rel32 +0x2d7`, ISSUE-249), RIP `0x20158B`, `Unsupported { opcode: 135 }`, with `com1_bytes=0`. ISSUE-201 remains open for the unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `JA rel32 +0x2d7` (ISSUE-249)

Base `PagedCpl0ProbeV1` admits only exact no-prefix `0F 87 D7 02 00 00`. The signed rel32 `+0x2d7` is added to next RIP; JA tests CF=0 and ZF=0. RFLAGS and definedness remain unchanged, and canonicality is checked only for a taken target. Final-byte fetch faults do not retire or mutate state. Adjacent displacements/conditions, operand/REX prefixes, short branches, CPL3 and other paged profiles remain unsupported.

Five focused profile tests cover the CF/ZF truth table and definedness masks, target choice, taken-only canonicality, fetch-fault atomicity, neighboring forms, CPL3 and profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 669 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting, and `git diff --check` pass. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue249.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,938 instructions and stops at `48 8D 71 10` (`LEA RSI,[RCX+0x10]`, ISSUE-250), RIP `0x201591`, `Unsupported { opcode: 141 }`, with `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `LEA RSI,[RCX+0x10]` (ISSUE-250)

Base `PagedCpl0ProbeV1` admits only exact `48 8D 71 10`. This qword LEA adds disp8 +0x10 to RCX with wrapping and writes RSI; it performs no memory access or canonicality check. Other GPRs, RFLAGS and definedness remain unchanged. A fetch fault on the displacement byte does not retire or mutate CPU state. Neighboring displacement/base/destination/width/prefix/SIB forms, CPL3 and other paged profiles remain unsupported.

Four focused profile tests cover ordinary/high/noncanonical/wrapping addresses, RSI-only mutation, flag-definedness preservation, no memory or I/O access, final-byte fetch-fault atomicity, neighbors, CPL3 and profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 673 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). All 55 `paged_probe_quantum` tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue250.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,940 instructions and stops at `0F 87 E4 02 00 00` (`JA rel32 +0x2e4`, ISSUE-251), RIP `0x201598`, `Unsupported { opcode: 135 }`, with `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `JA rel32 +0x2e4` (ISSUE-251)

Base `PagedCpl0ProbeV1` admits only exact no-prefix `0F 87 E4 02 00 00`. The signed rel32 `+0x2e4` is added to next RIP; JA tests CF=0 and ZF=0. RFLAGS and definedness remain unchanged, and canonicality is checked only for a taken target. Final-byte fetch faults do not retire or mutate state. Adjacent displacements/conditions, operand/REX prefixes, short branches, CPL3 and other paged profiles remain unsupported.

Five focused profile tests cover the CF/ZF truth table and definedness masks, target choice, taken-only canonicality, fetch-fault atomicity, neighboring forms, CPL3 and profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 678 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 55 `paged_probe_quantum` tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue251.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,941 instructions and stops at `4C 8D 51 18` (`LEA R10,[RCX+0x18]`, ISSUE-252), RIP `0x20159E`, `Unsupported { opcode: 141 }`, with `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `LEA R10,[RCX+0x18]` (ISSUE-252)

Base `PagedCpl0ProbeV1` admits only exact `4C 8D 51 18`. REX.R selects qword R10. This LEA adds disp8 +0x18 to RCX with wrapping; it performs no memory access or canonicality check. Other GPRs, flags and definedness remain unchanged. A final-byte fetch fault does not retire or mutate state. Neighboring displacement/base/destination/REX/width/prefix/SIB forms, CPL3 and other paged profiles remain unsupported.

Four focused profile tests cover ordinary/high/noncanonical/wrapping values, R10-only mutation, flags/definedness preservation, no memory or I/O access, final-byte fetch-fault atomicity, neighbors, CPL3 and profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 682 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 55 `paged_probe_quantum` tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting, and `git diff --check` pass. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue252.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,943 instructions and stops at `0F 87 EE 02 00 00` (`JA rel32 +0x2ee`, ISSUE-253), RIP `0x2015A5`, `Unsupported { opcode: 135 }`, with `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `JA rel32 +0x2ee` (ISSUE-253)

Base `PagedCpl0ProbeV1` admits only exact no-prefix `0F 87 EE 02 00 00`. The signed rel32 `+0x2ee` is added to next RIP; JA tests CF=0 and ZF=0. RFLAGS and definedness remain unchanged, and canonicality is checked only for a taken target. Final-byte fetch faults do not retire or mutate state. Adjacent displacements/conditions, operand/REX prefixes, short branches, CPL3 and other paged profiles remain unsupported.

Five focused profile tests cover the CF/ZF truth table and definedness masks, target choice, taken-only canonicality, fetch-fault atomicity, neighboring forms, CPL3 and profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 687 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 55 `paged_probe_quantum` tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue253.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,944 instructions and stops at `48 8D 71 1C` (`LEA RSI,[RCX+0x1c]`, ISSUE-254), RIP `0x2015AB`, `Unsupported { opcode: 141 }`, with `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `LEA RSI,[RCX+0x1c]` (ISSUE-254)

Base `PagedCpl0ProbeV1` admits only exact `48 8D 71 1C`. This qword LEA adds disp8 +0x1c to RCX with wrapping and writes RSI; it performs no memory access or canonicality check. Other GPRs, RFLAGS and definedness remain unchanged. A fetch fault on the displacement byte does not retire or mutate state. Neighboring displacement/base/destination/width/prefix/SIB forms, CPL3 and other paged profiles remain unsupported.

Four focused profile tests cover ordinary/high/noncanonical/wrapping addresses, RSI-only mutation, flags/definedness preservation, no memory or I/O access, final-byte fetch-fault atomicity, neighbors, CPL3 and profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 691 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 55 `paged_probe_quantum` tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting, and `git diff --check` pass. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue254.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,946 instructions and stops at `0F 87 FB 02 00 00` (`JA rel32 +0x2fb`, ISSUE-255), RIP `0x2015B2`, `Unsupported { opcode: 135 }`, with `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `JA rel32 +0x2fb` (ISSUE-255)

Base `PagedCpl0ProbeV1` admits only exact no-prefix `0F 87 FB 02 00 00`. The signed rel32 `+0x2fb` is added to next RIP; JA tests CF=0 and ZF=0. RFLAGS and definedness remain unchanged, and canonicality is checked only for a taken target. Final-byte fetch faults do not retire or mutate state. Adjacent displacements/conditions, operand/REX prefixes, short branches, CPL3 and other paged profiles remain unsupported.

Five focused profile tests cover the CF/ZF truth table and definedness masks, target choice, taken-only canonicality, fetch-fault atomicity, neighboring forms, CPL3 and profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 696 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 55 `paged_probe_quantum` tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue255.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,947 instructions and stops at `4C 8D 51 20` (`LEA R10,[RCX+0x20]`, ISSUE-256), RIP `0x2015B8`, `Unsupported { opcode: 141 }`, with `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `LEA R10,[RCX+0x20]` (ISSUE-256)

Base `PagedCpl0ProbeV1` admits only exact `4C 8D 51 20`. REX.R selects qword R10. This LEA adds disp8 +0x20 to RCX with wrapping; it performs no memory access or canonicality check. Other GPRs, flags and definedness remain unchanged. A final-byte fetch fault does not retire or mutate state. Neighboring displacement/base/destination/REX/width/prefix/SIB forms, CPL3 and other paged profiles remain unsupported.

Four focused profile tests cover ordinary/high/noncanonical/wrapping inputs, R10-only mutation, flags/definedness preservation, no memory or I/O access, final-byte fetch-fault atomicity, neighbors, CPL3 and profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 700 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 55 `paged_probe_quantum` tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,949 instructions and stops at `0F 87 A3 02 00 00` (`JA rel32 +0x2a3`, ISSUE-257), RIP `0x2015BF`, `Unsupported { opcode: 135 }`, with `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `JA rel32 +0x2a3` (ISSUE-257)

Base `PagedCpl0ProbeV1` admits only exact no-prefix `0F 87 A3 02 00 00`. The signed rel32 `+0x2a3` is added to next RIP; JA is taken iff CF=0 and ZF=0. RFLAGS and definedness remain unchanged; target canonicality is checked only when taken. Final-byte instruction-fetch faults do not retire or mutate state. Neighboring displacements/conditions, operand/REX prefixes, short branches, CPL3 and other paged profiles remain unsupported.

Five focused profile tests cover all CF/ZF combinations and definedness masks, taken/fall-through RIP, taken-only canonicality, fetch-fault atomicity, neighboring forms, CPL3 and profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 705 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 55 `paged_probe_quantum` tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,950 instructions and stops at `0F B7 5C 24 40` (`MOVZX EBX,word ptr [RSP+0x40]`, ISSUE-258), RIP `0x2015C5`, `Unsupported { opcode: 183 }`, with `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `MOVZX EBX,word ptr [RSP+0x40]` (ISSUE-258)

Base `PagedCpl0ProbeV1` admits only exact no-prefix `0F B7 5C 24 40`. The tracked word read at `[RSP+0x40]` is zero-extended into EBX. Only EBX/RIP change; RFLAGS and definedness are preserved, with register/RIP commit only after a successful read. A data read fault or final-byte instruction-fetch fault is atomic. A noncanonical RSP-based address raises #SS before data access. Neighboring destination/displacement/SIB/base/width/prefix forms, CPL3 and other profiles remain unsupported.

Six focused profile tests cover successful read and zero-extension, read faults, noncanonical RSP, final-byte fetch-fault atomicity, neighbors, CPL3 and profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 711 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 55 `paged_probe_quantum` tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,951 instructions and stops at `44 8B 14 08` (`MOV R10D,dword ptr [RAX+RCX]`, ISSUE-259), RIP `0x2015CA`, `Unsupported { opcode: 139 }`, with `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `MOV R10D,dword ptr [RAX+RCX]` (ISSUE-259)

Base `PagedCpl0ProbeV1` admits only exact `44 8B 14 08`. REX.R selects R10D; the SIB is scale 1 with RAX base, RCX index, no displacement and 64-bit address size. The tracked dword read zero-extends into R10D. Only R10/RIP change; RFLAGS/definedness are preserved, and no state commits on a data or instruction-fetch fault. A noncanonical effective address faults before data access. Neighboring base/index/scale/displacement/REX/prefix/width forms, CPL3 and other paged profiles remain unsupported.

Six focused profile tests cover register routing, effective address, zero-extension, preserved state, data/noncanonical/fetch faults, neighbors and CPL3/profile isolation. A four-level machine test verifies successful data read and R10D result, page-table accessed bits, and no dirty/data writes. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 717 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 56 `paged_probe_quantum` tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,952 instructions and stops at `66 83 FB 02` (`CMP BX,2`, ISSUE-260), RIP `0x2015CE`, `Unsupported { opcode: 131 }`, with `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `CMP BX,2` (ISSUE-260)

Base `PagedCpl0ProbeV1` admits only exact `66 83 FB 02`. Operand-size override selects the 16-bit BX destination; opcode `83 /7` sign-extends imm8 `0x02` to word width and applies shared 16-bit CMP semantics. Arithmetic flags/definedness update; no GPR or memory is written. The final-byte fetch fault is atomic. This slice adds only BX,2 and retains the previously admitted exact BX,3 and DI,1 forms. Other immediate values, registers/groups, widths, prefixes, memory forms, CPL3 and other paged profiles remain unsupported.

Four focused profile tests use different nonzero upper RBX patterns to catch accidental wider comparisons, verify flags/definedness, fetch-fault atomicity, neighbors, CPL3 and profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 721 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 56 `paged_probe_quantum` tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,953 instructions and stops at `40 0F 92 C5` (`SETB BPL`, ISSUE-261), RIP `0x2015D2`, `Unsupported { opcode: 146 }`, with `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `SETB BPL` (ISSUE-261)

Base `PagedCpl0ProbeV1` admits only exact `40 0F 92 C5`. Bare REX `0x40` selects BPL instead of the legacy high-byte register. SETB reads CF only and writes 0/1 to RBP's low byte, preserving upper 56 bits. RFLAGS and definedness are unchanged. A fetch fault on the final ModRM byte is atomic. This slice adds only exact SETB BPL and retains previously admitted exact SETcc forms. Other conditions/registers, ModRM/REX/prefix variants, memory forms, CPL3 and other paged profiles remain unsupported.

Four focused tests cover both CF values with defined/undefined CF, ZF independence, partial-register update, flags/definedness preservation, final-byte fetch-fault atomicity, neighboring encodings, CPL3 and profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 725 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 56 `paged_probe_quantum` tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,954 instructions and stops at `45 8D 7A F3` (`LEA R15D,[R10-0x0d]`, ISSUE-262), RIP `0x2015D6`, `Unsupported { opcode: 141 }`, with `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `LEA R15D,[R10-0x0d]` (ISSUE-262)

Base `PagedCpl0ProbeV1` admits only exact `45 8D 7A F3`. REX.R/B select R15D and R10. The instruction computes R10 plus signed disp8 `-0x0d` using 64-bit wrapping address arithmetic, then writes the low dword to R15D, zero-extending into R15. LEA makes no memory access or canonicality check; other GPRs, RFLAGS and definedness are preserved. A final-byte fetch fault is atomic. Neighboring displacement/register/base/REX/width/prefix forms, CPL3 and other paged profiles remain unsupported.

Four focused tests cover ordinary, wrapping and noncanonical inputs; dword truncation and zero-extension; preserved state/no memory or I/O; fetch-fault atomicity; neighbors; and CPL3/profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 729 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 56 `paged_probe_quantum` tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,955 instructions and stops at `41 83 FA 0B` (`CMP R10D,11`, ISSUE-263), RIP `0x2015DA`, `Unsupported { opcode: 131 }`, with `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `CMP R10D,0x0b` (ISSUE-263)

Base `PagedCpl0ProbeV1` admits only exact `41 83 FA 0B`. REX.B extends ModRM r/m=2 to R10D. Opcode `83 /7` sign-extends imm8 `0x0b` to 32 bits, applies shared dword CMP arithmetic flags/definedness, and changes no GPR or memory. Fault while fetching the immediate byte is atomic. Other immediates, registers/groups, widths, prefixes, memory forms, CPL3 and other paged profiles remain unsupported.

Four focused tests cover register routing, dword-specific values including a case that differs from word comparison, arithmetic flags/definedness, GPR preservation, immediate-fetch fault atomicity, neighboring forms, and CPL3/profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 733 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 56 `paged_probe_quantum` tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,956 instructions and stops at `41 0F 93 C6` (`SETAE R14B`, ISSUE-264), RIP `0x2015DE`, `Unsupported { opcode: 147 }`, with `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `SETAE R14B` (ISSUE-264)

Base `PagedCpl0ProbeV1` admits only exact `41 0F 93 C6`. REX.B extends ModRM r/m=6 to R14B. SETAE reads CF only, writing one when clear and zero when set into the low byte of R14. The upper 56 bits, flags and definedness remain unchanged, including when CF is undefined. The final ModRM-byte fetch fault is atomic. Other conditions/registers, ModRM/REX/prefix forms, memory destinations, CPL3 and other paged profiles remain unsupported.

Four focused tests cover both CF values with defined and undefined CF, partial-register update, preserved state/flags/definedness, fetch-fault atomicity, neighboring encodings and CPL3/profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 737 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 56 `paged_probe_quantum` tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,958 instructions and stops at `41 83 FF F4` (`CMP R15D,-0x0c`, ISSUE-265), RIP `0x2015E5`, `Unsupported { opcode: 131 }`, with `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `CMP R15D,-0x0c` (ISSUE-265)

Base `PagedCpl0ProbeV1` admits only exact `41 83 FF F4`. REX.B selects R15D; opcode `83 /7` sign-extends imm8 `0xf4` to dword -12 and applies shared dword CMP arithmetic flags/definedness without GPR or memory writes. A fetch fault on the immediate byte is atomic. Other immediates, registers/groups, widths, prefixes, memory forms, CPL3 and other paged profiles remain unsupported.

Four focused tests cover signed imm8 behavior and dword-vs-word distinction, flags/definedness, GPR preservation, final-byte fetch-fault atomicity, neighbors, and CPL3/profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 741 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 56 `paged_probe_quantum` tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,959 instructions and stops at `0F 82 30 02 00 00` (`JB rel32 +0x230`, ISSUE-266), RIP `0x2015E9`, `Unsupported { opcode: 130 }`, with `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `JB rel32 +0x230` (ISSUE-266)

Base `PagedCpl0ProbeV1` admits only exact no-prefix `0F 82 30 02 00 00`. JB evaluates CF only and, when taken, adds signed rel32 `+0x230` to next RIP. Flags and definedness are preserved; target canonicality is checked only when taken. Fetch fault on the final displacement byte is atomic. Other conditions/displacements, prefixes, short branches, CPL3 and other paged profiles remain unsupported.

Five focused tests cover CF/ZF combinations and CF definedness, target/fall-through, taken-only canonicality, final-byte fetch-fault atomicity, condition/displacement/prefix/short/JMP neighbors and CPL3/profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 746 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 56 `paged_probe_quantum` tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,960 instructions and stops at `44 20 F5` (`AND BPL,R14B`, ISSUE-267), RIP `0x2015EF`, `Unsupported { opcode: 32 }`, with `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `AND BPL,R14B` (ISSUE-267)

Base `PagedCpl0ProbeV1` admits only exact `44 20 F5`. REX.R selects R14B source, and REX presence selects BPL destination. The byte AND writes only BPL, clears CF/OF, updates SF/ZF/PF from the result, and marks AF undefined while retaining its concrete bit; unrelated flags and other state are preserved. Final ModRM-byte fetch fault is atomic. Other register orders, prefixes, REX forms, memory operands, CPL3 and other paged profiles remain unsupported.

Four focused tests cover register routing, byte results/logical flags, AF definedness, partial-register and full-state preservation, no memory/I/O, fetch-fault atomicity, neighbors, and CPL3/profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 750 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 56 `paged_probe_quantum` tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,961 instructions and stops at `0F 85 27 02 00 00` (`JNZ rel32 +0x227`, ISSUE-268), RIP `0x2015F2`, `Unsupported { opcode: 133 }`, with `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in the full `od-machine` suite.


#### S7-04 CPU-profile slice: exact helper `JNZ rel32 +0x227` (ISSUE-268)

Base `PagedCpl0ProbeV1` admits only exact no-prefix `0F 85 27 02 00 00`. JNZ reads ZF; ZF=0 takes the signed rel32 `+0x227` from next RIP, while ZF=1 falls through. RFLAGS and definedness are preserved, and target canonicality is checked only when taken. A final-displacement-byte fetch fault is atomic. Other displacements/conditions, prefixes, short branches, CPL3 and other paged profiles remain unsupported.

Five focused tests cover both ZF values with defined and undefined ZF, exact taken/fall-through RIP, taken-only canonicality, final-byte fetch-fault atomicity, neighboring forms and CPL3/profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 755 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 56 `paged_probe_quantum` tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,962 instructions and stops at `8B 6C 08 04` (`MOV EBP,dword ptr [RAX+RCX+4]`, ISSUE-269), RIP `0x2015F8`, `Unsupported { opcode: 139 }`, `com1_bytes=0`. ISSUE-201 remains open for the unrelated stale CLD-unsupported expectations in two full `od-machine` tests. Independent review approved with no findings.


#### S7-04 CPU-profile slice: exact helper `MOV EBP,dword ptr [RAX+RCX+4]` (ISSUE-269)

Base `PagedCpl0ProbeV1` admits only exact no-prefix `8B 6C 08 04`. The SIB computes the 64-bit effective address RAX+RCX+sign-extended disp8 +4. A tracked dword read writes EBP and zero-extends into RBP; flags/definedness remain unchanged and destination/RIP commit only on success. Noncanonical addresses fault before data access; data faults and final instruction-fetch faults are atomic. Neighboring SIB forms, destinations, widths, prefixes, CPL3 and other paged profiles remain unsupported.

Six focused profile tests cover the effective address, EBP routing/zero-extension, state/flags, read fault, noncanonical address, fetch fault, neighboring forms and CPL3 isolation. Two four-level machine tests cover accessed/dirty bits on success and precise data-page-fault atomicity. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 761 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 58 `paged_probe_quantum` tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass; cargo check reports only the existing unused `access_long64_4k_internal_with_writer` warning. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,964 instructions and stops at `81 FD 00 01 00 00` (`CMP EBP,0x100`, ISSUE-270), RIP `0x2015FF`, `Unsupported { opcode: 129 }`, `com1_bytes=0`. ISSUE-201 remains open for the unrelated stale CLD-unsupported expectations in two full `od-machine` tests.


#### S7-04 CPU-profile slice: exact helper `CMP EBP,0x100` (ISSUE-270)

Base `PagedCpl0ProbeV1` admits only exact no-prefix `81 FD 00 01 00 00`. Opcode 81 /7 compares dword EBP against imm32 0x100. Shared arithmetic flags/definedness are used, and no GPR or memory is written. Fault fetching the immediate is atomic; other group/register/immediate/width/prefix forms, CPL3 and other paged profiles remain unsupported.

Four focused tests cover equality and unequal cases, carry/sign/overflow flags, high qword values with dword-vs-word distinction, all arithmetic flag-definedness behavior, GPR preservation, final immediate-byte fetch-fault atomicity, neighbors and CPL3/profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 765 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass; cargo check reports only the existing unused `access_long64_4k_internal_with_writer` warning. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,965 instructions and stops at `41 0F 92 C3` (`SETB R11B`, ISSUE-271), RIP `0x201605`, `Unsupported { opcode: 146 }`, `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in two full `od-machine` tests.


#### S7-04 CPU-profile slice: exact helper `SETB R11B` (ISSUE-271)

Base `PagedCpl0ProbeV1` admits only exact `41 0F 92 C3`. REX.B selects R11B, and CF alone controls SETB. Only R11's low byte changes; upper bits, flags and definedness are preserved, even when CF is undefined. A final ModRM-byte fetch fault is atomic. Other condition/register/ModRM/REX/prefix and memory forms, CPL3 and other paged profiles remain unsupported.

Four focused tests cover CF values/definedness, ZF independence, R11 partial-register and state preservation, fetch-fault atomicity, neighbors and CPL3/profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 769 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass; cargo check reports only the existing unused `access_long64_4k_internal_with_writer` warning. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,966 instructions and stops at `45 31 F6` (`XOR R14D,R14D`, ISSUE-272), RIP `0x201609`, `Unsupported { opcode: 49 }`, `com1_bytes=0`. ISSUE-201 remains open for the unrelated stale CLD-unsupported expectations in two full `od-machine` tests.


#### S7-04 CPU-profile slice: exact helper `XOR R14D,R14D` (ISSUE-272)

Base `PagedCpl0ProbeV1` admits only exact `45 31 F6`. REX.R/B select R14D for both source and destination. Dword self-XOR writes zero and zero-extends R14, updating CF/OF/ZF/PF/SF and AF definedness per shared logical XOR semantics; unrelated flags remain unchanged. A final ModRM-byte fetch fault is atomic. Other registers/directions/REX/width/prefix forms, CPL3 and other paged profiles remain unsupported.

Four focused tests cover R14 zero-extension, result flags and definedness (including AF), preserved full state, fetch-fault atomicity, neighbors and CPL3/profile isolation. All od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 773 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass; cargo check reports only the existing unused `access_long64_4k_internal_with_writer` warning. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,967 instructions and stops at `85 ED` (`TEST EBP,EBP`, ISSUE-273), RIP `0x20160C`, `Unsupported { opcode: 133 }`, `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in two full `od-machine` tests.


#### S7-04 CPU-profile slice: exact helper `TEST EBP,EBP` (ISSUE-273)

Base `PagedCpl0ProbeV1` admits only exact no-prefix `85 ED`. The dword self-TEST writes no GPRs; shared logical semantics clear CF/OF, set SF/ZF/PF from EBP, preserve AF's concrete bit while marking AF undefined, and preserve unrelated flags/definedness. Final ModRM-byte fetch faults are atomic. Other registers, operand widths, prefixes, CPL3 and other paged profiles remain unsupported.

Four focused tests cover values across zero, parity, sign and upper-dword cases; flags/AF definedness; full GPR preservation; fetch-fault atomicity; neighbors and CPL3/profile isolation. Full od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 777 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass; cargo check reports only the existing unused `access_long64_4k_internal_with_writer` warning. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,968 instructions and stops at `41 0F 94 C6` (`SETE R14B`, ISSUE-274), RIP `0x20160E`, `Unsupported { opcode: 148 }`, `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in two full `od-machine` tests.


#### S7-04 CPU-profile slice: exact helper `SETE R14B` (ISSUE-274)

Base `PagedCpl0ProbeV1` admits only exact `41 0F 94 C6`. REX.B selects R14B. SETE reads ZF and writes one or zero to only the low byte of R14, preserving its upper bits and flags/definedness, including when ZF is undefined. Final ModRM-byte fetch faults are atomic. Other conditions/registers/ModRM/REX/prefix forms, memory destinations, CPL3 and other paged profiles remain unsupported.

Four focused tests cover ZF true/false with defined and undefined ZF, partial-byte/full-state preservation, fetch-fault atomicity, neighbors and CPL3/profile isolation. Full od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 781 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass; cargo check reports only the existing unused `access_long64_4k_internal_with_writer` warning. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,969 instructions and stops at `41 83 FA 06` (`CMP R10D,6`, ISSUE-275), RIP `0x201612`, `Unsupported { opcode: 131 }`, `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in two full `od-machine` tests.


#### S7-04 CPU-profile slice: exact helper `CMP R10D,6` (ISSUE-275)

Base `PagedCpl0ProbeV1` admits only exact `41 83 FA 06`. REX.B selects R10D; CMP uses dword operands and sign-extends imm8 6. No GPR is written; the shared arithmetic helper updates flags and definedness. Final immediate-byte fetch faults are atomic. Other immediates/registers/opcodes, operand widths, prefixes, memory forms, CPL3 and other paged profiles remain unsupported.

Four focused tests cover equal/borrow/overflow/sign/parity flag cases, high qword and word-vs-dword distinctions, GPR preservation, immediate-fetch atomicity, neighbors and CPL3/profile isolation. Full od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 785 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass; cargo check reports only the existing unused `access_long64_4k_internal_with_writer` warning. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,970 instructions and stops at `45 0F 44 F3` (`CMOVE R14D,R11D`, ISSUE-276), RIP `0x201616`, `Unsupported { opcode: 68 }`, `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in two full `od-machine` tests.


#### S7-04 CPU-profile slice: exact helper `CMOVE R14D,R11D` (ISSUE-276)

Base `PagedCpl0ProbeV1` admits only exact `45 0F 44 F3`. REX.R/B select R14D destination and R11D source. CMOVE tests ZF; when true it copies the source dword and zero-extends R14, and when false it preserves the old low R14D while zero-extending and clearing the upper dword, matching 32-bit CMOV semantics in 64-bit mode. Flags/definedness remain unchanged; a register source performs no memory access. Final ModRM-byte fetch faults are atomic. Other conditions/registers/directions/REX/width/prefix forms, memory source, CPL3 and other paged profiles remain unsupported.

Four focused tests cover ZF outcomes with defined/undefined ZF, source routing, source-versus-old-low-dword selection and zero-extension on both paths, full-state/flag preservation, fetch-fault atomicity, neighbors and CPL3/profile isolation. Full od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 789 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass; cargo check reports only the existing unused `access_long64_4k_internal_with_writer` warning. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,971 instructions and stops at `41 80 FE 01` (`CMP R14B,1`, ISSUE-277), RIP `0x20161A`, `Unsupported { opcode: 128 }`, `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in two full `od-machine` tests.


#### S7-04 CPU-profile slice: exact helper `CMP R14B,1` (ISSUE-277)

Base `PagedCpl0ProbeV1` admits only exact `41 80 FE 01`. REX.B selects R14B; the byte CMP reads only the low byte, writes no GPR, applies shared arithmetic flags/definedness, and preserves upper R14. Final immediate-byte fetch faults are atomic. Other immediates/registers/opcodes, operand widths, prefixes, memory forms, CPL3 and other paged profiles remain unsupported.

Four focused tests cover equality, borrow, sign/overflow, auxiliary carry and parity cases, upper-R14/full-GPR preservation, immediate-fetch atomicity, neighbors and CPL3/profile isolation. Full od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 793 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass; cargo check reports only the existing unused `access_long64_4k_internal_with_writer` warning. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,973 instructions and stops at `4C 8B 74 08 10` (`MOV R14,qword ptr [RAX+RCX+0x10]`, ISSUE-278), RIP `0x201620`, `Unsupported { opcode: 139 }`, `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in two full `od-machine` tests.


#### S7-04 CPU-profile slice: exact helper `MOV R14,[RAX+RCX+0x10]` (ISSUE-278)

Base `PagedCpl0ProbeV1` admits only exact `4C 8B 74 08 10`. REX.W/R select qword R14 destination; the SIB address is RAX + RCX + sign-extended disp8 0x10. The tracked qword read commits R14/RIP only, preserves flags/other GPRs, and faults atomically. Other destinations/base/index/scale/displacements/widths/prefixes, CPL3 and other paged profiles remain unsupported.

Six focused profile tests cover exact address and qword result, read fault, noncanonical address, fetch-fault atomicity, neighboring forms and CPL3/profile isolation. A four-level machine test verifies the tracked read sets accessed bits with dirty bits clear. Full od-isa suites pass (2,322 library, 1,591 CPL0 profile-set, 126 execution, 29 boundary, 6 flags, 799 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 59 `paged_probe_quantum` machine tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass; cargo check reports only the existing unused `access_long64_4k_internal_with_writer` warning. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,975 instructions and stops at `4D 85 F6` (`TEST R14,R14`, ISSUE-279), RIP `0x201628`, `Unsupported { opcode: 133 }`, `com1_bytes=0`. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in two full `od-machine` tests.


#### S7-04 CPU-profile slice: exact helper `TEST R14,R14` (ISSUE-279)

Base `PagedCpl0ProbeV1` admits only exact `4D 85 F6`. This qword self-TEST writes no GPR; shared logic semantics clear CF/OF, derive SF/ZF/PF from R14, preserve unrelated flag values and mark AF undefined. The three-byte instruction commits flags/RIP only after all bytes fetch successfully. Four profile tests cover zero/parity/sign/high-bit values, full state preservation, AF definedness, final-byte fetch-fault atomicity, neighboring register/direction/width/prefix/memory forms, CPL3 and other-profile isolation.

`cargo test --offline -p od-isa --test paged_probe_profile rust_helper_test_r14_r14 -- --nocapture` passed all four focused tests. The `other_paged_profiles_keep_s7_entry_instructions_unsupported` matrix also passed. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,976 instructions and stops at `74 33` (`JZ rel8 +0x33`, ISSUE-280), RIP `0x20162B`, `Unsupported { opcode: 116 }`, `com1_bytes=0`. Full od-isa suites pass (2,322 library, 1,591 profile-set, 126 execution, 29 boundary, 6 flags, 803 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 59 `paged_probe_quantum` machine tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting, and `git diff --check` pass; cargo check reports only the existing unused `access_long64_4k_internal_with_writer` warning. Independent review approved with no findings. ISSUE-201 remains open for the unrelated stale CLD-unsupported expectations in two full `od-machine` tests.


#### S7-04 CPU-profile slice: exact helper `JZ rel8 +0x33` (ISSUE-280)

Base `PagedCpl0ProbeV1` admits only exact no-prefix `74 33`. ZF controls the branch, rel8 is added to next RIP, flags/definedness are preserved, and only a taken target is checked for canonicality. Final displacement-byte fetch faults do not retire or mutate state. Existing JZ rel8 coverage now also exercises this displacement in the truth-table, canonicality and neighbor matrix tests. The exact encoding is unsupported in CPL3 and other paged profiles.

`cargo test --offline -p od-isa --test paged_probe_profile rust_helper_jz_rel8 -- --nocapture` passed all five JZ rel8 profile tests. Full od-isa suites pass (2,322 library, 1,591 profile-set, 126 execution, 29 boundary, 6 flags, 803 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 59 `paged_probe_quantum` machine tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass; cargo check reports only the existing unused `access_long64_4k_internal_with_writer` warning. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,977 instructions and stops at `4C 8B 54 08 08` (`MOV R10,[RAX+RCX+8]`, ISSUE-281), RIP `0x20162D`, `Unsupported { opcode: 139 }`, `com1_bytes=0`. ISSUE-201 remains open for the unrelated stale CLD-unsupported expectations in two full `od-machine` tests.


#### S7-04 CPU-profile slice: exact helper `MOV R10,[RAX+RCX+8]` (ISSUE-281)

Base `PagedCpl0ProbeV1` admits only exact `4C 8B 54 08 08`. REX.W/R select qword R10; the SIB address is RAX + RCX + disp8 8. The tracked qword read updates R10/RIP only after success, preserves flags and other GPRs, and leaves state unchanged on faults. Six focused profile tests cover address/value, read/noncanonical/fetch faults, neighbors and CPL3/profile isolation. Two machine tests cover the four-level accessed/no-dirty walk and precise unmapped-data #PF atomicity.

The six focused profile tests, other-profile rejection matrix and two targeted machine tests passed. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,978 instructions and stops at `44 89 F5` (`MOV EBP,R14D`, ISSUE-282), RIP `0x201632`, `Unsupported { opcode: 137 }`, `com1_bytes=0`. Full od-isa suites pass (2,322 library, 1,591 profile-set, 126 execution, 29 boundary, 6 flags, 809 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 61 `paged_probe_quantum` machine tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting, and `git diff --check` pass; cargo check reports only the existing unused `access_long64_4k_internal_with_writer` warning. Independent review approved with no findings. ISSUE-201 remains open for the unrelated stale CLD-unsupported expectations in two full `od-machine` tests.


#### S7-04 CPU-profile slice: exact helper `MOV EBP,R14D` (ISSUE-282)

Base `PagedCpl0ProbeV1` admits only exact `44 89 F5`. REX.R routes R14D to EBP; the dword destination write zero-extends RBP while flags/definedness and other GPRs remain unchanged. Four profile tests cover source routing, high dword zero-extension, state preservation, final-byte fetch-fault atomicity, neighboring register/opcode/width/prefix forms and CPL3/profile isolation.

`cargo test --offline -p od-isa --test paged_probe_profile rust_helper_mov_ebp_r14d -- --nocapture` passed all four focused tests; the other-paged-profile rejection matrix passed. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,979 instructions and stops at `44 09 D5` (`OR EBP,R10D`, ISSUE-283), RIP `0x201635`, `Unsupported { opcode: 9 }`, `com1_bytes=0`. Full od-isa suites pass (2,322 library, 1,591 profile-set, 126 execution, 29 boundary, 6 flags, 813 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 61 `paged_probe_quantum` machine tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting, and `git diff --check` pass; cargo check reports only the existing unused `access_long64_4k_internal_with_writer` warning. Independent review approved with no findings. ISSUE-201 remains open for the unrelated stale CLD-unsupported expectations in two full `od-machine` tests.


#### S7-04 CPU-profile slice: exact helper `OR EBP,R10D` (ISSUE-283)

Base `PagedCpl0ProbeV1` admits only exact `44 09 D5`. REX.R selects R10D as source and EBP as dword destination; the write zero-extends RBP. Shared OR semantics clear CF/OF, derive SF/ZF/PF, mark AF undefined and preserve unrelated flag values. Four profile tests cover values/flags/definedness, zero-extension, full state, final-byte fetch-fault atomicity, neighbors and CPL3/profile isolation.

`cargo test --offline -p od-isa --test paged_probe_profile rust_helper_or_ebp_r10d -- --nocapture` passed all four focused tests; the other-paged-profile matrix passed. Full od-isa suites pass (2,322 library, 1,591 profile-set, 126 execution, 29 boundary, 6 flags, 817 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 61 `paged_probe_quantum` machine tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass; cargo check reports only the existing unused `access_long64_4k_internal_with_writer` warning. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,980 instructions and stops at `81 E5 FF 0F 00 00` (`AND EBP,0xfff`, ISSUE-284), RIP `0x201638`, `Unsupported { opcode: 129 }`, `com1_bytes=0`. ISSUE-201 remains open for the unrelated stale CLD-unsupported expectations in two full `od-machine` tests.


#### S7-04 CPU-profile slice: exact helper `AND EBP,0xfff` (ISSUE-284)

Base `PagedCpl0ProbeV1` admits only exact no-prefix `81 E5 FF 0F 00 00`. Dword AND masks EBP with `0xfff`, writes EBP and zero-extends RBP; shared logic semantics clear CF/OF, derive SF/ZF/PF and leave AF undefined. Other flags remain unchanged. Four focused profile tests cover result/flags/definedness/zero-extension, state preservation, final-immediate fetch-fault atomicity, neighbors and CPL3/profile isolation.

`cargo test --offline -p od-isa --test paged_probe_profile rust_helper_and_ebp_imm32_0xfff -- --nocapture` passed all four focused tests; the other-paged-profile matrix passed. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,981 instructions and stops at `75 20` (`JNZ rel8 +0x20`, ISSUE-285), RIP `0x20163e`, `Unsupported { opcode: 117 }`, `com1_bytes=0`. Full od-isa suites pass (2,322 library, 1,591 profile-set, 126 execution, 29 boundary, 6 flags, 821 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 61 `paged_probe_quantum` machine tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass; cargo check reports only the existing unused `access_long64_4k_internal_with_writer` warning. Independent review approved with no findings. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in two full `od-machine` tests.


#### S7-04 CPU-profile slice: exact helper `JNZ rel8 +0x20` (ISSUE-285)

Base `PagedCpl0ProbeV1` admits only exact no-prefix `75 20`. JNZ tests ZF=0, adds signed rel8 +0x20 to next RIP when taken, otherwise falls through; it preserves RFLAGS/definedness and checks canonicality only for taken targets. Five profile tests cover the condition/definedness matrix, both RIP outcomes, taken-only noncanonical fault, displacement-fetch atomicity, neighbors and CPL3/profile isolation.

`cargo test --offline -p od-isa --test paged_probe_profile firmware_entry_jnz -- --nocapture` passed all five focused tests; the other-paged-profile matrix passed. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,982 instructions, falls through to `49 BF 00 00 00 00 10 00 00 00` at RIP `0x201640`, and reports `Unsupported { opcode: 191 }`; COM1 is empty. Full od-isa suites pass (2,322 library, 1,591 profile-set, 126 execution, 29 boundary, 6 flags, 821 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass; cargo check reports only the existing unused `access_long64_4k_internal_with_writer` warning. Independent review approved with no findings. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in two full `od-machine` tests.


#### S7-04 CPU-profile slice: exact helper `MOV R15,0x1000000000` (ISSUE-286)

Base `PagedCpl0ProbeV1` admits only exact `49 BF 00 00 00 00 10 00 00 00`. REX.W/B select qword R15; the full imm64 `0x1000000000` is copied, not sign-extended from imm32. The move changes only R15/RIP and preserves flags/definedness. Four profile tests cover value/routing/state, immediate-fetch atomicity, neighbors and CPL3/profile isolation.

`cargo test --offline -p od-isa --test paged_probe_profile rust_helper_mov_r15_imm64_0x1000000000 -- --nocapture` passed all four focused tests; other-profile isolation passed. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,983 instructions and stops at `4D 01 D6` (`ADD R14,R10`, ISSUE-287), RIP `0x20164a`, `Unsupported { opcode: 1 }`, `com1_bytes=0`. Full od-isa suites pass (2,322 library, 1,591 profile-set, 126 execution, 29 boundary, 6 flags, 825 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests). `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass; cargo check reports only the existing unused `access_long64_4k_internal_with_writer` warning. Independent review approved with no findings. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in two full `od-machine` tests.


#### S7-04 CPU-profile slice: exact helper `ADD R14,R10` (ISSUE-287)

Base `PagedCpl0ProbeV1` admits only exact `4D 01 D6`. REX.B/REX.R select qword destination R14 and source R10. Shared arithmetic semantics update qword result, flags and definedness, including AF; only R14/RIP change. Four profile tests cover arithmetic values/flags, full register preservation, final ModRM-byte fetch-fault atomicity, neighboring encodings and CPL3/profile isolation.

`cargo test --offline -p od-isa --test paged_probe_profile rust_helper_add_r14_r10 -- --nocapture` passed all four focused tests; the other-profile matrix passed. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,984 instructions and stops at `41 0F 92 C2` (`SETB R10B`, ISSUE-288), RIP `0x20164d`, `Unsupported { opcode: 146 }`, `com1_bytes=0`. Full od-isa suites pass (2,322 library, 1,591 profile-set, 126 execution, 29 boundary, 6 flags, 829 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 61 `paged_probe_quantum` machine tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass; cargo check reports only the existing unused `access_long64_4k_internal_with_writer` warning. Independent review approved with no findings. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in two full `od-machine` tests.

#### S7-04 CPU-profile slice: exact helper `SETB R10B` (ISSUE-288)

Base `PagedCpl0ProbeV1` admits only exact `41 0F 92 C2`. REX.B extends ModRM r/m=2 to R10B; the operation writes only bit-result 0/1 to the low byte and preserves upper R10, every other GPR, flags and flags-definedness. SETB reads CF only, including when CF is marked undefined. Other R10 condition forms, alternate REX and prefix encodings, memory destinations, CPL3 and other paged profiles remain unsupported; already admitted SETcc forms for other byte registers remain available under their own exact guards.

Four focused tests cover CF true/false with CF defined and undefined, R10 partial-byte and all-state preservation, final ModRM-byte fetch-fault atomicity, neighboring forms and CPL3/profile isolation. Full od-isa suites pass (2,322 library, 1,591 profile-set, 126 execution, 29 boundary, 6 flags, 833 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 61 `paged_probe_quantum` tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass; cargo check reports only the existing unused page-walk helper warning. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,985 instructions and stops at `4D 39 FE` (`CMP R14,R15`, ISSUE-289), RIP `0x201651`, `Unsupported { opcode: 57 }`, `com1_bytes=0`. Independent review approved with no findings. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in two full `od-machine` tests.

#### S7-04 CPU-profile slice: exact helper `CMP R14,R15` (ISSUE-289)

This slice admits only exact `4D 39 FE` in base `PagedCpl0ProbeV1`. REX.W/R/B selects qword r/m destination R14 and reg source R15; opcode 39 computes R14−R15 for arithmetic flags without writing a GPR. The shared qword arithmetic implementation updates CF/PF/AF/ZF/SF/OF and their definedness, preserving all non-arithmetic flag values. Incomplete instruction fetch is atomic. The neighbor tests reject their listed alternate operand routes, opcode direction, widths and prefixes; existing exact CMP admissions remain unchanged, and CPL3/other paged profiles do not gain this form.

Four focused tests cover qword routing and arithmetic edges including high bits, CF/AF/PF/ZF/SF/OF and definedness, full GPR preservation, ModRM fetch-fault atomicity, neighbors and CPL3/profile isolation. Full od-isa suites pass (2,322 library, 1,591 profile-set, 126 execution, 29 boundary, 6 flags, 837 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 61 paged machine tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass; cargo check reports only the existing unused page-walk helper warning. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,986 instructions and stops at `41 0F 97 C3` (`SETA R11B`, ISSUE-290), RIP `0x201654`, `Unsupported { opcode: 151 }`, `com1_bytes=0`. Independent review approved with no findings. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in two full `od-machine` tests.

#### S7-04 CPU-profile slice: exact helper `SETA R11B` (ISSUE-290)

This slice admits only exact `41 0F 97 C3` in base `PagedCpl0ProbeV1`. REX.B extends ModRM r/m=3 to R11B. SETA writes one only when CF=0 and ZF=0, otherwise zero; it preserves upper R11, all other GPRs, flags and definedness. Other SETcc admissions retain their own exact guards; only the unadmitted neighbors in the focused matrix and this instruction in CPL3/other paged profiles remain unsupported.

Four focused tests cover the CF/ZF truth table with both flags defined and undefined, partial-byte/state/flag preservation, ModRM fetch-fault atomicity, neighbors and CPL3/profile isolation. Full od-isa suites pass (2,322 library, 1,591 profile-set, 126 execution, 29 boundary, 6 flags, 841 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 61 paged machine tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass; cargo check reports only the existing unused page-walk helper warning. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,987 instructions and stops at `45 08 D3` (`OR R11B,R10B`, ISSUE-291), RIP `0x201658`, `Unsupported { opcode: 8 }`, `com1_bytes=0`. Independent review approved with no findings. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in two full `od-machine` tests.

#### S7-04 CPU-profile slice: exact helper `OR R11B,R10B` (ISSUE-291)

This slice admits only exact `45 08 D3` in base `PagedCpl0ProbeV1`. Opcode 08 uses ModRM r/m as destination and reg as source; REX.B/R select R11B and R10B. Byte OR changes only low R11 and updates logical flags; CF/OF clear, SF/ZF/PF reflect the result and AF is undefined. The partial-byte write preserves the rest of R11. The neighbor tests reject the listed alternate routes, directions, operations, prefixes and memory forms; existing exact logical admissions remain unchanged, and CPL3/other paged profiles do not gain this form.

Four focused tests cover operand routing, byte results and flags/AF-definedness, upper R11 and other-state preservation, ModRM fetch-fault atomicity, neighbors and CPL3/profile isolation. Full od-isa suites pass (2,322 library, 1,591 profile-set, 126 execution, 29 boundary, 6 flags, 845 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 61 paged machine tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass; cargo check reports only the existing unused page-walk helper warning. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,988 instructions and stops at `74 1D` (`JZ rel8 +0x1d`, ISSUE-292), RIP `0x20165b`, `Unsupported { opcode: 116 }`, `com1_bytes=0`. Independent review approved with no findings. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in two full `od-machine` tests.

#### S7-04 CPU-profile slice: exact helper `JZ rel8 +0x1d` (ISSUE-292)

This slice admits only exact no-prefix `74 1D` in base `PagedCpl0ProbeV1`. JZ reads ZF: when set, it adds signed rel8 +0x1d to next RIP; when clear, execution falls through. RFLAGS and definedness are preserved, and canonicality is checked only for a taken target. Displacement fetch faults commit nothing. The neighbor tests reject the listed adjacent displacements and condition/prefix forms; existing exact Jcc admissions remain unchanged, and CPL3/other paged profiles do not gain this form.

Five focused tests cover ZF true/false with defined and undefined ZF, exact taken/fall-through RIP targets, taken-only canonicality, displacement-fetch fault atomicity, neighbors and CPL3/profile isolation. Full od-isa suites pass (2,322 library, 1,591 profile-set, 126 execution, 29 boundary, 6 flags, 845 paged-profile, 7 profile-isolation, 15 descriptor, 0 doc tests); all 61 paged machine tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped formatting and `git diff --check` pass; cargo check reports only the existing unused page-walk helper warning. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 8,989 instructions and stops at `44 8B 6C 08 18` (`MOV R13D,dword ptr [RAX+RCX+0x18]`, ISSUE-293), RIP `0x20167a`, `Unsupported { opcode: 139 }`, `com1_bytes=0`. Independent review approved with no findings. ISSUE-201 remains open for unrelated stale CLD-unsupported expectations in two full `od-machine` tests.


### S7-04 CPU-profile slice: exact helper `MOV R13D,dword ptr [RAX+RCX+0x18]` (ISSUE-293)

The base `PagedCpl0ProbeV1` admits only exact `44 8B 6C 08 18`. REX.R selects the R13D destination; the SIB computes the 64-bit address RAX + RCX + sign-extended disp8 `0x18`; the tracked dword read zero-extends into R13. Only R13 and RIP change on success; flags/definedness and other GPRs remain unchanged. A read fault or instruction-fetch fault commits no architectural state. The neighboring displacement, destination, REX, SIB, width, prefix and store forms stay unsupported; CPL3 and other paged profiles remain isolated. Successful four-level machine execution sets accessed bits and leaves dirty bits clear for the read.

Six focused profile tests cover result/address/zero-extension, state and flags, read/fetch faults, noncanonical addressing, neighbors and CPL3/other-profile isolation. Full od-isa suites pass (2,322/1,591/126/29/6/851/7/15); all 63 paged machine tests pass. Scoped cargo check, rustfmt and diff-check pass; the check reports only the existing unused page-walk helper warning. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 8,990 retired instructions to `41 83 FD FF` (`CMP R13D,-1`, ISSUE-294) at RIP `0x20167f`; COM1 remains empty. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.


### S7-04 CPU-profile slice: exact helper `CMP R13D,-1` (ISSUE-294)

The base `PagedCpl0ProbeV1` admits only exact `41 83 FD FF`. REX.B selects R13D and opcode 83 sign-extends immediate byte `0xff` to the dword value `0xffff_ffff`. CMP reads only the low dword of R13 and writes no GPR; shared arithmetic semantics update CF/PF/AF/ZF/SF/OF and the corresponding definedness. Immediate-byte fetch faults commit no state. Neighboring registers, operations, widths, immediate sizes and profiles remain unsupported.

Four focused profile tests cover signed immediate and dword edge cases, equality/borrow/sign/overflow/AF, full GPR preservation, immediate-fetch atomicity, neighbors and CPL3/other-profile isolation. Full od-isa suites pass (2,322/1,591/126/29/6/855/7/15); all 63 paged machine tests pass. Scoped cargo check, rustfmt and diff-check pass; cargo check reports only the existing unused page-walk helper warning. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 8,991 retired instructions to `41 0F 95 C2` (`SETNE R10B`, ISSUE-295) at RIP `0x201683`; COM1 remains empty. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.


### S7-04 CPU-profile slice: exact helper `SETNE R10B` (ISSUE-295)

The base `PagedCpl0ProbeV1` admits only exact `41 0F 95 C2`. REX.B selects R10B and SETNE writes `!ZF` to its low byte. Only R10B and RIP change; upper R10, other GPRs, flags and flag definedness remain unchanged. A ModRM fetch fault commits no state. Neighbor conditions/registers, REX variants, memory destination and operand prefixes remain unsupported; CPL3 and other paged profiles remain isolated.

Four focused profile tests cover ZF true/false with defined/undefined ZF, CF independence, low-byte write and upper-register/full-state preservation, fetch-fault atomicity, neighbors and CPL3/other-profile isolation. Full od-isa suites pass (2,322/1,591/126/29/6/859/7/15); all 63 paged machine tests pass. Scoped cargo check, rustfmt and diff-check pass; only the existing unused page-walk helper warning remains. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 8,992 retired instructions to `4D 39 E9` (`CMP R9,R13`, ISSUE-296) at RIP `0x201687`; COM1 remains empty. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.


### S7-04 CPU-profile slice: exact helper `CMP R9,R13` (ISSUE-296)

The base `PagedCpl0ProbeV1` admits only exact `4D 39 E9`. REX.B extends ModRM r/m to qword R9, REX.R extends ModRM.reg to R13, and opcode 39 selects r/m-minus-reg: R9-R13. CMP writes no GPR and uses shared qword subtraction flags and definedness, including AF. A ModRM fetch fault commits no state; neighboring operand order/registers, widths, opcodes, prefixes and CPL3/other paged profiles remain unsupported.

Four focused profile tests cover operand direction, equality/borrow/sign/overflow/AF, full GPR preservation, ModRM-fetch atomicity, neighbors and CPL3/other-profile isolation. Full od-isa suites pass (2,322/1,591/126/29/6/863/7/15); all 63 paged machine tests pass. Scoped cargo check, rustfmt and diff-check pass; only the existing unused page-walk helper warning remains. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 8,993 retired instructions to `41 0F 96 C3` (`SETBE R11B`, ISSUE-297) at RIP `0x20168a`; COM1 remains empty. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.


### S7-04 CPU-profile slice: exact helper `SETBE R11B` (ISSUE-297)

The base `PagedCpl0ProbeV1` admits only exact `41 0F 96 C3`. REX.B selects low byte R11B and SETBE reads CF/ZF, writing `u8(CF || ZF)` to that byte. Only R11B and RIP change; upper R11, other GPRs, flag values and definedness remain unchanged. A ModRM fetch fault commits no state. Neighbor conditions/registers, REX variants and memory/operand-prefix forms remain unsupported; CPL3 and other paged profiles remain isolated.

Four focused profile tests cover all CF/ZF combinations with defined and undefined inputs, partial-register/full-state preservation, fetch-fault atomicity, neighbors and CPL3/other-profile isolation. Full od-isa suites pass (2,322/1,591/126/29/6/867/7/15); all 63 paged machine tests pass. Scoped cargo check, rustfmt and diff-check pass; only the existing unused page-walk helper warning remains. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 8,994 retired instructions to `45 84 DA` (`TEST R10B,R11B`, ISSUE-298) at RIP `0x20168e`; COM1 remains empty. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.


### S7-04 CPU-profile slice: exact helper `TEST R10B,R11B` (ISSUE-298)

The base `PagedCpl0ProbeV1` admits only exact `45 84 DA`. REX.B selects ModRM r/m=R10B as the left operand, while REX.R selects ModRM.reg=R11B as the right operand. TEST performs byte AND for flags without writing a GPR: CF/OF clear; ZF/SF/PF reflect the result; AF is undefined and its value is preserved by shared semantics. A ModRM fetch fault commits no state. Neighbor register order, REX fields, operation, prefixes and memory forms remain unsupported; CPL3 and other paged profiles remain isolated.

Four focused profile tests cover result and logical flag values, AF definedness, no GPR writes, ModRM-fetch atomicity, neighbors and CPL3/other-profile isolation. Full od-isa suites pass (2,322/1,591/126/29/6/871/7/15); all 63 paged machine tests pass. Scoped cargo check, rustfmt and diff-check pass; only the existing unused page-walk helper warning remains. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 8,995 retired instructions to `74 11` (`JZ rel8 +0x11`, ISSUE-299) at RIP `0x201691`; COM1 remains empty. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.

### S7-04 CPU-profile slice: exact helper `JZ rel8 +0x11` (ISSUE-299)

The base `PagedCpl0ProbeV1` admits only exact `74 11`. JZ reads ZF and adds signed rel8 `+0x11` to next RIP when ZF=1; when ZF=0 it falls through. It preserves flags and definedness, and validates canonicality only for a taken target. The displacement fetch is atomic; adjacent displacements, JNZ/near forms and prefixes remain unsupported, as do CPL3 and other paged profiles.

Five focused profile tests cover both ZF outcomes and definedness, exact taken/fall-through targets, taken-only canonicality, displacement-fetch fault atomicity, neighbors and CPL3/profile isolation. Full od-isa suites pass (2,322/1,591/126/29/6/871/7/15); all 63 paged machine tests pass. Scoped cargo check, rustfmt and diff-check pass; only the existing unused page-walk helper warning remains. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 8,996 retired instructions to `83 3C 30 00` (`CMP dword ptr [RAX+RSI],0`, ISSUE-300) at RIP `0x2016a4`; COM1 remains empty. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.

### S7-04 CPU-profile slice: exact helper `CMP dword ptr [RAX+RSI],0` (ISSUE-300)

The base `PagedCpl0ProbeV1` admits only exact `83 3C 30 00`. ModRM selects /7 CMP and the SIB computes the 64-bit address RAX + RSI; the dword operand is read through the tracked memory path. Opcode 83 sign-extends imm8 `0` to the dword operand. Shared dword CMP semantics update arithmetic flags and definedness, while GPRs remain unchanged. A read fault commits neither flags nor RIP; successful four-level reads set paging A bits without D. Alternate SIB/index/base/scale/displacement, immediate, width, opcode and prefix forms remain unsupported; CPL3 and other paged profiles remain isolated.

Five focused profile tests cover dword values and arithmetic flags, no GPR mutation, tracked read address/width, read and immediate-fetch fault atomicity, neighbors and CPL3/other-profile isolation. Two machine tests cover the four-level A/no-D walk and a non-present data-page fault with no CPU-state commit. Full od-isa suites pass (2,322/1,591/126/29/6/876/7/15); all 65 paged machine tests pass. Scoped cargo check, rustfmt and diff-check pass; only the existing unused page-walk helper warning remains. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 8,997 retired instructions to `74 0D` (`JZ rel8 +0x0d`, ISSUE-301) at RIP `0x2016a8`; COM1 remains empty. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.

### S7-04 CPU-profile slice: exact helper `JZ rel8 +0x0d` (ISSUE-301)

The base `PagedCpl0ProbeV1` admits only exact `74 0D`. JZ tests ZF and, when set, adds signed rel8 `+0x0d` to next RIP; otherwise it falls through. Flags and definedness remain unchanged. The branch target is checked for canonicality only when taken, while the fall-through is validated by the next fetch. The displacement-fetch fault commits no state; neighboring displacements, conditions, prefixes and near forms remain unsupported, as do CPL3 and other paged profiles.

Five focused tests cover both ZF outcomes and definedness, exact taken/fall-through targets including canonical boundaries, fetch-fault atomicity, neighbors and CPL3/other-profile isolation. Full od-isa suites pass (2,322/1,591/126/29/6/876/7/15); all 65 paged machine tests pass. Scoped cargo check, rustfmt and diff-check pass; only the existing unused page-walk helper warning remains. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 8,998 retired instructions to `49 83 F8 01` (`CMP R8,1`, ISSUE-302) at RIP `0x2016b7`; COM1 remains empty. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.

### S7-04 CPU-profile slice: exact helper `CMP R8,1` (ISSUE-302)

The base `PagedCpl0ProbeV1` admits only exact `49 83 F8 01`. REX.W/B select qword R8; opcode 83 sign-extends the imm8 value 1 to qword. Shared CMP semantics calculate qword subtraction flags and definedness including AF. CMP writes no GPR. A final-immediate fetch fault commits no state; other immediate/register/width/opcode/prefix variants remain unsupported, as do CPL3 and other paged profiles.

Four focused tests cover equality, borrow, sign/overflow/AF, upper-qword distinctions from R8D, GPR preservation, immediate-fetch fault atomicity, neighboring forms and CPL3/other-profile isolation. Full od-isa suites pass (2,322/1,591/126/29/6/880/7/15); all 65 paged machine tests pass. Scoped cargo check, rustfmt and diff-check pass; only the existing unused page-walk helper warning remains. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 8,999 retired instructions to `75 0A` (`JNZ rel8 +0x0a`, ISSUE-303) at RIP `0x2016bb`; COM1 remains empty. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.

### S7-04 CPU-profile slice: exact helper `JNZ rel8 +0x0a` (ISSUE-303)

The base `PagedCpl0ProbeV1` admits only exact `75 0A`. JNZ reads ZF and adds signed rel8 `+0x0a` to next RIP when ZF=0; when ZF=1 it falls through. Flags and definedness remain unchanged. Canonicality is checked only for a taken target; the fall-through is validated by the next fetch. A displacement-fetch fault commits no state. Other displacements, branch conditions, prefixes and near forms remain unsupported, as do CPL3 and other paged profiles.

Five focused tests cover both ZF outcomes and definedness, exact taken/fall-through RIP, taken-only canonicality, displacement-fetch atomicity, neighbors and CPL3/other-profile isolation. Full od-isa suites pass (2,322/1,591/126/29/6/880/7/15); all 65 paged machine tests pass. Scoped cargo check, rustfmt and diff-check pass; only the existing unused page-walk helper warning remains. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,000 retired instructions to `4C 8D 24 01` (`LEA R12,[RCX+RAX]`, ISSUE-304) at RIP `0x2016c7`; COM1 remains empty. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.

### S7-04 CPU-profile slice: exact helper `LEA R12,[RCX+RAX]` (ISSUE-304)

The base `PagedCpl0ProbeV1` admits only exact bytes `4C 8D 24 01`. REX.R selects R12; the SIB describes RCX+RAX at scale 1 with no displacement. The qword effective offset wraps at 64 bits and is written to R12. Only R12/RIP change; flags and definedness remain unchanged. LEA does not perform a memory access or canonicality check. Other SIB tuples, destinations, widths, prefixes and profiles remain unsupported.

Four focused tests cover address routing, wrapping and noncanonical offsets, full-state/flags/definedness retention, no memory or PIO effects, a fetch fault on the final SIB byte, neighbors, CPL3 and other-profile isolation. All od-isa suites pass (2,322/1,591/126/29/6/884/7/15); all 65 paged machine tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped rustfmt and `git diff --check` pass. Cargo check emits only the existing unused `access_long64_4k_internal_with_writer` warning. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,001 retired instructions to `49 83 C4 3C` (`ADD R12,0x3c`, ISSUE-305) at RIP `0x2016cb`; COM1 remains empty. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.

### S7-04 CPU-profile slice: exact helper `ADD R12,0x3c` (ISSUE-305)

The base `PagedCpl0ProbeV1` admits only exact bytes `49 83 C4 3C`. REX.W/B select qword R12 and opcode 83 sign-extends imm8 `0x3c`; the existing qword ADD semantics update the six arithmetic flags and their definedness. Only R12 and RIP change. Other operation groups, immediates, registers, widths, prefixes and profiles remain unsupported.

Four focused tests cover qword results with carry, auxiliary carry, parity, zero, sign and overflow cases, high bits, full-state and flag-definedness updates, final-immediate fetch-fault atomicity, neighbors, CPL3 and other-profile isolation. All od-isa suites pass (2,322/1,591/126/29/6/888/7/15); all 65 paged machine tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped rustfmt and `git diff --check` pass; cargo check emits only the existing unused page-walk helper warning. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,002 retired instructions to `49 C1 E0 05` (`SHL R8,5`, ISSUE-306) at RIP `0x2016cf`; COM1 remains empty. ISSUE-201 remains the unrelated CLD expectation issue.

### S7-04 CPU-profile slice: exact helper `SHL R8,5` (ISSUE-306)

The base `PagedCpl0ProbeV1` admits only exact bytes `49 C1 E0 05`. REX.B selects qword R8, and raw ModRM `/4` pins SHL rather than the decoder's normalized `/6` SAL alias. The qword shift count is five. Shared shift semantics update CF/SF/ZF/PF; AF and OF are undefined for a count greater than one. Only R8 and RIP change. Other counts/groups/registers/widths/prefixes and profiles remain unsupported.

Four focused tests cover qword result values, carry/parity/sign/zero behavior, AF/OF definedness, full-state retention, final-immediate fetch-fault atomicity, adjacent groups/counts/registers/widths/prefixes, CPL3 and other-profile isolation. All od-isa suites pass (2,322/1,591/126/29/6/892/7/15); all 65 paged machine tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped rustfmt and `git diff --check` pass; cargo check emits only the existing unused page-walk helper warning. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,003 retired instructions to `49 83 C0 E0` (`ADD R8,-0x20`, ISSUE-307) at RIP `0x2016d3`; COM1 remains empty. ISSUE-201 remains the unrelated CLD expectation issue.

### S7-04 CPU-profile slice: exact helper `ADD R8,-0x20` (ISSUE-307)

The base `PagedCpl0ProbeV1` admits only exact bytes `49 83 C0 E0`. REX.W/B select qword R8 and opcode 83 sign-extends imm8 `0xe0` to -32; shared qword ADD semantics update the six arithmetic flags and their definedness. Only R8 and RIP change. Other immediates, operation groups, registers, widths, prefixes and profiles remain unsupported.

Four focused tests cover negative immediate sign-extension, carry/overflow/auxiliary carry, parity/sign/zero, high-bit results, full-state and definedness updates, final-immediate fetch-fault atomicity, neighbors, CPL3 and other-profile isolation. All od-isa suites pass (2,322/1,591/126/29/6/896/7/15); all 65 paged machine tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped rustfmt and `git diff --check` pass; cargo check emits only the existing unused page-walk helper warning. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,004 retired instructions to `41 BB 01 00 00 00` (`MOV R11D,1`, ISSUE-308) at RIP `0x2016d7`; COM1 remains empty. ISSUE-201 remains the unrelated CLD expectation issue.

### S7-04 CPU-profile slice: exact helper `MOV R11D,1` (ISSUE-308)

The base `PagedCpl0ProbeV1` admits only exact bytes `41 BB 01 00 00 00`. REX.B selects R11D; the immediate32 value 1 writes that dword register view and zero-extends into R11. Flags/definedness and all other architectural state remain unchanged. Other immediate values/registers/widths/prefixes and profiles remain unsupported.

Four focused tests cover destination routing and zero-extension from a nonzero high qword, full-state/flags retention, final-immediate fetch-fault atomicity, neighboring encodings, CPL3 and other-profile isolation. All od-isa suites pass (2,322/1,591/126/29/6/900/7/15); all 65 paged machine tests pass. `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped rustfmt and `git diff --check` pass; cargo check emits only the existing unused page-walk helper warning. Independent review approved with no findings. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,006 retired instructions to `48 8D 34 01` (`LEA RSI,[RCX+RAX]`, ISSUE-309) at RIP `0x2016df`; COM1 remains empty. ISSUE-201 remains the unrelated CLD expectation issue.

### S7-04 CPU-profile slice: exact helper `LEA RSI,[RCX+RAX]` (ISSUE-309)

The base `PagedCpl0ProbeV1` admits only exact bytes `48 8D 34 01`. REX.W selects qword RSI; the SIB adds RCX and RAX with scale 1 and no displacement. The effective offset wraps at 64 bits. Only RSI and RIP change; flags/definedness are preserved. LEA performs no memory access or canonicality check. Other forms and profiles remain unsupported.

Four focused tests cover qword sum/state preservation, wrap and noncanonical effective offsets, fetch-fault atomicity at the final SIB byte, neighboring encodings and CPL3/other-profile isolation. All od-isa suites pass (2,322/1,591/126/29/6/904/7/15; 0 doc tests), and all 65 `paged_probe_quantum` tests pass. Scoped cargo check, rustfmt and diff-check pass; cargo check emits only the existing unused page-walk helper warning. The runner image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,007 retired instructions to `48 83 C6 24` (`ADD RSI,0x24`, ISSUE-310) at RIP `0x2016e3`, `Unsupported { opcode: 131 }`; COM1 remains empty. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.

### S7-04 CPU-profile slice: exact helper `ADD RSI,0x24` (ISSUE-310)

The base `PagedCpl0ProbeV1` admits only exact bytes `48 83 C6 24`. REX.W and opcode 83 select qword RSI plus the sign-extended imm8 `0x24`. Shared qword ADD semantics update CF/PF/AF/ZF/SF/OF and their definedness; only RSI and RIP change. Other immediates, arithmetic groups, registers, widths, prefixes and profiles remain unsupported.

Four focused tests cover qword results and wrap, carry/auxiliary-carry/overflow/parity/sign/zero flags, definedness and full-state preservation, last-immediate-byte fetch-fault atomicity, neighboring forms, CPL3 and other-profile isolation. Runner image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,009 retired instructions to `0F 87 52 01 00 00` (`JA rel32 +0x152`, ISSUE-311) at RIP `0x2016ea`, `Unsupported { opcode: 135 }`; COM1 remains empty. Full od-isa, paged-machine, scoped cargo check, rustfmt and diff-check pass; independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.

### S7-04 CPU-profile slice: exact helper `JA rel32 +0x152` (ISSUE-311)

The base `PagedCpl0ProbeV1` admits only exact no-prefix bytes `0F 87 52 01 00 00`. JA tests `CF=0 && ZF=0`, adds the signed rel32 displacement to next RIP, and leaves flags/definedness unchanged. Canonicality is checked only when the branch is taken. Other conditions, displacements, prefixes and profiles remain unsupported.

Five focused tests cover the CF/ZF truth table with defined and undefined inputs, exact taken/fall-through targets, taken-only canonicality, final-displacement fetch-fault atomicity, neighboring forms and CPL3/profile isolation. All od-isa suites pass (2,322/1,591/126/29/6/913/7/15; 0 doc tests), and all 65 `paged_probe_quantum` tests pass. Scoped cargo check, rustfmt and diff-check pass; cargo check emits only the existing unused page-walk helper warning. The runner image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,010 retired instructions to `4C 8D 14 01` (`LEA R10,[RCX+RAX]`, ISSUE-312) at RIP `0x2016f0`, `Unsupported { opcode: 141 }`; COM1 remains empty. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.

### S7-04 CPU-profile slice: exact helper `LEA R10,[RCX+RAX]` (ISSUE-312)

The base `PagedCpl0ProbeV1` admits only exact bytes `4C 8D 14 01`. REX.R selects qword R10; the SIB adds qword RCX and RAX with scale 1 and no displacement. The effective offset wraps at 64 bits. Only R10 and RIP change; flags/definedness are preserved. LEA performs no memory access or canonicality check. Other forms and profiles remain unsupported.

Four focused tests cover qword sum/state preservation, wrap and noncanonical effective offsets, final-SIB-byte fetch-fault atomicity, neighboring forms and CPL3/other-profile isolation. The runner image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,011 retired instructions to `49 83 C2 28` (`ADD R10,0x28`, ISSUE-313) at RIP `0x2016f4`, `Unsupported { opcode: 131 }`; COM1 remains empty. All od-isa suites pass (2,322/1,591/126/29/6/917/7/15; 0 doc tests), and all 65 `paged_probe_quantum` tests pass. Scoped cargo check, rustfmt and diff-check pass; cargo check emits only the existing unused page-walk helper warning. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.


### S7-04 CPU-profile slice: exact helper `ADD R10,0x28` (ISSUE-313)

The base `PagedCpl0ProbeV1` admits only exact bytes `49 83 C2 28`. REX.W/B select qword R10; opcode 83 sign-extends imm8 `0x28`. Shared ADD semantics update arithmetic flags and definedness, and only R10/RIP change. Other immediate values, arithmetic groups, widths, registers, prefixes and profiles remain unsupported.

Four focused tests cover qword results and arithmetic flags/definedness, final-immediate fetch-fault atomicity, neighboring encodings, CPL3 rejection and other-profile isolation. All od-isa suites pass (2,322/1,591/126/29/6/921/7/15; 0 doc tests), and all 65 `paged_probe_quantum` tests pass. Scoped cargo check, rustfmt and diff-check pass; cargo check emits only the existing unused page-walk helper warning. The runner image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,013 retired instructions to `0F 87 5F 01 00 00` (`JA rel32 +0x15f`, ISSUE-314) at RIP `0x2016fb`, `Unsupported { opcode: 135 }`; COM1 remains empty. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.


### S7-04 CPU-profile slice: exact helper `JA rel32 +0x15f` (ISSUE-314)

The base `PagedCpl0ProbeV1` admits only exact no-prefix bytes `0F 87 5F 01 00 00`. JA is taken iff `CF=0 && ZF=0`; the signed rel32 is added to next RIP. Flags and their definedness remain unchanged. Canonicality is checked only for a taken target. Other branch conditions, displacements, prefixes and profiles remain unsupported.

Five focused tests cover all CF/ZF combinations and definedness, exact taken/fall-through RIP, taken-only canonicality, final-displacement fetch-fault atomicity, neighboring forms and CPL3/profile isolation. The runner image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,015 retired instructions to `48 83 C6 30` (`ADD RSI,0x30`, ISSUE-315) at RIP `0x201705`, `Unsupported { opcode: 131 }`; COM1 remains empty. All od-isa suites pass (2,322/1,591/126/29/6/926/7/15; 0 doc tests), and all 65 `paged_probe_quantum` tests pass. Scoped cargo check, rustfmt and diff-check pass; cargo check reports only the existing unused page-walk helper warning. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.


### S7-04 CPU-profile slice: exact helper `ADD RSI,0x30` (ISSUE-315)

The base `PagedCpl0ProbeV1` admits only exact bytes `48 83 C6 30`. REX.W selects qword RSI; opcode 83 sign-extends imm8 `0x30`. Shared ADD semantics update arithmetic flags/definedness, and only RSI/RIP change. Other immediates, groups, destination registers, widths, prefixes and profiles remain unsupported.

Four focused tests cover qword results, wrapping and arithmetic flag values/definedness, final-immediate fetch-fault atomicity, adjacent immediate/opcode/register/width/prefix neighbors, CPL3 rejection and other-profile isolation. The runner image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,017 retired instructions to `0F 87 68 01 00 00` (`JA rel32 +0x168`, ISSUE-316) at RIP `0x20170c`, `Unsupported { opcode: 135 }`; COM1 remains empty. All od-isa suites pass (2,322/1,591/126/29/6/930/7/15; 0 doc tests), and all 65 `paged_probe_quantum` tests pass. Scoped cargo check, rustfmt and diff-check pass; cargo check reports only the existing unused page-walk helper warning. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.


### S7-04 CPU-profile slice: exact helper `JA rel32 +0x168` (ISSUE-316)

The base `PagedCpl0ProbeV1` admits only exact no-prefix bytes `0F 87 68 01 00 00`. JA is taken iff `CF=0 && ZF=0`; the signed rel32 is added to next RIP. Flags and definedness remain unchanged. Canonicality is checked only for a taken target. Other branch conditions, displacements, prefixes and profiles remain unsupported.

Five focused tests cover CF/ZF combinations and definedness, taken/fall-through RIP, taken-only canonicality, final-displacement fetch-fault atomicity, neighboring forms and CPL3/profile isolation. The runner image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,019 retired instructions to `49 83 C2 38` (`ADD R10,0x38`, ISSUE-317) at RIP `0x201716`, `Unsupported { opcode: 131 }`; COM1 remains empty. All od-isa suites pass (2,322/1,591/126/29/6/935/7/15; 0 doc tests), and all 65 `paged_probe_quantum` tests pass. Scoped cargo check, rustfmt and diff-check pass; cargo check reports only the existing unused page-walk helper warning. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.


### S7-04 CPU-profile slice: exact helper `ADD R10,0x38` (ISSUE-317)

The base `PagedCpl0ProbeV1` admits only exact bytes `49 83 C2 38`. REX.W/B select qword R10; opcode 83 sign-extends imm8 `0x38`. Shared ADD semantics update arithmetic flags/definedness, and only R10/RIP change. Other immediates, groups, destinations, widths, prefixes and profiles remain unsupported.

Four focused tests cover qword results and wrap, parity/carry/auxiliary-carry/overflow flag vectors and definedness, final-immediate fetch-fault atomicity, adjacent immediate/opcode/register/width/prefix neighbors, CPL3 and other-profile isolation. The runner image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,021 retired instructions to `0F 87 6E 01 00 00` (`JA rel32 +0x16e`, ISSUE-318) at RIP `0x20171d`, `Unsupported { opcode: 135 }`; COM1 remains empty. All od-isa suites pass (2,322/1,591/126/29/6/939/7/15; 0 doc tests), and all 65 `paged_probe_quantum` tests pass. Scoped cargo check, rustfmt and diff-check pass; cargo check reports only the existing unused page-walk helper warning. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.


### S7-04 CPU-profile slice: exact helper `JA rel32 +0x16e` (ISSUE-318)

The base `PagedCpl0ProbeV1` admits only exact no-prefix bytes `0F 87 6E 01 00 00`. JA is taken iff `CF=0 && ZF=0`; the signed rel32 is added to next RIP. Flags and definedness remain unchanged. Target canonicality is checked only when taken. Other branch conditions, displacements, prefixes and profiles remain unsupported.

Five focused tests cover CF/ZF combinations and definedness, both RIP targets, taken-only canonicality, final-displacement fetch-fault atomicity, neighboring forms and CPL3/profile isolation. The runner image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,023 retired instructions to `48 83 C6 3C` (`ADD RSI,0x3c`, ISSUE-319) at RIP `0x201727`, `Unsupported { opcode: 131 }`; COM1 remains empty. All od-isa suites pass (2,322/1,591/126/29/6/944/7/15; 0 doc tests), and all 65 `paged_probe_quantum` tests pass. Scoped cargo check, rustfmt and diff-check pass; cargo check reports only the existing unused page-walk helper warning. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.


### S7-04 CPU-profile slice: exact helper `ADD RSI,0x3c` (ISSUE-319)

The base `PagedCpl0ProbeV1` admits only exact bytes `48 83 C6 3C`. REX.W selects qword RSI; opcode 83 sign-extends imm8 `0x3c`. Shared ADD semantics update arithmetic flags/definedness, and only RSI/RIP change. Other immediates, groups, destinations, widths, prefixes and profiles remain unsupported.

Four focused tests cover qword result/wrap, parity/carry/auxiliary-carry/overflow flag vectors and definedness, final-immediate fetch-fault atomicity, adjacent immediate/opcode/register/width/prefix neighbors, CPL3 and other-profile isolation. The runner image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,025 retired instructions to `0F 87 77 01 00 00` (`JA rel32 +0x177`, ISSUE-320) at RIP `0x20172e`, `Unsupported { opcode: 135 }`; COM1 remains empty. All od-isa suites pass (2,322/1,591/126/29/6/948/7/15; 0 doc tests), and all 65 `paged_probe_quantum` tests pass. Scoped cargo check, rustfmt and diff-check pass; cargo check reports only the existing unused page-walk helper warning. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.


### S7-04 CPU-profile slice: exact helper `JA rel32 +0x177` (ISSUE-320)

The base `PagedCpl0ProbeV1` admits only exact no-prefix bytes `0F 87 77 01 00 00`. JA is taken iff `CF=0 && ZF=0`; the signed rel32 is added to next RIP. Flags and definedness remain unchanged. Target canonicality is checked only when taken. Other branch conditions, displacements, prefixes and profiles remain unsupported.

Five focused tests cover CF/ZF combinations and definedness, both RIP targets, taken-only canonicality, final-displacement fetch-fault atomicity, neighboring forms and CPL3/profile isolation. The runner image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,027 retired instructions to `49 83 C2 40` (`ADD R10,0x40`, ISSUE-321) at RIP `0x201738`, `Unsupported { opcode: 131 }`; COM1 remains empty. All od-isa suites pass (2,322/1,591/126/29/6/953/7/15; 0 doc tests), and all 65 `paged_probe_quantum` tests pass. Scoped cargo check, rustfmt and diff-check pass; cargo check reports only the existing unused page-walk helper warning. ISSUE-318 regression and profile-isolation tests pass. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.


### S7-04 CPU-profile slice: exact helper `ADD R10,0x40` (ISSUE-321)

The base `PagedCpl0ProbeV1` admits only exact bytes `49 83 C2 40`. REX.W/B select qword R10; opcode 83 sign-extends imm8 `0x40`. Shared ADD semantics update arithmetic flags/definedness, and only R10/RIP change. Other immediates, groups, destinations, widths, prefixes and profiles remain unsupported.

Four focused tests cover qword result/wrap, parity/carry/auxiliary-carry/overflow flag vectors and definedness, final-immediate fetch-fault atomicity, adjacent immediates 0x3f/0x41 and opcode/register/width/prefix neighbors, CPL3 and other-profile isolation. The runner image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,029 retired instructions to `0F 87 23 01 00 00` (`JA rel32 +0x123`, ISSUE-322) at RIP `0x20173f`, `Unsupported { opcode: 135 }`; COM1 remains empty. All od-isa suites pass (2,322/1,591/126/29/6/957/7/15; 0 doc tests), and all 65 `paged_probe_quantum` tests pass. Scoped cargo check, rustfmt and diff-check pass; cargo check reports only the existing unused page-walk helper warning. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.


### S7-04 CPU-profile slice: exact helper `JA rel32 +0x123` (ISSUE-322)

The base `PagedCpl0ProbeV1` admits only exact no-prefix bytes `0F 87 23 01 00 00`. JA is taken iff `CF=0 && ZF=0`; the signed rel32 is added to next RIP. Flags and definedness remain unchanged. Target canonicality is checked only when taken. Other branch conditions, displacements, prefixes and profiles remain unsupported.

Five focused tests cover CF/ZF combinations and definedness, both RIP targets, taken-only canonicality, final-displacement fetch-fault atomicity, adjacent displacements `0x122`/`0x124`, other branch conditions/prefixes and CPL3/profile isolation. The runner image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,030 retired instructions to `45 8B 54 04 E4` (`MOV R10D,dword ptr [R12+RAX-0x1c]`, ISSUE-323) at RIP `0x201745`, `Unsupported { opcode: 139 }`; COM1 remains empty. All od-isa suites pass (2,322/1,591/126/29/6/962/7/15; 0 doc tests), and all 65 `paged_probe_quantum` tests pass. Scoped cargo check, rustfmt and diff-check pass; cargo check reports only the existing unused page-walk helper warning. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation issue.

### S7-04 CPU-profile slice: exact helper `MOV R10D,dword ptr [R12+RAX-0x1c]` (ISSUE-323)

The base `PagedCpl0ProbeV1` admits only exact bytes `45 8B 54 04 E4`. REX.R selects R10D; REX.B selects SIB base R12; RAX is the scale-1 index; disp8 `0xe4` sign-extends to -0x1c. The effective-address sum wraps at 64 bits and the tracked four-byte read is canonicality/fault checked. The dword result zero-extends into R10; flags and definedness remain unchanged.

Six focused tests passed, including value/read-trace checks, address wrap, read-fault atomicity, noncanonical #GP before memory access, final-instruction-byte fetch-fault atomicity, neighboring encoding rejection, and CPL3 rejection. The other-paged-profile isolation matrix passed. Runner image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retired 9,032 instructions and stopped at `40 0F 92 C6` (`SETB SIL`, ISSUE-324), RIP `0x20174e`, `Unsupported { opcode: 146 }`; COM1 is empty. All od-isa suites pass (2,322/1,591/126/29/6/968/7/15; 0 doc tests); all 65 paged machine quantum tests pass. Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped rustfmt and `git diff --check` pass; cargo check emits only the existing unused page-walk helper warning. Independent review approved with no findings. ISSUE-201 remains the unrelated pair of stale CLD-unsupported expectations in full od-machine tests.

### S7-04 CPU-profile slice: exact helper `SETB SIL` (ISSUE-324)

The base `PagedCpl0ProbeV1` admits only exact bytes `40 0F 92 C6`. The mandatory REX prefix selects SIL rather than DH, and raw ModRM is pinned. SETB reads CF, writes only RSI[7:0] as 0/1, and preserves upper RSI, all other registers, flags and flag-definedness.

Four focused tests pass, covering both CF values with defined and undefined CF metadata, full state/upper-byte/flag preservation, final-byte fetch-fault atomicity, neighboring conditions/destinations/REX/high-byte/memory forms, and CPL3 isolation. The other-paged-profile matrix passes. Runner image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 9,033 instructions and stops at `45 8D 6A F3` (`LEA R13D,[R10-0xd]`, ISSUE-325), RIP `0x201752`, `Unsupported { opcode: 141 }`; COM1 is empty. All od-isa suites pass (2,322/1,591/126/29/6/972/7/15; 0 doc tests); all 65 paged machine quantum tests pass. Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped rustfmt and `git diff --check` pass; cargo check emits only the existing unused page-walk helper warning. Independent review approved with no findings. ISSUE-201 remains the unrelated pair of stale CLD-unsupported expectations in full od-machine tests.

### S7-04 CPU-profile slice: exact helper `LEA R13D,[R10-0xd]` (ISSUE-325)

The base `PagedCpl0ProbeV1` admits only exact bytes `45 8D 6A F3`. REX.R selects R13D and REX.B selects R10; disp8 `0xf3` sign-extends to -0x0d. The effective address is computed with 64-bit wrapping, then truncated to dword and zero-extended into R13. LEA performs no memory access or canonicality check; flags/definedness remain unchanged.

Four focused tests pass for arithmetic/truncation and zero-extension, wrapping and noncanonical effective addresses, state/flags/no-memory preservation, final-byte fetch-fault atomicity, neighbor rejection and CPL3 isolation. The other-paged-profile matrix passes. Runner image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 9,035 instructions and stops at `40 0F 93 C5` (`SETAE BPL`, ISSUE-326), RIP `0x20175a`, `Unsupported { opcode: 147 }`; COM1 is empty. All od-isa suites pass (2,322/1,591/126/29/6/976/7/15; 0 doc tests); all 65 paged machine quantum tests pass. Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped rustfmt and `git diff --check` pass; cargo check emits only the existing unused page-walk helper warning. Independent review approved with no findings. ISSUE-201 remains the unrelated pair of stale CLD-unsupported expectations in full od-machine tests.

### S7-04 CPU-profile slice: exact helper `SETAE BPL` (ISSUE-326)

The base `PagedCpl0ProbeV1` admits only exact bytes `40 0F 93 C5`. The mandatory bare REX selects BPL rather than CH; SETAE reads CF, writes one when CF is clear and zero otherwise, and changes only RBP[7:0]. Other RBP bits, registers, flags and flag-definedness are preserved.

Four focused tests pass for CF true/false with defined/undefined metadata, ZF variations, partial-register/full-state/flag preservation, final-byte fetch-fault atomicity, no-REX high-byte and other condition/register/REX/prefix/memory neighbors, and CPL3 isolation. The other-paged-profile matrix passes. Runner image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 9,036 instructions and stops at `41 83 FD F4` (`CMP R13D,-12`, ISSUE-327), RIP `0x20175e`, `Unsupported { opcode: 131 }`; COM1 is empty. All od-isa suites pass (2,322/1,591/126/29/6/980/7/15; 0 doc tests); all 65 paged machine quantum tests pass. Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped rustfmt and `git diff --check` pass; cargo check emits only the existing unused page-walk helper warning. Independent review approved with no findings. ISSUE-201 remains the unrelated pair of stale CLD-unsupported expectations in full od-machine tests.

### S7-04 CPU-profile slice: exact helper `CMP R13D,-12` (ISSUE-327)

The base `PagedCpl0ProbeV1` admits only exact bytes `41 83 FD F4`. REX.B selects R13D and opcode 83 sign-extends imm8 `0xf4` to dword -12. Shared dword subtract semantics set arithmetic flags/definedness without writing R13 or memory.

Four focused tests cover dword boundary flag vectors including equality, borrow/auxiliary carry, sign/overflow and parity; confirm upper R13 bits do not affect the dword operands; preserve all GPRs; and exercise final-immediate fetch-fault atomicity, neighboring encodings, CPL3 rejection and cross-profile isolation. Runner image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 9,037 instructions and stops at `0F 82 B7 00 00 00` (`JB rel32 +0xb7`, ISSUE-328), RIP `0x201762`, `Unsupported { opcode: 130 }`; COM1 remains empty. All od-isa suites pass (2,322/1,591/126/29/6/984/7/15; 0 doc tests), all 65 paged machine quantum tests pass. Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped rustfmt and `git diff --check` pass; cargo check emits only the existing unused page-walk helper warning. Independent review approved with no findings; post-review focused tests (4/4) and final diff-check pass. ISSUE-201 remains the unrelated pair of stale CLD-unsupported expectations in full od-machine tests.


### S7-04 CPU-profile slice: exact helper `JB rel32 +0xb7` (ISSUE-328)

The base `PagedCpl0ProbeV1` admits only exact no-prefix bytes `0F 82 B7 00 00 00`. JB tests CF alone, adding signed rel32 +0xb7 to next RIP only when CF=1. It preserves flags/definedness and checks canonicality only for a taken target.

Five focused tests cover CF=0/1 with ZF variation and defined/undefined CF metadata, taken/fall-through RIP, taken-only canonicality, final-displacement-byte fetch-fault atomicity, neighboring conditions/displacements/prefixes, CPL3 and other-profile isolation. Runner image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 9,038 instructions and stops at `40 20 EE` (`AND SIL,BPL`, ISSUE-329), RIP `0x201768`, `Unsupported { opcode: 32 }`; COM1 is empty. All od-isa suites pass (2,322/1,591/126/29/6/989/7/15; 0 doc tests), and all 65 paged machine quantum tests pass. Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped rustfmt and `git diff --check` pass; cargo check emits only the existing unused page-walk helper warning. Independent review approved with no findings. ISSUE-201 remains the unrelated pair of stale CLD-unsupported expectations in full od-machine tests.


### S7-04 CPU-profile slice: exact helper `AND SIL,BPL` (ISSUE-329)

The base `PagedCpl0ProbeV1` admits exact bytes `40 20 EE`. Bare REX selects SIL and BPL rather than high-byte registers; opcode 20 uses SIL as destination and BPL as source. The byte AND writes only SIL[7:0], clears CF/OF, updates SF/ZF/PF, and leaves AF undefined through shared logic semantics.

Four focused tests cover truth vectors and flags/definedness, upper-RSI and full-state preservation, final-ModRM fetch-fault atomicity, no-REX/high-byte and direction/register/prefix/memory neighbors, CPL3 and other-profile isolation. Runner image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 9,039 instructions and stops at `0F 85 AE 00 00 00` (`JNZ rel32 +0xae`, ISSUE-330), RIP `0x20176b`, `Unsupported { opcode: 133 }`; COM1 is empty. All od-isa suites pass (2,322/1,591/126/29/6/993/7/15; 0 doc tests), paged machine quantum passes (65), scoped cargo check, rustfmt and diff-check pass; cargo check emits only the existing unused page-walk helper warning. Independent review approved with no findings; reviewer focused tests (4/4), profile matrix (1/1), and diff-check pass. ISSUE-201 remains the unrelated pair of stale CLD-unsupported expectations in full od-machine tests.


### S7-04 CPU-profile slice: exact helper `JNZ rel32 +0xae` (ISSUE-330)

The base `PagedCpl0ProbeV1` admits only exact no-prefix bytes `0F 85 AE 00 00 00`. JNZ tests ZF alone, adding signed rel32 +0xae to next RIP only when ZF=0. It preserves flags/definedness and checks canonicality only for a taken target.

Five focused tests cover ZF outcomes and defined/undefined metadata, taken/fall-through RIP, taken-only canonicality, final-displacement-byte fetch-fault atomicity, neighboring conditions/displacements/prefixes, CPL3 and other-profile isolation. Runner image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 9,040 instructions and stops at `41 8B 6C 04 E8` (`MOV EBP,dword ptr [R12+RAX-0x18]`, ISSUE-331), RIP `0x201771`, `Unsupported { opcode: 139 }`; COM1 is empty. All od-isa suites pass (2,322/1,591/126/29/6/998/7/15; 0 doc tests), all 65 paged machine quantum tests pass. Scoped cargo check, rustfmt and diff-check pass; cargo check emits only the existing unused page-walk helper warning. Independent review approved with no findings. ISSUE-201 remains the unrelated pair of stale CLD-unsupported expectations in full od-machine tests.


### S7-04 CPU-profile slice: exact helper `MOV EBP,dword ptr [R12+RAX-0x18]` (ISSUE-331)

The base `PagedCpl0ProbeV1` admits only exact bytes `41 8B 6C 04 E8`. REX.B selects the SIB base R12; RAX is the scale-1 index; disp8 `0xe8` sign-extends to -0x18. The qword effective-address sum wraps at 64 bits and must be canonical before the tracked four-byte read. Success writes EBP, zero-extending RBP, and preserves flags/definedness. Data and instruction-fetch faults leave the destination and RIP uncommitted.

Six focused profile tests cover normal/wrapped effective addresses, dword result and zero-extension, full-state and flags, read-fault atomicity, noncanonical #GP before read, final-byte fetch-fault atomicity, neighbors, CPL3 and cross-profile isolation. Two paged machine tests verify the tracked four-level read sets Accessed bits on the code/data paths without Dirty and an unmapped data-page #PF does not partially commit state. Focused runs passed: 6 profile cases, 1 other-profile matrix case, and 2 machine integration cases. Full `cargo test --offline -p od-isa --quiet` passed (2,322/1,591/126/29/6/1,004/7/15; 0 doc tests); the full paged machine quantum suite passed (67). Scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, Rust formatting and `git diff --check` passed; cargo check emitted only the existing unused page-walk helper warning. Runner image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires 9,041 instructions and stops at RIP `0x201776`, bytes `31 F6` (`XOR ESI,ESI`, ISSUE-332), `Unsupported { opcode: 49 }`; COM1 remains empty. Independent review approved with no remaining findings; reviewer reran profile tests (6/6), the cross-profile matrix (1/1), machine tests (2/2), and diff-check. ISSUE-201 remains the unrelated pair of stale CLD-unsupported expectations in full `od-machine` tests.


### S7-04 CPU-profile slice: exact helper `XOR ESI,ESI` (ISSUE-332)

The base `PagedCpl0ProbeV1` admits only exact no-prefix bytes `31 F6`, dword self-XOR of ESI. This writes zero to ESI and zero-extends RSI. Shared logic semantics clear CF/OF, set ZF/PF, clear SF, and mark AF undefined while retaining its concrete value; unrelated flags/definedness and every other GPR are preserved.

Four focused profile tests cover different initial RSI high halves, exact zero result and flag outcomes, AF definedness, final-ModRM fetch-fault atomicity, neighbors, and CPL3 rejection. The other-profile matrix rejects `31 F6`. The firmware runner advances after 9,044 retired instructions to ISSUE-333: `45 31 ED` (`XOR R13D,R13D`) at RIP `0x201782`, `Unsupported { opcode: 49 }`; COM1 is empty. Full od-isa suites pass (2,322/1,591/126/29/6/1,008/7/15; 0 doc tests), the full paged machine quantum suite passes (67), and scoped cargo check, formatting and diff-check pass. Cargo check reports only the existing unused page-walk helper warning. Independent review approved with no remaining findings; reviewer reran focused tests (4/4), the cross-profile matrix (1/1), and diff-check. ISSUE-201 remains the unrelated pair of stale CLD-unsupported expectations in full `od-machine` tests.


### S7-04 CPU-profile slice: exact helper `XOR R13D,R13D` (ISSUE-333)

The base `PagedCpl0ProbeV1` admits only exact bytes `45 31 ED`. REX.R and REX.B jointly select R13D as both operands; dword self-XOR writes zero and zero-extends R13. CF/OF clear, ZF/PF set, SF clears, and AF's concrete value is retained while AF-definedness is cleared. All other GPRs and unrelated flags/definedness remain unchanged.

Four focused tests cover three initial R13 values and high halves, logical flag results and AF definedness, final-ModRM fetch-fault atomicity, neighbors and CPL3 rejection. The other-profile matrix rejects the encoding. The runner advances after 9,046 retired instructions to ISSUE-334: `41 0F 94 C5` (`SETE R13B`, with REX.B extending ModRM r/m=5) at RIP `0x201787`, `Unsupported { opcode: 148 }`; COM1 remains empty. Full od-isa suites pass (2,322/1,591/126/29/6/1,012/7/15; 0 doc tests), all 67 paged machine quantum tests pass, and scoped cargo check, formatting and diff-check pass; cargo check emits only the existing unused page-walk helper warning. Independent review approved with no remaining findings; reviewer reran focused tests (4/4), the cross-profile matrix (1/1), and diff-check. ISSUE-201 remains the unrelated pair of stale CLD-unsupported expectations in full `od-machine` tests.


### S7-04 CPU-profile slice: exact helper `SETE R13B` (ISSUE-334)

The base `PagedCpl0ProbeV1` admits only exact bytes `41 0F 94 C5`. REX.B extends ModRM r/m=5 to R13B; without REX, the byte register encoding would select CH. SETE reads ZF and writes one or zero only to R13B, preserving upper R13, all other GPRs, flags and definedness. The exact raw ModRM, REX, opcode, length and byte operand width are checked.

Four focused profile tests cover ZF true/false with defined/undefined metadata, partial-register preservation and full state, final-byte instruction-fetch fault atomicity, neighboring encodings, and CPL3 rejection. The cross-profile matrix rejects the exact instruction. Focused tests passed (4/4), as did the other-profile matrix (1/1). Full `cargo test --offline -p od-isa --quiet` passed (2,322/1,591/126/29/6/1,016/7/15; 0 doc tests), all 67 paged machine quantum tests passed, scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets` passed with only the existing unused page-walk helper warning, and scoped Rust formatting plus `git diff --check` passed. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,048 retired instructions to `44 0F 44 EE` (`CMOVE R13D,ESI`, ISSUE-335) at RIP `0x20178f`, `Unsupported { opcode: 68 }`; COM1 remains empty. Independent review approved with no remaining findings. ISSUE-201 remains the unrelated pair of stale CLD-unsupported expectations in full `od-machine` tests.


### S7-04 CPU-profile slice: exact helper `CMOVE R13D,ESI` (ISSUE-335)

The base `PagedCpl0ProbeV1` admits only exact bytes `44 0F 44 EE`. REX.R extends ModRM.reg to R13D and r/m selects ESI. CMOVE copies ESI when ZF=1. When ZF=0, it writes back the old low R13D; the dword write zero-extends R13 in either case. Flags, flag definedness, other registers and memory/I/O state are preserved.

Four focused profile tests cover both ZF outcomes with defined and undefined metadata, full-state preservation and false-condition zero-extension, final-byte instruction-fetch fault atomicity, neighboring encodings, and CPL3 rejection. The cross-profile matrix rejects the exact instruction. Focused tests passed (4/4), as did the other-profile matrix (1/1). Full `cargo test --offline -p od-isa --quiet` passed (2,322/1,591/126/29/6/1,020/7/15; 0 doc tests), all 67 paged machine quantum tests passed, scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets` passed with only the existing unused page-walk helper warning, and scoped Rust formatting plus `git diff --check` passed. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,049 retired instructions to `45 84 ED` (`TEST R13B,R13B`, ISSUE-336) at RIP `0x201793`, `Unsupported { opcode: 132 }`; COM1 remains empty. Independent review approved with no remaining findings. ISSUE-201 remains the unrelated pair of stale CLD-unsupported expectations in full `od-machine` tests.


### S7-04 CPU-profile slice: exact helper `TEST R13B,R13B` (ISSUE-336)

The base `PagedCpl0ProbeV1` admits only exact bytes `45 84 ED`; REX.R+B extend ModRM.reg and r/m to R13B. Shared TEST computes the byte AND for flags without writing R13. CF/OF clear, SF/ZF/PF derive from the byte result, and AF retains its prior concrete bit while becoming undefined. Other flags and GPRs are preserved.

Four focused profile tests cover six low-byte patterns, logical flags, AF and other flag-definedness, full-state preservation, final-ModRM fetch-fault atomicity, neighboring encodings, and CPL3 rejection. The cross-profile matrix rejects this exact form. Focused tests passed (4/4), as did the other-profile matrix (1/1). Full `cargo test --offline -p od-isa --quiet` passed (2,322/1,591/126/29/6/1,024/7/15; 0 doc tests), all 67 paged machine quantum tests passed, scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets` passed with only the existing unused page-walk helper warning, and scoped Rust formatting plus `git diff --check` passed. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,050 retired instructions to `0F 84 D1 FE FF FF` (`JZ rel32 -0x12f`, ISSUE-337) at RIP `0x201796`, `Unsupported { opcode: 132 }`; COM1 remains empty. Independent review approved with no remaining findings. ISSUE-201 remains the unrelated pair of stale CLD-unsupported expectations in full `od-machine` tests.


### S7-04 CPU-profile slice: exact helper `JZ rel32 -0x12f` (ISSUE-337)

The base `PagedCpl0ProbeV1` admits only exact bytes `0F 84 D1 FE FF FF`. The signed rel32 displacement is applied to next RIP. JZ reads ZF and preserves flags/definedness; taken targets are checked for canonicality while a not-taken fall-through is checked on the subsequent fetch.

Five focused profile tests cover taken/not-taken targets with defined and undefined ZF, taken-only canonicality including #GP for a noncanonical taken target, final-displacement fetch-fault atomicity, neighboring encodings, and CPL3 rejection. The other-profile matrix rejects the exact form. Focused tests passed (5/5), as did the other-profile matrix (1/1). Full `cargo test --offline -p od-isa --quiet` passed (2,322/1,591/126/29/6/1,029/7/15; 0 doc tests), all 67 paged machine quantum tests passed, scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets` passed with only the existing unused page-walk helper warning, and scoped Rust formatting plus `git diff --check` passed. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,051 retired instructions to `49 8B 74 04 F4` (`MOV RSI,qword ptr [R12+RAX-0x0c]`, ISSUE-338) at RIP `0x20179c`, `Unsupported { opcode: 139 }`; COM1 remains empty. Independent review approved with no remaining findings. ISSUE-201 remains the unrelated pair of stale CLD-unsupported expectations in full `od-machine` tests.


### S7-04 CPU-profile slice: exact helper `MOV RSI,qword ptr [R12+RAX-0x0c]` (ISSUE-338)

`PagedCpl0ProbeV1` admits only exact bytes `49 8B 74 04 F4`. REX.B selects R12 as SIB base, RAX is the scale-1 index, and disp8 `0xf4` sign-extends to -0x0c. The qword effective address wraps at 64 bits, is checked for canonicality, and is read through the tracked memory path. RSI and RIP commit only after a successful read; flags/definedness and other state remain unchanged.

Six focused profile tests cover normal and wrapping effective addresses, loaded qword/full-state/flag preservation, read and noncanonical-address faults, final-displacement fetch-fault atomicity, neighboring encodings and CPL3 isolation; the cross-profile matrix rejects this encoding. All focused tests (6/6), the other-profile matrix (1/1), and two `paged_probe_quantum` tests pass. Full `cargo test --offline -p od-isa --quiet` passed (2,322/1,591/126/29/6/1,035/7/15; 0 doc tests), all 69 paged machine quantum tests passed, and scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets` passed with only the existing unused page-walk helper warning. Scoped rustfmt on the three touched Rust files and `git diff --check` pass. Workspace `cargo fmt --all -- --check` reports formatting diffs only in untouched `crates/od-host/src/storage_abi_v1.rs` and `tools/image-tool/tests/cli.rs`; these were not modified. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,052 retired instructions to `48 85 F6` (`TEST RSI,RSI`, ISSUE-339) at RIP `0x2017a1`, `Unsupported { opcode: 133 }`; COM1 remains empty. Independent review approved after one plan Markdown-fence correction; no findings remain. ISSUE-201 remains the unrelated pair of stale CLD-unsupported expectations in full `od-machine` tests.


### S7-04 CPU-profile slice: exact helper `TEST RSI,RSI` (ISSUE-339)

`PagedCpl0ProbeV1` admits exact bytes `48 85 F6` as qword RSI self-TEST. The instruction writes no GPR. Shared TEST semantics clear CF/OF, derive SF/ZF from the full qword result and PF from its low byte, while preserving AF's concrete bit and marking AF undefined; unrelated flags and definedness remain unchanged.

Four focused tests cover zero/nonzero/sign/parity outcomes, definedness metadata, full-register preservation, final-ModRM fetch-fault atomicity, neighboring operands/widths/prefixes/opcodes, and CPL3 rejection. The other-profile matrix rejects the exact form. Full `cargo test --offline -p od-isa --quiet` passed (2,322/1,591/126/29/6/1,039/7/15; 0 doc tests), all 69 paged machine quantum tests passed, and scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets` passed with only the existing unused page-walk helper warning. Scoped rustfmt on touched Rust files and `git diff --check` pass. Workspace `cargo fmt --all -- --check` reports diffs only in untouched `crates/od-host/src/storage_abi_v1.rs` and `tools/image-tool/tests/cli.rs`. The firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,053 retired instructions to `0F 84 B6 FE FF FF` (`JZ rel32 -0x14a`, ISSUE-340) at RIP `0x2017a4`, `Unsupported { opcode: 132 }`; COM1 remains empty. Independent review approved with no findings. ISSUE-201 remains the unrelated pair of stale CLD-unsupported expectations in full `od-machine` tests.


### S7-04 CPU-profile slice: exact helper `JZ rel32 -0x14a` (ISSUE-340)

`PagedCpl0ProbeV1` admits only exact no-prefix bytes `0F 84 B6 FE FF FF`. JZ reads ZF and applies signed rel32 -0x14a to next RIP when taken; flags/definedness are preserved. A taken target is canonicality-checked, while a not-taken noncanonical fall-through faults only on the next fetch.

Five focused tests cover both ZF values and defined/undefined ZF metadata, taken/fall-through RIP, taken-only canonicality with #GP for an invalid target, final-displacement-byte fetch-fault atomicity, neighboring displacement/condition/prefix/branch forms, and CPL3 rejection. The other-profile matrix rejects this exact form. Full `cargo test --offline -p od-isa --quiet` passed (2,322/1,591/126/29/6/1,044/7/15; 0 doc tests), all 69 paged machine quantum tests passed, and scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets` passed with only the existing unused page-walk helper warning. Scoped rustfmt on touched Rust files and `git diff --check` pass. Workspace `cargo fmt --all -- --check` reports diffs only in untouched `crates/od-host/src/storage_abi_v1.rs` and `tools/image-tool/tests/cli.rs`. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,054 retired instructions to `4D 8B 54 04 EC` (`MOV R10,qword ptr [R12+RAX-0x14]`, ISSUE-341) at RIP `0x2017aa`, `Unsupported { opcode: 139 }`; COM1 remains empty. Independent review approved with no findings. ISSUE-201 remains the unrelated pair of stale CLD-unsupported expectations in full `od-machine` tests.

### S7-04 CPU-profile slice: exact helper `MOV R10,qword ptr [R12+RAX-0x14]` (ISSUE-341)

`PagedCpl0ProbeV1` admits exact bytes `4D 8B 54 04 EC`. REX.R/B select R10 and R12; RAX is the scale-1 SIB index and disp8 `0xec` sign-extends to -0x14. The qword effective address wraps at 64 bits, is checked for canonicality, and uses the tracked read path. Success updates R10/RIP only; read/address/fetch faults leave CPU state unchanged. Flags and definedness are preserved.

Six focused profile tests pass, covering qword/address/wrap behavior, complete state preservation, memory/read traces, read/noncanonical/fetch faults, encoding neighbors and CPL3 rejection. The other-profile matrix passes. Two targeted machine tests pass: the successful four-level read updates accessed bits but not dirty bits, and an unmapped data read faults without destination commit. Full `cargo test --offline -p od-isa --quiet` passes (2,322/1,591/126/29/6/1,050/7/15; 0 doc tests), all 71 paged machine quantum tests pass, and scoped `cargo check --offline -p od-isa -p od-machine -p od-runner --all-targets`, scoped rustfmt and `git diff --check` pass. The existing unused page-walk-helper warning remains. Independent review approved with no findings.

The firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,055 retired instructions to `89 F5` (`MOV EBP,ESI`, ISSUE-342) at RIP `0x2017af`, `Unsupported { opcode: 137 }`; COM1 remains empty. ISSUE-201 remains the unrelated pair of stale CLD-unsupported expectations in full `od-machine` tests.

### S7-04 CPU-profile slice: exact helper `MOV EBP,ESI` (ISSUE-342)

`PagedCpl0ProbeV1` admits only exact no-prefix bytes `89 F5`. The source is ESI's low dword; writing the dword destination zero-extends into RBP. The slice changes only RBP and RIP and preserves RFLAGS/definedness. Other source/destination registers, operand widths, opcode direction, prefixes, CPL3 and other profiles remain unsupported.

Four focused profile tests pass, covering zero-extension/full-state and definedness preservation, final-ModRM fetch-fault atomicity, neighboring encodings, and CPL3 rejection. The other-profile matrix and the ISSUE-282 `MOV EBP,R14D` neighbor regression pass. Full od-isa suites pass (2,322/1,591/126/29/6/1,054/7/15; 0 doc tests), all 71 paged machine quantum tests pass, and scoped cargo check, rustfmt and diff-check pass. Independent review approved with no findings.

The firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,058 retired instructions to `0F 85 A0 FE FF FF` (`JNZ rel32 -0x160`, ISSUE-343) at RIP `0x2017ba`, `Unsupported { opcode: 133 }`; COM1 remains empty. ISSUE-201 remains the unrelated pair of stale CLD-unsupported expectations in full `od-machine` tests.

### S7-04 CPU-profile slice: exact helper `JNZ rel32 -0x160` (ISSUE-343)

`PagedCpl0ProbeV1` admits only exact no-prefix bytes `0F 85 A0 FE FF FF`. JNZ tests ZF and applies signed rel32 -0x160 from next RIP when ZF=0; when ZF=1, it falls through. Flags and definedness are preserved. A taken target is canonicality-checked, while a noncanonical fall-through is checked only on the next fetch.

Five focused tests cover both ZF outcomes with defined/undefined metadata, branch/fall-through RIP, taken-only canonicality including #GP at a noncanonical target, final-displacement-byte fetch-fault atomicity, neighboring displacements/conditions/prefixes, and CPL3 rejection. The other-profile matrix rejects the exact encoding. Full od-isa suites pass (2,322/1,591/126/29/6/1,059/7/15; 0 doc tests), all 71 paged machine quantum tests pass, and scoped cargo check, rustfmt and diff-check pass. Independent review approved with no findings.

The firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,059 retired instructions to `4C 01 D6` (`ADD RSI,R10`, ISSUE-344) at RIP `0x2017c0`, `Unsupported { opcode: 1 }`; COM1 remains empty. ISSUE-201 remains the unrelated pair of stale CLD-unsupported expectations in full `od-machine` tests.

### S7-04 CPU-profile slice: exact helper `ADD RSI,R10` (ISSUE-344)

`PagedCpl0ProbeV1` admits only exact bytes `4C 01 D6`. Opcode 01 uses RSI as the r/m destination and REX.R selects R10 as the source. The shared qword ADD operation writes RSI and arithmetic flags/definedness; all other GPRs are preserved.

Four focused tests cover qword arithmetic results (including carry, signed overflow, auxiliary carry, zero and parity), GPR preservation, flag-definedness updates, final-ModRM fetch-fault atomicity, nearby encodings and CPL3 rejection. The other-profile matrix rejects the exact encoding. Full od-isa suites pass (2,322/1,591/126/29/6/1,063/7/15; 0 doc tests), all 71 paged machine quantum tests pass, and scoped cargo check, rustfmt and diff-check pass. Independent review approved with no findings.

The firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances after 9,061 retired instructions to `4C 39 FE` (`CMP RSI,R15`, ISSUE-345) at RIP `0x2017c7`, `Unsupported { opcode: 57 }`; COM1 remains empty. ISSUE-201 remains the unrelated pair of stale CLD-unsupported expectations in full `od-machine` tests.


### S7-04 exact helper `CMP RSI,R15` — ISSUE-345

`PagedCpl0ProbeV1` admits only exact `4C 39 FE`. Opcode 39 uses RSI as the
qword r/m operand and REX.R extends the register source to R15. Shared CMP
semantics subtract RSI-R15, update CF/PF/AF/ZF/SF/OF and their definedness, and
write no GPR. Other registers remain unchanged.

Four focused profile tests cover equality, borrow, sign, overflow, AF,
full-register preservation, defined and undefined initial arithmetic
metadata, final-ModRM fetch-fault atomicity, neighbor rejection, CPL3 and
other-profile isolation. Full od-isa suites pass
(2,322/1,591/126/29/6/1,067/7/15; zero doc tests), all 71 paged machine
quantum tests pass, and scoped cargo check, rustfmt and diff-check pass.
Independent review approved with no findings. The unrelated ISSUE-201 stale
CLD expectations remain open.

Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`)
advances after 9,062 retired instructions to `41 0F 97 C5` (`SETA R13B`,
ISSUE-346) at RIP `0x2017ca`, `Unsupported { opcode: 151 }`; COM1 is empty.


### S7-04 exact helper `SETA R13B` — ISSUE-346

`PagedCpl0ProbeV1` admits only exact bytes `41 0F 97 C5`. SETA evaluates
CF=0 and ZF=0. REX.B extends ModRM r/m=5 to R13B; only that low byte receives
0 or 1, with upper R13 preserved. RFLAGS and flag-definedness remain
unchanged, including when CF or ZF is undefined. Other conditions, register
encodings, prefixes, CPL3 and other profiles remain unsupported.

Four focused tests cover all four CF/ZF concrete combinations crossed with
defined/undefined metadata, complete CPU state and byte preservation, final
ModRM fetch-fault atomicity, neighboring conditions/registers/REX/prefixes,
CPL3 and other-profile rejection. Full od-isa suites pass
(2,322/1,591/126/29/6/1,071/7/15; zero doc tests), all 71 paged machine
quantum tests pass, and scoped cargo check, rustfmt and diff-check pass.
Independent review approved with no findings. ISSUE-201's stale CLD
expectations remain unrelated and open.

Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`)
advances after 9,063 retired instructions to `41 08 ED` (`OR R13B,BPL`,
ISSUE-347) at RIP `0x2017ce`, `Unsupported { opcode: 8 }`; COM1 is empty.


### S7-04 exact helper `OR R13B,BPL` — ISSUE-347

`PagedCpl0ProbeV1` admits only exact bytes `41 08 ED`. For opcode 08, REX.B
extends ModRM r/m=5 to R13B, while ModRM.reg=5 remains BPL. OR writes only
R13[7:0], preserving the upper 56 bits; CF/OF clear, SF/ZF/PF reflect the
byte result, AF becomes undefined, and definedness follows shared logic
semantics. Other byte-register pairs, widths, opcode directions, prefixes,
CPL3 and other profiles remain unsupported.

Four focused tests cover byte values/flags and AF definedness, upper-register
and full-state preservation, final-ModRM fetch-fault atomicity, neighbors,
CPL3 and other-profile rejection. Full od-isa suites pass
(2,322/1,591/126/29/6/1,075/7/15; zero doc tests), all 71 paged machine
quantum tests pass, and scoped cargo check, rustfmt and diff-check pass.
Independent review approved with no findings. ISSUE-201's stale CLD expectations remain
unrelated and open.

Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`)
advances after 9,064 retired instructions to `0F 85 89 FE FF FF`
(`JNZ rel32 -0x177`, ISSUE-348) at RIP `0x2017d1`,
`Unsupported { opcode: 133 }`; COM1 is empty.


### S7-04 exact helper `JNZ rel32 -0x177` — ISSUE-348

`PagedCpl0ProbeV1` admits only exact no-prefix bytes `0F 85 89 FE FF FF`.
JNZ tests ZF; when clear, signed rel32 -0x177 is added to next RIP, otherwise
execution falls through. Flags and definedness are unchanged. Canonicality is
checked for a taken target; fetch faults during the displacement leave state
unchanged. Other displacements/conditions/prefixes, CPL3 and other profiles
remain unsupported.

Five focused tests cover both ZF values and metadata, exact branch targets,
taken-only canonicality, final-displacement-byte fetch-fault atomicity,
neighboring encodings, CPL3 and other-profile isolation. Full od-isa suites pass
(2,322/1,591/126/29/6/1,080/7/15; zero doc tests), all 71 paged machine quantum
tests pass, and scoped cargo check, Rust formatting and diff-check pass.
Independent review approved with no findings. ISSUE-201's stale CLD
expectations remain unrelated and open.

Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`)
advances after 9,065 retired instructions to `45 8B 6C 04 FC`
(`MOV R13D,dword ptr [R12+RAX-4]`, ISSUE-349) at RIP `0x2017d7`,
`Unsupported { opcode: 139 }`; COM1 is empty.


### S7-04 exact helper `MOV R13D,dword ptr [R12+RAX-4]` — ISSUE-349

`PagedCpl0ProbeV1` admits only exact bytes `45 8B 6C 04 FC`. REX.R selects
R13D, REX.B selects R12 as SIB base, RAX is the scale-1 index, and disp8
`0xfc` sign-extends to -4 in the 64-bit effective address. A tracked dword
read commits R13D and RIP only after success; writing R13D zero-extends R13.
Flags are unchanged. Read and fetch faults leave CPU state untouched. Other
destinations, bases, indexes, scales, displacements, widths, prefixes, CPL3 and
other profiles remain unsupported.

Six focused tests cover the loaded value and R13 zero-extension, signed address
arithmetic including wrap, canonical/read/fetch fault atomicity, neighbors,
CPL3 and other-profile isolation. Two four-level machine tests verify a
successful tracked walk sets A bits without D bits and a data-page fault does
not commit CPU state. Full od-isa suites pass
(2,322/1,591/126/29/6/1,086/7/15; zero doc tests), paged machine quantum passes
(73/73), and scoped cargo check, rustfmt and diff-check pass. Independent
review approved with no findings. ISSUE-201's stale CLD expectations remain
unrelated and open.

Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`)
advances after 9,067 retired instructions to `0F 95 44 24 07`
(`SETNE byte ptr [RSP+7]`, ISSUE-350) at RIP `0x2017e0`,
`Unsupported { opcode: 149 }`; COM1 is empty.


### S7-04 exact helper `SETNE byte ptr [RSP+7]` — ISSUE-350

`PagedCpl0ProbeV1` admits only exact no-prefix bytes `0F 95 44 24 07`.
SETNE stores one byte to the RSP-relative SIB address: 1 when ZF is clear, 0
when set. The instruction preserves flags and definedness. The tracked byte
write commits RIP only on success; a failed store leaves CPU state unchanged.
Other conditions, displacements, address tuples, prefixes, CPL3 and other
profiles remain unsupported.

Five focused profile tests cover ZF outcomes and metadata, the exact byte-only
write, neighboring-byte retention, memory-write/fetch-fault atomicity, neighbors
and CPL3/profile isolation. Two four-level machine tests confirm one-byte tracked store A/D effects and a
write-protection fault that preserves memory and CPU state. Full od-isa suites
pass (2,322/1,591/126/29/6/1,091/7/15; zero doc tests), paged machine quantum
passes (75/75), scoped cargo check passes with one existing unused page-walk
helper warning, and scoped rustfmt/diff-check pass. Independent review approved with no findings. ISSUE-201's stale CLD expectations remain unrelated and open.

Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`)
advances after 9,069 retired instructions to `40 0F 96 C5` (`SETBE BPL`,
ISSUE-351) at RIP `0x2017e8`, `Unsupported { opcode: 150 }`; COM1 is empty.

### S7-04 exact helper `SETBE BPL` — ISSUE-351

`PagedCpl0ProbeV1` admits only exact bytes `40 0F 96 C5`. REX presence makes
ModRM r/m=5 select BPL instead of CH. SETBE writes 1 when CF or ZF is set and
0 otherwise; it modifies only RBP[7:0], preserving upper RBP, all flags, and
flag-definedness. Other conditions/destinations/REX forms, prefixes, CPL3 and
other profiles remain unsupported.

Four focused profile tests cover every CF/ZF pair with each flag defined and
undefined, BPL-only mutation, final-ModRM fetch-fault atomicity, neighbors, and
CPL3/other-profile isolation. Full od-isa suites, paged machine quantum, scoped
cargo check, rustfmt, and diff-check pass. Independent review approved with no
findings. ISSUE-201's stale CLD expectations remain unrelated and open.

The firmware runner retires 9,070 instructions and stops at `40 84 6C 24 07`
(`TEST byte ptr [RSP+7],BPL`, ISSUE-352) at RIP `0x2017ec`, with
`Unsupported { opcode: 132 }`; COM1 remains empty.

### S7-04 exact helper `TEST byte ptr [RSP+7],BPL` — ISSUE-352

`PagedCpl0ProbeV1` admits only exact bytes `40 84 6C 24 07`. Bare REX selects
BPL as the source register; the memory operand is the byte at `[RSP+7]`. Shared
TEST semantics update CF/OF, SF/ZF/PF and AF-definedness while preserving AF's
concrete value. The instruction writes neither a GPR nor memory. The read uses
the tracked path; memory faults and a noncanonical stack address preserve CPU
state.

Six focused profile tests cover flag values/definedness, no register or memory
writes, read faults, noncanonical-stack #SS, final-byte fetch fault, encoding
neighbors and CPL3/other-profile isolation. Two four-level machine tests verify
tracked A-bit updates with no D-bit update on a successful read and atomic CPU
state on an absent data-page fault. Full od-isa suites, paged machine quantum,
scoped cargo check, rustfmt and diff-check pass. Independent review approved with no findings.
ISSUE-201's stale CLD expectations remain unrelated and open.

The firmware runner retires 9,071 instructions and stops at `0F 85 9F FE FF FF`
(`JNZ rel32 -0x161`, ISSUE-353) at RIP `0x2017f1`, with
`Unsupported { opcode: 133 }`; COM1 remains empty.

### ISSUE-353 — JNZ rel32 -0x161

Admitted the exact `0F 85 9F FE FF FF` form in the base profile. Tests verify
ZF truth/definedness, flags and state preservation, next-RIP-relative target,
taken-only canonicality, final-byte fetch-fault atomicity, adjacent encodings,
CPL3 rejection, and other-profile isolation. Firmware advances to ISSUE-354:
`41 83 3C 04 00` (`CMP dword ptr [R12+RAX],0`) at RIP `0x2017f7` after 9,072
retired instructions; COM1 is empty. The unrelated ISSUE-201 CLD expectation
caveat remains open. Full od-isa suites pass (2,322/1,591/126/29/6/1,106/7/15;
zero doc tests), paged machine quantum passes (77/77), scoped cargo check passes
with one existing unused page-walk helper warning, and scoped rustfmt/diff-check
pass. Independent review approved with no remaining findings.

### ISSUE-354 — CMP dword ptr [R12+RAX],0

The base profile admits only `41 83 3C 04 00`. The REX.B/SIB form reads a dword
through tracked memory and compares it with sign-extended imm8 zero using
shared dword CMP flags and definedness; it writes no GPR or memory. Five
focused profile tests and two four-level machine tests cover flag outcomes,
read/fetch faults, A/no-D page-walk effects, encoding neighbors and profile
isolation. The runner advances to ISSUE-355 (`0F 85 AB FE FF FF`, JNZ rel32
-0x155) at RIP `0x2017fc`, retired=9,073; COM1 is empty. ISSUE-201's unrelated
CLD expectation caveat remains open. Full od-isa (2,322/1,591/126/29/6/1,111/
7/15; zero doc tests), paged machine quantum (79/79), scoped cargo check,
rustfmt and diff-check pass; cargo check reports only the existing unused
page-walk helper warning. Independent review approved with no findings.

### ISSUE-355 — JNZ rel32 -0x155

The base profile admits only `0F 85 AB FE FF FF`. Tests cover ZF and definedness,
branch targets, taken-only canonicality, final displacement fetch faults,
neighbors, CPL3 and other-profile isolation. Firmware advances to ISSUE-356,
`4D 39 F2` (`CMP R10,R14`) at RIP `0x201802` after 9,074 retired instructions;
COM1 is empty. ISSUE-201's unrelated CLD expectation caveat remains open. Full od-isa (2,322/1,591/126/29/6/1,116/7/15; zero doc tests), paged machine
quantum (79/79), scoped cargo check, rustfmt and diff-check pass; cargo check
reports only the existing unused page-walk helper warning. Independent review
approved with no findings.

### ISSUE-356 — CMP R10,R14

Admitted exact `4D 39 F2` in the base profile. Qword CMP reads R10 and R14 in
opcode-39 direction, updates subtraction flags and definedness, and leaves all
GPRs unchanged. Four focused profile tests and the other-profile matrix cover
flags, fetch atomicity, encoding and CPL isolation. Firmware advances to
ISSUE-357 (`72 34`, JB rel8 +0x34) at RIP `0x201805` after 9,075 retired
instructions; COM1 is empty. ISSUE-201's CLD expectation caveat remains open.
Full od-isa (2,322/1,591/126/29/6/1,120/7/15; zero doc tests), paged machine
quantum (79/79), scoped cargo check, rustfmt and diff-check pass; cargo check
reports only the existing unused page-walk helper warning. Independent review approved with no findings.

### ISSUE-357 — JB rel8 +0x34

The base profile admits only `72 34`. JB branches when CF=1; the branch
preserves all CPU state except RIP and leaves RFLAGS definedness unchanged.
The signed rel8 target is based on next RIP, and canonicality is checked only
for a taken target. Five focused profile tests cover CF true/false with defined
and undefined CF metadata, targets, canonicality, rel8 fetch faults, neighbors,
CPL3 and other-profile isolation. The runner advances to ISSUE-358
(`48 83 C0 20`, `ADD RAX,0x20`) at RIP `0x201807` after 9,076 retired
instructions; COM1 is empty. Image SHA-256 is
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`.
ISSUE-201's unrelated CLD caveat remains open. Full od-isa
(2,322/1,591/126/29/6/1,125/7/15; zero doc tests), paged machine quantum
(79/79), workspace all-targets check, scoped rustfmt and diff-check pass.
`cargo fmt --all -- --check` reports an unrelated pre-existing formatting diff
in `crates/od-host/src/storage_abi_v1.rs`; that file was not changed. Existing
unused-code/import/variable warnings are in od-machine and od-host. Independent
review approved with no remaining findings.

### ISSUE-359 — INC R11

The base profile admits only exact bytes `49 FF C3`. REX.W/B select qword R11
for FF /0; shared INC updates OF/SF/ZF/AF/PF while preserving CF and its
definedness. Four focused profile tests cover wrap/overflow, zero/sign/parity
and AF vectors, CF value/metadata, preservation of other state, final-ModRM
fetch-fault atomicity, neighbors and CPL3; the other-profile matrix rejects the
encoding. Firmware advances to ISSUE-360 (`49 89 F6`, `MOV R14,RSI`) at RIP
`0x20180e` after 9,078 retired instructions; COM1 is empty. Image SHA-256 is
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`.
ISSUE-201's unrelated CLD caveat remains open. Full od-isa
(2,322/1,591/126/29/6/1,133/7/15; zero doc tests), paged machine quantum
(79/79), workspace all-targets check, scoped rustfmt and diff-check pass.
`cargo fmt --all -- --check` reports the unrelated pre-existing formatting diff
in `crates/od-host/src/storage_abi_v1.rs`; that file was not changed. Existing
warnings remain confined to unrelated od-machine/od-host code. Independent
review approved with no remaining findings.

### ISSUE-358 — ADD RAX,0x20

The base profile admits only `48 83 C0 20`, qword ADD RAX,sign-extended imm8
+0x20. Shared arithmetic semantics update RAX, arithmetic flags and
definedness. Four focused profile tests cover qword edge results, carry,
overflow, zero, parity, definedness, immediate-fetch atomicity, neighbors and
CPL3; the other-profile matrix rejects this exact form. The runner advances to
ISSUE-359 (`49 FF C3`, `INC R11`) at RIP `0x20180b` after 9,077 retired
instructions; COM1 is empty. Image SHA-256 is
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`.
ISSUE-201's unrelated CLD caveat remains open. Full od-isa
(2,322/1,591/126/29/6/1,129/7/15; zero doc tests), paged machine quantum
(79/79), workspace all-targets check, scoped rustfmt and diff-check pass.
`cargo fmt --all -- --check` reports the unrelated pre-existing formatting diff
in `crates/od-host/src/storage_abi_v1.rs`; that file was not changed. Existing
warnings remain confined to unrelated od-machine/od-host code. Independent
review approved with no remaining findings.

### ISSUE-360 — MOV R14,RSI

The base profile admits only exact bytes `49 89 F6`: opcode 89 with REX.B
selects R14 as the qword r/m destination and RSI as the reg source. Only R14
and RIP change; all flags, flag-definedness, and other registers are preserved.
Focused state/routing, final-ModRM fetch-fault, neighbor/CPL3 tests and the
other-profile rejection matrix pass. The firmware runner advances to ISSUE-361,
`49 39 C0` (`CMP R8,RAX`) at RIP `0x201811` after 9,079 retired instructions;
`Unsupported { opcode: 57 }`, COM1 empty. Image SHA-256 is
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`.
ISSUE-201's unrelated CLD caveat remains open. Full od-isa
(2,322/1,591/126/29/6/1,137/7/15; zero doc tests), paged machine quantum
(79/79), workspace all-targets check, scoped rustfmt and diff-check pass.
`cargo fmt --all -- --check` reports the unrelated pre-existing formatting diff
in `crates/od-host/src/storage_abi_v1.rs`; that file was not changed.
Independent review approved with no remaining findings.

### ISSUE-361 — CMP R8,RAX

The base profile admits only exact `49 39 C0`: opcode 39 with REX.B compares
qword R8 (r/m destination) against RAX (reg source). CMP writes no GPR and
updates arithmetic flags and their definedness. Four focused tests cover
qword compare vectors, GPR preservation, final-ModRM fetch-fault atomicity,
neighboring encodings and CPL3; the other-profile rejection matrix passes. The
firmware runner advances to ISSUE-362, `0F 85 C5 FE FF FF` (JNZ rel32 -0x13b)
at RIP `0x201814` after 9,080 retired instructions; `Unsupported { opcode: 133 }`,
COM1 empty. Image SHA-256 is
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`.
ISSUE-201's unrelated CLD caveat remains open. Full od-isa suites pass (2,322/1,591/126/29/6/1,141/7/15; zero doc tests), paged machine quantum passes (79/79), workspace all-targets `cargo check --offline` passes, scoped rustfmt and diff-check pass. Workspace `cargo fmt --all -- --check` still reports pre-existing formatting differences in untouched `crates/od-host/src/storage_abi_v1.rs` and `tools/image-tool/tests/cli.rs`. Independent review approved with no findings.

### ISSUE-362 — JNZ rel32 -0x13b

The base profile admits only exact bytes `0F 85 C5 FE FF FF`. JNZ tests ZF,
adds signed rel32 -0x13b to next RIP when ZF=0, otherwise falls through, and
preserves RFLAGS/definedness. Taken targets alone receive canonicality checks;
a final-displacement fetch fault does not retire. Five focused tests cover the
ZF/definedness matrix, target handling, fetch-fault atomicity, neighbors and
CPL3; the other-profile rejection matrix passes. The runner advances to
ISSUE-363, `E9 9E FE FF FF` (JMP rel32 -0x162) at RIP `0x20181a` after 9,831
retired instructions; `Unsupported { opcode: 233 }`, COM1 empty. Image SHA-256
is `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`.
ISSUE-201's unrelated CLD caveat remains open. Full od-isa suites pass (2,322/1,591/126/29/6/1,146/7/15; zero doc tests), paged machine quantum passes (79/79), workspace all-targets `cargo check --offline` passes, scoped rustfmt and diff-check pass. Workspace `cargo fmt --all -- --check` still reports pre-existing formatting differences in untouched `crates/od-host/src/storage_abi_v1.rs` and `tools/image-tool/tests/cli.rs`. Independent review approved with no findings.

### ISSUE-363 — JMP rel32 -0x162

The base profile admits only exact bytes `E9 9E FE FF FF`. The signed rel32
-0x162 is added to next RIP; the target is canonicality-checked before RIP
commits. Registers, RFLAGS and flag-definedness are preserved. Five focused
tests cover signed target calculation, canonicality fault, final-immediate
fetch-fault atomicity, neighbors and CPL3; the other-profile rejection matrix
passes. The runner advances to ISSUE-364, `66 C7 07 FF FF` (MOV word ptr
[RDI],0xffff) at RIP `0x2016bd` after 9,832 retired instructions;
`Unsupported { opcode: 199 }`, COM1 empty. Image SHA-256 is
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`.
ISSUE-201's unrelated CLD caveat remains open. Full od-isa suites pass (2,322/1,591/126/29/6/1,151/7/15; zero doc tests), paged machine quantum passes (79/79), workspace all-targets `cargo check --offline` passes, scoped rustfmt and diff-check pass. Workspace `cargo fmt --all -- --check` still reports pre-existing formatting differences in untouched `crates/od-host/src/storage_abi_v1.rs` and `tools/image-tool/tests/cli.rs`. Independent review approved with no findings.

### ISSUE-364 — MOV word ptr [RDI],0xffff

The base profile admits only exact `66 C7 07 FF FF`. The word immediate store
uses the tracked paged-memory write path; the adapter now maps two-byte writes
to `AccessWidth::Word`. It changes exactly two bytes at RDI, preserving adjacent
bytes and CPU flags/registers, and commits RIP only after a successful write.
Five focused tests cover data width, fault atomicity, neighbors and CPL3; two
machine tests cover tracked four-level A/D effects and write-protection fault
atomicity. Other profiles reject the encoding. Firmware advances to ISSUE-365,
`E9 65 01 00 00` (JMP rel32 +0x165) at RIP `0x2016c2` after 9,833 retired
instructions; `Unsupported { opcode: 233 }`, COM1 empty. Image SHA-256 is
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`.
ISSUE-201's unrelated CLD caveat remains open. Full od-isa suites pass (2,322/1,591/126/29/6/1,156/7/15; zero doc tests), paged machine quantum passes (81/81), workspace all-targets `cargo check --offline` passes, scoped rustfmt and diff-check pass. Workspace `cargo fmt --all -- --check` still reports pre-existing formatting differences in untouched `crates/od-host/src/storage_abi_v1.rs` and `tools/image-tool/tests/cli.rs`. Independent review approved with no findings.


### ISSUE-365 — exact rel32 JMP +0x165

Admitted only `E9 65 01 00 00` in the base paged CPL0 profile. Five focused
tests cover signed target computation from next RIP, complete CPU and flag
metadata preservation, noncanonical target fault, final displacement-byte fetch
fault atomicity, neighboring encodings, CPL3 and other-profile isolation. The
firmware runner on `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`)
advances to `48 83 C4 08` (`ADD RSP,8`, ISSUE-366) at RIP `0x20182c` after
9,834 retired instructions; exit is unsupported opcode `0x83`, COM1 is empty.
ISSUE-201 remains the unrelated CLD expectation caveat.

Validation: the five focused ISSUE-365 tests and cross-profile matrix pass. Full
od-isa suites pass (2,322/1,591/126/29/6/1,161/7/15; zero doc tests), paged
machine quantum passes (81/81), workspace all-targets cargo check passes, and
scoped `rustfmt --edition 2024 --check` plus `git diff --check` pass. Workspace
check reports existing unused/dead-code warnings in `od-machine` and `od-host`.
Independent review approved with no findings. ISSUE-201 remains the unrelated CLD expectation
caveat.


### ISSUE-366 — exact ADD RSP,8

Admitted only `48 83 C4 08` in the base paged CPL0 profile. Four focused tests
cover qword RSP arithmetic including wrap, shared arithmetic flag values and
definedness, all other state preservation, final immediate-byte fetch-fault
atomicity, neighboring operation/width/prefix encodings and CPL3; the other
profile matrix rejects this exact form. Firmware on
`/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances
to `5B` (`POP RBX`, ISSUE-367) at RIP `0x201830` after 9,835 retired
instructions; exit is unsupported opcode `0x5b`, COM1 empty. ISSUE-201 remains
the unrelated CLD expectation caveat. Full od-isa suites pass
(2,322/1,591/126/29/6/1,165/7/15; zero doc tests), paged machine quantum passes
(81/81), workspace all-targets check passes with existing warnings, and scoped
rustfmt/diff-check pass. Independent review approved with no findings.


### ISSUE-367 — exact POP RBX

Admitted only bare `5B` in the base paged CPL0 profile. The qword stack value is
read via GuestMemory; RBX/RSP/RIP are committed only after the read succeeds, and
flags remain unchanged. Focused tests cover qword state, stack #SS/#PF and fetch
fault atomicity, neighbors, CPL3 and other-profile isolation. Three machine tests
verify four-level successful read with accessed/no-dirty effects, nonpresent data
page #PF with atomic CPU state, and noncanonical RSP #SS. Firmware on
`/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances
to `41 5C` (POP R12, ISSUE-368) at RIP `0x201831` after 9,836 retired
instructions; exit is unsupported opcode `0x5c`, COM1 empty. ISSUE-201 remains
the unrelated CLD expectation caveat. Full od-isa suites pass (2,322/1,591/126/29/6/1,171/7/15; zero doc tests),
paged machine quantum passes (84/84), workspace all-targets cargo check passes
with existing warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.


### ISSUE-368 — exact POP R12

Admitted only `41 5C` in base `PagedCpl0ProbeV1`. The exact REX.B stack-register
form reads a qword from old RSP via GuestMemory and commits R12, RSP+8, and RIP+2
only after successful stack validation/read. Flags and definedness remain
unchanged. Noncanonical RSP raises #SS; data read and second-byte instruction
fetch faults leave CPU state unchanged. Six focused ISA tests cover result/state,
flags metadata, #SS/#PF/fetch atomicity, neighbors, CPL3 and profile isolation.
Three machine tests verify four-level A/no-D behavior, nonpresent data page #PF
atomicity, and #SS.

Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advanced
to `41 5D` (POP R13, ISSUE-369) at RIP `0x201833` after 9,837 retired
instructions; exit is `Unsupported { opcode: 93 }`, COM1 empty. ISSUE-201's
unrelated stale CLD expectations remain open.

Validation: focused ISA tests pass (6/6); focused machine tests pass (3/3).
Full `cargo test --offline -p od-isa --quiet` passes (2,322/1,591/126/29/6/
1,177/7/15; zero doc tests); paged machine quantum passes (87/87);
`cargo check --workspace --all-targets --offline` passes with existing warnings;
scoped `rustfmt --edition 2024 --check` and `git diff --check` pass.
Independent review is pending.


### ISSUE-369 — exact POP R13

Admitted only `41 5D` in base `PagedCpl0ProbeV1`. REX.B selects R13 for the
POP opcode; it reads a qword from old RSP via GuestMemory, then commits R13,
RSP+8 and RIP+2 after stack-address validation and successful read. Flags and
definedness remain unchanged. Noncanonical RSP raises #SS; data #PF and
second-byte instruction-fetch faults leave CPU state unchanged. Six focused ISA
tests cover qword state, flags metadata, #SS/#PF/fetch atomicity, neighbors,
CPL3 and other-profile isolation. Three machine tests verify four-level A/no-D
effects, data-page #PF atomicity, and #SS.

Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advanced
to `41 5E` (POP R14, ISSUE-370) at RIP `0x201835` after 9,838 retired
instructions; exit is `Unsupported { opcode: 94 }`, COM1 empty. ISSUE-201's
unrelated stale CLD expectations remain open.

Validation: focused ISA 6/6 and machine 3/3 pass. Full `cargo test --offline
-p od-isa --quiet` passes (2,322/1,591/126/29/6/1,183/7/15; zero doc tests);
paged machine quantum passes (90/90); `cargo check --workspace --all-targets
--offline` passes with existing warnings; scoped `rustfmt --edition 2024 --check`
and `git diff --check` pass. Independent review is pending.


### ISSUE-370 — exact POP R14

Admitted only `41 5E` in base `PagedCpl0ProbeV1`. REX.B selects R14 for the
POP opcode; it reads a qword from old RSP via GuestMemory, then commits R14,
RSP+8 and RIP+2 after stack-address validation and successful read. Flags and
definedness remain unchanged. Noncanonical RSP raises #SS; data #PF and
second-byte instruction-fetch faults leave CPU state unchanged. Six focused ISA
tests cover qword state, flags metadata, #SS/#PF/fetch atomicity, neighbors,
CPL3 and other-profile isolation. Three machine tests verify four-level A/no-D
effects, data-page #PF atomicity, and #SS.

Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advanced
to `41 5F` (POP R15, ISSUE-371) at RIP `0x201837` after 9,839 retired
instructions; exit is `Unsupported { opcode: 95 }`, COM1 empty. ISSUE-201's
unrelated stale CLD expectations remain open.

Validation: focused ISA 6/6 and machine 3/3 pass. Full `cargo test --offline
-p od-isa --quiet` passes (2,322/1,591/126/29/6/1,189/7/15; zero doc tests);
paged machine quantum passes (93/93); `cargo check --workspace --all-targets
--offline` passes with existing warnings; scoped `rustfmt --edition 2024 --check`
and `git diff --check` pass. Independent review approved with no findings.


### ISSUE-371 — exact POP R15

Admitted only `41 5F` in base `PagedCpl0ProbeV1`. REX.B selects R15 for the
POP opcode; it reads a qword from old RSP via GuestMemory, then commits R15,
RSP+8 and RIP+2 after stack-address validation and successful read. Flags and
definedness remain unchanged. Noncanonical RSP raises #SS; data #PF and
second-byte instruction-fetch faults leave CPU state unchanged. Six focused ISA
tests cover qword state, flags metadata, #SS/#PF/fetch atomicity, neighbors,
CPL3 and other-profile isolation. Three machine tests verify four-level A/no-D
effects, data-page #PF atomicity, and #SS.

Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advanced
to `5D C3` (POP RBP, ISSUE-372) at RIP `0x201839` after 9,840 retired
instructions; exit is `Unsupported { opcode: 93 }`, COM1 empty. ISSUE-201's
unrelated stale CLD expectations remain open.

Validation: focused ISA 6/6 and machine 3/3 pass. Full od-isa suites pass (2,322/1,591/126/29/6/1,195/7/15; zero doc tests),
paged machine quantum passes (96/96), workspace all-targets cargo check passes
with existing warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.


### ISSUE-372 — exact POP RBP

Admitted only bare `5D` in base `PagedCpl0ProbeV1`. It reads a qword from old
RSP via GuestMemory, then commits RBP, RSP+8 and RIP+1 after stack-address
validation and successful read. Flags and definedness remain unchanged.
Noncanonical RSP raises #SS; data #PF and instruction-fetch faults leave CPU
state unchanged. Six focused ISA tests cover qword state, flags metadata,
#SS/#PF/fetch atomicity, neighbors, CPL3 and other-profile isolation. Three
machine tests verify four-level A/no-D effects, data-page #PF atomicity, and #SS.

Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advanced
to `C3` (near RET, ISSUE-373) at RIP `0x20183a` after 9,841 retired
instructions; exit is `Unsupported { opcode: 195 }`, COM1 empty. ISSUE-201's
unrelated stale CLD expectations remain open.

Validation: focused ISA 6/6 and machine 3/3 pass. Full od-isa suites pass (2,322/1,591/126/29/6/1,201/7/15; zero doc tests),
paged machine quantum passes (99/99), workspace all-targets cargo check passes
with existing warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.


### ISSUE-373 — exact near RET

Admitted only bare `C3` in base `PagedCpl0ProbeV1`. It reads the qword target
from old RSP through GuestMemory, checks target canonicality before committing,
then advances RSP by eight and loads RIP from that target. Flags and definedness
remain unchanged. Noncanonical RSP raises #SS; stack #PF or fetch fault does not
partially commit; noncanonical target raises #GP without changing CPU state.
Seven focused ISA tests cover successful state transition, flags metadata,
#SS/#PF/#GP/fetch atomicity, neighbors, CPL3 and other-profile isolation. Three
machine tests verify four-level A/no-D effects, stack-read #PF atomicity, and
#GP for a noncanonical target.

Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) followed
RET to `66 83 7C 24 18 FF` (CMP word [RSP+0x18],-1, ISSUE-374) at RIP
`0x200fe7` after 9,842 retired instructions; exit is
`Unsupported { opcode: 131 }`, COM1 empty. ISSUE-201's unrelated stale CLD
expectations remain open.

Validation: focused ISA 7/7 and machine 3/3 pass. Full od-isa suites pass (2,322/1,591/126/29/6/1,208/7/15; zero doc tests),
paged machine quantum passes (102/102), workspace all-targets cargo check passes
with existing warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.


### ISSUE-374 — exact word CMP memory read

The base `PagedCpl0ProbeV1` admits only `66 83 7C 24 18 FF` for this helper
compare: `CMP word ptr [RSP+0x18],-1`. The decoder supplies a word memory
operand and sign-extended imm8 -1. Execution reads exactly two bytes through
GuestMemory, computes shared 16-bit CMP flags, and commits flags/definedness
and RIP+6 only. Registers and memory are not written. Data-read faults and the
fetch fault on the final immediate byte preserve CPU state. Five focused ISA
tests cover result/flags/definedness, 2-byte addressing, read and immediate
fetch faults, neighbors and CPL3/other-profile rejection. Two machine tests
verify the four-level read sets accessed but not dirty bits and a missing data
PTE faults without CPU commit.

The firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances
to `75 3C` (JNZ rel8 +0x3c, ISSUE-375) at RIP `0x200fed` after 9,843 retired
instructions. Runner reports `Unsupported { opcode: 117 }`; COM1 is empty.
ISSUE-201 remains the unrelated stale CLD expectation caveat.

Validation: focused ISA 5/5 and machine 2/2 pass. Full od-isa suites pass
(2,322/1,591/126/29/6/1,213/7/15; zero doc tests); paged machine quantum
passes 104/104; workspace all-targets cargo check, scoped rustfmt and
diff-check pass. Independent review approved with no findings.


### ISSUE-375 — exact JNZ rel8 +0x3c

The base `PagedCpl0ProbeV1` admits only `75 3C` for this helper branch. JNZ
tests ZF: clear ZF branches by signed rel8 +0x3c from next RIP; set ZF falls
through by two. RFLAGS and definedness are preserved. The taken target is
checked for canonicality; fall-through canonicality is deferred until the next
fetch. A fault fetching either opcode or displacement does not commit state.
Six focused JNZ tests cover ZF outcomes with defined/undefined metadata, full
state preservation, canonicality, fetch fault, neighbors and CPL3 rejection;
the shared other-profile matrix rejects the bytes too.

Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances
to `4C 89 E9` (MOV RCX,R13, ISSUE-376) at RIP `0x200ffe` after 9,847 retired
instructions. Runner reports `Unsupported { opcode: 137 }`; COM1 is empty.
ISSUE-201 remains the unrelated stale CLD expectation caveat.

Validation: six focused JNZ tests and the other-profile rejection matrix pass.
Full od-isa suites pass (2,322/1,591/126/29/6/1,214/7/15; zero doc tests);
paged machine quantum passes 104/104; workspace all-targets offline cargo
check, scoped rustfmt and diff-check pass. Independent review approved with no findings.


### ISSUE-376 — exact MOV RCX,R13 (`4C 89 E9`)

Only exact no-prefix bytes `4C 89 E9` are admitted in base `PagedCpl0ProbeV1`.
REX.W+R selects qword source R13; ModRM E9 selects RCX as destination. The
instruction changes RCX and RIP only, preserving all flags, definedness and
other state. A fault during instruction fetch does not partially commit.
Focused ISA tests cover full-width routing, preserved state, fetch fault,
neighbors and CPL3; the shared other-profile matrix passes.

Runner image `/private/tmp/od-fw-s7-04-issue256.bin` has SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`. After
the admission it retires 9,848 instructions and stops at `4D 89 E0` (MOV
R8,R12, ISSUE-377), RIP `0x201001`, `Unsupported { opcode: 137 }`; COM1 is
empty. ISSUE-201 remains the unrelated CLD expectation caveat.

Validation: focused ISA 4/4 and other-profile matrix 1/1 pass. Full od-isa
suites pass (2,322/1,591/126/29/6/1,218/7/15; zero doc tests), paged machine
quantum passes (104/104), workspace all-targets offline cargo check passes
with existing warnings, and scoped rustfmt/diff-check pass. Independent review
approved with no findings.


### ISSUE-377 — exact MOV R8,R12 (`4D 89 E0`)

Only exact no-prefix bytes `4D 89 E0` are admitted in base `PagedCpl0ProbeV1`.
REX.W+R+B and ModRM E0 select qword R12 as source and R8 as destination. The
instruction changes R8 and RIP only, preserving flags, definedness and other
state. Fetch faults do not partially commit. Four focused ISA tests cover
full-width routing, preserved state, fetch faults and neighbors; the shared
other-profile matrix passes.

Runner image `/private/tmp/od-fw-s7-04-issue256.bin` has SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`. After
the admission it retires 9,849 instructions and stops at `41 89 D9` (MOV
R9D,EBX, ISSUE-378; REX.W is clear and the dword write zero-extends R9), RIP
`0x201004`, `Unsupported { opcode: 137 }`; COM1 is
empty. ISSUE-201 remains the unrelated CLD expectation caveat.

Validation: focused ISA 4/4 and other-profile matrix 1/1 pass. Full od-isa
suites pass (2,322/1,591/126/29/6/1,222/7/15; zero doc tests), paged machine
quantum passes (104/104), workspace all-targets offline cargo check passes
with existing warnings, and scoped rustfmt/diff-check pass. Independent review
approved with no findings.


### ISSUE-378 — exact MOV R9D,EBX (`41 89 D9`)

Only exact `41 89 D9` is admitted in base `PagedCpl0ProbeV1`. REX.B selects
R9D as destination, REX.W clear selects dword width, and ModRM.reg selects
EBX. The write zero-extends R9; flags, definedness and other state are
preserved. Fetch faults are atomic. Four focused ISA tests cover the dword
routing, zero-extension, preserved state, fetch faults and neighbors; the
shared other-profile matrix passes.

Runner image `/private/tmp/od-fw-s7-04-issue256.bin` has SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`. After
the admission it retires 9,850 instructions and stops at `E8 24 02 00 00`
(CALL rel32 +0x224, ISSUE-379), RIP `0x201007`, `Unsupported { opcode: 232 }`;
COM1 is empty. ISSUE-201 remains the unrelated CLD expectation caveat.

Validation: focused ISA 4/4 and other-profile matrix 1/1 pass. Full od-isa
suites pass (2,322/1,591/126/29/6/1,226/7/15; zero doc tests), paged machine
quantum passes (104/104), workspace all-targets offline cargo check passes
with existing warnings, and scoped rustfmt/diff-check pass. Independent review
approved with no findings.


### ISSUE-379 — exact CALL rel32 +0x224 (`E8 24 02 00 00`)

Only exact `E8 24 02 00 00` is admitted in base `PagedCpl0ProbeV1`. The
signed rel32 target is next RIP +0x224 (`0x201230` for this firmware call).
The instruction pushes the next RIP as a qword at old RSP-8 via tracked
GuestMemory, then commits RSP and target RIP; flags and definedness are
preserved. Target canonicality is checked before stack write; noncanonical
stack ranges raise #SS. Tracked-write #PF and instruction-fetch faults leave
CPU state unchanged. Four focused ISA tests and two machine tests cover target,
return address, page-walk A/D behavior, write-protection fault, #SS, fetch
fault, neighbors and profile boundaries.

Runner image `/private/tmp/od-fw-s7-04-issue256.bin` has SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`. After
the admission it retires 9,857 instructions and stops at `48 83 EC 18` (SUB
RSP,0x18, ISSUE-380), RIP `0x20123a`, `Unsupported { opcode: 131 }`; COM1 is
empty. ISSUE-201 remains the unrelated CLD expectation caveat.

Validation: focused ISA 4/4 and other-profile matrix 1/1 pass. Full od-isa
suites pass (2,322/1,591/126/29/6/1,230/7/15; zero doc tests), paged machine
quantum passes (106/106), workspace all-targets offline cargo check passes
with existing warnings, and scoped rustfmt/diff-check pass. Independent review
approved with no findings.


### ISSUE-380 — exact SUB RSP,0x18 (`48 83 EC 18`)

The base `PagedCpl0ProbeV1` admits exact no-prefix `48 83 EC 18`. The qword
SUB consumes sign-extended imm8 `0x18`, uses shared arithmetic flags and
definedness semantics, and changes only RSP, RFLAGS/definedness and RIP. This
register-only operation performs no memory access or canonicality validation
of the resulting RSP. Three focused ISA tests cover boundary result/flags,
definedness, noncanonical result, no memory effects and immediate-fetch fault;
neighbor, CPL3 and other-profile rejection tests cover isolation.

Runner image `/private/tmp/od-fw-s7-04-issue256.bin` has SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`. After
the admission it retires 9,858 instructions and stops at `66 C7 44 24 10 00
00` (word store, ISSUE-381), RIP `0x20123e`, `Unsupported { opcode: 199 }`;
COM1 is empty. ISSUE-201 remains the unrelated CLD expectation caveat.

Validation: focused SUB tests pass (3/3), along with neighbor/CPL3/other-profile
isolation. Full od-isa suites pass (2,322/1,591/126/29/6/1,233/7/15; zero doc
tests), paged machine quantum passes (106/106), workspace all-targets offline
check passes with existing warnings, and scoped rustfmt/diff-check pass.
Independent review approved with no findings.

### ISSUE-390 — exact qword ADD RAX,4 (`48 83 C0 04`)

The base `PagedCpl0ProbeV1` admits only this exact four-byte form: REX.W,
Primary opcode `83 /0`, qword RAX destination and sign-extended imm8 `+4`.
The executor reuses shared qword arithmetic for result, RFLAGS and flag
definedness. Focused cases check carry/AF/PF/ZF/SF/OF behavior, upper-qword
arithmetic, unchanged unrelated state, final-immediate fetch-fault atomicity,
neighbor encodings, CPL3 and other-profile isolation. Focused tests pass (4/4)
and the other-profile matrix passes (1/1).

On `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`), the
runner advances to `48 39 D0` (CMP RAX,RDX, ISSUE-391) at RIP `0x20127f` after
9,871 retired instructions, `Unsupported { opcode: 57 }`; COM1 is empty. Full
od-isa suites pass (2,322/1,591/126/29/6/1,279/7/15; zero doc tests), paged
machine quantum passes (110/110), workspace all-targets offline cargo check
passes with existing warnings, and scoped rustfmt and `git diff --check` pass.
Independent review approved with no findings. ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-381 — exact word store `[RSP+0x10],0` (`66 C7 44 24 10 00 00`)

Only exact `66 C7 44 24 10 00 00` is admitted in base `PagedCpl0ProbeV1`.
The decoded RSP SIB address adds 0x10 at qword address width and writes the
imm16 zero through tracked GuestMemory; no GPR or flag changes, RIP commits
only after write success. Noncanonical address, tracked #PF and fetch faults
are atomic. Six focused ISA tests cover width, value, flag/state preservation,
faults and boundaries; two machine tests verify the four-level A/D effects and
WP failure atomicity.

Runner image `/private/tmp/od-fw-s7-04-issue256.bin` has SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`. After
admission it retires 9,859 instructions and stops at `48 C7 44 24 08 00 00 00
00` (qword store, ISSUE-382), RIP `0x201245`, `Unsupported { opcode: 199 }`;
COM1 is empty. ISSUE-201 remains the unrelated CLD expectation caveat.

Validation: focused ISA tests 6/6, other-profile matrix 1/1, and machine
tests 2/2 pass. Full od-isa suites pass (2,322/1,591/126/29/6/1,239/7/15;
zero doc tests), paged quantum passes (108/108), workspace all-targets check
passes with existing warnings, and scoped rustfmt/diff-check pass.
Independent review approved with no findings.

### ISSUE-382 — exact qword immediate store `[RSP+8],0` (`48 C7 44 24 08 00 00 00 00`)

The base `PagedCpl0ProbeV1` now admits only this exact REX.W C7 /0 encoding.
The imm32 is sign-extended to qword zero and stored to the decoded qword SIB
address RSP+8 through tracked GuestMemory. GPRs and RFLAGS/definedness are
preserved; RIP advances only after a successful write. Focused ISA tests pass
(6/6), the other-profile rejection matrix passes (1/1), and two machine tests
pass for four-level A/D behavior and write-protection fault atomicity.

The runner on `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) advances
to `0F 84 17 02 00 00` (JZ rel32 +0x217, ISSUE-383) at RIP `0x201251` after
9,861 retired instructions; it reports `Unsupported { opcode: 132 }` and COM1 is
empty. ISSUE-201 remains the unrelated CLD expectation caveat.

Full od-isa suites pass (2,322/1,591/126/29/6/1,245/7/15; zero doc tests), paged machine quantum passes (110/110), workspace all-targets offline check passes with existing warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.

### ISSUE-383 — exact helper JZ rel32 +0x217 (`0F 84 17 02 00 00`)

The base `PagedCpl0ProbeV1` admits only this exact no-prefix JZ rel32. The
condition reads ZF, calculates a taken target relative to next RIP, preserves
flags/definedness, and checks target canonicality only if taken. Five focused
ISA tests cover both outcomes and flag-definedness, targets, canonicality, final
displacement fetch fault, neighbors and CPL3; the other-profile matrix passes.

On `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`), the
runner advances to `48 01 CE` (ADD RSI,RCX; ISSUE-384), RIP `0x201257`, after
9,862 retired instructions; it reports `Unsupported { opcode: 1 }` and COM1 is
empty. ISSUE-201 remains the unrelated CLD expectation caveat.

Full od-isa suites pass (2,322/1,591/126/29/6/1,250/7/15; zero doc tests), paged machine quantum passes (110/110), workspace all-targets offline check passes with existing warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.

### ISSUE-384 — exact helper ADD RSI,RCX (`48 01 CE`)

The base `PagedCpl0ProbeV1` admits only exact no-prefix `48 01 CE`. The
instruction computes qword `RSI + RCX`, with RSI the ModRM r/m destination and
RCX the reg source; shared arithmetic semantics update arithmetic RFLAGS and
definedness. Other GPRs and unrelated state are preserved, with no data-memory
or port access. Four focused tests pass for arithmetic edge cases/definedness,
final-ModRM fetch-fault atomicity, neighboring encodings, and CPL3 rejection;
the other-profile matrix passes.

On `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`), the
runner advances to `49 C1 E0 04` (SHL R8,4, ISSUE-385), RIP `0x20125a`, after
9,863 retired instructions; it reports `Unsupported { opcode: 193 }` and COM1
is empty. ISSUE-201 remains the unrelated CLD expectation caveat.

Full od-isa suites pass (2,322/1,591/126/29/6/1,254/7/15; zero doc tests),
paged machine quantum passes (110/110), workspace all-targets offline check
passes with existing warnings, and scoped rustfmt/diff-check pass. Independent
review approved with no findings.

### ISSUE-385 — exact SHL R8,4 (`49 C1 E0 04`)

The base `PagedCpl0ProbeV1` admits only exact REX.W+B `49 C1 E0 04`, raw
ModRM `/4`, qword SHL R8 by immediate 4. Shared shift semantics update R8 and
CF/SF/ZF/PF; OF and AF become undefined while their stored values are retained.
Four focused tests cover qword results/flags, final immediate fetch-fault
atomicity, neighbors and CPL3; the other-profile matrix passes.

The runner advances to `4B 8D 1C 40` (LEA RBX,[R8+R8*2], ISSUE-386), RIP
`0x20125e`, after 9,864 retired instructions, `Unsupported { opcode: 141 }`;
COM1 is empty. Image SHA-256 remains
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`.
ISSUE-201 remains the unrelated CLD expectation caveat. Full od-isa suites pass
(2,322/1,591/126/29/6/1,258/7/15; zero doc tests), paged machine quantum passes
(110/110), workspace all-targets offline cargo check, scoped rustfmt and
`git diff --check` pass. Independent review approved with no findings.

### ISSUE-386 — exact LEA RBX,[R8+R8*2] (`4B 8D 1C 40`)

The base `PagedCpl0ProbeV1` admits only exact `4B 8D 1C 40`. REX.W selects
qword RBX, REX.X/B extend both SIB index and base to R8, and the SIB scale is
2. LEA calculates R8 + R8*2 without memory access or canonicality checks. Four
focused tests cover the qword result, wrapping and noncanonical offsets, full
state/flags retention, final SIB-byte fetch-fault atomicity, neighbors and CPL3;
the other-profile matrix passes.

The runner advances to `BD 01 00 00 00` (MOV EBP,1, ISSUE-387), RIP `0x201265`,
after 9,866 retired instructions, `Unsupported { opcode: 189 }`; COM1 is empty.
Image SHA-256 remains
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`.
ISSUE-201 remains the unrelated CLD expectation caveat. Full od-isa suites pass
(2,322/1,591/126/29/6/1,262/7/15; zero doc tests), paged machine quantum passes
(110/110), workspace all-targets offline cargo check, scoped rustfmt and
`git diff --check` pass. Independent review approved with no findings.

### ISSUE-387 — exact MOV EBP,1 (`BD 01 00 00 00`)

This slice adds only exact no-prefix `BD 01 00 00 00` to base
`PagedCpl0ProbeV1`; it does not broaden pre-existing admissions. The dword
write zero-extends RBP and leaves RFLAGS/definedness unchanged. Four focused
tests cover exact state update, final-immediate fetch-fault atomicity, neighbors,
CPL3 and other-profile isolation; a compatibility test confirms the prior
`66 BD 01 00` MOV BP,1 form still preserves upper RBP bits. A qword MOV RBP,1
neighbor is explicitly rejected.

The runner advances to `49 BE 00 00 00 00 10 00 00 00` (MOV
R14,0x1000000000, ISSUE-388), RIP `0x20126a`, after 9,867 retired
instructions, `Unsupported { opcode: 190 }`; COM1 is empty. Image SHA-256
remains `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`.
ISSUE-201 remains the unrelated CLD expectation caveat. Full od-isa suites pass
(2,322/1,591/126/29/6/1,267/7/15; zero doc tests), paged machine quantum passes
(110/110), workspace all-targets offline cargo check, scoped rustfmt and
`git diff --check` pass. Independent review approved with no findings.

### ISSUE-388 — exact MOV R14,imm64 (`49 BE 00 00 00 00 10 00 00 00`)

Admit only exact `49 BE 00 00 00 00 10 00 00 00` in base
`PagedCpl0ProbeV1`. The full immediate is written to R14; only R14 and RIP
change, with flags and definedness preserved. Four focused tests cover exact
state, final-byte fetch-fault atomicity, neighboring values/encodings and CPL3;
the other-profile matrix covers the exact bytes.

On image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`), the
runner advances to `4A 8D 04 19` (LEA RAX,[RCX+R11], ISSUE-389), RIP
`0x201277`, after 9,869 retired instructions, `Unsupported { opcode: 141 }`;
COM1 is empty. ISSUE-201 remains the unrelated CLD expectation caveat. Full
od-isa suites pass (2,322/1,591/126/29/6/1,271/7/15; zero doc tests), paged
machine quantum passes (110/110), workspace all-targets offline cargo check
passes with existing warnings, and scoped rustfmt and `git diff --check` pass.
Independent review approved with no findings.

### ISSUE-389 — exact LEA RAX,[RCX+R11] (`4A 8D 04 19`)

Admit only exact `4A 8D 04 19` in base `PagedCpl0ProbeV1`. REX.W+X encode a
qword LEA with RCX as SIB base, R11 as index, scale 1, and displacement zero.
The effective offset wraps at 64 bits, requires no memory access, and is not
canonicality-checked. Only RAX/RIP change; flags and definedness are preserved.
Focused tests cover varied arithmetic, wrap/noncanonical values, final-byte
fetch-fault atomicity, neighbors and CPL3; the other-profile matrix covers the
exact bytes.

On image `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`), the
runner advances to `48 83 C0 04` (ADD RAX,4, ISSUE-390), RIP `0x20127b`, after
9,870 retired instructions, `Unsupported { opcode: 131 }`; COM1 is empty.
ISSUE-201 remains the unrelated CLD expectation caveat. Full od-isa suites pass
(2,322/1,591/126/29/6/1,275/7/15; zero doc tests), paged machine quantum passes
(110/110), workspace all-targets offline cargo check passes with existing
warnings, and scoped rustfmt and `git diff --check` pass. Independent review approved with no findings.

### ISSUE-391 — exact `CMP RAX,RDX` (`48 39 D0`)

Only exact `48 39 D0` is admitted in base `PagedCpl0ProbeV1`. REX.W selects
qword operation; opcode 39 uses RAX as r/m destination and RDX as reg source.
Shared CMP semantics update arithmetic flags and definedness without changing
GPRs. Focused tests cover equality, signed/unsigned and AF/PF cases, qword
high-bit behavior, final-ModRM fetch-fault atomicity, adjacent operands,
directions, prefixes, CPL3 and other profiles. Focused tests pass (4/4), and
the other-profile matrix passes (1/1). Full od-isa passes
(2,322/1,591/126/29/6/1,283/7/15; 0 doc tests), paged machine quantum passes
(110/110), `cargo check --workspace --all-targets --offline` passes, and scoped
rustfmt plus `git diff --check` pass. Independent review approved with no findings.

On `/private/tmp/od-fw-s7-04-issue256.bin` (SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`), the
runner advances to `0F 87 48 02 00 00` (JA rel32 +0x248, ISSUE-392), RIP
`0x201282`, after 9,872 retired instructions, `Unsupported { opcode: 135 }`;
COM1 is empty. ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-392 — exact `JA rel32 +0x248` (`0F 87 48 02 00 00`)

Only this exact displacement is admitted in base `PagedCpl0ProbeV1`. The branch
uses CF=0 && ZF=0, computes the signed rel32 target from next RIP, and preserves
all flags and definedness. Only taken targets receive a canonicality check.
Five focused tests cover condition/definedness, target/fallthrough, canonicality,
fetch-fault atomicity, neighboring forms and CPL3; the other-profile matrix
passes.

The firmware image `/private/tmp/od-fw-s7-04-issue256.bin` has SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`. After
admission, runner retires 9,873 instructions and stops at `4E 8D 04 19`
(`LEA R8,[RCX+R11]`, ISSUE-393), RIP `0x201288`,
`Unsupported { opcode: 141 }`; COM1 is empty. Full od-isa passes
(2,322/1,591/126/29/6/1,288/7/15; 0 doc tests), paged machine quantum passes
(110/110), workspace all-targets offline check passes, and scoped rustfmt and
diff-check pass. Independent review approved with no findings. ISSUE-201 remains
the unrelated CLD caveat.

### ISSUE-393 — exact `LEA R8,[RCX+R11]` (`4E 8D 04 19`)

Base `PagedCpl0ProbeV1` admits only this exact REX.W SIB form. REX.R selects
R8, REX.X extends the SIB index to R11, and RCX is the scale-one base. LEA
updates R8 and RIP only, preserves flags/definedness, and does not access
memory or check canonicality. Focused state/wrap/noncanonical/fetch-fault/
neighbor/CPL3 tests pass (4/4); other-profile matrix passes (1/1).

The firmware runner on image SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe` advances
to `49 83 C0 08` (ADD R8,8, ISSUE-394), RIP `0x20128c`, after 9,874 retired
instructions, `Unsupported { opcode: 131 }`; COM1 is empty. Full od-isa passes
(2,322/1,591/126/29/6/1,292/7/15; 0 doc tests), paged machine quantum passes
(110/110), workspace all-targets offline check passes, and scoped rustfmt and
diff-check pass. Independent review approved with no findings. ISSUE-201 remains the unrelated
CLD caveat.

### ISSUE-394 — exact `ADD R8,8` (`49 83 C0 08`)

Base `PagedCpl0ProbeV1` admits only this exact qword R8 immediate ADD. REX.W/B
select R8 and opcode 83 sign-extends imm8 +8. Shared ADD updates the qword
result and arithmetic flags/definedness, preserving other state. Focused tests
cover result/flag vectors, definedness, final-immediate fetch-fault atomicity,
neighbor encodings and CPL3 rejection (4/4); the other-profile matrix passes
(1/1).

The runner on image SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe` advances
to `49 39 D0` (CMP R8,RDX, ISSUE-395), RIP `0x201290`, after 9,875 retired
instructions, `Unsupported { opcode: 57 }`; COM1 is empty. Full od-isa passes
(2,322/1,591/126/29/6/1,296/7/15; 0 doc tests), paged machine quantum passes
(110/110), workspace all-targets offline check passes with existing warnings,
and scoped rustfmt and diff-check pass. Independent review is pending.
ISSUE-201 remains the unrelated CLD caveat.


### ISSUE-395 — `49 39 D0` (`CMP R8,RDX`)

The base paged profile admits only this exact qword CMP form. REX.W/B select R8
as the r/m operand and RDX as the reg operand. Shared subtraction updates
arithmetic flags/definedness and does not write a GPR; all other state remains
unchanged. Focused qword flags/register-preservation/fetch-fault/neighbor/CPL3
tests pass (4/4), and the other-profile matrix passes (1/1).

With image SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`, the
runner advances to `0F 87 4C 02 00 00` (JA rel32 +0x24c, ISSUE-396), RIP
`0x201293`, after 9,876 retired instructions, `Unsupported { opcode: 135 }`;
COM1 is empty. Full od-isa passes (2,322/1,591/126/29/6/1,300/7/15; zero doc
tests), paged machine quantum passes (110/110), workspace all-targets offline
check passes with existing warnings, and scoped rustfmt/diff-check pass.
Independent review is pending. ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-396 — exact `JA rel32 +0x24c` (`0F 87 4C 02 00 00`)

Base `PagedCpl0ProbeV1` admits only this exact JA displacement, with condition
CF=0 && ZF=0 and signed rel32 relative to next RIP. Flags/definedness are
preserved; target canonicality is checked only on the taken path. Five focused
condition/target/fetch-fault/neighbor/CPL3 tests pass; the other-profile matrix
passes (1/1). ISSUE-201 remains the unrelated CLD caveat.

With image SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`, the
runner advances to `48 83 C0 10` (ADD RAX,0x10, ISSUE-397), RIP `0x20129d`,
after 9,878 retired instructions, `Unsupported { opcode: 131 }`; COM1 is empty.
Full od-isa passes (2,322/1,591/126/29/6/1,305/7/15; zero doc tests), paged
machine quantum passes (110/110), workspace all-targets offline check passes
with existing warnings, and scoped rustfmt/diff-check pass. Independent review
is pending.

### ISSUE-397 — exact `48 83 C0 10` (`ADD RAX,0x10`)

The base paged profile admits only this qword accumulator immediate form.
Opcode 83 sign-extends imm8 +0x10; shared ADD updates RAX and arithmetic
flags/definedness. Focused result/flags/definedness/fetch-fault/neighbor/CPL3
tests pass (4/4), and the other-profile matrix passes (1/1). ISSUE-201 remains
the unrelated CLD caveat.

With image SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`, runner
advances to `0F 87 4D 02 00 00` (JA rel32 +0x24d, ISSUE-398), RIP `0x2012a4`,
after 9,880 retired instructions, `Unsupported { opcode: 135 }`; COM1 is empty.
Full od-isa passes (2,322/1,591/126/29/6/1,309/7/15; zero doc tests), paged
machine quantum passes (110/110), workspace all-targets offline check passes
with existing warnings, and scoped rustfmt/diff-check pass. Independent review
is pending.

### ISSUE-398 — exact `JA rel32 +0x24d` (`0F 87 4D 02 00 00`)

The base paged profile admits only this exact displacement. JA tests CF=0 &&
ZF=0, applies signed rel32 from next RIP, preserves flags/definedness and checks
canonicality only when taken. Focused condition/target/fetch-fault/neighbor/CPL3
tests pass (5/5); other-profile matrix passes (1/1). ISSUE-201 remains the
unrelated CLD caveat.

With image SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`, runner
advances to `49 83 C0 18` (ADD R8,0x18, ISSUE-399), RIP `0x2012ae`, after
9,882 retired instructions, `Unsupported { opcode: 131 }`; COM1 is empty. Full
od-isa passes (2,322/1,591/126/29/6/1,314/7/15; zero doc tests), paged machine
quantum passes (110/110), workspace all-targets offline check passes with
existing warnings, and scoped rustfmt/diff-check pass. Independent review is
pending.

### ISSUE-399 — exact helper ADD R8,0x18 (`49 83 C0 18`)

Implemented exact qword R8 ADD immediate in base `PagedCpl0ProbeV1`. The imm8
is sign-extended; shared ADD semantics update R8 plus arithmetic flags and
definedness. Focused semantic/state/fetch-fault/neighbor/CPL3 tests pass (4/4)
and other-profile matrix passes (1/1). ISSUE-201 remains the unrelated CLD
caveat.

Runner now advances to `0F 87 4E 02 00 00` (JA rel32 +0x24e, ISSUE-400) at RIP
`0x2012b5` after 9,884 retired instructions, `Unsupported { opcode: 135 }`;
COM1 is empty. Full `cargo test --offline -p od-isa --quiet` passes (2,322/1,591/126/29/6/1,318/7/15; zero doc tests), paged machine quantum passes (110/110), `cargo check --workspace --all-targets --offline` passes with existing warnings, and scoped rustfmt/diff-check pass. Independent review is pending.

### ISSUE-400 — exact helper JA rel32 +0x24e (`0F 87 4E 02 00 00`)

Implemented only this exact JA displacement in base `PagedCpl0ProbeV1`. The
condition is CF=0 && ZF=0; signed rel32 is added to next RIP; flags and
definedness are preserved; canonicality is checked only for a taken target.
Focused tests pass (5/5) and other-profile matrix passes (1/1). ISSUE-201
remains the unrelated CLD caveat.

Runner advances to `0F 87 4F 02 00 00` (JA rel32 +0x24f, ISSUE-401) at RIP
`0x2012c6` after 9,888 retired instructions, `Unsupported { opcode: 135}`;
COM1 is empty. Full `cargo test --offline -p od-isa --quiet` passes (2,322/1,591/126/29/6/1,323/7/15; zero doc tests), paged machine quantum passes (110/110), `cargo check --workspace --all-targets --offline` passes with existing warnings, and scoped rustfmt/diff-check pass. Independent review is pending.

### ISSUE-401 — exact helper JA rel32 +0x24f (`0F 87 4F 02 00 00`)

Implemented only this JA displacement in base `PagedCpl0ProbeV1`: CF=0 && ZF=0,
signed rel32 from next RIP, unchanged flags/definedness, and taken-only target
canonicality. Focused branch tests pass (5/5), both prior adjacent JA regression
tests pass (1/1 each), and other-profile matrix passes (1/1). ISSUE-201 remains
the unrelated CLD caveat.

Runner advances to `46 8B 04 1E` (MOV R8D,dword ptr [RSI+R11], ISSUE-402) at RIP
`0x2012cc` after 9,889 retired instructions, `Unsupported { opcode: 139}`;
COM1 is empty. Full od-isa passes (2,322/1,591/126/29/6/1,328/7/15; zero doc
tests), paged machine quantum passes (110/110), workspace all-targets offline
check passes with existing warnings, and scoped rustfmt/diff-check pass.
Independent review approved with no findings.

### ISSUE-402 — exact indexed dword load (`46 8B 04 1E`)

Implemented only `MOV R8D,dword ptr [RSI+R11]` in the base profile. REX.R/X
select R8D/R11; SIB uses scale 1, no displacement, and the tracked read writes
R8D with architectural zero-extension. Flags remain unchanged. Focused
success/read-fault/noncanonical/fetch-fault/neighbor/CPL3 tests pass (6/6), as
does the other-profile matrix (1/1). ISSUE-201 remains the unrelated CLD
caveat.

Runner advances to `41 8D 40 FF` (LEA EAX,[R8-1], ISSUE-403) at RIP `0x2012d0`
after 9,890 retired instructions, `Unsupported { opcode: 141}`; COM1 is empty.
Full `cargo test --offline -p od-isa --quiet` passes (2,322/1,591/126/29/6/1,334/7/15; zero doc tests), paged machine quantum passes (110/110), `cargo check --workspace --all-targets --offline` passes with existing warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.

### ISSUE-403 — exact `LEA EAX,[R8-1]` (`41 8D 40 FF`)

Implemented only this exact dword LEA in the base profile. REX.B selects R8 as
the 64-bit address base; qword EA arithmetic applies disp8 -1, then EAX receives
the low dword and zero-extends RAX. No memory access/canonicality check occurs;
flags and definedness are preserved. Focused tests pass (4/4) and the other-
profile matrix passes (1/1). ISSUE-201 remains the unrelated CLD caveat.

Runner command `cargo run --offline -q -p od-runner -- boot-firmware --image
/private/tmp/od-fw-s7-04-issue256.bin --com1-out
/private/tmp/od-fw-s7-04-issue403.com1 --budget 10000` (image SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires
9,891 instructions and stops at `83 F8 09` (CMP EAX,9, ISSUE-404), RIP
`0x2012d4`, `Unsupported { opcode: 131 }`, `com1_bytes=0` (expected runner exit
5 for unsupported). Full `cargo test --offline -p od-isa --quiet` passes
(2,322/1,591/126/29/6/1,338/7/15; zero doc tests), paged machine quantum passes
(110/110), workspace all-targets offline check passes with existing warnings,
and scoped rustfmt/diff-check pass. Independent review approved with no findings.

### ISSUE-404 — exact CMP EAX,9 (`83 F8 09`)

Implemented only the exact dword accumulator form in the base profile. Opcode
83 sign-extends imm8 9 to dword width; shared CMP computes subtraction flags and
definedness without changing a GPR. Focused flag/state/fetch-fault/neighbor/CPL3
tests pass (4/4), as does the other-profile matrix (1/1). ISSUE-201 remains the
unrelated CLD caveat.

Runner command `cargo run --offline -q -p od-runner -- boot-firmware --image
/private/tmp/od-fw-s7-04-issue256.bin --com1-out
/private/tmp/od-fw-s7-04-issue404.com1 --budget 10000` (image SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`) retires
9,892 instructions and stops at `0F 83 B2 01 00 00` (JAE rel32 +0x1b2,
ISSUE-405), RIP `0x2012d7`, `Unsupported { opcode: 131 }`, `com1_bytes=0`
(expected runner exit 5 for unsupported). Full `cargo test --offline -p od-isa
--quiet` passes (2,322/1,591/126/29/6/1,342/7/15; zero doc tests), paged
machine quantum passes (110/110), workspace all-targets offline check passes
with existing warnings, and scoped rustfmt/diff-check pass. Independent review
approved with no findings.


### ISSUE-405 — exact JAE rel32 +0x1b2 (`0F 83 B2 01 00 00`)

Added only the exact base-profile rel32 JAE form. CF=0 takes the branch; ZF is
ignored. The target is relative to next RIP, flags and definedness are
preserved, and canonicality is checked only for taken branches. Focused tests
pass (5/5), including definedness, both CF outcomes, target validation, final
byte fetch fault, neighbors and CPL3. Other-profile matrix passes (1/1).
ISSUE-201 remains the unrelated CLD caveat.

Runner image SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe` advances
to `41 83 F8 09` (CMP R8D,9, ISSUE-406), RIP `0x2012df`, after 9,894 retired
instructions, `Unsupported { opcode: 131}`, `com1_bytes=0` (expected runner
exit 5 for unsupported). Full `cargo test --offline -p od-isa --quiet` passes
(2,322/1,591/126/29/6/1,347/7/15; zero doc tests), paged machine quantum passes
(110/110), workspace all-targets offline cargo check passes with existing
warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.


### ISSUE-406 — exact `CMP R8D,9` (`41 83 F8 09`)

Added only the exact REX.B group-7 dword compare. REX.B selects R8D, REX.W is
clear, and opcode 83 sign-extends imm8 9 to dword width. Shared CMP updates
subtraction flags and definedness from low R8D without writing a GPR. Focused
R8D tests pass (4/4), CMP EAX regression tests pass (4/4), and other-profile
matrix passes (1/1). ISSUE-201 remains the unrelated CLD caveat.

Runner image SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe` advances
to `0F 94 C0` (SETE AL, ISSUE-407), RIP `0x2012e3`, after 9,895 retired
instructions, `Unsupported { opcode: 148}`, `com1_bytes=0` (expected runner
exit 5 for unsupported). Full `cargo test --offline -p od-isa --quiet` passes
(2,322/1,591/126/29/6/1,351/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline cargo check passes with
existing warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.


### ISSUE-407 — exact `SETE AL` (`0F 94 C0`)

Admitted only the exact unprefixed register encoding in the base paged profile.
ZF controls AL's 0/1 result; only AL is written, preserving upper RAX. Flags and
definedness are unchanged and the instruction performs no data-memory access.
Focused tests pass (4/4), covering both ZF values and definedness states, full
register/flags preservation, fetch fault, neighbors and CPL3. Other-profile
matrix passes (1/1). ISSUE-201 remains the unrelated CLD caveat.

Runner image SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe` advances
to `01 C0` (ADD EAX,EAX, ISSUE-408), RIP `0x2012e6`, after 9,896 retired
instructions, `Unsupported { opcode: 1}`, `com1_bytes=0` (expected runner
exit 5 for unsupported). Full `cargo test --offline -p od-isa --quiet` passes
(2,322/1,591/126/29/6/1,355/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline cargo check passes with
existing warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.


### ISSUE-408 — exact `ADD EAX,EAX` (`01 C0`)

Admitted only the exact base-profile dword register form. The low dword is
added to itself and written to EAX, zero-extending RAX; shared ADD supplies the
arithmetic flags and definedness. Focused tests pass (4/4), covering result,
zero-extension, flags, final-ModRM fetch fault, neighbors and CPL3. The
other-profile matrix passes (1/1). ISSUE-201 remains the unrelated CLD caveat.

Runner image SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe` advances
to `41 83 F8 08` (CMP R8D,8, ISSUE-409), RIP `0x2012e8`, after 9,897 retired
instructions, `Unsupported { opcode: 131}`, `com1_bytes=0` (expected runner
exit 5 for unsupported). Full `cargo test --offline -p od-isa --quiet` passes
(2,322/1,591/126/29/6/1,359/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline cargo check passes with
existing warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.

### ISSUE-409 — exact `CMP R8D,8` (`41 83 F8 08`)

The exact REX.B dword compare uses sign-extended imm8 8; REX.B selects R8D,
high R8 bits are ignored, and shared arithmetic provides flags/definedness
without a register write. Focused tests pass (4/4); other-profile matrix passes
(1/1). Full od-isa passes (2,322/1,591/126/29/6/1,363/7/15; zero doc tests),
paged machine quantum passes (110/110), workspace all-targets offline check
passes with existing warnings, and scoped rustfmt/diff-check pass. Independent
review approved with no findings. ISSUE-201 remains the unrelated CLD caveat.

Runner image SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`
advances to `0F 44 C5` (CMOVE EAX,EBP, ISSUE-410), RIP `0x2012ec`, after 9,898
retired instructions, `Unsupported { opcode: 68}`, `com1_bytes=0` (expected
runner exit 5 for unsupported).

### ISSUE-410 — exact `CMOVE EAX,EBP` (`0F 44 C5`)

Exact base-profile admission: ZF=1 copies low EBP into EAX; ZF=0 writes the old
low EAX value back, zero-extending RAX either way. Flags and definedness are
preserved. Focused tests pass (4/4), covering ZF true/false and defined/undefined
metadata, full register/RFLAGS state, final-byte fetch fault, encoding neighbors
and CPL3; other-profile matrix passes (1/1). Full od-isa passes
(2,322/1,591/126/29/6/1,367/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline check passes with existing
warnings, and scoped rustfmt/diff-check pass. Independent review approved with
no findings. ISSUE-201 remains the unrelated CLD caveat.

Runner advances to `66 41 39 C1` (CMP R9W,AX, ISSUE-411) at RIP `0x2012ef`,
after 9,899 retired instructions, unsupported opcode 57, COM1 empty.

### ISSUE-411 — exact `CMP R9W,AX` (`66 41 39 C1`)

Admit only exact `66 41 39 C1`. Operand-size override selects word width;
REX.B selects R9W as r/m destination and AX as the reg source. Shared CMP
computes 16-bit subtraction flags/definedness without register writes. Focused
tests pass (4/4), covering operand routing, edge flags, definedness, final-byte
fetch fault, neighbors and CPL3; other-profile matrix passes (1/1). Full od-isa
passes (2,322/1,591/126/29/6/1,371/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline check passes with existing
warnings, and scoped rustfmt/diff-check pass. Independent review approved with
no findings. ISSUE-201 remains the unrelated CLD caveat.

Runner advances to `0F 82 9D 01 00 00` (JB rel32 +0x19d, ISSUE-412) at RIP
`0x2012f3`, after 9,900 retired instructions, unsupported opcode 130, COM1 empty.

### ISSUE-412 — exact `JB rel32 +0x19d` (`0F 82 9D 01 00 00`)

Runner frontier: image SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`, RIP
`0x2012f3`, retired 9,900, bytes `0F 82 9D 01 00 00`, unsupported opcode 130,
COM1 empty. Only this exact base-profile form is admitted. JB tests CF=1,
preserves flags/definedness, computes rel32 from next RIP, and validates target
canonicality only when taken. Focused tests pass (5/5), other-profile matrix
passes (1/1), full od-isa passes (2,322/1,591/126/29/6/1,376/7/15; zero doc
tests), paged machine quantum passes (110/110), workspace all-targets offline
check and scoped rustfmt/diff-check pass. Independent review approved with no findings.
ISSUE-201 remains the unrelated CLD caveat.

Runner advances to `42 80 7C 04 08 01` (CMP byte ptr [RSP+R8+8],1; ISSUE-413),
RIP `0x2012f9`, after 9,901 retired instructions, unsupported opcode 128,
COM1 empty.

### ISSUE-413 — exact `CMP byte ptr [RSP+R8+8],1` (`42 80 7C 04 08 01`)

Runner frontier: image SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`, RIP
`0x2012f9`, retired 9,901, bytes `42 80 7C 04 08 01`, unsupported opcode 128,
COM1 empty. This exact base-profile form uses REX.X to select R8 in the SIB
index, with RSP base and disp8 +8. CMP reads one byte through tracked memory,
updates arithmetic flags/definedness, and writes no register or memory. Focused
tests pass (5/5), other-profile matrix passes (1/1), full od-isa passes
(2,322/1,591/126/29/6/1,381/7/15; zero doc tests), paged machine quantum passes
(110/110), workspace all-targets offline check and scoped rustfmt/diff-check
pass. Independent review approved with no findings. ISSUE-201 remains the unrelated CLD caveat.

Runner advances to `0F 84 9D 01 00 00` (JZ rel32 +0x19d; ISSUE-414), RIP
`0x2012ff`, after 9,902 retired instructions, unsupported opcode 132, COM1
empty.


### ISSUE-414 — exact `JZ rel32 +0x19d` (`0F 84 9D 01 00 00`)

Runner frontier: image SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`, RIP
`0x2012ff`, retired 9,902, bytes `0F 84 9D 01 00 00`, unsupported opcode 132,
COM1 empty. Only this exact base-profile encoding is admitted. JZ tests ZF,
preserves flags/definedness, computes rel32 from next RIP, and validates target
canonicality only when taken. Focused tests pass (5/5), other-profile matrix
passes (1/1), full od-isa passes (2,322/1,591/126/29/6/1,386/7/15; zero doc
tests), paged machine quantum passes (110/110), workspace all-targets offline
check and scoped rustfmt/diff-check pass. Independent review approved with no findings.
ISSUE-201 remains the unrelated CLD caveat.

Runner advances to `46 8B 64 1E 04` (MOV R12D,dword ptr [RSI+R11+4]; ISSUE-415),
RIP `0x201305`, after 9,903 retired instructions, unsupported opcode 139,
COM1 empty.

### ISSUE-415 — exact `MOV R12D,dword ptr [RSI+R11+4]` (`46 8B 64 1E 04`)

Runner frontier: image SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`, RIP
`0x201305`, retired 9,903, bytes `46 8B 64 1E 04`, unsupported opcode 139,
COM1 empty. This exact base-profile load uses REX.R for destination R12D and
REX.X for SIB index R11, with RSI base and disp8 +4. The tracked dword read
zero-extends R12; flags are unchanged and no state commits on fetch, canonical
address or data-read faults. Focused tests pass (6/6), other-profile matrix
passes (1/1), full od-isa passes (2,322/1,591/126/29/6/1,392/7/15; zero doc
tests), paged machine quantum passes (110/110), workspace all-targets offline
check and scoped rustfmt/diff-check pass. Independent review approved with no findings.
ISSUE-201 remains the unrelated CLD caveat.

Runner advances to `4E 8B 7C 1E 08` (MOV R15,qword ptr [RSI+R11+8]; ISSUE-416),
RIP `0x20130a`, after 9,904 retired instructions, unsupported opcode 139,
COM1 empty.

### ISSUE-416 — exact indexed qword load

At RIP `0x20130a`, exact bytes `4E 8B 7C 1E 08` admit `MOV R15,qword ptr
[RSI+R11+8]` in the base paged profile only. REX.R selects destination R15,
REX.X selects the R11 SIB index, RSI is the base, and disp8 is +8. The tracked
qword read preserves flags and commits neither R15 nor RIP on fetch,
noncanonical-address or data-read faults. Focused tests pass (6/6), other-
profile matrix passes (1/1), full od-isa passes
(2,322/1,591/126/29/6/1,398/7/15; zero doc tests), paged machine quantum passes
(110/110), workspace all-targets offline check passes with existing warnings,
and scoped rustfmt/diff-check pass. Independent review approved with no findings. ISSUE-201 remains
the unrelated CLD caveat.

Runner advances to `4A 8B 44 1E 10` (MOV RAX,qword ptr [RSI+R11+0x10]; ISSUE-417),
RIP `0x20130f`, after 9,905 retired instructions, unsupported opcode 139,
COM1 empty.

### ISSUE-417 — exact indexed qword load

At RIP `0x20130f`, exact bytes `4A 8B 44 1E 10` admit `MOV RAX,qword ptr
[RSI+R11+0x10]` in the base paged profile only. REX.X selects the R11 SIB
index, RSI is the base and disp8 is +0x10. The tracked qword read preserves
flags and commits neither RAX nor RIP on fetch, noncanonical-address or
data-read faults. Focused tests pass (6/6), other-profile matrix passes (1/1),
full od-isa passes (2,322/1,591/126/29/6/1,404/7/15; zero doc tests), paged
machine quantum passes (110/110), workspace all-targets offline check passes
with existing warnings, and scoped rustfmt/diff-check pass. Independent review
approved with no findings. ISSUE-201 remains the unrelated CLD caveat.



Runner advances to `46 8B 6C 1E 1C` (MOV R13D,dword ptr [RSI+R11+0x1c]; ISSUE-418),
RIP `0x201314`, after 9,906 retired instructions, unsupported opcode 139,
COM1 empty.

### ISSUE-418 — exact indexed dword load

At RIP `0x201314`, exact bytes `46 8B 6C 1E 1C` admit `MOV R13D,dword ptr
[RSI+R11+0x1c]` in the base paged profile only. REX.R selects R13D, REX.X
selects the R11 SIB index, RSI is the base, and disp8 is +0x1c. The tracked
dword read zero-extends R13, preserves flags, and commits neither R13 nor RIP
on fetch, noncanonical-address or data-read faults. Focused tests pass (6/6),
other-profile matrix passes (1/1), full od-isa passes
(2,322/1,591/126/29/6/1,410/7/15; zero doc tests), paged machine quantum passes
(110/110), workspace all-targets offline check passes with existing warnings,
and scoped rustfmt/diff-check pass. Independent review approved with no findings. ISSUE-201
remains the unrelated CLD caveat.

Runner advances to `42 C6 44 04 08 01` (MOV byte ptr [RSP+R8+8],1; ISSUE-419),
RIP `0x201319`, after 9,907 retired instructions, unsupported opcode 198,
COM1 empty.

### ISSUE-419 — exact `MOV byte ptr [RSP+R8+8],1` (`42 C6 44 04 08 01`)

At RIP `0x201319`, admit only this exact immediate byte store in the base
paged profile. REX.X selects R8 as the SIB index; RSP is the base, scale is 1,
disp8 is +8 and the immediate is 1. The tracked store writes one byte and
preserves registers and RFLAGS. Fetch, noncanonical RSP-based address and data
write faults do not retire the instruction or commit RIP; the data fault is
reported without partial memory modification. Focused byte-store/state/flags/
write-fault/noncanonical/fetch-fault/neighbors/CPL3 tests pass (6/6), other-
profile matrix passes (1/1). Full od-isa passes
(2,322/1,591/126/29/6/1,416/7/15; zero doc tests), paged machine quantum passes
(110/110), workspace all-targets offline check passes with existing warnings,
and scoped rustfmt/diff-check pass. Independent review approved with no findings. ISSUE-201
remains the unrelated CLD caveat.

Runner advances to `41 83 FC 03` (CMP R12D,3; ISSUE-420) at RIP `0x20131f`,
after 9,908 retired instructions, unsupported opcode 131, COM1 empty.

### ISSUE-420 — exact `CMP R12D,3` (`41 83 FC 03`)

At RIP `0x20131f`, admit only exact `41 83 FC 03` in the base paged profile.
REX.B selects R12D, and opcode 83 sign-extends imm8 3 to dword width. The
compare reads the low 32-bit operand, ignores upper R12, updates subtraction
flags and their definedness, and leaves all GPRs unchanged. Final-immediate
fetch faults leave flags and RIP unchanged. Focused dword-CMP/flags/
definedness/register-preservation/fetch-fault/neighbors/CPL3 tests pass (4/4),
other-profile matrix passes (1/1). Full od-isa passes
(2,322/1,591/126/29/6/1,420/7/15; zero doc tests), paged machine quantum passes
(110/110), workspace all-targets offline check passes with existing warnings,
and scoped rustfmt/diff-check pass. Independent review approved with no findings. ISSUE-201
remains the unrelated CLD caveat.

Runner advances to `0F 87 5B 01 00 00` (JA rel32 +0x15b; ISSUE-421) at RIP
`0x201323`, after 9,909 retired instructions, unsupported opcode 135, COM1
empty.

### ISSUE-421 — exact `JA rel32 +0x15b` (`0F 87 5B 01 00 00`)

Admitted only the exact base-profile branch. The predicate is CF=0 && ZF=0;
the signed displacement is added to next RIP, flags/definedness are preserved,
and target canonicality is checked only when taken. Five focused tests and the
other-profile matrix pass. Runner reaches ISSUE-422 `41 83 FD 04` at RIP
`0x201329` after 9,910 retired instructions, then reports Unsupported opcode
131 with empty COM1. Full od-isa passes
(2,322/1,591/126/29/6/1,425/7/15; zero doc tests), paged machine quantum passes
(110/110), workspace all-targets offline check passes with existing warnings,
and scoped rustfmt/diff-check pass. Independent review approved with no findings. ISSUE-201
remains the unrelated CLD caveat.

### ISSUE-422 — exact `CMP R13D,4` (`41 83 FD 04`)

Admitted only exact REX.B dword compare with sign-extended imm8 4. CMP updates
dword subtraction flags/definedness without changing R13. Four focused tests and
the other-profile matrix pass. Runner reaches ISSUE-423 `0F 83 76 01 00 00` at
RIP `0x20132d` after 9,911 retired instructions, then reports Unsupported
opcode 131 with empty COM1. Full od-isa passes
(2,322/1,591/126/29/6/1,429/7/15; zero doc tests), paged machine quantum passes
(110/110), workspace all-targets offline check passes with existing warnings,
and scoped rustfmt/diff-check pass. Independent review approved with no findings. ISSUE-201
remains the unrelated CLD caveat.

### ISSUE-423 — exact `JAE rel32 +0x176` (`0F 83 76 01 00 00`)

Admitted only this exact base-profile branch. JAE tests CF=0 and uses signed
rel32 from next RIP; flags/definedness are preserved and target canonicality is
checked only when taken. Five focused tests and the other-profile matrix pass.
Runner reaches ISSUE-424 `41 F6 C4 02` at RIP `0x201333` after 9,912 retired
instructions, then reports Unsupported opcode 246 with empty COM1. Full od-isa
passes (2,322/1,591/126/29/6/1,434/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline check passes with existing
warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.
ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-424 — exact `TEST R12B,2` (`41 F6 C4 02`)

Admitted only this exact immediate byte TEST in the base paged profile. It reads
R12B and immediate 2, writes no GPR, clears CF/OF, updates SF/ZF/PF, and marks
AF undefined while retaining its concrete value. Four focused tests and the
other-profile matrix (1/1) pass. Runner advances after 9,914 retired
instructions to `45 89 C5` (MOV R13D,R8D, ISSUE-425) at RIP `0x201339`,
Unsupported opcode 137, COM1 empty. Full od-isa passes
(2,322/1,591/126/29/6/1,438/7/15; zero doc tests), paged machine quantum passes
(110/110), workspace all-targets offline check passes with existing warnings,
and scoped rustfmt/diff-check pass. Independent review approved with no findings. ISSUE-201
remains the unrelated CLD caveat.

### ISSUE-425 — exact `MOV R13D,R8D` (`45 89 C5`)

Admitted only this exact base-profile dword register transfer. REX.R selects
source R8D, REX.B selects destination R13D, and REX.W is clear; writing R13D
zero-extends R13. Flags/definedness are preserved. Four focused tests and the
other-profile matrix (1/1) pass. Runner advances after 9,915 retired
instructions to `41 83 CD 08` (OR R13D,8, ISSUE-426) at RIP `0x20133c`,
Unsupported opcode 131, COM1 empty. Full od-isa passes
(2,322/1,591/126/29/6/1,442/7/15; zero doc tests), paged machine quantum passes
(110/110), workspace all-targets offline check passes with existing warnings,
and scoped rustfmt/diff-check pass. Independent review approved with no findings. ISSUE-201
remains the unrelated CLD caveat.

### ISSUE-426 — exact `OR R13D,8` (`41 83 CD 08`)

Admitted only this exact base-profile OR-immediate form. REX.B selects R13D;
opcode 83 /1 sign-extends imm8 8, and the dword write zero-extends R13. OR
clears CF/OF, updates SF/ZF/PF, and marks AF undefined while retaining its
concrete value. Four focused tests and other-profile matrix (1/1) pass. Runner
advances after 9,916 retired instructions to `41 83 FD 09` (CMP R13D,9,
ISSUE-427) at RIP `0x201340`, Unsupported opcode 131, COM1 empty. Full od-isa
passes (2,322/1,591/126/29/6/1,446/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline check passes with existing
warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.
ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-427 — exact `CMP R13D,9` (`41 83 FD 09`)

Admitted only this exact base-profile REX.B dword CMP. Opcode 83 sign-extends
imm8 9; comparison uses low R13D and updates subtraction flags/definedness
without writing a GPR. Four focused tests and other-profile matrix (1/1) pass.
Runner advances after 9,917 retired instructions to `0F 85 3A 01 00 00` (JNZ
rel32 +0x13a, ISSUE-428) at RIP `0x201344`, Unsupported opcode 133, COM1 empty.
Full od-isa passes (2,322/1,591/126/29/6/1,450/7/15; zero doc tests), paged
machine quantum passes (110/110), workspace all-targets offline check passes
with existing warnings, and scoped rustfmt/diff-check pass. Independent review
approved with no findings. ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-428 — exact `JNZ rel32 +0x13a` (`0F 85 3A 01 00 00`)

Admitted only the exact unprefixed base-profile displacement. JNZ uses ZF,
adds signed rel32 +0x13a to next RIP when taken, preserves flags/definedness,
and checks target canonicality only on the taken path. Five focused tests cover
the ZF/definedness matrix, targets, canonicality, final-displacement fetch
fault, neighboring encodings and CPL3; other-profile matrix 1/1 passes. Runner
advances after 9,918 retired instructions to `41 F6 C4 01` (TEST R12B,1,
ISSUE-429) at RIP `0x20134a`, Unsupported opcode 246, COM1 empty. Full od-isa
passes (2,322/1,591/126/29/6/1,455/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline check passes with existing
warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.
ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-429 — exact `TEST R12B,1` (`41 F6 C4 01`)

Admitted only this exact base-profile byte TEST. It reads R12B, writes no GPR,
clears CF/OF, sets SF/ZF/PF from the byte AND, and marks AF undefined without
changing its concrete value. Four focused tests cover flags/definedness,
register/state preservation, final-byte fetch fault, neighbors and CPL3;
other-profile matrix 1/1 passes. Runner advances after 9,920 retired
instructions to `41 83 C8 08` (OR R8D,8, ISSUE-430) at RIP `0x201370`,
Unsupported opcode 131, COM1 empty. Full od-isa passes
(2,322/1,591/126/29/6/1,459/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline check passes with existing
warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.
ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-430 — exact `OR R8D,8` (`41 83 C8 08`)

Admitted only the exact base-profile REX.B dword OR-immediate form. The result
zero-extends into R8; OR clears CF/OF, updates SF/ZF/PF, and marks AF undefined
while preserving its concrete value. Four focused tests cover logical result,
flags/definedness, state, final-byte fetch fault, neighbors and CPL3;
other-profile matrix 1/1 passes. Runner advances after 9,922 retired
instructions to `75 14` (JNZ rel8 +0x14, ISSUE-431) at RIP `0x201378`,
Unsupported opcode 117, COM1 empty. Full od-isa passes
(2,322/1,591/126/29/6/1,463/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline check passes with existing
warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.
ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-431 — exact `JNZ rel8 +0x14` (`75 14`)

Admitted only the exact base-profile short JNZ encoding. It branches when
ZF=0 using signed rel8 +0x14 from next RIP, preserves flags/definedness, and
checks canonicality only on the taken path. Focused cases cover ZF and
metadata, taken/fallthrough state, target canonicality, fetch fault, neighbors
and CPL3; other-profile matrix passes. Runner advances after 9,924 retired
to `0F 84 00 01 00 00` (JZ rel32 +0x100, ISSUE-432) at RIP `0x20137e`,
Unsupported opcode 132, COM1 empty. Full od-isa passes
(2,322/1,591/126/29/6/1,464/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline check passes with existing
warnings, and scoped rustfmt/diff-check pass. Independent review pending.
ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-432 — exact JZ rel32 +0x100

The base paged profile admits only `0F 84 00 01 00 00`. JZ reads ZF and adds
signed rel32 `+0x100` to next RIP when taken; flags and definedness are
unchanged, and only a taken target is checked for canonicality. Five focused
profile tests cover both ZF values/definedness, taken/fall-through RIP,
taken-only canonicality, last-byte fetch-fault atomicity, neighbors and CPL3;
the other-profile matrix is 1/1. Runner SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe` advances
after 9,925 retired instructions to `48 85 C0` (TEST RAX,RAX, ISSUE-433) at
RIP `0x201384`, Unsupported opcode 133, COM1 empty. Full od-isa passes
(2,322/1,591/126/29/6/1,469/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline check passes with existing
warnings, and scoped rustfmt/diff-check pass. Independent review is pending.
ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-433 — exact TEST RAX,RAX

The base paged profile admits only exact bytes `48 85 C0`. TEST leaves RAX and
all other GPRs unchanged, clears CF/OF, derives SF/ZF/PF from the qword AND,
and marks AF undefined while retaining its concrete value; the shared logical
semantics update definedness. Four focused profile tests cover varied qword
values/flags, full-state preservation, final-ModRM fetch-fault atomicity,
neighbors and CPL3; the other-profile matrix is 1/1. Runner advances after
9,926 retired instructions to `75 37` (JNZ rel8 +0x37, ISSUE-434) at RIP
`0x201387`, Unsupported opcode 117, COM1 empty. Full od-isa passes
(2,322/1,591/126/29/6/1,473/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline check passes with existing
warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.
ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-434 — exact JNZ rel8 +0x37

Five focused tests pass for both ZF outcomes and ZF definedness, branch target
and fall-through, taken-only canonicality, final-byte fetch-fault atomicity,
neighbors and CPL3 rejection. The other-profile matrix passes 1/1. Image
`/private/tmp/od-fw-s7-04-issue256.bin` has SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`. Runner
`cargo run --offline -q -p od-runner -- boot-firmware --image
/private/tmp/od-fw-s7-04-issue256.bin --com1-out
/private/tmp/od-fw-s7-04-issue434.com1 --budget 10000` retires 9,927
instructions and stops at `49 81 FF FF FF 00 00` (CMP R15,0xffff), RIP
`0x2013c0`, Unsupported opcode 129, COM1 empty. Full od-isa passes (2,322/1,591/126/29/6/1,478/7/15; zero doc tests), paged machine quantum passes (110/110), workspace all-targets offline check passes with existing warnings, and scoped rustfmt/diff-check pass; independent review approved with no findings. ISSUE-201 remains the
unrelated CLD caveat.

### ISSUE-435 — exact CMP R15,0xffff

Four focused tests pass for qword compare/flags/definedness, GPR preservation,
final-immediate fetch-fault atomicity, neighbors and CPL3; the other-profile
matrix passes 1/1. Image `/private/tmp/od-fw-s7-04-issue256.bin` has SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`. Runner
`cargo run --offline -q -p od-runner -- boot-firmware --image
/private/tmp/od-fw-s7-04-issue256.bin --com1-out
/private/tmp/od-fw-s7-04-issue435.com1 --budget 10000` retires 9,928
instructions and stops at `0F 87 B0 00 00 00` (JA rel32 +0xb0), RIP
`0x2013c7`, Unsupported opcode 135, COM1 empty. Full od-isa passes
(2,322/1,591/126/29/6/1,482/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline check passes with existing
warnings, and scoped rustfmt/diff-check pass; independent review approved with no findings.
ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-436 — exact JA rel32 +0xb0

Five focused tests pass for CF/ZF truth-table and definedness, target versus
fall-through, taken-only canonicality, final-displacement fetch-fault
atomicity, neighbors and CPL3. The other-profile matrix passes 1/1. Image
`/private/tmp/od-fw-s7-04-issue256.bin` has SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`. Runner
`cargo run --offline -q -p od-runner -- boot-firmware --image
/private/tmp/od-fw-s7-04-issue256.bin --com1-out
/private/tmp/od-fw-s7-04-issue436.com1 --budget 10000` retires 9,929
instructions and stops at `4C 01 F8` (ADD RAX,R15), RIP `0x2013cd`,
Unsupported opcode 1, COM1 empty. Full od-isa passes (2,322/1,591/126/29/6/1,487/7/15; zero doc tests), paged machine quantum passes (110/110), workspace all-targets offline check passes with existing warnings, and scoped rustfmt/diff-check pass; independent review approved with no findings. ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-437 — exact ADD RAX,R15

Four focused tests pass for qword result/flags/definedness, full-state
preservation, final-ModRM fetch-fault atomicity, neighbors and CPL3; the
other-profile matrix passes 1/1. Image SHA-256 is
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`. Runner
`cargo run --offline -q -p od-runner -- boot-firmware --image
/private/tmp/od-fw-s7-04-issue256.bin --com1-out
/private/tmp/od-fw-s7-04-issue437.com1 --budget 10000` retires 9,930
instructions and stops at `41 0F 92 C0` (SETB R8B), RIP `0x2013d0`,
Unsupported opcode 146, COM1 empty. Full od-isa passes
(2,322/1,591/126/29/6/1,491/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline check passes with existing
warnings, and scoped rustfmt/diff-check pass. Independent review pending.
ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-438 — exact SETB R8B

Four focused tests pass for both CF outcomes, defined/undefined CF metadata,
low-byte-only R8 update, upper-register/flags/full-state preservation,
final-ModRM fetch-fault atomicity, neighbors and CPL3; the other-profile matrix
passes 1/1. Image SHA-256 is
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`. Runner
`cargo run --offline -q -p od-runner -- boot-firmware --image
/private/tmp/od-fw-s7-04-issue256.bin --com1-out
/private/tmp/od-fw-s7-04-issue438.com1 --budget 10000` retires 9,931
instructions and stops at `48 3D 01 00 01 00` (CMP RAX,0x10001), RIP
`0x2013d4`, Unsupported opcode 61, COM1 empty. Full od-isa passes
(2,322/1,591/126/29/6/1,495/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline check passes with existing
warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.
ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-439 — exact CMP RAX,0x10001

Four focused tests pass for qword subtraction flags/definedness, unchanged
GPRs/full-state preservation, final-immediate fetch-fault atomicity, neighbors
and CPL3; the other-profile matrix passes 1/1. Image SHA-256 is
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`. Runner
`cargo run --offline -q -p od-runner -- boot-firmware --image
/private/tmp/od-fw-s7-04-issue256.bin --com1-out
/private/tmp/od-fw-s7-04-issue439.com1 --budget 10000` retires 9,932
instructions and stops at `0F 93 C0` (SETAE AL), RIP `0x2013da`, Unsupported
opcode 147, COM1 empty. Full od-isa passes
(2,322/1,591/126/29/6/1,499/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline check passes with existing
warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.
ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-440 — exact SETAE AL

Four focused tests pass for CF outcome with defined/undefined metadata, AL-only
mutation and upper-RAX preservation, flags/definedness/full-state preservation,
final-ModRM fetch-fault atomicity, neighboring encodings and CPL3; the other-
profile matrix passes 1/1. Image SHA-256 is
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`. Runner
`cargo run --offline -q -p od-runner -- boot-firmware --image
/private/tmp/od-fw-s7-04-issue256.bin --com1-out
/private/tmp/od-fw-s7-04-issue440.com1 --budget 10000` retires 9,933
instructions and stops at `44 08 C0` (OR AL,R8B), RIP `0x2013dd`, Unsupported
opcode 8, COM1 empty. Full od-isa passes
(2,322/1,591/126/29/6/1,503/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline check passes with existing
warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.
ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-441 — exact OR AL,R8B

Four focused tests pass for byte OR results, CF/OF clearing, SF/ZF/PF and AF
undefinedness, AL-only mutation with upper RAX and source R8 preservation,
final-ModRM fetch-fault atomicity, neighbors and CPL3; the other-profile matrix
passes 1/1. Image SHA-256 is
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`. Runner
`cargo run --offline -q -p od-runner -- boot-firmware --image
/private/tmp/od-fw-s7-04-issue256.bin --com1-out
/private/tmp/od-fw-s7-04-issue441.com1 --budget 10000` retires 9,934
instructions and stops at `0F 85 97 00 00 00` (JNZ rel32 +0x97), RIP
`0x2013e0`, Unsupported opcode 133, COM1 empty. Full od-isa passes
(2,322/1,591/126/29/6/1,507/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline check passes with existing
warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.
ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-442 — exact JNZ rel32 +0x97

Five focused tests pass for both ZF outcomes with defined/undefined ZF,
flags/definedness preservation, correct target/fall-through, taken-only
canonicality, final rel32 byte fetch-fault atomicity, neighbors and CPL3; the
other-profile matrix passes 1/1. Image SHA-256 is
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`. Runner
`cargo run --offline -q -p od-runner -- boot-firmware --image
/private/tmp/od-fw-s7-04-issue256.bin --com1-out
/private/tmp/od-fw-s7-04-issue442.com1 --budget 10000` retires 9,935
instructions and stops at `49 83 C3 30` (ADD R11,0x30), RIP `0x2013e6`,
Unsupported opcode 131, COM1 empty. Full od-isa passes
(2,322/1,591/126/29/6/1,512/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline check passes with existing
warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.
ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-443 — exact ADD R11,0x30

Four focused tests validate qword addition, carry/overflow/zero/sign/parity and
AF behavior, arithmetic flag definedness, full-state preservation, final
immediate-byte fetch-fault atomicity, neighboring encodings, and CPL3
rejection. The other-profile isolation matrix passes 1/1. Firmware image
`/private/tmp/od-fw-s7-04-issue256.bin` has SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`. Runner
`cargo run --offline -q -p od-runner -- boot-firmware --image
/private/tmp/od-fw-s7-04-issue256.bin --com1-out
/private/tmp/od-fw-s7-04-issue443.com1 --budget 10000` retires 9,936
instructions and stops at `49 FF C2` (INC R10), RIP `0x2013ea`, Unsupported
opcode 255, COM1 empty. Full od-isa passes
(2,322/1,591/126/29/6/1,516/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline check passes with existing
warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.
ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-444 — exact INC R10

Four focused tests cover qword wrapping, OF/SF/ZF/AF/PF result flags, CF value
and definedness preservation, unaffected state, final-ModRM fetch-fault
atomicity, neighbor encodings and CPL3 rejection. The other-profile isolation
matrix passes 1/1. Firmware image `/private/tmp/od-fw-s7-04-issue256.bin` has
SHA-256 `9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`.
Runner `cargo run --offline -q -p od-runner -- boot-firmware --image
/private/tmp/od-fw-s7-04-issue256.bin --com1-out
/private/tmp/od-fw-s7-04-issue444.com1 --budget 10000` retires 9,937
instructions and stops at `4C 39 DB` (CMP RBX,R11), RIP `0x2013ed`, Unsupported
opcode 57, COM1 empty. Full od-isa passes
(2,322/1,591/126/29/6/1,520/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline check passes with existing
warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.
ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-445 — exact CMP RBX,R11

Four focused tests cover qword operand routing, compare flags and definedness,
unchanged GPRs, final-ModRM fetch-fault atomicity, encoding neighbors and CPL3
rejection. The other-profile matrix passes 1/1. Firmware image
`/private/tmp/od-fw-s7-04-issue256.bin` has SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`.
Runner `cargo run --offline -q -p od-runner -- boot-firmware --image
/private/tmp/od-fw-s7-04-issue256.bin --com1-out
/private/tmp/od-fw-s7-04-issue445.com1 --budget 10000` retires 9,938
instructions and stops at `0F 85 81 FE FF FF` (JNZ rel32 -0x17f), RIP
`0x2013f0`, Unsupported opcode 133, COM1 empty. Full od-isa passes
(2,322/1,591/126/29/6/1,524/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline check passes with existing
warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.
ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-446 — exact JNZ rel32

Five focused tests cover both ZF values with defined/undefined ZF, signed target
calculation relative to next RIP, flags/definedness preservation, taken-only
canonicality, final-displacement fetch-fault atomicity, encoding neighbors and
CPL3 rejection. The other-profile matrix passes 1/1. Firmware image
`/private/tmp/od-fw-s7-04-issue256.bin` has SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`.
Runner `cargo run --offline -q -p od-runner -- boot-firmware --image
/private/tmp/od-fw-s7-04-issue256.bin --com1-out
/private/tmp/od-fw-s7-04-issue446.com1 --budget 10000` retires 9,990
instructions and stops at `0F 84 E6 00 00 00` (JZ rel32 +0xe6), RIP
`0x201391`, Unsupported opcode 132, COM1 empty. Full od-isa passes
(2,322/1,591/126/29/6/1,529/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline check passes with existing
warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.
ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-447 — exact JZ rel32

Five focused tests cover both ZF values with defined/undefined ZF, signed target
calculation relative to next RIP, flags/definedness preservation, taken-only
canonicality, final-displacement fetch-fault atomicity, encoding neighbors and
CPL3 rejection. The other-profile matrix passes 1/1. Firmware image
`/private/tmp/od-fw-s7-04-issue256.bin` has SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`.
Runner `cargo run --offline -q -p od-runner -- boot-firmware --image
/private/tmp/od-fw-s7-04-issue256.bin --com1-out
/private/tmp/od-fw-s7-04-issue447.com1 --budget 10000` retires 9,992
instructions and stops at `75 23` (JNZ rel8 +0x23), RIP `0x20139b`,
Unsupported opcode 117, COM1 empty. Full od-isa passes
(2,322/1,591/126/29/6/1,534/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline check passes with existing
warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.
ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-448 — exact JNZ rel8

Five focused tests cover both ZF values with defined/undefined ZF, signed target
calculation relative to next RIP, flags/definedness preservation, taken-only
canonicality, final-byte fetch-fault atomicity, encoding neighbors and CPL3
rejection. The other-profile matrix passes 1/1. Firmware image
`/private/tmp/od-fw-s7-04-issue256.bin` has SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`.
Runner `cargo run --offline -q -p od-runner -- boot-firmware --image
/private/tmp/od-fw-s7-04-issue256.bin --com1-out
/private/tmp/od-fw-s7-04-issue448.com1 --budget 10000` retires 9,995
instructions and stops at `4C 39 F0` (CMP RAX,R14), RIP `0x2013a4`, Unsupported
opcode 57, COM1 empty. Full od-isa passes
(2,322/1,591/126/29/6/1,539/7/15; zero doc tests), paged machine quantum
passes (110/110), workspace all-targets offline check passes with existing
warnings, and scoped rustfmt/diff-check pass. Independent review approved with no findings.
ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-449 — exact CMP RAX,R14

Four focused tests cover both admitted REX.R qword CMP forms. The checks
exercise operand routing, arithmetic flags and definedness, preserved GPRs,
final-ModRM fetch-fault atomicity, neighboring encodings and CPL3 refusal.
The other-profile isolation matrix passes 1/1.

The runner command
`cargo run --offline -q -p od-runner -- boot-firmware --image
/private/tmp/od-fw-s7-04-issue256.bin --com1-out
/private/tmp/od-fw-s7-04-issue449.com1 --budget 10000` uses image SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`.
It reports 9,996 retired instructions, RIP `0x2013a7`, Unsupported opcode 151
and empty COM1. Bytes `0F 97 C0` at the stop decode to SETA AL (ISSUE-450).

The full offline `od-isa` suite passes (2,322/1,591/126/29/6/1,539/7/15 and
zero doctests). The paged machine quantum suite passes 110/110. The workspace
all-targets offline check passes with existing warnings. Scoped rustfmt and
diff checks pass.

Independent review approved with no findings. ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-450 — exact SETA AL

Four focused tests cover the two unprefixed SETA register forms. The checks
exercise all CF/ZF combinations and definedness masks, a write confined to the
destination byte, final-ModRM fetch-fault atomicity, neighbors and CPL3.
The other-profile isolation matrix passes 1/1.

The runner command
`cargo run --offline -q -p od-runner -- boot-firmware --image
/private/tmp/od-fw-s7-04-issue256.bin --com1-out
/private/tmp/od-fw-s7-04-issue450.com1 --budget 10000` uses image SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`.
It reports 9,998 retired instructions, RIP `0x2013ad`, Unsupported opcode 133
and empty COM1. Bytes `0F 85 CA 00 00 00` select JNZ rel32 +0xca (ISSUE-451).

The full offline `od-isa` suite passes (2,322/1,591/126/29/6/1,539/7/15 and
zero doctests). The paged machine quantum suite passes 110/110. The workspace
all-targets offline check passes with existing warnings. Scoped rustfmt and
diff checks pass.

Independent review approved with no findings. ISSUE-201 remains the unrelated CLD caveat.

### ISSUE-451 — exact JNZ rel32 +0xca

Five focused tests exercise the +0x97 and +0xca JNZ rel32 forms. They cover
both ZF outcomes and definedness, signed target computation from next RIP,
taken-only canonicality, final-byte fetch-fault atomicity, neighboring
encodings and CPL3. The other-profile isolation matrix passes 1/1.

The runner command
`cargo run --offline -q -p od-runner -- boot-firmware --image
/private/tmp/od-fw-s7-04-issue256.bin --com1-out
/private/tmp/od-fw-s7-04-issue451.com1 --budget 10000` uses image SHA-256
`9517c0603eb6a4c25ea813b42cff5f45a4a612264b7ff14a4c0b4b1c3348febe`.
It reports 9,999 retired instructions, RIP `0x2013b3`, Unsupported opcode 235
and empty COM1. Bytes `EB 31` select JMP rel8 +0x31 (ISSUE-452).

The full offline `od-isa` suite passes (2,322/1,591/126/29/6/1,539/7/15 and
zero doctests). The paged machine quantum suite passes 110/110. The workspace
all-targets offline check passes with existing warnings. Scoped rustfmt and
diff checks pass.

Independent review approved with no findings. ISSUE-201 remains the unrelated CLD caveat.
