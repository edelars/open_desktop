# Контракт собственного vTPM 2.0

**Дата:** 7 ноября 2026 года. **Статус:** **фаза A реализована исполнителем (S7-21), ждёт ревью; коммита нет.** Остальное — не реализовано: CRB, таблица TPM2, TCG2, measured boot и event log (S7-22), фаза B и «Persistent state v1» (S7-23). Раздел «Состояние и границы» создан задачей S7-20 по [ADR-0009](../decisions/ADR-0009-vtpm-placement-and-crypto.md) (предложен, ждёт архитектурного ревью). Соответствие и сертификация TCG не заявляются.

Нормативные редакции: TPM 2.0 Library version 185, PC Client Platform TPM Profile 1.07 Errata 1.1 ([VIRTUAL_BOARD.md](VIRTUAL_BOARD.md), «Нормативные источники и редакции»). Что именно из них сверено — раздел «Источники и сверка» ниже.

## Состояние и границы

| Что | Владелец | Источник решения |
|---|---|---|
| Процессор команд TPM 2.0 и его состояние, кодек persistent state | `crates/od-tpm/` (фаза A — S7-21) | ADR-0009, решение 1 (D8) |
| CRB-устройство в окне `[0xFED4_0000, 0xFED4_5000)`, только locality 0 | `crates/od-machine/src/tpm_crb.rs` (S7-22) | ADR-0009, решение 4; [FIRMWARE_BOOT.md](FIRMWARE_BOOT.md), разделы 2 и 6 |
| Выдача/приём байтов TPM state host-стороне | `od-machine` public API (S7-22) | ADR-0009, решение 1 |
| Защита ключей, хранение в bundle, rollback | этап 9, S9-27 | решение владельца D1 |

- **Persistent** (формат v1 — S7-23): seeds иерархий, постоянная часть auth и флагов иерархий, NV-индексы, persistent objects, счётчики, состояние DA. Состояние DRBG не сохраняется: при создании и `restore` владелец передаёт свежую энтропию.
- **Volatile:** PCR, сессии, transient objects, загруженные контексты, hash/event sequences, locality, признак `Startup`.
- Байты persistent state версионные; неизвестная версия отвергается. Они содержат секреты в открытом виде и до S9-27 выдаются владельцу машины как есть.
- Seeds, приватные ключи, `authValue`, ключи сессий, состояние и выход DRBG, данные NV и `Unseal` никогда не логируются и не попадают в отчёты и эталоны.

Подробности, альтернативы и открытые вопросы — в [ADR-0009](../decisions/ADR-0009-vtpm-placement-and-crypto.md), решения 1, 4 и 5.

## API `od-tpm` (фаза A)

Crate не знает про шину, потоки, файлы, ОС-часы и ОС-энтропию; время и энтропию передаёт владелец.

| Вызов | Смысл |
|---|---|
| `Tpm::new(entropy: &[u8]) -> Result<Tpm, StateError>` | Новый TPM; не менее 48 октетов энтропии host (32 entropy input + 16 nonce, SP 800-90A для 256 бит) заводят HMAC_DRBG, тот генерирует три seed'а иерархий. Состояние — после `_TPM_Init`: первой должна прийти `TPM2_Startup`. Меньше 48 октетов — `StateError::InsufficientEntropy`. |
| `Tpm::execute(locality: u8, command: &[u8], now_ms: u64) -> Vec<u8>` | Одна команда → полный ответ. `locality` ≠ 0 → `TPM_RC_LOCALITY` (ADR-0009: функциональна только locality 0). `now_ms` — миллисекунды с `_TPM_Init`, не убывают (меньшее значение игнорируется); из них `TPM2_ReadClock` строит `Time` и `Clock`. Любая ошибка гостевой команды — ответ с `TPM_RC`; паники и `Err` на входных данных нет. |
| `Tpm::init()` | `_TPM_Init` того же объекта (сброс питания): volatile состояние теряется, persistent и состояние `Shutdown(STATE)` сохраняются, `Clock` копит время прошлого питания; дальше нужна `TPM2_Startup`. |
| `Tpm::persistent_snapshot() -> Vec<u8>` | Байты persistent state (формат ниже). Содержат seeds открытым текстом. |
| `Tpm::restore(snapshot, entropy) -> Result<Tpm, StateError>` | Восстановление; другая версия, повреждение или мало энтропии — ошибка, «пустой TPM молча» не создаётся. Энтропия пересевает DRBG. TPM в состоянии после `_TPM_Init`. |

