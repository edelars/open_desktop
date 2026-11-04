# od-machine

Системная модель гостевой машины.

Гостевая RAM/MMU/TLB, bus, устройства, vCPU scheduler и виртуальное время. Использует нижние контракты CPU/JIT и storage; host-примитивы получает через узкие интерфейсы.

Статус: локальный срез S4-02—S4-06 реализует валидируемую guest physical map,
production-owned RAM backing, checked physical byte/scalar accesses с
committed-write notification и минимальный синхронный MMIO/PIO bus boundary.
Диагностический flat identity bridge к `od-isa::GuestMemory` также проверяет
interpreter integration; MMU/page walker, UART/APIC/PCI, vCPU, JIT integration
и Linux boot ещё не реализованы. Читать ../AGENTS.md и
../../docs/PROJECT_LAYOUT.md перед дальнейшей реализацией; зависимости
добавлять вместе с фактическим использованием.
