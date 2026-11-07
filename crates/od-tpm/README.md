# od-tpm

Процессор команд vTPM 2.0: чистая логика без шины, MMIO, потоков, файлов, ОС-часов и ОС-энтропии (решение D8, [ADR-0009](../../docs/decisions/ADR-0009-vtpm-placement-and-crypto.md)).

Принимает байты команды, locality и время от владельца, возвращает байты ответа; ошибка гостевой команды — ответ с `TPM_RC`, не host-ошибка. Состояние делится на persistent (кодек snapshot `ODTPMSTA` v1) и volatile. CRB-устройство и выдача байтов state host-стороне — `od-machine` (S7-22).

Статус: фаза A (S7-21) — Startup/Shutdown, SelfTest, GetCapability, GetRandom/StirRandom, PCR (банк SHA-256, 24 PCR), Hash и hash/event sequences, ReadClock, password session. Фаза B, срез A (S7-23) — иерархии (Platform/Owner/Endorsement/Null, Lockout-авторизация): HierarchyControl/ChangeAuth, Clear/ClearControl; HMAC-сессии: StartAuthSession (unbound/bound к иерархии), FlushContext; база NV: DefineSpace/UndefineSpace/ReadPublic/Write/Read/WriteLock/ReadLock. Всё остальное отвечает `TPM_RC_COMMAND_CODE`. Срезы B (ключи RSA/ECC, policy, шифрование параметров) и C (persistent state v1, DA, EK, контексты) — S7-23, не начаты. Контракт, границы и список не сверенных мест — [docs/specs/VTPM.md](../../docs/specs/VTPM.md). Соответствие и сертификация TCG не заявляются.

Читать ../../AGENTS.md и ../../docs/PROJECT_LAYOUT.md перед изменениями. Зависимости добавлять только по ADR-0009.
