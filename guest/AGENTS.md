# Код, выполняемый внутри гостя

- `windows/` — только Windows guest agent/drivers. `linux/` — только Linux guest agent/drivers.
- `agent/` — userspace integration; `drivers/` — guest-target drivers; `tests/` — проверки гостевых компонентов.
- Host backend устройства принадлежит `crates/od-machine/` и/или macOS-оболочке. Не размещать его здесь из-за сходства названий.
- Версионированные сообщения host ↔ guest описывать в `protocols/guest/`. Внутренние заголовки конкретного driver остаются с driver.
- Guest-компоненты имеют отдельные target/build. Не включать их в host Cargo workspace.
- Не копировать сторонние drivers и guest tools. Штатный SDK/WDK разрешён; внешние реализации не становятся собственными от переименования.
