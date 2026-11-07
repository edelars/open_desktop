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
delivery, guest-visible UART/APIC/PCI integration, production vCPU scheduling,
JIT integration и Linux boot ещё не реализованы.
S4-09b adds a private tracked-walker test seam and verifies injected A-write
failure at each page-table level plus D-write failure after the complete A
prefix. Committed `AdUpdate` and callback order, exact `TableWrite` metadata,
unchanged rejected entries and the absent operand/D commit are checked. The
production writer still uses `PhysicalMemory::write_u64`; full S4-09 remains
open for CPU integration and system acceptance. The pending-review S4-09c test
slice covers the permission-level A/D matrix for non-present, U/S, R/W/WP and
NX/NXE behavior at all four levels, including CPL1/CPL2 supervisor behavior,
exact fault records and callback ordering. It changes no walker or CPU behavior
and does not add exception delivery/CR2 update. See the
[S4-09b report](../../docs/reports/STAGE_4_S4-09B_2026-11-05.md) and
[S4-09c report](../../docs/reports/STAGE_4_S4-09C_2026-11-06.md).
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
event lifecycle, reset/generation, production HLT wakeup and host waiting remain open.
Bounded S4-27a/b/c/d/e/f1/f2 add one-route caller-driven APIC entry, guest EOI,
same-CPL IRETQ/resume, exact STI and ordinary APIC quanta, explicit HLT wait
with maskable APIC wake, and pending IRQ delivery at the next post-IRET
boundary. S4-27f1 adds a separate bounded RAM-bound JIT diagnostic path with
cooperative event handoff through a real host executor. These remain diagnostic
one-vCPU paths; they do not provide production scheduling, asynchronous event
delivery, general interruptibility, or asynchronous interruption inside native
execution. S4-27f2 covers only synchronous caller-driven APIC/JIT composition.
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
aligned PTE writes сами не очищают cache; matching lookup может вернуть
resident entry до потребления инвалидирующего эффекта. Retired canonical
INVLPG, MOV CR3 или MOV CR0.WP effect синхронно очищает entry. Fetch всех трёх
инструкций обходит resident entry и не пишет A/D.
Remap и R/W permission refresh проверяются в одной сессии. Это ограниченный
диагностический эксперимент, а не tracked-access cache или production TLB;
CPU-owned `MOV CR3,RAX` синхронно меняет snapshot и инвалидирует entry даже
при записи прежнего значения; replacement snapshot используется следующим
lookup. Несогласованный retired effect очищает entry и блокирует дальнейшие
операции с сессией, поэтому stale translation не может быть использован.
S4-14e добавляет отдельный MOV-only CR0.WP handoff: запись invalidates
session cache и обновляет snapshot. S4-14f добавляет `execute_invlpg_wp` для
WP=0 и WP=1, сохраняя прежние `execute_invlpg` и MOV CR3 admission. Fetch
обходит кеш; canonical INVLPG синхронно очищает entry, а noncanonical NOP,
fault и отказ сохраняют его. Остальные reset/EFER transitions, tracked A/D,
JIT hooks, huge pages, PCID/global и DMA/SMP остаются вне реализации.
S4-14g добавляет отдельный `execute_mov_cr3_wp` для обоих WP значений. Он
сверяет CPU CR0/CR3/CR4 со snapshot, обходит кеш при fetch, проверяет retired
`Cr3Write` и синхронно устанавливает новый snapshot/очищает cache, включая
same-value запись. Старый `execute_mov_cr3` по-прежнему доступен только при
WP=1. Ошибки сверки после retirement очищают cache и отравляют session;
не-retiring результаты сохраняют snapshot и cache. Это диагностический
срез без production TLB и performance claim.
Читать ../AGENTS.md и
../../docs/PROJECT_LAYOUT.md перед дальнейшей реализацией; зависимости
добавлять вместе с фактическим использованием.

S4-18a adds `step_paged_lidt`, which executes the fixed CPU LIDT profile
through the tracked Long64 walker. The descriptor is read once as ten bytes
from RAM within one guest page; A-bit writes and their callbacks reflect the
walks actually completed, while the descriptor read sets no D bit. Crossing
sources and physical holes fail before CPU commit; the adapter does not route
to MMIO/PIO, and the loaded IDT base is not walked. This does not add table
gate handling or exception delivery. The bounded slice passed independent review;
see the [machine contract](../../docs/specs/MACHINE_CONTRACT.md#paged-lidt-adapter-s4-18a).

S4-18b adds `step_paged_lgdt`, which reads the exact ten-byte LGDT
pseudo-descriptor through the same tracked, RAM-only Long64 walker. It commits
GDTR and RIP only after a complete read and canonical-base check, reports the
actual fetch and operand A-bit writes, and leaves segment caches unchanged.
An end-to-end system case uses guest-loaded IDT and GDT state for page-fault
delivery and handler entry. This bounded profile does not provide TSS/IST,
descriptor Accessed writes, `#DF`, shutdown, or retry/recovery; see the
[machine contract](../../docs/specs/MACHINE_CONTRACT.md#paged-lgdt-adapter-s4-18b).