`Debug` у `Tpm`, DRBG и пароля сессии не печатает состояние; seeds, состояние DRBG и authValue sequence-объектов (`AuthValue`) стираются при уничтожении (`zeroize`); при проверке пароля `handle_auth` возвращает клон `AuthValue` (буфер фиксированного размера, стирается при `Drop`), копии в обычный `Vec` нет; пароль из команды (`TPM2B_AUTH` в области авторизации) — срез входного буфера команды, им владеет вызывающий, `od-tpm` его не стирает.

## Команды фазы A

Ровно эти 17 команд есть в таблице `crates/od-tpm/src/dispatch.rs`; `TPM2_GetCapability(TPM_CAP_COMMANDS)` перечисляет её же (тест сверяет список с отдельно выписанным перечнем кодов и проверяет, что каждый код диапазона 0x11F–0x1A1 вне таблицы отвечает `TPM_RC_COMMAND_CODE`). Коды команд — Part 2 v184, Table 14 (сверено 2026-11-07).

| Команда | Код | Авторизация | Границы фазы A |
|---|---|---|---|
| `TPM2_Startup` | `0x144` | — | `CLEAR`, `STATE`; повторная — `TPM_RC_INITIALIZE`; `STATE` без `Shutdown(STATE)` — `TPM_RC_VALUE` (параметр 1: `0x1C4`); на каждом `Startup` выполняются known-answer тесты SHA-256 и HMAC-SHA-256 |
| `TPM2_Shutdown` | `0x145` | — | `CLEAR`, `STATE`; `STATE` сохраняет PCR с `TPM_PT_PCR_SAVE` (0–15) и `pcrUpdateCounter`; изменение PCR после `Shutdown(STATE)` обесценивает сохранённое (Part 3, 9.4.1) |
| `TPM2_SelfTest` | `0x143` | — | `fullTest` YES повторяет тесты, NO — нечего проверять (всё проверено на `Startup`); `TPM_RC_TESTING` не возвращается |
| `TPM2_GetTestResult` | `0x17C` | — | `outData` пуст, `testResult` `TPM_RC_SUCCESS` или `TPM_RC_FAILURE` (режим отказа) |
| `TPM2_GetCapability` | `0x17A` | — | `TPM_CAP_ALGS` (SHA-256), `TPM_CAP_HANDLES` (PCR 0–23, загруженные sequence-объекты; остальные определённые типы — пустой список), `TPM_CAP_COMMANDS`, `TPM_CAP_PP_COMMANDS` и `TPM_CAP_AUDIT_COMMANDS` (пустые), `TPM_CAP_PCRS`, `TPM_CAP_TPM_PROPERTIES`; прочие — `TPM_RC_VALUE` (параметр 1). Данные одного ответа не больше 1024 октетов, `moreData` по `propertyCount` |
| `TPM2_GetRandom` | `0x17B` | — | не более одного дайджеста (32 октета) за вызов, больший запрос — без ошибки, 32 октета (Part 3, 16.1) |
| `TPM2_StirRandom` | `0x146` | — | до 128 октетов (`TPM_RC_SIZE` выше); данные подмешиваются в DRBG через `Update`, как «additional information», энтропией не считаются |
| `TPM2_PCR_Read` | `0x17E` | — | банк SHA-256; в ответе не больше 8 значений (`TPML_DIGEST`), `pcrSelectionOut` показывает возвращённые PCR, остальные читаются следующим вызовом |
| `TPM2_PCR_Extend` | `0x182` | password, handle 1 | PCR 0–16 и 23 на locality 0; PCR 17–22 — `TPM_RC_LOCALITY`; `TPM_RH_NULL` — разбор без действия; не более одного дайджеста (SHA-256) |
| `TPM2_PCR_Event` | `0x13C` | password, handle 1 | `eventData` 0…1024 октетов; в ответе дайджест SHA-256 (и для `TPM_RH_NULL`); остальное как у `PCR_Extend` |
| `TPM2_PCR_Reset` | `0x13D` | password, handle 1 | только PCR 16 и 23; остальные — `TPM_RC_LOCALITY` (Part 3, 22.8); `TPM_RH_NULL` — `TPM_RC_VALUE` |
| `TPM2_Hash` | `0x17D` | — | только SHA-256, до 1024 октетов, `hierarchy` ∈ owner/platform/endorsement/null; `validation` — всегда NULL-ticket (см. ниже) |
| `TPM2_HashSequenceStart` | `0x186` | — | `hashAlg` SHA-256 (hash sequence) или `TPM_ALG_NULL` (event sequence); до 3 sequence-объектов одновременно, четвёртый — `TPM_RC_OBJECT_MEMORY`; `auth` до 32 октетов; handle `0x8000000n` |
| `TPM2_SequenceUpdate` | `0x15C` | password, handle 1 | работает на hash и event sequence, до 1024 октетов за вызов |
| `TPM2_SequenceComplete` | `0x13E` | password, handle 1 | только hash sequence (`TPM_RC_MODE` на event); sequence уничтожается при успехе (`{F}`); `validation` — NULL-ticket |
| `TPM2_EventSequenceComplete` | `0x185` | password, handles 1 и 2 | только event sequence (`TPM_RC_MODE` на hash); PCR расширяется дайджестом; sequence уничтожается только при успехе |
| `TPM2_ReadClock` | `0x181` | — | `TPMS_TIME_INFO` (25 октетов): `time`, `clock`, `resetCount`, `restartCount`, `safe` |

