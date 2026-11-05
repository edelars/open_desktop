# od-machine

Системная модель гостевой машины.

Гостевая RAM/MMU/TLB, bus, устройства, vCPU scheduler и виртуальное время. Использует нижние контракты CPU/JIT и storage; host-примитивы получает через узкие интерфейсы.

Статус: локальный срез S4-02—S4-10a реализует валидируемую guest physical map,
production-owned RAM backing, checked physical byte/scalar accesses с
committed-write notification и минимальный синхронный MMIO/PIO bus boundary.
Диагностический flat identity bridge к `od-isa::GuestMemory` также проверяет
interpreter integration, bounded uncached translation probe и tracked RAM-only
Long64 4 KiB A/D access path. An opt-in one-entry immutable-snapshot cache is
only a pure-probe experiment for S4-13a; it does not implement production TLB
invalidation or cache tracked A/D effects. CPU-integrated MMU/page faults, UART/APIC/PCI,
production vCPU scheduling, JIT integration и Linux boot ещё не реализованы.
Bounded S4-21a adds a synchronous diagnostic vCPU session owning one
`LongModeState` and `MachineBus`, with runnable/paused/stopped/terminal states,
finite interpreter calls, preserved RFLAGS definedness, and distinct fault,
unsupported, and backend exits. It does not implement HLT wakeup, reset, IRQ,
MMU, JIT, VMHost/IPC, or REP continuation. Admission boundary для
неподдержанных 2 MiB/1 GiB pages закреплён тестами, но сами huge pages и новые
режимы ещё не реализованы. Cross-page accesses отвергаются до page walk;
split/transaction semantics и CPU restart/REP/RMW integration ещё не
реализованы. CPU-state snapshot bridge принимает CR0/CR4 из `LongModeState`,
но guest CR3/EFER transitions и startup evidence ещё не реализованы.
Экспериментальный one-entry pure-translation cache и release benchmark не
являются production TLB и не покрывают tracked A/D, invalidation или Linux.
S4-14a отдельно закрепляет только host-controlled `clear`/drop-and-recreate
lifecycle: remap и изменение прав видны после завершения immutable borrow и
создания нового cache; guest INVLPG/CR3/EFER transitions и CPU/JIT hooks остаются
открыты.
Читать ../AGENTS.md и
../../docs/PROJECT_LAYOUT.md перед дальнейшей реализацией; зависимости
добавлять вместе с фактическим использованием.
