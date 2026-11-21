# ADR-0009: размещение vTPM, криптографические зависимости, профиль команд, CRB и состояние

- **Дата:** 7 ноября 2026 года.
- **Статус:** **принят после архитектурного ревью 2026-11-18; замечаний нет** (S7-20). Размещение процессора команд (D8) и семейство криптобиблиотек (D9) — решения владельца от 2026-11-07 ([план этапа 7, §1.1](../plans/STAGE_7.md#11-решения-владельца-проекта-2026-11-07)); ADR фиксирует их с альтернативами и последствиями. Ревью охватило ADR целиком, включая стабильную цель `rsa =0.10.0` и feature sets; host-переход с RC на стабильный release выполнен и независимо одобрен в S7-23 2026-11-18; firmware target — S7-24.
- **Запись ревью:** 2026-11-18 — независимое архитектурное повторное ревью одобрило полный scope ADR без замечаний. S7-20 принят; host migration compatibility checks прошли и независимое ревью одобрило срез; firmware-target integration остаётся в S7-24.
- **Обновление решения 3:** 2026-11-19 владелец подтвердил PC Client PTP 1.07 как цель S7-23. Расширение цели и проверка фактической поддержки отслеживаются в [ISSUE-599](../KNOWN_ISSUES.md#issue-599).
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

### Источники проверки зависимостей (2026-11-07; advisory/release update 2026-11-18) и что они не доказывают

| Утверждение | Источник | Статус |
|---|---|---|
| Версии, даты выпуска, MSRV, заявленная лицензия, features и прямые зависимости | crates.io API (`/api/v1/crates/<name>/<version>`, `/dependencies`) | Проверено для каждого crate таблицы ниже |
| `no_std` и обязательность `alloc` | `src/lib.rs` скачанных `.crate` (атрибут `#![no_std]`, `extern crate alloc` без `cfg`) | Проверено чтением исходника; **сборка под `x86_64-unknown-none` не выполнялась** |
| Транзитивный граф и лицензии транзитивных пакетов | `cargo generate-lockfile` + `cargo tree -e normal` в двух временных проектах вне репозитория (host-набор и firmware-набор с `--target x86_64-unknown-none`); `cargo metadata` для лицензий | Проверено разрешение графа; компиляция не выполнялась |
| Статус RUSTSEC-2023-0071 | [RustSec advisory](https://rustsec.org/advisories/RUSTSEC-2023-0071.html), проверен 2026-11-18 (modified 2026-11-08): `Patched >=0.10.0`; [RustCrypto RSA v0.10.0 release notes](https://github.com/RustCrypto/RSA/releases/tag/v0.10.0), 2026-11-08 | Upstream сообщает исправление в стабильной версии: `crypto-bigint` для constant-time modular exponentiation, hardening private-key operations и constant-time validation harness. Это не подтверждает интеграцию или проверку текущего кода проекта. |
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

Метаданные первоначального набора crate'ов проверены 2026-11-07; исключение — статус RUSTSEC-2023-0071 и выпуск стабильной `rsa =0.10.0`, дополнительно проверенные 2026-11-18 по источникам в таблице «Источники проверки зависимостей». «no_std» — атрибут в исходнике; «alloc» — требуется ли аллокатор. Колонка «прошивка» — кандидат для S7-22/S7-24, не утверждение о сборке. Интеграция стабильного `rsa =0.10.0` пока не собрана.

| Crate | Версия | Назначение | Цели | no_std / alloc | Лицензия | Features host (`od-tpm`) | Features прошивки |
|---|---|---|---|---|---|---|---|
| `sha1` | `=0.11.0` | SHA-1: банк PCR SHA-1 (если S7-30 подтвердит), HMAC-SHA1 | host, прошивка (S7-22) | да / нет (`alloc` опционален) | MIT OR Apache-2.0 | `default-features = false` | `default-features = false` |
| `sha2` | `=0.11.1` | SHA-256/SHA-384: PCR, `TPM2_Hash`, KDF, проверка подписи | host, прошивка (S7-22, S7-24) | да / нет | MIT OR Apache-2.0 | `default-features = false` | `default-features = false` |
| `hmac` | `=0.13.0` | HMAC для сессий, KDFa, HMAC-DRBG | host; прошивка — только транзитивно через `rsa` | да / нет | MIT OR Apache-2.0 | `zeroize` | — |
| `aes` | `=0.9.4` | AES-128/256: шифрование параметров, симметричные объекты | host | да / нет | MIT OR Apache-2.0 | `default-features = false`, `zeroize` | — |
| `cfb-mode` | `=0.9.1` | режим CFB для AES (Part 1, шифрование параметров) | host | да / нет | MIT OR Apache-2.0 | `default-features = false`, `zeroize` | — |
| `rsa` | **Selected stable pin and current host implementation: `=0.10.0` (migration, API/feature compatibility checks, and independent review completed 2026-11-18).** | RSA-2048: host — PKCS#1 v1.5, OAEP, PSS по профилю; прошивка — проверка PKCS#1 v1.5/SHA-256 | host (S7-23), прошивка (S7-24) | да / **да** (`extern crate alloc` безусловно) | MIT OR Apache-2.0 | `default-features = false`, features `pkcs1v15`, `hazmat`; без `getrandom` (энтропия — DRBG `od-tpm`) | `default-features = false`, features `pkcs1v15`, `encoding` (для разбора RSA public key в PKCS#1 при проверке сертификата) |
| `p256` | `=0.14.0` | ECC NIST P-256: ECDSA, ECDH (`ECDH_ZGen`, KDFe) | host (S7-23) | да / нет (`alloc` опционален) | Apache-2.0 OR MIT | `default-features = false`, `arithmetic`, `ecdsa`, `ecdh` | — |
| `p384` | `=0.14.0` | ECC NIST P-384: ECDSA, ECDH (`ECDH_ZGen`, `ECDH_KeyGen`, KDFe) | host (S7-23, срез 6) | да / нет (`alloc` опционален) | Apache-2.0 OR MIT | `default-features = false`, `ecdsa` (включает `arithmetic` и `sha384`) | — |
| `ml-dsa` | `=0.1.2` | ML-DSA-65 (FIPS 204): `TPM_ALG_MLDSA`, подпись и проверка сообщения и внешнего mu, `SignDigest`, `VerifyDigestSignature`, последовательности, аттестация, `PolicySigned` | host (S7-23, срез 7) | да / да (`alloc` включён: ключи на куче, а не на стеке) | Apache-2.0 OR MIT | `default-features = false`, `alloc`, `zeroize` | — |
| `ecdsa` | `=0.17.0` | общий ECDSA (подключается через `p256/ecdsa`; прямая зависимость — только если S7-23 нужна напрямую) | host | да / нет | Apache-2.0 OR MIT | `default-features = false`, `digest` | — |
| `der` | `=0.8.3` | DER/ASN.1: разбор PKCS#7, X.509, `EFI_SIGNATURE_LIST` с X.509 | прошивка (S7-24); host — если понадобится | да / нет (`alloc` опционален; в firmware-наборе включён) | Apache-2.0 OR MIT | — | `default-features = false`, `alloc`, `oid` |
| `x509-cert` | `=0.3.0` | структуры X.509 (сертификаты `db`/`KEK`, цепочка Authenticode) | прошивка (S7-24) | да / **да** | Apache-2.0 OR MIT | — | `default-features = false` |
| `spki` | `=0.8.1` | `SubjectPublicKeyInfo` → открытый ключ RSA | прошивка (S7-24) | да / нет (`alloc` опционален) | Apache-2.0 OR MIT | — | `default-features = false`, `alloc` |

MSRV наибольший у `aes` (1.89) и `sha2` (1.88); локальный toolchain `rustc 1.99.0` ([firmware/uefi/README.md](../../firmware/uefi/README.md)).

**Транзитивные зависимости** (разрешены `cargo tree -e normal`, компиляция не выполнялась). Объединённый набор всех 11 crate'ов таблицы (временный проект на host-target с запасом features: `rsa` с `std`, `pkcs1v15`, `encoding`; `p256` с `alloc`, `ecdsa`, `ecdh`; плюс явный `zeroize`) разрешается в 52 пакета (11 прямых и 41 транзитивный); транзитивные: `digest 0.11.3`, `block-buffer 0.12.1`, `crypto-common 0.2.2`, `cipher 0.5.2`, `inout 0.2.6`, `cpufeatures 0.3.1`, `cpubits 0.1.1`, `cfg-if 1.0.5`, `libc 0.2.191` (через `cpufeatures` на host), `crypto-bigint 0.7.10`, `crypto-primes 0.7.2`, `ctutils 0.4.8`, `cmov 0.5.4`, `hybrid-array 0.4.15`, `typenum 1.20.1`, `subtle 2.6.1`, `zeroize 1.9.1`, `rand_core 0.10.1`, `signature 3.0.0`, `const-oid 0.10.2`, `elliptic-curve 0.14.3`, `primeorder 0.14.0`, `primefield 0.14.0`, `ff 0.14.0`, `group 0.14.0`, `wnaf 0.14.1`, `sec1 0.8.1`, `rfc6979 0.6.0`, `hkdf 0.13.0`, `pkcs1 0.8.0`, `pkcs8 0.11.0`, `base16ct 1.0.0`, `base64ct 1.8.3`, `pem-rfc7468 1.0.0`, `flagset 0.4.7`, `num-traits 0.2.19`, `der_derive 0.8.0` и proc-macro `syn 2.0.119`, `quote 1.0.47`, `proc-macro2 1.0.107`, `unicode-ident 1.0.27` (только при сборке). Firmware-набор (`sha1`, `sha2`, `rsa[pkcs1v15]`, `der`, `x509-cert`, `spki`, `--target x86_64-unknown-none`) разрешается в 30 пакетов вместе с прямыми — подмножество объединённого набора (`hmac`, `crypto-bigint`, `crypto-primes`, `signature`, `rand_core`, `zeroize`, `const-oid`, `flagset`, `der_derive` и proc-macro). Ни один пакет не является реализацией TPM, UEFI или эмулятора: все — примитивы, большие числа, кодирование, константное время, обнуление памяти, служебные макросы (проверено по именам и назначению crate'ов).

**Лицензии транзитивных пакетов**, отличные от «MIT OR Apache-2.0»: `subtle` — BSD-3-Clause; `flagset` — Apache-2.0; `ff`, `group` — «MIT/Apache-2.0»; `unicode-ident` — «(MIT OR Apache-2.0) AND Unicode-3.0» (proc-macro, в runtime не входит). Политика [DEPENDENCIES.md](../policies/DEPENDENCIES.md) разрешает BSD-3-Clause и Apache-2.0; D9-лицензионный вопрос 5 закрыт 2026-11-18.

### ML-KEM-768 для host S7-23

Для примитива FIPS 203 выбран RustCrypto `ml-kem =0.3.3` из crates.io
с `default-features = false`, `zeroize` и `hazmat`. TPM получает свежие
`d`, `z` и `m` собственным HMAC_DRBG. Явная передача `m` позволяет
обработать исчерпание DRBG как ошибку TPM и воспроизвести ACVP-векторы.
Форматы объектов TPM, авторизация, транспорты и сохранение остаются здесь.
Лицензия Apache-2.0 OR MIT. [Пакет и функции](https://docs.rs/crate/ml-kem/0.3.3),
[FIPS 203](https://nvlpubs.nist.gov/nistpubs/FIPS/NIST.FIPS.203.pdf).

### ML-DSA-65 для host S7-23 (срез 7)

Для примитива FIPS 204 выбран RustCrypto `ml-dsa =0.1.2` из crates.io (индекс
crates.io 2026-11-20: последняя версия линейки 0.1, не отозвана, MSRV 1.87; 0.1.0
выпущена 2026-05-17) с `default-features = false` и features `alloc`, `zeroize`.
Линейка совместима с уже подключёнными `ml-kem =0.3.3`, `module-lattice 0.2.4`,
`hybrid-array 0.4.15`, `shake 0.1.0`, `signature 3.0.0` и `crypto-common 0.2.2`:
в `Cargo.lock` появился один новый пакет, `ml-dsa`. Feature `hazmat` не существует; нужные
вызовы (`sign_deterministic`, `sign_mu_deterministic`, `verify_with_context`,
`verify_mu`) входят в основной API. `alloc` держит расширенный ключ и сид в `Box` вместо
стека (расширенный ключ ML-DSA-65 занимает десятки КиБ); `getrandom`, `rand_core` и `pkcs8`
(features по умолчанию) не включены: TPM получает сид из собственного HMAC_DRBG. Подпись
детерминированная (вариант FIPS 204 с `rnd` из нулей). Форматы объектов TPM, авторизация,
транспорты и сохранение остаются здесь: закрытое значение объекта — 32-октетный сид ξ
(Part 2 v185, Table 211). Лицензия Apache-2.0 OR MIT. Контрольные ответы самопроверки —
NIST ACVP (ревизия `975de31e`), внешняя проверка — OpenSSL 4.1. [Пакет и
функции](https://docs.rs/crate/ml-dsa/0.1.2), [FIPS 204](https://nvlpubs.nist.gov/nistpubs/FIPS/NIST.FIPS.204.pdf).

### ECC P-384 для host S7-23 (срез 6)

Для кривой NIST P-384 выбран RustCrypto `p384 =0.14.0` из crates.io (индекс crates.io, версия не отозвана) с `default-features = false` и `ecdsa`: та же линейка и те же features, что у `p256 =0.14.0`. Назначение: ECDSA (подпись и проверка по дайджесту SHA-256/384/512), `TPM2_ECDH_ZGen`/`ECDH_KeyGen` (умножение точки) и ключи P-384 для `od-tpm`. Крейт использует те же общие пакеты, что и `p256`: `elliptic-curve 0.14.3`, `ecdsa 0.17.0`, `primefield 0.14.0`, `primeorder 0.14.0`; feature `ecdh` не включён (нужна точка `(x, y)`, а не только `x`). Лицензия Apache-2.0 OR MIT. Заявленная библиотекой константность арифметики нами не проверялась (см. «Требования к секретам»). Подробности по пакетам и транзитивным зависимостям — в [DEPENDENCIES.md](../policies/DEPENDENCIES.md) (S7-23, ECC P-384); firmware target не затронут. [Пакет](https://docs.rs/crate/p384/0.14.0).

### Условие D9 о constant-time `rsa`

По состоянию на 2026-11-18 RustSec RUSTSEC-2023-0071 указывает `Patched >=0.10.0` (advisory modified 2026-11-08). RustCrypto RSA v0.10.0 release notes от 2026-11-08 сообщают переход на `crypto-bigint` для constant-time modular exponentiation, hardening private-key operations по опубликованному guidance и constant-time validation harness. Это upstream evidence; compatibility-тесты проекта и их ограничения зафиксированы ниже, но они не являются полной constant-time security certification.

Выбранная версия — стабильная `rsa =0.10.0`. Host features: `default-features = false`, `pkcs1v15`, `hazmat` (текущие `od-tpm` команды требуют PKCS#1 v1.5 и raw RSA; OAEP не требует отдельного feature; энтропию предоставляет TPM DRBG). Firmware features: `default-features = false`, `pkcs1v15`, `encoding` (подпись и разбор RSA public key из PKCS#1). `getrandom` и `std` не включаются. Host `od-tpm` migration to stable `=0.10.0` completed on 2026-11-18 with the selected features; deterministic RSA golden/interoperability tests, all 264 `od-tpm` tests, workspace check and formatting passed. Independent review approved the migration slice on 2026-11-18; [ISSUE-064](../KNOWN_ISSUES.md#issue-064-s7-rsa-private-ops-not-constant-time) is resolved. Firmware-target integration remains S7-24. S7-23 implemented the selected version/features and verified existing TPM operations; this decision record does not claim independent constant-time certification.

Основание первоначального выбора `0.10.0-rc.22`, а не `0.9.10`:

1. `0.9` принадлежит прежнему поколению трейтов (`digest 0.10`, `pkcs1 0.7`, `spki 0.7`, `signature <2.3`) и потребовал бы второй набор семейства рядом с `sha2 0.11`/`der 0.8`/`x509-cert 0.3` (DRY, лишний транзитивный объём).
2. `0.10` построен на `crypto-bigint`, сопровождающие рекомендуют его, а PKCS#1 v1.5 там включается явным feature `pkcs1v15`.
3. На момент первоначального выбора стабильной ветки 0.10 ещё не было; тогдашний pin был временным для реализации S7-23. Выход стабильной 0.10.0 снял ограничение; миграция выполнена 2026-11-18 (см. актуальный статус выше).

Границы риска, установленные до миграции: в прошивке выполняются только операции с открытым ключом (проверка подписи). Host implementation now uses stable `rsa =0.10.0`; ISSUE-064 was resolved after independent migration review on 2026-11-18. Firmware-target integration remains S7-24.

### Требования к секретам (открыты для S7-23/S7-24)

[DEPENDENCIES.md](../policies/DEPENDENCIES.md) требует фиксировать ограничения runtime и способ проверки, но отдельного правила о constant-time и обнулении памяти не содержит; условие D9 касается только `rsa`. ADR задаёт требования, выполнение и проверку которых записывают S7-23 (host) и S7-24 (прошивка):

- seeds, приватные ключи, `authValue`, ключи сессий, состояние HMAC-DRBG хранятся в типах, обнуляющих память при уничтожении (`zeroize`, уже транзитивный); `Debug`/`Display` таких типов не печатают содержимое;
- сравнение HMAC, `authValue`, policy digest — за постоянное время (`subtle`/`ctutils` или проверка `hmac`), не `==` по срезам;
- ECDSA/ECDH — только через `p256` (NIST P-256) и `p384` (NIST P-384) (заявленная библиотекой константность на `crypto-bigint` нами не проверялась — записать в S7-23 как заявление поставщика, не как наш факт);
- прошивка секретов не хранит (D5: private key `PK` на host уничтожается); требования к ней ограничиваются отсутствием утечки через логи.

### Сборка в `no_std` прошивке (условие проверки, не результат)

Сборка под `x86_64-unknown-none` в S7-20 **не выполнялась**. S7-22 (`sha1`, `sha2`) и S7-24 (`rsa`, `der`, `x509-cert`, `spki`) при первом появлении crate'а в `firmware/uefi/Cargo.toml` обязаны:

1. Собрать прошивку командой из [firmware/uefi/README.md](../../firmware/uefi/README.md) с `--locked` после отдельного шага получения пакетов и записать результат.
2. Проверить выбор backend: target `x86_64-unknown-none` собирается без SSE (soft-float), а `sha1`/`sha2`/`aes` выбирают SIMD-backend через `cpufeatures`. Если сборка или исполнение с SIMD-backend невозможны, включить программный backend объявленными в crate'ах cfg (`sha1_backend = "soft"`, `sha2_backend = "soft"`; проверено по `check-cfg` в `Cargo.toml` этих версий) и записать это в README.
3. Подтвердить, что `alloc`-зависимые crate'ы (`rsa`, `x509-cert`) работают с аллокатором прошивки (S7-04).
4. Повторить проверку транзитивного графа `cargo tree` для firmware Cargo.lock и сверить с перечнем выше.

S7-06 подключил `sha2 =0.11.1` для Authenticode. Сборка firmware
`x86_64-unknown-none --locked --offline` прошла с
`sha2_backend="soft"` и `default-features = false`.
Граф активных транзитивных зависимостей зафиксирован в реестре.
[Реестр зависимостей](../policies/DEPENDENCIES.md#криптографические-библиотеки-vtpm-и-прошивки-s7-20).

## Решение 3. Профиль команд

- **Цель профиля S7-23** — обязательные требования PC Client PTP 1.07 от 2026-03-23 с применимыми уточнениями Errata 1.1 от 2026-08-27. Фактическая поддержка, условия применимости и пробелы фиксируются по таблицам 3, 4, 16 и разделам 4.6, 4.7, 4.10 и 6. Требования Windows проверяются отдельно по S7-30/S7-32. [PTP 1.07](https://medias.cdn.trustedcomputinggroup.org/production/PC_Client_Specific_Platform_TPM_Profile_for_TPM_2p0_v1p07_Pub_5643562943.pdf), [Errata 1.1](https://medias.cdn.trustedcomputinggroup.org/production/TCG_Errata_v1_1_for_PTP_1_07_pub_f1aca71996.pdf), [инвентарь PTP](../specs/VTPM.md#целевой-профиль-pc-client-ptp-107).
- **Фаза A (S7-21):** `Startup`/`Shutdown`, `SelfTest`/`GetTestResult`, `GetCapability`, `GetRandom`/`StirRandom`, PCR (`PCR_Read/Extend/Event/Reset`, `PCR_Allocate` — поддержка или отказ записывается), `Hash` и hash/event sequences, `ReadClock`; authorization area с password session `TPM_RS_PW`. Точный список и правила PCR — [S7-21](../plans/STAGE_7.md#s7-21-реализовать-процессор-команд-tpm-20-фаза-a) (DRY: не дублируется здесь).
- **Фаза B (S7-23):** иерархии Platform/Owner/Endorsement/Null с seeds и auth, RSA/ECC ключи (`CreatePrimary`, `Create`, `Load`, `Sign`, `RSA_Encrypt/Decrypt`, `ECDH_ZGen` …), HMAC- и policy-сессии с шифрованием параметров, NV, обязательные `ContextSave/Load`, `Quote`/`Certify`, DA, версионный persistent state и расширение до полного обязательного набора PTP — [S7-23](../plans/STAGE_7.md#s7-23-реализовать-tpm-20-фаза-b-иерархии-ключи-сессии-nv).
- **Криптографическая цель:** весь набор `M` таблиц 3 и 4, включая SHA-256/384/512, AES-128/256-CFB, HMAC, MGF1, XOR, RSASSA/PSS/OAEP, RSA-3072 (с поддержкой действующего RSA-2048), ECDSA/ECDH с NIST P-256 и P-384, ML-KEM и ML-DSA. Первые обязательные PQC parameter sets — ML-KEM-768 и ML-DSA-65. Остальные рекомендованные наборы оцениваются при проверке размеров, ресурсов и совместимости. SHA-1 имеет статус `N` в PTP 1.07 и исключён из целевого профиля TPM. `GetCapability` перечисляет только фактически реализованные алгоритмы, кривые и команды. Подключение библиотек P-384 и PQC сопровождается закреплением версии, features, источника и лицензии по политике проекта и отдельным ревью.
- **PCR и measured boot:** два включённых по умолчанию банка SHA-256 и SHA-384, по 24 PCR. Сохраняется совместимость SHA-256 с ранее проверенным путём и появляется SHA-384, требуемый PTP. `PCR_Allocate` должен штатно переключать набор банков по правилам §4.7, включая доступные комбинации поддерживаемых hash algorithms и переход на следующем `_TPM_Init`. Firmware публикует фактическую конфигурацию и измеряет каждый активный банк с согласованным replay лога. Текущий фиксированный банк SHA-256 и отказ `PCR_Allocate` описывают существующую реализацию.
- **Платформа и применимость:** CRB-only с `CapLocality=0` остаётся допустимым целевым вариантом. Таблица 16 требует платформенные `_TPM_INIT` и `_TPM_Hash_Start/Data/End`: их семантика, PCR17 и establishment проверяются через доверенный платформенный путь. Arbitration и Seize дополнительных software localities применяются при выборе `CapLocality=1`. Chunking — опциональный механизм, а транспорт обязан вмещать крупнейшие поддерживаемые команды и ответы. Firmware upgrade по §4.10 входит в цель. Для vTPM предполагается обновление через host lifecycle с проверкой миграции persistent state. Пара `FieldUpgradeStart/Data` обязательна при выборе Library-defined upgrade; выбран host lifecycle, см. [решение 6](#решение-6-обновление-прошивки-410). Предустановка EK-сертификатов включает отдельные требования §4.6.3 к комплекту сертификатов и остаётся открытой для Windows T-04. [Интерфейс CRB](#решение-4-интерфейс-crb-реализует-s7-22), [Windows EK](../KNOWN_ISSUES.md#issue-598).
- **Контракт команд:** любая команда вне реализованного профиля возвращает `TPM_RC_COMMAND_CODE`; команда до `Startup` возвращает `TPM_RC_INITIALIZE` там, где того требует Library. Кодирование `TPM_RC_*` соответствует Part 2 v185. Фактические ограничения, включая текущий ответ `EFI_UNSUPPORTED` на TCG2 `SetActivePcrBanks`/`GetResultOfSetActivePcrBanks`, описаны в [VTPM](../specs/VTPM.md). Закрытие команд и их семантики проверяется отдельно от списка кодов.
- **Соответствие и сертификация TCG не заявляются** (план S7-20 п. 5, [VIRTUAL_BOARD.md](../specs/VIRTUAL_BOARD.md)).

## Решение 4. Интерфейс CRB (реализует S7-22)

- Только CRB по PC Client PTP 1.07 Errata 1.1; FIFO/TIS не реализуется. В ACPI-таблице TPM2 `StartMethod = 7` (CRB); таблица публикуется только при `PRESENT = 1` поля 6 handoff block ([FIRMWARE_BOOT.md](../specs/FIRMWARE_BOOT.md), разделы 2 и 6).
- Окно `[0xFED4_0000, 0xFED4_5000)`: пять страниц 4 KiB, страница `n` — locality `n`. **Функциональна только locality 0.** Запрос других localities (`TPM_LOC_CTRL` requestAccess на страницах 1–4) не удовлетворяется: locality не назначается, команды через них не исполняются. Точные значения регистров страниц 1–4 при чтении и записи S7-22 записывает в `VTPM.md` («CRB»). PCR0 инициализируется startup locality 0 (S7-21).
- Прерывания не используются: `TPM_CRB_INT_ENABLE`/`INT_STS` не вызывают IRQ (S7-22 записывает поведение регистров); ACPI TPM2 прерываний не объявляет.
- Регистры страницы locality (смещения от её начала;по PTP 1.07 и errata; поведение в [VTPM.md](../specs/VTPM.md#crb)):

| Смещение | Регистр | Ширина |
|---|---|---|
| `0x00` | `TPM_LOC_STATE` | 32 |
| `0x08` | `TPM_LOC_CTRL` | 32 |
| `0x0C` | `TPM_LOC_STS` | 32 |
| `0x30` | `TPM_CRB_INTF_ID` (тип интерфейса CRB, возможности; по нему ОС выбирает драйверный путь) | 64 |
| `0x38` | Reserved (PTP 1.07 Table 36), RAZ/WI | 64 |
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

- **Буферы:** единый device-resident буфер команды и ответа в locality 0. `CMD_SIZE = RSP_SIZE = 0xF80`, `CMD_ADDR = RSP_ADDR = 0xFED4_0080`. Физическое окно содержит 3968 байт. CRB revision 3 и `CapCRBChunk` передают кадры до 8192 байт. Предел покрывает рассчитанный ответ `Certify` в 4031 байт и `NV_Certify` в 4991 байт для ML-DSA-65, SHA-512 и трёх сессий. Значение объявляется через свойства TPM и EFI TCG2. [Контракт CRB](../specs/VTPM.md#crb).
- **Исполнение:** `nextChunk` накапливает промежуточные части команды. Запись START синхронно передаёт собранный кадр в `od-tpm`, помещает первый чанк ответа в буфер и завершает START. В Completion ответ выдаётся по чанкам и доступен для повтора чтения. Ошибка гостевого кадра или команды кодируется в TPM_RC. Время команды берётся из виртуальных часов платы.

## Решение 5. Состояние

Формат записывается в [VTPM.md](../specs/VTPM.md): сейчас там только раздел «Состояние и границы» со ссылкой сюда; «Состояние» фазы A — S7-21, «Persistent state v1» — S7-23.

- **Persistent state v1** (полностью — S7-23; S7-21 заводит версионный каркас с seeds и счётчиками): seeds иерархий (platform, storage/owner, endorsement); `authValue`, `authPolicy` и флаги включения иерархий в части, которую Part 1 относит к постоянной (разделение по `Startup(CLEAR)` — **не сверено**, S7-23); NV-индексы (public area и данные); persistent objects (`EvictControl`); счётчики (NV counters, `resetCount`/`restartCount`, сохраняемая часть clock); состояние DA (`failedTries`, параметры lockout). Предложение архитектора (открытый вопрос 6): PCR-политика (`PCR_SetAuthPolicy`/`PCR_SetAuthValue`) в v1 не входит, эти команды не поддерживаются.
- **Volatile** (сбрасывается по `Startup(CLEAR)` и при пересоздании процесса): значения PCR, сессии, transient objects, загруженные контексты, hash/event sequences, назначенная locality, признак выполненного `Startup`. Поддержка `Shutdown(STATE)`/`Startup(STATE)` (сохранение части volatile) — решение S7-21 с записью в `VTPM.md`; до него `Startup(STATE)` без сохранённого состояния возвращает код ошибки по Part 3, не `SUCCESS`.
- **DRBG:** состояние HMAC-DRBG в persistent state не входит; при создании и при `restore` владелец передаёт свежую энтропию host, чтобы восстановленный TPM не повторял выход генератора.
- **Формат:** версионные байты собственного кодека `od-tpm`; неизвестная или повреждённая версия → `restore` отвергается, TPM не стартует (не «пустой TPM молча»). Байты содержат секреты в открытом виде и выдаются владельцу машины через `od-machine` public API с этой пометкой в контракте; шифрование и хранение — S9-27.
- **Никогда не логируются** и не попадают в отчёты, эталоны COM1 и сообщения ошибок: seeds, приватные части ключей, `authValue`, ключи и nonce сессий, состояние и выход DRBG (включая ответ `GetRandom` — печатается только длина), данные NV и `Unseal`. Допустимы handles, коды `TPM_RC`, размеры и публичные области.

## Решение 6. Обновление прошивки (§4.10)

Номер 6, а не 4: номер 4 занят интерфейсом CRB, и якоря существующих ссылок не меняются. Решение принято 2026-11-20 для S7-23.

- **Требование.** PTP 1.07 §4.10, пп. 1–3: TPM SHALL реализовать firmware upgrade. Вариант, определённый в Library, разрешён (MAY). Только при этом варианте обязательна пара `TPM2_FieldUpgradeStart`/`TPM2_FieldUpgradeData`, а `TPM2_FirmwareRead` остаётся MAY. В таблице 16 `FieldUpgradeStart` и `FieldUpgradeData` имеют статус `O` с оговоркой «Both commands are required if either is implemented», `FirmwareRead` тоже `O`. Errata 1.1 не меняет ни §4.10, ни эти строки. Текст PTP и Errata сверен по PDF (`pdftotext`, 2026-11-20). [PTP 1.07](https://medias.cdn.trustedcomputinggroup.org/production/PC_Client_Specific_Platform_TPM_Profile_for_TPM_2p0_v1p07_Pub_5643562943.pdf)
- **Решение: вариант A, host lifecycle.** Прошивка vTPM — это код `od-tpm` в сборке host-процесса. Обновить её — значит заменить сборку на стороне владельца машины. Постоянное состояние переходит в новую сборку через снимок `ODTPMSTA`. Новая сборка читает любую версию из диапазона `MIN_SNAPSHOT_VERSION..=SNAPSHOT_VERSION` (сейчас v3…v8), заполняет отсутствующие поля по правилам этой версии (`crates/od-tpm/src/snapshot.rs`, `state.rs`) и записывает только текущую версию. Снимок вне диапазона (v1, v2 или версия новее сборки) получает `StateError::UnsupportedVersion`, и TPM не стартует: подмена пустым TPM запрещена (решение 5). Volatile state при обновлении сбрасывается, как при `_TPM_Init`.
- **Команды.** `FieldUpgradeStart` (`0x0000012F`), `FieldUpgradeData` (`0x00000141`) и `FirmwareRead` (`0x00000179`) не реализуются. В `TPM_CAP_COMMANDS` их нет, ответ на них — `TPM_RC_COMMAND_CODE` (контракт решения 3). Field Upgrade mode и `TPM_RC_UPGRADE` не используются.
- **Безопасность.** У гостя нет пути заменить код TPM: обновление ставит только владелец host-установки, тем же каналом, что и весь Open Desktop. Доверие к дистрибутиву и его подпись лежат вне `od-tpm`. Вариант Library (Part 3 v185 §27.2) требует `platformPolicy` и подписи манифеста ключом TPM Vendor Authorization Key, Name которого входит в данные прошивки. Ключа производителя и инфраструктуры подписи образов у проекта нет, а обновление без проверенной подписи недопустимо. Откат на старую сборку не портит состояние молча: старая сборка отвергает снимок более новой версии.
- **Альтернатива B отклонена.** Вариант Library включает FUM, цепочку дайджестов блоков, `TPM_RC_UPGRADE` для прочих команд, ключ производителя и загрузку кода в работающий процесс. Объём и риски этого не оправдывают, а приём исполняемого образа от гостя внутрь host-процесса нарушает модель изоляции. Пересмотр возможен, если vTPM появится вне host-процесса или если Windows либо сертификация потребуют этих команд.
- **Последствия.** Любое изменение формата снимка повышает `SNAPSHOT_VERSION`, и сборка продолжает читать все версии начиная с `MIN_SNAPSHOT_VERSION`. Повышение `MIN_SNAPSHOT_VERSION` — отдельное решение с записью в [VTPM](../specs/VTPM.md). Каждая читаемая версия покрывается тестом восстановления и повторного сохранения. `TPM_PT_FIRMWARE_VERSION_1/2` в `GetCapability` и `firmwareVersion` в аттестации берутся из одного источника.
- **NOT VERIFIED.** (1) SHOULD п. 4 о resilience and recovery по NIST SP 800-193 §4.4: сохранение предыдущего снимка до перезаписи и откат относятся к хранению (S9-27) и не проверены. (2) Поведение Windows без `FieldUpgrade*` и при смене `firmwareVersion` проверяется отдельно (S7-30/S7-32). (3) Правило изменения `TPM_PT_FIRMWARE_VERSION_*` при выпуске не определено. (4) Не проверено, переживают ли смену сборки контексты (`ContextSave`) и билеты, сохранённые гостем: их формат не версионируется снимком.

## Решение 7. Приёмка S7-23: открытые пункты

Решение принято 2026-11-20 при приёмке S7-23 (пункт «Готово» плана). Правило: пункт, проверяемый на host, закрывается в S7-23. Сюда входит и гостевая UEFI-прошивка в собственной машине на macOS. `blocked` допустим только для того, что наблюдается лишь в Windows-госте или требует решения владельца.

| № | Пункт | Решение | Основание и что сделать |
|---|---|---|---|
| 1 | Ответ с подписью ML-DSA-65 через устройство CRB и гостя | (а) закрыть в S7-23 | `TpmCrb` (`crates/od-machine/src/tpm_crb.rs`) тестируется на host, EFI-приложение `tpm` запускает `tests/system/firmware_tpm_smoke.py` тоже на host. Host-тест устройства: `NV_Certify` на 512 октетов ключом ML-DSA-65 даёт ответ больше окна 3968 байт и читается по страницам. `VerifySequenceComplete` с подписью 3309 октетов (команда 3348 октетов) помещается в окно 3968 и уходит одним кадром; чанками передаются только команды больше окна. Байты ответов совпадают с прямым вызовом `od-tpm`. Гостевой сценарий выполняет тот же обмен через `EFI_TCG2_PROTOCOL.SubmitCommand`, получает билет `MESSAGE_VERIFIED` и `TPM_RC_SIGNATURE` на искажённой подписи. COM1 печатает только размеры и коды. |
| 2 | Гостевой прогон CRB/TCG2 на целевом профиле (абзац Errata 1.1 в VTPM) | (а) закрыть в S7-23 | Целевой профиль — PTP 1.07 по решению 3, а не Windows. `firmware_tpm_smoke.py` и measured boot (`firmware_boot_smoke.py`) прогоняются на сборке полного профиля. Гость через `GetCapability` видит 101 команду, `TPM_ALG_MLDSA` и `TPM_ALG_MLKEM`, кривые P-256 и P-384, банки SHA-256 и SHA-384. Сценарий п. 1 входит в прогон. Результат записывается в отчёт этапа 7. Windows-гость — S7-32. |
| 3 | Свойства Table 2 | (а) для `TPM_PT_MEMORY` и `TPM_PT_ALGORITHM_SET`; (в) для нулевых значений | PTP §4.2 п. 2 требует перечислять все определённые свойства. Оба свойства добавляются со значениями, выведенными из реализации (раздельные пулы сессий, объектов, persistent-объектов и NV). Тест сверяет полный перечень `TPM_PT` Part 2 v185 Table 23 с ответом `GetCapability`. Нули `SPLIT_MAX`, `MODES`, `NV_WRITE_RECOVERY`, `AUDIT_COUNTER_0/1` — фактические значения, а не заглушки. `TPM2_Commit` вне обязательного набора и не реализована. FIPS-режим и сертификация не заявляются (решение 3). NV — память процесса и снимок host, задержки восстановления после записи нет. `TPM2_SetCommandCodeAuditStatus` не реализована, счётчик аудита команд не растёт. `CLOCK_UPDATE` = 0 — принятое отклонение: у TPM нет собственной NV, копию `Clock` фиксирует снимок host. S7-33 передаёт S9-27 требование писать снимок не реже нормы Part 1 и публиковать этот интервал вместо 0. Обоснование переносится в VTPM.md, раздел «Отклонения». |
| 4 | Part 1 v185 §44.6 (дополнительные случайные биты P-256) и §11.5 (размер proof) | (а) сверить в S7-23 | Текст Part 1 v185 опубликован TCG, сверка выполняется на host. §44.6: если допустимы оба метода, записать соответствие текущего rejection sampling. Если обязателен extra random bits, перевести на `ecc_generate_extra_random_bits` недетерминированные пути P-256 и P-384 (`Create`, `ECDH_KeyGen`, временные ключи). Вывод Primary не меняется: это vendor option (Part 1 §25.6.3), а смена изменила бы EK и SRK. Это записывается отдельно. §11.5: если размер proof задан как SHALL, исправить в S7-23 с новой версией снимка и тестом миграции. Если это рекомендация, принять отклонение (в) с записью влияния на билеты и контексты (см. NOT VERIFIED (4) решения 6). |
| 5 | `TPM_PT_HR_TRANSIENT_AVAIL` и лимит двух ML-DSA | (а) закрыть в S7-23 | `MAX_LOADED_MLDSA` = 2 прочитан из PTP Table 2 как «ровно два», а таблица требует «не менее двух». Памяти процесса лимит не экономит. Лимит снимается, ML-DSA ограничен общими слотами `MAX_OBJECTS`. `HR_TRANSIENT_AVAIL` не превышает свободных слотов ни одного пула (объекты, sequence): значение ≥ 1 должно гарантировать загрузку объекта любого типа (Part 2 Table 23, текст сверить). |
| 6 | Трасса TBS, производственная цепочка EK, совместимость ContextSave/Load с TBS | (б) blocked | Эти пункты наблюдаются только в Windows-госте или требуют решения владельца. Host-часть закрыта в S7-23. Трасса TBS и `ContextSave`/`ContextLoad` под менеджером ресурсов TBS — S7-32 (WinPE и Setup). Provisioning TPM, BitLocker, Windows Hello и attestation — этап 8 по roadmap. Производственная цепочка EK ждёт решения владельца об издателе ([ISSUE-598](../KNOWN_ISSUES.md#issue-598)). Запись сертификата при создании VM — S9-26, доверие Windows — S7-32 и этап 8. |

- **Остаётся для S7-32:** трасса команд TBS в WinPE и Setup, сверенная с `TPM_CAP_COMMANDS`. Поведение TBS с `ContextSave`/`ContextLoad`: виртуализация handles, окно `contextID`, `TPM_RC_CONTEXT_GAP`. Видимость TPM в `tpm.msc` или логе Setup. Запрашивает ли Setup EK-сертификат. Каждый пункт отмечается «наблюдалось/не наблюдалось».
- **Отметка S7-23.** Чекбокс ставится после выполнения пунктов 1–5 и независимого ревью. Пункт 6 отметке не мешает: «Готово» допускает Windows-зависимое как blocked.

## Последствия

- [PROJECT_LAYOUT.md](../PROJECT_LAYOUT.md) получает владельца `crates/od-tpm/` и стрелку `od-machine → od-tpm`; машиночитаемый перечень каталогов дополняется вместе с созданием crate в S7-21 (иначе `check_layout.py` даст ошибку отсутствующего каталога). [ARCHITECTURE §9](../ARCHITECTURE.md) перечисляет crate'ы каркаса; строку `od-tpm` вносит S7-21 вместе с crate (§9 допускает новые crate'ы при сформировавшейся границе).
- В host `Cargo.lock` появятся внешние пакеты `od-tpm`; их число не замерено (оценка по объединённому набору — 52 пакета вместе с 11 прямыми, оценка, не замерено; host-набор `od-tpm` может быть меньше); получение пакетов — отдельный шаг, проверки — `--locked --offline` ([DEPENDENCIES.md](../policies/DEPENDENCIES.md)). В workspace будут две версии `sha2` (`0.10.9` в test-only `od-testkit` и `0.11.1` в `od-tpm`); объединение не обязательно и не входит в S7-21.
- Прямая зависимость `zeroize =1.9.1` (`default-features = false`, Apache-2.0 OR MIT) подключена в `crates/od-tpm` для стирания секретов (`authValue`, seed'ы, состояние DRBG), хотя в исходном ADR она была только транзитивной; обоснование — требование ADR хранить секреты в zeroize-типах (раздел «Требования к секретам»); записано по процедуре [DEPENDENCIES.md](../policies/DEPENDENCIES.md). В S7-23 (срез A) для зависимости добавлен feature `alloc` (стирание `Vec<u8>`: данные NV-индексов, ключи сессий); новых пакетов он не добавляет.
- Срез B S7-23 (2026-11-07) подключил `rsa =0.10.0-rc.22` (`default-features = false`, `pkcs1v15`) и `p256 =0.14.0` (`default-features = false`, `ecdsa`; `ecdh` включит срез C для `ECDH_ZGen`; **срез C1 его не включил** — `ECDH_ZGen` возвращает точку, а не только `x`, и считается умножением `p256` (`arithmetic`), а к `rsa` добавлен feature `hazmat` для «сырого» RSA, см. [DEPENDENCIES.md](../policies/DEPENDENCIES.md)); `ecdsa 0.17.0` и `der 0.8.3` пришли транзитивно, `der`/`spki` как прямые зависимости host-стороны не нужны. `aes =0.9.4` и `cfb-mode =0.9.1` (`default-features = false`, `zeroize`) срез B подключил после ревью: шифрование `outPrivate` и симметричных объектов — AES-128-CFB (подробно в [VTPM.md](../specs/VTPM.md#фаза-b-срез-b-объекты-и-ключи-s7-23)).
- Прошивка получает криптографию в два шага: `sha1`/`sha2` в S7-22, `rsa`/`der`/`x509-cert`/`spki` в S7-24 — каждый с проверкой сборки по условию выше. Оставлять криптографию прошивки «на потом» нельзя: перечень зафиксирован здесь.
- Связь с задачами: S7-21 и S7-23 — `od-tpm`; S7-22 — CRB, ACPI TPM2, TCG2, measured boot (PCR0–5) поверх этого профиля; S7-24 — Secure Boot и PCR7 поверх firmware-набора; S7-26 — persistent state переживает сброс, volatile сбрасывается; S7-33 передаёт формат этапу 9; S9-27 — защита ключей и хранение байтов в bundle. Строку «vTPM persistent state» в [STORAGE_FORMAT.md](../specs/STORAGE_FORMAT.md) («state — `od-machine`») уточняет S7-33: процессор и формат — `od-tpm`, выдача — `od-machine`.
- **Риски на момент подготовки ADR:** (1) на момент подготовки ADR приватные RSA-операции тогдашней реализации `rc.22` не были подтверждены как constant-time; host migration to stable `=0.10.0` and compatibility checks completed 2026-11-18, with independent review approved 2026-11-18 ([ISSUE-064](../KNOWN_ISSUES.md#issue-064-s7-rsa-private-ops-not-constant-time)); (2) предрелизный `rsa` — дрейф API между `rc`; (3) сборка SIMD-backend'ов под soft-float target не проверена; (4) профиль может оказаться недостаточным для Windows до S7-30 (провижининг TPM, BitLocker); (5) секреты в выдаваемых байтах state не защищены до S9-27; (6) буфер CRB 3968 байт может оказаться меньше требуемого.

## Открытые вопросы (для S7-21…S7-24)

1. **S7-21:** сверить с Library v185 допустимость HMAC_DRBG и редакцию SP 800-90A; точные сигнатуры API `od-tpm`; поддержка `Shutdown(STATE)`/`Startup(STATE)` и `PCR_Allocate`; `TPM_PT_MAX_COMMAND_SIZE`/`MAX_RESPONSE_SIZE` ≤ 3968; таблица атрибутов PCR.
2. **S7-22:** сверить раскладку регистров CRB, `TPM_CRB_INTF_ID` и минимальный размер буфера с PTP 1.07 Errata 1.1; синхронный или отложенный вызов процессора; поведение страниц localities 1–4; сборка `sha1`/`sha2` под `x86_64-unknown-none` и выбор backend.
3. **S7-23:** разделение persistent/volatile для auth и флагов иерархий по Part 1; перечень команд по S7-30; выполнение и проверка требований к секретам (обнуление, сравнение за постоянное время); какой библиотекой проверять подпись в тесте «создать ключ → подписать → проверить» (тот же `rsa` не является независимой проверкой — выбрать и записать).
4. **S7-24:** сборка `rsa`/`der`/`x509-cert`/`spki` в прошивке и политика SHA-1 подписей (план S7-24 п. 2). `rsa/encoding` выбран в таблице выше для разбора RSA public key в PKCS#1; S7-24 проверяет интеграцию на firmware target.
6. **S7-30 и S7-21:** сужение профиля — `PCR_SetAuthPolicy` и `PCR_SetAuthValue` не поддерживаются, PCR-политика не входит в persistent state v1 (решение 5). Это предложение архитектора, не требование плана: S7-30 подтверждает, что Windows (провижининг TPM, BitLocker) обходится без этих команд, S7-21 записывает итог в `VTPM.md` и в `GetCapability(TPM_CAP_COMMANDS)`; при необходимости профиль и persistent state расширяются.

Вопрос 5 закрыт 2026-11-18: политика [DEPENDENCIES.md](../policies/DEPENDENCIES.md) разрешает BSD-3-Clause и Apache-2.0; лицензии `subtle` и `flagset` допустимы.

## Условие пересмотра

- S7-23 host migration to stable `rsa =0.10.0` with the features above is complete; API/feature compatibility and existing calls passed verification. Independent review approved the migration slice on 2026-11-18 and ISSUE-064 is resolved; any additional security validation remains bounded by the caveats above. Upstream advisory and release notes alone do not close the issue.
- S7-30 требует алгоритмов или команд вне профиля v1 (RSA-3072, P-384, иной буфер CRB) — дополнить ADR и зависимости.
- S9-27 требует изоляции секретов на уровне процесса — пересмотреть альтернативу 2 решения 1.
- Сборка firmware-набора под `x86_64-unknown-none` невозможна ни с одним backend — пересмотреть firmware-часть решения 2 новым решением владельца.