**Не реализовано и отвечает `TPM_RC_COMMAND_CODE`:**

- `TPM2_PCR_Allocate` (`0x12B`) — **отказ**. Команда требует platform authorization (иерархии — фаза B) и переносит выбор банков на следующий `_TPM_Init`; единственный банк SHA-256 из 24 PCR фиксирован. PTP 1.07 (Table 16) помечает её обязательной (`M`): отклонение от PTP записано ниже; решение — S7-30/S7-23. Для Windows TCG2 `SetActivePcrBanks` и так `EFI_UNSUPPORTED` (ADR-0009).
- `TPM2_PCR_SetAuthPolicy` (`0x12C`), `TPM2_PCR_SetAuthValue` (`0x183`) — не поддерживаются (ADR-0009, открытый вопрос 6). PTP 1.07 (clause 4.7, item 7) допускает их отсутствие, а реализованные обязаны вернуть `TPM_RC_VALUE`.
- Всё остальное (иерархии, ключи, сессии HMAC/policy, NV, контексты, `TPM2_HMAC`, `IncrementalSelfTest`, `TestParms`, `ClockSet` и т. д.) — фаза B (S7-23) или вне профиля. Ответ `TPM_RC_SUCCESS` без действия не возвращается ни для одной команды.

### Кадрирование и коды ответа

