# Контракт собственного vTPM 2.0

**Дата:** 7 ноября 2026 года. **Статус:** **фаза A реализована исполнителем (S7-21), срез A фазы B — исполнителем (S7-23: иерархии, HMAC-сессии, NV-база); оба ждут ревью, коммита среза A нет.** Остальное — не реализовано: CRB, таблица TPM2, TCG2, measured boot и event log (S7-22); ключи RSA/ECC и policy (срез B S7-23), «Persistent state v1», DA, EK, контексты (срез C S7-23). Раздел «Состояние и границы» создан задачей S7-20 по [ADR-0009](../decisions/ADR-0009-vtpm-placement-and-crypto.md) (предложен, ждёт архитектурного ревью). Соответствие и сертификация TCG не заявляются.

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

Эти 17 команд и 13 команд среза A фазы B (раздел «Фаза B, срез A» ниже) — всего 30 — есть в таблице `crates/od-tpm/src/dispatch.rs`; `TPM2_GetCapability(TPM_CAP_COMMANDS)` перечисляет её же (тест сверяет список с отдельно выписанным перечнем кодов и проверяет, что каждый код диапазона 0x11F–0x1A1 вне таблицы отвечает `TPM_RC_COMMAND_CODE`). Коды команд — Part 2 v184, Table 14 (сверено 2026-11-07).

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
- Всё остальное (ключи и объекты, policy-сессии, шифрование параметров, NV counters/Extend/SetBits, контексты, DA, `TPM2_HMAC`, `IncrementalSelfTest`, `TestParms`, `ClockSet` и т. д.) — срезы B/C фазы B (S7-23) или вне профиля. Ответ `TPM_RC_SUCCESS` без действия не возвращается ни для одной команды.

### Кадрирование и коды ответа

- Команда: `tag` (`0x8001` `TPM_ST_NO_SESSIONS` / `0x8002` `TPM_ST_SESSIONS`), `commandSize`, `commandCode`, handle area, для `0x8002` — `authorizationSize` и область авторизации, затем параметры. Ответ об ошибке — ровно 10 октетов, тег `0x8001`. Успешный ответ с сессиями содержит `parameterSize` и по одному подтверждению на сессию команды (пустые nonce и HMAC, `continueSession` = 1).
- Сессии: password session `TPM_RS_PW` (`0x40000009`): пустой `nonceCaller`, допустим только бит `continueSession`, `hmac` — пароль. Пароль сравнивается с `authValue` без ранних выходов, с отбрасыванием хвостовых нулей у обоих значений (Part 1, 17.4). `authValue` PCR — пустой (PTP 1.07, 4.7, п. 5). С среза A фазы B — ещё HMAC-сессии (раздел «Фаза B, срез A»). Handle policy-сессии и не загруженной HMAC-сессии — предупреждение `TPM_RC_REFERENCE_Sn`. Число сессий в команде равно числу авторизуемых handle: меньше — `TPM_RC_AUTH_MISSING`, больше — `TPM_RC_AUTHSIZE`.
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

## Фаза B, срез A: иерархии, HMAC-сессии, NV-база (S7-23)

Реализовано исполнителем, ждёт ревью; коммита нет. Не начато (срезы B и C S7-23): ключи RSA/ECC и все команды с объектами (`CreatePrimary`, `Create`, `Load`, `ReadPublic`, `Sign`, `RSA_*`, `ECDH_ZGen`, `EvictControl`, `TPM2_HMAC`), policy-сессии и `Policy*`, шифрование параметров (AES-CFB, XOR), salted-сессии, `ContextSave/Load`, `NV_Increment/Extend/SetBits` и счётчики, DA (`DictionaryAttack*`), `SetPrimaryPolicy`, `ChangePPS/EPS`, EK-шаблоны, `Quote/Certify/GetTime`, persistent state v1 (hierarchy auth и NV пока **не входят в snapshot**).

