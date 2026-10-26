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
своего verified fragment entry. Много одновременно живых code blocks требуют
отдельного bounded-slot дизайна внутри одной JIT region; это не реализовано.

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

Для будущего block runtime `od-host` остаётся владельцем executable mapping:
mapping должен жить на всём протяжении generated call. `od-jit` описывает
runtime tags и same-build offsets, но не зависит от `od-host` и не владеет
публикацией кода. Текущий `CompiledFragment` по-прежнему выполняет только
legacy `FragmentState` path.