- Команда: `tag` (`0x8001` `TPM_ST_NO_SESSIONS` / `0x8002` `TPM_ST_SESSIONS`), `commandSize`, `commandCode`, handle area, для `0x8002` — `authorizationSize` и область авторизации, затем параметры. Ответ об ошибке — ровно 10 октетов, тег `0x8001`. Успешный ответ с сессиями содержит `parameterSize` и по одному подтверждению на сессию команды (пустые nonce и HMAC, `continueSession` = 1).
- Сессии: только password session `TPM_RS_PW` (`0x40000009`): пустой `nonceCaller`, допустим только бит `continueSession`, `hmac` — пароль. Пароль сравнивается с `authValue` без ранних выходов, с отбрасыванием хвостовых нулей у обоих значений (Part 1, 17.4). `authValue` PCR — пустой (PTP 1.07, 4.7, п. 5). Handle сессии HMAC/policy — предупреждение `TPM_RC_REFERENCE_Sn` (сессия не загружена). Число сессий в команде равно числу авторизуемых handle: меньше — `TPM_RC_AUTH_MISSING`, больше — `TPM_RC_AUTHSIZE`.
- Порядок проверок: locality → заголовок (`TPM_RC_COMMAND_SIZE`, `TPM_RC_TAG`) → код команды (`TPM_RC_COMMAND_CODE`) → режим отказа (`TPM_RC_FAILURE`) и `Startup` (`TPM_RC_INITIALIZE`) → handle area (`TPM_RC_INSUFFICIENT`) → допустимость handle → авторизация → параметры → действие. Размер параметров должен совпасть с командой (`TPM_RC_SIZE`).
- Коды `TPM_RC` с индексами параметра (`+0x40 + N<<8`), handle (`N<<8`) и сессии (`+0x800 + N<<8`) — Part 2, 6.6.2; значения из Table 20 (сверено): `INITIALIZE 0x100`, `FAILURE 0x101`, `AUTH_MISSING 0x125`, `COMMAND_SIZE 0x142`, `COMMAND_CODE 0x143`, `AUTHSIZE 0x144`, `ATTRIBUTES 0x82`, `HASH 0x83`, `VALUE 0x84`, `MODE 0x89`, `HANDLE 0x8B`, `SIZE 0x95`, `TAG 0x97`, `INSUFFICIENT 0x9A`, `BAD_AUTH 0xA2`, `OBJECT_MEMORY 0x902`, `LOCALITY 0x907`, `REFERENCE_Hn 0x910+n`, `REFERENCE_Sn 0x918+n`.
- Режим отказа (known-answer тест не прошёл, в production недостижим): все команды кроме `GetTestResult` и `GetCapability` отвечают `TPM_RC_FAILURE`; эти две — только с тегом `0x8001`. Выход — `_TPM_Init`.

## PCR

Один банк — **SHA-256**, **24 PCR (0–23)**, `TPM_PT_PCR_SELECT_MIN` = `PCR_SELECT_MAX` = 3 октета (бит PCR *n* — бит *n* mod 8 октета *n* / 8, Part 1, 15.5). SHA-1 не реализуется (PTP 1.07, Table 3: `N`, «Made Not Allowed as of PTP 1.07»); SHA-384 и SHA-512 **не реализуются в фазе A** (PTP 1.07 помечает их `M`, план S7-21 — «опционально»): отклонение записано ниже. Сброс по `Startup`, `PCR_Reset`, расширение:

- **Расширение:** `PCR.new = SHA-256(PCR.old ‖ digest)` (Part 3, 22.2.1). Проверено на векторе, вычисленном независимо (Python `hashlib`): для `digest = SHA-256("abc")` PCR 0 из нулей даёт `589f9ffe…faee8d`, повторное — `bdeb6c6d…58a926`, PCR 17 из `0xFF…` — `ded4cee9…063efb`. PCR расширяются ровно переданным дайджестом (контракт плана §4.3).
- **`pcrUpdateCounter`:** растёт на успешное расширение/сброс, кроме PCR с `TPM_PT_PCR_NO_INCREMENT` (16, 21, 22, 23); обнуляется на `Startup(CLEAR)` (в том числе Restart — **NOT VERIFIED**: Part 3, 9.3.1 называет обнуление только для Reset), при `Startup(STATE)` восстанавливается.
- **Таблица атрибутов** (PTP 1.07, Table 14; колонки по localities 4, 3, 2, 1, 0 — Y/N; код реализует только колонку locality 0, остальные записаны для S7-22/DRTM и **не реализованы**):