| Команда | Код | Авторизация | Границы среза A |
|---|---|---|---|
| `TPM2_HierarchyControl` | `0x121` | handle 1: owner, platform, endorsement | `enable` ∈ owner/platform/endorsement/`TPM_RH_PLATFORM_NV`. Platform: CLEAR `phEnable` (SET нельзя), SET/CLEAR `phEnableNV`, `shEnable`, `ehEnable`; owner — только CLEAR `shEnable`, endorsement — только CLEAR `ehEnable`; прочее — `TPM_RC_AUTH_TYPE`. `{NV E}` |
| `TPM2_HierarchyChangeAuth` | `0x129` | handle 1: owner, platform, endorsement, lockout | новое значение до 32 октетов (`TPM_RC_SIZE`, параметр 1), хранится без хвостовых нулей; HMAC ответа — под новым значением; `TPM_RH_NULL` — `TPM_RC_VALUE` (handle 1). `{NV}` |
| `TPM2_Clear` | `0x126` | handle 1: lockout, platform | после `ClearControl` — `TPM_RC_DISABLED`; новый storage seed (из DRBG); `shEnable`, `ehEnable` SET; owner/endorsement/lockout auth пустые; NV-индексы без `PLATFORMCREATE` удалены; `Clock` с нуля, `resetCount` и `restartCount` 0, `safe` YES, `pcrUpdateCounter` + 1. Не применимо в срезе A: flush объектов, новые `shProof`/`ehProof` (доказательств нет), пустые policy. `{NV E}` |
| `TPM2_ClearControl` | `0x127` | handle 1: lockout, platform | platform SET/CLEAR `disableClear`; lockout только SET (CLEAR — `TPM_RC_AUTH_FAIL`). `{NV}` |
| `TPM2_StartAuthSession` | `0x176` | — (handles: `tpmKey`, `bind`) | см. «HMAC-сессии» ниже; `rHandle` |
| `TPM2_FlushContext` | `0x165` | сессии запрещены (`TPM_RC_AUTH_CONTEXT`) | `flushHandle` — параметр: загруженная сессия (тип `0x02` или `0x03`, старший октет игнорируется) или sequence-объект; прочие типы — `TPM_RC_VALUE`, не загружено — `TPM_RC_HANDLE` (параметр 1) |
| `TPM2_NV_DefineSpace` | `0x12A` | handle 1: owner, platform | см. «NV-база». `{NV}` |
| `TPM2_NV_UndefineSpace` | `0x122` | handle 1: owner, platform; handle 2 — индекс | platform удаляет любой индекс, owner — только без `PLATFORMCREATE` (`TPM_RC_NV_AUTHORIZATION`). `{NV}` |
| `TPM2_NV_ReadPublic` | `0x169` | — | `TPM2B_NV_PUBLIC` и `TPM2B_NAME` |
| `TPM2_NV_Write` | `0x137` | handle 1: platform, owner, сам индекс; handle 2 — индекс | `data` ≤ 512 октетов, `offset`; `{NV}` |
| `TPM2_NV_Read` | `0x14E` | как у `NV_Write` | `size` ≤ 512, `offset` |
| `TPM2_NV_WriteLock` | `0x138` | как у `NV_Write` | не в списке плана; нужен для `WRITEDEFINE`/`WRITE_STCLEAR`. `{NV}` |
| `TPM2_NV_ReadLock` | `0x14F` | как у `NV_Read` | не в списке плана; нужен для `READ_STCLEAR`. `{NV}` |

**Иерархии.** Platform, Owner (Storage), Endorsement, Null и Lockout-авторизация (Part 1 v184, 11.2–11.5, 12.4). Persistent: `ownerAuth`, `endorsementAuth`, `lockoutAuth`, `disableClear`, флаги `TPMA_PERMANENT.*AuthSet`, seeds platform/owner/endorsement (в snapshot — только seeds). Volatile: `platformAuth`, `phEnable`, `phEnableNV`, `shEnable`, `ehEnable`, Null seed. Правила `Startup` (Part 3, 9.3.1): на любом `Startup` `phEnable` SET, сессии и sequence-объекты сброшены; Reset — `platformAuth` пуст, новый Null seed, `phEnableNV`/`shEnable`/`ehEnable` SET, блокировки NV сняты; Restart — то же, Null seed сохранён; Resume — всё сохранённое `Shutdown(STATE)` восстановлено (`phEnable` SET). Отключённая иерархия (`enable` CLEAR) не может авторизовать: `TPM_RC_HIERARCHY` (с номером handle). Значение `TPMA_STARTUP_CLEAR` и `TPMA_PERMANENT` читаются `GetCapability(TPM_PT_STARTUP_CLEAR/TPM_PT_PERMANENT)`. Нет: `authPolicy`/`SetPrimaryPolicy`, proof-значения, DA — ошибка авторизации всегда `TPM_RC_BAD_AUTH`, блокировки нет.

