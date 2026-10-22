# Linux guest

- Следовать корневым правилам и `guest/AGENTS.md`.
- `agent/` — собственные userspace-инструменты на C/Rust; `drivers/` — только необходимые guest kernel-компоненты на C.
- Сначала определить, достаточно ли штатных guest drivers выбранной виртуальной платформы; не писать новый kernel-модуль без реального требования.
- Не помещать Linux host tooling и x86 oracle runner в guest agent. Физический oracle — в `tools/x86-runner/`.