| PCR | Alias | SAVE | NO_INCREMENT | Reset командой L4,L3,L2,L1,L0 | Extend L4,L3,L2,L1,L0 | Значение после `Startup(CLEAR)` на L0 |
|---|---|---|---|---|---|---|
| 0–15 | Static RTM | 1 | 0 | N,N,N,N,N | Y,Y,Y,Y,Y | 0 (PCR 0 — индикатор locality, для L0 это 0) |
| 16 | Debug | 0 | 1 | N,Y,Y,Y,Y | Y,Y,Y,Y,Y | 0 |
| 17 | Locality 4 | 0 | 0 | N,N,N,N,N | Y,Y,Y,N,N | `0xFF…FF` |
| 18 | Locality 3 | 0 | 0 | N,N,N,N,N | Y,Y,Y,N,N | `0xFF…FF` |
| 19 | Locality 2 | 0 | 0 | N,N,N,N,N | N,Y,Y,N,N | `0xFF…FF` |
| 20 | Locality 1 | 0 | 0 | N,Y,Y,N,N | N,Y,Y,Y,N | `0xFF…FF` |
| 21, 22 | Dynamic OS | 0 | 1 | N,Y,Y,N,N | N,N,Y,N,N | `0xFF…FF` |
| 23 | Application | 0 | 1 | N,Y,Y,Y,Y | Y,Y,Y,Y,Y | 0 |

  (Колонка «Reset командой» для PCR 17–19 в PTP равна N на всех localities; для PCR 20–22 — Y на localities 3 и 2.) Следствия для фазы A на locality 0: расширяются PCR 0–16 и 23; сбрасываются командой только PCR 16 и 23 (`TPM_RC_LOCALITY` для остальных); PCR 17–22 нельзя ни расширить, ни сбросить — сброс PCR 20–22 возможен только с localities 2/3 (DRTM), которые не поддержаны (ADR-0009). PCR 17–22 после старта `0xFF…FF`, их единственное изменение — перезапуск.
- **Политика PCR:** пустая `authPolicy` и пустой `authValue` для всех PCR (PTP 1.07, 4.7, п. 5–6); групп PCR нет.
- **Чтение:** `TPM2_PCR_Read` возвращает не более 8 значений за ответ.

## Состояние

Фаза A держит только каркас persistent state (ADR-0009, решение 5, S7-23 дополнит); остальное volatile.

- **Persistent:** три seed'а иерархий (platform, owner/storage, endorsement; по 32 октета, генерируются DRBG от энтропии владельца, командами фазы A не используются), база `Clock` (мс), `resetCount`, `restartCount`, флаг «последнее завершение работы упорядоченное».
- **Volatile:** значения PCR и `pcrUpdateCounter`, sequence-объекты, признаки `started`/`failed`, состояние `Shutdown(STATE)`, DRBG, `Time`.
- **`Startup`/`Shutdown` (Part 3, 9.3.1):** *Reset* — `Startup(CLEAR)` после `Shutdown(CLEAR)` или без `Shutdown` (`resetCount` + 1, `restartCount` = 0); *Restart* — `Startup(CLEAR)` после `Shutdown(STATE)` (`restartCount` + 1, PCR в начальных значениях); *Resume* — `Startup(STATE)` после `Shutdown(STATE)` (`restartCount` + 1, PCR 0–15 восстановлены, остальные в начальных значениях). Все transient-объекты (здесь — sequence) сбрасываются на любом `Startup`. Сохранённое `Shutdown(STATE)` состояние живёт в объекте `Tpm` и **в snapshot не входит**; после перезапуска процесса `Startup(STATE)` вернёт `TPM_RC_VALUE`.
- **`safe`** в `TPMS_CLOCK_INFO`: `YES` только если предыдущее завершение работы было упорядоченным (`Shutdown`) или TPM новый.

### Формат snapshot `ODTPMSTA` версия 1

Все числа big-endian. Содержит seeds **открытым текстом**; шифрование и хранение — S9-27.

| Смещение | Размер | Поле |
|---|---|---|
| 0 | 8 | магия `ODTPMSTA` |
| 8 | 2 | версия формата (1) |
| 10 | 2 | флаги (0; иное отвергается) |
| 12 | 4 | длина payload (113) |
| 16 | 113 | payload: seed platform (32), seed owner (32), seed endorsement (32), `Clock` мс (u64), `resetCount` (u32), `restartCount` (u32), orderly (u8: 0/1) |
| 129 | 32 | SHA-256 payload (контроль повреждения, не аутентификация) |

`restore` отвергает: длину ≠ 161, чужую магию, версию ≠ 1 (`UnsupportedVersion`), флаги ≠ 0, длину payload ≠ 113, неверный дайджест, orderly вне {0,1}. До завершения «Persistent state v1» в S7-23 раскладка может расти; версия остаётся 1, поскольку снимков вне тестов не существует.

## Источники и сверка

Тексты открыты и прочитаны 2026-11-07 (прямые ссылки на PDF):

