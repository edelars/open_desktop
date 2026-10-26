# od-host

Системные host-примитивы и входная граница движка.

macOS JIT memory, host I/O и glue для публичного C ABI. Верхний слой собирает machine из библиотек; нижние библиотеки не зависят от od-host. Swift UI остаётся в apps/macos.

Статус на 22 октября 2026 года: реализованы MAP_JIT-память на macOS ARM64, синхронизация instruction cache, `rlib`/`staticlib` и C ABI v0 для create/execute/request-stop/destroy. `CompiledFragment` отдельно позволяет выполнить один раз скомпилированный фрагмент повторно, сохраняя его executable mapping. На основном M3 Max JIT сравнен с interpreter и запущен через отдельные C и Swift smoke programs, слинкованные с `libod_host.a`; Swift VMHost подписан ad-hoc с Hardened Runtime и allow-jit entitlement. Launcher, IPC, cooperative stop и узкая изоляция двух VMHost проверены; детали — в [VMHost smoke](../../apps/vmhost/README.md) и [стенде](../../docs/testing/STAND.md). Developer ID/notarization, packaged application, полный GUI lifecycle и другой Mac не проверены. На других платформах API исполнения возвращает `UnsupportedPlatform`.

Для будущего block runtime `od-host` остаётся владельцем executable mapping:
mapping должен жить на всём протяжении generated call. `od-jit` описывает
runtime tags и same-build offsets, но не зависит от `od-host` и не владеет
публикацией кода. Текущий `CompiledFragment` по-прежнему выполняет только
legacy `FragmentState` path.
