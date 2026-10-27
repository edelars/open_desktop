# od-host

Системные host-примитивы и входная граница движка.

macOS JIT memory, host I/O и glue для публичного C ABI. Верхний слой собирает machine из библиотек; нижние библиотеки не зависят от od-host. Swift UI остаётся в apps/macos.

S3-07 добавил `PublishedCode::publish` для macOS ARM64: publisher создаёт приватный
`MAP_JIT` mapping, временно открывает JIT write access на текущем потоке, копирует
байты, закрывает write access, синхронизирует instruction cache и только затем
возвращает owning handle. Drop освобождает mapping через `munmap`. В одном
процессе `od-host` допускает только один одновременно живой published mapping;
вторая попытка получает `MappingAlreadyLive`, а C ABI возвращает существующий
`OD_STATUS_BUSY`. После Drop slot можно использовать повторно. Другие платформы
возвращают `UnsupportedPlatform`.

`pthread_jit_write_protect_np` действует на текущий поток. Publisher не меняет
код после публикации, но это не задаёт process-wide write-protection для других
JIT-пользователей процесса. API не позволяет безопасно вызвать произвольный
`PublishedCode`; legacy `CompiledFragment` сохраняет владеющий mapping на время
своего verified fragment entry. S3-18b добавляет host-only
`PublishedArenaBackend`: одна MAP_JIT region с 1..=16 фиксированными 4096-byte
slots и отдельными owning handles. Это не меняет release-on-return callbacks и
не подключено к multi-entry `od-jit` cache. Capacity 16 — предварительная
ограниченная конфигурация без performance evidence; concurrency/global
write-protection contract не расширен.

Локальная проверка S3-07 выполнена на macOS 26.7.1 ARM64, Xcode/macOS SDK 27.0:
`od-host` unit tests, подписанный ad-hoc VMHost smoke с Hardened Runtime и
`com.apple.security.cs.allow-jit`, и запуск JIT fragment через launcher. SDK
объявляет `pthread_jit_write_protect_np` доступным с macOS 11.0; Rust unit-test
binary имеет `LC_BUILD_VERSION minos 11.0`. Подписанный Swift smoke binary имеет
`minos 26.0`, поскольку команда сборки не задаёт deployment target. Репозиторий
не задаёт явный deployment target для конечного приложения; другой Mac не проверен.
Developer ID/notarization, packaged application и полный GUI lifecycle остаются
непроверенными; см. [VMHost smoke](../../apps/vmhost/README.md),
[стенд](../../docs/testing/STAND.md) и
[ISSUE-004](../../docs/KNOWN_ISSUES.md#issue-004--s0-08-developer-id-distribution-и-проверка-конечного-bundle).

S3-08 закрепил host-side разбор `BlockEntry` result: helper status 0 может
завершиться raw `RuntimeExitTag`; любой ненулевой status генерирует отдельный
`RAW_HELPER_CONTRACT_FAILURE_V1`, который `decode_block_entry_result` возвращает
как host error, не guest exit. Тестовый generated probe проверяет ARM64 frame
chain, сохранение x19-x29/LR/SP, D8-D15 canaries и helper output staging; helper
делает nested host call и намеренно пишет в staging даже при ошибке, чтобы
проверить, что entry не читает/не публикует output и не retire-ит guest state на
failure. Native tests прошли как ad-hoc подписанный Hardened Runtime процесс с
`com.apple.security.cs.allow-jit` на macOS 26.7.1 ARM64. Это ABI probe, не block
compiler/dispatcher, не arithmetic helper semantics и не универсальная safe
обёртка для произвольного entry pointer. Raw invocation остаётся unsafe;
`PublishedCode` owner удерживается до конца вызова.

Для будущего block runtime `od-host` остаётся владельцем executable mapping:
mapping должен жить на всём протяжении generated call. `od-jit` описывает
runtime tags и same-build offsets, но не зависит от `od-host` и не владеет
публикацией кода. Текущий `CompiledFragment` по-прежнему выполняет только
legacy `FragmentState` path.

S3-12 adds `PublishedCode::publish_block` and a safe `execute_block` wrapper for
production-emitted register-only blocks. The mapping owner retains immutable
`PcMap` metadata for its full lifetime; entry RIP and production-helper
provenance are checked before the generated call. Runtime tests cover planned
STOP/budget/stop-RIP exits, JZ/JNZ, committed prefixes and helper-failure
attribution. The map does not recover arbitrary host faults, panics, ABI
violations or asynchronous snapshots. Independent review found no remaining
concrete defects; the primary-checkout layout check remains with the root task.
See the JIT contract and stage plan for exact scope/evidence.

S3-18b uses the same `MapJitLease` and verified entry/result decoder for a
bounded fixed-slot arena. A live arena blocks standalone/another MAP_JIT
publication until its final backend/slot handle drops. Tests exercise two
resident blocks, independent execution, dropped-slot reuse, full-capacity
recovery, foreign-handle rejection and mapping-owner lifetime. Native tests
passed in the local macOS ARM64 Cargo test run. These tests do not constitute a separate
Hardened Runtime/allow-jit signed arena smoke. Slot count remains provisional;
no performance claim is made.
