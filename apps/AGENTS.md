# macOS-приложения

Наследовать корневой `AGENTS.md` и карту `docs/PROJECT_LAYOUT.md`.

- `macos/Sources/` — Swift UI, библиотека машин, окна, команды и presentation. `Resources/` — ресурсы приложения; `Configuration/` — entitlements и конфигурация сборки; `Tests/` — локальные проверки UI/управления.
- `cli/Sources/` — CLI управления машинами (`odctl`), оболочка macOS-продукта на том же контракте control, что и приложение; без listener и демона. Каталог создаётся в S11-13; не создавать его раньше задачи ([ADR-0006](../docs/decisions/ADR-0006-desktop-app-shell.md)). Общий Swift-код приложения и CLI — `macos/Sources/Shared/` (цель `ODShared` без `libod_host.a`).
- `vmhost/Sources/` — оболочка отдельного VM-процесса: lifecycle, системные ресурсы, IPC и мост к Rust. `Configuration/` — конфигурация helper; `Tests/` — проверки оболочки.
- Один VMHost владеет одной машиной. Приложение не исполняет CPU и не пишет напрямую в диск работающей VM.
- CPU/JIT/MMU, модель устройства и формат диска размещать в соответствующем Rust-компоненте. Здесь находятся только platform integration и UI.
- C ABI host-продукта описывать в `include/open_desktop/`; wire protocol — в `protocols/control/`. Не создавать отдельные противоречащие копии контрактов в Swift и Rust.
- Не вызывать Swift/UI/IPC на каждой гостевой инструкции или обычном доступе к RAM. Кадры и аудио не отправлять как большие сериализованные control-сообщения.
- Не переносить гостевой код в host-приложение. JIT entitlements нужны только тем процессам, которые фактически исполняют JIT.
- Способ сборки (Swift Package `apps/Package.swift`, скрипт сборки `.app`, цели) и размещение CLI определяет [ADR-0006](../docs/decisions/ADR-0006-desktop-app-shell.md) (статус: предложен); пакет `apps/Package.swift` создан в S11-05a: `swift build` и `swift test` из `apps/`. Не подменять его пустым проектом, который якобы собирает отсутствующее приложение.
