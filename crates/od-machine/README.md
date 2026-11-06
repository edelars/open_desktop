# od-machine

Системная модель гостевой машины.

Гостевая RAM/MMU/TLB, bus, устройства, vCPU scheduler и виртуальное время. Использует нижние контракты CPU/JIT и storage; host-примитивы получает через узкие интерфейсы.

Статус: локальный срез S4-02—S4-10a реализует валидируемую guest physical map,
production-owned RAM backing, checked physical byte/scalar accesses с
committed-write notification и минимальный синхронный MMIO/PIO bus boundary.
Диагностический flat identity bridge к `od-isa::GuestMemory` также проверяет
interpreter integration, bounded uncached translation probe and tracked Long64
4 KiB A/D access path. The tracked physical RAM accessor remains RAM-only; the
bounded CPU-connected `step_paged_mov` adapter routes its qword operands to RAM
or mapped MMIO through `MachineBus`; its separate S4-11b path permits only the
`MOV RAX,[RBX]` qword read across two guest RAM pages. Cross-page stores and
device splitting remain unsupported. S4-11c adds a separate exact `MOV EAX,[RBX]`
(`8B 03`) dword-read profile through same-page or two-page RAM; it does not
change the qword allowlist. An opt-in one-entry immutable-snapshot cache is
only a pure-probe experiment for S4-13a; it does not implement production TLB
invalidation or cache tracked A/D effects. Full CPU/MMU paging and fault
delivery, UART/APIC/PCI, production vCPU scheduling, JIT integration и Linux
boot ещё не реализованы.
S4-09b adds a private tracked-walker test seam and verifies injected A-write
failure at each page-table level plus D-write failure after the complete A
prefix. Committed `AdUpdate` and callback order, exact `TableWrite` metadata,
unchanged rejected entries and the absent operand/D commit are checked. The
production writer still uses `PhysicalMemory::write_u64`; full S4-09 remains
open for permission-level coverage, CPU integration and system acceptance. See
the [S4-09b report](../../docs/reports/STAGE_4_S4-09B_2026-11-05.md).
Bounded S4-21a adds a synchronous diagnostic vCPU session owning one
`LongModeState` and `MachineBus`, with runnable/paused/stopped/terminal states,
finite interpreter calls, preserved RFLAGS definedness, and distinct fault,
unsupported, and backend exits. It does not implement HLT wakeup, reset, IRQ,
MMU, JIT, VMHost/IPC, or REP continuation.
Bounded S4-22a adds an explicitly advanced virtual clock and fixed-frequency
TSC source for bare Long64 RDTSC. Runs, polling and host delay do not advance
time; overflow is rejected atomically. The legacy session constructor remains
without RDTSC. Linux frequency discovery, host monotonic integration, HLT
progression, events, and RDTSCP/MSR/CPUID support remain outside this slice.
Bounded S4-23a adds a fixed-capacity deterministic one-shot deadline queue.
It uses absolute nanosecond deadlines, explicit extraction and stable ordering;
it does not advance the virtual clock or dispatch CPU/device/IRQ work. Full
event lifecycle, reset/generation, HLT wakeup and host waiting remain open.
Admission boundary для неподдержанных 2 MiB/1 GiB pages закреплён тестами, но сами huge pages и новые
режимы ещё не реализованы. Cross-page accesses отвергаются до page walk;
split/transaction semantics и CPU restart/REP/RMW integration ещё не
реализованы. CPU-state snapshot bridge принимает CR0/CR4 из `LongModeState`,
но guest CR3/EFER transitions и startup evidence ещё не реализованы.
Экспериментальный one-entry pure-translation cache и release benchmark не
являются production TLB и не покрывают tracked A/D, invalidation или Linux.
S4-14a закрепляет host-controlled `clear`/drop-and-recreate lifecycle. S4-14b
добавляет отдельный CPU `INVLPG [RBX]` effect и feature-gated потребитель,
который очищает experimental one-entry cache перед следующим lookup. Это не
меняет контракт отдельного immutable cache. S4-14c добавляет рядом
feature-gated `MutableTranslationProbeSession`, эксклюзивно заимствующую один
`PhysicalMemory` и сохраняющую только values-only pure translation. Checked
aligned PTE writes не очищают cache; только retired canonical INVLPG effect
синхронно очищает его. Fetch обходится без resident entry и не пишет A/D.
Remap и R/W permission refresh проверяются в одной сессии. Это ограниченный
диагностический эксперимент, а не tracked-access cache или production TLB;
CR3/reset transitions, tracked A/D, JIT hooks, huge pages, PCID/global и
DMA/SMP остаются вне реализации.
Читать ../AGENTS.md и
../../docs/PROJECT_LAYOUT.md перед дальнейшей реализацией; зависимости
добавлять вместе с фактическим использованием.