**HMAC-сессии.** `TPM2_StartAuthSession`: `sessionType` только `TPM_SE_HMAC`; `tpmKey` только `TPM_RH_NULL` (salted не поддержаны, `encryptedSalt` пуст); `bind` — `TPM_RH_NULL` (unbound+unsalted, ключ сессии пуст) или авторизация иерархии (owner, platform, endorsement, lockout): `sessionKey = KDFa(SHA-256, authValue_bind, "ATH", nonceTPM, nonceCaller, 256)`; `authHash` только SHA-256; `symmetric` только `TPM_ALG_NULL` (атрибуты `decrypt`/`encrypt` — `TPM_RC_SYMMETRIC`, `audit*` — `TPM_RC_ATTRIBUTES`). `nonceCaller` 16…32 октета (`TPM_RC_SIZE`), размер задаёт размер `nonceTPM`; позже `nonceCaller` может быть пустым или до этого размера (`TPM_RC_NONCE` выше). Слотов 64 (`TPM_PT_HR_LOADED_MIN` = `TPM_PT_ACTIVE_SESSIONS_MAX` = 64), handle `0x02000000 + n`, наименьший свободный; нет слота — `TPM_RC_SESSION_HANDLES`. Авторизация (Part 1, 17.6.5, формулы 17, 21, 22): `authHMAC = HMAC-SHA-256(sessionKey ‖ authValue, cpHash ‖ nonceCaller ‖ nonceTPM_прошлый ‖ атрибуты)`; для сессии, привязанной к тому же объекту (тот же handle и тот же `authValue`, что записаны при старте), ключ — только `sessionKey`; пустой ключ допускает пустой HMAC (17.6.15) и ответ тоже с пустым HMAC. cpHash = `H(commandCode ‖ Name₁ ‖ … ‖ параметры)` (Name: handle для PCR и постоянных, `nameAlg ‖ H(public)` для NV, пустой для sequence), rpHash = `H(responseCode ‖ commandCode ‖ параметры ответа)`; ответ: `nonceTPM` новый, HMAC над `rpHash ‖ nonceTPM_новый ‖ nonceCaller ‖ атрибуты` с `authValue` **после** команды (для `HierarchyChangeAuth` и `Clear` по lockout — новое значение; для сущности, которой уже нет, — пустой). Сессия меняет `nonceTPM` только при успехе; `continueSession` CLEAR закрывает сессию после успеха. Один handle сессии — один раз в команде (иначе `TPM_RC_VALUE`, сессия 2…). Любой `Startup` и `_TPM_Init` закрывают все сессии. HMAC-сессии работают для всех авторизуемых команд (фаза A и срез A).