- TPM 2.0 Library Part 1, Part 2, Part 3 **v184, 2025/03/20**: `https://trustedcomputinggroup.org/wp-content/uploads/Trusted-Platform-Module-2.0-Library-Part-{1,2,3}-Version-184_pub.pdf`. Проверены: коды команд (Part 2, Table 14), `TPM_RC` (Table 20) и их кодирование (6.6.2), `TPM_ST`, `TPM_SU`, `TPM_CAP`, `TPM_PT`, `TPM_ALG_ID`, `TPM_RH`, `TPM_HT`, структуры Part 2 (Tables 38, 40, 45, 91–98, 105–106, 112–114, 120–128, 136, 139–140, 153–154), таблицы команд Part 3 (9.3, 9.4, 10.2, 10.4, 15.4, 16.1, 16.2, 17.4–17.7, 22.2–22.5, 22.8, 29.1, 30.2), Part 1 (9.2, 15.1, 15.5, 17.4, DRBG).
- PC Client Platform TPM Profile **1.07, 3/23/2026**: `https://medias.cdn.trustedcomputinggroup.org/production/PC_Client_Specific_Platform_TPM_Profile_for_TPM_2p0_v1p07_Pub_5643562943.pdf` (ссылка со страницы `https://trustedcomputinggroup.org/resource/pc-client-platform-tpm-profile-ptp-specification/`) и Errata 1.1: `https://medias.cdn.trustedcomputinggroup.org/production/TCG_Errata_v1_1_for_PTP_1_07_pub_f1aca71996.pdf`. Проверены: PCR (4.7.1, Tables 14–15), минимумы свойств (Table 2), алгоритмы (Table 3), обязательные команды (Table 16), требования к PCR (4.7). Регистры CRB в S7-21 не использовались и не сверялись (S7-22; тот же PTP открыт по этой ссылке).
- **Library v185 недоступна:** угаданные URL `…Part-2-Version-185_pub.pdf` и `…Part-3-Version-185_pub.pdf` вернули HTTP 403. Проект нацелен на v185 (ADR-0009), сверка выполнена по v184; PTP 1.07 ссылается на v185 (например, `TPM2_Sign` и `TPM2_VerifySignature` объявлены в нём deprecated в v185), поэтому каждое утверждение ниже сверено «по v184» и **NOT VERIFIED against TPM 2.0 Library v185** в части возможных изменений v185. `TPM_PT_REVISION` отдаёт 185 (в v184 «номер версии равен `TPM_SPEC_VERSION`»), год и день года — значения v184 (2025, 79).

### Не сверено с текстом (NOT VERIFIED)

Каждый пункт помечен в исходниках комментарием `NOT VERIFIED`:

1. Все числа выше — по v184, не по v185 (см. выше).
2. Код ответа `TPM_RC_LOCALITY` на расширение PCR, недопустимое для locality (Part 3, 22.2 не перечисляет его для `PCR_Extend`; названо только для `PCR_Reset`).
3. Коды для password session с непустым nonce (`TPM_RC_BAD_AUTH` + сессия), с лишними битами атрибутов (`TPM_RC_ATTRIBUTES` + сессия), для некорректной области авторизации (`TPM_RC_AUTHSIZE`) и для handle не загруженной сессии (`TPM_RC_REFERENCE_Sn`).
4. Отсутствие защиты от подбора (DA): любой неверный пароль — `TPM_RC_BAD_AUTH` (ответ «без последствий для DA»); освобождены ли от DA sequence-объекты — не проверено.
5. Порядок проверки `TPM_RC_INITIALIZE` относительно `TPM_RC_COMMAND_CODE` (неизвестная команда до `Startup` отвечает `COMMAND_CODE`).
6. Ответ `TPM_CAP_TPM_PROPERTIES` для `property` вне групп 1 и 2 (пустой список); `TPM_CAP_HANDLES` для постоянных handle иерархий (пустой список: иерархий нет до фазы B); `TPM_PT_MANUFACTURER` = `ODTP` (не зарегистрирован в TCG), версия firmware 1.0.
7. `TPM_RC_MODE` с номером handle для sequence не того вида.
8. Обнуление `pcrUpdateCounter` на Restart; пустой `outData` у `GetTestResult`; отсутствие `TPM_RC_TESTING`.
9. Построение HMAC_DRBG по SP 800-90A: текст SP 800-90A не читался. Реализация сверена с независимой реализацией на Python (`hmac`, `hashlib`), не с векторами NIST CAVP; Part 1 (DRBG Protected Capability) рекомендует SP 800-90A для не-FIPS TPM, HMAC_DRBG — такой механизм (закрывает часть открытого вопроса 1 ADR-0009; FIPS не заявляется).
10. `validation` в `TPM2_Hash`/`SequenceComplete` — всегда NULL-ticket (`TPM_ST_HASHCHECK`, `TPM_RH_NULL`, пустой digest). Настоящий ticket — HMAC над `proof` иерархии (фаза B); для ключей restricted-signing в фазе A такой дайджест не подписать.
11. `TPM_ST_SESSIONS` с `authorizationSize = 0` (и вообще `authorizationSize` < 9): команда без авторизуемых handle (например, `GetRandom`) обслуживается; **NOT VERIFIED against TPM 2.0 Library v185/v184: TPM_RC_AUTHSIZE при authorizationSize < 9 для SESSIONS.** Поведение сохранено; комментарии в `command.rs` (`parse_auth_area`) и `dispatch.rs`.

