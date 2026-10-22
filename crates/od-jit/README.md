# od-jit

Собственный IR и трансляция x86 в ARM64.

Lowering, allocator, emitter, dispatcher, code cache и карты состояния. Контракт гостевых memory accesses задаётся явно; сюда не переносить MMU, устройства и macOS UI.

Статус на 22 октября 2026 года: реализованы собственная ARM64 генерация и исполнение ограниченного S0-09 профиля. Arithmetic и control flow исполняются как native ARM64; host helper материализует x86 CF/PF/AF/ZF/SF/OF. Исполняемую память предоставляет `od-host`. `cargo test -p od-jit --offline` сравнивает native JIT с interpreter для арифметики, SUB/JNZ-цикла и CMP/JZ, а также проверяет RIP и budget. Полная ISA, оптимизации и performance claims не входят в этот срез.