**NV-база.** Только `TPM_NT_ORDINARY`. `DefineSpace` (`TPMS_NV_PUBLIC`: handle `0x01xxxxxx`, `nameAlg` SHA-256, атрибуты, `authPolicy` пуст или 32 октета, `dataSize`): `TPM_RC_NV_DEFINED`; `TPM_RC_ATTRIBUTES` — `WRITTEN`/`READLOCKED`/`WRITELOCKED` заданы, нет ни одного бита чтения или записи, `PLATFORMCREATE` не соответствует авторизации (platform — SET, owner — CLEAR), и то, чего срез A не умеет: `TPM_NT` counter/bits/extend/pin (Part 3, 31.3.1 прямо предписывает `TPM_RC_ATTRIBUTES`) и reserved, `POLICYREAD`, `POLICYWRITE`, `POLICY_DELETE`, `GLOBALLOCK`, `ORDERLY`, `CLEAR_STCLEAR`; `TPM_RC_SIZE` — `dataSize` > 8500 (`TPM_PT_NV_INDEX_MAX`), `WRITEALL` при `dataSize` > 512 (`TPM_PT_NV_BUFFER_MAX`, Part 3, 31.3.1), `auth` > 32; `TPM_RC_NV_SPACE` — больше 128 индексов или 16384 октетов данных в сумме (минимум PTP 1.07: 68 индексов и 11026 октетов); platform при `phEnableNV` CLEAR — `TPM_RC_HIERARCHY`. Доступ: platform — `PPREAD`/`PPWRITE`, owner — `OWNERREAD`/`OWNERWRITE`, сам индекс (handle 1 = handle 2, иначе `TPM_RC_NV_AUTHORIZATION`) — `AUTHREAD`/`AUTHWRITE` с `authValue` индекса; роль не разрешена атрибутами — `TPM_RC_AUTH_TYPE` (до проверки пароля). Индекс, созданный с `PLATFORMCREATE`, доступен только при `phEnableNV` SET, без него — только при `shEnable` SET (иначе `TPM_RC_HANDLE`, Part 2, Table 43). Чтение не записанного индекса — `TPM_RC_NV_UNINITIALIZED` (до проверки `authValue`, Part 3, 31.1; даже для `size` 0). `Write`: после авторизации `WRITELOCKED` — `TPM_RC_NV_LOCKED`, `WRITEALL` при размере ≠ `dataSize` и `offset + size` > `dataSize` — `TPM_RC_NV_RANGE`; успех ставит `WRITTEN`. `Read`: `READLOCKED` — `TPM_RC_NV_LOCKED`, `offset + size` > `dataSize` — `TPM_RC_NV_RANGE`. `WriteLock` (нужен `WRITEDEFINE` или `WRITE_STCLEAR`, иначе `TPM_RC_ATTRIBUTES`) и `ReadLock` (нужен `READ_STCLEAR`) ставят блокировки; `Startup(CLEAR)` снимает `READLOCKED` и `WRITELOCKED` для индексов с `WRITEDEFINE` CLEAR или `WRITTEN` CLEAR (Resume ничего не снимает). Name индекса — `nameAlg ‖ H(TPMS_NV_PUBLIC)` и меняется с `WRITTEN`/блокировками (Part 1, 14). NV-данные переживают `_TPM_Init`, но **в snapshot не входят** (срез C): `restore` возвращает TPM без индексов и с пустыми auth.

**GetCapability среза A.** `TPM_CAP_HANDLES`: PCR, определённые NV-индексы, загруженные HMAC-сессии (тип `0x02`), sequence-объекты, постоянные handle (`TPM_RH_OWNER`, `NULL`, `LOCKOUT`, `ENDORSEMENT`, `PLATFORM`); сохранённые сессии (`0x03`) и постоянные объекты — пусто. `TPM_CAP_TPM_PROPERTIES` добавляет `TPM_PT_HR_LOADED_MIN`, `ACTIVE_SESSIONS_MAX`, `NV_INDEX_MAX`, `NV_BUFFER_MAX`, `PERMANENT`, `STARTUP_CLEAR`, `HR_NV_INDEX`, `HR_LOADED`, `HR_LOADED_AVAIL`, `HR_ACTIVE`, `HR_ACTIVE_AVAIL`; `TPM_PT_TOTAL_COMMANDS` = 30; `TPMA_CC` помечает `{E}` (бит `extensive`) для `HierarchyControl` и `Clear`. Остальные свойства по-прежнему не перечисляются (см. «Отклонения»).

## Состояние

Фаза A держит только каркас persistent state (ADR-0009, решение 5, S7-23 дополнит); остальное volatile. В срезе A S7-23 в памяти добавлены auth иерархий, флаги и NV-индексы (persistent) и сессии, `platformAuth`, enables, Null seed (volatile), но формат snapshot не менялся.

