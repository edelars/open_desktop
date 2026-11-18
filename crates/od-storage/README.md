# od-storage

Хранение дисков и сериализация состояния.

RAW, собственный CoW v1 со снимками и (будущий) формат контейнера снимка VM. Не управляет vCPU: согласованную остановку и сбор состояния организует od-machine. Rust layout не является дисковым форматом.

Статус (2026-11-18): RAW и I/O contract в обычной сборке; CoW v1 (нормативен с 2026-11-06, `docs/specs/STORAGE_FORMAT.md` §9), ненормативный слой snapshots, inspect, export/import и salvage — за feature `test-support`. Подробности — `docs/reports/STAGE_9_REPORT.md`. Читать ../AGENTS.md и ../../docs/PROJECT_LAYOUT.md перед реализацией. Зависимости добавлять вместе с фактическим использованием.
