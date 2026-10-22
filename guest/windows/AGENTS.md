# Windows guest

- Следовать корневым правилам и `guest/AGENTS.md`.
- `agent/` — C++20/Win32 userspace. `drivers/display/` — собственный дисплей/WDDM; `drivers/devices/` — прочие собственные виртуальные устройства.
- Kernel drivers начинать на C/WDK. Не выбирать новый kernel runtime или язык только ради единообразия с Rust host.
- Userspace agent не получает kernel-привилегии без необходимости. Не смешивать agent и driver в одном модуле.
- Установка неподписанного тестового driver не доказывает пригодность production-конфигурации.
