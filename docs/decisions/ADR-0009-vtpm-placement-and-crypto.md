# ADR-0009: размещение vTPM, криптографические зависимости, профиль команд, CRB и состояние

- **Дата:** 7 ноября 2026 года.
- **Статус:** **предложен, ждёт архитектурного ревью** (S7-20). Размещение процессора команд (D8) и семейство криптобиблиотек (D9) — решения владельца от 2026-11-07 ([план этапа 7, §1.1](../plans/STAGE_7.md#11-решения-владельца-проекта-2026-11-07)); ADR их не выбирает заново, а записывает с альтернативами и последствиями. Версии, features, профиль команд, раскладка CRB и границы состояния — предложение архитектора, ждущее ревью.
- **Номер:** `ADR-0007` и `ADR-0008` зарезервированы планом этапа 11 (транспорт агента S11-20, сетевой backend; см. [карту проекта](../PROJECT_LAYOUT.md)), поэтому первый свободный номер — `0009`.
- **Связанные документы:** [план этапа 7](../plans/STAGE_7.md) (S7-20…S7-26, S7-30, S7-33, §3, §4.3), [отчёт этапа 7](../reports/STAGE_7_REPORT.md), [виртуальная плата](../specs/VIRTUAL_BOARD.md) (строка «TPM / Secure Boot state», карта адресов), [контракт входа в прошивку](../specs/FIRMWARE_BOOT.md) (окно CRB, поле 6 handoff block), [контракт vTPM](../specs/VTPM.md), [архитектура](../ARCHITECTURE.md) (разделы 3, 7, 9), [карта проекта](../PROJECT_LAYOUT.md), [политика зависимостей](../policies/DEPENDENCIES.md), [ADR-0001](ADR-0001-dependencies-and-boundaries.md), [ADR-0002](ADR-0002-general-purpose-libraries.md), [firmware/AGENTS.md](../../firmware/AGENTS.md), [состояние VM](../specs/VM_STATE.md), [формат хранения](../specs/STORAGE_FORMAT.md), [реестр проблем](../KNOWN_ISSUES.md).

## Вопрос

Где живёт собственный процессор команд TPM 2.0 и его состояние, какие криптобиблиотеки и в каких версиях и features используются на host и в `no_std` прошивке, какой профиль команд и какой интерфейс CRB реализуется, что входит в persistent/volatile состояние — так, чтобы S7-21…S7-24 выполнялись без новых архитектурных решений о зависимостях.

## Контекст и требования

- Windows требует TPM 2.0 ([GUEST_REQUIREMENTS.md](../specs/GUEST_REQUIREMENTS.md)); целевое устройство платы — собственная vTPM 2.0 с редакциями TPM 2.0 Library v185 и PC Client PTP 1.07 Errata 1.1 ([VIRTUAL_BOARD.md](../specs/VIRTUAL_BOARD.md), «Нормативные источники»). **Тексты этих спецификаций в рамках S7-20 не сверялись** (так же, как в [FIRMWARE_BOOT.md](../specs/FIRMWARE_BOOT.md)); каждое утверждение ниже о регистрах, кодах и обязательном наборе помечено «не сверено» и сверяется задачей-исполнителем до кода.
- Протокол TPM, его структуры и состояние — собственные; криптопримитивы и DER-парсеры — разрешённые библиотеки общего назначения ([AGENTS.md](../../AGENTS.md), «Самостоятельная реализация»; [DEPENDENCIES.md](../policies/DEPENDENCIES.md); план §3 п. 7). Готовые реализации TPM (ms-tpm-20-ref, swtpm, tpm2-tss) запрещены, в том числе транзитивно.
- TPM-устройство в прошивке не размещается ([firmware/AGENTS.md](../../firmware/AGENTS.md)); прошивка — клиент через CRB и `EFI_TCG2_PROTOCOL` (S7-22).
- Окно CRB `[0xFED4_0000, 0xFED4_5000)`, 20 KiB, localities 0–4; поле 6 handoff block `PRESENT = 0` до S7-22 ([FIRMWARE_BOOT.md](../specs/FIRMWARE_BOOT.md), разделы 2 и 6).
- Этап 7 задаёт формат и host-API выдачи/приёма байтов TPM state; запись в bundle, защита ключей и snapshot-согласование — этап 9, S9-27 (решение владельца D1).
- Контракты плана §4.3: команды вне профиля → `TPM_RC_COMMAND_CODE`, не `TPM_RC_SUCCESS` с нулями; PCR расширяются ровно тем, что записано в event log; seeds и persistent keys не логируются.
- Факт на 2026-11-07 (HEAD `426c98a`): кода TPM нет ни в одном crate ([VM_STATE.md](../specs/VM_STATE.md) §2.4); внешние зависимости workspace — `serde`, `serde_json`, `sha2 =0.10.9` (`od-testkit`), `libc`; `BusDevice` в `crates/od-machine/src/bus.rs` требует проверить операцию целиком до синхронного эффекта.

### Источники проверки зависимостей (2026-11-07) и что они не доказывают

| Утверждение | Источник | Статус |
|---|---|---|
| Версии, даты выпуска, MSRV, заявленная лицензия, features и прямые зависимости | crates.io API (`/api/v1/crates/<name>/<version>`, `/dependencies`) | Проверено для каждого crate таблицы ниже |
| `no_std` и обязательность `alloc` | `src/lib.rs` скачанных `.crate` (атрибут `#![no_std]`, `extern crate alloc` без `cfg`) | Проверено чтением исходника; **сборка под `x86_64-unknown-none` не выполнялась** |
| Транзитивный граф и лицензии транзитивных пакетов | `cargo generate-lockfile` + `cargo tree -e normal` в двух временных проектах вне репозитория (host-набор и firmware-набор с `--target x86_64-unknown-none`); `cargo metadata` для лицензий | Проверено разрешение графа; компиляция не выполнялась |
| Статус RUSTSEC-2023-0071 | `rustsec/advisory-db`, файл `crates/rsa/RUSTSEC-2023-0071.md` (прочитан 2026-11-07): `patched = []`, «still affected as of 2026-09-12: rsa 0.9.10 … and rsa 0.10.0-rc.18» | Проверено; для `0.10.0-rc.22` (выпущен 2026-11-07) статус в advisory не указан, патч не заявлен |
| Отсутствие пригодного общего DRBG в семействе | исходник `rfc6979-0.6.0/src/lib.rs`: `mod hmac_drbg;` без `pub`, используется только `KGenerator` для nonce ECDSA | Проверено: HMAC_DRBG внутри `rfc6979` не экспортируется |
| Context7 по RustCrypto | не использовался: crates.io и исходники crate дают первоисточник для перечисленных фактов | — |

## Решение 1. Размещение процессора команд (D8)

**Новый crate `crates/od-tpm/`** — чистая логика TPM 2.0 без шины, MMIO, устройств, потоков, файлов, ОС-часов и ОС-энтропии. Зависимость `od-machine → od-tpm`; `od-tpm` не зависит ни от одного crate проекта. Crate создаёт S7-21; S7-20 только записывает его в карту проекта.

| Компонент | Владелец | Ответственность |
|---|---|---|
| Процессор команд, маршалинг Part 2, сессии, PCR, иерархии, объекты, NV, policy, DA, собственные KDFa/KDFe и HMAC-DRBG, кодек persistent state | `crates/od-tpm/` (S7-21, S7-23) | Принимает байты команды и locality, возвращает байты ответа с `TPM_RC`; любая ошибка гостевой команды — ответ TPM, не `Err` host и не panic. Время (`TPM2_ReadClock`, `GetTime`) и энтропию получает аргументами от владельца |
| CRB-устройство: регистры, locality, буферы, вызов процессора | `crates/od-machine/src/tpm_crb.rs` (S7-22) | Модель на `MachineBus`; `DeviceError` — только для нарушений контракта шины (ширина, смещение), не для содержимого команды |
| Выдача/приём байтов TPM state host-стороне | `od-machine` public API (`tpm_state_snapshot()/restore`, S7-22) поверх кодека `od-tpm` | Байты выдаются владельцу машины как есть; записи в bundle нет (S9-27) |
| Клиент TPM: драйвер CRB, `EFI_TCG2_PROTOCOL`, event log, измерения | `firmware/uefi/src/tpm/` (S7-22), PCR7 — S7-24 | Хеширует измеряемые данные сам (`HashLogExtendEvent`), в TPM передаёт команды |
| Защита ключей, persistence, rollback-политика | `od-host`/`od-storage`, S9-27 | Не в этапе 7 |

Точные сигнатуры API `od-tpm` фиксирует S7-21 в [VTPM.md](../specs/VTPM.md); здесь задаётся только форма: «байты команды + locality → байты ответа», `persistent_snapshot() -> Vec<u8>` и `restore(&[u8])` с проверкой версии, создание с переданной владельцем энтропией.

**Почему отдельный crate (SOLID/KISS, [AGENTS.md](../../AGENTS.md)):**

1. Одна ответственность: протокол и криптографическое состояние TPM меняются по своим причинам (профиль команд S7-30, persistent v1), отдельно от модели шины и устройств.
2. Изоляция криптозависимостей: RSA/ECC/AES (52 пакета в полном наборе, из них 11 прямых и 41 транзитивный, см. ниже) попадают только в `od-tpm` и его потребителей, а не в каждый тест и инструмент, которому нужен `od-machine`.
3. Независимые host-side тесты: `cargo test -p od-tpm` проверяет команды без машины и прошивки (S7-21 п. 5, S7-23).
4. Одно место для формата persistent state, к которому подключится S9-27 (DRY: кодек не дублируется между устройством и хранилищем).
5. Направление зависимости сохраняет правило карты: нижняя библиотека не знает про машину и host.

**Альтернативы.**

1. *Модуль `crates/od-machine/src/tpm/`.* Отклонена владельцем (D8): тянет криптокрейты во весь `od-machine`, смешивает протокол с моделью устройства и не даёт независимых тестов. Выигрыш — на один crate меньше — не перевешивает.
2. *Отдельный процесс (TPM-сервис за сокетом, как swtpm).* Даёт изоляцию памяти ключей в отдельном адресном пространстве, но добавляет IPC, жизненный цикл, восстановление соединения и второй wire format; схема процессов [ARCHITECTURE §3](../ARCHITECTURE.md) содержит только приложение, CLI и по одному VMHost на машину, а Rust-движок (в том числе устройства) живёт внутри VMHost без отдельного IPC. По KISS не нужен, пока защита ключей отнесена к S9-27. Пересмотр — если S9-27 потребует изоляции секретов на уровне процесса.
3. *TPM внутри прошивки.* Запрещено [firmware/AGENTS.md](../../firmware/AGENTS.md): состояние TPM не должно жить в памяти гостя.
4. *Готовая реализация TPM.* Запрещено политикой зависимостей и планом §3 п. 7.

## Решение 2. Криптография (D9)

Примитивы — семейство RustCrypto на обеих целях. Версии фиксируются точным `=`; семейство согласовано по поколению трейтов (`digest 0.11`, `cipher 0.5`, `elliptic-curve 0.14`, `der 0.8`, `signature 3`), поэтому смешивать с прежним поколением (`digest 0.10`, `spki 0.7`) нельзя.

**Собственные части (не библиотеки):** протокол TPM 2.0 (команды, маршалинг, сессии, PCR, иерархии, NV, policy, DA, контексты); KDFa (SP 800-108, режим счётчика на HMAC) и KDFe (SP 800-56A, concatenation KDF на хеше) поверх `hmac`/`sha2`; HMAC-DRBG поверх `hmac` (общего экспортируемого DRBG в семействе нет — см. источники; редакцию SP 800-90A и допустимость HMAC_DRBG по Library v185 сверяет S7-21, **не сверено**); логика PKCS#7 `SignedData`/Authenticode и политика цепочки сертификатов (S7-24) поверх `der`/`x509-cert`/`spki`. Готовый `cms` (`0.2.3`, `0.3.0-rc.1`) существует в семействе, но не берётся: логика PKCS#7 — собственная по D9.

### Перечень crate'ов

Все проверки — 2026-11-07, источники в таблице выше. «no_std» — атрибут в исходнике; «alloc» — требуется ли аллокатор. Колонка «прошивка» — кандидат для S7-22/S7-24, не утверждение о сборке.

| Crate | Версия | Назначение | Цели | no_std / alloc | Лицензия | Features host (`od-tpm`) | Features прошивки |
|---|---|---|---|---|---|---|---|
| `sha1` | `=0.11.0` | SHA-1: банк PCR SHA-1 (если S7-30 подтвердит), HMAC-SHA1 | host, прошивка (S7-22) | да / нет (`alloc` опционален) | MIT OR Apache-2.0 | `default-features = false` | `default-features = false` |
| `sha2` | `=0.11.1` | SHA-256/SHA-384: PCR, `TPM2_Hash`, KDF, проверка подписи | host, прошивка (S7-22, S7-24) | да / нет | MIT OR Apache-2.0 | `default-features = false` | `default-features = false` |
| `hmac` | `=0.13.0` | HMAC для сессий, KDFa, HMAC-DRBG | host; прошивка — только транзитивно через `rsa` | да / нет | MIT OR Apache-2.0 | `zeroize` | — |
| `aes` | `=0.9.4` | AES-128/256: шифрование параметров, симметричные объекты | host | да / нет | MIT OR Apache-2.0 | по умолчанию (features `hazmat` не нужен) | — |
| `cfb-mode` | `=0.9.1` | режим CFB для AES (Part 1, шифрование параметров) | host | да / нет | MIT OR Apache-2.0 | `default-features = false`, `zeroize` | — |
| `rsa` | `=0.10.0-rc.22` (**pre-release**) | RSA-2048: host — PKCS#1 v1.5, OAEP, PSS по профилю; прошивка — проверка PKCS#1 v1.5/SHA-256 | host (S7-23), прошивка (S7-24) | да / **да** (`extern crate alloc` безусловно) | MIT OR Apache-2.0 | `default-features = false`, `pkcs1v15`; без `getrandom` (энтропия — DRBG `od-tpm`) | `default-features = false`, `pkcs1v15`; `encoding` — только если S7-24 разбирает ключ через `pkcs1` |
| `p256` | `=0.14.0` | ECC NIST P-256: ECDSA, ECDH (`ECDH_ZGen`, KDFe) | host (S7-23) | да / нет (`alloc` опционален) | Apache-2.0 OR MIT | `default-features = false`, `arithmetic`, `ecdsa`, `ecdh` | — |
| `ecdsa` | `=0.17.0` | общий ECDSA (подключается через `p256/ecdsa`; прямая зависимость — только если S7-23 нужна напрямую) | host | да / нет | Apache-2.0 OR MIT | `default-features = false`, `digest` | — |
| `der` | `=0.8.3` | DER/ASN.1: разбор PKCS#7, X.509, `EFI_SIGNATURE_LIST` с X.509 | прошивка (S7-24); host — если понадобится | да / нет (`alloc` опционален; в firmware-наборе включён) | Apache-2.0 OR MIT | — | `default-features = false`, `alloc`, `oid` |
| `x509-cert` | `=0.3.0` | структуры X.509 (сертификаты `db`/`KEK`, цепочка Authenticode) | прошивка (S7-24) | да / **да** | Apache-2.0 OR MIT | — | `default-features = false` |
| `spki` | `=0.8.1` | `SubjectPublicKeyInfo` → открытый ключ RSA | прошивка (S7-24) | да / нет (`alloc` опционален) | Apache-2.0 OR MIT | — | `default-features = false`, `alloc` |

MSRV наибольший у `aes` (1.89) и `sha2` (1.88); локальный toolchain `rustc 1.99.0` ([firmware/uefi/README.md](../../firmware/uefi/README.md)).

**Транзитивные зависимости** (разрешены `cargo tree -e normal`, компиляция не выполнялась). Объединённый набор всех 11 crate'ов таблицы (временный проект на host-target с запасом features: `rsa` с `std`, `pkcs1v15`, `encoding`; `p256` с `alloc`, `ecdsa`, `ecdh`; плюс явный `zeroize`) разрешается в 52 пакета (11 прямых и 41 транзитивный); транзитивные: `digest 0.11.3`, `block-buffer 0.12.1`, `crypto-common 0.2.2`, `cipher 0.5.2`, `inout 0.2.6`, `cpufeatures 0.3.1`, `cpubits 0.1.1`, `cfg-if 1.0.5`, `libc 0.2.191` (через `cpufeatures` на host), `crypto-bigint 0.7.10`, `crypto-primes 0.7.2`, `ctutils 0.4.8`, `cmov 0.5.4`, `hybrid-array 0.4.15`, `typenum 1.20.1`, `subtle 2.6.1`, `zeroize 1.9.1`, `rand_core 0.10.1`, `signature 3.0.0`, `const-oid 0.10.2`, `elliptic-curve 0.14.3`, `primeorder 0.14.0`, `primefield 0.14.0`, `ff 0.14.0`, `group 0.14.0`, `wnaf 0.14.1`, `sec1 0.8.1`, `rfc6979 0.6.0`, `hkdf 0.13.0`, `pkcs1 0.8.0`, `pkcs8 0.11.0`, `base16ct 1.0.0`, `base64ct 1.8.3`, `pem-rfc7468 1.0.0`, `flagset 0.4.7`, `num-traits 0.2.19`, `der_derive 0.8.0` и proc-macro `syn 2.0.119`, `quote 1.0.47`, `proc-macro2 1.0.107`, `unicode-ident 1.0.27` (только при сборке). Firmware-набор (`sha1`, `sha2`, `rsa[pkcs1v15]`, `der`, `x509-cert`, `spki`, `--target x86_64-unknown-none`) разрешается в 30 пакетов вместе с прямыми — подмножество объединённого набора (`hmac`, `crypto-bigint`, `crypto-primes`, `signature`, `rand_core`, `zeroize`, `const-oid`, `flagset`, `der_derive` и proc-macro). Ни один пакет не является реализацией TPM, UEFI или эмулятора: все — примитивы, большие числа, кодирование, константное время, обнуление памяти, служебные макросы (проверено по именам и назначению crate'ов).

**Лицензии транзитивных пакетов**, отличные от «MIT OR Apache-2.0»: `subtle` — BSD-3-Clause; `flagset` — Apache-2.0; `ff`, `group` — «MIT/Apache-2.0»; `unicode-ident` — «(MIT OR Apache-2.0) AND Unicode-3.0» (proc-macro, в runtime не входит). Все разрешительные; D9 называет MIT/Apache-2.0 для прямых crate'ов, отличия транзитивных записаны для ревью.

### Условие D9 о constant-time `rsa`

**Версии `rsa` с подтверждённой constant-time арифметикой на 2026-11-07 нет.** Последняя стабильная — `0.9.10`, предрелиз — `0.10.0-rc.22`; advisory RUSTSEC-2023-0071 имеет `patched = []` и прямо называет затронутыми `0.9.10` и `0.10.0-rc.18`. README `0.10.0-rc.22` советует переходить на 0.10 «to avoid the discovered side channels» и ссылается на продолжающуюся работу по constant-time hardening.

По D9 это записывается как **принятый риск и открытая проблема** — [ISSUE-064](../KNOWN_ISSUES.md#issue-064-s7-rsa-private-ops-not-constant-time). Выбор `0.10.0-rc.22`, а не `0.9.10`:

1. `0.9` принадлежит прежнему поколению трейтов (`digest 0.10`, `pkcs1 0.7`, `spki 0.7`, `signature <2.3`) и потребовал бы второй набор семейства рядом с `sha2 0.11`/`der 0.8`/`x509-cert 0.3` (DRY, лишний транзитивный объём).
2. `0.10` построен на `crypto-bigint`, сопровождающие рекомендуют его, а PKCS#1 v1.5 там включается явным feature `pkcs1v15`.
3. Цена: предрелизный API может меняться между `rc`; фиксация `=0.10.0-rc.22` и обновление только отдельным решением с ревью.

Границы риска: в прошивке выполняются только операции с открытым ключом (проверка подписи), Marvin их не касается. Риск относится к приватным операциям `od-tpm` на host (`Sign`, `RSA_Decrypt`, salted `StartAuthSession`, `ActivateCredential`, `Import`), где наблюдателем времени может быть гостевой код. Меры и условие закрытия — в ISSUE-064.

### Требования к секретам (открыты для S7-23/S7-24)

[DEPENDENCIES.md](../policies/DEPENDENCIES.md) требует фиксировать ограничения runtime и способ проверки, но отдельного правила о constant-time и обнулении памяти не содержит; условие D9 касается только `rsa`. ADR задаёт требования, выполнение и проверку которых записывают S7-23 (host) и S7-24 (прошивка):

- seeds, приватные ключи, `authValue`, ключи сессий, состояние HMAC-DRBG хранятся в типах, обнуляющих память при уничтожении (`zeroize`, уже транзитивный); `Debug`/`Display` таких типов не печатают содержимое;
- сравнение HMAC, `authValue`, policy digest — за постоянное время (`subtle`/`ctutils` или проверка `hmac`), не `==` по срезам;
- ECDSA/ECDH — только через `p256` (заявленная библиотекой константность на `crypto-bigint` нами не проверялась — записать в S7-23 как заявление поставщика, не как наш факт);
- прошивка секретов не хранит (D5: private key `PK` на host уничтожается); требования к ней ограничиваются отсутствием утечки через логи.

### Сборка в `no_std` прошивке (условие проверки, не результат)

Сборка под `x86_64-unknown-none` в S7-20 **не выполнялась**. S7-22 (`sha1`, `sha2`) и S7-24 (`rsa`, `der`, `x509-cert`, `spki`) при первом появлении crate'а в `firmware/uefi/Cargo.toml` обязаны:

1. Собрать прошивку командой из [firmware/uefi/README.md](../../firmware/uefi/README.md) с `--locked` после отдельного шага получения пакетов и записать результат.
2. Проверить выбор backend: target `x86_64-unknown-none` собирается без SSE (soft-float), а `sha1`/`sha2`/`aes` выбирают SIMD-backend через `cpufeatures`. Если сборка или исполнение с SIMD-backend невозможны, включить программный backend объявленными в crate'ах cfg (`sha1_backend = "soft"`, `sha2_backend = "soft"`; проверено по `check-cfg` в `Cargo.toml` этих версий) и записать это в README.
3. Подтвердить, что `alloc`-зависимые crate'ы (`rsa`, `x509-cert`) работают с аллокатором прошивки (S7-04).
4. Повторить проверку транзитивного графа `cargo tree` для firmware Cargo.lock и сверить с перечнем выше.

## Решение 3. Профиль команд

- **Цель профиля** — обязательные команды PC Client PTP 1.07 Errata 1.1 (точный перечень по тексту PTP — **не сверено**, сверяет S7-21 для фазы A и S7-23 для фазы B); перечень, нужный Windows, уточняет S7-30.
- **Фаза A (S7-21):** `Startup`/`Shutdown`, `SelfTest`/`GetTestResult`, `GetCapability`, `GetRandom`/`StirRandom`, PCR (`PCR_Read/Extend/Event/Reset`, `PCR_Allocate` — поддержка или отказ записывается), `Hash` и hash/event sequences, `ReadClock`; authorization area с password session `TPM_RS_PW`. Точный список и правила PCR — [S7-21](../plans/STAGE_7.md#s7-21-реализовать-процессор-команд-tpm-20-фаза-a) (DRY: не дублируется здесь).
- **Фаза B (S7-23):** иерархии Platform/Owner/Endorsement/Null с seeds и auth, RSA/ECC ключи (`CreatePrimary`, `Create`, `Load`, `Sign`, `RSA_Encrypt/Decrypt`, `ECDH_ZGen` …), HMAC- и policy-сессии с шифрованием параметров, NV, `ContextSave/Load` по итогам S7-30, `Quote`/`Certify`, DA, persistent state v1 — [S7-23](../plans/STAGE_7.md#s7-23-реализовать-tpm-20-фаза-b-иерархии-ключи-сессии-nv).
- **Алгоритмы v1:** SHA-256 (банк PCR обязателен), SHA-1 (банк — по S7-30), SHA-384 (банк — опционально), HMAC, AES-128/256 CFB, RSA-2048, ECC NIST P-256, KDF собственные. RSA-3072, ECC P-384, SM2/SM3/SM4, Camellia в v1 не входят и не показываются в `GetCapability(TPM_CAP_ALGS)`; добавление требует новой зависимости (например, `p384`) и записи в этот ADR. Достаточность набора для минимума PTP 1.07 — **не сверено** (S7-21/S7-30).
- **Нереализованное:** любая команда вне реализованного профиля → `TPM_RC_COMMAND_CODE`; команда до `Startup` → `TPM_RC_INITIALIZE`; `GetCapability(TPM_CAP_COMMANDS)` перечисляет ровно реализованные команды. Ответ `TPM_RC_SUCCESS` без действия запрещён (план §4.3). Не поддерживаются: field upgrade, attached components, vendor-команды, DRTM-сценарии (localities 2–4, сброс PCR17–22); со стороны TCG2 `SetActivePcrBanks`/`GetResultOfSetActivePcrBanks` → `EFI_UNSUPPORTED` (S7-22). Числовые значения `TPM_RC_*` берутся из Part 2 v185 исполнителем.
- **Соответствие и сертификация TCG не заявляются** (план S7-20 п. 5, [VIRTUAL_BOARD.md](../specs/VIRTUAL_BOARD.md)).

## Решение 4. Интерфейс CRB (реализует S7-22)

- Только CRB по PC Client PTP 1.07 Errata 1.1; FIFO/TIS не реализуется. В ACPI-таблице TPM2 `StartMethod = 7` (CRB); таблица публикуется только при `PRESENT = 1` поля 6 handoff block ([FIRMWARE_BOOT.md](../specs/FIRMWARE_BOOT.md), разделы 2 и 6).
- Окно `[0xFED4_0000, 0xFED4_5000)`: пять страниц 4 KiB, страница `n` — locality `n`. **Функциональна только locality 0.** Запрос других localities (`TPM_LOC_CTRL` requestAccess на страницах 1–4) не удовлетворяется: locality не назначается, команды через них не исполняются. Точные значения регистров страниц 1–4 при чтении и записи S7-22 записывает в `VTPM.md` («CRB»). PCR0 инициализируется startup locality 0 (S7-21).
- Прерывания не используются: `TPM_CRB_INT_ENABLE`/`INT_STS` не вызывают IRQ (S7-22 записывает поведение регистров); ACPI TPM2 прерываний не объявляет.
- Регистры страницы locality (смещения от её начала; **по PTP 1.07, не сверено** — S7-22 сверяет с текстом PTP до кода):

| Смещение | Регистр | Ширина |
|---|---|---|
| `0x00` | `TPM_LOC_STATE` | 32 |
| `0x08` | `TPM_LOC_CTRL` | 32 |
| `0x0C` | `TPM_LOC_STS` | 32 |
| `0x30` | `TPM_CRB_INTF_ID` (тип интерфейса CRB, возможности; по нему ОС выбирает драйверный путь) | 64 |
| `0x38` | `TPM_CRB_CTRL_EXT` | 64 |
| `0x40` | `TPM_CRB_CTRL_REQ` | 32 |
| `0x44` | `TPM_CRB_CTRL_STS` | 32 |
| `0x48` | `TPM_CRB_CTRL_CANCEL` | 32 |
| `0x4C` | `TPM_CRB_CTRL_START` | 32 |
| `0x50` | `TPM_CRB_INT_ENABLE` | 32 |
| `0x54` | `TPM_CRB_INT_STS` | 32 |
| `0x58` | `TPM_CRB_CTRL_CMD_SIZE` | 32 |
| `0x5C` | `TPM_CRB_CTRL_CMD_LADDR` | 32 |
| `0x60` | `TPM_CRB_CTRL_CMD_HADDR` | 32 |
| `0x64` | `TPM_CRB_CTRL_RSP_SIZE` | 32 |
| `0x68` | `TPM_CRB_CTRL_RSP_ADDR` | 64 |
| `0x80` | `TPM_CRB_DATA_BUFFER` (до конца страницы, `0xF80` байт) | — |

- **Буферы (предложение):** один общий буфер команды/ответа в странице locality 0 по смещению `0x80`, `CMD_SIZE = RSP_SIZE = 0xF80` (3968 байт), `CMD_ADDR = RSP_ADDR = 0xFED4_0080`. Отсюда ограничение для S7-21: `TPM_PT_MAX_COMMAND_SIZE` и `TPM_PT_MAX_RESPONSE_SIZE` в `GetCapability` не больше 3968. Минимальный размер буфера по PTP — **не сверено**; если S7-22/S7-30 покажут, что нужен больший буфер, он размещается внутри окна CRB с изменением [FIRMWARE_BOOT.md](../specs/FIRMWARE_BOOT.md), а не вне его.
- **Исполнение:** запись `TPM_CRB_CTRL_START = 1` вызывает процессор `od-tpm` синхронно внутри записи `BusDevice` (контракт шины: проверка до эффекта, затем синхронный эффект) либо отложенно через event queue — выбор и причину записывает S7-22. Ответ кладётся в буфер, `START` сбрасывается; некорректная команда гостя — ответ TPM с `TPM_RC`, не `DeviceError`.

## Решение 5. Состояние

Формат записывается в [VTPM.md](../specs/VTPM.md): сейчас там только раздел «Состояние и границы» со ссылкой сюда; «Состояние» фазы A — S7-21, «Persistent state v1» — S7-23.

- **Persistent state v1** (полностью — S7-23; S7-21 заводит версионный каркас с seeds и счётчиками): seeds иерархий (platform, storage/owner, endorsement); `authValue`, `authPolicy` и флаги включения иерархий в части, которую Part 1 относит к постоянной (разделение по `Startup(CLEAR)` — **не сверено**, S7-23); NV-индексы (public area и данные); persistent objects (`EvictControl`); счётчики (NV counters, `resetCount`/`restartCount`, сохраняемая часть clock); состояние DA (`failedTries`, параметры lockout). Предложение архитектора (открытый вопрос 6): PCR-политика (`PCR_SetAuthPolicy`/`PCR_SetAuthValue`) в v1 не входит, эти команды не поддерживаются.
- **Volatile** (сбрасывается по `Startup(CLEAR)` и при пересоздании процесса): значения PCR, сессии, transient objects, загруженные контексты, hash/event sequences, назначенная locality, признак выполненного `Startup`. Поддержка `Shutdown(STATE)`/`Startup(STATE)` (сохранение части volatile) — решение S7-21 с записью в `VTPM.md`; до него `Startup(STATE)` без сохранённого состояния возвращает код ошибки по Part 3, не `SUCCESS`.
- **DRBG:** состояние HMAC-DRBG в persistent state не входит; при создании и при `restore` владелец передаёт свежую энтропию host, чтобы восстановленный TPM не повторял выход генератора.
- **Формат:** версионные байты собственного кодека `od-tpm`; неизвестная или повреждённая версия → `restore` отвергается, TPM не стартует (не «пустой TPM молча»). Байты содержат секреты в открытом виде и выдаются владельцу машины через `od-machine` public API с этой пометкой в контракте; шифрование и хранение — S9-27.
- **Никогда не логируются** и не попадают в отчёты, эталоны COM1 и сообщения ошибок: seeds, приватные части ключей, `authValue`, ключи и nonce сессий, состояние и выход DRBG (включая ответ `GetRandom` — печатается только длина), данные NV и `Unseal`. Допустимы handles, коды `TPM_RC`, размеры и публичные области.

## Последствия

- [PROJECT_LAYOUT.md](../PROJECT_LAYOUT.md) получает владельца `crates/od-tpm/` и стрелку `od-machine → od-tpm`; машиночитаемый перечень каталогов дополняется вместе с созданием crate в S7-21 (иначе `check_layout.py` даст ошибку отсутствующего каталога). [ARCHITECTURE §9](../ARCHITECTURE.md) перечисляет crate'ы каркаса; строку `od-tpm` вносит S7-21 вместе с crate (§9 допускает новые crate'ы при сформировавшейся границе).
- В host `Cargo.lock` появятся внешние пакеты `od-tpm`; их число не замерено (оценка по объединённому набору — 52 пакета вместе с 11 прямыми, оценка, не замерено; host-набор `od-tpm` может быть меньше); получение пакетов — отдельный шаг, проверки — `--locked --offline` ([DEPENDENCIES.md](../policies/DEPENDENCIES.md)). В workspace будут две версии `sha2` (`0.10.9` в test-only `od-testkit` и `0.11.1` в `od-tpm`); объединение не обязательно и не входит в S7-21.
- Прямая зависимость `zeroize =1.9.1` (`default-features = false`, Apache-2.0 OR MIT) подключена в `crates/od-tpm` для стирания секретов (`authValue`, seed'ы, состояние DRBG), хотя в исходном ADR она была только транзитивной; обоснование — требование ADR хранить секреты в zeroize-типах (раздел «Требования к секретам»); записано по процедуре [DEPENDENCIES.md](../policies/DEPENDENCIES.md).
- Прошивка получает криптографию в два шага: `sha1`/`sha2` в S7-22, `rsa`/`der`/`x509-cert`/`spki` в S7-24 — каждый с проверкой сборки по условию выше. Оставлять криптографию прошивки «на потом» нельзя: перечень зафиксирован здесь.
- Связь с задачами: S7-21 и S7-23 — `od-tpm`; S7-22 — CRB, ACPI TPM2, TCG2, measured boot (PCR0–5) поверх этого профиля; S7-24 — Secure Boot и PCR7 поверх firmware-набора; S7-26 — persistent state переживает сброс, volatile сбрасывается; S7-33 передаёт формат этапу 9; S9-27 — защита ключей и хранение байтов в bundle. Строку «vTPM persistent state» в [STORAGE_FORMAT.md](../specs/STORAGE_FORMAT.md) («state — `od-machine`») уточняет S7-33: процессор и формат — `od-tpm`, выдача — `od-machine`.
- **Риски:** (1) приватные RSA-операции не constant-time ([ISSUE-064](../KNOWN_ISSUES.md#issue-064-s7-rsa-private-ops-not-constant-time)); (2) предрелизный `rsa` — дрейф API между `rc`; (3) сборка SIMD-backend'ов под soft-float target не проверена; (4) профиль может оказаться недостаточным для Windows до S7-30 (провижининг TPM, BitLocker); (5) секреты в выдаваемых байтах state не защищены до S9-27; (6) буфер CRB 3968 байт может оказаться меньше требуемого.

## Открытые вопросы (для S7-21…S7-24)

1. **S7-21:** сверить с Library v185 допустимость HMAC_DRBG и редакцию SP 800-90A; точные сигнатуры API `od-tpm`; поддержка `Shutdown(STATE)`/`Startup(STATE)` и `PCR_Allocate`; `TPM_PT_MAX_COMMAND_SIZE`/`MAX_RESPONSE_SIZE` ≤ 3968; таблица атрибутов PCR.
2. **S7-22:** сверить раскладку регистров CRB, `TPM_CRB_INTF_ID` и минимальный размер буфера с PTP 1.07 Errata 1.1; синхронный или отложенный вызов процессора; поведение страниц localities 1–4; сборка `sha1`/`sha2` под `x86_64-unknown-none` и выбор backend.
3. **S7-23:** разделение persistent/volatile для auth и флагов иерархий по Part 1; перечень команд по S7-30; выполнение и проверка требований к секретам (обнуление, сравнение за постоянное время); какой библиотекой проверять подпись в тесте «создать ключ → подписать → проверить» (тот же `rsa` не является независимой проверкой — выбрать и записать).
4. **S7-24:** сборка `rsa`/`der`/`x509-cert`/`spki` в прошивке; нужен ли `rsa/encoding` (`pkcs1`) или разбор `RSAPublicKey` собственный поверх `der`; политика SHA-1 подписей (план S7-24 п. 2).
5. **Ревью:** принять ли транзитивные лицензии BSD-3-Clause (`subtle`) и Apache-2.0-only (`flagset`) при формулировке D9 «MIT/Apache-2.0».
6. **S7-30 и S7-21:** сужение профиля — `PCR_SetAuthPolicy` и `PCR_SetAuthValue` не поддерживаются, PCR-политика не входит в persistent state v1 (решение 5). Это предложение архитектора, не требование плана: S7-30 подтверждает, что Windows (провижининг TPM, BitLocker) обходится без этих команд, S7-21 записывает итог в `VTPM.md` и в `GetCapability(TPM_CAP_COMMANDS)`; при необходимости профиль и persistent state расширяются.

## Условие пересмотра

- Выходит версия `rsa` с constant-time приватными операциями (advisory RUSTSEC-2023-0071 получает `patched`) или стабильный `0.10.0` — обновить версию и закрыть ISSUE-064.
- S7-30 требует алгоритмов или команд вне профиля v1 (RSA-3072, P-384, иной буфер CRB) — дополнить ADR и зависимости.
- S9-27 требует изоляции секретов на уровне процесса — пересмотреть альтернативу 2 решения 1.
- Сборка firmware-набора под `x86_64-unknown-none` невозможна ни с одним backend — пересмотреть firmware-часть решения 2 новым решением владельца.