- **Persistent:** три seed'а иерархий (platform, owner/storage, endorsement; по 32 октета, генерируются DRBG от энтропии владельца, командами фазы A не используются), база `Clock` (мс), `resetCount`, `restartCount`, флаг «последнее завершение работы упорядоченное».
- **Volatile:** значения PCR и `pcrUpdateCounter`, sequence-объекты, HMAC-сессии, `platformAuth`, enables иерархий, Null seed, признаки `started`/`failed`, состояние `Shutdown(STATE)` (PCR с `TPM_PT_PCR_SAVE` и volatile-блок иерархий), DRBG, `Time`.
- **Persistent в памяти, но не в snapshot (срез A):** `ownerAuth`, `endorsementAuth`, `lockoutAuth`, `disableClear`, флаги `AuthSet`, NV-индексы. Пока их нет в формате, `restore` даёт пустые auth и пустой NV (тест `snapshot_does_not_carry_hierarchy_auth_or_nv_yet`); формат `ODTPMSTA` v2 или расширение v1 — срез C.
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

- TPM 2.0 Library Part 1, Part 2, Part 3 **v184, 2025/03/20**: `https://trustedcomputinggroup.org/wp-content/uploads/Trusted-Platform-Module-2.0-Library-Part-{1,2,3}-Version-184_pub.pdf`. Проверены: коды команд (Part 2, Table 14), `TPM_RC` (Table 20) и их кодирование (6.6.2), `TPM_ST`, `TPM_SU`, `TPM_CAP`, `TPM_PT`, `TPM_ALG_ID`, `TPM_RH`, `TPM_HT`, структуры Part 2 (Tables 38, 40, 45, 91–98, 105–106, 112–114, 120–128, 136, 139–140, 153–154), таблицы команд Part 3 (9.3, 9.4, 10.2, 10.4, 15.4, 16.1, 16.2, 17.4–17.7, 22.2–22.5, 22.8, 29.1, 30.2), Part 1 (9.2, 15.1, 15.5, 17.4, DRBG). Срез A S7-23 (прочитано 2026-11-07 по тем же PDF v184): Part 1 — 9.4.10.2 (KDFa), 11.2–11.5 (иерархии, enables), 12.4.5 (Null seed), 14 и Table 6 (Names), 16.6.3–16.6.4 и Table 12 (атрибуты сессии), 16.7–16.8 (cpHash, rpHash), 17.6.3–17.6.17 (nonce, HMAC, sessionKey, bound-сессия, пустой HMAC, завершение), Part 2 — Table 20 (коды NV/сессии/иерархии), Table 25 (`TPM_SE`), Table 30 (`TPM_PT` var-группа), Tables 36, 40, 42, 43, 45, 58, 61–70, 159, 227–235; Part 3 — 9.3.1 (эффекты `Startup`), 11.1, 24.2, 24.6–24.8, 28.4, 30.2, 31.1–31.14.
- PC Client Platform TPM Profile **1.07, 3/23/2026**: `https://medias.cdn.trustedcomputinggroup.org/production/PC_Client_Specific_Platform_TPM_Profile_for_TPM_2p0_v1p07_Pub_5643562943.pdf` (ссылка со страницы `https://trustedcomputinggroup.org/resource/pc-client-platform-tpm-profile-ptp-specification/`) и Errata 1.1: `https://medias.cdn.trustedcomputinggroup.org/production/TCG_Errata_v1_1_for_PTP_1_07_pub_f1aca71996.pdf`. Проверены: PCR (4.7.1, Tables 14–15), минимумы свойств (Table 2), алгоритмы (Table 3), обязательные команды (Table 16), требования к PCR (4.7). Срез A S7-23: минимумы сессий и NV (Table 2: `TPM_PT_HR_LOADED_MIN` 3, `ACTIVE_SESSIONS_MAX` 64, `NV_INDEX_MAX` 8500, `NV_BUFFER_MAX` 512), поддержка атрибутов NV (Table 12), объём NV (4.6.2: 68 индексов, 11026 октетов). Регистры CRB в S7-21 не использовались и не сверялись (S7-22; тот же PTP открыт по этой ссылке).
- **Library v185 недоступна:** угаданные URL `…Part-2-Version-185_pub.pdf` и `…Part-3-Version-185_pub.pdf` вернули HTTP 403. Проект нацелен на v185 (ADR-0009), сверка выполнена по v184; PTP 1.07 ссылается на v185 (например, `TPM2_Sign` и `TPM2_VerifySignature` объявлены в нём deprecated в v185), поэтому каждое утверждение ниже сверено «по v184» и **NOT VERIFIED against TPM 2.0 Library v185** в части возможных изменений v185. `TPM_PT_REVISION` отдаёт 185 (в v184 «номер версии равен `TPM_SPEC_VERSION`»), год и день года — значения v184 (2025, 79).

