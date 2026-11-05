# od-machine

Системная модель гостевой машины.

Гостевая RAM/MMU/TLB, bus, устройства, vCPU scheduler и виртуальное время. Использует нижние контракты CPU/JIT и storage; host-примитивы получает через узкие интерфейсы.

Статус: локальный срез S4-02—S4-10a реализует валидируемую guest physical map,
production-owned RAM backing, checked physical byte/scalar accesses с
committed-write notification и минимальный синхронный MMIO/PIO bus boundary.
Диагностический flat identity bridge к `od-isa::GuestMemory` также проверяет
interpreter integration, bounded uncached translation probe и tracked RAM-only
Long64 4 KiB A/D access path; CPU-integrated MMU/page faults, UART/APIC/PCI,
vCPU, JIT integration и Linux boot ещё не реализованы. Admission boundary для
неподдержанных 2 MiB/1 GiB pages закреплён тестами, но сами huge pages и новые
режимы ещё не реализованы. Cross-page accesses отвергаются до page walk;
split/transaction semantics и CPU restart/REP/RMW integration ещё не
реализованы. CPU-state snapshot bridge принимает CR0/CR4 из `LongModeState`,
но guest CR3/EFER transitions и startup evidence ещё не реализованы. Читать ../AGENTS.md и
../../docs/PROJECT_LAYOUT.md перед дальнейшей реализацией; зависимости
добавлять вместе с фактическим использованием.