### Отклонения от PTP 1.07 и плана (записаны, не скрыты)

- SHA-384 (PTP: `M`, банк PCR) и SHA-512 (`M`) не реализованы; план S7-21 считает SHA-384 опциональным, SHA-1 для PCR запрещён PTP 1.07 (Table 3, `N`), что снимает вопрос банка SHA-1 «по S7-30». Нужен ли банк SHA-384 для Windows и для TCG2 event log — решение S7-22/S7-30.
- `TPM2_PCR_Allocate` (PTP: `M`) не поддержана (отказ `TPM_RC_COMMAND_CODE`).
- `TPM_CAP_PCR_PROPERTIES` не реализована (PTP 4.7.1, п. 4: TPM должен возвращать `TPM_PT_PCR_RESET_Lx`): `GetCapability` отвечает `TPM_RC_VALUE` с параметром 1 (`capability.rs`). Назначено S7-22: она обязана реализовать `TPM_CAP_PCR_PROPERTIES` или оставить этот ответ, записав в отчёт S7-22.
- `TPM_CAP_TPM_PROPERTIES` перечисляет только свойства реализованных возможностей (PTP 4.2, п. 2 требует все определённые); свойства сессий, NV, контекстов, DA, audit, persistent-объектов и атрибутов иерархий появятся с фазой B. `TPM_PT_HR_TRANSIENT_MIN` = 3 — это число sequence-слотов.
- `TPM_PT_MAX_COMMAND_SIZE` = `TPM_PT_MAX_RESPONSE_SIZE` = 3968 (предел буфера CRB, ADR-0009, решение 4), `TPM_PT_INPUT_BUFFER` = 1024, `TPM_PT_MAX_DIGEST` = 32, `TPM_PT_MAX_CAP_BUFFER` = 1024.
- Минимальные значения PTP Table 2 для сессий, persistent-объектов, NV (`TPM_PT_HR_*_MIN`, `TPM_PT_NV_*`) в фазе A не заявляются: этих возможностей нет.
- Криптозависимости: `hmac` (ADR-0009: `=0.13.0`) нет в офлайн-кеше и он нужен S7-23; HMAC-SHA-256 для DRBG написан внутри `random.rs` на `sha2` и проверен векторами RFC 4231 (случаи 1 и 2); S7-23 заменяет его крейтом `hmac =0.13.0` (DRY). Отклонение: HMAC-DRBG на sha2; S7-23 заменяет на hmac =0.13.0 (обязательство). `zeroize =1.9.1` (транзитивный по ADR) подключён напрямую для стирания секретов.

Тесты: `cargo test -p od-tpm --offline` (88 тестов: маршалинг round-trip каждой структуры, каждая команда — успех и ошибки, векторы SHA-256/PCR/DRBG, перечень `TPM_CAP_COMMANDS`, `Startup` до команд, snapshot/restore, прогон искажённых команд без паники).