### Не сверено с текстом (NOT VERIFIED)

Каждый пункт помечен в исходниках комментарием `NOT VERIFIED`:

1. Все числа выше — по v184, не по v185 (см. выше).
2. Код ответа `TPM_RC_LOCALITY` на расширение PCR, недопустимое для locality (Part 3, 22.2 не перечисляет его для `PCR_Extend`; названо только для `PCR_Reset`).
3. Коды для password session с непустым nonce (`TPM_RC_BAD_AUTH` + сессия), с лишними битами атрибутов (`TPM_RC_ATTRIBUTES` + сессия), для некорректной области авторизации (`TPM_RC_AUTHSIZE`) и для handle не загруженной сессии (`TPM_RC_REFERENCE_Sn`).
4. Отсутствие защиты от подбора (DA): любой неверный пароль — `TPM_RC_BAD_AUTH` (ответ «без последствий для DA»); освобождены ли от DA sequence-объекты — не проверено.
5. Порядок проверки `TPM_RC_INITIALIZE` относительно `TPM_RC_COMMAND_CODE` (неизвестная команда до `Startup` отвечает `COMMAND_CODE`).
6. Ответ `TPM_CAP_TPM_PROPERTIES` для `property` вне групп 1 и 2 (пустой список); `TPM_CAP_HANDLES` для постоянных handle иерархий (в фазе A — пустой список, в срезе A S7-23 — список, см. п. 19); `TPM_PT_MANUFACTURER` = `ODTP` (не зарегистрирован в TCG), версия firmware 1.0.
7. `TPM_RC_MODE` с номером handle для sequence не того вида.
8. Обнуление `pcrUpdateCounter` на Restart; пустой `outData` у `GetTestResult`; отсутствие `TPM_RC_TESTING`.
9. Построение HMAC_DRBG по SP 800-90A: текст SP 800-90A не читался. Реализация сверена с независимой реализацией на Python (`hmac`, `hashlib`), не с векторами NIST CAVP; Part 1 (DRBG Protected Capability) рекомендует SP 800-90A для не-FIPS TPM, HMAC_DRBG — такой механизм (закрывает часть открытого вопроса 1 ADR-0009; FIPS не заявляется).
10. `validation` в `TPM2_Hash`/`SequenceComplete` — всегда NULL-ticket (`TPM_ST_HASHCHECK`, `TPM_RH_NULL`, пустой digest). Настоящий ticket — HMAC над `proof` иерархии (фаза B); для ключей restricted-signing в фазе A такой дайджест не подписать.
11. `TPM_ST_SESSIONS` с `authorizationSize = 0` (и вообще `authorizationSize` < 9): команда без авторизуемых handle (например, `GetRandom`) обслуживается; **NOT VERIFIED against TPM 2.0 Library v185/v184: TPM_RC_AUTHSIZE при authorizationSize < 9 для SESSIONS.** Поведение сохранено; комментарии в `command.rs` (`parse_auth_area`) и `dispatch.rs`.

Срез A фазы B (S7-23), пометки `NOT VERIFIED` в `hierarchy.rs`, `session.rs`, `nv.rs`, `dispatch.rs`, `capability.rs`, `startup.rs`:

12. Коды, которых открытый текст v184 не называет: `TPM_RC_AUTH_TYPE` в `HierarchyControl` (роль не вправе менять этот enable, SET `phEnable`) и в NV-командах (роль не разрешена атрибутами индекса); `TPM_RC_AUTH_FAIL` (handle 1), когда lockout пытается CLEAR `disableClear`; `TPM_RC_VALUE` с номером handle для handle не того типа (по примечанию `#TPM_RC_VALUE` Part 2, Tables 61–70, это ответ на ошибку типа; индекс — по 6.6.2); `TPM_RC_HIERARCHY` (с номером handle) при авторизации отключённой иерархией (Part 3 называет код для `SetPrimaryPolicy`, Part 1, 11.2 — общее правило); `TPM_RC_HANDLE` с номером handle/параметра для неопределённого или недоступного NV-индекса и для `flushHandle`.
13. `TPM2_StartAuthSession`: `TPM_RC_VALUE` (параметр 3) для policy/trial; `TPM_RC_VALUE` (параметр 2) для непустого `encryptedSalt` при `tpmKey` NULL; `TPM_RC_VALUE` (handle 2) для `bind`, который не NULL и не авторизация иерархии (NV, PCR, объекты); `TPM_RC_SYMMETRIC` (параметр 4) для любого `symmetric` кроме NULL; `TPM_RC_HASH` (параметр 5) для любого `authHash` кроме SHA-256.
14. HMAC-сессия в команде: `TPM_RC_NONCE` для `nonceCaller` больше `nonceTPM` сессии; `TPM_RC_ATTRIBUTES` для reserved-битов и `audit*`; `TPM_RC_SYMMETRIC` для `decrypt`/`encrypt` (у сессии нет симметричного алгоритма); `TPM_RC_VALUE` (сессия n) при повторе handle сессии; ошибка HMAC и пароля — `TPM_RC_BAD_AUTH` (не `TPM_RC_AUTH_FAIL`: DA нет, Part 1, 17.6.8 говорит, что для сущности вне DA код заменяется на `BAD_AUTH`).
15. Сравнение привязки (bound): handle и `authValue`, записанные при старте, сравниваются с текущими (текст говорит «Name и authValue», для NV и persistent — authValue); `authValue` в `KDFa` берётся без хвостовых нулей (17.6.5 говорит так об авторизационных вычислениях); handle сессии `0x02nnnnnn` и `0x03nnnnnn` при flush называют один слот (28.4: «старший октет игнорируется»; пример текста содержит опечатку).
16. Name sequence-объекта в cpHash — пустой (Part 1, Table 6, примечание 1); HMAC ответа для сущности, которой команда уже нет (sequence после `SequenceComplete`), — с пустым `authValue` (текст говорит так только про `NV_UndefineSpaceSpecial`).
17. NV: `TPM_RC_RESERVED_BITS` для reserved-битов `TPMA_NV` в `DefineSpace`; `TPM_RC_ATTRIBUTES` для типов и атрибутов, которых срез A не поддерживает (см. «NV-база»); `TPM_RC_SIZE` для `size` `TPM2B_NV_PUBLIC`, не равного разобранной структуре, и для `authPolicy`, не пустого и не 32 октета; `TPM_RC_VALUE` (параметр 1) для `size` чтения больше 512; `TPM_RC_NV_RANGE` для `offset` за пределами; `TPM_RC_NV_LOCKED` (названо в 31.7.1 и 31.13.1, номер handle не нужен). Лимиты 128 индексов, 16384 октетов в сумме, 8500 на индекс, 512 на буфер — выбор реализации (минимумы PTP 1.07 выполнены), не значения из текста.
18. `Startup`: Null seed при Restart сохраняется (текст перечисляет смену Null seed только для Reset; для Restart — выведено из умолчания); `platformAuth` при Resume сохраняется (9.3.1: Resume «сохраняет platform controls кроме phEnable»), для этого `Shutdown(STATE)` сохраняет volatile-блок иерархий. Порядок: `phEnableNV` не влияет на `NV_UndefineSpace`/`ReadPublic` иначе, чем через доступность индекса (Table 43).
19. `GetCapability`: содержимое списка постоянных handle (`OWNER`, `NULL`, `LOCKOUT`, `ENDORSEMENT`, `PLATFORM`; какие значения `TPM_RH_*` перечисляет TPM, текст не говорит); значения `TPM_PT_HR_LOADED_MIN` = `ACTIVE_SESSIONS_MAX` = 64 (не «минимум» PTP, а ёмкость реализации).
20. `TPM2_Clear` обнуляет `Clock` смещением (`Clock` = база + время питания − момент Clear), `Time` не трогает; новые `shProof`/`ehProof` не создаются (их нет); `lockoutAuth` в срезе A — только значение без DA.

### Отклонения от PTP 1.07 и плана (записаны, не скрыты)

- SHA-384 (PTP: `M`, банк PCR) и SHA-512 (`M`) не реализованы; план S7-21 считает SHA-384 опциональным, SHA-1 для PCR запрещён PTP 1.07 (Table 3, `N`), что снимает вопрос банка SHA-1 «по S7-30». Нужен ли банк SHA-384 для Windows и для TCG2 event log — решение S7-22/S7-30.
- `TPM2_PCR_Allocate` (PTP: `M`) не поддержана (отказ `TPM_RC_COMMAND_CODE`).
- `TPM_CAP_PCR_PROPERTIES` не реализована (PTP 4.7.1, п. 4: TPM должен возвращать `TPM_PT_PCR_RESET_Lx`): `GetCapability` отвечает `TPM_RC_VALUE` с параметром 1 (`capability.rs`). Назначено S7-22: она обязана реализовать `TPM_CAP_PCR_PROPERTIES` или оставить этот ответ, записав в отчёт S7-22.
- `TPM_CAP_TPM_PROPERTIES` перечисляет только свойства реализованных возможностей (PTP 4.2, п. 2 требует все определённые); свойства сессий, NV, контекстов, DA, audit, persistent-объектов и атрибутов иерархий появятся с фазой B. `TPM_PT_HR_TRANSIENT_MIN` = 3 — это число sequence-слотов.
- `TPM_PT_MAX_COMMAND_SIZE` = `TPM_PT_MAX_RESPONSE_SIZE` = 3968 (предел буфера CRB, ADR-0009, решение 4), `TPM_PT_INPUT_BUFFER` = 1024, `TPM_PT_MAX_DIGEST` = 32, `TPM_PT_MAX_CAP_BUFFER` = 1024.
- Минимальные значения PTP Table 2 для сессий, persistent-объектов, NV (`TPM_PT_HR_*_MIN`, `TPM_PT_NV_*`) в фазе A не заявляются: этих возможностей нет.
- Криптозависимости: обязательство S7-21 выполнено в срезе A S7-23 — HMAC-SHA-256 для DRBG теперь крейт `hmac =0.13.0` (`crypto.rs`; версии не было в офлайн-кеше, получена `cargo fetch`), самописный HMAC удалён, векторы RFC 4231 (случаи 1, 2, 6) и DRBG проходят с прежними значениями; записано в [DEPENDENCIES.md](../policies/DEPENDENCIES.md). `zeroize =1.9.1` (транзитивный по ADR) подключён напрямую для стирания секретов, со срезом A — с feature `alloc` (стирание `Vec<u8>`).
- Срез A: `TPM2_NV_WriteLock` и `TPM2_NV_ReadLock` реализованы сверх списка плана (иначе `WRITELOCKED`/`READLOCKED` недостижимы); в `TPM_CAP_TPM_PROPERTIES` по-прежнему нет свойств контекстов, DA, audit, persistent-объектов и NV-счётчиков; PTP 1.07 Table 12 допускает неподдержку `GLOBALLOCK`, `ORDERLY`, `CLEAR_STCLEAR`, `READ_STCLEAR`: первые три refused (`TPM_RC_ATTRIBUTES`), `READ_STCLEAR` поддержан; `TPM2_PCR_Allocate` по-прежнему отказ.

Тесты: `cargo test -p od-tpm --offline` (133 теста: 88 фазы A без изменения ожидаемых значений, плюс срез A — unit-тесты `crypto.rs`/`nv.rs`/`marshal.rs` и `src/tests_b.rs`: каждая новая команда — успех, 2–3 ошибки, отказ авторизации (пароль и HMAC), не загруженная сессия; «смена auth — старый не работает» для всех четырёх авторизаций; правила `Startup` для иерархий; независимый вектор сессионного обмена (Python `hmac`/`hashlib`: DRBG, KDFa, cpHash/rpHash, HMAC команды и ответа, unbound и bound-сессия); перечень `TPM_CAP_COMMANDS` (30); прогон искажённых команд срезов A без паники).
